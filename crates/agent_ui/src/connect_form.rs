//! The connection screen: how a model gets connected. Three short groups read
//! from [`connections::catalog`]: Unaligned with its key field open, the
//! subscriptions the user may already have, then own keys, local models and
//! custom endpoints. One row is open at a time, so the screen has one filled
//! button.
//!
//! "Connected" means the server accepted the key: a key is stored, checked
//! with [`LanguageModelProvider::check_connection`], and only kept when the
//! check passes. A key that fails stays in its field with the reason under it.
//!
//! The same form is the chat panel's body while nothing is connected, the body
//! of the "Connections" modal, and (reduced to one row) the field that
//! replaces a refused key above the composer.
//!
//! What each kind of row is driven by:
//! - keys: the provider's `set_api_key` and `check_connection`;
//! - local servers: the address in settings, then the provider's
//!   `authenticate`, which asks the server for its models;
//! - custom endpoints: a model list fetched from the endpoint itself, then
//!   [`language_models::add_compatible_provider`];
//! - agents: [`AgentPanel::start_chat_with_agent`] and the `agent_servers`
//!   setting;
//! - account sign-ins (ChatGPT, GitHub Copilot, SuperGrok): the provider's own
//!   view. Starting a sign-in, cancelling it and the account's e-mail live on
//!   state entities of crates this one does not depend on, so the row adds
//!   who is billed and leaves the button to the provider.

use collections::HashMap;
use editor::{Editor, EditorEvent};
use fs::Fs;
use futures::AsyncReadExt as _;
use gpui::{
    AnyView, App, Context, DismissEvent, Entity, EventEmitter, FocusHandle, Focusable, FontWeight,
    ScrollHandle, SharedString, Subscription, Task, Window,
};
use http_client::{AsyncBody, HttpClient, Method, Request as HttpRequest};
use language_model::{
    ApiKeyConfiguration, CreateProviderSettingsView, IconOrSvg, LanguageModel,
    LanguageModelProvider, LanguageModelProviderId, LanguageModelRegistry,
    ProviderConnectionStatus, ProviderSettingsView,
};
use language_models::{AllLanguageModelSettings, CompatibleProviderModels};
use project::AgentId;
use settings::{
    LanguageModelSelection, Settings as _, SettingsStore, update_settings_file,
    update_settings_file_with_completion,
};
use std::{sync::Arc, time::Duration};
use ui::{ButtonLike, Checkbox, InputShell, InputShellSize, SectionLabel, Tooltip, prelude::*};
use workspace::{DismissDecision, ModalView, Workspace};

use crate::connections::{self, Backing, ConnectionKind, ConnectionSpec, ConnectionState, Group};
use crate::{AgentPanel, ManageProviders, UNALIGNED_PROVIDER_ID};

const UNALIGNED: LanguageModelProviderId = LanguageModelProviderId::new(UNALIGNED_PROVIDER_ID);

/// Where an empty Unaligned balance is topped up.
pub(crate) const BILLING_URL: &str = branding::PROVIDER_BILLING_URL;

/// Under the two sign-ins that present themselves to the vendor as the
/// vendor's own application.
const VENDOR_IDENTITY_NOTE: &str = "Signs in with the vendor's own app identity.";

/// A fetched model list longer than this starts unchecked, and gets a filter.
const CHECK_ALL_UP_TO: usize = 10;

/// The key stored for an endpoint that asks for none: the compatible
/// providers only answer once a key is stored.
const NO_KEY: &str = "none";

pub(crate) enum ConnectFormEvent {
    /// A connection passed its check; requests can be sent.
    Connected,
    /// The user backed out of re-entering a key.
    Dismissed,
    /// A chat with an agent was started from its row: the screen is done.
    AgentChatStarted,
}

/// A server on the user's machine, by the id of its provider.
struct LocalServer {
    default_address: &'static str,
    /// What to type in a terminal to start it.
    start_command: &'static str,
}

fn local_server(id: &LanguageModelProviderId) -> Option<LocalServer> {
    let (default_address, start_command) = match id.0.as_ref() {
        "ollama" => ("http://localhost:11434", "ollama serve"),
        "lmstudio" => ("http://localhost:1234/api/v0", "lms server start"),
        "llama.cpp" => (
            "http://localhost:8080",
            "llama-server -m model.gguf --port 8080",
        ),
        _ => return None,
    };
    Some(LocalServer {
        default_address,
        start_command,
    })
}

/// The address a local server is looked for at: the one in settings, else
/// the tool's default.
fn local_address(id: &LanguageModelProviderId, cx: &App) -> String {
    let from_settings = AllLanguageModelSettings::try_get(cx).map(|settings| match id.0.as_ref() {
        "ollama" => settings.ollama.api_url.clone(),
        "lmstudio" => settings.lmstudio.api_url.clone(),
        "llama.cpp" => settings.llama_cpp.api_url.clone(),
        _ => String::new(),
    });
    match from_settings {
        Some(address) if !address.is_empty() => address,
        _ => local_server(id).map_or_else(String::new, |server| server.default_address.to_string()),
    }
}

/// An address as typed, made into the URL the settings hold.
fn normalize_address(typed: &str) -> String {
    let typed = typed.trim().trim_end_matches('/');
    if typed.starts_with("http://") || typed.starts_with("https://") {
        typed.to_string()
    } else {
        format!("http://{typed}")
    }
}

/// An address as a sentence says it: without the scheme.
fn spoken_address(address: &str) -> &str {
    address
        .trim_start_matches("http://")
        .trim_start_matches("https://")
}

/// What "Detect" found the last time it was pressed in a local row.
#[derive(Clone, Debug, PartialEq, Eq)]
enum Detection {
    /// Nothing answered at this address.
    NoServer { address: SharedString },
    /// A server answered and serves no model.
    NoModels { address: SharedString },
}

/// One model id of a custom endpoint, fetched or typed.
#[derive(Clone, Debug, PartialEq, Eq)]
struct CustomModel {
    id: String,
    checked: bool,
}

/// The ids in an endpoint's answer to "which models do you have". OpenAI's
/// and Anthropic's shape is `{"data": [{"id": …}]}`; some servers answer
/// `{"models": [{"id" or "name": …}]}` or a bare array.
fn model_ids_from(body: &str) -> anyhow::Result<Vec<String>> {
    let value: serde_json::Value = serde_json::from_str(body)
        .map_err(|_| anyhow::anyhow!("the answer is not a model list"))?;
    let entries = value
        .get("data")
        .or_else(|| value.get("models"))
        .unwrap_or(&value)
        .as_array()
        .ok_or_else(|| anyhow::anyhow!("the answer is not a model list"))?;
    let mut ids: Vec<String> = Vec::new();
    for entry in entries {
        let id = entry
            .as_str()
            .or_else(|| entry.get("id").and_then(|id| id.as_str()))
            .or_else(|| entry.get("name").and_then(|name| name.as_str()));
        if let Some(id) = id
            && !id.is_empty()
            && !ids.iter().any(|known| known == id)
        {
            ids.push(id.to_string());
        }
    }
    Ok(ids)
}

/// Where an endpoint lists its models.
fn models_url(anthropic: bool, base_url: &str) -> String {
    let base_url = base_url.trim_end_matches('/');
    if anthropic {
        format!("{}/v1/models", base_url.trim_end_matches("/v1"))
    } else {
        format!("{base_url}/models")
    }
}

/// Whether anything answers HTTP at `address`, whatever it answers.
async fn server_answers(client: Arc<dyn HttpClient>, address: &str) -> bool {
    let Ok(request) = HttpRequest::builder()
        .method(Method::GET)
        .uri(address)
        .body(AsyncBody::default())
    else {
        return false;
    };
    client.send(request).await.is_ok()
}

/// Asks an endpoint for its model ids.
async fn fetch_model_ids(
    client: Arc<dyn HttpClient>,
    anthropic: bool,
    base_url: String,
    api_key: String,
) -> anyhow::Result<Vec<String>> {
    let mut request = HttpRequest::builder()
        .method(Method::GET)
        .uri(models_url(anthropic, &base_url))
        .header("Accept", "application/json");
    if anthropic {
        request = request.header("anthropic-version", "2023-06-01");
        if !api_key.is_empty() {
            request = request.header("x-api-key", api_key);
        }
    } else if !api_key.is_empty() {
        request = request.header("Authorization", format!("Bearer {api_key}"));
    }
    let mut response = client
        .send(request.body(AsyncBody::default())?)
        .await
        .map_err(|_| anyhow::anyhow!("no server answered at this URL"))?;
    let status = response.status();
    if status.as_u16() == 401 || status.as_u16() == 403 {
        anyhow::bail!("the server refused the key");
    }
    if !status.is_success() {
        anyhow::bail!("the server answered {}", status.as_u16());
    }
    let mut body = String::new();
    response.body_mut().read_to_string(&mut body).await?;
    model_ids_from(&body)
}

/// A line of the screen that can be open.
#[derive(Clone, Debug, PartialEq, Eq)]
enum Row {
    Provider(LanguageModelProviderId),
    Agent(AgentId),
    /// "Custom endpoint": a connection that does not exist yet.
    Custom,
}

impl Row {
    fn of(spec: &ConnectionSpec) -> Self {
        if spec.adds_custom_endpoint() {
            return Self::Custom;
        }
        match &spec.backing {
            Backing::Provider(id) => Self::Provider(id.clone()),
            Backing::Agent(id) => Self::Agent(id.clone()),
        }
    }

    fn element_id(&self, prefix: &str) -> ElementId {
        let name = match self {
            Self::Provider(id) => format!("{prefix}-{}", id.0),
            Self::Agent(id) => format!("{prefix}-agent-{}", id.0),
            Self::Custom => format!("{prefix}-custom"),
        };
        ElementId::Name(name.into())
    }
}

/// The fields of the "Custom endpoint" row.
struct CustomEndpoint {
    anthropic: bool,
    name: Entity<Editor>,
    api_url: Entity<Editor>,
    api_key: Entity<Editor>,
    /// One model id typed by hand: the way in when the endpoint does not
    /// list its models.
    model_id: Entity<Editor>,
    /// Narrows a long fetched list.
    filter: Entity<Editor>,
    /// The ids to choose from, fetched or typed.
    models: Vec<CustomModel>,
    /// "Fetch Models" is waiting for the endpoint.
    fetching: bool,
    /// How many ids the last fetch brought.
    fetched: Option<usize>,
}

impl CustomEndpoint {
    fn chosen_ids(&self) -> Vec<String> {
        self.models
            .iter()
            .filter(|model| model.checked)
            .map(|model| model.id.clone())
            .collect()
    }
}

/// What went wrong in a row, said under its field.
#[derive(Clone, Debug, PartialEq, Eq)]
struct Problem {
    row: Row,
    message: SharedString,
    /// A second line that says what to do about it.
    hint: Option<SharedString>,
    /// A check said it, so the row needs attention. Otherwise it is something
    /// to correct before a check can run (an empty field, a full keychain).
    needs_attention: bool,
}

impl Problem {
    /// Something to correct before trying.
    fn before_check(row: Row, message: impl Into<SharedString>) -> Self {
        Self {
            row,
            message: message.into(),
            hint: None,
            needs_attention: false,
        }
    }

    /// What a check answered.
    fn from_check(row: Row, message: impl Into<SharedString>) -> Self {
        Self {
            row,
            message: message.into(),
            hint: None,
            needs_attention: true,
        }
    }
}

/// The line at the top of the list after something changed, kept for the rest
/// of the form's life so the user knows which model answers now.
#[derive(Clone, Debug, PartialEq, Eq)]
enum Notice {
    Connected {
        id: LanguageModelProviderId,
        title: SharedString,
    },
    /// A custom endpoint was written to settings. Nothing was sent to it
    /// with the chosen models yet.
    Saved {
        id: LanguageModelProviderId,
        title: SharedString,
    },
    Disconnected {
        title: SharedString,
        /// The model and the connection that took over the default, when the
        /// removed connection held it.
        new_default: Option<(SharedString, SharedString)>,
    },
    /// An agent's entry was removed from settings.
    AgentRemoved { name: SharedString },
}

/// How a key that was typed fared.
enum KeyOutcome {
    /// The keychain refused to store it.
    NotSaved(anyhow::Error),
    /// It was stored, and could not be read back for the check.
    NotRead(anyhow::Error),
    Answered(ProviderConnectionStatus),
}

pub(crate) struct ConnectForm {
    focus_handle: FocusHandle,
    /// One row is open at a time, so the screen has one primary action.
    expanded: Option<Row>,
    show_more: bool,
    /// The modal has its own title.
    show_title: bool,
    /// Key fields of API-key connections, built when their row first opens.
    key_editors: HashMap<LanguageModelProviderId, Entity<Editor>>,
    /// The providers' own configuration views (sign-ins, cloud credentials).
    provider_views: HashMap<LanguageModelProviderId, AnyView>,
    /// Address fields of local servers, built when their row first opens.
    local_addresses: HashMap<LanguageModelProviderId, Entity<Editor>>,
    /// What "Detect" last answered, per local server, when it found nothing.
    detections: HashMap<LanguageModelProviderId, Detection>,
    custom: CustomEndpoint,
    problem: Option<Problem>,
    /// The row whose check is running. [`connections::state`] cannot know.
    checking: Option<Row>,
    /// The connection whose "Disconnect" was pressed and not yet confirmed.
    confirming_disconnect: Option<LanguageModelProviderId>,
    /// The agent whose "Remove" was pressed and not yet confirmed.
    confirming_removal: Option<AgentId>,
    /// The agent "Set Up" was pressed for, until it is in the settings.
    setting_up: Option<AgentId>,
    /// The account row whose provider changed state since the row was
    /// opened while still signed out: its sign-in was started (it may also
    /// have failed or been cancelled since; the provider does not say).
    sign_in_started: Option<LanguageModelProviderId>,
    notice: Option<Notice>,
    /// The connection whose key is being replaced after a refused request.
    reenter_target: Option<LanguageModelProviderId>,
    /// Set after a connect until a default model could be chosen: the model
    /// list arrives a moment after the key is stored.
    wants_default_model: Option<LanguageModelProviderId>,
    connect_task: Option<Task<()>>,
    fetch_task: Option<Task<()>>,
    _subscriptions: Vec<Subscription>,
}

impl EventEmitter<ConnectFormEvent> for ConnectForm {}

impl ConnectForm {
    pub(crate) fn new(window: &mut Window, cx: &mut Context<Self>) -> Self {
        let custom = CustomEndpoint {
            anthropic: false,
            name: Self::new_editor("Name, e.g. My server", false, window, cx),
            api_url: Self::new_editor("Base URL, e.g. https://host/v1", false, window, cx),
            api_key: Self::new_editor("Key (optional)", true, window, cx),
            model_id: Self::new_editor("Model id", false, window, cx),
            filter: Self::new_editor("Filter models", false, window, cx),
            models: Vec::new(),
            fetching: false,
            fetched: None,
        };

        let mut subscriptions = vec![cx.subscribe(
            &LanguageModelRegistry::global(cx),
            |this, _, event: &language_model::Event, cx| {
                // The open row connected through its own view (a local
                // server, a sign-in): as good as one connected here. Other
                // rows are left alone: credentials loading at startup must
                // not rewrite the user's default model.
                // An account row that moves while it is open and signed out:
                // its sign-in was started.
                if let language_model::Event::ProviderStateChanged(id) = event
                    && this.expanded == Some(Row::Provider(id.clone()))
                    && this.provider_views.contains_key(id)
                    && Self::spec_for_provider(id, cx).kind == ConnectionKind::SignIn
                {
                    this.sign_in_started = (!Self::is_authenticated(id, cx)).then(|| id.clone());
                }
                if let language_model::Event::ProviderStateChanged(id)
                | language_model::Event::AddedProvider(id) = event
                    && this.wants_default_model.is_none()
                    && this.checking.is_none()
                    && this.expanded == Some(Row::Provider(id.clone()))
                    && !this.key_editors.contains_key(id)
                    && Self::is_authenticated(id, cx)
                {
                    this.notice = Some(Notice::Connected {
                        id: id.clone(),
                        title: Self::spec_for_provider(id, cx).title,
                    });
                    this.wants_default_model = Some(id.clone());
                }
                this.select_default_model_if_needed(cx);
                this.reopen_default_row(cx);
                cx.notify();
            },
        )];
        for editor in [
            &custom.name,
            &custom.api_url,
            &custom.api_key,
            &custom.model_id,
        ] {
            subscriptions.push(Self::clear_problem_on_edit(editor, cx));
        }
        // The filter only changes what is drawn.
        subscriptions.push(
            cx.subscribe(&custom.filter, |_, _, event: &EditorEvent, cx| {
                if matches!(event, EditorEvent::BufferEdited) {
                    cx.notify();
                }
            }),
        );
        // Agents are set up and removed in settings, and a local server's
        // address lives there.
        subscriptions.push(cx.observe_global::<SettingsStore>(|this, cx| {
            if let Some(agent) = &this.setting_up
                && Self::agent_is_set_up(agent, cx)
            {
                this.setting_up = None;
            }
            cx.notify();
        }));

        let mut this = Self {
            focus_handle: cx.focus_handle(),
            expanded: None,
            show_more: false,
            show_title: true,
            key_editors: HashMap::default(),
            provider_views: HashMap::default(),
            local_addresses: HashMap::default(),
            detections: HashMap::default(),
            custom,
            problem: None,
            checking: None,
            confirming_disconnect: None,
            confirming_removal: None,
            setting_up: None,
            sign_in_started: None,
            notice: None,
            reenter_target: None,
            wants_default_model: None,
            connect_task: None,
            fetch_task: None,
            _subscriptions: subscriptions,
        };
        this.open_default_row(window, cx);
        this
    }

    /// Unaligned starts open while nothing is connected, and when its own key
    /// needs attention.
    fn open_default_row(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some(spec) = connections::catalog(cx)
            .into_iter()
            .find(|spec| spec.group == Group::Unaligned)
        else {
            return;
        };
        let Backing::Provider(id) = &spec.backing else {
            return;
        };
        let nothing_connected =
            !LanguageModelRegistry::read_global(cx).has_authenticated_provider(cx);
        let needs_attention = matches!(
            connections::state(&spec, cx),
            ConnectionState::NeedsAttention { .. }
        );
        if nothing_connected || needs_attention {
            self.key_editor(id, window, cx);
            self.expanded = Some(Row::Provider(id.clone()));
        }
    }

    /// The last connection went away while the form was closed up on its
    /// result: Unaligned opens again, as on a first run.
    fn reopen_default_row(&mut self, cx: &App) {
        if self.expanded.is_none()
            && self.checking.is_none()
            && self.reenter_target.is_none()
            && self.key_editors.contains_key(&UNALIGNED)
            && Self::provider(&UNALIGNED, cx).is_some()
            && !LanguageModelRegistry::read_global(cx).has_authenticated_provider(cx)
        {
            self.expanded = Some(Row::Provider(UNALIGNED));
        }
    }

    fn new_editor(
        placeholder: &str,
        masked: bool,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Entity<Editor> {
        cx.new(|cx| {
            let mut editor = Editor::single_line(window, cx);
            editor.set_placeholder_text(placeholder, window, cx);
            editor.set_masked(masked, cx);
            editor
        })
    }

    /// A sentence about what was typed is void once the text changes.
    fn clear_problem_on_edit(editor: &Entity<Editor>, cx: &mut Context<Self>) -> Subscription {
        cx.subscribe(editor, |this, _, event: &EditorEvent, cx| {
            if matches!(event, EditorEvent::BufferEdited) && this.problem.take().is_some() {
                cx.notify();
            }
        })
    }

    fn provider(id: &LanguageModelProviderId, cx: &App) -> Option<Arc<dyn LanguageModelProvider>> {
        LanguageModelRegistry::read_global(cx).provider(id)
    }

    fn is_authenticated(id: &LanguageModelProviderId, cx: &App) -> bool {
        Self::provider(id, cx).is_some_and(|provider| provider.is_authenticated(cx))
    }

    /// The catalog's row for a provider. A provider the catalog does not list
    /// (it was just unregistered) still gets its own name.
    fn spec_for_provider(id: &LanguageModelProviderId, cx: &App) -> ConnectionSpec {
        let backing = Backing::Provider(id.clone());
        connections::catalog(cx)
            .into_iter()
            .find(|spec| spec.backing == backing)
            .unwrap_or_else(|| ConnectionSpec {
                title: Self::provider(id, cx)
                    .map_or_else(|| id.0.clone(), |provider| provider.name().0),
                backing,
                kind: ConnectionKind::ApiKey,
                group: Group::More,
                note: None,
                icon: IconName::AppAssistant,
            })
    }

    /// Whether a key is being checked. While this is true the form should
    /// stay on screen: the verdict is about to appear in it.
    #[allow(dead_code, reason = "for the hosts of the form")]
    pub(crate) fn is_checking(&self) -> bool {
        self.checking.is_some()
    }

    /// Whether leaving the form now would lose something: a key check, a
    /// detection or a model list is on its way, or the sign-in of the open
    /// account row was started and has not finished (the browser, or a code
    /// to enter).
    fn is_busy(&self, cx: &App) -> bool {
        if self.checking.is_some() || self.custom.fetching {
            return true;
        }
        match (&self.expanded, &self.sign_in_started) {
            (Some(Row::Provider(open)), Some(started)) => {
                open == started && !Self::is_authenticated(started, cx)
            }
            _ => false,
        }
    }

    fn agent_is_set_up(agent: &AgentId, cx: &App) -> bool {
        connections::catalog(cx)
            .iter()
            .find(|spec| spec.backing == Backing::Agent(agent.clone()))
            .is_some_and(|spec| connections::state(spec, cx).is_connected())
    }

    /// The agent's name as the interface says it: "Claude Code", never the
    /// registry's own name for it.
    fn agent_name(agent: &AgentId) -> SharedString {
        crate::agent_display_name(agent, None)
    }

    /// How a row stands: what is known about the connection, overlaid with
    /// what only this form knows (a check that is running, or one it ran that
    /// failed).
    fn row_state(&self, spec: &ConnectionSpec, cx: &App) -> ConnectionState {
        let row = Row::of(spec);
        if self.checking.as_ref() == Some(&row) {
            return ConnectionState::Checking;
        }
        if let Backing::Agent(agent) = &spec.backing
            && self.setting_up.as_ref() == Some(agent)
            && !connections::state(spec, cx).is_connected()
        {
            return ConnectionState::Checking;
        }
        if let Some(problem) = self
            .problem
            .as_ref()
            .filter(|problem| problem.row == row && problem.needs_attention)
        {
            return ConnectionState::NeedsAttention {
                reason: problem.message.clone(),
            };
        }
        connections::state(spec, cx)
    }

    fn problem_for(&self, row: &Row) -> Option<&Problem> {
        self.problem.as_ref().filter(|problem| problem.row == *row)
    }

    /// The key field of an API-key connection, created on first use.
    fn key_editor(
        &mut self,
        id: &LanguageModelProviderId,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Entity<Editor> {
        if let Some(editor) = self.key_editors.get(id) {
            return editor.clone();
        }
        let placeholder = if *id == UNALIGNED {
            "sk-unal-…"
        } else {
            "Paste your key"
        };
        let editor = Self::new_editor(placeholder, true, window, cx);
        self._subscriptions
            .push(Self::clear_problem_on_edit(&editor, cx));
        self.key_editors.insert(id.clone(), editor.clone());
        editor
    }

    fn provider_view(
        &mut self,
        id: &LanguageModelProviderId,
        create_view: CreateProviderSettingsView,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if !self.provider_views.contains_key(id) {
            let view = create_view(window, cx);
            self.provider_views.insert(id.clone(), view);
        }
    }

    fn toggle(&mut self, row: Row, window: &mut Window, cx: &mut Context<Self>) {
        // A row is not swapped under a check that is about to answer in it.
        if self.checking.is_some() {
            return;
        }
        if self.expanded.as_ref() == Some(&row) {
            self.expanded = None;
            self.problem = None;
            self.confirming_disconnect = None;
            self.confirming_removal = None;
            self.sign_in_started = None;
            self.focus_handle.focus(window, cx);
            cx.notify();
        } else {
            self.expand(row, window, cx);
        }
    }

    /// The address field of a local server, created on first use with the
    /// address from settings.
    fn local_address_editor(
        &mut self,
        id: &LanguageModelProviderId,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Entity<Editor> {
        if let Some(editor) = self.local_addresses.get(id) {
            return editor.clone();
        }
        let address = local_address(id, cx);
        let editor = Self::new_editor("Server address", false, window, cx);
        editor.update(cx, |editor, cx| {
            editor.set_text(spoken_address(&address).to_string(), window, cx);
        });
        // A sentence about an address is void once the address changes.
        self._subscriptions.push(cx.subscribe(&editor, {
            let id = id.clone();
            move |this, _, event: &EditorEvent, cx| {
                if matches!(event, EditorEvent::BufferEdited) {
                    let cleared = this.detections.remove(&id).is_some();
                    if this.problem.take().is_some() || cleared {
                        cx.notify();
                    }
                }
            }
        }));
        self.local_addresses.insert(id.clone(), editor.clone());
        editor
    }

    /// Opens a row, builds what it shows and puts the cursor in it.
    fn expand(&mut self, row: Row, window: &mut Window, cx: &mut Context<Self>) {
        self.problem = None;
        self.confirming_disconnect = None;
        self.confirming_removal = None;
        self.sign_in_started = None;
        match &row {
            // A local server: its address and "Detect". Nothing is looked
            // for until the user asks.
            Row::Provider(id) if local_server(id).is_some() => {
                if Self::provider(id, cx).is_none() {
                    return;
                }
                let editor = self.local_address_editor(id, window, cx);
                editor.focus_handle(cx).focus(window, cx);
            }
            Row::Provider(id) => {
                let Some(provider) = Self::provider(id, cx) else {
                    return;
                };
                match provider.settings_view(cx) {
                    Some(ProviderSettingsView::ApiKey(configuration)) => {
                        // A key from the environment has no field to fill.
                        if !configuration.is_from_env_var {
                            let editor = self.key_editor(id, window, cx);
                            editor.focus_handle(cx).focus(window, cx);
                        }
                    }
                    Some(ProviderSettingsView::Inline(settings)) => {
                        self.provider_view(id, settings.create_view, window, cx);
                    }
                    Some(ProviderSettingsView::SubPage(settings)) => {
                        self.provider_view(id, settings.create_view, window, cx);
                    }
                    None => {}
                }
            }
            Row::Agent(_) => {}
            Row::Custom => {
                self.custom.name.focus_handle(cx).focus(window, cx);
            }
        }
        self.expanded = Some(row);
        cx.notify();
    }

    /// Reduces the form to one connection whose key is being replaced.
    /// Without one the whole screen is shown.
    pub(crate) fn begin_reenter(
        &mut self,
        provider_id: Option<LanguageModelProviderId>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.reenter_target = provider_id.clone();
        if let Some(id) = provider_id {
            self.expand(Row::Provider(id), window, cx);
        }
    }

    fn finish_connect(&mut self, id: LanguageModelProviderId, cx: &mut Context<Self>) {
        self.reenter_target = None;
        self.notice = Some(Notice::Connected {
            title: Self::spec_for_provider(&id, cx).title,
            id: id.clone(),
        });
        self.wants_default_model = Some(id);
        self.select_default_model_if_needed(cx);
        cx.emit(ConnectFormEvent::Connected);
    }

    /// Stores the key, asks the server about it, and only then calls the row
    /// connected.
    fn connect_key(
        &mut self,
        id: LanguageModelProviderId,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if self.checking.is_some() {
            return;
        }
        let row = Row::Provider(id.clone());
        let Some(editor) = self.key_editors.get(&id).cloned() else {
            return;
        };
        self.confirming_disconnect = None;
        let key = editor.read(cx).text(cx).trim().to_string();
        if key.is_empty() {
            self.problem = Some(Problem::before_check(row, "Paste your key first."));
            cx.notify();
            return;
        }
        let Some(provider) = Self::provider(&id, cx) else {
            self.problem = Some(Problem::before_check(
                row,
                "This connection is not available any more.",
            ));
            cx.notify();
            return;
        };
        let environment_variable = match provider.settings_view(cx) {
            Some(ProviderSettingsView::ApiKey(configuration)) => Some(configuration.env_var_name),
            _ => None,
        };

        self.checking = Some(row);
        self.problem = None;
        editor.update(cx, |editor, _| editor.set_read_only(true));
        cx.notify();

        let store_key = provider.set_api_key(Some(key), cx);
        // Not tied to the form: the modal may be closed with Done or Escape
        // while the server is asked, and the verdict still has to be acted
        // on (a refused key is not kept).
        cx.spawn_in(window, async move |this, cx| {
            let outcome = match store_key.await {
                Err(error) => KeyOutcome::NotSaved(error),
                Ok(()) => {
                    // The provider already asked its server when the key was
                    // stored: this waits for that same answer.
                    let Ok(check) = cx.update(|_, cx| provider.check_connection(cx)) else {
                        return;
                    };
                    match check.await {
                        Ok(status) => KeyOutcome::Answered(status),
                        Err(error) => KeyOutcome::NotRead(error),
                    }
                }
            };
            let accepted = matches!(
                outcome,
                KeyOutcome::Answered(ProviderConnectionStatus::Connected { .. })
            );
            if matches!(
                outcome,
                KeyOutcome::Answered(ProviderConnectionStatus::NeedsAttention { .. })
            ) {
                // A key that did not pass is not kept: nothing would work
                // with it, and the chat would look connected. It stays in
                // the field (while there is one) to be corrected.
                cx.update(|_, cx| provider.set_api_key(None, cx).detach_and_log_err(cx))
                    .ok();
            }
            let form_is_gone = this
                .update_in(cx, {
                    let id = id.clone();
                    |this, window, cx| {
                        this.finish_key_check(id, outcome, environment_variable, window, cx);
                    }
                })
                .is_err();
            if form_is_gone && accepted {
                // Nobody is left to wait for the model list: one attempt.
                cx.update(|_, cx| Self::settle_default_model(&id, cx)).ok();
            }
        })
        .detach();
    }

    fn finish_key_check(
        &mut self,
        id: LanguageModelProviderId,
        outcome: KeyOutcome,
        environment_variable: Option<SharedString>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let row = Row::Provider(id.clone());
        let editor = self.key_editors.get(&id).cloned();
        self.checking = None;
        if let Some(editor) = &editor {
            editor.update(cx, |editor, _| editor.set_read_only(false));
        }

        let problem = match outcome {
            KeyOutcome::Answered(ProviderConnectionStatus::Connected { .. }) => {
                if let Some(editor) = &editor {
                    editor.update(cx, |editor, cx| editor.clear(window, cx));
                }
                self.problem = None;
                // The row closes on its result: "Connected" and what is
                // left, with no field asking for another key.
                if self.expanded.as_ref() == Some(&row) && self.reenter_target.is_none() {
                    self.expanded = None;
                    self.focus_handle.focus(window, cx);
                }
                self.finish_connect(id, cx);
                cx.notify();
                return;
            }
            // The key was already taken out of the keychain by the check; it
            // stays in the field to be corrected or tried again.
            KeyOutcome::Answered(ProviderConnectionStatus::NeedsAttention { reason }) => {
                Problem::from_check(row, reason)
            }
            KeyOutcome::Answered(ProviderConnectionStatus::NotConnected) => {
                Problem::before_check(row, "The key was not kept. Try again.")
            }
            KeyOutcome::NotSaved(error) => Problem {
                row,
                message: format!("Could not save the key: {}.", sentence_body(&error)).into(),
                hint: keyring_hint(DESKTOP_KEYRING, environment_variable.as_ref()),
                needs_attention: false,
            },
            KeyOutcome::NotRead(error) => Problem {
                row,
                message: format!("Could not read the key back: {}.", sentence_body(&error)).into(),
                hint: keyring_hint(DESKTOP_KEYRING, environment_variable.as_ref()),
                needs_attention: false,
            },
        };
        self.problem = Some(problem);
        if let Some(editor) = &editor {
            editor.focus_handle(cx).focus(window, cx);
        }
        cx.notify();
    }

    /// "Disconnect" was pressed: ask once, in the row.
    fn request_disconnect(&mut self, id: LanguageModelProviderId, cx: &mut Context<Self>) {
        self.problem = None;
        self.confirming_disconnect = Some(id);
        cx.notify();
    }

    fn cancel_disconnect(&mut self, cx: &mut Context<Self>) {
        if self.confirming_disconnect.take().is_some() {
            cx.notify();
        }
    }

    /// Removes the key. When that connection held the default model, the
    /// default moves to the next connection that has one, and the top line
    /// says so.
    fn disconnect(&mut self, id: LanguageModelProviderId, cx: &mut Context<Self>) {
        self.confirming_disconnect = None;
        let Some(provider) = Self::provider(&id, cx) else {
            cx.notify();
            return;
        };
        let title = Self::spec_for_provider(&id, cx).title;
        let held_default = LanguageModelRegistry::read_global(cx)
            .default_model()
            .is_some_and(|model| model.provider_id == id);
        let new_default = held_default
            .then(|| Self::next_default_model(&id, cx))
            .flatten();

        provider.set_api_key(None, cx).detach_and_log_err(cx);
        if self.wants_default_model.as_ref() == Some(&id) {
            self.wants_default_model = None;
        }
        self.notice = Some(Notice::Disconnected {
            title,
            new_default: new_default.as_ref().map(|model| {
                (
                    model.name.0.clone(),
                    Self::spec_for_provider(&model.provider_id, cx).title,
                )
            }),
        });
        if let Some(model) = new_default {
            Self::write_default_model(&model, cx);
        }
        cx.notify();
    }

    /// The model that answers once `removed` is gone: the default model of
    /// the first other connection, in the order of the screen.
    fn next_default_model(removed: &LanguageModelProviderId, cx: &App) -> Option<LanguageModel> {
        connections::catalog(cx).iter().find_map(|spec| {
            let Backing::Provider(id) = &spec.backing else {
                return None;
            };
            if id == removed || !connections::state(spec, cx).is_connected() {
                return None;
            }
            let provider = Self::provider(id, cx)?;
            provider
                .default_model(cx)
                .or_else(|| provider.provided_models(cx).into_iter().next())
        })
    }

    /// Asks a local server whether it is there, and loads its models. An
    /// address that differs from the one in settings is written there first:
    /// the provider only ever talks to the address in settings.
    fn detect(&mut self, id: LanguageModelProviderId, window: &mut Window, cx: &mut Context<Self>) {
        if self.checking.is_some() {
            return;
        }
        let Some(provider) = Self::provider(&id, cx) else {
            return;
        };
        let row = Row::Provider(id.clone());
        let current = local_address(&id, cx);
        let typed = self
            .local_addresses
            .get(&id)
            .map(|editor| editor.read(cx).text(cx).trim().to_string())
            .unwrap_or_default();
        let address = if typed.is_empty() {
            current.clone()
        } else {
            normalize_address(&typed)
        };

        self.checking = Some(row.clone());
        self.problem = None;
        self.detections.remove(&id);
        cx.notify();

        let write_address = (address != current).then(|| {
            let id = id.clone();
            let address = address.clone();
            update_settings_file_with_completion(<dyn Fs>::global(cx), cx, move |settings, _| {
                let models = settings.language_models.get_or_insert_default();
                match id.0.as_ref() {
                    "ollama" => models.ollama.get_or_insert_default().api_url = Some(address),
                    "lmstudio" => models.lmstudio.get_or_insert_default().api_url = Some(address),
                    "llama.cpp" => models.llama_cpp.get_or_insert_default().api_url = Some(address),
                    _ => {}
                }
            })
        });
        let client = cx.http_client();
        self.connect_task = Some(cx.spawn_in(window, async move |this, cx| {
            if let Some(write_address) = write_address {
                let written = matches!(write_address.await, Ok(Ok(())));
                // The file is read back by a watcher: wait until the new
                // address is the one in use (two seconds at most).
                let mut applied = false;
                for _ in 0..40 {
                    if !written {
                        break;
                    }
                    applied = cx
                        .update(|_, cx| local_address(&id, cx) == address)
                        .unwrap_or(false);
                    if applied {
                        break;
                    }
                    cx.background_executor()
                        .timer(Duration::from_millis(50))
                        .await;
                }
                if !applied {
                    this.update(cx, |this, cx| {
                        this.checking = None;
                        this.problem = Some(Problem::before_check(
                            row,
                            "Could not save this address to your settings.",
                        ));
                        cx.notify();
                    })
                    .ok();
                    return;
                }
            }
            let Ok(authenticate) = cx.update(|_, cx| provider.authenticate(cx)) else {
                return;
            };
            let result = authenticate.await;
            let found = cx
                .update(|_, cx| provider.is_authenticated(cx))
                .unwrap_or(false);
            // A provider may answer "fine" with no server there (Ollama
            // does): the address itself says whether anything listens.
            let answered = found || (result.is_ok() && server_answers(client, &address).await);
            this.update(cx, |this, cx| {
                this.checking = None;
                if provider.is_authenticated(cx) {
                    this.finish_connect(id, cx);
                } else {
                    let address = SharedString::from(spoken_address(&address).to_string());
                    let detection = if answered {
                        Detection::NoModels { address }
                    } else {
                        Detection::NoServer { address }
                    };
                    this.detections.insert(id, detection);
                }
                cx.notify();
            })
            .ok();
        }));
    }

    /// The panel that hosts chats in this window, when it has a project to
    /// work in.
    fn agent_panel(window: &Window, cx: &App) -> Result<Entity<AgentPanel>, &'static str> {
        let workspace =
            Workspace::for_window(window, cx).ok_or("The chat is not available in this window.")?;
        let workspace = workspace.read(cx);
        if workspace
            .project()
            .read(cx)
            .visible_worktrees(cx)
            .next()
            .is_none()
        {
            return Err("Open a folder first: an agent works in a project.");
        }
        workspace
            .panel::<AgentPanel>(cx)
            .ok_or("The chat is not available in this window.")
    }

    /// "Set Up" and "New Chat" of an agent row: the chat panel adds the agent
    /// to the settings when it is missing, then starts a chat with it. The
    /// agent's download and its sign-in happen in that chat.
    fn start_agent_chat(&mut self, agent: AgentId, window: &mut Window, cx: &mut Context<Self>) {
        let row = Row::Agent(agent.clone());
        self.confirming_removal = None;
        let panel = match Self::agent_panel(window, cx) {
            Ok(panel) => panel,
            Err(reason) => {
                self.problem = Some(Problem::before_check(row, reason));
                cx.notify();
                return;
            }
        };
        self.problem = None;
        if !Self::agent_is_set_up(&agent, cx) {
            self.setting_up = Some(agent.clone());
        }
        // Outside this click: the form may be drawn by the panel itself.
        window.defer(cx, move |window, cx| {
            if let Some(workspace) = Workspace::for_window(window, cx) {
                workspace.update(cx, |workspace, cx| {
                    workspace.focus_panel::<AgentPanel>(window, cx);
                });
            }
            panel.update(cx, |panel, cx| {
                panel.start_chat_with_agent(agent, true, window, cx);
            });
        });
        cx.emit(ConnectFormEvent::AgentChatStarted);
        cx.notify();
    }

    fn request_agent_removal(&mut self, agent: AgentId, cx: &mut Context<Self>) {
        self.problem = None;
        self.confirming_removal = Some(agent);
        cx.notify();
    }

    fn cancel_agent_removal(&mut self, cx: &mut Context<Self>) {
        if self.confirming_removal.take().is_some() {
            cx.notify();
        }
    }

    /// Takes the agent out of the `agent_servers` setting. What it
    /// downloaded and its own sign-in are the agent's, and stay.
    fn remove_agent(&mut self, agent: AgentId, cx: &mut Context<Self>) {
        self.confirming_removal = None;
        if self.setting_up.as_ref() == Some(&agent) {
            self.setting_up = None;
        }
        self.notice = Some(Notice::AgentRemoved {
            name: Self::agent_name(&agent),
        });
        update_settings_file(<dyn Fs>::global(cx), cx, move |settings, _| {
            if let Some(agents) = settings.agent_servers.as_mut() {
                agents.remove(agent.0.as_ref());
            }
        });
        cx.notify();
    }

    /// The Base URL and key of the custom endpoint, or what is wrong with
    /// the URL.
    fn custom_endpoint_address(&self, cx: &App) -> Result<(String, String), &'static str> {
        let read = |editor: &Entity<Editor>| editor.read(cx).text(cx).trim().to_string();
        let api_url = read(&self.custom.api_url);
        if !(api_url.starts_with("http://") || api_url.starts_with("https://")) {
            return Err("Enter the base URL, starting with http:// or https://.");
        }
        Ok((api_url, read(&self.custom.api_key)))
    }

    /// "Fetch Models": asks the endpoint which models it has, with the key
    /// when one is typed. Nothing is stored.
    fn fetch_custom_models(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.custom.fetching || self.checking.is_some() {
            return;
        }
        let (api_url, api_key) = match self.custom_endpoint_address(cx) {
            Ok(address) => address,
            Err(reason) => {
                self.problem = Some(Problem::before_check(Row::Custom, reason));
                cx.notify();
                return;
            }
        };
        self.custom.fetching = true;
        self.problem = None;
        cx.notify();

        let fetch = fetch_model_ids(cx.http_client(), self.custom.anthropic, api_url, api_key);
        self.fetch_task = Some(cx.spawn_in(window, async move |this, cx| {
            let result = fetch.await;
            this.update(cx, |this, cx| {
                this.custom.fetching = false;
                match result {
                    Ok(ids) if ids.is_empty() => {
                        this.custom.fetched = None;
                        this.problem = Some(Problem::before_check(
                            Row::Custom,
                            "The endpoint lists no model. Add a model id by hand.",
                        ));
                    }
                    Ok(ids) => this.set_fetched_models(ids),
                    Err(error) => {
                        this.custom.fetched = None;
                        this.problem = Some(Problem::before_check(
                            Row::Custom,
                            format!(
                                "Could not fetch the model list: {}. You can add a model id \
                                by hand.",
                                sentence_body(&error)
                            ),
                        ));
                    }
                }
                cx.notify();
            })
            .ok();
        }));
    }

    /// A short list starts all checked; a long one starts empty, to be
    /// chosen from. Ids typed by hand stay, and stay first.
    fn set_fetched_models(&mut self, ids: Vec<String>) {
        let check_all = ids.len() <= CHECK_ALL_UP_TO;
        let previous = std::mem::take(&mut self.custom.models);
        // What was checked stays checked across a second fetch.
        let was_checked = |id: &String| {
            previous
                .iter()
                .any(|model| model.checked && model.id == *id)
        };
        let mut models: Vec<CustomModel> = previous
            .iter()
            .filter(|model| model.checked && !ids.contains(&model.id))
            .cloned()
            .collect();
        models.extend(ids.iter().map(|id| CustomModel {
            id: id.clone(),
            checked: check_all || was_checked(id),
        }));
        self.custom.fetched = Some(ids.len());
        self.custom.models = models;
    }

    /// "Add Model Id": the way in when the endpoint does not list models.
    fn add_custom_model_id(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let typed = self.custom.model_id.read(cx).text(cx);
        let mut added = false;
        for id in typed
            .split(|c: char| c == ',' || c.is_whitespace())
            .filter(|id| !id.is_empty())
        {
            added = true;
            match self.custom.models.iter_mut().find(|model| model.id == id) {
                Some(model) => model.checked = true,
                None => self.custom.models.insert(
                    0,
                    CustomModel {
                        id: id.to_string(),
                        checked: true,
                    },
                ),
            }
        }
        if added {
            self.problem = None;
            self.custom
                .model_id
                .update(cx, |editor, cx| editor.clear(window, cx));
        } else {
            self.problem = Some(Problem::before_check(Row::Custom, "Type a model id first."));
        }
        cx.notify();
    }

    fn toggle_custom_model(&mut self, id: &str, cx: &mut Context<Self>) {
        if let Some(model) = self.custom.models.iter_mut().find(|model| model.id == id) {
            model.checked = !model.checked;
            if self
                .problem
                .as_ref()
                .is_some_and(|problem| problem.row == Row::Custom)
            {
                self.problem = None;
            }
            cx.notify();
        }
    }

    /// Writes the endpoint to settings with the chosen models and stores its
    /// key. No request is sent with the models: the first message is the
    /// test, and the top line says so.
    fn connect_custom(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.checking.is_some() || self.custom.fetching {
            return;
        }
        let name = self.custom.name.read(cx).text(cx).trim().to_string();
        let model_ids = self.custom.chosen_ids();
        let address = self.custom_endpoint_address(cx);

        let problem = if name.is_empty() {
            Some("Give this endpoint a name.")
        } else if language_models::compatible_provider_name_error(&name, cx).is_some() {
            Some("This name is already used by another connection.")
        } else if let Err(reason) = &address {
            Some(*reason)
        } else if model_ids.is_empty() && self.custom.models.is_empty() {
            Some("Fetch the models, or add a model id by hand.")
        } else if model_ids.is_empty() {
            Some("Check at least one model.")
        } else {
            None
        };
        let (api_url, api_key) = match (problem, address) {
            (None, Ok(address)) => address,
            (problem, _) => {
                self.problem = Some(Problem::before_check(
                    Row::Custom,
                    problem.unwrap_or("Enter the base URL."),
                ));
                cx.notify();
                return;
            }
        };
        let api_key = if api_key.is_empty() {
            NO_KEY.to_string()
        } else {
            api_key
        };

        self.checking = Some(Row::Custom);
        self.problem = None;
        cx.notify();

        // The same path as Settings → AI → Add custom endpoint.
        let add_provider = language_models::add_compatible_provider(
            name.clone(),
            api_url,
            api_key,
            CompatibleProviderModels::from_model_ids(self.custom.anthropic, model_ids),
            <dyn Fs>::global(cx),
            cx,
        );
        self.connect_task = Some(cx.spawn_in(window, async move |this, cx| {
            let result = add_provider.await;
            this.update_in(cx, |this, window, cx| {
                this.checking = None;
                match result {
                    Ok(id) => {
                        for editor in [
                            &this.custom.name,
                            &this.custom.api_url,
                            &this.custom.api_key,
                            &this.custom.model_id,
                            &this.custom.filter,
                        ] {
                            editor.update(cx, |editor, cx| editor.clear(window, cx));
                        }
                        this.custom.models.clear();
                        this.custom.fetched = None;
                        if let Some(provider) = Self::provider(&id, cx) {
                            provider.authenticate(cx).detach();
                        }
                        this.expanded = None;
                        this.problem = None;
                        this.focus_handle.focus(window, cx);
                        this.finish_connect(id.clone(), cx);
                        this.notice = Some(Notice::Saved {
                            id,
                            title: name.into(),
                        });
                    }
                    Err(error) => {
                        this.problem = Some(Problem::before_check(
                            Row::Custom,
                            format!("Could not add the endpoint: {}.", sentence_body(&error)),
                        ));
                    }
                }
                cx.notify();
            })
            .ok();
        }));
    }

    fn set_custom_api(&mut self, anthropic: bool, window: &mut Window, cx: &mut Context<Self>) {
        self.custom.anthropic = anthropic;
        let placeholder = if anthropic {
            "Base URL, e.g. https://host"
        } else {
            "Base URL, e.g. https://host/v1"
        };
        self.custom.api_url.update(cx, |editor, cx| {
            editor.set_placeholder_text(placeholder, window, cx);
        });
        // A list fetched in the other dialect says nothing about this one.
        self.custom.models.retain(|model| model.checked);
        self.custom.fetched = None;
        self.problem = None;
        cx.notify();
    }

    /// Enter in a field connects the row it belongs to. Enter anywhere else
    /// (a row, a button) is that control's own business.
    fn confirm(&mut self, _: &menu::Confirm, window: &mut Window, cx: &mut Context<Self>) {
        let in_field = |editor: &Entity<Editor>, window: &Window, cx: &App| {
            editor.focus_handle(cx).contains_focused(window, cx)
        };
        match self.expanded.clone() {
            Some(Row::Custom) if in_field(&self.custom.model_id, window, cx) => {
                self.add_custom_model_id(window, cx)
            }
            // Enter in the address or the key asks for the list first: the
            // models are what is still missing.
            Some(Row::Custom)
                if self.custom.models.is_empty()
                    && [&self.custom.api_url, &self.custom.api_key]
                        .into_iter()
                        .any(|editor| in_field(editor, window, cx)) =>
            {
                self.fetch_custom_models(window, cx)
            }
            Some(Row::Custom)
                if [
                    &self.custom.name,
                    &self.custom.api_url,
                    &self.custom.api_key,
                ]
                .into_iter()
                .any(|editor| in_field(editor, window, cx)) =>
            {
                self.connect_custom(window, cx)
            }
            Some(Row::Provider(id))
                if self
                    .local_addresses
                    .get(&id)
                    .is_some_and(|editor| in_field(editor, window, cx)) =>
            {
                self.detect(id, window, cx)
            }
            Some(Row::Provider(id))
                if self
                    .key_editors
                    .get(&id)
                    .is_some_and(|editor| in_field(editor, window, cx)) =>
            {
                self.connect_key(id, window, cx)
            }
            // A provider's own view handles its own keys.
            _ => cx.propagate(),
        }
    }

    /// Escape answers "Cancel" to the question that is open, if one is.
    fn cancel(&mut self, _: &menu::Cancel, _: &mut Window, cx: &mut Context<Self>) {
        if self.confirming_disconnect.is_some() {
            self.cancel_disconnect(cx);
        } else if self.confirming_removal.is_some() {
            self.cancel_agent_removal(cx);
        } else {
            cx.propagate();
        }
    }

    /// Points the default model at the connection that was just made when the
    /// configured one cannot be used, so the first message goes somewhere.
    fn select_default_model_if_needed(&mut self, cx: &mut Context<Self>) {
        let Some(provider_id) = self.wants_default_model.clone() else {
            return;
        };
        if Self::settle_default_model(&provider_id, cx) {
            self.wants_default_model = None;
        }
    }

    /// One attempt at the above. Answers whether nothing is left to do:
    /// `false` means the connection has no model yet (its list is still
    /// loading, a registry event follows).
    fn settle_default_model(provider_id: &LanguageModelProviderId, cx: &App) -> bool {
        let registry = LanguageModelRegistry::read_global(cx);
        let default_is_usable = registry.default_model().is_some_and(|model| {
            registry
                .provider(&model.provider_id)
                .is_some_and(|provider| provider.is_authenticated(cx))
        });
        if default_is_usable {
            return true;
        }
        let Some(provider) = registry.provider(provider_id) else {
            return true;
        };
        let model: Option<LanguageModel> = provider
            .default_model(cx)
            .or_else(|| provider.provided_models(cx).into_iter().next());
        let Some(model) = model else {
            return false;
        };
        Self::write_default_model(&model, cx);
        true
    }

    fn write_default_model(model: &LanguageModel, cx: &App) {
        let provider_id = model.provider_id.0.to_string();
        let model_id = model.id.0.to_string();
        // Thinking stays on for a model that has it: turning it off is the
        // user's choice, made in the composer.
        let enable_thinking = model.supports_thinking();
        update_settings_file(<dyn Fs>::global(cx), cx, move |settings, _| {
            settings
                .agent
                .get_or_insert_default()
                .set_model(LanguageModelSelection {
                    provider: provider_id.into(),
                    model: model_id,
                    enable_thinking,
                    effort: None,
                    speed: None,
                });
        });
    }

    /// The top line: what was just connected or removed, and which model
    /// answers now.
    fn notice_text(&self, cx: &App) -> Option<SharedString> {
        let text = match self.notice.as_ref()? {
            Notice::Connected { id, title } => {
                // Disconnected since (from another copy of this screen): the
                // line would no longer be true.
                if !Self::is_authenticated(id, cx) {
                    return None;
                }
                let registry = LanguageModelRegistry::read_global(cx);
                let answering = registry
                    .default_model()
                    .filter(|model| Self::is_authenticated(&model.provider_id, cx));
                match answering {
                    Some(model) if model.provider_id == *id => {
                        format!("Connected to {title}. Answers come from {}.", model.name.0)
                    }
                    Some(model) => format!(
                        "Connected to {title}. Answers come from {} ({}).",
                        model.name.0,
                        Self::spec_for_provider(&model.provider_id, cx).title
                    ),
                    None => format!("Connected to {title}."),
                }
            }
            Notice::Saved { id, title } => {
                // Removed since: the line would no longer be true.
                Self::provider(id, cx)?;
                format!("Saved {title}. The first message will test it.")
            }
            Notice::AgentRemoved { name } => {
                format!("{name} removed. Its own files and sign-in were left alone.")
            }
            Notice::Disconnected {
                title,
                new_default: Some((model, connection)),
            } => format!("{title} disconnected. Default model is now {model} ({connection})."),
            Notice::Disconnected {
                title,
                new_default: None,
            } => format!("{title} disconnected."),
        };
        Some(text.into())
    }

    /// The word a row offers while it is not connected.
    fn action_word(spec: &ConnectionSpec) -> &'static str {
        match spec.kind {
            ConnectionKind::ApiKey => "Connect",
            ConnectionKind::SignIn => "Sign In",
            ConnectionKind::Local => "Detect",
            ConnectionKind::CustomEndpoint if spec.adds_custom_endpoint() => "Add",
            ConnectionKind::CustomEndpoint => "Connect",
            ConnectionKind::Agent => "Set Up",
        }
    }

    /// The fact a connected provider reports about itself ("$20.50 left"),
    /// also when [`connections::state`] could not carry it.
    fn connected_detail(
        spec: &ConnectionSpec,
        state: &ConnectionState,
        cx: &App,
    ) -> Option<SharedString> {
        match state {
            ConnectionState::Connected { detail } => detail.clone(),
            ConnectionState::FromEnvironment { .. } => {
                let Backing::Provider(id) = &spec.backing else {
                    return None;
                };
                match Self::provider(id, cx)?.connection_status(cx) {
                    ProviderConnectionStatus::Connected { detail } => detail,
                    _ => None,
                }
            }
            _ => None,
        }
    }

    /// The right-hand side of a row, as words: also what a screen reader
    /// says after the title.
    fn state_words(spec: &ConnectionSpec, state: &ConnectionState, cx: &App) -> SharedString {
        match state {
            ConnectionState::NotConnected => Self::action_word(spec).into(),
            ConnectionState::Checking => match spec.kind {
                ConnectionKind::Agent => "Setting up…".into(),
                ConnectionKind::Local => "Detecting…".into(),
                ConnectionKind::CustomEndpoint => "Saving…".into(),
                ConnectionKind::ApiKey | ConnectionKind::SignIn => "Checking…".into(),
            },
            ConnectionState::NeedsAttention { .. } => "Needs attention".into(),
            ConnectionState::Connected { .. } | ConnectionState::FromEnvironment { .. } => {
                let detail = Self::connected_detail(spec, state, cx);
                match (&spec.backing, detail) {
                    // An agent's only fact is that it is set up.
                    (Backing::Agent(_), Some(detail)) => detail,
                    (_, Some(detail)) => format!("Connected · {detail}").into(),
                    (_, None) => "Connected".into(),
                }
            }
        }
    }

    /// A small square in a status colour: square in square themes, a dot in
    /// round ones.
    fn status_mark(color: Color, cx: &App) -> impl IntoElement + use<> {
        div()
            .flex_none()
            .size(px(6.))
            .rounded_pill()
            .bg(color.color(cx))
    }

    fn secondary(text: impl Into<SharedString>) -> Label {
        Label::new(text).size(LabelSize::Small).color(Color::Muted)
    }

    fn render_field(
        id: ElementId,
        label: SharedString,
        editor: &Entity<Editor>,
        invalid: bool,
        disabled: bool,
        window: &Window,
        cx: &App,
    ) -> impl IntoElement + use<> {
        let focus_handle = editor.focus_handle(cx);
        let focused = focus_handle.contains_focused(window, cx);
        div()
            .id(id)
            .aria_label(label)
            .track_focus(&focus_handle.tab_index(0).tab_stop(true))
            .w_full()
            .min_w_0()
            .child(
                InputShell::new()
                    .size(InputShellSize::Large)
                    .focused(focused)
                    .invalid(invalid)
                    .disabled(disabled)
                    .w_full()
                    .child(editor.clone()),
            )
    }

    fn render_link(
        id: impl Into<ElementId>,
        label: impl Into<SharedString>,
        color: Color,
    ) -> Button {
        Button::new(id, label)
            .style(ButtonStyle::Transparent)
            .label_size(LabelSize::Small)
            .color(color)
            .tab_index(0isize)
    }

    fn render_external_link(
        id: impl Into<ElementId>,
        label: impl Into<SharedString>,
        url: impl Into<SharedString>,
    ) -> Button {
        let url = url.into();
        Self::render_link(id, label, Color::Default)
            .end_icon(
                Icon::new(IconName::ArrowUpRight)
                    .size(IconSize::XSmall)
                    .color(Color::Muted),
            )
            .on_click(move |_, _, cx| cx.open_url(&url))
    }

    /// The sentence under a field, and the line that says what to do.
    fn render_problem(
        message: Option<SharedString>,
        hint: Option<SharedString>,
    ) -> Option<impl IntoElement + use<>> {
        let message = message?;
        Some(
            v_flex()
                .gap_1()
                .child(
                    Label::new(message)
                        .size(LabelSize::Small)
                        .color(Color::Error),
                )
                .children(hint.map(Self::secondary)),
        )
    }

    /// Key field, Connect, and where keys are made. A connected row offers to
    /// replace the key or to disconnect instead.
    fn render_key_body(
        &self,
        spec: &ConnectionSpec,
        provider: &Arc<dyn LanguageModelProvider>,
        configuration: ApiKeyConfiguration,
        state: &ConnectionState,
        reentering: bool,
        window: &Window,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let id = provider.id();
        let title = spec.title.clone();
        let row = Row::Provider(id.clone());
        let is_unaligned = spec.group == Group::Unaligned;
        let checking = matches!(state, ConnectionState::Checking);
        let connected = state.is_connected();
        let stored = configuration.has_key;

        // Why Unaligned, in one line: the catalog's sentence.
        let pitch = spec.note.clone().filter(|_| is_unaligned && !reentering);
        let body = v_flex().gap_2().children(pitch.map(Self::secondary));

        // What the form found, else what the provider itself reports.
        let problem = self.problem_for(&row);
        let message = problem
            .map(|problem| problem.message.clone())
            .or_else(|| match state {
                ConnectionState::NeedsAttention { reason } => Some(reason.clone()),
                _ => None,
            });
        let hint = problem.and_then(|problem| problem.hint.clone());

        if configuration.is_from_env_var {
            // No field: a key typed here would be stored and never used.
            return body
                .children(Self::render_problem(message, None))
                .child(Self::secondary(format!(
                    "Using the key in {}. To use another one, change the variable and \
                    restart the IDE.",
                    configuration.env_var_name
                )))
                .into_any_element();
        }

        let Some(editor) = self.key_editors.get(&id) else {
            return body.into_any_element();
        };

        let actions = if self.confirming_disconnect.as_ref() == Some(&id) {
            v_flex()
                .gap_2()
                .child(
                    Label::new(format!(
                        "Disconnect {title}? Its key is removed from this computer."
                    ))
                    .size(LabelSize::Small),
                )
                .child(
                    h_flex()
                        .gap_2()
                        .child(
                            Button::new(row.element_id("confirm-disconnect"), "Disconnect")
                                .style(ButtonStyle::Outlined)
                                .color(Color::Error)
                                .tab_index(0isize)
                                .on_click(cx.listener({
                                    let id = id.clone();
                                    move |this, _, _, cx| this.disconnect(id.clone(), cx)
                                })),
                        )
                        .child(
                            Button::new(row.element_id("cancel-disconnect"), "Cancel")
                                .style(ButtonStyle::Outlined)
                                .tab_index(0isize)
                                .on_click(cx.listener(|this, _, _, cx| this.cancel_disconnect(cx))),
                        ),
                )
                .into_any_element()
        } else {
            let label = if checking {
                "Checking…"
            } else if stored && connected {
                "Replace Key"
            } else {
                "Connect"
            };
            // Filled only where it is the screen's one action: replacing a
            // key that works, or a key above the composer's Send, is not.
            let connect = if reentering || (stored && connected) {
                Button::new(row.element_id("connect"), label).style(ButtonStyle::Outlined)
            } else {
                Button::primary(row.element_id("connect"), label)
            };
            h_flex()
                .gap_2()
                .flex_wrap()
                .child(
                    connect
                        .tab_index(0isize)
                        .disabled(checking)
                        .on_click(cx.listener({
                            let id = id.clone();
                            move |this, _, window, cx| this.connect_key(id.clone(), window, cx)
                        })),
                )
                .when(
                    !configuration.api_key_url.is_empty() && !(stored && connected),
                    |this| {
                        let url = configuration.api_key_url.clone();
                        if is_unaligned && !reentering {
                            // The newcomer's real next step: a button, and
                            // where it leads.
                            this.child(
                                Button::new(row.element_id("get-key"), "Get a Key")
                                    .style(ButtonStyle::Outlined)
                                    .end_icon(
                                        Icon::new(IconName::ArrowUpRight)
                                            .size(IconSize::XSmall)
                                            .color(Color::Muted),
                                    )
                                    .tab_index(0isize)
                                    .tooltip(Tooltip::text(format!(
                                        "Opens {} in your browser",
                                        spoken_address(&url).trim_end_matches('/')
                                    )))
                                    .on_click(move |_, _, cx| cx.open_url(&url)),
                            )
                        } else {
                            this.child(Self::render_external_link(
                                row.element_id("get-key"),
                                "Get a key",
                                url,
                            ))
                        }
                    },
                )
                .when(is_unaligned && stored && connected, |this| {
                    this.child(Self::render_external_link(
                        row.element_id("add-funds"),
                        "Add funds",
                        BILLING_URL,
                    ))
                })
                .when(stored && !reentering && !checking, |this| {
                    this.child(
                        Self::render_link(row.element_id("disconnect"), "Disconnect", Color::Muted)
                            .on_click(cx.listener({
                                let id = id.clone();
                                move |this, _, _, cx| this.request_disconnect(id.clone(), cx)
                            })),
                    )
                })
                .into_any_element()
        };

        // A keychain that could not store the key (Linux without a Secret
        // Service): the key works until the app quits, and the row says so.
        let session_only = provider.key_is_session_only(cx);
        body.when(stored && connected && !reentering, |this| {
            this.child(Self::secondary(if session_only {
                KEY_SESSION_ONLY_SENTENCE
            } else {
                "A key is stored. Paste another one to replace it."
            }))
        })
        .child(Self::render_field(
            row.element_id("connect-key"),
            format!("{title} key").into(),
            editor,
            message.is_some(),
            checking,
            window,
            cx,
        ))
        .children(Self::render_problem(message, hint))
        .child(actions)
        .into_any_element()
    }

    /// Who is billed and what the button does, for the account sign-ins the
    /// catalog knows.
    fn sign_in_line(id: &LanguageModelProviderId) -> Option<&'static str> {
        match id.0.as_ref() {
            "openai-subscribed" => Some(
                "Uses your ChatGPT plan: nothing is billed by Unaligned. Sign-in opens your \
                browser.",
            ),
            "copilot_chat" => Some(
                "Uses your GitHub Copilot plan: nothing is billed by Unaligned. Sign-in shows \
                a code to enter on GitHub.",
            ),
            "x_ai_subscribed" => Some(
                "Uses your SuperGrok plan: nothing is billed by Unaligned. Sign-in opens your \
                browser.",
            ),
            _ => None,
        }
    }

    /// A provider that brings its own configuration view: an account
    /// sign-in, cloud credentials. The row says who is billed; the button,
    /// the waiting state, the account and "Sign Out" are the provider's.
    fn render_view_body(
        &self,
        spec: &ConnectionSpec,
        provider: &Arc<dyn LanguageModelProvider>,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let id = provider.id();
        let row = Row::Provider(id.clone());
        let connected = provider.is_authenticated(cx);
        let problem = self.problem_for(&row);
        let line = Self::sign_in_line(&id).filter(|_| !connected);
        let vendor_identity = matches!(id.0.as_ref(), "openai-subscribed" | "x_ai_subscribed");
        let waiting = self.sign_in_started.as_ref() == Some(&id) && !connected;

        v_flex()
            .gap_2()
            .children(line.map(Self::secondary))
            .children(Self::render_problem(
                problem.map(|problem| problem.message.clone()),
                problem.and_then(|problem| problem.hint.clone()),
            ))
            .children(self.provider_views.get(&id).cloned())
            .when(waiting && spec.kind == ConnectionKind::SignIn, |this| {
                this.child(Self::secondary(
                    "This window stays open while you sign in. Done closes it.",
                ))
            })
            .when(vendor_identity, |this| {
                this.child(Self::secondary(VENDOR_IDENTITY_NOTE))
            })
            .into_any_element()
    }

    /// A server on this machine: its address, "Detect", and what was found.
    fn render_local_body(
        &self,
        spec: &ConnectionSpec,
        provider: &Arc<dyn LanguageModelProvider>,
        server: LocalServer,
        state: &ConnectionState,
        window: &Window,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let id = provider.id();
        let row = Row::Provider(id.clone());
        let title = spec.title.clone();
        let checking = matches!(state, ConnectionState::Checking);
        let connected = provider.is_authenticated(cx);
        let problem = self.problem_for(&row);
        let detection = self.detections.get(&id).filter(|_| !connected && !checking);

        let result: AnyElement = if connected {
            let count = provider.provided_models(cx).len();
            let found = if count == 1 {
                "Found 1 model.".to_string()
            } else {
                format!("Found {count} models.")
            };
            h_flex()
                .gap_2()
                .child(Self::status_mark(Color::Success, cx))
                .child(Label::new(found).size(LabelSize::Small))
                .into_any_element()
        } else {
            match detection {
                Some(Detection::NoServer { address }) => v_flex()
                    .gap_1()
                    .child(
                        Label::new(format!("No server found at {address}.")).size(LabelSize::Small),
                    )
                    .child(
                        h_flex()
                            .gap_1()
                            .flex_wrap()
                            .child(Self::secondary("Start it with"))
                            .child(Self::secondary(server.start_command).buffer_font(cx))
                            .child(Self::secondary("then press Detect again.")),
                    )
                    .into_any_element(),
                Some(Detection::NoModels { address }) => v_flex()
                    .gap_1()
                    .child(
                        Label::new(format!("The server at {address} has no model yet."))
                            .size(LabelSize::Small),
                    )
                    .child(Self::secondary(format!(
                        "Load a model in {title}, then press Detect again."
                    )))
                    .into_any_element(),
                None => Self::secondary(
                    "Runs on this machine: no key, nothing billed, nothing leaves it.",
                )
                .into_any_element(),
            }
        };

        v_flex()
            .gap_2()
            .children(self.local_addresses.get(&id).map(|editor| {
                Self::render_field(
                    row.element_id("local-address"),
                    format!("{title} address").into(),
                    editor,
                    problem.is_some(),
                    checking,
                    window,
                    cx,
                )
            }))
            .children(Self::render_problem(
                problem.map(|problem| problem.message.clone()),
                problem.and_then(|problem| problem.hint.clone()),
            ))
            .child(result)
            .child(
                h_flex().child(
                    Button::new(
                        row.element_id("detect"),
                        if checking { "Detecting…" } else { "Detect" },
                    )
                    .style(ButtonStyle::Outlined)
                    .tab_index(0isize)
                    .disabled(checking)
                    .on_click(cx.listener({
                        let id = id.clone();
                        move |this, _, window, cx| this.detect(id.clone(), window, cx)
                    })),
                ),
            )
            .into_any_element()
    }

    fn render_provider_body(
        &self,
        spec: &ConnectionSpec,
        provider: &Arc<dyn LanguageModelProvider>,
        state: &ConnectionState,
        reentering: bool,
        window: &Window,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        if let Some(server) = local_server(&provider.id()) {
            return self.render_local_body(spec, provider, server, state, window, cx);
        }
        match provider.settings_view(cx) {
            Some(ProviderSettingsView::ApiKey(configuration)) => {
                self.render_key_body(spec, provider, configuration, state, reentering, window, cx)
            }
            Some(_) => self.render_view_body(spec, provider, cx),
            None => Self::secondary("Nothing to set up here.").into_any_element(),
        }
    }

    /// Whose plan an agent of the catalog runs on.
    fn agent_account(agent: &AgentId) -> &'static str {
        match agent.0.as_ref() {
            agent_servers::CLAUDE_AGENT_ID => "Claude",
            agent_servers::CODEX_ID => "ChatGPT",
            agent_servers::GEMINI_ID => "Google",
            _ => "own",
        }
    }

    /// An agent that brings its own subscription: one "Set Up" away, then
    /// "New Chat" and "Remove".
    fn render_agent_body(
        &self,
        agent: &AgentId,
        state: &ConnectionState,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let row = Row::Agent(agent.clone());
        let name = Self::agent_name(agent);
        let account = Self::agent_account(agent);
        let problem = self.problem_for(&row);
        let problem = Self::render_problem(
            problem.map(|problem| problem.message.clone()),
            problem.and_then(|problem| problem.hint.clone()),
        );
        let start_chat = cx.listener({
            let agent = agent.clone();
            move |this, _, window, cx| this.start_agent_chat(agent.clone(), window, cx)
        });
        let own_tools = "It has its own tools and modes; inline edit and commit messages keep \
            using your connected models.";

        if self.confirming_removal.as_ref() == Some(agent) {
            return v_flex()
                .gap_2()
                .child(
                    Label::new(format!(
                        "Remove {name}? It leaves your settings; its own files and sign-in \
                        stay on this computer."
                    ))
                    .size(LabelSize::Small),
                )
                .child(
                    h_flex()
                        .gap_2()
                        .child(
                            Button::new(row.element_id("confirm-remove"), "Remove")
                                .style(ButtonStyle::Outlined)
                                .color(Color::Error)
                                .tab_index(0isize)
                                .on_click(cx.listener({
                                    let agent = agent.clone();
                                    move |this, _, _, cx| this.remove_agent(agent.clone(), cx)
                                })),
                        )
                        .child(
                            Button::new(row.element_id("cancel-remove"), "Cancel")
                                .style(ButtonStyle::Outlined)
                                .tab_index(0isize)
                                .on_click(
                                    cx.listener(|this, _, _, cx| this.cancel_agent_removal(cx)),
                                ),
                        ),
                )
                .into_any_element();
        }

        if state.is_connected() {
            return v_flex()
                .gap_2()
                .child(Self::secondary(format!(
                    "{name} answers in the chat with your {account} account: your plan is \
                    billed, not Unaligned. {own_tools}"
                )))
                .children(problem)
                .child(
                    h_flex()
                        .gap_2()
                        .flex_wrap()
                        .child(
                            Button::new(row.element_id("new-chat"), "New Chat")
                                .style(ButtonStyle::Outlined)
                                .tab_index(0isize)
                                .on_click(start_chat),
                        )
                        .child(
                            Self::render_link(row.element_id("remove"), "Remove", Color::Muted)
                                .on_click(cx.listener({
                                    let agent = agent.clone();
                                    move |this, _, _, cx| {
                                        this.request_agent_removal(agent.clone(), cx)
                                    }
                                })),
                        ),
                )
                .into_any_element();
        }

        let setting_up = matches!(state, ConnectionState::Checking);
        v_flex()
            .gap_2()
            .child(Self::secondary(format!(
                "Downloads {name} and opens a chat with it. You sign in there with your \
                {account} account: your plan is billed, not Unaligned. {own_tools}"
            )))
            .children(problem)
            .child(
                h_flex().child(
                    Button::new(
                        row.element_id("set-up"),
                        if setting_up {
                            "Setting Up…"
                        } else {
                            "Set Up"
                        },
                    )
                    .style(ButtonStyle::Outlined)
                    .tab_index(0isize)
                    .disabled(setting_up)
                    .on_click(start_chat),
                ),
            )
            .into_any_element()
    }

    /// A field with its micro-label above: a placeholder alone vanishes
    /// with the first character.
    fn render_labelled_field(
        id: &'static str,
        label: &'static str,
        editor: &Entity<Editor>,
        disabled: bool,
        window: &Window,
        cx: &App,
    ) -> impl IntoElement + use<> {
        v_flex()
            .w_full()
            .gap_1()
            .child(SectionLabel::new(label))
            .child(Self::render_field(
                id.into(),
                label.into(),
                editor,
                false,
                disabled,
                window,
                cx,
            ))
    }

    /// The model ids of the custom endpoint as check rows, narrowed by the
    /// filter when the list is long.
    fn render_custom_models(&self, window: &Window, cx: &mut Context<Self>) -> Option<AnyElement> {
        let models = &self.custom.models;
        if models.is_empty() {
            return None;
        }
        let border = cx.theme().colors().border;
        let long = models.len() > CHECK_ALL_UP_TO;
        let filter = self.custom.filter.read(cx).text(cx).trim().to_lowercase();
        let chosen = models.iter().filter(|model| model.checked).count();
        let shown: Vec<&CustomModel> = models
            .iter()
            .filter(|model| !long || filter.is_empty() || model.id.to_lowercase().contains(&filter))
            .collect();
        let summary = match self.custom.fetched {
            Some(1) => format!("Found 1 model. {chosen} chosen."),
            Some(found) => format!("Found {found} models. {chosen} chosen."),
            None => format!("{chosen} chosen."),
        };
        let nothing_matches = shown.is_empty();

        Some(
            v_flex()
                .w_full()
                .gap_1p5()
                .child(Self::secondary(summary))
                .when(long, |this| {
                    this.child(Self::render_field(
                        "custom-filter".into(),
                        "Filter models".into(),
                        &self.custom.filter,
                        false,
                        false,
                        window,
                        cx,
                    ))
                })
                .child(
                    v_flex()
                        .id("custom-models")
                        .w_full()
                        .max_h(rems(11.))
                        .overflow_y_scroll()
                        .border_1()
                        .border_color(border)
                        .rounded_sm()
                        .p_1()
                        .gap_0p5()
                        .children(shown.into_iter().map(|model| {
                            let id = model.id.clone();
                            Checkbox::new(
                                ElementId::Name(format!("custom-model-{id}").into()),
                                if model.checked {
                                    ToggleState::Selected
                                } else {
                                    ToggleState::Unselected
                                },
                            )
                            .label(model.id.clone())
                            .label_size(LabelSize::Small)
                            .label_color(Color::Default)
                            .on_click(cx.listener(
                                move |this, _, _, cx| {
                                    this.toggle_custom_model(&id, cx);
                                },
                            ))
                        }))
                        .when(nothing_matches, |this| {
                            this.child(Self::secondary("No model matches this filter."))
                        }),
                )
                .into_any_element(),
        )
    }

    fn render_custom_body(&self, window: &Window, cx: &mut Context<Self>) -> AnyElement {
        let problem = self.problem_for(&Row::Custom);
        let adding = self.checking == Some(Row::Custom);
        let fetching = self.custom.fetching;
        let busy = adding || fetching;
        let anthropic = self.custom.anthropic;
        let models = self.render_custom_models(window, cx);
        let model_id_field = Self::render_labelled_field(
            "custom-model-id",
            "Model id",
            &self.custom.model_id,
            busy,
            window,
            cx,
        );

        let api_choice = |id: &'static str, label: &'static str, selected: bool, value: bool| {
            Button::new(id, label)
                .style(ButtonStyle::Outlined)
                .label_size(LabelSize::Small)
                .toggle_state(selected)
                .tab_index(0isize)
                .disabled(busy)
                .on_click(cx.listener(move |this, _, window, cx| {
                    this.set_custom_api(value, window, cx);
                }))
        };
        let field = |id: &'static str, label: &'static str, editor: &Entity<Editor>| {
            Self::render_labelled_field(id, label, editor, busy, window, cx)
        };

        v_flex()
            .gap_2()
            .child(Self::secondary(
                "Any server that speaks one of these two APIs. It is billed by whoever runs it.",
            ))
            .child(field("custom-name", "Name", &self.custom.name))
            .child(field("custom-url", "Base URL", &self.custom.api_url))
            .child(
                v_flex().gap_1().child(SectionLabel::new("API kind")).child(
                    h_flex()
                        .gap_1()
                        .flex_wrap()
                        .child(api_choice(
                            "custom-api-openai",
                            "OpenAI-compatible",
                            !anthropic,
                            false,
                        ))
                        .child(api_choice(
                            "custom-api-anthropic",
                            "Anthropic-compatible",
                            anthropic,
                            true,
                        )),
                ),
            )
            .child(field("custom-key", "Key", &self.custom.api_key))
            .child(
                h_flex().child(
                    Button::new(
                        "custom-fetch-models",
                        if fetching {
                            "Fetching…"
                        } else if self.custom.fetched.is_some() {
                            "Fetch Again"
                        } else {
                            "Fetch Models"
                        },
                    )
                    .style(ButtonStyle::Outlined)
                    .tab_index(0isize)
                    .disabled(busy)
                    .on_click(cx.listener(|this, _, window, cx| {
                        this.fetch_custom_models(window, cx);
                    })),
                ),
            )
            .children(models)
            .child(
                h_flex()
                    .w_full()
                    .gap_2()
                    .items_end()
                    .child(div().flex_1().min_w_0().child(model_id_field))
                    .child(
                        Button::new("custom-add-model-id", "Add Model Id")
                            .style(ButtonStyle::Outlined)
                            .tab_index(0isize)
                            .disabled(busy)
                            .on_click(cx.listener(|this, _, window, cx| {
                                this.add_custom_model_id(window, cx);
                            })),
                    ),
            )
            .children(Self::render_problem(
                problem.map(|problem| problem.message.clone()),
                problem.and_then(|problem| problem.hint.clone()),
            ))
            .child(
                h_flex()
                    .gap_2()
                    .flex_wrap()
                    .child(
                        // Outlined: the screen's one filled button is
                        // Unaligned's.
                        Button::new("connect-custom", if adding { "Saving…" } else { "Connect" })
                            .style(ButtonStyle::Outlined)
                            .tab_index(0isize)
                            .disabled(busy)
                            .on_click(
                                cx.listener(|this, _, window, cx| this.connect_custom(window, cx)),
                            ),
                    )
                    .child(
                        Self::render_link(
                            "custom-advanced",
                            "Model limits in Settings",
                            Color::Muted,
                        )
                        .on_click(|_, window, cx| {
                            window.dispatch_action(
                                Box::new(app_actions::OpenSettingsAt {
                                    path: "llm_providers".to_string(),
                                    target: None,
                                }),
                                cx,
                            );
                        }),
                    ),
            )
            .into_any_element()
    }

    /// The brand mark at icon size: two overlapping squares, the neutral one
    /// behind in the theme's text colour, the red one in front. The mark is
    /// a logo: it keeps its red in every theme and never follows the accent
    /// (ruled at gate 2).
    pub(crate) fn brand_mark(side: f32, cx: &App) -> impl IntoElement + use<> {
        let colors = cx.theme().colors();
        // In the logo each square is six tenths of the mark's side (41/68):
        // 8.4px of 14, the size of the icons of the other rows, 12px of 20.
        let square = rems_from_px(side * 0.6);
        div()
            .relative()
            .flex_none()
            .size(rems_from_px(side))
            .child(
                div()
                    .absolute()
                    .top_0()
                    .left_0()
                    .size(square)
                    .bg(colors.text),
            )
            .child(
                div()
                    .absolute()
                    .bottom_0()
                    .right_0()
                    .size(square)
                    // The one fixed brand colour allowed in the interface:
                    // the logo's own red, not a theme token.
                    .bg(gpui::rgb(0xe5322d)),
            )
    }

    fn provider_icon(provider: &Arc<dyn LanguageModelProvider>) -> Icon {
        match provider.icon() {
            IconOrSvg::Svg(path) => Icon::from_external_svg(path),
            IconOrSvg::Icon(name) => Icon::new(name),
        }
    }

    /// The state of a row at its right edge: the action word while there is
    /// nothing to report, else a mark and what is known. An open row that is
    /// not connected says nothing here: its own button says it.
    fn render_row_status(
        spec: &ConnectionSpec,
        state: &ConnectionState,
        expanded: bool,
        cx: &App,
    ) -> Option<impl IntoElement + use<>> {
        if expanded && *state == ConnectionState::NotConnected {
            return None;
        }
        let detail = Self::connected_detail(spec, state, cx);
        let mark = match state {
            ConnectionState::Connected { .. } | ConnectionState::FromEnvironment { .. } => {
                Some(if detail.as_ref().is_some_and(is_low_balance) {
                    Color::Warning
                } else {
                    Color::Success
                })
            }
            ConnectionState::NeedsAttention { .. } => Some(Color::Warning),
            ConnectionState::NotConnected | ConnectionState::Checking => None,
        };
        let (words, detail) = match (state, &spec.backing, detail) {
            // "Connected" in the UI font, the fact that changes in the code
            // font, so a balance does not jitter.
            (
                ConnectionState::Connected { .. } | ConnectionState::FromEnvironment { .. },
                Backing::Provider(_),
                Some(detail),
            ) => (
                SharedString::new_static("Connected"),
                Some(format!("· {detail}")),
            ),
            _ => (Self::state_words(spec, state, cx), None),
        };
        let color = if mark.is_some() {
            Color::Default
        } else {
            Color::Muted
        };

        Some(
            h_flex()
                .flex_none()
                .gap_1p5()
                .children(mark.map(|color| Self::status_mark(color, cx)))
                .child(Label::new(words).size(LabelSize::Small).color(color))
                .children(detail.map(|detail| {
                    Label::new(detail)
                        .size(LabelSize::Small)
                        .color(color)
                        .buffer_font(cx)
                })),
        )
    }

    /// One line of the list: icon, title, state. Click or Enter opens it.
    fn render_row_header(
        &self,
        spec: &ConnectionSpec,
        state: &ConnectionState,
        expanded: bool,
        cx: &mut Context<Self>,
    ) -> impl IntoElement + use<> {
        let row = Row::of(spec);
        let title = spec.title.clone();
        // The Unaligned row carries the brand mark; every other row a muted
        // 16px icon.
        let icon = if spec.group == Group::Unaligned {
            Self::brand_mark(14., cx).into_any_element()
        } else {
            let icon = match &spec.backing {
                Backing::Provider(id) if !spec.adds_custom_endpoint() => Self::provider(id, cx)
                    .map_or_else(
                        || Icon::new(spec.icon),
                        |provider| Self::provider_icon(&provider),
                    ),
                _ => Icon::new(spec.icon),
            };
            icon.size(IconSize::Small)
                .color(Color::Muted)
                .into_any_element()
        };
        let note = spec.note.clone().filter(|_| spec.group != Group::Unaligned);
        let spoken = format!("{title}, {}", Self::state_words(spec, state, cx));

        ButtonLike::new(row.element_id("connect-row"))
            .full_width()
            .height(ui::metrics::row_height(cx).into())
            .style(ButtonStyle::Subtle)
            .tab_index(0isize)
            .aria_label(spoken)
            .aria_expanded(expanded)
            .on_click(cx.listener(move |this, _, window, cx| {
                this.toggle(row.clone(), window, cx);
            }))
            .child(
                h_flex()
                    .w_full()
                    .min_w_0()
                    .px_1()
                    .gap_2()
                    .justify_between()
                    .child(
                        h_flex()
                            .min_w_0()
                            .gap_2()
                            .child(icon)
                            .child(Label::new(title).truncate())
                            .children(note.map(|note| Self::secondary(note).truncate())),
                    )
                    .child(
                        h_flex()
                            .flex_none()
                            .gap_1p5()
                            .children(Self::render_row_status(spec, state, expanded, cx))
                            .child(
                                Icon::new(if expanded {
                                    IconName::ChevronDown
                                } else {
                                    IconName::ChevronRight
                                })
                                .size(IconSize::XSmall)
                                .color(Color::Muted),
                            ),
                    ),
            )
    }

    fn render_row_body(
        &self,
        spec: &ConnectionSpec,
        state: &ConnectionState,
        window: &Window,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        if spec.adds_custom_endpoint() {
            return self.render_custom_body(window, cx);
        }
        match &spec.backing {
            Backing::Agent(agent) => self.render_agent_body(agent, state, cx),
            Backing::Provider(id) => match Self::provider(id, cx) {
                Some(provider) => {
                    self.render_provider_body(spec, &provider, state, false, window, cx)
                }
                None => {
                    Self::secondary("This connection is not available any more.").into_any_element()
                }
            },
        }
    }

    /// A row and, when it is the open one, its form underneath.
    fn render_row(
        &self,
        spec: &ConnectionSpec,
        state: &ConnectionState,
        window: &Window,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let expanded = self.expanded.as_ref() == Some(&Row::of(spec));
        let header = self.render_row_header(spec, state, expanded, cx);
        let body = expanded.then(|| self.render_row_body(spec, state, window, cx));

        v_flex()
            .w_full()
            .child(header)
            .children(body.map(|body| div().px_3().pt_1().pb_3().child(body)))
            .into_any_element()
    }

    /// A group of the screen: its label, then its rows in one outlined list.
    fn render_group(
        label: &'static str,
        // Who is billed, in one line under the label.
        billing: Option<&'static str>,
        rows: Vec<AnyElement>,
        cx: &App,
    ) -> Option<impl IntoElement + use<>> {
        if rows.is_empty() {
            return None;
        }
        let border = cx.theme().colors().border;
        let last = rows.len() - 1;
        Some(
            v_flex()
                .w_full()
                .gap_1p5()
                .child(
                    v_flex()
                        .gap_0p5()
                        .child(SectionLabel::new(label))
                        .children(billing.map(Self::secondary)),
                )
                .child(
                    v_flex()
                        .w_full()
                        .border_1()
                        .border_color(border)
                        // Square in the Unaligned family, rounded with the
                        // modal in round themes.
                        .rounded_sm()
                        .overflow_hidden()
                        .children(rows.into_iter().enumerate().map(|(index, row)| {
                            div()
                                .w_full()
                                .when(index < last, |this| this.border_b_1().border_color(border))
                                .child(row)
                        })),
                ),
        )
    }

    /// The row that folds the connections few people need.
    fn render_more_toggle(
        &self,
        hidden: usize,
        cx: &mut Context<Self>,
    ) -> impl IntoElement + use<> {
        let open = self.show_more;
        ButtonLike::new("connect-show-more")
            .full_width()
            .height(ui::metrics::row_height(cx).into())
            .style(ButtonStyle::Subtle)
            .tab_index(0isize)
            .aria_label("More connections")
            .aria_expanded(open)
            .on_click(cx.listener(|this, _, _, cx| {
                this.show_more = !this.show_more;
                cx.notify();
            }))
            .child(
                h_flex()
                    .w_full()
                    .px_1()
                    .gap_2()
                    .justify_between()
                    .child(
                        h_flex()
                            .gap_2()
                            .child(
                                Icon::new(IconName::Ellipsis)
                                    .size(IconSize::Small)
                                    .color(Color::Muted),
                            )
                            .child(Label::new("More").color(Color::Muted)),
                    )
                    .child(
                        h_flex()
                            .flex_none()
                            .gap_1p5()
                            .when(!open && hidden > 0, |this| {
                                this.child(Self::secondary(hidden.to_string()).buffer_font(cx))
                            })
                            .child(
                                Icon::new(if open {
                                    IconName::ChevronDown
                                } else {
                                    IconName::ChevronRight
                                })
                                .size(IconSize::XSmall)
                                .color(Color::Muted),
                            ),
                    ),
            )
    }

    /// The whole screen.
    fn render_picker(&self, window: &Window, cx: &mut Context<Self>) -> AnyElement {
        let rows: Vec<(ConnectionSpec, ConnectionState)> = connections::catalog(cx)
            .into_iter()
            .map(|spec| {
                let state = self.row_state(&spec, cx);
                (spec, state)
            })
            .collect();

        let mut unaligned = Vec::new();
        let mut subscriptions = Vec::new();
        let mut own = Vec::new();
        let mut more = Vec::new();
        let mut hidden = 0;
        let has_more = rows.iter().any(|(spec, _)| spec.group == Group::More);
        for (spec, state) in &rows {
            let group = match spec.group {
                Group::Unaligned => &mut unaligned,
                Group::Subscription => &mut subscriptions,
                Group::OwnKeyOrLocal => &mut own,
                Group::More => {
                    // Never hide a connection that is in use, or the open row.
                    let shown = self.show_more
                        || *state != ConnectionState::NotConnected
                        || self.expanded.as_ref() == Some(&Row::of(spec));
                    if !shown {
                        hidden += 1;
                        continue;
                    }
                    &mut more
                }
            };
            group.push(self.render_row(spec, state, window, cx));
        }
        if has_more {
            own.push(self.render_more_toggle(hidden, cx).into_any_element());
            own.extend(more);
        }

        v_flex()
            .w_full()
            .gap_4()
            .when(self.show_title, |this| {
                this.child(
                    Label::new("Connect a Model")
                        .size(LabelSize::Large)
                        .weight(FontWeight::MEDIUM),
                )
            })
            .children(self.notice_text(cx).map(|notice| {
                h_flex()
                    .gap_2()
                    .items_start()
                    .child(
                        div()
                            .pt(px(5.))
                            .child(Self::status_mark(Color::Success, cx)),
                    )
                    .child(Label::new(notice).size(LabelSize::Small))
            }))
            .children(Self::render_group(
                "Unaligned · recommended",
                None,
                unaligned,
                cx,
            ))
            .children(Self::render_group(
                "Use a subscription you already have",
                Some("You keep paying your plan, nothing more."),
                subscriptions,
                cx,
            ))
            .children(Self::render_group(
                "Use your own key or a local model",
                Some(
                    "A key is billed by the company that issued it. A local model is free \
                    and stays on this machine.",
                ),
                own,
                cx,
            ))
            .child(
                v_flex()
                    .gap_0p5()
                    .child(Self::secondary(STORAGE_SENTENCE))
                    .child(Self::secondary(
                        "You can connect several and choose per chat.",
                    )),
            )
            .into_any_element()
    }

    /// One connection's key field after a refused request, and a way out.
    fn render_reenter(
        &self,
        provider: &Arc<dyn LanguageModelProvider>,
        window: &Window,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let spec = Self::spec_for_provider(&provider.id(), cx);
        let state = self.row_state(&spec, cx);
        let title = spec.title.clone();
        v_flex()
            .w_full()
            .gap_3()
            .child(
                v_flex()
                    .gap_1()
                    .child(
                        Label::new(format!("Enter a new {title} key"))
                            .size(LabelSize::Large)
                            .weight(FontWeight::MEDIUM),
                    )
                    .child(Self::secondary(
                        "Paste a key that works to continue this chat.",
                    )),
            )
            .child(self.render_provider_body(&spec, provider, &state, true, window, cx))
            .child(
                h_flex()
                    .gap_2()
                    .child(
                        Self::render_link("cancel-reenter-key", "Cancel", Color::Muted).on_click(
                            cx.listener(|this, _, _, cx| {
                                this.problem = None;
                                this.reenter_target = None;
                                cx.emit(ConnectFormEvent::Dismissed);
                                cx.notify();
                            }),
                        ),
                    )
                    .child(
                        Self::render_link(
                            "reenter-all-connections",
                            "All connections",
                            Color::Muted,
                        )
                        .on_click(|_, window, cx| {
                            window.dispatch_action(Box::new(ManageProviders), cx);
                        }),
                    ),
            )
            .into_any_element()
    }

    /// The form itself. `reentering` is the variant shown after a refused
    /// key: that connection's field only, different words, and a way out.
    pub(crate) fn render_form(
        &self,
        reentering: bool,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> impl IntoElement {
        let reenter_provider = reentering
            .then(|| self.reenter_target.as_ref())
            .flatten()
            .and_then(|id| Self::provider(id, cx));

        v_flex()
            .w_full()
            .track_focus(&self.focus_handle)
            .on_action(cx.listener(Self::confirm))
            .on_action(cx.listener(Self::cancel))
            // Tab and Shift-Tab walk the rows, fields and buttons.
            .on_action(|_: &menu::SelectNext, window, cx| window.focus_next(cx))
            .on_action(|_: &menu::SelectPrevious, window, cx| window.focus_prev(cx))
            .child(match reenter_provider {
                Some(provider) => self.render_reenter(&provider, window, cx),
                None => self.render_picker(window, cx),
            })
    }
}

/// Whether credentials go to a desktop keyring that may not be running.
const DESKTOP_KEYRING: bool = cfg!(any(target_os = "linux", target_os = "freebsd"));

/// Said wherever a key is in use that the system keychain could not store.
/// The key is never written to a file instead.
pub(crate) const KEY_SESSION_ONLY_SENTENCE: &str =
    "This key is kept for this session only: no system keyring was found.";

/// Where credentials are kept, in the words of the user's system.
const STORAGE_SENTENCE: &str = if DESKTOP_KEYRING {
    "Keys and sign-ins are stored in your desktop keyring when one is running."
} else if cfg!(target_os = "windows") {
    "Keys and sign-ins are stored in Windows Credential Manager."
} else {
    "Keys and sign-ins are stored in your system keychain."
};

/// What to do when a key cannot be stored on a system whose keyring is a
/// service of the desktop: it is usually not running.
fn keyring_hint(
    desktop_keyring: bool,
    environment_variable: Option<&SharedString>,
) -> Option<SharedString> {
    if !desktop_keyring {
        return None;
    }
    Some(match environment_variable {
        Some(variable) => format!(
            "No keyring service answered. Start GNOME Keyring or KWallet, or set the \
            {variable} environment variable."
        )
        .into(),
        None => "No keyring service answered. Start GNOME Keyring or KWallet.".into(),
    })
}

/// Whether a connection's detail says its balance is running out. The
/// Unaligned provider ends the detail with "low" under one dollar ("$0.42 left
/// · low"); nothing else on the provider's interface carries that fact.
fn is_low_balance(detail: &SharedString) -> bool {
    detail.ends_with("· low")
}

/// An error as the end of a sentence: its whole chain, without a final period
/// (the sentence brings its own).
fn sentence_body(error: &anyhow::Error) -> String {
    format!("{error:#}").trim_end_matches('.').to_string()
}

impl Focusable for ConnectForm {
    fn focus_handle(&self, cx: &App) -> FocusHandle {
        match &self.expanded {
            Some(Row::Provider(id)) => self
                .key_editors
                .get(id)
                .or_else(|| self.local_addresses.get(id))
                .map(|editor| editor.focus_handle(cx))
                .unwrap_or_else(|| self.focus_handle.clone()),
            Some(Row::Custom) => self.custom.name.focus_handle(cx),
            Some(Row::Agent(_)) | None => self.focus_handle.clone(),
        }
    }
}

/// The connection screen as a modal: where connections are added and managed
/// once the panel is past its connection screen.
pub(crate) struct ManageProvidersModal {
    form: Entity<ConnectForm>,
    /// "Done" or Escape was used: close whatever the form is doing.
    closing: bool,
    scroll_handle: ScrollHandle,
    /// What the lower part of the list showed when it was last drawn: the
    /// open row and how many models the custom endpoint lists. When it
    /// changes, the list is scrolled so that row's form is in view.
    lower_rows_shown: Option<(Row, usize)>,
    _subscription: Subscription,
}

impl ManageProvidersModal {
    pub(crate) fn register(
        workspace: &mut Workspace,
        _window: Option<&mut Window>,
        _cx: &mut Context<Workspace>,
    ) {
        workspace.register_action(|workspace, _: &ManageProviders, window, cx| {
            workspace.toggle_modal(window, cx, Self::new);
        });
    }

    fn new(window: &mut Window, cx: &mut Context<Self>) -> Self {
        let form = cx.new(|cx| {
            let mut form = ConnectForm::new(window, cx);
            form.show_title = false;
            form
        });
        let subscription = cx.subscribe(&form, |this, _, event: &ConnectFormEvent, cx| {
            // Connecting keeps the modal open: another connection may follow.
            // A chat that starts with an agent takes the screen's place.
            if matches!(
                event,
                ConnectFormEvent::Dismissed | ConnectFormEvent::AgentChatStarted
            ) {
                this.close(cx);
            }
            cx.notify();
        });
        Self {
            form,
            closing: false,
            scroll_handle: ScrollHandle::new(),
            lower_rows_shown: None,
            _subscription: subscription,
        }
    }

    fn close(&mut self, cx: &mut Context<Self>) {
        self.closing = true;
        cx.emit(DismissEvent);
    }
}

impl ModalView for ManageProvidersModal {
    /// A click outside does not close the modal under a check that is about
    /// to answer, or a sign-in that may be waiting for the browser. "Done"
    /// and Escape always close.
    fn on_before_dismiss(
        &mut self,
        _window: &mut Window,
        cx: &mut Context<Self>,
    ) -> DismissDecision {
        if self.closing || !self.form.read(cx).is_busy(cx) {
            DismissDecision::Dismiss(true)
        } else {
            DismissDecision::Pending
        }
    }
}

impl EventEmitter<DismissEvent> for ManageProvidersModal {}

impl Focusable for ManageProvidersModal {
    fn focus_handle(&self, cx: &App) -> FocusHandle {
        self.form.focus_handle(cx)
    }
}

impl Render for ManageProvidersModal {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        // A row opened in the last group has its form under the fold: the
        // list follows it. Rows of the groups above always fit.
        let lower_rows_shown = self.form.read_with(cx, |form, cx| {
            let row = form.expanded.clone()?;
            let low = match &row {
                Row::Custom => true,
                Row::Agent(_) => false,
                Row::Provider(id) => matches!(
                    ConnectForm::spec_for_provider(id, cx).group,
                    Group::OwnKeyOrLocal | Group::More
                ),
            };
            low.then(|| (row, form.custom.models.len()))
        });
        if lower_rows_shown != self.lower_rows_shown {
            if lower_rows_shown.is_some() {
                self.scroll_handle.scroll_to_bottom();
            }
            self.lower_rows_shown = lower_rows_shown;
        }
        let form = self.form.update(cx, |form, cx| {
            form.render_form(false, window, cx).into_any_element()
        });
        let border = cx.theme().colors().border;

        v_flex()
            .key_context("ManageProvidersModal")
            .elevation_3(cx)
            .w(rems(32.))
            .on_action(cx.listener(|this, _: &menu::Cancel, _, cx| this.close(cx)))
            .child(
                h_flex()
                    .h(ui::metrics::panel_header_height(cx))
                    .px_4()
                    .border_b_1()
                    .border_color(border)
                    .child(
                        Label::new("Connections")
                            .size(LabelSize::Large)
                            .weight(FontWeight::MEDIUM),
                    ),
            )
            .child(
                div()
                    .id("manage-providers-scroll")
                    .max_h(vh(0.7, window))
                    .overflow_y_scroll()
                    .track_scroll(&self.scroll_handle)
                    .p_4()
                    .child(form),
            )
            .child(
                h_flex()
                    .px_4()
                    .py_2()
                    .justify_end()
                    .border_t_1()
                    .border_color(border)
                    .child(
                        Button::new("manage-providers-done", "Done")
                            .style(ButtonStyle::Outlined)
                            .tab_index(0isize)
                            .on_click(cx.listener(|this, _, _, cx| this.close(cx))),
                    ),
            )
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use anyhow::{Result, anyhow};
    use futures::{channel::oneshot, future::BoxFuture};
    use gpui::{AsyncApp, TestAppContext, UpdateGlobal as _, VisualTestContext};
    use language_model::{
        AuthenticateError, LanguageModelClient, LanguageModelCompletionError,
        LanguageModelCompletionStream, LanguageModelProviderName, LanguageModelProviderState,
        LanguageModelRequest, fake_provider::FakeLanguageModelProvider,
    };
    use language_models::AllLanguageModelSettings;
    use parking_lot::Mutex;
    use project::agent_server_store::AllAgentServersSettings;
    use settings::SettingsStore;
    use std::{cell::RefCell, rc::Rc};

    /// What a [`KeyProvider`] holds and how its server answers.
    #[derive(Default)]
    struct Script {
        stored_key: Option<String>,
        /// What the server says about a stored key. `None`: it accepts it.
        verdict: Option<ProviderConnectionStatus>,
        /// Makes the keychain refuse to store a key.
        store_error: Option<&'static str>,
        /// The next check waits for this before it answers.
        gate: Option<oneshot::Receiver<()>>,
        /// Nothing answers at the server's address (local servers).
        unreachable: bool,
    }

    /// A provider connected by one API key, with a keychain and a server the
    /// test controls.
    struct KeyProvider {
        fake: FakeLanguageModelProvider,
        script: Arc<Mutex<Script>>,
    }

    impl LanguageModelProviderState for KeyProvider {
        type ObservableEntity = ();

        fn observable_entity(&self) -> Option<Entity<Self::ObservableEntity>> {
            None
        }
    }

    impl LanguageModelClient for KeyProvider {
        fn stream_completion(
            &self,
            model: &LanguageModel,
            request: LanguageModelRequest,
            cx: &AsyncApp,
        ) -> BoxFuture<'static, Result<LanguageModelCompletionStream, LanguageModelCompletionError>>
        {
            self.fake.stream_completion(model, request, cx)
        }
    }

    impl LanguageModelProvider for KeyProvider {
        fn id(&self) -> LanguageModelProviderId {
            self.fake.id()
        }

        fn name(&self) -> LanguageModelProviderName {
            self.fake.name()
        }

        fn default_model(&self, cx: &App) -> Option<LanguageModel> {
            self.fake.default_model(cx)
        }

        fn default_fast_model(&self, cx: &App) -> Option<LanguageModel> {
            self.fake.default_fast_model(cx)
        }

        fn provided_models(&self, cx: &App) -> Vec<LanguageModel> {
            self.fake.provided_models(cx)
        }

        fn is_authenticated(&self, _: &App) -> bool {
            self.script.lock().stored_key.is_some()
        }

        fn authenticate(&self, _: &mut App) -> Task<Result<(), AuthenticateError>> {
            if self.script.lock().unreachable {
                return Task::ready(Err(AuthenticateError::Other(anyhow!("connection refused"))));
            }
            Task::ready(Ok(()))
        }

        fn settings_view(&self, _: &mut App) -> Option<ProviderSettingsView> {
            Some(ProviderSettingsView::ApiKey(ApiKeyConfiguration::new(
                self.script.lock().stored_key.is_some(),
                false,
                "TEST_API_KEY".into(),
                "https://example.com/keys".into(),
            )))
        }

        fn set_api_key(&self, key: Option<String>, _: &mut App) -> Task<Result<()>> {
            let mut script = self.script.lock();
            if let (Some(error), Some(_)) = (script.store_error, &key) {
                return Task::ready(Err(anyhow!(error)));
            }
            script.stored_key = key;
            Task::ready(Ok(()))
        }

        fn connection_status(&self, _: &App) -> ProviderConnectionStatus {
            status_of(&self.script.lock())
        }

        fn check_connection(&self, cx: &mut App) -> Task<Result<ProviderConnectionStatus>> {
            let script = self.script.clone();
            let gate = script.lock().gate.take();
            cx.spawn(async move |_| {
                if let Some(gate) = gate {
                    gate.await.ok();
                }
                Ok(status_of(&script.lock()))
            })
        }
    }

    fn status_of(script: &Script) -> ProviderConnectionStatus {
        match (&script.stored_key, &script.verdict) {
            (None, _) => ProviderConnectionStatus::NotConnected,
            (Some(_), Some(verdict)) => verdict.clone(),
            (Some(_), None) => ProviderConnectionStatus::Connected { detail: None },
        }
    }

    /// Hosts the form the way the chat panel does: it draws `render_form`.
    struct Host {
        form: Entity<ConnectForm>,
        /// Draw the variant that replaces a refused key.
        reentering: bool,
    }

    impl Render for Host {
        fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
            let reentering = self.reentering;
            self.form.update(cx, |form, cx| {
                form.render_form(reentering, window, cx).into_any_element()
            })
        }
    }

    fn init_test(cx: &mut TestAppContext) {
        cx.update(|cx| {
            let store = SettingsStore::test(cx);
            cx.set_global(store);
            theme::init(theme::LoadThemes::JustBase, cx);
            theme_settings::init(theme::LoadThemes::JustBase, cx);
            editor::init(cx);
            release_channel::init(semver::Version::new(0, 0, 0), cx);
            AllLanguageModelSettings::register(cx);
            AllAgentServersSettings::register(cx);
            <dyn Fs>::set_global(fs::FakeFs::new(cx.background_executor().clone()), cx);
            LanguageModelRegistry::test(cx);
            // `test` registers a provider that is always connected: these
            // tests start with nothing connected.
            LanguageModelRegistry::global(cx).update(cx, |registry, cx| {
                registry.unregister_provider(LanguageModelProviderId::from("fake".to_string()), cx);
                registry.set_default_model(None, cx);
            });
            connections::init(cx);
        });
    }

    fn register_key_provider(
        id: &str,
        name: &str,
        cx: &mut TestAppContext,
    ) -> (Arc<KeyProvider>, Arc<Mutex<Script>>) {
        let script = Arc::new(Mutex::new(Script::default()));
        let provider = Arc::new(KeyProvider {
            fake: FakeLanguageModelProvider::new(
                LanguageModelProviderId::from(id.to_string()),
                LanguageModelProviderName::from(name.to_string()),
            ),
            script: script.clone(),
        });
        cx.update(|cx| {
            LanguageModelRegistry::global(cx).update(cx, |registry, cx| {
                registry.register_provider(provider.clone(), cx);
            });
        });
        (provider, script)
    }

    /// The form in a window, and the events it emitted so far.
    fn open_form(
        cx: &mut TestAppContext,
    ) -> (
        Entity<ConnectForm>,
        Rc<RefCell<Vec<&'static str>>>,
        &mut VisualTestContext,
    ) {
        let (host, cx) = cx.add_window_view(|window, cx| Host {
            form: cx.new(|cx| ConnectForm::new(window, cx)),
            reentering: false,
        });
        let form = host.read_with(cx, |host, _| host.form.clone());
        let events = Rc::new(RefCell::new(Vec::new()));
        cx.update(|_, cx| {
            cx.subscribe(&form, {
                let events = events.clone();
                move |_, event: &ConnectFormEvent, _| {
                    events.borrow_mut().push(match event {
                        ConnectFormEvent::Connected => "connected",
                        ConnectFormEvent::Dismissed => "dismissed",
                        ConnectFormEvent::AgentChatStarted => "agent-chat-started",
                    });
                }
            })
            .detach();
        });
        (form, events, cx)
    }

    fn unaligned_state(form: &Entity<ConnectForm>, cx: &mut VisualTestContext) -> ConnectionState {
        form.read_with(cx, |form, cx| {
            form.row_state(&ConnectForm::spec_for_provider(&UNALIGNED, cx), cx)
        })
    }

    fn key_text(
        form: &Entity<ConnectForm>,
        id: &LanguageModelProviderId,
        cx: &mut VisualTestContext,
    ) -> String {
        form.read_with(cx, |form, cx| form.key_editors[id].read(cx).text(cx))
    }

    /// Types `key` into the Unaligned field and presses Connect.
    fn connect_unaligned(form: &Entity<ConnectForm>, key: &str, cx: &mut VisualTestContext) {
        // Typing comes first, as it does for a user: an edit clears the
        // sentence of the attempt before it.
        form.update_in(cx, |form, window, cx| {
            let editor = form.key_editor(&UNALIGNED, window, cx);
            editor.update(cx, |editor, cx| editor.set_text(key, window, cx));
        });
        form.update_in(cx, |form, window, cx| {
            form.connect_key(UNALIGNED, window, cx)
        });
    }

    #[gpui::test]
    fn test_unaligned_is_open_while_nothing_is_connected(cx: &mut TestAppContext) {
        init_test(cx);
        register_key_provider("unaligned", "Unaligned", cx);
        register_key_provider("openai", "OpenAI", cx);

        let (form, events, cx) = open_form(cx);

        form.read_with(cx, |form, _| {
            assert_eq!(form.expanded, Some(Row::Provider(UNALIGNED)));
            assert!(form.key_editors.contains_key(&UNALIGNED));
            assert_eq!(form.notice, None);
        });
        assert_eq!(unaligned_state(&form, cx), ConnectionState::NotConnected);
        assert!(events.borrow().is_empty());
    }

    #[gpui::test]
    fn test_unaligned_stays_closed_once_something_is_connected(cx: &mut TestAppContext) {
        init_test(cx);
        register_key_provider("unaligned", "Unaligned", cx);
        let (_, openai) = register_key_provider("openai", "OpenAI", cx);
        openai.lock().stored_key = Some("sk-test".into());

        let (form, _, cx) = open_form(cx);

        form.read_with(cx, |form, _| assert_eq!(form.expanded, None));
    }

    #[gpui::test]
    fn test_accepted_key_is_checked_before_the_row_says_connected(cx: &mut TestAppContext) {
        init_test(cx);
        let (_, script) = register_key_provider("unaligned", "Unaligned", cx);
        let (answer, gate) = oneshot::channel();
        {
            let mut script = script.lock();
            script.gate = Some(gate);
            script.verdict = Some(ProviderConnectionStatus::Connected {
                detail: Some("$20.50 left".into()),
            });
        }
        let (form, events, cx) = open_form(cx);

        connect_unaligned(&form, "  sk-unal-test  ", cx);
        cx.run_until_parked();

        // The key is stored and the server has not answered: not connected yet.
        assert_eq!(script.lock().stored_key.as_deref(), Some("sk-unal-test"));
        assert_eq!(unaligned_state(&form, cx), ConnectionState::Checking);
        assert!(events.borrow().is_empty());
        form.read_with(cx, |form, cx| {
            assert!(form.is_checking());
            assert!(form.is_busy(cx));
            assert!(form.key_editors[&UNALIGNED].read(cx).read_only(cx));
            assert_eq!(form.notice_text(cx), None);
        });

        answer.send(()).expect("the check should be waiting");
        cx.run_until_parked();

        assert_eq!(
            unaligned_state(&form, cx),
            ConnectionState::Connected {
                detail: Some("$20.50 left".into())
            }
        );
        assert_eq!(*events.borrow(), ["connected"]);
        assert_eq!(key_text(&form, &UNALIGNED, cx), "");
        form.read_with(cx, |form, cx| {
            assert!(!form.is_checking());
            assert!(!form.key_editors[&UNALIGNED].read(cx).read_only(cx));
            // The row closes on its result.
            assert_eq!(form.expanded, None);
            let spec = ConnectForm::spec_for_provider(&UNALIGNED, cx);
            let state = form.row_state(&spec, cx);
            assert_eq!(
                ConnectForm::state_words(&spec, &state, cx),
                "Connected · $20.50 left"
            );
            assert!(
                form.notice_text(cx)
                    .is_some_and(|notice| notice.starts_with("Connected to Unaligned.")),
                "{:?}",
                form.notice_text(cx)
            );
        });

        // The key is removed elsewhere (the modal, Settings): the line would
        // no longer be true, and Unaligned opens again as on a first run.
        script.lock().stored_key = None;
        form.update(cx, |form, cx| form.reopen_default_row(cx));
        form.read_with(cx, |form, cx| {
            assert_eq!(form.notice_text(cx), None);
            assert_eq!(form.expanded, Some(Row::Provider(UNALIGNED)));
        });
    }

    #[gpui::test]
    fn test_refused_key_is_not_kept_and_stays_in_the_field(cx: &mut TestAppContext) {
        init_test(cx);
        let (_, script) = register_key_provider("unaligned", "Unaligned", cx);
        script.lock().verdict = Some(ProviderConnectionStatus::NeedsAttention {
            reason: "This key was refused.".into(),
        });
        let (form, events, cx) = open_form(cx);

        connect_unaligned(&form, "sk-unal-bad", cx);
        cx.run_until_parked();

        assert_eq!(
            unaligned_state(&form, cx),
            ConnectionState::NeedsAttention {
                reason: "This key was refused.".into()
            }
        );
        assert!(
            events.borrow().is_empty(),
            "a refused key must not report a connection"
        );
        assert_eq!(key_text(&form, &UNALIGNED, cx), "sk-unal-bad");
        assert_eq!(script.lock().stored_key, None);
        form.read_with(cx, |form, cx| {
            assert_eq!(form.expanded, Some(Row::Provider(UNALIGNED)));
            assert_eq!(form.notice_text(cx), None);
            assert!(!form.is_checking());
            assert!(!form.key_editors[&UNALIGNED].read(cx).read_only(cx));
        });

        // Correcting the key takes the sentence away, and a good key connects.
        script.lock().verdict = None;
        connect_unaligned(&form, "sk-unal-good", cx);
        cx.run_until_parked();

        assert_eq!(*events.borrow(), ["connected"]);
        assert_eq!(
            unaligned_state(&form, cx),
            ConnectionState::Connected { detail: None }
        );
        assert_eq!(script.lock().stored_key.as_deref(), Some("sk-unal-good"));
    }

    #[gpui::test]
    fn test_empty_key_is_not_sent(cx: &mut TestAppContext) {
        init_test(cx);
        let (_, script) = register_key_provider("unaligned", "Unaligned", cx);
        let (form, events, cx) = open_form(cx);

        connect_unaligned(&form, "   ", cx);
        cx.run_until_parked();

        assert_eq!(script.lock().stored_key, None);
        assert!(events.borrow().is_empty());
        form.read_with(cx, |form, _| {
            let problem = form.problem.as_ref().expect("the row should say why");
            assert_eq!(problem.message, "Paste your key first.");
            assert!(!problem.needs_attention);
        });
        assert_eq!(unaligned_state(&form, cx), ConnectionState::NotConnected);
    }

    #[gpui::test]
    fn test_keychain_failure_says_why(cx: &mut TestAppContext) {
        init_test(cx);
        let (_, script) = register_key_provider("unaligned", "Unaligned", cx);
        script.lock().store_error = Some("the keychain is locked.");
        let (form, events, cx) = open_form(cx);

        connect_unaligned(&form, "sk-unal-test", cx);
        cx.run_until_parked();

        assert!(events.borrow().is_empty());
        assert_eq!(key_text(&form, &UNALIGNED, cx), "sk-unal-test");
        form.read_with(cx, |form, _| {
            let problem = form.problem.as_ref().expect("the row should say why");
            assert_eq!(
                problem.message,
                "Could not save the key: the keychain is locked."
            );
            assert_eq!(
                problem.hint,
                keyring_hint(DESKTOP_KEYRING, Some(&"TEST_API_KEY".into()))
            );
            assert!(!problem.needs_attention);
        });
        assert_eq!(unaligned_state(&form, cx), ConnectionState::NotConnected);
    }

    #[test]
    fn test_keyring_hint_names_the_variable_of_the_provider() {
        assert_eq!(keyring_hint(false, Some(&"UNALIGNED_API_KEY".into())), None);
        assert_eq!(
            keyring_hint(true, Some(&"UNALIGNED_API_KEY".into())).as_deref(),
            Some(
                "No keyring service answered. Start GNOME Keyring or KWallet, or set the \
                UNALIGNED_API_KEY environment variable."
            )
        );
        assert_eq!(
            keyring_hint(true, None).as_deref(),
            Some("No keyring service answered. Start GNOME Keyring or KWallet.")
        );
    }

    #[gpui::test]
    fn test_replacing_a_refused_key_above_the_composer(cx: &mut TestAppContext) {
        init_test(cx);
        let (_, script) = register_key_provider("unaligned", "Unaligned", cx);
        script.lock().stored_key = Some("sk-unal-revoked".into());
        script.lock().verdict = Some(ProviderConnectionStatus::NeedsAttention {
            reason: "This key was refused.".into(),
        });
        let (host, cx) = cx.add_window_view(|window, cx| Host {
            form: cx.new(|cx| ConnectForm::new(window, cx)),
            reentering: true,
        });
        let form = host.read_with(cx, |host, _| host.form.clone());

        form.update_in(cx, |form, window, cx| {
            form.begin_reenter(Some(UNALIGNED), window, cx);
        });
        cx.run_until_parked();

        // The stored key is the refused one: the row says so before anything
        // is typed.
        assert_eq!(
            unaligned_state(&form, cx),
            ConnectionState::NeedsAttention {
                reason: "This key was refused.".into()
            }
        );
        form.read_with(cx, |form, _| {
            assert_eq!(form.reenter_target, Some(UNALIGNED));
            assert_eq!(form.expanded, Some(Row::Provider(UNALIGNED)));
        });

        script.lock().verdict = None;
        connect_unaligned(&form, "sk-unal-new", cx);
        cx.run_until_parked();

        assert_eq!(script.lock().stored_key.as_deref(), Some("sk-unal-new"));
        form.read_with(cx, |form, _| assert_eq!(form.reenter_target, None));
        assert_eq!(
            unaligned_state(&form, cx),
            ConnectionState::Connected { detail: None }
        );
    }

    /// Enter as a user presses it: down, then up. A focused row or button
    /// answers the release (the screenshot hook only sends the press).
    fn press_enter(cx: &mut VisualTestContext) {
        cx.simulate_keystrokes("enter");
        cx.simulate_event(gpui::KeyUpEvent {
            keystroke: gpui::Keystroke::parse("enter").expect("a valid keystroke"),
        });
    }

    #[gpui::test]
    fn test_keyboard_walks_rows_and_connects(cx: &mut TestAppContext) {
        init_test(cx);
        // The bindings of the default keymap that this screen relies on.
        cx.update(|cx| {
            cx.bind_keys([
                gpui::KeyBinding::new("enter", menu::Confirm, None),
                gpui::KeyBinding::new("tab", menu::SelectNext, None),
                gpui::KeyBinding::new("shift-tab", menu::SelectPrevious, None),
            ]);
        });
        let (_, script) = register_key_provider("unaligned", "Unaligned", cx);
        let (form, events, cx) = open_form(cx);
        let field = form.read_with(cx, |form, cx| form.key_editors[&UNALIGNED].focus_handle(cx));
        let in_field =
            |cx: &mut VisualTestContext| cx.update(|window, cx| field.contains_focused(window, cx));
        cx.update(|window, cx| field.focus(window, cx));
        cx.run_until_parked();
        assert!(in_field(cx));

        // Shift-Tab leaves the field for the row above it; Enter there is the
        // row's own click, not "connect".
        cx.simulate_keystrokes("shift-tab");
        assert!(!in_field(cx));
        press_enter(cx);
        form.read_with(cx, |form, _| {
            assert_eq!(form.expanded, None);
            assert_eq!(form.problem, None);
        });

        // Tab reaches the row again, Enter opens it with the cursor in the
        // field, and Enter in the field connects.
        cx.simulate_keystrokes("tab");
        press_enter(cx);
        form.read_with(cx, |form, _| {
            assert_eq!(form.expanded, Some(Row::Provider(UNALIGNED)));
        });
        assert!(in_field(cx));

        form.update_in(cx, |form, window, cx| {
            form.key_editors[&UNALIGNED]
                .update(cx, |editor, cx| editor.set_text("sk-unal-test", window, cx));
        });
        press_enter(cx);

        assert_eq!(script.lock().stored_key.as_deref(), Some("sk-unal-test"));
        assert_eq!(*events.borrow(), ["connected"]);
    }

    #[test]
    fn test_low_balance_is_read_from_the_detail() {
        assert!(is_low_balance(&"$0.04 left · low".into()));
        assert!(is_low_balance(&"under $0.01 left · low".into()));
        assert!(!is_low_balance(&"$20.50 left".into()));
        assert!(!is_low_balance(&"12 models".into()));
    }

    #[gpui::test]
    fn test_disconnect_asks_once_and_moves_the_default_model(cx: &mut TestAppContext) {
        init_test(cx);
        let (_, unaligned) = register_key_provider("unaligned", "Unaligned", cx);
        let (openai_provider, openai) = register_key_provider("openai", "OpenAI", cx);
        unaligned.lock().stored_key = Some("sk-unal-test".into());
        openai.lock().stored_key = Some("sk-test".into());
        let openai_id = openai_provider.id();
        cx.update(|cx| {
            let model = openai_provider.default_model(cx);
            LanguageModelRegistry::global(cx).update(cx, |registry, cx| {
                registry.set_default_model(model, cx);
            });
        });
        let (form, _, cx) = open_form(cx);

        form.update(cx, |form, cx| {
            form.request_disconnect(openai_id.clone(), cx)
        });
        cx.run_until_parked();

        // Asking is not doing.
        assert_eq!(openai.lock().stored_key.as_deref(), Some("sk-test"));
        form.read_with(cx, |form, _| {
            assert_eq!(form.confirming_disconnect, Some(openai_id.clone()));
        });

        form.update(cx, |form, cx| form.cancel_disconnect(cx));
        assert_eq!(openai.lock().stored_key.as_deref(), Some("sk-test"));
        form.read_with(cx, |form, _| assert_eq!(form.confirming_disconnect, None));

        form.update(cx, |form, cx| {
            form.request_disconnect(openai_id.clone(), cx);
            form.disconnect(openai_id.clone(), cx);
        });
        cx.run_until_parked();

        assert_eq!(openai.lock().stored_key, None);
        assert_eq!(unaligned.lock().stored_key.as_deref(), Some("sk-unal-test"));
        form.read_with(cx, |form, cx| {
            assert_eq!(form.confirming_disconnect, None);
            assert_eq!(
                form.notice_text(cx).as_deref(),
                Some("OpenAI disconnected. Default model is now Fake (Unaligned).")
            );
        });
    }

    #[gpui::test]
    fn test_disconnecting_the_last_connection_promises_no_model(cx: &mut TestAppContext) {
        init_test(cx);
        let (provider, script) = register_key_provider("unaligned", "Unaligned", cx);
        script.lock().stored_key = Some("sk-unal-test".into());
        cx.update(|cx| {
            let model = provider.default_model(cx);
            LanguageModelRegistry::global(cx).update(cx, |registry, cx| {
                registry.set_default_model(model, cx);
            });
        });
        let (form, _, cx) = open_form(cx);

        form.update(cx, |form, cx| form.disconnect(UNALIGNED, cx));
        cx.run_until_parked();

        assert_eq!(script.lock().stored_key, None);
        form.read_with(cx, |form, cx| {
            assert_eq!(
                form.notice_text(cx).as_deref(),
                Some("Unaligned disconnected.")
            );
        });
    }

    #[gpui::test]
    fn test_modal_ignores_outside_clicks_while_a_key_is_checked(cx: &mut TestAppContext) {
        init_test(cx);
        let (_, script) = register_key_provider("unaligned", "Unaligned", cx);
        let (answer, gate) = oneshot::channel();
        script.lock().gate = Some(gate);
        let (modal, cx) = cx.add_window_view(ManageProvidersModal::new);
        let form = modal.read_with(cx, |modal, _| modal.form.clone());
        let decides_to_close = |modal: &Entity<ManageProvidersModal>,
                                cx: &mut VisualTestContext| {
            modal.update_in(cx, |modal, window, cx| {
                matches!(
                    modal.on_before_dismiss(window, cx),
                    DismissDecision::Dismiss(true)
                )
            })
        };

        assert!(decides_to_close(&modal, cx));

        connect_unaligned(&form, "sk-unal-slow", cx);
        cx.run_until_parked();
        assert!(!decides_to_close(&modal, cx));

        // "Done" and Escape close whatever the form is doing.
        modal.update(cx, |modal, _| modal.closing = true);
        assert!(decides_to_close(&modal, cx));
        modal.update(cx, |modal, _| modal.closing = false);

        answer.send(()).expect("the check should be waiting");
        cx.run_until_parked();
        assert!(decides_to_close(&modal, cx));
    }

    #[gpui::test]
    fn test_modal_ignores_outside_clicks_while_models_are_fetched(cx: &mut TestAppContext) {
        init_test(cx);
        register_key_provider("unaligned", "Unaligned", cx);
        let (modal, cx) = cx.add_window_view(ManageProvidersModal::new);
        let form = modal.read_with(cx, |modal, _| modal.form.clone());
        let decision = |cx: &mut VisualTestContext| {
            modal.update_in(cx, |modal, window, cx| {
                matches!(
                    modal.on_before_dismiss(window, cx),
                    DismissDecision::Dismiss(true)
                )
            })
        };

        form.update(cx, |form, _| form.custom.fetching = true);
        assert!(!decision(cx));
        form.update(cx, |form, _| form.custom.fetching = false);
        assert!(decision(cx));
    }

    #[gpui::test]
    fn test_refused_key_is_dropped_when_the_form_closed_during_the_check(cx: &mut TestAppContext) {
        init_test(cx);
        let (_, script) = register_key_provider("unaligned", "Unaligned", cx);
        script.lock().verdict = Some(ProviderConnectionStatus::NeedsAttention {
            reason: "This key was refused.".into(),
        });
        let (answer, gate) = oneshot::channel();
        script.lock().gate = Some(gate);
        let (host, cx) = cx.add_window_view(|window, cx| Host {
            form: cx.new(|cx| ConnectForm::new(window, cx)),
            reentering: false,
        });
        let form = host.read_with(cx, |host, _| host.form.clone());

        connect_unaligned(&form, "sk-unal-bad", cx);
        cx.run_until_parked();
        assert_eq!(script.lock().stored_key.as_deref(), Some("sk-unal-bad"));

        // "Done" during the check: the form is gone before the answer.
        let weak_form = form.downgrade();
        host.update_in(cx, |host, window, cx| {
            host.form = cx.new(|cx| ConnectForm::new(window, cx));
        });
        drop(form);
        cx.run_until_parked();
        assert!(weak_form.upgrade().is_none(), "the form should be released");

        answer.send(()).expect("the check should be waiting");
        cx.run_until_parked();
        assert_eq!(
            script.lock().stored_key,
            None,
            "a refused key must not stay in the keychain"
        );
    }

    #[gpui::test]
    fn test_local_row_looks_for_the_server_only_when_asked(cx: &mut TestAppContext) {
        init_test(cx);
        register_key_provider("unaligned", "Unaligned", cx);
        let (_, script) = register_key_provider("ollama", "Ollama", cx);
        script.lock().unreachable = true;
        let ollama = LanguageModelProviderId::from("ollama".to_string());
        let (form, events, cx) = open_form(cx);

        form.update_in(cx, |form, window, cx| {
            form.expand(Row::Provider(ollama.clone()), window, cx)
        });
        cx.run_until_parked();
        form.read_with(cx, |form, cx| {
            assert!(!form.is_checking(), "opening the row must not search");
            assert!(form.detections.is_empty());
            assert_eq!(form.problem, None);
            assert_eq!(
                form.local_addresses[&ollama].read(cx).text(cx),
                "localhost:11434",
                "the address comes from settings, without its scheme"
            );
        });

        form.update_in(cx, |form, window, cx| {
            form.detect(ollama.clone(), window, cx)
        });
        cx.run_until_parked();
        form.read_with(cx, |form, _| {
            assert_eq!(
                form.detections.get(&ollama),
                Some(&Detection::NoServer {
                    address: "localhost:11434".into()
                })
            );
            assert!(!form.is_checking());
        });
        assert!(events.borrow().is_empty());

        // A server that answers and has no model is not "not found".
        script.lock().unreachable = false;
        form.update_in(cx, |form, window, cx| {
            form.detect(ollama.clone(), window, cx)
        });
        cx.run_until_parked();
        form.read_with(cx, |form, _| {
            assert_eq!(
                form.detections.get(&ollama),
                Some(&Detection::NoModels {
                    address: "localhost:11434".into()
                })
            );
        });

        // The server is there: connected, and the sentence is gone.
        script.lock().stored_key = Some("models".into());
        form.update_in(cx, |form, window, cx| {
            form.detect(ollama.clone(), window, cx)
        });
        cx.run_until_parked();
        assert_eq!(*events.borrow(), ["connected"]);
        form.read_with(cx, |form, _| assert!(form.detections.is_empty()));
    }

    #[test]
    fn test_local_addresses_are_normalized() {
        assert_eq!(
            normalize_address("localhost:11434/"),
            "http://localhost:11434"
        );
        assert_eq!(
            normalize_address(" https://box.lan:8080 "),
            "https://box.lan:8080"
        );
        assert_eq!(
            spoken_address("http://localhost:1234/api/v0"),
            "localhost:1234/api/v0"
        );
    }

    #[test]
    fn test_model_lists_are_read_in_every_common_shape() {
        let ids = |body: &str| model_ids_from(body).expect("a model list");
        assert_eq!(
            ids(r#"{"object":"list","data":[{"id":"a"},{"id":"b"},{"id":"a"}]}"#),
            ["a", "b"]
        );
        assert_eq!(
            ids(r#"{"models":[{"id":"a","name":"A"},{"name":"b"}]}"#),
            ["a", "b"]
        );
        assert_eq!(ids(r#"["a",{"id":"b"}]"#), ["a", "b"]);
        assert!(ids(r#"{"data":[]}"#).is_empty());
        assert!(model_ids_from("<html>").is_err());
        assert!(model_ids_from(r#"{"error":"no"}"#).is_err());

        assert_eq!(
            models_url(false, "https://host/v1/"),
            "https://host/v1/models"
        );
        assert_eq!(models_url(true, "https://host"), "https://host/v1/models");
        assert_eq!(
            models_url(true, "https://host/v1"),
            "https://host/v1/models"
        );
    }

    #[gpui::test]
    fn test_custom_endpoint_models_are_chosen_then_required(cx: &mut TestAppContext) {
        init_test(cx);
        register_key_provider("unaligned", "Unaligned", cx);
        let (form, events, cx) = open_form(cx);
        let numbered = |count: usize| (0..count).map(|n| format!("model-{n}")).collect::<Vec<_>>();
        let problem = |form: &Entity<ConnectForm>, cx: &mut VisualTestContext| {
            form.read_with(cx, |form, _| {
                form.problem
                    .as_ref()
                    .map(|problem| problem.message.to_string())
            })
        };

        // Ten or fewer: all checked. More: none, to be chosen.
        form.update(cx, |form, _| form.set_fetched_models(numbered(10)));
        form.read_with(cx, |form, _| {
            assert_eq!(form.custom.chosen_ids(), numbered(10));
            assert_eq!(form.custom.fetched, Some(10));
        });
        form.update(cx, |form, cx| {
            form.custom.models.clear();
            form.set_fetched_models(numbered(11));
            assert!(form.custom.chosen_ids().is_empty());
            form.toggle_custom_model("model-3", cx);
        });

        // An id typed by hand joins the list, checked, and survives a fetch
        // that does not list it; what was checked stays checked.
        form.update_in(cx, |form, window, cx| {
            form.custom
                .model_id
                .update(cx, |editor, cx| editor.set_text("my-model", window, cx));
            form.add_custom_model_id(window, cx);
            form.set_fetched_models(numbered(12));
        });
        form.read_with(cx, |form, cx| {
            assert_eq!(form.custom.chosen_ids(), ["my-model", "model-3"]);
            assert_eq!(form.custom.model_id.read(cx).text(cx), "");
        });

        // Connect says what is missing, one thing at a time, and adds nothing.
        form.update_in(cx, |form, window, cx| form.connect_custom(window, cx));
        assert_eq!(
            problem(&form, cx).as_deref(),
            Some("Give this endpoint a name.")
        );
        // Typing comes first, in its own step: an edit clears the sentence
        // of the attempt before it.
        form.update_in(cx, |form, window, cx| {
            form.custom
                .name
                .update(cx, |editor, cx| editor.set_text("My server", window, cx));
        });
        form.update_in(cx, |form, window, cx| form.connect_custom(window, cx));
        assert_eq!(
            problem(&form, cx).as_deref(),
            Some("Enter the base URL, starting with http:// or https://.")
        );
        form.update_in(cx, |form, window, cx| {
            form.custom.api_url.update(cx, |editor, cx| {
                editor.set_text("http://localhost:9/v1", window, cx)
            });
            for model in &mut form.custom.models {
                model.checked = false;
            }
        });
        form.update_in(cx, |form, window, cx| form.connect_custom(window, cx));
        assert_eq!(
            problem(&form, cx).as_deref(),
            Some("Check at least one model.")
        );
        form.read_with(cx, |form, _| assert!(!form.is_checking()));
        assert!(events.borrow().is_empty());
    }

    #[gpui::test]
    fn test_agent_row_is_ready_once_set_up_and_asks_before_removing(cx: &mut TestAppContext) {
        init_test(cx);
        register_key_provider("unaligned", "Unaligned", cx);
        let claude = AgentId::new(agent_servers::CLAUDE_AGENT_ID);
        let (form, events, cx) = open_form(cx);
        let words = |form: &Entity<ConnectForm>, cx: &mut VisualTestContext| {
            form.read_with(cx, |form, cx| {
                let spec = connections::catalog(cx)
                    .into_iter()
                    .find(|spec| spec.backing == Backing::Agent(claude.clone()))
                    .expect("the catalog lists Claude Code");
                ConnectForm::state_words(&spec, &form.row_state(&spec, cx), cx).to_string()
            })
        };

        assert_eq!(words(&form, cx), "Set Up");
        assert_eq!(ConnectForm::agent_name(&claude), "Claude Code");

        // A window without a workspace has no chat to start: the row says
        // so, and nothing is reported as started.
        form.update_in(cx, |form, window, cx| {
            form.expand(Row::Agent(claude.clone()), window, cx);
            form.start_agent_chat(claude.clone(), window, cx);
        });
        form.read_with(cx, |form, _| {
            assert_eq!(
                form.problem
                    .as_ref()
                    .map(|problem| problem.message.as_ref()),
                Some("The chat is not available in this window.")
            );
            assert_eq!(form.setting_up, None);
        });
        assert!(events.borrow().is_empty());

        // While the agent is being added the row says so; once it is in the
        // settings it is ready.
        form.update(cx, |form, cx| {
            form.setting_up = Some(claude.clone());
            cx.notify();
        });
        assert_eq!(words(&form, cx), "Setting up…");
        cx.update(|_, cx| {
            SettingsStore::update_global(cx, |store, cx| {
                store
                    .set_user_settings(
                        r#"{ "agent_servers": { "claude-acp": { "type": "registry" } } }"#,
                        cx,
                    )
                    .expect("user settings should load");
            });
        });
        cx.run_until_parked();
        assert_eq!(words(&form, cx), "Ready");
        form.read_with(cx, |form, _| assert_eq!(form.setting_up, None));

        // "Remove" asks once; Cancel keeps the agent.
        form.update(cx, |form, cx| {
            form.request_agent_removal(claude.clone(), cx)
        });
        form.read_with(cx, |form, _| {
            assert_eq!(form.confirming_removal, Some(claude.clone()))
        });
        form.update(cx, |form, cx| form.cancel_agent_removal(cx));
        form.read_with(cx, |form, _| assert_eq!(form.confirming_removal, None));
        assert_eq!(words(&form, cx), "Ready");

        form.update(cx, |form, cx| {
            form.request_agent_removal(claude.clone(), cx);
            form.remove_agent(claude.clone(), cx);
        });
        form.read_with(cx, |form, cx| {
            assert_eq!(form.confirming_removal, None);
            assert_eq!(
                form.notice_text(cx).as_deref(),
                Some("Claude Code removed. Its own files and sign-in were left alone.")
            );
        });
    }
}

//! What the user can connect, in which group, and how each connection stands
//! right now: a fixed catalog joined with live state from the provider
//! registry and from settings. Nothing here is stored or drawn.
//!
//! The connection screen, the model picker, the status bar and Settings read
//! this one model, so they cannot disagree. Everything is matched by provider
//! or agent id: two providers may share a display name.

use agent_servers::{CLAUDE_AGENT_ID, CODEX_ID, GEMINI_ID};
use collections::HashMap;
use gpui::{App, Global, SharedString};
use language_model::{
    IDE_CLOUD_PROVIDER_ID, IconOrSvg, LanguageModelProvider, LanguageModelProviderId,
    LanguageModelRegistry, ProviderConnectionStatus, ProviderSettingsView,
};
use language_models::AllLanguageModelSettings;
use project::{AgentId, agent_server_store::AllAgentServersSettings};
use settings::Settings as _;
use std::sync::Arc;
use ui::IconName;

use crate::{UNALIGNED_PROVIDER_ID, show_external_agents};

/// How a connection is made, which decides the word on its button ("Connect",
/// "Sign in", "Detect", "Set up") and the form behind it.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub(crate) enum ConnectionKind {
    /// One API key.
    ApiKey,
    /// An account: OAuth in the browser, or a device code.
    SignIn,
    /// A server on the user's machine: connecting means finding it.
    Local,
    /// An OpenAI- or Anthropic-compatible endpoint described in settings.
    CustomEndpoint,
    /// An external agent that brings its own sign-in and its own models.
    Agent,
}

/// What stands behind a connection.
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub(crate) enum Backing {
    Provider(LanguageModelProviderId),
    Agent(AgentId),
}

/// The groups of the connection screen, in the order they are shown.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub(crate) enum Group {
    Unaligned,
    Subscription,
    OwnKeyOrLocal,
    More,
}

/// One row of the catalog.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct ConnectionSpec {
    pub backing: Backing,
    pub kind: ConnectionKind,
    pub group: Group,
    pub title: SharedString,
    pub note: Option<SharedString>,
    /// Providers from extensions may draw themselves with an SVG file, which
    /// this cannot hold: their rows get the generic icon, and a screen that
    /// wants the real one asks the provider.
    pub icon: IconName,
}

impl ConnectionSpec {
    /// Whether this is the row that adds a custom endpoint, as opposed to a
    /// row for one that is already configured.
    pub(crate) fn adds_custom_endpoint(&self) -> bool {
        self.kind == ConnectionKind::CustomEndpoint
            && self.backing == Backing::Provider(NEW_CUSTOM_ENDPOINT)
    }
}

/// How a connection stands.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) enum ConnectionState {
    NotConnected,
    /// A check is running. [`state`] never answers this: only the screen that
    /// started a check knows it is running, and shows this in the meantime.
    Checking,
    Connected {
        /// A short fact worth showing next to the title: "$20.50 left",
        /// "Ready".
        detail: Option<SharedString>,
    },
    NeedsAttention {
        /// One sentence, shown as is.
        reason: SharedString,
    },
    /// Connected with a key read from an environment variable: there is no key
    /// to type, and a key typed anyway would not be the one used.
    FromEnvironment {
        var: SharedString,
    },
}

impl ConnectionState {
    /// Whether requests can be sent through this connection.
    pub(crate) fn is_connected(&self) -> bool {
        matches!(self, Self::Connected { .. } | Self::FromEnvironment { .. })
    }
}

/// The backing of the "Custom endpoint" row that adds one. No real provider
/// can have this id: a custom endpoint's id is its name, and the catalog skips
/// an endpoint without a name.
const NEW_CUSTOM_ENDPOINT: LanguageModelProviderId = LanguageModelProviderId::new("");

struct KnownProvider {
    id: &'static str,
    title: &'static str,
    note: Option<&'static str>,
    kind: ConnectionKind,
}

const fn known(id: &'static str, title: &'static str, kind: ConnectionKind) -> KnownProvider {
    KnownProvider {
        id,
        title,
        note: None,
        kind,
    }
}

const UNALIGNED: KnownProvider = KnownProvider {
    id: UNALIGNED_PROVIDER_ID,
    title: "Unaligned",
    note: Some("One key for every model. Pay as you go, no subscription."),
    kind: ConnectionKind::ApiKey,
};

/// Subscriptions that are model providers: they answer in the same chat, with
/// the same tools, as any key.
const SUBSCRIPTION_PROVIDERS: &[KnownProvider] = &[
    known(
        "openai-subscribed",
        "ChatGPT Plus or Pro",
        ConnectionKind::SignIn,
    ),
    known("copilot_chat", "GitHub Copilot", ConnectionKind::SignIn),
    known("x_ai_subscribed", "SuperGrok", ConnectionKind::SignIn),
];

struct KnownAgent {
    id: &'static str,
    title: &'static str,
    note: &'static str,
    icon: IconName,
}

/// Subscriptions that are only reachable through their vendor's own agent.
const SUBSCRIPTION_AGENTS: &[KnownAgent] = &[
    KnownAgent {
        id: CLAUDE_AGENT_ID,
        title: "Claude Pro or Max",
        note: "through Claude Code",
        icon: IconName::AiClaude,
    },
    KnownAgent {
        id: CODEX_ID,
        title: "ChatGPT",
        note: "through Codex",
        icon: IconName::AiOpenAi,
    },
    KnownAgent {
        id: GEMINI_ID,
        title: "Gemini",
        note: "through Gemini CLI",
        icon: IconName::AiGemini,
    },
];

const OWN_KEY_PROVIDERS: &[KnownProvider] = &[
    known("anthropic", "Anthropic", ConnectionKind::ApiKey),
    known("openai", "OpenAI", ConnectionKind::ApiKey),
    known("google", "Google", ConnectionKind::ApiKey),
    known("openrouter", "OpenRouter", ConnectionKind::ApiKey),
];

/// Keys fewer people have: folded under "More" with every other registered
/// provider, and listed first there.
const MORE_KEY_PROVIDERS: &[KnownProvider] = &[
    known("deepseek", "DeepSeek", ConnectionKind::ApiKey),
    known("mistral", "Mistral", ConnectionKind::ApiKey),
    known("x_ai", "xAI", ConnectionKind::ApiKey),
];

const LOCAL_PROVIDERS: &[KnownProvider] = &[
    known("ollama", "Ollama", ConnectionKind::Local),
    known("lmstudio", "LM Studio", ConnectionKind::Local),
    known("llama.cpp", "llama.cpp", ConnectionKind::Local),
];

/// Shown next to an agent that has an entry in the `agent_servers` setting.
/// Whether it is signed in is only known inside a chat with it.
const AGENT_SET_UP: &str = "Ready";

/// What a provider's settings view says about its connection.
struct ProviderFacts {
    kind: ConnectionKind,
    /// The environment variable the key comes from, when it does.
    key_environment_variable: Option<SharedString>,
}

impl ProviderFacts {
    fn read(provider: &dyn LanguageModelProvider, cx: &mut App) -> Self {
        match provider.settings_view(cx) {
            Some(ProviderSettingsView::ApiKey(key)) => Self {
                kind: ConnectionKind::ApiKey,
                key_environment_variable: key.is_from_env_var.then_some(key.env_var_name),
            },
            Some(ProviderSettingsView::Inline(_)) => Self {
                kind: ConnectionKind::SignIn,
                key_environment_variable: None,
            },
            // A page of its own asks for credentials the user owns (AWS keys,
            // a gateway key): closest to a key.
            Some(ProviderSettingsView::SubPage(_)) | None => Self {
                kind: ConnectionKind::ApiKey,
                key_environment_variable: None,
            },
        }
    }
}

/// [`ProviderFacts`] per registered provider. A settings view can only be
/// asked for with `&mut App`, while the catalog and its states are read with
/// `&App` during rendering, so the answers are kept here and refreshed
/// whenever the registry says a provider changed.
#[derive(Default)]
struct ProviderFactsCache(HashMap<LanguageModelProviderId, ProviderFacts>);

impl Global for ProviderFactsCache {}

/// Starts following the provider registry, which must be installed. Without
/// this the catalog still works, but no state is ever
/// [`ConnectionState::FromEnvironment`].
pub(crate) fn init(cx: &mut App) {
    let registry = LanguageModelRegistry::global(cx);
    cx.set_global(ProviderFactsCache::default());
    for provider in registry.read(cx).providers() {
        refresh_provider_facts(&provider.id(), cx);
    }
    cx.subscribe(
        &registry,
        |_, event: &language_model::Event, cx| match event {
            language_model::Event::ProviderStateChanged(id)
            | language_model::Event::AddedProvider(id)
            | language_model::Event::RemovedProvider(id) => refresh_provider_facts(id, cx),
            _ => {}
        },
    )
    .detach();
}

fn refresh_provider_facts(id: &LanguageModelProviderId, cx: &mut App) {
    let provider = LanguageModelRegistry::read_global(cx).provider(id);
    let facts = provider.map(|provider| ProviderFacts::read(provider.as_ref(), cx));
    let cache = cx.default_global::<ProviderFactsCache>();
    match facts {
        Some(facts) => {
            cache.0.insert(id.clone(), facts);
        }
        None => {
            cache.0.remove(id);
        }
    }
}

fn provider_facts<'a>(id: &LanguageModelProviderId, cx: &'a App) -> Option<&'a ProviderFacts> {
    cx.try_global::<ProviderFactsCache>()?.0.get(id)
}

fn provider_icon(provider: &dyn LanguageModelProvider) -> IconName {
    match provider.icon() {
        IconOrSvg::Icon(icon) => icon,
        IconOrSvg::Svg(_) => IconName::AppAssistant,
    }
}

/// Everything the user can connect, in the order it is shown: Unaligned, the
/// subscriptions (providers, then agents), the four keys most people have,
/// local models, the custom endpoints from settings and the row that adds
/// one, then under "More" three further keys and every other registered
/// provider by name.
///
/// A provider is listed only while it is registered and visible. Agents are
/// listed whether or not they are set up, unless [`show_external_agents`] is
/// off. The Zed hosted provider is never listed.
pub(crate) fn catalog(cx: &App) -> Vec<ConnectionSpec> {
    let mut providers: Vec<Arc<dyn LanguageModelProvider>> = LanguageModelRegistry::read_global(cx)
        .visible_providers()
        .into_iter()
        .filter(|provider| provider.id() != IDE_CLOUD_PROVIDER_ID)
        .collect();
    let mut take = |id: &str| {
        let index = providers
            .iter()
            .position(|provider| provider.id().0.as_ref() == id)?;
        Some(providers.swap_remove(index))
    };
    let mut known_specs = |known: &[KnownProvider], group: Group| {
        known
            .iter()
            .filter_map(|known| {
                let provider = take(known.id)?;
                Some(ConnectionSpec {
                    backing: Backing::Provider(provider.id()),
                    kind: known.kind,
                    group,
                    title: SharedString::new_static(known.title),
                    note: known.note.map(SharedString::new_static),
                    icon: provider_icon(provider.as_ref()),
                })
            })
            .collect::<Vec<_>>()
    };

    let mut catalog = known_specs(&[UNALIGNED], Group::Unaligned);

    catalog.extend(known_specs(SUBSCRIPTION_PROVIDERS, Group::Subscription));
    if show_external_agents() {
        catalog.extend(SUBSCRIPTION_AGENTS.iter().map(|agent| ConnectionSpec {
            backing: Backing::Agent(AgentId::new(agent.id)),
            kind: ConnectionKind::Agent,
            group: Group::Subscription,
            title: SharedString::new_static(agent.title),
            note: Some(SharedString::new_static(agent.note)),
            icon: agent.icon,
        }));
    }

    catalog.extend(known_specs(OWN_KEY_PROVIDERS, Group::OwnKeyOrLocal));
    catalog.extend(known_specs(LOCAL_PROVIDERS, Group::OwnKeyOrLocal));
    let more_keys = known_specs(MORE_KEY_PROVIDERS, Group::More);
    for (name, icon) in custom_endpoints(cx) {
        let id = LanguageModelProviderId(SharedString::from(name));
        // An endpoint named like a built-in provider replaces it in the
        // registry, and already has that provider's row.
        if catalog
            .iter()
            .any(|spec| spec.backing == Backing::Provider(id.clone()))
        {
            continue;
        }
        let icon = take(id.0.as_ref()).map_or(icon, |provider| provider_icon(provider.as_ref()));
        catalog.push(ConnectionSpec {
            title: id.0.clone(),
            backing: Backing::Provider(id),
            kind: ConnectionKind::CustomEndpoint,
            group: Group::OwnKeyOrLocal,
            note: None,
            icon,
        });
    }
    catalog.push(ConnectionSpec {
        backing: Backing::Provider(NEW_CUSTOM_ENDPOINT),
        kind: ConnectionKind::CustomEndpoint,
        group: Group::OwnKeyOrLocal,
        title: SharedString::new_static("Custom endpoint"),
        note: None,
        icon: IconName::Plus,
    });

    catalog.extend(more_keys);
    providers.sort_by_cached_key(|provider| (provider.name().0.to_lowercase(), provider.id()));
    catalog.extend(providers.into_iter().map(|provider| {
        let id = provider.id();
        ConnectionSpec {
            kind: provider_facts(&id, cx).map_or(ConnectionKind::ApiKey, |facts| facts.kind),
            backing: Backing::Provider(id),
            group: Group::More,
            title: provider.name().0,
            note: None,
            icon: provider_icon(provider.as_ref()),
        }
    }));

    catalog
}

/// The custom endpoints in settings, by name, each with the icon of its kind.
fn custom_endpoints(cx: &App) -> Vec<(Arc<str>, IconName)> {
    let Some(settings) = AllLanguageModelSettings::try_get(cx) else {
        return Vec::new();
    };
    let openai_compatible = settings
        .openai_compatible
        .keys()
        .map(|name| (name.clone(), IconName::AiOpenAiCompat));
    // An Anthropic-compatible endpoint named like an OpenAI-compatible one is
    // not registered.
    let anthropic_compatible = settings
        .anthropic_compatible
        .keys()
        .filter(|name| !settings.openai_compatible.contains_key(*name))
        .map(|name| (name.clone(), IconName::AiAnthropicCompat));
    let mut endpoints = openai_compatible
        .chain(anthropic_compatible)
        .filter(|(name, _)| !name.is_empty())
        .collect::<Vec<_>>();
    endpoints.sort_by_cached_key(|(name, _)| (name.to_lowercase(), name.clone()));
    endpoints
}

/// How the connection of `spec` stands right now.
///
/// A provider answers for itself through
/// [`LanguageModelProvider::connection_status`]; when it is connected with a
/// key read from the environment, the answer is
/// [`ConnectionState::FromEnvironment`] instead. A connection that needs
/// attention says so even when its key comes from the environment (see
/// [`key_environment_variable`]). An agent is connected once it has an entry in
/// the `agent_servers` setting.
pub(crate) fn state(spec: &ConnectionSpec, cx: &App) -> ConnectionState {
    match &spec.backing {
        Backing::Provider(id) => {
            let Some(provider) = LanguageModelRegistry::read_global(cx).provider(id) else {
                return ConnectionState::NotConnected;
            };
            match provider.connection_status(cx) {
                ProviderConnectionStatus::NotConnected => ConnectionState::NotConnected,
                ProviderConnectionStatus::NeedsAttention { reason } => {
                    ConnectionState::NeedsAttention { reason }
                }
                ProviderConnectionStatus::Connected { detail } => {
                    match key_environment_variable(spec, cx) {
                        Some(var) => ConnectionState::FromEnvironment { var },
                        None => ConnectionState::Connected { detail },
                    }
                }
            }
        }
        Backing::Agent(id) => {
            let set_up = AllAgentServersSettings::try_get(cx)
                .is_some_and(|agents| agents.contains_key(id.0.as_ref()));
            if set_up {
                ConnectionState::Connected {
                    detail: Some(SharedString::new_static(AGENT_SET_UP)),
                }
            } else {
                ConnectionState::NotConnected
            }
        }
    }
}

/// The environment variable the key of `spec` comes from, when its provider's
/// settings view says it does. While this is set a key typed into a form is
/// stored but not used, whatever [`state`] answers.
pub(crate) fn key_environment_variable(spec: &ConnectionSpec, cx: &App) -> Option<SharedString> {
    let Backing::Provider(id) = &spec.backing else {
        return None;
    };
    provider_facts(id, cx)?.key_environment_variable.clone()
}

/// How many rows of the catalog are connected, agents that are set up
/// included. Rows that need attention do not count.
pub(crate) fn connected_count(cx: &App) -> usize {
    catalog(cx)
        .iter()
        .filter(|spec| state(spec, cx).is_connected())
        .count()
}

/// Whether a subscription can answer right now: a signed-in account, or an
/// agent that is set up. The model picker uses this to offer subscriptions
/// next to connected models.
pub(crate) fn subscription_ready(cx: &App) -> bool {
    catalog(cx)
        .iter()
        .any(|spec| spec.group == Group::Subscription && state(spec, cx).is_connected())
}

#[cfg(test)]
mod tests {
    use super::*;
    use futures::future::BoxFuture;
    use gpui::{AsyncApp, Entity, Task, TestAppContext, UpdateGlobal as _};
    use language_model::{
        ApiKeyConfiguration, AuthenticateError, LanguageModel, LanguageModelClient,
        LanguageModelCompletionError, LanguageModelCompletionStream, LanguageModelProviderName,
        LanguageModelProviderState, LanguageModelRequest, fake_provider::FakeLanguageModelProvider,
    };
    use settings::SettingsStore;

    fn init_test(cx: &mut TestAppContext) -> Arc<FakeLanguageModelProvider> {
        cx.update(|cx| {
            let store = SettingsStore::test(cx);
            cx.set_global(store);
            AllLanguageModelSettings::register(cx);
            AllAgentServersSettings::register(cx);
            let fake = LanguageModelRegistry::test(cx);
            init(cx);
            fake
        })
    }

    fn register_fake(id: &str, name: &str, cx: &mut TestAppContext) {
        let provider = Arc::new(FakeLanguageModelProvider::new(
            LanguageModelProviderId::from(id.to_string()),
            LanguageModelProviderName::from(name.to_string()),
        ));
        register(provider, cx);
    }

    fn register<T: LanguageModelProvider + LanguageModelProviderState>(
        provider: Arc<T>,
        cx: &mut TestAppContext,
    ) {
        cx.update(|cx| {
            LanguageModelRegistry::global(cx).update(cx, |registry, cx| {
                registry.register_provider(provider, cx);
            });
        });
    }

    fn set_user_settings(json: &str, cx: &mut TestAppContext) {
        cx.update(|cx| {
            SettingsStore::update_global(cx, |store, cx| {
                store
                    .set_user_settings(json, cx)
                    .expect("user settings should load");
            });
        });
    }

    fn spec_for_provider(id: &str, cx: &App) -> ConnectionSpec {
        let backing = Backing::Provider(LanguageModelProviderId::from(id.to_string()));
        catalog(cx)
            .into_iter()
            .find(|spec| spec.backing == backing)
            .unwrap_or_else(|| panic!("no row for provider {id}"))
    }

    /// A provider whose connection is one API key, with the status and the
    /// key's origin chosen by the test.
    struct KeyProvider {
        fake: FakeLanguageModelProvider,
        key_from_environment: bool,
        status: ProviderConnectionStatus,
    }

    impl KeyProvider {
        fn new(id: &str, key_from_environment: bool, status: ProviderConnectionStatus) -> Self {
            Self {
                fake: FakeLanguageModelProvider::new(
                    LanguageModelProviderId::from(id.to_string()),
                    LanguageModelProviderName::from(id.to_string()),
                ),
                key_from_environment,
                status,
            }
        }

        fn has_key(&self) -> bool {
            self.status != ProviderConnectionStatus::NotConnected
        }
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
            self.has_key()
        }

        fn authenticate(&self, _: &mut App) -> Task<Result<(), AuthenticateError>> {
            Task::ready(Ok(()))
        }

        fn settings_view(&self, _: &mut App) -> Option<ProviderSettingsView> {
            Some(ProviderSettingsView::ApiKey(ApiKeyConfiguration::new(
                self.has_key(),
                self.key_from_environment,
                "TEST_API_KEY".into(),
                "https://example.com/keys".into(),
            )))
        }

        fn connection_status(&self, _: &App) -> ProviderConnectionStatus {
            self.status.clone()
        }
    }

    #[gpui::test]
    fn test_groups_and_order(cx: &mut TestAppContext) {
        init_test(cx);
        // Registered out of order, and under display names that differ from
        // the catalog's titles: only ids decide where a provider goes.
        for (id, name) in [
            ("vercel_ai_gateway", "Vercel AI Gateway"),
            ("llama.cpp", "llama.cpp server"),
            ("x_ai", "xAI key"),
            ("copilot_chat", "GitHub Copilot Chat"),
            ("openai", "OpenAI key"),
            ("amazon-bedrock", "Amazon Bedrock"),
            ("unaligned", "Unaligned API"),
            ("lmstudio", "LM Studio server"),
            ("mistral", "Mistral key"),
            ("x_ai_subscribed", "SuperGrok account"),
            ("openrouter", "OpenRouter key"),
            ("anthropic", "Anthropic key"),
            ("zed.dev", "Zed"),
            ("ollama", "Ollama server"),
            ("deepseek", "DeepSeek key"),
            ("openai-subscribed", "ChatGPT Subscription"),
            ("google", "Google AI"),
            ("opencode", "OpenCode"),
        ] {
            register_fake(id, name, cx);
        }
        set_user_settings(
            r#"{
                "language_models": {
                    "openai_compatible": {
                        "Work server": { "api_url": "http://localhost:1/v1", "available_models": [] }
                    },
                    "anthropic_compatible": {
                        "home server": { "api_url": "http://localhost:2", "available_models": [] },
                        "Work server": { "api_url": "http://localhost:3", "available_models": [] }
                    }
                }
            }"#,
            cx,
        );

        cx.update(|cx| {
            let rows = catalog(cx)
                .into_iter()
                .map(|spec| {
                    let id = match &spec.backing {
                        Backing::Provider(id) => id.0.to_string(),
                        Backing::Agent(id) => id.0.to_string(),
                    };
                    (spec.group, spec.kind, id, spec.title.to_string())
                })
                .collect::<Vec<_>>();
            let row = |group, kind, id: &str, title: &str| (group, kind, id.into(), title.into());
            pretty_assertions::assert_eq!(
                rows,
                vec![
                    row(
                        Group::Unaligned,
                        ConnectionKind::ApiKey,
                        "unaligned",
                        "Unaligned"
                    ),
                    row(
                        Group::Subscription,
                        ConnectionKind::SignIn,
                        "openai-subscribed",
                        "ChatGPT Plus or Pro"
                    ),
                    row(
                        Group::Subscription,
                        ConnectionKind::SignIn,
                        "copilot_chat",
                        "GitHub Copilot"
                    ),
                    row(
                        Group::Subscription,
                        ConnectionKind::SignIn,
                        "x_ai_subscribed",
                        "SuperGrok"
                    ),
                    row(
                        Group::Subscription,
                        ConnectionKind::Agent,
                        "claude-acp",
                        "Claude Pro or Max"
                    ),
                    row(
                        Group::Subscription,
                        ConnectionKind::Agent,
                        "codex-acp",
                        "ChatGPT"
                    ),
                    row(
                        Group::Subscription,
                        ConnectionKind::Agent,
                        "gemini",
                        "Gemini"
                    ),
                    row(
                        Group::OwnKeyOrLocal,
                        ConnectionKind::ApiKey,
                        "anthropic",
                        "Anthropic"
                    ),
                    row(
                        Group::OwnKeyOrLocal,
                        ConnectionKind::ApiKey,
                        "openai",
                        "OpenAI"
                    ),
                    row(
                        Group::OwnKeyOrLocal,
                        ConnectionKind::ApiKey,
                        "google",
                        "Google"
                    ),
                    row(
                        Group::OwnKeyOrLocal,
                        ConnectionKind::ApiKey,
                        "openrouter",
                        "OpenRouter"
                    ),
                    row(
                        Group::OwnKeyOrLocal,
                        ConnectionKind::Local,
                        "ollama",
                        "Ollama"
                    ),
                    row(
                        Group::OwnKeyOrLocal,
                        ConnectionKind::Local,
                        "lmstudio",
                        "LM Studio"
                    ),
                    row(
                        Group::OwnKeyOrLocal,
                        ConnectionKind::Local,
                        "llama.cpp",
                        "llama.cpp"
                    ),
                    row(
                        Group::OwnKeyOrLocal,
                        ConnectionKind::CustomEndpoint,
                        "home server",
                        "home server"
                    ),
                    row(
                        Group::OwnKeyOrLocal,
                        ConnectionKind::CustomEndpoint,
                        "Work server",
                        "Work server"
                    ),
                    row(
                        Group::OwnKeyOrLocal,
                        ConnectionKind::CustomEndpoint,
                        "",
                        "Custom endpoint"
                    ),
                    row(Group::More, ConnectionKind::ApiKey, "deepseek", "DeepSeek"),
                    row(Group::More, ConnectionKind::ApiKey, "mistral", "Mistral"),
                    row(Group::More, ConnectionKind::ApiKey, "x_ai", "xAI"),
                    row(
                        Group::More,
                        ConnectionKind::ApiKey,
                        "amazon-bedrock",
                        "Amazon Bedrock"
                    ),
                    row(Group::More, ConnectionKind::ApiKey, "fake", "Fake"),
                    row(Group::More, ConnectionKind::ApiKey, "opencode", "OpenCode"),
                    row(
                        Group::More,
                        ConnectionKind::ApiKey,
                        "vercel_ai_gateway",
                        "Vercel AI Gateway"
                    ),
                ]
            );

            let catalog = catalog(cx);
            assert_eq!(
                catalog[0].note.as_deref(),
                Some("One key for every model. Pay as you go, no subscription.")
            );
            let notes = catalog
                .iter()
                .filter(|spec| spec.kind == ConnectionKind::Agent)
                .map(|spec| spec.note.as_deref())
                .collect::<Vec<_>>();
            assert_eq!(
                notes,
                [
                    Some("through Claude Code"),
                    Some("through Codex"),
                    Some("through Gemini CLI")
                ]
            );
            assert_eq!(
                catalog
                    .iter()
                    .filter(|spec| spec.adds_custom_endpoint())
                    .count(),
                1
            );
        });
    }

    #[gpui::test]
    fn test_providers_are_matched_by_id_not_by_name(cx: &mut TestAppContext) {
        init_test(cx);
        register_fake("anthropic", "Claude API", cx);
        register_fake("acme-extension", "Anthropic", cx);
        register_fake("mirror", "Unaligned", cx);

        cx.update(|cx| {
            let anthropic = spec_for_provider("anthropic", cx);
            assert_eq!(anthropic.group, Group::OwnKeyOrLocal);
            assert_eq!(anthropic.title.as_ref(), "Anthropic");

            let namesake = spec_for_provider("acme-extension", cx);
            assert_eq!(namesake.group, Group::More);
            assert_eq!(namesake.title.as_ref(), "Anthropic");

            assert_eq!(spec_for_provider("mirror", cx).group, Group::More);
            assert!(
                catalog(cx)
                    .iter()
                    .all(|spec| spec.group != Group::Unaligned),
                "a provider named Unaligned is not the Unaligned provider"
            );
        });
    }

    #[gpui::test]
    fn test_authenticated_provider_is_connected(cx: &mut TestAppContext) {
        let fake = init_test(cx);

        cx.update(|cx| {
            assert!(fake.is_authenticated(cx));
            assert_eq!(
                state(&spec_for_provider("fake", cx), cx),
                ConnectionState::Connected { detail: None }
            );
            assert_eq!(connected_count(cx), 1);

            let unregistered = ConnectionSpec {
                backing: Backing::Provider(LanguageModelProviderId::new("gone")),
                kind: ConnectionKind::ApiKey,
                group: Group::More,
                title: "Gone".into(),
                note: None,
                icon: IconName::AppAssistant,
            };
            assert_eq!(state(&unregistered, cx), ConnectionState::NotConnected);
        });
    }

    #[gpui::test]
    fn test_provider_status_and_environment_key(cx: &mut TestAppContext) {
        init_test(cx);
        let balance = ProviderConnectionStatus::Connected {
            detail: Some("$20.50 left".into()),
        };
        let refused = ProviderConnectionStatus::NeedsAttention {
            reason: "This key was refused.".into(),
        };
        for (id, key_from_environment, status) in [
            ("typed-key", false, balance.clone()),
            ("environment-key", true, balance),
            ("refused-key", false, refused.clone()),
            ("refused-environment-key", true, refused),
            ("no-key", false, ProviderConnectionStatus::NotConnected),
        ] {
            register(
                Arc::new(KeyProvider::new(id, key_from_environment, status)),
                cx,
            );
        }

        cx.update(|cx| {
            let state_of = |id: &str| state(&spec_for_provider(id, cx), cx);
            assert_eq!(
                state_of("typed-key"),
                ConnectionState::Connected {
                    detail: Some("$20.50 left".into())
                }
            );
            assert_eq!(
                state_of("environment-key"),
                ConnectionState::FromEnvironment {
                    var: "TEST_API_KEY".into()
                }
            );
            assert_eq!(
                state_of("refused-key"),
                ConnectionState::NeedsAttention {
                    reason: "This key was refused.".into()
                }
            );
            assert_eq!(
                state_of("refused-environment-key"),
                ConnectionState::NeedsAttention {
                    reason: "This key was refused.".into()
                }
            );
            assert_eq!(
                key_environment_variable(&spec_for_provider("refused-environment-key", cx), cx)
                    .as_deref(),
                Some("TEST_API_KEY")
            );
            assert_eq!(state_of("no-key"), ConnectionState::NotConnected);
            assert_eq!(
                spec_for_provider("typed-key", cx).kind,
                ConnectionKind::ApiKey
            );

            // The fake from `init_test`, the typed key and the environment key.
            assert_eq!(connected_count(cx), 3);
        });

        cx.update(|cx| {
            LanguageModelRegistry::global(cx).update(cx, |registry, cx| {
                registry.unregister_provider(LanguageModelProviderId::new("environment-key"), cx);
            });
        });
        cx.update(|cx| {
            assert!(
                provider_facts(&LanguageModelProviderId::new("environment-key"), cx).is_none(),
                "facts about a removed provider are dropped"
            );
            assert_eq!(connected_count(cx), 2);
        });
    }

    #[gpui::test]
    fn test_agent_is_connected_once_set_up(cx: &mut TestAppContext) {
        init_test(cx);
        let agent_states = |cx: &App| {
            catalog(cx)
                .iter()
                .filter(|spec| spec.kind == ConnectionKind::Agent)
                .map(|spec| state(spec, cx))
                .collect::<Vec<_>>()
        };

        cx.update(|cx| {
            assert_eq!(
                agent_states(cx),
                [
                    ConnectionState::NotConnected,
                    ConnectionState::NotConnected,
                    ConnectionState::NotConnected
                ]
            );
            assert_eq!(connected_count(cx), 1);
            assert!(!subscription_ready(cx));
        });

        set_user_settings(
            r#"{ "agent_servers": { "codex-acp": { "type": "registry" } } }"#,
            cx,
        );

        cx.update(|cx| {
            assert_eq!(
                agent_states(cx),
                [
                    ConnectionState::NotConnected,
                    ConnectionState::Connected {
                        detail: Some("Ready".into())
                    },
                    ConnectionState::NotConnected
                ]
            );
            assert_eq!(connected_count(cx), 2);
            assert!(subscription_ready(cx));
        });
    }
}

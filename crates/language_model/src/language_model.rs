mod api_key;
mod registry;
mod request;

#[cfg(any(test, feature = "test-support"))]
pub mod fake_provider;

pub use language_model_core::*;

use anyhow::Result;
use futures::FutureExt;
use futures::{StreamExt, future::BoxFuture, stream::BoxStream};
use gpui::{AnyView, App, AsyncApp, BackgroundExecutor, Task, Window};
use icons::IconName;
use parking_lot::Mutex;
use std::sync::Arc;

pub type CreateProviderSettingsView = Arc<dyn Fn(&mut Window, &mut App) -> AnyView + 'static>;

pub use crate::api_key::{ApiKey, ApiKeyState, read_credentials, set_credentials_read_gate};
pub use crate::registry::*;
pub use crate::request::{LanguageModelImageExt, gpui_size_to_image_size, image_size_to_gpui};
pub use env_var::{EnvVar, env_var};

const BACKGROUND_STREAM_BUFFER_SIZE: usize = 32;

pub fn init(cx: &mut App) {
    registry::init(cx);
}

pub fn stream_in_background<Output>(
    mut events: BoxStream<'static, Output>,
    executor: BackgroundExecutor,
) -> BoxStream<'static, Output>
where
    Output: Send + 'static,
{
    // `futures::channel::mpsc` parks a full sender behind a `std` mutex that the
    // receiver also locks, which traps when the receiver polls on the browser's
    // main thread.
    let (sender, receiver) = async_channel::bounded(BACKGROUND_STREAM_BUFFER_SIZE);
    let task = executor.spawn(async move {
        while let Some(event) = events.next().await {
            if sender.send(event).await.is_err() {
                return;
            }
        }
    });

    receiver
        .map(move |event| {
            let _task = &task;
            event
        })
        .boxed()
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DisabledReason(pub SharedString);

impl DisabledReason {
    pub fn new(reason: impl Into<SharedString>) -> Self {
        Self(reason.into())
    }
}

/// The outcome of an explicit [`LanguageModel::compact`] request.
#[derive(Debug, Clone, PartialEq)]
pub struct CompactionResult {
    /// The replacement context to persist and use in subsequent requests.
    pub context: CompactedContext,
    /// Token usage of the compaction request itself, as reported by the
    /// provider.
    pub usage: TokenUsage,
}

pub struct LanguageModelTextStream {
    pub message_id: Option<String>,
    pub stream: BoxStream<'static, Result<String, LanguageModelCompletionError>>,
    // Has complete token usage after the stream has finished
    pub last_token_usage: Arc<Mutex<TokenUsage>>,
}

impl Default for LanguageModelTextStream {
    fn default() -> Self {
        Self {
            message_id: None,
            stream: Box::pin(futures::stream::empty()),
            last_token_usage: Arc::new(Mutex::new(TokenUsage::default())),
        }
    }
}

/// A language model offered by a [`LanguageModelProvider`].
///
/// This is plain data: identity, capabilities, and limits. Everything that
/// talks to a backend (streaming completions, counting tokens, compaction)
/// lives on the provider, which is handed the model it should serve and
/// resolves its own configuration for it by `id`. Find that provider with
/// [`LanguageModelRegistry::provider_for_model`].
#[derive(Clone, Debug, PartialEq)]
pub struct LanguageModel {
    /// Identifies the model within its provider. Stable across provider
    /// refreshes; the name and capabilities may change.
    pub id: LanguageModelId,
    pub name: LanguageModelName,
    pub provider_id: LanguageModelProviderId,
    pub provider_name: LanguageModelProviderName,
    /// The provider that ultimately serves requests, when it differs from
    /// `provider_id` (for example, a model offered through a gateway).
    pub upstream_provider_id: Option<LanguageModelProviderId>,
    pub upstream_provider_name: Option<LanguageModelProviderName>,
    pub telemetry_id: SharedString,
    /// Whether this model is the "latest", so we can highlight it in the UI.
    pub is_latest: bool,
    /// Why the model is currently disabled, if it is.
    pub disabled_reason: Option<DisabledReason>,
    /// Whether requests to this model require the user to consent to the
    /// upstream provider retaining inference logs (i.e. the model cannot be
    /// offered with Zero Data Retention).
    pub requires_data_retention: bool,
    /// When this model refuses a request, the model ID to fall back to (same provider).
    pub refusal_fallback_model_id: Option<&'static str>,
    /// Information about the cost of using this model, if available.
    pub cost_info: Option<LanguageModelCostInfo>,
    pub supports_thinking: bool,
    /// Whether thinking can be turned off entirely for this model. Some
    /// models (e.g. Claude Fable 5) always think and cannot honor an "off"
    /// request. Only meaningful when `supports_thinking` is `true`.
    pub supports_disabling_thinking: bool,
    pub supports_fast_mode: bool,
    /// The effort levels that can be used when thinking.
    pub supported_effort_levels: Arc<[LanguageModelEffortLevel]>,
    /// Whether this model supports provider-side automatic context
    /// compaction (requested via `LanguageModelRequest::compact_at_tokens`).
    pub supports_server_side_compaction: bool,
    pub supports_explicit_compaction: bool,
    /// Whether native compaction honors `LanguageModelRequest::max_output_tokens`.
    pub supports_explicit_compaction_output_limit: bool,
    /// The provider-enforced input size required for explicit compaction.
    pub minimum_explicit_compaction_input_tokens: Option<u64>,
    /// How Zed builds prompt-based compaction requests. This does not affect
    /// provider-native compaction.
    pub prompt_compaction_strategy: PromptCompactionStrategy,
    pub supports_images: bool,
    pub supports_tools: bool,
    pub tool_choice_support: LanguageModelToolChoiceSupport,
    /// Whether this model or provider supports streaming tool calls.
    pub supports_streaming_tools: bool,
    /// Whether this model/provider reports accurate split input/output token
    /// counts. When true, the UI may show separate input/output token indicators.
    pub supports_split_token_display: bool,
    /// The model's context-window capacity.
    pub max_token_count: u64,
    /// The input ceiling before reserving output from any shared window.
    /// Equals `max_token_count` unless the model has a separate prompt limit.
    pub max_input_tokens: u64,
    pub max_output_tokens: Option<u64>,
}

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub enum PromptCompactionStrategy {
    #[default]
    RebuildPrompt,
    /// Preserve tools, system instructions, conversation history, and reasoning
    /// settings while disabling tool calls. Providers must only opt in when
    /// `tool_choice: none` is enforced without invalidating that cached prefix.
    PreserveRequestPrefix,
}

/// Which [`LanguageModelToolChoice`] values a model accepts.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct LanguageModelToolChoiceSupport {
    pub auto: bool,
    pub any: bool,
    pub none: bool,
}

impl LanguageModelToolChoiceSupport {
    pub const ALL: Self = Self {
        auto: true,
        any: true,
        none: true,
    };

    pub fn supports(&self, choice: LanguageModelToolChoice) -> bool {
        match choice {
            LanguageModelToolChoice::Auto => self.auto,
            LanguageModelToolChoice::Any => self.any,
            LanguageModelToolChoice::None => self.none,
        }
    }
}

impl LanguageModel {
    /// Creates a model with the given identity and conservative defaults for
    /// every capability. Providers override the capabilities they support
    /// with struct update syntax.
    pub fn new(
        id: LanguageModelId,
        name: LanguageModelName,
        provider_id: LanguageModelProviderId,
        provider_name: LanguageModelProviderName,
        telemetry_id: impl Into<SharedString>,
        max_token_count: u64,
    ) -> Self {
        Self {
            id,
            name,
            provider_id,
            provider_name,
            upstream_provider_id: None,
            upstream_provider_name: None,
            telemetry_id: telemetry_id.into(),
            is_latest: false,
            disabled_reason: None,
            requires_data_retention: false,
            refusal_fallback_model_id: None,
            cost_info: None,
            supports_thinking: false,
            supports_disabling_thinking: true,
            supports_fast_mode: false,
            supported_effort_levels: Arc::default(),
            supports_server_side_compaction: false,
            supports_explicit_compaction: false,
            supports_explicit_compaction_output_limit: false,
            minimum_explicit_compaction_input_tokens: None,
            prompt_compaction_strategy: PromptCompactionStrategy::default(),
            supports_images: false,
            supports_tools: false,
            tool_choice_support: LanguageModelToolChoiceSupport::default(),
            supports_streaming_tools: false,
            supports_split_token_display: false,
            max_token_count,
            max_input_tokens: max_token_count,
            max_output_tokens: None,
        }
    }

    pub fn id(&self) -> LanguageModelId {
        self.id.clone()
    }

    pub fn name(&self) -> LanguageModelName {
        self.name.clone()
    }

    pub fn provider_id(&self) -> LanguageModelProviderId {
        self.provider_id.clone()
    }

    pub fn provider_name(&self) -> LanguageModelProviderName {
        self.provider_name.clone()
    }

    pub fn upstream_provider_id(&self) -> LanguageModelProviderId {
        self.upstream_provider_id
            .clone()
            .unwrap_or_else(|| self.provider_id.clone())
    }

    pub fn upstream_provider_name(&self) -> LanguageModelProviderName {
        self.upstream_provider_name
            .clone()
            .unwrap_or_else(|| self.provider_name.clone())
    }

    pub fn is_latest(&self) -> bool {
        self.is_latest
    }

    pub fn is_disabled(&self) -> Option<DisabledReason> {
        self.disabled_reason.clone()
    }

    pub fn requires_data_retention(&self) -> bool {
        self.requires_data_retention
    }

    pub fn refusal_fallback_model_id(&self) -> Option<&'static str> {
        self.refusal_fallback_model_id
    }

    pub fn telemetry_id(&self) -> String {
        self.telemetry_id.to_string()
    }

    pub fn model_cost_info(&self) -> Option<LanguageModelCostInfo> {
        self.cost_info.clone()
    }

    pub fn supports_thinking(&self) -> bool {
        self.supports_thinking
    }

    pub fn supports_disabling_thinking(&self) -> bool {
        self.supports_disabling_thinking
    }

    /// Whether thinking can be turned off at the selected effort level.
    pub fn supports_disabling_thinking_at_effort(&self, effort: Option<&str>) -> bool {
        let effort_level = match effort {
            Some(effort) => self
                .supported_effort_levels
                .iter()
                .find(|effort_level| effort_level.value.as_ref() == effort),
            None => self
                .supported_effort_levels
                .iter()
                .find(|effort_level| effort_level.is_default),
        };

        effort_level
            .and_then(|effort_level| effort_level.supports_disabling_thinking)
            .unwrap_or(self.supports_disabling_thinking)
    }

    pub fn supports_fast_mode(&self) -> bool {
        self.supports_fast_mode
    }

    pub fn supported_effort_levels(&self) -> Vec<LanguageModelEffortLevel> {
        self.supported_effort_levels.to_vec()
    }

    /// Returns the default effort level to use when thinking.
    pub fn default_effort_level(&self) -> Option<LanguageModelEffortLevel> {
        self.supported_effort_levels
            .iter()
            .find(|effort_level| effort_level.is_default)
            .cloned()
    }

    pub fn supports_server_side_compaction(&self) -> bool {
        self.supports_server_side_compaction
    }

    pub fn supports_explicit_compaction(&self) -> bool {
        self.supports_explicit_compaction
    }

    pub fn supports_explicit_compaction_output_limit(&self) -> bool {
        self.supports_explicit_compaction_output_limit
    }

    pub fn minimum_explicit_compaction_input_tokens(&self) -> Option<u64> {
        self.minimum_explicit_compaction_input_tokens
    }

    pub fn prompt_compaction_strategy(&self) -> PromptCompactionStrategy {
        self.prompt_compaction_strategy
    }

    pub fn supports_images(&self) -> bool {
        self.supports_images
    }

    pub fn supports_tools(&self) -> bool {
        self.supports_tools
    }

    pub fn supports_tool_choice(&self, choice: LanguageModelToolChoice) -> bool {
        self.tool_choice_support.supports(choice)
    }

    pub fn supports_streaming_tools(&self) -> bool {
        self.supports_streaming_tools
    }

    pub fn supports_split_token_display(&self) -> bool {
        self.supports_split_token_display
    }

    pub fn max_token_count(&self) -> u64 {
        self.max_token_count
    }

    /// Returns the input ceiling before reserving output from any shared window.
    pub fn max_input_tokens(&self) -> u64 {
        self.max_input_tokens
    }

    /// Returns the combined input and output ceiling, if one applies.
    ///
    /// This shares the context window with output.
    pub fn max_total_tokens(&self) -> Option<u64> {
        Some(self.max_token_count)
    }

    pub fn max_output_tokens(&self) -> Option<u64> {
        self.max_output_tokens
    }
}

/// The error for a request to `model` when its provider doesn't offer it.
pub fn unavailable_error(model: &LanguageModel) -> LanguageModelCompletionError {
    LanguageModelCompletionError::ModelUnavailable {
        provider: model.provider_name.clone(),
        model: model.id.clone(),
    }
}

/// Either a built-in icon name or a path to an external SVG.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum IconOrSvg {
    /// A built-in icon from Zed's icon set.
    Icon(IconName),
    /// Path to a custom SVG icon file.
    Svg(SharedString),
}

impl Default for IconOrSvg {
    fn default() -> Self {
        Self::Icon(IconName::AppAssistant)
    }
}

/// Sends requests to a provider's models.
pub trait LanguageModelClient: 'static {
    /// Streams a completion of `request` from `model`, which must be one of
    /// this provider's models.
    fn stream_completion(
        &self,
        model: &LanguageModel,
        request: LanguageModelRequest,
        cx: &AsyncApp,
    ) -> BoxFuture<'static, Result<LanguageModelCompletionStream, LanguageModelCompletionError>>;

    /// Streams the text of a completion of `request` from `model`.
    fn stream_completion_text(
        &self,
        model: &LanguageModel,
        request: LanguageModelRequest,
        cx: &AsyncApp,
    ) -> BoxFuture<'static, Result<LanguageModelTextStream, LanguageModelCompletionError>> {
        let future = self.stream_completion(model, request, cx);

        async move {
            let events = future.await?;
            let mut events = events.fuse();
            let mut message_id = None;
            let mut first_item_text = None;
            let last_token_usage = Arc::new(Mutex::new(TokenUsage::default()));

            if let Some(first_event) = events.next().await {
                match first_event {
                    Ok(LanguageModelCompletionEvent::StartMessage { message_id: id }) => {
                        message_id = Some(id);
                    }
                    Ok(LanguageModelCompletionEvent::Text(text)) => {
                        first_item_text = Some(text);
                    }
                    _ => (),
                }
            }

            let stream = futures::stream::iter(first_item_text.map(Ok))
                .chain(events.filter_map({
                    let last_token_usage = last_token_usage.clone();
                    move |result| {
                        let last_token_usage = last_token_usage.clone();
                        async move {
                            match result {
                                Ok(LanguageModelCompletionEvent::Queued { .. }) => None,
                                Ok(LanguageModelCompletionEvent::Started) => None,
                                Ok(LanguageModelCompletionEvent::StartMessage { .. }) => None,
                                Ok(LanguageModelCompletionEvent::Text(text)) => Some(Ok(text)),
                                Ok(LanguageModelCompletionEvent::Thinking { .. }) => None,
                                Ok(LanguageModelCompletionEvent::RedactedThinking { .. }) => None,
                                Ok(LanguageModelCompletionEvent::ReasoningDetails(_)) => None,
                                Ok(LanguageModelCompletionEvent::InputTransformations(_)) => None,
                                Ok(LanguageModelCompletionEvent::Stop(_)) => None,
                                Ok(LanguageModelCompletionEvent::ToolUse(_)) => None,
                                Ok(LanguageModelCompletionEvent::ToolUseJsonParseError {
                                    ..
                                }) => None,
                                Ok(LanguageModelCompletionEvent::Compaction(_)) => None,
                                Ok(LanguageModelCompletionEvent::UsageUpdate(token_usage)) => {
                                    *last_token_usage.lock() = token_usage;
                                    None
                                }
                                Err(err) => Some(Err(err)),
                            }
                        }
                    }
                }))
                .boxed();

            Ok(LanguageModelTextStream {
                message_id,
                stream,
                last_token_usage,
            })
        }
        .boxed()
    }

    /// Completes `request` from `model`, resolving to its first complete tool use.
    fn stream_completion_tool(
        &self,
        model: &LanguageModel,
        request: LanguageModelRequest,
        cx: &AsyncApp,
    ) -> BoxFuture<'static, Result<LanguageModelToolUse, LanguageModelCompletionError>> {
        let future = self.stream_completion(model, request, cx);

        async move {
            let events = future.await?;
            let mut events = events.fuse();

            while let Some(event) = events.next().await {
                match event {
                    Ok(LanguageModelCompletionEvent::ToolUse(tool_use))
                        if tool_use.is_input_complete =>
                    {
                        return Ok(tool_use);
                    }
                    Err(err) => {
                        return Err(err);
                    }
                    _ => {}
                }
            }

            Err(LanguageModelCompletionError::Other(anyhow::anyhow!(
                "Stream ended without receiving a complete tool use"
            )))
        }
        .boxed()
    }

    /// Counts request input without generating output, when supported by the provider.
    ///
    /// Counts may be estimates and differ from subsequent measured usage. Callers
    /// choose the content to count; this does not infer which input is already
    /// covered by a previous usage report. Unsupported providers return `None`.
    fn count_input_tokens(
        &self,
        _model: &LanguageModel,
        _request: LanguageModelRequest,
        _cx: &AsyncApp,
    ) -> BoxFuture<'static, Result<Option<u64>, LanguageModelCompletionError>> {
        async { Ok(None) }.boxed()
    }

    /// Compacts `request` into replacement context, for models that report
    /// [`LanguageModel::supports_explicit_compaction`].
    fn compact(
        &self,
        model: &LanguageModel,
        _request: LanguageModelRequest,
        _cx: &AsyncApp,
    ) -> BoxFuture<'static, Result<CompactionResult, LanguageModelCompletionError>> {
        let provider = model.provider_name.clone();
        async move {
            Err(LanguageModelCompletionError::Other(anyhow::anyhow!(
                "{provider} does not support explicit compaction"
            )))
        }
        .boxed()
    }

    /// The API key used to serve `model`, if this provider uses one.
    fn api_key(&self, _model: &LanguageModel, _cx: &App) -> Option<String> {
        None
    }
}

pub trait LanguageModelProvider: LanguageModelClient {
    fn id(&self) -> LanguageModelProviderId;
    fn name(&self) -> LanguageModelProviderName;
    fn icon(&self) -> IconOrSvg {
        IconOrSvg::default()
    }
    fn default_model(&self, cx: &App) -> Option<LanguageModel>;
    fn default_fast_model(&self, cx: &App) -> Option<LanguageModel>;
    fn provided_models(&self, cx: &App) -> Vec<LanguageModel>;
    fn recommended_models(&self, _cx: &App) -> Vec<LanguageModel> {
        Vec::new()
    }

    fn is_authenticated(&self, cx: &App) -> bool;
    fn authenticate(&self, cx: &mut App) -> Task<Result<(), AuthenticateError>>;
    fn refresh_models(&self, _cx: &mut App) {}
    fn settings_view(&self, cx: &mut App) -> Option<ProviderSettingsView>;

    fn set_api_key(&self, _key: Option<String>, _cx: &mut App) -> Task<Result<()>> {
        Task::ready(Ok(()))
    }

    /// Whether the key in use is kept in memory only, because the system
    /// keychain could not store it (see `ApiKeyState::is_session_only`). The
    /// connection screen then says that it will be asked again next time.
    fn key_is_session_only(&self, _cx: &App) -> bool {
        false
    }

    /// Copy shown when this provider rejects a request as unauthenticated
    /// (HTTP 401). The default assumes API-key authentication; providers using
    /// other mechanisms (account or subscription based auth) should override
    /// this so users aren't told to check an API key they don't have.
    fn authentication_error_message(&self) -> SharedString {
        format!(
            "The API key for {} is invalid or has expired. \
            Update your key in Connections to continue.",
            self.name().0
        )
        .into()
    }

    /// Copy shown when a request fails because no credentials are configured
    /// for this provider. The default assumes API-key authentication;
    /// providers using other mechanisms (account or subscription based auth)
    /// should override this.
    fn missing_credentials_error_message(&self) -> SharedString {
        format!(
            "No API key is configured for {}. \
            Add your key in Connections to continue.",
            self.name().0
        )
        .into()
    }

    /// Copy shown the first time a user enables fast mode for a model from
    /// this provider. Returning `None` skips the confirmation prompt and lets
    /// the toggle apply silently.
    fn fast_mode_confirmation(&self, _cx: &App) -> Option<FastModeConfirmation> {
        None
    }

    /// What is known about this provider's connection without doing any work.
    ///
    /// The default only knows whether credentials are present. Providers that
    /// can tell more (the server accepted the key, the balance, why the
    /// connection needs attention) override this.
    fn connection_status(&self, cx: &App) -> ProviderConnectionStatus {
        if self.is_authenticated(cx) {
            ProviderConnectionStatus::Connected { detail: None }
        } else {
            ProviderConnectionStatus::NotConnected
        }
    }

    /// Checks the connection now and resolves to its status.
    ///
    /// The default loads the stored credentials and reports
    /// [`Self::connection_status`]. Providers that can ask their server
    /// whether the credentials are accepted override this. A refused key is a
    /// status ([`ProviderConnectionStatus::NeedsAttention`]), not an error.
    ///
    /// The default finds the provider again through the global
    /// [`LanguageModelRegistry`] when it is installed (as [`init`] and
    /// `LanguageModelRegistry::test` do), and answers from `authenticate`
    /// alone when it is not.
    fn check_connection(&self, cx: &mut App) -> Task<Result<ProviderConnectionStatus>> {
        let provider_id = self.id();
        let authenticate = self.authenticate(cx);
        cx.spawn(async move |cx| {
            let result = authenticate.await;
            // The provider is not `'static` here, so it is found again through
            // the registry once its credentials are loaded.
            // Without the registry (a test of one provider) the answer is
            // read from the result of `authenticate` below.
            let status = cx.update(|cx| {
                LanguageModelRegistry::try_read_global(cx)?
                    .provider(&provider_id)
                    .map(|provider| provider.connection_status(cx))
            });
            match (status, result) {
                (Some(status), _) => Ok(status),
                (None, Ok(())) => Ok(ProviderConnectionStatus::Connected { detail: None }),
                (None, Err(AuthenticateError::CredentialsNotFound)) => {
                    Ok(ProviderConnectionStatus::NotConnected)
                }
                (None, Err(error)) => Err(error.into()),
            }
        })
    }

    /// What `usage` cost on the model `model_id`, in US dollars, for providers
    /// that know their prices.
    fn usage_cost(
        &self,
        _model_id: &LanguageModelId,
        _usage: &TokenUsage,
        _cx: &App,
    ) -> Option<f64> {
        None
    }
}

/// How a provider's connection stands, for the screens that list providers.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ProviderConnectionStatus {
    /// No credentials are configured.
    NotConnected,
    /// Credentials are present and, as far as is known, accepted.
    Connected {
        /// A short fact worth showing next to the provider's name, such as
        /// "$20.50 left", "12 models" or an account name.
        detail: Option<SharedString>,
    },
    /// Credentials are present but something is wrong.
    NeedsAttention {
        /// One sentence, shown as is.
        reason: SharedString,
    },
}

impl ProviderConnectionStatus {
    pub fn is_connected(&self) -> bool {
        matches!(self, Self::Connected { .. })
    }
}

/// A provider's settings UI, modeled as mutually exclusive presentation modes.
#[derive(Clone)]
pub enum ProviderSettingsView {
    ApiKey(ApiKeyConfiguration),
    Inline(InlineProviderSettings),
    SubPage(SubPageProviderSettings),
}

#[derive(Clone)]
pub struct InlineProviderSettings {
    pub title: Option<SharedString>,
    pub description: Option<InlineDescription>,
    pub create_view: CreateProviderSettingsView,
}

#[derive(Clone)]
pub struct SubPageProviderSettings {
    pub description: Option<InlineDescription>,
    pub create_view: CreateProviderSettingsView,
}

impl SubPageProviderSettings {
    pub fn new(create_view: impl Fn(&mut Window, &mut App) -> AnyView + 'static) -> Self {
        Self {
            description: None,
            create_view: Arc::new(create_view),
        }
    }

    pub fn description(mut self, description: InlineDescription) -> Self {
        self.description = Some(description);
        self
    }
}

impl ApiKeyConfiguration {
    pub fn new(
        has_key: bool,
        is_from_env_var: bool,
        env_var_name: SharedString,
        api_key_url: SharedString,
    ) -> Self {
        Self {
            has_key,
            is_from_env_var,
            env_var_name,
            api_key_url,
        }
    }
}

/// A live snapshot of a single-API-key provider's credential state, used by the
/// settings UI to render the provider's "API Key" section.
#[derive(Clone)]
pub struct ApiKeyConfiguration {
    pub has_key: bool,
    pub is_from_env_var: bool,
    pub env_var_name: SharedString,
    pub api_key_url: SharedString,
}

/// The subtitle rendered beneath a provider's name when its configuration is
/// shown inline.
#[derive(Clone)]
pub enum InlineDescription {
    /// A clickable "Where to find key" link pointing at the given URL, for
    /// API-key based providers.
    ApiKeyUrl(SharedString),
    /// Plain descriptive text, e.g. explaining a sign-in based provider.
    Text(SharedString),
}

/// Provider-specific copy shown the first time a user enables fast mode.
#[derive(Debug, Clone)]
pub struct FastModeConfirmation {
    pub title: SharedString,
    pub message: SharedString,
}

pub trait LanguageModelProviderState: 'static {
    type ObservableEntity;

    fn observable_entity(&self) -> Option<gpui::Entity<Self::ObservableEntity>>;

    fn subscribe<T: 'static>(
        &self,
        cx: &mut gpui::Context<T>,
        callback: impl Fn(&mut T, &mut gpui::Context<T>) + 'static,
    ) -> Option<gpui::Subscription> {
        let entity = self.observable_entity()?;
        Some(cx.observe(&entity, move |this, _, cx| {
            callback(this, cx);
        }))
    }
}

#[derive(Clone, Debug, PartialEq)]
pub enum LanguageModelCostInfo {
    /// Cost in US dollars per million input and output tokens
    TokenCost {
        input_token_cost_per_1m: f64,
        output_token_cost_per_1m: f64,
    },
    /// Cost per request
    RequestCost { cost_per_request: f64 },
}

impl LanguageModelCostInfo {
    /// A short label for the model picker: "$3/$15" (input/output per million
    /// tokens) or "0.33×" (per request).
    pub fn to_shared_string(&self) -> SharedString {
        match self {
            LanguageModelCostInfo::RequestCost { cost_per_request } => {
                let cost_str = format!("{}×", Self::cost_value_to_string(cost_per_request));
                SharedString::from(cost_str)
            }
            LanguageModelCostInfo::TokenCost {
                input_token_cost_per_1m,
                output_token_cost_per_1m,
            } => {
                let input_cost = Self::price_to_string(*input_token_cost_per_1m);
                let output_cost = Self::price_to_string(*output_token_cost_per_1m);
                SharedString::from(format!("${}/${}", input_cost, output_cost))
            }
        }
    }

    fn cost_value_to_string(cost: &f64) -> SharedString {
        if (cost.fract() - 0.0).abs() < std::f64::EPSILON {
            SharedString::from(format!("{:.0}", cost))
        } else {
            SharedString::from(format!("{:.2}", cost))
        }
    }

    /// A price in dollars without rounding it away: whole numbers print bare
    /// ("3"), others with at least two decimals and as many as they need, up
    /// to four ("1.25", "0.40", "0.375", "0.0625"). Prices under a tenth of a
    /// cent keep two significant digits instead ("0.00012").
    fn price_to_string(price: f64) -> String {
        if price.fract().abs() < f64::EPSILON {
            return format!("{price:.0}");
        }
        let decimals = if price.abs() >= 0.001 {
            4
        } else {
            ((-price.abs().log10()).ceil() as usize + 1).min(12)
        };
        let formatted = format!("{price:.decimals$}");
        let trimmed = formatted.trim_end_matches('0');
        let kept_decimals = trimmed.rsplit('.').next().map_or(0, str::len);
        if kept_decimals < 2 {
            format!("{price:.2}")
        } else {
            trimmed.to_string()
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn token_cost_label_keeps_small_prices_readable() {
        let label = |input, output| {
            LanguageModelCostInfo::TokenCost {
                input_token_cost_per_1m: input,
                output_token_cost_per_1m: output,
            }
            .to_shared_string()
            .to_string()
        };
        assert_eq!(label(3.0, 15.0), "$3/$15");
        assert_eq!(label(1.25, 10.0), "$1.25/$10");
        assert_eq!(label(0.05, 0.4), "$0.05/$0.40");
        assert_eq!(label(0.375, 1.875), "$0.375/$1.875");
        assert_eq!(label(1.125, 6.75), "$1.125/$6.75");
        assert_eq!(label(0.0625, 0.1), "$0.0625/$0.10");
        assert_eq!(label(0.004125, 0.075), "$0.0041/$0.075");
        assert_eq!(label(0.004, 0.09), "$0.004/$0.09");
        assert_eq!(label(0.00001, 0.000123), "$0.00001/$0.00012");
    }

    #[gpui::test]
    async fn provider_connection_defaults(cx: &mut gpui::TestAppContext) {
        let registered = cx.update(LanguageModelRegistry::test);
        let unregistered = fake_provider::FakeLanguageModelProvider::new(
            LanguageModelProviderId::from("elsewhere".to_string()),
            LanguageModelProviderName::from("Elsewhere".to_string()),
        );
        let connected = ProviderConnectionStatus::Connected { detail: None };

        cx.read(|cx| {
            assert_eq!(registered.connection_status(cx), connected);
            let usage = TokenUsage {
                input_tokens: 1_000,
                ..Default::default()
            };
            let model_id = LanguageModelId::from("fake".to_string());
            assert_eq!(registered.usage_cost(&model_id, &usage, cx), None);
        });
        for provider in [registered.as_ref(), &unregistered] {
            let status = cx.update(|cx| provider.check_connection(cx)).await.unwrap();
            assert_eq!(status, connected);
            assert!(status.is_connected());
        }
    }

    #[test]
    fn request_cost_label_is_a_multiplier() {
        let cost = LanguageModelCostInfo::RequestCost {
            cost_per_request: 1.5,
        };
        assert_eq!(cost.to_shared_string().as_ref(), "1.50×");
    }
}

use anyhow::{Result, anyhow};
use collections::BTreeMap;
use credentials_provider::CredentialsProvider;
use futures::{
    AsyncReadExt, FutureExt, StreamExt,
    future::{BoxFuture, Shared},
};
use gpui::{App, AppContext, AsyncApp, Context, Entity, Global, SharedString, Task, WeakEntity};
use http_client::{
    AsyncBody, CustomHeaders, HttpClient, Method, Request as HttpRequest, RequestBuilderExt, http,
};
use language_model::chat_completion::{ChatCompletionEventMapper, ResponseStreamEvent};
use language_model::{
    ApiKeyConfiguration, ApiKeyState, AuthenticateError, DisabledReason, EnvVar, IconOrSvg,
    LanguageModel, LanguageModelClient, LanguageModelCompletionError,
    LanguageModelCompletionStream, LanguageModelCostInfo, LanguageModelId, LanguageModelName,
    LanguageModelProvider, LanguageModelProviderId, LanguageModelProviderName,
    LanguageModelProviderState, LanguageModelRequest, LanguageModelToolChoiceSupport,
    ModelRateLimiters, ProviderConnectionStatus, ProviderSettingsView, RateLimiter, TokenUsage,
    env_var, unavailable_error,
};
use serde::Deserialize;
pub use settings::OpenAiCompatibleModelCapabilities as ModelCapabilities;
pub use settings::VercelAiGatewayAvailableModel as AvailableModel;
use settings::{Settings, SettingsStore};
use std::sync::{Arc, LazyLock};
use std::time::Duration;
use ui::IconName;
use util::ResultExt;

/// The model a new connection starts with: the first of these that the API
/// lists and that is not down, otherwise the first model by name.
const DEFAULT_MODEL_PREFERENCE: &[&str] = &["claude-sonnet-5-5", "claude-sonnet-5", "gpt-5.5"];

/// The model for titles, summaries and commit messages: the first of these
/// that the API lists and that is not down, otherwise the cheapest by input
/// price. Without one, those small jobs are billed at the chat model's price.
const FAST_MODEL_PREFERENCE: &[&str] =
    &["deepseek-v4.1-flash", "claude-haiku-5-5", "glm-5.3-flash"];

/// The most output a request asks for when it does not say and the model's
/// own limit is known: the model's limit, up to this. A fixed 16,000 used to
/// be asked for, and long answers (a whole file, or a model whose hidden
/// reasoning counts as output) stopped on "Output Limit Reached".
const DEFAULT_OUTPUT_TOKEN_CEILING: u64 = 64_000;

/// The output asked for when neither the request nor the API says what the
/// model can write.
const UNKNOWN_MODEL_OUTPUT_TOKENS: u64 = 16_000;

/// The API refuses a request (402) unless the balance covers the input plus
/// all the output asked for. So the output asked for is also held to what
/// this share of the balance can pay, after the input, at the model's price.
const BALANCE_SHARE_FOR_ONE_REQUEST: f64 = 0.9;

/// A request never asks for less output than this, however low the balance:
/// below it the server's 402 ("balance too low") is the honest answer.
const MIN_OUTPUT_TOKEN_RESERVATION: u64 = 2_000;

/// Under this many dollars the balance is called low.
const LOW_BALANCE_THRESHOLD: f64 = 1.0;

/// The reason of `ProviderConnectionStatus::NeedsAttention` after a 401. The
/// chat recognises it to offer "Replace Key" instead of a generic card.
pub const KEY_REFUSED_REASON: &str = "This key was refused.";
const UNREACHABLE_REASON: &str = "Could not reach Unaligned.";
const MODEL_DOWN_REASON: &str = "Temporarily unavailable";

const PROVIDER_ID: LanguageModelProviderId =
    LanguageModelProviderId::new(crate::UNALIGNED_PROVIDER_ID);
const PROVIDER_NAME: LanguageModelProviderName = LanguageModelProviderName::new("Unaligned");

/// The API's origin, without a path: the model list and the balance live
/// under `/v1`, completions under `/openai/v1`.
const API_URL: &str = branding::PROVIDER_API_URL;
const API_KEY_ENV_VAR_NAME: &str = "UNALIGNED_API_KEY";
static API_KEY_ENV_VAR: LazyLock<EnvVar> = env_var!(API_KEY_ENV_VAR_NAME);

/// Whether the API currently serves a model, as `GET /v1/models` reports it.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum ModelStatus {
    Up,
    Degraded,
    Down,
    #[default]
    #[serde(other)]
    Unknown,
}

/// A model's prices in US dollars per million tokens.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct ModelPrice {
    pub input: f64,
    pub output: f64,
    /// The price of input read from the prompt cache, when it has its own.
    pub cached: Option<f64>,
}

impl ModelPrice {
    /// What `usage` costs at these prices, in US dollars. This is the API's
    /// own formula, computed here: the server's bill is the reference.
    pub fn cost_of(&self, usage: &TokenUsage) -> f64 {
        let cached = self.cached.unwrap_or(self.input);
        let uncached_input = usage.input_tokens + usage.cache_creation_input_tokens;
        (uncached_input as f64 * self.input
            + usage.cache_read_input_tokens as f64 * cached
            + usage.output_tokens as f64 * self.output)
            / 1_000_000.0
    }
}

/// What the API says about a model beyond its limits. Models that come only
/// from settings have the default: no price, unknown status.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct UnalignedModelDetails {
    /// Who makes the model ("Anthropic", "DeepSeek").
    pub org: Option<SharedString>,
    pub status: ModelStatus,
    pub price: Option<ModelPrice>,
    pub reasoning: bool,
    pub uncensored: bool,
}

/// A model this provider offers: how to call it and what the API says of it.
#[derive(Clone, Debug, PartialEq)]
struct UnalignedModel {
    config: AvailableModel,
    details: UnalignedModelDetails,
}

impl UnalignedModel {
    fn is_usable(&self) -> bool {
        self.details.status != ModelStatus::Down
    }

    fn display_name(&self) -> &str {
        self.config
            .display_name
            .as_deref()
            .unwrap_or(&self.config.name)
    }
}

/// What the server last said about the stored key.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum KeyCheck {
    Accepted,
    Refused,
    /// No verdict: the network or the server failed. The key is not blamed.
    Unreachable,
}

#[derive(Default, Clone, Debug, PartialEq)]
pub struct UnalignedSettings {
    pub api_url: String,
    pub available_models: Vec<AvailableModel>,
    pub custom_headers: CustomHeaders,
}

pub struct UnalignedLanguageModelProvider {
    http_client: Arc<dyn HttpClient>,
    state: Entity<State>,
    request_limiters: ModelRateLimiters,
}

struct GlobalUnalignedProvider(Arc<UnalignedLanguageModelProvider>);

impl Global for GlobalUnalignedProvider {}

pub struct State {
    api_key_state: ApiKeyState,
    credentials_provider: Arc<dyn CredentialsProvider>,
    http_client: Arc<dyn HttpClient>,
    /// The fetched list, prices included: `usage_cost` reads them from here.
    available_models: Vec<UnalignedModel>,
    fetch_models_task: Option<Task<()>>,
    last_fetch_models_error: Option<String>,
    /// The balance the server last reported for the stored key, in US dollars.
    balance: Option<f64>,
    /// A reading of the balance is already waiting for its delay after a
    /// request: requests that end meanwhile are covered by it.
    balance_refresh_scheduled: bool,
    /// What the server last said about the stored key; `None` until asked.
    key_check: Option<KeyCheck>,
    /// The request to `/v1/balance`, shared by everyone who asks while it is
    /// in flight (`is_checking_key`).
    key_check_task: Option<Shared<Task<Option<KeyCheck>>>>,
    is_checking_key: bool,
    /// Bumped whenever the key or the server changes, so that an answer
    /// about an earlier key is dropped instead of being shown for the new one.
    key_generation: u64,
}

impl State {
    fn is_authenticated(&self) -> bool {
        self.api_key_state.has_key()
    }

    fn set_api_key(&mut self, api_key: Option<String>, cx: &mut Context<Self>) -> Task<Result<()>> {
        let credentials_provider = self.credentials_provider.clone();
        let api_url = UnalignedLanguageModelProvider::api_url(cx);
        let store = self.api_key_state.store(
            api_url,
            api_key,
            |this| &mut this.api_key_state,
            credentials_provider,
            cx,
        );

        // Resolves once the key is stored. The server is asked about it in
        // the background: `check_connection` waits for that answer.
        cx.spawn(async move |this, cx| {
            store.await?;
            this.update(cx, |this, cx| {
                this.forget_key_check(cx);
                this.restart_fetch_models_task(cx);
                this.start_key_check(cx);
            })?;
            Ok(())
        })
    }

    fn load_key(&mut self, cx: &mut Context<Self>) -> Task<Result<(), AuthenticateError>> {
        let credentials_provider = self.credentials_provider.clone();
        let api_url = UnalignedLanguageModelProvider::api_url(cx);
        self.api_key_state.load_if_needed(
            api_url,
            |this| &mut this.api_key_state,
            credentials_provider,
            cx,
        )
    }

    fn authenticate(&mut self, cx: &mut Context<Self>) -> Task<Result<(), AuthenticateError>> {
        let task = self.load_key(cx);

        cx.spawn(async move |this, cx| {
            let result = task.await;
            this.update(cx, |this, cx| {
                this.restart_fetch_models_task(cx);
                this.start_key_check(cx);
            })
            .ok();
            result
        })
    }

    /// Drops everything known about the key: called when the key or the
    /// server it is sent to changes.
    fn forget_key_check(&mut self, cx: &mut Context<Self>) {
        self.key_generation += 1;
        self.key_check_task = None;
        self.is_checking_key = false;
        self.key_check = None;
        self.balance = None;
        cx.notify();
    }

    /// Asks the server about the stored key with `GET /v1/balance`, which
    /// both tests the key and reads the balance, and records the answer.
    /// Resolves to `None` when there is no key or when the key changed in the
    /// meantime. Callers that ask while a request is in flight share it.
    fn check_key(&mut self, cx: &mut Context<Self>) -> Shared<Task<Option<KeyCheck>>> {
        if self.is_checking_key
            && let Some(task) = &self.key_check_task
        {
            return task.clone();
        }

        let api_url = UnalignedLanguageModelProvider::api_url(cx);
        let Some(api_key) = self.api_key_state.key(&api_url) else {
            return Task::ready(None).shared();
        };
        let http_client = self.http_client.clone();
        let extra_headers = UnalignedLanguageModelProvider::settings(cx)
            .custom_headers
            .clone();
        let generation = self.key_generation;

        let task = cx
            .spawn(async move |this, cx| {
                let result =
                    fetch_balance(http_client.as_ref(), &api_url, &api_key, &extra_headers).await;
                this.update(cx, |this, cx| {
                    if this.key_generation != generation {
                        return None;
                    }
                    this.is_checking_key = false;
                    let check = match result {
                        Ok(balance) => {
                            this.balance = Some(balance);
                            KeyCheck::Accepted
                        }
                        Err(BalanceError::Refused) => {
                            this.balance = None;
                            KeyCheck::Refused
                        }
                        // The last known balance stays: it is still the best
                        // figure there is.
                        Err(BalanceError::Unreachable(error)) => {
                            log::warn!("could not check the Unaligned key: {error:#}");
                            KeyCheck::Unreachable
                        }
                    };
                    this.key_check = Some(check);
                    cx.notify();
                    Some(check)
                })
                .ok()
                .flatten()
            })
            .shared();
        self.is_checking_key = true;
        self.key_check_task = Some(task.clone());
        task
    }

    /// Starts [`Self::check_key`] without waiting for the answer: the state
    /// keeps the request alive and observers are notified when it arrives.
    fn start_key_check(&mut self, cx: &mut Context<Self>) {
        drop(self.check_key(cx));
    }

    /// Loads the key if that was not done yet, then asks the server about it.
    fn verify_key(&mut self, cx: &mut Context<Self>) -> Task<Result<Option<KeyCheck>>> {
        let load = self.load_key(cx);
        cx.spawn(async move |this, cx| {
            match load.await {
                Ok(()) | Err(AuthenticateError::CredentialsNotFound) => {}
                Err(error) => return Err(error.into()),
            }
            let check = this.update(cx, |this, cx| this.check_key(cx))?;
            Ok(check.await)
        })
    }

    fn connection_status(&self, cx: &App) -> ProviderConnectionStatus {
        if !self.is_authenticated() {
            return ProviderConnectionStatus::NotConnected;
        }
        match self.key_check {
            Some(KeyCheck::Refused) => {
                return ProviderConnectionStatus::NeedsAttention {
                    reason: KEY_REFUSED_REASON.into(),
                };
            }
            Some(KeyCheck::Unreachable) => {
                return ProviderConnectionStatus::NeedsAttention {
                    reason: UNREACHABLE_REASON.into(),
                };
            }
            Some(KeyCheck::Accepted) | None => {}
        }
        let knows_a_model = !self.available_models.is_empty()
            || !UnalignedLanguageModelProvider::settings(cx)
                .available_models
                .is_empty();
        if !knows_a_model && let Some(error) = &self.last_fetch_models_error {
            return ProviderConnectionStatus::NeedsAttention {
                reason: format!("Could not load the model list: {error}").into(),
            };
        }
        ProviderConnectionStatus::Connected {
            detail: self
                .balance
                .filter(|_| self.key_check == Some(KeyCheck::Accepted))
                .map(balance_detail),
        }
    }

    fn fetch_models(
        &mut self,
        cx: &mut Context<Self>,
    ) -> Task<Result<(), LanguageModelCompletionError>> {
        let http_client = self.http_client.clone();
        let api_url = UnalignedLanguageModelProvider::api_url(cx);
        let api_key = self.api_key_state.key(&api_url);
        let extra_headers = UnalignedLanguageModelProvider::settings(cx)
            .custom_headers
            .clone();
        cx.spawn(async move |this, cx| {
            let models = list_models(
                http_client.as_ref(),
                &api_url,
                api_key.as_deref(),
                &extra_headers,
            )
            .await?;
            this.update(cx, |this, cx| {
                this.available_models = models;
                cx.notify();
            })
            .map_err(|e| LanguageModelCompletionError::Other(e))?;
            Ok(())
        })
    }

    fn restart_fetch_models_task(&mut self, cx: &mut Context<Self>) {
        if self.is_authenticated() {
            let fetch_task = self.fetch_models(cx);
            let task = cx.spawn(async move |this, cx| {
                let result = fetch_task.await;
                this.update(cx, |this, cx| {
                    this.last_fetch_models_error =
                        result.as_ref().err().map(|error| format!("{error:#}"));
                    // `connection_status` reports this error.
                    cx.notify();
                })
                .ok();
                result.log_err();
            });
            self.fetch_models_task.replace(task);
        } else {
            self.available_models = Vec::new();
            self.last_fetch_models_error = None;
        }
    }
}

impl UnalignedLanguageModelProvider {
    pub fn new(
        http_client: Arc<dyn HttpClient>,
        credentials_provider: Arc<dyn CredentialsProvider>,
        cx: &mut App,
    ) -> Self {
        let state = cx.new(|cx| {
            cx.observe_global::<SettingsStore>({
                let mut last_settings = UnalignedLanguageModelProvider::settings(cx).clone();
                move |this: &mut State, cx| {
                    let current_settings = UnalignedLanguageModelProvider::settings(cx);
                    if current_settings != &last_settings {
                        // Another address is another server: what the old one
                        // said about the key does not carry over.
                        let server_changed = current_settings.api_url != last_settings.api_url;
                        last_settings = current_settings.clone();
                        if server_changed {
                            this.forget_key_check(cx);
                        }
                        this.authenticate(cx).detach();
                        cx.notify();
                    }
                }
            })
            .detach();
            State {
                api_key_state: ApiKeyState::new(Self::api_url(cx), (*API_KEY_ENV_VAR).clone()),
                credentials_provider,
                http_client: http_client.clone(),
                available_models: Vec::new(),
                fetch_models_task: None,
                last_fetch_models_error: None,
                balance: None,
                balance_refresh_scheduled: false,
                key_check: None,
                key_check_task: None,
                is_checking_key: false,
                key_generation: 0,
            }
        });

        Self {
            http_client,
            state,
            request_limiters: ModelRateLimiters::default(),
        }
    }

    fn settings(cx: &App) -> &UnalignedSettings {
        &crate::AllLanguageModelSettings::get_global(cx).unaligned
    }

    fn api_url(cx: &App) -> SharedString {
        let api_url = &Self::settings(cx).api_url;
        if api_url.is_empty() {
            API_URL.into()
        } else {
            SharedString::new(api_url.as_str())
        }
    }

    /// The provider registered at startup. The registry only hands out
    /// `dyn LanguageModelProvider`: this is how the code that shows the
    /// balance reaches [`Self::balance`] and [`Self::refresh_balance`].
    /// Observe it with `LanguageModelProviderState::subscribe`.
    pub fn global(cx: &App) -> Option<Arc<Self>> {
        cx.try_global::<GlobalUnalignedProvider>()
            .map(|global| global.0.clone())
    }

    pub fn set_global(provider: Arc<Self>, cx: &mut App) {
        cx.set_global(GlobalUnalignedProvider(provider));
    }

    /// The balance the server last reported for the stored key, in US
    /// dollars. `None` until a check succeeded.
    pub fn balance(&self, cx: &App) -> Option<f64> {
        self.state.read(cx).balance
    }

    /// Asks the server for the balance again. The new value is in
    /// [`Self::balance`] and in `connection_status` once this resolves, and
    /// observers of the provider are notified. Fails when the key is refused
    /// or the server cannot be reached; without a key it does nothing.
    pub fn refresh_balance(&self, cx: &mut App) -> Task<Result<()>> {
        let verify = self.state.update(cx, |state, cx| state.verify_key(cx));
        cx.spawn(async move |_| match verify.await? {
            Some(KeyCheck::Accepted) | None => Ok(()),
            Some(KeyCheck::Refused) => Err(anyhow!(KEY_REFUSED_REASON)),
            Some(KeyCheck::Unreachable) => Err(anyhow!(UNREACHABLE_REASON)),
        })
    }

    /// What the API says about one of this provider's models: maker, status,
    /// prices, whether it reasons, whether it is uncensored.
    pub fn model_details(
        &self,
        model_id: &LanguageModelId,
        cx: &App,
    ) -> Option<UnalignedModelDetails> {
        self.state
            .read(cx)
            .available_models
            .iter()
            .find(|model| model.config.name == model_id.0.as_ref())
            .map(|model| model.details.clone())
    }

    /// Every model this provider offers, keyed by id: the fetched list, then
    /// settings entries overriding how a model is called (what the API says
    /// of it, such as its price and status, stays).
    fn unaligned_models(&self, cx: &App) -> BTreeMap<String, UnalignedModel> {
        let mut models = BTreeMap::default();

        for model in self.state.read(cx).available_models.clone() {
            models.insert(model.config.name.clone(), model);
        }

        for config in &Self::settings(cx).available_models {
            let details = models
                .get(&config.name)
                .map(|model: &UnalignedModel| model.details.clone())
                .unwrap_or_default();
            models.insert(
                config.name.clone(),
                UnalignedModel {
                    config: config.clone(),
                    details,
                },
            );
        }

        models
    }

    /// The current configuration of `model`, if this provider still offers it.
    fn config(
        &self,
        model: &LanguageModel,
        cx: &App,
    ) -> Result<AvailableModel, LanguageModelCompletionError> {
        self.unaligned_models(cx)
            .remove(model.id.0.as_ref())
            .map(|model| model.config)
            .ok_or_else(|| unavailable_error(model))
    }

    fn stream_open_ai(
        &self,
        request_limiter: &RateLimiter,
        request: open_ai::Request,
        cx: &AsyncApp,
    ) -> BoxFuture<
        'static,
        Result<
            futures::stream::BoxStream<
                'static,
                Result<ResponseStreamEvent, LanguageModelCompletionError>,
            >,
            LanguageModelCompletionError,
        >,
    > {
        let http_client = self.http_client.clone();
        let (api_key, api_url, extra_headers) = self.state.read_with(cx, |state, cx| {
            let api_url = UnalignedLanguageModelProvider::api_url(cx);
            let extra_headers = UnalignedLanguageModelProvider::settings(cx)
                .custom_headers
                .clone();
            (state.api_key_state.key(&api_url), api_url, extra_headers)
        });

        let future = request_limiter.stream(async move {
            let provider = PROVIDER_NAME;
            let Some(api_key) = api_key else {
                return Err(LanguageModelCompletionError::NoApiKey { provider });
            };
            let api_url = format!("{}/openai/v1", api_url.trim_end_matches('/'));
            let request = open_ai::stream_completion(
                http_client.as_ref(),
                provider.0.as_str(),
                &api_url,
                &api_key,
                request,
                &extra_headers,
            );
            let response = request.await.map_err(map_open_ai_error)?;
            Ok(response)
        });

        async move { Ok(future.await?.boxed()) }.boxed()
    }
}

impl LanguageModelProviderState for UnalignedLanguageModelProvider {
    type ObservableEntity = State;

    fn observable_entity(&self) -> Option<Entity<Self::ObservableEntity>> {
        Some(self.state.clone())
    }
}

impl LanguageModelProvider for UnalignedLanguageModelProvider {
    fn id(&self) -> LanguageModelProviderId {
        PROVIDER_ID
    }

    fn name(&self) -> LanguageModelProviderName {
        PROVIDER_NAME
    }

    fn icon(&self) -> IconOrSvg {
        IconOrSvg::Icon(IconName::AiUnaligned)
    }

    fn default_model(&self, cx: &App) -> Option<LanguageModel> {
        pick_default_model(&self.unaligned_models(cx)).map(language_model)
    }

    fn default_fast_model(&self, cx: &App) -> Option<LanguageModel> {
        pick_fast_model(&self.unaligned_models(cx)).map(language_model)
    }

    fn provided_models(&self, cx: &App) -> Vec<LanguageModel> {
        self.unaligned_models(cx)
            .values()
            .map(language_model)
            .collect()
    }

    fn is_authenticated(&self, cx: &App) -> bool {
        self.state.read(cx).is_authenticated()
    }

    fn authenticate(&self, cx: &mut App) -> Task<Result<(), AuthenticateError>> {
        self.state.update(cx, |state, cx| state.authenticate(cx))
    }

    fn refresh_models(&self, cx: &mut App) {
        self.state
            .update(cx, |state, cx| state.restart_fetch_models_task(cx));
    }

    fn settings_view(&self, cx: &mut App) -> Option<ProviderSettingsView> {
        let state = self.state.read(cx);
        Some(ProviderSettingsView::ApiKey(ApiKeyConfiguration::new(
            state.api_key_state.has_key(),
            state.api_key_state.is_from_env_var(),
            state.api_key_state.env_var_name().clone(),
            branding::PROVIDER_KEYS_URL.into(),
        )))
    }

    fn set_api_key(&self, api_key: Option<String>, cx: &mut App) -> Task<Result<()>> {
        self.state
            .update(cx, |state, cx| state.set_api_key(api_key, cx))
    }

    fn connection_status(&self, cx: &App) -> ProviderConnectionStatus {
        self.state.read(cx).connection_status(cx)
    }

    fn key_is_session_only(&self, cx: &App) -> bool {
        self.state.read(cx).api_key_state.is_session_only()
    }

    /// Asks the server whether it accepts the stored key (`GET /v1/balance`).
    /// A refused key or an unreachable server is a status, not an error; the
    /// only error is a keychain that could not be read.
    fn check_connection(&self, cx: &mut App) -> Task<Result<ProviderConnectionStatus>> {
        let state = self.state.clone();
        let verify = state.update(cx, |state, cx| state.verify_key(cx));
        cx.spawn(async move |cx| {
            verify.await?;
            Ok(state.read_with(cx, |state, cx| state.connection_status(cx)))
        })
    }

    fn usage_cost(&self, model_id: &LanguageModelId, usage: &TokenUsage, cx: &App) -> Option<f64> {
        let state = self.state.read(cx);
        let model = state
            .available_models
            .iter()
            .find(|model| model.config.name == model_id.0.as_ref())?;
        Some(model.details.price?.cost_of(usage))
    }
}

impl LanguageModelClient for UnalignedLanguageModelProvider {
    fn stream_completion(
        &self,
        model: &LanguageModel,
        request: LanguageModelRequest,
        cx: &AsyncApp,
    ) -> BoxFuture<'static, Result<LanguageModelCompletionStream, LanguageModelCompletionError>>
    {
        let config = match cx.update(|cx| self.config(model, cx)) {
            Ok(config) => config,
            Err(error) => return async move { Err(error) }.boxed(),
        };
        let request_limiter = self.request_limiters.for_model(&model.id);
        let affordable_output_tokens = cx.update(|cx| {
            let state = self.state.read(cx);
            let price = state
                .available_models
                .iter()
                .find(|available| available.config.name == config.name)
                .and_then(|available| available.details.price)?;
            affordable_output_tokens(state.balance?, &price, &request)
        });
        let request = match into_request(request, &config, affordable_output_tokens) {
            Ok(request) => request,
            Err(error) => return async move { Err(error.into()) }.boxed(),
        };
        let completions = self.stream_open_ai(&request_limiter, request, cx);
        let executor = cx.background_executor().clone();
        // The sender lives in the stream: when the stream is dropped, at the
        // end of the answer or on cancel, the balance is read again.
        let (request_done_tx, request_done_rx) = futures::channel::oneshot::channel::<()>();
        refresh_balance_when_request_ends(self.state.downgrade(), request_done_rx, cx);
        async move {
            let mapper = ChatCompletionEventMapper::new();
            let events = mapper.map_stream(completions.await?).map(move |event| {
                let _request_done = &request_done_tx;
                event
            });
            Ok(language_model::stream_in_background(
                events.boxed(),
                executor,
            ))
        }
        .boxed()
    }
}

/// How long after the end of a request the balance is read again. The server
/// books the charge once the answer has been sent, so asking at once would
/// often read the amount from before the request.
const BALANCE_REFRESH_DELAY: Duration = Duration::from_millis(700);

/// When the first reading after a request shows the same amount as before, the
/// charge may not have been booked yet: one more reading is taken this much
/// later, and that is the last one.
const BALANCE_REFRESH_RETRY_DELAY: Duration = Duration::from_millis(2500);

/// Reads the balance again once the request whose stream holds the other end
/// of `request_done` is over, whether it finished, failed or was cancelled.
/// Every request of this provider goes through here (chat turns and each of
/// their tool rounds, inline edits, titles, commit messages), so the amount
/// shown in the status bar follows the spending without polling.
fn refresh_balance_when_request_ends(
    state: WeakEntity<State>,
    request_done: futures::channel::oneshot::Receiver<()>,
    cx: &AsyncApp,
) {
    cx.spawn(async move |cx| {
        // Resolves when the sender is dropped with the stream: the result is
        // always "cancelled", which is the signal.
        request_done.await.ok();
        let (balance_before, already_scheduled) = state
            .update(cx, |state, _| {
                let already_scheduled = state.balance_refresh_scheduled;
                state.balance_refresh_scheduled = true;
                (state.balance, already_scheduled)
            })
            .ok()?;
        if already_scheduled {
            return Some(());
        }

        cx.background_executor().timer(BALANCE_REFRESH_DELAY).await;
        state
            .update(cx, |state, cx| {
                // From here on a request that ends schedules its own reading.
                state.balance_refresh_scheduled = false;
                state.check_key(cx)
            })
            .ok()?
            .await;

        let balance_after = state.read_with(cx, |state, _| state.balance).ok()?;
        if balance_after == balance_before {
            cx.background_executor()
                .timer(BALANCE_REFRESH_RETRY_DELAY)
                .await;
            state
                .update(cx, |state, cx| state.check_key(cx))
                .ok()?
                .await;
        }
        Some(())
    })
    .detach();
}

/// How many output tokens the balance can still pay for at `price`, once the
/// request's own input is paid. The input is estimated from the size of the
/// text (three bytes a token, on the generous side) since the true count is
/// only known after the answer.
fn affordable_output_tokens(
    balance: f64,
    price: &ModelPrice,
    request: &LanguageModelRequest,
) -> Option<u64> {
    if price.output <= 0.0 {
        return None;
    }
    let input_bytes: usize = request
        .messages
        .iter()
        .map(|message| message.string_contents().len())
        .sum();
    let input_cost = (input_bytes as f64 / 3.0) * price.input / 1_000_000.0;
    let left_for_output = (balance * BALANCE_SHARE_FOR_ONE_REQUEST - input_cost).max(0.0);
    Some((left_for_output / price.output * 1_000_000.0) as u64)
}

fn into_request(
    request: LanguageModelRequest,
    config: &AvailableModel,
    affordable_output_tokens: Option<u64>,
) -> Result<open_ai::Request> {
    let max_output_tokens =
        output_token_limit(request.max_output_tokens, config, affordable_output_tokens);
    crate::provider::open_ai::into_open_ai(
        request,
        &config.name,
        config.capabilities.parallel_tool_calls,
        config.capabilities.prompt_cache_key,
        Some(max_output_tokens),
        crate::provider::open_ai::ChatCompletionMaxTokensParameter::MaxCompletionTokens,
        None,
        false,
    )
}

/// The output limit a request is sent with (`max_completion_tokens`).
///
/// A request that names a limit gets it, within the model's own; so does the
/// `max_completion_tokens` a settings entry chose for the model. Otherwise it
/// is the model's own limit (up to [`DEFAULT_OUTPUT_TOKEN_CEILING`]), held to
/// what the balance can pay for when that is known: with a comfortable
/// balance a long answer is never cut short, and with a low one the request
/// still passes the server's check instead of being refused.
fn output_token_limit(
    requested: Option<u64>,
    config: &AvailableModel,
    affordable_output_tokens: Option<u64>,
) -> u64 {
    let model_limit = config.max_output_tokens.unwrap_or(u64::MAX);
    if let Some(chosen) = requested.or(config.max_completion_tokens) {
        return chosen.min(model_limit);
    }
    let default = match config.max_output_tokens {
        Some(model_limit) => model_limit.min(DEFAULT_OUTPUT_TOKEN_CEILING),
        None => UNKNOWN_MODEL_OUTPUT_TOKENS,
    };
    match affordable_output_tokens {
        Some(affordable) => default.min(affordable.max(MIN_OUTPUT_TOKEN_RESERVATION)),
        None => default,
    }
}

/// "$20.50 left", with "· low" under a dollar.
fn balance_detail(balance: f64) -> SharedString {
    let amount = if balance < 0.01 {
        "under $0.01".to_string()
    } else {
        format!("${balance:.2}")
    };
    if balance < LOW_BALANCE_THRESHOLD {
        format!("{amount} left · low").into()
    } else {
        format!("{amount} left").into()
    }
}

fn pick_default_model(models: &BTreeMap<String, UnalignedModel>) -> Option<&UnalignedModel> {
    pick_preferred(models, DEFAULT_MODEL_PREFERENCE).or_else(|| {
        models
            .values()
            .filter(|model| model.is_usable())
            .min_by_key(|model| (model.display_name().to_lowercase(), &model.config.name))
    })
}

fn pick_fast_model(models: &BTreeMap<String, UnalignedModel>) -> Option<&UnalignedModel> {
    pick_preferred(models, FAST_MODEL_PREFERENCE).or_else(|| {
        models
            .values()
            .filter(|model| model.is_usable())
            .filter_map(|model| Some((model.details.price?.input, model)))
            .min_by(|(a, _), (b, _)| a.total_cmp(b))
            .map(|(_, model)| model)
    })
}

fn pick_preferred<'a>(
    models: &'a BTreeMap<String, UnalignedModel>,
    preference: &[&str],
) -> Option<&'a UnalignedModel> {
    preference
        .iter()
        .filter_map(|id| models.get(*id))
        .find(|model| model.is_usable())
}

fn map_open_ai_error(error: open_ai::RequestError) -> LanguageModelCompletionError {
    match error {
        open_ai::RequestError::HttpResponseError {
            status_code,
            body,
            headers,
            ..
        } => {
            let retry_after = headers
                .get(http::header::RETRY_AFTER)
                .and_then(|value| value.to_str().ok()?.parse::<u64>().ok())
                .map(std::time::Duration::from_secs);

            LanguageModelCompletionError::from_http_status(
                PROVIDER_NAME,
                status_code,
                extract_error_message(&body),
                retry_after,
            )
        }
        error => error.into(),
    }
}

fn extract_error_message(body: &str) -> String {
    let json = match serde_json::from_str::<serde_json::Value>(body) {
        Ok(json) => json,
        Err(_) => return body.to_string(),
    };

    let message = json
        .get("error")
        .and_then(|value| {
            value
                .get("message")
                .and_then(serde_json::Value::as_str)
                .or_else(|| value.as_str())
        })
        .or_else(|| json.get("message").and_then(serde_json::Value::as_str))
        .map(ToString::to_string)
        .unwrap_or_else(|| body.to_string());

    message
}

fn language_model(model: &UnalignedModel) -> LanguageModel {
    let details = &model.details;
    let model = &model.config;
    LanguageModel {
        cost_info: details.price.map(|price| LanguageModelCostInfo::TokenCost {
            input_token_cost_per_1m: price.input,
            output_token_cost_per_1m: price.output,
        }),
        disabled_reason: (details.status == ModelStatus::Down)
            .then(|| DisabledReason::new(MODEL_DOWN_REASON)),
        supports_tools: model.capabilities.tools,
        supports_images: model.capabilities.images,
        tool_choice_support: LanguageModelToolChoiceSupport {
            auto: model.capabilities.tools,
            any: model.capabilities.tools,
            none: true,
        },
        supports_streaming_tools: true,
        supports_split_token_display: true,
        // The model's true limit. What a request reserves is decided when it
        // is sent (`output_token_limit`).
        max_output_tokens: model.max_output_tokens,
        ..LanguageModel::new(
            LanguageModelId::from(model.name.clone()),
            LanguageModelName::from(
                model
                    .display_name
                    .clone()
                    .unwrap_or_else(|| model.name.clone()),
            ),
            PROVIDER_ID,
            PROVIDER_NAME,
            format!("unaligned/{}", model.name),
            model.max_tokens,
        )
    }
}

#[derive(Deserialize)]
struct ModelsResponse {
    models: Vec<ApiModel>,
}

/// One entry of `GET /v1/models`, the API's own list: unlike the OpenAI-shaped
/// one, it carries the context window, the prices, whether the model is up
/// and what it can do.
#[derive(Deserialize)]
struct ApiModel {
    id: String,
    name: Option<String>,
    org: Option<String>,
    context: Option<u64>,
    max_output: Option<u64>,
    price: Option<ApiPrice>,
    status: Option<ModelStatus>,
    #[serde(default)]
    vision: bool,
    #[serde(default)]
    reasoning: bool,
    #[serde(default)]
    uncensored: bool,
}

/// US dollars per million tokens. (The API also sends `tokens`, which is not
/// a unit and is not read.)
#[derive(Deserialize)]
struct ApiPrice {
    input: Option<f64>,
    output: Option<f64>,
    cached: Option<f64>,
}

#[derive(Deserialize)]
struct BalanceResponse {
    balance: f64,
}

enum BalanceError {
    /// The server answered 401: it does not accept this key.
    Refused,
    /// Anything else: no network, a server error, an answer that makes no
    /// sense. Says nothing about the key.
    Unreachable(anyhow::Error),
}

/// `GET /v1/balance`: what is left on the account or, for a reseller's
/// customer key, on that key.
async fn fetch_balance(
    client: &dyn HttpClient,
    api_url: &str,
    api_key: &str,
    extra_headers: &CustomHeaders,
) -> Result<f64, BalanceError> {
    let unreachable = |error: anyhow::Error| BalanceError::Unreachable(error);
    let uri = format!("{}/v1/balance", api_url.trim_end_matches('/'));
    let request = HttpRequest::builder()
        .method(Method::GET)
        .uri(uri)
        .header("Accept", "application/json")
        .header("Authorization", format!("Bearer {}", api_key))
        .extra_headers(extra_headers)
        .body(AsyncBody::default())
        .map_err(|error| unreachable(error.into()))?;
    let mut response = client.send(request).await.map_err(unreachable)?;

    let status = response.status();
    if status == http::StatusCode::UNAUTHORIZED {
        return Err(BalanceError::Refused);
    }
    let mut body = String::new();
    response
        .body_mut()
        .read_to_string(&mut body)
        .await
        .map_err(|error| unreachable(error.into()))?;
    if !status.is_success() {
        return Err(unreachable(anyhow!(
            "the balance request failed with status {status}: {}",
            extract_error_message(&body)
        )));
    }
    let response: BalanceResponse =
        serde_json::from_str(&body).map_err(|error| unreachable(error.into()))?;
    Ok(response.balance)
}

fn parse_models(body: &str) -> serde_json::Result<Vec<UnalignedModel>> {
    let response: ModelsResponse = serde_json::from_str(body)?;
    Ok(response
        .models
        .into_iter()
        .map(|model| UnalignedModel {
            details: UnalignedModelDetails {
                org: model.org.map(SharedString::from),
                status: model.status.unwrap_or_default(),
                price: model.price.and_then(|price| {
                    Some(ModelPrice {
                        input: price.input?,
                        output: price.output?,
                        cached: price.cached,
                    })
                }),
                reasoning: model.reasoning,
                uncensored: model.uncensored,
            },
            config: AvailableModel {
                name: model.id.clone(),
                display_name: model.name.or(Some(model.id)),
                max_tokens: model.context.unwrap_or(128_000),
                max_output_tokens: model.max_output,
                max_completion_tokens: None,
                capabilities: ModelCapabilities {
                    tools: true,
                    images: model.vision,
                    parallel_tool_calls: false,
                    prompt_cache_key: false,
                    chat_completions: true,
                    interleaved_reasoning: false,
                    max_tokens_parameter: false,
                },
            },
        })
        .collect())
}

async fn list_models(
    client: &dyn HttpClient,
    api_url: &str,
    api_key: Option<&str>,
    extra_headers: &CustomHeaders,
) -> Result<Vec<UnalignedModel>, LanguageModelCompletionError> {
    let uri = format!("{}/v1/models", api_url.trim_end_matches('/'));
    let mut request_builder = HttpRequest::builder()
        .method(Method::GET)
        .uri(uri)
        .header("Accept", "application/json");
    if let Some(api_key) = api_key {
        request_builder = request_builder.header("Authorization", format!("Bearer {}", api_key));
    }
    let request = request_builder
        .extra_headers(extra_headers)
        .body(AsyncBody::default())
        .map_err(|error| LanguageModelCompletionError::BuildRequestBody {
            provider: PROVIDER_NAME,
            error,
        })?;
    let host = request.uri().host().unwrap_or(api_url).to_owned();
    let mut response =
        client
            .send(request)
            .await
            .map_err(|error| LanguageModelCompletionError::HttpSend {
                provider: PROVIDER_NAME,
                host,
                error,
            })?;

    let mut body = String::new();
    response
        .body_mut()
        .read_to_string(&mut body)
        .await
        .map_err(|error| LanguageModelCompletionError::ApiReadResponseError {
            provider: PROVIDER_NAME,
            error,
        })?;

    if !response.status().is_success() {
        return Err(LanguageModelCompletionError::from_http_status(
            PROVIDER_NAME,
            response.status(),
            extract_error_message(&body),
            None,
        ));
    }

    parse_models(&body).map_err(|error| LanguageModelCompletionError::DeserializeResponse {
        provider: PROVIDER_NAME,
        error,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    struct TestCredentialsProvider;

    impl CredentialsProvider for TestCredentialsProvider {
        fn read_credentials<'a>(
            &'a self,
            _url: &'a str,
            _cx: &'a AsyncApp,
        ) -> std::pin::Pin<
            Box<dyn std::future::Future<Output = Result<Option<(String, Vec<u8>)>>> + 'a>,
        > {
            Box::pin(async { Ok(None) })
        }

        fn write_credentials<'a>(
            &'a self,
            _url: &'a str,
            _username: &'a str,
            _password: &'a [u8],
            _cx: &'a AsyncApp,
        ) -> std::pin::Pin<Box<dyn std::future::Future<Output = Result<()>> + 'a>> {
            Box::pin(async { Ok(()) })
        }

        fn delete_credentials<'a>(
            &'a self,
            _url: &'a str,
            _cx: &'a AsyncApp,
        ) -> std::pin::Pin<Box<dyn std::future::Future<Output = Result<()>> + 'a>> {
            Box::pin(async { Ok(()) })
        }
    }

    const MODELS: &str = r#"{"models":[
        {"id":"claude-sonnet-5-5","name":"Claude Sonnet 5.5","org":"Anthropic","context":400000,"max_output":64000,
         "price":{"input":1.25,"output":6.25,"cached":0.125,"tokens":1.25},"status":"down","vision":true,"reasoning":true,"uncensored":false},
        {"id":"claude-sonnet-5","name":"Claude Sonnet 5","org":"Anthropic","context":400000,"max_output":64000,
         "price":{"input":1.125,"output":5.625,"cached":0.1125,"tokens":1.125},"status":"up","vision":true,"reasoning":true,"uncensored":false},
        {"id":"deepseek-v4.1-flash","name":"DeepSeek V4.1 Flash","org":"DeepSeek","context":128000,"max_output":16384,
         "price":{"input":0.05,"output":0.2,"cached":0.005,"tokens":0.05},"status":"degraded","vision":false,"reasoning":true,"uncensored":false},
        {"id":"glm-5.3-uncensored","name":"GLM-5.3 Uncensored","org":"Zhipu AI","context":1000000,"max_output":8000,
         "status":"being-moved","uncensored":true},
        {"id":"bare"}
    ]}"#;

    fn json_response(
        status: u16,
        body: &'static str,
    ) -> anyhow::Result<http_client::Response<AsyncBody>> {
        Ok(http_client::Response::builder()
            .status(status)
            .header("Content-Type", "application/json")
            .body(AsyncBody::from(body))?)
    }

    /// A provider whose server answers the model list and leaves the balance
    /// to `balance`, which is given the key the request carried.
    fn provider_with_balance(
        cx: &mut gpui::TestAppContext,
        balance: impl Fn(&str) -> anyhow::Result<http_client::Response<AsyncBody>>
        + Send
        + Sync
        + 'static,
    ) -> UnalignedLanguageModelProvider {
        provider_with_credentials(cx, Arc::new(TestCredentialsProvider), balance)
    }

    /// A machine with no keychain service (Linux without a Secret Service):
    /// nothing can be read, written or deleted.
    struct NoKeyringCredentialsProvider;

    impl CredentialsProvider for NoKeyringCredentialsProvider {
        fn read_credentials<'a>(
            &'a self,
            _url: &'a str,
            _cx: &'a AsyncApp,
        ) -> std::pin::Pin<
            Box<dyn std::future::Future<Output = Result<Option<(String, Vec<u8>)>>> + 'a>,
        > {
            Box::pin(async { Err(anyhow::anyhow!("no keyring service")) })
        }

        fn write_credentials<'a>(
            &'a self,
            _url: &'a str,
            _username: &'a str,
            _password: &'a [u8],
            _cx: &'a AsyncApp,
        ) -> std::pin::Pin<Box<dyn std::future::Future<Output = Result<()>> + 'a>> {
            Box::pin(async { Err(anyhow::anyhow!("no keyring service")) })
        }

        fn delete_credentials<'a>(
            &'a self,
            _url: &'a str,
            _cx: &'a AsyncApp,
        ) -> std::pin::Pin<Box<dyn std::future::Future<Output = Result<()>> + 'a>> {
            Box::pin(async { Err(anyhow::anyhow!("no keyring service")) })
        }
    }

    // Unaligned: a key the keychain cannot store is still used, from memory,
    // until the app quits, and the provider says so (the connection screen
    // shows "This key is kept for this session only…").
    #[gpui::test]
    async fn key_is_used_for_the_session_when_no_keyring_stores_it(cx: &mut gpui::TestAppContext) {
        let provider =
            provider_with_credentials(cx, Arc::new(NoKeyringCredentialsProvider), |key| {
                assert_eq!(key, "test-key", "the balance request carries the key");
                json_response(200, r#"{"balance":20.5}"#)
            });
        cx.read(|cx| assert!(!provider.key_is_session_only(cx)));

        let status = connect(&provider, "test-key", cx).await;

        assert_eq!(
            status,
            ProviderConnectionStatus::Connected {
                detail: Some("$20.50 left".into()),
            }
        );
        cx.read(|cx| {
            assert!(provider.is_authenticated(cx));
            assert!(provider.key_is_session_only(cx));
        });

        // Disconnecting forgets it.
        cx.update(|cx| provider.set_api_key(None, cx))
            .await
            .unwrap();
        cx.run_until_parked();
        cx.read(|cx| {
            assert!(!provider.is_authenticated(cx));
            assert!(!provider.key_is_session_only(cx));
        });
    }

    #[gpui::test]
    async fn a_stored_key_is_not_session_only(cx: &mut gpui::TestAppContext) {
        let provider = provider_with_balance(cx, |_| json_response(200, r#"{"balance":20.5}"#));
        connect(&provider, "test-key", cx).await;
        cx.read(|cx| assert!(!provider.key_is_session_only(cx)));
    }

    fn provider_with_credentials(
        cx: &mut gpui::TestAppContext,
        credentials: Arc<dyn CredentialsProvider>,
        balance: impl Fn(&str) -> anyhow::Result<http_client::Response<AsyncBody>>
        + Send
        + Sync
        + 'static,
    ) -> UnalignedLanguageModelProvider {
        cx.update(|cx| {
            let settings_store = SettingsStore::test(cx);
            cx.set_global(settings_store);
        });
        let balance = Arc::new(balance);
        let http_client = http_client::FakeHttpClient::create(move |request| {
            let balance = balance.clone();
            async move {
                match request.uri().path() {
                    "/v1/models" => json_response(200, MODELS),
                    "/v1/balance" => {
                        let key = request
                            .headers()
                            .get("Authorization")
                            .and_then(|value| value.to_str().ok())
                            .and_then(|value| value.strip_prefix("Bearer "))
                            .unwrap_or_default()
                            .to_string();
                        balance(&key)
                    }
                    _ => json_response(404, r#"{"error":{"message":"not found"}}"#),
                }
            }
        });
        cx.update(|cx| UnalignedLanguageModelProvider::new(http_client, credentials, cx))
    }

    async fn connect(
        provider: &UnalignedLanguageModelProvider,
        key: &str,
        cx: &mut gpui::TestAppContext,
    ) -> ProviderConnectionStatus {
        cx.update(|cx| provider.set_api_key(Some(key.to_string()), cx))
            .await
            .unwrap();
        let status = cx.update(|cx| provider.check_connection(cx)).await.unwrap();
        cx.run_until_parked();
        status
    }

    fn models(body: &str) -> BTreeMap<String, UnalignedModel> {
        parse_models(body)
            .unwrap()
            .into_iter()
            .map(|model| (model.config.name.clone(), model))
            .collect()
    }

    #[gpui::test]
    async fn accepted_key_is_connected_with_its_balance(cx: &mut gpui::TestAppContext) {
        let provider = provider_with_balance(cx, |key| {
            assert_eq!(key, "test-key", "the balance request carries the key");
            json_response(200, r#"{"balance":20.499949}"#)
        });
        cx.read(|cx| {
            assert_eq!(
                provider.connection_status(cx),
                ProviderConnectionStatus::NotConnected
            );
            assert_eq!(provider.balance(cx), None);
        });

        let status = connect(&provider, "test-key", cx).await;

        let connected = ProviderConnectionStatus::Connected {
            detail: Some("$20.50 left".into()),
        };
        assert_eq!(status, connected);
        cx.read(|cx| {
            assert_eq!(provider.connection_status(cx), connected);
            assert_eq!(provider.balance(cx), Some(20.499949));
        });
    }

    #[gpui::test]
    async fn refused_key_needs_attention_and_is_kept(cx: &mut gpui::TestAppContext) {
        let provider = provider_with_balance(cx, |_| {
            json_response(
                401,
                r#"{"error":{"code":"unauthorized","message":"Missing or invalid credentials."}}"#,
            )
        });

        let status = connect(&provider, "test-key", cx).await;

        let refused = ProviderConnectionStatus::NeedsAttention {
            reason: "This key was refused.".into(),
        };
        assert_eq!(status, refused);
        cx.read(|cx| {
            assert_eq!(provider.connection_status(cx), refused);
            assert_eq!(provider.balance(cx), None);
            assert!(
                provider.is_authenticated(cx),
                "a refused key stays stored: the user decides what to do with it"
            );
        });
        let refresh = cx.update(|cx| provider.refresh_balance(cx)).await;
        assert!(refresh.is_err(), "a refresh with a refused key fails");
    }

    #[gpui::test]
    async fn unreachable_server_does_not_blame_the_key(cx: &mut gpui::TestAppContext) {
        let unreachable = ProviderConnectionStatus::NeedsAttention {
            reason: "Could not reach Unaligned.".into(),
        };

        let provider =
            provider_with_balance(cx, |_| Err(anyhow::anyhow!("connection reset by peer")));
        assert_eq!(connect(&provider, "test-key", cx).await, unreachable);
        cx.read(|cx| assert!(provider.is_authenticated(cx)));

        let provider = provider_with_balance(cx, |_| {
            json_response(500, r#"{"error":{"message":"Internal server error."}}"#)
        });
        assert_eq!(connect(&provider, "test-key", cx).await, unreachable);

        let provider = provider_with_balance(cx, |_| json_response(200, "<html>a proxy</html>"));
        assert_eq!(connect(&provider, "test-key", cx).await, unreachable);
    }

    #[gpui::test]
    async fn a_new_key_replaces_what_was_known_of_the_old_one(cx: &mut gpui::TestAppContext) {
        let provider = provider_with_balance(cx, |key| match key {
            "good-key" => json_response(200, r#"{"balance":0.42}"#),
            _ => json_response(401, "{}"),
        });

        let status = connect(&provider, "old-key", cx).await;
        assert!(matches!(
            status,
            ProviderConnectionStatus::NeedsAttention { .. }
        ));

        let status = connect(&provider, "good-key", cx).await;
        assert_eq!(
            status,
            ProviderConnectionStatus::Connected {
                detail: Some("$0.42 left · low".into())
            }
        );
        cx.update(|cx| provider.refresh_balance(cx)).await.unwrap();
        cx.read(|cx| assert_eq!(provider.balance(cx), Some(0.42)));

        cx.update(|cx| provider.set_api_key(None, cx))
            .await
            .unwrap();
        cx.run_until_parked();
        cx.read(|cx| {
            assert_eq!(
                provider.connection_status(cx),
                ProviderConnectionStatus::NotConnected
            );
            assert_eq!(provider.balance(cx), None);
            assert!(provider.provided_models(cx).is_empty());
        });
    }

    #[gpui::test]
    async fn a_stored_key_is_connected_until_the_server_answers(cx: &mut gpui::TestAppContext) {
        cx.update(|cx| {
            let settings_store = SettingsStore::test(cx);
            cx.set_global(settings_store);
        });
        let http_client = http_client::FakeHttpClient::create(|request| async move {
            match request.uri().path() {
                "/v1/models" => json_response(200, MODELS),
                _ => futures::future::pending().await,
            }
        });
        let provider = cx.update(|cx| {
            UnalignedLanguageModelProvider::new(http_client, Arc::new(TestCredentialsProvider), cx)
        });

        cx.update(|cx| provider.set_api_key(Some("test-key".to_string()), cx))
            .await
            .unwrap();
        cx.run_until_parked();

        cx.read(|cx| {
            assert_eq!(
                provider.connection_status(cx),
                ProviderConnectionStatus::Connected { detail: None }
            );
        });
    }

    #[gpui::test]
    async fn model_list_error_is_reported_when_no_model_is_known(cx: &mut gpui::TestAppContext) {
        cx.update(|cx| {
            let settings_store = SettingsStore::test(cx);
            cx.set_global(settings_store);
        });
        let http_client = http_client::FakeHttpClient::create(|request| async move {
            match request.uri().path() {
                "/v1/balance" => json_response(200, r#"{"balance":3}"#),
                _ => json_response(503, r#"{"error":{"message":"maintenance"}}"#),
            }
        });
        let provider = cx.update(|cx| {
            UnalignedLanguageModelProvider::new(http_client, Arc::new(TestCredentialsProvider), cx)
        });

        connect(&provider, "test-key", cx).await;

        let status = cx.read(|cx| provider.connection_status(cx));
        let ProviderConnectionStatus::NeedsAttention { reason } = status else {
            panic!("expected the model list error, got {status:?}");
        };
        assert!(
            reason.starts_with("Could not load the model list: ") && reason.contains("maintenance"),
            "unexpected reason: {reason}"
        );
    }

    #[gpui::test]
    async fn fetched_models_carry_price_and_status(cx: &mut gpui::TestAppContext) {
        let provider =
            provider_with_balance(cx, |_| json_response(200, r#"{"balance":20.499949}"#));
        connect(&provider, "test-key", cx).await;

        cx.read(|cx| {
            let models = provider.provided_models(cx);
            let model = |id: &str| {
                models
                    .iter()
                    .find(|model| model.id.0.as_ref() == id)
                    .unwrap_or_else(|| panic!("no model {id}"))
            };

            let down = model("claude-sonnet-5-5");
            assert_eq!(
                down.disabled_reason,
                Some(DisabledReason::new("Temporarily unavailable"))
            );
            assert_eq!(
                down.cost_info,
                Some(LanguageModelCostInfo::TokenCost {
                    input_token_cost_per_1m: 1.25,
                    output_token_cost_per_1m: 6.25,
                })
            );
            assert_eq!(down.max_output_tokens, Some(64_000));
            assert!(down.supports_images);

            let degraded = model("deepseek-v4.1-flash");
            assert_eq!(degraded.disabled_reason, None);
            assert_eq!(
                degraded.cost_info.as_ref().unwrap().to_shared_string(),
                "$0.05/$0.20"
            );
            assert!(!degraded.supports_images);
            assert_eq!(model("bare").cost_info, None);

            // Sonnet 5.5 is down: the next preferred model takes its place.
            assert_eq!(
                provider.default_model(cx).unwrap().id.0.as_ref(),
                "claude-sonnet-5"
            );
            assert_eq!(
                provider.default_fast_model(cx).unwrap().id.0.as_ref(),
                "deepseek-v4.1-flash"
            );

            let details = provider
                .model_details(&LanguageModelId::from("glm-5.3-uncensored".to_string()), cx)
                .unwrap();
            assert_eq!(details.org.as_deref(), Some("Zhipu AI"));
            assert_eq!(details.status, ModelStatus::Unknown);
            assert_eq!(details.price, None);
            assert!(details.uncensored);
            assert!(!details.reasoning);

            // 1M input, 2M cached input, 0.5M output on Sonnet 5.5.
            let usage = TokenUsage {
                input_tokens: 1_000_000,
                output_tokens: 500_000,
                cache_creation_input_tokens: 0,
                cache_read_input_tokens: 2_000_000,
            };
            let cost = provider
                .usage_cost(
                    &LanguageModelId::from("claude-sonnet-5-5".to_string()),
                    &usage,
                    cx,
                )
                .unwrap();
            assert!((cost - (1.25 + 2.0 * 0.125 + 0.5 * 6.25)).abs() < 1e-9);
            assert_eq!(
                provider.usage_cost(&LanguageModelId::from("bare".to_string()), &usage, cx),
                None
            );
        });
    }

    #[test]
    fn model_list_parsing_reads_price_status_and_flags() {
        let models = models(MODELS);
        assert_eq!(models.len(), 5);

        let sonnet = &models["claude-sonnet-5-5"];
        assert_eq!(
            sonnet.config.display_name.as_deref(),
            Some("Claude Sonnet 5.5")
        );
        assert_eq!(sonnet.config.max_tokens, 400_000);
        assert_eq!(sonnet.config.max_output_tokens, Some(64_000));
        assert_eq!(
            sonnet.details,
            UnalignedModelDetails {
                org: Some("Anthropic".into()),
                status: ModelStatus::Down,
                price: Some(ModelPrice {
                    input: 1.25,
                    output: 6.25,
                    cached: Some(0.125),
                }),
                reasoning: true,
                uncensored: false,
            }
        );
        assert_eq!(models["claude-sonnet-5"].details.status, ModelStatus::Up);
        assert_eq!(
            models["deepseek-v4.1-flash"].details.status,
            ModelStatus::Degraded
        );
        // A status this build does not know is not an error.
        assert_eq!(
            models["glm-5.3-uncensored"].details.status,
            ModelStatus::Unknown
        );

        let bare = &models["bare"];
        assert_eq!(bare.config.display_name.as_deref(), Some("bare"));
        assert_eq!(bare.config.max_tokens, 128_000);
        assert_eq!(bare.details, UnalignedModelDetails::default());
    }

    #[test]
    fn default_models_skip_what_is_down() {
        let list = |entries: &[(&str, &str, f64)]| {
            let entries = entries
                .iter()
                .map(|(id, status, input)| {
                    format!(
                        r#"{{"id":"{id}","name":"Model {id}","status":"{status}","price":{{"input":{input},"output":1.0}}}}"#
                    )
                })
                .collect::<Vec<_>>()
                .join(",");
            models(&format!(r#"{{"models":[{entries}]}}"#))
        };
        let default = |models: &BTreeMap<String, UnalignedModel>| {
            pick_default_model(models).map(|model| model.config.name.clone())
        };
        let fast = |models: &BTreeMap<String, UnalignedModel>| {
            pick_fast_model(models).map(|model| model.config.name.clone())
        };

        let all_up = list(&[
            ("zeta", "up", 0.01),
            ("claude-sonnet-5", "up", 1.0),
            ("claude-sonnet-5-5", "up", 1.25),
            ("gpt-5.5", "up", 0.75),
            ("claude-haiku-5-5", "up", 0.4),
            ("deepseek-v4.1-flash", "up", 0.05),
        ]);
        assert_eq!(default(&all_up).as_deref(), Some("claude-sonnet-5-5"));
        assert_eq!(fast(&all_up).as_deref(), Some("deepseek-v4.1-flash"));

        let preferred_down = list(&[
            ("zeta", "up", 0.01),
            ("claude-sonnet-5", "down", 1.0),
            ("claude-sonnet-5-5", "down", 1.25),
            ("gpt-5.5", "degraded", 0.75),
            ("claude-haiku-5-5", "unknown", 0.4),
            ("deepseek-v4.1-flash", "down", 0.05),
        ]);
        assert_eq!(default(&preferred_down).as_deref(), Some("gpt-5.5"));
        assert_eq!(fast(&preferred_down).as_deref(), Some("claude-haiku-5-5"));

        // None of the preferred ids: first by name, cheapest by input price,
        // never a model that is down.
        let others = list(&[
            ("alpha", "down", 0.001),
            ("beta", "up", 0.3),
            ("gamma", "up", 0.02),
        ]);
        assert_eq!(default(&others).as_deref(), Some("beta"));
        assert_eq!(fast(&others).as_deref(), Some("gamma"));

        let all_down = list(&[("claude-sonnet-5-5", "down", 1.25)]);
        assert_eq!(default(&all_down), None);
        assert_eq!(fast(&all_down), None);
    }

    #[test]
    fn requests_reserve_a_modest_output_by_default() {
        let models = models(MODELS);
        let sonnet = &models["claude-sonnet-5-5"].config;
        let small = &models["glm-5.3-uncensored"].config;
        let bare = &models["bare"].config;

        // Without a word from the request: the model's own limit, or a modest
        // figure when the API does not give one.
        assert_eq!(output_token_limit(None, sonnet, None), 64_000);
        assert_eq!(output_token_limit(None, small, None), 8_000);
        assert_eq!(output_token_limit(None, bare, None), 16_000);
        // A low balance holds the default to what it can pay for, never under
        // the floor; a comfortable one changes nothing.
        assert_eq!(output_token_limit(None, sonnet, Some(30_000)), 30_000);
        assert_eq!(output_token_limit(None, sonnet, Some(500)), 2_000);
        assert_eq!(output_token_limit(None, sonnet, Some(5_000_000)), 64_000);
        // A request that names a limit gets it, within the model's own.
        assert_eq!(output_token_limit(Some(40_000), sonnet, None), 40_000);
        assert_eq!(output_token_limit(Some(1_024), sonnet, None), 1_024);
        assert_eq!(output_token_limit(Some(100_000), sonnet, None), 64_000);
        assert_eq!(output_token_limit(Some(40_000), sonnet, Some(500)), 40_000);
        // A settings entry can choose another default for its model.
        let configured = AvailableModel {
            max_completion_tokens: Some(32_000),
            ..sonnet.clone()
        };
        assert_eq!(output_token_limit(None, &configured, None), 32_000);

        let request = into_request(LanguageModelRequest::default(), sonnet, None).unwrap();
        assert_eq!(request.max_completion_tokens, Some(64_000));
        // $1.00 at $15 a million output tokens, nothing to read: 90% of the
        // balance pays for 60,000 tokens.
        let price = ModelPrice {
            input: 3.0,
            output: 15.0,
            cached: None,
        };
        assert_eq!(
            affordable_output_tokens(1.0, &price, &LanguageModelRequest::default()),
            Some(60_000)
        );
        assert_eq!(request.max_tokens, None);
        let request = into_request(
            LanguageModelRequest {
                max_output_tokens: Some(40_000),
                ..Default::default()
            },
            sonnet,
            None,
        )
        .unwrap();
        assert_eq!(request.max_completion_tokens, Some(40_000));
        // The model itself still reports its true limit.
        assert_eq!(
            language_model(&models["claude-sonnet-5-5"]).max_output_tokens,
            Some(64_000)
        );
    }

    #[test]
    fn balance_detail_wording() {
        assert_eq!(balance_detail(20.499949), "$20.50 left");
        assert_eq!(balance_detail(1.0), "$1.00 left");
        assert_eq!(balance_detail(0.42), "$0.42 left · low");
        assert_eq!(balance_detail(0.01), "$0.01 left · low");
        assert_eq!(balance_detail(0.004), "under $0.01 left · low");
        assert_eq!(balance_detail(0.0), "under $0.01 left · low");
    }

    #[gpui::test]
    async fn restart_fetch_models_task_records_error_on_failure(cx: &mut gpui::TestAppContext) {
        cx.update(|cx| {
            let settings_store = SettingsStore::test(cx);
            cx.set_global(settings_store);
        });

        let http_client = http_client::FakeHttpClient::create(|_request| async move {
            Ok(http_client::Response::builder()
                .status(401)
                .body(AsyncBody::from(r#"{"error":{"message":"invalid key"}}"#))?)
        });
        let provider = cx.update(|cx| {
            UnalignedLanguageModelProvider::new(http_client, Arc::new(TestCredentialsProvider), cx)
        });

        cx.update(|cx| provider.set_api_key(Some("test-key".to_string()), cx))
            .await
            .unwrap();
        cx.update(|cx| {
            provider
                .state
                .update(cx, |state, cx| state.restart_fetch_models_task(cx))
        });
        cx.run_until_parked();

        let error = cx.read(|cx| provider.state.read(cx).last_fetch_models_error.clone());
        assert!(
            error.is_some(),
            "a failed model fetch should be recorded, not silently dropped"
        );
    }
}

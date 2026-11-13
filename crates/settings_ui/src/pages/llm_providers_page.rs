use std::{collections::HashSet, sync::Arc};

use collections::HashMap;
use editor::Editor;
use gpui::{AnyView, Entity, Focusable as _, Global, PromptLevel, ScrollHandle, prelude::*};
use language_model::{
    ApiKeyConfiguration, CreateProviderSettingsView, IDE_CLOUD_PROVIDER_ID, IconOrSvg,
    InlineDescription, LanguageModelProvider, LanguageModelProviderId, LanguageModelRegistry,
    ProviderConnectionStatus, ProviderSettingsView,
};

use settings::{
    AnthropicCompatibleAvailableModel, AnthropicCompatibleModelCapabilities,
    AnthropicCompatibleSettingsContent, OpenAiCompatibleAvailableModel,
    OpenAiCompatibleModelCapabilities, OpenAiCompatibleSettingsContent, OpenAiReasoningEffort,
};
use ui::{
    ButtonLink, Checkbox, ContextMenu, Divider, DropdownMenu, DropdownStyle, IconPosition,
    Indicator, PopoverMenu, SectionLabel, ToggleState, prelude::*,
};
use util::ResultExt as _;

use crate::SettingsWindow;
use crate::components::SettingsInputField;

// The groups, their sentences and the row titles of the chat's connection
// screen (`agent_ui::connections` and `connect_form`). This crate cannot depend
// on `agent_ui`, so the words are repeated here on purpose: change them there
// and here together. Rows are matched by provider id.
const UNALIGNED_PROVIDER_ID: &str = "unaligned";
const UNALIGNED_NOTE: &str = "One key for every model. Pay as you go, no subscription.";
const SUBSCRIPTION_SIGN_IN_NOTE: &str =
    "Sign in with the plan you already pay for. Its models answer in the chat.";
const SUBSCRIPTION_PROVIDERS: &[(&str, &str)] = &[
    ("openai-subscribed", "ChatGPT Plus or Pro"),
    ("copilot_chat", "GitHub Copilot"),
    ("x_ai_subscribed", "SuperGrok"),
];
const OWN_KEY_OR_LOCAL_PROVIDERS: &[(&str, &str)] = &[
    ("anthropic", "Anthropic"),
    ("openai", "OpenAI"),
    ("google", "Google"),
    ("openrouter", "OpenRouter"),
    ("ollama", "Ollama"),
    ("lmstudio", "LM Studio"),
    ("llama.cpp", "llama.cpp"),
];
const MORE_KEY_PROVIDERS: &[(&str, &str)] = &[
    ("deepseek", "DeepSeek"),
    ("mistral", "Mistral"),
    ("x_ai", "xAI"),
];

#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
enum ProviderGroup {
    Unaligned,
    Subscription,
    OwnKeyOrLocal,
    More,
}

impl ProviderGroup {
    const ALL: [Self; 4] = [
        Self::Unaligned,
        Self::Subscription,
        Self::OwnKeyOrLocal,
        Self::More,
    ];

    fn title(self) -> &'static str {
        match self {
            Self::Unaligned => "Unaligned · recommended",
            Self::Subscription => "Use a subscription you already have",
            Self::OwnKeyOrLocal => "Use your own key or a local model",
            Self::More => "More",
        }
    }

    fn note(self) -> Option<&'static str> {
        match self {
            Self::Unaligned | Self::More => None,
            Self::Subscription => Some("You keep paying your plan, nothing more."),
            Self::OwnKeyOrLocal => Some(
                "A key is billed by the company that issued it. A local model is free \
                and stays on this machine.",
            ),
        }
    }
}

/// Where a provider goes on the page: its group, its rank inside the group
/// (`None` after the known ones) and the title the connection screen gives it
/// (`None` for the provider's own name).
fn provider_place(
    id: &str,
    is_custom_endpoint: bool,
) -> (ProviderGroup, Option<usize>, Option<&'static str>) {
    let known = |list: &'static [(&'static str, &'static str)]| {
        list.iter()
            .position(|(known_id, _)| *known_id == id)
            .map(|index| (index, list[index].1))
    };
    if id == UNALIGNED_PROVIDER_ID {
        (ProviderGroup::Unaligned, Some(0), Some("Unaligned"))
    } else if let Some((index, title)) = known(SUBSCRIPTION_PROVIDERS) {
        (ProviderGroup::Subscription, Some(index), Some(title))
    } else if let Some((index, title)) = known(OWN_KEY_OR_LOCAL_PROVIDERS) {
        (ProviderGroup::OwnKeyOrLocal, Some(index), Some(title))
    } else if is_custom_endpoint {
        (ProviderGroup::OwnKeyOrLocal, None, None)
    } else if let Some((index, title)) = known(MORE_KEY_PROVIDERS) {
        (ProviderGroup::More, Some(index), Some(title))
    } else {
        (ProviderGroup::More, None, None)
    }
}

/// The rows of the page as `(group, id, name)`, in the connection screen's
/// order: the known providers of a group as listed above, then the custom
/// endpoints as registered, and under "More" every other provider by name.
fn ordered_rows<T>(
    providers: Vec<T>,
    id_and_name: impl Fn(&T) -> (SharedString, SharedString),
    custom_endpoints: &HashSet<SharedString>,
) -> Vec<(ProviderGroup, SharedString, T)> {
    let mut rows = providers
        .into_iter()
        .map(|provider| {
            let (id, name) = id_and_name(&provider);
            let (group, rank, title) = provider_place(&id, custom_endpoints.contains(&id));
            let by_name = match (group, rank) {
                (ProviderGroup::More, None) => name.to_lowercase(),
                _ => String::new(),
            };
            let title = title.map_or(name, SharedString::new_static);
            (
                (group, rank.unwrap_or(usize::MAX), by_name),
                title,
                provider,
            )
        })
        .collect::<Vec<_>>();
    rows.sort_by(|a, b| a.0.cmp(&b.0));
    rows.into_iter()
        .map(|((group, ..), title, provider)| (group, title, provider))
        .collect()
}

/// The names of the custom endpoints in the settings: each is a provider
/// whose id is that name.
fn custom_endpoint_ids(cx: &App) -> HashSet<SharedString> {
    let Some(language_models) = cx
        .global::<settings::SettingsStore>()
        .merged_settings()
        .language_models
        .as_ref()
    else {
        return HashSet::default();
    };
    let openai = language_models.openai_compatible.iter().flatten();
    let anthropic = language_models.anthropic_compatible.iter().flatten();
    openai
        .map(|(name, _)| SharedString::from(name.clone()))
        .chain(anthropic.map(|(name, _)| SharedString::from(name.clone())))
        .collect()
}

/// What "Connect" on this page is doing or found, per provider. The page keeps
/// no state of its own between frames, so this lives in a global.
#[derive(Default)]
struct KeyChecks(HashMap<LanguageModelProviderId, KeyCheck>);

impl Global for KeyChecks {}

#[derive(Clone)]
enum KeyCheck {
    Checking,
    /// The key did not pass (the provider's own sentence) or could not be
    /// stored. It is no longer in the keychain.
    Failed(SharedString),
}

fn set_key_check(id: &LanguageModelProviderId, check: Option<KeyCheck>, cx: &mut App) {
    let checks = cx.default_global::<KeyChecks>();
    match check {
        Some(check) => {
            checks.0.insert(id.clone(), check);
        }
        None => {
            checks.0.remove(id);
        }
    }
    cx.refresh_windows();
}

/// Stores the key, asks the provider whether it works and keeps it only if it
/// does, as the connection screen does: "Connected" means the key was accepted.
fn connect_with_key(provider: Arc<dyn LanguageModelProvider>, key: String, cx: &mut App) {
    let id = provider.id();
    set_key_check(&id, Some(KeyCheck::Checking), cx);
    let store_key = provider.set_api_key(Some(key), cx);
    cx.spawn(async move |cx| {
        let failure = match store_key.await {
            Err(error) => Some(format!("Could not save the key: {error}").into()),
            Ok(()) => match cx.update(|cx| provider.check_connection(cx)).await {
                Ok(ProviderConnectionStatus::Connected { .. }) => None,
                Ok(ProviderConnectionStatus::NeedsAttention { reason }) => {
                    cx.update(|cx| provider.set_api_key(None, cx))
                        .await
                        .log_err();
                    Some(reason)
                }
                Ok(ProviderConnectionStatus::NotConnected) => {
                    Some("The key was not kept. Try again.".into())
                }
                Err(error) => Some(format!("Could not read the key back: {error}").into()),
            },
        };
        cx.update(|cx| set_key_check(&id, failure.map(KeyCheck::Failed), cx));
    })
    .detach();
}

pub(crate) fn render_llm_providers_page(
    settings_window: &SettingsWindow,
    scroll_handle: &ScrollHandle,
    window: &mut Window,
    cx: &mut Context<SettingsWindow>,
) -> AnyElement {
    let providers = LanguageModelRegistry::read_global(cx)
        .visible_providers()
        .into_iter()
        .filter(|provider| provider.id() != IDE_CLOUD_PROVIDER_ID)
        .collect::<Vec<_>>();
    let rows = ordered_rows(
        providers,
        |provider| (provider.id().0, provider.name().0),
        &custom_endpoint_ids(cx),
    );

    // Any server that speaks one of the two common dialects can be added.
    let custom_endpoint = v_flex()
        .min_w_0()
        .flex_none()
        .p_3()
        .gap_2()
        .border_1()
        .border_color(cx.theme().colors().border)
        .child(
            v_flex()
                .gap_0p5()
                .child(Label::new("Custom endpoint"))
                .child(
                    Label::new(
                        "Connect any server that speaks the OpenAI or the Anthropic API: \
                        a base URL, a key and the model ids.",
                    )
                    .size(LabelSize::Small)
                    .color(Color::Muted),
                ),
        )
        .child(
            h_flex()
                .gap_2()
                .flex_wrap()
                .child(
                    Button::new("add-openai-compatible", "Add OpenAI-Compatible")
                        .style(ButtonStyle::Outlined)
                        .label_size(LabelSize::Small)
                        .tab_index(0isize)
                        .start_icon(
                            Icon::new(IconName::Plus)
                                .size(IconSize::Small)
                                .color(Color::Muted),
                        )
                        .on_click(cx.listener(|this, _, window, cx| {
                            open_llm_provider_form(
                                this,
                                CompatibleProviderKind::OpenAi,
                                window,
                                cx,
                            );
                        })),
                )
                .child(
                    Button::new("add-anthropic-compatible", "Add Anthropic-Compatible")
                        .style(ButtonStyle::Outlined)
                        .label_size(LabelSize::Small)
                        .tab_index(0isize)
                        .start_icon(
                            Icon::new(IconName::Plus)
                                .size(IconSize::Small)
                                .color(Color::Muted),
                        )
                        .on_click(cx.listener(|this, _, window, cx| {
                            open_llm_provider_form(
                                this,
                                CompatibleProviderKind::Anthropic,
                                window,
                                cx,
                            );
                        })),
                ),
        );
    let mut custom_endpoint = Some(custom_endpoint);

    let mut page = v_flex()
        .id("llm-providers-page")
        .size_full()
        .px_8()
        .pt_2()
        .pb_16()
        .gap_3()
        .track_scroll(scroll_handle)
        .overflow_y_scroll();

    for group in ProviderGroup::ALL {
        let cards = rows
            .iter()
            .filter(|(row_group, ..)| *row_group == group)
            .map(|(_, title, provider)| {
                render_provider_section(settings_window, provider, title.clone(), window, cx)
            })
            .collect::<Vec<_>>();
        // The own-key group always shows: it ends on the row that adds a
        // custom endpoint.
        let custom_endpoint = custom_endpoint
            .take_if(|_| group == ProviderGroup::OwnKeyOrLocal)
            .map(IntoElement::into_any_element);
        if cards.is_empty() && custom_endpoint.is_none() {
            continue;
        }
        page = page
            .child(
                v_flex()
                    .when(group != ProviderGroup::Unaligned, |this| this.mt_2())
                    .gap_0p5()
                    .child(SectionLabel::new(group.title()))
                    .when_some(group.note(), |this, note| {
                        this.child(Label::new(note).size(LabelSize::Small).color(Color::Muted))
                    }),
            )
            .children(cards)
            .children(custom_endpoint);
    }

    page.into_any_element()
}

#[derive(Clone, Copy, PartialEq, Eq)]
pub(crate) enum CompatibleProviderKind {
    OpenAi,
    Anthropic,
}

impl CompatibleProviderKind {
    fn label(self) -> &'static str {
        match self {
            Self::OpenAi => "OpenAI",
            Self::Anthropic => "Anthropic",
        }
    }

    fn default_api_url(self) -> &'static str {
        match self {
            Self::OpenAi => "https://api.openai.com/v1",
            Self::Anthropic => "https://api.anthropic.com",
        }
    }
}

pub(crate) fn render_add_llm_provider_popover(
    settings_window: &SettingsWindow,
    _window: &mut Window,
    cx: &mut Context<SettingsWindow>,
) -> impl IntoElement {
    let focus_handle = settings_window
        .llm_provider_add_focus_handle
        .clone()
        .tab_index(0)
        .tab_stop(true);

    let settings_window = cx.entity().downgrade();

    PopoverMenu::new("add-llm-provider-popover")
        .trigger(
            Button::new("add-llm-provider", "Add Custom Endpoint")
                .style(ButtonStyle::Outlined)
                .track_focus(&focus_handle)
                .label_size(LabelSize::Small)
                .start_icon(
                    Icon::new(IconName::Plus)
                        .size(IconSize::Small)
                        .color(Color::Muted),
                ),
        )
        .anchor(gpui::Anchor::TopRight)
        .offset(gpui::Point {
            x: px(0.0),
            y: px(2.0),
        })
        .menu(move |window, cx| {
            let settings_window = settings_window.clone();
            Some(ContextMenu::build(window, cx, move |menu, _window, _cx| {
                menu.header("Compatible APIs")
                    .entry("OpenAI-Compatible", None, {
                        let settings_window = settings_window.clone();
                        move |window, cx| {
                            settings_window
                                .update(cx, |this, cx| {
                                    open_llm_provider_form(
                                        this,
                                        CompatibleProviderKind::OpenAi,
                                        window,
                                        cx,
                                    );
                                })
                                .log_err();
                        }
                    })
                    .entry("Anthropic-Compatible", None, {
                        let settings_window = settings_window;
                        move |window, cx| {
                            settings_window
                                .update(cx, |this, cx| {
                                    open_llm_provider_form(
                                        this,
                                        CompatibleProviderKind::Anthropic,
                                        window,
                                        cx,
                                    );
                                })
                                .log_err();
                        }
                    })
            }))
        })
}

fn render_provider_section(
    settings_window: &SettingsWindow,
    provider: &Arc<dyn LanguageModelProvider>,
    title: SharedString,
    window: &mut Window,
    cx: &mut Context<SettingsWindow>,
) -> AnyElement {
    let provider_id = provider.id();
    let provider_name = provider.name().0;
    let is_subscription = provider_place(&provider_id.0, false).0 == ProviderGroup::Subscription;
    let key_check = cx
        .try_global::<KeyChecks>()
        .and_then(|checks| checks.0.get(&provider_id).cloned());

    let body = match provider.settings_view(cx) {
        Some(ProviderSettingsView::ApiKey(config)) => {
            render_api_key_providers_item(provider, provider_name, config, key_check.clone(), cx)
        }
        Some(ProviderSettingsView::Inline(settings)) => {
            let view = get_or_create_configuration_view(
                settings_window,
                &provider_id,
                settings.create_view,
                window,
                cx,
            );
            // The three subscriptions describe themselves in upstream's words
            // ("… in Zed's agent"): their row says what signing in does here.
            let (title, description) = if is_subscription {
                (
                    None,
                    Some(InlineDescription::Text(SUBSCRIPTION_SIGN_IN_NOTE.into())),
                )
            } else {
                (settings.title, settings.description)
            };
            render_inline_body(provider_name, title, description, view)
        }
        Some(ProviderSettingsView::SubPage(settings)) => {
            render_subpage_item(provider, settings.description, cx)
        }
        None => div().into_any_element(),
    };

    let state = if matches!(key_check, Some(KeyCheck::Checking)) {
        ConnectionState::Checking
    } else {
        match provider.connection_status(cx) {
            ProviderConnectionStatus::Connected { detail } => ConnectionState::Connected(detail),
            ProviderConnectionStatus::NeedsAttention { reason } => {
                ConnectionState::NeedsAttention(reason)
            }
            ProviderConnectionStatus::NotConnected => ConnectionState::NotConnected,
        }
    };
    let reason = match &state {
        ConnectionState::NeedsAttention(reason) => Some(reason.clone()),
        _ => None,
    };
    let note = (provider_id.0.as_ref() == UNALIGNED_PROVIDER_ID
        && matches!(state, ConnectionState::NotConnected))
    .then_some(UNALIGNED_NOTE);
    let colors = cx.theme().colors();

    // One flat card per provider: a header line with the name and the
    // connection state, the configuration underneath.
    v_flex()
        .min_w_0()
        .flex_none()
        .border_1()
        .border_color(colors.border)
        .child(render_provider_header(title, provider.icon(), state, cx))
        .child(
            v_flex()
                .min_w_0()
                .px_3()
                .pb_3()
                .when_some(note, |this, note| {
                    this.child(
                        div()
                            .pt_2p5()
                            .child(Label::new(note).size(LabelSize::Small).color(Color::Muted)),
                    )
                })
                .when_some(reason, |this, reason| {
                    this.child(
                        div().pt_2p5().child(
                            Label::new(reason)
                                .size(LabelSize::Small)
                                .color(Color::Warning),
                        ),
                    )
                })
                .child(body),
        )
        .into_any_element()
}

/// A provider's state in the words of the connection screen.
enum ConnectionState {
    NotConnected,
    Checking,
    Connected(Option<SharedString>),
    NeedsAttention(SharedString),
}

impl ConnectionState {
    fn label(&self) -> (SharedString, Color) {
        match self {
            Self::NotConnected => ("Not connected".into(), Color::Muted),
            Self::Checking => ("Checking…".into(), Color::Muted),
            Self::Connected(Some(detail)) => {
                (format!("Connected · {detail}").into(), Color::Success)
            }
            Self::Connected(None) => ("Connected".into(), Color::Success),
            Self::NeedsAttention(_) => ("Needs attention".into(), Color::Warning),
        }
    }
}

/// The card's header: icon and name on the left, connection state on the right.
/// Renders providers' external SVG icons too.
fn render_provider_header(
    title: SharedString,
    icon: IconOrSvg,
    state: ConnectionState,
    cx: &mut Context<SettingsWindow>,
) -> impl IntoElement {
    let icon = match icon {
        IconOrSvg::Svg(path) => Icon::from_external_svg(path),
        IconOrSvg::Icon(name) => Icon::new(name),
    }
    .size(IconSize::Small)
    .color(Color::Muted);

    let (state_label, state_color) = state.label();
    let is_connected = matches!(state, ConnectionState::Connected(_));

    h_flex()
        .w_full()
        .min_w_0()
        .h(rems_from_px(32_f32))
        .px_3()
        .gap_2()
        .justify_between()
        .border_b_1()
        .border_color(cx.theme().colors().border)
        .bg(cx.theme().colors().surface_background)
        .child(
            h_flex()
                .min_w_0()
                .gap_2()
                .child(icon)
                .child(Label::new(title).truncate()),
        )
        .child(
            h_flex()
                .flex_none()
                .gap_1p5()
                .when(is_connected, |this| {
                    this.child(Indicator::dot().color(Color::Success))
                })
                .child(
                    Label::new(state_label)
                        .size(LabelSize::Small)
                        .color(state_color),
                ),
        )
}

fn render_api_key_providers_item(
    provider: &Arc<dyn LanguageModelProvider>,
    provider_name: SharedString,
    config: ApiKeyConfiguration,
    key_check: Option<KeyCheck>,
    _cx: &mut Context<SettingsWindow>,
) -> AnyElement {
    let provider_id = provider.id();
    let is_from_env_var = config.is_from_env_var;
    let env_var_name = config.env_var_name;
    let api_key_url = config.api_key_url;
    let checking = matches!(key_check, Some(KeyCheck::Checking));

    if config.has_key && is_from_env_var {
        // No field: a key typed here would be stored and never used.
        return div()
            .pt_2p5()
            .child(
                Label::new(format!(
                    "Using the key in {env_var_name}. To use another one, change the variable \
                    and restart the IDE."
                ))
                .size(LabelSize::Small)
                .color(Color::Muted),
            )
            .into_any_element();
    }

    // While a key is being checked the field stays, with the key in it: a
    // refused key is corrected there.
    if config.has_key && !checking {
        let button_id = format!("disconnect-{}", provider_id.0);

        return h_flex()
            .pt_2p5()
            .w_full()
            .min_w_0()
            .gap_4()
            .justify_between()
            .child(
                Label::new("A key is stored on this computer.")
                    .size(LabelSize::Small)
                    .color(Color::Muted)
                    .truncate(),
            )
            .child(
                Button::new(button_id, "Disconnect")
                    .style(ButtonStyle::Outlined)
                    .label_size(LabelSize::Small)
                    .tab_index(0isize)
                    .on_click({
                        let provider = provider.clone();
                        move |_, window, cx| {
                            // Asked once, as on the connection screen.
                            let answer = window.prompt(
                                PromptLevel::Warning,
                                &format!("Disconnect {provider_name}?"),
                                Some("Its key is removed from this computer."),
                                &["Disconnect", "Cancel"],
                                cx,
                            );
                            let provider = provider.clone();
                            cx.spawn(async move |cx| {
                                if answer.await.ok() == Some(0) {
                                    cx.update(|cx| provider.set_api_key(None, cx))
                                        .await
                                        .log_err();
                                    // A refusal noted earlier is about a key
                                    // that is gone.
                                    cx.update(|cx| set_key_check(&provider.id(), None, cx));
                                }
                            })
                            .detach();
                        }
                    }),
            )
            .into_any_element();
    }

    let input_id = format!("{}-api-key-input", provider_id.0);
    let aria_label = format!("{provider_name} API Key");
    let failure = match key_check {
        Some(KeyCheck::Failed(reason)) => Some(reason),
        _ => None,
    };

    v_flex()
        .gap_2()
        .child(
            h_flex()
                .pt_2p5()
                .w_full()
                .min_w_0()
                .gap_4()
                .justify_between()
                .child(
                    v_flex()
                        .w_full()
                        .min_w_0()
                        .max_w_1_2()
                        .gap_0p5()
                        .child(Label::new("API Key"))
                        .child(
                            h_flex()
                                .w_full()
                                .min_w_0()
                                .flex_wrap()
                                .gap_1()
                                .child(
                                    Label::new("Paste your key and press Enter to connect.")
                                        .size(LabelSize::Small)
                                        .color(Color::Muted),
                                )
                                .child(
                                    ButtonLink::new("Get a key", api_key_url)
                                        .no_icon(true)
                                        .label_size(LabelSize::Small)
                                        .label_color(Color::Muted),
                                ),
                        )
                        .child(
                            Label::new(format!(
                                "Or set the {env_var_name} environment variable and restart the IDE."
                            ))
                            .size(LabelSize::XSmall)
                            .color(Color::Muted),
                        ),
                )
                .child(
                    SettingsInputField::new(input_id)
                        .tab_index(0)
                        .with_placeholder("Paste your key")
                        .display_confirm_button()
                        .aria_label(aria_label)
                        .on_confirm({
                            let provider = provider.clone();
                            move |api_key, _window, cx| {
                                let key = api_key.map(|key| key.trim().to_string());
                                if let Some(key) = key.filter(|key| !key.is_empty()) {
                                    connect_with_key(provider.clone(), key, cx);
                                }
                            }
                        }),
                ),
        )
        .when(checking, |this| {
            this.child(
                Label::new("Checking…")
                    .size(LabelSize::Small)
                    .color(Color::Muted),
            )
        })
        .when_some(failure, |this, failure| {
            this.child(
                Label::new(failure)
                    .size(LabelSize::Small)
                    .color(Color::Warning),
            )
        })
        .into_any_element()
}

fn render_inline_body(
    provider_name: SharedString,
    title: Option<SharedString>,
    description: Option<InlineDescription>,
    view: impl IntoElement,
) -> AnyElement {
    let view = view.into_any_element();

    if title.is_none() && description.is_none() {
        return v_flex()
            .pt_1()
            .w_full()
            .min_w_0()
            .child(view)
            .into_any_element();
    }

    h_flex()
        .pt_2p5()
        .w_full()
        .min_w_0()
        .gap_4()
        .justify_between()
        .child(
            v_flex()
                .w_full()
                .min_w_0()
                .max_w_1_2()
                .debug_selector(|| "inline-provider-description".into())
                .when_some(title, |this, title| this.child(Label::new(title)))
                .when_some(description, |this, description| {
                    this.child(render_inline_description(provider_name, description))
                }),
        )
        .child(
            h_flex()
                .min_w_0()
                .max_w_1_2()
                .flex_1()
                .justify_end()
                .child(view),
        )
        .into_any_element()
}

fn render_subpage_item(
    provider: &Arc<dyn LanguageModelProvider>,
    description: Option<InlineDescription>,
    cx: &mut Context<SettingsWindow>,
) -> AnyElement {
    let provider_id = provider.id();
    let provider_name = provider.name().0;

    h_flex()
        .pt_2p5()
        .w_full()
        .min_w_0()
        .gap_4()
        .justify_between()
        .child(
            v_flex()
                .w_full()
                .min_w_0()
                .max_w_1_2()
                .gap_0p5()
                .child(Label::new("Configure Provider"))
                .when_some(description, |this, description| {
                    this.child(render_inline_description(provider_name, description))
                }),
        )
        .child(
            Button::new(format!("configure-{}", provider_id.0), "Configure")
                .style(ButtonStyle::Outlined)
                .size(ButtonSize::Medium)
                .end_icon(
                    Icon::new(IconName::ChevronRight)
                        .size(IconSize::Small)
                        .color(Color::Muted),
                )
                .tab_index(0isize)
                .on_click(cx.listener(move |this, _, window, cx| {
                    open_provider_configuration(this, provider_id.clone(), window, cx);
                })),
        )
        .into_any_element()
}

fn render_inline_description(
    provider_name: SharedString,
    description: InlineDescription,
) -> AnyElement {
    match description {
        InlineDescription::ApiKeyUrl(url) => h_flex()
            .gap_0p5()
            .child(
                Label::new("To find an API key, visit the")
                    .size(LabelSize::Small)
                    .color(Color::Muted),
            )
            .child(
                ButtonLink::new(format!("{provider_name} dashboard."), url)
                    .label_size(LabelSize::Small),
            )
            .into_any_element(),
        InlineDescription::Text(text) => Label::new(text)
            .size(LabelSize::Small)
            .color(Color::Muted)
            .into_any_element(),
    }
}

fn open_provider_configuration(
    settings_window: &mut SettingsWindow,
    provider_id: LanguageModelProviderId,
    window: &mut Window,
    cx: &mut Context<SettingsWindow>,
) {
    let title = LanguageModelRegistry::read_global(cx)
        .provider(&provider_id)
        .map(|provider| provider.name().0)
        .unwrap_or_else(|| provider_id.0.clone());

    settings_window.configuring_provider = Some(provider_id);

    settings_window.push_dynamic_sub_page(
        title,
        "Connections",
        Some("llm_providers"),
        true,
        render_provider_config_sub_page,
        window,
        cx,
    );
}

fn render_provider_config_sub_page(
    settings_window: &SettingsWindow,
    scroll_handle: &ScrollHandle,
    window: &mut Window,
    cx: &mut Context<SettingsWindow>,
) -> AnyElement {
    let Some(provider_id) = settings_window.configuring_provider.clone() else {
        return div().into_any_element();
    };
    let Some(provider) = LanguageModelRegistry::read_global(cx).provider(&provider_id) else {
        return div().into_any_element();
    };

    let Some(create_view) =
        provider
            .settings_view(cx)
            .and_then(|settings_view| match settings_view {
                ProviderSettingsView::Inline(settings) => Some(settings.create_view),
                ProviderSettingsView::SubPage(settings) => Some(settings.create_view),
                ProviderSettingsView::ApiKey(_) => None,
            })
    else {
        return div().into_any_element();
    };
    let view =
        get_or_create_configuration_view(settings_window, &provider_id, create_view, window, cx);

    v_flex()
        .id("provider-config-sub-page")
        .size_full()
        .pt_2p5()
        .px_8()
        .pb_16()
        .track_scroll(scroll_handle)
        .overflow_y_scroll()
        .child(view)
        .into_any_element()
}

fn get_or_create_configuration_view(
    settings_window: &SettingsWindow,
    provider_id: &LanguageModelProviderId,
    create_view: CreateProviderSettingsView,
    window: &mut Window,
    cx: &mut Context<SettingsWindow>,
) -> AnyView {
    if let Some(view) = settings_window
        .provider_configuration_views
        .get(provider_id)
    {
        return view.clone();
    }

    let view = create_view(window, cx);

    // Store the view for future renders by deferring a mutation
    let provider_id = provider_id.clone();
    let view_clone = view.clone();
    cx.defer_in(window, move |this, _window, _cx| {
        this.provider_configuration_views
            .insert(provider_id, view_clone);
    });

    view
}

pub(crate) struct LlmProviderForm {
    kind: CompatibleProviderKind,
    provider_name: Entity<Editor>,
    api_url: Entity<Editor>,
    api_key: Entity<Editor>,
    models: Vec<ModelInput>,
    error: Option<SharedString>,
}

impl LlmProviderForm {
    fn new(
        kind: CompatibleProviderKind,
        window: &mut Window,
        cx: &mut Context<SettingsWindow>,
    ) -> Self {
        Self {
            kind,
            provider_name: new_input(kind.label(), None, false, window, cx),
            api_url: new_input(kind.default_api_url(), None, false, window, cx),
            api_key: new_input(
                "000000000000000000000000000000000000000000000000",
                None,
                true,
                window,
                cx,
            ),
            models: vec![ModelInput::new(0, window, cx)],
            error: None,
        }
    }
}

struct ModelInput {
    name: Entity<Editor>,
    max_completion_tokens: Entity<Editor>,
    max_output_tokens: Entity<Editor>,
    max_tokens: Entity<Editor>,
    reasoning_effort: OpenAiReasoningEffort,
    supports_tools: ToggleState,
    supports_images: ToggleState,
    supports_parallel_tool_calls: ToggleState,
    supports_prompt_cache_key: ToggleState,
    supports_chat_completions: ToggleState,
    supports_thinking: ToggleState,
    interleaved_reasoning: ToggleState,
    max_tokens_parameter: ToggleState,
}

impl ModelInput {
    fn new(_index: usize, window: &mut Window, cx: &mut Context<SettingsWindow>) -> Self {
        let OpenAiCompatibleModelCapabilities {
            tools,
            images,
            parallel_tool_calls,
            prompt_cache_key,
            chat_completions,
            interleaved_reasoning,
            max_tokens_parameter,
        } = OpenAiCompatibleModelCapabilities::default();

        Self {
            name: new_input(
                "e.g. gpt-5, claude-opus-4, gemini-2.5-pro",
                None,
                false,
                window,
                cx,
            ),
            max_completion_tokens: new_input("200000", Some("200000"), false, window, cx),
            max_output_tokens: new_input("Max Output Tokens", Some("32000"), false, window, cx),
            max_tokens: new_input("Max Tokens", Some("200000"), false, window, cx),
            reasoning_effort: OpenAiReasoningEffort::Medium,
            supports_tools: tools.into(),
            supports_images: images.into(),
            supports_parallel_tool_calls: parallel_tool_calls.into(),
            supports_prompt_cache_key: prompt_cache_key.into(),
            supports_chat_completions: chat_completions.into(),
            supports_thinking: ToggleState::Unselected,
            interleaved_reasoning: interleaved_reasoning.into(),
            max_tokens_parameter: max_tokens_parameter.into(),
        }
    }
}

fn new_input(
    placeholder: &str,
    initial: Option<&str>,
    masked: bool,
    window: &mut Window,
    cx: &mut Context<SettingsWindow>,
) -> Entity<Editor> {
    let placeholder = placeholder.to_string();
    let initial = initial.map(str::to_string);
    cx.new(|cx| {
        let mut editor = Editor::single_line(window, cx);
        editor.set_placeholder_text(placeholder.as_str(), window, cx);
        editor.set_masked(masked, cx);
        if let Some(text) = initial {
            editor.set_text(text, window, cx);
        }
        editor
    })
}

fn open_llm_provider_form(
    settings_window: &mut SettingsWindow,
    kind: CompatibleProviderKind,
    window: &mut Window,
    cx: &mut Context<SettingsWindow>,
) {
    settings_window.llm_provider_form = Some(LlmProviderForm::new(kind, window, cx));
    settings_window.push_dynamic_sub_page(
        format!("Add {}-Compatible Provider", kind.label()),
        "Agent Configuration",
        Some("llm_providers"),
        true,
        render_llm_provider_form_page,
        window,
        cx,
    );
}

fn render_llm_provider_form_page(
    settings_window: &SettingsWindow,
    scroll_handle: &ScrollHandle,
    window: &mut Window,
    cx: &mut Context<SettingsWindow>,
) -> AnyElement {
    let Some(form) = settings_window.llm_provider_form.as_ref() else {
        return div().into_any_element();
    };

    v_flex()
        .size_full()
        .child(
            v_flex()
                .id("llm-provider-form-page")
                .track_scroll(scroll_handle)
                .pt_2p5()
                .px_8()
                .pb_16()
                .gap_4()
                .overflow_y_scroll()
                .child(Label::new(match form.kind {
                    CompatibleProviderKind::OpenAi => {
                        "This provider will use an OpenAI-compatible API."
                    }
                    CompatibleProviderKind::Anthropic => {
                        "This provider will use an Anthropic Messages-compatible API."
                    }
                }))
                .child(Divider::horizontal().flex_shrink_0())
                .child(render_form_field(
                    "Provider Name",
                    "A unique name used to identify this provider.",
                    &form.provider_name,
                    cx,
                ))
                .child(render_form_field(
                    "API URL",
                    "The base URL for the compatible API.",
                    &form.api_url,
                    cx,
                ))
                .child(render_form_field(
                    "API Key",
                    "Stored in the system keychain, not in settings.json.",
                    &form.api_key,
                    cx,
                ))
                .child(render_models_section(form, window, cx)),
        )
        .child(
            v_flex()
                .px_8()
                .py_2p5()
                .gap_1()
                .border_t_1()
                .border_color(cx.theme().colors().border_variant)
                .when_some(form.error.clone(), |this, error| {
                    this.child(render_form_error(error))
                })
                .child(render_form_actions(cx)),
        )
        .into_any_element()
}

fn render_form_field(
    title: &'static str,
    description: &'static str,
    editor: &Entity<Editor>,
    cx: &mut Context<SettingsWindow>,
) -> AnyElement {
    let colors = cx.theme().colors();
    let focus_handle = editor.focus_handle(cx).tab_index(0).tab_stop(true);
    v_flex()
        .w_full()
        .gap_1p5()
        .child(
            v_flex()
                .gap_0p5()
                .child(
                    h_flex().gap_0p5().child(Label::new(title)).child(
                        Label::new("*")
                            .size(LabelSize::Small)
                            .color(Color::Error)
                            .mb_2(),
                    ),
                )
                .child(
                    Label::new(description)
                        .size(LabelSize::Small)
                        .color(Color::Muted),
                ),
        )
        .child(
            h_flex()
                .w_full()
                .min_w_64()
                .h_8()
                .px_2()
                .rounded_md()
                .border_1()
                .border_color(colors.border)
                .bg(colors.editor_background)
                .track_focus(&focus_handle)
                .focus(|style| style.border_color(colors.border_focused))
                .child(editor.clone()),
        )
        .into_any_element()
}

fn render_models_section(
    form: &LlmProviderForm,
    window: &mut Window,
    cx: &mut Context<SettingsWindow>,
) -> impl IntoElement {
    v_flex()
        .mt_1()
        .gap_2()
        .child(
            h_flex()
                .justify_between()
                .child(Label::new("Models"))
                .child(
                    Button::new("add-model", "Add Model")
                        .start_icon(
                            Icon::new(IconName::Plus)
                                .size(IconSize::XSmall)
                                .color(Color::Muted),
                        )
                        .label_size(LabelSize::Small)
                        .on_click(cx.listener(|this, _, window, cx| {
                            if let Some(form) = this.llm_provider_form.as_mut() {
                                let index = form.models.len();
                                form.models.push(ModelInput::new(index, window, cx));
                            }
                            cx.notify();
                        })),
                ),
        )
        .children(form.models.iter().enumerate().map(|(index, model)| {
            render_model(form.kind, model, index, form.models.len(), window, cx)
        }))
}

fn render_model(
    kind: CompatibleProviderKind,
    model: &ModelInput,
    index: usize,
    model_count: usize,
    window: &mut Window,
    cx: &mut Context<SettingsWindow>,
) -> AnyElement {
    v_flex()
        .p_2()
        .gap_2()
        .rounded_sm()
        .border_1()
        .border_dashed()
        .border_color(cx.theme().colors().border.opacity(0.6))
        .bg(cx.theme().colors().element_active.opacity(0.15))
        .child(render_form_field(
            "Model Name",
            "The model's name in the provider's API.",
            &model.name,
            cx,
        ))
        .when(matches!(kind, CompatibleProviderKind::OpenAi), |this| {
            this.child(render_form_field(
                "Max Completion Tokens",
                "Maximum completion tokens for OpenAI-compatible requests.",
                &model.max_completion_tokens,
                cx,
            ))
        })
        .child(render_form_field(
            "Max Output Tokens",
            "The maximum number of tokens the model can output.",
            &model.max_output_tokens,
            cx,
        ))
        .child(render_form_field(
            "Max Tokens",
            "The model context window size.",
            &model.max_tokens,
            cx,
        ))
        .child(render_model_capabilities(kind, model, index, window, cx))
        .when(model_count > 1, |this| {
            this.child(
                Button::new(("remove-model", index), "Remove Model")
                    .start_icon(
                        Icon::new(IconName::Trash)
                            .size(IconSize::XSmall)
                            .color(Color::Muted),
                    )
                    .label_size(LabelSize::Small)
                    .style(ButtonStyle::Outlined)
                    .full_width()
                    .on_click(cx.listener(move |this, _, _window, cx| {
                        if let Some(form) = this.llm_provider_form.as_mut()
                            && index < form.models.len()
                        {
                            form.models.remove(index);
                        }
                        cx.notify();
                    })),
            )
        })
        .into_any_element()
}

fn render_model_capabilities(
    kind: CompatibleProviderKind,
    model: &ModelInput,
    index: usize,
    window: &mut Window,
    cx: &mut Context<SettingsWindow>,
) -> impl IntoElement {
    v_flex()
        .gap_1()
        .child(render_capability_checkbox(
            "supports-tools",
            index,
            "Supports tools",
            model.supports_tools,
            |model, state| model.supports_tools = state,
            cx,
        ))
        .child(render_capability_checkbox(
            "supports-images",
            index,
            "Supports images",
            model.supports_images,
            |model, state| model.supports_images = state,
            cx,
        ))
        .when(matches!(kind, CompatibleProviderKind::OpenAi), |this| {
            this.child(render_capability_checkbox(
                "supports-parallel-tool-calls",
                index,
                "Supports parallel_tool_calls",
                model.supports_parallel_tool_calls,
                |model, state| model.supports_parallel_tool_calls = state,
                cx,
            ))
            .child(render_capability_checkbox(
                "supports-prompt-cache-key",
                index,
                "Supports prompt_cache_key",
                model.supports_prompt_cache_key,
                |model, state| model.supports_prompt_cache_key = state,
                cx,
            ))
            .child(render_capability_checkbox(
                "supports-chat-completions",
                index,
                "Supports /chat/completions",
                model.supports_chat_completions,
                |model, state| model.supports_chat_completions = state,
                cx,
            ))
            .when(model.supports_chat_completions.selected(), |this| {
                this.child(render_capability_checkbox(
                    "max-tokens-parameter",
                    index,
                    "Uses max_tokens for output limit",
                    model.max_tokens_parameter,
                    |model, state| model.max_tokens_parameter = state,
                    cx,
                ))
            })
            .child(render_capability_checkbox(
                "supports-thinking",
                index,
                "Supports thinking",
                model.supports_thinking,
                |model, state| model.supports_thinking = state,
                cx,
            ))
            .when(model.supports_thinking.selected(), |this| {
                this.child(render_reasoning_effort_selector(
                    model.reasoning_effort,
                    index,
                    window,
                    cx,
                ))
                .when(model.supports_chat_completions.selected(), |this| {
                    this.child(render_capability_checkbox(
                        "interleaved-reasoning",
                        index,
                        "Preserves thinking in chat history",
                        model.interleaved_reasoning,
                        |model, state| model.interleaved_reasoning = state,
                        cx,
                    ))
                })
            })
        })
}

fn render_capability_checkbox(
    id: &'static str,
    index: usize,
    label: &'static str,
    state: ToggleState,
    update: fn(&mut ModelInput, ToggleState),
    cx: &mut Context<SettingsWindow>,
) -> impl IntoElement {
    Checkbox::new((id, index), state)
        .label(label)
        .on_click(cx.listener(move |this, checked, _window, cx| {
            if let Some(form) = this.llm_provider_form.as_mut()
                && let Some(model) = form.models.get_mut(index)
            {
                update(model, *checked);
            }
            cx.notify();
        }))
}

fn render_reasoning_effort_selector(
    selected: OpenAiReasoningEffort,
    index: usize,
    window: &mut Window,
    cx: &mut Context<SettingsWindow>,
) -> impl IntoElement {
    let settings_window = cx.weak_entity();
    let menu = ContextMenu::build(window, cx, move |mut menu, _window, _cx| {
        for effort in OpenAiReasoningEffort::OPENAI_COMPATIBLE_SELECTABLE {
            let is_selected = effort == selected;
            let settings_window = settings_window.clone();
            menu.push_item(
                ui::ContextMenuEntry::new(effort.label())
                    .toggleable(IconPosition::End, is_selected)
                    .handler(move |_window, cx| {
                        settings_window
                            .update(cx, |this, cx| {
                                if let Some(form) = this.llm_provider_form.as_mut()
                                    && let Some(model) = form.models.get_mut(index)
                                {
                                    model.reasoning_effort = effort;
                                }
                                cx.notify();
                            })
                            .ok();
                    }),
            );
        }
        menu
    });

    v_flex()
        .gap_1()
        .child(Label::new("Default reasoning effort").size(LabelSize::Small))
        .child(
            DropdownMenu::new(
                ElementId::Name(format!("reasoning-effort-selector-{index}").into()),
                selected.label(),
                menu,
            )
            .style(DropdownStyle::Outlined)
            .trigger_size(ButtonSize::Compact)
            .full_width(true)
            .aria_label("Default reasoning effort"),
        )
}

fn render_form_error(error: SharedString) -> impl IntoElement {
    h_flex()
        .w_full()
        .gap_2()
        .child(
            Icon::new(IconName::XCircle)
                .size(IconSize::Small)
                .color(Color::Error),
        )
        .child(Label::new(error).size(LabelSize::Small).color(Color::Error))
}

fn render_form_actions(cx: &mut Context<SettingsWindow>) -> impl IntoElement {
    h_flex()
        .w_full()
        .gap_1()
        .justify_end()
        .child(
            Button::new("llm-provider-form-cancel", "Cancel").on_click(cx.listener(
                |this, _, window, cx| {
                    this.llm_provider_form = None;
                    this.pop_sub_page(window, cx);
                },
            )),
        )
        .child(
            Button::new("llm-provider-form-save", "Save Provider")
                .style(ButtonStyle::Filled)
                .on_click(cx.listener(|this, _, window, cx| {
                    save_llm_provider_form(this, window, cx);
                })),
        )
}

struct LlmProviderFormValues {
    kind: CompatibleProviderKind,
    provider_name: String,
    api_url: String,
    api_key: String,
    models: Vec<ModelValues>,
}

struct ModelValues {
    name: String,
    max_completion_tokens: String,
    max_output_tokens: String,
    max_tokens: String,
    reasoning_effort: OpenAiReasoningEffort,
    supports_tools: bool,
    supports_images: bool,
    supports_parallel_tool_calls: bool,
    supports_prompt_cache_key: bool,
    supports_chat_completions: bool,
    supports_thinking: bool,
    interleaved_reasoning: bool,
    max_tokens_parameter: bool,
}

enum ParsedModels {
    OpenAi(Vec<OpenAiCompatibleAvailableModel>),
    Anthropic(Vec<AnthropicCompatibleAvailableModel>),
}

fn save_llm_provider_form(
    settings_window: &mut SettingsWindow,
    window: &mut Window,
    cx: &mut Context<SettingsWindow>,
) {
    let values = {
        let Some(form) = settings_window.llm_provider_form.as_ref() else {
            return;
        };
        LlmProviderFormValues {
            kind: form.kind,
            provider_name: form.provider_name.read(cx).text(cx),
            api_url: form.api_url.read(cx).text(cx),
            api_key: form.api_key.read(cx).text(cx),
            models: form
                .models
                .iter()
                .map(|model| ModelValues {
                    name: model.name.read(cx).text(cx),
                    max_completion_tokens: model.max_completion_tokens.read(cx).text(cx),
                    max_output_tokens: model.max_output_tokens.read(cx).text(cx),
                    max_tokens: model.max_tokens.read(cx).text(cx),
                    reasoning_effort: model.reasoning_effort,
                    supports_tools: model.supports_tools.selected(),
                    supports_images: model.supports_images.selected(),
                    supports_parallel_tool_calls: model.supports_parallel_tool_calls.selected(),
                    supports_prompt_cache_key: model.supports_prompt_cache_key.selected(),
                    supports_chat_completions: model.supports_chat_completions.selected(),
                    supports_thinking: model.supports_thinking.selected(),
                    interleaved_reasoning: model.interleaved_reasoning.selected(),
                    max_tokens_parameter: model.max_tokens_parameter.selected(),
                })
                .collect(),
        }
    };

    let (provider_name, api_url, api_key, models) = match validate_llm_provider_form(&values, cx) {
        Ok(value) => value,
        Err(error) => {
            if let Some(form) = settings_window.llm_provider_form.as_mut() {
                form.error = Some(error);
            }
            cx.notify();
            return;
        }
    };

    let fs = <dyn fs::Fs>::global(cx);
    cx.spawn_in(window, async move |this, cx| {
        let result = async {
            let provider_id = LanguageModelProviderId(provider_name.clone().into());
            let settings_update = cx.update(|_window, cx| {
                settings::update_settings_file_with_completion(fs, cx, move |settings, _cx| {
                    let language_models = settings.language_models.get_or_insert_default();
                    match models {
                        ParsedModels::OpenAi(available_models) => {
                            language_models
                                .openai_compatible
                                .get_or_insert_default()
                                .insert(
                                    Arc::from(provider_name.as_str()),
                                    OpenAiCompatibleSettingsContent {
                                        api_url: api_url.clone(),
                                        available_models,
                                        custom_headers: None,
                                    },
                                );
                        }
                        ParsedModels::Anthropic(available_models) => {
                            language_models
                                .anthropic_compatible
                                .get_or_insert_default()
                                .insert(
                                    Arc::from(provider_name.as_str()),
                                    AnthropicCompatibleSettingsContent {
                                        api_url: api_url.clone(),
                                        available_models,
                                        custom_headers: None,
                                    },
                                );
                        }
                    }
                })
            })?;

            settings_update
                .await
                .map_err(|_| anyhow::anyhow!("Settings update was canceled"))??;

            let set_api_key = cx.update(|_window, cx| {
                let provider = LanguageModelRegistry::read_global(cx)
                    .provider(&provider_id)
                    .ok_or_else(|| anyhow::anyhow!("Provider was not registered"))?;
                anyhow::Ok(provider.set_api_key(Some(api_key), cx))
            })??;
            set_api_key.await?;

            cx.update(|window, cx| {
                this.update(cx, |this, cx| {
                    this.provider_configuration_views.remove(&provider_id);
                    this.llm_provider_form = None;
                    this.pop_sub_page(window, cx);
                })
            })??;

            anyhow::Ok(())
        }
        .await;

        if let Err(error) = result {
            this.update(cx, |this, cx| {
                if let Some(form) = this.llm_provider_form.as_mut() {
                    form.error = Some(error.to_string().into());
                }
                cx.notify();
            })?;
        }

        anyhow::Ok(())
    })
    .detach_and_log_err(cx);
}

fn validate_llm_provider_form(
    values: &LlmProviderFormValues,
    cx: &App,
) -> Result<(String, String, String, ParsedModels), SharedString> {
    let provider_name = values.provider_name.clone();
    if provider_name.is_empty() {
        return Err("Provider Name cannot be empty".into());
    }

    if LanguageModelRegistry::read_global(cx)
        .providers()
        .iter()
        .any(|provider| {
            provider.id().0.as_ref() == provider_name.as_str()
                || provider.name().0.as_ref() == provider_name.as_str()
        })
    {
        return Err("Provider Name is already taken by another provider".into());
    }

    let api_url = values.api_url.clone();
    if api_url.is_empty() {
        return Err("API URL cannot be empty".into());
    }

    let api_key = values.api_key.clone();
    if api_key.is_empty() {
        return Err("API Key cannot be empty".into());
    }

    let models = match values.kind {
        CompatibleProviderKind::OpenAi => ParsedModels::OpenAi(
            values
                .models
                .iter()
                .map(parse_open_ai_model)
                .collect::<Result<Vec<_>, _>>()?,
        ),
        CompatibleProviderKind::Anthropic => ParsedModels::Anthropic(
            values
                .models
                .iter()
                .map(parse_anthropic_model)
                .collect::<Result<Vec<_>, _>>()?,
        ),
    };

    let mut model_names = HashSet::new();
    let model_names_are_unique = match &models {
        ParsedModels::OpenAi(models) => models
            .iter()
            .all(|model| model_names.insert(model.name.clone())),
        ParsedModels::Anthropic(models) => models
            .iter()
            .all(|model| model_names.insert(model.name.clone())),
    };
    if !model_names_are_unique {
        return Err("Model Names must be unique".into());
    }

    Ok((provider_name, api_url, api_key, models))
}

fn parse_model_name(model: &ModelValues) -> Result<String, SharedString> {
    if model.name.is_empty() {
        return Err("Model Name cannot be empty".into());
    }
    Ok(model.name.clone())
}

fn parse_open_ai_model(
    model: &ModelValues,
) -> Result<OpenAiCompatibleAvailableModel, SharedString> {
    Ok(OpenAiCompatibleAvailableModel {
        name: parse_model_name(model)?,
        display_name: None,
        max_completion_tokens: Some(parse_u64_field(
            &model.max_completion_tokens,
            "Max Completion Tokens",
        )?),
        max_output_tokens: Some(parse_u64_field(
            &model.max_output_tokens,
            "Max Output Tokens",
        )?),
        max_tokens: parse_u64_field(&model.max_tokens, "Max Tokens")?,
        reasoning_effort: model.supports_thinking.then_some(model.reasoning_effort),
        capabilities: OpenAiCompatibleModelCapabilities {
            tools: model.supports_tools,
            images: model.supports_images,
            parallel_tool_calls: model.supports_parallel_tool_calls,
            prompt_cache_key: model.supports_prompt_cache_key,
            chat_completions: model.supports_chat_completions,
            interleaved_reasoning: model.supports_thinking
                && model.supports_chat_completions
                && model.interleaved_reasoning,
            max_tokens_parameter: model.supports_chat_completions && model.max_tokens_parameter,
        },
    })
}

fn parse_anthropic_model(
    model: &ModelValues,
) -> Result<AnthropicCompatibleAvailableModel, SharedString> {
    Ok(AnthropicCompatibleAvailableModel {
        name: parse_model_name(model)?,
        display_name: None,
        max_tokens: parse_u64_field(&model.max_tokens, "Max Tokens")?,
        tool_override: None,
        max_output_tokens: Some(parse_u64_field(
            &model.max_output_tokens,
            "Max Output Tokens",
        )?),
        default_temperature: None,
        extra_beta_headers: Vec::new(),
        mode: None,
        capabilities: AnthropicCompatibleModelCapabilities {
            tools: model.supports_tools,
            images: model.supports_images,
            prompt_caching: false,
        },
    })
}

fn parse_u64_field(value: &str, name: &str) -> Result<u64, SharedString> {
    value
        .parse::<u64>()
        .map_err(|_| format!("{name} must be a number").into())
}

#[cfg(test)]
mod tests {
    use gpui::{TestAppContext, VisualTestContext, size};
    use settings::SettingsStore;

    use super::*;

    #[test]
    fn rows_follow_the_connection_screen() {
        // Registration order is alphabetical by id, as the registry gives it.
        let providers = vec![
            ("amazon-bedrock", "Amazon Bedrock"),
            ("anthropic", "Anthropic"),
            ("copilot_chat", "GitHub Copilot Chat"),
            ("deepseek", "DeepSeek"),
            ("home server", "home server"),
            ("lmstudio", "LM Studio"),
            ("mistral", "Mistral"),
            ("ollama", "Ollama"),
            ("openai", "OpenAI"),
            ("openai-subscribed", "ChatGPT Subscription"),
            ("opencode", "OpenCode"),
            ("unaligned", "Unaligned"),
            ("x_ai", "xAI"),
            ("x_ai_subscribed", "xAI Subscription"),
        ];
        let custom_endpoints = HashSet::from_iter(["home server".into()]);
        let rows = ordered_rows(
            providers,
            |(id, name)| (SharedString::from(*id), SharedString::from(*name)),
            &custom_endpoints,
        )
        .into_iter()
        .map(|(group, title, (id, _))| (group, id, title.to_string()))
        .collect::<Vec<_>>();
        let row = |group, id, title: &str| (group, id, title.to_string());

        assert_eq!(
            rows,
            vec![
                row(ProviderGroup::Unaligned, "unaligned", "Unaligned"),
                row(
                    ProviderGroup::Subscription,
                    "openai-subscribed",
                    "ChatGPT Plus or Pro"
                ),
                row(
                    ProviderGroup::Subscription,
                    "copilot_chat",
                    "GitHub Copilot"
                ),
                row(ProviderGroup::Subscription, "x_ai_subscribed", "SuperGrok"),
                row(ProviderGroup::OwnKeyOrLocal, "anthropic", "Anthropic"),
                row(ProviderGroup::OwnKeyOrLocal, "openai", "OpenAI"),
                row(ProviderGroup::OwnKeyOrLocal, "ollama", "Ollama"),
                row(ProviderGroup::OwnKeyOrLocal, "lmstudio", "LM Studio"),
                row(ProviderGroup::OwnKeyOrLocal, "home server", "home server"),
                row(ProviderGroup::More, "deepseek", "DeepSeek"),
                row(ProviderGroup::More, "mistral", "Mistral"),
                row(ProviderGroup::More, "x_ai", "xAI"),
                row(ProviderGroup::More, "amazon-bedrock", "Amazon Bedrock"),
                row(ProviderGroup::More, "opencode", "OpenCode"),
            ]
        );
        assert_eq!(
            ProviderGroup::ALL.map(ProviderGroup::title),
            [
                "Unaligned · recommended",
                "Use a subscription you already have",
                "Use your own key or a local model",
                "More",
            ]
        );
    }

    #[test]
    fn connection_states_use_the_connection_screens_words() {
        let label = |state: ConnectionState| state.label().0.to_string();
        assert_eq!(label(ConnectionState::NotConnected), "Not connected");
        assert_eq!(label(ConnectionState::Checking), "Checking…");
        assert_eq!(label(ConnectionState::Connected(None)), "Connected");
        assert_eq!(
            label(ConnectionState::Connected(Some("$20.50 left".into()))),
            "Connected · $20.50 left"
        );
        assert_eq!(
            label(ConnectionState::NeedsAttention(
                "This key was refused.".into()
            )),
            "Needs attention"
        );
    }

    const LONG_STATUS: &str = "This subscription is signed in on another computer and has to \
        be confirmed in the browser before its models can answer in the chat.";

    struct WideConfigurationProviderRow;

    impl Render for WideConfigurationProviderRow {
        fn render(&mut self, _window: &mut Window, _cx: &mut Context<Self>) -> impl IntoElement {
            div().size_full().p_4().child(
                div()
                    .w_full()
                    .debug_selector(|| "provider-row".into())
                    .child(render_inline_body(
                        "ChatGPT Plus or Pro".into(),
                        Some("Signed In".into()),
                        Some(InlineDescription::Text(
                            "Your plan's models answer in the chat with the same tools as a key."
                                .into(),
                        )),
                        div()
                            .min_w_0()
                            .debug_selector(|| "provider-configuration".into())
                            .child(Label::new(LONG_STATUS)),
                    )),
            )
        }
    }

    #[gpui::test]
    fn wide_configuration_stays_within_provider_row(cx: &mut TestAppContext) {
        cx.update(|cx| {
            let settings_store = SettingsStore::test(cx);
            cx.set_global(settings_store);
            theme::init(theme::LoadThemes::JustBase, cx);
            theme_settings::init(theme::LoadThemes::JustBase, cx);
        });

        for width in [500., 600., 800.] {
            let window = cx.open_window(size(px(width), px(400.)), |_window, _cx| {
                WideConfigurationProviderRow
            });
            cx.run_until_parked();

            let mut visual_context = VisualTestContext::from_window(window.into(), cx);
            let provider_row_bounds = visual_context
                .debug_bounds("provider-row")
                .expect("provider row should be rendered");
            let description_bounds = visual_context
                .debug_bounds("inline-provider-description")
                .expect("provider description should be rendered");
            let configuration_bounds = visual_context
                .debug_bounds("provider-configuration")
                .expect("provider configuration should be rendered");

            assert!(
                configuration_bounds.right() <= provider_row_bounds.right(),
                "the configuration extends past the provider row at {width}px"
            );
            assert!(
                description_bounds.size.width >= px(width / 3.),
                "provider description collapsed at {width}px"
            );
        }
    }
}

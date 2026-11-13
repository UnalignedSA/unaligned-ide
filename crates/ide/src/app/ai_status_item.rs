//! The AI chip at the right end of the status bar.
//!
//! It says in a few capitals what would answer a chat message: `CONNECT AI`
//! while nothing is connected, else the default model, and for an Unaligned
//! model what is left on the key (`UNALIGNED $20.50`). A click opens the
//! Connections modal. The chip also publishes the same state for the welcome
//! page, which cannot see the model registry.

use std::time::{Duration, Instant};

use acp_thread::{AcpThread, AcpThreadEvent};
use agent_ui::{AgentPanel, AgentPanelEvent, ManageProviders};
use gpui::{
    App, Context, Empty, Entity, EntityId, IntoElement, ParentElement, Render, SharedString,
    Styled, Subscription, Window,
};
use language_model::{LanguageModelProvider as _, LanguageModelRegistry, ProviderConnectionStatus};
use language_models::{UNALIGNED_PROVIDER_ID, provider::unaligned::UnalignedLanguageModelProvider};
use project::DisableAiSettings;
use settings::{Settings as _, SettingsStore};
use ui::{Tooltip, prelude::*};
use workspace::{
    HideStatusItem, StatusItemView, Workspace,
    active_file_name::{
        status_bar_item, status_bar_item_content, status_bar_text, status_bar_text_in,
    },
    item::ItemHandle,
    welcome::AiConnection,
};

/// The chip shows at most this many characters of the model's name.
const MODEL_NAME_MAX_CHARS: usize = 24;

/// Under this many dollars the balance is drawn in the warning colour. The
/// provider's own wording says "· low" under the same amount.
const LOW_BALANCE: f64 = 1.0;

/// A pointer resting on the chip asks for the balance again, at most this
/// often. It covers what no chat turn of this window announces: a top-up made
/// on the website, an inline edit, a chat in another window.
const HOVER_REFRESH_INTERVAL: Duration = Duration::from_secs(60);

const TOOLTIP_TITLE: &str = "AI Connections";
const BALANCE_NOTE: &str = "Balance is refreshed after each answer.";
const LOW_BALANCE_NOTE: &str = "Low balance. Click to add funds or change connection.";
const CONNECT_NOTE: &str =
    "Unaligned key, a subscription you already have, your own key or a local model.";

/// What the chip says.
#[derive(Clone, Debug, PartialEq)]
enum AiState {
    /// No key, subscription or local model is connected.
    NotConnected,
    /// Something is connected, but no model is chosen or its list has not
    /// arrived yet.
    NoModel,
    /// The default model's connection reports a problem (a refused key, a
    /// server that cannot be reached).
    NeedsAttention { reason: SharedString },
    /// A chat message would reach `model_name`.
    Connected {
        model_name: SharedString,
        provider_name: SharedString,
        /// The dollars left on the Unaligned key, when the model is served by
        /// Unaligned and the server has answered.
        unaligned_balance: Option<f64>,
    },
}

impl AiState {
    fn current(cx: &App) -> Self {
        let registry = LanguageModelRegistry::read_global(cx);
        if !registry.has_authenticated_provider(cx) {
            return Self::NotConnected;
        }
        let Some(model) = registry.default_model() else {
            return Self::NoModel;
        };
        let Some(provider) = registry.provider(&model.provider_id) else {
            return Self::NoModel;
        };
        match provider.connection_status(cx) {
            ProviderConnectionStatus::NotConnected => Self::NoModel,
            ProviderConnectionStatus::NeedsAttention { reason } => Self::NeedsAttention { reason },
            ProviderConnectionStatus::Connected { .. } => {
                let unaligned_balance = (model.provider_id.0.as_ref() == UNALIGNED_PROVIDER_ID)
                    .then(|| UnalignedLanguageModelProvider::global(cx))
                    .flatten()
                    .and_then(|provider| provider.balance(cx));
                Self::Connected {
                    model_name: model.name.0.clone(),
                    provider_name: model.provider_name.0.clone(),
                    unaligned_balance,
                }
            }
        }
    }

    /// The same state in the welcome page's terms.
    fn for_welcome_page(&self) -> AiConnection {
        match self {
            Self::Connected { model_name, .. } => AiConnection::Connected {
                model_name: model_name.clone(),
            },
            Self::NotConnected | Self::NoModel | Self::NeedsAttention { .. } => {
                AiConnection::NotConnected
            }
        }
    }

    /// The second line of the tooltip.
    fn note(&self) -> SharedString {
        match self {
            Self::NotConnected => CONNECT_NOTE.into(),
            Self::NoModel => "No model is selected.".into(),
            Self::NeedsAttention { reason } => reason.clone(),
            Self::Connected {
                unaligned_balance: Some(balance),
                ..
            } if *balance < LOW_BALANCE => LOW_BALANCE_NOTE.into(),
            Self::Connected {
                unaligned_balance: Some(_),
                ..
            } => BALANCE_NOTE.into(),
            Self::Connected {
                model_name,
                provider_name,
                unaligned_balance: None,
            } => format!("{provider_name} · {model_name}").into(),
        }
    }

    fn aria_label(&self) -> SharedString {
        match self {
            Self::NotConnected => format!("{TOOLTIP_TITLE}: no model is connected").into(),
            Self::NoModel => format!("{TOOLTIP_TITLE}: no model is selected").into(),
            Self::NeedsAttention { reason } => format!("{TOOLTIP_TITLE}: {reason}").into(),
            Self::Connected {
                model_name,
                unaligned_balance: Some(balance),
                ..
            } => format!(
                "{TOOLTIP_TITLE}: Unaligned, {} left, {model_name}. {}",
                balance_text(*balance),
                self.note()
            )
            .into(),
            Self::Connected {
                model_name,
                provider_name,
                unaligned_balance: None,
            } => format!("{TOOLTIP_TITLE}: {provider_name}, {model_name}").into(),
        }
    }
}

/// "$20.50", and "under $0.01" below a cent, as the provider's own wording has it.
fn balance_text(balance: f64) -> String {
    if balance < 0.01 {
        "under $0.01".to_string()
    } else {
        format!("${balance:.2}")
    }
}

/// The model's display name, cut to [`MODEL_NAME_MAX_CHARS`] characters.
fn short_model_name(name: &str) -> String {
    if name.chars().count() <= MODEL_NAME_MAX_CHARS {
        return name.to_string();
    }
    let kept: String = name.chars().take(MODEL_NAME_MAX_CHARS - 1).collect();
    format!("{}…", kept.trim_end())
}

pub struct AiStatusItem {
    state: AiState,
    /// The chat whose turns are watched for the balance.
    watched_thread: Option<(EntityId, Subscription)>,
    last_balance_refresh: Option<Instant>,
    _agent_panel_subscription: Option<Subscription>,
    _subscriptions: Vec<Subscription>,
}

impl AiStatusItem {
    pub fn new(workspace: Entity<Workspace>, cx: &mut Context<Self>) -> Self {
        let registry = LanguageModelRegistry::global(cx);
        let subscriptions = vec![
            // The registry forwards every change of a provider's state, the
            // Unaligned balance included, as `ProviderStateChanged`.
            cx.subscribe(&registry, |this, _, event, cx| match event {
                language_model::Event::DefaultModelChanged
                | language_model::Event::ProviderStateChanged(_)
                | language_model::Event::AddedProvider(_)
                | language_model::Event::RemovedProvider(_)
                | language_model::Event::ProvidersChanged => this.update_state(cx),
                language_model::Event::InlineAssistantModelChanged
                | language_model::Event::CommitMessageModelChanged
                | language_model::Event::CompactionModelChanged
                | language_model::Event::ThreadSummaryModelChanged => {}
            }),
            // Panels are loaded after the status bar is built.
            cx.subscribe(&workspace, |this, _, event, cx| {
                if let workspace::Event::PanelAdded(panel) = event
                    && let Ok(panel) = panel.clone().downcast::<AgentPanel>()
                {
                    this.watch_agent_panel(panel, cx);
                }
            }),
            // `disable_ai` hides the chip.
            cx.observe_global::<SettingsStore>(|_, cx| cx.notify()),
        ];

        let state = AiState::current(cx);
        state.for_welcome_page().set(cx);
        Self {
            state,
            watched_thread: None,
            last_balance_refresh: None,
            _agent_panel_subscription: None,
            _subscriptions: subscriptions,
        }
    }

    fn update_state(&mut self, cx: &mut Context<Self>) {
        let state = AiState::current(cx);
        state.for_welcome_page().set(cx);
        if state != self.state {
            self.state = state;
            cx.notify();
        }
    }

    fn watch_agent_panel(&mut self, panel: Entity<AgentPanel>, cx: &mut Context<Self>) {
        self._agent_panel_subscription = Some(
            cx.subscribe(&panel, |this, panel, _: &AgentPanelEvent, cx| {
                this.watch_active_thread(&panel, cx)
            }),
        );
        self.watch_active_thread(&panel, cx);
    }

    /// Follows the chat shown in the panel, so the end of each of its turns
    /// can ask for the balance. Nothing runs between turns.
    fn watch_active_thread(&mut self, panel: &Entity<AgentPanel>, cx: &mut Context<Self>) {
        let thread: Option<Entity<AcpThread>> = panel.read(cx).active_agent_thread(cx);
        let Some(thread) = thread else {
            self.watched_thread = None;
            return;
        };
        if self
            .watched_thread
            .as_ref()
            .is_some_and(|(watched, _)| *watched == thread.entity_id())
        {
            return;
        }
        let subscription = cx.subscribe(&thread, |this, _, event, cx| {
            if matches!(
                event,
                AcpThreadEvent::Stopped { .. } | AcpThreadEvent::Error | AcpThreadEvent::Refusal
            ) {
                this.refresh_balance(cx);
            }
        });
        self.watched_thread = Some((thread.entity_id(), subscription));
    }

    /// Asks Unaligned what is left, when the model that answers is Unaligned's.
    /// The new balance arrives through the registry's `ProviderStateChanged`.
    fn refresh_balance(&mut self, cx: &mut Context<Self>) {
        let answers_through_unaligned = LanguageModelRegistry::read_global(cx)
            .default_model()
            .is_some_and(|model| model.provider_id.0.as_ref() == UNALIGNED_PROVIDER_ID);
        if !answers_through_unaligned {
            return;
        }
        let Some(provider) = UnalignedLanguageModelProvider::global(cx) else {
            return;
        };
        if !provider.is_authenticated(cx) {
            return;
        }
        self.last_balance_refresh = Some(Instant::now());
        // A refused key or an unreachable server is already told by the
        // provider's connection status, which the chip shows.
        provider.refresh_balance(cx).detach();
    }

    fn refresh_balance_on_hover(&mut self, cx: &mut Context<Self>) {
        let refreshed_recently = self
            .last_balance_refresh
            .is_some_and(|refreshed| refreshed.elapsed() < HOVER_REFRESH_INTERVAL);
        if !refreshed_recently {
            self.refresh_balance(cx);
        }
    }
}

impl Render for AiStatusItem {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        if DisableAiSettings::get_global(cx).disable_ai {
            return Empty.into_any_element();
        }

        let text_color = cx.theme().colors().text;
        let warning = cx.theme().status().warning;

        let content = match &self.state {
            // The one state that asks for something: the text colour, never the accent.
            AiState::NotConnected => {
                status_bar_item_content().child(status_bar_text_in("Connect AI", text_color, cx))
            }
            AiState::NoModel => status_bar_item_content().child(status_bar_text("No model", cx)),
            AiState::NeedsAttention { .. } => {
                status_bar_item_content().child(status_bar_text_in("Check connection", warning, cx))
            }
            AiState::Connected {
                model_name,
                unaligned_balance,
                ..
            } => status_bar_item_content()
                .gap_2()
                .children(unaligned_balance.map(|balance| {
                    let amount = balance_text(balance);
                    h_flex()
                        .gap_1p5()
                        .child(status_bar_text("Unaligned", cx))
                        .child(if balance < LOW_BALANCE {
                            status_bar_text_in(amount, warning, cx)
                        } else {
                            status_bar_text(amount, cx)
                        })
                }))
                .child(status_bar_text(short_model_name(model_name), cx)),
        };

        let note = self.state.note();
        div()
            .id("ai-status")
            .on_hover(cx.listener(|this, hovered: &bool, _, cx| {
                if *hovered {
                    this.refresh_balance_on_hover(cx);
                }
            }))
            .child(
                status_bar_item("ai-status-chip", cx)
                    .tab_index(0isize)
                    .aria_label(self.state.aria_label())
                    .child(content)
                    .tooltip(move |_window, cx| {
                        Tooltip::with_meta(TOOLTIP_TITLE, Some(&ManageProviders), note.clone(), cx)
                    })
                    .on_click(|_, window, cx| {
                        window.dispatch_action(Box::new(ManageProviders), cx);
                    }),
            )
            .into_any_element()
    }
}

impl StatusItemView for AiStatusItem {
    fn set_active_pane_item(
        &mut self,
        _active_pane_item: Option<&dyn ItemHandle>,
        _window: &mut Window,
        _cx: &mut Context<Self>,
    ) {
        // The chip follows the connections, not the active item.
    }

    fn hide_setting(&self, _: &App) -> Option<HideStatusItem> {
        // Hidden by `disable_ai`; there is no setting of its own.
        None
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn balance_wording() {
        assert_eq!(balance_text(20.5), "$20.50");
        assert_eq!(balance_text(0.04), "$0.04");
        assert_eq!(balance_text(0.004), "under $0.01");
        assert_eq!(balance_text(0.0), "under $0.01");
    }

    #[test]
    fn model_names_are_cut_to_twenty_four_characters() {
        assert_eq!(short_model_name("Claude Sonnet 5.5"), "Claude Sonnet 5.5");
        let exactly = "a".repeat(MODEL_NAME_MAX_CHARS);
        assert_eq!(short_model_name(&exactly), exactly);
        let cut = short_model_name("Qwen3 Coder 480B A35B Instruct Turbo");
        assert_eq!(cut, "Qwen3 Coder 480B A35B I…");
        assert_eq!(cut.chars().count(), MODEL_NAME_MAX_CHARS);
    }

    #[test]
    fn welcome_page_only_hears_connected_when_a_model_answers() {
        assert_eq!(
            AiState::Connected {
                model_name: "Claude Sonnet 5.5".into(),
                provider_name: "Unaligned".into(),
                unaligned_balance: Some(20.5),
            }
            .for_welcome_page(),
            AiConnection::Connected {
                model_name: "Claude Sonnet 5.5".into()
            }
        );
        for state in [
            AiState::NotConnected,
            AiState::NoModel,
            AiState::NeedsAttention {
                reason: "This key was refused.".into(),
            },
        ] {
            assert_eq!(state.for_welcome_page(), AiConnection::NotConnected);
        }
    }
}

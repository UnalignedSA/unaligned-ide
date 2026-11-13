mod agent_api_keys_onboarding;
mod agent_panel_onboarding_card;
mod agent_panel_onboarding_content;
mod edit_prediction_onboarding_content;
mod plan_definitions;
mod young_account_banner;

pub use agent_api_keys_onboarding::{ApiKeysWithProviders, ApiKeysWithoutProviders};
pub use agent_panel_onboarding_card::AgentPanelOnboardingCard;
pub use agent_panel_onboarding_content::AgentPanelOnboarding;
use cloud_api_types::Plan;
pub use edit_prediction_onboarding_content::EditPredictionOnboarding;
pub use plan_definitions::PlanDefinitions;
pub use young_account_banner::YoungAccountBanner;

use std::sync::Arc;

use client::{Client, UserStore, zed_urls};
use gpui::{Action as _, AnyElement, Entity, IntoElement, ParentElement, TaskExt};
use ui::{Divider, DividerColor, RegisterComponent, SectionLabel, Tooltip, prelude::*};

/// Section header shared by the AI onboarding surfaces: the interface's micro label,
/// an optional muted note, then a 1px rule.
pub(crate) fn section_header(title: &str, note: Option<&str>, _cx: &App) -> impl IntoElement {
    h_flex()
        .gap_2()
        .child(
            div()
                .flex_none()
                .child(SectionLabel::new(title.to_string())),
        )
        .when_some(note, |this, note| {
            this.child(
                div()
                    .flex_none()
                    .child(SectionLabel::new(note.to_string()).color(Color::Muted)),
            )
        })
        .child(Divider::horizontal().color(DividerColor::BorderVariant))
}

/// What every surface of this crate shows in a build without accounts: the hosted
/// plans below do not exist there, so the only thing to offer is a connection.
const CONNECT_DETAIL: &str =
    "Use an Unaligned key, a subscription you already have, your own key or a local model.";
// Defined in the agent crate, which depends on this one: built by name.
const MANAGE_CONNECTIONS_ACTION: &str = "agent::ManageProviders";

pub(crate) fn connect_a_model_button(id: &'static str) -> Button {
    Button::new(id, "Connect a Model")
        .full_width()
        .style(ButtonStyle::Outlined)
        .on_click(move |_, window, cx| {
            let action = cx
                .build_action(MANAGE_CONNECTIONS_ACTION, None)
                .unwrap_or_else(|_| app_actions::agent::OpenSettings.boxed_clone());
            window.dispatch_action(action, cx);
        })
}

#[derive(PartialEq)]
pub enum SignInStatus {
    SignedIn,
    SigningIn,
    SignedOut,
}

impl From<client::Status> for SignInStatus {
    fn from(status: client::Status) -> Self {
        if status.is_signing_in() {
            Self::SigningIn
        } else if status.is_signed_out() {
            Self::SignedOut
        } else {
            Self::SignedIn
        }
    }
}

#[derive(RegisterComponent, IntoElement)]
pub struct ZedAiOnboarding {
    /// False in this product: no hosted plan, no sign-in. The plan screens below
    /// are then never drawn.
    pub accounts_available: bool,
    pub sign_in_status: SignInStatus,
    pub plan: Option<Plan>,
    pub account_too_young: bool,
    pub continue_with_zed_ai: Arc<dyn Fn(&mut Window, &mut App)>,
    pub sign_in: Arc<dyn Fn(&mut Window, &mut App)>,
    pub dismiss_onboarding: Option<Arc<dyn Fn(&mut Window, &mut App)>>,
}

impl ZedAiOnboarding {
    pub fn new(
        client: Arc<Client>,
        user_store: &Entity<UserStore>,
        continue_with_zed_ai: Arc<dyn Fn(&mut Window, &mut App)>,
        cx: &mut App,
    ) -> Self {
        let store = user_store.read(cx);
        let status = *client.status().borrow();

        Self {
            accounts_available: client.accounts_available(),
            sign_in_status: status.into(),
            plan: store.plan(),
            account_too_young: store.account_too_young(),
            continue_with_zed_ai,
            sign_in: Arc::new(move |_window, cx| {
                if !client.accounts_available() {
                    return;
                }
                cx.spawn({
                    let client = client.clone();
                    async move |cx| client.sign_in_with_optional_connect(true, cx).await
                })
                .detach_and_log_err(cx);
            }),
            dismiss_onboarding: None,
        }
    }

    pub fn with_dismiss(
        mut self,
        dismiss_callback: impl Fn(&mut Window, &mut App) + 'static,
    ) -> Self {
        self.dismiss_onboarding = Some(Arc::new(dismiss_callback));
        self
    }

    fn render_dismiss_button(&self) -> Option<AnyElement> {
        self.dismiss_onboarding.as_ref().map(|dismiss_callback| {
            let callback = dismiss_callback.clone();

            h_flex()
                .absolute()
                .top_0()
                .right_0()
                .child(
                    IconButton::new("dismiss_onboarding", IconName::Close)
                        .icon_size(IconSize::Small)
                        .tooltip(Tooltip::text("Dismiss"))
                        .on_click(move |_, window, cx| {
                            telemetry::event!("Banner Dismissed", source = "AI Onboarding",);
                            callback(window, cx)
                        }),
                )
                .into_any_element()
        })
    }

    /// The only screen of a build without accounts.
    fn render_connect_a_model(&self) -> AnyElement {
        v_flex()
            .w_full()
            .relative()
            .gap_1()
            .child(Headline::new("Connect a Model").size(HeadlineSize::XSmall))
            .child(Label::new(CONNECT_DETAIL).color(Color::Muted).mb_2())
            .child(connect_a_model_button("connect-a-model"))
            .children(self.render_dismiss_button())
            .into_any_element()
    }

    fn render_sign_in_disclaimer(&self, _cx: &mut App) -> AnyElement {
        let signing_in = matches!(self.sign_in_status, SignInStatus::SigningIn);

        v_flex()
            .w_full()
            .relative()
            .gap_1()
            .child(Headline::new("Hosted AI").size(HeadlineSize::XSmall))
            .child(
                Label::new("Sign in to try GPT Luna. Your 14 days begin when you start the trial.")
                    .color(Color::Muted)
                    .mb_2(),
            )
            .child(PlanDefinitions.sign_in_upsell())
            .child(
                Button::new("sign_in", "Sign In to Try GPT Luna")
                    .disabled(signing_in)
                    .full_width()
                    .style(ButtonStyle::Outlined)
                    .on_click({
                        let callback = self.sign_in.clone();
                        move |_, window, cx| {
                            telemetry::event!("Start Trial Clicked", state = "pre-sign-in");
                            callback(window, cx)
                        }
                    }),
            )
            .children(self.render_dismiss_button())
            .into_any_element()
    }

    fn render_free_plan_state(&self, cx: &mut App) -> AnyElement {
        if self.account_too_young {
            v_flex()
                .relative()
                .min_w_0()
                .gap_1()
                .child(Headline::new("Hosted AI").size(HeadlineSize::XSmall))
                .child(YoungAccountBanner)
                .child(
                    v_flex()
                        .mt_2()
                        .gap_1()
                        .child(section_header("Pro", None, cx))
                        .child(PlanDefinitions.pro_plan())
                        .child(
                            Button::new("pro", "Get Started")
                                .full_width()
                                .style(ButtonStyle::Outlined)
                                .on_click(move |_, _window, cx| {
                                    telemetry::event!(
                                        "Upgrade To Pro Clicked",
                                        state = "young-account"
                                    );
                                    cx.open_url(&zed_urls::upgrade_to_zed_pro_url(cx))
                                }),
                        ),
                )
                .into_any_element()
        } else {
            v_flex()
                .w_full()
                .relative()
                .gap_1()
                .child(Headline::new("Hosted AI").size(HeadlineSize::XSmall))
                .child(
                    v_flex()
                        .mt_2()
                        .gap_1()
                        .child(section_header("Free", Some("Current plan"), cx))
                        .child(PlanDefinitions.free_plan()),
                )
                .children(self.render_dismiss_button())
                .child(
                    v_flex()
                        .mt_2()
                        .gap_1()
                        .child(section_header("Pro Trial", None, cx))
                        .child(PlanDefinitions.pro_trial(true))
                        .child(
                            Button::new("pro", "Start Free Trial")
                                .full_width()
                                .style(ButtonStyle::Outlined)
                                .on_click(move |_, _window, cx| {
                                    telemetry::event!(
                                        "Start Trial Clicked",
                                        state = "post-sign-in"
                                    );
                                    cx.open_url(&zed_urls::start_trial_url(cx))
                                }),
                        ),
                )
                .into_any_element()
        }
    }

    fn render_trial_state(&self, _cx: &mut App) -> AnyElement {
        v_flex()
            .w_full()
            .relative()
            .gap_1()
            .child(Headline::new("Pro Trial").size(HeadlineSize::XSmall))
            .child(
                Label::new("Included for 14 days from when your trial started:")
                    .color(Color::Muted)
                    .mb_2(),
            )
            .child(PlanDefinitions.pro_trial(false))
            .children(self.render_dismiss_button())
            .into_any_element()
    }

    fn render_pro_plan_state(&self, _cx: &mut App) -> AnyElement {
        v_flex()
            .w_full()
            .relative()
            .gap_1()
            .child(Headline::new("Pro").size(HeadlineSize::XSmall))
            .child(
                Label::new("Here's what you get:")
                    .color(Color::Muted)
                    .mb_2(),
            )
            .child(PlanDefinitions.pro_plan())
            .children(self.render_dismiss_button())
            .into_any_element()
    }

    fn render_business_plan_state(&self, _cx: &mut App) -> AnyElement {
        v_flex()
            .w_full()
            .relative()
            .gap_1()
            .child(Headline::new("Business").size(HeadlineSize::XSmall))
            .child(
                Label::new("Here's what you get:")
                    .color(Color::Muted)
                    .mb_2(),
            )
            .child(PlanDefinitions.business_plan())
            .children(self.render_dismiss_button())
            .into_any_element()
    }

    fn render_vip_plan_state(&self, _cx: &mut App) -> AnyElement {
        v_flex()
            .w_full()
            .relative()
            .gap_1()
            .child(Headline::new("VIP").size(HeadlineSize::XSmall))
            .child(
                Label::new("Here's what you get:")
                    .color(Color::Muted)
                    .mb_2(),
            )
            .child(PlanDefinitions.vip_plan())
            .children(self.render_dismiss_button())
            .into_any_element()
    }

    fn render_student_plan_state(&self, _cx: &mut App) -> AnyElement {
        v_flex()
            .w_full()
            .relative()
            .gap_1()
            .child(Headline::new("Student").size(HeadlineSize::XSmall))
            .child(
                Label::new("Here's what you get:")
                    .color(Color::Muted)
                    .mb_2(),
            )
            .child(PlanDefinitions.student_plan())
            .children(self.render_dismiss_button())
            .into_any_element()
    }
}

impl RenderOnce for ZedAiOnboarding {
    fn render(self, _window: &mut ui::Window, cx: &mut App) -> impl IntoElement {
        if !self.accounts_available {
            return self.render_connect_a_model();
        }
        if matches!(self.sign_in_status, SignInStatus::SignedIn) {
            match self.plan {
                None => self.render_free_plan_state(cx),
                Some(Plan::ZedFree) => self.render_free_plan_state(cx),
                Some(Plan::ZedProTrial) => self.render_trial_state(cx),
                Some(Plan::ZedPro) => self.render_pro_plan_state(cx),
                Some(Plan::ZedBusiness) => self.render_business_plan_state(cx),
                Some(Plan::ZedVip) => self.render_vip_plan_state(cx),
                Some(Plan::ZedStudent) => self.render_student_plan_state(cx),
            }
        } else {
            self.render_sign_in_disclaimer(cx)
        }
    }
}

impl Component for ZedAiOnboarding {
    fn scope() -> ComponentScope {
        ComponentScope::Onboarding
    }

    fn name() -> &'static str {
        "Agent New User Onboarding"
    }

    fn description() -> &'static str {
        "The onboarding surface shown to new agent panel users, \
        guiding them through signing in and selecting a plan \
        before they can start using the agent."
    }

    fn preview(_window: &mut Window, _cx: &mut App) -> AnyElement {
        fn onboarding(
            sign_in_status: SignInStatus,
            plan: Option<Plan>,
            account_too_young: bool,
        ) -> AnyElement {
            div()
                .w_full()
                .min_w_40()
                .max_w(px(1100.))
                .child(
                    AgentPanelOnboardingCard::new().child(
                        ZedAiOnboarding {
                            accounts_available: true,
                            sign_in_status,
                            plan,
                            account_too_young,
                            continue_with_zed_ai: Arc::new(|_, _| {}),
                            sign_in: Arc::new(|_, _| {}),
                            dismiss_onboarding: None,
                        }
                        .into_any_element(),
                    ),
                )
                .into_any_element()
        }

        v_flex()
            .min_w_0()
            .gap_4()
            .children(vec![
                single_example(
                    "Not Signed-in",
                    onboarding(SignInStatus::SignedOut, None, false),
                ),
                single_example(
                    "Young Account",
                    onboarding(SignInStatus::SignedIn, None, true),
                ),
                single_example(
                    "Free Plan",
                    onboarding(SignInStatus::SignedIn, Some(Plan::ZedFree), false),
                ),
                single_example(
                    "Pro Trial",
                    onboarding(SignInStatus::SignedIn, Some(Plan::ZedProTrial), false),
                ),
                single_example(
                    "Pro Plan",
                    onboarding(SignInStatus::SignedIn, Some(Plan::ZedPro), false),
                ),
                single_example(
                    "Business Plan",
                    onboarding(SignInStatus::SignedIn, Some(Plan::ZedBusiness), false),
                ),
                single_example(
                    "VIP Plan",
                    onboarding(SignInStatus::SignedIn, Some(Plan::ZedVip), false),
                ),
                single_example(
                    "Student Plan",
                    onboarding(SignInStatus::SignedIn, Some(Plan::ZedStudent), false),
                ),
            ])
            .into_any_element()
    }
}

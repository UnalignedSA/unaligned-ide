use gpui::{IntoElement, ParentElement, RenderOnce};
use language_model::{IDE_CLOUD_PROVIDER_ID, IconOrSvg, LanguageModelRegistry};
use ui::{List, ListBulletItem, prelude::*};

pub struct ApiKeysWithProviders {
    configured_providers: Vec<(IconOrSvg, SharedString)>,
}

impl ApiKeysWithProviders {
    pub fn new(cx: &mut Context<Self>) -> Self {
        cx.subscribe(
            &LanguageModelRegistry::global(cx),
            |this: &mut Self, _registry, event: &language_model::Event, cx| match event {
                language_model::Event::ProviderStateChanged(_)
                | language_model::Event::AddedProvider(_)
                | language_model::Event::RemovedProvider(_)
                | language_model::Event::ProvidersChanged => {
                    this.configured_providers = Self::compute_configured_providers(cx)
                }
                _ => {}
            },
        )
        .detach();

        Self {
            configured_providers: Self::compute_configured_providers(cx),
        }
    }

    fn compute_configured_providers(cx: &App) -> Vec<(IconOrSvg, SharedString)> {
        LanguageModelRegistry::read_global(cx)
            .visible_providers()
            .iter()
            .filter(|provider| {
                provider.is_authenticated(cx) && provider.id() != IDE_CLOUD_PROVIDER_ID
            })
            .map(|provider| (provider.icon(), provider.name().0))
            .collect()
    }
}

impl Render for ApiKeysWithProviders {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let configured_providers_list =
            self.configured_providers
                .iter()
                .cloned()
                .map(|(icon, name)| {
                    h_flex()
                        .gap_1p5()
                        .child(
                            match icon {
                                IconOrSvg::Icon(icon_name) => Icon::new(icon_name),
                                IconOrSvg::Svg(icon_path) => Icon::from_external_svg(icon_path),
                            }
                            .size(IconSize::XSmall)
                            .color(Color::Muted),
                        )
                        .child(Label::new(name).size(LabelSize::Small))
                });
        // Flat strip above the message editor: one line on top, no shadow, no inner card.
        h_flex()
            .px_3()
            .py_1p5()
            .gap_2()
            .flex_wrap()
            .border_t_1()
            .border_color(cx.theme().colors().border)
            .bg(cx.theme().colors().panel_background)
            .child(
                h_flex()
                    .min_w_0()
                    .gap_2()
                    .child(
                        Icon::new(IconName::Info)
                            .size(IconSize::XSmall)
                            .color(Color::Muted),
                    )
                    .child(
                        div().w_full().child(
                            Label::new("Keys found in your environment are ready to use:")
                                .size(LabelSize::Small)
                                .color(Color::Muted),
                        ),
                    ),
            )
            .children(configured_providers_list)
    }
}

#[derive(IntoElement)]
pub struct ApiKeysWithoutProviders;

impl ApiKeysWithoutProviders {
    pub fn new() -> Self {
        Self
    }
}

impl RenderOnce for ApiKeysWithoutProviders {
    fn render(self, _window: &mut Window, cx: &mut App) -> impl IntoElement {
        v_flex()
            .mt_2()
            .gap_1()
            .child(crate::section_header("Your own keys", None, cx))
            .child(List::new().child(ListBulletItem::new(
                "Add your own keys to use AI without an account.",
            )))
            // Never the filled accent: the surface that holds this card has its own
            // primary action.
            .child(crate::connect_a_model_button("configure-providers"))
    }
}

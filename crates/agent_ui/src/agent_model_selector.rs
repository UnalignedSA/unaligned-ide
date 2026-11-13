use crate::{
    ModelUsageContext,
    connections::{self, Backing},
    language_model_selector::{LanguageModelSelector, language_model_selector},
    ui::ModelSelectorTooltip,
};
use fs::Fs;
use gpui::{Entity, FocusHandle, SharedString};
use language_model::LanguageModelRegistry;
use picker::popover_menu::PickerPopoverMenu;
use settings::update_settings_file;
use std::sync::Arc;
use ui::{ButtonLike, PopoverMenuHandle, Tooltip, prelude::*};

pub struct AgentModelSelector {
    selector: Entity<LanguageModelSelector>,
    menu_handle: PopoverMenuHandle<LanguageModelSelector>,
}

impl AgentModelSelector {
    pub(crate) fn new(
        fs: Arc<dyn Fs>,
        menu_handle: PopoverMenuHandle<LanguageModelSelector>,
        focus_handle: FocusHandle,
        model_usage_context: ModelUsageContext,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Self {
        Self {
            selector: cx.new(move |cx| {
                language_model_selector(
                    {
                        let model_context = model_usage_context.clone();
                        move |cx| model_context.model(cx)
                    },
                    {
                        let fs = fs.clone();
                        move |model, cx| {
                            let provider = model.provider_id().0.to_string();
                            let model_id = model.id().0.to_string();
                            match &model_usage_context {
                                ModelUsageContext::InlineAssistant => {
                                    update_settings_file(fs.clone(), cx, move |settings, _cx| {
                                        settings
                                            .agent
                                            .get_or_insert_default()
                                            .set_inline_assistant_model(provider.clone(), model_id);
                                    });
                                }
                            }
                        }
                    },
                    {
                        let fs = fs.clone();
                        move |model, should_be_favorite, cx| {
                            crate::favorite_models::toggle_in_settings(
                                model,
                                should_be_favorite,
                                fs.clone(),
                                cx,
                            );
                        }
                    },
                    true, // Use popover styles for picker
                    focus_handle.clone(),
                    window,
                    cx,
                )
            }),
            menu_handle,
        }
    }

    pub fn toggle(&self, window: &mut Window, cx: &mut Context<Self>) {
        self.menu_handle.toggle(window, cx);
    }

    pub fn active_model(&self, cx: &App) -> Option<language_model::LanguageModel> {
        self.selector.read(cx).delegate.active_model(cx)
    }

    pub fn cycle_favorite_models(&self, window: &mut Window, cx: &mut Context<Self>) {
        self.selector.update(cx, |selector, cx| {
            selector.delegate.cycle_favorite_models(window, cx);
        });
    }
}

impl Render for AgentModelSelector {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let model = self.selector.read(cx).delegate.active_model(cx);
        let model_name = model
            .as_ref()
            .map(|model| model.name().0)
            .unwrap_or_else(|| SharedString::from("Select a Model"));

        // Who answers and at which price is said by the tooltip; the button
        // itself carries the model's name only. The provider is found by id
        // and named as its connection is in the catalog.
        let provider_name = model.as_ref().and_then(|model| {
            let catalog_title = connections::catalog(cx).into_iter().find_map(|spec| {
                matches!(&spec.backing, Backing::Provider(id) if id == &model.provider_id)
                    .then_some(spec.title)
            });
            catalog_title.or_else(|| {
                LanguageModelRegistry::read_global(cx)
                    .provider(&model.provider_id)
                    .map(|provider| provider.name().0)
            })
        });
        let cost = model
            .as_ref()
            .and_then(|model| model.cost_info.as_ref())
            .map(|cost| cost.to_shared_string());
        // An open menu lifts the trigger to the text colour; the accent stays
        // with the send button.
        let color = if self.menu_handle.is_deployed() {
            Color::Default
        } else {
            Color::Muted
        };

        let show_cycle_row = self.selector.read(cx).delegate.favorites_count() > 1;

        let tooltip = Tooltip::element({
            move |_, _cx| {
                ModelSelectorTooltip::new()
                    .provider(provider_name.clone())
                    .cost(cost.clone())
                    .show_cycle_row(show_cycle_row)
                    .into_any_element()
            }
        });

        PickerPopoverMenu::new(
            self.selector.clone(),
            ButtonLike::new("active-model").child(
                h_flex()
                    .gap_1()
                    .child(
                        Label::new(model_name)
                            .size(LabelSize::Small)
                            .color(color)
                            .truncate(),
                    )
                    .child(
                        Icon::new(IconName::ChevronDown)
                            .color(Color::Muted)
                            .size(IconSize::XSmall),
                    ),
            ),
            tooltip,
            gpui::Anchor::TopRight,
            cx,
        )
        .with_handle(self.menu_handle.clone())
        .offset(gpui::Point {
            x: px(0.0),
            y: px(2.0),
        })
        .render(window, cx)
    }
}

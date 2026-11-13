use std::rc::Rc;

use acp_thread::{AgentModelInfo, AgentModelSelector};
use gpui::{Entity, FocusHandle};
use picker::popover_menu::PickerPopoverMenu;
use ui::{ButtonLike, PopoverMenuHandle, Tooltip, prelude::*};

use crate::ui::ModelSelectorTooltip;
use crate::{ModelSelector, model_selector::acp_model_selector};

pub struct ModelSelectorPopover {
    selector: Entity<ModelSelector>,
    menu_handle: PopoverMenuHandle<ModelSelector>,
}

impl ModelSelectorPopover {
    pub(crate) fn new(
        selector: Rc<dyn AgentModelSelector>,
        menu_handle: PopoverMenuHandle<ModelSelector>,
        focus_handle: FocusHandle,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Self {
        Self {
            selector: cx
                .new(move |cx| acp_model_selector(selector, focus_handle.clone(), window, cx)),
            menu_handle,
        }
    }

    pub fn toggle(&self, window: &mut Window, cx: &mut Context<Self>) {
        self.menu_handle.toggle(window, cx);
    }

    pub fn active_model<'a>(&self, cx: &'a App) -> Option<&'a AgentModelInfo> {
        self.selector.read(cx).delegate.active_model()
    }

    pub fn cycle_favorite_models(&self, window: &mut Window, cx: &mut Context<Self>) {
        self.selector.update(cx, |selector, cx| {
            selector.delegate.cycle_favorite_models(window, cx);
        });
    }
}

impl Render for ModelSelectorPopover {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let selector = self.selector.read(cx);
        let model = selector.delegate.active_model();
        let model_name = model
            .as_ref()
            .map(|model| model.name.clone())
            .unwrap_or_else(|| SharedString::from("Select a Model"));

        // Who answers and at which price is said by the tooltip; the button
        // itself carries the model's name only.
        let connection = selector.delegate.active_model_connection();
        let cost = model.as_ref().and_then(|model| model.cost.clone());

        let (color, icon) = if self.menu_handle.is_deployed() {
            (Color::Default, IconName::ChevronUp)
        } else {
            (Color::Muted, IconName::ChevronDown)
        };

        let show_cycle_row = selector.delegate.favorites_count() > 1;

        let tooltip = Tooltip::element({
            move |_, _cx| {
                ModelSelectorTooltip::new()
                    .provider(connection.clone())
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
                    .child(Icon::new(icon).color(Color::Muted).size(IconSize::XSmall)),
            ),
            tooltip,
            // Opens from the button's left edge and is kept inside the window:
            // with the chat docked right it stays within the chat column.
            gpui::Anchor::BottomLeft,
            cx,
        )
        .with_handle(self.menu_handle.clone())
        .render(window, cx)
    }
}

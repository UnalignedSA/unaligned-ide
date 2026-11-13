use gpui::{AnyElement, IntoElement, ParentElement};
use smallvec::SmallVec;
use ui::prelude::*;

#[derive(IntoElement)]
pub struct AgentPanelOnboardingCard {
    children: SmallVec<[AnyElement; 2]>,
}

impl AgentPanelOnboardingCard {
    pub fn new() -> Self {
        Self {
            children: SmallVec::new(),
        }
    }
}

impl ParentElement for AgentPanelOnboardingCard {
    fn extend(&mut self, elements: impl IntoIterator<Item = AnyElement>) {
        self.children.extend(elements)
    }
}

impl RenderOnce for AgentPanelOnboardingCard {
    fn render(self, _window: &mut Window, cx: &mut App) -> impl IntoElement {
        let color = cx.theme().colors();

        // Flat card: one raised background level and a 1px line, no gradient or shadow.
        div().min_w_0().p_2p5().bg(color.editor_background).child(
            v_flex()
                .relative()
                .size_full()
                .min_w_0()
                .px_4()
                .py_3()
                .gap_2()
                .border_1()
                .border_color(color.border)
                .bg(color.surface_background)
                .overflow_hidden()
                .children(self.children),
        )
    }
}

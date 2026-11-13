use crate::prelude::*;

#[derive(IntoElement)]
pub struct ListSeparator;

impl RenderOnce for ListSeparator {
    fn render(self, _window: &mut Window, cx: &mut App) -> impl IntoElement {
        div()
            .h_px()
            .w_full()
            .my(DynamicSpacing::Base04.rems(cx))
            .bg(cx.theme().colors().border)
    }
}

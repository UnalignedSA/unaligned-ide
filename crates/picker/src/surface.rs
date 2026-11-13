//! The parts every picker shares, so that the command palette, the file
//! finder, the theme selector and every other picker read as one system: one
//! query row, one row inset, one footer, one empty state.
//!
//! [`Picker`](crate::Picker) uses them for its own chrome. Delegates that draw
//! their own footer build it from [`footer`] and [`footer_button`]; views that
//! look like a picker without being one (go to line) copy the same metrics.

use gpui::{Action, App, Div, FocusHandle};
use ui::{KeyBinding, prelude::*};

/// The height of a picker's query row, in pixels at the default font size.
pub const QUERY_ROW_HEIGHT: f32 = 36.;

/// The height of a picker's footer, in pixels at the default font size.
pub const FOOTER_HEIGHT: f32 = 32.;

/// What the picker adds on each side of a row, in pixels: together with the
/// 8px of a `ListItem` it puts row content on the 12px inset that the query
/// row and the empty state use.
pub(crate) const ROW_GUTTER: f32 = 4.;

/// The query row of a picker: a search icon, then the editor, on the raised
/// ground of the picker itself. Add a right-aligned hint or a toggle with
/// `.child(..)`; the caller draws the 1px line under the row.
pub fn query_row(editor: impl IntoElement) -> Div {
    h_flex()
        .h(rems_from_px(QUERY_ROW_HEIGHT))
        .flex_none()
        .px_3()
        .gap_2()
        .overflow_hidden()
        .child(
            Icon::new(IconName::MagnifyingGlass)
                .size(IconSize::Small)
                .color(Color::Muted),
        )
        .child(div().flex_1().min_w_0().child(editor))
}

/// The footer of a picker: a 32px strip on the chrome ground under a 1px
/// line. Children are spread with `justify_between`: a single group of
/// actions goes to the right with `.justify_end()`.
pub fn footer(cx: &App) -> Div {
    let colors = cx.theme().colors();
    h_flex()
        .w_full()
        .h(rems_from_px(FOOTER_HEIGHT))
        .flex_none()
        .px_1p5()
        .gap_1()
        .justify_between()
        .bg(colors.background)
        .border_t_1()
        .border_color(colors.border)
        // Follows the corners of the surface it closes (none in a square theme).
        .rounded_b_lg()
}

/// An action in a picker's footer: a subtle button with a small muted label.
/// Give it a key cap with [`footer_key_cap`].
pub fn footer_button(id: impl Into<ElementId>, label: impl Into<SharedString>) -> Button {
    Button::new(id, label)
        .label_size(LabelSize::Small)
        .color(Color::Muted)
}

/// The key cap of a footer action, at the footer's size.
pub fn footer_key_cap(action: &dyn Action, focus_handle: &FocusHandle, cx: &App) -> KeyBinding {
    KeyBinding::for_action_in(action, focus_handle, cx).size(rems_from_px(12_f32))
}

/// The one action a picker may offer when nothing matches, shown as a row of
/// the empty state ("More Themes…", "Connections…").
pub struct PickerEmptyAction {
    pub label: SharedString,
    pub action: Box<dyn Action>,
}

impl PickerEmptyAction {
    pub fn new(label: impl Into<SharedString>, action: Box<dyn Action>) -> Self {
        Self {
            label: label.into(),
            action,
        }
    }
}

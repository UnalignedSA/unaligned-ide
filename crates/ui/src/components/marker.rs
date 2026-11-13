use gpui::Hsla;

use crate::prelude::*;

const MARKER_THICKNESS: Pixels = px(2.);

/// The edge of its parent a [`selected_marker`] is drawn on.
#[derive(Debug, PartialEq, Eq, Hash, Clone, Copy, RegisterComponent)]
pub enum MarkerSide {
    /// Selected rows of navigation lists, activity-bar icons of the left dock.
    Left,
    /// Activity-bar icons of the right dock.
    Right,
    /// The active editor tab.
    Top,
    /// The active text tab, activity-bar icons of the bottom dock.
    Bottom,
}

/// The 2px accent bar that marks the active or selected thing.
///
/// It is absolutely positioned along one edge of its parent, which must be
/// `relative()`, so adding or removing it never moves the parent's content.
/// The returned [`Div`] can be inset further (for example `.left_2().right_2()`
/// to keep a bottom marker under a label).
///
/// # Usage Example
///
/// ```
/// use ui::{MarkerSide, prelude::*, selected_marker};
///
/// fn row(selected: bool, cx: &App) -> Div {
///     h_flex()
///         .relative()
///         .child("src")
///         .when(selected, |this| this.child(selected_marker(MarkerSide::Left, cx)))
/// }
/// ```
pub fn selected_marker(side: MarkerSide, cx: &App) -> Div {
    marker(side, cx.theme().colors().text_accent)
}

/// The same bar as [`selected_marker`] in a neutral colour, for the selected
/// thing of a pane or panel that does not have the focus.
pub fn selected_marker_inactive(side: MarkerSide, cx: &App) -> Div {
    marker(side, cx.theme().colors().border_strong)
}

fn marker(side: MarkerSide, color: Hsla) -> Div {
    let bar = div().absolute().bg(color);
    match side {
        MarkerSide::Left => bar.left_0().top_0().bottom_0().w(MARKER_THICKNESS),
        MarkerSide::Right => bar.right_0().top_0().bottom_0().w(MARKER_THICKNESS),
        MarkerSide::Top => bar.top_0().left_0().right_0().h(MARKER_THICKNESS),
        MarkerSide::Bottom => bar.bottom_0().left_0().right_0().h(MARKER_THICKNESS),
    }
}

impl Component for MarkerSide {
    fn scope() -> ComponentScope {
        ComponentScope::Navigation
    }

    fn name() -> &'static str {
        "SelectedMarker"
    }

    fn description() -> &'static str {
        "The selected marker: a 2px accent bar on one edge of the active or \
        selected thing (`selected_marker`), and its neutral twin for unfocused \
        panes (`selected_marker_inactive`)."
    }

    fn preview(_window: &mut Window, cx: &mut App) -> AnyElement {
        let sample = |label: &'static str, marker: Div| {
            h_flex()
                .relative()
                .h_8()
                .w_24()
                .px_3()
                .bg(cx.theme().colors().element_selected)
                .child(Label::new(label).size(LabelSize::Small))
                .child(marker)
        };
        let sides = [
            ("Left", MarkerSide::Left),
            ("Right", MarkerSide::Right),
            ("Top", MarkerSide::Top),
            ("Bottom", MarkerSide::Bottom),
        ];

        v_flex()
            .gap_6()
            .child(
                example_group(vec![
                    single_example(
                        "Focused: selected_marker",
                        h_flex()
                            .gap_4()
                            .children(
                                sides.map(|(label, side)| sample(label, selected_marker(side, cx))),
                            )
                            .into_any_element(),
                    ),
                    single_example(
                        "Unfocused: selected_marker_inactive",
                        h_flex()
                            .gap_4()
                            .children(sides.map(|(label, side)| {
                                sample(label, selected_marker_inactive(side, cx))
                            }))
                            .into_any_element(),
                    ),
                ])
                .vertical(),
            )
            .into_any_element()
    }
}

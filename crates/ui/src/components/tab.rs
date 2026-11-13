use std::cmp::Ordering;

use gpui::{AnyElement, IntoElement, Stateful};
use smallvec::SmallVec;

use crate::prelude::*;

const START_TAB_SLOT_SIZE: Pixels = px(12.);
// Wide enough for the close button, so the dirty dot and the button share one slot.
const END_TAB_SLOT_SIZE: Pixels = px(18.);
const ACTIVE_MARKER_HEIGHT: Pixels = px(2.);

/// The position of a [`Tab`] within a list of tabs.
#[derive(Debug, PartialEq, Eq, Clone, Copy)]
pub enum TabPosition {
    /// The tab is first in the list.
    First,

    /// The tab is in the middle of the list (i.e., it is not the first or last tab).
    ///
    /// The [`Ordering`] is where this tab is positioned with respect to the selected tab.
    Middle(Ordering),

    /// The tab is last in the list.
    Last,
}

#[derive(Debug, PartialEq, Eq, Clone, Copy)]
pub enum TabCloseSide {
    Start,
    End,
}

#[derive(IntoElement, RegisterComponent)]
pub struct Tab {
    div: Stateful<Div>,
    selected: bool,
    position: TabPosition,
    close_side: TabCloseSide,
    focused: bool,
    compact: bool,
    start_slot: Option<AnyElement>,
    end_slot: Option<AnyElement>,
    children: SmallVec<[AnyElement; 2]>,
}

impl Tab {
    pub fn new(id: impl Into<ElementId>) -> Self {
        let id = id.into();
        Self {
            div: div()
                .id(id.clone())
                .debug_selector(|| format!("TAB-{}", id)),
            selected: false,
            position: TabPosition::First,
            close_side: TabCloseSide::End,
            focused: true,
            compact: false,
            start_slot: None,
            end_slot: None,
            children: SmallVec::new(),
        }
    }

    pub fn position(mut self, position: TabPosition) -> Self {
        self.position = position;
        self
    }

    pub fn close_side(mut self, close_side: TabCloseSide) -> Self {
        self.close_side = close_side;
        self
    }

    /// Whether the tab's container has focus. The selected tab of an unfocused
    /// container keeps its marker, but in a neutral colour.
    pub fn focused(mut self, focused: bool) -> Self {
        self.focused = focused;
        self
    }

    /// Compact tabs (pinned tabs) use tighter padding.
    pub fn compact(mut self, compact: bool) -> Self {
        self.compact = compact;
        self
    }

    pub fn start_slot<E: IntoElement>(mut self, element: impl Into<Option<E>>) -> Self {
        self.start_slot = element.into().map(IntoElement::into_any_element);
        self
    }

    pub fn end_slot<E: IntoElement>(mut self, element: impl Into<Option<E>>) -> Self {
        self.end_slot = element.into().map(IntoElement::into_any_element);
        self
    }

    pub fn content_height(cx: &App) -> Pixels {
        Self::container_height(cx) - px(1.)
    }

    /// 35px at the default density, like VS Code's editor tabs.
    ///
    /// This is [`metrics::panel_header_height`](crate::metrics::panel_header_height)
    /// in pixels: a tab bar and the panel header next to it are the same height.
    pub fn container_height(cx: &App) -> Pixels {
        let rem_size = theme::theme_settings(cx).ui_font_size(cx);
        crate::metrics::panel_header_height(cx).to_pixels(rem_size)
    }
}

impl InteractiveElement for Tab {
    fn interactivity(&mut self) -> &mut gpui::Interactivity {
        self.div.interactivity()
    }
}

impl StatefulInteractiveElement for Tab {}

impl Toggleable for Tab {
    fn toggle_state(mut self, selected: bool) -> Self {
        self.selected = selected;
        self
    }
}

impl ParentElement for Tab {
    fn extend(&mut self, elements: impl IntoIterator<Item = AnyElement>) {
        self.children.extend(elements)
    }
}

impl RenderOnce for Tab {
    #[allow(refining_impl_trait)]
    fn render(self, _: &mut Window, cx: &mut App) -> Stateful<Div> {
        let colors = cx.theme().colors();
        let (text_color, tab_bg) = if self.selected {
            (colors.text, colors.tab_active_background)
        } else {
            (colors.text_muted, colors.tab_inactive_background)
        };
        let hover_text_color = colors.text;
        let hover_bg = colors.ghost_element_hover;
        // The accent is reserved for the active tab of the focused container.
        let marker_color = if self.focused {
            colors.text_accent
        } else {
            colors.border_focused
        };

        // The slot on the close side is always reserved so tabs keep their width
        // when the dirty dot or the close button appears; the other one only
        // takes space when it is used.
        let close_slot = h_flex()
            .flex_none()
            .size(END_TAB_SLOT_SIZE)
            .justify_center()
            .children(self.end_slot);
        let other_slot = self.start_slot.map(|slot| {
            h_flex()
                .flex_none()
                .size(START_TAB_SLOT_SIZE)
                .justify_center()
                .child(slot)
        });

        let (start_slot, end_slot) = match self.close_side {
            TabCloseSide::End => (other_slot, Some(close_slot)),
            TabCloseSide::Start => (Some(close_slot), other_slot),
        };

        let (edge_padding, close_side_padding) = if self.compact {
            (px(6.), px(2.))
        } else {
            (px(10.), px(4.))
        };
        let (padding_left, padding_right) = match self.close_side {
            TabCloseSide::End => (edge_padding, close_side_padding),
            TabCloseSide::Start => (close_side_padding, edge_padding),
        };

        self.div
            .relative()
            .flex_none()
            .h(Tab::container_height(cx))
            .bg(tab_bg)
            .border_color(colors.border)
            // Flat tabs: one separator on the right of each tab. Inactive tabs
            // carry the bar's bottom line, the active one merges with the editor.
            .border_r_1()
            .map(|this| {
                if self.selected {
                    this.pb_px()
                } else {
                    this.border_b_1().hover(|style| style.bg(hover_bg))
                }
            })
            .cursor_pointer()
            .child(
                h_flex()
                    .group("")
                    .relative()
                    .h(Tab::content_height(cx))
                    .pl(padding_left)
                    .pr(padding_right)
                    .gap(px(6.))
                    .text_color(text_color)
                    .when(!self.selected, |this| {
                        this.hover(|style| style.text_color(hover_text_color))
                    })
                    .children(start_slot)
                    .children(self.children)
                    .children(end_slot),
            )
            .when(self.selected, |this| {
                this.child(
                    div()
                        .absolute()
                        .top_0()
                        .left_0()
                        .right_0()
                        .h(ACTIVE_MARKER_HEIGHT)
                        .bg(marker_color),
                )
            })
    }
}

impl Component for Tab {
    fn scope() -> ComponentScope {
        ComponentScope::Navigation
    }

    fn description() -> &'static str {
        "A tab component that can be used in a tabbed interface, \
        supporting different positions and states."
    }

    fn preview(_window: &mut Window, _cx: &mut App) -> AnyElement {
        v_flex()
            .gap_6()
            .children(vec![example_group_with_title(
                "Variations",
                vec![
                    single_example(
                        "Default",
                        Tab::new("default").child("Default Tab").into_any_element(),
                    ),
                    single_example(
                        "Selected",
                        Tab::new("selected")
                            .toggle_state(true)
                            .child("Selected Tab")
                            .into_any_element(),
                    ),
                    single_example(
                        "Selected, unfocused",
                        Tab::new("selected_unfocused")
                            .toggle_state(true)
                            .focused(false)
                            .child("Selected Tab")
                            .into_any_element(),
                    ),
                    single_example(
                        "Compact",
                        Tab::new("compact")
                            .compact(true)
                            .child("Pinned Tab")
                            .into_any_element(),
                    ),
                    single_example(
                        "First",
                        Tab::new("first")
                            .position(TabPosition::First)
                            .child("First Tab")
                            .into_any_element(),
                    ),
                    single_example(
                        "Middle",
                        Tab::new("middle")
                            .position(TabPosition::Middle(Ordering::Equal))
                            .child("Middle Tab")
                            .into_any_element(),
                    ),
                    single_example(
                        "Last",
                        Tab::new("last")
                            .position(TabPosition::Last)
                            .child("Last Tab")
                            .into_any_element(),
                    ),
                ],
            )])
            .into_any_element()
    }
}

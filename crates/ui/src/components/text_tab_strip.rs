use gpui::{AnyElement, ClickEvent, Role, Stateful, StyleRefinement};
use smallvec::SmallVec;

use crate::{
    CountBadge, CountBadgeSize, IconButton, MarkerSide, focus_ring, metrics, micro_label_size,
    prelude::*, selected_marker,
};

/// The padding on each side of a tab's label. The selected marker is inset by
/// the same amount, so it sits under the label, not under the padding.
const TAB_PADDING: Pixels = px(10.);
/// With [`TAB_PADDING`] this puts the first label on the 12px inset.
const STRIP_START_PADDING: Pixels = px(2.);
const MAX_COUNT: usize = 999;

/// The two sizes of a [`TextTabStrip`].
#[derive(Debug, Default, PartialEq, Eq, Hash, Clone, Copy)]
pub enum TextTabSize {
    /// A strip as tall as a panel header (35px) with micro-label tabs: mono,
    /// uppercase, 11px. For the header of a panel.
    #[default]
    Default,
    /// A 30px strip with 12px UI-font tabs, shown as written. For tabs inside
    /// a page or a dialog.
    Compact,
}

impl TextTabSize {
    /// The height of a strip of this size, its bottom line included.
    pub fn strip_height(self, cx: &App) -> Rems {
        match self {
            TextTabSize::Default => metrics::panel_header_height(cx),
            TextTabSize::Compact => rems_from_px(30_f32),
        }
    }
}

/// One tab of a [`TextTabStrip`]: a text label, an optional count, and a 2px
/// marker under the label when it is selected.
///
/// The marker is an overlay, so selecting a tab never moves its label.
/// A `TextTab` is a stateful element with the id it was created with: drag
/// handlers, tooltips and other listeners can be attached to it directly, and
/// it can be wrapped (for example as the trigger of a right-click menu) and
/// added to the strip with `.child(..)`.
///
/// # Usage Example
///
/// ```
/// use ui::{TextTab, prelude::*};
///
/// let tab = TextTab::new("terminal-tab", "Terminal")
///     .selected(true)
///     .count(Some(2))
///     .on_click(|_, _window, _cx| {});
/// ```
#[derive(IntoElement)]
pub struct TextTab {
    div: Stateful<Div>,
    id: ElementId,
    label: SharedString,
    selected: bool,
    count: Option<usize>,
    size: TextTabSize,
}

impl TextTab {
    /// Creates a tab. `label` is written in sentence case; the default size
    /// shows it in uppercase.
    pub fn new(id: impl Into<ElementId>, label: impl Into<SharedString>) -> Self {
        let id = id.into();
        Self {
            div: div().id(id.clone()),
            id,
            label: label.into(),
            selected: false,
            count: None,
            size: TextTabSize::Default,
        }
    }

    /// The element id of this tab.
    ///
    /// Call it as `TextTab::id(&tab)`: with `InteractiveElement` in scope
    /// (it is part of the prelude), `tab.id()` resolves to
    /// `InteractiveElement::id`, which takes an id instead of returning one.
    pub fn id(&self) -> &ElementId {
        &self.id
    }

    /// Marks the tab as the selected one of its strip.
    pub fn selected(mut self, selected: bool) -> Self {
        self.selected = selected;
        self
    }

    /// A count shown after the label (changed files, problems), or `None`.
    pub fn count(mut self, count: impl Into<Option<usize>>) -> Self {
        self.count = count.into();
        self
    }

    /// Sets the size. [`TextTabStrip::tab`] does this for the tabs it is
    /// given; only a tab added to a strip through `.child(..)` needs it.
    pub fn size(mut self, size: TextTabSize) -> Self {
        self.size = size;
        self
    }

    /// Called when the tab is clicked, or activated with Enter or Space while
    /// it has the keyboard focus.
    pub fn on_click(
        mut self,
        handler: impl Fn(&ClickEvent, &mut Window, &mut App) + 'static,
    ) -> Self {
        self.div = self.div.on_click(handler);
        self
    }
}

impl InteractiveElement for TextTab {
    fn interactivity(&mut self) -> &mut gpui::Interactivity {
        self.div.interactivity()
    }
}

impl StatefulInteractiveElement for TextTab {}

impl RenderOnce for TextTab {
    #[allow(refining_impl_trait)]
    fn render(self, _window: &mut Window, cx: &mut App) -> Stateful<Div> {
        let colors = cx.theme().colors();
        let text_color = colors.text;
        let focus_ring = focus_ring(cx);
        let buffer_font = theme::theme_settings(cx).buffer_font(cx).clone();

        let label = match self.size {
            TextTabSize::Default => SharedString::from(self.label.to_uppercase()),
            TextTabSize::Compact => self.label.clone(),
        };
        self.div
            .role(Role::Tab)
            .aria_label(self.label)
            .aria_selected(self.selected)
            .tab_index(0)
            .relative()
            .flex()
            .flex_none()
            .items_center()
            .h_full()
            .px(TAB_PADDING)
            .gap_1p5()
            .cursor_pointer()
            .whitespace_nowrap()
            .map(|this| match self.size {
                TextTabSize::Default => this.font(buffer_font).text_size(micro_label_size()),
                TextTabSize::Compact => this.text_ui_sm(cx),
            })
            .text_color(if self.selected {
                text_color
            } else {
                colors.text_muted
            })
            .when(!self.selected, |this| {
                this.hover(|style| style.text_color(text_color))
            })
            .focus_visible(move |mut style: StyleRefinement| {
                style.box_shadow = Some(focus_ring);
                style
            })
            .child(label)
            .when_some(self.count, |this, count| {
                // Neutral like every count; "999+" beyond 999, where the badge
                // alone would stop at 99.
                this.child(
                    CountBadge::new(count)
                        .size(CountBadgeSize::Default)
                        .inline()
                        .max(MAX_COUNT),
                )
            })
            .when(self.selected, |this| {
                this.child(
                    selected_marker(MarkerSide::Bottom, cx)
                        .left(TAB_PADDING)
                        .right(TAB_PADDING),
                )
            })
    }
}

enum TextTabStripItem {
    Tab(TextTab),
    Element(AnyElement),
}

/// A row of [`TextTab`]s with a line under it and an optional group of
/// buttons on the right: the header of the bottom panel, the tabs of the
/// source-control panel, the sections of a settings page.
///
/// Tabs are added with [`tab`](Self::tab). An element that wraps a tab (a
/// right-click menu, a drop target) is added in the same order with
/// `.child(..)`. The strip scrolls horizontally when the tabs do not fit.
///
/// # Usage Example
///
/// ```
/// use ui::{TextTab, TextTabSize, TextTabStrip, prelude::*};
///
/// let strip = TextTabStrip::new("bottom-panel-tabs")
///     .tab(TextTab::new("terminal", "Terminal").selected(true))
///     .tab(TextTab::new("problems", "Problems").count(Some(3)))
///     .end_slot(IconButton::new("hide", IconName::Close).icon_size(IconSize::Small));
///
/// let in_a_page = TextTabStrip::new("settings-scope")
///     .size(TextTabSize::Compact)
///     .tab(TextTab::new("user", "User").selected(true))
///     .tab(TextTab::new("project", "Project"));
/// ```
#[derive(IntoElement, RegisterComponent)]
pub struct TextTabStrip {
    id: ElementId,
    items: SmallVec<[TextTabStripItem; 6]>,
    end_slots: SmallVec<[AnyElement; 4]>,
    size: TextTabSize,
}

impl TextTabStrip {
    /// Creates an empty strip.
    pub fn new(id: impl Into<ElementId>) -> Self {
        Self {
            id: id.into(),
            items: SmallVec::new(),
            end_slots: SmallVec::new(),
            size: TextTabSize::Default,
        }
    }

    /// Adds a tab. It takes the size of the strip.
    pub fn tab(mut self, tab: TextTab) -> Self {
        self.items.push(TextTabStripItem::Tab(tab));
        self
    }

    /// Adds an element to the right-aligned group, usually an [`IconButton`].
    /// Can be called several times: elements are laid out in call order with
    /// a 4px gap.
    pub fn end_slot(mut self, element: impl IntoElement) -> Self {
        self.end_slots.push(element.into_any_element());
        self
    }

    /// Sets the size of the strip and of its tabs.
    pub fn size(mut self, size: TextTabSize) -> Self {
        self.size = size;
        self
    }
}

impl ParentElement for TextTabStrip {
    fn extend(&mut self, elements: impl IntoIterator<Item = AnyElement>) {
        self.items
            .extend(elements.into_iter().map(TextTabStripItem::Element));
    }
}

impl RenderOnce for TextTabStrip {
    fn render(self, _window: &mut Window, cx: &mut App) -> impl IntoElement {
        let size = self.size;
        let tabs = self.items.into_iter().map(|item| match item {
            TextTabStripItem::Tab(tab) => tab.size(size).into_any_element(),
            TextTabStripItem::Element(element) => element,
        });

        h_flex()
            .id(self.id)
            .flex_none()
            .w_full()
            .h(size.strip_height(cx))
            .pl(STRIP_START_PADDING)
            .gap_2()
            .justify_between()
            .border_b_1()
            .border_color(cx.theme().colors().border)
            .child(
                h_flex()
                    .id("tabs")
                    .role(Role::TabList)
                    .h_full()
                    .min_w_0()
                    .overflow_x_scroll()
                    .children(tabs),
            )
            .when(!self.end_slots.is_empty(), |this| {
                this.child(
                    h_flex()
                        .flex_none()
                        // 8px, so a 16px icon in a 24px button ends on the 12px inset.
                        .pr_2()
                        .gap_1()
                        .children(self.end_slots),
                )
            })
    }
}

impl Component for TextTabStrip {
    fn scope() -> ComponentScope {
        ComponentScope::Navigation
    }

    fn description() -> &'static str {
        "The text tab strip: a row of text tabs with a 2px marker under the \
        selected one, in a panel-header size (mono, uppercase) and a compact \
        size (UI font) for pages and dialogs."
    }

    fn preview(_window: &mut Window, cx: &mut App) -> AnyElement {
        let action = |id: &'static str, icon: IconName| {
            IconButton::new(id, icon)
                .icon_size(IconSize::Small)
                .icon_color(Color::Muted)
        };
        let frame = |strip: TextTabStrip| {
            v_flex()
                .w(px(420.))
                .border_1()
                .border_color(cx.theme().colors().border)
                .bg(cx.theme().colors().background)
                .child(strip)
                .child(div().h_8())
                .into_any_element()
        };

        v_flex()
            .gap_6()
            .child(
                example_group(vec![
                    single_example(
                        "Default",
                        frame(
                            TextTabStrip::new("preview-strip-default")
                                .tab(TextTab::new("terminal", "Terminal").selected(true))
                                .tab(TextTab::new("problems", "Problems").count(Some(3)))
                                .tab(TextTab::new("debug", "Debug"))
                                .end_slot(action("preview-strip-maximize", IconName::Maximize))
                                .end_slot(action("preview-strip-hide", IconName::Close)),
                        ),
                    ),
                    single_example(
                        "Default, selected tab with a count",
                        frame(
                            TextTabStrip::new("preview-strip-count")
                                .tab(
                                    TextTab::new("changes", "Changes")
                                        .selected(true)
                                        .count(Some(12)),
                                )
                                .tab(TextTab::new("history", "History")),
                        ),
                    ),
                    single_example(
                        "Compact",
                        frame(
                            TextTabStrip::new("preview-strip-compact")
                                .size(TextTabSize::Compact)
                                .tab(TextTab::new("user", "User").selected(true))
                                .tab(TextTab::new("project", "Project"))
                                .tab(TextTab::new("installed", "Installed").count(Some(8))),
                        ),
                    ),
                ])
                .vertical(),
            )
            .into_any_element()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn tab_keeps_the_id_it_was_created_with() {
        let tab = TextTab::new(("panel-tab", 3usize), "Terminal").selected(true);
        assert_eq!(TextTab::id(&tab), &ElementId::from(("panel-tab", 3usize)));
    }

    #[test]
    fn strip_keeps_tabs_and_wrapped_elements_in_order() {
        let strip = TextTabStrip::new("strip")
            .tab(TextTab::new("first", "First"))
            .child(div())
            .tab(TextTab::new("third", "Third"));

        let kinds = strip
            .items
            .iter()
            .map(|item| match item {
                TextTabStripItem::Tab(tab) => Some(tab.label.as_ref()),
                TextTabStripItem::Element(_) => None,
            })
            .collect::<Vec<_>>();
        assert_eq!(kinds, [Some("First"), None, Some("Third")]);
    }
}

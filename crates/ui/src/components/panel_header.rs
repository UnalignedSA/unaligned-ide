use gpui::{AnyElement, Stateful};
use smallvec::SmallVec;

use crate::{IconButton, SectionLabel, metrics, prelude::*};

/// The header row of a panel: a title on the left, icon buttons on the right,
/// one line under it.
///
/// It is as tall as the editor tab bar, so the headers of the side panels and
/// the tab bar share their bottom edge across the window. The title sits on
/// the 12px inset that list rows and empty states use.
///
/// `PanelHeader` is a stateful element: mouse and drag handlers
/// (`on_drag`, `drag_over`, `on_drop`, `on_click`, …) can be attached to it
/// directly. Give it an explicit [`id`](Self::id) when you do.
///
/// # Usage Example
///
/// ```
/// use ui::{PanelHeader, prelude::*};
///
/// let header = PanelHeader::new("Explorer")
///     .id("project-panel-header")
///     .end_slot(IconButton::new("new-file", IconName::Plus).icon_size(IconSize::Small))
///     .end_slot(IconButton::new("collapse-all", IconName::ListCollapse).icon_size(IconSize::Small));
/// ```
#[derive(IntoElement, RegisterComponent)]
pub struct PanelHeader {
    div: Stateful<Div>,
    title: SharedString,
    title_slot: Option<AnyElement>,
    start_slot: Option<AnyElement>,
    end_slots: SmallVec<[AnyElement; 4]>,
}

impl PanelHeader {
    /// Creates a header whose title is rendered as a [`SectionLabel`].
    pub fn new(title: impl Into<SharedString>) -> Self {
        let title = title.into();
        Self {
            div: div().id(ElementId::Name(title.clone())),
            title,
            title_slot: None,
            start_slot: None,
            end_slots: SmallVec::new(),
        }
    }

    /// Sets the element id of the header, which is otherwise derived from the
    /// title. A stable id is what drag handlers and tests should rely on.
    pub fn id(mut self, id: impl Into<ElementId>) -> Self {
        self.div.interactivity().element_id = Some(id.into());
        self
    }

    /// Replaces the title label, for panels with a richer title (a branch
    /// chip, a breadcrumb, a title that can be edited).
    pub fn title_slot(mut self, title: impl IntoElement) -> Self {
        self.title_slot = Some(title.into_any_element());
        self
    }

    /// An element shown before the title, for example a back button.
    pub fn start_slot(mut self, element: impl IntoElement) -> Self {
        self.start_slot = Some(element.into_any_element());
        self
    }

    /// Adds an element to the right-aligned group, usually an [`IconButton`].
    /// Can be called several times: elements are laid out in call order with
    /// a 4px gap.
    pub fn end_slot(mut self, element: impl IntoElement) -> Self {
        self.end_slots.push(element.into_any_element());
        self
    }
}

impl InteractiveElement for PanelHeader {
    fn interactivity(&mut self) -> &mut gpui::Interactivity {
        self.div.interactivity()
    }
}

impl StatefulInteractiveElement for PanelHeader {}

impl RenderOnce for PanelHeader {
    #[allow(refining_impl_trait)]
    fn render(self, _window: &mut Window, cx: &mut App) -> Stateful<Div> {
        let colors = cx.theme().colors();

        self.div
            .h_flex()
            .flex_none()
            .w_full()
            .h(metrics::panel_header_height(cx))
            .pl_3()
            // 8px, so a 16px icon in a 24px button ends on the same 12px inset.
            .pr_2()
            .gap_2()
            .justify_between()
            .bg(colors.background)
            .border_b_1()
            .border_color(colors.border)
            .child(
                h_flex()
                    .min_w_0()
                    .gap_1p5()
                    .children(self.start_slot)
                    .child(
                        self.title_slot
                            .unwrap_or_else(|| SectionLabel::new(self.title).into_any_element()),
                    ),
            )
            .when(!self.end_slots.is_empty(), |this| {
                this.child(h_flex().flex_none().gap_1().children(self.end_slots))
            })
    }
}

impl Component for PanelHeader {
    fn scope() -> ComponentScope {
        ComponentScope::Layout
    }

    fn description() -> &'static str {
        "The panel header: a micro-label title on the left, icon buttons on \
        the right and a 1px line under it, as tall as the editor tab bar."
    }

    fn preview(_window: &mut Window, cx: &mut App) -> AnyElement {
        let action = |id: &'static str, icon: IconName| {
            IconButton::new(id, icon)
                .icon_size(IconSize::Small)
                .icon_color(Color::Muted)
        };
        let panel = |header: PanelHeader| {
            v_flex()
                .w(px(280.))
                .border_1()
                .border_color(cx.theme().colors().border)
                .bg(cx.theme().colors().background)
                .child(header)
                .child(
                    div().px_3().py_2().child(
                        Label::new("Panel content")
                            .size(LabelSize::Small)
                            .color(Color::Muted),
                    ),
                )
        };

        v_flex()
            .gap_6()
            .child(
                example_group(vec![
                    single_example(
                        "Title only, and with actions",
                        h_flex()
                            .gap_4()
                            .child(panel(PanelHeader::new("Outline")))
                            .child(panel(
                                PanelHeader::new("Explorer")
                                    .id("preview-header-actions")
                                    .end_slot(action("preview-new-file", IconName::Plus))
                                    .end_slot(action("preview-new-folder", IconName::FolderOpen))
                                    .end_slot(action("preview-more", IconName::Ellipsis)),
                            ))
                            .into_any_element(),
                    ),
                    single_example(
                        "Start slot, and title slot",
                        h_flex()
                            .gap_4()
                            .child(panel(
                                PanelHeader::new("History")
                                    .id("preview-header-start")
                                    .start_slot(action("preview-back", IconName::ArrowLeft))
                                    .end_slot(action("preview-close", IconName::Close)),
                            ))
                            .child(panel(
                                PanelHeader::new("Source control")
                                    .id("preview-header-title-slot")
                                    .title_slot(
                                        h_flex()
                                            .gap_1p5()
                                            .child(SectionLabel::new("Source control"))
                                            .child(
                                                Label::new("main")
                                                    .size(LabelSize::Small)
                                                    .color(Color::Muted),
                                            ),
                                    )
                                    .end_slot(action("preview-refresh", IconName::RotateCw)),
                            ))
                            .into_any_element(),
                    ),
                ])
                .vertical(),
            )
            .into_any_element()
    }
}

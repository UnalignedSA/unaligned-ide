use gpui::{
    AnyElement, App, Context, EventEmitter, Font, FontWeight, Global, IntoElement, MouseButton,
    Render, Subscription, Window,
};
use ui::{KeyBinding, Tooltip, prelude::*};
use workspace::{
    ToolbarItemEvent, ToolbarItemLocation, ToolbarItemView,
    item::{HighlightedText, ItemEvent, ItemHandle},
};

type RenderBreadcrumbTextFn = fn(
    Vec<HighlightedText>,
    Option<Font>,
    Option<AnyElement>,
    &dyn ItemHandle,
    bool,
    &mut Window,
    &App,
) -> AnyElement;

pub struct RenderBreadcrumbText(pub RenderBreadcrumbTextFn);

impl Global for RenderBreadcrumbText {}

pub struct Breadcrumbs {
    pane_focused: bool,
    active_item: Option<Box<dyn ItemHandle>>,
    subscription: Option<Subscription>,
}

impl Default for Breadcrumbs {
    fn default() -> Self {
        Self::new()
    }
}

impl Breadcrumbs {
    pub fn new() -> Self {
        Self {
            pane_focused: false,
            active_item: Default::default(),
            subscription: Default::default(),
        }
    }
}

impl EventEmitter<ToolbarItemEvent> for Breadcrumbs {}

/// Longer trails are elided in the middle.
const MAX_SEGMENTS: usize = 12;

struct Segment {
    text: SharedString,
    /// Symbols use the item's breadcrumb font (the buffer font for editors).
    is_symbol: bool,
    /// The file name is the one segment that reflects the dirty state.
    is_file_name: bool,
}

impl Breadcrumbs {
    /// Splits the leading path segment into its components, so every folder is
    /// its own crumb like in VS Code.
    fn segments(segments: Vec<HighlightedText>, has_project_path: bool) -> Vec<Segment> {
        let mut result = Vec::new();
        for (index, segment) in segments.into_iter().enumerate() {
            let text = segment.text.replace('\n', " ");
            if index == 0 && has_project_path {
                let parts = text
                    .split(['/', std::path::MAIN_SEPARATOR])
                    .filter(|part| !part.is_empty())
                    .collect::<Vec<_>>();
                let last = parts.len().saturating_sub(1);
                for (part_ix, part) in parts.iter().enumerate() {
                    result.push(Segment {
                        text: SharedString::from(part.to_string()),
                        is_symbol: false,
                        is_file_name: part_ix == last,
                    });
                }
            } else {
                result.push(Segment {
                    text: text.into(),
                    is_symbol: index > 0,
                    is_file_name: false,
                });
            }
        }

        let prefix_end = MAX_SEGMENTS / 2;
        if result.len() > MAX_SEGMENTS {
            let suffix_start = result.len() - MAX_SEGMENTS / 2;
            result.splice(
                prefix_end..suffix_start,
                Some(Segment {
                    text: "…".into(),
                    is_symbol: false,
                    is_file_name: false,
                }),
            );
        }
        result
    }
}

impl Render for Breadcrumbs {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let element = h_flex()
            .id("breadcrumb-container")
            .flex_grow_1()
            .h_6()
            .overflow_x_scroll()
            .text_ui_sm(cx);

        let Some(active_item) = self.active_item.as_ref() else {
            return element.into_any_element();
        };

        let Some((segments, breadcrumb_font)) = active_item.breadcrumbs(cx) else {
            return element.into_any_element();
        };

        let prefix_element = active_item.breadcrumb_prefix(window, cx);
        let has_project_path = active_item.project_path(cx).is_some();
        let is_dirty = active_item.is_dirty(cx);
        let focus_handle = active_item.item_focus_handle(cx);

        // Built by name: this crate does not depend on the crates defining them.
        let toggle_outline = cx
            .build_action("outline::Toggle", None)
            .ok()
            .filter(|action| window.is_action_available_in(action.as_ref(), &focus_handle));
        let copy_path = cx
            .build_action("workspace::CopyPath", None)
            .ok()
            .filter(|_| has_project_path);

        let colors = cx.theme().colors();
        let muted = colors.text_muted;
        let text = colors.text;
        let hover_background = colors.ghost_element_hover;
        let clickable = toggle_outline.is_some();

        let crumbs = Self::segments(segments, has_project_path)
            .into_iter()
            .enumerate()
            .flat_map(|(index, segment)| {
                let separator = (index > 0).then(|| {
                    Icon::new(IconName::ChevronRight)
                        .size(IconSize::XSmall)
                        .color(Color::Placeholder)
                        .into_any_element()
                });
                let emphasized = segment.is_file_name && is_dirty;
                let crumb = h_flex()
                    .id(("breadcrumb", index))
                    .flex_none()
                    .h_5()
                    .px_1()
                    .text_color(if emphasized { text } else { muted })
                    .when(emphasized, |this| this.font_weight(FontWeight::BOLD))
                    .when(segment.is_symbol, |this| {
                        this.when_some(breadcrumb_font.clone(), |this, font| {
                            this.font_family(font.family)
                        })
                    })
                    .hover(|style| style.bg(hover_background).text_color(text))
                    .when(clickable, |this| this.cursor_pointer())
                    .child(segment.text)
                    .into_any_element();
                separator.into_iter().chain(Some(crumb))
            });

        let trail = h_flex()
            .id("breadcrumb-trail")
            .flex_none()
            .children(crumbs)
            .when_some(toggle_outline, |this, action| {
                let tooltip_action = action.boxed_clone();
                let tooltip_focus_handle = focus_handle.clone();
                let click_focus_handle = focus_handle.clone();
                this.tooltip(Tooltip::element(move |_window, cx| {
                    v_flex()
                        .gap_1()
                        .child(
                            h_flex()
                                .gap_1()
                                .justify_between()
                                .child(Label::new("Show Symbol Outline"))
                                .child(KeyBinding::for_action_in(
                                    tooltip_action.as_ref(),
                                    &tooltip_focus_handle,
                                    cx,
                                )),
                        )
                        .when(has_project_path, |this| {
                            this.child(
                                h_flex()
                                    .gap_1()
                                    .justify_between()
                                    .pt_1()
                                    .border_t_1()
                                    .border_color(cx.theme().colors().border_variant)
                                    .child(Label::new("Right-Click to Copy Path")),
                            )
                        })
                        .into_any_element()
                }))
                .on_click(move |_, window, cx| {
                    click_focus_handle.dispatch_action(action.as_ref(), window, cx);
                })
            })
            .when_some(copy_path, |this, action| {
                let focus_handle = focus_handle.clone();
                this.on_mouse_down(MouseButton::Right, move |_, window, cx| {
                    focus_handle.dispatch_action(action.as_ref(), window, cx);
                })
            });

        element
            .gap_1p5()
            .children(prefix_element)
            .child(trail)
            .into_any_element()
    }
}

impl ToolbarItemView for Breadcrumbs {
    fn set_active_pane_item(
        &mut self,
        active_pane_item: Option<&dyn ItemHandle>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> ToolbarItemLocation {
        cx.notify();
        self.active_item = None;

        let Some(item) = active_pane_item else {
            return ToolbarItemLocation::Hidden;
        };

        let this = cx.entity().downgrade();
        self.subscription = Some(item.subscribe_to_item_events(
            window,
            cx,
            Box::new(move |event, _, cx| {
                if let ItemEvent::UpdateBreadcrumbs = event {
                    this.update(cx, |this, cx| {
                        cx.notify();
                        if let Some(active_item) = this.active_item.as_ref() {
                            cx.emit(ToolbarItemEvent::ChangeLocation(
                                active_item.breadcrumb_location(cx),
                            ))
                        }
                    })
                    .ok();
                }
            }),
        ));
        self.active_item = Some(item.boxed_clone());
        item.breadcrumb_location(cx)
    }

    fn pane_focus_update(
        &mut self,
        pane_focused: bool,
        _window: &mut Window,
        _: &mut Context<Self>,
    ) {
        self.pane_focused = pane_focused;
    }
}

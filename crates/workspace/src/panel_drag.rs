//! Moving a panel to another dock by dragging its icon or its tab.
//!
//! A source (an activity-bar cell, a bottom tab) is wrapped with
//! [`panel_drag_source`]. While the drag lasts the workspace draws one zone per
//! dock that accepts the panel; releasing over a zone calls
//! [`Workspace::move_panel_to_dock`].

use std::sync::Arc;

use gpui::{
    AnyElement, Bounds, Context, EntityId, Pixels, Point, Render, Size, StatefulInteractiveElement,
    Subscription, WeakEntity, Window, point, px, size,
};
use settings::Settings as _;
use theme_settings::ThemeSettings;
use ui::{SectionLabel, prelude::*};

use crate::{
    BottomDockLayout, Event, MoveFocusedPanelToBottomDock, MoveFocusedPanelToLeftDock,
    MoveFocusedPanelToRightDock, Toast, Workspace, WorkspaceSettings,
    dock::{DockPosition, PanelHandle},
    notifications::NotificationId,
};

/// The payload of a panel drag: the panel, and the dock it is dragged from.
#[derive(Clone)]
pub struct DraggedPanel {
    pub panel: Arc<dyn PanelHandle>,
    pub from: DockPosition,
}

impl Render for DraggedPanel {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let icon = self.panel.icon(window, cx);
        let title = self.panel.title(window, cx);
        let ui_font = ThemeSettings::get_global(cx).ui_font.clone();
        let colors = cx.theme().colors();
        // A flat chip on the raised ground, outlined so it reads over any surface.
        h_flex()
            .gap_1p5()
            .px_2()
            .py_1()
            .bg(colors.elevated_surface_background)
            .border_1()
            .border_color(colors.border_strong)
            .font(ui_font)
            .children(icon.map(|icon| Icon::new(icon).size(IconSize::Small).color(Color::Muted)))
            .child(Label::new(title))
    }
}

/// Makes `element` a handle that drags `panel`, which lives in the dock at
/// `from`, to another dock.
///
/// The element keeps its own click, menu and tooltip: the drag only starts once
/// the pointer has moved with the button held. An element can be the source of
/// one drag only, so do not call `on_drag` on it again.
///
/// The handle also stands for its dock as a place to release a panel: a panel
/// dropped on it goes to the dock at `from`. For the handle's own panel that
/// means "show it where it is", so a click during which the pointer slipped by
/// a few pixels, which starts a drag, still opens the panel.
pub fn panel_drag_source<E>(
    element: E,
    panel: Arc<dyn PanelHandle>,
    from: DockPosition,
    workspace: WeakEntity<Workspace>,
) -> E
where
    E: StatefulInteractiveElement,
{
    element
        .on_drag(DraggedPanel { panel, from }, {
            let workspace = workspace.clone();
            move |dragged, _cursor_offset, window, cx| {
                workspace
                    .update(cx, |workspace, cx| {
                        workspace.begin_panel_drag(dragged.clone(), window, cx);
                    })
                    .ok();
                cx.new(|_| dragged.clone())
            }
        })
        // No `can_drop` here: it would also gate the element's other drops.
        // `move_panel_to_dock` refuses a dock the panel does not accept.
        .on_drop(move |dragged: &DraggedPanel, window, cx| {
            workspace
                .update(cx, |workspace, cx| {
                    workspace.move_panel_to_dock(&dragged.panel, from, window, cx);
                })
                .ok();
        })
}

/// The state the workspace keeps while a panel is being dragged.
pub(crate) struct ActivePanelDrag {
    dragged: DraggedPanel,
    /// Escape cancels the drag wherever the keyboard focus is, including in a
    /// terminal, which would otherwise receive the key.
    _cancel_on_escape: Subscription,
}

/// A side dock that is closed gets a zone this wide, as a share of the workspace.
const CLOSED_SIDE_ZONE_FRACTION: f32 = 0.22;
const CLOSED_SIDE_ZONE_MAX_WIDTH: Pixels = px(320.);
/// A closed bottom dock gets a zone this high, as a share of the workspace.
const CLOSED_BOTTOM_ZONE_FRACTION: f32 = 0.28;
/// The zone of an open dock follows the dock, within these bounds: it stays
/// easy to aim at and never covers the zones of the other docks.
const MIN_OPEN_ZONE_SIZE: Pixels = px(96.);
const MAX_SIDE_ZONE_FRACTION: f32 = 0.4;
const MAX_BOTTOM_ZONE_FRACTION: f32 = 0.6;

struct PanelMoveFailed;

/// Where the three drop zones are, relative to a workspace area of `workspace_size`.
///
/// The zones never overlap, so one release cannot land in two docks: the corner
/// shared by a side dock and the bottom dock goes to the one that owns it in
/// the current `bottom_dock_layout`.
fn drop_zone_bounds(
    layout: BottomDockLayout,
    workspace_size: Size<Pixels>,
    left_width: Pixels,
    right_width: Pixels,
    bottom_height: Pixels,
) -> [(DockPosition, Bounds<Pixels>); 3] {
    let zero = px(0.);
    let width = workspace_size.width.max(zero);
    let height = workspace_size.height.max(zero);
    let left_width = left_width.max(zero).min(width / 2.);
    let right_width = right_width.max(zero).min(width / 2.);
    let bottom_height = bottom_height.max(zero).min(height);
    let side_height_above_bottom = height - bottom_height;

    // Whether the bottom dock runs under the left and under the right dock.
    let (bottom_under_left, bottom_under_right) = match layout {
        BottomDockLayout::Full => (true, true),
        BottomDockLayout::LeftAligned => (true, false),
        BottomDockLayout::RightAligned => (false, true),
        BottomDockLayout::Contained => (false, false),
    };

    let left_height = if bottom_under_left {
        side_height_above_bottom
    } else {
        height
    };
    let right_height = if bottom_under_right {
        side_height_above_bottom
    } else {
        height
    };
    let bottom_left = if bottom_under_left { zero } else { left_width };
    let bottom_right = if bottom_under_right {
        width
    } else {
        width - right_width
    };

    [
        (
            DockPosition::Left,
            Bounds::new(point(zero, zero), size(left_width, left_height)),
        ),
        (
            DockPosition::Right,
            Bounds::new(
                point(width - right_width, zero),
                size(right_width, right_height),
            ),
        ),
        (
            DockPosition::Bottom,
            Bounds::new(
                point(bottom_left, side_height_above_bottom),
                size((bottom_right - bottom_left).max(zero), bottom_height),
            ),
        ),
    ]
}

/// Written in sentence case: `SectionLabel` shows it in capitals.
fn drop_zone_caption(position: DockPosition) -> &'static str {
    match position {
        DockPosition::Left => "Left",
        DockPosition::Right => "Right",
        DockPosition::Bottom => "Bottom",
    }
}

fn drop_zone_selector(position: DockPosition) -> &'static str {
    match position {
        DockPosition::Left => "PANEL-DROP-ZONE-LEFT",
        DockPosition::Right => "PANEL-DROP-ZONE-RIGHT",
        DockPosition::Bottom => "PANEL-DROP-ZONE-BOTTOM",
    }
}

impl Workspace {
    /// Whether a panel is being dragged to another dock right now.
    pub fn panel_drag_active(&self) -> bool {
        self.panel_drag.is_some()
    }

    /// The panel that is being dragged to another dock, if any. A place that
    /// offers itself as a target (an empty group of the activity bar) uses it
    /// to stay hidden when its dock does not accept that panel.
    pub fn dragged_panel(&self) -> Option<&Arc<dyn PanelHandle>> {
        self.panel_drag.as_ref().map(|drag| &drag.dragged.panel)
    }

    /// Called from the `on_drag` of a panel drag source, once per drag.
    pub(crate) fn begin_panel_drag(
        &mut self,
        dragged: DraggedPanel,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        // A maximized panel or pane would cover the docks the zones stand for.
        self.clear_zoom_for_panel_move(window, cx);
        for dock in self.all_docks() {
            dock.update(cx, |dock, cx| dock.set_panel_drag_active(cx));
        }
        let cancel_on_escape = cx.intercept_keystrokes(|event, window, cx| {
            if event.keystroke.key == "escape" && cx.stop_active_drag(window) {
                cx.stop_propagation();
            }
        });
        self.panel_drag = Some(ActivePanelDrag {
            dragged,
            _cancel_on_escape: cancel_on_escape,
        });
        cx.notify();
    }

    /// Forgets the panel drag once it is over, however it ended (drop, release
    /// outside of a zone, Escape). Called at the start of every render: a drag
    /// that ends always refreshes the window.
    pub(crate) fn end_panel_drag_if_over(&mut self, cx: &mut Context<Self>) {
        if self.panel_drag.is_some() && !cx.has_active_drag() {
            self.panel_drag = None;
        }
    }

    fn clear_zoom_for_panel_move(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        for dock in self.all_docks() {
            dock.update(cx, |dock, cx| {
                if dock
                    .active_panel()
                    .is_some_and(|panel| panel.is_zoomed(window, cx))
                {
                    dock.zoom_out(window, cx);
                    cx.notify();
                }
            });
        }
        for pane in &self.panes {
            pane.update(cx, |pane, cx| {
                if pane.is_zoomed() {
                    pane.set_zoomed(false, cx);
                }
            });
        }
        if self.zoomed.is_some() || self.zoomed_position.is_some() {
            self.zoomed = None;
            self.zoomed_position = None;
            cx.emit(Event::ZoomChanged);
            cx.notify();
        }
    }

    /// Consumes the "reveal on arrival" mark left by [`Self::move_panel_to_dock`]
    /// for a panel that is arriving in the dock at `position`.
    pub(crate) fn take_panel_reveal(&mut self, panel_id: EntityId, position: DockPosition) -> bool {
        let pending = &mut self.panels_to_reveal_on_arrival;
        let count = pending.len();
        pending.retain(|entry| *entry != (panel_id, position));
        pending.len() != count
    }

    /// Moves `panel` to the dock at `to`, where it arrives open and focused even
    /// if it was hidden before. Does nothing when the panel does not accept that
    /// dock. When the panel already lives there it is only opened and focused.
    ///
    /// The move itself is the panel's `set_position`, which for the built-in
    /// panels writes their `dock` setting: the panel changes dock when the
    /// settings come back, and a toast tells the user if they never do.
    pub fn move_panel_to_dock(
        &mut self,
        panel: &Arc<dyn PanelHandle>,
        to: DockPosition,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let panel_id = panel.panel_id();
        let Some(from) = self.all_docks().into_iter().find_map(|dock| {
            let dock = dock.read(cx);
            dock.panel_index_for_id(panel_id).map(|_| dock.position())
        }) else {
            return;
        };

        if from == to {
            self.reveal_panel_in_its_dock(panel, to, window, cx);
            return;
        }

        if !panel.position_is_valid(to, cx) {
            return;
        }

        // A maximized panel or pane would hide the dock the panel arrives in,
        // and a panel that stays maximized hides where it went.
        self.clear_zoom_for_panel_move(window, cx);

        if !self.panels_to_reveal_on_arrival.contains(&(panel_id, to)) {
            self.panels_to_reveal_on_arrival.push((panel_id, to));
        }
        panel.set_position(to, window, cx);

        // Settings writes are applied one after the other, so this empty one
        // completes after the panel's own: by then the panel has either changed
        // dock or it never will (settings.json does not parse, or the `dock`
        // key is overridden by a project or a profile).
        let settings_written = settings::update_settings_file_with_completion(
            self.app_state.fs.clone(),
            cx,
            |_, _| {},
        );
        let panel = panel.clone();
        cx.spawn_in(window, async move |workspace, cx| {
            settings_written.await.ok();
            workspace
                .update_in(cx, |workspace, window, cx| {
                    workspace.take_panel_reveal(panel_id, to);
                    if panel.position(window, cx) != to {
                        workspace.show_toast(
                            Toast::new(
                                NotificationId::unique::<PanelMoveFailed>(),
                                "Could not move the panel: check settings.json for errors.",
                            )
                            .autohide(),
                            cx,
                        );
                    }
                })
                .ok();
        })
        .detach();
    }

    fn reveal_panel_in_its_dock(
        &mut self,
        panel: &Arc<dyn PanelHandle>,
        position: DockPosition,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let panel_id = panel.panel_id();
        self.dismiss_zoomed_items_to_reveal(Some(position), window, cx);
        let dock = self.dock_at_position(position).clone();
        dock.update(cx, |dock, cx| {
            if let Some(index) = dock.panel_index_for_id(panel_id) {
                dock.activate_panel(index, window, cx);
                dock.set_open(true, window, cx);
            }
        });
        window.focus(&panel.activation_focus_handle(cx), cx);
        cx.notify();
        self.serialize_workspace(window, cx);
    }

    /// The panel shown in the dock that has the keyboard focus, if the focus
    /// is inside that panel.
    fn focused_panel(&self, window: &Window, cx: &App) -> Option<Arc<dyn PanelHandle>> {
        self.all_docks().into_iter().find_map(|dock| {
            let dock = dock.read(cx);
            dock.active_panel()
                .filter(|panel| panel.panel_focus_handle(cx).contains_focused(window, cx))
                .cloned()
        })
    }

    fn move_focused_panel_to_dock(
        &mut self,
        to: DockPosition,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if let Some(panel) = self.focused_panel(window, cx) {
            self.move_panel_to_dock(&panel, to, window, cx);
        }
    }

    pub(crate) fn move_focused_panel_to_left_dock(
        &mut self,
        _: &MoveFocusedPanelToLeftDock,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.move_focused_panel_to_dock(DockPosition::Left, window, cx);
    }

    pub(crate) fn move_focused_panel_to_right_dock(
        &mut self,
        _: &MoveFocusedPanelToRightDock,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.move_focused_panel_to_dock(DockPosition::Right, window, cx);
    }

    pub(crate) fn move_focused_panel_to_bottom_dock(
        &mut self,
        _: &MoveFocusedPanelToBottomDock,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.move_focused_panel_to_dock(DockPosition::Bottom, window, cx);
    }

    /// How deep the drop zone of the dock at `position` is: the dock's own size
    /// when it is open, a share of the workspace when it is closed.
    fn drop_zone_extent(&self, position: DockPosition, window: &Window, cx: &App) -> Pixels {
        let (available, closed_extent, max_fraction) = match position {
            DockPosition::Left | DockPosition::Right => {
                let width = self.bounds.size.width;
                (
                    width,
                    (width * CLOSED_SIDE_ZONE_FRACTION).min(CLOSED_SIDE_ZONE_MAX_WIDTH),
                    MAX_SIDE_ZONE_FRACTION,
                )
            }
            DockPosition::Bottom => {
                let height = self.bounds.size.height;
                (
                    height,
                    height * CLOSED_BOTTOM_ZONE_FRACTION,
                    MAX_BOTTOM_ZONE_FRACTION,
                )
            }
        };

        let dock = self.dock_at_position(position).read(cx);
        let open_extent = dock
            .visible_panel()
            .and_then(|_| self.dock_size(dock, window, cx));
        match open_extent {
            Some(extent) => extent.max(MIN_OPEN_ZONE_SIZE).min(available * max_fraction),
            None => closed_extent,
        }
    }

    /// The layer of drop zones drawn over the workspace while a panel is
    /// dragged: one outlined rectangle per dock that accepts the panel, filled
    /// while the pointer is over it. Docks that refuse the panel get nothing.
    pub(crate) fn render_panel_drop_zones(
        &self,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Option<AnyElement> {
        let panel = self.panel_drag.as_ref()?.dragged.panel.clone();
        let workspace_size = self.bounds.size;
        if workspace_size.width <= px(0.) || workspace_size.height <= px(0.) {
            return None;
        }

        let zones = drop_zone_bounds(
            WorkspaceSettings::get_global(cx).bottom_dock_layout,
            workspace_size,
            self.drop_zone_extent(DockPosition::Left, window, cx),
            self.drop_zone_extent(DockPosition::Right, window, cx),
            self.drop_zone_extent(DockPosition::Bottom, window, cx),
        );

        let colors = cx.theme().colors();
        let outline = colors.drop_target_border;
        let fill = colors.drop_target_background;
        let caption_background = colors.elevated_surface_background;
        let caption_border = colors.border_strong;

        let zones = zones
            .into_iter()
            .filter(|(position, bounds)| {
                panel.position_is_valid(*position, cx)
                    && bounds.size.width > px(0.)
                    && bounds.size.height > px(0.)
            })
            .map(|(position, bounds)| {
                let Point { x, y } = bounds.origin;
                div()
                    .debug_selector(|| drop_zone_selector(position).to_string())
                    .absolute()
                    .left(x)
                    .top(y)
                    .w(bounds.size.width)
                    .h(bounds.size.height)
                    .flex()
                    .items_center()
                    .justify_center()
                    .border_1()
                    .border_color(outline)
                    .can_drop(move |dragged, _window, cx| {
                        dragged
                            .downcast_ref::<DraggedPanel>()
                            .is_some_and(|dragged| dragged.panel.position_is_valid(position, cx))
                    })
                    .drag_over::<DraggedPanel>(move |style, _, _, _| style.bg(fill))
                    .on_drop(
                        cx.listener(move |workspace, dragged: &DraggedPanel, window, cx| {
                            workspace.move_panel_to_dock(&dragged.panel, position, window, cx);
                        }),
                    )
                    .child(
                        // The caption sits on its own raised chip: the zone is
                        // drawn over whatever the dock or the editor shows.
                        div()
                            .px_1p5()
                            .py_0p5()
                            .bg(caption_background)
                            .border_1()
                            .border_color(caption_border)
                            .child(SectionLabel::new(drop_zone_caption(position))),
                    )
            })
            .collect::<Vec<_>>();

        if zones.is_empty() {
            return None;
        }

        Some(
            div()
                .absolute()
                .inset_0()
                .children(zones)
                .into_any_element(),
        )
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{
        MultiWorkspace,
        dock::{
            PanelEvent, PanelSizeState, default_panel_title,
            test::{TestPanel, ToggleTestPanel},
        },
    };
    use fs::FakeFs;
    use gpui::{
        Entity, Focusable as _, Modifiers, MouseButton, MouseDownEvent, MouseMoveEvent,
        MouseUpEvent, TestAppContext, VisualTestContext,
    };
    use project::Project;
    use settings::SettingsStore;

    const ALL_POSITIONS: [DockPosition; 3] = [
        DockPosition::Left,
        DockPosition::Right,
        DockPosition::Bottom,
    ];

    fn init_test(cx: &mut TestAppContext) {
        cx.update(|cx| {
            let settings_store = SettingsStore::test(cx);
            cx.set_global(settings_store);
            cx.set_global(db::AppDatabase::test_new());
            theme_settings::init(theme::LoadThemes::JustBase, cx);
        });
    }

    async fn new_workspace(cx: &mut TestAppContext) -> (Entity<Workspace>, &mut VisualTestContext) {
        init_test(cx);
        let fs = FakeFs::new(cx.executor());
        let project = Project::test(fs, [], cx).await;
        // The window root is what attaches the workspace's actions.
        let (multi_workspace, cx) =
            cx.add_window_view(|window, cx| MultiWorkspace::test_new(project, window, cx));
        let workspace =
            multi_workspace.read_with(cx, |multi_workspace, _| multi_workspace.workspace().clone());
        cx.run_until_parked();
        (workspace, cx)
    }

    fn add_panel(
        workspace: &Entity<Workspace>,
        position: DockPosition,
        activation_priority: u32,
        configure: impl FnOnce(&mut TestPanel),
        cx: &mut VisualTestContext,
    ) -> Entity<TestPanel> {
        workspace.update_in(cx, |workspace, window, cx| {
            let panel = cx.new(|cx| {
                let mut panel = TestPanel::new(position, activation_priority, cx);
                configure(&mut panel);
                panel
            });
            workspace.add_panel(panel.clone(), window, cx);
            panel
        })
    }

    fn move_panel(
        workspace: &Entity<Workspace>,
        panel: &Entity<TestPanel>,
        to: DockPosition,
        cx: &mut VisualTestContext,
    ) {
        let handle: Arc<dyn PanelHandle> = Arc::new(panel.clone());
        workspace.update_in(cx, |workspace, window, cx| {
            workspace.move_panel_to_dock(&handle, to, window, cx);
        });
        cx.run_until_parked();
    }

    /// The dock that holds the panel, whether the panel is the one shown there.
    fn dock_of(
        workspace: &Entity<Workspace>,
        panel: &Entity<TestPanel>,
        cx: &mut VisualTestContext,
    ) -> Vec<DockPosition> {
        let panel_id = panel.entity_id();
        workspace.read_with(cx, |workspace, cx| {
            workspace
                .all_docks()
                .into_iter()
                .filter(|dock| dock.read(cx).panel_index_for_id(panel_id).is_some())
                .map(|dock| dock.read(cx).position())
                .collect()
        })
    }

    #[track_caller]
    fn assert_open_and_focused_in(
        workspace: &Entity<Workspace>,
        panel: &Entity<TestPanel>,
        position: DockPosition,
        cx: &mut VisualTestContext,
    ) {
        assert_eq!(
            dock_of(workspace, panel, cx),
            vec![position],
            "the panel should live in the {position:?} dock only"
        );
        let panel_id = panel.entity_id();
        let (settings_position, is_open, visible_panel, is_focused) =
            workspace.update_in(cx, |workspace, window, cx| {
                let dock = workspace.dock_at_position(position).read(cx);
                (
                    panel.read(cx).position,
                    dock.is_open(),
                    dock.visible_panel().map(|panel| panel.panel_id()),
                    panel.read(cx).focus_handle(cx).is_focused(window),
                )
            });
        assert_eq!(settings_position, position);
        assert!(is_open, "the {position:?} dock should be open");
        assert_eq!(
            visible_panel,
            Some(panel_id),
            "the panel should be the one shown in the {position:?} dock"
        );
        assert!(is_focused, "the panel should have the keyboard focus");
    }

    fn mouse_down(position: Point<Pixels>, cx: &mut VisualTestContext) {
        cx.simulate_event(MouseDownEvent {
            position,
            button: MouseButton::Left,
            modifiers: Modifiers::default(),
            click_count: 1,
            first_mouse: false,
        });
        cx.run_until_parked();
    }

    fn mouse_move(position: Point<Pixels>, cx: &mut VisualTestContext) {
        cx.simulate_event(MouseMoveEvent {
            position,
            pressed_button: Some(MouseButton::Left),
            modifiers: Modifiers::default(),
        });
        cx.run_until_parked();
    }

    fn mouse_up(position: Point<Pixels>, cx: &mut VisualTestContext) {
        cx.simulate_event(MouseUpEvent {
            position,
            button: MouseButton::Left,
            modifiers: Modifiers::default(),
            click_count: 1,
        });
        cx.run_until_parked();
    }

    /// Presses on the element and moves far enough for the drag to start.
    fn start_drag(selector: &'static str, cx: &mut VisualTestContext) {
        let source = cx
            .debug_bounds(selector)
            .unwrap_or_else(|| panic!("no element with the selector {selector}"));
        mouse_down(source.center(), cx);
        mouse_move(source.center() + point(px(6.), px(6.)), cx);
    }

    #[gpui::test]
    async fn test_move_panel_between_every_pair_of_docks(cx: &mut TestAppContext) {
        let (workspace, cx) = new_workspace(cx).await;
        // Hidden and unfocused at the start: its dock is closed.
        let panel = add_panel(&workspace, DockPosition::Left, 100, |_| {}, cx);

        // Left → Right → Bottom → Left → Bottom → Right → Left: the six moves.
        for to in [
            DockPosition::Right,
            DockPosition::Bottom,
            DockPosition::Left,
            DockPosition::Bottom,
            DockPosition::Right,
            DockPosition::Left,
        ] {
            move_panel(&workspace, &panel, to, cx);
            assert_open_and_focused_in(&workspace, &panel, to, cx);
        }

        workspace.read_with(cx, |workspace, _| {
            assert!(workspace.panels_to_reveal_on_arrival.is_empty());
            assert!(workspace.notification_ids().is_empty());
        });
    }

    #[gpui::test]
    async fn test_hidden_panel_arrives_open_and_active(cx: &mut TestAppContext) {
        let (workspace, cx) = new_workspace(cx).await;
        let shown = add_panel(&workspace, DockPosition::Left, 100, |_| {}, cx);
        let hidden = add_panel(&workspace, DockPosition::Left, 200, |_| {}, cx);
        let resident = add_panel(&workspace, DockPosition::Bottom, 300, |_| {}, cx);
        workspace.update_in(cx, |workspace, window, cx| {
            workspace.toggle_dock(DockPosition::Left, window, cx);
            workspace.toggle_dock(DockPosition::Bottom, window, cx);
        });
        cx.run_until_parked();
        workspace.read_with(cx, |workspace, cx| {
            let left_dock = workspace.left_dock().read(cx);
            assert_eq!(
                left_dock.visible_panel().map(|panel| panel.panel_id()),
                Some(shown.entity_id())
            );
        });

        // To a closed, empty dock.
        move_panel(&workspace, &hidden, DockPosition::Right, cx);
        assert_open_and_focused_in(&workspace, &hidden, DockPosition::Right, cx);
        workspace.read_with(cx, |workspace, cx| {
            let left_dock = workspace.left_dock().read(cx);
            assert!(
                left_dock.is_open(),
                "the dock it left keeps showing its panel"
            );
            assert_eq!(
                left_dock.visible_panel().map(|panel| panel.panel_id()),
                Some(shown.entity_id())
            );
        });

        // To an open dock that shows another panel.
        move_panel(&workspace, &hidden, DockPosition::Bottom, cx);
        assert_open_and_focused_in(&workspace, &hidden, DockPosition::Bottom, cx);
        assert_eq!(
            dock_of(&workspace, &resident, cx),
            vec![DockPosition::Bottom]
        );
    }

    /// The panel a dock shows: `None` when the dock is closed.
    fn shown_in(
        workspace: &Entity<Workspace>,
        position: DockPosition,
        cx: &mut VisualTestContext,
    ) -> Option<EntityId> {
        workspace.read_with(cx, |workspace, cx| {
            workspace
                .dock_at_position(position)
                .read(cx)
                .visible_panel()
                .map(|panel| panel.panel_id())
        })
    }

    fn show_in_left_dock(
        workspace: &Entity<Workspace>,
        panel: &Entity<TestPanel>,
        cx: &mut VisualTestContext,
    ) {
        let panel_id = panel.entity_id();
        workspace.update_in(cx, |workspace, window, cx| {
            workspace.left_dock().update(cx, |dock, cx| {
                let index = dock
                    .panel_index_for_id(panel_id)
                    .expect("the panel should be in the left dock");
                dock.activate_panel(index, window, cx);
                dock.set_open(true, window, cx);
            });
        });
        cx.run_until_parked();
    }

    #[gpui::test]
    async fn test_dock_shows_its_next_panel_when_the_shown_one_leaves(cx: &mut TestAppContext) {
        let (workspace, cx) = new_workspace(cx).await;
        let with_button = |panel: &mut TestPanel| panel.icon = Some(IconName::FileTree);
        let first = add_panel(&workspace, DockPosition::Left, 100, with_button, cx);
        let second = add_panel(&workspace, DockPosition::Left, 200, with_button, cx);
        let third = add_panel(&workspace, DockPosition::Left, 300, with_button, cx);
        // A panel whose button is hidden is never the one a dock falls back on.
        let without_button = add_panel(&workspace, DockPosition::Left, 400, |_| {}, cx);
        show_in_left_dock(&workspace, &second, cx);

        // The panel that followed the one that left takes its place.
        move_panel(&workspace, &second, DockPosition::Right, cx);
        assert_open_and_focused_in(&workspace, &second, DockPosition::Right, cx);
        assert_eq!(
            shown_in(&workspace, DockPosition::Left, cx),
            Some(third.entity_id())
        );
        assert!(third.read_with(cx, |panel, _| panel.active));

        // Nothing with a button follows the last one: the one before it.
        move_panel(&workspace, &third, DockPosition::Bottom, cx);
        assert_open_and_focused_in(&workspace, &third, DockPosition::Bottom, cx);
        assert_eq!(
            shown_in(&workspace, DockPosition::Left, cx),
            Some(first.entity_id())
        );

        // Nothing is left to show: only now does the dock close.
        move_panel(&workspace, &first, DockPosition::Right, cx);
        assert_open_and_focused_in(&workspace, &first, DockPosition::Right, cx);
        assert_eq!(shown_in(&workspace, DockPosition::Left, cx), None);
        assert_eq!(
            dock_of(&workspace, &without_button, cx),
            vec![DockPosition::Left]
        );
        workspace.read_with(cx, |workspace, cx| {
            assert!(!workspace.left_dock().read(cx).is_open());
        });

        // The right dock now holds two panels and shows the last arrival; the
        // one before it comes back when that one leaves.
        move_panel(&workspace, &first, DockPosition::Bottom, cx);
        assert_eq!(
            shown_in(&workspace, DockPosition::Right, cx),
            Some(second.entity_id())
        );
    }

    #[gpui::test]
    async fn test_dock_is_left_alone_when_another_panel_leaves(cx: &mut TestAppContext) {
        let (workspace, cx) = new_workspace(cx).await;
        let with_button = |panel: &mut TestPanel| panel.icon = Some(IconName::FileTree);
        let shown = add_panel(&workspace, DockPosition::Left, 100, with_button, cx);
        let other = add_panel(&workspace, DockPosition::Left, 200, with_button, cx);
        let third = add_panel(&workspace, DockPosition::Left, 300, with_button, cx);
        show_in_left_dock(&workspace, &shown, cx);

        // A panel that was not the one shown leaves: nothing changes here.
        move_panel(&workspace, &other, DockPosition::Right, cx);
        assert_eq!(
            shown_in(&workspace, DockPosition::Left, cx),
            Some(shown.entity_id())
        );

        // A closed dock stays closed when the panel it would show leaves.
        workspace.update_in(cx, |workspace, window, cx| {
            workspace.toggle_dock(DockPosition::Left, window, cx);
        });
        cx.run_until_parked();
        assert_eq!(shown_in(&workspace, DockPosition::Left, cx), None);
        move_panel(&workspace, &shown, DockPosition::Right, cx);
        assert_eq!(shown_in(&workspace, DockPosition::Left, cx), None);
        assert_eq!(dock_of(&workspace, &third, cx), vec![DockPosition::Left]);
        workspace.read_with(cx, |workspace, cx| {
            assert!(!workspace.left_dock().read(cx).is_open());
        });
    }

    #[gpui::test]
    async fn test_panels_moved_together_do_not_carry_their_neighbor(cx: &mut TestAppContext) {
        let (workspace, cx) = new_workspace(cx).await;
        let with_button = |panel: &mut TestPanel| panel.icon = Some(IconName::FileTree);
        let shown = add_panel(&workspace, DockPosition::Left, 100, with_button, cx);
        let hidden = add_panel(&workspace, DockPosition::Left, 200, with_button, cx);
        let staying = add_panel(&workspace, DockPosition::Left, 300, with_button, cx);
        show_in_left_dock(&workspace, &shown, cx);

        // One settings change moves two panels, as a layout preset does. The
        // second one was not on screen: it must not arrive as the shown panel
        // only because it stood next to the one that was.
        for panel in [&shown, &hidden] {
            panel.update(cx, |panel, _| panel.position = DockPosition::Right);
        }
        cx.update(|_, cx| cx.update_global::<SettingsStore, _>(|_, _| {}));
        cx.run_until_parked();

        assert_eq!(dock_of(&workspace, &shown, cx), vec![DockPosition::Right]);
        assert_eq!(dock_of(&workspace, &hidden, cx), vec![DockPosition::Right]);
        assert_eq!(
            shown_in(&workspace, DockPosition::Right, cx),
            Some(shown.entity_id()),
            "the panel that was on screen is the one shown where it arrives"
        );
        assert_eq!(
            shown_in(&workspace, DockPosition::Left, cx),
            Some(staying.entity_id()),
            "the dock they left shows the panel that stayed"
        );
    }

    #[gpui::test]
    async fn test_move_to_the_same_dock_only_reveals(cx: &mut TestAppContext) {
        let (workspace, cx) = new_workspace(cx).await;
        let panel = add_panel(&workspace, DockPosition::Right, 100, |_| {}, cx);
        workspace.read_with(cx, |workspace, cx| {
            assert!(!workspace.right_dock().read(cx).is_open());
        });

        move_panel(&workspace, &panel, DockPosition::Right, cx);
        assert_open_and_focused_in(&workspace, &panel, DockPosition::Right, cx);
        workspace.read_with(cx, |workspace, _| {
            assert!(workspace.panels_to_reveal_on_arrival.is_empty());
            assert!(workspace.notification_ids().is_empty());
        });
    }

    #[gpui::test]
    async fn test_invalid_target_is_refused(cx: &mut TestAppContext) {
        let (workspace, cx) = new_workspace(cx).await;
        let panel = add_panel(
            &workspace,
            DockPosition::Left,
            100,
            |panel| {
                panel.valid_positions = Some(vec![DockPosition::Left, DockPosition::Right]);
            },
            cx,
        );

        move_panel(&workspace, &panel, DockPosition::Bottom, cx);

        assert_eq!(dock_of(&workspace, &panel, cx), vec![DockPosition::Left]);
        workspace.read_with(cx, |workspace, cx| {
            assert_eq!(panel.read(cx).position, DockPosition::Left);
            assert!(!workspace.bottom_dock().read(cx).is_open());
            assert!(!workspace.left_dock().read(cx).is_open());
            assert!(workspace.panels_to_reveal_on_arrival.is_empty());
            assert!(
                workspace.notification_ids().is_empty(),
                "a refused dock is not an error to report"
            );
        });
    }

    #[gpui::test]
    async fn test_panel_that_does_not_move_is_reported(cx: &mut TestAppContext) {
        let (workspace, cx) = new_workspace(cx).await;
        let panel = add_panel(
            &workspace,
            DockPosition::Left,
            100,
            |panel| panel.ignores_set_position = true,
            cx,
        );

        move_panel(&workspace, &panel, DockPosition::Right, cx);

        assert_eq!(dock_of(&workspace, &panel, cx), vec![DockPosition::Left]);
        workspace.read_with(cx, |workspace, _| {
            assert_eq!(
                workspace.notification_ids(),
                vec![NotificationId::unique::<PanelMoveFailed>()]
            );
            assert!(
                workspace.panels_to_reveal_on_arrival.is_empty(),
                "a move that failed must not reveal the panel on a later settings change"
            );
        });
    }

    #[gpui::test]
    async fn test_size_is_kept_on_the_same_axis_and_reset_on_axis_change(cx: &mut TestAppContext) {
        let (workspace, cx) = new_workspace(cx).await;
        let panel = add_panel(&workspace, DockPosition::Left, 100, |_| {}, cx);
        let resized = PanelSizeState {
            size: Some(px(250.)),
            flex: None,
        };
        workspace.update_in(cx, |workspace, window, cx| {
            assert!(workspace.set_panel_size_state::<TestPanel>(resized, window, cx));
        });
        cx.run_until_parked();

        let sizes = |cx: &mut VisualTestContext| {
            workspace.read_with(cx, |workspace, cx| {
                (
                    workspace.panel_size_state::<TestPanel>(cx),
                    workspace.persisted_panel_size_state("TestPanel", cx),
                )
            })
        };
        assert_eq!(sizes(cx), (Some(resized), Some(resized)));

        // Left → Right: still a width.
        move_panel(&workspace, &panel, DockPosition::Right, cx);
        assert_eq!(sizes(cx), (Some(resized), Some(resized)));

        // Right → Bottom: the width must not come back as a height, now or
        // after a restart.
        move_panel(&workspace, &panel, DockPosition::Bottom, cx);
        assert_eq!(
            sizes(cx),
            (
                Some(PanelSizeState::default()),
                Some(PanelSizeState::default())
            )
        );

        // Bottom → Left: the same for a height.
        let resized = PanelSizeState {
            size: Some(px(180.)),
            flex: None,
        };
        workspace.update_in(cx, |workspace, window, cx| {
            assert!(workspace.set_panel_size_state::<TestPanel>(resized, window, cx));
        });
        cx.run_until_parked();
        assert_eq!(sizes(cx), (Some(resized), Some(resized)));
        move_panel(&workspace, &panel, DockPosition::Left, cx);
        assert_eq!(
            sizes(cx),
            (
                Some(PanelSizeState::default()),
                Some(PanelSizeState::default())
            )
        );
    }

    #[gpui::test]
    async fn test_moving_a_panel_clears_zoom(cx: &mut TestAppContext) {
        let (workspace, cx) = new_workspace(cx).await;
        let panel = add_panel(&workspace, DockPosition::Bottom, 100, |_| {}, cx);
        workspace.update_in(cx, |workspace, window, cx| {
            workspace.toggle_dock(DockPosition::Bottom, window, cx);
        });
        panel.update(cx, |_, cx| cx.emit(PanelEvent::ZoomIn));
        cx.run_until_parked();
        workspace.read_with(cx, |workspace, cx| {
            assert!(panel.read(cx).zoomed);
            assert_eq!(workspace.zoomed_position, Some(DockPosition::Bottom));
        });

        move_panel(&workspace, &panel, DockPosition::Left, cx);

        assert_open_and_focused_in(&workspace, &panel, DockPosition::Left, cx);
        workspace.read_with(cx, |workspace, cx| {
            assert!(!panel.read(cx).zoomed);
            assert!(workspace.zoomed.is_none());
            assert_eq!(workspace.zoomed_position, None);
        });
    }

    #[gpui::test]
    async fn test_move_focused_panel_actions(cx: &mut TestAppContext) {
        let (workspace, cx) = new_workspace(cx).await;
        let panel = add_panel(&workspace, DockPosition::Right, 100, |_| {}, cx);

        // Nothing is focused in a panel: the actions do nothing.
        cx.dispatch_action(MoveFocusedPanelToLeftDock);
        cx.run_until_parked();
        assert_eq!(dock_of(&workspace, &panel, cx), vec![DockPosition::Right]);

        workspace.update_in(cx, |workspace, window, cx| {
            workspace.toggle_panel_focus::<TestPanel>(window, cx);
        });
        cx.run_until_parked();

        cx.dispatch_action(MoveFocusedPanelToBottomDock);
        cx.run_until_parked();
        assert_open_and_focused_in(&workspace, &panel, DockPosition::Bottom, cx);

        cx.dispatch_action(MoveFocusedPanelToLeftDock);
        cx.run_until_parked();
        assert_open_and_focused_in(&workspace, &panel, DockPosition::Left, cx);

        cx.dispatch_action(MoveFocusedPanelToRightDock);
        cx.run_until_parked();
        assert_open_and_focused_in(&workspace, &panel, DockPosition::Right, cx);
    }

    #[gpui::test]
    async fn test_drag_bottom_tab_to_left_zone_with_mouse_events(cx: &mut TestAppContext) {
        let (workspace, cx) = new_workspace(cx).await;
        let panel = add_panel(&workspace, DockPosition::Bottom, 100, |_| {}, cx);
        workspace.update_in(cx, |workspace, window, cx| {
            workspace.toggle_dock(DockPosition::Bottom, window, cx);
        });
        cx.run_until_parked();
        for position in ALL_POSITIONS {
            assert_eq!(
                cx.debug_bounds(drop_zone_selector(position)),
                None,
                "no zone at rest"
            );
        }

        start_drag("PANEL-TAB-0", cx);
        assert!(cx.update(|_, cx| cx.has_active_drag()));
        assert!(workspace.read_with(cx, |workspace, _| workspace.panel_drag_active()));
        let workspace_bounds = workspace.read_with(cx, |workspace, _| workspace.bounds);
        let zones = ALL_POSITIONS.map(|position| {
            cx.debug_bounds(drop_zone_selector(position))
                .unwrap_or_else(|| panic!("the {position:?} zone should be drawn"))
        });
        for zone in zones {
            assert!(zone.size.width > px(0.) && zone.size.height > px(0.));
            assert!(
                workspace_bounds.contains(&zone.origin)
                    && zone.right() <= workspace_bounds.right()
                    && zone.bottom() <= workspace_bounds.bottom(),
                "{zone:?} should be inside the workspace area {workspace_bounds:?}"
            );
        }
        let left_zone = zones[0];

        mouse_move(left_zone.center(), cx);
        mouse_up(left_zone.center(), cx);

        assert_open_and_focused_in(&workspace, &panel, DockPosition::Left, cx);
        assert!(!cx.update(|_, cx| cx.has_active_drag()));
        assert!(!workspace.read_with(cx, |workspace, _| workspace.panel_drag_active()));
        assert_eq!(
            cx.debug_bounds(drop_zone_selector(DockPosition::Left)),
            None
        );
    }

    #[gpui::test]
    async fn test_drag_activity_bar_icon_shows_only_accepting_zones(cx: &mut TestAppContext) {
        let (workspace, cx) = new_workspace(cx).await;
        let panel = add_panel(
            &workspace,
            DockPosition::Left,
            100,
            |panel| {
                panel.icon = Some(IconName::FileTree);
                panel.valid_positions = Some(vec![DockPosition::Left, DockPosition::Right]);
            },
            cx,
        );
        cx.run_until_parked();

        start_drag("PANEL-BUTTON-TestPanel", cx);
        assert!(workspace.read_with(cx, |workspace, _| workspace.panel_drag_active()));
        assert!(
            cx.debug_bounds(drop_zone_selector(DockPosition::Left))
                .is_some()
        );
        assert_eq!(
            cx.debug_bounds(drop_zone_selector(DockPosition::Bottom)),
            None,
            "a dock that refuses the panel shows no zone"
        );
        let right_zone = cx
            .debug_bounds(drop_zone_selector(DockPosition::Right))
            .expect("the right zone should be drawn");

        // Released where the bottom zone would be: nothing happens.
        let workspace_bounds = workspace.read_with(cx, |workspace, _| workspace.bounds);
        let over_the_editor = point(
            workspace_bounds.center().x,
            workspace_bounds.bottom() - px(20.),
        );
        mouse_move(over_the_editor, cx);
        mouse_up(over_the_editor, cx);
        assert_eq!(dock_of(&workspace, &panel, cx), vec![DockPosition::Left]);
        assert!(!workspace.read_with(cx, |workspace, _| workspace.panel_drag_active()));
        workspace.read_with(cx, |workspace, cx| {
            assert!(!workspace.left_dock().read(cx).is_open());
            assert!(workspace.notification_ids().is_empty());
        });

        // A second drag, released over the right zone.
        start_drag("PANEL-BUTTON-TestPanel", cx);
        mouse_move(right_zone.center(), cx);
        mouse_up(right_zone.center(), cx);
        assert_open_and_focused_in(&workspace, &panel, DockPosition::Right, cx);
    }

    #[gpui::test]
    async fn test_escape_cancels_the_drag(cx: &mut TestAppContext) {
        let (workspace, cx) = new_workspace(cx).await;
        let panel = add_panel(&workspace, DockPosition::Bottom, 100, |_| {}, cx);
        workspace.update_in(cx, |workspace, window, cx| {
            workspace.toggle_dock(DockPosition::Bottom, window, cx);
        });
        cx.run_until_parked();

        start_drag("PANEL-TAB-0", cx);
        let left_zone = cx
            .debug_bounds(drop_zone_selector(DockPosition::Left))
            .expect("the left zone should be drawn");
        mouse_move(left_zone.center(), cx);

        cx.simulate_keystrokes("escape");
        cx.run_until_parked();
        assert!(!cx.update(|_, cx| cx.has_active_drag()));
        assert!(!workspace.read_with(cx, |workspace, _| workspace.panel_drag_active()));
        assert_eq!(
            cx.debug_bounds(drop_zone_selector(DockPosition::Left)),
            None
        );

        mouse_up(left_zone.center(), cx);
        assert_eq!(dock_of(&workspace, &panel, cx), vec![DockPosition::Bottom]);
        workspace.read_with(cx, |workspace, cx| {
            assert!(workspace.bottom_dock().read(cx).is_open());
            assert!(!workspace.left_dock().read(cx).is_open());
        });
    }

    #[gpui::test]
    async fn test_click_on_a_drag_source_still_clicks(cx: &mut TestAppContext) {
        let (workspace, cx) = new_workspace(cx).await;
        let panel = add_panel(
            &workspace,
            DockPosition::Left,
            100,
            |panel| panel.icon = Some(IconName::FileTree),
            cx,
        );
        workspace.update(cx, |workspace, _| {
            workspace.register_action(|workspace, _: &ToggleTestPanel, window, cx| {
                workspace.toggle_panel_focus::<TestPanel>(window, cx);
            });
        });
        cx.run_until_parked();

        let button = cx
            .debug_bounds("PANEL-BUTTON-TestPanel")
            .expect("the panel should have a button in the activity bar");
        mouse_down(button.center(), cx);
        mouse_up(button.center(), cx);

        assert!(!workspace.read_with(cx, |workspace, _| workspace.panel_drag_active()));
        assert_open_and_focused_in(&workspace, &panel, DockPosition::Left, cx);
    }

    #[gpui::test]
    async fn test_release_on_the_handle_itself_shows_the_panel(cx: &mut TestAppContext) {
        let (workspace, cx) = new_workspace(cx).await;
        let panel = add_panel(
            &workspace,
            DockPosition::Left,
            100,
            |panel| panel.icon = Some(IconName::FileTree),
            cx,
        );
        cx.run_until_parked();

        // A click during which the pointer slips: a drag starts, and ends on
        // the icon it started from.
        let button = cx
            .debug_bounds("PANEL-BUTTON-TestPanel")
            .expect("the panel should have a button in the activity bar");
        start_drag("PANEL-BUTTON-TestPanel", cx);
        assert!(workspace.read_with(cx, |workspace, _| workspace.panel_drag_active()));
        mouse_up(button.center() + point(px(6.), px(6.)), cx);

        assert!(!workspace.read_with(cx, |workspace, _| workspace.panel_drag_active()));
        assert_open_and_focused_in(&workspace, &panel, DockPosition::Left, cx);
    }

    #[gpui::test]
    async fn test_drop_on_a_handle_of_another_dock_moves_there(cx: &mut TestAppContext) {
        let (workspace, cx) = new_workspace(cx).await;
        let _left_panel = add_panel(
            &workspace,
            DockPosition::Left,
            100,
            |panel| panel.icon = Some(IconName::FileTree),
            cx,
        );
        let bottom_panel = add_panel(&workspace, DockPosition::Bottom, 200, |_| {}, cx);
        workspace.update_in(cx, |workspace, window, cx| {
            workspace.toggle_dock(DockPosition::Bottom, window, cx);
        });
        cx.run_until_parked();

        // The activity bar stands for the left dock.
        let left_button = cx
            .debug_bounds("PANEL-BUTTON-TestPanel")
            .expect("the left panel should have a button in the activity bar");
        start_drag("PANEL-TAB-0", cx);
        mouse_move(left_button.center(), cx);
        mouse_up(left_button.center(), cx);

        assert_open_and_focused_in(&workspace, &bottom_panel, DockPosition::Left, cx);
    }

    #[gpui::test]
    fn test_drop_zones_never_overlap(_cx: &mut TestAppContext) {
        let workspace_size = size(px(1200.), px(800.));
        for layout in [
            BottomDockLayout::Contained,
            BottomDockLayout::Full,
            BottomDockLayout::LeftAligned,
            BottomDockLayout::RightAligned,
        ] {
            // The last case asks for more than there is.
            for (left, right, bottom) in
                [(264., 264., 224.), (300., 480., 480.), (900., 900., 900.)]
            {
                let zones =
                    drop_zone_bounds(layout, workspace_size, px(left), px(right), px(bottom));
                assert_eq!(
                    zones.map(|(position, _)| position),
                    [
                        DockPosition::Left,
                        DockPosition::Right,
                        DockPosition::Bottom
                    ]
                );
                for (position, zone) in zones {
                    assert!(
                        zone.left() >= px(0.)
                            && zone.top() >= px(0.)
                            && zone.right() <= workspace_size.width
                            && zone.bottom() <= workspace_size.height,
                        "{layout:?}: the {position:?} zone {zone:?} leaves the workspace"
                    );
                }
                for (ix, (position, zone)) in zones.iter().enumerate() {
                    for (other_position, other) in &zones[ix + 1..] {
                        let apart = zone.right() <= other.left()
                            || other.right() <= zone.left()
                            || zone.bottom() <= other.top()
                            || other.bottom() <= zone.top();
                        assert!(
                            apart,
                            "{layout:?}: the {position:?} and {other_position:?} zones overlap"
                        );
                    }
                }
            }
        }

        // Contained: the side docks keep their corners.
        let [(_, left), (_, right), (_, bottom)] = drop_zone_bounds(
            BottomDockLayout::Contained,
            workspace_size,
            px(200.),
            px(300.),
            px(250.),
        );
        assert_eq!(left.size, size(px(200.), px(800.)));
        assert_eq!(right.origin, point(px(900.), px(0.)));
        assert_eq!(bottom.origin, point(px(200.), px(550.)));
        assert_eq!(bottom.size, size(px(700.), px(250.)));

        // Full: the bottom dock takes the whole width.
        let [(_, left), (_, right), (_, bottom)] = drop_zone_bounds(
            BottomDockLayout::Full,
            workspace_size,
            px(200.),
            px(300.),
            px(250.),
        );
        assert_eq!(left.size, size(px(200.), px(550.)));
        assert_eq!(right.size, size(px(300.), px(550.)));
        assert_eq!(bottom.origin, point(px(0.), px(550.)));
        assert_eq!(bottom.size, size(px(1200.), px(250.)));
    }

    #[gpui::test]
    async fn test_panel_titles(cx: &mut TestAppContext) {
        assert_eq!(default_panel_title("Terminal Panel").as_ref(), "Terminal");
        assert_eq!(default_panel_title("Project Panel").as_ref(), "Project");
        assert_eq!(default_panel_title("DebugPanel").as_ref(), "Debug");
        assert_eq!(default_panel_title("Outline").as_ref(), "Outline");
        assert_eq!(default_panel_title("Panel").as_ref(), "Panel");

        let (workspace, cx) = new_workspace(cx).await;
        // Without a tooltip the title comes from the persistent name.
        let without_button = add_panel(&workspace, DockPosition::Left, 100, |_| {}, cx);
        let with_button = add_panel(
            &workspace,
            DockPosition::Right,
            200,
            |panel| panel.icon = Some(IconName::FileTree),
            cx,
        );
        cx.update(|window, cx| {
            assert_eq!(
                PanelHandle::title(&without_button, window, cx).as_ref(),
                "Test"
            );
            assert_eq!(
                PanelHandle::title(&with_button, window, cx).as_ref(),
                "Test"
            );
        });
    }
}

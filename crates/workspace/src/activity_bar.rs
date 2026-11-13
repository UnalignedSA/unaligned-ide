//! The activity bar: the column of icons along the window's left edge.
//!
//! It is the one place where panels are switched. Every panel that has a
//! button is listed, grouped by the dock it lives in (left, right, bottom), and
//! an icon shows or hides its panel wherever that panel is docked. An icon can
//! also be dragged: onto another group of the bar, or onto one of the drop
//! zones the workspace draws over the docks (see [`crate::panel_drag`]).
//!
//! Under the panels, the bar ends with the AI connections and the Manage menu.

use std::{rc::Rc, sync::Arc};

use gpui::{
    Action, Anchor, AnyElement, App, Context, Entity, FocusHandle, Global, Hsla, IntoElement,
    ParentElement, Pixels, Render, Role, SharedString, Styled, Subscription, WeakEntity, Window,
    div, px, svg,
};
use settings::SettingsStore;
use ui::{
    ButtonLike, ContextMenu, CountBadge, MarkerSide, PopoverMenu, Tooltip, prelude::*,
    right_click_menu, selected_marker,
};

use crate::{
    Workspace,
    dock::{Dock, DockPosition, PanelHandle, panel_context_menu},
    panel_drag::{DraggedPanel, panel_drag_source},
};

/// The side of the icons in the bar, whatever the density.
const ICON_SIZE: f32 = 20.;

/// The line between the bar and what is next to it. It is drawn over the
/// bar's right edge instead of beside it, so that a cell is as wide as the
/// bar and its icon sits on whole pixels.
const BAR_LINE: Pixels = px(1.);

/// The action that opens the connection screen. It belongs to the AI crate,
/// which depends on this one, so it is built by name.
const CONNECTIONS_ACTION: &str = "agent::ManageProviders";

type ManageMenuSection =
    Rc<dyn Fn(ContextMenu, &mut Window, &mut Context<ContextMenu>) -> ContextMenu>;

#[derive(Default)]
struct ManageMenuSections(Vec<ManageMenuSection>);

impl Global for ManageMenuSections {}

/// Adds a section to the Manage menu (the gear at the bottom of the activity
/// bar). Sections come after the built-in entries, each behind a separator, in
/// the order they were registered. Call it once, at startup.
///
/// ```ignore
/// workspace::activity_bar::register_manage_menu_section(cx, |menu, _window, _cx| {
///     menu.action("Reset Layout", Box::new(ResetLayout))
/// });
/// ```
pub fn register_manage_menu_section(
    cx: &mut App,
    section: impl Fn(ContextMenu, &mut Window, &mut Context<ContextMenu>) -> ContextMenu + 'static,
) {
    cx.default_global::<ManageMenuSections>()
        .0
        .push(Rc::new(section));
}

/// The groups of the bar, top to bottom, with the edge of a cell that carries
/// the marker of the panel shown in that dock.
const GROUPS: [(DockPosition, MarkerSide); 3] = [
    (DockPosition::Left, MarkerSide::Left),
    (DockPosition::Right, MarkerSide::Right),
    (DockPosition::Bottom, MarkerSide::Bottom),
];

fn group_id(position: DockPosition) -> &'static str {
    match position {
        DockPosition::Left => "activity-bar-left-group",
        DockPosition::Right => "activity-bar-right-group",
        DockPosition::Bottom => "activity-bar-bottom-group",
    }
}

fn group_selector(position: DockPosition) -> &'static str {
    match position {
        DockPosition::Left => "ACTIVITY-BAR-GROUP-LEFT",
        DockPosition::Right => "ACTIVITY-BAR-GROUP-RIGHT",
        DockPosition::Bottom => "ACTIVITY-BAR-GROUP-BOTTOM",
    }
}

/// What the bar needs to know about a panel to draw its icon.
struct PanelCell {
    panel: Arc<dyn PanelHandle>,
    icon: IconName,
    title: SharedString,
    count: Option<usize>,
    /// The panel is the one shown in its dock, and that dock is open.
    is_shown: bool,
    click_action: Box<dyn Action>,
    toggle_action: Box<dyn Action>,
    /// Focused before the action is dispatched, so that it is dispatched from
    /// inside the workspace even when the keyboard focus is elsewhere.
    focus_handle: FocusHandle,
}

pub struct ActivityBar {
    workspace: WeakEntity<Workspace>,
    /// In the order of the groups: left, right, bottom.
    docks: [Entity<Dock>; 3],
    _subscriptions: [Subscription; 4],
}

impl ActivityBar {
    pub(crate) fn new(
        workspace: WeakEntity<Workspace>,
        left_dock: Entity<Dock>,
        right_dock: Entity<Dock>,
        bottom_dock: Entity<Dock>,
        cx: &mut Context<Self>,
    ) -> Self {
        let subscriptions = [
            cx.observe(&left_dock, |_, _, cx| cx.notify()),
            cx.observe(&right_dock, |_, _, cx| cx.notify()),
            cx.observe(&bottom_dock, |_, _, cx| cx.notify()),
            // A panel's button can be hidden, and its dock changed, in the settings.
            cx.observe_global::<SettingsStore>(|_, cx| cx.notify()),
        ];
        Self {
            workspace,
            docks: [left_dock, right_dock, bottom_dock],
            _subscriptions: subscriptions,
        }
    }

    fn dock(&self, position: DockPosition) -> &Entity<Dock> {
        match position {
            DockPosition::Left => &self.docks[0],
            DockPosition::Right => &self.docks[1],
            DockPosition::Bottom => &self.docks[2],
        }
    }

    /// The panels of a dock that have a button, in the dock's own order.
    fn panel_cells(&self, position: DockPosition, window: &Window, cx: &App) -> Vec<PanelCell> {
        let dock = self.dock(position).read(cx);
        let shown_panel_id = dock.visible_panel().map(|panel| panel.panel_id());
        let focus_handle = dock.panel_button_focus_handle(window, cx);
        dock.panels()
            .filter(|panel| panel.enabled(cx))
            .filter_map(|panel| {
                // A panel without an icon has its button hidden in the settings.
                let icon = panel.icon(window, cx)?;
                let is_shown = shown_panel_id == Some(panel.panel_id());
                let toggle_action = panel.toggle_action(window, cx);
                let click_action = if is_shown {
                    // The panel's own action would only move the focus into a
                    // panel that is shown but not focused: the icon hides it.
                    dock.toggle_action()
                } else {
                    toggle_action.boxed_clone()
                };
                Some(PanelCell {
                    panel: panel.clone(),
                    icon,
                    title: panel.title(window, cx),
                    count: panel
                        .icon_label(window, cx)
                        .and_then(|label| label.parse::<usize>().ok()),
                    is_shown,
                    click_action,
                    toggle_action,
                    focus_handle: focus_handle.clone(),
                })
            })
            .collect()
    }

    fn render_panel_cell(
        &self,
        cell: PanelCell,
        position: DockPosition,
        marker_side: MarkerSide,
        cx: &App,
    ) -> AnyElement {
        let PanelCell {
            panel,
            icon,
            title,
            count,
            is_shown,
            click_action,
            toggle_action,
            focus_handle,
        } = cell;
        let panel_id = panel.panel_id();
        let name = panel.persistent_name();
        let cell_size = ui::metrics::activity_bar_width(cx);
        let dock = self.dock(position).clone();
        let workspace = self.workspace.clone();
        let menu_panel = panel.clone();
        let menu_workspace = workspace.clone();

        right_click_menu(("activity-bar-panel-menu", panel_id))
            .menu(move |window, cx| {
                panel_context_menu(
                    menu_panel.clone(),
                    dock.clone(),
                    menu_workspace.clone(),
                    position,
                    window,
                    cx,
                )
            })
            .anchor(Anchor::TopLeft)
            .attach(Anchor::TopRight)
            .trigger(move |is_menu_open, _window, cx| {
                let button = bar_button(("activity-bar-panel", panel_id), icon, is_shown, cx)
                    .aria_label(title.clone())
                    .on_click(move |_, window, cx| {
                        window.focus(&focus_handle, cx);
                        window.dispatch_action(click_action.boxed_clone(), cx);
                    })
                    // The menu opened from this icon would be under its tooltip.
                    .when(!is_menu_open, |this| {
                        this.tooltip(move |_window, cx| {
                            Tooltip::for_action(title.clone(), &*toggle_action, cx)
                        })
                    });

                let cell = div()
                    .id(("panel-drag-source", panel_id))
                    .debug_selector(|| format!("PANEL-BUTTON-{name}"))
                    .relative()
                    .flex_none()
                    .size(cell_size)
                    .child(button)
                    .when(is_shown, |this| {
                        // Kept clear of the bar's own line on the right.
                        this.child(match marker_side {
                            MarkerSide::Left | MarkerSide::Top => selected_marker(marker_side, cx),
                            MarkerSide::Right | MarkerSide::Bottom => {
                                selected_marker(marker_side, cx).right(BAR_LINE)
                            }
                        })
                    })
                    .when_some(count, |this, count| {
                        // Over the icon's lower right corner, clear of the
                        // marker whichever edge it is on.
                        this.child(
                            div()
                                .absolute()
                                .right(px(4.))
                                .bottom(px(5.))
                                .child(CountBadge::new(count).inline()),
                        )
                    });
                // The press lands on the button inside; the cell around it
                // starts the drag once the pointer moves, and a plain click
                // still reaches the button.
                panel_drag_source(cell, panel, position, workspace)
            })
            .into_any_element()
    }

    /// One group of the bar: the icons of a dock's panels. The whole group is
    /// a place to drop a dragged panel, which then moves to that dock.
    fn render_group(&self, position: DockPosition, cells: Vec<AnyElement>, cx: &App) -> AnyElement {
        let colors = cx.theme().colors();
        let fill = colors.drop_target_background;
        let slot_size = ui::metrics::activity_bar_width(cx);
        let is_empty = cells.is_empty();
        let workspace = self.workspace.clone();

        v_flex()
            .id(group_id(position))
            .debug_selector(|| group_selector(position).to_string())
            .flex_none()
            .w_full()
            .can_drop(move |dragged, _window, cx| {
                dragged
                    .downcast_ref::<DraggedPanel>()
                    .is_some_and(|dragged| dragged.panel.position_is_valid(position, cx))
            })
            .drag_over::<DraggedPanel>(move |style, _, _, _| style.bg(fill))
            .on_drop(move |dragged: &DraggedPanel, window, cx| {
                workspace
                    .update(cx, |workspace, cx| {
                        workspace.move_panel_to_dock(&dragged.panel, position, window, cx);
                    })
                    .ok();
            })
            .when(is_empty, |this| {
                // Only drawn during a panel drag: a slot that stands for a
                // dock without any panel, so a panel can still be dropped there.
                this.h(slot_size)
                    .items_center()
                    .justify_center()
                    .child(dock_glyph(position, colors.text_faint))
            })
            .children(cells)
            .into_any_element()
    }

    /// The bottom of the bar: the AI connections and the Manage menu.
    fn render_footer(&self, cx: &App) -> impl IntoElement {
        // Without the AI crate (tests of this crate) the action is unknown:
        // the agent settings are the nearest thing every build has.
        let connections_action = cx
            .build_action(CONNECTIONS_ACTION, None)
            .unwrap_or_else(|_| app_actions::agent::OpenSettings.boxed_clone());
        let cell_size = ui::metrics::activity_bar_width(cx);
        let footer_cell = || div().relative().flex_none().size(cell_size);

        v_flex()
            .flex_none()
            .child(
                footer_cell().child(
                    bar_button("activity-bar-connections", IconName::Plug, false, cx)
                        .aria_label("AI Connections")
                        .tooltip({
                            let action = connections_action.boxed_clone();
                            move |_window, cx| {
                                Tooltip::with_meta(
                                    "AI Connections",
                                    Some(&*action),
                                    "Keys, subscriptions and local models",
                                    cx,
                                )
                            }
                        })
                        .on_click(move |_, window, cx| {
                            window.dispatch_action(connections_action.boxed_clone(), cx);
                        }),
                ),
            )
            .child(
                footer_cell().child(
                    PopoverMenu::new("activity-bar-manage-menu")
                        .trigger_with_tooltip(
                            bar_button("activity-bar-manage", IconName::Settings, false, cx)
                                .aria_label("Manage"),
                            Tooltip::text("Manage"),
                        )
                        .anchor(Anchor::BottomLeft)
                        .attach(Anchor::BottomRight)
                        .menu(|window, cx| Some(manage_menu(window, cx))),
                ),
            )
    }
}

/// The menu of the gear: where the product is configured.
fn manage_menu(window: &mut Window, cx: &mut App) -> Entity<ContextMenu> {
    let sections = cx
        .try_global::<ManageMenuSections>()
        .map(|sections| sections.0.clone())
        .unwrap_or_default();
    ContextMenu::build(window, cx, move |menu, window, cx| {
        let mut menu = menu
            .action("Settings", app_actions::OpenSettings.boxed_clone())
            .action("Keyboard Shortcuts", app_actions::OpenKeymap.boxed_clone())
            .separator()
            .action(
                "Themes",
                app_actions::theme_selector::Toggle::default().boxed_clone(),
            )
            .action(
                "Extensions",
                app_actions::Extensions::default().boxed_clone(),
            );
        for section in &sections {
            menu = section(menu.separator(), window, cx);
        }
        menu
    })
}

/// A button that fills a cell of the bar: a 20px icon, muted at rest, in the
/// text colour while hovered or while `active`. It has no ground of its own.
fn bar_button(id: impl Into<ElementId>, icon: IconName, active: bool, cx: &App) -> ButtonLike {
    let colors = cx.theme().colors();
    let hover_color = colors.icon;
    let color = if active {
        colors.icon
    } else {
        colors.icon_muted
    };
    let cell_size = ui::metrics::activity_bar_width(cx);
    ButtonLike::new(id)
        .style(ButtonStyle::Transparent)
        .size(ButtonSize::None)
        .width(cell_size)
        .height(cell_size.into())
        .tab_index(0isize)
        .child(
            // `ButtonLike` is the unnamed hover group its content reacts to.
            svg()
                .path(icon.path())
                .size(rems_from_px(ICON_SIZE))
                .flex_none()
                .text_color(color)
                .group_hover("", move |style| style.text_color(hover_color)),
        )
}

/// A small drawing of the window with one dock filled in, for the slot of a
/// dock that has no panel yet.
fn dock_glyph(position: DockPosition, color: Hsla) -> impl IntoElement {
    let filled = div().absolute().bg(color);
    div()
        .relative()
        .flex_none()
        .w(px(16.))
        .h(px(12.))
        .border_1()
        .border_color(color)
        .child(match position {
            DockPosition::Left => filled.left_0().top_0().bottom_0().w(px(5.)),
            DockPosition::Right => filled.right_0().top_0().bottom_0().w(px(5.)),
            DockPosition::Bottom => filled.left_0().right_0().bottom_0().h(px(4.)),
        })
}

fn group_divider(color: Hsla) -> impl IntoElement {
    div().flex_none().h_px().mx_2().my_2().bg(color)
}

impl Render for ActivityBar {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        // An empty group is given a slot only while a panel is being dragged,
        // and only when its dock accepts that panel.
        let dragged_panel = if cx.has_active_drag() {
            self.workspace
                .upgrade()
                .and_then(|workspace| workspace.read(cx).dragged_panel().cloned())
        } else {
            None
        };
        let border_color = cx.theme().colors().border;

        let mut groups = Vec::new();
        for (position, marker_side) in GROUPS {
            let cells = self
                .panel_cells(position, window, cx)
                .into_iter()
                .map(|cell| self.render_panel_cell(cell, position, marker_side, cx))
                .collect::<Vec<_>>();
            if cells.is_empty()
                && !dragged_panel
                    .as_ref()
                    .is_some_and(|panel| panel.position_is_valid(position, cx))
            {
                continue;
            }
            if !groups.is_empty() {
                groups.push(group_divider(border_color).into_any_element());
            }
            groups.push(self.render_group(position, cells, cx));
        }

        v_flex()
            .id("activity-bar")
            .role(Role::Toolbar)
            .aria_label("Activity bar")
            .relative()
            .h_full()
            .w(ui::metrics::activity_bar_width(cx))
            .flex_none()
            .justify_between()
            .bg(cx.theme().colors().background)
            .child(
                v_flex()
                    .id("activity-bar-panels")
                    .flex_1()
                    .min_h_0()
                    .overflow_y_scroll()
                    .children(groups),
            )
            .child(self.render_footer(cx))
            // The only line the bar draws: the ones above and below it are the
            // title bar's and the status bar's.
            .child(
                div()
                    .absolute()
                    .top_0()
                    .bottom_0()
                    .right_0()
                    .w(BAR_LINE)
                    .bg(border_color),
            )
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{
        MultiWorkspace,
        dock::test::{TestPanel, ToggleTestPanel},
    };
    use fs::FakeFs;
    use gpui::{
        EventEmitter, Focusable, Modifiers, MouseButton, MouseDownEvent, MouseMoveEvent,
        MouseUpEvent, Pixels, TestAppContext, VisualTestContext, actions, point,
    };
    use project::Project;

    use crate::dock::{Panel, PanelEvent};

    actions!(test_only, [ToggleBottomTestPanel]);

    /// A second kind of panel, so that two toggle actions can be told apart.
    struct BottomTestPanel {
        focus_handle: FocusHandle,
        position: DockPosition,
        count: Option<usize>,
    }

    impl EventEmitter<PanelEvent> for BottomTestPanel {}

    impl Focusable for BottomTestPanel {
        fn focus_handle(&self, _cx: &App) -> FocusHandle {
            self.focus_handle.clone()
        }
    }

    impl Render for BottomTestPanel {
        fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
            div()
                .id("bottom-test-panel")
                .track_focus(&self.focus_handle(cx))
        }
    }

    impl Panel for BottomTestPanel {
        fn persistent_name() -> &'static str {
            "BottomTestPanel"
        }

        fn panel_key() -> &'static str {
            "BottomTestPanel"
        }

        fn position(&self, _: &Window, _: &App) -> DockPosition {
            self.position
        }

        fn position_is_valid(&self, _: DockPosition) -> bool {
            true
        }

        fn set_position(&mut self, position: DockPosition, _: &mut Window, cx: &mut Context<Self>) {
            self.position = position;
            cx.update_global::<SettingsStore, _>(|_, _| {});
        }

        fn default_size(&self, _: &Window, _: &App) -> Pixels {
            px(200.)
        }

        fn icon(&self, _: &Window, _: &App) -> Option<IconName> {
            Some(IconName::Terminal)
        }

        fn icon_tooltip(&self, _: &Window, _: &App) -> Option<&'static str> {
            Some("Bottom Test Panel")
        }

        fn icon_label(&self, _: &Window, _: &App) -> Option<String> {
            self.count.map(|count| count.to_string())
        }

        fn toggle_action(&self) -> Box<dyn Action> {
            ToggleBottomTestPanel.boxed_clone()
        }

        fn activation_priority(&self) -> u32 {
            300
        }
    }

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
        workspace.update(cx, |workspace, _| {
            workspace.register_action(|workspace, _: &ToggleTestPanel, window, cx| {
                workspace.toggle_panel_focus::<TestPanel>(window, cx);
            });
            workspace.register_action(|workspace, _: &ToggleBottomTestPanel, window, cx| {
                workspace.toggle_panel_focus::<BottomTestPanel>(window, cx);
            });
        });
        cx.run_until_parked();
        (workspace, cx)
    }

    fn add_test_panel(
        workspace: &Entity<Workspace>,
        position: DockPosition,
        cx: &mut VisualTestContext,
    ) -> Entity<TestPanel> {
        let panel = workspace.update_in(cx, |workspace, window, cx| {
            let panel = cx.new(|cx| {
                let mut panel = TestPanel::new(position, 100, cx);
                panel.icon = Some(IconName::FileTree);
                panel
            });
            workspace.add_panel(panel.clone(), window, cx);
            panel
        });
        cx.run_until_parked();
        panel
    }

    fn add_bottom_test_panel(
        workspace: &Entity<Workspace>,
        count: Option<usize>,
        cx: &mut VisualTestContext,
    ) -> Entity<BottomTestPanel> {
        let panel = workspace.update_in(cx, |workspace, window, cx| {
            let panel = cx.new(|cx| BottomTestPanel {
                focus_handle: cx.focus_handle(),
                position: DockPosition::Bottom,
                count,
            });
            workspace.add_panel(panel.clone(), window, cx);
            panel
        });
        cx.run_until_parked();
        panel
    }

    #[track_caller]
    fn click(selector: &'static str, cx: &mut VisualTestContext) {
        let bounds = cx
            .debug_bounds(selector)
            .unwrap_or_else(|| panic!("no element with the selector {selector}"));
        cx.simulate_click(bounds.center(), Modifiers::none());
        cx.run_until_parked();
    }

    /// The width of the bar, which is also the side of a cell: the metric is
    /// in rems, and a test window has its own rem size.
    fn bar_width(cx: &mut VisualTestContext) -> Pixels {
        cx.update(|window, cx| ui::metrics::activity_bar_width(cx).to_pixels(window.rem_size()))
    }

    /// What a dock shows: `None` when it is closed.
    fn shown_in(
        workspace: &Entity<Workspace>,
        position: DockPosition,
        cx: &mut VisualTestContext,
    ) -> Option<gpui::EntityId> {
        workspace.read_with(cx, |workspace, cx| {
            workspace
                .dock_at_position(position)
                .read(cx)
                .visible_panel()
                .map(|panel| panel.panel_id())
        })
    }

    #[gpui::test]
    async fn test_icons_toggle_panels_of_the_right_and_bottom_docks(cx: &mut TestAppContext) {
        let (workspace, cx) = new_workspace(cx).await;
        let right_panel = add_test_panel(&workspace, DockPosition::Right, cx);
        let bottom_panel = add_bottom_test_panel(&workspace, None, cx);

        // Both docks start closed, and both panels have their icon in the bar.
        assert_eq!(shown_in(&workspace, DockPosition::Right, cx), None);
        assert_eq!(shown_in(&workspace, DockPosition::Bottom, cx), None);

        click("PANEL-BUTTON-TestPanel", cx);
        assert_eq!(
            shown_in(&workspace, DockPosition::Right, cx),
            Some(right_panel.entity_id()),
            "a click on the icon opens the right dock on its panel"
        );
        assert_eq!(shown_in(&workspace, DockPosition::Bottom, cx), None);
        workspace.update_in(cx, |_, window, cx| {
            assert!(
                right_panel
                    .read(cx)
                    .focus_handle(cx)
                    .contains_focused(window, cx),
                "the panel that was opened has the keyboard focus"
            );
        });

        click("PANEL-BUTTON-BottomTestPanel", cx);
        assert_eq!(
            shown_in(&workspace, DockPosition::Bottom, cx),
            Some(bottom_panel.entity_id()),
            "a click on the icon opens the bottom dock on its panel"
        );
        assert_eq!(
            shown_in(&workspace, DockPosition::Right, cx),
            Some(right_panel.entity_id()),
            "the right dock is left alone"
        );
        workspace.update_in(cx, |_, window, cx| {
            assert!(
                bottom_panel
                    .read(cx)
                    .focus_handle(cx)
                    .contains_focused(window, cx)
            );
        });

        // A second click on each icon hides its panel, focused or not.
        click("PANEL-BUTTON-TestPanel", cx);
        assert_eq!(shown_in(&workspace, DockPosition::Right, cx), None);
        assert_eq!(
            shown_in(&workspace, DockPosition::Bottom, cx),
            Some(bottom_panel.entity_id())
        );
        click("PANEL-BUTTON-BottomTestPanel", cx);
        assert_eq!(shown_in(&workspace, DockPosition::Bottom, cx), None);
    }

    #[gpui::test]
    async fn test_groups_follow_the_docks(cx: &mut TestAppContext) {
        let (workspace, cx) = new_workspace(cx).await;
        let bar_width = bar_width(cx);

        // No panel: no group at all.
        for (position, _) in GROUPS {
            assert_eq!(cx.debug_bounds(group_selector(position)), None);
        }

        let panel = add_test_panel(&workspace, DockPosition::Right, cx);
        let _bottom_panel = add_bottom_test_panel(&workspace, Some(3), cx);
        assert_eq!(
            cx.debug_bounds(group_selector(DockPosition::Left)),
            None,
            "an empty group takes no room at rest"
        );
        let right_group = cx
            .debug_bounds(group_selector(DockPosition::Right))
            .expect("the right dock's panel should be listed");
        let bottom_group = cx
            .debug_bounds(group_selector(DockPosition::Bottom))
            .expect("the bottom dock's panel should be listed");
        assert_eq!(right_group.size.width, bar_width);
        assert_eq!(right_group.size.height, bar_width);
        assert_eq!(bottom_group.size.height, bar_width);
        assert!(
            right_group.bottom() < bottom_group.top(),
            "the right group comes before the bottom group, with a divider between them"
        );
        let cell = cx
            .debug_bounds("PANEL-BUTTON-TestPanel")
            .expect("the panel should have an icon");
        assert_eq!(cell, right_group, "the only cell of a group fills it");

        // The panel moves to the left dock: so does its icon.
        let handle: Arc<dyn PanelHandle> = Arc::new(panel.clone());
        workspace.update_in(cx, |workspace, window, cx| {
            workspace.move_panel_to_dock(&handle, DockPosition::Left, window, cx);
        });
        cx.run_until_parked();
        assert_eq!(cx.debug_bounds(group_selector(DockPosition::Right)), None);
        let left_group = cx
            .debug_bounds(group_selector(DockPosition::Left))
            .expect("the panel should now be listed in the left group");
        assert_eq!(cx.debug_bounds("PANEL-BUTTON-TestPanel"), Some(left_group));
        assert!(
            left_group.top()
                < cx.debug_bounds(group_selector(DockPosition::Bottom))
                    .unwrap()
                    .top()
        );

        // A panel whose button is hidden in the settings has no icon.
        panel.update(cx, |panel, cx| {
            panel.icon = None;
            cx.notify();
        });
        cx.run_until_parked();
        assert_eq!(cx.debug_bounds("PANEL-BUTTON-TestPanel"), None);
        assert_eq!(cx.debug_bounds(group_selector(DockPosition::Left)), None);
    }

    fn mouse_down(position: gpui::Point<Pixels>, cx: &mut VisualTestContext) {
        cx.simulate_event(MouseDownEvent {
            position,
            button: MouseButton::Left,
            modifiers: Modifiers::default(),
            click_count: 1,
            first_mouse: false,
        });
        cx.run_until_parked();
    }

    fn mouse_move(position: gpui::Point<Pixels>, cx: &mut VisualTestContext) {
        cx.simulate_event(MouseMoveEvent {
            position,
            pressed_button: Some(MouseButton::Left),
            modifiers: Modifiers::default(),
        });
        cx.run_until_parked();
    }

    fn mouse_up(position: gpui::Point<Pixels>, cx: &mut VisualTestContext) {
        cx.simulate_event(MouseUpEvent {
            position,
            button: MouseButton::Left,
            modifiers: Modifiers::default(),
            click_count: 1,
        });
        cx.run_until_parked();
    }

    #[gpui::test]
    async fn test_drag_an_icon_into_another_group(cx: &mut TestAppContext) {
        let (workspace, cx) = new_workspace(cx).await;
        let panel = add_test_panel(&workspace, DockPosition::Left, cx);
        let bottom_panel = add_bottom_test_panel(&workspace, None, cx);
        let dock_of = |panel_id, cx: &mut VisualTestContext| {
            workspace.read_with(cx, |workspace, cx| {
                workspace
                    .all_docks()
                    .into_iter()
                    .find(|dock| dock.read(cx).panel_index_for_id(panel_id).is_some())
                    .map(|dock| dock.read(cx).position())
            })
        };

        // At rest the right dock, which has no panel, has no group.
        assert_eq!(cx.debug_bounds(group_selector(DockPosition::Right)), None);

        // Dragging the left panel's icon gives the empty group a slot.
        let source = cx.debug_bounds("PANEL-BUTTON-TestPanel").unwrap();
        mouse_down(source.center(), cx);
        mouse_move(source.center() + point(px(6.), px(6.)), cx);
        assert!(workspace.read_with(cx, |workspace, _| workspace.panel_drag_active()));
        let right_slot = cx
            .debug_bounds(group_selector(DockPosition::Right))
            .expect("an empty group should get a slot during a panel drag");
        assert_eq!(right_slot.size.height, bar_width(cx));
        assert!(
            right_slot.top() > source.bottom()
                && right_slot.bottom()
                    < cx.debug_bounds(group_selector(DockPosition::Bottom))
                        .unwrap()
                        .top(),
            "the slot of the right dock sits between the left and the bottom group"
        );

        mouse_move(right_slot.center(), cx);
        mouse_up(right_slot.center(), cx);
        assert_eq!(dock_of(panel.entity_id(), cx), Some(DockPosition::Right));
        assert_eq!(
            shown_in(&workspace, DockPosition::Right, cx),
            Some(panel.entity_id()),
            "the panel arrives open"
        );
        assert!(!workspace.read_with(cx, |workspace, _| workspace.panel_drag_active()));
        assert_eq!(
            cx.debug_bounds(group_selector(DockPosition::Left)),
            None,
            "the slot is gone once the drag is over"
        );

        // Dragging the bottom panel's icon onto the right group, which now
        // holds a panel, moves it there too.
        let source = cx.debug_bounds("PANEL-BUTTON-BottomTestPanel").unwrap();
        mouse_down(source.center(), cx);
        mouse_move(source.center() + point(px(6.), px(6.)), cx);
        // Measured during the drag: the left group now has a slot above it.
        let right_group = cx
            .debug_bounds(group_selector(DockPosition::Right))
            .unwrap();
        mouse_move(right_group.center(), cx);
        mouse_up(right_group.center(), cx);
        assert_eq!(
            dock_of(bottom_panel.entity_id(), cx),
            Some(DockPosition::Right)
        );
        assert_eq!(
            shown_in(&workspace, DockPosition::Right, cx),
            Some(bottom_panel.entity_id())
        );
    }

    #[gpui::test]
    async fn test_a_group_refuses_a_panel_its_dock_does_not_accept(cx: &mut TestAppContext) {
        let (workspace, cx) = new_workspace(cx).await;
        let panel = add_test_panel(&workspace, DockPosition::Left, cx);
        panel.update(cx, |panel, _| {
            panel.valid_positions = Some(vec![DockPosition::Left, DockPosition::Right]);
        });
        let _bottom_panel = add_bottom_test_panel(&workspace, None, cx);

        let source = cx.debug_bounds("PANEL-BUTTON-TestPanel").unwrap();
        mouse_down(source.center(), cx);
        mouse_move(source.center() + point(px(6.), px(6.)), cx);
        // Measured once the drag has started: the slot of the empty right
        // group has pushed the bottom group down.
        let bottom_group = cx
            .debug_bounds(group_selector(DockPosition::Bottom))
            .unwrap();
        mouse_move(bottom_group.center(), cx);
        mouse_up(bottom_group.center(), cx);

        workspace.read_with(cx, |workspace, cx| {
            assert!(
                workspace
                    .left_dock()
                    .read(cx)
                    .panel_index_for_id(panel.entity_id())
                    .is_some(),
                "the panel stays in the left dock"
            );
            assert!(workspace.notification_ids().is_empty());
        });
        assert!(!workspace.read_with(cx, |workspace, _| workspace.panel_drag_active()));
    }

    #[gpui::test]
    async fn test_no_slot_for_an_empty_group_that_refuses_the_panel(cx: &mut TestAppContext) {
        let (workspace, cx) = new_workspace(cx).await;
        let panel = add_test_panel(&workspace, DockPosition::Left, cx);
        panel.update(cx, |panel, _| {
            panel.valid_positions = Some(vec![DockPosition::Left, DockPosition::Right]);
        });

        let source = cx.debug_bounds("PANEL-BUTTON-TestPanel").unwrap();
        mouse_down(source.center(), cx);
        mouse_move(source.center() + point(px(6.), px(6.)), cx);
        mouse_move(source.center() + point(px(8.), px(8.)), cx);

        assert!(workspace.read_with(cx, |workspace, _| workspace.panel_drag_active()));
        assert!(
            cx.debug_bounds(group_selector(DockPosition::Right))
                .is_some(),
            "the empty right group accepts the panel: it gets a slot"
        );
        assert!(
            cx.debug_bounds(group_selector(DockPosition::Bottom))
                .is_none(),
            "the empty bottom group refuses the panel: no slot"
        );

        mouse_up(source.center() + point(px(8.), px(8.)), cx);
    }

    #[gpui::test]
    async fn test_manage_menu_sections(cx: &mut TestAppContext) {
        let (_workspace, cx) = new_workspace(cx).await;
        cx.update(|_, cx| {
            register_manage_menu_section(cx, |menu, _, _| menu.label("First Section"));
            register_manage_menu_section(cx, |menu, _, _| menu.label("Second Section"));
        });
        let sections = cx.update(|_, cx| cx.global::<ManageMenuSections>().0.len());
        assert_eq!(sections, 2);
        // Building the menu runs every section.
        cx.update(|window, cx| {
            manage_menu(window, cx);
        });
    }
}

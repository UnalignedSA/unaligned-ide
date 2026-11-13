//! Layout presets, Zen mode and "Reset layout".
//!
//! A preset is a set of dock positions, which are user settings: the one
//! definition of "Classic" and "AI first" is `agent_settings::PanelLayout`.
//! This module applies a preset to a window (which docks are open is per
//! window, not a setting), toggles Zen mode, and resets the layout.

use agent_settings::{AgentSettings, WindowLayout};
use futures::channel::oneshot;
use gpui::{App, Context, Window, actions};
use settings::Settings as _;

use crate::{
    Event, Toast, ToggleCenteredLayout, Workspace,
    dock::{DockPosition, PanelSizeState},
    notifications::NotificationId,
};

actions!(
    workspace,
    [
        /// Puts every panel back in its default dock at its default size, and
        /// the window back in its default arrangement.
        ResetLayout,
        /// Enters or leaves Zen mode: only the editor, centered.
        ToggleZenMode,
    ]
);

/// `Panel::persistent_name` of the Explorer, the panel the left dock shows by
/// default.
const EXPLORER_PANEL: &str = "Project Panel";
/// `Panel::persistent_name` of the chat.
const CHAT_PANEL: &str = "AgentPanel";

/// Registers the layout actions. Called from `workspace::init`.
pub(crate) fn init(cx: &mut App) {
    cx.observe_new(|workspace: &mut Workspace, _window, _cx| {
        workspace.register_action(|workspace, _: &ToggleZenMode, window, cx| {
            workspace.toggle_zen_mode(window, cx);
        });
        workspace.register_action(|workspace, _: &ResetLayout, window, cx| {
            workspace.reset_layout(window, cx);
        });
    })
    .detach();
}

/// What Zen mode needs to remember in order to put the window back as it was.
/// `Workspace::zen` holds one while Zen mode is on.
#[derive(Default)]
pub(crate) struct ZenState {
    /// The docks that were open when Zen mode was entered.
    open_docks: Vec<DockPosition>,
    /// Whether entering Zen mode is what turned the centered layout on.
    turned_on_centered_layout: bool,
}

struct LayoutChangeFailed;

impl Workspace {
    /// Enters Zen mode, or leaves it and puts the window back as it was.
    ///
    /// Zen mode belongs to the window and is kept in memory only: no setting is
    /// written. While it is on the workspace draws neither the activity bar nor
    /// the status bar.
    pub fn toggle_zen_mode(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.zen.is_some() {
            self.leave_zen_mode(window, cx);
        } else {
            self.enter_zen_mode(window, cx);
        }
    }

    fn enter_zen_mode(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.zen.is_some() {
            return;
        }

        let open_docks = self
            .all_docks()
            .into_iter()
            .filter_map(|dock| {
                let dock = dock.read(cx);
                dock.is_open().then(|| dock.position())
            })
            .collect();
        let turned_on_centered_layout = !self.centered_layout;

        self.close_all_docks(window, cx);
        if turned_on_centered_layout {
            self.toggle_centered_layout(&ToggleCenteredLayout, window, cx);
        }
        self.zen = Some(ZenState {
            open_docks,
            turned_on_centered_layout,
        });
        cx.notify();
    }

    /// Leaves Zen mode: the docks that were open when it started are opened
    /// again, and the centered layout is turned off if Zen mode turned it on.
    /// Does nothing when Zen mode is off.
    fn leave_zen_mode(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some(state) = self.zen.take() else {
            return;
        };

        for position in state.open_docks {
            self.open_dock_without_focus(position, window, cx);
        }
        if state.turned_on_centered_layout && self.centered_layout {
            self.toggle_centered_layout(&ToggleCenteredLayout, window, cx);
        }
        cx.notify();
        self.serialize_workspace(window, cx);
    }

    /// Switches the window to a layout preset: writes the dock positions to the
    /// user's settings and, once the panels have moved, opens the docks the
    /// preset is about. Classic: the left dock on the Explorer. AI first: the
    /// left dock on the chat and the right dock on the Explorer.
    pub fn apply_layout(
        &mut self,
        layout: WindowLayout,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let panels_to_show: &'static [&'static str] = match &layout {
            WindowLayout::Editor(_) => &[EXPLORER_PANEL],
            WindowLayout::Agent(_) => &[CHAT_PANEL, EXPLORER_PANEL],
            WindowLayout::Custom(_) => &[],
        };

        // A preset is something to look at: Zen mode would hide the docks it
        // opens, and a maximized panel or pane would cover them.
        self.leave_zen_mode(window, cx);
        self.clear_zoom_for_layout_change(window, cx);

        let written = AgentSettings::set_layout(layout, self.app_state().fs.clone(), cx);
        self.after_layout_settings_written(written, window, cx, move |workspace, window, cx| {
            for persistent_name in panels_to_show {
                workspace.show_panel_named(persistent_name, window, cx);
            }
        });
    }

    /// Undoes every rearrangement: the user's layout settings are removed in
    /// one update of the settings file (dock positions, bottom dock alignment,
    /// flexible widths), then this window gets its default sizes back, nothing
    /// is maximized, Zen mode and the centered layout are off, and the left
    /// dock is open on the Explorer.
    pub fn reset_layout(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.leave_zen_mode(window, cx);
        self.clear_zoom_for_layout_change(window, cx);
        if self.maximized_pane.take().is_some() {
            cx.notify();
        }
        if self.centered_layout {
            self.toggle_centered_layout(&ToggleCenteredLayout, window, cx);
        }

        let written = AgentSettings::reset_layout(self.app_state().fs.clone(), cx);
        self.after_layout_settings_written(written, window, cx, |workspace, window, cx| {
            // The panels are back in their default docks by now.
            if !workspace.show_panel_named(EXPLORER_PANEL, window, cx) {
                workspace.open_dock_without_focus(DockPosition::Left, window, cx);
            }
            // Every panel of every dock, shown or not: a dock's own
            // `reset_panel_sizes` only reaches the panel it shows (and, for the
            // docks named by `resize_all_panels_in_dock`, the panels sized the
            // same way), so a panel resized and then hidden kept its size.
            for dock in workspace.all_docks() {
                let panels = dock.read(cx).panels().cloned().collect::<Vec<_>>();
                for panel in panels {
                    dock.update(cx, |dock, cx| {
                        dock.set_panel_size_state(panel.as_ref(), PanelSizeState::default(), cx);
                    });
                    workspace.persist_panel_size_state(
                        panel.panel_key(),
                        PanelSizeState::default(),
                        cx,
                    );
                }
            }
        });
    }

    /// Runs `then` once a settings update has been applied. The settings store
    /// applies the new text itself after writing the file, so by then every
    /// panel is in the dock its setting names. A failed update (a settings file
    /// that does not parse, a read-only file) is reported and `then` is skipped.
    fn after_layout_settings_written(
        &mut self,
        written: oneshot::Receiver<anyhow::Result<()>>,
        window: &mut Window,
        cx: &mut Context<Self>,
        then: impl FnOnce(&mut Self, &mut Window, &mut Context<Self>) + 'static,
    ) {
        cx.spawn_in(window, async move |workspace, cx| {
            let result = written.await;
            workspace
                .update_in(cx, |workspace, window, cx| match result {
                    Ok(Ok(())) => {
                        then(workspace, window, cx);
                        cx.notify();
                        workspace.serialize_workspace(window, cx);
                    }
                    Ok(Err(_)) | Err(_) => {
                        workspace.show_toast(
                            Toast::new(
                                NotificationId::unique::<LayoutChangeFailed>(),
                                "Could not change the layout: check settings.json for errors.",
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

    /// Opens the dock that holds the panel with this persistent name and shows
    /// that panel in it, without moving the keyboard focus. Returns false when
    /// no such panel exists or when it is turned off (the chat, when AI is
    /// disabled).
    fn show_panel_named(
        &mut self,
        persistent_name: &str,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> bool {
        if persistent_name == CHAT_PANEL
            && AgentSettings::try_get(cx).is_some_and(|settings| !settings.enabled(cx))
        {
            return false;
        }

        let Some((dock, panel_index)) = self.all_docks().into_iter().find_map(|dock| {
            dock.read(cx)
                .panel_index_for_persistent_name(persistent_name, cx)
                .map(|panel_index| (dock.clone(), panel_index))
        }) else {
            return false;
        };

        dock.update(cx, |dock, cx| {
            dock.activate_panel(panel_index, window, cx);
            dock.set_open(true, window, cx);
        });
        cx.notify();
        true
    }

    /// Opens a dock on the panel it last showed, or on its first panel, without
    /// moving the keyboard focus. A dock with no panel stays closed.
    fn open_dock_without_focus(
        &mut self,
        position: DockPosition,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let dock = self.dock_at_position(position).clone();
        dock.update(cx, |dock, cx| {
            if dock.active_panel().is_none() {
                let Ok(panel_index) = dock.first_enabled_panel_idx(cx) else {
                    return;
                };
                dock.activate_panel(panel_index, window, cx);
            }
            dock.set_open(true, window, cx);
        });
        cx.notify();
    }

    /// Un-maximizes every panel and pane: a maximized one would cover the docks
    /// a layout change is about.
    fn clear_zoom_for_layout_change(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        for dock in self.all_docks() {
            dock.update(cx, |dock, cx| {
                dock.zoom_out(window, cx);
                cx.notify();
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
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{
        MultiWorkspace,
        dock::{PanelEvent, test::TestPanel},
    };
    use fs::{FakeFs, Fs as _};
    use gpui::{AppContext as _, Entity, Focusable as _, TestAppContext, VisualTestContext, px};
    use project::Project;
    use settings::SettingsStore;
    use std::sync::Arc;

    fn init_test(cx: &mut TestAppContext) {
        cx.update(|cx| {
            let settings_store = SettingsStore::test(cx);
            cx.set_global(settings_store);
            cx.set_global(db::AppDatabase::test_new());
            theme_settings::init(theme::LoadThemes::JustBase, cx);
            // The layout actions are registered on every new workspace.
            super::init(cx);
        });
    }

    async fn new_workspace(
        cx: &mut TestAppContext,
    ) -> (Entity<Workspace>, Arc<FakeFs>, &mut VisualTestContext) {
        init_test(cx);
        let fs = FakeFs::new(cx.executor());
        let project = Project::test(fs.clone(), [], cx).await;
        // The window root is what attaches the workspace's actions.
        let (multi_workspace, cx) =
            cx.add_window_view(|window, cx| MultiWorkspace::test_new(project, window, cx));
        let workspace =
            multi_workspace.read_with(cx, |multi_workspace, _| multi_workspace.workspace().clone());
        cx.run_until_parked();
        (workspace, fs, cx)
    }

    fn add_panel(
        workspace: &Entity<Workspace>,
        position: DockPosition,
        cx: &mut VisualTestContext,
    ) -> Entity<TestPanel> {
        workspace.update_in(cx, |workspace, window, cx| {
            let panel = cx.new(|cx| TestPanel::new(position, 100, cx));
            workspace.add_panel(panel.clone(), window, cx);
            panel
        })
    }

    fn set_dock_open(
        workspace: &Entity<Workspace>,
        position: DockPosition,
        open: bool,
        cx: &mut VisualTestContext,
    ) {
        workspace.update_in(cx, |workspace, window, cx| {
            let dock = workspace.dock_at_position(position).clone();
            dock.update(cx, |dock, cx| {
                if dock.active_panel().is_none() {
                    dock.activate_panel(0, window, cx);
                }
                dock.set_open(open, window, cx);
            });
        });
    }

    /// Open docks, in the order left, bottom, right.
    fn open_docks(workspace: &Entity<Workspace>, cx: &mut VisualTestContext) -> Vec<DockPosition> {
        workspace.read_with(cx, |workspace, cx| {
            workspace
                .all_docks()
                .into_iter()
                .filter(|dock| dock.read(cx).is_open())
                .map(|dock| dock.read(cx).position())
                .collect()
        })
    }

    /// The text of the user's settings file, once something has written it.
    /// (This crate cannot name the file: it is found by its name.)
    async fn settings_file_text(fs: &FakeFs) -> Option<String> {
        let path = fs
            .files()
            .into_iter()
            .find(|path| path.ends_with("settings.json"))?;
        fs.load(&path).await.ok()
    }

    fn toggle_zen(workspace: &Entity<Workspace>, cx: &mut VisualTestContext) {
        workspace.update_in(cx, |workspace, window, cx| {
            workspace.toggle_zen_mode(window, cx);
        });
        cx.run_until_parked();
    }

    #[gpui::test]
    async fn test_zen_mode_remembers_and_restores_open_docks(cx: &mut TestAppContext) {
        let (workspace, _fs, cx) = new_workspace(cx).await;
        let left = add_panel(&workspace, DockPosition::Left, cx);
        let right = add_panel(&workspace, DockPosition::Right, cx);
        let bottom = add_panel(&workspace, DockPosition::Bottom, cx);
        set_dock_open(&workspace, DockPosition::Left, true, cx);
        set_dock_open(&workspace, DockPosition::Bottom, true, cx);
        set_dock_open(&workspace, DockPosition::Right, false, cx);
        cx.run_until_parked();
        assert_eq!(
            open_docks(&workspace, cx),
            vec![DockPosition::Left, DockPosition::Bottom]
        );
        workspace.read_with(cx, |workspace, _| {
            assert!(!workspace.zen_mode());
            assert!(!workspace.centered_layout);
        });
        let right_history_before = right.read_with(cx, |panel, _| panel.activation_history.clone());

        toggle_zen(&workspace, cx);
        workspace.read_with(cx, |workspace, _| {
            assert!(workspace.zen_mode());
            assert!(
                workspace.centered_layout,
                "Zen mode turns the centered layout on"
            );
        });
        assert_eq!(open_docks(&workspace, cx), vec![]);
        assert!(!left.read_with(cx, |panel, _| panel.active));
        assert!(!bottom.read_with(cx, |panel, _| panel.active));

        toggle_zen(&workspace, cx);
        workspace.read_with(cx, |workspace, _| {
            assert!(!workspace.zen_mode());
            assert!(
                !workspace.centered_layout,
                "the centered layout Zen mode turned on is turned off again"
            );
        });
        assert_eq!(
            open_docks(&workspace, cx),
            vec![DockPosition::Left, DockPosition::Bottom],
            "exactly the docks that were open come back"
        );
        assert!(left.read_with(cx, |panel, _| panel.active));
        assert!(bottom.read_with(cx, |panel, _| panel.active));
        // The right dock was closed before and after: its panel was never
        // told anything by Zen mode.
        assert_eq!(
            right.read_with(cx, |panel, _| panel.activation_history.clone()),
            right_history_before
        );
    }

    #[gpui::test]
    async fn test_zen_mode_keeps_a_centered_layout_that_was_already_on(cx: &mut TestAppContext) {
        let (workspace, _fs, cx) = new_workspace(cx).await;
        add_panel(&workspace, DockPosition::Right, cx);
        set_dock_open(&workspace, DockPosition::Right, true, cx);
        workspace.update_in(cx, |workspace, window, cx| {
            workspace.toggle_centered_layout(&ToggleCenteredLayout, window, cx);
        });
        cx.run_until_parked();

        toggle_zen(&workspace, cx);
        workspace.read_with(cx, |workspace, _| {
            assert!(workspace.zen_mode());
            assert!(workspace.centered_layout);
        });
        assert_eq!(open_docks(&workspace, cx), vec![]);

        toggle_zen(&workspace, cx);
        workspace.read_with(cx, |workspace, _| {
            assert!(!workspace.zen_mode());
            assert!(
                workspace.centered_layout,
                "the user's own centered layout is left on"
            );
        });
        assert_eq!(open_docks(&workspace, cx), vec![DockPosition::Right]);
    }

    #[gpui::test]
    async fn test_zen_mode_respects_what_the_user_does_meanwhile(cx: &mut TestAppContext) {
        let (workspace, _fs, cx) = new_workspace(cx).await;
        add_panel(&workspace, DockPosition::Left, cx);
        add_panel(&workspace, DockPosition::Bottom, cx);
        set_dock_open(&workspace, DockPosition::Left, true, cx);
        cx.run_until_parked();

        toggle_zen(&workspace, cx);
        // In Zen mode the user opens the terminal's dock and turns the
        // centered layout off by hand.
        set_dock_open(&workspace, DockPosition::Bottom, true, cx);
        workspace.update_in(cx, |workspace, window, cx| {
            workspace.toggle_centered_layout(&ToggleCenteredLayout, window, cx);
            assert!(!workspace.centered_layout);
        });
        cx.run_until_parked();

        toggle_zen(&workspace, cx);
        workspace.read_with(cx, |workspace, _| {
            assert!(!workspace.zen_mode());
            assert!(
                !workspace.centered_layout,
                "leaving does not toggle the centered layout back on"
            );
        });
        assert_eq!(
            open_docks(&workspace, cx),
            vec![DockPosition::Left, DockPosition::Bottom]
        );
    }

    #[gpui::test]
    async fn test_zen_mode_action_and_escape(cx: &mut TestAppContext) {
        let (workspace, fs, cx) = new_workspace(cx).await;
        add_panel(&workspace, DockPosition::Left, cx);
        set_dock_open(&workspace, DockPosition::Left, true, cx);
        cx.run_until_parked();
        let settings_before = settings_file_text(&fs).await;

        cx.dispatch_action(ToggleZenMode);
        cx.run_until_parked();
        assert!(workspace.read_with(cx, |workspace, _| workspace.zen_mode()));
        assert_eq!(open_docks(&workspace, cx), vec![]);

        // Escape does not leave Zen mode.
        cx.simulate_keystrokes("escape");
        cx.dispatch_action(menu::Cancel);
        cx.run_until_parked();
        assert!(workspace.read_with(cx, |workspace, _| workspace.zen_mode()));

        cx.dispatch_action(ToggleZenMode);
        cx.run_until_parked();
        assert!(!workspace.read_with(cx, |workspace, _| workspace.zen_mode()));
        assert_eq!(open_docks(&workspace, cx), vec![DockPosition::Left]);

        // Zen mode writes no settings.
        assert_eq!(settings_file_text(&fs).await, settings_before);
        assert_eq!(settings_before, None, "no test step wrote the settings");
    }

    #[gpui::test]
    async fn test_reset_layout(cx: &mut TestAppContext) {
        let (workspace, fs, cx) = new_workspace(cx).await;
        // The user's settings: an "AI first"-like layout with a side terminal,
        // a full-width bottom dock and one unrelated key.
        cx.update(|_, cx| {
            settings::update_settings_file_with_completion(fs.clone(), cx, |settings, _| {
                let agent = settings.agent.get_or_insert_default();
                agent.dock = Some(settings::DockPosition::Left);
                agent.flexible = Some(true);
                agent.default_width = Some(520.0.into());
                settings.project_panel.get_or_insert_default().dock =
                    Some(settings::DockSide::Right);
                settings.terminal.get_or_insert_default().dock =
                    Some(settings::TerminalDockPosition::Right);
                settings.debugger.get_or_insert_default().dock = Some(settings::DockPosition::Left);
                settings.workspace.bottom_dock_layout = Some(settings::BottomDockLayout::Full);
            })
        })
        .await
        .expect("the update was not dropped")
        .expect("the settings file was written");
        let before = settings_file_text(&fs).await.expect("a settings file");
        assert_eq!(before.matches("\"dock\"").count(), 4, "{before}");
        assert!(before.contains("bottom_dock_layout"), "{before}");

        let left = add_panel(&workspace, DockPosition::Left, cx);
        let bottom = add_panel(&workspace, DockPosition::Bottom, cx);
        // Two panels in the right dock: the first is the one it shows, the
        // second was resized earlier and is hidden behind it.
        let right_shown = add_panel(&workspace, DockPosition::Right, cx);
        // (Two panels of one dock need different activation priorities.)
        let right_hidden = workspace.update_in(cx, |workspace, window, cx| {
            let panel = cx.new(|cx| TestPanel::new(DockPosition::Right, 101, cx));
            workspace.add_panel(panel.clone(), window, cx);
            panel
        });
        set_dock_open(&workspace, DockPosition::Left, false, cx);
        set_dock_open(&workspace, DockPosition::Bottom, true, cx);
        workspace.update_in(cx, |workspace, window, cx| {
            let dock = workspace.right_dock().clone();
            dock.update(cx, |dock, cx| {
                let shown_ix = dock
                    .panel_index_for_id(right_shown.entity_id())
                    .expect("the panel is in the right dock");
                dock.activate_panel(shown_ix, window, cx);
                assert!(dock.set_panel_size_state(
                    &right_hidden,
                    PanelSizeState {
                        size: Some(px(640.)),
                        flex: None,
                    },
                    cx,
                ));
            });
        });

        // The user resized both panels, maximized the bottom one, then entered
        // Zen mode (which also turns the centered layout on).
        workspace.update_in(cx, |workspace, window, cx| {
            for position in [DockPosition::Left, DockPosition::Bottom] {
                let dock = workspace.dock_at_position(position).clone();
                dock.update(cx, |dock, cx| {
                    dock.resize_panel_sizes(Some(px(512.)), None, window, cx);
                });
            }
        });
        bottom.update(cx, |_, cx| cx.emit(PanelEvent::ZoomIn));
        cx.run_until_parked();
        workspace.read_with(cx, |workspace, cx| {
            assert!(bottom.read(cx).zoomed);
            assert_eq!(workspace.zoomed_position, Some(DockPosition::Bottom));
            assert_eq!(
                workspace
                    .left_dock()
                    .read(cx)
                    .stored_panel_size_state(&left)
                    .and_then(|state| state.size),
                Some(px(512.))
            );
        });
        toggle_zen(&workspace, cx);
        workspace.read_with(cx, |workspace, _| {
            assert!(workspace.zen_mode());
            assert!(workspace.centered_layout);
        });

        cx.dispatch_action(ResetLayout);
        cx.run_until_parked();

        // One write removed the layout keys and kept the rest.
        let written = settings_file_text(&fs).await.expect("a settings file");
        assert!(!written.contains("\"dock\""), "{written}");
        assert!(!written.contains("\"flexible\""), "{written}");
        assert!(!written.contains("bottom_dock_layout"), "{written}");
        assert!(written.contains("\"default_width\": 520"), "{written}");

        workspace.update_in(cx, |workspace, window, cx| {
            assert!(!workspace.zen_mode(), "Reset leaves Zen mode");
            assert!(!workspace.centered_layout, "Reset turns centering off");
            assert!(workspace.zoomed.is_none(), "Reset clears zoom");
            assert_eq!(workspace.zoomed_position, None);
            assert!(!bottom.read(cx).zoomed);

            let left_dock = workspace.left_dock().read(cx);
            assert!(left_dock.is_open(), "Reset opens the left dock");
            assert_eq!(
                left_dock.visible_panel().map(|panel| panel.panel_id()),
                Some(left.entity_id())
            );
            // Every panel stores "no size", which is its default size.
            assert_eq!(
                left_dock.stored_panel_size(&left, window, cx),
                Some(left.read(cx).default_size),
                "the left panel is back at its default size"
            );
            assert_eq!(
                left_dock.stored_panel_size_state(&left),
                Some(PanelSizeState::default())
            );
            let right_dock = workspace.right_dock().read(cx);
            assert_eq!(
                right_dock.stored_panel_size_state(&right_hidden),
                Some(PanelSizeState::default()),
                "a resized panel that is not shown is reset as well"
            );
            assert_eq!(
                right_dock.stored_panel_size_state(&right_shown),
                Some(PanelSizeState::default())
            );
            assert_eq!(
                workspace
                    .bottom_dock()
                    .read(cx)
                    .stored_panel_size_state(&bottom),
                Some(PanelSizeState::default()),
                "the bottom panel is back at its default size"
            );
        });
        // The bottom dock was open before Zen mode: leaving Zen brings it back.
        assert_eq!(
            open_docks(&workspace, cx),
            vec![DockPosition::Left, DockPosition::Bottom]
        );
    }

    #[gpui::test]
    async fn test_apply_layout_writes_the_preset_and_leaves_zen(cx: &mut TestAppContext) {
        let (workspace, fs, cx) = new_workspace(cx).await;
        let left = add_panel(&workspace, DockPosition::Left, cx);
        set_dock_open(&workspace, DockPosition::Left, true, cx);
        left.update(cx, |_, cx| cx.emit(PanelEvent::ZoomIn));
        cx.run_until_parked();
        toggle_zen(&workspace, cx);

        workspace.update_in(cx, |workspace, window, cx| {
            workspace.apply_layout(WindowLayout::agent(), window, cx);
        });
        cx.run_until_parked();

        let written = settings_file_text(&fs).await.expect("a settings file");
        let value: serde_json::Value =
            settings::parse_json_with_comments(&written).expect("settings are valid");
        let dock_of = |section: &str| {
            value
                .get(section)
                .and_then(|section| section.get("dock"))
                .and_then(|dock| dock.as_str())
        };
        assert_eq!(dock_of("agent"), Some("left"), "{written}");
        assert_eq!(dock_of("project_panel"), Some("right"), "{written}");
        assert_eq!(dock_of("outline_panel"), Some("right"), "{written}");
        assert_eq!(dock_of("git_panel"), Some("right"), "{written}");
        assert_eq!(dock_of("collaboration_panel"), Some("right"), "{written}");
        // Already at the bottom by default: nothing to write.
        assert_eq!(dock_of("terminal"), None, "{written}");
        assert_eq!(dock_of("debugger"), None, "{written}");
        cx.update(|_, cx| {
            assert!(matches!(
                AgentSettings::get_layout(cx),
                WindowLayout::Agent(_)
            ));
        });

        workspace.read_with(cx, |workspace, cx| {
            assert!(!workspace.zen_mode(), "a preset leaves Zen mode");
            assert!(workspace.zoomed.is_none(), "a preset un-maximizes");
            assert!(!left.read(cx).zoomed);
        });
        assert_eq!(open_docks(&workspace, cx), vec![DockPosition::Left]);

        // Back to Classic: the keys that differ from the merged settings are
        // written, which is the five side panels.
        workspace.update_in(cx, |workspace, window, cx| {
            workspace.apply_layout(WindowLayout::editor(), window, cx);
        });
        cx.run_until_parked();
        cx.update(|_, cx| {
            assert!(matches!(
                AgentSettings::get_layout(cx),
                WindowLayout::Editor(_)
            ));
        });
    }

    #[gpui::test]
    async fn test_show_panel_named_opens_the_dock_without_taking_focus(cx: &mut TestAppContext) {
        let (workspace, _fs, cx) = new_workspace(cx).await;
        let panel = add_panel(&workspace, DockPosition::Right, cx);
        cx.run_until_parked();
        assert_eq!(open_docks(&workspace, cx), vec![]);

        let (shown, missing) = workspace.update_in(cx, |workspace, window, cx| {
            (
                workspace.show_panel_named("TestPanel", window, cx),
                workspace.show_panel_named(EXPLORER_PANEL, window, cx),
            )
        });
        cx.run_until_parked();
        assert!(shown);
        assert!(!missing, "there is no Explorer in this test window");
        assert_eq!(open_docks(&workspace, cx), vec![DockPosition::Right]);
        workspace.update_in(cx, |workspace, window, cx| {
            let dock = workspace.right_dock().read(cx);
            assert_eq!(
                dock.visible_panel().map(|panel| panel.panel_id()),
                Some(panel.entity_id())
            );
            assert!(!panel.read(cx).focus_handle(cx).is_focused(window));
        });
    }
}

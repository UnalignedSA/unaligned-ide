use gpui::{
    Action as _, App, Context, Empty, Entity, EventEmitter, IntoElement, ParentElement, Render,
    SharedString, Subscription, Window,
};
use project::Project;
use settings::Settings;
use ui::{Tooltip, prelude::*};
use util::paths::PathStyle;

use crate::{
    HideStatusItem, StatusItemView, Workspace, item::ItemHandle,
    workspace_settings::StatusBarSettings,
};

// The shared building blocks of status bar items. They are defined in the
// private `status_bar` module and exposed here for the item views that live in
// other crates.
pub use crate::status_bar::{
    status_bar_button, status_bar_icon, status_bar_icon_button, status_bar_item,
    status_bar_item_content, status_bar_item_height, status_bar_item_height_for, status_bar_label,
    status_bar_message, status_bar_text, status_bar_text_in, status_bar_value,
};

/// The status bar item that shows the checked out branch of the active repository.
pub struct ActiveBranch {
    project: Entity<Project>,
    _git_store_subscription: Subscription,
}

impl ActiveBranch {
    pub fn new(workspace: &Workspace, cx: &mut Context<Self>) -> Self {
        let project = workspace.project().clone();
        let git_store = project.read(cx).git_store().clone();
        Self {
            project,
            _git_store_subscription: cx.subscribe(&git_store, |_, _, _, cx| cx.notify()),
        }
    }
}

impl Render for ActiveBranch {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let Some(repository) = self.project.read(cx).active_repository(cx) else {
            return Empty.into_any_element();
        };
        let repository = repository.read(cx);
        // A detached HEAD has no branch: show the short commit hash instead.
        let name: SharedString = match repository.branch.as_ref() {
            Some(branch) => branch.name().to_string().into(),
            None => match repository.head_commit.as_ref() {
                Some(commit) => commit.sha.chars().take(7).collect::<String>().into(),
                None => return Empty.into_any_element(),
            },
        };
        // Commits not pushed yet and commits not pulled yet, when the branch
        // follows a remote one and they are not both zero.
        let sync = repository
            .branch
            .as_ref()
            .and_then(|branch| branch.tracking_status())
            .filter(|status| status.ahead > 0 || status.behind > 0);
        let sync_text = sync.map(|status| sync_text(status.ahead, status.behind));
        let aria_label = match sync {
            Some(status) => format!(
                "Git branch: {name}, {} to push, {} to pull",
                status.ahead, status.behind
            ),
            None => format!("Git branch: {name}"),
        };
        // A branch name is data, not a label: the bar and the tooltip keep its real case.
        let tooltip_meta = match sync {
            Some(status) => format!(
                "{name} · {} to push, {} to pull",
                status.ahead, status.behind
            ),
            None => name.to_string(),
        };

        status_bar_item("active-branch", cx)
            .tab_index(0isize)
            .aria_label(aria_label)
            .child(
                status_bar_item_content()
                    .child(status_bar_icon(IconName::GitBranch))
                    .child(
                        status_bar_message(name, cx)
                            .flex_initial()
                            .max_w(rems_from_px(240_f32))
                            .truncate(),
                    )
                    .children(sync_text.map(|text| status_bar_text(text, cx))),
            )
            .tooltip(move |_window, cx| {
                Tooltip::with_meta(
                    "Switch Branch",
                    Some(&app_actions::git::Branch),
                    tooltip_meta.clone(),
                    cx,
                )
            })
            .on_click(|_, window, cx| {
                window.dispatch_action(app_actions::git::Branch.boxed_clone(), cx);
            })
            .into_any_element()
    }
}

/// "↑2 ↓1": commits to push, then commits to pull, each only when not zero.
fn sync_text(ahead: u32, behind: u32) -> String {
    let mut parts = Vec::new();
    if ahead > 0 {
        parts.push(format!("↑{ahead}"));
    }
    if behind > 0 {
        parts.push(format!("↓{behind}"));
    }
    parts.join(" ")
}

impl StatusItemView for ActiveBranch {
    fn set_active_pane_item(
        &mut self,
        _active_pane_item: Option<&dyn ItemHandle>,
        _window: &mut Window,
        _cx: &mut Context<Self>,
    ) {
        // The branch follows the active repository, not the active item.
    }

    fn hide_setting(&self, _: &App) -> Option<HideStatusItem> {
        // Only visible inside a git repository; there is no setting for it yet.
        None
    }
}

pub struct ActiveFileName {
    project_path: Option<SharedString>,
    full_path: Option<SharedString>,
}

impl ActiveFileName {
    pub fn new() -> Self {
        Self {
            project_path: None,
            full_path: None,
        }
    }
}

impl Render for ActiveFileName {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        if !StatusBarSettings::get_global(cx).show_active_file {
            return Empty.into_any_element();
        }

        let Some(project_path) = self.project_path.clone() else {
            return Empty.into_any_element();
        };

        let tooltip_text = self
            .full_path
            .clone()
            .unwrap_or_else(|| project_path.clone());

        status_bar_button("active-file-name-button")
            .aria_label(project_path.clone())
            .child(status_bar_label(project_path))
            .tooltip(Tooltip::text(tooltip_text))
            .into_any_element()
    }
}

impl EventEmitter<crate::ToolbarItemEvent> for ActiveFileName {}

impl StatusItemView for ActiveFileName {
    fn set_active_pane_item(
        &mut self,
        active_pane_item: Option<&dyn ItemHandle>,
        _window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if let Some(item) = active_pane_item {
            self.project_path = item
                .project_path(cx)
                .map(|path| path.path.display(PathStyle::local()).into_owned().into());
            self.full_path = item.tab_tooltip_text(cx);
        } else {
            self.project_path = None;
            self.full_path = None;
        }
        cx.notify();
    }

    fn hide_setting(&self, _: &App) -> Option<HideStatusItem> {
        Some(HideStatusItem::new(|settings| {
            settings.status_bar.get_or_insert_default().show_active_file = Some(false);
        }))
    }
}

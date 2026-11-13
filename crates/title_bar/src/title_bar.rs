mod application_menu;
mod centered_row;
pub mod collab;
mod onboarding_banner;
mod plan_chip;
mod update_version;

use crate::application_menu::{ApplicationMenu, show_menus};
use crate::centered_row::CenteredRow;
use crate::plan_chip::PlanChip;
use agent_settings::{AgentSettings, WindowLayout};
use path::PathStyle;
pub use platform_title_bar::{
    self, DraggedWindowTab, MergeAllWindows, MoveTabToNewWindow, PlatformTitleBar,
    ShowNextWindowTab, ShowPreviousWindowTab,
};
use platform_title_bar::{SystemWindowTabs, TitleBarSettings};
use project::{linked_worktree_short_name, repo_identity_path};

#[cfg(not(target_os = "macos"))]
use crate::application_menu::{
    ActivateDirection, ActivateMenuLeft, ActivateMenuRight, OpenApplicationMenu,
};

use auto_update::AutoUpdateStatus;
use call::ActiveCall;
use client::{Client, UserStore, zed_urls};
use command_palette_hooks::CommandPaletteFilter;

use gpui::{
    Action, Anchor, Animation, AnimationExt, AnyElement, AnyView, App, Context, Element, Entity,
    Focusable, InteractiveElement, IntoElement, MouseButton, ParentElement, Render,
    StatefulInteractiveElement, Styled, Subscription, TaskExt, WeakEntity, Window, actions, div,
    pulsating_between,
};
use onboarding_banner::OnboardingBanner;
use project::{
    Project, git_store::GitStoreEvent, project_settings::ProjectSettings,
    trusted_worktrees::TrustedWorktrees,
};
use remote::RemoteConnectionOptions;
use settings::{Settings as _, SettingsStore};

use std::any::TypeId;
use std::sync::Arc;
use std::time::Duration;
use theme::ActiveTheme;
use ui::{
    Avatar, ButtonLike, ContextMenu, IconWithIndicator, Indicator, PopoverMenu, PopoverMenuHandle,
    TintColor, Tooltip, metrics, prelude::*, utils::platform_title_bar_height,
};
use update_version::UpdateVersion;
use util::ResultExt;
use workspace::{
    AccessibleMode, MultiWorkspace, ToggleBottomDock, ToggleCenteredLayout, ToggleFileFinder,
    ToggleLeftDock, ToggleRightDock, ToggleWorktreeSecurity, Workspace,
    dock::DockPosition,
    layout_presets::{ResetLayout, ToggleZenMode},
    notifications::{NotifyResultExt, NotifyTaskExt as _},
};

use app_actions::OpenRemote;

pub use onboarding_banner::restore_banner;

// Kept short: the left group must leave room for the centred command center.
const MAX_PROJECT_NAME_LENGTH: usize = 28;
const MAX_BRANCH_NAME_LENGTH: usize = 28;
/// Preferred width of the command center, in pixels at the default rem size.
const COMMAND_CENTER_WIDTH: f32 = 440.;
const COMMAND_CENTER_MIN_WIDTH: f32 = 72.;
/// Space kept between the command center and the groups beside it, in pixels
/// at the default rem size.
const COMMAND_CENTER_GAP: f32 = 8.;
/// Left padding of the second row the bar gets when the menus are shown in
/// full, in rems (`pl_2`).
const MENU_ROW_PADDING: f32 = 0.5;
/// Accounts, calls and collaboration are not part of this product: there is
/// no server to sign in to. Their controls (sign-in, user menu, avatar, plan,
/// onboarding banner, call, screen share, mute, share project, connection
/// state) stay in the code for now and are never drawn, whatever the
/// `title_bar.show_*` settings say.
const ACCOUNT_AND_COLLABORATION_UI: bool = false;
const MAX_SHORT_SHA_LENGTH: usize = 8;

actions!(
    collab,
    [
        /// Toggles the user menu dropdown.
        ToggleUserMenu,
        /// Toggles the project menu dropdown.
        ToggleProjectMenu,
        /// Switches to a different git branch.
        SwitchBranch,
        /// A debug action to simulate an update being available to test the update banner UI.
        SimulateUpdateAvailable
    ]
);

actions!(
    workspace,
    [
        /// Switches to the Classic layout: files on the left, chat on the right,
        /// terminal at the bottom.
        UseClassicLayout,
        /// Switches to the AI-first layout: chat on the left, files on the right,
        /// terminal at the bottom.
        UseAgenticLayout,
    ]
);

pub fn init(cx: &mut App) {
    platform_title_bar::PlatformTitleBar::init(cx);

    update_layout_action_filter(cx);

    cx.observe_global::<SettingsStore>(update_layout_action_filter)
        .detach();

    cx.observe_new(|workspace: &mut Workspace, window, cx| {
        let Some(window) = window else {
            return;
        };
        let window_tabs = cx.new(|_| SystemWindowTabs::new());
        workspace.set_window_tabs_item(AnyView::from(window_tabs), cx);
        apply_title_bar_visibility(workspace, window, cx);

        cx.observe_global_in::<SettingsStore>(window, apply_title_bar_visibility)
            .detach();
        cx.observe_window_bounds(window, apply_title_bar_visibility)
            .detach();

        workspace.register_action(|workspace, _: &UseClassicLayout, window, cx| {
            workspace.apply_layout(WindowLayout::Editor(None), window, cx);
        });

        workspace.register_action(|workspace, _: &UseAgenticLayout, window, cx| {
            workspace.apply_layout(WindowLayout::Agent(None), window, cx);
        });

        workspace.register_action(|workspace, _: &SimulateUpdateAvailable, _window, cx| {
            if let Some(titlebar) = workspace
                .titlebar_item()
                .and_then(|item| item.downcast::<TitleBar>().ok())
            {
                titlebar.update(cx, |titlebar, cx| {
                    titlebar.toggle_update_simulation(cx);
                });
            }
        });

        #[cfg(not(target_os = "macos"))]
        workspace.register_action(|workspace, action: &OpenApplicationMenu, window, cx| {
            if let Some(titlebar) = workspace
                .titlebar_item()
                .and_then(|item| item.downcast::<TitleBar>().ok())
            {
                titlebar.update(cx, |titlebar, cx| {
                    if let Some(ref menu) = titlebar.application_menu {
                        menu.update(cx, |menu, cx| menu.open_menu(action, window, cx));
                    }
                });
            }
        });

        #[cfg(not(target_os = "macos"))]
        workspace.register_action(|workspace, _: &ActivateMenuRight, window, cx| {
            if let Some(titlebar) = workspace
                .titlebar_item()
                .and_then(|item| item.downcast::<TitleBar>().ok())
            {
                titlebar.update(cx, |titlebar, cx| {
                    if let Some(ref menu) = titlebar.application_menu {
                        menu.update(cx, |menu, cx| {
                            menu.navigate_menus_in_direction(ActivateDirection::Right, window, cx)
                        });
                    }
                });
            }
        });

        #[cfg(not(target_os = "macos"))]
        workspace.register_action(|workspace, _: &ActivateMenuLeft, window, cx| {
            if let Some(titlebar) = workspace
                .titlebar_item()
                .and_then(|item| item.downcast::<TitleBar>().ok())
            {
                titlebar.update(cx, |titlebar, cx| {
                    if let Some(ref menu) = titlebar.application_menu {
                        menu.update(cx, |menu, cx| {
                            menu.navigate_menus_in_direction(ActivateDirection::Left, window, cx)
                        });
                    }
                });
            }
        });
    })
    .detach();
}

fn apply_title_bar_visibility(
    workspace: &mut Workspace,
    window: &mut Window,
    cx: &mut Context<Workspace>,
) {
    let visible = TitleBarSettings::is_title_bar_visible(window, cx);

    if !visible {
        workspace.clear_titlebar_item(window, cx);
    } else if workspace.titlebar_item().is_none() {
        let multi_workspace = workspace.multi_workspace().cloned();
        let item = cx.new(|cx| TitleBar::new("title-bar", workspace, multi_workspace, window, cx));
        workspace.set_titlebar_item(AnyView::from(item), window, cx);
    }

    #[cfg(target_os = "macos")]
    window.set_native_titlebar_visibility(visible);
}

/// Hides or shows the panel layout actions in the command palette based on
/// whether AI is currently disabled.
fn update_layout_action_filter(cx: &mut App) {
    let disable_ai = project::DisableAiSettings::get_global(cx).disable_ai;
    let layout_actions = [
        TypeId::of::<UseClassicLayout>(),
        TypeId::of::<UseAgenticLayout>(),
    ];
    CommandPaletteFilter::update_global(cx, |filter, _| {
        if disable_ai {
            filter.hide_action_types(&layout_actions);
        } else {
            filter.show_action_types(layout_actions.iter());
        }
    });
}

pub struct TitleBar {
    platform_titlebar: Entity<PlatformTitleBar>,
    project: Entity<Project>,
    user_store: Entity<UserStore>,
    client: Arc<Client>,
    workspace: WeakEntity<Workspace>,
    multi_workspace: Option<WeakEntity<MultiWorkspace>>,
    application_menu: Option<Entity<ApplicationMenu>>,
    _subscriptions: Vec<Subscription>,
    banner: Option<Entity<OnboardingBanner>>,
    update_version: Entity<UpdateVersion>,
    screen_share_popover_handle: PopoverMenuHandle<ContextMenu>,
    _diagnostics_subscription: Option<gpui::Subscription>,
}

impl Render for TitleBar {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        if self.multi_workspace.is_none() {
            if let Some(mw) = self
                .workspace
                .upgrade()
                .and_then(|ws| ws.read(cx).multi_workspace().cloned())
            {
                self.multi_workspace = Some(mw.clone());
                self.platform_titlebar.update(cx, |titlebar, _cx| {
                    titlebar.set_multi_workspace(mw);
                });
            }
        }

        let title_bar_settings = *TitleBarSettings::get_global(cx);
        let button_layout = title_bar_settings.button_layout;
        let is_git_enabled = ProjectSettings::get_global(cx).git.enabled.status;

        let show_menus = show_menus(cx);

        let mut project_name = None;
        let mut repository = None;
        let mut linked_worktree_name = None;
        if let Some(worktree) = self.effective_active_worktree(cx) {
            let path_style = worktree.read(cx).path_style();
            repository = self.get_repository_for_worktree(&worktree, path_style, cx);
            let worktree_abs_path = worktree.read(cx).abs_path();
            project_name = worktree
                .read(cx)
                .root_name()
                .file_name()
                .map(|name| SharedString::from(name.to_string()));
            if let Some(repo) = &repository {
                let repo = repo.read(cx);
                let identity = repo_identity_path(&repo.common_dir_abs_path, path_style);
                linked_worktree_name = linked_worktree_short_name(
                    repo.linked_worktree_anchor_path(),
                    repo.work_directory_abs_path.as_ref(),
                    path_style,
                )
                .or_else(|| {
                    repo.is_linked_worktree()
                        .then_some(project_name.clone())
                        .flatten()
                });

                let display_name =
                    if path_style.extension(identity) == Some(std::ffi::OsStr::new("git")) {
                        path_style.file_stem(identity)
                    } else {
                        path_style.file_name(identity)
                    };

                if let Some(repo_name) = display_name.and_then(|n| n.to_str()) {
                    let visible_worktrees_in_repo =
                        self.visible_worktrees_in_repository(repo, path_style, cx);
                    let name = if visible_worktrees_in_repo == 1 {
                        if let Ok(relative) = path_style
                            .strip_prefix(&worktree_abs_path, &*repo.work_directory_abs_path)
                        {
                            if relative.as_os_str().is_empty() {
                                repo_name.to_string()
                            } else {
                                format!(
                                    "{}{}{}",
                                    repo_name,
                                    path_style.primary_separator(),
                                    relative.display()
                                )
                            }
                        } else {
                            repo_name.to_string()
                        }
                    } else {
                        repo_name.to_string()
                    };
                    project_name = Some(SharedString::from(name));
                }
            }
        }

        // Three parts placed by `CenteredRow`: the groups keep their content
        // width and the command center sits on the middle of the whole bar,
        // window controls included; it is what shrinks when room is short.
        let project_name_for_search = project_name.clone();

        let left_group = h_flex()
            .h_full()
            .max_w_full()
            .overflow_hidden()
            .gap_0p5()
            .map(|title_bar| {
                let mut render_project_items =
                    title_bar_settings.show_branch_name || title_bar_settings.show_project_items;
                title_bar
                    .when_some(
                        self.application_menu.clone().filter(|_| !show_menus),
                        |title_bar, menu| {
                            // Hide the project/branch items to make room when the
                            // menu bar is expanded -- except in accessible mode,
                            // where the menu bar is always expanded but those
                            // controls must still remain reachable.
                            render_project_items &= !menu
                                .update(cx, |menu, cx| menu.all_menus_shown(cx))
                                || cx.accessible_mode();
                            title_bar.child(menu)
                        },
                    )
                    .children(self.render_restricted_mode(cx))
                    .when(render_project_items, |title_bar| {
                        title_bar
                            .when(title_bar_settings.show_project_items, |title_bar| {
                                title_bar
                                    .children(self.render_project_host(cx))
                                    .child(self.render_project_name(project_name, window, cx))
                            })
                            .when_some(
                                repository.filter(|_| is_git_enabled),
                                |title_bar, repository| {
                                    title_bar.children(self.render_branch_chip(
                                        repository,
                                        linked_worktree_name,
                                        cx,
                                    ))
                                },
                            )
                    })
            })
            .on_mouse_down(MouseButton::Left, |_, _, cx| cx.stop_propagation());

        let command_center = self
            .render_command_center(project_name_for_search, cx)
            .into_any_element();

        let account_and_collaboration = if ACCOUNT_AND_COLLABORATION_UI {
            self.render_account_and_collaboration(window, cx)
        } else {
            Vec::new()
        };

        // The update notice comes and goes on the inner side, so the layout
        // controls keep their place at the end of the bar.
        let right_group = h_flex()
            .h_full()
            .flex_none()
            .pl_2()
            .pr_1()
            .gap_1()
            .on_mouse_down(MouseButton::Left, |_, _, cx| cx.stop_propagation())
            .children(account_and_collaboration)
            .child(self.update_version.clone())
            .child(self.render_layout_toggles(cx));

        let rem_size = window.rem_size();
        let row = |left_inset: Pixels, right_inset: Pixels| {
            CenteredRow::new(left_group, command_center, right_group)
                .insets(left_inset, right_inset)
                .center_width(
                    rems_from_px(COMMAND_CENTER_WIDTH).to_pixels(rem_size),
                    rems_from_px(COMMAND_CENTER_MIN_WIDTH).to_pixels(rem_size),
                )
                .gap(rems_from_px(COMMAND_CENTER_GAP).to_pixels(rem_size))
                .into_any_element()
        };

        if show_menus {
            // The menus fill the bar that holds the window controls; the
            // three parts move to a second row, which has none.
            self.platform_titlebar.update(cx, |this, _| {
                this.set_button_layout(button_layout);
                this.set_children(
                    self.application_menu
                        .clone()
                        .map(|menu| menu.into_any_element()),
                );
            });

            let height = platform_title_bar_height(window);
            let title_bar_color = self.platform_titlebar.update(cx, |platform_titlebar, cx| {
                platform_titlebar.title_bar_color(window, cx)
            });

            v_flex()
                .w_full()
                .child(self.platform_titlebar.clone().into_any_element())
                .child(
                    h_flex()
                        .bg(title_bar_color)
                        .h(height)
                        .pl(rems(MENU_ROW_PADDING))
                        .w_full()
                        .border_b_1()
                        .border_color(cx.theme().colors().border)
                        .child(row(rem_size * MENU_ROW_PADDING, px(0.))),
                )
                .into_any_element()
        } else {
            let (left_inset, right_inset) = self.platform_titlebar.update(cx, |this, cx| {
                this.set_button_layout(button_layout);
                this.content_insets(window, cx)
            });
            let row = row(left_inset, right_inset);
            self.platform_titlebar
                .update(cx, |this, _| this.set_children([row]));
            self.platform_titlebar.clone().into_any_element()
        }
    }
}

impl TitleBar {
    pub fn new(
        id: impl Into<ElementId>,
        workspace: &Workspace,
        multi_workspace: Option<WeakEntity<MultiWorkspace>>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Self {
        let project = workspace.project().clone();
        let git_store = project.read(cx).git_store().clone();
        let user_store = workspace.app_state().user_store.clone();
        let client = workspace.app_state().client.clone();
        let active_call = ActiveCall::global(cx);

        let platform_style = platform_title_bar::platform_style();
        let application_menu = match platform_style {
            PlatformStyle::Mac => {
                if option_env!("IDE_USE_CROSS_PLATFORM_MENU").is_some() {
                    Some(cx.new(|cx| ApplicationMenu::new(window, cx)))
                } else {
                    None
                }
            }
            PlatformStyle::Linux | PlatformStyle::Windows => {
                Some(cx.new(|cx| ApplicationMenu::new(window, cx)))
            }
        };

        let mut subscriptions = Vec::new();
        subscriptions.push(
            cx.observe(&workspace.weak_handle().upgrade().unwrap(), |_, _, cx| {
                cx.notify()
            }),
        );

        subscriptions.push(cx.observe(&active_call, |this, _, cx| this.active_call_changed(cx)));
        subscriptions.push(
            cx.subscribe(&git_store, move |_, _, event, cx| match event {
                GitStoreEvent::ActiveRepositoryChanged(_)
                | GitStoreEvent::RepositoryUpdated(_, _, true) => {
                    cx.notify();
                }
                _ => {}
            }),
        );
        subscriptions.push(cx.observe(&user_store, |_a, _, cx| cx.notify()));
        if let Some(workspace_entity) = workspace.weak_handle().upgrade() {
            subscriptions.push(cx.subscribe(
                &workspace_entity,
                |_, _, event: &workspace::Event, cx| {
                    if matches!(event, workspace::Event::WorktreeCreationChanged) {
                        cx.notify();
                    }
                },
            ));
        }
        subscriptions.push(cx.observe_button_layout_changed(window, |_, _, cx| cx.notify()));
        if let Some(trusted_worktrees) = TrustedWorktrees::try_get_global(cx) {
            subscriptions.push(cx.subscribe(&trusted_worktrees, |_, _, _, cx| {
                cx.notify();
            }));
        }

        let update_version = cx.new(|cx| UpdateVersion::new(cx));
        let platform_titlebar = cx.new(|cx| {
            let mut titlebar = PlatformTitleBar::new(id, cx).without_system_window_tabs();
            if let Some(mw) = multi_workspace.clone() {
                titlebar = titlebar.with_multi_workspace(mw);
            }
            titlebar
        });

        let banner = None;

        let mut this = Self {
            platform_titlebar,
            application_menu,
            workspace: workspace.weak_handle(),
            multi_workspace,
            project,
            user_store,
            client,
            _subscriptions: subscriptions,
            banner,
            update_version,
            screen_share_popover_handle: PopoverMenuHandle::default(),
            _diagnostics_subscription: None,
        };

        this.observe_diagnostics(cx);

        this
    }

    /// The controls of accounts, calls and collaboration, in the order they
    /// had in the right group. Not called: see `ACCOUNT_AND_COLLABORATION_UI`.
    fn render_account_and_collaboration(
        &mut self,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Vec<AnyElement> {
        let title_bar_settings = *TitleBarSettings::get_global(cx);
        let status = self.client.status();
        let status = &*status.borrow();
        let user = self.user_store.read(cx).current_user();
        let is_signing_in = user.is_none()
            && matches!(
                status,
                client::Status::Authenticating
                    | client::Status::Authenticated
                    | client::Status::Connecting
            );
        let is_signed_out_or_auth_error = user.is_none()
            && matches!(
                status,
                client::Status::SignedOut | client::Status::AuthenticationError
            );
        let show_sign_in = is_signed_out_or_auth_error
            && title_bar_settings.show_sign_in
            && self.client.accounts_available();

        let mut controls = Vec::new();
        controls.push(self.render_collaborator_list(window, cx).into_any_element());
        if let Some(banner) = self
            .banner
            .clone()
            .filter(|_| title_bar_settings.show_onboarding_banner)
        {
            controls.push(banner.into_any_element());
        }
        controls.push(self.render_call_controls(window, cx));
        controls.extend(self.render_connection_status(status, cx));
        if show_sign_in {
            controls.push(self.render_sign_in_button(cx).into_any_element());
        }
        if is_signing_in {
            controls.push(
                Label::new("Signing in…")
                    .size(LabelSize::Small)
                    .color(Color::Muted)
                    .with_animation(
                        "signing-in",
                        Animation::new(Duration::from_secs(2))
                            .repeat()
                            .with_easing(pulsating_between(0.4, 0.8)),
                        |label, delta| label.alpha(delta),
                    )
                    .into_any_element(),
            );
        }
        if title_bar_settings.show_user_menu {
            controls.push(self.render_user_menu_button(cx).into_any_element());
        }
        controls
    }

    /// VS Code style command center: an outlined field on the middle of the
    /// bar that opens the file finder. It fills the width `CenteredRow` gives
    /// it. The command palette keeps its key binding and its menu entry: the
    /// chevron button that used to sit at the field's end said nothing about
    /// what it did.
    fn render_command_center(
        &self,
        project_name: Option<SharedString>,
        cx: &mut Context<Self>,
    ) -> impl IntoElement {
        let colors = cx.theme().colors();
        let label = project_name.unwrap_or_else(|| "Search".into());

        h_flex().size_full().child(
            h_flex()
                .w_full()
                .h(metrics::icon_hit_area(cx))
                .overflow_hidden()
                // A named radius: square in the Unaligned family, rounded
                // in themes that round.
                .rounded_md()
                .border_1()
                .border_color(colors.border_strong)
                .bg(colors.editor_background)
                .hover(|style| style.bg(colors.element_hover))
                .on_mouse_down(MouseButton::Left, |_, _, cx| cx.stop_propagation())
                .child(
                    div().flex_1().min_w_0().overflow_hidden().child(
                        ButtonLike::new("command-center")
                            .full_width()
                            .style(ButtonStyle::Transparent)
                            .aria_label("Search Files")
                            .tab_index(0isize)
                            .child(
                                h_flex()
                                    .min_w_0()
                                    .gap_1p5()
                                    .child(
                                        Icon::new(IconName::MagnifyingGlass)
                                            .size(IconSize::XSmall)
                                            .color(Color::Muted),
                                    )
                                    .child(
                                        Label::new(label)
                                            .size(LabelSize::Small)
                                            .color(Color::Muted)
                                            .truncate(),
                                    ),
                            )
                            .tooltip(|_window, cx| {
                                Tooltip::for_action(
                                    "Search Files",
                                    &ToggleFileFinder::default(),
                                    cx,
                                )
                            })
                            .on_click(cx.listener(|this, _, window, cx| {
                                this.dispatch_in_workspace(
                                    &ToggleFileFinder::default(),
                                    window,
                                    cx,
                                );
                            })),
                    ),
                ),
        )
    }

    /// Dispatches from the workspace's focused pane: the title bar's own buttons
    /// can hold focus, and they sit outside the tree that handles these actions.
    fn dispatch_in_workspace(&self, action: &dyn Action, window: &mut Window, cx: &mut App) {
        if let Some(workspace) = self.workspace.upgrade() {
            let focus_handle = workspace.read(cx).focus_handle(cx);
            focus_handle.dispatch_action(action, window, cx);
        } else {
            window.dispatch_action(action.boxed_clone(), cx);
        }
    }

    /// The three VS Code layout toggles (left, bottom and right dock), then
    /// the layout menu.
    fn render_layout_toggles(&self, cx: &mut Context<Self>) -> impl IntoElement {
        let (left_open, bottom_open, right_open) = self
            .workspace
            .upgrade()
            .map(|workspace| {
                let workspace = workspace.read(cx);
                (
                    workspace.left_dock().read(cx).is_open(),
                    workspace.bottom_dock().read(cx).is_open(),
                    workspace.right_dock().read(cx).is_open(),
                )
            })
            .unwrap_or_default();

        h_flex()
            .gap_px()
            .child(self.render_layout_toggle(
                "toggle-left-dock",
                "Toggle Left Dock",
                DockPosition::Left,
                left_open,
                cx,
            ))
            .child(self.render_layout_toggle(
                "toggle-bottom-dock",
                "Toggle Bottom Dock",
                DockPosition::Bottom,
                bottom_open,
                cx,
            ))
            .child(self.render_layout_toggle(
                "toggle-right-dock",
                "Toggle Right Dock",
                DockPosition::Right,
                right_open,
                cx,
            ))
            .child(self.render_layout_menu(cx))
    }

    /// The layout menu: the two presets, Zen mode, centered layout, reset.
    fn render_layout_menu(&self, cx: &mut Context<Self>) -> impl IntoElement {
        let workspace = self.workspace.clone();
        let line = cx.theme().colors().icon_muted;

        // Drawn like the toggles beside it: a window outline, here with both a
        // side region and a bottom region.
        let glyph = div()
            .flex()
            .flex_row()
            .w(px(14.))
            .h(px(12.))
            .border_1()
            .border_color(line)
            .child(div().h_full().w(px(4.)).border_r_1().border_color(line))
            .child(
                div()
                    .flex()
                    .flex_col()
                    .justify_end()
                    .flex_1()
                    .h_full()
                    .child(div().w_full().h(px(4.)).border_t_1().border_color(line)),
            );

        PopoverMenu::new("layout-menu")
            .trigger_with_tooltip(
                ButtonLike::new("layout-menu-trigger")
                    .style(ButtonStyle::Subtle)
                    .width(rems_from_px(26_f32))
                    .aria_label("Layout")
                    .tab_index(0isize)
                    .child(glyph),
                Tooltip::text("Layout"),
            )
            .anchor(Anchor::TopRight)
            .menu(move |window, cx| {
                let workspace = workspace.clone();
                let (zen_mode, centered_layout, key_context) = workspace
                    .read_with(cx, |workspace, cx| {
                        // Key bindings are looked up from where the user was
                        // typing, not from this button.
                        let key_context = workspace
                            .active_item(cx)
                            .map(|item| item.item_focus_handle(cx))
                            .unwrap_or_else(|| workspace.focus_handle(cx));
                        (
                            workspace.zen_mode(),
                            workspace.centered_layout,
                            Some(key_context),
                        )
                    })
                    .unwrap_or_default();

                // The presets place the chat, which does not exist without AI.
                let ai_enabled = !project::DisableAiSettings::get_global(cx).disable_ai;
                let current_layout = AgentSettings::get_layout(cx);
                let is_classic = matches!(current_layout, WindowLayout::Editor(_));
                let is_ai_first = matches!(current_layout, WindowLayout::Agent(_));

                Some(ContextMenu::build(window, cx, move |menu, _, _| {
                    menu.when_some(key_context, |menu, key_context| menu.context(key_context))
                        .when(ai_enabled, |menu| {
                            menu.toggleable_entry(
                                "Classic Layout",
                                is_classic,
                                IconPosition::Start,
                                Some(UseClassicLayout.boxed_clone()),
                                {
                                    let workspace = workspace.clone();
                                    move |window, cx| {
                                        workspace
                                            .update(cx, |workspace, cx| {
                                                workspace.apply_layout(
                                                    WindowLayout::Editor(None),
                                                    window,
                                                    cx,
                                                );
                                            })
                                            .log_err();
                                    }
                                },
                            )
                            .toggleable_entry(
                                "AI-First Layout",
                                is_ai_first,
                                IconPosition::Start,
                                Some(UseAgenticLayout.boxed_clone()),
                                {
                                    let workspace = workspace.clone();
                                    move |window, cx| {
                                        workspace
                                            .update(cx, |workspace, cx| {
                                                workspace.apply_layout(
                                                    WindowLayout::Agent(None),
                                                    window,
                                                    cx,
                                                );
                                            })
                                            .log_err();
                                    }
                                },
                            )
                            .separator()
                        })
                        .toggleable_entry(
                            "Zen Mode",
                            zen_mode,
                            IconPosition::Start,
                            Some(ToggleZenMode.boxed_clone()),
                            {
                                let workspace = workspace.clone();
                                move |window, cx| {
                                    workspace
                                        .update(cx, |workspace, cx| {
                                            workspace.toggle_zen_mode(window, cx);
                                        })
                                        .log_err();
                                }
                            },
                        )
                        .toggleable_entry(
                            "Centered Layout",
                            centered_layout,
                            IconPosition::Start,
                            Some(ToggleCenteredLayout.boxed_clone()),
                            {
                                let workspace = workspace.clone();
                                move |window, cx| {
                                    workspace
                                        .update(cx, |workspace, cx| {
                                            workspace.toggle_centered_layout(
                                                &ToggleCenteredLayout,
                                                window,
                                                cx,
                                            );
                                        })
                                        .log_err();
                                }
                            },
                        )
                        .separator()
                        // Not a toggle: the never-checked slot only keeps its
                        // label on the same inset as the entries above.
                        .toggleable_entry(
                            "Reset Layout",
                            false,
                            IconPosition::Start,
                            Some(ResetLayout.boxed_clone()),
                            move |window, cx| {
                                workspace
                                    .update(cx, |workspace, cx| {
                                        workspace.reset_layout(window, cx);
                                    })
                                    .log_err();
                            },
                        )
                }))
            })
    }

    fn render_layout_toggle(
        &self,
        id: &'static str,
        label: &'static str,
        position: DockPosition,
        is_open: bool,
        cx: &mut Context<Self>,
    ) -> impl IntoElement {
        // Muted like every other icon of the frame: the state is carried by the
        // fill of the dock region, not by a brighter colour.
        let line = cx.theme().colors().icon_muted;

        // The glyph is drawn rather than loaded: a window outline whose dock
        // region is filled when that dock is open, outlined when it is closed.
        let region = div()
            .when(is_open, |this| this.bg(line))
            .border_color(line)
            .map(|this| match position {
                DockPosition::Left => this.h_full().w(px(4.)).border_r_1(),
                DockPosition::Right => this.h_full().w(px(4.)).border_l_1(),
                DockPosition::Bottom => this.w_full().h(px(4.)).border_t_1(),
            });
        let glyph = div()
            .flex()
            .w(px(14.))
            .h(px(12.))
            .border_1()
            .border_color(line)
            .map(|this| match position {
                DockPosition::Left => this.flex_row(),
                DockPosition::Right => this.flex_row().justify_end(),
                DockPosition::Bottom => this.flex_col().justify_end(),
            })
            .child(region);

        ButtonLike::new(id)
            .style(ButtonStyle::Subtle)
            .width(rems_from_px(26_f32))
            .aria_label(label)
            .tab_index(0isize)
            .child(glyph)
            .tooltip(move |_window, cx| match position {
                DockPosition::Left => Tooltip::for_action(label, &ToggleLeftDock, cx),
                DockPosition::Bottom => Tooltip::for_action(label, &ToggleBottomDock, cx),
                DockPosition::Right => Tooltip::for_action(label, &ToggleRightDock, cx),
            })
            .on_click(cx.listener(move |this, _, window, cx| {
                this.workspace
                    .update(cx, |workspace, cx| {
                        workspace.toggle_dock(position, window, cx);
                    })
                    .log_err();
            }))
    }

    fn worktree_count(&self, cx: &App) -> usize {
        self.project.read(cx).visible_worktrees(cx).count()
    }

    fn toggle_update_simulation(&mut self, cx: &mut Context<Self>) {
        self.update_version
            .update(cx, |banner, cx| banner.update_simulation(cx));
        cx.notify();
    }

    /// Returns the worktree to display in the title bar.
    /// - Prefer the worktree owning the project's active repository
    /// - Fall back to the first visible worktree
    pub fn effective_active_worktree(&self, cx: &App) -> Option<Entity<project::Worktree>> {
        let project = self.project.read(cx);

        if let Some(repo) = project.active_repository(cx) {
            let repo = repo.read(cx);
            let repo_path = &repo.work_directory_abs_path;

            for worktree in project.visible_worktrees(cx) {
                let worktree_path = worktree.read(cx).abs_path();
                if worktree_path == *repo_path || worktree_path.starts_with(repo_path.as_ref()) {
                    return Some(worktree);
                }
            }
        }

        project.visible_worktrees(cx).next()
    }

    fn get_repository_for_worktree(
        &self,
        worktree: &Entity<project::Worktree>,
        path_style: PathStyle,
        cx: &App,
    ) -> Option<Entity<project::git_store::Repository>> {
        let project = self.project.read(cx);
        let git_store = project.git_store().read(cx);
        let worktree_path = worktree.read(cx).abs_path();

        git_store
            .repositories()
            .values()
            .filter(|repo| {
                let repo_path = &repo.read(cx).work_directory_abs_path;
                worktree_path == *repo_path
                    || path_style.starts_with(&worktree_path, repo_path.as_ref())
            })
            .max_by_key(|repo| repo.read(cx).work_directory_abs_path.as_os_str().len())
            .cloned()
    }

    fn visible_worktrees_in_repository(
        &self,
        repository: &project::git_store::Repository,
        path_style: PathStyle,
        cx: &App,
    ) -> usize {
        let repo_path = &repository.work_directory_abs_path;
        self.project
            .read(cx)
            .visible_worktrees(cx)
            .filter(|worktree| {
                let worktree_path = worktree.read(cx).abs_path();
                worktree_path == *repo_path
                    || path_style.starts_with(&worktree_path, repo_path.as_ref())
            })
            .count()
    }

    fn render_remote_project_connection(&self, cx: &mut Context<Self>) -> Option<AnyElement> {
        let workspace = self.workspace.clone();

        let options = self.project.read(cx).remote_connection_options(cx)?;
        let host: SharedString = options.display_name().into();

        let (tooltip_title, icon) = match options {
            RemoteConnectionOptions::Ssh(_) => ("Remote Project", IconName::Server),
            RemoteConnectionOptions::Wsl(_) => ("Remote Project", IconName::Linux),
            RemoteConnectionOptions::Docker(_dev_container_connection) => {
                ("Dev Container", IconName::Box)
            }
            #[cfg(any(test, feature = "test-support"))]
            RemoteConnectionOptions::Mock(_) => ("Mock Remote Project", IconName::Server),
        };

        let (indicator_color, meta) = match self.project.read(cx).remote_connection_state(cx)? {
            remote::ConnectionState::Connecting => (Color::Info, format!("Connecting to: {host}")),
            remote::ConnectionState::Connected => (Color::Success, format!("Connected to: {host}")),
            remote::ConnectionState::HeartbeatMissed => (
                Color::Warning,
                format!("Connection attempt to {host} missed. Retrying..."),
            ),
            remote::ConnectionState::Reconnecting => (
                Color::Warning,
                format!("Lost connection to {host}. Reconnecting..."),
            ),
            remote::ConnectionState::Disconnected => {
                (Color::Error, format!("Disconnected from {host}"))
            }
        };

        let icon_color = match self.project.read(cx).remote_connection_state(cx)? {
            remote::ConnectionState::Connecting => Color::Info,
            remote::ConnectionState::Connected => Color::Default,
            remote::ConnectionState::HeartbeatMissed => Color::Warning,
            remote::ConnectionState::Reconnecting => Color::Warning,
            remote::ConnectionState::Disconnected => Color::Error,
        };

        let meta = SharedString::from(meta);

        Some(
            PopoverMenu::new("remote-project-menu")
                .menu(move |window, cx| {
                    let workspace_entity = workspace.upgrade()?;
                    let fs = workspace_entity.read(cx).project().read(cx).fs().clone();
                    Some(recent_projects::RemoteServerProjects::popover(
                        fs,
                        workspace.clone(),
                        None,
                        window,
                        cx,
                    ))
                })
                .trigger_with_tooltip(
                    ButtonLike::new("remote_project")
                        .selected_style(ButtonStyle::Tinted(TintColor::Accent))
                        .child(
                            h_flex()
                                .gap_2()
                                .max_w_32()
                                .child(
                                    IconWithIndicator::new(
                                        Icon::new(icon).size(IconSize::Small).color(icon_color),
                                        Some(Indicator::dot().color(indicator_color)),
                                    )
                                    .indicator_border_color(Some(
                                        cx.theme().colors().title_bar_background,
                                    ))
                                    .into_any_element(),
                                )
                                .child(Label::new(host).size(LabelSize::Small).truncate()),
                        ),
                    move |_window, cx| {
                        Tooltip::with_meta(
                            tooltip_title,
                            Some(&OpenRemote::default()),
                            meta.clone(),
                            cx,
                        )
                    },
                )
                .anchor(gpui::Anchor::TopLeft)
                .into_any_element(),
        )
    }

    pub fn render_restricted_mode(&self, cx: &mut Context<Self>) -> Option<AnyElement> {
        let has_restricted_worktrees =
            TrustedWorktrees::has_restricted_worktrees(&self.project.read(cx).worktree_store(), cx);
        if !has_restricted_worktrees {
            return None;
        }

        let button = Button::new("restricted_mode_trigger", "Restricted Mode")
            .style(ButtonStyle::Tinted(TintColor::Warning))
            .label_size(LabelSize::Small)
            .color(Color::Warning)
            .start_icon(
                Icon::new(IconName::Warning)
                    .size(IconSize::Small)
                    .color(Color::Warning),
            )
            .tooltip(|_, cx| {
                Tooltip::with_meta(
                    "You're in Restricted Mode",
                    Some(&ToggleWorktreeSecurity),
                    "Mark this project as trusted and unlock all features",
                    cx,
                )
            })
            .on_click({
                cx.listener(move |this, _, window, cx| {
                    this.workspace
                        .update(cx, |workspace, cx| {
                            workspace.show_worktree_trust_security_modal(true, window, cx)
                        })
                        .log_err();
                })
            });

        if ui::utils::MACOS_SDK_26_OR_LATER {
            // Make up for Tahoe's traffic light buttons having less spacing around them
            Some(div().child(button).ml_0p5().into_any_element())
        } else {
            Some(button.into_any_element())
        }
    }

    pub fn render_project_host(&self, cx: &mut Context<Self>) -> Option<AnyElement> {
        if self.project.read(cx).is_via_remote_server() {
            return self.render_remote_project_connection(cx);
        }

        if self.project.read(cx).is_disconnected(cx) {
            return Some(
                Button::new("disconnected", "Disconnected")
                    .disabled(true)
                    .color(Color::Disabled)
                    .label_size(LabelSize::Small)
                    .into_any_element(),
            );
        }

        let host = self.project.read(cx).host()?;
        let host_user = self.user_store.read(cx).get_cached_user(host.user_id)?;
        let participant_index = self
            .user_store
            .read(cx)
            .participant_indices()
            .get(&host_user.legacy_id)?;

        Some(
            Button::new("project_owner_trigger", host_user.username.clone())
                .color(Color::Player(participant_index.0))
                .label_size(LabelSize::Small)
                .tab_index(0isize)
                .tooltip(move |_, cx| {
                    let tooltip_title = format!(
                        "{} is sharing this project. Click to follow.",
                        host_user.username
                    );

                    Tooltip::with_meta(tooltip_title, None, "Click to Follow", cx)
                })
                .on_click({
                    let host_peer_id = host.peer_id;
                    cx.listener(move |this, _, window, cx| {
                        this.workspace
                            .update(cx, |workspace, cx| {
                                workspace.follow(host_peer_id, window, cx);
                            })
                            .log_err();
                    })
                })
                .into_any_element(),
        )
    }

    fn render_project_name(
        &self,
        name: Option<SharedString>,
        _: &mut Window,
        cx: &mut Context<Self>,
    ) -> impl IntoElement {
        let workspace = self.workspace.clone();
        // Without a folder the button is an offer, not a project's name: muted.
        let has_project = name.is_some();

        let display_name = if let Some(ref name) = name {
            util::truncate_and_trailoff(name, MAX_PROJECT_NAME_LENGTH)
        } else {
            "Open Recent Project".to_string()
        };

        let is_sidebar_open = self
            .multi_workspace
            .as_ref()
            .and_then(|mw| mw.upgrade())
            .map(|mw| mw.read(cx).sidebar_open())
            .unwrap_or(false)
            && PlatformTitleBar::is_multi_workspace_enabled(cx);

        let is_threads_list_view_active = self
            .multi_workspace
            .as_ref()
            .and_then(|mw| mw.upgrade())
            .map(|mw| mw.read(cx).is_threads_list_view_active(cx))
            .unwrap_or(false);

        if is_sidebar_open && is_threads_list_view_active {
            return self
                .render_recent_projects_popover(display_name, cx)
                .into_any_element();
        }

        let focus_handle = workspace
            .upgrade()
            .map(|w| w.read(cx).focus_handle(cx))
            .unwrap_or_else(|| cx.focus_handle());

        let window_project_groups: Vec<_> = self
            .multi_workspace
            .as_ref()
            .and_then(|mw| mw.upgrade())
            .map(|mw| mw.read(cx).project_group_keys())
            .unwrap_or_default();

        PopoverMenu::new("recent-projects-menu")
            .menu(move |window, cx| {
                Some(recent_projects::RecentProjects::popover(
                    workspace.clone(),
                    window_project_groups.clone(),
                    None,
                    focus_handle.clone(),
                    window,
                    cx,
                ))
            })
            .trigger_with_tooltip(
                Button::new("project_name_trigger", display_name)
                    .label_size(LabelSize::Small)
                    .when(!has_project, |this| this.color(Color::Muted))
                    .tab_index(0isize)
                    .when(self.worktree_count(cx) > 1, |this| {
                        this.end_icon(
                            Icon::new(IconName::ChevronDown)
                                .size(IconSize::XSmall)
                                .color(Color::Muted),
                        )
                    })
                    .selected_style(ButtonStyle::Tinted(TintColor::Accent)),
                move |_window, cx| {
                    Tooltip::for_action("Recent Projects", &app_actions::OpenRecent::default(), cx)
                },
            )
            .anchor(gpui::Anchor::TopLeft)
            .into_any_element()
    }

    fn render_recent_projects_popover(
        &self,
        display_name: String,
        cx: &mut Context<Self>,
    ) -> impl IntoElement {
        let workspace = self.workspace.clone();

        let focus_handle = workspace
            .upgrade()
            .map(|w| w.read(cx).focus_handle(cx))
            .unwrap_or_else(|| cx.focus_handle());

        let window_project_groups: Vec<_> = self
            .multi_workspace
            .as_ref()
            .and_then(|mw| mw.upgrade())
            .map(|mw| mw.read(cx).project_group_keys())
            .unwrap_or_default();

        PopoverMenu::new("sidebar-title-recent-projects-menu")
            .menu(move |window, cx| {
                Some(recent_projects::RecentProjects::popover(
                    workspace.clone(),
                    window_project_groups.clone(),
                    None,
                    focus_handle.clone(),
                    window,
                    cx,
                ))
            })
            .trigger_with_tooltip(
                Button::new("project_name_trigger", display_name)
                    .label_size(LabelSize::Small)
                    .tab_index(0isize)
                    .when(self.worktree_count(cx) > 1, |this| {
                        this.end_icon(
                            Icon::new(IconName::ChevronDown)
                                .size(IconSize::XSmall)
                                .color(Color::Muted),
                        )
                    })
                    .selected_style(ButtonStyle::Tinted(TintColor::Accent)),
                move |_window, cx| {
                    Tooltip::for_action("Recent Projects", &app_actions::OpenRecent::default(), cx)
                },
            )
            .anchor(gpui::Anchor::TopLeft)
    }

    /// The one git chip of the bar: the branch, preceded by the worktree's
    /// name when the folder is a linked worktree ("fix-login · main"). It
    /// opens the branch picker.
    fn render_branch_chip(
        &self,
        repository: Entity<project::git_store::Repository>,
        linked_worktree_name: Option<SharedString>,
        cx: &mut Context<Self>,
    ) -> Option<AnyElement> {
        let workspace = self.workspace.upgrade()?;
        let settings = *TitleBarSettings::get_global(cx);

        let (branch_name, status_icon, is_detached_head) = {
            let repo = repository.read(cx);

            let is_detached_head = repo.branch.is_none();

            let branch_name = repo
                .branch
                .as_ref()
                .map(|branch| branch.name())
                .map(|name| util::truncate_and_trailoff(name, MAX_BRANCH_NAME_LENGTH))
                .or_else(|| {
                    repo.head_commit.as_ref().map(|commit| {
                        commit
                            .sha
                            .chars()
                            .take(MAX_SHORT_SHA_LENGTH)
                            .collect::<String>()
                    })
                });

            let status = repo.status_summary();
            let tracked = status.index + status.worktree;
            let status_icon = if status.conflict > 0 {
                (IconName::Warning, Color::VersionControlConflict)
            } else if tracked.modified > 0 {
                (IconName::SquareDot, Color::VersionControlModified)
            } else if tracked.added > 0 || status.untracked > 0 {
                (IconName::SquarePlus, Color::VersionControlAdded)
            } else if tracked.deleted > 0 {
                (IconName::SquareMinus, Color::VersionControlDeleted)
            } else {
                (IconName::GitBranch, Color::Muted)
            };

            (branch_name, status_icon, is_detached_head)
        };

        let branch_name = branch_name.filter(|_| settings.show_branch_name);
        let worktree_name = linked_worktree_name
            .filter(|_| settings.show_worktree_name)
            .map(|name| util::truncate_and_trailoff(&name, MAX_BRANCH_NAME_LENGTH));
        if branch_name.is_none() && worktree_name.is_none() {
            return None;
        }

        let (creation_in_progress, is_switch) = {
            let creation = workspace.read(cx).active_worktree_creation();
            (creation.label.clone(), creation.is_switch)
        };
        let is_creating = creation_in_progress.is_some();

        let label = if let Some(name) = &creation_in_progress {
            if is_switch {
                format!("Loading {name}…")
            } else {
                format!("Creating {name}…")
            }
        } else {
            branch_chip_label(
                worktree_name.as_deref(),
                branch_name.as_deref(),
                is_detached_head,
            )
        };

        let (icon, icon_color) = if is_detached_head && branch_name.is_some() {
            (IconName::GitBranchPlus, Color::Muted)
        } else if settings.show_branch_status_icon {
            status_icon
        } else {
            (IconName::GitBranch, Color::Muted)
        };

        let tooltip_meta = match (&worktree_name, &branch_name) {
            (Some(worktree), Some(branch)) if is_detached_head => {
                format!("Worktree: {worktree} · Detached HEAD: {branch}")
            }
            (Some(worktree), Some(branch)) => {
                format!("Worktree: {worktree} · Currently Checked Out: {branch}")
            }
            (None, Some(branch)) if is_detached_head => format!("Detached HEAD: {branch}"),
            (None, Some(branch)) => format!("Currently Checked Out: {branch}"),
            (Some(worktree), None) => format!("Worktree: {worktree}"),
            (None, None) => String::new(),
        };

        let effective_repository = Some(repository);
        Some(
            PopoverMenu::new("branch-menu")
                .menu(move |window, cx| {
                    git_ui_core::build_branch_picker(
                        workspace.downgrade(),
                        effective_repository.clone(),
                        window,
                        cx,
                    )
                })
                .trigger_with_tooltip(
                    Button::new("project_branch_trigger", label)
                        .selected_style(ButtonStyle::Tinted(TintColor::Accent))
                        .label_size(LabelSize::Small)
                        .color(Color::Muted)
                        .tab_index(0isize)
                        .loading(is_creating)
                        .start_icon(Icon::new(icon).size(IconSize::XSmall).color(icon_color)),
                    move |_window, cx| {
                        Tooltip::with_meta(
                            "Branch & Stash",
                            Some(&app_actions::git::Branch),
                            tooltip_meta.clone(),
                            cx,
                        )
                    },
                )
                .anchor(gpui::Anchor::TopLeft)
                .into_any_element(),
        )
    }

    fn active_call_changed(&mut self, cx: &mut Context<Self>) {
        self.observe_diagnostics(cx);
        cx.notify();
    }

    fn observe_diagnostics(&mut self, cx: &mut Context<Self>) {
        let diagnostics = ActiveCall::global(cx)
            .read(cx)
            .room()
            .and_then(|room| room.read(cx).diagnostics().cloned());

        if let Some(diagnostics) = diagnostics {
            self._diagnostics_subscription = Some(cx.observe(&diagnostics, |_, _, cx| cx.notify()));
        } else {
            self._diagnostics_subscription = None;
        }
    }

    fn share_project(&mut self, cx: &mut Context<Self>) {
        let active_call = ActiveCall::global(cx);
        let project = self.project.clone();
        active_call
            .update(cx, |call, cx| call.share_project(project, cx))
            .detach_and_log_err(cx);
    }

    fn unshare_project(&mut self, _: &mut Window, cx: &mut Context<Self>) {
        let active_call = ActiveCall::global(cx);
        let project = self.project.clone();
        active_call
            .update(cx, |call, cx| call.unshare_project(project, cx))
            .log_err();
    }

    fn render_connection_status(
        &self,
        status: &client::Status,
        cx: &mut Context<Self>,
    ) -> Option<AnyElement> {
        match status {
            client::Status::ConnectionError
            | client::Status::ConnectionLost
            | client::Status::Reauthenticating
            | client::Status::Reconnecting
            | client::Status::ReconnectionError { .. } => Some(
                div()
                    .id("disconnected")
                    .child(Icon::new(IconName::Disconnected).size(IconSize::Small))
                    .tooltip(Tooltip::text("Disconnected"))
                    .into_any_element(),
            ),
            client::Status::UpgradeRequired => {
                let auto_updater = auto_update::AutoUpdater::get(cx);
                let label = match auto_updater.map(|auto_update| auto_update.read(cx).status()) {
                    Some(AutoUpdateStatus::Checking) => "Checking for Updates…",
                    _ => "Update to Collaborate",
                };

                Some(
                    Button::new("connection-status", label)
                        .label_size(LabelSize::Small)
                        .on_click(|_, window, cx| {
                            auto_update::check(&Default::default(), window, cx);
                        })
                        .into_any_element(),
                )
            }
            _ => None,
        }
    }

    pub fn render_sign_in_button(&mut self, _: &mut Context<Self>) -> Button {
        let client = self.client.clone();
        let workspace = self.workspace.clone();
        Button::new("sign_in", "Sign In")
            .label_size(LabelSize::Small)
            .color(Color::Muted)
            .tab_index(0isize)
            .on_click(move |_, window, cx| {
                let client = client.clone();
                let workspace = workspace.clone();
                window
                    .spawn(cx, async move |mut cx| {
                        client
                            .sign_in_with_optional_connect(true, cx)
                            .await
                            .notify_workspace_async_err(workspace, &mut cx);
                    })
                    .detach();
            })
    }

    pub fn render_user_menu_button(&mut self, cx: &mut Context<Self>) -> impl Element {
        let user_store = self.user_store.clone();
        let workspace = self.workspace.clone();
        let user = user_store.read(cx).current_user();

        let user_avatar = user.as_ref().map(|u| u.avatar_uri.clone());
        let username = user.as_ref().map(|u| u.username.clone());

        let is_signed_in = user.is_some();

        let current_organization = user_store.read(cx).current_organization();
        let business_organization = current_organization
            .as_ref()
            .filter(|organization| !organization.is_personal);
        let organizations: Vec<_> = user_store
            .read(cx)
            .organizations()
            .iter()
            .map(|organization| {
                let plan = user_store.read(cx).plan_for_organization(&organization.id);
                (organization.clone(), plan)
            })
            .collect();

        let show_user_picture = TitleBarSettings::get_global(cx).show_user_picture;

        let trigger = if is_signed_in && show_user_picture {
            let avatar = user_avatar.map(|avatar| Avatar::new(avatar));

            ButtonLike::new("user-menu")
                .aria_label("User menu")
                .tab_index(0isize)
                .child(
                    h_flex()
                        .when_some(business_organization, |this, organization| {
                            this.gap_2()
                                .child(Label::new(&organization.name).size(LabelSize::Small))
                        })
                        .children(avatar),
                )
        } else {
            ButtonLike::new("user-menu")
                .aria_label("User menu")
                .tab_index(0isize)
                .child(
                    Icon::new(IconName::Person)
                        .size(IconSize::Small)
                        .color(Color::Muted),
                )
        };

        PopoverMenu::new("user-menu")
            .trigger(trigger)
            .menu(move |window, cx| {
                let username = username.clone();
                let current_organization = current_organization.clone();
                let organizations = organizations.clone();
                let user_store = user_store.clone();
                let workspace = workspace.clone();

                ContextMenu::build(window, cx, |menu, _, _cx| {
                    menu.when(is_signed_in, |this| {
                        let username = username.clone();
                        this.custom_entry(
                            move |_window, _cx| {
                                let username = username.clone().unwrap_or_default();

                                h_flex()
                                    .w_full()
                                    .justify_between()
                                    .child(Label::new(username))
                                    .into_any_element()
                            },
                            move |_, cx| {
                                cx.open_url(&zed_urls::account_url(cx));
                            },
                        )
                        .separator()
                    })
                    .when(is_signed_in, |this| {
                        let mut this = this.header("Organization");

                        for (organization, plan) in &organizations {
                            let organization = organization.clone();
                            let plan = *plan;

                            let is_current =
                                current_organization
                                    .as_ref()
                                    .is_some_and(|current_organization| {
                                        current_organization.id == organization.id
                                    });

                            this = this.custom_entry(
                                {
                                    let organization = organization.clone();
                                    move |_window, _cx| {
                                        h_flex()
                                            .w_full()
                                            .gap_4()
                                            .justify_between()
                                            .child(
                                                h_flex()
                                                    .gap_1()
                                                    .child(Label::new(&organization.name))
                                                    .when(is_current, |this| {
                                                        this.child(
                                                            Icon::new(IconName::Check)
                                                                .color(Color::Accent),
                                                        )
                                                    }),
                                            )
                                            .children(plan.map(|plan| PlanChip::new(plan)))
                                            .into_any_element()
                                    }
                                },
                                {
                                    let user_store = user_store.clone();
                                    let organization = organization.clone();
                                    let workspace = workspace.clone();
                                    move |window, cx| {
                                        let task = user_store.update(cx, |user_store, cx| {
                                            user_store
                                                .set_current_organization(organization.clone(), cx)
                                        });
                                        task.detach_and_notify_err(workspace.clone(), window, cx);
                                    }
                                },
                            );
                        }

                        this.separator()
                    })
                    .action("Settings", app_actions::OpenSettings.boxed_clone())
                    .action("Keymap", Box::new(app_actions::OpenKeymap))
                    .action(
                        "Themes…",
                        app_actions::theme_selector::Toggle::default().boxed_clone(),
                    )
                    .action(
                        "Icon Themes…",
                        app_actions::icon_theme_selector::Toggle::default().boxed_clone(),
                    )
                    .action(
                        "Extensions",
                        app_actions::Extensions::default().boxed_clone(),
                    )
                    .when(is_signed_in, |this| {
                        this.separator()
                            .action("Sign Out", client::SignOut.boxed_clone())
                    })
                })
                .into()
            })
            .anchor(Anchor::TopRight)
    }
}

/// What the git chip says: the branch, after the worktree's name when the
/// folder is a linked worktree. A detached HEAD offers to create a branch.
fn branch_chip_label(
    worktree_name: Option<&str>,
    branch_name: Option<&str>,
    is_detached_head: bool,
) -> String {
    let branch = branch_name.map(|branch| {
        if is_detached_head {
            "Create Branch"
        } else {
            branch
        }
    });
    match (worktree_name, branch) {
        (Some(worktree), Some(branch)) => format!("{worktree} · {branch}"),
        (Some(name), None) | (None, Some(name)) => name.to_string(),
        (None, None) => String::new(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use gpui::{
        AnyWindowHandle, Empty, EntityId, SystemWindowTab, SystemWindowTabController,
        TestAppContext, VisualTestContext,
    };
    use platform_title_bar::SidebarWindowControls;
    use settings::{SidebarSide, TitleBarVisibility};

    #[test]
    fn test_branch_chip_label() {
        // An ordinary checkout: the branch alone, once.
        assert_eq!(branch_chip_label(None, Some("main"), false), "main");
        // A linked worktree: both, in one chip.
        assert_eq!(
            branch_chip_label(Some("fix-login"), Some("fix/login"), false),
            "fix-login · fix/login"
        );
        assert_eq!(
            branch_chip_label(None, Some("a1b2c3d4"), true),
            "Create Branch"
        );
        assert_eq!(
            branch_chip_label(Some("fix-login"), Some("a1b2c3d4"), true),
            "fix-login · Create Branch"
        );
        // Branch names hidden by the setting.
        assert_eq!(
            branch_chip_label(Some("fix-login"), None, false),
            "fix-login"
        );
    }

    #[gpui::test]
    async fn test_title_bar_visibility(cx: &mut TestAppContext) {
        let (workspace, cx) = init_test(TitleBarVisibility::Never, cx).await;
        assert_eq!(chrome(&workspace, cx), Chrome::HIDDEN);

        set_visibility(Some(TitleBarVisibility::Always), cx);
        assert_eq!(chrome(&workspace, cx), Chrome::SHOWN);
        let title_bar = title_bar_id(&workspace, cx);

        cx.update_global::<SettingsStore, _>(|store, cx| {
            store.update_user_settings(cx, |settings| {
                settings.title_bar.get_or_insert_default().show_menus = Some(true);
            });
        });
        cx.run_until_parked();
        assert_eq!(title_bar_id(&workspace, cx), title_bar);

        toggle_fullscreen(&workspace, cx);
        assert_eq!(
            chrome(&workspace, cx),
            Chrome {
                title_bar: true,
                ..Chrome::HIDDEN
            }
        );

        set_visibility(Some(TitleBarVisibility::HideInFullScreen), cx);
        assert_eq!(chrome(&workspace, cx), Chrome::HIDDEN);

        toggle_fullscreen(&workspace, cx);
        assert_eq!(chrome(&workspace, cx), Chrome::SHOWN);

        toggle_fullscreen(&workspace, cx);
        assert_eq!(chrome(&workspace, cx), Chrome::HIDDEN);

        toggle_fullscreen(&workspace, cx);
        set_visibility(Some(TitleBarVisibility::Never), cx);
        assert_eq!(chrome(&workspace, cx), Chrome::HIDDEN);

        set_visibility(None, cx);
        assert_eq!(chrome(&workspace, cx), Chrome::SHOWN);
    }

    #[gpui::test]
    async fn test_window_tabs_stay_mounted_without_title_bar(cx: &mut TestAppContext) {
        let other_window = cx.add_window(|_, _| Empty);
        let (workspace, cx) = init_test(TitleBarVisibility::Never, cx).await;
        workspace.update_in(cx, |_, window, cx| {
            let workspace_window = window.window_handle();
            SystemWindowTabController::add_tab(
                cx,
                workspace_window.window_id(),
                Vec::from([
                    SystemWindowTab::new(SharedString::from("workspace"), workspace_window),
                    SystemWindowTab::new(
                        SharedString::from("other"),
                        AnyWindowHandle::from(other_window),
                    ),
                ]),
            );
            SystemWindowTabController::set_visible(cx, true);
            cx.notify();
        });
        cx.run_until_parked();
        assert_eq!(chrome(&workspace, cx), Chrome::HIDDEN);
        assert!(window_tabs_mounted(cx));

        set_visibility(Some(TitleBarVisibility::HideInFullScreen), cx);
        assert_eq!(chrome(&workspace, cx), Chrome::SHOWN);
        assert!(window_tabs_mounted(cx));

        toggle_fullscreen(&workspace, cx);
        assert_eq!(chrome(&workspace, cx), Chrome::HIDDEN);
        assert!(window_tabs_mounted(cx));
    }

    #[derive(Debug, PartialEq)]
    struct Chrome {
        title_bar: bool,
        left_sidebar: SidebarWindowControls,
        right_sidebar: SidebarWindowControls,
    }

    impl Chrome {
        const SHOWN: Self = Self {
            title_bar: true,
            left_sidebar: SidebarWindowControls {
                traffic_lights: cfg!(target_os = "macos"),
                left: !cfg!(target_os = "macos"),
                right: false,
            },
            right_sidebar: SidebarWindowControls {
                traffic_lights: false,
                left: false,
                right: !cfg!(target_os = "macos"),
            },
        };
        const HIDDEN: Self = Self {
            title_bar: false,
            left_sidebar: NO_WINDOW_CONTROLS,
            right_sidebar: NO_WINDOW_CONTROLS,
        };
    }

    const NO_WINDOW_CONTROLS: SidebarWindowControls = SidebarWindowControls {
        traffic_lights: false,
        left: false,
        right: false,
    };

    async fn init_test(
        visibility: TitleBarVisibility,
        cx: &mut TestAppContext,
    ) -> (Entity<Workspace>, &mut VisualTestContext) {
        cx.update(|cx| {
            let mut settings = SettingsStore::test(cx);
            settings.update_user_settings(cx, |settings| {
                settings.title_bar.get_or_insert_default().show = Some(visibility);
            });
            cx.set_global(settings);
            cx.set_global(db::AppDatabase::test_new());
            theme_settings::init(theme::LoadThemes::JustBase, cx);
            command_palette_hooks::init(cx);
        });
        let project = Project::test(fs::FakeFs::new(cx.executor()), [], cx).await;
        cx.update(|cx| {
            let project = project.read(cx);
            call::init(project.client(), project.user_store(), cx);
            init(cx);
        });
        let (workspace, cx) =
            cx.add_window_view(|window, cx| Workspace::test_new(project, window, cx));
        cx.run_until_parked();
        (workspace, cx)
    }

    fn set_visibility(visibility: Option<TitleBarVisibility>, cx: &mut VisualTestContext) {
        cx.update_global::<SettingsStore, _>(|store, cx| {
            store.update_user_settings(cx, |settings| {
                settings.title_bar.get_or_insert_default().show = visibility;
            });
        });
        cx.run_until_parked();
    }

    fn toggle_fullscreen(workspace: &Entity<Workspace>, cx: &mut VisualTestContext) {
        workspace.update_in(cx, |_, window, _| window.toggle_fullscreen());
        cx.run_until_parked();
    }

    fn chrome(workspace: &Entity<Workspace>, cx: &mut VisualTestContext) -> Chrome {
        workspace.update_in(cx, |workspace, window, cx| Chrome {
            title_bar: workspace.titlebar_item().is_some(),
            left_sidebar: SidebarWindowControls::new(SidebarSide::Left, window, cx),
            right_sidebar: SidebarWindowControls::new(SidebarSide::Right, window, cx),
        })
    }

    fn title_bar_id(workspace: &Entity<Workspace>, cx: &mut VisualTestContext) -> Option<EntityId> {
        workspace.read_with(cx, |workspace, _| {
            workspace.titlebar_item().map(|item| item.entity_id())
        })
    }

    fn window_tabs_mounted(cx: &mut VisualTestContext) -> bool {
        cx.update(|window, _| window.refresh());
        cx.run_until_parked();
        cx.debug_bounds("system-window-tabs")
            .is_some_and(|bounds| bounds.size.height > px(0.))
    }
}

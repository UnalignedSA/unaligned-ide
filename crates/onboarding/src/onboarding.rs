use crate::multibuffer_hint::MultibufferHint;
use db::kvp::KeyValueStore;
use fs::Fs;
use gpui::{
    Action, App, AppContext, AsyncWindowContext, Context, Entity, EventEmitter, FocusHandle,
    Focusable, FontWeight, Global, IntoElement, KeyContext, Render, ScrollHandle, SharedString,
    Subscription, Task, WeakEntity, Window, actions, relative,
};
use notifications::status_toast::StatusToast;
use schemars::JsonSchema;
use serde::Deserialize;
use settings::{SettingsStore, VsCodeSettingsSource};
use std::sync::Arc;
use theme_settings::ThemeSettings;
use ui::{
    DISPLAY_FONT, KeyBinding, ParentElement as _, StatefulInteractiveElement, WithScrollbar as _,
    prelude::*, rems_from_px,
};
use workspace::welcome::AiConnection;

use app_actions::OpenOnboarding;
pub use workspace::welcome::ShowWelcome;
use workspace::welcome::WelcomePage;
use workspace::{
    AppState, Workspace, WorkspaceId,
    dock::DockPosition,
    item::{Item, ItemEvent},
    open_new, register_serializable_item, with_active_or_new_workspace,
};

mod base_keymap_picker;
mod basics_page;
pub mod multibuffer_hint;
mod theme_preview;

/// Imports settings from Visual Studio Code.
#[derive(Copy, Clone, Debug, Default, PartialEq, Deserialize, JsonSchema, Action)]
#[action(namespace = app)]
#[serde(deny_unknown_fields)]
pub struct ImportVsCodeSettings {
    #[serde(default)]
    pub skip_prompt: bool,
}

/// Imports settings from Cursor editor.
#[derive(Copy, Clone, Debug, Default, PartialEq, Deserialize, JsonSchema, Action)]
#[action(namespace = app)]
#[serde(deny_unknown_fields)]
pub struct ImportCursorSettings {
    #[serde(default)]
    pub skip_prompt: bool,
}

pub const FIRST_OPEN: &str = "first_open";

actions!(
    onboarding,
    [
        /// Finish the onboarding process.
        Finish,
        /// Sign in while in the onboarding flow.
        /// Kept so existing keymaps still load; nothing handles it any more.
        SignIn,
        /// Opened the user account page upstream.
        /// Kept so existing keymaps still load; nothing handles it any more.
        OpenAccount,
        /// Resets the welcome screen hints to their initial state.
        ResetHints
    ]
);

pub fn init(cx: &mut App) {
    cx.observe_new(|workspace: &mut Workspace, _, _cx| {
        workspace
            .register_action(|_workspace, _: &ResetHints, _, cx| MultibufferHint::set_count(0, cx));
        // Handled by the workspace of the window the action was dispatched in, so
        // it also works while the application is in the background.
        workspace.register_action(|workspace, _: &Finish, window, cx| {
            go_to_welcome_page(workspace, window, cx);
            // With no model connected yet, the next thing to do is in the chat panel
            // (the key field of the connection screen): put the focus there, so the
            // newcomer can paste a key without a click.
            if !matches!(
                cx.try_global::<AiConnection>(),
                Some(AiConnection::Connected { .. })
            ) {
                // The welcome page that just took the focus is not in the rendered tree
                // yet, and an action dispatched now would start from the window's root
                // and miss the workspace: wait until it has been drawn.
                window.on_next_frame(|window, _| {
                    window.on_next_frame(|window, cx| {
                        window
                            .dispatch_action(app_actions::assistant::FocusAgent.boxed_clone(), cx);
                    });
                });
            }
        });
    })
    .detach();

    cx.on_action(|_: &OpenOnboarding, cx| {
        with_active_or_new_workspace(cx, |workspace, window, cx| {
            workspace
                .with_local_workspace(window, cx, |workspace, window, cx| {
                    let existing = workspace
                        .active_pane()
                        .read(cx)
                        .items()
                        .find_map(|item| item.downcast::<Onboarding>());

                    if let Some(existing) = existing {
                        workspace.activate_item(&existing, true, true, window, cx);
                    } else {
                        let settings_page = Onboarding::new(workspace, cx);
                        workspace.add_item_to_active_pane(
                            Box::new(settings_page),
                            None,
                            true,
                            window,
                            cx,
                        )
                    }
                })
                .detach();
        });
    });

    cx.on_action(|_: &ShowWelcome, cx| {
        with_active_or_new_workspace(cx, |workspace, window, cx| {
            workspace
                .with_local_workspace(window, cx, |workspace, window, cx| {
                    let existing = workspace
                        .active_pane()
                        .read(cx)
                        .items()
                        .find_map(|item| item.downcast::<WelcomePage>());

                    if let Some(existing) = existing {
                        workspace.activate_item(&existing, true, true, window, cx);
                    } else {
                        let settings_page = cx
                            .new(|cx| WelcomePage::new(workspace.weak_handle(), false, window, cx));
                        workspace.add_item_to_active_pane(
                            Box::new(settings_page),
                            None,
                            true,
                            window,
                            cx,
                        )
                    }
                })
                .detach();
        });
    });

    cx.observe_new(|workspace: &mut Workspace, _window, _cx| {
        workspace.register_action(|_workspace, action: &ImportVsCodeSettings, window, cx| {
            let fs = <dyn Fs>::global(cx);
            let action = *action;

            let workspace = cx.weak_entity();

            window
                .spawn(cx, async move |cx: &mut AsyncWindowContext| {
                    handle_import_vscode_settings(
                        workspace,
                        VsCodeSettingsSource::VsCode,
                        action.skip_prompt,
                        fs,
                        cx,
                    )
                    .await
                })
                .detach();
        });

        workspace.register_action(|_workspace, action: &ImportCursorSettings, window, cx| {
            let fs = <dyn Fs>::global(cx);
            let action = *action;

            let workspace = cx.weak_entity();

            window
                .spawn(cx, async move |cx: &mut AsyncWindowContext| {
                    handle_import_vscode_settings(
                        workspace,
                        VsCodeSettingsSource::Cursor,
                        action.skip_prompt,
                        fs,
                        cx,
                    )
                    .await
                })
                .detach();
        });
    })
    .detach();

    base_keymap_picker::init(cx);

    register_serializable_item::<Onboarding>(cx);
    register_serializable_item::<WelcomePage>(cx);
}

pub fn show_onboarding_view(app_state: Arc<AppState>, cx: &mut App) -> Task<anyhow::Result<()>> {
    open_new(
        Default::default(),
        app_state,
        cx,
        |workspace, window, cx| {
            {
                workspace.toggle_dock(DockPosition::Left, window, cx);
                let onboarding_page = Onboarding::new(workspace, cx);
                workspace.add_item_to_center(Box::new(onboarding_page.clone()), window, cx);

                window.focus(&onboarding_page.focus_handle(cx), cx);

                cx.notify();
            };
            let kvp = KeyValueStore::global(cx);
            db::write_and_log(cx, move || async move {
                kvp.write_kvp(FIRST_OPEN.to_string(), "false".to_string())
                    .await
            });
        },
    )
}

/// The first-launch page: three choices (look, AI, keys) on one screen.
struct Onboarding {
    workspace: WeakEntity<Workspace>,
    focus_handle: FocusHandle,
    scroll_handle: ScrollHandle,
    /// One focus handle per theme tile, in the order of `basics_page::FAMILY_THEMES`.
    theme_tile_focus_handles: Vec<FocusHandle>,
    hovered_theme_tile: Option<usize>,
    /// The theme settings as they were before a tile started showing its theme.
    /// `Some` while a preview is on screen.
    theme_before_preview: Option<ThemeSettings>,
    /// Created at the first render: focus subscriptions need the window.
    theme_tile_focus_subscriptions: Vec<Subscription>,
    _subscriptions: Vec<Subscription>,
}

impl Onboarding {
    fn new(workspace: &Workspace, cx: &mut App) -> Entity<Self> {
        let workspace = workspace.weak_handle();
        cx.new(|cx| Self::build(workspace, cx))
    }

    fn build(workspace: WeakEntity<Workspace>, cx: &mut Context<Self>) -> Self {
        Self {
            workspace,
            focus_handle: cx.focus_handle(),
            scroll_handle: ScrollHandle::new(),
            theme_tile_focus_handles: basics_page::FAMILY_THEMES
                .iter()
                .map(|_| cx.focus_handle())
                .collect(),
            hovered_theme_tile: None,
            theme_before_preview: None,
            theme_tile_focus_subscriptions: Vec::new(),
            _subscriptions: vec![
                cx.observe_global::<SettingsStore>(|_, cx| cx.notify()),
                cx.observe_global::<AiConnection>(|_, cx| cx.notify()),
                // A page closed in the middle of a preview gives the theme back.
                cx.on_release(|this, cx| this.end_theme_preview(cx)),
            ],
        }
    }

    fn render_header(&self, cx: &mut Context<Self>) -> impl IntoElement {
        v_flex()
            .w_full()
            .px_3()
            .gap_2()
            .child(
                h_flex().w_full().gap_3().child(brand_mark(cx)).child(
                    // The one display line of the page, as on the welcome page that
                    // follows it. Unbounded is wide: in a narrow pane the name wraps.
                    div()
                        .min_w_0()
                        .font_family(DISPLAY_FONT)
                        .font_weight(FontWeight::MEDIUM)
                        .text_size(rems_from_px(28_f32))
                        .line_height(relative(1.15))
                        .text_color(cx.theme().colors().text)
                        .child(branding::PRODUCT_NAME),
                ),
            )
            .child(
                Label::new(
                    "Three choices and you are ready. Everything can be changed later in Settings.",
                )
                .size(LabelSize::Small)
                .color(Color::Muted),
            )
    }

    /// The page's one filled button.
    fn render_footer(&self, tab_index: isize, cx: &mut Context<Self>) -> impl IntoElement {
        let focus_handle = self.focus_handle.clone();

        h_flex()
            .w_full()
            .px_3()
            .pt_4()
            .border_t_1()
            .border_color(cx.theme().colors().border)
            .child(
                Button::primary("finish_setup", "Start Coding")
                    .size(ButtonSize::Large)
                    .tab_index(tab_index)
                    .key_binding(
                        KeyBinding::for_action_in(&Finish, &self.focus_handle, cx)
                            .size(rems_from_px(12_f32)),
                    )
                    .on_click(move |_, window, cx| {
                        focus_handle.dispatch_action(&Finish, window, cx)
                    }),
            )
    }
}

/// The brand mark: two overlapping squares, the neutral one behind, the red one in
/// front, as on the welcome page (`workspace::welcome`).
fn brand_mark(cx: &App) -> impl IntoElement {
    // The logo's own red, in every theme: the one fixed colour allowed outside the theme tokens.
    const BRAND_RED: u32 = 0xe5322d;
    // In the logo each square is 41/68 of the mark's side: 19px of 32.
    let square = rems_from_px(19_f32);

    div()
        .relative()
        .flex_none()
        .size(rems_from_px(32_f32))
        .child(
            div()
                .absolute()
                .top_0()
                .left_0()
                .size(square)
                // The text colour, so the square shows on light and dark grounds.
                .bg(cx.theme().colors().text),
        )
        .child(
            div()
                .absolute()
                .bottom_0()
                .right_0()
                .size(square)
                .bg(gpui::rgb(BRAND_RED)),
        )
}

impl Render for Onboarding {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        self.observe_theme_tile_focus(window, cx);

        // The page is one tab group (see the root element below): its own focus
        // handle sits at index 1 of the group and the controls follow it.
        let mut tab_index = 1;
        let header = self.render_header(cx).into_any_element();
        let sections = self.render_sections(&mut tab_index, cx).into_any_element();
        let footer = self.render_footer(tab_index, cx).into_any_element();

        // In a window too short for the whole page, the button that ends it stays
        // in view under the part that scrolls instead of waiting below the fold.
        let footer_is_pinned =
            window.viewport_size().height < rems_from_px(780_f32).to_pixels(window.rem_size());
        let (inline_footer, pinned_footer) = if footer_is_pinned {
            (None, Some(footer))
        } else {
            (Some(footer), None)
        };

        v_flex()
            .image_cache(gpui::retain_all("onboarding-page"))
            .key_context({
                let mut ctx = KeyContext::new_with_defaults(cx.platform_info().os);
                ctx.add("Onboarding");
                ctx.add("menu");
                ctx
            })
            .track_focus(&self.focus_handle)
            // A tab group of its own, after the frame's groups (title bar, status
            // bar: group 0). Without it the page's stops are top-level ones and Tab
            // walks the whole title bar between the first control and the second.
            .tab_group()
            .tab_index(1)
            .tab_stop(false)
            .size_full()
            .bg(cx.theme().colors().editor_background)
            // The preview follows the focus at once, without waiting for the frame
            // that reports the focus change.
            .on_action(cx.listener(|this, _: &menu::SelectNext, window, cx| {
                window.focus_next(cx);
                this.sync_theme_preview(window, cx);
            }))
            .on_action(cx.listener(|this, _: &menu::SelectPrevious, window, cx| {
                window.focus_prev(cx);
                this.sync_theme_preview(window, cx);
            }))
            .vertical_scrollbar_for(&self.scroll_handle, window, cx)
            .child(
                div()
                    .id("page-content")
                    .w_full()
                    .flex_1()
                    .min_h_0()
                    .overflow_y_scroll()
                    .child(
                        div().w_full().px_4().py_8().child(
                            // One centred column, 720 at most; everything in it starts
                            // on the same 12px inset, as on the welcome page.
                            v_flex()
                                .w_full()
                                .min_w_0()
                                .max_w(rems_from_px(720_f32))
                                .mx_auto()
                                .gap_6()
                                .child(header)
                                .child(sections)
                                .children(inline_footer),
                        ),
                    )
                    .track_scroll(&self.scroll_handle),
            )
            .children(pinned_footer.map(|footer| {
                div().flex_none().w_full().px_4().pb_4().child(
                    div()
                        .w_full()
                        .max_w(rems_from_px(720_f32))
                        .mx_auto()
                        .child(footer),
                )
            }))
    }
}

impl EventEmitter<ItemEvent> for Onboarding {}

impl Focusable for Onboarding {
    fn focus_handle(&self, _: &App) -> gpui::FocusHandle {
        self.focus_handle.clone()
    }
}

impl Item for Onboarding {
    type Event = ItemEvent;

    fn tab_content_text(&self, _detail: usize, _cx: &App) -> SharedString {
        "Get Started".into()
    }

    fn show_toolbar(&self) -> bool {
        false
    }

    fn can_split(&self) -> bool {
        true
    }

    fn clone_on_split(
        &self,
        _workspace_id: Option<WorkspaceId>,
        _: &mut Window,
        cx: &mut Context<Self>,
    ) -> Task<Option<Entity<Self>>> {
        let workspace = self.workspace.clone();
        Task::ready(Some(cx.new(|cx| Onboarding::build(workspace, cx))))
    }

    fn to_item_events(event: &Self::Event, f: &mut dyn FnMut(workspace::item::ItemEvent)) {
        f(*event)
    }
}

/// Replaces the first-launch page by the welcome page, in the pane that holds it.
fn go_to_welcome_page(workspace: &mut Workspace, window: &mut Window, cx: &mut Context<Workspace>) {
    let Some((pane, onboarding, onboarding_idx)) = workspace.panes().iter().find_map(|pane| {
        pane.read(cx)
            .items()
            .enumerate()
            .find_map(|(idx, item)| Some((pane.clone(), item.downcast::<Onboarding>()?, idx)))
    }) else {
        return;
    };

    // A theme that was only being looked at is not kept.
    onboarding.update(cx, |onboarding, cx| onboarding.end_theme_preview(cx));

    let weak_workspace = workspace.weak_handle();
    pane.update(cx, |pane, cx| {
        // Get the index here to get around the borrow checker
        let idx = pane.items().enumerate().find_map(|(idx, item)| {
            let _ = item.downcast::<WelcomePage>()?;
            Some(idx)
        });

        if let Some(idx) = idx {
            pane.activate_item(idx, true, true, window, cx);
        } else {
            let item = Box::new(cx.new(|cx| WelcomePage::new(weak_workspace, false, window, cx)));
            pane.add_item(item, true, true, Some(onboarding_idx), window, cx);
        }

        pane.remove_item(onboarding.entity_id(), false, false, window, cx);
    });
}

pub async fn handle_import_vscode_settings(
    workspace: WeakEntity<Workspace>,
    source: VsCodeSettingsSource,
    skip_prompt: bool,
    fs: Arc<dyn Fs>,
    cx: &mut AsyncWindowContext,
) {
    use util::truncate_and_remove_front;

    let vscode_settings =
        match settings::VsCodeSettings::load_user_settings(source, fs.clone()).await {
            Ok(vscode_settings) => vscode_settings,
            Err(err) => {
                zlog::error!("{err:?}");
                let _ = cx.prompt(
                    gpui::PromptLevel::Info,
                    &format!("Could not find or load a {source} settings file"),
                    None,
                    &["OK"],
                );
                return;
            }
        };

    if !skip_prompt {
        let prompt = cx.prompt(
            gpui::PromptLevel::Warning,
            &format!(
                "Importing {} settings may overwrite your existing settings. \
                Will import settings from {}",
                vscode_settings.source,
                truncate_and_remove_front(&vscode_settings.path.to_string_lossy(), 128),
            ),
            None,
            &["Import", "Cancel"],
        );
        let result = cx.spawn(async move |_| prompt.await.ok()).await;
        if result != Some(0) {
            return;
        }
    };

    let Ok(result_channel) = cx.update(|_, cx| {
        let source = vscode_settings.source;
        let path = vscode_settings.path.clone();
        let result_channel = cx
            .global::<SettingsStore>()
            .import_vscode_settings(fs, vscode_settings);
        zlog::info!("Imported {source} settings from {}", path.display());
        result_channel
    }) else {
        return;
    };

    let result = result_channel.await;
    workspace
        .update_in(cx, |workspace, _, cx| match result {
            Ok(_) => {
                let confirmation_toast = StatusToast::new(
                    format!("Your {} settings were successfully imported.", source),
                    cx,
                    |this, _| {
                        this.icon(
                            Icon::new(IconName::Check)
                                .size(IconSize::Small)
                                .color(Color::Success),
                        )
                        .dismiss_button(true)
                    },
                );
                SettingsImportState::update(cx, |state, _| match source {
                    VsCodeSettingsSource::VsCode => {
                        state.vscode = true;
                    }
                    VsCodeSettingsSource::Cursor => {
                        state.cursor = true;
                    }
                });
                workspace.toggle_status_toast(confirmation_toast, cx);
            }
            Err(_) => {
                let error_toast = StatusToast::new(
                    "Failed to import settings. See log for details",
                    cx,
                    |this, _| {
                        this.icon(
                            Icon::new(IconName::Close)
                                .size(IconSize::Small)
                                .color(Color::Error),
                        )
                        .action("Open Log", |window, cx| {
                            window.dispatch_action(workspace::OpenLog.boxed_clone(), cx)
                        })
                        .dismiss_button(true)
                    },
                );
                workspace.toggle_status_toast(error_toast, cx);
            }
        })
        .ok();
}

#[derive(Default, Copy, Clone)]
pub struct SettingsImportState {
    pub cursor: bool,
    pub vscode: bool,
}

impl Global for SettingsImportState {}

impl SettingsImportState {
    pub fn global(cx: &App) -> Self {
        cx.try_global().cloned().unwrap_or_default()
    }
    pub fn update<R>(cx: &mut App, f: impl FnOnce(&mut Self, &mut App) -> R) -> R {
        cx.update_default_global(f)
    }
}

impl workspace::SerializableItem for Onboarding {
    fn serialized_item_kind() -> &'static str {
        "OnboardingPage"
    }

    fn cleanup(
        workspace_id: workspace::WorkspaceId,
        alive_items: Vec<workspace::ItemId>,
        _window: &mut Window,
        cx: &mut App,
    ) -> gpui::Task<gpui::Result<()>> {
        workspace::delete_unloaded_items(
            alive_items,
            workspace_id,
            "onboarding_pages",
            &persistence::OnboardingPagesDb::global(cx),
            cx,
        )
    }

    fn deserialize(
        _project: Entity<project::Project>,
        workspace: WeakEntity<Workspace>,
        workspace_id: workspace::WorkspaceId,
        item_id: workspace::ItemId,
        window: &mut Window,
        cx: &mut App,
    ) -> gpui::Task<gpui::Result<Entity<Self>>> {
        let db = persistence::OnboardingPagesDb::global(cx);
        window.spawn(cx, async move |cx| {
            if let Some(_) = db.get_onboarding_page(item_id, workspace_id)? {
                workspace.update(cx, |workspace, cx| Onboarding::new(workspace, cx))
            } else {
                Err(anyhow::anyhow!("No onboarding page to deserialize"))
            }
        })
    }

    fn serialize(
        &mut self,
        workspace: &mut Workspace,
        item_id: workspace::ItemId,
        _closing: bool,
        cx: &mut ui::Context<Self>,
    ) -> Option<gpui::Task<gpui::Result<()>>> {
        let workspace_id = workspace.database_id()?;

        let db = persistence::OnboardingPagesDb::global(cx);
        Some(
            cx.background_spawn(
                async move { db.save_onboarding_page(item_id, workspace_id).await },
            ),
        )
    }

    fn should_serialize(&self, event: &Self::Event) -> bool {
        event == &ItemEvent::UpdateTab
    }
}

mod persistence {
    use db::{
        query,
        sqlez::{domain::Domain, thread_safe_connection::ThreadSafeConnection},
        sqlez_macros::sql,
    };
    use workspace::WorkspaceDb;

    pub struct OnboardingPagesDb(ThreadSafeConnection);

    impl Domain for OnboardingPagesDb {
        const NAME: &str = stringify!(OnboardingPagesDb);

        const MIGRATIONS: &[&str] = &[
            sql!(
                        CREATE TABLE onboarding_pages (
                            workspace_id INTEGER,
                            item_id INTEGER UNIQUE,
                            page_number INTEGER,

                            PRIMARY KEY(workspace_id, item_id),
                            FOREIGN KEY(workspace_id) REFERENCES workspaces(workspace_id)
                            ON DELETE CASCADE
                        ) STRICT;
            ),
            sql!(
                        CREATE TABLE onboarding_pages_2 (
                            workspace_id INTEGER,
                            item_id INTEGER UNIQUE,

                            PRIMARY KEY(workspace_id, item_id),
                            FOREIGN KEY(workspace_id) REFERENCES workspaces(workspace_id)
                            ON DELETE CASCADE
                        ) STRICT;
                        INSERT INTO onboarding_pages_2 SELECT workspace_id, item_id FROM onboarding_pages;
                        DROP TABLE onboarding_pages;
                        ALTER TABLE onboarding_pages_2 RENAME TO onboarding_pages;
            ),
        ];
    }

    db::static_connection!(OnboardingPagesDb, [WorkspaceDb]);

    impl OnboardingPagesDb {
        query! {
            pub async fn save_onboarding_page(
                item_id: workspace::ItemId,
                workspace_id: workspace::WorkspaceId
            ) -> Result<()> {
                INSERT OR REPLACE INTO onboarding_pages(item_id, workspace_id)
                VALUES (?, ?)
            }
        }

        query! {
            pub fn get_onboarding_page(
                item_id: workspace::ItemId,
                workspace_id: workspace::WorkspaceId
            ) -> Result<Option<workspace::ItemId>> {
                SELECT item_id
                FROM onboarding_pages
                WHERE item_id = ? AND workspace_id = ?
            }
        }
    }
}

use crate::{
    NewFile, Open, OpenMode, PathList, RecentWorkspace, SerializedWorkspaceLocation, Workspace,
    WorkspaceSettings,
    item::{Item, ItemEvent},
    persistence::WorkspaceDb,
};
use agent_settings::AgentSettings;
use git::Clone as GitClone;
use gpui::{
    Action, App, Context, Entity, EventEmitter, FocusHandle, Focusable, FontWeight, Global, Role,
    StyleRefinement, Task, TaskExt, WeakEntity, Window, actions, relative,
};
use menu::{SelectNext, SelectPrevious};

use app_actions::{OpenKeymap, OpenRecent, OpenSettings};
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use settings::{DefaultOpenBehavior, Settings};
use ui::{DISPLAY_FONT, EmptyState, KeyBinding, SectionLabel, focus_ring, metrics, prelude::*};
use util::{ResultExt, paths::PathExt};

#[derive(PartialEq, Clone, Debug, Deserialize, Serialize, JsonSchema, Action)]
#[action(namespace = welcome)]
#[serde(transparent)]
pub struct OpenRecentProject {
    pub index: usize,
}

actions!(
    app,
    [
        /// Show the welcome page
        ShowWelcome
    ]
);

/// How many recent projects the page lists; "Open Recent…" has the rest.
const MAX_RECENT_PROJECTS: usize = 8;

// These actions live in crates that depend on this one (`agent_ui`, `install_cli`),
// so they are built by name instead of being imported. One that is not registered
// in this build hides its row.
const MANAGE_CONNECTIONS_ACTION: &str = "agent::ManageProviders";
const FOCUS_CHAT_ACTION: &str = "agent::ToggleFocus";
const INSTALL_CLI_ACTION: &str = "cli::InstallCliBinary";

const TAGLINE: &str = "A fast editor made for coding with AI.";
const CONNECT_DETAIL: &str =
    "Unaligned key, a subscription you already have, your own key or a local model.";

/// What the welcome page knows about the AI connection.
///
/// This crate cannot name the crates that know which models are connected
/// (`language_model`, `agent_ui`), so the one that knows publishes the state
/// with [`AiConnection::set`] whenever it changes. Until someone does, the
/// page shows a neutral row that opens the connection screen.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub enum AiConnection {
    /// Nobody said.
    #[default]
    Unknown,
    /// No model can be used yet.
    NotConnected,
    /// A model is ready; `model_name` is the default model's display name.
    Connected { model_name: SharedString },
}

impl Global for AiConnection {}

impl AiConnection {
    /// Publishes the state; open welcome pages follow.
    pub fn set(self, cx: &mut App) {
        if cx.try_global::<Self>() != Some(&self) {
            cx.set_global(self);
        }
    }

    fn get(cx: &App) -> Self {
        cx.try_global::<Self>().cloned().unwrap_or_default()
    }
}

/// The one row of the "AI" section, for a given connection state.
#[derive(Debug, PartialEq)]
struct AiRow {
    label: SharedString,
    description: Option<&'static str>,
    action_name: &'static str,
    /// With nothing connected the row is the page's second action: an outlined
    /// button directly under "Open Folder" instead of a row of the "AI" section.
    prominent: bool,
}

impl AiRow {
    fn new(connection: &AiConnection) -> Self {
        match connection {
            AiConnection::Unknown => Self {
                label: "AI Connections".into(),
                description: Some(CONNECT_DETAIL),
                action_name: MANAGE_CONNECTIONS_ACTION,
                prominent: false,
            },
            AiConnection::NotConnected => Self {
                label: "Connect a Model".into(),
                description: Some(CONNECT_DETAIL),
                action_name: MANAGE_CONNECTIONS_ACTION,
                prominent: true,
            },
            AiConnection::Connected { model_name } => Self {
                label: format!("Chat with {model_name}").into(),
                description: None,
                action_name: FOCUS_CHAT_ACTION,
                prominent: false,
            },
        }
    }
}

/// The brand mark: two overlapping squares, the neutral one behind, the red one in
/// front. Drawn in code, so it is exact at any scale.
fn brand_mark(cx: &App) -> Div {
    // The logo's own red, in every theme: the one fixed colour allowed outside the theme tokens.
    const BRAND_RED: u32 = 0xe5322d;
    let colors = cx.theme().colors();
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
                .bg(colors.text),
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

/// One row of the page's lists: a label, optionally a muted detail on the same line
/// (a project's path) or a muted sentence under it, and the key cap of its action on
/// the right. The hover ground runs from edge to edge of the column; the text sits on
/// the 12px inset shared with the section labels.
#[derive(IntoElement)]
struct WelcomeRow {
    id: ElementId,
    label: SharedString,
    detail: Option<SharedString>,
    description: Option<SharedString>,
    action: Box<dyn Action>,
    tab_index: isize,
    focus_handle: FocusHandle,
}

impl WelcomeRow {
    fn new(
        id: impl Into<ElementId>,
        label: impl Into<SharedString>,
        action: Box<dyn Action>,
        tab_index: isize,
        focus_handle: FocusHandle,
    ) -> Self {
        Self {
            id: id.into(),
            label: label.into(),
            detail: None,
            description: None,
            action,
            tab_index,
            focus_handle,
        }
    }

    fn detail(mut self, detail: impl Into<SharedString>) -> Self {
        self.detail = Some(detail.into());
        self
    }

    fn description(mut self, description: impl Into<SharedString>) -> Self {
        self.description = Some(description.into());
        self
    }
}

impl RenderOnce for WelcomeRow {
    fn render(self, _window: &mut Window, cx: &mut App) -> impl IntoElement {
        let colors = cx.theme().colors();
        let hover_background = colors.ghost_element_hover;
        let active_background = colors.ghost_element_active;
        let focus_ring = focus_ring(cx);
        let has_detail = self.detail.is_some();
        let has_description = self.description.is_some();
        let key_binding = KeyBinding::for_action_in(&*self.action, &self.focus_handle, cx)
            .size(rems_from_px(12_f32));
        let selector_id = self.id.clone();

        h_flex()
            .id(self.id)
            .debug_selector(move || format!("WELCOME-ROW-{selector_id}"))
            .role(Role::Button)
            .aria_label(self.label.clone())
            .tab_index(self.tab_index)
            .w_full()
            .min_h(metrics::row_height(cx))
            .px_3()
            .gap_2()
            .when(has_description, |this| this.py_1p5())
            .justify_between()
            .cursor_pointer()
            .hover(move |style| style.bg(hover_background))
            .active(move |style| style.bg(active_background))
            .focus_visible(move |mut style: StyleRefinement| {
                style.box_shadow = Some(focus_ring);
                style
            })
            .child(
                v_flex()
                    .flex_1()
                    .min_w_0()
                    .gap_0p5()
                    .child(
                        h_flex()
                            .w_full()
                            .min_w_0()
                            .gap_2()
                            .child(
                                // A long name gives way to its path only past two thirds
                                // of the row.
                                div()
                                    .min_w_0()
                                    .when(has_detail, |this| {
                                        this.flex_shrink_0().max_w(relative(0.66))
                                    })
                                    .child(Label::new(self.label).truncate()),
                            )
                            .when_some(self.detail, |this, detail| {
                                this.child(
                                    div().flex_1().min_w_0().child(
                                        Label::new(detail)
                                            .size(LabelSize::Small)
                                            .color(Color::Muted)
                                            .truncate(),
                                    ),
                                )
                            }),
                    )
                    .when_some(self.description, |this, description| {
                        this.child(
                            Label::new(description)
                                .size(LabelSize::Small)
                                .color(Color::Muted),
                        )
                    }),
            )
            // The key cap is never squeezed: a wide one ("Ctrl+Shift+P") shortens the
            // label instead.
            .child(div().flex_none().child(key_binding))
            .on_click(move |_, window, cx| {
                self.focus_handle.dispatch_action(&*self.action, window, cx)
            })
    }
}

/// A quiet text link of the footer.
#[derive(IntoElement)]
struct FooterLink {
    id: ElementId,
    label: SharedString,
    action: Box<dyn Action>,
    tab_index: isize,
    focus_handle: FocusHandle,
}

impl RenderOnce for FooterLink {
    fn render(self, _window: &mut Window, cx: &mut App) -> impl IntoElement {
        let colors = cx.theme().colors();
        let hover_color = colors.text;
        let focus_ring = focus_ring(cx);

        div()
            .id(self.id)
            .role(Role::Button)
            .aria_label(self.label.clone())
            .tab_index(self.tab_index)
            .cursor_pointer()
            .text_ui_sm(cx)
            .text_color(colors.text_muted)
            .hover(move |style| style.text_color(hover_color))
            .focus_visible(move |mut style: StyleRefinement| {
                style.box_shadow = Some(focus_ring);
                style
            })
            .child(self.label)
            .on_click(move |_, window, cx| {
                self.focus_handle.dispatch_action(&*self.action, window, cx)
            })
    }
}

pub struct WelcomePage {
    workspace: WeakEntity<Workspace>,
    focus_handle: FocusHandle,
    /// `None` until the list has been read from the database.
    recent_workspaces: Option<Vec<RecentWorkspace>>,
}

impl WelcomePage {
    /// `_fallback_to_recent_projects` is kept for the callers: the page always lists the
    /// recent projects.
    pub fn new(
        workspace: WeakEntity<Workspace>,
        _fallback_to_recent_projects: bool,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Self {
        let focus_handle = cx.focus_handle();
        cx.on_focus(&focus_handle, window, |_, _, cx| cx.notify())
            .detach();
        cx.observe_global::<AiConnection>(|_, cx| cx.notify())
            .detach();

        // The page is often built from inside an update of its workspace
        // (`app::ShowWelcome`, `onboarding::Finish`), where reading that workspace
        // panics. Nothing here touches it: the task below reads it once the caller
        // has returned.
        let db = WorkspaceDb::global(cx);
        cx.spawn_in(window, {
            let workspace = workspace.clone();
            async move |this: WeakEntity<Self>, cx| {
                let Ok(fs) =
                    workspace.read_with(cx, |workspace, _| workspace.app_state().fs.clone())
                else {
                    return;
                };
                let workspaces = db
                    .recent_project_workspaces(fs.as_ref())
                    .await
                    .log_err()
                    .unwrap_or_default();

                this.update(cx, |this, cx| {
                    this.recent_workspaces = Some(workspaces);
                    cx.notify();
                })
                .ok();
            }
        })
        .detach();

        WelcomePage {
            workspace,
            focus_handle,
            recent_workspaces: None,
        }
    }

    fn select_next(&mut self, _: &SelectNext, window: &mut Window, cx: &mut Context<Self>) {
        window.focus_next(cx);
        cx.notify();
    }

    fn select_previous(&mut self, _: &SelectPrevious, window: &mut Window, cx: &mut Context<Self>) {
        window.focus_prev(cx);
        cx.notify();
    }

    fn open_recent_project(
        &mut self,
        action: &OpenRecentProject,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let Some(recent) = self
            .recent_workspaces
            .as_ref()
            .and_then(|workspaces| workspaces.get(action.index))
        else {
            return;
        };

        if matches!(recent.location, SerializedWorkspaceLocation::Local) {
            let paths = recent.paths.paths().to_vec();
            let open_mode = match WorkspaceSettings::get_global(cx).default_open_behavior {
                DefaultOpenBehavior::ExistingWindow => OpenMode::Activate,
                DefaultOpenBehavior::NewWindow => OpenMode::NewWindow,
            };
            self.workspace
                .update(cx, |workspace, cx| {
                    workspace
                        .open_workspace_for_paths(open_mode, paths, window, cx)
                        .detach_and_log_err(cx);
                })
                .log_err();
        } else {
            window.dispatch_action(OpenRecent::default().boxed_clone(), cx);
        }
    }

    fn row(
        &self,
        id: impl Into<ElementId>,
        label: impl Into<SharedString>,
        action: Box<dyn Action>,
        tab_index: &mut isize,
    ) -> WelcomeRow {
        let index = *tab_index;
        *tab_index += 1;
        WelcomeRow::new(id, label, action, index, self.focus_handle.clone())
    }

    fn render_header(&self, cx: &mut Context<Self>) -> Div {
        v_flex()
            .w_full()
            .px_3()
            .gap_2()
            .child(
                h_flex().w_full().gap_3().child(brand_mark(cx)).child(
                    // The one display line of the page. Unbounded is wide: in a narrow
                    // pane the name wraps instead of pushing the column.
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
                Label::new(TAGLINE)
                    .size(LabelSize::Small)
                    .color(Color::Muted),
            )
    }

    /// `connect` is the AI row when nothing is connected: it is drawn here, as the
    /// page's second action, instead of in a section of its own.
    fn render_start(
        &self,
        connect: Option<(AiRow, Box<dyn Action>)>,
        tab_index: &mut isize,
        cx: &mut Context<Self>,
    ) -> Div {
        let open_folder_index = *tab_index;
        *tab_index += 1;
        let focus_handle = self.focus_handle.clone();
        let connect = connect.map(|(row, action)| {
            let index = *tab_index;
            *tab_index += 1;
            let focus_handle = self.focus_handle.clone();
            v_flex()
                .px_3()
                .pb_1()
                .gap_1p5()
                .items_start()
                .child(
                    // Outlined: the page keeps one filled button.
                    Button::new("welcome-connect-model", row.label)
                        .style(ButtonStyle::Outlined)
                        .size(ButtonSize::Medium)
                        .tab_index(index)
                        .on_click(move |_, window, cx| {
                            focus_handle.dispatch_action(&*action, window, cx)
                        }),
                )
                .children(row.description.map(|description| {
                    Label::new(description)
                        .size(LabelSize::Small)
                        .color(Color::Muted)
                }))
        });

        v_flex()
            .w_full()
            .gap_2()
            .child(div().px_3().child(SectionLabel::new("Start")))
            .child(
                // The page's one filled button.
                h_flex().px_3().pb_1().child(
                    Button::primary("welcome-open-folder", "Open Folder")
                        // D4: buttons are 26px, here as in the chat beside the page.
                        .size(ButtonSize::Medium)
                        .tab_index(open_folder_index)
                        .key_binding(
                            KeyBinding::for_action_in(&Open::DEFAULT, &self.focus_handle, cx)
                                .size(rems_from_px(12_f32)),
                        )
                        .on_click(move |_, window, cx| {
                            focus_handle.dispatch_action(&Open::DEFAULT, window, cx)
                        }),
                ),
            )
            .children(connect)
            .child(
                v_flex()
                    .w_full()
                    .child(self.row(
                        "welcome-clone-repository",
                        "Clone Repository",
                        GitClone.boxed_clone(),
                        tab_index,
                    ))
                    .child(self.row(
                        "welcome-new-file",
                        "New File",
                        NewFile.boxed_clone(),
                        tab_index,
                    ))
                    .child(self.row(
                        "welcome-open-recent",
                        "Open Recent…",
                        OpenRecent::default().boxed_clone(),
                        tab_index,
                    )),
            )
    }

    /// The AI row and its action. `None` when AI is turned off, or when the action of
    /// the row is not registered.
    fn ai_row(&self, cx: &App) -> Option<(AiRow, Box<dyn Action>)> {
        if !AgentSettings::get_global(cx).enabled(cx) {
            return None;
        }
        let ai_row = AiRow::new(&AiConnection::get(cx));
        let action = cx.build_action(ai_row.action_name, None).ok()?;
        Some((ai_row, action))
    }

    fn render_ai(&self, (ai_row, action): (AiRow, Box<dyn Action>), tab_index: &mut isize) -> Div {
        let row = self
            .row("welcome-ai", ai_row.label, action, tab_index)
            .when_some(ai_row.description, |row, description| {
                row.description(description)
            });

        v_flex()
            .w_full()
            .gap_2()
            .child(div().px_3().child(SectionLabel::new("AI")))
            .child(row)
    }

    fn render_recent(&self, tab_index: &mut isize) -> Div {
        let rows = self
            .recent_workspaces
            .iter()
            .flatten()
            .take(MAX_RECENT_PROJECTS)
            .enumerate()
            .map(|(index, recent)| {
                self.row(
                    ("welcome-recent", index),
                    project_name(&recent.identity_paths),
                    OpenRecentProject { index }.boxed_clone(),
                    tab_index,
                )
                .detail(project_location(&recent.location, &recent.identity_paths))
            })
            .collect::<Vec<_>>();
        // Nothing is shown while the list is being read, so the empty state never flashes.
        let is_empty = self
            .recent_workspaces
            .as_ref()
            .is_some_and(|workspaces| workspaces.is_empty());

        v_flex()
            .w_full()
            .child(div().px_3().child(SectionLabel::new("Recent")))
            // The empty state brings its own 12px of air.
            .when(!rows.is_empty(), |this| {
                this.child(v_flex().w_full().pt_2().children(rows))
            })
            .when(is_empty, |this| {
                this.child(
                    EmptyState::new("No recent projects")
                        .description("Folders you open are listed here."),
                )
            })
    }

    fn render_footer(&self, tab_index: &mut isize, cx: &mut Context<Self>) -> Div {
        let mut links: Vec<(&'static str, &'static str, Box<dyn Action>)> = vec![
            ("welcome-settings", "Settings", OpenSettings.boxed_clone()),
            (
                "welcome-keyboard-shortcuts",
                "Keyboard Shortcuts",
                OpenKeymap.boxed_clone(),
            ),
        ];
        // The installer links the binary into /usr/local/bin and asks for rights through
        // AppleScript: it only works on macOS.
        if cfg!(target_os = "macos")
            && let Ok(action) = cx.build_action(INSTALL_CLI_ACTION, None)
        {
            links.push(("welcome-install-cli", "Install Command-Line Tool", action));
        }

        h_flex()
            .w_full()
            .px_3()
            .pt_4()
            .gap_x_6()
            .gap_y_2()
            .flex_wrap()
            .border_t_1()
            .border_color(cx.theme().colors().border)
            .children(links.into_iter().map(|(id, label, action)| {
                let index = *tab_index;
                *tab_index += 1;
                FooterLink {
                    id: id.into(),
                    label: label.into(),
                    action,
                    tab_index: index,
                    focus_handle: self.focus_handle.clone(),
                }
            }))
    }
}

impl Render for WelcomePage {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        // The stops start at 1 inside the page's own tab group (see the root below).
        let mut tab_index = 1;

        let header = self.render_header(cx);
        // With nothing connected, "Connect a Model" is the second action of "Start".
        let (connect, ai_row) = match self.ai_row(cx) {
            Some(row) if row.0.prominent => (Some(row), None),
            other => (None, other),
        };
        let start = self.render_start(connect, &mut tab_index, cx);
        let ai = ai_row.map(|row| self.render_ai(row, &mut tab_index));
        let recent = self.render_recent(&mut tab_index);
        let footer = self.render_footer(&mut tab_index, cx);

        div()
            .key_context("Welcome")
            .track_focus(&self.focus_handle(cx))
            // A tab group of its own, after the frame's groups (title bar, status bar:
            // group 0). Without it the page's stops are top-level ones and Tab walks the
            // whole title bar between "Open Folder" and the next row.
            .tab_group()
            .tab_index(1)
            .tab_stop(false)
            .on_action(cx.listener(Self::select_previous))
            .on_action(cx.listener(Self::select_next))
            .on_action(cx.listener(Self::open_recent_project))
            .size_full()
            .bg(cx.theme().colors().editor_background)
            .child(
                div()
                    .id("welcome-content")
                    .size_full()
                    .overflow_y_scroll()
                    .child(
                        div().w_full().px_4().py_8().child(
                            // One centred column, 720 at most; everything in it starts
                            // on the same 12px inset.
                            v_flex()
                                .w_full()
                                .min_w_0()
                                .max_w(rems_from_px(720_f32))
                                .mx_auto()
                                .gap_8()
                                .child(header)
                                .child(
                                    h_flex()
                                        .w_full()
                                        .items_start()
                                        .flex_wrap()
                                        // 24 between the columns leaves each row room
                                        // for its longest label and a 200px key cap.
                                        .gap_6()
                                        .child(
                                            v_flex()
                                                .flex_1()
                                                .min_w(rems_from_px(280_f32))
                                                .gap_6()
                                                .child(start)
                                                .children(ai),
                                        )
                                        .child(
                                            v_flex()
                                                .flex_1()
                                                .min_w(rems_from_px(280_f32))
                                                .child(recent),
                                        ),
                                )
                                .child(footer),
                        ),
                    ),
            )
    }
}

impl EventEmitter<ItemEvent> for WelcomePage {}

impl Focusable for WelcomePage {
    fn focus_handle(&self, _: &App) -> gpui::FocusHandle {
        self.focus_handle.clone()
    }
}

impl Item for WelcomePage {
    type Event = ItemEvent;

    fn tab_content_text(&self, _detail: usize, _cx: &App) -> SharedString {
        "Welcome".into()
    }

    fn show_toolbar(&self) -> bool {
        false
    }

    fn to_item_events(event: &Self::Event, f: &mut dyn FnMut(crate::item::ItemEvent)) {
        f(*event)
    }
}

impl crate::SerializableItem for WelcomePage {
    fn serialized_item_kind() -> &'static str {
        "WelcomePage"
    }

    fn cleanup(
        workspace_id: crate::WorkspaceId,
        alive_items: Vec<crate::ItemId>,
        _window: &mut Window,
        cx: &mut App,
    ) -> Task<gpui::Result<()>> {
        crate::delete_unloaded_items(
            alive_items,
            workspace_id,
            "welcome_pages",
            &persistence::WelcomePagesDb::global(cx),
            cx,
        )
    }

    fn deserialize(
        _project: Entity<project::Project>,
        workspace: gpui::WeakEntity<Workspace>,
        workspace_id: crate::WorkspaceId,
        item_id: crate::ItemId,
        window: &mut Window,
        cx: &mut App,
    ) -> Task<gpui::Result<Entity<Self>>> {
        if persistence::WelcomePagesDb::global(cx)
            .get_welcome_page(item_id, workspace_id)
            .ok()
            .is_some_and(|is_open| is_open)
        {
            Task::ready(Ok(
                cx.new(|cx| WelcomePage::new(workspace, false, window, cx))
            ))
        } else {
            Task::ready(Err(anyhow::anyhow!("No welcome page to deserialize")))
        }
    }

    fn serialize(
        &mut self,
        workspace: &mut Workspace,
        item_id: crate::ItemId,
        _closing: bool,
        cx: &mut Context<Self>,
    ) -> Option<Task<gpui::Result<()>>> {
        let workspace_id = workspace.database_id()?;
        let db = persistence::WelcomePagesDb::global(cx);
        Some(cx.background_spawn(
            async move { db.save_welcome_page(item_id, workspace_id, true).await },
        ))
    }

    fn should_serialize(&self, event: &Self::Event) -> bool {
        event == &ItemEvent::UpdateTab
    }
}

mod persistence {
    use crate::WorkspaceDb;
    use db::{
        query,
        sqlez::{domain::Domain, thread_safe_connection::ThreadSafeConnection},
        sqlez_macros::sql,
    };

    pub struct WelcomePagesDb(ThreadSafeConnection);

    impl Domain for WelcomePagesDb {
        const NAME: &str = stringify!(WelcomePagesDb);

        const MIGRATIONS: &[&str] = (&[sql!(
                    CREATE TABLE welcome_pages (
                        workspace_id INTEGER,
                        item_id INTEGER UNIQUE,
                        is_open INTEGER DEFAULT FALSE,

                        PRIMARY KEY(workspace_id, item_id),
                        FOREIGN KEY(workspace_id) REFERENCES workspaces(workspace_id)
                        ON DELETE CASCADE
                    ) STRICT;
        )]);
    }

    db::static_connection!(WelcomePagesDb, [WorkspaceDb]);

    impl WelcomePagesDb {
        query! {
            pub async fn save_welcome_page(
                item_id: crate::ItemId,
                workspace_id: crate::WorkspaceId,
                is_open: bool
            ) -> Result<()> {
                INSERT OR REPLACE INTO welcome_pages(item_id, workspace_id, is_open)
                VALUES (?, ?, ?)
            }
        }

        query! {
            pub fn get_welcome_page(
                item_id: crate::ItemId,
                workspace_id: crate::WorkspaceId
            ) -> Result<bool> {
                SELECT is_open
                FROM welcome_pages
                WHERE item_id = ? AND workspace_id = ?
            }
        }
    }
}

/// Muted detail of a recent project row: where it lives, with the home directory shortened.
fn project_location(location: &SerializedWorkspaceLocation, paths: &PathList) -> String {
    let paths = paths.paths();
    let mut text = match paths.first() {
        Some(path) => path.compact().to_string_lossy().into_owned(),
        None => String::new(),
    };
    if paths.len() > 1 {
        text.push_str(&format!(" +{}", paths.len() - 1));
    }
    match location {
        SerializedWorkspaceLocation::Local => text,
        SerializedWorkspaceLocation::Remote(options) => {
            format!("{}: {}", options.display_name(), text)
        }
    }
}

fn project_name(paths: &PathList) -> String {
    let joined = paths
        .paths()
        .iter()
        .filter_map(|p| p.file_name().map(|n| n.to_string_lossy().to_string()))
        .collect::<Vec<_>>()
        .join(", ");
    if joined.is_empty() {
        "Untitled".to_string()
    } else {
        joined
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use fs::FakeFs;
    use gpui::{TestAppContext, VisualTestContext};
    use project::Project;
    use settings::SettingsStore;

    fn init_test(cx: &mut TestAppContext) {
        cx.update(|cx| {
            let settings_store = SettingsStore::test(cx);
            cx.set_global(settings_store);
            cx.set_global(db::AppDatabase::test_new());
            theme_settings::init(theme::LoadThemes::JustBase, cx);
        });
    }

    async fn test_workspace(
        cx: &mut TestAppContext,
    ) -> (Entity<Workspace>, &mut VisualTestContext) {
        init_test(cx);
        let fs = FakeFs::new(cx.executor());
        let project = Project::test(fs, [], cx).await;
        cx.add_window_view(|window, cx| Workspace::test_new(project, window, cx))
    }

    // `app::ShowWelcome` builds the page inside `Workspace::with_local_workspace`, and
    // `onboarding::Finish` inside the active pane's update inside the workspace's: in both
    // the workspace is being updated while `WelcomePage::new` runs.
    #[gpui::test]
    async fn test_page_can_be_built_while_the_workspace_is_updated(cx: &mut TestAppContext) {
        let (workspace, cx) = test_workspace(cx).await;

        let page = workspace.update_in(cx, |workspace, window, cx| {
            let page = cx.new(|cx| WelcomePage::new(workspace.weak_handle(), false, window, cx));
            workspace.add_item_to_active_pane(Box::new(page.clone()), None, true, window, cx);
            page
        });
        workspace.update_in(cx, |workspace, window, cx| {
            workspace.active_pane().update(cx, |pane, cx| {
                let item = Box::new(
                    cx.new(|cx| WelcomePage::new(workspace.weak_handle(), false, window, cx)),
                );
                pane.add_item(item, true, true, None, window, cx);
            });
        });
        cx.run_until_parked();

        // The recent projects were asked for once the update was over.
        page.read_with(cx, |page, _| assert!(page.recent_workspaces.is_some()));
    }

    #[gpui::test]
    async fn test_rows_are_drawn_and_follow_the_ai_connection(cx: &mut TestAppContext) {
        let (workspace, cx) = test_workspace(cx).await;

        workspace.update_in(cx, |workspace, window, cx| {
            let page = cx.new(|cx| WelcomePage::new(workspace.weak_handle(), false, window, cx));
            workspace.add_item_to_active_pane(Box::new(page), None, true, window, cx);
        });
        cx.run_until_parked();

        for selector in [
            "WELCOME-ROW-welcome-clone-repository",
            "WELCOME-ROW-welcome-new-file",
            "WELCOME-ROW-welcome-open-recent",
        ] {
            assert!(cx.debug_bounds(selector).is_some(), "{selector} is missing");
        }
        // The crate that registers `agent::ManageProviders` is not part of this test, so
        // the row that needs it is left out; `agent::ToggleFocus` is known everywhere.
        for connection in [AiConnection::Unknown, AiConnection::NotConnected] {
            cx.update(|_, cx| connection.set(cx));
            cx.run_until_parked();
            assert!(cx.debug_bounds("WELCOME-ROW-welcome-ai").is_none());
        }
        // An open page follows the published state.
        cx.update(|_, cx| {
            AiConnection::Connected {
                model_name: "Claude Sonnet 5.5".into(),
            }
            .set(cx)
        });
        cx.run_until_parked();
        assert!(cx.debug_bounds("WELCOME-ROW-welcome-ai").is_some());
    }

    // The page is one tab group: Tab walks its own stops in order and does not leave
    // for the frame (title bar, status bar) between "Open Folder" and the next row.
    #[gpui::test]
    async fn test_tab_stays_in_the_page_between_its_rows(cx: &mut TestAppContext) {
        let (workspace, cx) = test_workspace(cx).await;

        let page = workspace.update_in(cx, |workspace, window, cx| {
            let page = cx.new(|cx| WelcomePage::new(workspace.weak_handle(), false, window, cx));
            workspace.add_item_to_active_pane(Box::new(page.clone()), None, true, window, cx);
            page
        });
        cx.run_until_parked();

        let focus_handle = page.read_with(cx, |page, _| page.focus_handle.clone());
        cx.update(|window, cx| window.focus(&focus_handle, cx));
        cx.run_until_parked();

        // Open Folder, Clone Repository, New File, Open Recent…
        let mut focused = Vec::new();
        for stop in 0..4 {
            cx.update(|window, cx| window.focus_next(cx));
            cx.run_until_parked();
            cx.update(|window, cx| {
                assert!(
                    focus_handle.contains_focused(window, cx),
                    "stop {stop} left the welcome page"
                );
                assert!(
                    !focus_handle.is_focused(window),
                    "stop {stop} is the page itself, not one of its rows"
                );
                focused.push(window.focused(cx));
            });
        }
        // Each Tab moved to another stop.
        for pair in focused.windows(2) {
            assert!(pair[0] != pair[1], "Tab did not move the focus");
        }
    }

    #[test]
    fn test_ai_row_follows_the_connection() {
        let unknown = AiRow::new(&AiConnection::Unknown);
        assert_eq!(unknown.label, "AI Connections");
        assert_eq!(unknown.description, Some(CONNECT_DETAIL));
        assert_eq!(unknown.action_name, MANAGE_CONNECTIONS_ACTION);
        assert!(!unknown.prominent);

        let not_connected = AiRow::new(&AiConnection::NotConnected);
        assert_eq!(not_connected.label, "Connect a Model");
        assert_eq!(not_connected.description, Some(CONNECT_DETAIL));
        assert_eq!(not_connected.action_name, MANAGE_CONNECTIONS_ACTION);
        // The page's second action, directly under "Open Folder".
        assert!(not_connected.prominent);

        let connected = AiRow::new(&AiConnection::Connected {
            model_name: "Claude Sonnet 5.5".into(),
        });
        assert_eq!(connected.label, "Chat with Claude Sonnet 5.5");
        assert_eq!(connected.description, None);
        assert_eq!(connected.action_name, FOCUS_CHAT_ACTION);
        assert!(!connected.prominent);
    }

    #[test]
    fn test_project_name_empty() {
        let paths = PathList::new::<&str>(&[]);
        assert_eq!(project_name(&paths), "Untitled");
    }

    #[test]
    fn test_project_name_single() {
        let paths = PathList::new(&["/home/user/my-project"]);
        assert_eq!(project_name(&paths), "my-project");
    }

    #[test]
    fn test_project_name_multiple() {
        // PathList sorts lexicographically, so filenames appear in alpha order
        let paths = PathList::new(&["/home/user/zed", "/home/user/api"]);
        assert_eq!(project_name(&paths), "api, zed");
    }

    #[test]
    fn test_project_name_root_path_filtered() {
        // A bare root "/" has no file_name(), falls back to "Untitled"
        let paths = PathList::new(&["/"]);
        assert_eq!(project_name(&paths), "Untitled");
    }
}

//! The Servers panel: the SSH servers a person works on, one click away.
//!
//! It lists the `ssh_connections` of the settings. A click on a server
//! connects and shows the folders of the host; a click on a folder opened
//! before goes straight back to it. The plus button and the pencil open
//! [`ServerForm`](crate::server_form::ServerForm), where a server is described
//! in a few fields instead of an `ssh` command line.

use std::{path::PathBuf, sync::Arc};

use anyhow::Result;
use db::kvp::KeyValueStore;
use fs::Fs;
use futures::StreamExt as _;
use gpui::{
    Action, App, AsyncWindowContext, Context, Entity, EventEmitter, FocusHandle, Focusable,
    IntoElement, ParentElement, Pixels, PromptLevel, Render, SharedString, Styled, Subscription,
    Task, WeakEntity, Window, actions, px,
};
use remote::{RemoteConnectionOptions, SshConnectionOptions};
use settings::{
    RemoteProject, Settings as _, SettingsStore, SshConnection, update_settings_file,
    watch_config_file,
};
use ui::{IconButton, ListItem, PanelHeader, SectionLabel, Tooltip, prelude::*};
use util::ResultExt as _;
use workspace::{
    MultiWorkspace, OpenOptions, Workspace,
    dock::{DockPosition, Panel, PanelEvent},
};

use crate::{
    RemoteServerProjects, RemoteSettings, default_open_in_new_window, open_remote_project,
    remote_servers::{ServerIndex, SshServerIndex},
    server_form::ServerForm,
    server_store,
    ssh_config::parse_ssh_config_hosts,
};

actions!(
    servers_panel,
    [
        /// Shows the Servers panel, or gives it the keyboard.
        ToggleFocus,
        /// Opens the form that adds a server.
        NewServer
    ]
);

const POSITION_KEY: &str = "servers_panel.position";

pub fn init(cx: &mut App) {
    cx.observe_new(|workspace: &mut Workspace, _, _| {
        workspace.register_action(|workspace, _: &ToggleFocus, window, cx| {
            workspace.toggle_panel_focus::<ServersPanel>(window, cx);
        });
        workspace.register_action(|workspace, _: &NewServer, window, cx| {
            ServerForm::open(workspace, None, window, cx);
        });
    })
    .detach();
}

pub struct ServersPanel {
    workspace: WeakEntity<Workspace>,
    focus_handle: FocusHandle,
    position: DockPosition,
    /// The hosts named in the person's `~/.ssh/config`, kept up to date.
    config_hosts: Vec<String>,
    _settings_changed: Subscription,
    _config_watch: Task<()>,
}

impl ServersPanel {
    pub async fn load(
        workspace: WeakEntity<Workspace>,
        mut cx: AsyncWindowContext,
    ) -> Result<Entity<Self>> {
        let kvp = cx.update(|_, cx| KeyValueStore::global(cx))?;
        let position = cx
            .background_spawn(async move { kvp.read_kvp(POSITION_KEY) })
            .await
            .log_err()
            .flatten();
        let position = match position.as_deref() {
            Some("right") => DockPosition::Right,
            _ => DockPosition::Left,
        };
        workspace.update_in(&mut cx, |workspace, _window, cx| {
            let fs = workspace.app_state().fs.clone();
            let workspace = workspace.weak_handle();
            cx.new(|cx| Self {
                workspace,
                focus_handle: cx.focus_handle(),
                position,
                config_hosts: Vec::new(),
                _settings_changed: cx.observe_global::<SettingsStore>(|_, cx| cx.notify()),
                _config_watch: Self::watch_ssh_config(fs, cx),
            })
        })
    }

    /// Follows the person's SSH configuration file: the hosts it names are
    /// offered under the servers, so that what already works from a terminal
    /// is one click away here too.
    fn watch_ssh_config(fs: Arc<dyn Fs>, cx: &mut Context<Self>) -> Task<()> {
        let (mut contents, watcher) =
            watch_config_file(cx.background_executor(), fs, paths::user_ssh_config_file());
        cx.spawn(async move |this, cx| {
            let _watcher = watcher;
            while let Some(content) = contents.next().await {
                let hosts = parse_ssh_config_hosts(&content);
                let updated = this.update(cx, |this, cx| {
                    this.config_hosts = hosts.into_iter().collect();
                    cx.notify();
                });
                if updated.is_err() {
                    break;
                }
            }
        })
    }

    /// The hosts of the SSH configuration that are not in the list already.
    fn offered_config_hosts(&self, servers: &[SshConnection], cx: &App) -> Vec<String> {
        if !RemoteSettings::get_global(cx).read_ssh_config {
            return Vec::new();
        }
        self.config_hosts
            .iter()
            .filter(|host| !servers.iter().any(|server| &server.host == *host))
            .cloned()
            .collect()
    }

    /// Adds a host of the SSH configuration to the servers and connects to
    /// it. `ssh` reads the rest (address, user, key, jump host) from the
    /// configuration file, as it does in a terminal.
    fn connect_config_host(&mut self, host: String, window: &mut Window, cx: &mut Context<Self>) {
        let Some(workspace) = self.workspace.upgrade() else {
            return;
        };
        let index = Self::servers(cx).len();
        let server = SshConnection {
            host,
            ..Default::default()
        };
        let added = server.clone();
        self.update_servers(cx, move |servers| servers.push(added));
        connect_to_server(workspace, index, server, window, cx);
    }

    fn servers(cx: &App) -> Vec<SshConnection> {
        RemoteSettings::get_global(cx).ssh_connections().collect()
    }

    /// The server this window's project lives on, as an entry of `servers`.
    fn connected_server(&self, servers: &[SshConnection], cx: &App) -> Option<usize> {
        let workspace = self.workspace.upgrade()?;
        let project = workspace.read(cx).project().read(cx);
        let RemoteConnectionOptions::Ssh(options) = project.remote_connection_options(cx)? else {
            return None;
        };
        let host = options.host.to_string();
        servers.iter().position(|server| {
            server.host == host
                && server.username == options.username
                && server.port == options.port
        })
    }

    fn new_server(&mut self, _: &NewServer, window: &mut Window, cx: &mut Context<Self>) {
        self.open_form(None, window, cx);
    }

    fn open_form(&mut self, editing: Option<usize>, window: &mut Window, cx: &mut Context<Self>) {
        let Some(workspace) = self.workspace.upgrade() else {
            return;
        };
        // The form is a modal of the workspace, which must not be borrowed
        // while this panel, one of its children, is.
        window.defer(cx, move |window, cx| {
            workspace.update(cx, |workspace, cx| {
                ServerForm::open(workspace, editing, window, cx);
            });
        });
    }

    /// Connects to a server and shows the folders of the host.
    fn connect(&mut self, index: usize, window: &mut Window, cx: &mut Context<Self>) {
        let Some(server) = Self::servers(cx).into_iter().nth(index) else {
            return;
        };
        let Some(workspace) = self.workspace.upgrade() else {
            return;
        };
        connect_to_server(workspace, index, server, window, cx);
    }

    /// Goes back to a folder of a server that was opened before.
    fn open_project(
        &mut self,
        index: usize,
        project: RemoteProject,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let Some(server) = Self::servers(cx).into_iter().nth(index) else {
            return;
        };
        let Some(app_state) = self
            .workspace
            .read_with(cx, |workspace, _| workspace.app_state().clone())
            .log_err()
        else {
            return;
        };
        let replace_window = if default_open_in_new_window(cx) {
            None
        } else {
            window.window_handle().downcast::<MultiWorkspace>()
        };
        let secret_name = server_store::secret_name_of(&server);
        let mut options = SshConnectionOptions::from(server);
        cx.spawn_in(window, async move |_, cx| {
            options.password = server_store::read_secret(&secret_name, cx).await;
            let result = open_remote_project(
                RemoteConnectionOptions::Ssh(options),
                project.paths.into_iter().map(PathBuf::from).collect(),
                app_state,
                OpenOptions {
                    requesting_window: replace_window,
                    ..OpenOptions::default()
                },
                cx,
            )
            .await;
            if let Err(error) = result {
                log::error!("Failed to connect: {error:#}");
                cx.prompt(
                    PromptLevel::Critical,
                    "Failed to connect",
                    Some(&error.to_string()),
                    &["OK"],
                )
                .await
                .ok();
            }
        })
        .detach();
    }

    fn forget_project(&mut self, index: usize, project: RemoteProject, cx: &mut Context<Self>) {
        let Some(server) = Self::servers(cx).into_iter().nth(index) else {
            return;
        };
        self.update_servers(cx, move |servers| {
            if let Some(found) = servers
                .get_mut(index)
                .filter(|found| is_same(found, &server))
            {
                found.projects.remove(&project);
            }
        });
    }

    fn remove_server(&mut self, index: usize, window: &mut Window, cx: &mut Context<Self>) {
        let servers = Self::servers(cx);
        let Some(server) = servers.get(index).cloned() else {
            return;
        };
        let answer = window.prompt(
            PromptLevel::Warning,
            &format!("Remove \"{}\"?", server_store::display_name(&server)),
            Some(
                "It leaves this list, with the password or the key copy kept for it. Nothing changes on the server itself.",
            ),
            &["Remove", "Cancel"],
            cx,
        );
        cx.spawn(async move |this, cx| {
            if answer.await != Ok(0) {
                return;
            }
            server_store::delete_secret(&server_store::secret_name_of(&server), cx).await;
            this.update(cx, |this, cx| {
                let removed = server.clone();
                this.update_servers(cx, move |servers| {
                    if servers
                        .get(index)
                        .is_some_and(|found| is_same(found, &removed))
                    {
                        servers.remove(index);
                    }
                });
                // Its key file, when it is one of ours and no other server has it.
                if let Some(key) = server_store::identity_file(&server.args) {
                    let others: Vec<SshConnection> = servers
                        .iter()
                        .enumerate()
                        .filter(|(other, _)| *other != index)
                        .map(|(_, server)| server.clone())
                        .collect();
                    server_store::remove_key_if_unused(std::path::Path::new(key), &others);
                }
            })
            .ok();
        })
        .detach();
    }

    fn update_servers(
        &mut self,
        cx: &mut Context<Self>,
        update: impl FnOnce(&mut Vec<SshConnection>) + Send + Sync + 'static,
    ) {
        let Some(fs) = self
            .workspace
            .read_with(cx, |workspace, _| workspace.app_state().fs.clone())
            .log_err()
        else {
            return;
        };
        update_settings_file(fs, cx, move |settings, _| {
            update(settings.remote.ssh_connections.get_or_insert_default());
        });
    }

    fn render_server(
        &self,
        index: usize,
        server: &SshConnection,
        is_connected: bool,
        cx: &mut Context<Self>,
    ) -> impl IntoElement {
        let name = server_store::display_name(server);
        let address = server_store::address(server);
        let show_address = address != name;
        let tooltip: SharedString = if is_connected {
            format!("{name}: this window is on it. Click to choose another folder.").into()
        } else {
            format!("Connect to {address} and choose a folder").into()
        };

        ListItem::new(("server", index))
            .start_slot(
                Icon::new(IconName::Server)
                    .size(IconSize::Small)
                    .color(if is_connected {
                        Color::Success
                    } else {
                        Color::Muted
                    }),
            )
            .child(
                h_flex()
                    .w_full()
                    .min_w_0()
                    .gap_2()
                    .overflow_hidden()
                    // The name is what people look for: the address gives way first.
                    .child(div().flex_none().child(Label::new(name)))
                    .when(show_address, |row| {
                        row.child(
                            Label::new(address)
                                .size(LabelSize::Small)
                                .color(Color::Muted)
                                .truncate(),
                        )
                    }),
            )
            .end_slot_on_hover(
                h_flex()
                    .gap_0p5()
                    .child(
                        IconButton::new(("edit-server", index), IconName::Pencil)
                            .icon_size(IconSize::Small)
                            .icon_color(Color::Muted)
                            .tooltip(Tooltip::text("Edit"))
                            .on_click(cx.listener(move |this, _, window, cx| {
                                cx.stop_propagation();
                                this.open_form(Some(index), window, cx);
                            })),
                    )
                    .child(
                        IconButton::new(("remove-server", index), IconName::Trash)
                            .icon_size(IconSize::Small)
                            .icon_color(Color::Muted)
                            .tooltip(Tooltip::text("Remove from the list"))
                            .on_click(cx.listener(move |this, _, window, cx| {
                                cx.stop_propagation();
                                this.remove_server(index, window, cx);
                            })),
                    ),
            )
            .tooltip(Tooltip::text(tooltip))
            .on_click(cx.listener(move |this, _, window, cx| {
                this.connect(index, window, cx);
            }))
    }

    fn render_project(
        &self,
        index: usize,
        project_index: usize,
        project: &RemoteProject,
        cx: &mut Context<Self>,
    ) -> impl IntoElement {
        let label = project.paths.join(", ");
        let id = index * 10_000 + project_index;
        let to_open = project.clone();
        let to_forget = project.clone();
        ListItem::new(("server-folder", id))
            .indent_level(1)
            .start_slot(
                Icon::new(IconName::Folder)
                    .size(IconSize::Small)
                    .color(Color::Muted),
            )
            .child(Label::new(label.clone()).color(Color::Muted).truncate())
            .end_slot_on_hover(
                IconButton::new(("forget-folder", id), IconName::Close)
                    .icon_size(IconSize::Small)
                    .icon_color(Color::Muted)
                    .tooltip(Tooltip::text("Forget this folder"))
                    .on_click(cx.listener(move |this, _, _, cx| {
                        cx.stop_propagation();
                        this.forget_project(index, to_forget.clone(), cx);
                    })),
            )
            .tooltip(Tooltip::text(format!("Open {label}")))
            .on_click(cx.listener(move |this, _, window, cx| {
                this.open_project(index, to_open.clone(), window, cx);
            }))
    }

    fn render_empty(&self, cx: &mut Context<Self>) -> impl IntoElement {
        v_flex()
            .p_3()
            .gap_3()
            .child(
                Label::new("Work on a server over SSH: its files, its terminal and the AI, as on this computer.")
                    .size(LabelSize::Small)
                    .color(Color::Muted),
            )
            .child(
                Button::primary("add-first-server", "Add a Server")
                    .full_width()
                    .on_click(cx.listener(|this, _, window, cx| {
                        this.open_form(None, window, cx);
                    })),
            )
    }
}

/// Whether two entries are the same server: the settings may have changed
/// between a click and what it does.
fn is_same(a: &SshConnection, b: &SshConnection) -> bool {
    a.host == b.host && a.username == b.username && a.port == b.port
}

/// Connects to `server`, entry `index` of the settings, with its saved
/// password or passphrase if it has one, and shows the folders of the host.
pub(crate) fn connect_to_server(
    workspace: Entity<Workspace>,
    index: usize,
    server: SshConnection,
    window: &mut Window,
    cx: &mut App,
) {
    let create_new_window = default_open_in_new_window(cx);
    let secret_name = server_store::secret_name_of(&server);
    let mut options = SshConnectionOptions::from(server);
    window
        .spawn(cx, async move |cx| {
            options.password = server_store::read_secret(&secret_name, cx).await;
            cx.update(|window, cx| {
                RemoteServerProjects::connect_and_pick_project(
                    &workspace,
                    create_new_window,
                    ServerIndex::Ssh(SshServerIndex(index)),
                    RemoteConnectionOptions::Ssh(options),
                    false,
                    window,
                    cx,
                );
            })
            .ok();
        })
        .detach();
}

impl Render for ServersPanel {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let servers = Self::servers(cx);
        let connected = self.connected_server(&servers, cx);

        let header = PanelHeader::new("Servers")
            .id("servers-panel-header")
            .end_slot(
                IconButton::new("new-server", IconName::Plus)
                    .icon_size(IconSize::Small)
                    .icon_color(Color::Muted)
                    .tooltip(Tooltip::text("New Server"))
                    .on_click(cx.listener(|this, _, window, cx| {
                        this.open_form(None, window, cx);
                    })),
            );

        let mut rows = Vec::new();
        for (index, server) in servers.iter().enumerate() {
            rows.push(
                self.render_server(index, server, connected == Some(index), cx)
                    .into_any_element(),
            );
            for (project_index, project) in server.projects.iter().enumerate() {
                rows.push(
                    self.render_project(index, project_index, project, cx)
                        .into_any_element(),
                );
            }
        }

        let config_hosts = self.offered_config_hosts(&servers, cx);
        if !config_hosts.is_empty() {
            rows.push(
                h_flex()
                    .px_3()
                    .pt_3()
                    .pb_1()
                    .child(SectionLabel::new("From your SSH config"))
                    .into_any_element(),
            );
            for (index, host) in config_hosts.into_iter().enumerate() {
                let to_connect = host.clone();
                rows.push(
                    ListItem::new(("config-host", index))
                        .start_slot(
                            Icon::new(IconName::Server)
                                .size(IconSize::Small)
                                .color(Color::Muted),
                        )
                        .child(Label::new(host.clone()).color(Color::Muted).truncate())
                        .tooltip(Tooltip::text(format!(
                            "Connect to {host} as ~/.ssh/config describes it, and add it to the servers"
                        )))
                        .on_click(cx.listener(move |this, _, window, cx| {
                            this.connect_config_host(to_connect.clone(), window, cx);
                        }))
                        .into_any_element(),
                );
            }
        }

        v_flex()
            .id("servers-panel")
            .key_context("ServersPanel")
            .track_focus(&self.focus_handle)
            .on_action(cx.listener(Self::new_server))
            .size_full()
            .child(header)
            .child(if rows.is_empty() {
                self.render_empty(cx).into_any_element()
            } else {
                v_flex()
                    .id("servers-list")
                    .flex_1()
                    .overflow_y_scroll()
                    .py_1()
                    .children(rows)
                    .into_any_element()
            })
    }
}

impl Focusable for ServersPanel {
    fn focus_handle(&self, _: &App) -> FocusHandle {
        self.focus_handle.clone()
    }
}

impl EventEmitter<PanelEvent> for ServersPanel {}

impl Panel for ServersPanel {
    fn persistent_name() -> &'static str {
        "ServersPanel"
    }

    fn panel_key() -> &'static str {
        "ServersPanel"
    }

    fn position(&self, _: &Window, _: &App) -> DockPosition {
        self.position
    }

    fn position_is_valid(&self, position: DockPosition) -> bool {
        matches!(position, DockPosition::Left | DockPosition::Right)
    }

    fn set_position(&mut self, position: DockPosition, _: &mut Window, cx: &mut Context<Self>) {
        self.position = position;
        let value = match position {
            DockPosition::Right => "right",
            _ => "left",
        };
        let kvp = KeyValueStore::global(cx);
        cx.background_spawn(async move {
            kvp.write_kvp(POSITION_KEY.to_string(), value.to_string())
                .await
                .log_err();
        })
        .detach();
        cx.notify();
    }

    fn default_size(&self, _: &Window, _: &App) -> Pixels {
        px(280.)
    }

    fn icon(&self, _: &Window, _: &App) -> Option<IconName> {
        Some(IconName::Server)
    }

    fn icon_tooltip(&self, _: &Window, _: &App) -> Option<&'static str> {
        Some("Servers Panel")
    }

    fn toggle_action(&self) -> Box<dyn Action> {
        Box::new(ToggleFocus)
    }

    fn activation_priority(&self) -> u32 {
        // After source control (3), before the outline (6).
        4
    }
}

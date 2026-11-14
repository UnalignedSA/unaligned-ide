//! The form that adds or edits a server of the Servers panel.
//!
//! A server is an address, a user, and a way to sign in: a password, or a
//! private key. The key can be one found in `~/.ssh`, a file chosen anywhere,
//! or text pasted from the clipboard, in any format people have keys in
//! (`ssh_keys` reads them, and converts a PuTTY key). What the person types as
//! a password or a passphrase goes to the system keychain; the settings file
//! gets the address, the user and the key's path, never a secret.

use std::path::{Path, PathBuf};

use gpui::{
    App, AsyncWindowContext, Context, DismissEvent, Entity, EventEmitter, FocusHandle, Focusable,
    IntoElement, ParentElement, PathPromptOptions, Render, SharedString, Styled, Task, WeakEntity,
    Window,
};
use settings::{Settings as _, SshConnection, update_settings_file};
use ssh_keys::{KeyError, KeyFormat, KeyInfo};
use ui::{Headline, HeadlineSize, ListItem, SectionLabel, prelude::*};
use ui_input::InputField;
use util::ResultExt as _;
use workspace::{ModalView, Workspace};
use zeroize::Zeroizing;

use crate::{
    RemoteSettings,
    server_store::{self, TypedAddress},
    servers_panel::connect_to_server,
};

/// A key file is a few kilobytes. Anything larger is not read.
const MAX_KEY_FILE_BYTES: u64 = 64 * 1024;
/// At most this many keys of `~/.ssh` are offered.
const MAX_FOUND_KEYS: usize = 12;

const PASSWORD_HINT: &str =
    "Optional. Kept in your system's keychain. Left empty, it is asked at each connection.";
const PASSPHRASE_HINT: &str = "Only if the key has one. Kept in your system's keychain.";

#[derive(Clone, Copy, PartialEq, Eq)]
enum SignIn {
    Password,
    Key,
}

/// A private key found in the person's `~/.ssh`.
struct FoundKey {
    path: PathBuf,
    name: String,
    info: KeyInfo,
    /// `ssh` can use the file where it is: it is in a format `ssh` reads and
    /// nobody else on the machine can read it.
    usable_in_place: bool,
}

enum KeyChoice {
    None,
    /// The key the server already has.
    Kept(PathBuf),
    /// One of the keys of `~/.ssh`.
    Found(usize),
    /// A key from a file chosen elsewhere, or pasted.
    New {
        text: Zeroizing<String>,
        info: KeyInfo,
        source: SharedString,
    },
}

/// What saving has to do about the key.
enum KeyPlan {
    /// Sign in with a password.
    None,
    Use(PathBuf),
    /// Make a key file of the application's own from this text.
    Import {
        text: Zeroizing<String>,
        name_hint: String,
    },
    ImportFile {
        path: PathBuf,
        name_hint: String,
    },
}

pub(crate) struct ServerForm {
    workspace: WeakEntity<Workspace>,
    /// The entry being edited, as it was when the form opened.
    editing: Option<(usize, SshConnection)>,
    name: Entity<InputField>,
    address: Entity<InputField>,
    port: Entity<InputField>,
    user: Entity<InputField>,
    secret: Entity<InputField>,
    sign_in: SignIn,
    key: KeyChoice,
    found_keys: Vec<FoundKey>,
    /// What the form says about the chosen key, and whether it is a problem.
    key_note: Option<(SharedString, bool)>,
    has_saved_secret: bool,
    forget_secret: bool,
    error: Option<SharedString>,
    saving: bool,
    _tasks: Vec<Task<()>>,
}

impl EventEmitter<DismissEvent> for ServerForm {}
impl ModalView for ServerForm {}

impl Focusable for ServerForm {
    fn focus_handle(&self, cx: &App) -> FocusHandle {
        self.name.focus_handle(cx)
    }
}

impl ServerForm {
    /// Opens the form over `workspace`, for a new server or for entry
    /// `editing` of the settings.
    pub(crate) fn open(
        workspace: &mut Workspace,
        editing: Option<usize>,
        window: &mut Window,
        cx: &mut Context<Workspace>,
    ) {
        let editing = editing.and_then(|index| {
            RemoteSettings::get_global(cx)
                .ssh_connections()
                .nth(index)
                .map(|server| (index, server))
        });
        let handle = cx.entity().downgrade();
        workspace.toggle_modal(window, cx, |window, cx| {
            Self::new(handle, editing, window, cx)
        });
    }

    fn new(
        workspace: WeakEntity<Workspace>,
        editing: Option<(usize, SshConnection)>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Self {
        let field = |label: &'static str,
                     placeholder: &'static str,
                     index: isize,
                     window: &mut Window,
                     cx: &mut Context<Self>| {
            cx.new(|cx| {
                InputField::new(window, cx, placeholder)
                    .label(label)
                    .tab_index(index)
                    .tab_stop(true)
            })
        };
        let name = field("Name", "Production, my VPS… (optional)", 1, window, cx);
        let address = field("Address", "203.0.113.4 or example.org", 2, window, cx);
        // Narrow: a port is five digits.
        let port = cx.new(|cx| {
            InputField::new(window, cx, "22")
                .label("Port")
                .label_min_width(gpui::px(56.))
                .tab_index(3)
                .tab_stop(true)
        });
        let user = field("User", "root", 4, window, cx);
        let secret = cx.new(|cx| {
            InputField::new(window, cx, "")
                .tab_index(5)
                .tab_stop(true)
                .masked(true)
        });

        let mut sign_in = SignIn::Password;
        let mut key = KeyChoice::None;
        if let Some((_, server)) = &editing {
            let fill = |field: &Entity<InputField>,
                        text: &str,
                        window: &mut Window,
                        cx: &mut Context<Self>| {
                field.update(cx, |field, cx| field.set_text(text, window, cx));
            };
            fill(
                &name,
                server.nickname.as_deref().unwrap_or_default(),
                window,
                cx,
            );
            fill(&address, &server.host, window, cx);
            if let Some(port_number) = server.port {
                fill(&port, &port_number.to_string(), window, cx);
            }
            fill(
                &user,
                server.username.as_deref().unwrap_or_default(),
                window,
                cx,
            );
            if let Some(path) = server_store::identity_file(&server.args) {
                sign_in = SignIn::Key;
                key = KeyChoice::Kept(PathBuf::from(path));
            }
        }

        let mut tasks = Vec::new();
        // The keys of ~/.ssh, read off the main thread.
        let ssh_dir = util::paths::home_dir().join(".ssh");
        tasks.push(cx.spawn(async move |this, cx| {
            let found = cx
                .background_spawn(async move { find_keys(&ssh_dir) })
                .await;
            this.update(cx, |this, cx| {
                this.found_keys = found;
                cx.notify();
            })
            .ok();
        }));
        // Whether a password or a passphrase is already kept for this server.
        if let Some((_, server)) = &editing {
            let secret_name = server_store::secret_name_of(server);
            tasks.push(cx.spawn(async move |this, cx| {
                let saved = server_store::read_secret(&secret_name, cx).await.is_some();
                this.update(cx, |this, cx| {
                    this.has_saved_secret = saved;
                    cx.notify();
                })
                .ok();
            }));
        }

        Self {
            workspace,
            editing,
            name,
            address,
            port,
            user,
            secret,
            sign_in,
            key,
            found_keys: Vec::new(),
            key_note: None,
            has_saved_secret: false,
            forget_secret: false,
            error: None,
            saving: false,
            _tasks: tasks,
        }
    }

    fn cancel(&mut self, _: &menu::Cancel, _: &mut Window, cx: &mut Context<Self>) {
        cx.emit(DismissEvent);
    }

    fn confirm(&mut self, _: &menu::Confirm, window: &mut Window, cx: &mut Context<Self>) {
        // Enter does what the first button says.
        self.save(self.editing.is_none(), window, cx);
    }

    fn set_sign_in(&mut self, sign_in: SignIn, cx: &mut Context<Self>) {
        self.sign_in = sign_in;
        self.error = None;
        cx.notify();
    }

    fn choose_found_key(&mut self, index: usize, cx: &mut Context<Self>) {
        if index >= self.found_keys.len() {
            return;
        }
        // The row says what the key is; the note is for problems.
        self.key_note = None;
        self.key = KeyChoice::Found(index);
        self.error = None;
        cx.notify();
    }

    fn choose_key_file(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let paths = cx.prompt_for_paths(PathPromptOptions {
            files: true,
            directories: false,
            multiple: false,
            prompt: Some("Use This Key".into()),
        });
        let task = cx.spawn_in(window, async move |this, cx| {
            let Ok(Ok(Some(paths))) = paths.await else {
                return;
            };
            let Some(path) = paths.into_iter().next() else {
                return;
            };
            let source: SharedString = path
                .file_name()
                .map(|name| name.to_string_lossy().into_owned())
                .unwrap_or_else(|| "the chosen file".to_string())
                .into();
            let text = cx
                .background_spawn(async move { read_key_file(&path) })
                .await;
            this.update(cx, |this, cx| match text {
                Ok(text) => this.use_key_text(text, source, cx),
                Err(message) => {
                    this.key_note = Some((message.into(), true));
                    cx.notify();
                }
            })
            .ok();
        });
        self._tasks.push(task);
    }

    fn paste_key(&mut self, cx: &mut Context<Self>) {
        let text = cx
            .read_from_clipboard()
            .and_then(|item| item.text())
            .map(Zeroizing::new);
        match text {
            Some(text) => self.use_key_text(text, "Pasted key".into(), cx),
            None => {
                self.key_note = Some((
                    "The clipboard holds no text. Copy the whole key first, from \"-----BEGIN\" to \"-----END\"."
                        .into(),
                    true,
                ));
                cx.notify();
            }
        }
    }

    fn use_key_text(
        &mut self,
        text: Zeroizing<String>,
        source: SharedString,
        cx: &mut Context<Self>,
    ) {
        match ssh_keys::inspect(&text) {
            Ok(info) => {
                self.key_note = None;
                self.key = KeyChoice::New { text, info, source };
                self.error = None;
            }
            Err(error) => {
                self.key_note = Some((error.to_string().into(), true));
            }
        }
        cx.notify();
    }

    /// Reads the fields. An `Err` is a sentence for the person.
    fn read_fields(&self, cx: &App) -> Result<(TypedAddress, Option<String>), SharedString> {
        let mut typed = server_store::parse_address(&self.address.read(cx).text(cx))?;
        let port = self.port.read(cx).text(cx);
        if !port.trim().is_empty() {
            typed.port = Some(server_store::parse_port(&port)?);
        }
        let user = self.user.read(cx).text(cx);
        if !user.trim().is_empty() {
            typed.username = Some(user.trim().to_string());
        }
        let name = self.name.read(cx).text(cx);
        let name = Some(name.trim().to_string()).filter(|name| !name.is_empty());
        Ok((typed, name))
    }

    fn key_plan(&self, name_hint: &str) -> Result<KeyPlan, SharedString> {
        if self.sign_in == SignIn::Password {
            return Ok(KeyPlan::None);
        }
        let name_hint = name_hint.to_string();
        match &self.key {
            KeyChoice::None => {
                Err("Choose a private key, or sign in with a password instead.".into())
            }
            KeyChoice::Kept(path) => Ok(KeyPlan::Use(path.clone())),
            KeyChoice::Found(index) => {
                let found = self
                    .found_keys
                    .get(*index)
                    .ok_or("That key is no longer there. Choose it again.")?;
                if found.usable_in_place {
                    Ok(KeyPlan::Use(found.path.clone()))
                } else {
                    Ok(KeyPlan::ImportFile {
                        path: found.path.clone(),
                        name_hint,
                    })
                }
            }
            KeyChoice::New { text, .. } => Ok(KeyPlan::Import {
                text: text.clone(),
                name_hint,
            }),
        }
    }

    fn save(&mut self, connect_after: bool, window: &mut Window, cx: &mut Context<Self>) {
        if self.saving {
            return;
        }
        let Some(workspace) = self.workspace.upgrade() else {
            return;
        };
        let (typed, name) = match self.read_fields(cx) {
            Ok(fields) => fields,
            Err(message) => return self.fail(message, cx),
        };
        let plan = match self.key_plan(name.as_deref().unwrap_or(&typed.host)) {
            Ok(plan) => plan,
            Err(message) => return self.fail(message, cx),
        };

        let original = self.editing.clone();
        let mut server = original
            .as_ref()
            .map(|(_, server)| server.clone())
            .unwrap_or_default();
        server.host = typed.host;
        server.username = typed.username;
        server.port = typed.port;
        server.nickname = name;

        let secret = Zeroizing::new(self.secret.read(cx).text(cx));
        let forget_secret = self.forget_secret;
        let fs = workspace.read(cx).app_state().fs.clone();
        let index = match &original {
            Some((index, _)) => *index,
            None => RemoteSettings::get_global(cx).ssh_connections().count(),
        };

        self.saving = true;
        self.error = None;
        cx.notify();

        let task = cx.spawn_in(window, async move |this, cx| {
            let result = apply(server, original, index, plan, secret, forget_secret, fs, cx).await;
            this.update_in(cx, |this, window, cx| match result {
                Ok(server) => {
                    cx.emit(DismissEvent);
                    if connect_after {
                        connect_to_server(workspace, index, server, window, cx);
                    }
                }
                Err(message) => {
                    this.saving = false;
                    this.fail(message, cx);
                }
            })
            .ok();
        });
        self._tasks.push(task);
    }

    fn fail(&mut self, message: impl Into<SharedString>, cx: &mut Context<Self>) {
        self.error = Some(message.into());
        cx.notify();
    }

    fn render_sign_in(&self, cx: &mut Context<Self>) -> impl IntoElement {
        let choice =
            |id: &'static str, label: &'static str, sign_in: SignIn, cx: &mut Context<Self>| {
                Button::new(id, label)
                    .style(ButtonStyle::Subtle)
                    .toggle_state(self.sign_in == sign_in)
                    .on_click(cx.listener(move |this, _, _, cx| this.set_sign_in(sign_in, cx)))
            };
        h_flex()
            .gap_2()
            .child(SectionLabel::new("Sign in with"))
            .child(choice("sign-in-password", "Password", SignIn::Password, cx))
            .child(choice("sign-in-key", "Private Key", SignIn::Key, cx))
    }

    fn render_secret(&self, cx: &mut Context<Self>) -> impl IntoElement {
        let (label, hint) = match self.sign_in {
            SignIn::Password => ("Password", PASSWORD_HINT),
            SignIn::Key => ("Key passphrase", PASSPHRASE_HINT),
        };
        let saved = self.has_saved_secret && !self.forget_secret;
        v_flex()
            .gap_1()
            .child(SectionLabel::new(label))
            .child(self.secret.clone())
            .child(
                Label::new(if saved {
                    "One is saved for this server. Type another to replace it."
                } else {
                    hint
                })
                .size(LabelSize::XSmall)
                .color(Color::Muted),
            )
            .when(saved, |column| {
                column.child(
                    h_flex().child(
                        Button::new("forget-secret", "Forget the Saved One")
                            .label_size(LabelSize::Small)
                            .style(ButtonStyle::Subtle)
                            .on_click(cx.listener(|this, _, _, cx| {
                                this.forget_secret = true;
                                cx.notify();
                            })),
                    ),
                )
            })
    }

    fn render_key(&self, cx: &mut Context<Self>) -> impl IntoElement {
        let key_row = |id: (&'static str, usize),
                       title: SharedString,
                       detail: Option<SharedString>,
                       selected: bool| {
            ListItem::new(id)
                .toggle_state(selected)
                .start_slot(
                    Icon::new(if selected {
                        IconName::Check
                    } else {
                        IconName::FileLock
                    })
                    .size(IconSize::Small)
                    .color(if selected {
                        Color::Accent
                    } else {
                        Color::Muted
                    }),
                )
                .child(
                    h_flex()
                        .gap_2()
                        .overflow_hidden()
                        .child(div().flex_none().child(Label::new(title)))
                        .children(detail.map(|detail| {
                            Label::new(detail)
                                .size(LabelSize::Small)
                                .color(Color::Muted)
                                .truncate()
                        })),
                )
        };

        let mut rows = Vec::new();
        match &self.key {
            KeyChoice::Kept(path) => rows.push(
                key_row(
                    ("kept-key", 0),
                    file_name(path).into(),
                    Some("the key of this server".into()),
                    true,
                )
                .into_any_element(),
            ),
            KeyChoice::New { info, source, .. } => rows.push(
                key_row(
                    ("new-key", 0),
                    source.clone(),
                    Some(describe(info).into()),
                    true,
                )
                .into_any_element(),
            ),
            KeyChoice::None | KeyChoice::Found(_) => {}
        }
        for (index, found) in self.found_keys.iter().enumerate() {
            let selected = matches!(self.key, KeyChoice::Found(chosen) if chosen == index);
            rows.push(
                key_row(
                    ("found-key", index),
                    found.name.clone().into(),
                    Some(describe(&found.info).into()),
                    selected,
                )
                .on_click(cx.listener(move |this, _, _, cx| this.choose_found_key(index, cx)))
                .into_any_element(),
            );
        }

        v_flex()
            .gap_1p5()
            .when(!self.found_keys.is_empty(), |column| {
                column.child(
                    Label::new("Keys found on this computer (~/.ssh), or bring another:")
                        .size(LabelSize::XSmall)
                        .color(Color::Muted),
                )
            })
            .children(rows)
            .child(
                h_flex()
                    .gap_2()
                    .child(
                        Button::new("choose-key-file", "Choose a File…")
                            .style(ButtonStyle::Outlined)
                            .on_click(cx.listener(|this, _, window, cx| {
                                this.choose_key_file(window, cx);
                            })),
                    )
                    .child(
                        Button::new("paste-key", "Paste a Key")
                            .style(ButtonStyle::Outlined)
                            .on_click(cx.listener(|this, _, _, cx| this.paste_key(cx))),
                    ),
            )
            .children(self.key_note.clone().map(|(note, is_problem)| {
                Label::new(note)
                    .size(LabelSize::Small)
                    .color(if is_problem {
                        Color::Error
                    } else {
                        Color::Muted
                    })
            }))
    }
}

/// Does the work of a save, off the form. Returns the entry as saved, or a
/// sentence for the person.
async fn apply(
    mut server: SshConnection,
    original: Option<(usize, SshConnection)>,
    index: usize,
    plan: KeyPlan,
    secret: Zeroizing<String>,
    forget_secret: bool,
    fs: std::sync::Arc<dyn fs::Fs>,
    cx: &mut AsyncWindowContext,
) -> Result<SshConnection, SharedString> {
    // The key first: nothing is written anywhere if it cannot be read.
    let passphrase = Some(secret.clone()).filter(|secret| !secret.is_empty());
    let key = match plan {
        KeyPlan::None => None,
        KeyPlan::Use(path) => Some(path),
        KeyPlan::Import { text, name_hint } => Some(
            cx.background_spawn(async move { import_key(&text, passphrase, &name_hint) })
                .await?,
        ),
        KeyPlan::ImportFile { path, name_hint } => Some(
            cx.background_spawn(async move {
                let text = read_key_file(&path)?;
                import_key(&text, passphrase, &name_hint)
            })
            .await?,
        ),
    };
    server.args = match &key {
        Some(key) => server_store::with_identity(&server.args, key),
        None => server_store::without_identity(&server.args),
    };

    let old_key = original
        .as_ref()
        .and_then(|(_, server)| server_store::identity_file(&server.args))
        .map(PathBuf::from);
    let old_secret_name = original
        .as_ref()
        .map(|(_, server)| server_store::secret_name_of(server));
    let secret_name = server_store::secret_name_of(&server);

    // The settings file.
    let saved = server.clone();
    let was = original.map(|(_, server)| server);
    let servers_after = cx
        .update(|_, cx| {
            let mut servers: Vec<SshConnection> =
                RemoteSettings::get_global(cx).ssh_connections().collect();
            match servers.get_mut(index) {
                Some(entry) if was.is_some() => *entry = saved.clone(),
                _ => servers.push(saved.clone()),
            }
            update_settings_file(fs, cx, move |settings, _| {
                let entries = settings.remote.ssh_connections.get_or_insert_default();
                match (entries.get_mut(index), &was) {
                    // Still the entry the form was opened on.
                    (Some(entry), Some(was))
                        if entry.host == was.host
                            && entry.username == was.username
                            && entry.port == was.port =>
                    {
                        *entry = saved
                    }
                    _ => entries.push(saved),
                }
            });
            servers
        })
        .map_err(|_| SharedString::from("The window was closed before the server was saved."))?;

    // The keychain.
    let moved = old_secret_name
        .as_ref()
        .is_some_and(|old| *old != secret_name);
    if !secret.is_empty() {
        if let Err(error) = server_store::write_secret(&secret_name, &secret, cx).await {
            log::error!("could not save the secret of {secret_name}: {error:#}");
            return Err(
                "The server is saved, but the system keychain refused the password: it will be asked at each connection."
                    .into(),
            );
        }
        if moved && let Some(old) = &old_secret_name {
            server_store::delete_secret(old, cx).await;
        }
    } else if let Some(old) = &old_secret_name {
        if forget_secret {
            server_store::delete_secret(old, cx).await;
        } else if moved && let Some(kept) = server_store::read_secret(old, cx).await {
            let kept = Zeroizing::new(kept);
            server_store::write_secret(&secret_name, &kept, cx)
                .await
                .log_err();
            server_store::delete_secret(old, cx).await;
        }
    }

    // The key file this server had before, when it is ours and now unused.
    if let Some(old_key) = old_key.filter(|old_key| Some(old_key) != key.as_ref()) {
        cx.background_spawn(async move {
            server_store::remove_key_if_unused(&old_key, &servers_after);
        })
        .await;
    }

    Ok(server)
}

/// Reads, converts if needed, and writes a key as a file of the
/// application's own. Returns its path.
fn import_key(
    text: &str,
    passphrase: Option<Zeroizing<String>>,
    name_hint: &str,
) -> Result<PathBuf, SharedString> {
    let imported = ssh_keys::import(text, passphrase.as_ref().map(|passphrase| passphrase.as_str()))
        .map_err(|error| -> SharedString {
            match error {
                KeyError::NeedsPassphrase => {
                    "This PuTTY key is protected: type its passphrase below, it is needed to convert the key."
                        .into()
                }
                KeyError::WrongPassphrase => "That is not the passphrase of this key.".into(),
                other => other.to_string().into(),
            }
        })?;
    let text = Zeroizing::new(imported.text);
    ssh_keys::write_key_file(&server_store::managed_keys_dir(), name_hint, &text)
        .map_err(|error| format!("The key could not be saved on this computer: {error}").into())
}

fn read_key_file(path: &Path) -> Result<Zeroizing<String>, String> {
    let name = file_name(path);
    let size = std::fs::metadata(path)
        .map_err(|error| format!("{name} could not be read: {error}"))?
        .len();
    if size > MAX_KEY_FILE_BYTES {
        return Err(format!(
            "{name} is too large to be a key. Choose the private key file."
        ));
    }
    let bytes = Zeroizing::new(
        std::fs::read(path).map_err(|error| format!("{name} could not be read: {error}"))?,
    );
    // Not `read_to_string`: Windows tools write keys as UTF-16 or Latin-1.
    Ok(Zeroizing::new(ssh_keys::text_from_bytes(&bytes)))
}

fn file_name(path: &Path) -> String {
    path.file_name()
        .map(|name| name.to_string_lossy().into_owned())
        .unwrap_or_else(|| path.to_string_lossy().into_owned())
}

/// The private keys of a folder, by name. Files that are not private keys
/// (public keys, `known_hosts`, `config`) are left out by what they contain.
fn find_keys(directory: &Path) -> Vec<FoundKey> {
    let Ok(entries) = std::fs::read_dir(directory) else {
        return Vec::new();
    };
    let mut found = Vec::new();
    for entry in entries.flatten() {
        let path = entry.path();
        let name = file_name(&path);
        if name.ends_with(".pub") {
            continue;
        }
        let Ok(metadata) = std::fs::metadata(&path) else {
            continue;
        };
        if !metadata.is_file() || metadata.len() == 0 || metadata.len() > MAX_KEY_FILE_BYTES {
            continue;
        }
        let Ok(text) = read_key_file(&path) else {
            continue;
        };
        let Ok(info) = ssh_keys::inspect(&text) else {
            continue;
        };
        let usable_in_place = info.format != KeyFormat::Putty && is_private_to_owner(&metadata);
        found.push(FoundKey {
            path,
            name,
            info,
            usable_in_place,
        });
    }
    found.sort_by(|a, b| a.name.cmp(&b.name));
    found.truncate(MAX_FOUND_KEYS);
    found
}

/// `ssh` refuses a key file that others can read.
#[cfg(unix)]
fn is_private_to_owner(metadata: &std::fs::Metadata) -> bool {
    use std::os::unix::fs::PermissionsExt as _;
    metadata.permissions().mode() & 0o077 == 0
}

#[cfg(not(unix))]
fn is_private_to_owner(_metadata: &std::fs::Metadata) -> bool {
    true
}

/// A key in a few words: "ED25519 key · protected by a passphrase".
fn describe(info: &KeyInfo) -> String {
    let mut parts = vec![match &info.algorithm {
        Some(algorithm) => format!("{algorithm} key"),
        None => "Private key".to_string(),
    }];
    if info.format == KeyFormat::Putty {
        parts.push("PuTTY key, converted on save".to_string());
    }
    if info.encrypted {
        parts.push("protected by a passphrase".to_string());
    }
    parts.join(" · ")
}

impl Render for ServerForm {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let is_new = self.editing.is_none();
        let saving = self.saving;

        v_flex()
            .key_context("ServerForm")
            .on_action(cx.listener(Self::cancel))
            .on_action(cx.listener(Self::confirm))
            // Tab and Shift-Tab walk the fields. Caught before the fields see
            // the key: a text field would take Tab for itself.
            .capture_key_down(|event: &gpui::KeyDownEvent, window, cx| {
                let keystroke = &event.keystroke;
                let modifiers = keystroke.modifiers;
                if keystroke.key == "tab"
                    && !modifiers.control
                    && !modifiers.alt
                    && !modifiers.platform
                {
                    if modifiers.shift {
                        window.focus_prev(cx);
                    } else {
                        window.focus_next(cx);
                    }
                    cx.stop_propagation();
                }
            })
            .elevation_3(cx)
            .w(rems(30.))
            .p_4()
            .gap_3()
            .child(
                Headline::new(if is_new { "New Server" } else { "Edit Server" })
                    .size(HeadlineSize::XSmall),
            )
            .child(self.name.clone())
            .child(
                h_flex()
                    .gap_2()
                    .items_start()
                    .child(div().flex_1().child(self.address.clone()))
                    .child(div().w(rems(6.)).child(self.port.clone())),
            )
            .child(self.user.clone())
            .child(self.render_sign_in(cx))
            .when(self.sign_in == SignIn::Key, |form| {
                form.child(self.render_key(cx))
            })
            .child(self.render_secret(cx))
            .children(
                self.error
                    .clone()
                    .map(|error| Label::new(error).size(LabelSize::Small).color(Color::Error)),
            )
            .child(
                h_flex()
                    .pt_1()
                    .gap_2()
                    .justify_end()
                    .child(
                        Button::new("cancel-server", "Cancel")
                            .style(ButtonStyle::Subtle)
                            .on_click(cx.listener(|_, _, _, cx| cx.emit(DismissEvent))),
                    )
                    .child(
                        if is_new {
                            Button::new("save-server", "Save").style(ButtonStyle::Outlined)
                        } else {
                            Button::primary("save-server", "Save")
                        }
                        .disabled(saving)
                        .on_click(cx.listener(|this, _, window, cx| {
                            this.save(false, window, cx);
                        })),
                    )
                    .child(
                        if is_new {
                            Button::primary("save-and-connect-server", "Save and Connect")
                        } else {
                            Button::new("save-and-connect-server", "Save and Connect")
                                .style(ButtonStyle::Outlined)
                        }
                        .disabled(saving)
                        .on_click(cx.listener(|this, _, window, cx| {
                            this.save(true, window, cx);
                        })),
                    ),
            )
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn info(format: KeyFormat, algorithm: Option<&str>, encrypted: bool) -> KeyInfo {
        KeyInfo {
            format,
            algorithm: algorithm.map(str::to_string),
            encrypted,
            comment: None,
        }
    }

    #[test]
    fn test_a_key_in_a_few_words() {
        assert_eq!(
            describe(&info(KeyFormat::OpenSsh, Some("ED25519"), false)),
            "ED25519 key"
        );
        assert_eq!(
            describe(&info(KeyFormat::Pem, None, true)),
            "Private key · protected by a passphrase"
        );
        assert_eq!(
            describe(&info(KeyFormat::Putty, Some("RSA 4096"), true)),
            "RSA 4096 key · PuTTY key, converted on save · protected by a passphrase"
        );
    }
}

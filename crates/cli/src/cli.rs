use std::path::PathBuf;

use anyhow::Result;
use collections::HashMap;
pub use ipc_channel::ipc;
use serde::{Deserialize, Serialize};

#[derive(Serialize, Deserialize)]
pub struct IpcHandshake {
    pub requests: ipc::IpcSender<CliRequest>,
    pub responses: ipc::IpcReceiver<CliResponse>,
}

/// Controls how CLI paths are opened — whether to reuse existing windows,
/// create new ones, or add to the sidebar.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "snake_case")]
pub enum OpenBehavior {
    /// Consult the user's `cli_default_open_behavior` setting.
    #[default]
    Default,
    /// Always create a new window. No matching against existing worktrees.
    /// Corresponds to `zed -n`.
    AlwaysNew,
    /// Create a new window unless opening a subpath of an existing project.
    PreferNewWindow,
    /// Match broadly including subdirectories, and fall back to any existing
    /// window if no worktree matched. Corresponds to `zed -a`.
    Add,
    /// Open directories as a new workspace in the current Zed window's sidebar.
    /// Reuse existing windows for files in open worktrees.
    /// Corresponds to `zed -e`.
    ExistingWindow,
    /// New window for directories, reuse existing window for files in open
    /// worktrees. The classic pre-sidebar behavior.
    /// Corresponds to `zed --classic`.
    Classic,
    /// Replace the content of an existing window with a new workspace.
    /// Corresponds to `zed -r`.
    Reuse,
}

/// The setting-level enum for configuring default behavior. This only has
/// two values because the other modes are always explicitly requested via
/// CLI flags.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum CliBehaviorSetting {
    /// Open directories as a new workspace in the current Zed window's sidebar.
    ExistingWindow,
    /// Open paths in a new window unless they are subpaths of an existing project.
    NewWindow,
}

#[derive(Debug, Serialize, Deserialize)]
pub enum CliRequest {
    Open {
        paths: Vec<String>,
        urls: Vec<String>,
        diff_paths: Vec<[String; 2]>,
        diff_all: bool,
        wsl: Option<String>,
        wait: bool,
        #[serde(default)]
        open_behavior: OpenBehavior,
        env: Option<HashMap<String, String>>,
        user_data_dir: Option<String>,
        dev_container: bool,
        #[serde(default)]
        cwd: Option<PathBuf>,
    },
    SetOpenBehavior {
        behavior: CliBehaviorSetting,
    },
    /// Print a diagnostic snapshot of the app's GPUI and platform state.
    DebugState,
}

#[derive(Debug, Serialize, Deserialize)]
pub enum CliResponse {
    Ping,
    Stdout { message: String },
    Stderr { message: String },
    Exit { status: i32 },
    PromptOpenBehavior,
}

/// When Zed started not as an *.app but as a binary (e.g. local development),
/// there's a possibility to tell it to behave "regularly".
///
/// Note that in the main zed binary, this variable is unset after it's read for the first time,
/// therefore it should always be accessed through the `FORCE_CLI_MODE` static.
pub const FORCE_CLI_MODE_ENV_VAR_NAME: &str = "IDE_FORCE_CLI_MODE";

/// Abstracts the transport for sending CLI responses (Zed → CLI).
///
/// Production code uses `IpcSender<CliResponse>`. Tests can provide in-memory
/// implementations to avoid OS-level IPC.
pub trait CliResponseSink: Send + 'static {
    fn send(&self, response: CliResponse) -> Result<()>;
}

impl CliResponseSink for ipc::IpcSender<CliResponse> {
    fn send(&self, response: CliResponse) -> Result<()> {
        ipc::IpcSender::send(self, response).map_err(|error| anyhow::anyhow!("{error}"))
    }
}

/// The scheme of the links the app parses internally (`ide://file/…`,
/// `ide://settings`, …). It is a neutral identifier, not the product's name
/// and not upstream's: chats and JSON schemas store such links, and the link
/// parsers spell it as a literal (`"ide://…"`, with `ide-cli://` and
/// `ide-dock-action://` for the two private channels). It is never registered
/// with the system; the scheme the product claims is [`branding::URL_SCHEME`].
pub const INTERNAL_URL_SCHEME: &str = "ide";

/// Returns what follows `<branding::URL_SCHEME>://` when `url` is one of the
/// product's own links, as the outside world writes them.
///
/// The scheme is compared without regard to case, as URL schemes are.
pub fn strip_product_url_prefix(url: &str) -> Option<&str> {
    strip_scheme_prefix(url, branding::URL_SCHEME)
}

/// Whether `url` is one of the product's own links (`unaligned-ide://…`).
pub fn is_product_url(url: &str) -> bool {
    strip_product_url_prefix(url).is_some()
}

/// Rewrites a leading `<branding::URL_SCHEME>://` to the internal `ide://`
/// form, so that the existing link parser understands the product's own links.
/// Anything else is returned unchanged.
///
/// Call this at every place that receives a link from outside the app, before
/// the link is parsed.
pub fn to_internal_url(url: &str) -> std::borrow::Cow<'_, str> {
    match strip_product_url_prefix(url) {
        Some(rest) => std::borrow::Cow::Owned(format!("{INTERNAL_URL_SCHEME}://{rest}")),
        None => std::borrow::Cow::Borrowed(url),
    }
}

fn strip_scheme_prefix<'a>(url: &'a str, scheme: &str) -> Option<&'a str> {
    let head = url.get(..scheme.len())?;
    if !head.eq_ignore_ascii_case(scheme) {
        return None;
    }
    url[scheme.len()..].strip_prefix("://")
}

#[cfg(test)]
mod url_tests {
    use super::*;

    fn product(rest: &str) -> String {
        format!("{}://{rest}", branding::URL_SCHEME)
    }

    #[test]
    fn product_links_become_internal_links() {
        for rest in [
            "",
            "open",
            "file/tmp/project/main.rs:3",
            "settings",
            "settings/languages/Rust/tab_size",
            "extension/html",
            "agent",
            "agent?prompt=hello%20world",
            "git/clone/?repo=https://example.com/a/b.git",
            "git/commit/abc123?repo=path/to/repo",
            "ssh/me@host/path",
            "schemas/settings",
        ] {
            assert_eq!(
                to_internal_url(&product(rest)),
                format!("ide://{rest}"),
                "{rest:?}"
            );
            assert!(is_product_url(&product(rest)), "{rest:?}");
        }
    }

    #[test]
    fn the_scheme_is_matched_without_regard_to_case() {
        let upper = format!("{}://settings", branding::URL_SCHEME.to_uppercase());
        assert_eq!(to_internal_url(&upper), "ide://settings");
    }

    #[test]
    fn other_arguments_are_left_alone() {
        for arg in [
            "ide://settings",
            "ide-cli://server",
            "file:///tmp/a.rs",
            "https://example.com",
            "ssh://me@host/path",
            "/tmp/a.rs:3:4",
            "src/main.rs",
            "",
            "-",
            // The scheme alone, or as the start of a longer word, is a path.
            branding::URL_SCHEME,
            "unaligned-ide-notes://settings",
            "unaligned-ide:/settings",
            // Not at the start.
            "notes/unaligned-ide://settings",
            // Not a char boundary where the scheme would end.
            "unaligned-idé://settings",
        ] {
            assert_eq!(to_internal_url(arg), arg, "{arg:?}");
            assert!(!is_product_url(arg), "{arg:?}");
        }
    }

    /// The scheme the product claims on the system is its own, and the
    /// internal one is a neutral identifier that nothing registers. Neither
    /// may be upstream's, or the app would produce, or take over, a real
    /// Zed's links. The two are also kept apart, so that a link written in
    /// the internal form is never mistaken for one the system can open.
    #[test]
    fn the_product_scheme_is_not_the_internal_one() {
        assert!(!branding::URL_SCHEME.eq_ignore_ascii_case(INTERNAL_URL_SCHEME));
        assert!(!branding::URL_SCHEME.is_empty());
        assert!(!INTERNAL_URL_SCHEME.is_empty());
        for upstream in ["zed", "zed-cli", "zed-dock-action"] {
            assert!(!INTERNAL_URL_SCHEME.eq_ignore_ascii_case(upstream));
            assert!(!branding::URL_SCHEME.eq_ignore_ascii_case(upstream));
        }
    }
}

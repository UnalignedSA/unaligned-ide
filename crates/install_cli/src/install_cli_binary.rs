use super::register_zed_scheme;
use anyhow::Result;
use gpui::{AppContext as _, AsyncApp, Context, PromptLevel, Window, actions};
use release_channel::ReleaseChannel;
use std::ops::Deref;
use std::path::{Path, PathBuf};
use util::ResultExt;
use workspace::notifications::simple_message_notification::MessageNotification;
use workspace::notifications::{DetachAndPromptErr, NotificationId};
use workspace::{Toast, Workspace};

actions!(
    cli,
    [
        /// Installs the command-line tool to the system PATH.
        InstallCliBinary,
    ]
);

/// The directory "Install CLI" puts its link in.
const CLI_LINK_DIR: &str = "/usr/local/bin";

/// The name of the installed command: the product's own (`branding::APP_ID`).
///
/// It is never `zed`: `/usr/local/bin/zed` belongs to a real Zed that may be
/// installed on the same machine, and this app must neither replace nor
/// remove it.
const CLI_COMMAND_NAME: &str = branding::APP_ID;

/// Command names of upstream's editor (its own and the ones distributions
/// give it). A link with one of these names is never created or removed.
const UPSTREAM_COMMAND_NAMES: [&str; 4] = ["zed", "zeditor", "zedit", "zed-editor"];

// A brand file that named upstream's command must not build.
const _: () = assert!(
    !is_upstream_command(CLI_COMMAND_NAME) && !CLI_COMMAND_NAME.is_empty(),
    "brand.env: APP_ID must be the product's own command name, never upstream's `zed`"
);

/// Where the link goes: `/usr/local/bin/<branding::APP_ID>`.
fn cli_link_path() -> PathBuf {
    Path::new(CLI_LINK_DIR).join(CLI_COMMAND_NAME)
}

const fn is_upstream_command(name: &str) -> bool {
    let mut index = 0;
    while index < UPSTREAM_COMMAND_NAMES.len() {
        if eq_ignore_ascii_case(name, UPSTREAM_COMMAND_NAMES[index]) {
            return true;
        }
        index += 1;
    }
    false
}

const fn eq_ignore_ascii_case(left: &str, right: &str) -> bool {
    let (left, right) = (left.as_bytes(), right.as_bytes());
    if left.len() != right.len() {
        return false;
    }
    let mut index = 0;
    while index < left.len() {
        if !left[index].eq_ignore_ascii_case(&right[index]) {
            return false;
        }
        index += 1;
    }
    true
}

/// The last check before anything is written or removed: the path must carry
/// the product's command name, whatever the caller passes.
fn ensure_own_link_path(link_path: &Path) -> Result<()> {
    let name = link_path.file_name().and_then(|name| name.to_str());
    anyhow::ensure!(
        name == Some(CLI_COMMAND_NAME) && !is_upstream_command(CLI_COMMAND_NAME),
        "refusing to install the command-line tool as {}: it is not this product's command",
        link_path.display()
    );
    Ok(())
}

/// What already sits at the link's path.
#[derive(Debug, PartialEq, Eq)]
enum ExistingLink {
    /// Nothing: the link can be created.
    Nothing,
    /// A link to this very CLI binary: nothing to do.
    UpToDate,
    /// A link to something else (an earlier copy of the app): it is replaced.
    Stale,
    /// A file or directory that is not a link: someone else's, left alone.
    NotALink,
}

async fn existing_link(cli_path: &Path, link_path: &Path) -> ExistingLink {
    match smol::fs::symlink_metadata(link_path).await {
        Err(_) => ExistingLink::Nothing,
        Ok(metadata) if !metadata.file_type().is_symlink() => ExistingLink::NotALink,
        Ok(_) => match smol::fs::read_link(link_path).await {
            Ok(target) if target == cli_path => ExistingLink::UpToDate,
            _ => ExistingLink::Stale,
        },
    }
}

/// Creates the link with the user's own rights. Returns whether the link is in
/// place; `false` means the directory needs administrator rights.
///
/// Fails without touching anything when the path is not the product's own
/// command, or when something that is not a link already has that name.
async fn link_without_escalation(cli_path: &Path, link_path: &Path) -> Result<bool> {
    ensure_own_link_path(link_path)?;

    match existing_link(cli_path, link_path).await {
        // Don't re-create symlink if it points to the same CLI binary.
        ExistingLink::UpToDate => return Ok(true),
        ExistingLink::NotALink => anyhow::bail!(
            "{} already exists and is not a link made by {}. It was left as it is.",
            link_path.display(),
            branding::PRODUCT_NAME
        ),
        // An outdated link of the product's own name: try replacing it
        // without escalating.
        ExistingLink::Stale => {
            smol::fs::remove_file(link_path).await.log_err();
        }
        ExistingLink::Nothing => {}
    }

    Ok(smol::fs::unix::symlink(cli_path, link_path)
        .await
        .log_err()
        .is_some())
}

/// Attempts to install the CLI symlink. Returns the installed path on success,
/// or `None` if the user dismissed the macOS administrator authentication
/// prompt. Returns an error if the install could not be completed, most
/// commonly because the user is not an admin.
async fn install_script(cx: &AsyncApp) -> Result<Option<PathBuf>> {
    let cli_path = cx.update(|cx| cx.path_for_auxiliary_executable("cli"))?;
    let link_path = cli_link_path();
    let link_path = link_path.as_path();
    let bin_dir_path = Path::new(CLI_LINK_DIR);

    if link_without_escalation(&cli_path, link_path).await? {
        return Ok(Some(link_path.into()));
    }

    // The symlink could not be created without escalating, so use osascript
    // with admin privileges to create it. `link_without_escalation` has
    // checked that the path is the product's own and holds nothing foreign.
    let output = smol::process::Command::new("/usr/bin/osascript")
        .args([
            "-e",
            &format!(
                "do shell script \" \
                    mkdir -p \'{}\' && \
                    ln -sf \'{}\' \'{}\' \
                \" with administrator privileges",
                bin_dir_path.to_string_lossy(),
                cli_path.to_string_lossy(),
                link_path.to_string_lossy(),
            ),
        ])
        .output()
        .await?;

    if output.status.success() {
        return Ok(Some(link_path.into()));
    }

    // osascript reports "User canceled." (error -128) when the administrator
    // prompt is dismissed. Treat that as a cancellation rather than a failure
    // so we don't show an error the user already chose to avoid.
    let stderr = String::from_utf8_lossy(&output.stderr);
    if stderr.contains("User canceled") || stderr.contains("-128") {
        return Ok(None);
    }

    // The privileged write failed, most commonly because the user is not an
    // admin.
    anyhow::bail!("error running osascript: {}", stderr.trim());
}

pub fn install_cli_binary(window: &mut Window, cx: &mut Context<Workspace>) {
    let linux_prompt_detail = format!(
        "If you installed {product} from a release archive, add ~/.local/bin to your PATH.\n\nIf you installed {product} from a different source like your package manager, then you may need to create an alias/symlink manually: the command is named `{command}`.",
        product = branding::PRODUCT_NAME,
        command = CLI_COMMAND_NAME,
    );

    cx.spawn_in(window, async move |workspace, cx| {
        if cfg!(any(target_os = "linux", target_os = "freebsd")) {
            let prompt = cx.prompt(
                PromptLevel::Warning,
                "The command-line tool should already be installed",
                Some(&linux_prompt_detail),
                &["OK"],
            );
            cx.background_spawn(prompt).detach();
            return Ok(());
        }
        let path = match install_script(cx.deref()).await {
            Ok(Some(path)) => path,
            // The user dismissed the administrator prompt; nothing to do.
            Ok(None) => return Ok(()),
            Err(error) => {
                log::error!("failed to install the command-line tool: {error:#}");
                let detail =
                    format!("{error}\n\nYou can add `{CLI_COMMAND_NAME}` to your PATH manually.");
                workspace.update(cx, |workspace, cx| {
                    struct CliInstallFailed;

                    workspace.show_notification(
                        NotificationId::unique::<CliInstallFailed>(),
                        cx,
                        |cx| {
                            cx.new(|cx| {
                                MessageNotification::new(detail, cx)
                                    .with_title("Couldn't Install the Command-Line Tool")
                            })
                        },
                    );
                })?;
                return Ok(());
            }
        };

        workspace.update_in(cx, |workspace, _, cx| {
            struct InstalledCli;

            workspace.show_toast(
                Toast::new(
                    NotificationId::unique::<InstalledCli>(),
                    format!(
                        "Installed `{}` to {}. You can launch {} from your terminal.",
                        CLI_COMMAND_NAME,
                        path.to_string_lossy(),
                        ReleaseChannel::global(cx).display_name()
                    ),
                ),
                cx,
            )
        })?;
        register_zed_scheme(cx).await.log_err();
        Ok(())
    })
    .detach_and_prompt_err(
        "Cannot install the command-line tool",
        window,
        cx,
        |_, _, _| None,
    );
}

#[cfg(test)]
mod tests {
    use super::*;

    fn block_on<T>(future: impl Future<Output = T>) -> T {
        smol::block_on(future)
    }

    /// A directory standing in for `/usr/local/bin`, holding a fake CLI binary.
    fn fake_bin_dir() -> (tempfile::TempDir, PathBuf) {
        let dir = tempfile::tempdir().unwrap();
        let cli_path = dir.path().join("the-cli-binary");
        std::fs::write(&cli_path, b"cli").unwrap();
        (dir, cli_path)
    }

    #[test]
    fn the_link_carries_the_products_name_never_upstreams() {
        let link_path = cli_link_path();
        assert_eq!(
            link_path,
            Path::new("/usr/local/bin").join(branding::APP_ID)
        );
        assert_ne!(link_path, Path::new("/usr/local/bin/zed"));
        assert!(!is_upstream_command(CLI_COMMAND_NAME));
        ensure_own_link_path(&link_path).unwrap();
    }

    #[test]
    fn upstream_command_paths_are_refused() {
        for path in [
            "/usr/local/bin/zed",
            "/usr/local/bin/Zed",
            "/usr/local/bin/zeditor",
            "/usr/local/bin/zed-editor",
            "/usr/local/bin/something-else",
            "/usr/local/bin",
            "/",
        ] {
            assert!(ensure_own_link_path(Path::new(path)).is_err(), "{path}");
        }
        for name in ["zed", "ZED", "zeditor", "zedit", "zed-editor"] {
            assert!(is_upstream_command(name), "{name}");
        }
    }

    /// Asked to link under upstream's name, the installer fails before it
    /// reads, removes or writes anything: an existing `zed` link survives.
    #[test]
    fn a_zed_link_in_the_same_directory_is_never_touched() {
        let (dir, cli_path) = fake_bin_dir();
        let real_zed_cli = dir.path().join("real-zed-cli");
        std::fs::write(&real_zed_cli, b"zed").unwrap();
        let zed_link = dir.path().join("zed");
        std::os::unix::fs::symlink(&real_zed_cli, &zed_link).unwrap();

        // Directly: refused.
        assert!(block_on(link_without_escalation(&cli_path, &zed_link)).is_err());
        assert_eq!(std::fs::read_link(&zed_link).unwrap(), real_zed_cli);

        // Installing the product's own link beside it leaves it alone too.
        let own_link = dir.path().join(CLI_COMMAND_NAME);
        assert!(block_on(link_without_escalation(&cli_path, &own_link)).unwrap());
        assert_eq!(std::fs::read_link(&own_link).unwrap(), cli_path);
        assert_eq!(std::fs::read_link(&zed_link).unwrap(), real_zed_cli);
        assert_eq!(std::fs::read(&real_zed_cli).unwrap(), b"zed");
    }

    #[test]
    fn an_up_to_date_link_is_kept_and_a_stale_one_is_replaced() {
        let (dir, cli_path) = fake_bin_dir();
        let own_link = dir.path().join(CLI_COMMAND_NAME);

        assert_eq!(
            block_on(existing_link(&cli_path, &own_link)),
            ExistingLink::Nothing
        );
        assert!(block_on(link_without_escalation(&cli_path, &own_link)).unwrap());
        assert_eq!(
            block_on(existing_link(&cli_path, &own_link)),
            ExistingLink::UpToDate
        );
        assert!(block_on(link_without_escalation(&cli_path, &own_link)).unwrap());

        // The app moved: the link of the product's own name follows.
        let moved_cli = dir.path().join("moved-cli-binary");
        std::fs::write(&moved_cli, b"cli").unwrap();
        assert_eq!(
            block_on(existing_link(&moved_cli, &own_link)),
            ExistingLink::Stale
        );
        assert!(block_on(link_without_escalation(&moved_cli, &own_link)).unwrap());
        assert_eq!(std::fs::read_link(&own_link).unwrap(), moved_cli);
    }

    /// A real file of the product's name (not a link) is someone else's: it is
    /// reported and left as it is.
    #[test]
    fn a_file_that_is_not_a_link_is_left_alone() {
        let (dir, cli_path) = fake_bin_dir();
        let own_link = dir.path().join(CLI_COMMAND_NAME);
        std::fs::write(&own_link, b"someone else's program").unwrap();

        assert_eq!(
            block_on(existing_link(&cli_path, &own_link)),
            ExistingLink::NotALink
        );
        let error = block_on(link_without_escalation(&cli_path, &own_link)).unwrap_err();
        assert!(error.to_string().contains("left as it is"), "{error}");
        assert_eq!(std::fs::read(&own_link).unwrap(), b"someone else's program");
    }

    /// A directory the user cannot write to answers "needs escalation"
    /// without an error, and nothing is created.
    #[test]
    fn an_unwritable_directory_asks_for_escalation() {
        use std::os::unix::fs::PermissionsExt as _;

        let (dir, cli_path) = fake_bin_dir();
        let locked = dir.path().join("locked");
        std::fs::create_dir(&locked).unwrap();
        std::fs::set_permissions(&locked, std::fs::Permissions::from_mode(0o555)).unwrap();
        let own_link = locked.join(CLI_COMMAND_NAME);

        let linked = block_on(link_without_escalation(&cli_path, &own_link)).unwrap();
        std::fs::set_permissions(&locked, std::fs::Permissions::from_mode(0o755)).unwrap();
        assert!(!linked);
        assert!(std::fs::symlink_metadata(&own_link).is_err());
    }

    /// No source of this crate names upstream's command path or its docs.
    #[test]
    fn sources_do_not_name_upstreams_link() {
        let needle = ["/usr/local/bin/", "zed\""].concat();
        let source = include_str!("install_cli_binary.rs");
        // The tests above spell the path on purpose; the code before them
        // must not.
        let code = source.split("#[cfg(test)]").next().unwrap();
        assert!(!code.contains(&needle));
        assert!(!code.contains(&["zed", ".dev"].concat()));
    }
}

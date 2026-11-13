//! Installing a release the user asked for.
//!
//! Nothing here runs by itself: [`crate::AutoUpdater::install_update`] is
//! called by a click. The steps, in order:
//!
//! 1. the release's manifest and its signature are downloaded and the
//!    signature is checked against the public keys compiled into the
//!    application (`update_manifest::verify`);
//! 2. the manifest must be this product's and the announced version's;
//! 3. the installer of this platform is downloaded, and kept only when it has
//!    the exact size and SHA-256 the manifest gives;
//! 4. it is put in place the way this installation was made: a macOS bundle
//!    is replaced, an AppImage file is replaced, a Windows installation gets
//!    its installer run when the application restarts.
//!
//! A release without a manifest, a build without a public key or an
//! installation of another kind (a build from source, a distribution's
//! package) installs nothing: the update is announced with a link to its page.

use std::{
    ffi::OsString,
    fmt,
    path::{Path, PathBuf},
    sync::Arc,
    time::Duration,
};

use anyhow::{Context as _, Result, ensure};
use futures::{AsyncReadExt as _, AsyncWriteExt as _};
use github_releases::GithubReleaseAsset;
use http_client::{AsyncBody, HttpClient, HttpRequestExt as _, RedirectPolicy, Request};
use semver::Version;
use sha2::{Digest as _, Sha256};
use update_manifest::{
    MANIFEST_FILE_NAME, MAX_MANIFEST_BYTES, MAX_SIGNATURE_BYTES, Manifest, ManifestFile,
    SIGNATURE_FILE_NAME,
};
use util::command::new_command;

/// The whole time the manifest or its signature may take: they are a few
/// hundred bytes.
const SMALL_FILE_DEADLINE: Duration = Duration::from_secs(30);
/// The whole time an installer may take. Generous for a slow line, and still
/// an end for a download that stays silent: the updater would otherwise wait
/// for it until the application quits.
const INSTALLER_DEADLINE: Duration = Duration::from_secs(2 * 60 * 60);

/// A request for `url` that follows redirections (GitHub serves release files
/// from its storage) and is given up after `deadline`.
async fn get(
    http: &Arc<dyn HttpClient>,
    url: &str,
    deadline: Duration,
) -> Result<http_client::Response<AsyncBody>> {
    let request = Request::get(url)
        .follow_redirects(RedirectPolicy::FollowAll)
        .timeout(deadline)
        .body(AsyncBody::default())?;
    let response = http
        .send(request)
        .await
        .with_context(|| format!("could not download {url}"))?;
    ensure!(
        response.status().is_success(),
        "{url} answered {}",
        response.status()
    );
    Ok(response)
}

/// A release, or one of its files, is not what the product's update key
/// signed: no signature, a signature of something else, another product or
/// version, a file of another size or content. This is not a failure to try
/// again or to work around by hand: the release is not to be trusted.
#[derive(Debug)]
pub struct Rejected(String);

impl fmt::Display for Rejected {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.0)
    }
}

impl std::error::Error for Rejected {}

fn rejected(reason: impl Into<String>) -> anyhow::Error {
    Rejected(reason.into()).into()
}

/// Whether `error` says that a release is not the signed one (see [`Rejected`]).
pub fn is_rejection(error: &anyhow::Error) -> bool {
    error
        .chain()
        .any(|cause| cause.downcast_ref::<Rejected>().is_some())
}

/// Where the three files an installation needs are downloaded from.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct UpdatePackage {
    pub(crate) manifest_url: String,
    pub(crate) signature_url: String,
    pub(crate) file_name: String,
    pub(crate) file_url: String,
}

/// How this copy of the application was installed, which is how it is
/// updated. `None` (see [`Installation::detect`]) for every other case.
#[derive(Clone, Debug)]
// Each platform builds one variant only.
#[allow(dead_code)]
pub(crate) enum Installation {
    /// A macOS application bundle in a folder the user can write to.
    MacBundle { app_path: PathBuf },
    /// A Linux AppImage file.
    AppImage { path: PathBuf },
    /// A Windows installation made by the product's installer.
    WindowsSetup,
    #[cfg(test)]
    Fake {
        installed: Arc<parking_lot::Mutex<Vec<Vec<u8>>>>,
    },
}

#[cfg(test)]
impl PartialEq for Installation {
    fn eq(&self, other: &Self) -> bool {
        match (self, other) {
            (Self::MacBundle { app_path: a }, Self::MacBundle { app_path: b }) => a == b,
            (Self::AppImage { path: a }, Self::AppImage { path: b }) => a == b,
            (Self::WindowsSetup, Self::WindowsSetup) => true,
            (Self::Fake { installed: a }, Self::Fake { installed: b }) => Arc::ptr_eq(a, b),
            _ => false,
        }
    }
}

/// What the application restarts into once the update is in place.
#[derive(Debug, Default, PartialEq, Eq)]
pub(crate) struct Restart {
    /// `None`: the application's own path.
    pub(crate) path: Option<PathBuf>,
    pub(crate) arguments: Vec<OsString>,
}

impl Installation {
    /// Looks at the running process. `app_path` is what the platform calls
    /// the application's path (the bundle on macOS, the executable elsewhere).
    pub(crate) fn detect(app_path: Option<&Path>) -> Option<Self> {
        #[cfg(target_os = "macos")]
        {
            detect_mac_bundle(app_path?)
        }
        #[cfg(target_os = "linux")]
        {
            let _ = app_path;
            detect_appimage(
                std::env::var_os("APPIMAGE").map(PathBuf::from).as_deref(),
                std::env::var_os("APPDIR").map(PathBuf::from).as_deref(),
                &std::env::current_exe().ok()?,
            )
        }
        #[cfg(target_os = "windows")]
        {
            let _ = app_path;
            detect_windows_setup(&std::env::current_exe().ok()?)
        }
        #[cfg(not(any(target_os = "macos", target_os = "linux", target_os = "windows")))]
        {
            let _ = app_path;
            None
        }
    }

    /// The name of this platform's installer in a release. The packaging
    /// scripts (script/unaligned/bundle-*) write the same names.
    pub(crate) fn file_name(&self) -> String {
        let id = branding::APP_ID;
        let arch = std::env::consts::ARCH;
        match self {
            Self::MacBundle { .. } => format!("{id}-macos-{arch}.dmg"),
            Self::AppImage { .. } => format!("{id}-linux-{arch}.AppImage"),
            Self::WindowsSetup => format!("{id}-windows-{arch}-setup.exe"),
            #[cfg(test)]
            Self::Fake { .. } => format!("{id}-test.bin"),
        }
    }

    /// Puts a downloaded and verified installer in place. `work_dir` is a
    /// folder of the application's own, emptied before the download.
    pub(crate) async fn install(&self, downloaded: &Path, work_dir: &Path) -> Result<Restart> {
        match self {
            Self::MacBundle { app_path } => {
                let bundle_name = format!("{}.app", branding::PRODUCT_NAME);
                install_mac_bundle(downloaded, &bundle_name, app_path, work_dir).await?;
                smol::fs::remove_file(downloaded).await.ok();
                Ok(Restart::default())
            }
            Self::AppImage { path } => {
                install_appimage(downloaded, path).await?;
                smol::fs::remove_file(downloaded).await.ok();
                Ok(Restart {
                    path: Some(path.clone()),
                    arguments: Vec::new(),
                })
            }
            Self::WindowsSetup => Ok(windows_setup_restart(downloaded)),
            #[cfg(test)]
            Self::Fake { installed } => {
                let content = smol::fs::read(downloaded).await?;
                installed.lock().push(content);
                Ok(Restart::default())
            }
        }
    }
}

/// Whether this process may create files in `directory`. Asked of the system
/// (`access`), without writing anything.
#[cfg(unix)]
fn can_write_to(directory: &Path) -> bool {
    use std::os::unix::ffi::OsStrExt as _;
    let Ok(path) = std::ffi::CString::new(directory.as_os_str().as_bytes()) else {
        return false;
    };
    // SAFETY: `path` is a valid NUL-terminated string for the duration of the call.
    unsafe { libc::access(path.as_ptr(), libc::W_OK) == 0 }
}

// On Windows only the tests of the other platforms' detection call it.
#[cfg(not(unix))]
#[allow(dead_code)]
fn can_write_to(_directory: &Path) -> bool {
    true
}

/// A bundle that can be replaced where it is: in a folder this user can write
/// to. Not one on a mounted disk image (read-only), and not one that macOS
/// runs from a read-only copy because it was started straight from the
/// downloads folder ("translocation"): the path of that copy is not where
/// the user keeps the application.
#[cfg(any(target_os = "macos", test))]
fn detect_mac_bundle(app_path: &Path) -> Option<Installation> {
    let is_bundle = app_path
        .extension()
        .is_some_and(|extension| extension == "app");
    let translocated = app_path
        .components()
        .any(|component| component.as_os_str() == "AppTranslocation");
    let parent = app_path.parent()?;
    (is_bundle && !translocated && app_path.is_absolute() && can_write_to(parent)).then(|| {
        Installation::MacBundle {
            app_path: app_path.to_path_buf(),
        }
    })
}

/// The AppImage runtime sets `APPIMAGE` (the file) and `APPDIR` (where it is
/// mounted) for the program it starts, and every child inherits them. The
/// running executable must therefore be inside `APPDIR`: an editor that was
/// merely started from another AppImage's terminal must never replace that
/// other application's file.
#[cfg(any(target_os = "linux", test))]
fn detect_appimage(
    appimage: Option<&Path>,
    appdir: Option<&Path>,
    current_exe: &Path,
) -> Option<Installation> {
    let appimage = appimage?;
    let appdir = appdir?.canonicalize().ok()?;
    let current_exe = current_exe.canonicalize().ok()?;
    (appimage.is_absolute()
        && appimage.is_file()
        && current_exe.starts_with(&appdir)
        && can_write_to(appimage.parent()?))
    .then(|| Installation::AppImage {
        path: appimage.to_path_buf(),
    })
}

/// The product's installer (Inno Setup) leaves its uninstaller beside the
/// executable. A folder without it was not made by the installer: running
/// one would install a second copy somewhere else.
#[cfg(any(target_os = "windows", test))]
fn detect_windows_setup(current_exe: &Path) -> Option<Installation> {
    current_exe
        .parent()?
        .join("unins000.exe")
        .is_file()
        .then_some(Installation::WindowsSetup)
}

/// The installer runs once the application has quit (the platform's restart
/// waits for that), replaces the files and starts the new version: `/update`
/// is read by crates/ide/resources/windows/installer.iss. `/SILENT` keeps the
/// progress window, so the user sees something between the two versions;
/// `/NOCANCEL` takes its Cancel button away, because an upgrade stopped half
/// way would leave neither version whole.
fn windows_setup_restart(installer: &Path) -> Restart {
    Restart {
        path: Some(installer.to_path_buf()),
        arguments: ["/SILENT", "/SP-", "/NOCANCEL", "/NORESTART", "/update=true"]
            .into_iter()
            .map(OsString::from)
            .collect(),
    }
}

/// Where release files are downloaded from.
pub(crate) const RELEASE_FILES_ORIGIN: &str = "https://github.com";

/// The address of a file of a release. Built from the repository and the tag,
/// not taken from GitHub's answer: nothing but this product's own release
/// files is ever requested.
fn release_file_url(origin: &str, repository: &str, tag: &str, name: &str) -> String {
    format!("{origin}/{repository}/releases/download/{tag}/{name}")
}

/// The addresses of a release's manifest, signature and installer, when the
/// release has all three.
pub(crate) fn find_package(
    origin: &str,
    repository: &str,
    tag: &str,
    assets: &[GithubReleaseAsset],
    file_name: &str,
) -> Option<UpdatePackage> {
    let has = |name: &str| assets.iter().any(|asset| asset.name == name);
    if !has(MANIFEST_FILE_NAME) || !has(SIGNATURE_FILE_NAME) || !has(file_name) {
        return None;
    }
    let url = |name: &str| release_file_url(origin, repository, tag, name);
    Some(UpdatePackage {
        manifest_url: url(MANIFEST_FILE_NAME),
        signature_url: url(SIGNATURE_FILE_NAME),
        file_name: file_name.to_string(),
        file_url: url(file_name),
    })
}

/// Checks, when a release is found, that it is one the product's key signed:
/// a build that holds a key announces no other. Downloads the manifest and
/// its signature (two small files) and nothing else. An error that
/// [`is_rejection`] recognises means the release is not the product's.
pub(crate) async fn signed_manifest_of_release(
    http: &Arc<dyn HttpClient>,
    origin: &str,
    repository: &str,
    tag: &str,
    assets: &[GithubReleaseAsset],
    public_keys: &[String],
    version: &Version,
) -> Result<Manifest> {
    let has = |name: &str| assets.iter().any(|asset| asset.name == name);
    if !has(MANIFEST_FILE_NAME) || !has(SIGNATURE_FILE_NAME) {
        return Err(rejected(format!(
            "the release has no {MANIFEST_FILE_NAME} or no {SIGNATURE_FILE_NAME}"
        )));
    }
    signed_manifest(
        http,
        &release_file_url(origin, repository, tag, MANIFEST_FILE_NAME),
        &release_file_url(origin, repository, tag, SIGNATURE_FILE_NAME),
        public_keys,
        version,
    )
    .await
}

/// The latest release a stand-in for GitHub lists (a development build's test
/// set-up, see `test_setup` in the crate's root): the same JSON as GitHub's
/// list of releases, at `<origin>/repos/<repository>/releases`.
pub(crate) async fn latest_release_of_stand_in(
    origin: &str,
    repository: &str,
    http: &Arc<dyn HttpClient>,
) -> Result<github_releases::GithubRelease> {
    let url = format!("{origin}/repos/{repository}/releases");
    let body = fetch_small(http, &url, 1024 * 1024).await?;
    let releases: Vec<github_releases::GithubRelease> = serde_json::from_slice(&body)
        .with_context(|| format!("{url} is not a list of releases"))?;
    releases
        .into_iter()
        .find(|release| !release.pre_release)
        .context("finding a prerelease")
}

async fn fetch_small(http: &Arc<dyn HttpClient>, url: &str, limit: usize) -> Result<Vec<u8>> {
    let mut response = get(http, url, SMALL_FILE_DEADLINE).await?;
    let mut bytes = Vec::new();
    response
        .body_mut()
        .take(limit as u64 + 1)
        .read_to_end(&mut bytes)
        .await
        .with_context(|| format!("could not read {url}"))?;
    ensure!(bytes.len() <= limit, "{url} is larger than {limit} bytes");
    Ok(bytes)
}

/// Downloads a manifest and its signature, checks the signature, and checks
/// that the manifest is this product's and exactly `version`'s. Nothing of a
/// release may be trusted before this returns `Ok`.
async fn signed_manifest(
    http: &Arc<dyn HttpClient>,
    manifest_url: &str,
    signature_url: &str,
    public_keys: &[String],
    version: &Version,
) -> Result<Manifest> {
    let manifest = fetch_small(http, manifest_url, MAX_MANIFEST_BYTES).await?;
    let signature = fetch_small(http, signature_url, MAX_SIGNATURE_BYTES).await?;
    let signature = String::from_utf8(signature)
        .map_err(|_| rejected("the signature file does not hold a signature"))?;
    let public_keys: Vec<&str> = public_keys.iter().map(String::as_str).collect();

    let manifest = update_manifest::verify(&manifest, &signature, &public_keys)
        .map_err(|error| rejected(format!("{error:#}")))?;
    if manifest.product != branding::APP_ID {
        return Err(rejected(format!(
            "the signed manifest is for another product ({})",
            manifest.product
        )));
    }
    // A manifest of another version, however well signed, is not this
    // release's: an older release cannot be served under a newer number.
    if manifest.version != version.to_string() {
        return Err(rejected(format!(
            "the signed manifest is for version {}, not {version}",
            manifest.version
        )));
    }
    Ok(manifest)
}

/// Downloads the manifest of `package` again, checks its signature and
/// returns what it says about this platform's installer. An error means
/// nothing of the release may be installed.
pub(crate) async fn signed_file_entry(
    http: &Arc<dyn HttpClient>,
    package: &UpdatePackage,
    public_keys: &[String],
    version: &Version,
) -> Result<ManifestFile> {
    let manifest = signed_manifest(
        http,
        &package.manifest_url,
        &package.signature_url,
        public_keys,
        version,
    )
    .await?;
    manifest.file(&package.file_name).cloned().ok_or_else(|| {
        rejected(format!(
            "the signed manifest does not list {}",
            package.file_name
        ))
    })
}

/// Downloads `url` into `target` and keeps the file only when it has the size
/// and the SHA-256 of `expected`. `on_progress` gets the fraction received.
pub(crate) async fn download_verified(
    http: &Arc<dyn HttpClient>,
    url: &str,
    target: &Path,
    expected: &ManifestFile,
    on_progress: impl FnMut(f32),
) -> Result<()> {
    let result = download_and_hash(http, url, target, expected, on_progress).await;
    if result.is_err() {
        smol::fs::remove_file(target).await.ok();
    }
    result
}

async fn download_and_hash(
    http: &Arc<dyn HttpClient>,
    url: &str,
    target: &Path,
    expected: &ManifestFile,
    mut on_progress: impl FnMut(f32),
) -> Result<()> {
    let mut response = get(http, url, INSTALLER_DEADLINE).await?;

    let mut file = smol::fs::File::create(target)
        .await
        .with_context(|| format!("could not create {target:?}"))?;
    let mut hasher = Sha256::new();
    let mut received = 0u64;
    let mut last_percent = None;
    let mut buffer = vec![0u8; 64 * 1024];
    let body = response.body_mut();
    loop {
        let read = body
            .read(&mut buffer)
            .await
            .with_context(|| format!("the download of {url} was interrupted"))?;
        if read == 0 {
            break;
        }
        received += read as u64;
        // Stop at once: nothing more than the signed size is ever written.
        if received > expected.size {
            return Err(rejected(format!(
                "{url} is larger than the {} bytes the signed manifest gives",
                expected.size
            )));
        }
        hasher.update(&buffer[..read]);
        file.write_all(&buffer[..read]).await?;

        let percent = received * 100 / expected.size;
        if last_percent != Some(percent) {
            last_percent = Some(percent);
            on_progress(received as f32 / expected.size as f32);
        }
    }
    file.flush().await?;
    file.sync_all().await?;
    drop(file);

    // Too short may be a connection that closed early: worth another try.
    ensure!(
        received == expected.size,
        "{url} is {received} bytes, the signed manifest gives {}",
        expected.size
    );
    if update_manifest::hex(&hasher.finalize()) != expected.sha256 {
        return Err(rejected(format!(
            "{url} is not the file the signed manifest describes"
        )));
    }
    Ok(())
}

/// `<folder>/.<name><suffix>`: a hidden neighbour of `path`, on the same
/// volume, so that a rename between the two is one atomic step.
fn neighbour(path: &Path, suffix: &str) -> Result<PathBuf> {
    let parent = path
        .parent()
        .with_context(|| format!("{path:?} has no parent folder"))?;
    let name = path
        .file_name()
        .with_context(|| format!("{path:?} has no name"))?;
    let mut neighbour_name = OsString::from(".");
    neighbour_name.push(name);
    neighbour_name.push(suffix);
    Ok(parent.join(neighbour_name))
}

async fn remove_if_present(path: &Path) -> Result<()> {
    let Ok(metadata) = smol::fs::symlink_metadata(path).await else {
        return Ok(());
    };
    let removed = if metadata.is_dir() {
        smol::fs::remove_dir_all(path).await
    } else {
        smol::fs::remove_file(path).await
    };
    removed.with_context(|| format!("could not remove {path:?}"))
}

async fn run(program: &str, arguments: &[&std::ffi::OsStr]) -> Result<()> {
    let output = new_command(program)
        .args(arguments)
        .output()
        .await
        .with_context(|| format!("could not run {program}"))?;
    ensure!(
        output.status.success(),
        "{program} failed: {}",
        String::from_utf8_lossy(&output.stderr).trim()
    );
    Ok(())
}

/// Puts the complete new bundle at `staging` in the place of the one at
/// `app_path`. On macOS the two are exchanged in one step by the file system
/// (`renamex_np`, APFS and HFS+): there is no moment without an application
/// at its path, and the old one ends up at `staging`. Elsewhere, or on a
/// volume that cannot exchange, two renames in a row through `previous`,
/// undone when the second one fails.
fn replace_directory(app_path: &Path, staging: &Path, previous: &Path) -> std::io::Result<()> {
    #[cfg(target_os = "macos")]
    {
        use std::os::unix::ffi::OsStrExt as _;
        let from = std::ffi::CString::new(staging.as_os_str().as_bytes())?;
        let to = std::ffi::CString::new(app_path.as_os_str().as_bytes())?;
        // SAFETY: both are valid NUL-terminated strings for the duration of the call.
        if unsafe { libc::renamex_np(from.as_ptr(), to.as_ptr(), libc::RENAME_SWAP) } == 0 {
            return Ok(());
        }
    }
    std::fs::rename(app_path, previous)?;
    if let Err(error) = std::fs::rename(staging, app_path) {
        if let Err(undo) = std::fs::rename(previous, app_path) {
            log::error!("update: the installed application is now at {previous:?}: {undo}");
        }
        return Err(error);
    }
    Ok(())
}

/// Replaces the bundle at `app_path` with the bundle `bundle_name` of a disk
/// image. The new bundle is first copied beside the old one; the old one is
/// touched only once that copy is complete, by one exchange. A failure before
/// it leaves the installed application as it was.
// Called on macOS only; compiled everywhere so that every platform's build
// checks it.
#[allow(dead_code)]
async fn install_mac_bundle(
    disk_image: &Path,
    bundle_name: &str,
    app_path: &Path,
    work_dir: &Path,
) -> Result<()> {
    let staging = neighbour(app_path, ".update")?;
    let previous = neighbour(app_path, ".previous")?;
    // Never delete the only copy: if an earlier replacement stopped between
    // its two renames, the application is at `previous`. Put it back.
    if smol::fs::symlink_metadata(app_path).await.is_err()
        && smol::fs::symlink_metadata(&previous).await.is_ok()
    {
        smol::fs::rename(&previous, app_path)
            .await
            .with_context(|| format!("could not put {previous:?} back at {app_path:?}"))?;
    }
    remove_if_present(&staging).await?;
    remove_if_present(&previous).await?;

    let mount_point = work_dir.join("mount");
    // An image left attached by an installation that was cut short.
    if smol::fs::symlink_metadata(&mount_point).await.is_ok() {
        run(
            "hdiutil",
            &[
                "detach".as_ref(),
                "-force".as_ref(),
                mount_point.as_os_str(),
            ],
        )
        .await
        .ok();
    }
    smol::fs::create_dir_all(&mount_point).await?;
    run(
        "hdiutil",
        &[
            "attach".as_ref(),
            "-nobrowse".as_ref(),
            "-readonly".as_ref(),
            "-noautoopen".as_ref(),
            "-mountpoint".as_ref(),
            mount_point.as_os_str(),
            disk_image.as_os_str(),
        ],
    )
    .await
    .context("could not open the disk image")?;

    // Whatever the copy did, the image is detached before anything else.
    let copied = copy_bundle(&mount_point.join(bundle_name), &staging).await;
    if let Err(error) = run(
        "hdiutil",
        &[
            "detach".as_ref(),
            "-force".as_ref(),
            mount_point.as_os_str(),
        ],
    )
    .await
    {
        log::error!("update: {error:#}");
    }
    if let Err(error) = copied {
        remove_if_present(&staging).await.ok();
        return Err(error);
    }

    // Off the main thread and without a pause in the middle: nothing else of
    // the application runs between the steps of the replacement.
    let replaced = smol::unblock({
        let (app_path, staging, previous) =
            (app_path.to_path_buf(), staging.clone(), previous.clone());
        move || replace_directory(&app_path, &staging, &previous)
    })
    .await;
    if let Err(error) = replaced {
        remove_if_present(&staging).await.ok();
        return Err(error).with_context(|| {
            format!("could not replace {app_path:?} (is its folder one you can write to?)")
        });
    }
    // The old bundle: at `staging` after an exchange, at `previous` otherwise.
    for old in [&staging, &previous] {
        if let Err(error) = remove_if_present(old).await {
            log::warn!("update: {error:#}");
        }
    }
    Ok(())
}

async fn copy_bundle(source: &Path, staging: &Path) -> Result<()> {
    ensure!(
        smol::fs::metadata(&source.join("Contents").join("MacOS"))
            .await
            .is_ok_and(|metadata| metadata.is_dir()),
        "the disk image does not hold {:?}",
        source.file_name().unwrap_or_default()
    );
    // `ditto` copies a bundle as it is: links, attributes and signature.
    run("ditto", &[source.as_os_str(), staging.as_os_str()])
        .await
        .with_context(|| {
            format!("could not copy the new version beside {staging:?} (is the folder one you can write to?)")
        })
}

/// Replaces an AppImage file. The running application keeps the old file's
/// content open; the new one takes its name in one rename.
// Called on Linux only; compiled everywhere so that every platform's build
// checks it.
#[allow(dead_code)]
async fn install_appimage(downloaded: &Path, appimage: &Path) -> Result<()> {
    let staging = neighbour(appimage, ".update")?;
    remove_if_present(&staging).await?;
    let result = async {
        smol::fs::copy(downloaded, &staging)
            .await
            .with_context(|| {
                format!("could not write beside {appimage:?} (is the folder one you can write to?)")
            })?;
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt as _;
            // The permissions of the file being replaced, executable in any case.
            let mode = smol::fs::metadata(appimage)
                .await
                .map(|metadata| metadata.permissions().mode() & 0o777)
                .unwrap_or(0o755);
            smol::fs::set_permissions(&staging, std::fs::Permissions::from_mode(mode | 0o100))
                .await?;
        }
        // On disk before it takes the name: a power cut just after the rename
        // must not leave a truncated file under the application's name.
        smol::fs::File::open(&staging).await?.sync_all().await?;
        smol::fs::rename(&staging, appimage)
            .await
            .with_context(|| format!("could not replace {appimage:?}"))
    }
    .await;
    if result.is_err() {
        remove_if_present(&staging).await.ok();
    }
    result
}

#[cfg(test)]
mod tests {
    use super::*;
    use http_client::{FakeHttpClient, Response};

    fn asset(name: &str) -> GithubReleaseAsset {
        GithubReleaseAsset {
            name: name.to_string(),
            // Deliberately somewhere else: the response's addresses are not used.
            browser_download_url: format!("https://elsewhere.example/{name}"),
            digest: None,
        }
    }

    #[test]
    fn test_a_package_needs_the_manifest_the_signature_and_the_installer() {
        let all = [
            asset("update.json"),
            asset("update.json.sig"),
            asset("ide-linux-x86_64.AppImage"),
            asset("ide-macos-aarch64.dmg"),
        ];
        assert_eq!(
            find_package(
                RELEASE_FILES_ORIGIN,
                "example/ide",
                "v1.2.3",
                &all,
                "ide-macos-aarch64.dmg"
            ),
            Some(UpdatePackage {
                manifest_url: "https://github.com/example/ide/releases/download/v1.2.3/update.json"
                    .into(),
                signature_url:
                    "https://github.com/example/ide/releases/download/v1.2.3/update.json.sig".into(),
                file_name: "ide-macos-aarch64.dmg".into(),
                file_url:
                    "https://github.com/example/ide/releases/download/v1.2.3/ide-macos-aarch64.dmg"
                        .into(),
            })
        );
        assert_eq!(
            find_package(
                RELEASE_FILES_ORIGIN,
                "example/ide",
                "v1.2.3",
                &all,
                "ide-windows-x86_64-setup.exe"
            ),
            None
        );
        for missing in 0..2 {
            let mut assets = all
                .iter()
                .map(|asset| asset.name.as_str())
                .collect::<Vec<_>>();
            assets.remove(missing);
            let assets = assets.into_iter().map(asset).collect::<Vec<_>>();
            assert_eq!(
                find_package(
                    RELEASE_FILES_ORIGIN,
                    "example/ide",
                    "v1.2.3",
                    &assets,
                    "ide-macos-aarch64.dmg"
                ),
                None
            );
        }
    }

    #[test]
    fn test_only_a_bundle_at_its_own_place_is_a_mac_installation() {
        let directory = tempfile::tempdir().unwrap();
        let app_path = directory.path().join("Example IDE.app");
        assert_eq!(
            detect_mac_bundle(&app_path),
            Some(Installation::MacBundle {
                app_path: app_path.clone()
            })
        );
        // A folder that cannot be written to, like a mounted disk image.
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt as _;
            let read_only = directory.path().join("image");
            std::fs::create_dir(&read_only).unwrap();
            std::fs::set_permissions(&read_only, std::fs::Permissions::from_mode(0o555)).unwrap();
            assert_eq!(detect_mac_bundle(&read_only.join("Example IDE.app")), None);
            assert_eq!(
                detect_appimage_in_read_only_folder(&read_only, directory.path()),
                None
            );
            std::fs::set_permissions(&read_only, std::fs::Permissions::from_mode(0o755)).unwrap();
        }
        for path in [
            // An unbundled build: the folder of the executable.
            "/Users/someone/ide/target/debug",
            // Started from the downloads folder: macOS runs a read-only copy.
            "/private/var/folders/ab/T/AppTranslocation/1234/d/Example IDE.app",
            "Example IDE.app",
        ] {
            assert_eq!(detect_mac_bundle(Path::new(path)), None, "{path}");
        }
    }

    /// An AppImage that is really running from its mount, in `folder`.
    #[cfg(unix)]
    fn detect_appimage_in_read_only_folder(folder: &Path, scratch: &Path) -> Option<Installation> {
        use std::os::unix::fs::PermissionsExt as _;
        std::fs::set_permissions(folder, std::fs::Permissions::from_mode(0o755)).unwrap();
        let appimage = folder.join("ide.AppImage");
        std::fs::write(&appimage, b"image").unwrap();
        std::fs::set_permissions(folder, std::fs::Permissions::from_mode(0o555)).unwrap();
        let appdir = scratch.join("read-only-mount");
        std::fs::create_dir_all(&appdir).unwrap();
        let editor = appdir.join("editor");
        std::fs::write(&editor, b"editor").unwrap();
        detect_appimage(Some(&appimage), Some(&appdir), &editor)
    }

    #[test]
    fn test_an_appimage_is_recognised_by_its_own_mount_only() {
        let directory = tempfile::tempdir().unwrap();
        let appimage = directory.path().join("ide.AppImage");
        std::fs::write(&appimage, b"image").unwrap();
        let appdir = directory.path().join("mount");
        std::fs::create_dir_all(appdir.join("libexec")).unwrap();
        let editor = appdir.join("libexec").join("editor");
        std::fs::write(&editor, b"editor").unwrap();
        let other = directory.path().join("other-editor");
        std::fs::write(&other, b"editor").unwrap();

        assert_eq!(
            detect_appimage(Some(&appimage), Some(&appdir), &editor),
            Some(Installation::AppImage {
                path: appimage.clone()
            })
        );
        // The variables of another AppImage, inherited through a terminal.
        assert_eq!(
            detect_appimage(Some(&appimage), Some(&appdir), &other),
            None
        );
        assert_eq!(detect_appimage(None, Some(&appdir), &editor), None);
        assert_eq!(detect_appimage(Some(&appimage), None, &editor), None);
        assert_eq!(
            detect_appimage(Some(Path::new("ide.AppImage")), Some(&appdir), &editor),
            None
        );
        assert_eq!(
            detect_appimage(
                Some(&directory.path().join("gone.AppImage")),
                Some(&appdir),
                &editor
            ),
            None
        );
    }

    #[test]
    fn test_a_windows_installation_has_its_uninstaller() {
        let directory = tempfile::tempdir().unwrap();
        let editor = directory.path().join("ide.exe");
        assert_eq!(detect_windows_setup(&editor), None);
        std::fs::write(directory.path().join("unins000.exe"), b"uninstaller").unwrap();
        assert_eq!(
            detect_windows_setup(&editor),
            Some(Installation::WindowsSetup)
        );

        let restart = windows_setup_restart(Path::new("C:\\cache\\ide-setup.exe"));
        assert_eq!(restart.path, Some("C:\\cache\\ide-setup.exe".into()));
        assert!(restart.arguments.contains(&OsString::from("/update=true")));
        assert!(restart.arguments.contains(&OsString::from("/NOCANCEL")));
    }

    fn serving(body: &'static [u8]) -> Arc<dyn HttpClient> {
        FakeHttpClient::create(move |_| async move {
            Ok(Response::builder().status(200).body(body.to_vec().into())?)
        })
    }

    fn entry(content: &[u8]) -> ManifestFile {
        ManifestFile {
            name: "file".into(),
            size: content.len() as u64,
            sha256: update_manifest::hex(&Sha256::digest(content)),
        }
    }

    #[test]
    fn test_a_download_is_kept_only_when_it_is_the_signed_file() {
        smol::block_on(async {
            let directory = tempfile::tempdir().unwrap();
            let target = directory.path().join("download");
            let http = serving(b"the new version");
            let url = "https://github.com/example/ide/releases/download/v1.2.3/file";

            let mut progress = Vec::new();
            download_verified(
                &http,
                url,
                &target,
                &entry(b"the new version"),
                |fraction| progress.push(fraction),
            )
            .await
            .unwrap();
            assert_eq!(std::fs::read(&target).unwrap(), b"the new version");
            assert_eq!(progress.last(), Some(&1.0));

            // Same size, other content.
            let error = download_verified(&http, url, &target, &entry(b"the old version"), |_| {})
                .await
                .unwrap_err();
            assert!(error.to_string().contains("not the file"), "{error}");
            assert!(is_rejection(&error));
            assert!(!target.exists());

            // Longer than signed: refused, and nothing is left.
            let error = download_verified(&http, url, &target, &entry(b"the new"), |_| {})
                .await
                .unwrap_err();
            assert!(error.to_string().contains("larger"), "{error}");
            assert!(is_rejection(&error));
            assert!(!target.exists());

            // Shorter than signed.
            let error = download_verified(&http, url, &target, &entry(b"the new version!"), |_| {})
                .await
                .unwrap_err();
            assert!(error.to_string().contains("bytes"), "{error}");
            // A cut connection looks the same: not a verdict on the release.
            assert!(!is_rejection(&error));
            assert!(!target.exists());
        });
    }

    #[cfg(unix)]
    #[test]
    fn test_an_appimage_is_replaced_in_one_step() {
        smol::block_on(async {
            use std::os::unix::fs::PermissionsExt as _;

            let directory = tempfile::tempdir().unwrap();
            let appimage = directory.path().join("ide.AppImage");
            std::fs::write(&appimage, b"old").unwrap();
            std::fs::set_permissions(&appimage, std::fs::Permissions::from_mode(0o750)).unwrap();
            let downloaded = directory.path().join("download");
            std::fs::write(&downloaded, b"new").unwrap();
            std::fs::set_permissions(&downloaded, std::fs::Permissions::from_mode(0o600)).unwrap();

            install_appimage(&downloaded, &appimage).await.unwrap();
            assert_eq!(std::fs::read(&appimage).unwrap(), b"new");
            let mode = std::fs::metadata(&appimage).unwrap().permissions().mode() & 0o777;
            assert_eq!(mode, 0o750);
            assert!(!directory.path().join(".ide.AppImage.update").exists());

            // A folder that cannot be written to: the old file stays.
            let locked = directory.path().join("locked");
            std::fs::create_dir(&locked).unwrap();
            let locked_appimage = locked.join("ide.AppImage");
            std::fs::write(&locked_appimage, b"old").unwrap();
            std::fs::set_permissions(&locked, std::fs::Permissions::from_mode(0o555)).unwrap();
            let result = install_appimage(&downloaded, &locked_appimage).await;
            std::fs::set_permissions(&locked, std::fs::Permissions::from_mode(0o755)).unwrap();
            assert!(result.is_err());
            assert_eq!(std::fs::read(&locked_appimage).unwrap(), b"old");
        });
    }

    /// Real `hdiutil` and `ditto`, on a disk image made for the test.
    #[cfg(target_os = "macos")]
    #[test]
    fn test_a_mac_bundle_is_replaced_from_a_disk_image() {
        fn bundle(path: &Path, version: &str) {
            std::fs::create_dir_all(path.join("Contents/MacOS")).unwrap();
            std::fs::write(path.join("Contents/MacOS/editor"), version).unwrap();
            std::fs::write(path.join("Contents/Info.plist"), version).unwrap();
        }
        fn hdiutil(arguments: &[&std::ffi::OsStr]) {
            smol::block_on(run("hdiutil", arguments)).unwrap();
        }

        let directory = tempfile::tempdir().unwrap();
        let root = directory.path();

        // The new version, in a disk image.
        let image_content = root.join("image");
        bundle(&image_content.join("Example IDE.app"), "new");
        let disk_image = root.join("new.dmg");
        hdiutil(&[
            "create".as_ref(),
            "-srcfolder".as_ref(),
            image_content.as_os_str(),
            "-volname".as_ref(),
            "Example IDE".as_ref(),
            "-format".as_ref(),
            "UDZO".as_ref(),
            disk_image.as_os_str(),
        ]);

        // The installed version, with a file the new one no longer has.
        let applications = root.join("Applications");
        let app_path = applications.join("Example IDE.app");
        bundle(&app_path, "old");
        std::fs::write(app_path.join("Contents/MacOS/removed-since"), "old").unwrap();
        let work_dir = root.join("work");
        std::fs::create_dir(&work_dir).unwrap();

        smol::block_on(install_mac_bundle(
            &disk_image,
            "Example IDE.app",
            &app_path,
            &work_dir,
        ))
        .unwrap();
        assert_eq!(
            std::fs::read_to_string(app_path.join("Contents/MacOS/editor")).unwrap(),
            "new"
        );
        assert!(!app_path.join("Contents/MacOS/removed-since").exists());
        let left: Vec<_> = std::fs::read_dir(&applications)
            .unwrap()
            .map(|entry| entry.unwrap().file_name())
            .collect();
        assert_eq!(left, ["Example IDE.app"]);
        // The image is detached: its mount point is an empty folder again.
        assert_eq!(
            std::fs::read_dir(work_dir.join("mount")).unwrap().count(),
            0
        );

        // An image without the expected bundle changes nothing.
        std::fs::write(app_path.join("Contents/MacOS/editor"), "installed").unwrap();
        let error = smol::block_on(install_mac_bundle(
            &disk_image,
            "Another.app",
            &app_path,
            &work_dir,
        ))
        .unwrap_err();
        assert!(error.to_string().contains("does not hold"), "{error:#}");
        assert_eq!(
            std::fs::read_to_string(app_path.join("Contents/MacOS/editor")).unwrap(),
            "installed"
        );
        assert_eq!(
            std::fs::read_dir(work_dir.join("mount")).unwrap().count(),
            0
        );
    }
}

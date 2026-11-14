//! The remote server: the program the application puts on a host to work
//! there (an SSH host, a WSL distribution, a container).
//!
//! It is a file of the application's own release, `<APP_ID>-remote-server-
//! <os>-<arch>.gz`, listed in the release's signed manifest like the
//! installers. The application downloads it to this machine, keeps it only
//! when it has the size and the SHA-256 the signed manifest gives, and uploads
//! that file itself. A host is never given an address to download from: it
//! could not check what it receives, and it needs no access to the internet.
//!
//! The server must be of the application's exact version (the two speak a
//! protocol that changes between versions), so it is taken from the release
//! of that version and never from the latest one.

use std::{
    path::{Path, PathBuf},
    sync::{
        Arc,
        atomic::{AtomicUsize, Ordering},
    },
    time::{Duration, SystemTime},
};

use anyhow::{Context as _, Result};
use http_client::HttpClient;
use semver::Version;
use update_manifest::{MANIFEST_FILE_NAME, ManifestFile, SIGNATURE_FILE_NAME};

use crate::install;

/// Where remote servers come from and how they are checked.
pub(crate) struct Source {
    pub(crate) http: Arc<dyn HttpClient>,
    /// `https://github.com`, or a development build's stand-in.
    pub(crate) origin: String,
    pub(crate) repository: String,
    pub(crate) public_keys: Vec<String>,
    /// Where checked downloads are kept, so that a second host needs no
    /// second download.
    pub(crate) cache_dir: PathBuf,
}

/// The name, in a release, of the remote server for a host's system: one
/// executable, compressed with gzip.
pub(crate) fn file_name(os: &str, arch: &str) -> String {
    format!("{}-remote-server-{os}-{arch}.gz", branding::APP_ID)
}

/// A download left unfinished (the window was closed, the application quit)
/// is removed once it is older than this. Longer than any download may take,
/// so that a download another window is making is never taken for one.
const ABANDONED_AFTER: Duration = Duration::from_secs(3 * 60 * 60);

/// The start of the names under which the servers of one system are kept.
fn kept_prefix(os: &str, arch: &str) -> String {
    format!("{}-remote-server-{os}-{arch}-", branding::APP_ID)
}

/// Returns the path of the remote server of `version` for `os` and `arch`,
/// downloaded now or kept from an earlier time, and in both cases checked
/// against the signed manifest of the release of `version`.
pub(crate) async fn download(
    source: &Source,
    version: &Version,
    os: &str,
    arch: &str,
    on_progress: impl FnMut(f32),
) -> Result<PathBuf> {
    // The release of 1.2.3 is tagged v1.2.3, whatever the build adds to its
    // own version.
    let version = Version::new(version.major, version.minor, version.patch);
    let tag = format!("v{version}");
    let url =
        |name: &str| install::release_file_url(&source.origin, &source.repository, &tag, name);

    // Nothing of the release is trusted before the manifest's signature.
    let manifest = install::signed_manifest(
        &source.http,
        &url(MANIFEST_FILE_NAME),
        &url(SIGNATURE_FILE_NAME),
        &source.public_keys,
        &version,
    )
    .await
    .with_context(|| format!("checking the signed list of the files of release {tag}"))?;

    let name = file_name(os, arch);
    let entry = manifest.file(&name).cloned().with_context(|| {
        format!("release {tag} has no remote server for {os} {arch}: its signed list has no {name}")
    })?;

    smol::fs::create_dir_all(&source.cache_dir)
        .await
        .with_context(|| format!("could not create {:?}", source.cache_dir))?;
    let kept_name = format!("{}{version}.gz", kept_prefix(os, arch));
    let kept = source.cache_dir.join(&kept_name);
    if is_the_signed_file(&kept, &entry).await {
        return Ok(kept);
    }
    remove_abandoned_downloads(&source.cache_dir).await;

    // Under a name of its own, so that two windows connecting at once do not
    // write into each other's download; the rename is one step.
    static DOWNLOADS: AtomicUsize = AtomicUsize::new(0);
    let partial = Partial(source.cache_dir.join(format!(
        ".{kept_name}.{}-{}{PARTIAL_SUFFIX}",
        std::process::id(),
        DOWNLOADS.fetch_add(1, Ordering::Relaxed)
    )));
    install::download_verified(&source.http, &url(&name), &partial.0, &entry, on_progress).await?;
    let moved = smol::fs::rename(&partial.0, &kept).await;

    // What counts is the file that will be uploaded: it is read back from the
    // disk, whoever put it there (this download, or another window's that
    // finished first and whose file could not be replaced while it is read).
    if !is_the_signed_file(&kept, &entry).await {
        return match moved {
            Err(error) => {
                Err(error).with_context(|| format!("could not move the download to {kept:?}"))
            }
            Ok(()) => {
                smol::fs::remove_file(&kept).await.ok();
                Err(install::rejected(format!(
                    "{kept:?} is not the file the signed manifest describes"
                )))
            }
        };
    }

    remove_other_versions(&source.cache_dir, &kept_prefix(os, arch), &kept_name).await;
    Ok(kept)
}

const PARTIAL_SUFFIX: &str = ".part";

/// A download in progress. Dropped before its end (the download failed, or
/// was given up: the connection window was closed), its file is removed.
struct Partial(PathBuf);

impl Drop for Partial {
    fn drop(&mut self) {
        // Already renamed after a download that ended well: nothing to remove.
        std::fs::remove_file(&self.0).ok();
    }
}

/// Removes the downloads an earlier run left unfinished without a chance to
/// clean up (the application was killed).
async fn remove_abandoned_downloads(cache_dir: &Path) {
    let Ok(mut entries) = smol::fs::read_dir(cache_dir).await else {
        return;
    };
    let now = SystemTime::now();
    while let Some(Ok(entry)) = futures::StreamExt::next(&mut entries).await {
        let name = entry.file_name();
        let Some(name) = name.to_str() else {
            continue;
        };
        if !name.starts_with('.') || !name.ends_with(PARTIAL_SUFFIX) {
            continue;
        }
        let abandoned = entry
            .metadata()
            .await
            .ok()
            .and_then(|metadata| metadata.modified().ok())
            .and_then(|modified| now.duration_since(modified).ok())
            .is_some_and(|age| age > ABANDONED_AFTER);
        if abandoned {
            smol::fs::remove_file(entry.path()).await.ok();
        }
    }
}

/// Whether `path` is a file with the size and the SHA-256 of `entry`. A file
/// kept from an earlier download is read again every time: what is uploaded
/// to a host is always what the manifest describes.
async fn is_the_signed_file(path: &Path, entry: &ManifestFile) -> bool {
    let Ok(metadata) = smol::fs::metadata(path).await else {
        return false;
    };
    if !metadata.is_file() || metadata.len() != entry.size {
        return false;
    }
    let path = path.to_path_buf();
    match smol::unblock(move || update_manifest::hash_file(&path)).await {
        Ok((size, sha256)) => size == entry.size && sha256 == entry.sha256,
        Err(_) => false,
    }
}

/// The servers of earlier versions are of no use to this one.
async fn remove_other_versions(cache_dir: &Path, prefix: &str, kept_name: &str) {
    let Ok(mut entries) = smol::fs::read_dir(cache_dir).await else {
        return;
    };
    while let Some(Ok(entry)) = futures::StreamExt::next(&mut entries).await {
        let name = entry.file_name();
        let Some(name) = name.to_str() else {
            continue;
        };
        if name != kept_name && name.starts_with(prefix) && name.ends_with(".gz") {
            smol::fs::remove_file(entry.path()).await.ok();
        }
    }
}

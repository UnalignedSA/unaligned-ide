//! Provides constructs for the app version and release channel.
//!
//! Every name and identifier here is built from the `branding` crate.

#![deny(missing_docs)]

use std::{env, str::FromStr, sync::LazyLock};

use branding::{APP_ID, DOCS_URL, PRODUCT_NAME, REVERSE_DNS_ID};
use const_format::concatcp;
use gpui::{App, Global};
use semver::Version;

/// stable | dev | nightly | preview
pub static RELEASE_CHANNEL_NAME: LazyLock<String> = LazyLock::new(|| {
    if cfg!(debug_assertions) {
        env::var("IDE_RELEASE_CHANNEL").unwrap_or_else(|_| compile_time_release_channel_name())
    } else {
        compile_time_release_channel_name()
    }
});

/// When a crate in zed is used as a dependency that uses the `crane` nix
/// library, it vendors each crate separately and builds it in isolation, which
/// makes the `include_str!` fail.
///
/// The build script checks for `$IDE_RELEASE_CHANNEL` and emits the `cfg`
#[cfg(__do_not_set_zed_release_channel)]
fn compile_time_release_channel_name() -> String {
    env!("IDE_RELEASE_CHANNEL").trim().to_string()
}

#[cfg(not(__do_not_set_zed_release_channel))]
fn compile_time_release_channel_name() -> String {
    include_str!("../../ide/RELEASE_CHANNEL").trim().to_string()
}

#[doc(hidden)]
pub static RELEASE_CHANNEL: LazyLock<ReleaseChannel> =
    LazyLock::new(|| match ReleaseChannel::from_str(&RELEASE_CHANNEL_NAME) {
        Ok(channel) => channel,
        _ => panic!("invalid release channel {}", *RELEASE_CHANNEL_NAME),
    });

/// The app identifier for the current release channel, Windows only.
///
/// The single-instance mutex (`<identifier>-Instance-Mutex`) and the named
/// pipe of the CLI (`<identifier>-Named-Pipe`) are built from it, so it must
/// never be one of upstream's `Zed-Editor-*` values.
#[cfg(target_os = "windows")]
pub fn app_identifier() -> &'static str {
    RELEASE_CHANNEL.app_identifier()
}

/// The Git commit SHA that the app was built at.
#[derive(Clone, Eq, Debug, PartialEq)]
pub struct AppCommitSha(String);

struct GlobalAppCommitSha(AppCommitSha);

impl Global for GlobalAppCommitSha {}

impl AppCommitSha {
    /// Creates a new [`AppCommitSha`].
    pub fn new(sha: String) -> Self {
        AppCommitSha(sha)
    }

    /// Returns the global [`AppCommitSha`], if one is set.
    pub fn try_global(cx: &App) -> Option<AppCommitSha> {
        cx.try_global::<GlobalAppCommitSha>()
            .map(|sha| sha.0.clone())
    }

    /// Sets the global [`AppCommitSha`].
    pub fn set_global(sha: AppCommitSha, cx: &mut App) {
        cx.set_global(GlobalAppCommitSha(sha))
    }

    /// Returns the full commit SHA.
    pub fn full(&self) -> String {
        self.0.to_string()
    }

    /// Returns the short (7 character) commit SHA.
    pub fn short(&self) -> String {
        self.0.chars().take(7).collect()
    }
}

struct GlobalAppVersion(Version);

impl Global for GlobalAppVersion {}

/// The version of the app.
pub struct AppVersion;

impl AppVersion {
    /// Load the app version from env.
    pub fn load(
        pkg_version: &str,
        build_id: Option<&str>,
        commit_sha: Option<AppCommitSha>,
    ) -> Version {
        let mut version: Version = if let Ok(from_env) = env::var("IDE_APP_VERSION") {
            from_env.parse().expect("invalid IDE_APP_VERSION")
        } else {
            pkg_version.parse().expect("invalid version in Cargo.toml")
        };
        let mut pre = String::from(RELEASE_CHANNEL.dev_name());

        if let Some(build_id) = build_id {
            pre.push('.');
            pre.push_str(&build_id);
        }

        if let Some(sha) = commit_sha {
            pre.push('.');
            pre.push_str(&sha.0);
        }
        if let Ok(build) = semver::BuildMetadata::new(&pre) {
            version.build = build;
        }

        version
    }

    /// Returns the global version number.
    pub fn global(cx: &App) -> Version {
        if cx.has_global::<GlobalAppVersion>() {
            cx.global::<GlobalAppVersion>().0.clone()
        } else {
            Version::new(0, 0, 0)
        }
    }
}

/// A release channel of the app.
#[derive(Debug, Copy, Clone, PartialEq, Eq, Default)]
pub enum ReleaseChannel {
    /// The development release channel.
    ///
    /// Used for local debug builds of the app.
    #[default]
    Dev,

    /// The Nightly release channel.
    Nightly,

    /// The Preview release channel.
    Preview,

    /// The Stable release channel.
    Stable,
}

struct GlobalReleaseChannel(ReleaseChannel);

impl Global for GlobalReleaseChannel {}

/// Initializes the release channel.
pub fn init(app_version: Version, cx: &mut App) {
    cx.set_global(GlobalAppVersion(app_version));
    cx.set_global(GlobalReleaseChannel(*RELEASE_CHANNEL))
}

/// Initializes the release channel for tests that rely on fake release channel.
pub fn init_test(app_version: Version, release_channel: ReleaseChannel, cx: &mut App) {
    cx.set_global(GlobalAppVersion(app_version));
    cx.set_global(GlobalReleaseChannel(release_channel))
}

/// Returns the docs URL for the current release channel for the given
/// `slug`.
///
/// The product's documentation is the provider's (its API, billing, coding
/// tools): none of the pages of upstream's manual exist there, and an unknown
/// slug is a 404. Nothing calls this any more; before linking to a page, check
/// that the site has it.
pub fn docs_url(slug: &str, cx: &App) -> String {
    ReleaseChannel::try_global(cx)
        .unwrap_or(*RELEASE_CHANNEL)
        .docs_url(slug)
}

impl ReleaseChannel {
    /// All release channels.
    pub const ALL: [ReleaseChannel; 4] = [
        ReleaseChannel::Dev,
        ReleaseChannel::Nightly,
        ReleaseChannel::Preview,
        ReleaseChannel::Stable,
    ];

    /// Returns the global [`ReleaseChannel`].
    pub fn global(cx: &App) -> Self {
        cx.global::<GlobalReleaseChannel>().0
    }

    /// Returns the global [`ReleaseChannel`], if one is set.
    pub fn try_global(cx: &App) -> Option<Self> {
        cx.try_global::<GlobalReleaseChannel>()
            .map(|channel| channel.0)
    }

    /// Returns whether we want to poll for updates for this [`ReleaseChannel`]
    pub fn poll_for_updates(&self) -> bool {
        !matches!(self, ReleaseChannel::Dev)
    }

    /// Returns the display name for this [`ReleaseChannel`]: the product's
    /// name, followed by the channel for every channel but stable.
    pub fn display_name(&self) -> &'static str {
        match self {
            ReleaseChannel::Dev => concatcp!(PRODUCT_NAME, " Dev"),
            ReleaseChannel::Nightly => concatcp!(PRODUCT_NAME, " Nightly"),
            ReleaseChannel::Preview => concatcp!(PRODUCT_NAME, " Preview"),
            ReleaseChannel::Stable => PRODUCT_NAME,
        }
    }

    /// Returns the programmatic name for this [`ReleaseChannel`].
    pub fn dev_name(&self) -> &'static str {
        match self {
            ReleaseChannel::Dev => "dev",
            ReleaseChannel::Nightly => "nightly",
            ReleaseChannel::Preview => "preview",
            ReleaseChannel::Stable => "stable",
        }
    }

    /// Returns the application ID that's used by Wayland as application ID
    /// and WM_CLASS on X11.
    /// This also has to match the bundle identifier of the app on macOS and
    /// the name of its `.desktop` file on Linux.
    pub fn app_id(&self) -> &'static str {
        match self {
            ReleaseChannel::Dev => concatcp!(REVERSE_DNS_ID, "-Dev"),
            ReleaseChannel::Nightly => concatcp!(REVERSE_DNS_ID, "-Nightly"),
            ReleaseChannel::Preview => concatcp!(REVERSE_DNS_ID, "-Preview"),
            ReleaseChannel::Stable => REVERSE_DNS_ID,
        }
    }

    /// Returns the identifier that names what an instance of this
    /// [`ReleaseChannel`] shares with the CLI on Windows (see
    /// [`app_identifier`]).
    pub fn app_identifier(&self) -> &'static str {
        match self {
            ReleaseChannel::Dev => concatcp!(APP_ID, "-Dev"),
            ReleaseChannel::Nightly => concatcp!(APP_ID, "-Nightly"),
            ReleaseChannel::Preview => concatcp!(APP_ID, "-Preview"),
            ReleaseChannel::Stable => concatcp!(APP_ID, "-Stable"),
        }
    }

    /// Returns the query parameter for this [`ReleaseChannel`].
    pub fn release_query_param(&self) -> Option<&'static str> {
        match self {
            Self::Dev => None,
            Self::Nightly => Some("nightly=1"),
            Self::Preview => Some("preview=1"),
            Self::Stable => None,
        }
    }

    /// Returns the docs URL for the given `slug`, under `branding::DOCS_URL`.
    ///
    /// The product has one documentation site for every channel: upstream's
    /// `/nightly/` and `/preview/` path segments do not exist there, so the
    /// URL is the same on all channels.
    pub fn docs_url(&self, slug: &str) -> String {
        if slug.is_empty() {
            DOCS_URL.to_string()
        } else {
            format!("{DOCS_URL}/{slug}")
        }
    }
}

/// Error indicating that release channel string does not match any known release channel names.
#[derive(Copy, Clone, Debug, Hash, PartialEq)]
pub struct InvalidReleaseChannel;

impl FromStr for ReleaseChannel {
    type Err = InvalidReleaseChannel;

    fn from_str(channel: &str) -> Result<Self, Self::Err> {
        Ok(match channel {
            "dev" => ReleaseChannel::Dev,
            "nightly" => ReleaseChannel::Nightly,
            "preview" => ReleaseChannel::Preview,
            "stable" => ReleaseChannel::Stable,
            _ => return Err(InvalidReleaseChannel),
        })
    }
}

#[cfg(test)]
mod tests {
    use super::ReleaseChannel;

    #[test]
    fn test_docs_url_for_release_channel() {
        let docs = branding::DOCS_URL;
        // Upstream expected `{docs}/nightly/settings` on dev and nightly and
        // `{docs}/preview/settings` on preview. The product's site has no
        // per-channel docs, so every channel gets the same URL (gate 1, Q3).
        for channel in ReleaseChannel::ALL {
            assert_eq!(channel.docs_url("settings"), format!("{docs}/settings"));
            assert_eq!(channel.docs_url(""), docs);
            assert!(!channel.docs_url("settings").contains("/nightly"));
            assert!(!channel.docs_url("settings").contains("/preview"));
            assert!(!channel.docs_url("settings").contains("zed.dev"));
        }
    }

    #[test]
    fn test_display_name_comes_from_the_brand() {
        assert_eq!(
            ReleaseChannel::Stable.display_name(),
            branding::PRODUCT_NAME
        );
        assert_eq!(
            ReleaseChannel::Preview.display_name(),
            format!("{} Preview", branding::PRODUCT_NAME)
        );
        assert_eq!(
            ReleaseChannel::Nightly.display_name(),
            format!("{} Nightly", branding::PRODUCT_NAME)
        );
        assert_eq!(
            ReleaseChannel::Dev.display_name(),
            format!("{} Dev", branding::PRODUCT_NAME)
        );
    }

    #[test]
    fn test_identifiers_come_from_the_brand_and_differ_per_channel() {
        assert_eq!(ReleaseChannel::Stable.app_id(), branding::REVERSE_DNS_ID);
        assert_eq!(
            ReleaseChannel::Dev.app_id(),
            format!("{}-Dev", branding::REVERSE_DNS_ID)
        );
        assert_eq!(
            ReleaseChannel::Stable.app_identifier(),
            format!("{}-Stable", branding::APP_ID)
        );

        for (index, channel) in ReleaseChannel::ALL.iter().enumerate() {
            // Nothing may be shared with an upstream Zed running on the same machine.
            for value in [
                channel.display_name(),
                channel.app_id(),
                channel.app_identifier(),
            ] {
                assert!(!value.starts_with(branding::UPSTREAM_NAME), "{value:?}");
                assert!(!value.starts_with("dev.zed."), "{value:?}");
            }
            for other in &ReleaseChannel::ALL[index + 1..] {
                assert_ne!(channel.display_name(), other.display_name());
                assert_ne!(channel.app_id(), other.app_id());
                assert_ne!(channel.app_identifier(), other.app_identifier());
            }
        }
    }

    /// The macOS bundle metadata of the `zed` crate cannot read Rust constants;
    /// it must still say what the app says about itself.
    #[test]
    fn test_bundle_metadata_follows_the_brand() {
        let manifest = include_str!("../../ide/Cargo.toml");
        for channel in ReleaseChannel::ALL {
            let section = format!("[package.metadata.bundle-{}]", channel.dev_name());
            let (_, section_body) = manifest
                .split_once(&section)
                .unwrap_or_else(|| panic!("no {section} in crates/ide/Cargo.toml"));
            let section_body = section_body.split("\n[").next().unwrap();
            for line in [
                format!("identifier = \"{}\"", channel.app_id()),
                format!("name = \"{}\"", channel.display_name()),
                format!("osx_url_schemes = [\"{}\"]", branding::URL_SCHEME),
            ] {
                assert!(
                    section_body.lines().any(|candidate| candidate == line),
                    "{section} must contain `{line}`"
                );
            }
        }
    }
}

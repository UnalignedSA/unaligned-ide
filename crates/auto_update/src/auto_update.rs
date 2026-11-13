//! Update checks against the product's own releases.
//!
//! This crate never downloads and never installs anything. A check asks the
//! GitHub Releases of the repository named in the brand file
//! (`branding::releases_repository()`), compares the latest release with the
//! running version and, when it is newer, remembers it as an
//! [`AvailableUpdate`]: a version and the address of the release's page, which
//! the user opens in a browser. While the brand file names no repository,
//! nothing is contacted at all.
//!
//! Upstream's updater (a download from its own release service, then an
//! in-place install by `rsync`, a disk image or an installer) is gone: it would
//! have replaced this application with upstream's builds.

use anyhow::{Context as _, Result};
use client::Client;
use futures::channel::oneshot;
use gpui::{
    App, AppContext as _, AsyncApp, Context, Entity, EventEmitter, Global, PromptLevel, Task,
    Window, actions,
};
use http_client::HttpClient;
use release_channel::ReleaseChannel;
use semver::Version;
use settings::{RegisterSetting, Settings, SettingsStore};
use std::{env, fmt, path::PathBuf, sync::Arc, time::Duration};
use workspace::Workspace;

/// How long a running app waits between two automatic checks. One request a
/// day stays far below what GitHub allows an anonymous caller (60 an hour for
/// one network address).
const POLL_INTERVAL: Duration = Duration::from_secs(24 * 60 * 60);

/// The owner of upstream's repository. A brand file that names it would make
/// the app announce upstream's releases as its own, so it counts as "none".
const UPSTREAM_REPOSITORY_OWNER: &str = "zed-industries";

const NOT_SET_UP_MESSAGE: &str = "Updates are not set up for this build yet.";

actions!(
    auto_update,
    [
        /// Checks for available updates.
        Check,
        /// Dismisses the update error message.
        DismissMessage,
        /// Opens the release notes for the current version in a browser.
        ViewReleaseNotes,
    ]
);

/// What the updater is doing.
///
/// A newer release is not a status: see [`AutoUpdater::available_update`].
/// This build downloads and installs nothing, so there is no status for it.
#[derive(Clone, Debug)]
pub enum AutoUpdateStatus {
    Idle,
    Checking,
    Errored { error: Arc<anyhow::Error> },
}

impl PartialEq for AutoUpdateStatus {
    fn eq(&self, other: &Self) -> bool {
        match (self, other) {
            (AutoUpdateStatus::Idle, AutoUpdateStatus::Idle) => true,
            (AutoUpdateStatus::Checking, AutoUpdateStatus::Checking) => true,
            (AutoUpdateStatus::Errored { error: e1 }, AutoUpdateStatus::Errored { error: e2 }) => {
                e1.to_string() == e2.to_string()
            }
            _ => false,
        }
    }
}

/// A release newer than the running build, found by a check. Nothing is
/// downloaded: `url` is the release's page, for the user's browser.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct AvailableUpdate {
    pub version: Version,
    pub url: String,
}

/// The result of asking for the latest release.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum UpdateCheck {
    /// The brand file names no releases repository: nothing was requested.
    NotConfigured,
    /// No published release is newer than the running build.
    UpToDate,
    /// A newer release exists.
    Available(AvailableUpdate),
}

/// GitHub answered 403 or 429: too many anonymous requests came from this
/// network address during the last hour.
#[derive(Debug)]
pub struct RateLimited {
    status: u16,
}

impl fmt::Display for RateLimited {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            f,
            "GitHub refused the update check (HTTP {}): too many requests from this network address. Try again later.",
            self.status
        )
    }
}

impl std::error::Error for RateLimited {}

pub enum AutoUpdateEvent {
    /// A manual check received a release response and found no newer version.
    UpToDate,
}

pub struct AutoUpdater {
    status: AutoUpdateStatus,
    available_update: Option<AvailableUpdate>,
    dismissed_update: Option<Version>,
    current_version: Version,
    releases_repository: Option<String>,
    client: Arc<Client>,
    pending_poll: Option<Task<Option<()>>>,
    manual_check_finished: Option<oneshot::Sender<Option<AvailableUpdate>>>,
    update_check_type: UpdateCheckType,
    _wake_subscription: gpui::Subscription,
    dismissed_status: Option<AutoUpdateStatus>,
}

#[derive(Clone, Copy, Debug, RegisterSetting)]
struct AutoUpdateSetting(bool);

/// Whether to ask once a day if a newer version has been released. A newer
/// version is only announced, never downloaded or installed.
impl Settings for AutoUpdateSetting {
    fn from_settings(content: &settings::SettingsContent) -> Self {
        Self(content.auto_update.unwrap())
    }
}

#[derive(Default)]
struct GlobalAutoUpdate(Option<Entity<AutoUpdater>>);

impl Global for GlobalAutoUpdate {}

pub fn init(client: Arc<Client>, cx: &mut App) {
    let releases_repository = configured_releases_repository();
    let has_repository = releases_repository.is_some();
    cx.observe_new(move |workspace: &mut Workspace, _window, _cx| {
        workspace.register_action(|_, action, window, cx| check(action, window, cx));

        // Without a repository there are no release notes to open: the action
        // gets no handler, so menus show it disabled.
        if has_repository {
            workspace.register_action(|_, action, _, cx| {
                view_release_notes(action, cx);
            });
        }
    })
    .detach();

    init_updater(releases_repository, update_explanation(), client, cx);
}

/// Creates the updater. It polls only when all of this holds: a releases
/// repository is configured, no packager explanation is present, the release
/// channel checks for updates and the `auto_update` setting is on.
fn init_updater(
    releases_repository: Option<String>,
    update_explanation: Option<String>,
    client: Arc<Client>,
    cx: &mut App,
) {
    let has_repository = releases_repository.is_some();
    let version = release_channel::AppVersion::global(cx);
    let auto_updater = cx.new(|cx| {
        let updater = AutoUpdater::new(version, releases_repository, client, cx);

        if has_repository && update_explanation.is_none() && channel_checks_for_updates(cx) {
            let mut update_subscription = AutoUpdateSetting::get_global(cx)
                .0
                .then(|| updater.start_polling(cx));

            cx.observe_global::<SettingsStore>(move |updater: &mut AutoUpdater, cx| {
                if AutoUpdateSetting::get_global(cx).0 {
                    if update_subscription.is_none() {
                        update_subscription = Some(updater.start_polling(cx))
                    }
                } else {
                    update_subscription.take();
                }
            })
            .detach();
        }

        updater
    });
    cx.set_global(GlobalAutoUpdate(Some(auto_updater)));
}

/// The `owner/repo` slug update checks ask, or `None` when the brand file
/// names none.
fn configured_releases_repository() -> Option<String> {
    validated_releases_repository(branding::releases_repository()?)
}

/// Accepts a GitHub `owner/repo` slug and nothing else. Upstream's own
/// repository is refused: its releases are not this product's.
fn validated_releases_repository(slug: &str) -> Option<String> {
    let is_name = |name: &str| {
        !name.is_empty()
            && name != "."
            && name != ".."
            && name
                .chars()
                .all(|c| c.is_ascii_alphanumeric() || matches!(c, '-' | '_' | '.'))
    };
    let Some((owner, repository)) = slug.split_once('/') else {
        log::error!("update checks are off: {slug:?} is not an `owner/repo` slug");
        return None;
    };
    if !is_name(owner) || !is_name(repository) {
        log::error!("update checks are off: {slug:?} is not an `owner/repo` slug");
        return None;
    }
    if owner.eq_ignore_ascii_case(UPSTREAM_REPOSITORY_OWNER) {
        log::error!("update checks are off: {slug:?} is upstream's repository, not this product's");
        return None;
    }
    Some(slug.to_string())
}

/// The text a packager gave, at build time or at run time, to say how this
/// installation is updated. When present the app checks nothing by itself.
fn update_explanation() -> Option<String> {
    option_env!("IDE_UPDATE_EXPLANATION")
        .map(ToOwned::to_owned)
        .or_else(|| env::var("IDE_UPDATE_EXPLANATION").ok())
}

fn channel_checks_for_updates(cx: &App) -> bool {
    ReleaseChannel::try_global(cx)
        .map(|channel| channel.poll_for_updates())
        .unwrap_or(false)
}

/// The product's site without its scheme, as written in a sentence.
fn website_name() -> &'static str {
    let url = branding::WEBSITE_URL;
    let url = url
        .strip_prefix("https://")
        .or_else(|| url.strip_prefix("http://"))
        .unwrap_or(url);
    url.trim_end_matches('/')
}

pub fn check(_: &Check, window: &mut Window, cx: &mut App) {
    check_with_explanation(update_explanation(), window, cx);
}

fn check_with_explanation(update_explanation: Option<String>, window: &mut Window, cx: &mut App) {
    if let Some(message) = update_explanation {
        drop(window.prompt(
            PromptLevel::Info,
            &format!(
                "{} was installed via a package manager.",
                branding::PRODUCT_NAME
            ),
            Some(&message),
            &["OK"],
            cx,
        ));
        return;
    }

    let Some(updater) = AutoUpdater::get(cx) else {
        drop(window.prompt(
            PromptLevel::Info,
            "Could not check for updates",
            Some("The update check is not available in this build."),
            &["OK"],
            cx,
        ));
        return;
    };

    if updater.read(cx).releases_repository.is_none() {
        drop(window.prompt(
            PromptLevel::Info,
            NOT_SET_UP_MESSAGE,
            Some(&format!(
                "New versions will be announced on {}.",
                website_name()
            )),
            &["OK"],
            cx,
        ));
        return;
    }

    if !channel_checks_for_updates(cx) {
        drop(window.prompt(
            PromptLevel::Info,
            "Development builds do not check for updates.",
            None,
            &["OK"],
            cx,
        ));
        return;
    }

    let finished = updater.update(cx, |updater, cx| updater.check_manually(cx));
    window
        .spawn(cx, async move |cx| {
            // An up-to-date answer and an error are shown by the title bar.
            let Ok(Some(update)) = finished.await else {
                return;
            };
            let answer = cx
                .prompt(
                    PromptLevel::Info,
                    &format!(
                        "{} {} is available",
                        branding::PRODUCT_NAME,
                        update.version
                    ),
                    Some("This build does not install updates itself: download the new version from its release page."),
                    &["Open Release Page", "Later"],
                )
                .await;
            if let Ok(0) = answer {
                cx.update(|_, cx| cx.open_url(&update.url)).ok();
            }
        })
        .detach();
}

/// The page that describes the running version: its release on GitHub for a
/// stable or preview build, the list of releases otherwise. `None` while the
/// brand file names no releases repository.
pub fn release_notes_url(cx: &mut App) -> Option<String> {
    let release_channel = ReleaseChannel::try_global(cx)?;
    let auto_updater = AutoUpdater::get(cx)?;
    let auto_updater = auto_updater.read(cx);
    let repository = auto_updater.releases_repository.as_deref()?;
    let url = match release_channel {
        ReleaseChannel::Stable | ReleaseChannel::Preview => {
            let mut current_version = auto_updater.current_version.clone();
            current_version.pre = semver::Prerelease::EMPTY;
            current_version.build = semver::BuildMetadata::EMPTY;
            release_page_url(repository, &format!("v{current_version}"))
        }
        ReleaseChannel::Nightly | ReleaseChannel::Dev => {
            format!("https://github.com/{repository}/releases")
        }
    };
    Some(url)
}

pub fn view_release_notes(_: &ViewReleaseNotes, cx: &mut App) -> Option<()> {
    let url = release_notes_url(cx)?;
    cx.open_url(&url);
    None
}

fn release_page_url(repository: &str, tag: &str) -> String {
    format!("https://github.com/{repository}/releases/tag/{tag}")
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub enum UpdateCheckType {
    Automatic,
    Manual,
}

impl UpdateCheckType {
    pub fn is_manual(self) -> bool {
        self == Self::Manual
    }
}

impl EventEmitter<AutoUpdateEvent> for AutoUpdater {}

impl AutoUpdater {
    pub fn get(cx: &mut App) -> Option<Entity<Self>> {
        cx.default_global::<GlobalAutoUpdate>().0.clone()
    }

    fn new(
        current_version: Version,
        releases_repository: Option<String>,
        client: Arc<Client>,
        cx: &mut Context<Self>,
    ) -> Self {
        // A check that was in flight when the machine went to sleep is almost
        // certainly riding a TCP connection that silently died during suspend,
        // so it would otherwise appear to stall indefinitely.
        let wake_subscription = cx.on_system_wake({
            let this = cx.entity().downgrade();
            move |cx| {
                this.update(cx, |this, cx| this.restart_after_wake(cx)).ok();
            }
        });

        Self {
            status: AutoUpdateStatus::Idle,
            available_update: None,
            dismissed_update: None,
            current_version,
            releases_repository,
            client,
            pending_poll: None,
            manual_check_finished: None,
            update_check_type: UpdateCheckType::Automatic,
            _wake_subscription: wake_subscription,
            dismissed_status: None,
        }
    }

    fn restart_after_wake(&mut self, cx: &mut Context<Self>) {
        if !matches!(self.status, AutoUpdateStatus::Checking) {
            return;
        }

        let check_type = self.update_check_type;
        self.pending_poll.take();
        self.status = AutoUpdateStatus::Idle;
        self.poll(check_type, cx);
    }

    /// Checks now, then once every [`POLL_INTERVAL`] for as long as the
    /// returned task is kept.
    pub fn start_polling(&self, cx: &mut Context<Self>) -> Task<Result<()>> {
        cx.spawn(async move |this, cx| {
            loop {
                this.update(cx, |this, cx| this.poll(UpdateCheckType::Automatic, cx))?;
                cx.background_executor().timer(POLL_INTERVAL).await;
            }
        })
    }

    pub fn update_check_type(&self) -> UpdateCheckType {
        self.update_check_type
    }

    /// Asks for the latest release, unless a check is already running. Does
    /// nothing, and contacts nothing, without a releases repository.
    pub fn poll(&mut self, check_type: UpdateCheckType, cx: &mut Context<Self>) {
        if check_type.is_manual() {
            self.dismissed_status = None;
            self.dismissed_update = None;
        }
        if self.pending_poll.is_some() {
            if self.update_check_type == UpdateCheckType::Automatic {
                self.update_check_type = check_type;
                cx.notify();
            }
            return;
        }
        self.update_check_type = check_type;

        let Some(repository) = self.releases_repository.clone() else {
            self.finish_check(Ok(UpdateCheck::NotConfigured), cx);
            return;
        };
        let http_client = self.client.http_client();
        let installed_version = self.current_version.clone();

        self.status = AutoUpdateStatus::Checking;
        log::info!("checking {repository} for a newer release");
        cx.notify();

        self.pending_poll = Some(cx.spawn(async move |this, cx| {
            let result =
                check_latest_release(Some(&repository), installed_version, http_client).await;
            this.update(cx, |this, cx| this.finish_check(result, cx))
                .ok()
        }));
    }

    /// Starts a check on the user's request. The receiver gets the newer
    /// release if there is one, and `None` in every other case.
    fn check_manually(
        &mut self,
        cx: &mut Context<Self>,
    ) -> oneshot::Receiver<Option<AvailableUpdate>> {
        let (finished, receiver) = oneshot::channel();
        self.manual_check_finished = Some(finished);
        self.poll(UpdateCheckType::Manual, cx);
        receiver
    }

    fn finish_check(&mut self, result: Result<UpdateCheck>, cx: &mut Context<Self>) {
        self.pending_poll = None;
        let check_type = self.update_check_type;
        let mut found = None;
        self.status = match result {
            Ok(UpdateCheck::Available(update)) => {
                log::info!("version {} is available at {}", update.version, update.url);
                self.available_update = Some(update.clone());
                found = Some(update);
                AutoUpdateStatus::Idle
            }
            Ok(UpdateCheck::UpToDate) => {
                self.available_update = None;
                if check_type.is_manual() {
                    cx.emit(AutoUpdateEvent::UpToDate);
                }
                AutoUpdateStatus::Idle
            }
            Ok(UpdateCheck::NotConfigured) => AutoUpdateStatus::Idle,
            Err(error) => match check_type {
                // Be quiet if the check was automated (offline, or refused by
                // GitHub for an hour): the next attempt is a day away.
                UpdateCheckType::Automatic => {
                    log::info!("update check failed, next attempt in a day: {error:#}");
                    AutoUpdateStatus::Idle
                }
                UpdateCheckType::Manual => {
                    log::error!("update check failed: {error:#}");
                    AutoUpdateStatus::Errored {
                        error: Arc::new(error),
                    }
                }
            },
        };
        if let Some(finished) = self.manual_check_finished.take() {
            finished.send(found).ok();
        }
        cx.notify();
    }

    pub fn current_version(&self) -> Version {
        self.current_version.clone()
    }

    pub fn status(&self) -> AutoUpdateStatus {
        self.status.clone()
    }

    /// The newer release found by the last successful check, unless the user
    /// dismissed that version. A manual check brings a dismissed one back.
    pub fn available_update(&self) -> Option<AvailableUpdate> {
        self.available_update
            .clone()
            .filter(|update| self.dismissed_update.as_ref() != Some(&update.version))
    }

    /// Stops announcing the release that is available now. A later release is
    /// announced again.
    pub fn dismiss_available_update(&mut self, cx: &mut Context<Self>) {
        if let Some(update) = &self.available_update {
            self.dismissed_update = Some(update.version.clone());
            cx.notify();
        }
    }

    pub fn dismissed_status(&self) -> Option<AutoUpdateStatus> {
        self.dismissed_status.clone()
    }

    pub fn dismiss_status(&mut self, status: AutoUpdateStatus, cx: &mut Context<Self>) {
        self.dismissed_status = Some(status);
        cx.notify();
    }

    pub fn dismiss(&mut self, cx: &mut Context<Self>) -> bool {
        if let AutoUpdateStatus::Idle = self.status {
            return false;
        }
        self.status = AutoUpdateStatus::Idle;
        cx.notify();
        true
    }

    /// Remote server builds are not published for this product, so there is
    /// nothing to download: this always fails, without any request. A remote
    /// project therefore opens only where a matching server is already on the
    /// host, or from a development build, which compiles its own.
    ///
    /// Upstream fetched its own `zed-remote-server` here; that binary must
    /// never be uploaded to a user's host by this application.
    pub async fn download_remote_server_release(
        _release_channel: ReleaseChannel,
        _version: Option<Version>,
        os: &str,
        arch: &str,
        _set_status: impl Fn(&str, &mut AsyncApp) + Send + 'static,
        _cx: &mut AsyncApp,
    ) -> Result<PathBuf> {
        anyhow::bail!(
            "{} does not publish remote server builds yet, so there is none to download for {os} {arch}",
            branding::PRODUCT_NAME
        )
    }

    /// Always `Ok(None)`: there is no address a remote host could download a
    /// server from (see [`Self::download_remote_server_release`]).
    pub async fn get_remote_server_release_url(
        _channel: ReleaseChannel,
        _version: Option<Version>,
        _os: &str,
        _arch: &str,
        _cx: &mut AsyncApp,
    ) -> Result<Option<String>> {
        Ok(None)
    }
}

/// Asks GitHub for the latest release of `repository` and compares it with the
/// running version. This is the only request the crate ever makes; without a
/// repository it makes none.
async fn check_latest_release(
    repository: Option<&str>,
    installed_version: Version,
    http_client: Arc<dyn HttpClient>,
) -> Result<UpdateCheck> {
    let Some(repository) = repository else {
        return Ok(UpdateCheck::NotConfigured);
    };

    let release =
        match github_releases::latest_github_release(repository, false, false, http_client).await {
            Ok(release) => release,
            Err(error) if no_release_is_published(&error) => return Ok(UpdateCheck::UpToDate),
            Err(error) => {
                return Err(match refused_with_status(&error) {
                    Some(status @ (403 | 429)) => RateLimited { status }.into(),
                    _ => error.context(format!("could not read the releases of {repository}")),
                });
            }
        };

    let fetched_version = parse_release_tag(&release.tag_name)?;
    Ok(match newer_version(installed_version, fetched_version) {
        Some(version) => UpdateCheck::Available(AvailableUpdate {
            version,
            url: release_page_url(repository, &release.tag_name),
        }),
        None => UpdateCheck::UpToDate,
    })
}

/// The HTTP status of a request GitHub refused. `latest_github_release` only
/// reports it as text ("status error 403, response: …").
fn refused_with_status(error: &anyhow::Error) -> Option<u16> {
    error.chain().find_map(|cause| {
        let message = cause.to_string();
        let status = message.strip_prefix("status error ")?;
        status.split(',').next()?.trim().parse().ok()
    })
}

/// Whether `latest_github_release` failed because the repository has no
/// published release (it reports that as an error, with this text).
fn no_release_is_published(error: &anyhow::Error) -> bool {
    error
        .chain()
        .any(|cause| cause.to_string() == "finding a prerelease")
}

/// Release tags are the version, with or without a leading `v`.
fn parse_release_tag(tag: &str) -> Result<Version> {
    let version = tag.strip_prefix('v').unwrap_or(tag);
    version
        .parse::<Version>()
        .with_context(|| format!("release tag {tag:?} is not a version"))
}

fn newer_version(mut installed_version: Version, fetched_version: Version) -> Option<Version> {
    // Ignore the pre-release and build fields of the running version: release
    // tags carry neither.
    installed_version.pre = semver::Prerelease::EMPTY;
    installed_version.build = semver::BuildMetadata::EMPTY;
    (fetched_version > installed_version).then_some(fetched_version)
}

#[cfg(test)]
mod tests {
    use super::*;
    use clock::FakeSystemClock;
    use gpui::{BorrowAppContext as _, TestAppContext, VisualTestContext};
    use http_client::{FakeHttpClient, HttpClientWithUrl, Response};
    use parking_lot::Mutex;

    #[ctor::ctor(unsafe)]
    fn init_logger() {
        zlog::init_test();
    }

    const REPOSITORY: &str = "example/ide";
    const RELEASES_URL: &str = "https://api.github.com/repos/example/ide/releases";

    /// A stand-in for GitHub. It answers the releases list of [`REPOSITORY`]
    /// with the status and body it was last given, anything else with 404, and
    /// records the address of every request: no test touches the network.
    struct FakeGitHub {
        requests: Arc<Mutex<Vec<String>>>,
        response: Arc<Mutex<(u16, String)>>,
        http_client: Arc<HttpClientWithUrl>,
    }

    impl FakeGitHub {
        fn new(status: u16, body: impl Into<String>) -> Self {
            let requests = Arc::new(Mutex::new(Vec::new()));
            let response = Arc::new(Mutex::new((status, body.into())));
            let http_client = FakeHttpClient::create({
                let requests = requests.clone();
                let response = response.clone();
                move |request| {
                    let url = request.uri().to_string();
                    let (status, body) = if url == RELEASES_URL {
                        response.lock().clone()
                    } else {
                        (404, String::new())
                    };
                    requests.lock().push(url);
                    async move { Ok(Response::builder().status(status).body(body.into())?) }
                }
            });
            Self {
                requests,
                response,
                http_client,
            }
        }

        fn respond_with(&self, status: u16, body: impl Into<String>) {
            *self.response.lock() = (status, body.into());
        }

        fn requests(&self) -> Vec<String> {
            self.requests.lock().clone()
        }
    }

    /// The body GitHub sends for a list of releases, newest first.
    fn releases(tags: &[(&str, bool)]) -> String {
        let releases = tags
            .iter()
            .map(|(tag, pre_release)| {
                format!(
                    r#"{{"tag_name":"{tag}","prerelease":{pre_release},"assets":[],"tarball_url":"","zipball_url":""}}"#
                )
            })
            .collect::<Vec<_>>();
        format!("[{}]", releases.join(","))
    }

    fn release(tag: &str) -> String {
        releases(&[(tag, false)])
    }

    fn installed() -> Version {
        Version::new(0, 100, 0)
    }

    fn available(version: Version, tag: &str) -> AvailableUpdate {
        AvailableUpdate {
            version,
            url: format!("https://github.com/example/ide/releases/tag/{tag}"),
        }
    }

    async fn check_against(github: &FakeGitHub) -> Result<UpdateCheck> {
        check_latest_release(Some(REPOSITORY), installed(), github.http_client.clone()).await
    }

    fn init_test_updater(
        github: &FakeGitHub,
        channel: ReleaseChannel,
        repository: Option<&str>,
        auto_update: bool,
        cx: &mut TestAppContext,
    ) -> Entity<AutoUpdater> {
        cx.update(|cx| {
            settings::init(cx);
            set_auto_update(auto_update, cx);
            release_channel::init_test(installed(), channel, cx);
            let client = Client::new(
                Arc::new(FakeSystemClock::new()),
                github.http_client.clone(),
                cx,
            );
            init_updater(repository.map(str::to_string), None, client, cx);
            AutoUpdater::get(cx).expect("the updater should exist")
        })
    }

    fn set_auto_update(auto_update: bool, cx: &mut App) {
        cx.update_global(|store: &mut SettingsStore, cx| {
            store
                .set_user_settings(&format!(r#"{{"auto_update": {auto_update}}}"#), cx)
                .expect("the user settings should parse");
        });
    }

    fn a_day_passes(cx: &mut TestAppContext) {
        cx.background_executor.advance_clock(POLL_INTERVAL);
        cx.background_executor.run_until_parked();
    }

    #[gpui::test]
    async fn test_a_newer_release_is_available() {
        let github = FakeGitHub::new(200, release("v0.100.1"));
        assert_eq!(
            check_against(&github).await.unwrap(),
            UpdateCheck::Available(available(Version::new(0, 100, 1), "v0.100.1"))
        );

        // The tag may also be the bare version.
        github.respond_with(200, release("0.101.0"));
        assert_eq!(
            check_against(&github).await.unwrap(),
            UpdateCheck::Available(available(Version::new(0, 101, 0), "0.101.0"))
        );

        assert_eq!(github.requests(), [RELEASES_URL, RELEASES_URL]);
    }

    #[gpui::test]
    async fn test_the_same_or_an_older_release_is_up_to_date() {
        let github = FakeGitHub::new(200, release("v0.100.0"));
        assert_eq!(check_against(&github).await.unwrap(), UpdateCheck::UpToDate);

        github.respond_with(200, release("v0.99.9"));
        assert_eq!(check_against(&github).await.unwrap(), UpdateCheck::UpToDate);
    }

    #[gpui::test]
    async fn test_the_running_versions_build_metadata_is_ignored() {
        let github = FakeGitHub::new(200, release("v0.100.0"));
        let installed = "0.100.0-pre+nightly.abc".parse::<Version>().unwrap();
        let check =
            check_latest_release(Some(REPOSITORY), installed, github.http_client.clone()).await;
        assert_eq!(check.unwrap(), UpdateCheck::UpToDate);
    }

    #[gpui::test]
    async fn test_pre_releases_are_skipped() {
        let github = FakeGitHub::new(
            200,
            releases(&[("v0.102.0-rc1", true), ("v0.100.1", false)]),
        );
        assert_eq!(
            check_against(&github).await.unwrap(),
            UpdateCheck::Available(available(Version::new(0, 100, 1), "v0.100.1"))
        );

        github.respond_with(200, releases(&[("v0.102.0-rc1", true)]));
        assert_eq!(check_against(&github).await.unwrap(), UpdateCheck::UpToDate);
    }

    #[gpui::test]
    async fn test_a_repository_without_releases_is_up_to_date() {
        let github = FakeGitHub::new(200, "[]");
        assert_eq!(check_against(&github).await.unwrap(), UpdateCheck::UpToDate);
    }

    #[gpui::test]
    async fn test_a_malformed_tag_is_an_error() {
        let github = FakeGitHub::new(200, release("latest"));
        let error = check_against(&github).await.unwrap_err();
        assert!(
            error
                .to_string()
                .contains(r#"release tag "latest" is not a version"#),
            "{error:#}"
        );
    }

    #[gpui::test]
    async fn test_a_rate_limit_is_recognized() {
        for status in [403, 429] {
            let github = FakeGitHub::new(status, r#"{"message":"API rate limit exceeded"}"#);
            let error = check_against(&github).await.unwrap_err();
            let rate_limited = error
                .downcast_ref::<RateLimited>()
                .unwrap_or_else(|| panic!("{status} should be a rate limit, got: {error:#}"));
            assert_eq!(rate_limited.status, status);
        }

        // Any other refusal is an ordinary error that names the repository.
        let github = FakeGitHub::new(404, r#"{"message":"Not Found"}"#);
        let error = check_against(&github).await.unwrap_err();
        assert!(error.downcast_ref::<RateLimited>().is_none());
        assert!(error.to_string().contains(REPOSITORY), "{error:#}");
    }

    #[gpui::test]
    async fn test_nothing_is_requested_without_a_repository() {
        let github = FakeGitHub::new(200, release("v9.0.0"));
        let check = check_latest_release(None, installed(), github.http_client.clone()).await;
        assert_eq!(check.unwrap(), UpdateCheck::NotConfigured);
        assert_eq!(github.requests(), Vec::<String>::new());
    }

    #[test]
    fn test_release_tags() {
        assert_eq!(parse_release_tag("v1.2.3").unwrap(), Version::new(1, 2, 3));
        assert_eq!(parse_release_tag("1.2.3").unwrap(), Version::new(1, 2, 3));
        assert!(parse_release_tag("v1.2").is_err());
        assert!(parse_release_tag("release-1.2.3").is_err());
        assert!(parse_release_tag("").is_err());
    }

    #[test]
    fn test_releases_repository_must_be_a_slug_and_not_upstreams() {
        assert_eq!(
            validated_releases_repository("example/ide").as_deref(),
            Some("example/ide")
        );
        assert_eq!(
            validated_releases_repository("Some-Org/some.repo_1").as_deref(),
            Some("Some-Org/some.repo_1")
        );
        for slug in [
            "",
            "ide",
            "example/",
            "/ide",
            "example/ide/extra",
            "https://github.com/example/ide",
            "example/ide?x=1",
            "../ide",
            "zed-industries/zed",
            "Zed-Industries/zed",
        ] {
            assert_eq!(validated_releases_repository(slug), None, "{slug:?}");
        }
    }

    #[test]
    fn test_the_brand_names_no_upstream_repository() {
        // Whatever the brand file says, the app never asks upstream's releases.
        if let Some(repository) = configured_releases_repository() {
            assert!(
                !repository
                    .to_ascii_lowercase()
                    .starts_with(UPSTREAM_REPOSITORY_OWNER)
            );
        }
    }

    #[gpui::test]
    fn test_auto_update_setting_is_read_from_the_settings(cx: &mut TestAppContext) {
        cx.update(|cx| {
            settings::init(cx);
            set_auto_update(true, cx);
            assert!(AutoUpdateSetting::get_global(cx).0);
            set_auto_update(false, cx);
            assert!(!AutoUpdateSetting::get_global(cx).0);
        });
    }

    #[gpui::test]
    async fn test_polling_announces_a_newer_release_and_downloads_nothing(cx: &mut TestAppContext) {
        let github = FakeGitHub::new(200, release("v0.100.0"));
        let updater =
            init_test_updater(&github, ReleaseChannel::Stable, Some(REPOSITORY), true, cx);
        cx.background_executor.run_until_parked();

        updater.read_with(cx, |updater, _| {
            assert_eq!(updater.status(), AutoUpdateStatus::Idle);
            assert_eq!(updater.available_update(), None);
            assert_eq!(updater.current_version(), installed());
        });
        assert_eq!(github.requests(), [RELEASES_URL]);

        github.respond_with(200, release("v0.100.1"));
        a_day_passes(cx);

        let update = available(Version::new(0, 100, 1), "v0.100.1");
        updater.read_with(cx, |updater, _| {
            // Finding a release is the end of it: nothing is downloaded or
            // installed, so the status is back to idle.
            assert_eq!(updater.status(), AutoUpdateStatus::Idle);
            assert_eq!(updater.available_update(), Some(update.clone()));
        });

        // GitHub refuses the next check: nothing changes, nothing is shown.
        github.respond_with(403, r#"{"message":"API rate limit exceeded"}"#);
        a_day_passes(cx);
        updater.read_with(cx, |updater, _| {
            assert_eq!(updater.status(), AutoUpdateStatus::Idle);
            assert_eq!(updater.available_update(), Some(update.clone()));
        });

        // A dismissed release stays quiet until a newer one appears.
        updater.update(cx, |updater, cx| updater.dismiss_available_update(cx));
        github.respond_with(200, release("v0.100.1"));
        a_day_passes(cx);
        updater.read_with(cx, |updater, _| {
            assert_eq!(updater.available_update(), None)
        });

        github.respond_with(200, release("v0.100.2"));
        a_day_passes(cx);
        updater.read_with(cx, |updater, _| {
            assert_eq!(
                updater.available_update(),
                Some(available(Version::new(0, 100, 2), "v0.100.2"))
            );
        });

        // Five checks, five requests, all to the releases list: the release
        // itself was never fetched.
        assert_eq!(github.requests(), [RELEASES_URL; 5]);
    }

    #[gpui::test]
    async fn test_no_polling_while_the_setting_is_off(cx: &mut TestAppContext) {
        let github = FakeGitHub::new(200, release("v0.100.1"));
        let updater =
            init_test_updater(&github, ReleaseChannel::Stable, Some(REPOSITORY), false, cx);
        a_day_passes(cx);
        a_day_passes(cx);
        assert_eq!(github.requests(), Vec::<String>::new());
        updater.read_with(cx, |updater, _| {
            assert_eq!(updater.available_update(), None)
        });

        // Turning the setting on starts the checks, turning it off stops them.
        cx.update(|cx| set_auto_update(true, cx));
        cx.background_executor.run_until_parked();
        assert_eq!(github.requests(), [RELEASES_URL]);

        cx.update(|cx| set_auto_update(false, cx));
        a_day_passes(cx);
        a_day_passes(cx);
        assert_eq!(github.requests(), [RELEASES_URL]);
    }

    #[gpui::test]
    async fn test_no_polling_in_the_dev_channel(cx: &mut TestAppContext) {
        let github = FakeGitHub::new(200, release("v0.100.1"));
        init_test_updater(&github, ReleaseChannel::Dev, Some(REPOSITORY), true, cx);
        a_day_passes(cx);
        a_day_passes(cx);
        assert_eq!(github.requests(), Vec::<String>::new());
    }

    #[gpui::test]
    async fn test_no_polling_without_a_repository(cx: &mut TestAppContext) {
        let github = FakeGitHub::new(200, release("v0.100.1"));
        let updater = init_test_updater(&github, ReleaseChannel::Stable, None, true, cx);
        a_day_passes(cx);

        // Even a direct request to check does nothing.
        updater.update(cx, |updater, cx| updater.poll(UpdateCheckType::Manual, cx));
        cx.background_executor.run_until_parked();
        updater.read_with(cx, |updater, _| {
            assert_eq!(updater.status(), AutoUpdateStatus::Idle);
            assert_eq!(updater.available_update(), None);
        });
        assert_eq!(github.requests(), Vec::<String>::new());
    }

    #[gpui::test]
    async fn test_no_polling_with_a_packagers_explanation(cx: &mut TestAppContext) {
        let github = FakeGitHub::new(200, release("v0.100.1"));
        cx.update(|cx| {
            settings::init(cx);
            set_auto_update(true, cx);
            release_channel::init_test(installed(), ReleaseChannel::Stable, cx);
            let client = Client::new(
                Arc::new(FakeSystemClock::new()),
                github.http_client.clone(),
                cx,
            );
            init_updater(
                Some(REPOSITORY.to_string()),
                Some("Use the package manager.".to_string()),
                client,
                cx,
            );
        });
        a_day_passes(cx);
        assert_eq!(github.requests(), Vec::<String>::new());

        let cx = cx.add_empty_window();
        cx.update(|window, cx| {
            check_with_explanation(Some("Use the package manager.".to_string()), window, cx)
        });
        cx.run_until_parked();
        assert_eq!(
            cx.pending_prompt(),
            Some((
                format!(
                    "{} was installed via a package manager.",
                    branding::PRODUCT_NAME
                ),
                "Use the package manager.".to_string()
            ))
        );
        assert_eq!(github.requests(), Vec::<String>::new());
    }

    fn check_manually(cx: &mut VisualTestContext) {
        cx.update(|window, cx| check_with_explanation(None, window, cx));
        cx.run_until_parked();
    }

    #[gpui::test]
    async fn test_manual_check_without_a_repository_says_so_and_requests_nothing(
        cx: &mut TestAppContext,
    ) {
        for channel in [ReleaseChannel::Stable, ReleaseChannel::Dev] {
            let github = FakeGitHub::new(200, release("v9.0.0"));
            let updater = init_test_updater(&github, channel, None, true, cx);
            let cx = cx.add_empty_window();
            check_manually(cx);

            let (message, detail) = cx.pending_prompt().expect("a prompt should be open");
            assert_eq!(message, NOT_SET_UP_MESSAGE);
            assert_eq!(
                detail,
                format!("New versions will be announced on {}.", website_name())
            );
            assert!(!website_name().contains('/'), "{}", website_name());
            cx.simulate_prompt_answer("OK");
            cx.run_until_parked();

            updater.read_with(cx, |updater, _| {
                assert_eq!(updater.status(), AutoUpdateStatus::Idle);
                assert_eq!(updater.available_update(), None);
            });
            assert_eq!(github.requests(), Vec::<String>::new());
            assert_eq!(cx.opened_url(), None);
        }
    }

    #[gpui::test]
    async fn test_manual_check_offers_the_release_page(cx: &mut TestAppContext) {
        let github = FakeGitHub::new(200, release("v0.100.1"));
        // The manual check does not depend on the `auto_update` setting.
        let updater =
            init_test_updater(&github, ReleaseChannel::Stable, Some(REPOSITORY), false, cx);
        let cx = cx.add_empty_window();
        check_manually(cx);

        let update = available(Version::new(0, 100, 1), "v0.100.1");
        updater.read_with(cx, |updater, _| {
            assert_eq!(updater.status(), AutoUpdateStatus::Idle);
            assert_eq!(updater.available_update(), Some(update.clone()));
        });
        let (message, _) = cx.pending_prompt().expect("a prompt should be open");
        assert_eq!(
            message,
            format!("{} 0.100.1 is available", branding::PRODUCT_NAME)
        );
        assert_eq!(cx.opened_url(), None);

        cx.simulate_prompt_answer("Open Release Page");
        cx.run_until_parked();
        assert_eq!(cx.opened_url(), Some(update.url));
        assert_eq!(github.requests(), [RELEASES_URL]);
    }

    #[gpui::test]
    async fn test_manual_check_declined_opens_nothing(cx: &mut TestAppContext) {
        let github = FakeGitHub::new(200, release("v0.100.1"));
        init_test_updater(&github, ReleaseChannel::Stable, Some(REPOSITORY), false, cx);
        let cx = cx.add_empty_window();
        check_manually(cx);

        cx.simulate_prompt_answer("Later");
        cx.run_until_parked();
        assert_eq!(cx.opened_url(), None);
        assert_eq!(github.requests(), [RELEASES_URL]);
    }

    #[gpui::test]
    async fn test_manual_check_reports_up_to_date(cx: &mut TestAppContext) {
        let github = FakeGitHub::new(200, release("v0.100.0"));
        let updater =
            init_test_updater(&github, ReleaseChannel::Stable, Some(REPOSITORY), false, cx);
        let up_to_date_events = Arc::new(Mutex::new(0));
        let _subscription = cx.update(|cx| {
            let up_to_date_events = up_to_date_events.clone();
            cx.subscribe(&updater, move |_, event, _| match event {
                AutoUpdateEvent::UpToDate => *up_to_date_events.lock() += 1,
            })
        });
        let cx = cx.add_empty_window();
        check_manually(cx);

        assert_eq!(*up_to_date_events.lock(), 1);
        assert!(!cx.has_pending_prompt());
        updater.read_with(cx, |updater, _| {
            assert_eq!(updater.status(), AutoUpdateStatus::Idle);
            assert_eq!(updater.available_update(), None);
        });
    }

    #[gpui::test]
    async fn test_manual_check_shows_a_refusal(cx: &mut TestAppContext) {
        let github = FakeGitHub::new(403, r#"{"message":"API rate limit exceeded"}"#);
        let updater =
            init_test_updater(&github, ReleaseChannel::Stable, Some(REPOSITORY), false, cx);
        let cx = cx.add_empty_window();
        check_manually(cx);

        assert!(!cx.has_pending_prompt());
        updater.read_with(cx, |updater, _| {
            let AutoUpdateStatus::Errored { error } = updater.status() else {
                panic!("expected an error, got {:?}", updater.status());
            };
            assert!(error.downcast_ref::<RateLimited>().is_some(), "{error:#}");
            assert_eq!(updater.available_update(), None);
        });
        assert_eq!(github.requests(), [RELEASES_URL]);
    }

    #[gpui::test]
    async fn test_manual_check_in_the_dev_channel_requests_nothing(cx: &mut TestAppContext) {
        let github = FakeGitHub::new(200, release("v0.100.1"));
        init_test_updater(&github, ReleaseChannel::Dev, Some(REPOSITORY), true, cx);
        let cx = cx.add_empty_window();
        check_manually(cx);

        let (message, _) = cx.pending_prompt().expect("a prompt should be open");
        assert_eq!(message, "Development builds do not check for updates.");
        assert_eq!(github.requests(), Vec::<String>::new());
    }

    #[gpui::test]
    async fn test_release_notes_are_the_products_own_release_page(cx: &mut TestAppContext) {
        let github = FakeGitHub::new(200, release("v0.100.0"));
        init_test_updater(&github, ReleaseChannel::Stable, Some(REPOSITORY), false, cx);
        cx.update(|cx| {
            assert_eq!(
                release_notes_url(cx).as_deref(),
                Some("https://github.com/example/ide/releases/tag/v0.100.0")
            );
            view_release_notes(&ViewReleaseNotes, cx);
        });
        assert_eq!(
            cx.opened_url().as_deref(),
            Some("https://github.com/example/ide/releases/tag/v0.100.0")
        );
        assert_eq!(github.requests(), Vec::<String>::new());
    }

    #[gpui::test]
    async fn test_release_notes_of_a_dev_build_are_the_list_of_releases(cx: &mut TestAppContext) {
        let github = FakeGitHub::new(200, release("v0.100.0"));
        init_test_updater(&github, ReleaseChannel::Dev, Some(REPOSITORY), false, cx);
        cx.update(|cx| {
            assert_eq!(
                release_notes_url(cx).as_deref(),
                Some("https://github.com/example/ide/releases")
            );
        });
    }

    #[gpui::test]
    async fn test_no_release_notes_without_a_repository(cx: &mut TestAppContext) {
        let github = FakeGitHub::new(200, release("v0.100.0"));
        init_test_updater(&github, ReleaseChannel::Stable, None, false, cx);
        cx.update(|cx| {
            assert_eq!(release_notes_url(cx), None);
            view_release_notes(&ViewReleaseNotes, cx);
        });
        assert_eq!(cx.opened_url(), None);
        assert_eq!(github.requests(), Vec::<String>::new());
    }

    #[gpui::test]
    async fn test_no_remote_server_is_ever_downloaded(cx: &mut TestAppContext) {
        let github = FakeGitHub::new(200, release("v0.100.1"));
        init_test_updater(&github, ReleaseChannel::Stable, Some(REPOSITORY), false, cx);
        let mut cx = cx.to_async();

        let url = AutoUpdater::get_remote_server_release_url(
            ReleaseChannel::Stable,
            Some(installed()),
            "linux",
            "x86_64",
            &mut cx,
        )
        .await
        .unwrap();
        assert_eq!(url, None);

        let error = AutoUpdater::download_remote_server_release(
            ReleaseChannel::Stable,
            Some(installed()),
            "linux",
            "x86_64",
            |_, _| {},
            &mut cx,
        )
        .await
        .unwrap_err();
        assert!(
            error
                .to_string()
                .contains("does not publish remote server builds"),
            "{error:#}"
        );
        assert_eq!(github.requests(), Vec::<String>::new());
    }
}

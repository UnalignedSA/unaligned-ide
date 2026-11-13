//! What is left of upstream's telemetry client.
//!
//! This build sends no telemetry, whatever the `telemetry.metrics` setting
//! says: there is no sender. Events are dropped where they are reported, no
//! queue is kept, nothing is posted and no `telemetry.log` file is written.
//! The type and its public functions remain because the rest of the app
//! compiles against them (identifiers of the installation, the settings, the
//! `telemetry::event!` call sites, which now do nothing).

use crate::TelemetrySettings;
use anyhow::{Context as _, Result};
use clock::SystemClock;
use fs::Fs;
use futures::channel::mpsc;
use gpui::{App, Task};
use http_client::HttpClientWithUrl;
use parking_lot::Mutex;
use regex::Regex;
use release_channel::ReleaseChannel;
use settings::{Settings, SettingsStore};
use sha2::{Digest, Sha256};
use std::collections::HashSet;
use std::sync::LazyLock;
use std::{env, path::PathBuf, sync::Arc};
use telemetry_events::{AssistantEventData, AssistantPhase, Event, EventWrapper};

pub struct TelemetrySubscription {
    pub historical_events: Result<HistoricalEvents>,
    pub queued_events: Vec<EventWrapper>,
    pub live_events: mpsc::UnboundedReceiver<EventWrapper>,
}

pub struct HistoricalEvents {
    pub events: Vec<EventWrapper>,
    pub parse_error_count: usize,
}
use worktree::{UpdatedEntriesSet, WorktreeId};

pub struct Telemetry {
    state: Arc<Mutex<TelemetryState>>,
}

struct TelemetryState {
    settings: TelemetrySettings,
    system_id: Option<Arc<str>>,       // Per system
    installation_id: Option<Arc<str>>, // Per app installation (different for dev, nightly, preview, and stable)
    metrics_id: Option<Arc<str>>,      // Per logged-in user
    is_staff: Option<bool>,
    worktrees_with_project_type_events_sent: HashSet<WorktreeId>,
}

static IDE_CLIENT_CHECKSUM_SEED: LazyLock<Option<Vec<u8>>> = LazyLock::new(|| {
    option_env!("IDE_CLIENT_CHECKSUM_SEED")
        .map(|s| s.as_bytes().into())
        .or_else(|| {
            env::var("IDE_CLIENT_CHECKSUM_SEED")
                .ok()
                .map(|s| s.as_bytes().into())
        })
});

/// Where crash reports are uploaded. `None` in this product: nothing sets the
/// variable, at build time or at run time, so no crash handler process is
/// started outside the dev channel and no minidump is ever uploaded. Never
/// give it a default.
pub static MINIDUMP_ENDPOINT: LazyLock<Option<String>> = LazyLock::new(|| {
    option_env!("IDE_MINIDUMP_ENDPOINT")
        .map(str::to_string)
        .or_else(|| env::var("IDE_MINIDUMP_ENDPOINT").ok())
});

/// Whether the crash handler process is started. Without an endpoint this is
/// false unless a developer asks for local minidumps with
/// `IDE_GENERATE_MINIDUMPS`.
pub fn should_install_crash_handler(channel: ReleaseChannel) -> bool {
    matches!(
        env::var("IDE_GENERATE_MINIDUMPS").as_deref(),
        Ok("true" | "1")
    ) || (channel != ReleaseChannel::Dev && MINIDUMP_ENDPOINT.is_some())
}

static DOTNET_PROJECT_FILES_REGEX: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"^(global\.json|Directory\.Build\.props|.*\.(csproj|fsproj|vbproj|sln))$").unwrap()
});

/// Returns the OS name, including the display server the platform connected to.
#[cfg_attr(
    not(any(target_os = "linux", target_os = "freebsd")),
    expect(unused_variables)
)]
pub fn os_name(cx: &App) -> String {
    #[cfg(target_os = "macos")]
    {
        "macOS".to_string()
    }
    #[cfg(target_os = "linux")]
    {
        format!("Linux {}", cx.compositor_name())
    }
    #[cfg(target_os = "freebsd")]
    {
        format!("FreeBSD {}", cx.compositor_name())
    }

    #[cfg(target_os = "windows")]
    {
        "Windows".to_string()
    }
}

/// Returns the OS name, guessing the display server from the environment.
///
/// Prefer [`os_name`] once an [`App`] exists; the platform may connect to a
/// different display server than the environment suggests.
pub fn guessed_os_name() -> String {
    #[cfg(target_os = "macos")]
    {
        "macOS".to_string()
    }
    #[cfg(target_os = "linux")]
    {
        format!("Linux {}", gpui::guess_compositor())
    }
    #[cfg(target_os = "freebsd")]
    {
        format!("FreeBSD {}", gpui::guess_compositor())
    }

    #[cfg(target_os = "windows")]
    {
        "Windows".to_string()
    }
}

/// Note: This might do blocking IO! Only call from background threads
pub fn os_version() -> String {
    cfg_select! {
       feature = "test-support" => {
           // MacOS branch in particular is quite slow, hence we ought to "avoid" it in tests.
           "test binary".to_owned()
       }
       target_os = "macos" => {
           static MACOS_VERSION_REGEX: LazyLock<Regex> = LazyLock::new(|| {
               Regex::new(r"(\s*\(Build [^)]*[0-9]\))").unwrap()
           });
           use objc2_foundation::NSProcessInfo;
           let process_info = NSProcessInfo::processInfo();
           let version_nsstring = process_info.operatingSystemVersionString();
           // "Version 15.6.1 (Build 24G90)" -> "15.6.1 (Build 24G90)"
           let version_string = version_nsstring.to_string().replace("Version ", "");
           // "15.6.1 (Build 24G90)" -> "15.6.1"
           // "26.0.0 (Build 25A5349a)" -> unchanged (Beta or Rapid Security Response; ends with letter)
           MACOS_VERSION_REGEX
               .replace_all(&version_string, "")
               .to_string()
       }
       any(target_os = "linux", target_os = "freebsd") => {
           use std::path::Path;

           let content = if let Ok(file) = std::fs::read_to_string(&Path::new("/etc/os-release")) {
               file
           } else if let Ok(file) = std::fs::read_to_string(&Path::new("/usr/lib/os-release")) {
               file
           } else if let Ok(file) = std::fs::read_to_string(&Path::new("/var/run/os-release")) {
               file
           } else {
               log::error!(
                   "Failed to load /etc/os-release, /usr/lib/os-release, or /var/run/os-release"
               );
               "".to_string()
           };
           util::parse_os_release(&content).unwrap_or_else(|| "unknown".to_string())
       }
       target_os = "windows" => {
           let mut info = unsafe { std::mem::zeroed() };
           let status = unsafe { windows::Wdk::System::SystemServices::RtlGetVersion(&mut info) };
           if status.is_ok() {
               semver::Version::new(
                   info.dwMajorVersion as _,
                   info.dwMinorVersion as _,
                   info.dwBuildNumber as _,
               )
               .to_string()
           } else {
               "unknown".to_string()
           }
       }
    }
}

impl Telemetry {
    /// `_http_client` is kept for the callers: nothing is ever sent, so the
    /// client is not even stored.
    pub fn new(
        _clock: Arc<dyn SystemClock>,
        _http_client: Arc<HttpClientWithUrl>,
        cx: &mut App,
    ) -> Arc<Self> {
        let state = Arc::new(Mutex::new(TelemetryState {
            settings: *TelemetrySettings::get_global(cx),
            system_id: None,
            installation_id: None,
            metrics_id: None,
            is_staff: None,
            worktrees_with_project_type_events_sent: HashSet::new(),
        }));

        cx.observe_global::<SettingsStore>({
            let state = state.clone();

            move |cx| {
                let mut state = state.lock();
                state.settings = *TelemetrySettings::get_global(cx);
            }
        })
        .detach();

        // Upstream installs the queue of `telemetry::event!` here
        // (`::telemetry::init`) and forwards it to the sender. This build
        // never installs it, so every `telemetry::event!` in the app is
        // dropped at its call site; and no `telemetry.log` is created.
        Arc::new(Self { state })
    }

    /// Where upstream mirrors the events it sends. This build never creates
    /// the file.
    pub fn log_file_path() -> PathBuf {
        paths::logs_dir().join("telemetry.log")
    }

    /// For the view that shows what telemetry is sent: no history, nothing
    /// queued, and a stream of live events that is already closed.
    pub async fn subscribe_with_history(
        self: &Arc<Self>,
        _fs: Arc<dyn Fs>,
    ) -> TelemetrySubscription {
        let (_, live_events) = mpsc::unbounded();
        TelemetrySubscription {
            historical_events: Ok(HistoricalEvents {
                events: Vec::new(),
                parse_error_count: 0,
            }),
            queued_events: Vec::new(),
            live_events,
        }
    }

    pub fn has_checksum_seed(&self) -> bool {
        IDE_CLIENT_CHECKSUM_SEED.is_some()
    }

    /// Records the identifiers other parts of the app ask for
    /// ([`Self::system_id`], [`Self::installation_id`]). It starts nothing.
    pub fn start(
        self: &Arc<Self>,
        system_id: Option<String>,
        installation_id: Option<String>,
        _session_id: String,
        _cx: &App,
    ) {
        let mut state = self.state.lock();
        state.system_id = system_id.map(|id| id.into());
        state.installation_id = installation_id.map(|id| id.into());
    }

    pub fn metrics_enabled(self: &Arc<Self>) -> bool {
        self.state.lock().settings.metrics
    }

    pub fn diagnostics_enabled(self: &Arc<Self>) -> bool {
        self.state.lock().settings.diagnostics
    }

    pub fn set_authenticated_user_info(
        self: &Arc<Self>,
        metrics_id: Option<String>,
        is_staff: bool,
    ) {
        let mut state = self.state.lock();

        if !state.settings.metrics {
            return;
        }

        let metrics_id: Option<Arc<str>> = metrics_id.map(|id| id.into());
        state.metrics_id.clone_from(&metrics_id);
        state.is_staff = Some(is_staff);
        drop(state);
    }

    pub fn report_assistant_event(self: &Arc<Self>, event: AssistantEventData) {
        let event_type = match event.phase {
            AssistantPhase::Response => "Assistant Responded",
            AssistantPhase::Invoked => "Assistant Invoked",
            AssistantPhase::Accepted => "Assistant Response Accepted",
            AssistantPhase::Rejected => "Assistant Response Rejected",
        };

        telemetry::event!(
            event_type,
            conversation_id = event.conversation_id,
            kind = event.kind,
            phase = event.phase,
            message_id = event.message_id,
            model = event.model,
            model_provider = event.model_provider,
            response_latency = event.response_latency,
            error_message = event.error_message,
            language_name = event.language_name,
        );
    }

    /// Upstream coalesces edits into "Editor Edited" events. Events go
    /// nowhere in this build, so an edit costs nothing here: no lock, no clock.
    pub fn log_edit_event(self: &Arc<Self>, _environment: &'static str, _is_via_ssh: bool) {}

    /// Upstream reports "Project Opened" with the kind of project it finds in
    /// a worktree update. Events go nowhere in this build, so nothing is
    /// scanned.
    pub fn report_discovered_project_type_events(
        self: &Arc<Self>,
        _worktree_id: WorktreeId,
        _updated_entries_set: &UpdatedEntriesSet,
    ) {
    }

    // Upstream's detection, kept for merges and for its four tests: nothing
    // calls it in this build.
    #[allow(dead_code)]
    fn detect_project_types(
        self: &Arc<Self>,
        worktree_id: WorktreeId,
        updated_entries_set: &UpdatedEntriesSet,
    ) -> Option<Vec<String>> {
        let mut state = self.state.lock();

        if state
            .worktrees_with_project_type_events_sent
            .contains(&worktree_id)
        {
            return None;
        }

        let mut project_types: HashSet<&str> = HashSet::new();

        for (path, _, _) in updated_entries_set.iter() {
            let Some(file_name) = path.file_name() else {
                continue;
            };

            let project_type = match file_name {
                "pnpm-lock.yaml" => Some("pnpm"),
                "yarn.lock" => Some("yarn"),
                "package.json" => Some("node"),
                _ if DOTNET_PROJECT_FILES_REGEX.is_match(file_name) => Some("dotnet"),
                _ => None,
            };

            if let Some(project_type) = project_type {
                project_types.insert(project_type);
            };
        }

        if !project_types.is_empty() {
            state
                .worktrees_with_project_type_events_sent
                .insert(worktree_id);
        }

        let mut project_types: Vec<_> = project_types.into_iter().map(String::from).collect();
        project_types.sort();
        Some(project_types)
    }

    /// Report a telemetry event that originated on a remote server.
    ///
    /// The remote server cannot upload telemetry itself, so it forwards events
    /// (as a JSON-serialized [`Event`]) to the client. Since the OS metadata in
    /// [`EventRequestBody`] is batch-level (describing the uploading client),
    /// the remote server's OS is attached as event properties instead, so the
    /// origin can still be distinguished downstream.
    pub fn report_remote_event(
        self: &Arc<Self>,
        event_json: &str,
        connection_type: &str,
        os_name: String,
        os_version: Option<String>,
        architecture: String,
    ) -> Result<()> {
        // The remote server forwards a bare `telemetry_events::FlexibleEvent`
        // (the type behind `telemetry::event!`), not the tagged `Event` enum.
        let mut flexible: telemetry_events::FlexibleEvent =
            serde_json::from_str(event_json).context("invalid remote telemetry event")?;
        flexible
            .event_properties
            .insert("remote".into(), true.into());
        flexible
            .event_properties
            .insert("remote_connection_type".into(), connection_type.into());
        flexible
            .event_properties
            .insert("remote_os_name".into(), os_name.into());
        flexible
            .event_properties
            .insert("remote_architecture".into(), architecture.into());
        if let Some(os_version) = os_version {
            flexible
                .event_properties
                .insert("remote_os_version".into(), os_version.into());
        }
        self.report_event(Event::Flexible(flexible));
        Ok(())
    }

    /// Upstream returns the events waiting to be sent. Nothing is ever queued
    /// in this build, so the list is always empty.
    #[cfg(any(test, feature = "test-support"))]
    pub fn queued_events(self: &Arc<Self>) -> Vec<telemetry_events::FlexibleEvent> {
        Vec::new()
    }

    /// Drops the event. There is no queue and no sender: this is the single
    /// place every reported event ends, whatever `telemetry.metrics` says.
    fn report_event(self: &Arc<Self>, event: Event) {
        drop(event);
    }

    pub fn metrics_id(self: &Arc<Self>) -> Option<Arc<str>> {
        self.state.lock().metrics_id.clone()
    }

    pub fn system_id(self: &Arc<Self>) -> Option<Arc<str>> {
        self.state.lock().system_id.clone()
    }

    pub fn installation_id(self: &Arc<Self>) -> Option<Arc<str>> {
        self.state.lock().installation_id.clone()
    }

    pub fn is_staff(self: &Arc<Self>) -> Option<bool> {
        self.state.lock().is_staff
    }

    /// Upstream posts the queued events here. This build has nothing to
    /// post and no code that could.
    pub async fn flush_events_inner(self: &Arc<Self>) -> Result<()> {
        Ok(())
    }

    /// See [`Self::flush_events_inner`]: completes at once, sends nothing.
    pub fn flush_events(self: &Arc<Self>) -> Task<()> {
        Task::ready(())
    }
}

pub fn calculate_json_checksum(json: &impl AsRef<[u8]>) -> Option<String> {
    let checksum_seed = IDE_CLIENT_CHECKSUM_SEED.as_ref()?;

    let mut summer = Sha256::new();
    summer.update(checksum_seed);
    summer.update(json);
    summer.update(checksum_seed);
    let mut checksum = String::new();
    for byte in summer.finalize().as_slice() {
        use std::fmt::Write;
        write!(&mut checksum, "{:02x}", byte).unwrap();
    }

    Some(checksum)
}

#[cfg(test)]
mod tests {
    use super::*;
    use clock::FakeSystemClock;
    use std::time::Duration;

    use gpui::{BackgroundExecutor, TestAppContext, UpdateGlobal as _};
    use http_client::FakeHttpClient;
    use std::collections::HashMap;
    use std::sync::atomic::{AtomicUsize, Ordering};
    use telemetry_events::FlexibleEvent;
    use util::rel_path::RelPath;
    use worktree::{PathChange, ProjectEntryId, WorktreeId};

    /// Upstream queued events and flushed them when the queue was full. This
    /// build has no sender: with `telemetry.metrics` forced on, many events
    /// reported and an explicit flush, nothing is queued and no request
    /// reaches the HTTP client.
    #[gpui::test]
    async fn test_events_are_dropped_whatever_the_setting(
        executor: BackgroundExecutor,
        cx: &mut TestAppContext,
    ) {
        init_test(cx);
        set_metrics(true, cx);
        let (http, requests) = counting_http_client();
        let clock = Arc::new(FakeSystemClock::new());

        let telemetry = cx.update(|cx| {
            let telemetry = Telemetry::new(clock.clone(), http, cx);
            telemetry.start(
                Some("system_id".to_string()),
                Some("installation_id".to_string()),
                "session_id".to_string(),
                cx,
            );
            telemetry
        });
        assert!(telemetry.metrics_enabled());

        for _ in 0..100 {
            telemetry.report_event(Event::Flexible(test_event()));
            ::telemetry::event!("Test Event", test_key = "test_value");
        }
        assert!(telemetry.queued_events().is_empty());

        telemetry.flush_events().await;
        telemetry.flush_events_inner().await.unwrap();
        executor.run_until_parked();

        assert!(telemetry.queued_events().is_empty());
        assert_eq!(requests.load(Ordering::SeqCst), 0);
    }

    /// Upstream flushed the queue after a delay. Here no timer is armed and
    /// nothing is sent, however long the app runs.
    #[gpui::test]
    async fn test_nothing_is_sent_after_the_flush_interval(
        executor: BackgroundExecutor,
        cx: &mut TestAppContext,
    ) {
        init_test(cx);
        set_metrics(true, cx);
        let (http, requests) = counting_http_client();
        let clock = Arc::new(FakeSystemClock::new());

        let telemetry = cx.update(|cx| {
            let telemetry = Telemetry::new(clock.clone(), http, cx);
            telemetry.start(
                Some("system_id".to_string()),
                Some("installation_id".to_string()),
                "session_id".to_string(),
                cx,
            );
            telemetry
        });

        telemetry.report_event(Event::Flexible(test_event()));
        executor.advance_clock(Duration::from_secs(60 * 60));
        executor.run_until_parked();

        assert!(telemetry.queued_events().is_empty());
        assert_eq!(requests.load(Ordering::SeqCst), 0);
    }

    /// The view of what telemetry is sent gets no history, no queue and a
    /// stream that has already ended.
    #[gpui::test]
    async fn test_subscription_is_empty(cx: &mut TestAppContext) {
        use futures::StreamExt as _;

        init_test(cx);
        set_metrics(true, cx);
        let (http, _requests) = counting_http_client();
        let clock = Arc::new(FakeSystemClock::new());
        let telemetry = cx.update(|cx| Telemetry::new(clock.clone(), http, cx));
        telemetry.report_event(Event::Flexible(test_event()));

        let fs = fs::FakeFs::new(cx.executor());
        let mut subscription = telemetry.subscribe_with_history(fs).await;
        let history = subscription.historical_events.unwrap();
        assert!(history.events.is_empty());
        assert_eq!(history.parse_error_count, 0);
        assert!(subscription.queued_events.is_empty());
        assert!(subscription.live_events.next().await.is_none());
    }

    /// Events forwarded by a remote server are parsed (a malformed one is
    /// still an error for the caller) and then dropped like local ones.
    #[gpui::test]
    async fn test_remote_events_are_dropped(cx: &mut TestAppContext) {
        init_test(cx);
        set_metrics(true, cx);
        let (http, requests) = counting_http_client();
        let clock = Arc::new(FakeSystemClock::new());

        let telemetry = cx.update(|cx| {
            let telemetry = Telemetry::new(clock.clone(), http, cx);
            telemetry.start(
                Some("system_id".to_string()),
                Some("installation_id".to_string()),
                "session_id".to_string(),
                cx,
            );
            telemetry
        });

        // Mirror what the remote server forwards: a bare `FlexibleEvent`, which
        // is the type produced by `telemetry::event!` / sent over the queue.
        let event_json = serde_json::to_string(&FlexibleEvent {
            event_type: "fs_watcher_poll".to_string(),
            event_properties: HashMap::from_iter([(
                "path".to_string(),
                serde_json::Value::String("/code/project".to_string()),
            )]),
        })
        .unwrap();

        telemetry
            .report_remote_event(
                &event_json,
                "ssh",
                "Linux".to_string(),
                Some("ubuntu 24.04".to_string()),
                "aarch64".to_string(),
            )
            .unwrap();
        assert!(
            telemetry
                .report_remote_event(
                    "not json",
                    "ssh",
                    "Linux".to_string(),
                    None,
                    "x".to_string()
                )
                .is_err()
        );

        telemetry.flush_events().await;
        assert!(telemetry.queued_events().is_empty());
        assert_eq!(requests.load(Ordering::SeqCst), 0);
    }

    #[gpui::test]
    fn test_project_discovery_does_not_double_report(cx: &mut gpui::TestAppContext) {
        init_test(cx);

        let clock = Arc::new(FakeSystemClock::new());
        let http = FakeHttpClient::with_200_response();
        let telemetry = cx.update(|cx| Telemetry::new(clock.clone(), http, cx));
        let worktree_id = 1;

        // Scan of empty worktree finds nothing
        test_project_discovery_helper(telemetry.clone(), vec![], Some(vec![]), worktree_id);

        // Files added, second scan of worktree 1 finds project type
        test_project_discovery_helper(
            telemetry.clone(),
            vec!["package.json"],
            Some(vec!["node"]),
            worktree_id,
        );

        // Third scan of worktree does not double report, as we already reported
        test_project_discovery_helper(telemetry, vec!["package.json"], None, worktree_id);
    }

    #[gpui::test]
    fn test_pnpm_project_discovery(cx: &mut gpui::TestAppContext) {
        init_test(cx);

        let clock = Arc::new(FakeSystemClock::new());
        let http = FakeHttpClient::with_200_response();
        let telemetry = cx.update(|cx| Telemetry::new(clock.clone(), http, cx));

        test_project_discovery_helper(
            telemetry,
            vec!["package.json", "pnpm-lock.yaml"],
            Some(vec!["node", "pnpm"]),
            1,
        );
    }

    #[gpui::test]
    fn test_yarn_project_discovery(cx: &mut gpui::TestAppContext) {
        init_test(cx);

        let clock = Arc::new(FakeSystemClock::new());
        let http = FakeHttpClient::with_200_response();
        let telemetry = cx.update(|cx| Telemetry::new(clock.clone(), http, cx));

        test_project_discovery_helper(
            telemetry,
            vec!["package.json", "yarn.lock"],
            Some(vec!["node", "yarn"]),
            1,
        );
    }

    #[gpui::test]
    fn test_dotnet_project_discovery(cx: &mut gpui::TestAppContext) {
        init_test(cx);

        let clock = Arc::new(FakeSystemClock::new());
        let http = FakeHttpClient::with_200_response();
        let telemetry = cx.update(|cx| Telemetry::new(clock.clone(), http, cx));

        // Using different worktrees, as production code blocks from reporting a
        // project type for the same worktree multiple times

        test_project_discovery_helper(
            telemetry.clone(),
            vec!["global.json"],
            Some(vec!["dotnet"]),
            1,
        );
        test_project_discovery_helper(
            telemetry.clone(),
            vec!["Directory.Build.props"],
            Some(vec!["dotnet"]),
            2,
        );
        test_project_discovery_helper(
            telemetry.clone(),
            vec!["file.csproj"],
            Some(vec!["dotnet"]),
            3,
        );
        test_project_discovery_helper(
            telemetry.clone(),
            vec!["file.fsproj"],
            Some(vec!["dotnet"]),
            4,
        );
        test_project_discovery_helper(
            telemetry.clone(),
            vec!["file.vbproj"],
            Some(vec!["dotnet"]),
            5,
        );
        test_project_discovery_helper(telemetry.clone(), vec!["file.sln"], Some(vec!["dotnet"]), 6);

        // Each worktree should only send a single project type event, even when
        // encountering multiple files associated with that project type
        test_project_discovery_helper(
            telemetry,
            vec!["global.json", "Directory.Build.props"],
            Some(vec!["dotnet"]),
            7,
        );
    }

    fn init_test(cx: &mut TestAppContext) {
        cx.update(|cx| {
            let settings_store = SettingsStore::test(cx);
            cx.set_global(settings_store);
        });
    }

    /// Sets `telemetry.metrics`, the setting that made upstream send events.
    fn set_metrics(enabled: bool, cx: &mut TestAppContext) {
        cx.update(|cx| {
            SettingsStore::update_global(cx, |store, cx| {
                store.update_user_settings(cx, |settings| {
                    settings.telemetry.get_or_insert_default().metrics = Some(enabled);
                });
            });
        });
    }

    /// An HTTP client that answers 200 and counts the requests it receives.
    fn counting_http_client() -> (Arc<HttpClientWithUrl>, Arc<AtomicUsize>) {
        let requests = Arc::new(AtomicUsize::new(0));
        let http = FakeHttpClient::create({
            let requests = requests.clone();
            move |_| {
                requests.fetch_add(1, Ordering::SeqCst);
                async move {
                    Ok(http_client::Response::builder()
                        .status(200)
                        .body(Default::default())
                        .unwrap())
                }
            }
        });
        (http, requests)
    }

    fn test_event() -> FlexibleEvent {
        FlexibleEvent {
            event_type: "test".to_string(),
            event_properties: HashMap::from_iter([(
                "test_key".to_string(),
                serde_json::Value::String("test_value".to_string()),
            )]),
        }
    }

    fn test_project_discovery_helper(
        telemetry: Arc<Telemetry>,
        file_paths: Vec<&str>,
        expected_project_types: Option<Vec<&str>>,
        worktree_id_num: usize,
    ) {
        let worktree_id = WorktreeId::from_usize(worktree_id_num);
        let entries: Vec<_> = file_paths
            .into_iter()
            .enumerate()
            .filter_map(|(i, path)| {
                Some((
                    Arc::from(RelPath::from_unix_str(path).ok()?),
                    ProjectEntryId::from_proto(i as u64 + 1),
                    PathChange::Added,
                ))
            })
            .collect();
        let updated_entries: UpdatedEntriesSet = Arc::from(entries.as_slice());

        let detected_project_types = telemetry.detect_project_types(worktree_id, &updated_entries);

        let expected_project_types =
            expected_project_types.map(|types| types.iter().map(|&t| t.to_string()).collect());

        assert_eq!(detected_project_types, expected_project_types);
    }
}

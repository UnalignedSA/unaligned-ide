//! When the extension registry is contacted without the user asking for it.
//!
//! Upstream asked the registry for updates, and installed the
//! `auto_install_extensions` entries, at every startup. Here the registry is
//! contacted for those two jobs only:
//!
//! - when the Extensions page is opened ([`page_opened`]),
//! - at most once every [`CHECK_INTERVAL`], the first time no earlier than
//!   [`FIRST_CHECK_DELAY`] after launch, and only while at least one extension
//!   from the registry is installed.
//!
//! The time of the last contact is kept in the key-value store, so restarting
//! the app does not bring the next check forward.

use std::time::{Duration, SystemTime, UNIX_EPOCH};

use db::kvp::KeyValueStore;
use extension_host::ExtensionStore;
use gpui::App;
use util::ResultExt as _;

/// How long after launch the first update check may run.
const FIRST_CHECK_DELAY: Duration = Duration::from_secs(10 * 60);
/// The shortest time between two update checks.
const CHECK_INTERVAL: Duration = Duration::from_secs(24 * 60 * 60);
/// How long to wait before looking again when there was nothing to check.
const IDLE_RECHECK_DELAY: Duration = Duration::from_secs(60 * 60);
/// Unix seconds of the last update check, in the key-value store.
const LAST_CHECK_KEY: &str = "extensions_last_registry_check";

pub(crate) fn init(cx: &mut App) {
    cx.spawn(async move |cx| {
        cx.background_executor().timer(FIRST_CHECK_DELAY).await;
        loop {
            let wait = cx.update(check_if_due);
            cx.background_executor().timer(wait).await;
        }
    })
    .detach();
}

/// The Extensions page was opened: the user is looking at the registry, so the
/// installed extensions are brought up to date now.
pub(crate) fn page_opened(cx: &mut App) {
    sync_with_registry(cx);
}

/// Runs the update check when one is due and returns how long to wait before
/// asking again.
fn check_if_due(cx: &mut App) -> Duration {
    let Some(store) = ExtensionStore::try_global(cx) else {
        return IDLE_RECHECK_DELAY;
    };
    if !store.read(cx).has_registry_extensions() {
        return IDLE_RECHECK_DELAY;
    }

    let last_check = KeyValueStore::global(cx)
        .read_kvp(LAST_CHECK_KEY)
        .log_err()
        .flatten()
        .and_then(|seconds| seconds.parse::<u64>().ok());
    match time_until_next_check(last_check, unix_seconds_now()) {
        Some(remaining) => remaining,
        None => {
            sync_with_registry(cx);
            CHECK_INTERVAL
        }
    }
}

fn sync_with_registry(cx: &mut App) {
    let Some(store) = ExtensionStore::try_global(cx) else {
        return;
    };
    store.update(cx, |store, cx| store.sync_with_registry(cx));

    let key_value_store = KeyValueStore::global(cx);
    let now = unix_seconds_now().to_string();
    db::write_and_log(cx, move || async move {
        key_value_store
            .write_kvp(LAST_CHECK_KEY.to_string(), now)
            .await
    });
}

/// `None` when a check is due, else the time left before the next one.
///
/// A last check in the future (the clock was set back) counts as due.
fn time_until_next_check(last_check: Option<u64>, now: u64) -> Option<Duration> {
    let elapsed = now.checked_sub(last_check?)?;
    let remaining = CHECK_INTERVAL.as_secs().checked_sub(elapsed)?;
    (remaining > 0).then(|| Duration::from_secs(remaining))
}

fn unix_seconds_now() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|duration| duration.as_secs())
        .unwrap_or_default()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_time_until_next_check() {
        let day = CHECK_INTERVAL.as_secs();
        let now = 1_800_000_000;

        // Never checked: due.
        assert_eq!(time_until_next_check(None, now), None);
        // Checked a day or more ago: due.
        assert_eq!(time_until_next_check(Some(now - day), now), None);
        assert_eq!(time_until_next_check(Some(now - 3 * day), now), None);
        // Checked an hour ago: twenty-three hours left.
        assert_eq!(
            time_until_next_check(Some(now - 3600), now),
            Some(Duration::from_secs(day - 3600))
        );
        // Checked just now (the Extensions page was opened): a full day.
        assert_eq!(time_until_next_check(Some(now), now), Some(CHECK_INTERVAL));
        // The clock went backwards: due, rather than waiting for it to catch up.
        assert_eq!(time_until_next_check(Some(now + 500), now), None);
    }
}

use std::collections::HashMap;
use std::future::Future;
use std::path::PathBuf;
use std::pin::Pin;
use std::sync::{Arc, LazyLock};

use anyhow::Result;
use credentials_provider::CredentialsProvider;
use futures::FutureExt as _;
use gpui::{App, AsyncApp, Global};
use release_channel::ReleaseChannel;

/// An environment variable whose presence indicates that the system keychain
/// should be used in development.
///
/// By default, running a debug build in the Dev channel uses the development
/// credentials provider. Setting this environment variable allows you to
/// interact with the system keychain (for instance, if you need to test
/// something).
///
/// Only works in development. Setting this environment variable in other
/// release channels, or in a release build, is a no-op: those always use the
/// system keychain.
static IDE_DEVELOPMENT_USE_KEYCHAIN: LazyLock<bool> = LazyLock::new(|| {
    std::env::var("IDE_DEVELOPMENT_USE_KEYCHAIN").is_ok_and(|value| !value.is_empty())
});

pub struct AppCredentialsProvider(pub Arc<dyn CredentialsProvider>);

impl Global for AppCredentialsProvider {}

/// Returns the global [`CredentialsProvider`].
pub fn init_global(cx: &mut App) {
    // The `CredentialsProvider` trait has `Send + Sync` bounds on it, so it
    // seems like this is a false positive from Clippy.
    #[allow(clippy::arc_with_non_send_sync)]
    let provider = new(cx);
    cx.set_global(AppCredentialsProvider(provider));
}

pub fn global(cx: &App) -> Arc<dyn CredentialsProvider> {
    cx.try_global::<AppCredentialsProvider>()
        .map(|provider| provider.0.clone())
        .unwrap_or_else(|| new(cx))
}

fn new(cx: &App) -> Arc<dyn CredentialsProvider> {
    let use_development_provider = use_development_provider(
        cfg!(debug_assertions),
        ReleaseChannel::try_global(cx),
        *IDE_DEVELOPMENT_USE_KEYCHAIN,
    );

    if use_development_provider {
        Arc::new(DevelopmentCredentialsProvider::new())
    } else {
        Arc::new(KeychainCredentialsProvider)
    }
}

/// Whether secrets go to the clear-text development file instead of the
/// system keychain.
///
/// Only a debug build may do that. A release build is what users run, and its
/// channel is `dev` until packaging sets another one: whatever the channel, it
/// must keep API keys in the system keychain, never in a readable file.
fn use_development_provider(
    is_debug_build: bool,
    release_channel: Option<ReleaseChannel>,
    development_use_keychain: bool,
) -> bool {
    if !is_debug_build {
        return false;
    }

    match release_channel {
        Some(ReleaseChannel::Dev) => {
            // In development we default to using the development
            // credentials provider to avoid getting spammed by relentless
            // keychain access prompts.
            //
            // However, if the `IDE_DEVELOPMENT_USE_KEYCHAIN` environment
            // variable is set, we will use the actual keychain.
            !development_use_keychain
        }
        Some(ReleaseChannel::Nightly | ReleaseChannel::Preview | ReleaseChannel::Stable) | None => {
            false
        }
    }
}

/// Returns the name under which the secret for `url` is kept in the system
/// keychain.
///
/// The platforms' keychains have no notion of "this application's entries":
/// an entry is found by its name alone, and upstream Zed names its entries by
/// the bare URL (`https://zed.dev` for its account token,
/// `https://api.anthropic.com` for an API key, …). Prefixing every name keeps
/// the two applications' secrets apart in both directions: this app never
/// reads, replaces or deletes an entry of Zed's, and never signs in with
/// Zed's account token.
///
/// Every access of [`KeychainCredentialsProvider`] goes through this function.
/// The un-prefixed name must never be read, not even as a migration.
fn keychain_key(url: &str) -> String {
    format!("{}{url}", branding::KEYCHAIN_PREFIX)
}

/// A credentials provider that stores credentials in the system keychain,
/// under names of this application's own (see [`keychain_key`]).
struct KeychainCredentialsProvider;

impl CredentialsProvider for KeychainCredentialsProvider {
    fn read_credentials<'a>(
        &'a self,
        url: &'a str,
        cx: &'a AsyncApp,
    ) -> Pin<Box<dyn Future<Output = Result<Option<(String, Vec<u8>)>>> + 'a>> {
        let key = keychain_key(url);
        async move { cx.update(|cx| cx.read_credentials(&key)).await }.boxed_local()
    }

    fn write_credentials<'a>(
        &'a self,
        url: &'a str,
        username: &'a str,
        password: &'a [u8],
        cx: &'a AsyncApp,
    ) -> Pin<Box<dyn Future<Output = Result<()>> + 'a>> {
        let key = keychain_key(url);
        async move {
            cx.update(move |cx| cx.write_credentials(&key, username, password))
                .await
        }
        .boxed_local()
    }

    #[cfg(target_os = "macos")]
    fn write_credentials_if_not_exists<'a>(
        &'a self,
        url: &'a str,
        username: &'a str,
        password: &'a [u8],
        cx: &'a AsyncApp,
    ) -> Pin<Box<dyn Future<Output = Result<bool>> + 'a>> {
        let key = keychain_key(url);
        async move {
            cx.update(move |cx| cx.write_credentials_if_not_exists(&key, username, password))
                .await
        }
        .boxed_local()
    }

    fn delete_credentials<'a>(
        &'a self,
        url: &'a str,
        cx: &'a AsyncApp,
    ) -> Pin<Box<dyn Future<Output = Result<()>> + 'a>> {
        let key = keychain_key(url);
        async move { cx.update(move |cx| cx.delete_credentials(&key)).await }.boxed_local()
    }
}

/// A credentials provider that stores credentials in a local file.
///
/// This MUST only be used in development, as this is not a secure way of storing
/// credentials on user machines.
///
/// Its existence is purely to work around the annoyance of having to constantly
/// re-allow access to the system keychain when developing the app. The file is
/// in the app's own configuration directory, so its keys need no prefix.
struct DevelopmentCredentialsProvider {
    path: PathBuf,
}

impl DevelopmentCredentialsProvider {
    fn new() -> Self {
        let path = paths::config_dir().join("development_credentials");

        Self { path }
    }

    fn load_credentials(&self) -> Result<HashMap<String, (String, Vec<u8>)>> {
        let json = std::fs::read(&self.path)?;
        let credentials: HashMap<String, (String, Vec<u8>)> = serde_json::from_slice(&json)?;

        Ok(credentials)
    }

    fn save_credentials(&self, credentials: &HashMap<String, (String, Vec<u8>)>) -> Result<()> {
        let json = serde_json::to_string(credentials)?;
        std::fs::write(&self.path, json)?;

        Ok(())
    }
}

impl CredentialsProvider for DevelopmentCredentialsProvider {
    fn read_credentials<'a>(
        &'a self,
        url: &'a str,
        _cx: &'a AsyncApp,
    ) -> Pin<Box<dyn Future<Output = Result<Option<(String, Vec<u8>)>>> + 'a>> {
        async move {
            Ok(self
                .load_credentials()
                .unwrap_or_default()
                .get(url)
                .cloned())
        }
        .boxed_local()
    }

    fn write_credentials<'a>(
        &'a self,
        url: &'a str,
        username: &'a str,
        password: &'a [u8],
        _cx: &'a AsyncApp,
    ) -> Pin<Box<dyn Future<Output = Result<()>> + 'a>> {
        async move {
            let mut credentials = self.load_credentials().unwrap_or_default();
            credentials.insert(url.to_string(), (username.to_string(), password.to_vec()));

            self.save_credentials(&credentials)
        }
        .boxed_local()
    }

    fn delete_credentials<'a>(
        &'a self,
        url: &'a str,
        _cx: &'a AsyncApp,
    ) -> Pin<Box<dyn Future<Output = Result<()>> + 'a>> {
        async move {
            let mut credentials = self.load_credentials()?;
            credentials.remove(url);

            self.save_credentials(&credentials)
        }
        .boxed_local()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn keychain_keys_are_namespaced() {
        assert!(!branding::KEYCHAIN_PREFIX.is_empty());
        for url in [
            "https://zed.dev",
            "https://api.anthropic.com",
            "https://api.unaligned.pw",
        ] {
            let key = keychain_key(url);
            assert_ne!(key, url);
            assert_eq!(key, format!("{}{url}", branding::KEYCHAIN_PREFIX));
        }
    }

    #[test]
    fn release_builds_never_use_the_development_file() {
        for channel in ReleaseChannel::ALL.into_iter().map(Some).chain([None]) {
            for development_use_keychain in [false, true] {
                assert!(!use_development_provider(
                    false,
                    channel,
                    development_use_keychain
                ));
            }
        }
    }

    #[test]
    fn debug_builds_use_the_development_file_only_in_the_dev_channel() {
        assert!(use_development_provider(
            true,
            Some(ReleaseChannel::Dev),
            false
        ));
        assert!(!use_development_provider(
            true,
            Some(ReleaseChannel::Dev),
            true
        ));
        for channel in [
            Some(ReleaseChannel::Nightly),
            Some(ReleaseChannel::Preview),
            Some(ReleaseChannel::Stable),
            None,
        ] {
            assert!(!use_development_provider(true, channel, false));
        }
    }
}

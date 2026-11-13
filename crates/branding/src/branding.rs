//! The product's identity: its name, identifiers and addresses.
//!
//! Change the product name here and nowhere else. More exactly: the values
//! live in `crates/branding/brand.env`, next to this crate's manifest, and
//! every other crate reads them through the constants below. Nothing else in
//! the code base may spell the product's name, its identifiers or its
//! addresses as a literal.
//!
//! The constants are `&'static str` built with `env!`, so they can be used in
//! `const` contexts (`const TITLE: &str = branding::PRODUCT_NAME;`) and with
//! compile-time concatenation such as `const_format::concatcp!`.
//!
//! This crate has no dependencies on purpose: any GPL crate of the workspace
//! can depend on it, including the lowest ones (`paths`, `release_channel`).

/// Declares one constant per key of `brand.env` and the list of all of them.
macro_rules! brand {
    ($($(#[$meta:meta])* $name:ident;)+) => {
        $(
            $(#[$meta])*
            pub const $name: &str = env!(concat!("BRAND_", stringify!($name)));
        )+

        /// Every brand constant as `(name, value)`, in declaration order.
        /// For diagnostics and for tests that guard the values.
        pub const ALL: &[(&str, &str)] = &[$((stringify!($name), $name)),+];
    };
}

brand! {
    /// The product's name as shown to the user: window titles, About, menus.
    PRODUCT_NAME;
    /// A shorter name for places where the full one does not fit.
    PRODUCT_SHORT_NAME;
    /// The identifier that names every directory, log file, socket, pipe and
    /// mutex of the app. The same on every platform, lowercase, never "zed".
    APP_ID;
    /// The reverse-DNS identifier: macOS bundle id, Linux desktop file and
    /// Wayland app id, Windows AppUserModelID. Release channels other than
    /// stable append a suffix (see `release_channel::ReleaseChannel::app_id`).
    REVERSE_DNS_ID;
    /// The scheme of the app's own links (without `://`).
    URL_SCHEME;
    /// The folder inside a project that holds its own settings, tasks and
    /// debug scenarios, with its leading dot.
    PROJECT_DIR;
    /// Who publishes the app: installer metadata, copyright lines.
    PUBLISHER;
    /// The identifier of the Windows installation (Inno Setup's `AppId`).
    /// Read by the packaging script; never change it after a release.
    WINDOWS_INSTALLER_GUID;
    /// The product's website.
    WEBSITE_URL;
    /// The product's documentation.
    DOCS_URL;
    /// Where a user writes to the people behind the product (the site's
    /// contact page). Bug reports go to [`issues_url`] once a tracker exists.
    SUPPORT_URL;
    /// Where a user creates an API key for the featured provider.
    PROVIDER_KEYS_URL;
    /// Where a user tops up the balance of the featured provider.
    PROVIDER_BILLING_URL;
    /// The base address of the featured provider's API.
    PROVIDER_API_URL;
    /// The address of the source code. Empty while it is not published: use
    /// [`source_repository_url`].
    SOURCE_REPOSITORY_URL;
    /// The `owner/repo` slug releases are downloaded from. Empty while nothing
    /// is published: use [`releases_repository`].
    RELEASES_REPOSITORY;
    /// Where users report problems. Empty while there is no tracker: use
    /// [`issues_url`].
    ISSUES_URL;
    /// The public keys an update must be signed with before the application
    /// installs it: Ed25519 keys in base64, separated by commas. Empty: the
    /// application announces new versions and installs none.
    UPDATE_PUBLIC_KEYS;
    /// The address of the product's OAuth client metadata document. Empty
    /// while the product's site publishes none: use
    /// [`oauth_client_metadata_url`].
    OAUTH_CLIENT_METADATA_URL;
    /// The base address of the extension registry.
    EXTENSIONS_API_URL;
    /// The product token of the HTTP `User-Agent` header (no spaces).
    USER_AGENT_PRODUCT;
    /// Prepended to the name of every secret the app keeps in the system
    /// keychain, so that it never reads or replaces another application's.
    KEYCHAIN_PREFIX;
    /// The name of the project this is a fork of.
    UPSTREAM_NAME;
    /// The upstream version the fork is based on.
    UPSTREAM_VERSION;
    /// The credit line shown in About and in the licences page.
    UPSTREAM_CREDIT;
}

/// The `owner/repo` slug releases are downloaded from, or `None` while nothing
/// is published.
pub fn releases_repository() -> Option<&'static str> {
    non_empty(RELEASES_REPOSITORY)
}

/// The address of the source code, or `None` while it is not published.
pub fn source_repository_url() -> Option<&'static str> {
    non_empty(SOURCE_REPOSITORY_URL)
}

/// Where users report problems, or `None` while there is no tracker.
pub fn issues_url() -> Option<&'static str> {
    non_empty(ISSUES_URL)
}

/// The address of the product's OAuth client metadata document, or `None`
/// while the product's site publishes none.
pub fn oauth_client_metadata_url() -> Option<&'static str> {
    non_empty(OAUTH_CLIENT_METADATA_URL)
}

fn non_empty(value: &'static str) -> Option<&'static str> {
    if value.is_empty() { None } else { Some(value) }
}

// The app must never share anything with a real Zed installed on the same
// machine. Everything that is stored or opened on the machine derives from
// these two values, so they are checked here, at compile time.
const _: () = {
    assert!(!is_zed(APP_ID), "brand.env: APP_ID must not be \"zed\"");
    assert!(!APP_ID.is_empty(), "brand.env: APP_ID must not be empty");
    assert!(
        !KEYCHAIN_PREFIX.is_empty(),
        "brand.env: KEYCHAIN_PREFIX must not be empty"
    );
    assert!(
        !PRODUCT_NAME.is_empty(),
        "brand.env: PRODUCT_NAME must not be empty"
    );
};

const fn is_zed(value: &str) -> bool {
    let bytes = value.as_bytes();
    bytes.len() == 3
        && bytes[0].eq_ignore_ascii_case(&b'z')
        && bytes[1].eq_ignore_ascii_case(&b'e')
        && bytes[2].eq_ignore_ascii_case(&b'd')
}

#[cfg(test)]
mod tests {
    use super::*;

    /// `brand.env` and the list of constants must match: a key present in the
    /// file but not declared here would be silently ignored.
    #[test]
    fn brand_env_and_constants_match() {
        let file = include_str!("../brand.env");
        let keys_in_file: Vec<&str> = file
            .lines()
            .map(str::trim)
            .filter(|line| !line.is_empty() && !line.starts_with('#'))
            .filter_map(|line| line.split_once('='))
            .map(|(key, _)| key.trim())
            .collect();
        let mut declared: Vec<&str> = ALL.iter().map(|(name, _)| *name).collect();
        let mut in_file = keys_in_file.clone();
        declared.sort_unstable();
        in_file.sort_unstable();
        assert_eq!(declared, in_file);
    }

    #[test]
    fn empty_values_mean_none() {
        assert_eq!(
            releases_repository().is_none(),
            RELEASES_REPOSITORY.is_empty()
        );
        assert_eq!(
            source_repository_url().is_none(),
            SOURCE_REPOSITORY_URL.is_empty()
        );
        assert_eq!(issues_url().is_none(), ISSUES_URL.is_empty());
        assert_eq!(
            oauth_client_metadata_url().is_none(),
            OAUTH_CLIENT_METADATA_URL.is_empty()
        );
    }

    #[test]
    fn identifiers_are_usable_in_paths_and_headers() {
        for id in [APP_ID, URL_SCHEME, USER_AGENT_PRODUCT] {
            assert!(
                id.chars().all(|c| c.is_ascii_alphanumeric() || c == '-'),
                "{id:?} must be ASCII letters, digits and dashes"
            );
        }
        assert_eq!(APP_ID, APP_ID.to_lowercase());
        assert!(REVERSE_DNS_ID.contains('.'));
    }
}

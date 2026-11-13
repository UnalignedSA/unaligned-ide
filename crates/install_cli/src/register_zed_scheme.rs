use anyhow::Result;
use gpui::{AsyncApp, actions};

actions!(
    cli,
    [
        /// Makes this app the system's handler for its own links.
        RegisterUrlScheme
    ]
);

/// The one URL scheme the app registers with the system: the product's own
/// (`branding::URL_SCHEME`, without `://`).
///
/// It is never `zed`: that scheme belongs to a real Zed that may be installed
/// on the same machine, and registering it would send Zed's links here. Links
/// are parsed in the internal `ide://` form (see `cli::to_internal_url`), a
/// neutral identifier that is never registered with the system.
pub const REGISTERED_URL_SCHEME: &str = branding::URL_SCHEME;

/// Schemes of upstream's editor. The first is the one a real Zed registers;
/// the other two are internal to the app and never go through the system.
const UPSTREAM_URL_SCHEMES: [&str; 3] = ["zed", "zed-cli", "zed-dock-action"];

// A brand file that named upstream's scheme must not build.
const _: () = assert!(
    !is_upstream_scheme(REGISTERED_URL_SCHEME) && !REGISTERED_URL_SCHEME.is_empty(),
    "brand.env: URL_SCHEME must be the product's own, never upstream's `zed`"
);

/// Registers the app as the system's handler for [`REGISTERED_URL_SCHEME`].
/// (The name of the function and of its action are upstream's identifiers.)
pub async fn register_zed_scheme(cx: &AsyncApp) -> Result<()> {
    let scheme = checked_scheme(REGISTERED_URL_SCHEME)?;
    cx.update(|cx| cx.register_url_scheme(scheme)).await
}

/// The last check before the system call: refuses upstream's schemes whatever
/// the caller passes.
fn checked_scheme(scheme: &str) -> Result<&str> {
    anyhow::ensure!(
        !scheme.is_empty() && !is_upstream_scheme(scheme),
        "refusing to register the URL scheme {scheme:?}: it is not this product's"
    );
    Ok(scheme)
}

const fn is_upstream_scheme(scheme: &str) -> bool {
    let mut index = 0;
    while index < UPSTREAM_URL_SCHEMES.len() {
        if eq_ignore_ascii_case(scheme, UPSTREAM_URL_SCHEMES[index]) {
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_registered_scheme_is_the_products_own() {
        assert_eq!(REGISTERED_URL_SCHEME, branding::URL_SCHEME);
        assert_eq!(
            checked_scheme(REGISTERED_URL_SCHEME).unwrap(),
            branding::URL_SCHEME
        );
        assert!(!is_upstream_scheme(REGISTERED_URL_SCHEME));
    }

    #[test]
    fn upstream_schemes_are_refused() {
        for scheme in ["zed", "Zed", "ZED", "zed-cli", "zed-dock-action", ""] {
            assert!(checked_scheme(scheme).is_err(), "{scheme:?}");
        }
        // Only the exact names are upstream's.
        assert!(checked_scheme("zedlike").is_ok());
    }

    /// The crate has one place that talks to the system about URL schemes,
    /// and it goes through `checked_scheme` with the product's constant.
    #[test]
    fn there_is_one_registration_call_and_it_is_guarded() {
        let call = ["register_url", "_scheme("].concat();
        let sources = [
            include_str!("register_zed_scheme.rs"),
            include_str!("install_cli.rs"),
            #[cfg(not(target_os = "windows"))]
            include_str!("install_cli_binary.rs"),
        ];
        let calls: usize = sources
            .iter()
            .map(|source| source.matches(&call).count())
            .sum();
        assert_eq!(calls, 1);
        assert!(include_str!("register_zed_scheme.rs").contains(&[&call, "scheme))"].concat()));
        for source in sources {
            assert!(!source.contains(&["IDE_URL", "_SCHEME"].concat()));
        }
    }
}

//! Helper functions for the web pages the app links to.
//!
//! Upstream builds these from the configured server URL, so that they point
//! at Zed's site: its account pages, its terms, its documentation. The
//! product has no accounts, plans or trials, so every account-related helper
//! returns the product's website (`branding::WEBSITE_URL`). The function
//! names are upstream's because other crates call them.
//!
//! Upstream's helpers for pages of its manual (AI privacy, edit predictions,
//! skills, sandboxing, model providers) are gone: the product's documentation
//! (`branding::DOCS_URL`) is about the provider's API and has no such pages,
//! so the buttons that opened them were removed instead of opening a page that
//! does not exist.

use gpui::App;

fn website_url() -> String {
    branding::WEBSITE_URL.to_string()
}

/// Upstream: the account page. The product has no accounts: its website.
pub fn account_url(_cx: &App) -> String {
    website_url()
}

/// Upstream: the page that starts a trial. The product has no trials: its
/// website.
pub fn start_trial_url(_cx: &App) -> String {
    website_url()
}

/// Upstream: the page that upgrades a plan. The product has no plans: its
/// website.
pub fn upgrade_to_zed_pro_url(_cx: &App) -> String {
    website_url()
}

/// Upstream: the terms of the hosted service. The product has no hosted
/// service of its own: its website.
pub fn terms_of_service(_cx: &App) -> String {
    website_url()
}

/// An internal link to a shared agent thread, handled by the app itself.
pub fn shared_agent_thread_url(session_id: &str) -> String {
    format!("ide://agent/shared/{}", session_id)
}

#[cfg(test)]
mod tests {
    use super::*;
    use gpui::TestAppContext;

    #[gpui::test]
    fn test_no_helper_points_at_upstream(cx: &mut TestAppContext) {
        cx.update(|cx| {
            let website = branding::WEBSITE_URL;
            for url in [
                account_url(cx),
                start_trial_url(cx),
                upgrade_to_zed_pro_url(cx),
                terms_of_service(cx),
            ] {
                assert_eq!(url, website);
            }

            assert_eq!(shared_agent_thread_url("abc"), "ide://agent/shared/abc");
        });
    }
}

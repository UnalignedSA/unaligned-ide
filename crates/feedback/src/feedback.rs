use app_actions::feedback::{ContactSupport, FileBugReport, RequestFeature};
use client::telemetry;
use extension_host::ExtensionStore;
use gpui::{App, ClipboardItem, PromptLevel, actions};
use system_specs::{CopySystemSpecsIntoClipboard, SystemSpecs};
use util::ResultExt;
use workspace::Workspace;

actions!(
    app,
    [
        /// Opens the project's source repository, or the website while the
        /// source is not published.
        OpenSourceRepository,
        /// Copies installed extensions to the clipboard for bug reports.
        CopyInstalledExtensionsIntoClipboard
    ]
);

/// The form of the product's issue tracker for a new report, with what the
/// application knows already filled in: its version and the system. Nothing
/// is sent by opening it: the person reads the form, completes it and sends
/// it, or not. `None` while there is no tracker.
///
/// `template` is a file of `.github/ISSUE_TEMPLATE`, whose `version` and
/// `system` fields take the two values. Never upstream's tracker: a problem
/// of this fork is not theirs.
fn new_issue_url(template: &str, cx: &App) -> Option<String> {
    let issues = branding::issues_url()?;
    let version = format!(
        "{} ({})",
        release_channel::AppVersion::global(cx),
        release_channel::ReleaseChannel::global(cx).dev_name()
    );
    let system = format!(
        "{} {} ({})",
        telemetry::os_name(cx),
        telemetry::os_version(),
        std::env::consts::ARCH
    );
    issue_form_url(issues, template, &version, &system)
}

fn issue_form_url(issues: &str, template: &str, version: &str, system: &str) -> Option<String> {
    let url = url::Url::parse_with_params(
        &format!("{}/new", issues.trim_end_matches('/')),
        &[
            ("template", template),
            ("version", version),
            ("system", system),
        ],
    )
    .ok()?;
    Some(url.into())
}

/// Where a report goes: the form of the tracker, or the contact page of the
/// product's website while there is no tracker.
fn report_url(template: &str, cx: &App) -> String {
    new_issue_url(template, cx).unwrap_or_else(|| branding::SUPPORT_URL.to_string())
}

/// The product's source repository, or its website while the source is not
/// published.
fn repository_url() -> &'static str {
    branding::source_repository_url().unwrap_or(branding::WEBSITE_URL)
}

pub fn init(cx: &mut App) {
    cx.observe_new(|workspace: &mut Workspace, _, _| {
        workspace
            .register_action(|_, _: &CopySystemSpecsIntoClipboard, window, cx| {
                let os_name = telemetry::os_name(cx);
                let specs = SystemSpecs::new(window, cx, os_name, telemetry::os_version());

                cx.spawn_in(window, async move |_, cx| {
                    let specs = specs.await.to_string();

                    cx.update(|_, cx| {
                        cx.write_to_clipboard(ClipboardItem::new_string(specs.clone()))
                    })
                    .log_err();

                    cx.prompt(
                        PromptLevel::Info,
                        "Copied into clipboard",
                        Some(&specs),
                        &["OK"],
                    )
                    .await
                })
                .detach();
            })
            .register_action(|_, _: &CopyInstalledExtensionsIntoClipboard, window, cx| {
                let clipboard_text = format_installed_extensions_for_clipboard(cx);
                cx.write_to_clipboard(ClipboardItem::new_string(clipboard_text.clone()));
                drop(window.prompt(
                    PromptLevel::Info,
                    "Copied into clipboard",
                    Some(&clipboard_text),
                    &["OK"],
                    cx,
                ));
            })
            .register_action(|_, _: &RequestFeature, _, cx| {
                cx.open_url(&report_url("feature_request.yml", cx));
            })
            .register_action(|_, _: &FileBugReport, _, cx| {
                cx.open_url(&report_url("bug_report.yml", cx));
            })
            // The contact page, not the tracker: for what is private, and for
            // people without an account on the tracker.
            .register_action(|_, _: &ContactSupport, _, cx| {
                cx.open_url(branding::SUPPORT_URL);
            })
            .register_action(|_, _: &OpenSourceRepository, _, cx| {
                cx.open_url(repository_url());
            });
    })
    .detach();
}

fn format_installed_extensions_for_clipboard(cx: &mut App) -> String {
    let store = ExtensionStore::global(cx);
    let store = store.read(cx);
    let mut lines = Vec::with_capacity(store.extension_index.extensions.len());

    for (extension_id, entry) in store.extension_index.extensions.iter() {
        let line = format!(
            "- {} ({}) v{}{}",
            entry.manifest.name,
            extension_id,
            entry.manifest.version,
            if entry.dev { " (dev)" } else { "" }
        );
        lines.push(line);
    }

    lines.sort();

    if lines.is_empty() {
        return "No extensions installed.".to_string();
    }

    format!(
        "Installed extensions ({}):\n{}",
        lines.len(),
        lines.join("\n")
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_the_report_form_carries_the_version_and_the_system() {
        let url = issue_form_url(
            "https://github.com/example/ide/issues/",
            "bug_report.yml",
            "0.1.2 (stable)",
            "macOS 15.1 (aarch64)",
        )
        .unwrap();
        assert_eq!(
            url,
            "https://github.com/example/ide/issues/new?template=bug_report.yml&version=0.1.2+%28stable%29&system=macOS+15.1+%28aarch64%29"
        );
        // A tracker address that is not one gives no form.
        assert_eq!(
            issue_form_url("not an address", "bug_report.yml", "1", "2"),
            None
        );
    }
}

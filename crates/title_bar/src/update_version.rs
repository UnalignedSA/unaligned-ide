use std::sync::Arc;
use std::time::Duration;

use anyhow::anyhow;
use auto_update::{
    AutoUpdateEvent, AutoUpdateStatus, AutoUpdater, AvailableUpdate, UpdateCheckType,
};
use gpui::{Empty, Render, Task};
use semver::Version;
use ui::{UpdateButton, prelude::*};

const UP_TO_DATE_DISPLAY_DURATION: Duration = Duration::from_secs(4);

/// The title bar's word on updates: a check in progress, its failure, "up to
/// date" for a moment, or a quiet button for a newer release. A click on that
/// button installs the release when this copy can be updated in place, and
/// opens the release's page otherwise; the button then follows the download,
/// the installation, and offers the restart. Nothing is downloaded before
/// that click.
pub struct UpdateVersion {
    status: AutoUpdateStatus,
    available_update: Option<AvailableUpdate>,
    update_check_type: UpdateCheckType,
    dismissed_status: Option<AutoUpdateStatus>,
    showing_up_to_date: bool,
    _up_to_date_timer: Option<Task<()>>,
}

impl UpdateVersion {
    pub fn new(cx: &mut Context<Self>) -> Self {
        if let Some(auto_updater) = AutoUpdater::get(cx) {
            cx.observe(&auto_updater, |this, auto_update, cx| {
                let auto_update = auto_update.read(cx);
                this.status = auto_update.status();
                this.available_update = auto_update.available_update();
                this.update_check_type = auto_update.update_check_type();
                this.dismissed_status = auto_update.dismissed_status();
                if !matches!(this.status, AutoUpdateStatus::Idle) {
                    this.hide_up_to_date();
                }
                cx.notify();
            })
            .detach();
            cx.subscribe(&auto_updater, |this, _, event, cx| match event {
                AutoUpdateEvent::UpToDate => this.show_up_to_date(cx),
            })
            .detach();
            Self {
                status: auto_updater.read(cx).status(),
                available_update: auto_updater.read(cx).available_update(),
                update_check_type: UpdateCheckType::Automatic,
                dismissed_status: auto_updater.read(cx).dismissed_status(),
                showing_up_to_date: false,
                _up_to_date_timer: None,
            }
        } else {
            Self {
                status: AutoUpdateStatus::Idle,
                available_update: None,
                update_check_type: UpdateCheckType::Automatic,
                dismissed_status: None,
                showing_up_to_date: false,
                _up_to_date_timer: None,
            }
        }
    }

    fn show_up_to_date(&mut self, cx: &mut Context<Self>) {
        self.showing_up_to_date = true;
        self._up_to_date_timer = Some(cx.spawn(async move |this, cx| {
            cx.background_executor()
                .timer(UP_TO_DATE_DISPLAY_DURATION)
                .await;
            this.update(cx, |this, cx| {
                this.hide_up_to_date();
                cx.notify();
            })
            .ok();
        }));
    }

    fn hide_up_to_date(&mut self) {
        self.showing_up_to_date = false;
        self._up_to_date_timer = None;
    }

    /// Steps through the states the button can show, without any check or
    /// download: checking, update available, downloading, installing, restart
    /// to update, update failed, check failed, nothing.
    pub fn update_simulation(&mut self, cx: &mut Context<Self>) {
        let version = Version::new(1, 99, 0);
        let (status, available_update) = match (&self.status, &self.available_update) {
            (AutoUpdateStatus::Idle, None) => (AutoUpdateStatus::Checking, None),
            (AutoUpdateStatus::Checking, _) => (
                AutoUpdateStatus::Idle,
                Some(AvailableUpdate {
                    version,
                    url: branding::WEBSITE_URL.to_string(),
                    package: None,
                }),
            ),
            (AutoUpdateStatus::Idle, Some(_)) => (
                AutoUpdateStatus::Downloading {
                    version,
                    progress: Some(0.42),
                },
                None,
            ),
            (AutoUpdateStatus::Downloading { .. }, _) => {
                (AutoUpdateStatus::Installing { version }, None)
            }
            (AutoUpdateStatus::Installing { .. }, _) => {
                (AutoUpdateStatus::Updated { version }, None)
            }
            (AutoUpdateStatus::Updated { .. }, _) => (
                AutoUpdateStatus::InstallFailed {
                    error: Arc::new(anyhow!("The download was interrupted")),
                },
                None,
            ),
            (AutoUpdateStatus::InstallFailed { .. }, _) => (
                AutoUpdateStatus::Errored {
                    error: Arc::new(anyhow!("Network timeout")),
                },
                None,
            ),
            _ => (AutoUpdateStatus::Idle, None),
        };

        self.status = status;
        self.available_update = available_update;
        self.update_check_type = UpdateCheckType::Manual;
        self.dismissed_status = None;
        self.hide_up_to_date();
        cx.notify()
    }

    fn is_dismissed(&self) -> bool {
        self.dismissed_status.as_ref() == Some(&self.status)
    }

    fn dismiss(&mut self, cx: &mut Context<Self>) {
        self.dismissed_status = Some(self.status.clone());
        if let Some(auto_updater) = AutoUpdater::get(cx) {
            let status = self.status.clone();
            auto_updater.update(cx, |auto_updater, cx| {
                auto_updater.dismiss_status(status, cx)
            });
        }
        cx.notify()
    }

    /// Puts a failed installation away: the updater is idle again, and the
    /// release it could not install is offered as before.
    fn dismiss_failure(&mut self, cx: &mut Context<Self>) {
        if let Some(auto_updater) = AutoUpdater::get(cx) {
            auto_updater.update(cx, |auto_updater, cx| {
                auto_updater.dismiss(cx);
            });
        }
        self.status = AutoUpdateStatus::Idle;
        cx.notify()
    }

    fn dismiss_available_update(&mut self, cx: &mut Context<Self>) {
        self.available_update = None;
        if let Some(auto_updater) = AutoUpdater::get(cx) {
            auto_updater.update(cx, |auto_updater, cx| {
                auto_updater.dismiss_available_update(cx)
            });
        }
        cx.notify()
    }

    fn up_to_date_message() -> String {
        format!("{} Is Up to Date", branding::PRODUCT_NAME)
    }

    fn available_tooltip_message(update: &AvailableUpdate) -> String {
        let action = if update.can_be_installed() {
            "Click to download and install it."
        } else {
            "Open its release page."
        };
        format!(
            "{} {} is available. {action}",
            branding::PRODUCT_NAME,
            update.version
        )
    }
}

impl Render for UpdateVersion {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        if self.showing_up_to_date {
            return UpdateButton::new(IconName::Check, Self::up_to_date_message())
                .disabled(true)
                .into_any_element();
        }
        match &self.status {
            AutoUpdateStatus::Checking if self.update_check_type.is_manual() => {
                return UpdateButton::new(IconName::LoadCircle, "Checking for Updates…")
                    .icon_animate(true)
                    .disabled(true)
                    .into_any_element();
            }
            AutoUpdateStatus::Errored { error } if !self.is_dismissed() => {
                return UpdateButton::new(IconName::Warning, "Update Check Failed")
                    .icon_color(Color::Warning)
                    .tooltip(error.to_string())
                    .with_dismiss()
                    .on_click(|_, window, cx| {
                        window.dispatch_action(Box::new(workspace::OpenLog), cx);
                    })
                    .on_dismiss(cx.listener(|this, _, _window, cx| this.dismiss(cx)))
                    .into_any_element();
            }
            AutoUpdateStatus::Downloading { version, progress } => {
                return UpdateButton::downloading(*progress)
                    .tooltip(UpdateButton::downloading_tooltip_message(
                        version, *progress,
                    ))
                    .into_any_element();
            }
            AutoUpdateStatus::Installing { version } => {
                return UpdateButton::installing(UpdateButton::version_tooltip_message(version))
                    .into_any_element();
            }
            AutoUpdateStatus::Updated { version } if !self.is_dismissed() => {
                return UpdateButton::updated(UpdateButton::version_tooltip_message(version))
                    .on_click(|_, _, cx| workspace::reload(cx))
                    .on_dismiss(cx.listener(|this, _, _window, cx| this.dismiss(cx)))
                    .into_any_element();
            }
            // Nothing was replaced. The release's page is the other way to
            // the new version; dismissing brings "Update Available" back for
            // another try.
            AutoUpdateStatus::InstallFailed { error } if !self.is_dismissed() => {
                let url = self
                    .available_update
                    .as_ref()
                    .map(|update| update.url.clone());
                let tooltip = match &url {
                    Some(_) => format!("{error:#}. Click to open the release page."),
                    None => format!("{error:#}"),
                };
                return UpdateButton::errored(tooltip)
                    .on_click(move |_, window, cx| match &url {
                        Some(url) => cx.open_url(url),
                        None => window.dispatch_action(Box::new(workspace::OpenLog), cx),
                    })
                    .on_dismiss(cx.listener(|this, _, _window, cx| this.dismiss_failure(cx)))
                    .into_any_element();
            }
            // A restart that was put off: the button stays out of the way.
            AutoUpdateStatus::Updated { .. } => return Empty.into_any_element(),
            _ => {}
        }

        // The terminal state of a check that found a newer release: an
        // outlined button, like every other state here. It installs the
        // release when this copy can be updated in place, and opens the
        // release's page otherwise. The accent is not used.
        let Some(update) = &self.available_update else {
            return Empty.into_any_element();
        };
        let button = if update.can_be_installed() {
            UpdateButton::new(IconName::Download, "Update Available").on_click(|_, _, cx| {
                if let Some(auto_updater) = AutoUpdater::get(cx) {
                    auto_updater.update(cx, |auto_updater, cx| auto_updater.install_update(cx));
                }
            })
        } else {
            let url = update.url.clone();
            UpdateButton::new(IconName::ArrowUpRight, "Update Available")
                .on_click(move |_, _, cx| cx.open_url(&url))
        };
        button
            .tooltip(Self::available_tooltip_message(update))
            .with_dismiss()
            .on_dismiss(cx.listener(|this, _, _window, cx| this.dismiss_available_update(cx)))
            .into_any_element()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_messages_name_the_product() {
        assert_eq!(
            UpdateVersion::up_to_date_message(),
            format!("{} Is Up to Date", branding::PRODUCT_NAME)
        );

        let update = AvailableUpdate {
            version: Version::new(1, 2, 3),
            url: "https://example.com/releases/tag/v1.2.3".to_string(),
            package: None,
        };
        assert_eq!(
            UpdateVersion::available_tooltip_message(&update),
            format!(
                "{} 1.2.3 is available. Open its release page.",
                branding::PRODUCT_NAME
            )
        );
    }
}

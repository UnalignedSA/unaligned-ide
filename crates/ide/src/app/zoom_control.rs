//! The zoom of the code: the `- 100% +` buttons of the status bar, and the
//! saving of the zoom.
//!
//! The zoom is the editor's font size. 100% is the size of the default
//! settings, so the percentage also tells a size somebody wrote in their
//! settings file.
//!
//! A zoom step is applied at once, in memory, by the zoom actions
//! (`app::IncreaseBufferFontSize` and its siblings) and by the buttons. A
//! moment after the last step the size is written to the user's settings
//! (`buffer_font_size`), so it is still there after a restart, as in VS Code.
//! Writing each step straight to the file would make a step wait for the file
//! to be written and read back, and would lose steps of a held key.

use std::{sync::Arc, time::Duration};

use app_actions::{DecreaseBufferFontSize, IncreaseBufferFontSize, ResetBufferFontSize};
use fs::Fs;
use gpui::{
    App, AppContext as _, Context, Div, Entity, Global, IntoElement, ParentElement, Pixels, Render,
    Styled, Subscription, Task, Window,
};
use settings::{Settings as _, SettingsStore, update_settings_file};
use theme_settings::ThemeSettings;
use ui::{Tooltip, prelude::*};
use workspace::{
    HideStatusItem, StatusBarSettings, StatusItemView,
    active_file_name::{status_bar_item, status_bar_text},
    item::ItemHandle,
};

/// How long after the last zoom step the size is written to the settings.
const SAVE_DELAY: Duration = Duration::from_millis(400);

const RESET_NOTE: &str = "The zoom of the code is kept after a restart.";

/// What to do after the editor's font size changed.
#[derive(Debug, PartialEq)]
enum Step {
    /// Write the size on screen to the settings in a moment.
    SaveSoon,
    /// Nothing is left to write.
    Forget,
    /// Put this size back on screen: see [`Unsaved::font_size_changed`].
    Restore(Pixels),
}

/// The zoom that is on screen and not yet in the settings.
#[derive(Default)]
struct Unsaved {
    /// The size on screen, while it differs from the settings.
    wanted: Option<Pixels>,
    /// The size last written to the settings, until the settings show it.
    written: Option<Pixels>,
}

impl Unsaved {
    /// `adjusted`: a size is set in memory, over the settings. `shown`: the
    /// size on screen. `saved`: the size of the settings.
    ///
    /// The application drops the size set in memory whenever the size of the
    /// settings changes. When that change is our own write coming back while
    /// a later step was already on screen, the later step is put back (and
    /// written in its turn): without this, a click that falls between a write
    /// and its echo is undone under the user's pointer.
    fn font_size_changed(&mut self, adjusted: bool, shown: Pixels, saved: Pixels) -> Step {
        if adjusted {
            if shown == saved {
                self.wanted = None;
                return Step::Forget;
            }
            self.wanted = Some(shown);
            return Step::SaveSoon;
        }

        let own_write_came_back = self.written.take() == Some(saved);
        match self.wanted.take() {
            Some(wanted) if own_write_came_back && wanted != saved => Step::Restore(wanted),
            _ => Step::Forget,
        }
    }

    /// The size to write now, if any.
    fn write(&mut self) -> Option<Pixels> {
        self.written = self.wanted;
        self.wanted
    }
}

/// Writes the zoom to the settings a moment after it last changed. One for
/// the application: the zoom is the same in every window.
struct ZoomSaver {
    fs: Arc<dyn Fs>,
    unsaved: Unsaved,
    save: Option<Task<()>>,
    _subscription: Subscription,
}

struct GlobalZoomSaver(#[allow(dead_code)] Entity<ZoomSaver>);

impl Global for GlobalZoomSaver {}

/// Starts keeping the zoom across restarts.
pub fn init(fs: Arc<dyn Fs>, cx: &mut App) {
    let saver = cx.new(|cx| ZoomSaver {
        fs,
        unsaved: Unsaved::default(),
        save: None,
        _subscription: theme_settings::observe_buffer_font_size_adjustment(
            cx,
            ZoomSaver::font_size_changed,
        ),
    });
    cx.set_global(GlobalZoomSaver(saver));
}

impl ZoomSaver {
    fn font_size_changed(&mut self, cx: &mut Context<Self>) {
        let settings = ThemeSettings::get_global(cx);
        let shown = settings.buffer_font_size(cx);
        let saved = theme_settings::clamp_font_size(settings.buffer_font_size_settings());
        let adjusted = theme_settings::buffer_font_size_is_adjusted(cx);

        match self.unsaved.font_size_changed(adjusted, shown, saved) {
            Step::SaveSoon => {
                self.save = Some(cx.spawn(async move |this, cx| {
                    cx.background_executor().timer(SAVE_DELAY).await;
                    this.update(cx, |this, cx| this.save_now(cx)).ok();
                }));
            }
            Step::Forget => self.save = None,
            Step::Restore(size) => {
                self.save = None;
                // Not from inside the notification of the very value it sets.
                cx.defer(move |cx| theme_settings::adjust_buffer_font_size(cx, |_| size));
            }
        }
    }

    fn save_now(&mut self, cx: &mut Context<Self>) {
        self.save = None;
        let Some(size) = self.unsaved.write() else {
            return;
        };
        update_settings_file(self.fs.clone(), cx, move |settings, _| {
            settings.theme.buffer_font_size = Some(f32::from(size).into());
        });
    }
}

/// Back to 100%: drops the zoom on screen and the size kept in the user's
/// settings, which leaves the size of the default settings.
pub fn reset(fs: Arc<dyn Fs>, cx: &mut App) {
    theme_settings::reset_buffer_font_size(cx);
    update_settings_file(fs, cx, |settings, _| {
        settings.theme.buffer_font_size = None;
    });
}

/// The zoom in percent of the default size.
fn zoom_percent(cx: &App) -> u32 {
    let shown = f32::from(ThemeSettings::get_global(cx).buffer_font_size(cx));
    let normal = cx
        .global::<SettingsStore>()
        .raw_default_settings()
        .theme
        .buffer_font_size
        .map_or(shown, |size| size.0);
    percent(shown, normal)
}

fn percent(shown: f32, normal: f32) -> u32 {
    if normal <= 0. {
        return 100;
    }
    (shown / normal * 100.).round() as u32
}

/// The zoom buttons of the status bar: zoom out, the zoom in percent (a click
/// goes back to 100%), zoom in. They act on the code whatever has the focus;
/// the keys act on what has the focus (the chat has a font size of its own).
pub struct ZoomControl {
    fs: Arc<dyn Fs>,
    _subscriptions: [Subscription; 2],
}

impl ZoomControl {
    pub fn new(fs: Arc<dyn Fs>, cx: &mut Context<Self>) -> Self {
        Self {
            fs,
            _subscriptions: [
                theme_settings::observe_buffer_font_size_adjustment(cx, |_, cx| cx.notify()),
                cx.observe_global::<SettingsStore>(|_, cx| cx.notify()),
            ],
        }
    }
}

/// The inside of one of the two icon buttons.
fn zoom_icon(icon: IconName) -> Div {
    h_flex()
        .h_full()
        .px_1()
        .child(Icon::new(icon).size(IconSize::XSmall).color(Color::Muted))
}

impl Render for ZoomControl {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        if !StatusBarSettings::get_global(cx).zoom_buttons {
            return div().hidden();
        }

        let percent = zoom_percent(cx);
        let percent_text = format!("{percent}%");
        let fs = self.fs.clone();

        h_flex()
            .child(
                status_bar_item("zoom-out", cx)
                    .tab_index(0isize)
                    .aria_label("Zoom out")
                    .child(zoom_icon(IconName::Dash))
                    .tooltip(|_window, cx| {
                        Tooltip::for_action(
                            "Zoom Out",
                            &DecreaseBufferFontSize { persist: false },
                            cx,
                        )
                    })
                    .on_click(|_, _, cx| theme_settings::decrease_buffer_font_size(cx)),
            )
            .child(
                status_bar_item("zoom-reset", cx)
                    .tab_index(0isize)
                    .aria_label(format!("Zoom {percent} percent, reset"))
                    .child(
                        h_flex()
                            .h_full()
                            // As wide as `100%`, so the buttons do not move
                            // while the number changes.
                            .min_w(rems_from_px(32_f32))
                            .justify_center()
                            .child(status_bar_text(percent_text, cx)),
                    )
                    .tooltip(|_window, cx| {
                        Tooltip::with_meta(
                            "Reset Zoom",
                            Some(&ResetBufferFontSize { persist: false }),
                            RESET_NOTE,
                            cx,
                        )
                    })
                    .on_click(move |_, _, cx| reset(fs.clone(), cx)),
            )
            .child(
                status_bar_item("zoom-in", cx)
                    .tab_index(0isize)
                    .aria_label("Zoom in")
                    .child(zoom_icon(IconName::Plus))
                    .tooltip(|_window, cx| {
                        Tooltip::for_action(
                            "Zoom In",
                            &IncreaseBufferFontSize { persist: false },
                            cx,
                        )
                    })
                    .on_click(|_, _, cx| theme_settings::increase_buffer_font_size(cx)),
            )
    }
}

impl StatusItemView for ZoomControl {
    fn set_active_pane_item(
        &mut self,
        _active_pane_item: Option<&dyn ItemHandle>,
        _window: &mut Window,
        _cx: &mut Context<Self>,
    ) {
        // The zoom is the application's, not the active item's.
    }

    fn hide_setting(&self, _: &App) -> Option<HideStatusItem> {
        Some(HideStatusItem::new(|settings| {
            settings.status_bar.get_or_insert_default().zoom_buttons = Some(false);
        }))
    }
}

#[cfg(test)]
mod tests {
    use gpui::px;

    use super::*;

    #[test]
    fn test_percent_of_the_default_size() {
        assert_eq!(percent(15., 15.), 100);
        assert_eq!(percent(16., 15.), 107);
        assert_eq!(percent(12., 15.), 80);
        assert_eq!(percent(30., 15.), 200);
        assert_eq!(percent(15., 0.), 100);
    }

    #[test]
    fn test_a_step_is_written_once_and_forgotten_when_the_settings_show_it() {
        let mut unsaved = Unsaved::default();
        // A step: 16 on screen, 15 in the settings.
        assert_eq!(
            unsaved.font_size_changed(true, px(16.), px(15.)),
            Step::SaveSoon
        );
        assert_eq!(unsaved.write(), Some(px(16.)));
        // The settings now say 16, and the size set in memory was dropped.
        assert_eq!(
            unsaved.font_size_changed(false, px(16.), px(16.)),
            Step::Forget
        );
        assert_eq!(unsaved.write(), None);
    }

    #[test]
    fn test_a_step_made_while_a_write_comes_back_is_put_back() {
        let mut unsaved = Unsaved::default();
        assert_eq!(
            unsaved.font_size_changed(true, px(16.), px(15.)),
            Step::SaveSoon
        );
        assert_eq!(unsaved.write(), Some(px(16.)));
        // A second step before the file was read back.
        assert_eq!(
            unsaved.font_size_changed(true, px(17.), px(15.)),
            Step::SaveSoon
        );
        // The write of 16 comes back and the application drops the 17.
        assert_eq!(
            unsaved.font_size_changed(false, px(16.), px(16.)),
            Step::Restore(px(17.))
        );
        // Putting it back is a step like any other.
        assert_eq!(
            unsaved.font_size_changed(true, px(17.), px(16.)),
            Step::SaveSoon
        );
        assert_eq!(unsaved.write(), Some(px(17.)));
        assert_eq!(
            unsaved.font_size_changed(false, px(17.), px(17.)),
            Step::Forget
        );
    }

    #[test]
    fn test_a_reset_or_a_hand_edit_is_not_fought() {
        // A reset before the write: nothing is written.
        let mut unsaved = Unsaved::default();
        assert_eq!(
            unsaved.font_size_changed(true, px(16.), px(15.)),
            Step::SaveSoon
        );
        assert_eq!(
            unsaved.font_size_changed(false, px(15.), px(15.)),
            Step::Forget
        );
        assert_eq!(unsaved.write(), None);

        // The settings file edited by hand while a step waits: the file wins.
        let mut unsaved = Unsaved::default();
        assert_eq!(
            unsaved.font_size_changed(true, px(16.), px(15.)),
            Step::SaveSoon
        );
        assert_eq!(
            unsaved.font_size_changed(false, px(20.), px(20.)),
            Step::Forget
        );
        assert_eq!(unsaved.write(), None);

        // Back to the saved size by steps: nothing to write.
        let mut unsaved = Unsaved::default();
        assert_eq!(
            unsaved.font_size_changed(true, px(16.), px(15.)),
            Step::SaveSoon
        );
        assert_eq!(
            unsaved.font_size_changed(true, px(15.), px(15.)),
            Step::Forget
        );
        assert_eq!(unsaved.write(), None);
    }
}

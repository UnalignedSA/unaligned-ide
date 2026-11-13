//! The heights and widths of the window chrome, in one place.
//!
//! Every value is in rems, so it follows the UI font size, and steps with the
//! [`UiDensity`] setting. The pixel values in the docs are the ones at the
//! default density and a 16px UI font.
//!
//! ```
//! use ui::{metrics, prelude::*};
//!
//! fn header(cx: &App) -> Div {
//!     h_flex().h(metrics::panel_header_height(cx)).px_3()
//! }
//! ```

use gpui::{App, Pixels, Rems, Window};
use theme::UiDensity;

use crate::{DynamicSpacing, rems_from_px};

/// The height of an input, see [`input_height`].
#[derive(Debug, Default, PartialEq, Eq, PartialOrd, Ord, Hash, Clone, Copy)]
pub enum InputSize {
    /// 26px: inputs in toolbars and search rows.
    Small,
    /// 28px: inputs in forms and settings.
    #[default]
    Medium,
    /// 32px: the one prominent input of a view.
    Large,
}

fn by_density(cx: &App, compact: f32, default: f32, comfortable: f32) -> Rems {
    let px = match theme::theme_settings(cx).ui_density(cx) {
        UiDensity::Compact => compact,
        UiDensity::Default => default,
        UiDensity::Comfortable => comfortable,
    };
    rems_from_px(px)
}

/// The height of a panel header and of a tab bar: 35px (31 compact, 39 comfortable).
///
/// [`Tab::container_height`](crate::Tab::container_height) is this value in
/// pixels, so the header of a side panel and the editor tab bar next to it
/// always share their bottom edge.
pub fn panel_header_height(cx: &App) -> Rems {
    // The tab row itself, plus the 2px marker and the 1px line under it.
    Rems(DynamicSpacing::Base32.rems(cx).0 + rems_from_px(3_f32).0)
}

/// The height of a second header row inside a panel: 28px (24 compact, 32 comfortable).
pub fn sub_header_height(cx: &App) -> Rems {
    by_density(cx, 24., 28., 32.)
}

/// The height of a row in a list or a tree: 24px (22 compact, 28 comfortable).
pub fn row_height(cx: &App) -> Rems {
    by_density(cx, 22., 24., 28.)
}

/// The height of the status bar: 24px (22 compact, 28 comfortable).
pub fn status_bar_height(cx: &App) -> Rems {
    by_density(cx, 22., 24., 28.)
}

/// The width of the activity bar, and the side of one of its square buttons:
/// 44px (40 compact, 48 comfortable).
pub fn activity_bar_width(cx: &App) -> Rems {
    by_density(cx, 40., 44., 48.)
}

/// The height of an input: 26, 28 or 32px.
///
/// An input states its size, so it does not step with the density: a small
/// input stays as tall as the [`ButtonSize::Medium`](crate::ButtonSize::Medium)
/// button next to it.
pub fn input_height(size: InputSize) -> Rems {
    rems_from_px(match size {
        InputSize::Small => 26_f32,
        InputSize::Medium => 28_f32,
        InputSize::Large => 32_f32,
    })
}

/// The side of the square a 16px icon button responds in: 24px (22 compact, 28 comfortable).
pub fn icon_hit_area(cx: &App) -> Rems {
    by_density(cx, 22., 24., 28.)
}

/// The height of the title bar, in pixels: it is the one height the platform has a
/// say in (a fixed 32px on Windows, 1.75rem with a floor of 34px elsewhere), so it
/// takes the window and does not follow the density.
pub fn title_bar_height(window: &Window) -> Pixels {
    crate::utils::platform_title_bar_height(window)
}

#[cfg(test)]
mod tests {
    use gpui::{TestAppContext, px};

    use super::*;
    use crate::Tab;

    fn init_test(cx: &mut TestAppContext) {
        cx.update(|cx| {
            let settings_store = settings::SettingsStore::test(cx);
            cx.set_global(settings_store);
            theme_settings::init(theme::LoadThemes::JustBase, cx);
        });
    }

    #[gpui::test]
    fn default_density_matches_the_design_values(cx: &mut TestAppContext) {
        init_test(cx);
        cx.update(|cx| {
            assert_eq!(panel_header_height(cx), rems_from_px(35_f32));
            assert_eq!(sub_header_height(cx), rems_from_px(28_f32));
            assert_eq!(row_height(cx), rems_from_px(24_f32));
            assert_eq!(status_bar_height(cx), rems_from_px(24_f32));
            assert_eq!(activity_bar_width(cx), rems_from_px(44_f32));
            assert_eq!(icon_hit_area(cx), rems_from_px(24_f32));
            assert_eq!(input_height(InputSize::Small), rems_from_px(26_f32));
            assert_eq!(input_height(InputSize::Medium), rems_from_px(28_f32));
            assert_eq!(input_height(InputSize::Large), rems_from_px(32_f32));
        });
    }

    #[gpui::test]
    fn tab_bar_and_panel_header_share_one_height(cx: &mut TestAppContext) {
        init_test(cx);
        cx.update(|cx| {
            let rem_size = theme::theme_settings(cx).ui_font_size(cx);
            assert_eq!(
                Tab::container_height(cx),
                panel_header_height(cx).to_pixels(rem_size)
            );
            assert_eq!(
                Tab::container_height(cx),
                px(35.) * (f32::from(rem_size) / 16.)
            );
        });
    }
}

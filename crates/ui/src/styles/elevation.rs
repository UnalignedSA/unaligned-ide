use std::fmt::{self, Display, Formatter};

use gpui::{App, BoxShadow, Hsla, hsla, px};
use theme::{ActiveTheme, Appearance};

/// Today, elevation is primarily used to add shadows to elements, and set the correct background for elements like buttons.
///
/// Elevation can be thought of as the physical closeness of an element to the
/// user. Elements with lower elevations are physically further away on the
/// z-axis and appear to be underneath elements with higher elevations.
///
/// In the future, a more complete approach to elevation may be added.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ElevationIndex {
    /// On the layer of the app background. This is under panels, panes, and
    /// other surfaces.
    Background,
    /// The primary surface – Contains panels, panes, containers, etc.
    Surface,
    /// The same elevation as the primary surface, but used for the editable areas, like buffers
    EditorSurface,
    /// A surface that is elevated above the primary surface. but below washes, models, and dragged elements.
    ElevatedSurface,
    /// A surface above the [ElevationIndex::ElevatedSurface] that is used for dialogs, alerts, modals, etc.
    ModalSurface,
}

impl Display for ElevationIndex {
    fn fmt(&self, f: &mut Formatter) -> fmt::Result {
        match self {
            ElevationIndex::Background => write!(f, "Background"),
            ElevationIndex::Surface => write!(f, "Surface"),
            ElevationIndex::EditorSurface => write!(f, "Editor Surface"),
            ElevationIndex::ElevatedSurface => write!(f, "Elevated Surface"),
            ElevationIndex::ModalSurface => write!(f, "Modal Surface"),
        }
    }
}

impl ElevationIndex {
    /// Returns an appropriate shadow for the given elevation index.
    ///
    /// The design language is flat: surfaces are separated by 1px lines, so
    /// only modal surfaces, which float over unrelated content, keep a single
    /// neutral shadow.
    pub fn shadow(self, cx: &App) -> Vec<BoxShadow> {
        let is_light = cx.theme().appearance() == Appearance::Light;

        match self {
            ElevationIndex::ModalSurface => vec![
                BoxShadow::new(
                    px(0.),
                    px(8.),
                    hsla(0., 0., 0., if is_light { 0.12 } else { 0.36 }),
                )
                .blur_radius(px(24.)),
            ],

            _ => vec![],
        }
    }

    /// Returns the background color for the given elevation index.
    pub fn bg(&self, cx: &mut App) -> Hsla {
        match self {
            ElevationIndex::Background => cx.theme().colors().background,
            ElevationIndex::Surface => cx.theme().colors().surface_background,
            ElevationIndex::EditorSurface => cx.theme().colors().editor_background,
            ElevationIndex::ElevatedSurface => cx.theme().colors().elevated_surface_background,
            ElevationIndex::ModalSurface => cx.theme().colors().elevated_surface_background,
        }
    }

    /// Returns a color that is appropriate a filled element on this elevation
    pub fn on_elevation_bg(&self, cx: &App) -> Hsla {
        match self {
            ElevationIndex::Background => cx.theme().colors().surface_background,
            ElevationIndex::Surface => cx.theme().colors().background,
            ElevationIndex::EditorSurface => cx.theme().colors().surface_background,
            ElevationIndex::ElevatedSurface => cx.theme().colors().background,
            ElevationIndex::ModalSurface => cx.theme().colors().background,
        }
    }

    /// Attempts to return a darker background color than the current elevation index's background.
    ///
    /// If the current background color is already dark, it will return a lighter color instead.
    pub fn darker_bg(&self, cx: &App) -> Hsla {
        match self {
            ElevationIndex::Background => cx.theme().colors().surface_background,
            ElevationIndex::Surface => cx.theme().colors().editor_background,
            ElevationIndex::EditorSurface => cx.theme().colors().surface_background,
            ElevationIndex::ElevatedSurface => cx.theme().colors().editor_background,
            ElevationIndex::ModalSurface => cx.theme().colors().editor_background,
        }
    }
}

/// The backdrop drawn over the window behind a modal: the theme's background,
/// see-through enough to keep the context visible.
pub fn scrim(cx: &App) -> Hsla {
    let opacity = match cx.theme().appearance() {
        Appearance::Light => 0.4,
        Appearance::Dark => 0.6,
    };
    cx.theme().colors().background.opacity(opacity)
}

/// The ring drawn on keyboard focus: 2px, inset so that scrolling and
/// clipping parents can never cut it off.
pub fn focus_ring(cx: &App) -> Vec<BoxShadow> {
    vec![
        BoxShadow::new(px(0.), px(0.), cx.theme().colors().border_focused)
            .spread_radius(px(2.))
            .inset(),
    ]
}

/// The fill of the one primary action of a view (the theme's `accent.background`).
pub fn accent_fill(cx: &App) -> Hsla {
    cx.theme().colors().accent_background
}

/// The fill of the primary action under the pointer (the theme's `accent.hover`).
pub fn accent_fill_hover(cx: &App) -> Hsla {
    cx.theme().colors().accent_hover
}

/// Text and icons sitting on [`accent_fill`] (the theme's `accent.foreground`).
/// A theme that does not set it gets black or white, whichever reads better
/// on its accent.
pub fn on_accent(cx: &App) -> Hsla {
    cx.theme().colors().accent_foreground
}

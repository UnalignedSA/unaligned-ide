#![allow(missing_docs)]

use crate::schema::{status_colors_refinement, syntax_overrides, theme_colors_refinement};
use crate::{merge_accent_colors, merge_player_colors};
use collections::HashMap;
use gpui::{
    App, Context, Font, FontFallbacks, FontStyle, FontWeight, Global, Hsla, Pixels, SharedString,
    Subscription, Window, px,
};
use refineable::Refineable;
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
pub use settings::{FontFamilyName, IconThemeName, ThemeAppearanceMode, ThemeName};
use settings::{IntoGpui, RegisterSetting, Settings, SettingsContent};
use std::sync::Arc;
use theme::{
    Appearance, CornerStyle, DEFAULT_ICON_THEME_NAME, DerivedColors, SyntaxTheme, Theme,
    ThemeColors, UiDensity,
};

const MIN_FONT_SIZE: Pixels = px(6.0);
const MAX_FONT_SIZE: Pixels = px(100.0);
const MIN_LINE_HEIGHT: f32 = 1.0;
/// The opacity of the selection drawn in a custom accent when the theme's own selection is opaque.
const DEFAULT_SELECTION_ALPHA: f32 = 0.25;

pub(crate) fn ui_density_from_settings(val: settings::UiDensity) -> UiDensity {
    match val {
        settings::UiDensity::Compact => UiDensity::Compact,
        settings::UiDensity::Default => UiDensity::Default,
        settings::UiDensity::Comfortable => UiDensity::Comfortable,
    }
}

pub fn appearance_to_mode(appearance: Appearance) -> ThemeAppearanceMode {
    match appearance {
        Appearance::Light => ThemeAppearanceMode::Light,
        Appearance::Dark => ThemeAppearanceMode::Dark,
    }
}

/// Customizable settings for the UI and theme system.
#[derive(Clone, PartialEq, RegisterSetting)]
pub struct ThemeSettings {
    /// The UI font size. Determines the size of text in the UI,
    /// as well as the size of a [gpui::Rems] unit.
    ///
    /// Changing this will impact the size of all UI elements.
    ui_font_size: Pixels,
    /// The font used for UI elements.
    pub ui_font: Font,
    /// The font size used for buffers, and the terminal.
    ///
    /// The terminal font size can be overridden using it's own setting.
    buffer_font_size: Pixels,
    /// The font used for buffers, and the terminal.
    ///
    /// The terminal font family can be overridden using it's own setting.
    pub buffer_font: Font,
    /// The agent UI font family. Determines the family of response text in the agent panel.
    /// Falls back to the UI font family if unset.
    agent_ui_font_family: Option<SharedString>,
    /// The agent font size. Determines the size of text in the agent panel. Falls back to the UI font size if unset.
    agent_ui_font_size: Option<Pixels>,
    /// The agent buffer font family. Determines the family of user messages in the agent panel.
    /// Falls back to the buffer font family if unset.
    agent_buffer_font_family: Option<SharedString>,
    /// The agent buffer font size. Determines the size of user messages in the agent panel.
    agent_buffer_font_size: Option<Pixels>,
    git_commit_buffer_font_size: Option<Pixels>,
    /// The font family to use for rendering in the markdown preview.
    /// Falls back to the UI font family if unset.
    markdown_preview_font_family: Option<SharedString>,
    /// The font family to use for code in the markdown preview.
    /// Falls back to the buffer font family if unset.
    markdown_preview_code_font_family: Option<SharedString>,
    /// The font size to use for rendering in the markdown preview.
    /// Falls back to the UI font size if unset.
    markdown_preview_font_size: Option<Pixels>,
    /// The theme to use for the markdown preview.
    /// Falls back to the main editor theme if unset.
    pub markdown_preview_theme: Option<ThemeSelection>,
    /// The font family used for Mermaid diagrams.
    /// Falls back to the UI font family if unset.
    mermaid_font_family: Option<SharedString>,
    /// The font weight for headings in the markdown preview.
    markdown_preview_heading_font_weight: FontWeight,
    /// The line height for buffers, and the terminal.
    ///
    /// Changing this may affect the spacing of some UI elements.
    ///
    /// The terminal font family can be overridden using it's own setting.
    pub buffer_line_height: BufferLineHeight,
    /// The current theme selection.
    pub theme: ThemeSelection,
    /// Manual overrides for the active theme.
    ///
    /// Note: This setting is still experimental. See [this tracking issue](https://github.com/zed-industries/zed/issues/18078)
    pub experimental_theme_overrides: Option<settings::ThemeStyleContent>,
    /// Manual overrides per theme
    pub theme_overrides: HashMap<String, settings::ThemeStyleContent>,
    /// The current icon theme selection.
    pub icon_theme: IconThemeSelection,
    /// The density of the UI.
    /// Note: This setting is still experimental. See [this tracking issue](
    pub ui_density: UiDensity,
    /// The amount of fading applied to unnecessary code.
    pub unnecessary_code_fade: f32,
    /// Whether corners follow the active theme or are forced to one style.
    pub corners: settings::CornersSetting,
    /// The accent color chosen by the user, when the `accent` setting is not "theme".
    pub accent: Option<Hsla>,
}

/// Returns the name of the default theme for the given [`Appearance`].
pub fn default_theme(appearance: Appearance) -> &'static str {
    match appearance {
        Appearance::Light => settings::DEFAULT_LIGHT_THEME,
        Appearance::Dark => settings::DEFAULT_DARK_THEME,
    }
}

#[derive(Default)]
struct BufferFontSize(Pixels);

impl Global for BufferFontSize {}

#[derive(Default)]
pub(crate) struct UiFontSize(Pixels);

impl Global for UiFontSize {}

/// In-memory override for the UI font size in the agent panel.
#[derive(Default)]
pub struct AgentUiFontSize(Pixels);

impl Global for AgentUiFontSize {}

/// In-memory override for the buffer font size in the agent panel.
#[derive(Default)]
pub struct AgentBufferFontSize(Pixels);

impl Global for AgentBufferFontSize {}

#[derive(Default)]
pub struct GitCommitBufferFontSize(Pixels);

impl Global for GitCommitBufferFontSize {}

/// In-memory override for the markdown preview font size.
#[derive(Default)]
pub struct MarkdownPreviewFontSize(Pixels);

impl Global for MarkdownPreviewFontSize {}

/// Represents the selection of a theme, which can be either static or dynamic.
#[derive(Clone, Debug, Serialize, Deserialize, JsonSchema, PartialEq, Eq)]
#[serde(untagged)]
pub enum ThemeSelection {
    /// A static theme selection, represented by a single theme name.
    Static(ThemeName),
    /// A dynamic theme selection, which can change based the [ThemeMode].
    Dynamic {
        /// The mode used to determine which theme to use.
        #[serde(default)]
        mode: ThemeAppearanceMode,
        /// The theme to use for light mode.
        light: ThemeName,
        /// The theme to use for dark mode.
        dark: ThemeName,
    },
}

impl From<settings::ThemeSelection> for ThemeSelection {
    fn from(selection: settings::ThemeSelection) -> Self {
        match selection {
            settings::ThemeSelection::Static(theme) => ThemeSelection::Static(theme),
            settings::ThemeSelection::Dynamic { mode, light, dark } => {
                ThemeSelection::Dynamic { mode, light, dark }
            }
        }
    }
}

impl ThemeSelection {
    /// Returns the theme name for the selected [ThemeMode].
    pub fn name(&self, system_appearance: Appearance) -> ThemeName {
        match self {
            Self::Static(theme) => theme.clone(),
            Self::Dynamic { mode, light, dark } => match mode {
                ThemeAppearanceMode::Light => light.clone(),
                ThemeAppearanceMode::Dark => dark.clone(),
                ThemeAppearanceMode::System => match system_appearance {
                    Appearance::Light => light.clone(),
                    Appearance::Dark => dark.clone(),
                },
            },
        }
    }

    /// Returns the [ThemeMode] for the [ThemeSelection].
    pub fn mode(&self) -> Option<ThemeAppearanceMode> {
        match self {
            ThemeSelection::Static(_) => None,
            ThemeSelection::Dynamic { mode, .. } => Some(*mode),
        }
    }
}

/// Represents the selection of an icon theme, which can be either static or dynamic.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum IconThemeSelection {
    /// A static icon theme selection, represented by a single icon theme name.
    Static(IconThemeName),
    /// A dynamic icon theme selection, which can change based on the [`ThemeMode`].
    Dynamic {
        /// The mode used to determine which theme to use.
        mode: ThemeAppearanceMode,
        /// The icon theme to use for light mode.
        light: IconThemeName,
        /// The icon theme to use for dark mode.
        dark: IconThemeName,
    },
}

impl From<settings::IconThemeSelection> for IconThemeSelection {
    fn from(selection: settings::IconThemeSelection) -> Self {
        match selection {
            settings::IconThemeSelection::Static(theme) => IconThemeSelection::Static(theme),
            settings::IconThemeSelection::Dynamic { mode, light, dark } => {
                IconThemeSelection::Dynamic { mode, light, dark }
            }
        }
    }
}

impl IconThemeSelection {
    /// Returns the icon theme name based on the given [`Appearance`].
    pub fn name(&self, system_appearance: Appearance) -> IconThemeName {
        match self {
            Self::Static(theme) => theme.clone(),
            Self::Dynamic { mode, light, dark } => match mode {
                ThemeAppearanceMode::Light => light.clone(),
                ThemeAppearanceMode::Dark => dark.clone(),
                ThemeAppearanceMode::System => match system_appearance {
                    Appearance::Light => light.clone(),
                    Appearance::Dark => dark.clone(),
                },
            },
        }
    }

    /// Returns the [`ThemeMode`] for the [`IconThemeSelection`].
    pub fn mode(&self) -> Option<ThemeAppearanceMode> {
        match self {
            IconThemeSelection::Static(_) => None,
            IconThemeSelection::Dynamic { mode, .. } => Some(*mode),
        }
    }
}

/// Sets the theme for the given appearance to the theme with the specified name.
///
/// The caller should make sure that the [`Appearance`] matches the theme associated with the name.
///
/// If the current [`ThemeAppearanceMode`] is set to [`System`] and the user's system [`Appearance`]
/// is different than the new theme's [`Appearance`], this function will update the
/// [`ThemeAppearanceMode`] to the new theme's appearance in order to display the new theme.
///
/// [`System`]: ThemeAppearanceMode::System
pub fn set_theme(
    current: &mut SettingsContent,
    theme_name: impl Into<Arc<str>>,
    theme_appearance: Appearance,
    system_appearance: Appearance,
) {
    let theme_name = ThemeName(theme_name.into());

    let Some(selection) = current.theme.theme.as_mut() else {
        current.theme.theme = Some(settings::ThemeSelection::Static(theme_name));
        return;
    };

    match selection {
        settings::ThemeSelection::Static(theme) => {
            *theme = theme_name;
        }
        settings::ThemeSelection::Dynamic { mode, light, dark } => {
            match theme_appearance {
                Appearance::Light => *light = theme_name,
                Appearance::Dark => *dark = theme_name,
            }

            let should_update_mode =
                !(mode == &ThemeAppearanceMode::System && theme_appearance == system_appearance);

            if should_update_mode {
                *mode = appearance_to_mode(theme_appearance);
            }
        }
    }
}

/// Sets the icon theme for the given appearance to the icon theme with the specified name.
pub fn set_icon_theme(
    current: &mut SettingsContent,
    icon_theme_name: IconThemeName,
    appearance: Appearance,
) {
    if let Some(selection) = current.theme.icon_theme.as_mut() {
        let icon_theme_to_update = match selection {
            settings::IconThemeSelection::Static(theme) => theme,
            settings::IconThemeSelection::Dynamic { mode, light, dark } => match mode {
                ThemeAppearanceMode::Light => light,
                ThemeAppearanceMode::Dark => dark,
                ThemeAppearanceMode::System => match appearance {
                    Appearance::Light => light,
                    Appearance::Dark => dark,
                },
            },
        };

        *icon_theme_to_update = icon_theme_name;
    } else {
        current.theme.icon_theme = Some(settings::IconThemeSelection::Static(icon_theme_name));
    }
}

/// Sets the mode for the theme.
pub fn set_mode(content: &mut SettingsContent, mode: ThemeAppearanceMode) {
    let theme = content.theme.as_mut();

    if let Some(selection) = theme.theme.as_mut() {
        match selection {
            settings::ThemeSelection::Static(_) => {
                *selection = settings::ThemeSelection::Dynamic {
                    mode: ThemeAppearanceMode::System,
                    light: ThemeName(settings::DEFAULT_LIGHT_THEME.into()),
                    dark: ThemeName(settings::DEFAULT_DARK_THEME.into()),
                };
            }
            settings::ThemeSelection::Dynamic {
                mode: mode_to_update,
                ..
            } => *mode_to_update = mode,
        }
    } else {
        theme.theme = Some(settings::ThemeSelection::Dynamic {
            mode,
            light: ThemeName(settings::DEFAULT_LIGHT_THEME.into()),
            dark: ThemeName(settings::DEFAULT_DARK_THEME.into()),
        });
    }

    if let Some(selection) = theme.icon_theme.as_mut() {
        match selection {
            settings::IconThemeSelection::Static(icon_theme) => {
                *selection = settings::IconThemeSelection::Dynamic {
                    mode,
                    light: icon_theme.clone(),
                    dark: icon_theme.clone(),
                };
            }
            settings::IconThemeSelection::Dynamic {
                mode: mode_to_update,
                ..
            } => *mode_to_update = mode,
        }
    } else {
        theme.icon_theme = Some(settings::IconThemeSelection::Static(IconThemeName(
            DEFAULT_ICON_THEME_NAME.into(),
        )));
    }
}

pub use theme::BufferLineHeight;

pub fn buffer_line_height_from_settings(value: settings::BufferLineHeight) -> BufferLineHeight {
    match value {
        settings::BufferLineHeight::Comfortable => BufferLineHeight::Comfortable,
        settings::BufferLineHeight::Standard => BufferLineHeight::Standard,
        settings::BufferLineHeight::Custom(line_height) => BufferLineHeight::Custom(line_height),
    }
}

impl ThemeSettings {
    /// Returns the buffer font size.
    pub fn buffer_font_size(&self, cx: &App) -> Pixels {
        let font_size = cx
            .try_global::<BufferFontSize>()
            .map(|size| size.0)
            .unwrap_or(self.buffer_font_size);
        clamp_font_size(font_size)
    }

    /// Returns the UI font size.
    pub fn ui_font_size(&self, cx: &App) -> Pixels {
        let font_size = cx
            .try_global::<UiFontSize>()
            .map(|size| size.0)
            .unwrap_or(self.ui_font_size);
        clamp_font_size(font_size)
    }

    /// Returns the agent panel font size. Falls back to the UI font size if unset.
    pub fn agent_ui_font_size(&self, cx: &App) -> Pixels {
        cx.try_global::<AgentUiFontSize>()
            .map(|size| size.0)
            .or(self.agent_ui_font_size)
            .map(clamp_font_size)
            .unwrap_or_else(|| self.ui_font_size(cx))
    }

    pub fn agent_ui_font_family(&self) -> &SharedString {
        self.agent_ui_font_family
            .as_ref()
            .unwrap_or(&self.ui_font.family)
    }

    /// Returns the agent panel buffer font size.
    pub fn agent_buffer_font_size(&self, cx: &App) -> Pixels {
        cx.try_global::<AgentBufferFontSize>()
            .map(|size| size.0)
            .or(self.agent_buffer_font_size)
            .map(clamp_font_size)
            .unwrap_or_else(|| self.buffer_font_size(cx))
    }

    pub fn agent_buffer_font_family(&self) -> &SharedString {
        self.agent_buffer_font_family
            .as_ref()
            .unwrap_or(&self.buffer_font.family)
    }

    pub fn git_commit_buffer_font_size(&self, cx: &App) -> Pixels {
        cx.try_global::<GitCommitBufferFontSize>()
            .map(|size| size.0)
            .or(self.git_commit_buffer_font_size)
            .map(clamp_font_size)
            .unwrap_or_else(|| self.buffer_font_size(cx))
    }

    /// Returns the font family to use in the markdown preview,
    /// falling back to the UI font family when unset.
    pub fn markdown_preview_font_family(&self) -> &SharedString {
        self.markdown_preview_font_family
            .as_ref()
            .unwrap_or(&self.ui_font.family)
    }

    /// Returns the font family to use for code in the markdown preview,
    /// falling back to the buffer font family when unset.
    pub fn markdown_preview_code_font_family(&self) -> &SharedString {
        self.markdown_preview_code_font_family
            .as_ref()
            .unwrap_or(&self.buffer_font.family)
    }

    /// Returns the font family to use for Mermaid diagrams,
    /// falling back to the UI font family when unset.
    pub fn mermaid_font_family(&self) -> &SharedString {
        self.mermaid_font_family
            .as_ref()
            .unwrap_or(&self.ui_font.family)
    }

    /// Returns the font weight to use for headings in the markdown preview.
    pub fn markdown_preview_heading_font_weight(&self) -> FontWeight {
        self.markdown_preview_heading_font_weight
    }

    /// Returns the markdown preview font size.
    ///
    /// Note: the fallback deliberately uses `self.ui_font_size` instead of `ui_font_size(cx)`,
    /// so that temporary UI zoom does not also resize the markdown preview.
    pub fn markdown_preview_font_size(&self, cx: &App) -> Pixels {
        cx.try_global::<MarkdownPreviewFontSize>()
            .map(|size| size.0)
            .or(self.markdown_preview_font_size)
            .map(clamp_font_size)
            .unwrap_or_else(|| clamp_font_size(self.ui_font_size))
    }

    /// Returns the buffer font size, read from the settings.
    ///
    /// The real buffer font size is stored in-memory, to support temporary font size changes.
    /// Use [`Self::buffer_font_size`] to get the real font size.
    pub fn buffer_font_size_settings(&self) -> Pixels {
        self.buffer_font_size
    }

    /// Returns the UI font size, read from the settings.
    ///
    /// The real UI font size is stored in-memory, to support temporary font size changes.
    /// Use [`Self::ui_font_size`] to get the real font size.
    pub fn ui_font_size_settings(&self) -> Pixels {
        self.ui_font_size
    }

    /// Returns the agent font size, read from the settings.
    ///
    /// The real agent font size is stored in-memory, to support temporary font size changes.
    /// Use [`Self::agent_ui_font_size`] to get the real font size.
    pub fn agent_ui_font_size_settings(&self) -> Option<Pixels> {
        self.agent_ui_font_size
    }

    /// Returns the agent buffer font size, read from the settings.
    ///
    /// The real agent buffer font size is stored in-memory, to support temporary font size changes.
    /// Use [`Self::agent_buffer_font_size`] to get the real font size.
    pub fn agent_buffer_font_size_settings(&self) -> Option<Pixels> {
        self.agent_buffer_font_size
    }

    pub fn git_commit_buffer_font_size_settings(&self) -> Option<Pixels> {
        self.git_commit_buffer_font_size
    }

    /// Returns the markdown preview font size, read from the settings.
    ///
    /// The real markdown preview font size is stored in-memory, to support temporary
    /// font size changes. Use [`Self::markdown_preview_font_size`] to get the real font size.
    pub fn markdown_preview_font_size_settings(&self) -> Option<Pixels> {
        self.markdown_preview_font_size
    }

    /// Returns the buffer's line height.
    pub fn line_height(&self) -> f32 {
        f32::max(self.buffer_line_height.value(), MIN_LINE_HEIGHT)
    }

    /// Applies the theme overrides, if there are any, to the current theme.
    pub fn apply_theme_overrides(&self, mut arc_theme: Arc<Theme>) -> Arc<Theme> {
        if let Some(experimental_theme_overrides) = &self.experimental_theme_overrides {
            let mut theme = (*arc_theme).clone();
            ThemeSettings::modify_theme(&mut theme, experimental_theme_overrides);
            arc_theme = Arc::new(theme);
        }

        if let Some(theme_overrides) = self.theme_overrides.get(arc_theme.name.as_ref()) {
            let mut theme = (*arc_theme).clone();
            ThemeSettings::modify_theme(&mut theme, theme_overrides);
            arc_theme = Arc::new(theme);
        }

        // The `corners` and `accent` settings come last: they are the user's choice for every
        // theme, so they win over what a theme or a theme override asks for.
        Self::apply_corners_and_accent(arc_theme, self.forced_corner_style(), self.accent)
    }

    /// Writes a forced corner style and a custom accent into the theme. The theme is cloned only
    /// when something changes.
    fn apply_corners_and_accent(
        mut arc_theme: Arc<Theme>,
        corners: Option<CornerStyle>,
        accent: Option<Hsla>,
    ) -> Arc<Theme> {
        let corners = corners.filter(|corners| *corners != arc_theme.styles.corners);
        if corners.is_some() || accent.is_some() {
            let mut theme = (*arc_theme).clone();
            if let Some(corners) = corners {
                theme.styles.corners = corners;
            }
            if let Some(accent) = accent {
                Self::apply_accent(&mut theme, accent);
            }
            arc_theme = Arc::new(theme);
        }
        arc_theme
    }

    /// The corner style forced by the `corners` setting, or `None` when it follows the theme.
    pub fn forced_corner_style(&self) -> Option<CornerStyle> {
        match self.corners {
            settings::CornersSetting::Theme => None,
            settings::CornersSetting::Square => Some(CornerStyle::Square),
            settings::CornersSetting::Soft => Some(CornerStyle::Soft),
            settings::CornersSetting::Round => Some(CornerStyle::Round),
        }
    }

    /// Replaces the theme's accent: the fill of the primary action (its hover and its text are
    /// recomputed), accent text and icons, hovered links, and the local player's cursor and
    /// selection.
    fn apply_accent(theme: &mut Theme, accent: Hsla) {
        let appearance = theme.appearance;
        let colors = &mut theme.styles.colors;
        colors.accent_background = accent;
        colors.accent_hover = ThemeColors::accent_hover_for(accent, appearance);
        colors.accent_foreground = ThemeColors::accent_foreground_for(accent);
        colors.text_accent = accent;
        colors.icon_accent = accent;
        colors.link_text_hover = accent;

        if let Some(local) = theme.styles.player.0.first_mut() {
            // The selection keeps the translucency the theme gave it.
            let selection_alpha = if local.selection.a < 1.0 {
                local.selection.a
            } else {
                DEFAULT_SELECTION_ALPHA
            };
            let previous_selection = local.selection;
            local.cursor = accent;
            local.selection = accent.alpha(selection_alpha);

            // Selections in inputs follow the local player's selection unless the theme gave
            // them a colour of their own (see `theme::apply_theme_color_defaults`).
            let follows_player = colors.element_selection_background == previous_selection
                || colors.element_selection_background
                    == previous_selection.alpha(DEFAULT_SELECTION_ALPHA);
            if follows_player {
                colors.element_selection_background = local.selection;
            }
        }
    }

    fn modify_theme(base_theme: &mut Theme, theme_overrides: &settings::ThemeStyleContent) {
        if let Some(window_background_appearance) = theme_overrides.window_background_appearance {
            base_theme.styles.window_background_appearance =
                window_background_appearance.into_gpui();
        }
        if let Some(corners) = theme_overrides.corners {
            base_theme.styles.corners = crate::corner_style_from_content(corners);
        }
        let status_color_refinement = status_colors_refinement(&theme_overrides.status);

        let theme_color_refinement = theme_colors_refinement(
            &theme_overrides.colors,
            &status_color_refinement,
            base_theme.appearance.is_light(),
        );
        // A derived token (accent fill and its hover and text, strong line, faint text) that
        // still holds its fallback keeps following the colour it falls back to when that colour
        // is overridden, unless the override states the token itself.
        let appearance = base_theme.appearance;
        let derived_in_sync = base_theme
            .styles
            .colors
            .derived_colors_in_sync(base_theme.styles.player.local().cursor, appearance);
        base_theme.styles.colors.refine(&theme_color_refinement);
        base_theme.styles.status.refine(&status_color_refinement);
        merge_player_colors(&mut base_theme.styles.player, &theme_overrides.players);
        base_theme.styles.colors.derive_colors(
            derived_in_sync.and(DerivedColors::unset_in(&theme_color_refinement)),
            base_theme.styles.player.local().cursor,
            appearance,
        );
        merge_accent_colors(&mut base_theme.styles.accents, &theme_overrides.accents);
        base_theme.styles.syntax = SyntaxTheme::merge(
            base_theme.styles.syntax.clone(),
            syntax_overrides(theme_overrides),
        );
    }
}

/// Observe changes to the adjusted buffer font size.
pub fn observe_buffer_font_size_adjustment<V: 'static>(
    cx: &mut Context<V>,
    f: impl 'static + Fn(&mut V, &mut Context<V>),
) -> Subscription {
    cx.observe_global::<BufferFontSize>(f)
}

/// Gets the font size, adjusted by the difference between the current buffer font size and the one set in the settings.
pub fn adjusted_font_size(size: Pixels, cx: &App) -> Pixels {
    let adjusted_font_size =
        if let Some(BufferFontSize(adjusted_size)) = cx.try_global::<BufferFontSize>() {
            let buffer_font_size = ThemeSettings::get_global(cx).buffer_font_size;
            let delta = *adjusted_size - buffer_font_size;
            size + delta
        } else {
            size
        };
    clamp_font_size(adjusted_font_size)
}

/// Adjusts the buffer font size, without persisting the result in the settings.
/// This will be effective until the app is restarted.
pub fn adjust_buffer_font_size(cx: &mut App, f: impl FnOnce(Pixels) -> Pixels) {
    let buffer_font_size = ThemeSettings::get_global(cx).buffer_font_size;
    let adjusted_size = cx
        .try_global::<BufferFontSize>()
        .map_or(buffer_font_size, |adjusted_size| adjusted_size.0);
    cx.set_global(BufferFontSize(clamp_font_size(f(adjusted_size))));
    cx.refresh_windows();
}

/// Resets the buffer font size to the default value.
pub fn reset_buffer_font_size(cx: &mut App) {
    if cx.has_global::<BufferFontSize>() {
        cx.remove_global::<BufferFontSize>();
        cx.refresh_windows();
    }
}

#[allow(missing_docs)]
pub fn setup_ui_font(window: &mut Window, cx: &mut App) -> gpui::Font {
    let (ui_font, ui_font_size) = {
        let theme_settings = ThemeSettings::get_global(cx);
        let font = theme_settings.ui_font.clone();
        (font, theme_settings.ui_font_size(cx))
    };

    window.set_rem_size(ui_font_size);
    ui_font
}

/// Sets the adjusted UI font size.
pub fn adjust_ui_font_size(cx: &mut App, f: impl FnOnce(Pixels) -> Pixels) {
    let ui_font_size = ThemeSettings::get_global(cx).ui_font_size(cx);
    let adjusted_size = cx
        .try_global::<UiFontSize>()
        .map_or(ui_font_size, |adjusted_size| adjusted_size.0);
    cx.set_global(UiFontSize(clamp_font_size(f(adjusted_size))));
    cx.refresh_windows();
}

/// Resets the UI font size to the default value.
pub fn reset_ui_font_size(cx: &mut App) {
    if cx.has_global::<UiFontSize>() {
        cx.remove_global::<UiFontSize>();
        cx.refresh_windows();
    }
}

/// Sets the adjusted font size of agent responses in the agent panel.
pub fn adjust_agent_ui_font_size(cx: &mut App, f: impl FnOnce(Pixels) -> Pixels) {
    let agent_ui_font_size = ThemeSettings::get_global(cx).agent_ui_font_size(cx);
    let adjusted_size = cx
        .try_global::<AgentUiFontSize>()
        .map_or(agent_ui_font_size, |adjusted_size| adjusted_size.0);
    cx.set_global(AgentUiFontSize(clamp_font_size(f(adjusted_size))));
    cx.refresh_windows();
}

/// Resets the agent response font size in the agent panel to the default value.
pub fn reset_agent_ui_font_size(cx: &mut App) {
    if cx.has_global::<AgentUiFontSize>() {
        cx.remove_global::<AgentUiFontSize>();
        cx.refresh_windows();
    }
}

/// Sets the adjusted font size of user messages in the agent panel.
pub fn adjust_agent_buffer_font_size(cx: &mut App, f: impl FnOnce(Pixels) -> Pixels) {
    let agent_buffer_font_size = ThemeSettings::get_global(cx).agent_buffer_font_size(cx);
    let adjusted_size = cx
        .try_global::<AgentBufferFontSize>()
        .map_or(agent_buffer_font_size, |adjusted_size| adjusted_size.0);
    cx.set_global(AgentBufferFontSize(clamp_font_size(f(adjusted_size))));
    cx.refresh_windows();
}

/// Resets the user message font size in the agent panel to the default value.
pub fn reset_agent_buffer_font_size(cx: &mut App) {
    if cx.has_global::<AgentBufferFontSize>() {
        cx.remove_global::<AgentBufferFontSize>();
        cx.refresh_windows();
    }
}

pub fn adjust_git_commit_buffer_font_size(cx: &mut App, f: impl FnOnce(Pixels) -> Pixels) {
    let git_commit_buffer_font_size = ThemeSettings::get_global(cx).git_commit_buffer_font_size(cx);
    let adjusted_size = cx
        .try_global::<GitCommitBufferFontSize>()
        .map_or(git_commit_buffer_font_size, |adjusted_size| adjusted_size.0);
    cx.set_global(GitCommitBufferFontSize(clamp_font_size(f(adjusted_size))));
    cx.refresh_windows();
}

pub fn reset_git_commit_buffer_font_size(cx: &mut App) {
    if cx.has_global::<GitCommitBufferFontSize>() {
        cx.remove_global::<GitCommitBufferFontSize>();
        cx.refresh_windows();
    }
}

/// Sets the adjusted font size of the markdown preview.
pub fn adjust_markdown_preview_font_size(cx: &mut App, f: impl FnOnce(Pixels) -> Pixels) {
    let markdown_preview_font_size = ThemeSettings::get_global(cx).markdown_preview_font_size(cx);
    let adjusted_size = cx
        .try_global::<MarkdownPreviewFontSize>()
        .map_or(markdown_preview_font_size, |adjusted_size| adjusted_size.0);
    cx.set_global(MarkdownPreviewFontSize(clamp_font_size(f(adjusted_size))));
    cx.refresh_windows();
}

/// Resets the markdown preview font size to the default value.
pub fn reset_markdown_preview_font_size(cx: &mut App) {
    if cx.has_global::<MarkdownPreviewFontSize>() {
        cx.remove_global::<MarkdownPreviewFontSize>();
        cx.refresh_windows();
    }
}

/// Ensures font size is within the valid range.
pub fn clamp_font_size(size: Pixels) -> Pixels {
    size.clamp(MIN_FONT_SIZE, MAX_FONT_SIZE)
}

fn font_fallbacks_from_settings(
    fallbacks: Option<Vec<settings::FontFamilyName>>,
) -> Option<FontFallbacks> {
    fallbacks.map(|fallbacks| {
        FontFallbacks::from_fonts(
            fallbacks
                .into_iter()
                .map(|font_family| font_family.0.to_string())
                .collect(),
        )
    })
}

/// Reads the `accent` setting: `None` for "theme" (or unset), the color otherwise. A value that
/// is not a color is reported once in the log and ignored.
fn accent_from_settings(accent: Option<&settings::AccentSetting>) -> Option<Hsla> {
    let color = accent?.custom_color()?;
    match theme::try_parse_color(color) {
        // An accent is a fill: a translucent one would let the ground show through buttons.
        Ok(color) => Some(color.alpha(1.0)),
        Err(error) => {
            log::warn!("ignoring the `accent` setting {color:?}: {error}");
            None
        }
    }
}

impl settings::Settings for ThemeSettings {
    fn from_settings(settings_content: &settings::SettingsContent) -> Self {
        let content = &settings_content.theme;
        let markdown_preview = settings_content.markdown_preview.as_ref();
        let theme_selection: ThemeSelection = content.theme.clone().unwrap().into();
        let icon_theme_selection: IconThemeSelection = content.icon_theme.clone().unwrap().into();
        Self {
            ui_font_size: clamp_font_size(content.ui_font_size.unwrap().into_gpui()),
            ui_font: Font {
                family: content.ui_font_family.as_ref().unwrap().0.clone().into(),
                features: content.ui_font_features.clone().unwrap().into_gpui(),
                fallbacks: font_fallbacks_from_settings(content.ui_font_fallbacks.clone()),
                weight: content.ui_font_weight.unwrap().into_gpui(),
                style: Default::default(),
            },
            buffer_font: Font {
                family: content
                    .buffer_font_family
                    .as_ref()
                    .unwrap()
                    .0
                    .clone()
                    .into(),
                features: content.buffer_font_features.clone().unwrap().into_gpui(),
                fallbacks: font_fallbacks_from_settings(content.buffer_font_fallbacks.clone()),
                weight: content.buffer_font_weight.unwrap().into_gpui(),
                style: FontStyle::default(),
            },
            buffer_font_size: clamp_font_size(content.buffer_font_size.unwrap().into_gpui()),
            buffer_line_height: buffer_line_height_from_settings(
                content.buffer_line_height.unwrap(),
            ),
            agent_ui_font_family: content
                .agent_ui_font_family
                .as_ref()
                .map(|font| font.0.clone().into()),
            agent_ui_font_size: content.agent_ui_font_size.map(|s| s.into_gpui()),
            agent_buffer_font_family: content
                .agent_buffer_font_family
                .as_ref()
                .map(|font| font.0.clone().into()),
            agent_buffer_font_size: content.agent_buffer_font_size.map(|s| s.into_gpui()),
            git_commit_buffer_font_size: content.git_commit_buffer_font_size.map(|s| s.into_gpui()),
            markdown_preview_font_family: markdown_preview
                .and_then(|preview| preview.font_family.as_ref())
                .map(|f| f.0.clone().into()),
            markdown_preview_code_font_family: markdown_preview
                .and_then(|preview| preview.code_font_family.as_ref())
                .map(|f| f.0.clone().into()),
            markdown_preview_font_size: markdown_preview
                .and_then(|preview| preview.font_size)
                .map(|size| size.into_gpui()),
            markdown_preview_theme: markdown_preview
                .and_then(|preview| preview.theme.clone())
                .map(ThemeSelection::from),
            mermaid_font_family: content
                .mermaid_font_family
                .as_ref()
                .map(|font| font.0.clone().into()),
            markdown_preview_heading_font_weight: markdown_preview
                .and_then(|preview| preview.heading_font_weight)
                .map(|weight| weight.into_gpui())
                .unwrap_or(FontWeight::SEMIBOLD),
            theme: theme_selection,
            experimental_theme_overrides: content.experimental_theme_overrides.clone(),
            theme_overrides: content.theme_overrides.clone(),
            icon_theme: icon_theme_selection,
            ui_density: ui_density_from_settings(content.ui_density.unwrap_or_default()),
            unnecessary_code_fade: content.unnecessary_code_fade.unwrap().0.clamp(0.0, 0.9),
            corners: content.corners.unwrap_or_default(),
            accent: accent_from_settings(content.accent.as_ref()),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn theme_with_colors(colors: ::settings::ThemeColorsContent) -> Theme {
        crate::refine_theme(&crate::ThemeContent {
            name: "Test".into(),
            appearance: ::theme::AppearanceContent::Dark,
            style: ::settings::ThemeStyleContent {
                colors,
                ..Default::default()
            },
        })
    }

    fn style_with_colors(colors: ::settings::ThemeColorsContent) -> ::settings::ThemeStyleContent {
        ::settings::ThemeStyleContent {
            colors,
            ..Default::default()
        }
    }

    fn color(hex: &str) -> Hsla {
        ::theme::try_parse_color(hex).unwrap()
    }

    #[test]
    fn derived_tokens_fall_back_when_the_theme_does_not_set_them() {
        let test_theme = theme_with_colors(::settings::ThemeColorsContent {
            border: Some("#112233".into()),
            text_muted: Some("#445566".into()),
            ..Default::default()
        });
        let colors = &test_theme.styles.colors;
        let cursor = test_theme.styles.player.local().cursor;

        assert_eq!(colors.accent_background, cursor);
        assert_eq!(
            colors.accent_hover,
            ThemeColors::accent_hover_for(cursor, Appearance::Dark)
        );
        assert_eq!(
            colors.accent_foreground,
            ThemeColors::accent_foreground_for(cursor)
        );
        assert_eq!(colors.border_strong, color("#112233"));
        assert_eq!(colors.text_faint, color("#445566"));
    }

    #[test]
    fn derived_tokens_keep_the_values_of_the_theme_file() {
        let test_theme = theme_with_colors(::settings::ThemeColorsContent {
            border: Some("#112233".into()),
            border_strong: Some("#3a3733".into()),
            text_muted: Some("#445566".into()),
            text_faint: Some("#827d75".into()),
            accent_background: Some("#e5322d".into()),
            accent_hover: Some("#f2453f".into()),
            accent_foreground: Some("#ffffff".into()),
            ..Default::default()
        });
        let colors = &test_theme.styles.colors;

        assert_eq!(colors.accent_background, color("#e5322d"));
        assert_eq!(colors.accent_hover, color("#f2453f"));
        assert_eq!(colors.accent_foreground, color("#ffffff"));
        assert_eq!(colors.border_strong, color("#3a3733"));
        assert_eq!(colors.text_faint, color("#827d75"));
    }

    #[test]
    fn accent_hover_and_foreground_follow_an_accent_set_by_the_theme() {
        let test_theme = theme_with_colors(::settings::ThemeColorsContent {
            accent_background: Some("#ffd400".into()),
            ..Default::default()
        });
        let colors = &test_theme.styles.colors;

        assert_eq!(
            colors.accent_hover,
            ThemeColors::accent_hover_for(color("#ffd400"), Appearance::Dark)
        );
        // Black is far more readable than white on yellow.
        assert_eq!(colors.accent_foreground, gpui::black());
    }

    #[test]
    fn accent_foreground_is_the_more_readable_of_black_and_white() {
        assert_eq!(
            ThemeColors::accent_foreground_for(color("#1a3a8a")),
            gpui::white()
        );
        assert_eq!(
            ThemeColors::accent_foreground_for(color("#74ade8")),
            gpui::black()
        );
        assert!((::theme::contrast_ratio(gpui::black(), gpui::white()) - 21.0).abs() < 0.01);
    }

    #[test]
    fn derived_tokens_follow_an_override_of_the_color_they_fall_back_to() {
        let mut test_theme = theme_with_colors(::settings::ThemeColorsContent::default());

        ThemeSettings::modify_theme(
            &mut test_theme,
            &::settings::ThemeStyleContent {
                colors: ::settings::ThemeColorsContent {
                    border: Some("#112233".into()),
                    text_muted: Some("#445566".into()),
                    ..Default::default()
                },
                players: vec![::settings::PlayerColorContent {
                    cursor: Some("#ffd400".into()),
                    background: None,
                    selection: None,
                }],
                ..Default::default()
            },
        );

        let colors = &test_theme.styles.colors;
        assert_eq!(colors.border_strong, color("#112233"));
        assert_eq!(colors.text_faint, color("#445566"));
        assert_eq!(colors.accent_background, color("#ffd400"));
        assert_eq!(colors.accent_foreground, gpui::black());
    }

    #[test]
    fn derived_tokens_set_by_the_theme_survive_an_override_of_their_fallback() {
        let mut test_theme = theme_with_colors(::settings::ThemeColorsContent {
            border: Some("#262422".into()),
            border_strong: Some("#3a3733".into()),
            accent_background: Some("#e5322d".into()),
            accent_foreground: Some("#ffffff".into()),
            ..Default::default()
        });

        ThemeSettings::modify_theme(
            &mut test_theme,
            &::settings::ThemeStyleContent {
                colors: ::settings::ThemeColorsContent {
                    border: Some("#112233".into()),
                    ..Default::default()
                },
                players: vec![::settings::PlayerColorContent {
                    cursor: Some("#ffd400".into()),
                    background: None,
                    selection: None,
                }],
                ..Default::default()
            },
        );

        let colors = &test_theme.styles.colors;
        assert_eq!(colors.border_strong, color("#3a3733"));
        assert_eq!(colors.accent_background, color("#e5322d"));
        assert_eq!(colors.accent_foreground, color("#ffffff"));
    }

    #[test]
    fn an_override_of_the_accent_recomputes_its_hover_and_text() {
        let mut test_theme = theme_with_colors(::settings::ThemeColorsContent::default());

        ThemeSettings::modify_theme(
            &mut test_theme,
            &style_with_colors(::settings::ThemeColorsContent {
                accent_background: Some("#ffd400".into()),
                ..Default::default()
            }),
        );

        let colors = &test_theme.styles.colors;
        assert_eq!(colors.accent_background, color("#ffd400"));
        assert_eq!(
            colors.accent_hover,
            ThemeColors::accent_hover_for(color("#ffd400"), Appearance::Dark)
        );
        assert_eq!(colors.accent_foreground, gpui::black());
    }

    #[test]
    fn corners_come_from_the_theme_then_overrides_then_the_setting() {
        let round = theme_with_colors(::settings::ThemeColorsContent::default());
        assert_eq!(round.corners(), CornerStyle::Round);
        assert_eq!(CornerStyle::Round.scale(), 1.0);
        assert_eq!(CornerStyle::Soft.scale(), 0.5);
        assert_eq!(CornerStyle::Square.scale(), 0.0);

        let square = crate::refine_theme(&crate::ThemeContent {
            name: "Square".into(),
            appearance: ::theme::AppearanceContent::Dark,
            style: ::settings::ThemeStyleContent {
                corners: Some(::settings::CornerStyleContent::Square),
                ..Default::default()
            },
        });
        assert_eq!(square.corners(), CornerStyle::Square);

        let mut overridden = square.clone();
        ThemeSettings::modify_theme(
            &mut overridden,
            &::settings::ThemeStyleContent {
                corners: Some(::settings::CornerStyleContent::Soft),
                ..Default::default()
            },
        );
        assert_eq!(overridden.corners(), CornerStyle::Soft);

        // "theme": nothing is forced, and the theme is not even cloned.
        let square = Arc::new(square);
        let untouched = ThemeSettings::apply_corners_and_accent(square.clone(), None, None);
        assert!(Arc::ptr_eq(&square, &untouched));

        let forced =
            ThemeSettings::apply_corners_and_accent(square, Some(CornerStyle::Round), None);
        assert_eq!(forced.corners(), CornerStyle::Round);
    }

    #[test]
    fn corners_are_read_from_theme_json_and_from_settings_json() {
        let style: ::settings::ThemeStyleContent =
            serde_json::from_str(r##"{ "corners": "square", "border": "#112233" }"##).unwrap();
        assert_eq!(style.corners, Some(::settings::CornerStyleContent::Square));
        let style: ::settings::ThemeStyleContent =
            serde_json::from_str(r##"{ "border": "#112233" }"##).unwrap();
        assert_eq!(style.corners, None);

        for (json, expected) in [
            (
                r#"{ "corners": "theme" }"#,
                ::settings::CornersSetting::Theme,
            ),
            (
                r#"{ "corners": "square" }"#,
                ::settings::CornersSetting::Square,
            ),
            (r#"{ "corners": "soft" }"#, ::settings::CornersSetting::Soft),
            (
                r#"{ "corners": "round" }"#,
                ::settings::CornersSetting::Round,
            ),
        ] {
            let content: ::settings::ThemeSettingsContent = serde_json::from_str(json).unwrap();
            assert_eq!(content.corners, Some(expected), "{json}");
        }
        let content: ::settings::ThemeSettingsContent = serde_json::from_str("{}").unwrap();
        assert_eq!(
            content.corners.unwrap_or_default(),
            ::settings::CornersSetting::Theme
        );
    }

    #[test]
    fn accent_setting_reads_theme_or_a_color() {
        let theme_accent = ::settings::AccentSetting::default();
        assert_eq!(accent_from_settings(Some(&theme_accent)), None);
        assert_eq!(accent_from_settings(None), None);
        assert_eq!(
            accent_from_settings(Some(&::settings::AccentSetting("#2f81f7".into()))),
            Some(color("#2f81f7"))
        );
        // Not a colour: ignored, the theme's accent stays.
        assert_eq!(
            accent_from_settings(Some(&::settings::AccentSetting("blue".into()))),
            None
        );
        // An accent is always opaque.
        assert_eq!(
            accent_from_settings(Some(&::settings::AccentSetting("#2f81f780".into())))
                .map(|accent| accent.a),
            Some(1.0)
        );
    }

    #[test]
    fn a_custom_accent_replaces_every_accent_role() {
        let base = Arc::new(theme_with_colors(::settings::ThemeColorsContent {
            accent_background: Some("#e5322d".into()),
            accent_hover: Some("#f2453f".into()),
            accent_foreground: Some("#ffffff".into()),
            ..Default::default()
        }));
        let accent = color("#ffd400");
        let themed = ThemeSettings::apply_corners_and_accent(base.clone(), None, Some(accent));
        let colors = &themed.styles.colors;

        assert_eq!(colors.accent_background, accent);
        assert_eq!(
            colors.accent_hover,
            ThemeColors::accent_hover_for(accent, Appearance::Dark)
        );
        assert_eq!(colors.accent_foreground, gpui::black());
        assert_eq!(colors.text_accent, accent);
        assert_eq!(colors.icon_accent, accent);
        assert_eq!(colors.link_text_hover, accent);

        let local = themed.styles.player.local();
        assert_eq!(local.cursor, accent);
        assert_eq!(local.selection.alpha(1.0), accent);
        assert!(local.selection.a < 1.0);
        assert_eq!(colors.element_selection_background, local.selection);

        // The other players and the theme it was made from are untouched.
        assert_eq!(themed.styles.player.0[1], base.styles.player.0[1]);
        assert_eq!(base.styles.colors.accent_background, color("#e5322d"));
    }

    #[test]
    fn code_lens_foreground_from_theme_survives_text_muted_override() {
        let magenta = ::theme::try_parse_color("#ff00ff").unwrap();
        let green = ::theme::try_parse_color("#00ff00").unwrap();
        let mut test_theme = theme_with_colors(::settings::ThemeColorsContent {
            editor_code_lens_foreground: Some("#ff00ff".into()),
            ..Default::default()
        });

        ThemeSettings::modify_theme(
            &mut test_theme,
            &style_with_colors(::settings::ThemeColorsContent {
                text_muted: Some("#00ff00".into()),
                ..Default::default()
            }),
        );

        assert_eq!(
            test_theme.styles.colors.editor_code_lens_foreground,
            Some(magenta)
        );
        assert_eq!(test_theme.styles.colors.text_muted, green);
    }

    #[test]
    fn code_lens_foreground_from_earlier_override_survives_later_muted_override() {
        let magenta = ::theme::try_parse_color("#ff00ff").unwrap();
        let green = ::theme::try_parse_color("#00ff00").unwrap();
        let mut test_theme = theme_with_colors(Default::default());

        ThemeSettings::modify_theme(
            &mut test_theme,
            &style_with_colors(::settings::ThemeColorsContent {
                editor_code_lens_foreground: Some("#ff00ff".into()),
                ..Default::default()
            }),
        );
        ThemeSettings::modify_theme(
            &mut test_theme,
            &style_with_colors(::settings::ThemeColorsContent {
                text_muted: Some("#00ff00".into()),
                ..Default::default()
            }),
        );

        assert_eq!(
            test_theme.styles.colors.editor_code_lens_foreground,
            Some(magenta)
        );
        assert_eq!(test_theme.styles.colors.text_muted, green);
    }

    #[test]
    fn code_lens_foreground_follows_effective_text_muted_without_explicit_color() {
        let green = ::theme::try_parse_color("#00ff00").unwrap();
        let mut test_theme = theme_with_colors(Default::default());

        ThemeSettings::modify_theme(
            &mut test_theme,
            &style_with_colors(::settings::ThemeColorsContent {
                text_muted: Some("#00ff00".into()),
                ..Default::default()
            }),
        );

        assert_eq!(test_theme.styles.colors.editor_code_lens_foreground, None);
        assert_eq!(
            test_theme
                .styles
                .colors
                .color(::theme::ThemeColorField::EditorCodeLensForeground),
            green
        );
    }
}

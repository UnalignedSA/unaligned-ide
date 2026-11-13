#![allow(missing_docs)]

use gpui::{App, Hsla, Rgba, SharedString, WindowBackgroundAppearance};
use refineable::Refineable;
use std::sync::Arc;
use strum::{AsRefStr, EnumIter, IntoEnumIterator};

use crate::{
    AccentColors, ActiveTheme, Appearance, PlayerColors, StatusColors, StatusColorsRefinement,
    SyntaxTheme, SystemColors,
};

#[derive(Refineable, Clone, Debug, PartialEq)]
#[refineable(Debug, serde::Deserialize)]
pub struct ThemeColors {
    /// Border color. Used for most borders, is usually a high contrast color.
    pub border: Hsla,
    /// Border color. Used for deemphasized borders, like a visual divider between two sections
    pub border_variant: Hsla,
    /// Border color. Used for focused elements, like keyboard focused list item.
    pub border_focused: Hsla,
    /// Border color. Used for selected elements, like an active search filter or selected checkbox.
    pub border_selected: Hsla,
    /// Border color. Used for transparent borders. Used for placeholder borders when an element gains a border on state change.
    pub border_transparent: Hsla,
    /// Border color. Used for disabled elements, like a disabled input or button.
    pub border_disabled: Hsla,
    /// Border color. Used for the outline of things the user can type in or press (inputs, outlined
    /// buttons, key caps) and for the edge of raised surfaces.
    ///
    /// Theme key `border.strong`; `border` when the theme does not set it.
    pub border_strong: Hsla,
    /// Border color. Used for elevated surfaces, like a context menu, popup, or dialog.
    pub elevated_surface_background: Hsla,
    /// Background Color. Used for grounded surfaces like a panel or tab.
    pub surface_background: Hsla,
    /// Background Color. Used for the app background and blank panels or windows.
    pub background: Hsla,
    /// Background Color. Used for the background of an element that should have a different background than the surface it's on.
    ///
    /// Elements might include: Buttons, Inputs, Checkboxes, Radio Buttons...
    ///
    /// For an element that should have the same background as the surface it's on, use `ghost_element_background`.
    pub element_background: Hsla,
    /// Background Color. Used for the hover state of an element that should have a different background than the surface it's on.
    ///
    /// Hover states are triggered by the mouse entering an element, or a finger touching an element on a touch screen.
    pub element_hover: Hsla,
    /// Background Color. Used for the active state of an element that should have a different background than the surface it's on.
    ///
    /// Active states are triggered by the mouse button being pressed down on an element, or the Return button or other activator being pressed.
    pub element_active: Hsla,
    /// Background Color. Used for the selected state of an element that should have a different background than the surface it's on.
    ///
    /// Selected states are triggered by the element being selected (or "activated") by the user.
    ///
    /// This could include a selected checkbox, a toggleable button that is toggled on, etc.
    pub element_selected: Hsla,
    /// Background Color. Used for the background of selections in a UI element.
    pub element_selection_background: Hsla,
    /// Background Color. Used for the disabled state of an element that should have a different background than the surface it's on.
    ///
    /// Disabled states are shown when a user cannot interact with an element, like a disabled button or input.
    pub element_disabled: Hsla,
    /// Background Color. Used for the area that shows where a dragged element will be dropped.
    pub drop_target_background: Hsla,
    /// Border Color. Used for the border that shows where a dragged element will be dropped.
    pub drop_target_border: Hsla,
    /// Used for the background of a ghost element that should have the same background as the surface it's on.
    ///
    /// Elements might include: Buttons, Inputs, Checkboxes, Radio Buttons...
    ///
    /// For an element that should have a different background than the surface it's on, use `element_background`.
    pub ghost_element_background: Hsla,
    /// Background Color. Used for the hover state of a ghost element that should have the same background as the surface it's on.
    ///
    /// Hover states are triggered by the mouse entering an element, or a finger touching an element on a touch screen.
    pub ghost_element_hover: Hsla,
    /// Background Color. Used for the active state of a ghost element that should have the same background as the surface it's on.
    ///
    /// Active states are triggered by the mouse button being pressed down on an element, or the Return button or other activator being pressed.
    pub ghost_element_active: Hsla,
    /// Background Color. Used for the selected state of a ghost element that should have the same background as the surface it's on.
    ///
    /// Selected states are triggered by the element being selected (or "activated") by the user.
    ///
    /// This could include a selected checkbox, a toggleable button that is toggled on, etc.
    pub ghost_element_selected: Hsla,
    /// Background Color. Used for the disabled state of a ghost element that should have the same background as the surface it's on.
    ///
    /// Disabled states are shown when a user cannot interact with an element, like a disabled button or input.
    pub ghost_element_disabled: Hsla,
    /// Text Color. Default text color used for most text.
    pub text: Hsla,
    /// Text Color. Color of muted or deemphasized text. It is a subdued version of the standard text color.
    pub text_muted: Hsla,
    /// Text Color. Color of the placeholder text typically shown in input fields to guide the user to enter valid data.
    pub text_placeholder: Hsla,
    /// Text Color. Color used for text denoting disabled elements. Typically, the color is faded or grayed out to emphasize the disabled state.
    pub text_disabled: Hsla,
    /// Text Color. Color used for emphasis or highlighting certain text, like an active filter or a matched character in a search.
    pub text_accent: Hsla,
    /// Text Color. The faintest text that must still be readable: section labels, panel titles,
    /// table headers.
    ///
    /// Theme key `text.faint`; `text.muted` when the theme does not set it (the placeholder
    /// colour of most themes is too weak for a label).
    pub text_faint: Hsla,
    /// Background Color. The fill of the primary action of a view, and of the marker of the
    /// active or selected thing.
    ///
    /// Theme key `accent.background`; the local player's cursor colour when the theme does not
    /// set it.
    pub accent_background: Hsla,
    /// Background Color. The hover state of an element filled with `accent_background`.
    ///
    /// Theme key `accent.hover`; `accent_background` lightened (dark themes) or darkened (light
    /// themes) when the theme does not set it.
    pub accent_hover: Hsla,
    /// Text Color. Text and icons drawn on `accent_background`.
    ///
    /// Theme key `accent.foreground`; black or white, whichever contrasts more with
    /// `accent_background`, when the theme does not set it.
    pub accent_foreground: Hsla,
    /// Fill Color. Used for the default fill color of an icon.
    pub icon: Hsla,
    /// Fill Color. Used for the muted or deemphasized fill color of an icon.
    ///
    /// This might be used to show an icon in an inactive pane, or to deemphasize a series of icons to give them less visual weight.
    pub icon_muted: Hsla,
    /// Fill Color. Used for the disabled fill color of an icon.
    ///
    /// Disabled states are shown when a user cannot interact with an element, like a icon button.
    pub icon_disabled: Hsla,
    /// Fill Color. Used for the placeholder fill color of an icon.
    ///
    /// This might be used to show an icon in an input that disappears when the user enters text.
    pub icon_placeholder: Hsla,
    /// Fill Color. Used for the accent fill color of an icon.
    ///
    /// This might be used to show when a toggleable icon button is selected.
    pub icon_accent: Hsla,
    /// Color used to accent some debugger elements
    /// Is used by breakpoints
    pub debugger_accent: Hsla,

    // ===
    // UI Elements
    // ===
    pub status_bar_background: Hsla,
    pub title_bar_background: Hsla,
    pub title_bar_inactive_background: Hsla,
    pub toolbar_background: Hsla,
    pub tab_bar_background: Hsla,
    pub tab_inactive_background: Hsla,
    pub tab_active_background: Hsla,
    pub search_match_background: Hsla,
    pub search_active_match_background: Hsla,
    pub panel_background: Hsla,
    pub panel_focused_border: Hsla,
    pub panel_indent_guide: Hsla,
    pub panel_indent_guide_hover: Hsla,
    pub panel_indent_guide_active: Hsla,

    /// The color of the overlay surface on top of panel.
    pub panel_overlay_background: Hsla,
    /// The color of the overlay surface on top of panel when hovered over.
    pub panel_overlay_hover: Hsla,

    pub pane_focused_border: Hsla,
    pub pane_group_border: Hsla,
    /// The color of the scrollbar thumb.
    pub scrollbar_thumb_background: Hsla,
    /// The color of the scrollbar thumb when hovered over.
    pub scrollbar_thumb_hover_background: Hsla,
    /// The color of the scrollbar thumb whilst being actively dragged.
    pub scrollbar_thumb_active_background: Hsla,
    /// The border color of the scrollbar thumb.
    pub scrollbar_thumb_border: Hsla,
    /// The background color of the scrollbar track.
    pub scrollbar_track_background: Hsla,
    /// The border color of the scrollbar track.
    pub scrollbar_track_border: Hsla,
    /// The color of the minimap thumb.
    pub minimap_thumb_background: Hsla,
    /// The color of the minimap thumb when hovered over.
    pub minimap_thumb_hover_background: Hsla,
    /// The color of the minimap thumb whilst being actively dragged.
    pub minimap_thumb_active_background: Hsla,
    /// The border color of the minimap thumb.
    pub minimap_thumb_border: Hsla,

    /// Background color for Vim Normal mode indicator.
    pub vim_normal_background: Hsla,
    /// Background color for Vim Insert mode indicator.
    pub vim_insert_background: Hsla,
    /// Background color for Vim Replace mode indicator.
    pub vim_replace_background: Hsla,
    /// Background color for Vim Visual mode indicator.
    pub vim_visual_background: Hsla,
    /// Background color for Vim Visual Line mode indicator.
    pub vim_visual_line_background: Hsla,
    /// Background color for Vim Visual Block mode indicator.
    pub vim_visual_block_background: Hsla,
    /// Background color for Vim yank highlight.
    pub vim_yank_background: Hsla,
    /// Foreground color for Helix jump labels.
    pub vim_helix_jump_label_foreground: Hsla,
    /// Background color for Vim Helix Normal mode indicator.
    pub vim_helix_normal_background: Hsla,
    /// Background color for Vim Helix Select mode indicator.
    pub vim_helix_select_background: Hsla,
    /// Foreground color for Vim Normal mode indicator.
    pub vim_normal_foreground: Hsla,
    /// Foreground color for Vim Insert mode indicator.
    pub vim_insert_foreground: Hsla,
    /// Foreground color for Vim Replace mode indicator.
    pub vim_replace_foreground: Hsla,
    /// Foreground color for Vim Visual mode indicator.
    pub vim_visual_foreground: Hsla,
    /// Foreground color for Vim Visual Line mode indicator.
    pub vim_visual_line_foreground: Hsla,
    /// Foreground color for Vim Visual Block mode indicator.
    pub vim_visual_block_foreground: Hsla,
    /// Foreground color for Vim Helix Normal mode indicator.
    pub vim_helix_normal_foreground: Hsla,
    /// Foreground color for Vim Helix Select mode indicator.
    pub vim_helix_select_foreground: Hsla,

    // ===
    // Editor
    // ===
    pub editor_foreground: Hsla,
    /// Text color used for CodeLens items in the editor.
    ///
    /// Falls back to `text_muted` when not explicitly set.
    pub editor_code_lens_foreground: Option<Hsla>,
    pub editor_background: Hsla,
    pub editor_gutter_background: Hsla,
    pub editor_subheader_background: Hsla,
    pub editor_active_line_background: Hsla,
    pub editor_highlighted_line_background: Hsla,
    /// Line color of the line a debugger is currently stopped at
    pub editor_debugger_active_line_background: Hsla,
    /// Text Color. Used for the text of the line number in the editor gutter.
    pub editor_line_number: Hsla,
    /// Text Color. Used for the text of the line number in the editor gutter when the line is highlighted.
    pub editor_active_line_number: Hsla,
    /// Text Color. Used for the text of the line number in the editor gutter when the line is hovered over.
    pub editor_hover_line_number: Hsla,
    /// Text Color. Used to mark invisible characters in the editor.
    ///
    /// Example: spaces, tabs, carriage returns, etc.
    pub editor_invisible: Hsla,
    pub editor_wrap_guide: Hsla,
    pub editor_active_wrap_guide: Hsla,
    pub editor_indent_guide: Hsla,
    pub editor_indent_guide_active: Hsla,
    /// Read-access of a symbol, like reading a variable.
    ///
    /// A document highlight is a range inside a text document which deserves
    /// special attention. Usually a document highlight is visualized by changing
    /// the background color of its range.
    pub editor_document_highlight_read_background: Hsla,
    /// Read-access of a symbol, like reading a variable.
    ///
    /// A document highlight is a range inside a text document which deserves
    /// special attention. Usually a document highlight is visualized by changing
    /// the background color of its range.
    pub editor_document_highlight_write_background: Hsla,
    /// Highlighted brackets background color.
    ///
    /// Matching brackets in the cursor scope are highlighted with this background color.
    pub editor_document_highlight_bracket_background: Hsla,
    /// Filled background color for added diff hunk row highlights in the editor.
    pub editor_diff_hunk_added_background: Hsla,
    /// Hollow background color for added diff hunk row highlights in the editor.
    pub editor_diff_hunk_added_hollow_background: Hsla,
    /// Hollow border color for added diff hunk row highlights in the editor.
    pub editor_diff_hunk_added_hollow_border: Hsla,
    /// Filled background color for deleted diff hunk row highlights in the editor.
    pub editor_diff_hunk_deleted_background: Hsla,
    /// Hollow background color for deleted diff hunk row highlights in the editor.
    pub editor_diff_hunk_deleted_hollow_background: Hsla,
    /// Hollow border color for deleted diff hunk row highlights in the editor.
    pub editor_diff_hunk_deleted_hollow_border: Hsla,

    // ===
    // Terminal
    // ===
    /// Terminal layout background color.
    pub terminal_background: Hsla,
    /// Terminal foreground color.
    pub terminal_foreground: Hsla,
    /// Bright terminal foreground color.
    pub terminal_bright_foreground: Hsla,
    /// Dim terminal foreground color.
    pub terminal_dim_foreground: Hsla,
    /// Terminal ANSI background color.
    pub terminal_ansi_background: Hsla,
    /// Black ANSI terminal color.
    pub terminal_ansi_black: Hsla,
    /// Bright black ANSI terminal color.
    pub terminal_ansi_bright_black: Hsla,
    /// Dim black ANSI terminal color.
    pub terminal_ansi_dim_black: Hsla,
    /// Red ANSI terminal color.
    pub terminal_ansi_red: Hsla,
    /// Bright red ANSI terminal color.
    pub terminal_ansi_bright_red: Hsla,
    /// Dim red ANSI terminal color.
    pub terminal_ansi_dim_red: Hsla,
    /// Green ANSI terminal color.
    pub terminal_ansi_green: Hsla,
    /// Bright green ANSI terminal color.
    pub terminal_ansi_bright_green: Hsla,
    /// Dim green ANSI terminal color.
    pub terminal_ansi_dim_green: Hsla,
    /// Yellow ANSI terminal color.
    pub terminal_ansi_yellow: Hsla,
    /// Bright yellow ANSI terminal color.
    pub terminal_ansi_bright_yellow: Hsla,
    /// Dim yellow ANSI terminal color.
    pub terminal_ansi_dim_yellow: Hsla,
    /// Blue ANSI terminal color.
    pub terminal_ansi_blue: Hsla,
    /// Bright blue ANSI terminal color.
    pub terminal_ansi_bright_blue: Hsla,
    /// Dim blue ANSI terminal color.
    pub terminal_ansi_dim_blue: Hsla,
    /// Magenta ANSI terminal color.
    pub terminal_ansi_magenta: Hsla,
    /// Bright magenta ANSI terminal color.
    pub terminal_ansi_bright_magenta: Hsla,
    /// Dim magenta ANSI terminal color.
    pub terminal_ansi_dim_magenta: Hsla,
    /// Cyan ANSI terminal color.
    pub terminal_ansi_cyan: Hsla,
    /// Bright cyan ANSI terminal color.
    pub terminal_ansi_bright_cyan: Hsla,
    /// Dim cyan ANSI terminal color.
    pub terminal_ansi_dim_cyan: Hsla,
    /// White ANSI terminal color.
    pub terminal_ansi_white: Hsla,
    /// Bright white ANSI terminal color.
    pub terminal_ansi_bright_white: Hsla,
    /// Dim white ANSI terminal color.
    pub terminal_ansi_dim_white: Hsla,

    /// Represents a link text hover color.
    pub link_text_hover: Hsla,

    /// Represents an added entry or hunk in vcs, like git.
    pub version_control_added: Hsla,
    /// Represents a deleted entry in version control systems.
    pub version_control_deleted: Hsla,
    /// Represents a modified entry in version control systems.
    pub version_control_modified: Hsla,
    /// Represents a renamed entry in version control systems.
    pub version_control_renamed: Hsla,
    /// Represents a conflicting entry in version control systems.
    pub version_control_conflict: Hsla,
    /// Represents an ignored entry in version control systems.
    pub version_control_ignored: Hsla,
    /// Represents an added word in a word diff.
    pub version_control_word_added: Hsla,
    /// Represents a deleted word in a word diff.
    pub version_control_word_deleted: Hsla,
    /// Represents the "ours" region of a merge conflict.
    pub version_control_conflict_marker_ours: Hsla,
    /// Represents the "theirs" region of a merge conflict.
    pub version_control_conflict_marker_theirs: Hsla,
}

#[derive(EnumIter, Debug, Clone, Copy, AsRefStr)]
#[strum(serialize_all = "snake_case")]
pub enum ThemeColorField {
    Border,
    BorderVariant,
    BorderFocused,
    BorderSelected,
    BorderTransparent,
    BorderDisabled,
    BorderStrong,
    ElevatedSurfaceBackground,
    SurfaceBackground,
    Background,
    ElementBackground,
    ElementHover,
    ElementActive,
    ElementSelected,
    ElementDisabled,
    DropTargetBackground,
    DropTargetBorder,
    GhostElementBackground,
    GhostElementHover,
    GhostElementActive,
    GhostElementSelected,
    GhostElementDisabled,
    Text,
    TextMuted,
    TextPlaceholder,
    TextDisabled,
    TextAccent,
    TextFaint,
    AccentBackground,
    AccentHover,
    AccentForeground,
    Icon,
    IconMuted,
    IconDisabled,
    IconPlaceholder,
    IconAccent,
    StatusBarBackground,
    TitleBarBackground,
    TitleBarInactiveBackground,
    ToolbarBackground,
    TabBarBackground,
    TabInactiveBackground,
    TabActiveBackground,
    SearchMatchBackground,
    SearchActiveMatchBackground,
    PanelBackground,
    PanelFocusedBorder,
    PanelIndentGuide,
    PanelIndentGuideHover,
    PanelIndentGuideActive,
    PanelOverlayBackground,
    PanelOverlayHover,
    PaneFocusedBorder,
    PaneGroupBorder,
    ScrollbarThumbBackground,
    ScrollbarThumbHoverBackground,
    ScrollbarThumbActiveBackground,
    ScrollbarThumbBorder,
    ScrollbarTrackBackground,
    ScrollbarTrackBorder,
    MinimapThumbBackground,
    MinimapThumbHoverBackground,
    MinimapThumbActiveBackground,
    MinimapThumbBorder,
    EditorForeground,
    EditorCodeLensForeground,
    EditorBackground,
    EditorGutterBackground,
    EditorSubheaderBackground,
    EditorActiveLineBackground,
    EditorHighlightedLineBackground,
    EditorLineNumber,
    EditorActiveLineNumber,
    EditorInvisible,
    EditorWrapGuide,
    EditorActiveWrapGuide,
    EditorIndentGuide,
    EditorIndentGuideActive,
    EditorDocumentHighlightReadBackground,
    EditorDocumentHighlightWriteBackground,
    EditorDocumentHighlightBracketBackground,
    TerminalBackground,
    TerminalForeground,
    TerminalBrightForeground,
    TerminalDimForeground,
    TerminalAnsiBackground,
    TerminalAnsiBlack,
    TerminalAnsiBrightBlack,
    TerminalAnsiDimBlack,
    TerminalAnsiRed,
    TerminalAnsiBrightRed,
    TerminalAnsiDimRed,
    TerminalAnsiGreen,
    TerminalAnsiBrightGreen,
    TerminalAnsiDimGreen,
    TerminalAnsiYellow,
    TerminalAnsiBrightYellow,
    TerminalAnsiDimYellow,
    TerminalAnsiBlue,
    TerminalAnsiBrightBlue,
    TerminalAnsiDimBlue,
    TerminalAnsiMagenta,
    TerminalAnsiBrightMagenta,
    TerminalAnsiDimMagenta,
    TerminalAnsiCyan,
    TerminalAnsiBrightCyan,
    TerminalAnsiDimCyan,
    TerminalAnsiWhite,
    TerminalAnsiBrightWhite,
    TerminalAnsiDimWhite,
    LinkTextHover,
    VersionControlAdded,
    VersionControlDeleted,
    VersionControlModified,
    VersionControlRenamed,
    VersionControlConflict,
    VersionControlIgnored,
}

/// The tokens of [`ThemeColors`] that fall back to another colour of the theme when a theme file
/// does not state them. A `true` field means "derive this one".
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct DerivedColors {
    pub accent_background: bool,
    pub accent_hover: bool,
    pub accent_foreground: bool,
    pub border_strong: bool,
    pub text_faint: bool,
}

impl DerivedColors {
    /// Every derived token.
    pub fn all() -> Self {
        Self {
            accent_background: true,
            accent_hover: true,
            accent_foreground: true,
            border_strong: true,
            text_faint: true,
        }
    }

    /// The derived tokens that the given refinement does not state.
    pub fn unset_in(refinement: &ThemeColorsRefinement) -> Self {
        Self {
            accent_background: refinement.accent_background.is_none(),
            accent_hover: refinement.accent_hover.is_none(),
            accent_foreground: refinement.accent_foreground.is_none(),
            border_strong: refinement.border_strong.is_none(),
            text_faint: refinement.text_faint.is_none(),
        }
    }

    /// The tokens that are set in both.
    pub fn and(self, other: Self) -> Self {
        Self {
            accent_background: self.accent_background && other.accent_background,
            accent_hover: self.accent_hover && other.accent_hover,
            accent_foreground: self.accent_foreground && other.accent_foreground,
            border_strong: self.border_strong && other.border_strong,
            text_faint: self.text_faint && other.text_faint,
        }
    }
}

/// How much the hover of an accent fill moves in lightness: lighter on dark themes, darker on light.
const ACCENT_HOVER_LIGHTNESS_STEP: f32 = 0.08;

/// The relative luminance of a colour as defined by WCAG 2 (alpha is ignored).
fn relative_luminance(color: Hsla) -> f32 {
    fn linear(channel: f32) -> f32 {
        if channel <= 0.04045 {
            channel / 12.92
        } else {
            ((channel + 0.055) / 1.055).powf(2.4)
        }
    }
    let rgba = Rgba::from(color);
    0.2126 * linear(rgba.r) + 0.7152 * linear(rgba.g) + 0.0722 * linear(rgba.b)
}

/// The WCAG 2 contrast ratio between two opaque colours, from 1.0 to 21.0.
pub fn contrast_ratio(a: Hsla, b: Hsla) -> f32 {
    let a = relative_luminance(a);
    let b = relative_luminance(b);
    let (lighter, darker) = if a >= b { (a, b) } else { (b, a) };
    (lighter + 0.05) / (darker + 0.05)
}

impl ThemeColors {
    /// The hover state of an accent fill: lightened on dark themes, darkened on light ones.
    pub fn accent_hover_for(accent_background: Hsla, appearance: Appearance) -> Hsla {
        let mut hover = accent_background;
        hover.l = match appearance {
            Appearance::Dark => (hover.l + ACCENT_HOVER_LIGHTNESS_STEP).min(1.0),
            Appearance::Light => (hover.l - ACCENT_HOVER_LIGHTNESS_STEP).max(0.0),
        };
        hover
    }

    /// The text colour for an accent fill: black or white, whichever has the higher WCAG
    /// contrast with it.
    pub fn accent_foreground_for(accent_background: Hsla) -> Hsla {
        let white = gpui::white();
        let black = gpui::black();
        if contrast_ratio(black, accent_background) > contrast_ratio(white, accent_background) {
            black
        } else {
            white
        }
    }

    /// Which derived tokens currently hold exactly their fallback value.
    ///
    /// Used before applying an override, so that a token that was following another colour keeps
    /// following it when that colour is overridden.
    pub fn derived_colors_in_sync(
        &self,
        local_cursor: Hsla,
        appearance: Appearance,
    ) -> DerivedColors {
        DerivedColors {
            accent_background: self.accent_background == local_cursor,
            accent_hover: self.accent_hover
                == Self::accent_hover_for(self.accent_background, appearance),
            accent_foreground: self.accent_foreground
                == Self::accent_foreground_for(self.accent_background),
            border_strong: self.border_strong == self.border,
            text_faint: self.text_faint == self.text_muted,
        }
    }

    /// Fills the selected derived tokens from the colours they fall back to.
    ///
    /// `local_cursor` is the local player's cursor colour (`players[0].cursor`). Call this once
    /// when a theme is built, after the theme file's own values are applied: nothing is derived
    /// at render time.
    pub fn derive_colors(
        &mut self,
        which: DerivedColors,
        local_cursor: Hsla,
        appearance: Appearance,
    ) {
        if which.accent_background {
            self.accent_background = local_cursor;
        }
        if which.accent_hover {
            self.accent_hover = Self::accent_hover_for(self.accent_background, appearance);
        }
        if which.accent_foreground {
            self.accent_foreground = Self::accent_foreground_for(self.accent_background);
        }
        if which.border_strong {
            self.border_strong = self.border;
        }
        if which.text_faint {
            self.text_faint = self.text_muted;
        }
    }

    pub fn panel_background_for_overlay(&self) -> Hsla {
        if self.background.a >= 1.0 || self.panel_background.a >= 1.0 {
            self.background.blend(self.panel_background)
        } else {
            self.panel_overlay_background
        }
    }

    pub fn color(&self, field: ThemeColorField) -> Hsla {
        match field {
            ThemeColorField::Border => self.border,
            ThemeColorField::BorderVariant => self.border_variant,
            ThemeColorField::BorderFocused => self.border_focused,
            ThemeColorField::BorderSelected => self.border_selected,
            ThemeColorField::BorderTransparent => self.border_transparent,
            ThemeColorField::BorderDisabled => self.border_disabled,
            ThemeColorField::BorderStrong => self.border_strong,
            ThemeColorField::ElevatedSurfaceBackground => self.elevated_surface_background,
            ThemeColorField::SurfaceBackground => self.surface_background,
            ThemeColorField::Background => self.background,
            ThemeColorField::ElementBackground => self.element_background,
            ThemeColorField::ElementHover => self.element_hover,
            ThemeColorField::ElementActive => self.element_active,
            ThemeColorField::ElementSelected => self.element_selected,
            ThemeColorField::ElementDisabled => self.element_disabled,
            ThemeColorField::DropTargetBackground => self.drop_target_background,
            ThemeColorField::DropTargetBorder => self.drop_target_border,
            ThemeColorField::GhostElementBackground => self.ghost_element_background,
            ThemeColorField::GhostElementHover => self.ghost_element_hover,
            ThemeColorField::GhostElementActive => self.ghost_element_active,
            ThemeColorField::GhostElementSelected => self.ghost_element_selected,
            ThemeColorField::GhostElementDisabled => self.ghost_element_disabled,
            ThemeColorField::Text => self.text,
            ThemeColorField::TextMuted => self.text_muted,
            ThemeColorField::TextPlaceholder => self.text_placeholder,
            ThemeColorField::TextDisabled => self.text_disabled,
            ThemeColorField::TextAccent => self.text_accent,
            ThemeColorField::TextFaint => self.text_faint,
            ThemeColorField::AccentBackground => self.accent_background,
            ThemeColorField::AccentHover => self.accent_hover,
            ThemeColorField::AccentForeground => self.accent_foreground,
            ThemeColorField::Icon => self.icon,
            ThemeColorField::IconMuted => self.icon_muted,
            ThemeColorField::IconDisabled => self.icon_disabled,
            ThemeColorField::IconPlaceholder => self.icon_placeholder,
            ThemeColorField::IconAccent => self.icon_accent,
            ThemeColorField::StatusBarBackground => self.status_bar_background,
            ThemeColorField::TitleBarBackground => self.title_bar_background,
            ThemeColorField::TitleBarInactiveBackground => self.title_bar_inactive_background,
            ThemeColorField::ToolbarBackground => self.toolbar_background,
            ThemeColorField::TabBarBackground => self.tab_bar_background,
            ThemeColorField::TabInactiveBackground => self.tab_inactive_background,
            ThemeColorField::TabActiveBackground => self.tab_active_background,
            ThemeColorField::SearchMatchBackground => self.search_match_background,
            ThemeColorField::SearchActiveMatchBackground => self.search_active_match_background,
            ThemeColorField::PanelBackground => self.panel_background,
            ThemeColorField::PanelFocusedBorder => self.panel_focused_border,
            ThemeColorField::PanelIndentGuide => self.panel_indent_guide,
            ThemeColorField::PanelIndentGuideHover => self.panel_indent_guide_hover,
            ThemeColorField::PanelIndentGuideActive => self.panel_indent_guide_active,
            ThemeColorField::PanelOverlayBackground => self.panel_overlay_background,
            ThemeColorField::PanelOverlayHover => self.panel_overlay_hover,
            ThemeColorField::PaneFocusedBorder => self.pane_focused_border,
            ThemeColorField::PaneGroupBorder => self.pane_group_border,
            ThemeColorField::ScrollbarThumbBackground => self.scrollbar_thumb_background,
            ThemeColorField::ScrollbarThumbHoverBackground => self.scrollbar_thumb_hover_background,
            ThemeColorField::ScrollbarThumbActiveBackground => {
                self.scrollbar_thumb_active_background
            }
            ThemeColorField::ScrollbarThumbBorder => self.scrollbar_thumb_border,
            ThemeColorField::ScrollbarTrackBackground => self.scrollbar_track_background,
            ThemeColorField::ScrollbarTrackBorder => self.scrollbar_track_border,
            ThemeColorField::MinimapThumbBackground => self.minimap_thumb_background,
            ThemeColorField::MinimapThumbHoverBackground => self.minimap_thumb_hover_background,
            ThemeColorField::MinimapThumbActiveBackground => self.minimap_thumb_active_background,
            ThemeColorField::MinimapThumbBorder => self.minimap_thumb_border,
            ThemeColorField::EditorForeground => self.editor_foreground,
            ThemeColorField::EditorCodeLensForeground => {
                self.editor_code_lens_foreground.unwrap_or(self.text_muted)
            }
            ThemeColorField::EditorBackground => self.editor_background,
            ThemeColorField::EditorGutterBackground => self.editor_gutter_background,
            ThemeColorField::EditorSubheaderBackground => self.editor_subheader_background,
            ThemeColorField::EditorActiveLineBackground => self.editor_active_line_background,
            ThemeColorField::EditorHighlightedLineBackground => {
                self.editor_highlighted_line_background
            }
            ThemeColorField::EditorLineNumber => self.editor_line_number,
            ThemeColorField::EditorActiveLineNumber => self.editor_active_line_number,
            ThemeColorField::EditorInvisible => self.editor_invisible,
            ThemeColorField::EditorWrapGuide => self.editor_wrap_guide,
            ThemeColorField::EditorActiveWrapGuide => self.editor_active_wrap_guide,
            ThemeColorField::EditorIndentGuide => self.editor_indent_guide,
            ThemeColorField::EditorIndentGuideActive => self.editor_indent_guide_active,
            ThemeColorField::EditorDocumentHighlightReadBackground => {
                self.editor_document_highlight_read_background
            }
            ThemeColorField::EditorDocumentHighlightWriteBackground => {
                self.editor_document_highlight_write_background
            }
            ThemeColorField::EditorDocumentHighlightBracketBackground => {
                self.editor_document_highlight_bracket_background
            }
            ThemeColorField::TerminalBackground => self.terminal_background,
            ThemeColorField::TerminalForeground => self.terminal_foreground,
            ThemeColorField::TerminalBrightForeground => self.terminal_bright_foreground,
            ThemeColorField::TerminalDimForeground => self.terminal_dim_foreground,
            ThemeColorField::TerminalAnsiBackground => self.terminal_ansi_background,
            ThemeColorField::TerminalAnsiBlack => self.terminal_ansi_black,
            ThemeColorField::TerminalAnsiBrightBlack => self.terminal_ansi_bright_black,
            ThemeColorField::TerminalAnsiDimBlack => self.terminal_ansi_dim_black,
            ThemeColorField::TerminalAnsiRed => self.terminal_ansi_red,
            ThemeColorField::TerminalAnsiBrightRed => self.terminal_ansi_bright_red,
            ThemeColorField::TerminalAnsiDimRed => self.terminal_ansi_dim_red,
            ThemeColorField::TerminalAnsiGreen => self.terminal_ansi_green,
            ThemeColorField::TerminalAnsiBrightGreen => self.terminal_ansi_bright_green,
            ThemeColorField::TerminalAnsiDimGreen => self.terminal_ansi_dim_green,
            ThemeColorField::TerminalAnsiYellow => self.terminal_ansi_yellow,
            ThemeColorField::TerminalAnsiBrightYellow => self.terminal_ansi_bright_yellow,
            ThemeColorField::TerminalAnsiDimYellow => self.terminal_ansi_dim_yellow,
            ThemeColorField::TerminalAnsiBlue => self.terminal_ansi_blue,
            ThemeColorField::TerminalAnsiBrightBlue => self.terminal_ansi_bright_blue,
            ThemeColorField::TerminalAnsiDimBlue => self.terminal_ansi_dim_blue,
            ThemeColorField::TerminalAnsiMagenta => self.terminal_ansi_magenta,
            ThemeColorField::TerminalAnsiBrightMagenta => self.terminal_ansi_bright_magenta,
            ThemeColorField::TerminalAnsiDimMagenta => self.terminal_ansi_dim_magenta,
            ThemeColorField::TerminalAnsiCyan => self.terminal_ansi_cyan,
            ThemeColorField::TerminalAnsiBrightCyan => self.terminal_ansi_bright_cyan,
            ThemeColorField::TerminalAnsiDimCyan => self.terminal_ansi_dim_cyan,
            ThemeColorField::TerminalAnsiWhite => self.terminal_ansi_white,
            ThemeColorField::TerminalAnsiBrightWhite => self.terminal_ansi_bright_white,
            ThemeColorField::TerminalAnsiDimWhite => self.terminal_ansi_dim_white,
            ThemeColorField::LinkTextHover => self.link_text_hover,
            ThemeColorField::VersionControlAdded => self.version_control_added,
            ThemeColorField::VersionControlDeleted => self.version_control_deleted,
            ThemeColorField::VersionControlModified => self.version_control_modified,
            ThemeColorField::VersionControlRenamed => self.version_control_renamed,
            ThemeColorField::VersionControlConflict => self.version_control_conflict,
            ThemeColorField::VersionControlIgnored => self.version_control_ignored,
        }
    }

    pub fn iter(&self) -> impl Iterator<Item = (ThemeColorField, Hsla)> + '_ {
        ThemeColorField::iter().map(move |field| (field, self.color(field)))
    }

    pub fn to_vec(&self) -> Vec<(ThemeColorField, Hsla)> {
        self.iter().collect()
    }
}

pub fn all_theme_colors(cx: &mut App) -> Vec<(Hsla, SharedString)> {
    let theme = cx.theme();
    ThemeColorField::iter()
        .map(|field| {
            let color = theme.colors().color(field);
            let name = field.as_ref().to_string();
            (color, SharedString::from(name))
        })
        .collect()
}

/// How round a theme draws the corners of buttons, inputs, menus and other surfaces.
///
/// It is applied through [`gpui::set_corner_radius_scale`] when the theme becomes active.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash)]
pub enum CornerStyle {
    /// No rounding: every named radius is zero.
    Square,
    /// Half of the usual rounding.
    Soft,
    /// The usual rounding. This is what a theme gets when it does not say.
    #[default]
    Round,
}

impl CornerStyle {
    /// The factor applied to the named corner radii for this style.
    pub fn scale(self) -> f32 {
        match self {
            Self::Square => 0.0,
            Self::Soft => 0.5,
            Self::Round => 1.0,
        }
    }
}

#[derive(Refineable, Clone, Debug, PartialEq)]
pub struct ThemeStyles {
    /// The background appearance of the window.
    pub window_background_appearance: WindowBackgroundAppearance,
    /// How round the corners of buttons, inputs, menus and other surfaces are.
    pub corners: CornerStyle,
    pub system: SystemColors,
    /// An array of colors used for theme elements that iterate through a series of colors.
    ///
    /// Example: Player colors, rainbow brackets and indent guides, etc.
    pub accents: AccentColors,

    #[refineable]
    pub colors: ThemeColors,

    #[refineable]
    pub status: StatusColors,

    pub player: PlayerColors,

    pub syntax: Arc<SyntaxTheme>,
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::*;

    #[test]
    fn panel_background_for_overlay() {
        let mut colors = ThemeColors::light();
        colors.panel_overlay_background = Hsla::from(gpui::rgb(0xff0000));

        for (panel, background, expected) in [
            (0x58585a00, 0xdcdcddff, 0xdcdcddff),
            (0x00000080, 0xdcdcddff, 0x6d6d6eff),
            (0xebebecff, 0x24252900, 0xebebecff),
            (0xebebec00, 0x24252900, 0xff0000ff),
            (0xebebec00, 0x24252980, 0xff0000ff),
            (0x24252980, 0x24252980, 0xff0000ff),
        ] {
            colors.panel_background = Hsla::from(gpui::rgba(panel));
            colors.background = Hsla::from(gpui::rgba(background));
            assert_eq!(
                u32::from(Rgba::from(colors.panel_background_for_overlay())),
                expected,
                "panel {panel:#010x}, background {background:#010x}"
            );
        }
    }

    #[test]
    fn corner_style_drives_the_named_radii() {
        // The only test of this crate that touches the process-wide scale.
        assert_eq!(CornerStyle::default(), CornerStyle::Round);
        assert_eq!(gpui::corner_radius_scale(), 1.0);
        assert_eq!(gpui::scaled_radius(0.25), gpui::rems(0.25));

        gpui::set_corner_radius_scale(CornerStyle::Soft.scale());
        assert_eq!(gpui::scaled_radius(0.25), gpui::rems(0.125));

        gpui::set_corner_radius_scale(CornerStyle::Square.scale());
        assert_eq!(gpui::scaled_radius(1.5), gpui::rems(0.));

        gpui::set_corner_radius_scale(f32::NAN);
        assert_eq!(gpui::corner_radius_scale(), 0.0);
        gpui::set_corner_radius_scale(-1.0);
        assert_eq!(gpui::corner_radius_scale(), 0.0);

        gpui::set_corner_radius_scale(CornerStyle::Round.scale());
        assert_eq!(gpui::corner_radius_scale(), 1.0);
    }

    #[test]
    fn default_colors_resolve_the_derived_tokens() {
        for (colors, appearance, cursor) in [
            (
                ThemeColors::dark(),
                Appearance::Dark,
                PlayerColors::dark().local().cursor,
            ),
            (
                ThemeColors::light(),
                Appearance::Light,
                PlayerColors::light().local().cursor,
            ),
        ] {
            assert_eq!(
                colors.derived_colors_in_sync(cursor, appearance),
                DerivedColors::all()
            );
        }

        let fallback = crate::fallback_themes::zed_default_dark();
        assert_eq!(
            fallback
                .colors()
                .derived_colors_in_sync(fallback.players().local().cursor, Appearance::Dark),
            DerivedColors::all()
        );
        assert_eq!(fallback.corners(), CornerStyle::Round);
    }

    #[test]
    fn contrast_ratio_matches_wcag() {
        let white = gpui::white();
        let black = gpui::black();
        assert!((contrast_ratio(white, black) - 21.0).abs() < 0.01);
        assert!((contrast_ratio(black, white) - 21.0).abs() < 0.01);
        assert!((contrast_ratio(white, white) - 1.0).abs() < 0.001);

        // White on the brand red #e5322d is about 4.4:1 (below AA), black about 4.8:1.
        let red: Hsla = gpui::rgb(0xe5322d).into();
        let on_white = contrast_ratio(white, red);
        let on_black = contrast_ratio(black, red);
        assert!((4.2..4.6).contains(&on_white), "{on_white}");
        assert!((4.6..5.0).contains(&on_black), "{on_black}");
        assert_eq!(ThemeColors::accent_foreground_for(red), black);
    }

    #[test]
    fn override_a_single_theme_color() {
        let mut colors = ThemeColors::light();

        let magenta: Hsla = gpui::rgb(0xff00ff).into();

        assert_ne!(colors.text, magenta);

        let overrides = ThemeColorsRefinement {
            text: Some(magenta),
            ..Default::default()
        };

        colors.refine(&overrides);

        assert_eq!(colors.text, magenta);
    }

    #[test]
    fn override_multiple_theme_colors() {
        let mut colors = ThemeColors::light();

        let magenta: Hsla = gpui::rgb(0xff00ff).into();
        let green: Hsla = gpui::rgb(0x00ff00).into();

        assert_ne!(colors.text, magenta);
        assert_ne!(colors.background, green);

        let overrides = ThemeColorsRefinement {
            text: Some(magenta),
            background: Some(green),
            ..Default::default()
        };

        colors.refine(&overrides);

        assert_eq!(colors.text, magenta);
        assert_eq!(colors.background, green);
    }

    #[test]
    fn deserialize_theme_colors_refinement_from_json() {
        let colors: ThemeColorsRefinement = serde_json::from_value(json!({
            "background": "#ff00ff",
            "text": "#ff0000"
        }))
        .unwrap();

        assert_eq!(colors.background, Some(gpui::rgb(0xff00ff).into()));
        assert_eq!(colors.text, Some(gpui::rgb(0xff0000).into()));
    }
}

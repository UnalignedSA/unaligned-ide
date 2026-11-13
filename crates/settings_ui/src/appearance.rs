//! The controls of the Appearance page that are not a plain dropdown, switch
//! or field: the theme tiles, the segmented switches (mode, corners, density)
//! and the accent swatches.

use gpui::{
    AnyElement, App, Context, Div, Hsla, ReadGlobal as _, Rgba, Role, SharedString, Stateful,
    Window,
};
use settings::{SettingsStore, ThemeAppearanceMode, ThemeName, ThemeSelection};
use theme::{SystemAppearance, ThemeRegistry};
use ui::{ToggleButtonGroup, ToggleButtonGroupStyle, ToggleButtonSimple, Tooltip, prelude::*};
use util::ResultExt as _;

use crate::{
    SettingField, SettingItem, SettingsFieldMetadata, SettingsUiFile, SettingsWindow,
    components::render_theme_grid, get_current_value, render_settings_item, update_settings_file,
};

/// The accent colours offered next to "Theme Default". These are values of the
/// `accent` setting, not colours of the interface.
const ACCENT_SWATCHES: [(&str, &str); 6] = [
    ("Red", "#e5322d"),
    ("Azure", "#2f6fe0"),
    ("Green", "#3f9a5a"),
    ("Amber", "#c98a1a"),
    ("Violet", "#8a5fd0"),
    ("Graphite", "#8a857d"),
];

/// Whether a click on a tile writes the light slot. `None`: one fixed theme.
fn editing_light_slot(selection: &ThemeSelection, system_is_light: bool) -> Option<bool> {
    match selection {
        ThemeSelection::Static(_) => None,
        ThemeSelection::Dynamic { mode, .. } => Some(match mode {
            ThemeAppearanceMode::Light => true,
            ThemeAppearanceMode::Dark => false,
            ThemeAppearanceMode::System => system_is_light,
        }),
    }
}

/// The theme the selection shows right now.
fn theme_in_use(selection: &ThemeSelection, system_is_light: bool) -> &ThemeName {
    match selection {
        ThemeSelection::Static(name) => name,
        ThemeSelection::Dynamic { light, dark, .. } => {
            if editing_light_slot(selection, system_is_light) == Some(true) {
                light
            } else {
                dark
            }
        }
    }
}

/// The selection with `name` in the slot in use; the other slot and the mode are kept.
pub(crate) fn with_theme_in_use(
    selection: Option<ThemeSelection>,
    name: ThemeName,
    system_is_light: bool,
) -> ThemeSelection {
    match selection {
        None | Some(ThemeSelection::Static(_)) => ThemeSelection::Static(name),
        Some(selection @ ThemeSelection::Dynamic { .. }) => {
            let write_light = editing_light_slot(&selection, system_is_light) == Some(true);
            let ThemeSelection::Dynamic { mode, light, dark } = selection else {
                unreachable!("matched as dynamic above")
            };
            if write_light {
                ThemeSelection::Dynamic {
                    mode,
                    light: name,
                    dark,
                }
            } else {
                ThemeSelection::Dynamic {
                    mode,
                    light,
                    dark: name,
                }
            }
        }
    }
}

/// Writes the light / dark / system mode, keeping both theme slots. A fixed
/// theme becomes a pair: it keeps its own slot, the family default takes the other.
pub(crate) fn write_theme_mode(
    selection: &mut Option<ThemeSelection>,
    mode: ThemeAppearanceMode,
    is_light_theme: impl Fn(&str) -> bool,
) {
    let default_light = || ThemeName(settings::DEFAULT_LIGHT_THEME.into());
    let default_dark = || ThemeName(settings::DEFAULT_DARK_THEME.into());
    *selection = Some(match selection.take() {
        Some(ThemeSelection::Dynamic { light, dark, .. }) => {
            ThemeSelection::Dynamic { mode, light, dark }
        }
        Some(ThemeSelection::Static(name)) => {
            if is_light_theme(&name.0) {
                ThemeSelection::Dynamic {
                    mode,
                    light: name,
                    dark: default_dark(),
                }
            } else {
                ThemeSelection::Dynamic {
                    mode,
                    light: default_light(),
                    dark: name,
                }
            }
        }
        None => ThemeSelection::Dynamic {
            mode,
            light: default_light(),
            dark: default_dark(),
        },
    });
}

/// The "Theme" row: title and description as any row, then the grid of tiles
/// across the whole width.
pub(crate) fn render_theme_tiles_item(
    settings_window: &SettingsWindow,
    item: &SettingItem,
    field: SettingField<ThemeSelection>,
    file: SettingsUiFile,
    _metadata: Option<&SettingsFieldMetadata>,
    sub_field: bool,
    window: &mut Window,
    cx: &mut Context<SettingsWindow>,
) -> Stateful<Div> {
    let (_, selection) =
        SettingsStore::global(cx).get_value_from_file(file.to_settings(), field.pick);
    let selection = selection.cloned();
    let system_is_light = SystemAppearance::global(cx).is_light();

    let current: SharedString = selection
        .as_ref()
        .map(|selection| SharedString::from(theme_in_use(selection, system_is_light).0.clone()))
        .unwrap_or_else(|| cx.theme().name.clone());

    // Says which of the two slots a click fills, so that choosing a theme
    // while in dark mode is not a surprise the day the system turns light.
    let slot_hint = selection
        .as_ref()
        .and_then(|selection| editing_light_slot(selection, system_is_light))
        .map(|light| {
            if light {
                "Used in light mode"
            } else {
                "Used in dark mode"
            }
        });
    let control = match slot_hint {
        Some(hint) => Label::new(hint)
            .size(LabelSize::Small)
            .color(Color::Muted)
            .into_any_element(),
        None => div().into_any_element(),
    };

    let header = render_settings_item(settings_window, item, file.clone(), control, sub_field, cx);

    let grid = render_theme_grid(
        current,
        move |name, window, cx| {
            let system_is_light = SystemAppearance::global(cx).is_light();
            let new_selection = with_theme_in_use(
                selection.clone(),
                ThemeName(name.as_ref().into()),
                system_is_light,
            );
            update_settings_file(
                file.clone(),
                field.json_path,
                window,
                cx,
                move |settings, app| {
                    (field.write)(settings, Some(new_selection), app);
                },
            )
            .log_err();
        },
        window,
        cx,
    );

    v_flex()
        .id("theme-tiles")
        .w_full()
        .gap_3()
        .child(header)
        .child(grid)
}

/// A short fixed choice shown whole, as a segmented switch: the current
/// option sits on the selected ground, never on the accent.
pub(crate) fn render_segmented<T, const N: usize>(
    options: [(T, &'static str); N],
    field: SettingField<T>,
    file: SettingsUiFile,
    title: &'static str,
    cx: &mut App,
) -> AnyElement
where
    T: Copy + PartialEq + Send + Sync + 'static,
{
    let current_value = get_current_value(&SettingsStore::global(cx), &file, &field, cx);
    let (current_value, disabled) = current_value
        .map(|current| (Some(*current.value), current.disabled))
        .unwrap_or((None, false));
    let selected_index = options
        .iter()
        .position(|(value, _)| Some(*value) == current_value)
        .unwrap_or(usize::MAX);

    let mut tab_index = 0_isize;
    ToggleButtonGroup::single_row(
        SharedString::new_static(title),
        options.map(|(value, label)| {
            let file = file.clone();
            ToggleButtonSimple::new(label, move |_, window, cx| {
                if disabled || Some(value) == current_value {
                    return;
                }
                update_settings_file(
                    file.clone(),
                    field.json_path,
                    window,
                    cx,
                    move |settings, app| {
                        (field.write)(settings, Some(value), app);
                    },
                )
                .log_err();
            })
        }),
    )
    .style(ToggleButtonGroupStyle::Outlined)
    .auto_width()
    .selected_index(selected_index)
    .tab_index(&mut tab_index)
    .into_any_element()
}

/// The "Accent" row: "Theme Default", then one swatch per offered colour.
pub(crate) fn render_accent_swatches(
    field: SettingField<settings::AccentSetting>,
    file: SettingsUiFile,
    _metadata: Option<&SettingsFieldMetadata>,
    title: &'static str,
    _description: &'static str,
    _window: &mut Window,
    cx: &mut App,
) -> AnyElement {
    let current_value = get_current_value(&SettingsStore::global(cx), &file, &field, cx);
    let (current_color, disabled) = current_value
        .map(|current| {
            (
                current.value.custom_color().map(str::to_ascii_lowercase),
                current.disabled,
            )
        })
        .unwrap_or((None, false));

    let write = move |value: &'static str, window: &mut Window, cx: &mut App| {
        if disabled {
            return;
        }
        update_settings_file(
            file.clone(),
            field.json_path,
            window,
            cx,
            move |settings, app| {
                (field.write)(
                    settings,
                    Some(settings::AccentSetting(value.to_string())),
                    app,
                );
            },
        )
        .log_err();
    };

    let follows_theme = current_color.is_none();
    let mut tab_index = 0_isize;
    let theme_default = ToggleButtonGroup::single_row(
        SharedString::new_static(title),
        [ToggleButtonSimple::new("Theme Default", {
            let write = write.clone();
            move |_, window, cx| write(settings::AccentSetting::THEME, window, cx)
        })],
    )
    .style(ToggleButtonGroupStyle::Outlined)
    .auto_width()
    .selected_index(if follows_theme { 0 } else { usize::MAX })
    .tab_index(&mut tab_index);

    let ring = cx.theme().colors().text;
    let line = cx.theme().colors().border_strong;
    let swatches = ACCENT_SWATCHES.into_iter().map(|(name, hex)| {
        let selected = current_color.as_deref() == Some(hex);
        let fill: Hsla = Rgba::try_from(hex)
            .map(Hsla::from)
            .unwrap_or_else(|_| cx.theme().colors().accent_background);
        let write = write.clone();
        // The ring of the chosen swatch is the text colour: the swatch itself
        // is the accent, a ring in the accent would not show on it.
        div()
            .id(SharedString::new_static(name))
            .role(Role::Button)
            .aria_label(format!("{name} accent"))
            .tab_index(0_isize)
            .cursor_pointer()
            .p(px(2.))
            .border_2()
            .rounded_md()
            .border_color(if selected {
                ring
            } else {
                gpui::transparent_black()
            })
            .focus_visible(|style| style.border_color(line))
            .child(
                div()
                    .size(px(18.))
                    .rounded_sm()
                    .border_1()
                    .border_color(line)
                    .bg(fill),
            )
            .tooltip(Tooltip::text(name))
            .on_click(move |_, window, cx| write(hex, window, cx))
    });

    h_flex()
        .gap_2()
        .child(theme_default)
        .child(h_flex().gap_0p5().children(swatches))
        .into_any_element()
}

/// Whether a registered theme is a light one (an unknown name counts as dark).
pub(crate) fn is_light_theme(name: &str, cx: &App) -> bool {
    ThemeRegistry::global(cx)
        .get(name)
        .is_ok_and(|theme| theme.appearance().is_light())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn name(name: &str) -> ThemeName {
        ThemeName(name.into())
    }

    fn pair(mode: ThemeAppearanceMode) -> ThemeSelection {
        ThemeSelection::Dynamic {
            mode,
            light: name("Unaligned Light"),
            dark: name("Unaligned"),
        }
    }

    #[test]
    fn a_tile_fills_the_slot_in_use_and_keeps_the_other() {
        // Dark mode: the dark slot, whatever the system says.
        assert_eq!(
            with_theme_in_use(
                Some(pair(ThemeAppearanceMode::Dark)),
                name("One Dark"),
                true
            ),
            ThemeSelection::Dynamic {
                mode: ThemeAppearanceMode::Dark,
                light: name("Unaligned Light"),
                dark: name("One Dark"),
            }
        );
        // System mode on a light system: the light slot.
        assert_eq!(
            with_theme_in_use(
                Some(pair(ThemeAppearanceMode::System)),
                name("One Light"),
                true
            ),
            ThemeSelection::Dynamic {
                mode: ThemeAppearanceMode::System,
                light: name("One Light"),
                dark: name("Unaligned"),
            }
        );
        // A fixed theme stays a fixed theme.
        assert_eq!(
            with_theme_in_use(
                Some(ThemeSelection::Static(name("Ayu Dark"))),
                name("One Dark"),
                false
            ),
            ThemeSelection::Static(name("One Dark"))
        );
    }

    #[test]
    fn the_mode_switch_keeps_both_slots() {
        let mut selection = Some(ThemeSelection::Dynamic {
            mode: ThemeAppearanceMode::Dark,
            light: name("One Light"),
            dark: name("Unaligned Midnight"),
        });
        write_theme_mode(&mut selection, ThemeAppearanceMode::System, |_| false);
        assert_eq!(
            selection,
            Some(ThemeSelection::Dynamic {
                mode: ThemeAppearanceMode::System,
                light: name("One Light"),
                dark: name("Unaligned Midnight"),
            })
        );

        // A fixed light theme keeps the light slot; the family fills the dark one.
        let mut selection = Some(ThemeSelection::Static(name("One Light")));
        write_theme_mode(&mut selection, ThemeAppearanceMode::Light, |name| {
            name == "One Light"
        });
        assert_eq!(
            selection,
            Some(ThemeSelection::Dynamic {
                mode: ThemeAppearanceMode::Light,
                light: name("One Light"),
                dark: name("Unaligned"),
            })
        );

        let mut selection = None;
        write_theme_mode(&mut selection, ThemeAppearanceMode::Light, |_| false);
        assert_eq!(selection, Some(pair(ThemeAppearanceMode::Light)));
    }
}

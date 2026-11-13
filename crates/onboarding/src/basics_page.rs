//! The three sections of the first-launch page: look, AI and keys.

use std::sync::Arc;

use fs::Fs;
use gpui::{
    Action, App, Context, FocusHandle, IntoElement, Role, StyleRefinement, UpdateGlobal as _,
    Window,
};
use project::DisableAiSettings;
use settings::{BaseKeymap, Settings, SettingsStore, update_settings_file};
use theme::{Appearance, SystemAppearance, Theme, ThemeRegistry};
use theme_settings::{ThemeAppearanceMode, ThemeName, ThemeSelection, ThemeSettings};
use ui::{
    SectionLabel, SwitchField, ToggleButtonGroup, ToggleButtonGroupSize, ToggleButtonSimple,
    focus_ring, prelude::*,
};
use vim_mode_setting::VimModeSetting;
use workspace::welcome::AiConnection;

use crate::{
    ImportCursorSettings, ImportVsCodeSettings, Onboarding, SettingsImportState,
    base_keymap_picker::{self, ToggleBaseKeymapSelector},
    theme_preview::{ThemePreviewStyle, ThemePreviewTile},
};

/// The product's own themes, in the order of the tiles. One that is missing from
/// the registry is skipped.
pub(crate) const FAMILY_THEMES: [&str; 5] = [
    "Unaligned",
    "Unaligned Light",
    "Unaligned Dusk",
    "Unaligned Midnight",
    "Unaligned Contrast",
];

/// The keymaps offered as segments; the others are behind "Other…".
const KEYMAP_CHOICES: [(&str, BaseKeymap); 4] = [
    ("VS Code", BaseKeymap::VSCode),
    ("JetBrains", BaseKeymap::JetBrains),
    ("Sublime Text", BaseKeymap::SublimeText),
    ("Cursor", BaseKeymap::Cursor),
];

const AI_DETAIL: &str =
    "Connect Unaligned, a subscription you already have, your own key or a local model.";
// Defined in the agent crate, which depends on this one: built by name.
const MANAGE_CONNECTIONS_ACTION: &str = "agent::ManageProviders";

fn appearance_mode(appearance: Appearance) -> ThemeAppearanceMode {
    match appearance {
        Appearance::Light => ThemeAppearanceMode::Light,
        Appearance::Dark => ThemeAppearanceMode::Dark,
    }
}

/// The light and dark themes of a selection. A single theme stands on its own side
/// (when its appearance is known); the other side is the family's default.
fn light_and_dark(
    selection: &ThemeSelection,
    static_appearance: Option<Appearance>,
) -> (ThemeName, ThemeName) {
    let default_light = ThemeName(settings::DEFAULT_LIGHT_THEME.into());
    let default_dark = ThemeName(settings::DEFAULT_DARK_THEME.into());
    match selection {
        ThemeSelection::Dynamic { light, dark, .. } => (light.clone(), dark.clone()),
        ThemeSelection::Static(name) => match static_appearance {
            Some(Appearance::Light) => (name.clone(), default_dark),
            Some(Appearance::Dark) => (default_light, name.clone()),
            None => (default_light, default_dark),
        },
    }
}

/// The selection once the theme `name` has been picked: it takes the side of its
/// appearance and is the one on screen; the other side is kept.
pub(crate) fn selection_after_pick(
    current: &ThemeSelection,
    static_appearance: Option<Appearance>,
    name: &str,
    appearance: Appearance,
    system_appearance: Appearance,
) -> ThemeSelection {
    let (mut light, mut dark) = light_and_dark(current, static_appearance);
    match appearance {
        Appearance::Light => light = ThemeName(name.into()),
        Appearance::Dark => dark = ThemeName(name.into()),
    }
    let follows_system =
        current.mode() == Some(ThemeAppearanceMode::System) && appearance == system_appearance;
    ThemeSelection::Dynamic {
        mode: if follows_system {
            ThemeAppearanceMode::System
        } else {
            appearance_mode(appearance)
        },
        light,
        dark,
    }
}

/// The selection once Light, Dark or System has been chosen: both themes are kept,
/// so the switch never leaves the family the user is in.
pub(crate) fn selection_after_mode(
    current: &ThemeSelection,
    static_appearance: Option<Appearance>,
    mode: ThemeAppearanceMode,
) -> ThemeSelection {
    let (light, dark) = light_and_dark(current, static_appearance);
    ThemeSelection::Dynamic { mode, light, dark }
}

fn to_settings_content(selection: ThemeSelection) -> settings::ThemeSelection {
    match selection {
        ThemeSelection::Static(name) => settings::ThemeSelection::Static(name),
        ThemeSelection::Dynamic { mode, light, dark } => {
            settings::ThemeSelection::Dynamic { mode, light, dark }
        }
    }
}

fn write_theme_selection(selection: ThemeSelection, cx: &mut App) {
    let fs = <dyn Fs>::global(cx);
    update_settings_file(fs, cx, move |settings, _| {
        settings.theme.theme = Some(to_settings_content(selection));
    });
}

fn write_keymap_base(keymap_base: BaseKeymap, cx: &App) {
    let fs = <dyn Fs>::global(cx);
    update_settings_file(fs, cx, move |setting, _| {
        setting.base_keymap = Some(keymap_base.into());
    });
}

/// The appearance of the theme a static selection names, when the registry has it.
fn static_appearance(selection: &ThemeSelection, cx: &App) -> Option<Appearance> {
    match selection {
        ThemeSelection::Static(name) => ThemeRegistry::global(cx)
            .get(&name.0)
            .ok()
            .map(|theme| theme.appearance()),
        ThemeSelection::Dynamic { .. } => None,
    }
}

impl Onboarding {
    /// The theme selection the user has chosen: what the settings say. A tile
    /// shows its theme in miniature; the application changes theme on a click
    /// only. (It used to follow the pointer and the focus too, and a preview
    /// that was never ended stayed on screen until the next settings reload.)
    fn chosen_theme_selection(&self, cx: &App) -> ThemeSelection {
        ThemeSettings::get_global(cx).theme.clone()
    }

    fn family_theme(index: usize, cx: &App) -> Option<Arc<Theme>> {
        let name = FAMILY_THEMES.get(index)?;
        ThemeRegistry::global(cx).get(name).ok()
    }

    /// Keeps the theme of a tile: written to the settings file.
    fn pick_theme(&mut self, index: usize, cx: &mut Context<Self>) {
        let Some(theme) = Self::family_theme(index, cx) else {
            return;
        };
        let chosen = self.chosen_theme_selection(cx);
        let selection = selection_after_pick(
            &chosen,
            static_appearance(&chosen, cx),
            &theme.name,
            theme.appearance(),
            *SystemAppearance::global(cx),
        );
        self.keep_theme_selection(selection, cx);
    }

    fn pick_mode(&mut self, mode: ThemeAppearanceMode, cx: &mut Context<Self>) {
        let chosen = self.chosen_theme_selection(cx);
        let selection = selection_after_mode(&chosen, static_appearance(&chosen, cx), mode);
        self.keep_theme_selection(selection, cx);
    }

    /// The choice is shown at once, and the settings file follows a moment later
    /// with the same selection.
    fn keep_theme_selection(&mut self, selection: ThemeSelection, cx: &mut Context<Self>) {
        let mut settings = ThemeSettings::get_global(cx).clone();
        if settings.theme != selection {
            settings.theme = selection.clone();
            SettingsStore::update_global(cx, |store, _| store.override_global(settings));
        }
        write_theme_selection(selection, cx);
        cx.notify();
    }

    fn focus_theme_tile(&mut self, index: usize, window: &mut Window, cx: &mut Context<Self>) {
        if let Some(handle) = self.theme_tile_focus_handles.get(index) {
            window.focus(handle, cx);
        }
    }

    fn render_look_section(
        &mut self,
        number: usize,
        tab_index: &mut isize,
        cx: &mut Context<Self>,
    ) -> impl IntoElement {
        let system_appearance = *SystemAppearance::global(cx);
        let chosen = self.chosen_theme_selection(cx);
        let chosen_name = chosen.name(system_appearance).0;
        let mode = chosen.mode().unwrap_or_else(|| {
            appearance_mode(static_appearance(&chosen, cx).unwrap_or(system_appearance))
        });

        let this = cx.weak_entity();
        let mode_switch = ToggleButtonGroup::single_row(
            "theme-selector-onboarding-dark-light",
            [
                ("Light", ThemeAppearanceMode::Light),
                ("Dark", ThemeAppearanceMode::Dark),
                ("System", ThemeAppearanceMode::System),
            ]
            .map(|(label, mode)| {
                let this = this.clone();
                ToggleButtonSimple::new(label, move |_, _, cx| {
                    this.update(cx, |this, cx| this.pick_mode(mode, cx)).ok();
                })
            }),
        )
        .size(ToggleButtonGroupSize::Medium)
        .tab_index(tab_index)
        .selected_index(match mode {
            ThemeAppearanceMode::Light => 0,
            ThemeAppearanceMode::Dark => 1,
            ThemeAppearanceMode::System => 2,
        })
        .style(ui::ToggleButtonGroupStyle::Outlined)
        .width(rems_from_px(3.0_f32 * 72.0_f32));

        let tiles = (0..FAMILY_THEMES.len())
            .filter_map(|index| {
                let theme = Self::family_theme(index, cx)?;
                let tile_tab_index = *tab_index;
                *tab_index += 1;
                Some(self.render_theme_tile(index, theme, chosen_name.as_ref(), tile_tab_index, cx))
            })
            .collect::<Vec<_>>();
        let has_tiles = !tiles.is_empty();

        let more_themes = quiet_link(
            "onboarding-more-themes",
            "More Themes…",
            app_actions::theme_selector::Toggle::default().boxed_clone(),
            *tab_index,
            self.focus_handle.clone(),
            cx,
        );
        *tab_index += 1;

        v_flex()
            .w_full()
            .px_3()
            .gap_2()
            .child(
                h_flex()
                    .w_full()
                    .gap_2()
                    .justify_between()
                    .child(SectionLabel::new(format!("{number} · Look")))
                    .child(mode_switch),
            )
            .when(has_tiles, |this| {
                this.child(
                    h_flex()
                        .id("onboarding-theme-tiles")
                        .w_full()
                        .gap_2()
                        .items_start()
                        .children(tiles),
                )
            })
            .child(h_flex().child(more_themes))
    }

    fn render_theme_tile(
        &self,
        index: usize,
        theme: Arc<Theme>,
        chosen_name: &str,
        tab_index: isize,
        cx: &mut Context<Self>,
    ) -> impl IntoElement + use<> {
        let colors = cx.theme().colors();
        let name = theme.name.clone();
        let is_selected = name.as_ref() == chosen_name;
        let hover_border = colors.border_strong;
        let focus_ring = focus_ring(cx);
        let tile_count = self.theme_tile_focus_handles.len();
        let selector_name = name.clone();

        v_flex()
            .flex_1()
            .min_w_0()
            .gap_1()
            .items_center()
            .child(
                div()
                    .id(("onboarding-theme-tile", index))
                    .debug_selector(move || format!("ONBOARDING-THEME-{selector_name}"))
                    .role(Role::Button)
                    .aria_label(name.clone())
                    // A tracked handle carries its own place in the tab order: the
                    // element's `tab_index` only reaches a handle the element made.
                    .when_some(self.theme_tile_focus_handles.get(index), |this, handle| {
                        this.track_focus(&handle.clone().tab_index(tab_index).tab_stop(true))
                    })
                    .w_full()
                    .h(rems_from_px(88_f32))
                    .cursor_pointer()
                    .border_2()
                    .rounded(ThemePreviewTile::root_radius())
                    .map(|this| {
                        if is_selected {
                            // The marker of the chosen theme.
                            this.border_color(colors.text_accent)
                        } else {
                            this.border_color(colors.border_transparent)
                                .hover(move |style| style.border_color(hover_border))
                        }
                    })
                    .focus_visible(move |mut style: StyleRefinement| {
                        style.box_shadow = Some(focus_ring);
                        style
                    })
                    .on_click(cx.listener(move |this, _, _, cx| this.pick_theme(index, cx)))
                    .on_action(
                        cx.listener(move |this, _: &menu::Confirm, _, cx| {
                            this.pick_theme(index, cx)
                        }),
                    )
                    // Left and right move along the row; Tab and the vertical arrows
                    // follow the page.
                    .on_action(cx.listener(move |this, _: &menu::SelectChild, window, cx| {
                        this.focus_theme_tile((index + 1) % tile_count.max(1), window, cx)
                    }))
                    .on_action(
                        cx.listener(move |this, _: &menu::SelectParent, window, cx| {
                            this.focus_theme_tile(
                                (index + tile_count.saturating_sub(1)) % tile_count.max(1),
                                window,
                                cx,
                            )
                        }),
                    )
                    .child(
                        ThemePreviewTile::new(theme, 0xBEEF as f32)
                            .style(ThemePreviewStyle::Bordered),
                    ),
            )
            .child(
                Label::new(name)
                    .size(LabelSize::Small)
                    .color(if is_selected {
                        Color::Default
                    } else {
                        Color::Muted
                    })
                    .truncate(),
            )
    }

    /// One sentence and one action, which opens the connection screen. `None` when
    /// AI is turned off.
    fn render_ai_section(
        &self,
        number: usize,
        tab_index: &mut isize,
        cx: &mut Context<Self>,
    ) -> Option<impl IntoElement + use<>> {
        if DisableAiSettings::get_global(cx).disable_ai {
            return None;
        }

        // Published by the crate that knows the models; unknown until it has spoken.
        let (line, button_label): (SharedString, &'static str) =
            match cx.try_global::<AiConnection>() {
                Some(AiConnection::Connected { model_name }) => {
                    (format!("Connected: {model_name}.").into(), "Manage")
                }
                _ => (AI_DETAIL.into(), "Connect a Model"),
            };
        let button_tab_index = *tab_index;
        *tab_index += 1;

        Some(
            v_flex()
                .w_full()
                .px_3()
                .gap_2()
                .child(SectionLabel::new(format!("{number} · AI")))
                .child(
                    h_flex()
                        .w_full()
                        .gap_x_4()
                        .gap_y_2()
                        .flex_wrap()
                        .justify_between()
                        .child(
                            div()
                                .flex_1()
                                .min_w(rems_from_px(240_f32))
                                .child(Label::new(line).color(Color::Muted)),
                        )
                        .child(
                            Button::new("connect-ai-provider", button_label)
                                .style(ButtonStyle::Outlined)
                                .size(ButtonSize::Medium)
                                .label_size(LabelSize::Small)
                                .tab_index(button_tab_index)
                                .on_click(|_, window, cx| {
                                    let action = cx
                                        .build_action(MANAGE_CONNECTIONS_ACTION, None)
                                        .unwrap_or_else(|_| {
                                            app_actions::assistant::FocusAgent.boxed_clone()
                                        });
                                    window.dispatch_action(action, cx);
                                }),
                        ),
                ),
        )
    }

    fn render_keys_section(
        &self,
        number: usize,
        tab_index: &mut isize,
        cx: &mut Context<Self>,
    ) -> impl IntoElement + use<> {
        let base_keymap = *BaseKeymap::get_global(cx);
        let selected_segment = KEYMAP_CHOICES
            .iter()
            .position(|(_, keymap)| *keymap == base_keymap);
        // A keymap chosen behind "Other…" is named beside the link.
        let other_keymap_name = if selected_segment.is_none() {
            BaseKeymap::OPTIONS
                .iter()
                .find(|(_, keymap)| *keymap == base_keymap)
                .map(|(name, keymap)| base_keymap_picker::display_name(name, *keymap))
        } else {
            None
        };

        let keymap_switch = ToggleButtonGroup::single_row(
            "base_keymap_selection",
            KEYMAP_CHOICES.map(|(label, keymap)| {
                ToggleButtonSimple::new(label, move |_, _, cx| write_keymap_base(keymap, cx))
            }),
        )
        // No segment is marked when the keymap is one of the others.
        .selected_index(selected_segment.unwrap_or(usize::MAX))
        .tab_index(tab_index)
        .size(ToggleButtonGroupSize::Medium)
        .style(ui::ToggleButtonGroupStyle::Outlined)
        .width(rems_from_px(4.0_f32 * 112.0_f32));

        let other_keymaps = quiet_link(
            "onboarding-other-keymaps",
            "Other…",
            ToggleBaseKeymapSelector.boxed_clone(),
            *tab_index,
            self.focus_handle.clone(),
            cx,
        );
        *tab_index += 1;

        let vim_tab_index = *tab_index;
        *tab_index += 1;
        let vim_mode = SwitchField::new(
            "onboarding-vim-mode",
            Some("Vim Mode"),
            Some("Modal editing, as in Vim and Neovim.".into()),
            if VimModeSetting::get_global(cx).0 {
                ui::ToggleState::Selected
            } else {
                ui::ToggleState::Unselected
            },
            {
                let fs = <dyn Fs>::global(cx);
                move |&selection, _, cx| {
                    let vim_mode = match selection {
                        ToggleState::Selected => true,
                        ToggleState::Unselected => false,
                        ToggleState::Indeterminate => return,
                    };
                    update_settings_file(fs.clone(), cx, move |setting, _| {
                        setting.vim_mode = Some(vim_mode);
                    });
                }
            },
        )
        .tab_index(vim_tab_index);

        let import_state = SettingsImportState::global(cx);
        let imports: [(&'static str, Box<dyn Action>, bool); 2] = [
            (
                "Import Settings from VS Code",
                ImportVsCodeSettings { skip_prompt: false }.boxed_clone(),
                import_state.vscode,
            ),
            (
                "Import Settings from Cursor",
                ImportCursorSettings { skip_prompt: false }.boxed_clone(),
                import_state.cursor,
            ),
        ];
        let import_buttons = imports.map(|(label, action, imported)| {
            let button_tab_index = *tab_index;
            *tab_index += 1;
            Button::new(label, label)
                .style(ButtonStyle::Outlined)
                .size(ButtonSize::Medium)
                .label_size(LabelSize::Small)
                .tab_index(button_tab_index)
                // An import that is done shows a check, not a fill.
                .when(imported, |this| {
                    this.end_icon(
                        Icon::new(IconName::Check)
                            .size(IconSize::Small)
                            .color(Color::Success),
                    )
                })
                .on_click(move |_, window, cx| {
                    window.dispatch_action(action.boxed_clone(), cx);
                })
        });

        v_flex()
            .w_full()
            .px_3()
            .gap_3()
            .child(SectionLabel::new(format!("{number} · Keys")))
            .child(
                h_flex()
                    .w_full()
                    .gap_x_4()
                    .gap_y_2()
                    .flex_wrap()
                    .child(keymap_switch)
                    .child(other_keymaps)
                    .when_some(other_keymap_name, |this, name| {
                        this.child(
                            Label::new(format!("Current: {name}"))
                                .size(LabelSize::Small)
                                .color(Color::Muted),
                        )
                    }),
            )
            .child(vim_mode)
            .child(h_flex().gap_2().flex_wrap().children(import_buttons))
    }

    /// The numbered sections, in the order the page reads.
    pub(crate) fn render_sections(
        &mut self,
        tab_index: &mut isize,
        cx: &mut Context<Self>,
    ) -> impl IntoElement {
        let mut number = 1;
        let look = self
            .render_look_section(number, tab_index, cx)
            .into_any_element();
        number += 1;
        let ai = self
            .render_ai_section(number, tab_index, cx)
            .map(|section| section.into_any_element());
        if ai.is_some() {
            number += 1;
        }
        let keys = self
            .render_keys_section(number, tab_index, cx)
            .into_any_element();

        v_flex()
            .id("basics-page")
            .w_full()
            .gap_6()
            .child(look)
            .children(ai)
            .child(keys)
    }
}

/// A quiet text link: muted, text colour on hover, the focus ring when reached with
/// the keyboard. Same look as the links at the foot of the welcome page.
fn quiet_link(
    id: &'static str,
    label: &'static str,
    action: Box<dyn Action>,
    tab_index: isize,
    focus_handle: FocusHandle,
    cx: &App,
) -> impl IntoElement + use<> {
    let colors = cx.theme().colors();
    let hover_color = colors.text;
    let focus_ring = focus_ring(cx);

    div()
        .id(id)
        .role(Role::Button)
        .aria_label(label)
        .tab_index(tab_index)
        .cursor_pointer()
        .text_ui_sm(cx)
        .text_color(colors.text_muted)
        .hover(move |style| style.text_color(hover_color))
        .focus_visible(move |mut style: StyleRefinement| {
            style.box_shadow = Some(focus_ring);
            style
        })
        .child(label)
        .on_click(move |_, window, cx| focus_handle.dispatch_action(&*action, window, cx))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn dynamic(mode: ThemeAppearanceMode, light: &str, dark: &str) -> ThemeSelection {
        ThemeSelection::Dynamic {
            mode,
            light: ThemeName(light.into()),
            dark: ThemeName(dark.into()),
        }
    }

    #[test]
    fn test_picking_a_theme_keeps_the_other_side() {
        let default = dynamic(ThemeAppearanceMode::Dark, "Unaligned Light", "Unaligned");

        // A dark theme replaces the dark side and stays dark.
        assert_eq!(
            selection_after_pick(
                &default,
                None,
                "Unaligned Midnight",
                Appearance::Dark,
                Appearance::Light
            ),
            dynamic(
                ThemeAppearanceMode::Dark,
                "Unaligned Light",
                "Unaligned Midnight"
            )
        );
        // A light theme shows at once, whatever the system says.
        assert_eq!(
            selection_after_pick(
                &default,
                None,
                "Unaligned Light",
                Appearance::Light,
                Appearance::Dark
            ),
            dynamic(ThemeAppearanceMode::Light, "Unaligned Light", "Unaligned")
        );
        // "System" survives a pick that the system would show anyway.
        let system = dynamic(ThemeAppearanceMode::System, "Unaligned Light", "Unaligned");
        assert_eq!(
            selection_after_pick(
                &system,
                None,
                "Unaligned Dusk",
                Appearance::Dark,
                Appearance::Dark
            ),
            dynamic(
                ThemeAppearanceMode::System,
                "Unaligned Light",
                "Unaligned Dusk"
            )
        );
    }

    #[test]
    fn test_mode_switch_stays_in_the_family() {
        // From the defaults.
        let default = dynamic(ThemeAppearanceMode::Dark, "Unaligned Light", "Unaligned");
        assert_eq!(
            selection_after_mode(&default, None, ThemeAppearanceMode::Light),
            dynamic(ThemeAppearanceMode::Light, "Unaligned Light", "Unaligned")
        );

        // From a single theme, as an older settings file may hold: the theme keeps
        // its side, the other side is the family's default, and the mode asked for
        // is the mode set.
        let single = ThemeSelection::Static(ThemeName("Unaligned Midnight".into()));
        assert_eq!(
            selection_after_mode(&single, Some(Appearance::Dark), ThemeAppearanceMode::Light),
            dynamic(
                ThemeAppearanceMode::Light,
                "Unaligned Light",
                "Unaligned Midnight"
            )
        );
        assert_eq!(
            selection_after_mode(&single, None, ThemeAppearanceMode::System),
            dynamic(ThemeAppearanceMode::System, "Unaligned Light", "Unaligned")
        );
    }

    #[test]
    fn test_family_has_five_distinct_themes_led_by_the_defaults() {
        let mut names = FAMILY_THEMES.to_vec();
        names.sort_unstable();
        names.dedup();
        assert_eq!(names.len(), 5);
        assert_eq!(FAMILY_THEMES[0], settings::DEFAULT_DARK_THEME);
        assert_eq!(FAMILY_THEMES[1], settings::DEFAULT_LIGHT_THEME);
    }

    #[test]
    fn test_vs_code_is_the_first_keymap_offered() {
        // The default itself comes from `assets/settings/default.json`.
        assert_eq!(KEYMAP_CHOICES[0], ("VS Code", BaseKeymap::VSCode));
        assert!(
            !KEYMAP_CHOICES
                .iter()
                .any(|(_, keymap)| *keymap == BaseKeymap::Zed)
        );
    }
}

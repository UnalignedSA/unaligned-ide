mod icon_theme_selector;

use app_actions::{ExtensionCategoryFilter, Extensions};
use fs::Fs;
use fuzzy::{StringMatch, StringMatchCandidate, match_strings};
use gpui::{
    App, Context, DismissEvent, Entity, EventEmitter, Focusable, Hsla, Render, UpdateGlobal,
    WeakEntity, Window, actions,
};
use picker::{Picker, PickerDelegate, PickerEmptyAction};
use settings::{Settings, SettingsStore, update_settings_file};
use std::sync::Arc;
use theme::{Appearance, SystemAppearance, Theme, ThemeMeta, ThemeRegistry};
use theme_settings::{
    ThemeAppearanceMode, ThemeName, ThemeSelection, ThemeSettings, appearance_to_mode,
};
use ui::{ListItem, ListItemSpacing, SectionLabel, prelude::*, v_flex};
use util::ResultExt;
use workspace::{ModalView, Workspace, ui::HighlightedLabel, with_active_or_new_workspace};

use crate::icon_theme_selector::{IconThemeSelector, IconThemeSelectorDelegate};

/// The product's own themes, in the order the selector offers them. These are
/// the names the bundled theme files carry (`assets/themes/unaligned`).
const FAMILY_ORDER: [&str; 5] = [
    "Unaligned",
    "Unaligned Light",
    "Unaligned Dusk",
    "Unaligned Midnight",
    "Unaligned Contrast",
];

/// Any other theme whose name starts like this is listed with the family
/// (later additions such as "Unaligned Mono").
const FAMILY_PREFIX: &str = "Unaligned ";

/// Where a theme sits in the list: the family in its fixed order, then other
/// themes of the family by name, then everything else, dark before light.
#[derive(PartialEq, Eq, PartialOrd, Ord)]
enum ThemeRank {
    Family(usize),
    FamilyExtra,
    Other { is_light: bool },
}

/// The sections of the unfiltered list, each under its label.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum ThemeSection {
    Family,
    OtherDark,
    OtherLight,
}

impl ThemeSection {
    /// Written in sentence case; `SectionLabel` shows it in uppercase.
    fn label(self) -> SharedString {
        match self {
            Self::Family => branding::PRODUCT_SHORT_NAME.into(),
            Self::OtherDark => "Other dark".into(),
            Self::OtherLight => "Other light".into(),
        }
    }
}

/// One row of the list: a section label, or the theme of `matches[_]`.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Row {
    Label(ThemeSection),
    Match(usize),
}

impl ThemeRank {
    fn of(meta: &ThemeMeta) -> Self {
        if let Some(position) = FAMILY_ORDER
            .iter()
            .position(|name| *name == meta.name.as_ref())
        {
            Self::Family(position)
        } else if meta.name.starts_with(FAMILY_PREFIX) {
            Self::FamilyExtra
        } else {
            Self::Other {
                is_light: meta.appearance.is_light(),
            }
        }
    }

    fn section(&self) -> ThemeSection {
        match self {
            Self::Family(_) | Self::FamilyExtra => ThemeSection::Family,
            Self::Other { is_light: false } => ThemeSection::OtherDark,
            Self::Other { is_light: true } => ThemeSection::OtherLight,
        }
    }
}

/// The three colours that tell one theme from another at a glance: its chrome
/// ground, its sheet ground and its accent.
#[derive(Clone, Copy)]
struct ThemeSwatch {
    chrome: Hsla,
    sheet: Hsla,
    accent: Hsla,
}

impl ThemeSwatch {
    fn of(theme: &Theme) -> Self {
        let colors = theme.colors();
        Self {
            chrome: colors.background,
            sheet: colors.editor_background,
            accent: colors.accent_background,
        }
    }

    /// 28×16: three flat rectangles inside a 1px line.
    fn render(self, cx: &App) -> impl IntoElement {
        h_flex()
            .flex_none()
            .w(rems_from_px(28_f32))
            .h(rems_from_px(16_f32))
            .border_1()
            .border_color(cx.theme().colors().border)
            .overflow_hidden()
            .child(div().flex_1().h_full().bg(self.chrome))
            .child(div().flex_1().h_full().bg(self.sheet))
            .child(div().flex_1().h_full().bg(self.accent))
    }
}

fn more_themes_action() -> Box<dyn gpui::Action> {
    Box::new(Extensions {
        category_filter: Some(ExtensionCategoryFilter::Themes),
        id: None,
    })
}

actions!(
    theme_selector,
    [
        /// Reloads all themes from disk.
        Reload
    ]
);

pub fn init(cx: &mut App) {
    cx.on_action(|action: &app_actions::theme_selector::Toggle, cx| {
        let action = action.clone();
        with_active_or_new_workspace(cx, move |workspace, window, cx| {
            toggle_theme_selector(workspace, &action, window, cx);
        });
    });
    cx.on_action(|action: &app_actions::icon_theme_selector::Toggle, cx| {
        let action = action.clone();
        with_active_or_new_workspace(cx, move |workspace, window, cx| {
            toggle_icon_theme_selector(workspace, &action, window, cx);
        });
    });
}

fn toggle_theme_selector(
    workspace: &mut Workspace,
    toggle: &app_actions::theme_selector::Toggle,
    window: &mut Window,
    cx: &mut Context<Workspace>,
) {
    let fs = workspace.app_state().fs.clone();
    workspace.toggle_modal(window, cx, |window, cx| {
        let delegate = ThemeSelectorDelegate::new(
            cx.entity().downgrade(),
            fs,
            toggle.themes_filter.as_ref(),
            cx,
        );
        ThemeSelector::new(delegate, window, cx)
    });
}

fn toggle_icon_theme_selector(
    workspace: &mut Workspace,
    toggle: &app_actions::icon_theme_selector::Toggle,
    window: &mut Window,
    cx: &mut Context<Workspace>,
) {
    let fs = workspace.app_state().fs.clone();
    workspace.toggle_modal(window, cx, |window, cx| {
        let delegate = IconThemeSelectorDelegate::new(
            cx.entity().downgrade(),
            fs,
            toggle.themes_filter.as_ref(),
            cx,
        );
        IconThemeSelector::new(delegate, window, cx)
    });
}

impl ModalView for ThemeSelector {
    fn on_before_dismiss(
        &mut self,
        _window: &mut Window,
        cx: &mut Context<Self>,
    ) -> workspace::DismissDecision {
        self.picker.update(cx, |picker, cx| {
            picker.delegate.revert_theme(cx);
        });
        workspace::DismissDecision::Dismiss(true)
    }
}

struct ThemeSelector {
    picker: Entity<Picker<ThemeSelectorDelegate>>,
}

impl EventEmitter<DismissEvent> for ThemeSelector {}

impl Focusable for ThemeSelector {
    fn focus_handle(&self, cx: &App) -> gpui::FocusHandle {
        self.picker.focus_handle(cx)
    }
}

impl Render for ThemeSelector {
    fn render(&mut self, _window: &mut Window, _cx: &mut Context<Self>) -> impl IntoElement {
        v_flex()
            .key_context("ThemeSelector")
            .w(rems(34.))
            .child(self.picker.clone())
    }
}

impl ThemeSelector {
    pub fn new(
        delegate: ThemeSelectorDelegate,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Self {
        let picker = cx.new(|cx| Picker::uniform_list(delegate, window, cx));
        Self { picker }
    }
}

struct ThemeSelectorDelegate {
    fs: Arc<dyn Fs>,
    themes: Vec<ThemeMeta>,
    /// One swatch per entry of `themes` (none if the theme failed to load).
    swatches: Vec<Option<ThemeSwatch>>,
    /// The section of each entry of `themes`.
    sections: Vec<ThemeSection>,
    matches: Vec<StringMatch>,
    /// What the list shows: `matches`, and while there is no query the label
    /// of each section before its first theme. Selection indices are rows.
    rows: Vec<Row>,
    /// The theme that was selected before the `ThemeSelector` menu was opened.
    ///
    /// We use this to return back to theme that was set if the user dismisses the menu.
    original_theme_settings: ThemeSettings,
    /// The current system appearance.
    original_system_appearance: Appearance,
    /// The id of the original theme in the list of themes.
    /// Using `Option<usize>` instead of `usize` because it's possible that the
    /// original theme is not present in the list of themes when it is first
    /// built, depending on the provided `themes_filter`. For example, when a
    /// theme is installed, the `themes_filter` is set to the new theme names
    /// and, if we used `unwrap_or(0)` as a fallback, the first theme in the
    /// list would be shown as "active".
    original_theme_id: Option<usize>,
    /// The currently selected new theme.
    new_theme: Arc<Theme>,
    selection_completed: bool,
    selected_theme: Option<Arc<Theme>>,
    selected_index: usize,
    selector: WeakEntity<ThemeSelector>,
}

impl ThemeSelectorDelegate {
    fn new(
        selector: WeakEntity<ThemeSelector>,
        fs: Arc<dyn Fs>,
        themes_filter: Option<&Vec<String>>,
        cx: &mut Context<ThemeSelector>,
    ) -> Self {
        let original_theme = cx.theme().clone();
        let original_theme_settings = ThemeSettings::get_global(cx).clone();
        let original_system_appearance = SystemAppearance::global(cx).0;

        let registry = ThemeRegistry::global(cx);
        let mut themes = registry
            .list()
            .into_iter()
            .filter(|meta| {
                if let Some(theme_filter) = themes_filter {
                    theme_filter.contains(&meta.name.to_string())
                } else {
                    true
                }
            })
            .collect::<Vec<_>>();

        // The product's family first, in its own order; then the rest, dark
        // before light, by name.
        themes.sort_unstable_by(|a, b| {
            ThemeRank::of(a)
                .cmp(&ThemeRank::of(b))
                .then_with(|| a.name.cmp(&b.name))
        });
        let sections = themes
            .iter()
            .map(|meta| ThemeRank::of(meta).section())
            .collect::<Vec<_>>();
        let swatches = themes
            .iter()
            .map(|meta| {
                registry
                    .get(&meta.name)
                    .ok()
                    .map(|theme| ThemeSwatch::of(&theme))
            })
            .collect();

        let original_theme_id = themes
            .iter()
            .position(|meta| meta.name == original_theme.name);

        let matches: Vec<StringMatch> = themes
            .iter()
            .enumerate()
            .map(|(id, meta)| StringMatch {
                candidate_id: id,
                score: 0.0,
                positions: Default::default(),
                string: meta.name.to_string(),
            })
            .collect();

        let rows = Self::rows_for(&matches, &sections, true);

        // The current theme is likely in this list, so default to first showing that.
        let selected_index = matches
            .iter()
            .position(|mat| mat.string == original_theme.name)
            .and_then(|match_ix| Self::row_of(&rows, match_ix))
            .unwrap_or_else(|| Self::first_selectable_row(&rows));

        Self {
            fs,
            themes,
            swatches,
            sections,
            matches,
            rows,
            original_theme_settings,
            original_system_appearance,
            original_theme_id,
            new_theme: original_theme, // Start with the original theme.
            selected_index,
            selection_completed: false,
            selected_theme: None,
            selector,
        }
    }

    /// The rows for `matches`. A list in the selector's own order (no query)
    /// is labelled by section when it holds more than one; a filtered list is
    /// in score order and has no labels.
    fn rows_for(matches: &[StringMatch], sections: &[ThemeSection], labelled: bool) -> Vec<Row> {
        let section_of = |mat: &StringMatch| sections.get(mat.candidate_id).copied();
        let labelled = labelled
            && matches.first().is_some_and(|first| {
                matches
                    .iter()
                    .any(|mat| section_of(mat) != section_of(first))
            });

        let mut rows = Vec::with_capacity(matches.len() + 3);
        let mut current_section = None;
        for (match_ix, mat) in matches.iter().enumerate() {
            let section = section_of(mat);
            if labelled
                && section != current_section
                && let Some(section) = section
            {
                rows.push(Row::Label(section));
            }
            current_section = section;
            rows.push(Row::Match(match_ix));
        }
        rows
    }

    /// The row that shows `matches[match_ix]`.
    fn row_of(rows: &[Row], match_ix: usize) -> Option<usize> {
        rows.iter().position(|row| *row == Row::Match(match_ix))
    }

    fn first_selectable_row(rows: &[Row]) -> usize {
        rows.iter()
            .position(|row| matches!(row, Row::Match(_)))
            .unwrap_or(0)
    }

    /// The theme shown in row `ix`; `None` for a label.
    fn match_at(&self, ix: usize) -> Option<&StringMatch> {
        match self.rows.get(ix)? {
            Row::Match(match_ix) => self.matches.get(*match_ix),
            Row::Label(_) => None,
        }
    }

    fn is_original_theme(&self, index: usize) -> bool {
        self.match_at(index)
            .zip(self.original_theme_id)
            .is_some_and(|(mat, original_theme_id)| mat.candidate_id == original_theme_id)
    }

    fn show_selected_theme(
        &mut self,
        cx: &mut Context<Picker<ThemeSelectorDelegate>>,
    ) -> Option<Arc<Theme>> {
        if let Some(mat) = self.match_at(self.selected_index).cloned() {
            let registry = ThemeRegistry::global(cx);

            match registry.get(&mat.string) {
                Ok(theme) => {
                    self.set_theme(theme.clone(), cx);
                    Some(theme)
                }
                Err(error) => {
                    log::error!("error loading theme {}: {}", mat.string, error);
                    None
                }
            }
        } else {
            None
        }
    }

    fn revert_theme(&mut self, cx: &mut App) {
        if !self.selection_completed {
            SettingsStore::update_global(cx, |store, _| {
                store.override_global(self.original_theme_settings.clone());
            });
            self.selection_completed = true;
        }
    }

    fn set_theme(&mut self, new_theme: Arc<Theme>, cx: &mut App) {
        // Update the global (in-memory) theme settings.
        SettingsStore::update_global(cx, |store, _| {
            override_global_theme(
                store,
                &new_theme,
                &self.original_theme_settings.theme,
                self.original_system_appearance,
            )
        });

        self.new_theme = new_theme;
    }
}

/// Overrides the global (in-memory) theme settings.
///
/// Note that this does **not** update the user's `settings.json` file (see the
/// [`ThemeSelectorDelegate::confirm`] method and [`theme_settings::set_theme`] function).
fn override_global_theme(
    store: &mut SettingsStore,
    new_theme: &Theme,
    original_theme: &ThemeSelection,
    system_appearance: Appearance,
) {
    let theme_name = ThemeName(new_theme.name.clone().into());
    let new_appearance = new_theme.appearance();
    let new_theme_is_light = new_appearance.is_light();

    let mut curr_theme_settings = store.get::<ThemeSettings>(None).clone();

    match (original_theme, &curr_theme_settings.theme) {
        // Override the currently selected static theme.
        (ThemeSelection::Static(_), ThemeSelection::Static(_)) => {
            curr_theme_settings.theme = ThemeSelection::Static(theme_name);
        }

        // If the current theme selection is dynamic, then only override the global setting for the
        // specific mode (light or dark).
        (
            ThemeSelection::Dynamic {
                mode: original_mode,
                light: original_light,
                dark: original_dark,
            },
            ThemeSelection::Dynamic { .. },
        ) => {
            let new_mode = update_mode_if_new_appearance_is_different_from_system(
                original_mode,
                system_appearance,
                new_appearance,
            );

            let updated_theme = retain_original_opposing_theme(
                new_theme_is_light,
                new_mode,
                theme_name,
                original_light,
                original_dark,
            );

            curr_theme_settings.theme = updated_theme;
        }

        // The theme selection mode changed while selecting new themes (someone edited the settings
        // file on disk while we had the dialogue open), so don't do anything.
        _ => return,
    };

    store.override_global(curr_theme_settings);
}

/// Helper function for determining the new [`ThemeAppearanceMode`] for the new theme.
///
/// If the the original theme mode was [`System`] and the new theme's appearance matches the system
/// appearance, we don't need to change the mode setting.
///
/// Otherwise, we need to change the mode in order to see the new theme.
///
/// [`System`]: ThemeAppearanceMode::System
fn update_mode_if_new_appearance_is_different_from_system(
    original_mode: &ThemeAppearanceMode,
    system_appearance: Appearance,
    new_appearance: Appearance,
) -> ThemeAppearanceMode {
    if original_mode == &ThemeAppearanceMode::System && system_appearance == new_appearance {
        ThemeAppearanceMode::System
    } else {
        appearance_to_mode(new_appearance)
    }
}

/// Helper function for updating / displaying the [`ThemeSelection`] while using the theme selector.
///
/// We want to retain the alternate theme selection of the original settings (before the menu was
/// opened), not the currently selected theme (which likely has changed multiple times while the
/// menu has been open).
fn retain_original_opposing_theme(
    new_theme_is_light: bool,
    new_mode: ThemeAppearanceMode,
    theme_name: ThemeName,
    original_light: &ThemeName,
    original_dark: &ThemeName,
) -> ThemeSelection {
    if new_theme_is_light {
        ThemeSelection::Dynamic {
            mode: new_mode,
            light: theme_name,
            dark: original_dark.clone(),
        }
    } else {
        ThemeSelection::Dynamic {
            mode: new_mode,
            light: original_light.clone(),
            dark: theme_name,
        }
    }
}

impl PickerDelegate for ThemeSelectorDelegate {
    type ListItem = ui::ListItem;

    fn name() -> &'static str {
        "theme selector"
    }

    fn placeholder_text(&self, _window: &mut Window, _cx: &mut App) -> Arc<str> {
        "Select Theme…".into()
    }

    fn match_count(&self) -> usize {
        self.rows.len()
    }

    fn can_select(&self, ix: usize, _window: &mut Window, _cx: &mut Context<Picker<Self>>) -> bool {
        matches!(self.rows.get(ix), Some(Row::Match(_)))
    }

    fn no_matches_action(
        &self,
        _window: &mut Window,
        _cx: &mut Context<Picker<Self>>,
    ) -> Option<PickerEmptyAction> {
        Some(PickerEmptyAction::new("More Themes…", more_themes_action()))
    }

    fn confirm(
        &mut self,
        _secondary: bool,
        _window: &mut Window,
        cx: &mut Context<Picker<ThemeSelectorDelegate>>,
    ) {
        self.selection_completed = true;

        let theme_name: Arc<str> = self.new_theme.name.as_str().into();
        let theme_appearance = self.new_theme.appearance;
        let system_appearance = SystemAppearance::global(cx).0;

        telemetry::event!("Settings Changed", setting = "theme", value = theme_name);

        update_settings_file(self.fs.clone(), cx, move |settings, _| {
            theme_settings::set_theme(settings, theme_name, theme_appearance, system_appearance);
        });

        self.selector
            .update(cx, |_, cx| {
                cx.emit(DismissEvent);
            })
            .ok();
    }

    fn dismissed(&mut self, _: &mut Window, cx: &mut Context<Picker<ThemeSelectorDelegate>>) {
        self.revert_theme(cx);

        self.selector.update(cx, |_, cx| cx.emit(DismissEvent)).ok();
    }

    fn selected_index(&self) -> usize {
        self.selected_index
    }

    fn set_selected_index(
        &mut self,
        ix: usize,
        _: &mut Window,
        cx: &mut Context<Picker<ThemeSelectorDelegate>>,
    ) {
        self.selected_index = ix;
        self.selected_theme = self.show_selected_theme(cx);
    }

    fn update_matches(
        &mut self,
        query: String,
        window: &mut Window,
        cx: &mut Context<Picker<ThemeSelectorDelegate>>,
    ) -> gpui::Task<()> {
        let background = cx.background_executor().clone();
        let candidates = self
            .themes
            .iter()
            .enumerate()
            .map(|(id, meta)| StringMatchCandidate::new(id, &meta.name))
            .collect::<Vec<_>>();

        cx.spawn_in(window, async move |this, cx| {
            let matches = if query.is_empty() {
                candidates
                    .into_iter()
                    .enumerate()
                    .map(|(index, candidate)| StringMatch {
                        candidate_id: index,
                        string: candidate.string,
                        positions: Vec::new(),
                        score: 0.0,
                    })
                    .collect()
            } else {
                match_strings(
                    &candidates,
                    &query,
                    false,
                    true,
                    100,
                    &Default::default(),
                    background,
                )
                .await
            };

            this.update(cx, |this, cx| {
                let delegate = &mut this.delegate;
                delegate.rows = Self::rows_for(&matches, &delegate.sections, query.is_empty());
                delegate.matches = matches;
                let first_row = Self::first_selectable_row(&delegate.rows);
                if query.is_empty() && delegate.selected_theme.is_none() {
                    let last_row = delegate.rows.len().saturating_sub(1);
                    delegate.selected_index = delegate.selected_index.clamp(first_row, last_row);
                    if matches!(
                        delegate.rows.get(delegate.selected_index),
                        Some(Row::Label(_))
                    ) {
                        delegate.selected_index += 1;
                    }
                } else if let Some(selected) = delegate.selected_theme.as_ref() {
                    delegate.selected_index = delegate
                        .matches
                        .iter()
                        .position(|mtch| mtch.string == selected.name)
                        .and_then(|match_ix| Self::row_of(&delegate.rows, match_ix))
                        .unwrap_or(first_row);
                } else {
                    delegate.selected_index = first_row;
                }
                // Preserve the previously selected theme when the filter yields no results.
                if let Some(theme) = this.delegate.show_selected_theme(cx) {
                    this.delegate.selected_theme = Some(theme);
                }
            })
            .log_err();
        })
    }

    fn render_match(
        &self,
        ix: usize,
        selected: bool,
        _window: &mut Window,
        cx: &mut Context<Picker<Self>>,
    ) -> Option<Self::ListItem> {
        if let Some(Row::Label(section)) = self.rows.get(ix) {
            return Some(
                ListItem::new(("theme-section", ix))
                    .inset(true)
                    .spacing(ListItemSpacing::Dense)
                    .selectable(false)
                    .child(SectionLabel::new(section.label())),
            );
        }

        let theme_match = self.match_at(ix)?;
        let is_original_theme = self.is_original_theme(ix);
        let swatch = self
            .swatches
            .get(theme_match.candidate_id)
            .copied()
            .flatten();

        Some(
            ListItem::new(ix)
                .inset(true)
                .spacing(ListItemSpacing::Dense)
                .toggle_state(selected)
                .start_slot::<AnyElement>(swatch.map(|swatch| swatch.render(cx).into_any_element()))
                .child(HighlightedLabel::new(
                    theme_match.string.clone(),
                    theme_match.positions.clone(),
                ))
                .when(is_original_theme, |this| {
                    this.end_slot(Icon::new(IconName::Check).color(Color::Muted))
                }),
        )
    }

    fn render_footer(
        &self,
        _: &mut Window,
        cx: &mut Context<Picker<Self>>,
    ) -> Option<gpui::AnyElement> {
        // With nothing to list, the empty state offers the same action.
        if self.matches.is_empty() {
            return None;
        }
        Some(
            picker::footer(cx)
                .justify_end()
                .child(
                    picker::footer_button("more-themes", "More Themes…")
                        .on_click(|_, window, cx| window.dispatch_action(more_themes_action(), cx)),
                )
                .into_any_element(),
        )
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use gpui::{TestAppContext, VisualTestContext};
    use project::Project;
    use serde_json::json;
    use theme::{Appearance, ThemeFamily, ThemeRegistry, default_color_scales};
    use util::path;
    use workspace::MultiWorkspace;

    fn init_test(cx: &mut TestAppContext) -> Arc<workspace::AppState> {
        cx.update(|cx| {
            let app_state = workspace::AppState::test(cx);
            settings::init(cx);
            theme::init(theme::LoadThemes::JustBase, cx);
            editor::init(cx);
            super::init(cx);
            app_state
        })
    }

    fn register_test_themes(cx: &mut TestAppContext) {
        cx.update(|cx| {
            let registry = ThemeRegistry::global(cx);
            let base_theme = registry.get("One Dark").unwrap();

            let mut test_light = (*base_theme).clone();
            test_light.id = "test-light".to_string();
            test_light.name = "Test Light".into();
            test_light.appearance = Appearance::Light;

            let mut test_dark_a = (*base_theme).clone();
            test_dark_a.id = "test-dark-a".to_string();
            test_dark_a.name = "Test Dark A".into();

            let mut test_dark_b = (*base_theme).clone();
            test_dark_b.id = "test-dark-b".to_string();
            test_dark_b.name = "Test Dark B".into();

            registry.register_test_themes([ThemeFamily {
                id: "test-family".to_string(),
                name: "Test Family".into(),
                author: "test".into(),
                themes: vec![test_light, test_dark_a, test_dark_b],
                scales: default_color_scales(),
            }]);
        });
    }

    async fn setup_test(cx: &mut TestAppContext) -> Arc<workspace::AppState> {
        let app_state = init_test(cx);
        register_test_themes(cx);
        app_state
            .fs
            .as_fake()
            .insert_tree(path!("/test"), json!({}))
            .await;
        app_state
    }

    fn open_theme_selector(
        workspace: &Entity<workspace::Workspace>,
        cx: &mut VisualTestContext,
    ) -> Entity<Picker<ThemeSelectorDelegate>> {
        cx.dispatch_action(app_actions::theme_selector::Toggle {
            themes_filter: None,
        });
        cx.run_until_parked();
        workspace.update(cx, |workspace, cx| {
            workspace
                .active_modal::<ThemeSelector>(cx)
                .expect("theme selector should be open")
                .read(cx)
                .picker
                .clone()
        })
    }

    fn selected_theme_name(
        picker: &Entity<Picker<ThemeSelectorDelegate>>,
        cx: &mut VisualTestContext,
    ) -> String {
        picker.read_with(cx, |picker, _| {
            picker
                .delegate
                .match_at(picker.delegate.selected_index)
                .expect("selected index should point to a match")
                .string
                .clone()
        })
    }

    fn previewed_theme_name(
        picker: &Entity<Picker<ThemeSelectorDelegate>>,
        cx: &mut VisualTestContext,
    ) -> String {
        picker.read_with(cx, |picker, _| picker.delegate.new_theme.name.to_string())
    }

    #[gpui::test]
    async fn test_theme_selector_preserves_selection_on_empty_filter(cx: &mut TestAppContext) {
        let app_state = setup_test(cx).await;
        let project = Project::test(app_state.fs.clone(), [path!("/test").as_ref()], cx).await;
        let (multi_workspace, cx) =
            cx.add_window_view(|window, cx| MultiWorkspace::test_new(project.clone(), window, cx));
        let workspace =
            multi_workspace.read_with(cx, |multi_workspace, _| multi_workspace.workspace().clone());
        let picker = open_theme_selector(&workspace, cx);

        let target_index = picker.read_with(cx, |picker, _| {
            picker
                .delegate
                .matches
                .iter()
                .position(|m| m.string == "Test Light")
                .and_then(|match_ix| ThemeSelectorDelegate::row_of(&picker.delegate.rows, match_ix))
                .unwrap()
        });
        picker.update_in(cx, |picker, window, cx| {
            picker.set_selected_index(target_index, None, true, window, cx);
        });
        cx.run_until_parked();

        assert_eq!(previewed_theme_name(&picker, cx), "Test Light");

        picker.update_in(cx, |picker, window, cx| {
            picker.update_matches("zzz".to_string(), window, cx);
        });
        cx.run_until_parked();

        picker.update_in(cx, |picker, window, cx| {
            picker.update_matches("".to_string(), window, cx);
        });
        cx.run_until_parked();

        assert_eq!(
            selected_theme_name(&picker, cx),
            "Test Light",
            "selected theme should be preserved after clearing an empty filter"
        );
        assert_eq!(
            previewed_theme_name(&picker, cx),
            "Test Light",
            "previewed theme should be preserved after clearing an empty filter"
        );
    }

    fn register_family_themes(cx: &mut TestAppContext) {
        cx.update(|cx| {
            let registry = ThemeRegistry::global(cx);
            let base_theme = registry.get("One Dark").unwrap();
            // Registered out of order, with one later addition to the family.
            let themes = [
                ("Unaligned Mono", Appearance::Dark),
                ("Unaligned Contrast", Appearance::Dark),
                ("Unaligned Midnight", Appearance::Dark),
                ("Unaligned Dusk", Appearance::Dark),
                ("Unaligned Light", Appearance::Light),
                ("Unaligned", Appearance::Dark),
            ]
            .into_iter()
            .map(|(name, appearance)| {
                let mut theme = (*base_theme).clone();
                theme.id = name.to_lowercase().replace(' ', "-");
                theme.name = name.into();
                theme.appearance = appearance;
                theme
            })
            .collect();

            registry.register_test_themes([ThemeFamily {
                id: "test-product-family".to_string(),
                name: "Test Product Family".into(),
                author: "test".into(),
                themes,
                scales: default_color_scales(),
            }]);
        });
    }

    #[gpui::test]
    async fn test_theme_selector_lists_the_family_first(cx: &mut TestAppContext) {
        let app_state = setup_test(cx).await;
        register_family_themes(cx);
        let project = Project::test(app_state.fs.clone(), [path!("/test").as_ref()], cx).await;
        let (multi_workspace, cx) =
            cx.add_window_view(|window, cx| MultiWorkspace::test_new(project.clone(), window, cx));
        let workspace =
            multi_workspace.read_with(cx, |multi_workspace, _| multi_workspace.workspace().clone());
        let picker = open_theme_selector(&workspace, cx);

        let family = [
            "Unaligned",
            "Unaligned Light",
            "Unaligned Dusk",
            "Unaligned Midnight",
            "Unaligned Contrast",
            "Unaligned Mono",
        ];
        picker.read_with(cx, |picker, _| {
            let delegate = &picker.delegate;
            let names = delegate
                .themes
                .iter()
                .map(|meta| meta.name.to_string())
                .collect::<Vec<_>>();
            assert_eq!(&names[..family.len()], &family);

            // The rest: dark before light, then by name.
            let rest = delegate.themes[family.len()..]
                .iter()
                .map(|meta| (meta.appearance.is_light(), meta.name.to_string()))
                .collect::<Vec<_>>();
            let mut sorted = rest.clone();
            sorted.sort();
            assert_eq!(rest, sorted);
            assert!(names.contains(&"Test Light".to_string()));

            // Each section under its label, which is a row that no theme owns.
            let labels = delegate
                .rows
                .iter()
                .enumerate()
                .filter_map(|(ix, row)| match row {
                    Row::Label(section) => Some((ix, *section)),
                    Row::Match(_) => None,
                })
                .collect::<Vec<_>>();
            let first_light =
                family.len() + rest.iter().position(|(is_light, _)| *is_light).unwrap();
            assert_eq!(
                labels,
                [
                    (0, ThemeSection::Family),
                    (family.len() + 1, ThemeSection::OtherDark),
                    (first_light + 2, ThemeSection::OtherLight),
                ]
            );
            assert_eq!(delegate.match_count(), names.len() + 3);
            assert!(delegate.match_at(0).is_none());
            assert_eq!(delegate.match_at(1).unwrap().string, "Unaligned");
        });

        // Moving through the list previews each theme and steps over the labels.
        picker.update_in(cx, |picker, window, cx| {
            picker.set_selected_index(2, None, true, window, cx);
        });
        cx.run_until_parked();
        assert_eq!(previewed_theme_name(&picker, cx), "Unaligned Light");

        picker.update_in(cx, |picker, window, cx| {
            picker.set_selected_index(family.len(), None, true, window, cx);
            picker.select_next(&Default::default(), window, cx);
        });
        cx.run_until_parked();
        let first_other = picker.read_with(cx, |picker, _| {
            assert_eq!(picker.delegate.selected_index, family.len() + 2);
            picker.delegate.themes[family.len()].name.to_string()
        });
        assert_eq!(selected_theme_name(&picker, cx), first_other);
        assert_eq!(previewed_theme_name(&picker, cx), first_other);

        // A label cannot be selected.
        picker.update_in(cx, |picker, window, cx| {
            picker.set_selected_index(0, None, true, window, cx);
        });
        cx.run_until_parked();
        assert_eq!(selected_theme_name(&picker, cx), first_other);

        // A query lists matches only, without labels.
        picker.update_in(cx, |picker, window, cx| {
            picker.update_matches("dusk".to_string(), window, cx);
        });
        cx.run_until_parked();
        picker.read_with(cx, |picker, _| {
            assert!(
                picker
                    .delegate
                    .rows
                    .iter()
                    .all(|row| matches!(row, Row::Match(_)))
            );
            assert_eq!(picker.delegate.matches[0].string, "Unaligned Dusk");
        });
    }
}

//! The application's menus inside the window, where the system has no menu
//! bar of its own (Windows, Linux): the product's mark, the first menus by
//! name, and a "…" that holds the others, as editors on those systems do.
//!
//! ```text
//! [mark]  File  Edit  Selection  View  Go  Run  …
//! ```
//!
//! The mark opens the product's own menu (About, Settings, Quit). How many
//! menus are written out follows the width of the window: a narrow window
//! keeps the first ones and moves the rest under "…", down to the mark and
//! "…" alone. With the setting `title_bar.show_menus`, or in accessible mode,
//! every menu is written out.

use gpui::{Action, Entity, OwnedMenu, OwnedMenuItem, Pixels, Subscription, actions, px};
use project::DisableAiSettings;
use settings::{Settings, SettingsStore};
use workspace::AccessibleMode;

use schemars::JsonSchema;
use serde::Deserialize;

use smallvec::SmallVec;
use ui::{ButtonLike, ContextMenu, PopoverMenu, PopoverMenuHandle, Tooltip, prelude::*};

use platform_title_bar::TitleBarSettings;

actions!(
    app_menu,
    [
        /// Activates the menu on the right in the client-side application menu.
        ///
        /// Does not apply to platform menu bars (e.g. on macOS).
        ActivateMenuRight,
        /// Activates the menu on the left in the client-side application menu.
        ///
        /// Does not apply to platform menu bars (e.g. on macOS).
        ActivateMenuLeft
    ]
);

/// Opens the named menu in the client-side application menu. The first menu,
/// which carries the application's name, is called "App" here.
///
/// Does not apply to platform menu bars (e.g. on macOS).
#[derive(Clone, Deserialize, JsonSchema, PartialEq, Default, Action)]
#[action(namespace = app_menu)]
pub struct OpenApplicationMenu(String);

/// The name key bindings give to the first menu: its real name is the
/// product's, which a keymap file cannot spell (see `app_menus.rs`).
#[cfg(not(target_os = "macos"))]
const APP_MENU_ALIAS: &str = "App";

#[cfg(not(target_os = "macos"))]
pub enum ActivateDirection {
    Left,
    Right,
}

#[derive(Clone)]
struct MenuEntry {
    menu: OwnedMenu,
    handle: PopoverMenuHandle<ContextMenu>,
}

pub struct ApplicationMenu {
    /// Every menu, the product's own first.
    entries: SmallVec<[MenuEntry; 8]>,
    /// The "…" that holds the menus the bar has no room to write out.
    overflow: PopoverMenuHandle<ContextMenu>,
    pending_menu_open: Option<String>,
    _settings_subscription: Subscription,
}

/// How many menus are written out after the mark in a window this wide. The
/// menus are, in order: File, Edit, Selection, View, Go, Run, then the ones
/// that are used least.
fn menus_written_out(window_width: Pixels) -> usize {
    if window_width >= px(1100.) {
        6
    } else if window_width >= px(900.) {
        4
    } else if window_width >= px(700.) {
        2
    } else {
        0
    }
}

impl ApplicationMenu {
    pub fn new(_: &mut Window, cx: &mut Context<Self>) -> Self {
        let menus = cx.get_menus().unwrap_or_default();

        let entries = Self::build_entries(menus);
        let mut disable_ai = DisableAiSettings::get_global(cx).disable_ai;
        let settings_subscription =
            cx.observe_global::<SettingsStore>(move |application_menu, cx| {
                let new_disable_ai = DisableAiSettings::get_global(cx).disable_ai;
                if new_disable_ai != disable_ai {
                    disable_ai = new_disable_ai;
                    for entry in &application_menu.entries {
                        if entry.handle.is_deployed() {
                            entry.handle.hide(cx);
                        }
                    }
                    let menus = cx.get_menus().unwrap_or_default();
                    application_menu.entries = Self::build_entries(menus);
                }
                cx.notify();
            });

        Self {
            entries,
            overflow: PopoverMenuHandle::default(),
            pending_menu_open: None,
            _settings_subscription: settings_subscription,
        }
    }

    /// How many menus follow the mark by name; the others are under "…".
    fn written_out(&self, window: &Window, cx: &App) -> usize {
        let menus = self.entries.len().saturating_sub(1);
        if show_menus(cx) || cx.accessible_mode() {
            return menus;
        }
        let written_out = menus_written_out(window.viewport_size().width).min(menus);
        // A "…" for one menu would take the room of that menu.
        if written_out + 1 == menus {
            menus
        } else {
            written_out
        }
    }

    /// The handles of what the bar shows, left to right: the mark, the menus
    /// written out, and "…" when there is one.
    fn shown_handles(&self, written_out: usize) -> Vec<PopoverMenuHandle<ContextMenu>> {
        let mut handles: Vec<_> = self
            .entries
            .iter()
            .take(written_out + 1)
            .map(|entry| entry.handle.clone())
            .collect();
        if self.entries.len() > written_out + 1 {
            handles.push(self.overflow.clone());
        }
        handles
    }

    /// Whether menus are written out in the title bar, which then has no room
    /// for the project's name and branch beside them (the command centre and
    /// the status bar show both).
    pub fn shows_menus_by_name(&self, window: &Window, cx: &App) -> bool {
        self.written_out(window, cx) > 0
    }

    fn build_entries(menus: Vec<OwnedMenu>) -> SmallVec<[MenuEntry; 8]> {
        menus
            .into_iter()
            .map(|menu| MenuEntry {
                menu,
                handle: PopoverMenuHandle::default(),
            })
            .collect()
    }

    fn sanitize_menu_items(items: Vec<OwnedMenuItem>) -> Vec<OwnedMenuItem> {
        let mut cleaned = Vec::new();
        let mut last_was_separator = false;

        for item in items {
            match item {
                OwnedMenuItem::Separator => {
                    if !last_was_separator {
                        cleaned.push(item);
                        last_was_separator = true;
                    }
                }
                OwnedMenuItem::Submenu(submenu) => {
                    // Skip empty submenus
                    if !submenu.items.is_empty() {
                        cleaned.push(OwnedMenuItem::Submenu(submenu));
                        last_was_separator = false;
                    }
                }
                item => {
                    cleaned.push(item);
                    last_was_separator = false;
                }
            }
        }

        // Remove trailing separator
        if let Some(OwnedMenuItem::Separator) = cleaned.last() {
            cleaned.pop();
        }

        cleaned
    }

    /// Adds the items of a menu to `menu`. A sub-menu of the application's
    /// menus is written flat, as before.
    fn add_items(menu: ContextMenu, items: Vec<OwnedMenuItem>) -> ContextMenu {
        Self::sanitize_menu_items(items)
            .into_iter()
            .fold(menu, |menu, item| match item {
                OwnedMenuItem::Separator => menu.separator(),
                OwnedMenuItem::Action {
                    name,
                    action,
                    checked,
                    disabled,
                    ..
                } => menu.action_checked_with_disabled(name, action, checked, disabled),
                OwnedMenuItem::Submenu(submenu) => {
                    submenu
                        .items
                        .into_iter()
                        .fold(menu, |menu, item| match item {
                            OwnedMenuItem::Separator => menu.separator(),
                            OwnedMenuItem::Action {
                                name,
                                action,
                                checked,
                                disabled,
                                ..
                            } => menu.action_checked_with_disabled(name, action, checked, disabled),
                            OwnedMenuItem::Submenu(_) => menu,
                            OwnedMenuItem::SystemMenu(_) => {
                                // A system menu doesn't make sense in this context, so ignore it
                                menu
                            }
                        })
                }
                OwnedMenuItem::SystemMenu(_) => {
                    // A system menu doesn't make sense in this context, so ignore it
                    menu
                }
            })
    }

    fn build_menu_from_items(
        entry: MenuEntry,
        window: &mut Window,
        cx: &mut App,
    ) -> Entity<ContextMenu> {
        ContextMenu::build(window, cx, |menu, window, cx| {
            // Grab current focus handle so menu can shown items in context with the focused element
            let menu = menu.when_some(window.focused(cx), |menu, focused| menu.context(focused));
            Self::add_items(menu, entry.menu.items)
        })
    }

    /// The menu under "…": each menu that is not written out, as a sub-menu.
    fn build_overflow_menu(
        entries: Vec<MenuEntry>,
        window: &mut Window,
        cx: &mut App,
    ) -> Entity<ContextMenu> {
        ContextMenu::build(window, cx, |menu, window, cx| {
            let focused = window.focused(cx);
            let menu = menu.when_some(focused.clone(), |menu, focused| menu.context(focused));
            entries.into_iter().fold(menu, |menu, entry| {
                let focused = focused.clone();
                let items = entry.menu.items.clone();
                menu.submenu(entry.menu.name, move |menu, _, _| {
                    let menu =
                        menu.when_some(focused.clone(), |menu, focused| menu.context(focused));
                    Self::add_items(menu, items.clone())
                })
            })
        })
    }

    /// Opens this menu when the pointer comes over it while another menu of
    /// the bar is open (or always, with `open_menus_on_hover`).
    fn switch_on_hover(
        current: PopoverMenuHandle<ContextMenu>,
        all: Vec<PopoverMenuHandle<ContextMenu>>,
    ) -> impl Fn(&bool, &mut Window, &mut App) + 'static {
        move |hover_enter, window, cx| {
            if *hover_enter
                && !current.is_deployed()
                && (open_menus_on_hover(cx) || all.iter().any(|handle| handle.is_deployed()))
            {
                all.iter().for_each(|handle| handle.hide(cx));

                // We need to defer this so that this menu handle can take focus from the previous menu
                let handle = current.clone();
                window.defer(cx, move |window, cx| handle.show(window, cx));
            }
        }
    }

    /// The product's mark, which opens the product's own menu.
    fn render_application_menu(
        &self,
        entry: &MenuEntry,
        all_handles: Vec<PopoverMenuHandle<ContextMenu>>,
        cx: &App,
    ) -> impl IntoElement {
        let handle = entry.handle.clone();

        let menu_name = entry.menu.name.clone();
        let entry = entry.clone();

        // Application menu must have same ids as first menu item in standard menu
        div()
            .id(format!("{}-menu-item", menu_name))
            .occlude()
            .child(
                PopoverMenu::new(format!("{}-menu-popover", menu_name))
                    .menu(move |window, cx| {
                        Self::build_menu_from_items(entry.clone(), window, cx).into()
                    })
                    .trigger_with_tooltip(
                        ButtonLike::new(SharedString::from(format!("{}-menu-trigger", menu_name)))
                            .style(ButtonStyle::Subtle)
                            .tab_index(0isize)
                            .aria_label(format!("{} Menu", branding::PRODUCT_NAME))
                            .child(div().px_1().child(brand_mark(cx))),
                        Tooltip::text(format!("{} Menu", branding::PRODUCT_NAME)),
                    )
                    .with_handle(handle.clone()),
            )
            .on_hover(Self::switch_on_hover(handle, all_handles))
    }

    fn render_standard_menu(
        &self,
        entry: &MenuEntry,
        all_handles: Vec<PopoverMenuHandle<ContextMenu>>,
    ) -> impl IntoElement {
        let current_handle = entry.handle.clone();

        let menu_name = entry.menu.name.clone();
        let entry = entry.clone();

        div()
            .id(format!("{}-menu-item", menu_name))
            .occlude()
            .child(
                PopoverMenu::new(format!("{}-menu-popover", menu_name))
                    .menu(move |window, cx| {
                        Self::build_menu_from_items(entry.clone(), window, cx).into()
                    })
                    .trigger(
                        Button::new(
                            SharedString::from(format!("{}-menu-trigger", menu_name)),
                            menu_name,
                        )
                        .style(ButtonStyle::Subtle)
                        .label_size(LabelSize::Small)
                        .tab_index(0isize),
                    )
                    .with_handle(current_handle.clone()),
            )
            .on_hover(Self::switch_on_hover(current_handle, all_handles))
    }

    /// "…": the menus the bar has no room to write out.
    fn render_overflow_menu(
        &self,
        entries: Vec<MenuEntry>,
        all_handles: Vec<PopoverMenuHandle<ContextMenu>>,
    ) -> impl IntoElement {
        let handle = self.overflow.clone();
        div()
            .id("more-menus-item")
            .occlude()
            .child(
                PopoverMenu::new("more-menus-popover")
                    .menu(move |window, cx| {
                        Self::build_overflow_menu(entries.clone(), window, cx).into()
                    })
                    .trigger_with_tooltip(
                        IconButton::new("more-menus-trigger", ui::IconName::Ellipsis)
                            .style(ButtonStyle::Subtle)
                            .icon_size(IconSize::Small)
                            .tab_index(0isize)
                            .aria_label("More Menus"),
                        Tooltip::text("More Menus"),
                    )
                    .with_handle(handle.clone()),
            )
            .on_hover(Self::switch_on_hover(handle, all_handles))
    }

    #[cfg(not(target_os = "macos"))]
    pub fn open_menu(
        &mut self,
        action: &OpenApplicationMenu,
        _window: &mut Window,
        _cx: &mut Context<Self>,
    ) {
        let name = if action.0 == APP_MENU_ALIAS {
            branding::PRODUCT_NAME.to_string()
        } else {
            action.0.clone()
        };
        self.pending_menu_open = Some(name);
    }

    #[cfg(not(target_os = "macos"))]
    pub fn navigate_menus_in_direction(
        &mut self,
        direction: ActivateDirection,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let handles = self.shown_handles(self.written_out(window, cx));
        let current_index = handles.iter().position(|handle| handle.is_deployed());
        let Some(current_index) = current_index else {
            // No menu is open, so there is nothing to switch between. Let the
            // arrow key continue to the title bar's toolbar navigation so it
            // can move focus between title bar controls. Without this, the
            // `left`/`right` bindings in the `ApplicationMenu` context would
            // silently swallow the key as a no-op.
            cx.propagate();
            return;
        };

        let next_index = match direction {
            ActivateDirection::Left => {
                if current_index == 0 {
                    handles.len() - 1
                } else {
                    current_index - 1
                }
            }
            ActivateDirection::Right => {
                if current_index == handles.len() - 1 {
                    0
                } else {
                    current_index + 1
                }
            }
        };

        handles[current_index].hide(cx);

        // We need to defer this so that this menu handle can take focus from the previous menu
        let next_handle = handles[next_index].clone();
        cx.defer_in(window, move |_, window, cx| next_handle.show(window, cx));
    }
}

pub(crate) fn show_menus(cx: &App) -> bool {
    TitleBarSettings::get_global(cx).show_menus
        && (cfg!(not(target_os = "macos")) || option_env!("IDE_USE_CROSS_PLATFORM_MENU").is_some())
}

pub(crate) fn open_menus_on_hover(cx: &mut App) -> bool {
    TitleBarSettings::get_global(cx).open_menus_on_hover
}

/// The product's mark at the size of a title bar icon: two overlapping
/// squares, the neutral one behind, the red one in front. The same drawing as
/// the welcome page's (`workspace::welcome`), which is private to that crate.
fn brand_mark(cx: &App) -> Div {
    // The logo's own red, in every theme: the one fixed colour allowed outside the theme tokens.
    const BRAND_RED: u32 = 0xe5322d;
    // In the logo each square is 41/68 of the mark's side.
    let side = rems_from_px(14_f32);
    let square = rems_from_px(8.5_f32);

    div()
        .relative()
        .flex_none()
        .size(side)
        .child(
            div()
                .absolute()
                .top_0()
                .left_0()
                .size(square)
                // The text colour, so the square shows on light and dark grounds.
                .bg(cx.theme().colors().text),
        )
        .child(
            div()
                .absolute()
                .bottom_0()
                .right_0()
                .size(square)
                .bg(gpui::rgb(BRAND_RED)),
        )
}

impl Render for ApplicationMenu {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let written_out = self.written_out(window, cx);
        let handles = self.shown_handles(written_out);

        // A key binding asked for a menu by its name: the one written out,
        // or "…" when the menu is under it.
        if let Some(pending_menu_open) = self.pending_menu_open.take() {
            let index = self
                .entries
                .iter()
                .position(|entry| entry.menu.name == pending_menu_open);
            let handle_to_show = index.map(|index| {
                if index <= written_out {
                    self.entries[index].handle.clone()
                } else {
                    self.overflow.clone()
                }
            });
            if let Some(handle_to_show) = handle_to_show.filter(|handle| !handle.is_deployed()) {
                handles
                    .iter()
                    .filter(|handle| handle.is_deployed())
                    .for_each(|handle| handle.hide(cx));
                cx.defer_in(window, move |_, window, cx| handle_to_show.show(window, cx));
            }
        }

        let under_more: Vec<MenuEntry> =
            self.entries.iter().skip(written_out + 1).cloned().collect();

        div()
            .key_context("ApplicationMenu")
            .flex()
            .flex_row()
            .items_center()
            .gap_x_0p5()
            .when_some(self.entries.first(), |this, entry| {
                this.child(self.render_application_menu(entry, handles.clone(), cx))
            })
            .children(
                self.entries
                    .iter()
                    .skip(1)
                    .take(written_out)
                    .map(|entry| self.render_standard_menu(entry, handles.clone())),
            )
            .when(!under_more.is_empty(), |this| {
                this.child(self.render_overflow_menu(under_more, handles.clone()))
            })
    }
}

#[cfg(test)]
mod tests {
    use gpui::{Action, Menu, MenuItem, OwnedMenu, OwnedMenuItem, TestAppContext, UpdateGlobal};
    use project::DisableAiSettings;
    use settings::{Settings, SettingsStore};

    use crate::application_menu::{ApplicationMenu, MenuEntry, OpenApplicationMenu};

    fn init_test(cx: &mut TestAppContext) {
        cx.update(|cx| {
            let settings_store = SettingsStore::test(cx);
            cx.set_global(settings_store);
            DisableAiSettings::register(cx);
            theme_settings::init(theme::LoadThemes::JustBase, cx);
        });
    }

    fn owned_menu(name: &str, item_name: &str) -> OwnedMenu {
        OwnedMenu {
            name: name.into(),
            disabled: false,
            items: vec![OwnedMenuItem::Action {
                name: item_name.to_string(),
                action: OpenApplicationMenu(String::new()).boxed_clone(),
                os_action: None,
                checked: false,
                disabled: false,
            }],
        }
    }

    fn has_item(entries: &[MenuEntry], menu_name: &str, item_name: &str) -> bool {
        entries.iter().any(|entry| {
            entry.menu.name == menu_name
                && entry.menu.items.iter().any(
                    |item| matches!(item, OwnedMenuItem::Action { name, .. } if name == item_name),
                )
        })
    }

    fn menu_entry<'a>(entries: &'a [MenuEntry], menu_name: &str) -> &'a MenuEntry {
        entries
            .iter()
            .find(|entry| entry.menu.name == menu_name)
            .expect("expected menu")
    }

    fn view_menu(include_agent_panel: bool) -> Menu {
        let mut items = Vec::new();
        if include_agent_panel {
            items.push(MenuItem::action(
                "Agent Panel",
                OpenApplicationMenu(String::new()),
            ));
        }
        items.push(MenuItem::action(
            "Diagnostics",
            OpenApplicationMenu(String::new()),
        ));

        Menu {
            name: "View".into(),
            items,
            disabled: false,
        }
    }

    #[test]
    fn test_build_entries_reflects_current_menu_data() {
        let with_agent_panel =
            ApplicationMenu::build_entries(vec![owned_menu("View", "Agent Panel")]);
        assert!(has_item(&with_agent_panel, "View", "Agent Panel"));

        let without_agent_panel =
            ApplicationMenu::build_entries(vec![owned_menu("View", "Diagnostics")]);
        assert!(!has_item(&without_agent_panel, "View", "Agent Panel"));
    }

    #[gpui::test]
    fn test_entries_refresh_on_settings_change(cx: &mut TestAppContext) {
        init_test(cx);

        cx.update(|cx| {
            cx.set_menus(vec![view_menu(true)]);
        });

        let (app_menu, cx) = cx.add_window_view(|window, cx| ApplicationMenu::new(window, cx));
        let view_handle = cx.update(|window, cx| {
            let view_handle = app_menu.read_with(cx, |app_menu, _| {
                menu_entry(&app_menu.entries, "View").handle.clone()
            });
            view_handle.show(window, cx);
            view_handle
        });
        assert!(view_handle.is_deployed());

        assert!(app_menu.read_with(cx, |app_menu, _| has_item(
            &app_menu.entries,
            "View",
            "Agent Panel"
        )));

        assert!(app_menu.read_with(cx, |app_menu, _| has_item(
            &app_menu.entries,
            "View",
            "Diagnostics"
        )));

        cx.update(|_, cx| {
            cx.set_menus(vec![view_menu(false)]);
            DisableAiSettings::override_global(DisableAiSettings { disable_ai: true }, cx);
        });

        assert!(!view_handle.is_deployed());
        assert!(
            !app_menu.read_with(cx, |app_menu, _| has_item(
                &app_menu.entries,
                "View",
                "Agent Panel"
            )),
            "expected Agent Panel is not in the View menu"
        );

        assert!(app_menu.read_with(cx, |app_menu, _| has_item(
            &app_menu.entries,
            "View",
            "Diagnostics"
        )));

        cx.update(|_, cx| {
            cx.set_menus(vec![view_menu(true)]);
            DisableAiSettings::override_global(DisableAiSettings { disable_ai: false }, cx);
        });

        assert!(app_menu.read_with(cx, |app_menu, _| has_item(
            &app_menu.entries,
            "View",
            "Agent Panel"
        )));
        assert!(app_menu.read_with(cx, |app_menu, _| has_item(
            &app_menu.entries,
            "View",
            "Diagnostics"
        )));
    }

    #[gpui::test]
    fn test_entries_preserve_deployed_menu_on_unrelated_settings_change(cx: &mut TestAppContext) {
        init_test(cx);

        cx.update(|cx| {
            cx.set_menus(vec![
                Menu::new(branding::PRODUCT_NAME).items([MenuItem::action(
                    "Settings",
                    OpenApplicationMenu(String::new()),
                )]),
                view_menu(true),
            ]);
        });

        let (app_menu, cx) = cx.add_window_view(|window, cx| ApplicationMenu::new(window, cx));

        let product_handle = cx.update(|window, cx| {
            let product_handle = app_menu.read_with(cx, |app_menu, _| {
                menu_entry(&app_menu.entries, branding::PRODUCT_NAME)
                    .handle
                    .clone()
            });
            product_handle.show(window, cx);
            product_handle
        });
        cx.run_until_parked();
        assert!(product_handle.is_deployed());

        cx.update(|window, cx| {
            let view_handle = app_menu.read_with(cx, |app_menu, _| {
                menu_entry(&app_menu.entries, "View").handle.clone()
            });
            product_handle.hide(cx);
            view_handle.show(window, cx);
        });
        cx.run_until_parked();
        assert!(!product_handle.is_deployed());

        assert!(app_menu.read_with(cx, |app_menu, _| {
            menu_entry(&app_menu.entries, "View").handle.is_deployed()
        }));

        cx.update(|_, cx| {
            SettingsStore::update_global(cx, |_, _| {});
        });
        cx.run_until_parked();

        assert!(app_menu.read_with(cx, |app_menu, _| {
            menu_entry(&app_menu.entries, "View").handle.is_deployed()
        }));
    }
}

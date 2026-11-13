//! The theme tiles of the Appearance page: a miniature editor drawn from a
//! [`Theme`], the name under it, a live preview while the pointer is on it.

use std::sync::Arc;

use gpui::{App, Global, Hsla, Role, SharedString, Window};
use settings::Settings as _;
use theme::{GlobalTheme, Theme, ThemeRegistry};
use theme_settings::ThemeSettings;
use ui::{SectionLabel, prelude::*};

const TILE_WIDTH: Pixels = px(160.);
const TILE_HEIGHT: Pixels = px(100.);

/// The prefix shared by the built-in family: its tiles come first.
const FAMILY_PREFIX: &str = "Unaligned";

/// The family in the order it is presented, whatever the registry's order.
const FAMILY_ORDER: &[&str] = &[
    "Unaligned",
    "Unaligned Light",
    "Unaligned Dusk",
    "Unaligned Midnight",
    "Unaligned Contrast",
];

/// The other bundled families, in the order they are presented. Themes that
/// belong to none of them (installed extensions) follow, by name.
const BUNDLED_PREFIXES: &[&str] = &["One ", "Ayu ", "Gruvbox "];

/// The theme a tile is showing on the whole app while the pointer is on it.
#[derive(Default)]
struct ThemeTilePreview(Option<SharedString>);

impl Global for ThemeTilePreview {}

/// Whether a tile's preview is replacing the saved theme right now.
pub fn theme_preview_active(cx: &App) -> bool {
    cx.try_global::<ThemeTilePreview>()
        .is_some_and(|preview| preview.0.is_some())
}

/// Puts the saved theme back if a tile's preview replaced it.
pub fn end_theme_preview(cx: &mut App) {
    if theme_preview_active(cx) {
        cx.set_global(ThemeTilePreview(None));
        theme_settings::reload_theme(cx);
    }
}

fn begin_theme_preview(name: &SharedString, cx: &mut App) {
    let Ok(theme) = ThemeRegistry::global(cx).get(name) else {
        return;
    };
    // Same overrides as the saved theme gets (corners, accent, per-theme
    // overrides), so the preview is what a click would give.
    let theme = ThemeSettings::get_global(cx).apply_theme_overrides(theme);
    cx.set_global(ThemeTilePreview(Some(name.clone())));
    GlobalTheme::update_theme(cx, theme);
    cx.refresh_windows();
}

fn end_theme_preview_of(name: &SharedString, cx: &mut App) {
    // Moving from one tile to the next may deliver "entered B" before
    // "left A": only the tile that owns the preview may end it.
    let owns_preview = cx
        .try_global::<ThemeTilePreview>()
        .is_some_and(|preview| preview.0.as_ref() == Some(name));
    if owns_preview {
        end_theme_preview(cx);
    }
}

/// The names of the registry split in two: the built-in family, then the rest.
pub(crate) fn grouped_theme_names(
    names: Vec<SharedString>,
) -> (Vec<SharedString>, Vec<SharedString>) {
    let (mut family, mut others): (Vec<_>, Vec<_>) = names
        .into_iter()
        .partition(|name| name.starts_with(FAMILY_PREFIX));

    let family_rank = |name: &SharedString| {
        FAMILY_ORDER
            .iter()
            .position(|known| *known == name.as_ref())
            .unwrap_or(FAMILY_ORDER.len())
    };
    family.sort_by(|a, b| family_rank(a).cmp(&family_rank(b)).then_with(|| a.cmp(b)));

    let bundled_rank = |name: &SharedString| {
        BUNDLED_PREFIXES
            .iter()
            .position(|prefix| name.starts_with(prefix))
            .unwrap_or(BUNDLED_PREFIXES.len())
    };
    others.sort_by(|a, b| bundled_rank(a).cmp(&bundled_rank(b)).then_with(|| a.cmp(b)));

    (family, others)
}

/// A miniature editor drawn with a theme's own colours.
#[derive(IntoElement)]
pub struct ThemeTile {
    theme: Arc<Theme>,
    selected: bool,
}

impl ThemeTile {
    pub fn new(theme: Arc<Theme>) -> Self {
        Self {
            theme,
            selected: false,
        }
    }

    pub fn selected(mut self, selected: bool) -> Self {
        self.selected = selected;
        self
    }
}

fn bar(width: Pixels, color: Hsla) -> Div {
    div().h(px(3.)).w(width).bg(color)
}

impl RenderOnce for ThemeTile {
    fn render(self, _window: &mut Window, cx: &mut App) -> impl IntoElement {
        let colors = self.theme.colors();
        let syntax = self.theme.syntax();
        let syntax_color = |name: &str| {
            syntax
                .style_for_name(name)
                .and_then(|style| style.color)
                .unwrap_or(colors.text)
        };
        let keyword = syntax_color("keyword");
        let function = syntax_color("function");
        let string = syntax_color("string");
        let type_color = syntax_color("type");
        let comment = syntax_color("comment");
        let text = colors.text;

        let code_line = |indent: f32, bars: &[(f32, Hsla)]| {
            h_flex()
                .gap(px(3.))
                .pl(px(indent))
                .children(bars.iter().map(|(width, color)| bar(px(*width), *color)))
        };

        let outline = if self.selected {
            cx.theme().colors().text_accent
        } else {
            gpui::transparent_black()
        };

        // The 2px outline of the selected tile is the current theme's accent;
        // everything inside it belongs to the theme the tile shows.
        div().border_2().border_color(outline).child(
            v_flex()
                .w(TILE_WIDTH)
                .h(TILE_HEIGHT)
                .overflow_hidden()
                .border_1()
                .border_color(colors.border_strong)
                .bg(colors.background)
                .child(
                    h_flex()
                        .h(px(9.))
                        .flex_none()
                        .px(px(5.))
                        .bg(colors.title_bar_background)
                        .border_b_1()
                        .border_color(colors.border)
                        .child(bar(px(22.), colors.text_muted.opacity(0.6))),
                )
                .child(
                    h_flex()
                        .flex_1()
                        .min_h_0()
                        .items_stretch()
                        .child(
                            v_flex()
                                .w(px(38.))
                                .flex_none()
                                .p(px(5.))
                                .gap(px(4.))
                                .bg(colors.panel_background)
                                .border_r_1()
                                .border_color(colors.border)
                                .child(bar(px(20.), colors.text_muted.opacity(0.7)))
                                .child(bar(px(26.), colors.text_muted.opacity(0.7)))
                                .child(
                                    div()
                                        .mx(px(-5.))
                                        .px(px(5.))
                                        .py(px(2.))
                                        .bg(colors.element_selected)
                                        .child(bar(px(18.), text)),
                                )
                                .child(bar(px(24.), colors.text_muted.opacity(0.7)))
                                .child(bar(px(14.), colors.text_muted.opacity(0.7))),
                        )
                        .child(
                            v_flex()
                                .flex_1()
                                .min_w_0()
                                .bg(colors.editor_background)
                                .child(
                                    h_flex()
                                        .h(px(11.))
                                        .flex_none()
                                        .bg(colors.tab_bar_background)
                                        .border_b_1()
                                        .border_color(colors.border)
                                        .child(
                                            v_flex()
                                                .h_full()
                                                .w(px(34.))
                                                .bg(colors.tab_active_background)
                                                .border_r_1()
                                                .border_color(colors.border)
                                                .child(
                                                    div().h(px(2.)).w_full().bg(colors.text_accent),
                                                ),
                                        ),
                                )
                                .child(
                                    v_flex()
                                        .flex_1()
                                        .min_h_0()
                                        .p(px(6.))
                                        .gap(px(4.))
                                        .child(code_line(0., &[(34., comment)]))
                                        .child(code_line(0., &[(12., keyword), (26., function)]))
                                        .child(code_line(
                                            8.,
                                            &[(10., keyword), (16., text), (24., string)],
                                        ))
                                        .child(code_line(8., &[(18., type_color), (30., text)]))
                                        .child(code_line(16., &[(22., function), (14., string)]))
                                        .child(code_line(0., &[(6., text)])),
                                )
                                .child(
                                    h_flex().flex_none().justify_end().p(px(5.)).child(
                                        div()
                                            .h(px(8.))
                                            .w(px(26.))
                                            .rounded_sm()
                                            .bg(colors.accent_background),
                                    ),
                                ),
                        ),
                )
                .child(
                    div()
                        .h(px(7.))
                        .flex_none()
                        .bg(colors.status_bar_background)
                        .border_t_1()
                        .border_color(colors.border),
                ),
        )
    }
}

/// The grid of the Appearance page: the built-in family, then every other
/// theme of the registry under "OTHER THEMES", folded until asked for.
///
/// `current` is the saved theme of the slot a click writes to. Hovering a
/// tile shows its theme on the whole app until the pointer leaves it.
pub fn render_theme_grid(
    current: SharedString,
    on_select: impl Fn(SharedString, &mut Window, &mut App) + 'static,
    window: &mut Window,
    cx: &mut App,
) -> impl IntoElement {
    let registry = ThemeRegistry::global(cx);
    let (family, others) = grouped_theme_names(registry.list_names());
    let other_count = others.len();

    // The other themes stay folded so that mode, corners, accent and density
    // are on the first screen; they open by themselves when one of them is in use.
    let current_is_other = others.contains(&current);
    let others_open = window.use_keyed_state("other-themes-open", cx, |_, _| current_is_other);
    let show_others = *others_open.read(cx);
    let others = if show_others { others } else { Vec::new() };
    let on_select: Arc<dyn Fn(SharedString, &mut Window, &mut App)> = Arc::new(on_select);

    let tile = |name: SharedString, cx: &mut App| {
        let theme = registry.get(&name).ok()?;
        let selected = name == current;
        let on_select = on_select.clone();
        let label_color = if selected {
            Color::Default
        } else {
            Color::Muted
        };
        Some(
            v_flex()
                .id(SharedString::from(format!("theme-tile-{name}")))
                .role(Role::Button)
                .aria_label(name.clone())
                .tab_index(0_isize)
                .gap_1()
                .cursor_pointer()
                .focus_visible(|style| style.bg(cx.theme().colors().element_hover))
                .child(ThemeTile::new(theme).selected(selected))
                .child(
                    div().px(px(2.)).w(TILE_WIDTH + px(6.)).child(
                        Label::new(name.clone())
                            .size(LabelSize::Small)
                            .color(label_color)
                            .truncate(),
                    ),
                )
                .on_hover({
                    let name = name.clone();
                    move |hovered, _window, cx| {
                        if *hovered {
                            begin_theme_preview(&name, cx);
                        } else {
                            end_theme_preview_of(&name, cx);
                        }
                    }
                })
                .on_click(move |_, window, cx| {
                    on_select(name.clone(), window, cx);
                }),
        )
    };

    let family_tiles = family
        .into_iter()
        .filter_map(|name| tile(name, cx))
        .collect::<Vec<_>>();
    let other_tiles = others
        .into_iter()
        .filter_map(|name| tile(name, cx))
        .collect::<Vec<_>>();

    v_flex()
        .w_full()
        .gap_3()
        .child(h_flex().flex_wrap().gap_3().children(family_tiles))
        .when(other_count > 0, |this| {
            this.child(
                h_flex()
                    .id("other-themes")
                    .role(Role::Button)
                    .aria_label("Other Themes")
                    .aria_expanded(show_others)
                    .tab_index(0_isize)
                    .gap_1()
                    .cursor_pointer()
                    .focus_visible(|style| style.bg(cx.theme().colors().element_hover))
                    .child(
                        Icon::new(if show_others {
                            IconName::ChevronDown
                        } else {
                            IconName::ChevronRight
                        })
                        .size(IconSize::Small)
                        .color(Color::Muted),
                    )
                    .child(SectionLabel::new(format!("Other Themes · {other_count}")))
                    .on_click(move |_, window, cx| {
                        others_open.update(cx, |open, cx| {
                            *open = !*open;
                            cx.notify();
                        });
                        window.refresh();
                    }),
            )
            .when(show_others, |this| {
                this.child(h_flex().flex_wrap().gap_3().children(other_tiles))
            })
        })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_family_comes_first_in_its_own_order() {
        let names = [
            "Ayu Dark",
            "Gruvbox Dark",
            "One Light",
            "Unaligned Midnight",
            "Catppuccin Mocha",
            "Unaligned Light",
            "Unaligned",
            "One Dark",
            "Unaligned Sand",
            "Unaligned Contrast",
            "Unaligned Dusk",
        ]
        .into_iter()
        .map(SharedString::new_static)
        .collect();

        let (family, others) = grouped_theme_names(names);

        assert_eq!(
            family,
            [
                "Unaligned",
                "Unaligned Light",
                "Unaligned Dusk",
                "Unaligned Midnight",
                "Unaligned Contrast",
                "Unaligned Sand",
            ]
        );
        assert_eq!(
            others,
            [
                "One Dark",
                "One Light",
                "Ayu Dark",
                "Gruvbox Dark",
                "Catppuccin Mocha",
            ]
        );
    }
}

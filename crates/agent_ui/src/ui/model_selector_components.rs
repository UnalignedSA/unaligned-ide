use app_actions::agent::ToggleModelSelector;
use gpui::{Action, ClickEvent, FocusHandle, prelude::*};
use language_model::{DisabledReason, LanguageModelRegistry};
use ui::{
    Chip, ElevationIndex, KeyBinding, ListItem, ListItemSpacing, SectionLabel, Tooltip, prelude::*,
};

use crate::{CycleFavoriteModels, ManageProviders};

enum ModelIcon {
    Name(IconName),
    Path(SharedString),
}

/// A price as the picker shows it: the provider's `$3/$15` (input/output per
/// million tokens) with air around the slash. Digits are never touched, so
/// the price stays exact; anything that is not such a pair is kept as it is.
pub(crate) fn spaced_price(cost: &str) -> SharedString {
    match cost.split_once('/') {
        Some((input, output))
            if input.starts_with('$') && output.starts_with('$') && !output.contains('/') =>
        {
            format!("{input} / {output}").into()
        }
        _ => SharedString::from(cost.to_string()),
    }
}

/// The size of a context window as a short hint: `128K`, `400K`, `1M`, `1.05M`.
pub(crate) fn context_hint(tokens: u64) -> Option<SharedString> {
    match tokens {
        0 => None,
        1..=999 => Some(tokens.to_string().into()),
        1_000..=999_499 => Some(format!("{}K", (tokens + 500) / 1_000).into()),
        _ => {
            let millions = format!("{:.2}", tokens as f64 / 1_000_000.);
            let millions = millions.trim_end_matches('0').trim_end_matches('.');
            Some(format!("{millions}M").into())
        }
    }
}

/// Asks every connected provider for its model list again.
pub(crate) fn refresh_connected_models(cx: &mut App) {
    let providers = LanguageModelRegistry::read_global(cx).visible_providers();
    for provider in providers {
        if provider.is_authenticated(cx) {
            provider.refresh_models(cx);
        }
    }
}

/// The label of a group of the model picker.
#[derive(IntoElement)]
pub struct ModelSelectorHeader {
    title: SharedString,
    has_border: bool,
    is_sub_group: bool,
    detail: Option<SharedString>,
}

/// What the prices of the rows are, said once at the top of the list.
pub(crate) const PRICE_UNIT: &str = "$ per 1M tokens · in / out";

impl ModelSelectorHeader {
    pub fn new(title: impl Into<SharedString>, has_border: bool) -> Self {
        Self {
            title: title.into(),
            has_border,
            is_sub_group: false,
            detail: None,
        }
    }

    /// A note at the right end of the label's line, in the same style: the
    /// unit of the column under it.
    pub fn detail(mut self, detail: Option<impl Into<SharedString>>) -> Self {
        self.detail = detail.map(Into::into);
        self
    }

    /// A group that continues the one above it (the next maker of the same
    /// connection): set apart by space, without a rule.
    pub fn sub_group(mut self, is_sub_group: bool) -> Self {
        self.is_sub_group = is_sub_group;
        self
    }
}

impl RenderOnce for ModelSelectorHeader {
    fn render(self, _window: &mut Window, cx: &mut App) -> impl IntoElement {
        div()
            .px_2()
            .pb_1()
            .when(self.has_border, |this| {
                this.mt_1()
                    .pt_2()
                    .border_t_1()
                    .border_color(cx.theme().colors().border_variant)
            })
            .when(self.is_sub_group && !self.has_border, |this| this.pt_2())
            .child(
                h_flex()
                    .gap_2()
                    .justify_between()
                    .child(SectionLabel::new(self.title))
                    .when_some(self.detail, |this, detail| {
                        this.child(div().flex_none().child(SectionLabel::new(detail)))
                    }),
            )
    }
}

/// The width kept at the right end of every row for the check mark and, on
/// hover, the favourite star.
const END_SLOT_WIDTH: f32 = 28.;

/// How a model is doing, when its provider says so.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub(crate) enum ModelHealth {
    #[default]
    Up,
    Degraded,
    Down,
}

#[derive(IntoElement)]
pub struct ModelSelectorListItem {
    index: usize,
    title: SharedString,
    secondary_line: Option<SharedString>,
    icon: Option<ModelIcon>,
    is_selected: bool,
    is_focused: bool,
    is_latest: bool,
    is_favorite: bool,
    tag: Option<&'static str>,
    disabled: Option<DisabledReason>,
    health: ModelHealth,
    on_toggle_favorite: Option<Box<dyn Fn(&ClickEvent, &mut Window, &mut App) + 'static>>,
    cost_info: Option<SharedString>,
    context_hint: Option<SharedString>,
}

impl ModelSelectorListItem {
    pub fn new(index: usize, title: impl Into<SharedString>) -> Self {
        Self {
            index,
            title: title.into(),
            secondary_line: None,
            icon: None,
            is_selected: false,
            is_focused: false,
            is_latest: false,
            is_favorite: false,
            tag: None,
            disabled: None,
            health: ModelHealth::Up,
            on_toggle_favorite: None,
            cost_info: None,
            context_hint: None,
        }
    }

    pub fn icon(mut self, icon: IconName) -> Self {
        self.icon = Some(ModelIcon::Name(icon));
        self
    }

    pub fn icon_path(mut self, path: SharedString) -> Self {
        self.icon = Some(ModelIcon::Path(path));
        self
    }

    pub fn is_selected(mut self, is_selected: bool) -> Self {
        self.is_selected = is_selected;
        self
    }

    pub fn disabled(mut self, disabled: Option<DisabledReason>) -> Self {
        self.disabled = disabled;
        self
    }

    pub fn is_focused(mut self, is_focused: bool) -> Self {
        self.is_focused = is_focused;
        self
    }

    pub fn is_latest(mut self, is_latest: bool) -> Self {
        self.is_latest = is_latest;
        self
    }

    pub fn is_favorite(mut self, is_favorite: bool) -> Self {
        self.is_favorite = is_favorite;
        self
    }

    pub fn on_toggle_favorite(
        mut self,
        handler: impl Fn(&ClickEvent, &mut Window, &mut App) + 'static,
    ) -> Self {
        self.on_toggle_favorite = Some(Box::new(handler));
        self
    }

    /// The price as the provider prints it (`$3/$15`, `0.33×`).
    pub fn cost_info(mut self, cost_info: Option<SharedString>) -> Self {
        self.cost_info = cost_info;
        self
    }

    /// The context window as a short hint (`400K`), see [`context_hint`].
    pub(crate) fn context_hint(mut self, context_hint: Option<SharedString>) -> Self {
        self.context_hint = context_hint;
        self
    }

    /// A word under the name for the model to pick without reading prices
    /// ("Recommended", "Fast").
    pub(crate) fn tag(mut self, tag: Option<&'static str>) -> Self {
        self.tag = tag;
        self
    }

    /// Marks a model its provider reports as degraded or down.
    pub(crate) fn health(mut self, health: ModelHealth) -> Self {
        self.health = health;
        self
    }

    /// A second, quieter line under the name, saying what choosing the row does.
    pub(crate) fn secondary_line(mut self, text: impl Into<SharedString>) -> Self {
        self.secondary_line = Some(text.into());
        self
    }
}

impl RenderOnce for ModelSelectorListItem {
    fn render(self, _window: &mut Window, cx: &mut App) -> impl IntoElement {
        let is_disabled = self.disabled.is_some();
        let index = self.index;

        let model_icon_color = if is_disabled {
            Color::Disabled
        } else if self.is_selected {
            Color::Default
        } else {
            Color::Muted
        };
        let detail_color = if is_disabled {
            Color::Disabled
        } else {
            Color::Muted
        };

        // The status red is the one colour a row may carry: it says the model
        // cannot answer. A slow model is only named.
        let health_label = match self.health {
            ModelHealth::Up => None,
            ModelHealth::Degraded => Some(("degraded", Color::Warning)),
            ModelHealth::Down => Some(("down", Color::Error)),
        };
        let shows_info_icon = is_disabled && health_label.is_none();

        let is_favorite = self.is_favorite;

        // The tag is the row's second line: said in full, and never at the
        // price of the name or the columns.
        let secondary_line = self.secondary_line.or(self
            .tag
            .filter(|_| !is_disabled)
            .map(SharedString::new_static));

        ListItem::new(index)
            .inset(true)
            .spacing(ListItemSpacing::Dense)
            .toggle_state(self.is_focused)
            .when_some(self.disabled, |this, disabled_reason| {
                this.disabled(true)
                    .tooltip(Tooltip::text(disabled_reason.0))
            })
            .child(
                h_flex()
                    .w_full()
                    .gap_2()
                    .justify_between()
                    .child(
                        h_flex()
                            .flex_1()
                            .min_w_0()
                            .gap_1p5()
                            .overflow_hidden()
                            .when_some(self.icon, |this, icon| {
                                this.child(
                                    match icon {
                                        ModelIcon::Name(icon_name) => Icon::new(icon_name),
                                        ModelIcon::Path(icon_path) => {
                                            Icon::from_external_svg(icon_path)
                                        }
                                    }
                                    .color(model_icon_color)
                                    .size(IconSize::Small),
                                )
                            })
                            .child(
                                v_flex()
                                    .min_w_0()
                                    .child(
                                        Label::new(self.title)
                                            .when(is_disabled, |this| this.color(Color::Disabled))
                                            .truncate(),
                                    )
                                    .when_some(secondary_line, |this, text| {
                                        this.child(
                                            Label::new(text)
                                                .size(LabelSize::Small)
                                                .color(Color::Muted)
                                                .truncate(),
                                        )
                                    }),
                            )
                            .when(self.is_latest, |parent| parent.child(Chip::new("Latest"))),
                    )
                    .child(
                        h_flex()
                            .flex_none()
                            .gap_2()
                            .when_some(health_label, |this, (text, color)| {
                                this.child(
                                    Label::new(text)
                                        .size(LabelSize::Small)
                                        .buffer_font(cx)
                                        .color(color),
                                )
                            })
                            .when_some(self.cost_info, |this, cost_info| {
                                let price = spaced_price(&cost_info);
                                let tooltip_text = if cost_info.ends_with('×') {
                                    format!("Cost multiplier per request: {price}")
                                } else if cost_info.contains('$') {
                                    format!("Input / output per million tokens: {price}")
                                } else {
                                    format!("Cost: {price}")
                                };

                                this.child(
                                    div()
                                        .id(("model-price", index))
                                        .child(
                                            Label::new(price)
                                                .size(LabelSize::Small)
                                                .buffer_font(cx)
                                                .color(detail_color),
                                        )
                                        .tooltip(Tooltip::text(tooltip_text)),
                                )
                            })
                            .when_some(self.context_hint, |this, context| {
                                let tooltip_text = format!("Context window: {context} tokens");
                                this.child(
                                    h_flex()
                                        .id(("model-context", index))
                                        .w(rems_from_px(38_f32))
                                        .justify_end()
                                        .child(
                                            Label::new(context)
                                                .size(LabelSize::Small)
                                                .buffer_font(cx)
                                                .color(detail_color),
                                        )
                                        .tooltip(Tooltip::text(tooltip_text)),
                                )
                            }),
                    ),
            )
            .end_slot(
                // The mark's place is kept on every row, as wide as the star
                // that takes it on hover, so that prices line up whether a
                // row is the current model or not.
                h_flex()
                    .w(rems_from_px(END_SLOT_WIDTH))
                    .justify_center()
                    .when(self.is_selected, |this| {
                        this.child(
                            Icon::new(IconName::Check)
                                .size(IconSize::Small)
                                .color(Color::Default),
                        )
                    })
                    .when(shows_info_icon && !self.is_selected, |this| {
                        this.child(
                            Icon::new(IconName::Info)
                                .size(IconSize::Small)
                                .color(Color::Muted),
                        )
                    }),
            )
            .when_some(
                self.on_toggle_favorite.filter(|_| !is_disabled),
                |this, handle_click| {
                    // A toggle that is on takes the text colour; the filled
                    // star says the rest.
                    let (icon, color, tooltip) = if is_favorite {
                        (IconName::StarFilled, Color::Default, "Unfavorite Model")
                    } else {
                        (IconName::Star, Color::Muted, "Favorite Model")
                    };
                    this.end_slot_on_hover(
                        h_flex()
                            .w(rems_from_px(END_SLOT_WIDTH))
                            .justify_center()
                            .child(
                                IconButton::new(("toggle-favorite", index), icon)
                                    .layer(ElevationIndex::ElevatedSurface)
                                    .icon_color(color)
                                    .icon_size(IconSize::Small)
                                    .tooltip(Tooltip::text(tooltip))
                                    .on_click(move |event, window, cx| {
                                        (handle_click)(event, window, cx)
                                    }),
                            ),
                    )
                },
            )
    }
}

/// The strip under the model list: the way to the connections, and a fresh
/// model list.
#[derive(IntoElement)]
pub struct ModelSelectorFooter {
    label: &'static str,
    action: Box<dyn Action>,
    focus_handle: FocusHandle,
    offers_refresh: bool,
}

impl ModelSelectorFooter {
    pub fn new(action: Box<dyn Action>, focus_handle: FocusHandle) -> Self {
        Self {
            label: "Configure",
            action,
            focus_handle,
            offers_refresh: false,
        }
    }

    /// The footer of the chat: "Connections…", where more models are
    /// connected, and "Refresh Models".
    pub fn connections(focus_handle: FocusHandle) -> Self {
        Self {
            label: "Connections…",
            action: Box::new(ManageProviders),
            focus_handle,
            offers_refresh: true,
        }
    }
}

impl RenderOnce for ModelSelectorFooter {
    fn render(self, window: &mut Window, cx: &mut App) -> impl IntoElement {
        let action = self.action;
        let focus_handle = self.focus_handle;

        // The footer every picker shares: a strip on the chrome ground with
        // subtle buttons, a key cap where the action has a binding.
        let key_cap = picker::footer_key_cap(action.as_ref(), &focus_handle, cx);
        let key_cap = key_cap.has_binding(window).then_some(key_cap);

        picker::footer(cx)
            .child(
                picker::footer_button("configure", self.label)
                    .key_binding(key_cap)
                    .on_click(move |_, window, cx| {
                        window.dispatch_action(action.boxed_clone(), cx);
                    }),
            )
            .when(self.offers_refresh, |this| {
                this.child(
                    picker::footer_button("refresh-models", "Refresh Models")
                        .tooltip(Tooltip::text(
                            "Ask every connected provider for its models again",
                        ))
                        .on_click(|_, _, cx| refresh_connected_models(cx)),
                )
            })
    }
}

/// What the model picker says when nothing is connected, with the way out
/// (see [`picker::PickerDelegate::no_matches_action`]). Never "No matches":
/// that is the answer to a query.
pub(crate) const NOTHING_CONNECTED: &str = "No model connected";

/// The action offered when nothing is connected.
pub(crate) fn connect_a_model_action() -> picker::PickerEmptyAction {
    picker::PickerEmptyAction::new("Connect a Model", Box::new(ManageProviders))
}

/// The tooltip of the model button: what the button does, then who answers
/// and at which price.
#[derive(IntoElement)]
pub struct ModelSelectorTooltip {
    show_cycle_row: bool,
    provider: Option<SharedString>,
    cost: Option<SharedString>,
}

impl ModelSelectorTooltip {
    pub fn new() -> Self {
        Self {
            show_cycle_row: true,
            provider: None,
            cost: None,
        }
    }

    pub fn show_cycle_row(mut self, show: bool) -> Self {
        self.show_cycle_row = show;
        self
    }

    /// The connection the current model answers through.
    pub fn provider(mut self, provider: Option<SharedString>) -> Self {
        self.provider = provider;
        self
    }

    /// The current model's price as the provider prints it (`$3/$15`).
    pub fn cost(mut self, cost: Option<SharedString>) -> Self {
        self.cost = cost;
        self
    }
}

impl RenderOnce for ModelSelectorTooltip {
    fn render(self, _window: &mut Window, cx: &mut App) -> impl IntoElement {
        let price_line = self.cost.map(|cost| {
            let price = spaced_price(&cost);
            if cost.ends_with('×') {
                format!("{price} per request")
            } else if cost.contains('$') {
                format!("{price} per million tokens, input / output")
            } else {
                price.to_string()
            }
        });

        v_flex()
            .gap_1()
            .child(
                h_flex()
                    .gap_2()
                    .justify_between()
                    .child(Label::new("Change Model"))
                    .child(KeyBinding::for_action(&ToggleModelSelector, cx)),
            )
            .when(self.provider.is_some() || price_line.is_some(), |this| {
                this.child(
                    v_flex()
                        .when_some(self.provider, |this, provider| {
                            this.child(
                                Label::new(format!("Through {provider}"))
                                    .size(LabelSize::Small)
                                    .color(Color::Muted),
                            )
                        })
                        .when_some(price_line, |this, price_line| {
                            this.child(
                                Label::new(price_line)
                                    .size(LabelSize::Small)
                                    .color(Color::Muted),
                            )
                        }),
                )
            })
            .when(self.show_cycle_row, |this| {
                this.child(
                    h_flex()
                        .pt_1()
                        .gap_2()
                        .border_t_1()
                        .border_color(cx.theme().colors().border_variant)
                        .justify_between()
                        .child(Label::new("Cycle Favorite Models"))
                        .child(KeyBinding::for_action(&CycleFavoriteModels, cx)),
                )
            })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_spaced_price_keeps_every_digit() {
        assert_eq!(spaced_price("$3/$15").as_ref(), "$3 / $15");
        assert_eq!(spaced_price("$0.375/$1.875").as_ref(), "$0.375 / $1.875");
        assert_eq!(spaced_price("$0.0625/$0.10").as_ref(), "$0.0625 / $0.10");
        // A request multiplier, or anything that is not a pair, is left alone.
        assert_eq!(spaced_price("0.33×").as_ref(), "0.33×");
        assert_eq!(spaced_price("free/beta").as_ref(), "free/beta");
    }

    #[test]
    fn test_context_hint() {
        assert_eq!(context_hint(0), None);
        assert_eq!(context_hint(128_000).unwrap().as_ref(), "128K");
        assert_eq!(context_hint(400_000).unwrap().as_ref(), "400K");
        assert_eq!(context_hint(1_000_000).unwrap().as_ref(), "1M");
        assert_eq!(context_hint(1_050_000).unwrap().as_ref(), "1.05M");
        assert_eq!(context_hint(2_000_000).unwrap().as_ref(), "2M");
    }
}

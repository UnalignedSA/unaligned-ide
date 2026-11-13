use gpui::FontWeight;

use crate::prelude::*;

/// The height of a [`CountBadge`].
#[derive(Debug, PartialEq, Eq, PartialOrd, Ord, Hash, Clone, Copy)]
pub enum CountBadgeSize {
    /// 14px high: the corner of an icon. What [`CountBadge::new`] starts with.
    Small,
    /// 16px high: next to a title or a tab label.
    Default,
}

impl CountBadgeSize {
    fn height(self) -> Rems {
        match self {
            CountBadgeSize::Small => rems_from_px(14_f32),
            CountBadgeSize::Default => rems_from_px(16_f32),
        }
    }

    fn padding_x(self) -> Rems {
        match self {
            CountBadgeSize::Small => rems_from_px(1_f32),
            CountBadgeSize::Default => rems_from_px(3_f32),
        }
    }
}

/// A small badge that displays a numeric count: a pill in themes with rounded
/// corners, a square in square themes.
///
/// The badge is neutral: the selected-element ground with the text colour. A
/// count is not a state and never takes the accent. Where the number itself is
/// the alert (unread notifications, errors), the caller asks for a tinted
/// ground with [`tint`](Self::tint).
///
/// The count is capped at 99 and displayed as "99+" beyond that, unless
/// [`max`](Self::max) says otherwise.
///
/// By default the badge sits on the top right corner of its parent, which
/// must be `relative()`. With [`inline`](Self::inline) it takes its place in
/// the flow instead, next to a title for example.
///
/// ```
/// use ui::{CountBadge, CountBadgeSize, prelude::*};
///
/// let corner = div().relative().child(CountBadge::new(3));
/// let titled = h_flex()
///     .gap_1()
///     .child(Label::new("Changes"))
///     .child(CountBadge::new(12).size(CountBadgeSize::Default).inline());
/// let alert = div()
///     .relative()
///     .child(CountBadge::new(3).tint(Color::Error));
/// ```
#[derive(IntoElement, RegisterComponent)]
pub struct CountBadge {
    count: usize,
    size: CountBadgeSize,
    inline: bool,
    tint: Option<Color>,
    max: usize,
}

impl CountBadge {
    pub fn new(count: usize) -> Self {
        Self {
            count,
            size: CountBadgeSize::Small,
            inline: false,
            tint: None,
            max: 99,
        }
    }

    /// Sets the height of the badge.
    pub fn size(mut self, size: CountBadgeSize) -> Self {
        self.size = size;
        self
    }

    /// Lays the badge out in the flow of its parent instead of pinning it to
    /// the parent's top right corner.
    pub fn inline(mut self) -> Self {
        self.inline = true;
        self
    }

    /// Tints the ground with a status colour, for a count that is itself an
    /// alert. `Color::Error` is the look every badge had upstream. Without
    /// this the badge is neutral.
    pub fn tint(mut self, color: Color) -> Self {
        self.tint = Some(color);
        self
    }

    /// The largest count shown as a number; beyond it the badge reads
    /// "{max}+". 99 by default.
    pub fn max(mut self, max: usize) -> Self {
        self.max = max;
        self
    }
}

impl RenderOnce for CountBadge {
    fn render(self, _window: &mut Window, cx: &mut App) -> impl IntoElement {
        let label = if self.count > self.max {
            format!("{}+", self.max)
        } else {
            self.count.to_string()
        };

        let colors = cx.theme().colors();
        let bg = match self.tint {
            Some(tint) => colors.editor_background.blend(tint.color(cx).opacity(0.4)),
            None => colors.element_selected,
        };

        h_flex()
            .map(|this| {
                if self.inline {
                    this.flex_none()
                } else {
                    this.absolute().top_0().right_0()
                }
            })
            .px(self.size.padding_x())
            .h(self.size.height())
            .min_w(self.size.height())
            .rounded_pill()
            .justify_center()
            .text_center()
            .border_1()
            .border_color(colors.border)
            .bg(bg)
            .child(
                // Mono, like every other small label, and never under the
                // smallest text size of the interface.
                Label::new(label)
                    .size(LabelSize::Custom(crate::micro_label_size()))
                    .line_height_style(LineHeightStyle::UiLabel)
                    .buffer_font(cx)
                    .weight(FontWeight::MEDIUM),
            )
    }
}

impl Component for CountBadge {
    fn scope() -> ComponentScope {
        ComponentScope::Status
    }

    fn description() -> &'static str {
        "A small badge that displays a numeric count, in two heights, \
        pinned to a corner or inline. Neutral unless the caller asks for a tint."
    }

    fn preview(_window: &mut Window, cx: &mut App) -> AnyElement {
        let container = || {
            div()
                .relative()
                .size_8()
                .border_1()
                .border_color(cx.theme().colors().border)
                .bg(cx.theme().colors().background)
        };
        let titled = |title: &'static str, count: usize, size: CountBadgeSize| {
            h_flex()
                .gap_1()
                .child(Label::new(title))
                .child(CountBadge::new(count).size(size).inline())
        };

        let cornered = |size: CountBadgeSize| {
            h_flex()
                .gap_6()
                .children(
                    [3, 42, 150].map(|count| container().child(CountBadge::new(count).size(size))),
                )
                .into_any_element()
        };
        let tinted = |tint: Color| {
            h_flex()
                .gap_6()
                .child(container().child(CountBadge::new(3).tint(tint)))
                .child(
                    container().child(CountBadge::new(42).size(CountBadgeSize::Default).tint(tint)),
                )
                .child(
                    h_flex().gap_1().child(Label::new("Unread")).child(
                        CountBadge::new(150)
                            .size(CountBadgeSize::Default)
                            .inline()
                            .tint(tint),
                    ),
                )
                .into_any_element()
        };

        v_flex()
            .gap_6()
            .child(example_group_with_title(
                "Neutral (default), on a corner: 3, 42, capped at 99",
                vec![
                    single_example("Small (14px)", cornered(CountBadgeSize::Small)),
                    single_example("Default (16px)", cornered(CountBadgeSize::Default)),
                ],
            ))
            .child(example_group_with_title(
                "Neutral, inline",
                vec![
                    single_example(
                        "Small, Default, Default Capped",
                        h_flex()
                            .gap_6()
                            .child(titled("Changes", 7, CountBadgeSize::Small))
                            .child(titled("Changes", 7, CountBadgeSize::Default))
                            .child(titled("Problems", 150, CountBadgeSize::Default))
                            .into_any_element(),
                    ),
                    single_example(
                        "Capped at 999 with .max(999)",
                        h_flex()
                            .gap_1()
                            .child(Label::new("Changes"))
                            .child(
                                CountBadge::new(1500)
                                    .size(CountBadgeSize::Default)
                                    .inline()
                                    .max(999),
                            )
                            .into_any_element(),
                    ),
                ],
            ))
            .child(example_group_with_title(
                "Tinted, only where a caller asks: .tint(..)",
                vec![
                    single_example("Color::Error", tinted(Color::Error)),
                    single_example("Color::Warning", tinted(Color::Warning)),
                ],
            ))
            .into_any_element()
    }
}

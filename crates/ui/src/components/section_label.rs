use gpui::Role;

use crate::{micro_label_size, prelude::*};

/// The micro label of the interface: code font, uppercase, 11px, faint.
///
/// Used for section labels, panel titles, table headers and field labels.
/// The text is written in sentence case by the caller and uppercased once
/// here; assistive technology still gets the original string.
///
/// # Usage Example
///
/// ```
/// use ui::{SectionLabel, prelude::*};
///
/// let label = SectionLabel::new("Open editors");
/// let quieter = SectionLabel::new("Timeline").color(Color::Muted);
/// ```
#[derive(IntoElement, RegisterComponent)]
pub struct SectionLabel {
    text: SharedString,
    color: Option<Color>,
}

impl SectionLabel {
    /// Creates a section label. `text` is shown in uppercase.
    pub fn new(text: impl Into<SharedString>) -> Self {
        Self {
            text: text.into(),
            color: None,
        }
    }

    /// Replaces the faint text colour, for example with [`Color::Default`]
    /// on the title of the thing that is currently open.
    pub fn color(mut self, color: Color) -> Self {
        self.color = Some(color);
        self
    }
}

impl RenderOnce for SectionLabel {
    fn render(self, _window: &mut Window, cx: &mut App) -> impl IntoElement {
        let color = self
            .color
            .unwrap_or_else(|| Color::Custom(cx.theme().colors().text_faint));

        div()
            .id(self.text.clone())
            .role(Role::Label)
            .aria_label(self.text.clone())
            .min_w_0()
            .child(
                Label::new(self.text.to_uppercase())
                    .size(LabelSize::Custom(micro_label_size()))
                    .color(color)
                    .buffer_font(cx)
                    .truncate(),
            )
    }
}

impl Component for SectionLabel {
    fn scope() -> ComponentScope {
        ComponentScope::Typography
    }

    fn description() -> &'static str {
        "The section label: the mono, uppercase, 11px micro label used for \
        section labels, panel titles, table headers and field labels."
    }

    fn preview(_window: &mut Window, _cx: &mut App) -> AnyElement {
        v_flex()
            .gap_6()
            .child(
                example_group(vec![
                    single_example(
                        "Default (faint), muted, default text colour",
                        h_flex()
                            .gap_8()
                            .child(SectionLabel::new("Open editors"))
                            .child(SectionLabel::new("Timeline").color(Color::Muted))
                            .child(SectionLabel::new("Explorer").color(Color::Default))
                            .into_any_element(),
                    ),
                    single_example(
                        "Truncated",
                        div()
                            .w_32()
                            .child(SectionLabel::new("A section with a very long name"))
                            .into_any_element(),
                    ),
                ])
                .vertical(),
            )
            .into_any_element()
    }
}

use gpui::{IntoElement, ParentElement, Role, Styled};
use ui::{Divider, DividerColor, SectionLabel, prelude::*};

#[derive(IntoElement)]
pub struct SettingsSectionHeader {
    icon: Option<IconName>,
    label: SharedString,
    no_padding: bool,
}

impl SettingsSectionHeader {
    pub fn new(label: impl Into<SharedString>) -> Self {
        Self {
            label: label.into(),
            icon: None,
            no_padding: false,
        }
    }

    pub fn icon(mut self, icon: IconName) -> Self {
        self.icon = Some(icon);
        self
    }

    pub fn no_padding(mut self, no_padding: bool) -> Self {
        self.no_padding = no_padding;
        self
    }
}

impl RenderOnce for SettingsSectionHeader {
    fn render(self, _: &mut Window, _cx: &mut App) -> impl IntoElement {
        let label_text = self.label.clone();
        // The shared micro-label: mono, uppercase, 11px, the readable faint step.
        let label = SectionLabel::new(self.label);

        v_flex()
            .id(label_text.clone())
            .role(Role::Heading)
            .aria_level(2)
            .aria_label(label_text)
            .w_full()
            .when(!self.no_padding, |this| this.px_8())
            .gap_1p5()
            .map(|this| {
                if let Some(icon) = self.icon {
                    this.child(
                        h_flex()
                            .gap_1p5()
                            .child(Icon::new(icon).size(IconSize::Small).color(Color::Muted))
                            .child(label),
                    )
                } else {
                    this.child(label)
                }
            })
            .child(Divider::horizontal().color(DividerColor::Border))
    }
}

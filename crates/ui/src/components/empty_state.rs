use gpui::{ClickEvent, Role, StyleRefinement};
use smallvec::SmallVec;

use crate::{KeyBinding, focus_ring, metrics, prelude::*};

const MAX_ACTIONS: usize = 3;
const CENTERED_MAX_WIDTH: Pixels = px(320.);

type ClickHandler = Box<dyn Fn(&ClickEvent, &mut Window, &mut App) + 'static>;

struct EmptyStateAction {
    label: SharedString,
    key_binding: Option<KeyBinding>,
    on_click: ClickHandler,
}

/// What a surface shows when it has nothing to show: what the place is for,
/// and the few things that can be done from here.
///
/// An optional 20px muted icon, one line of body text, an optional
/// description, at most one primary button and up to three actions as rows
/// "label … key cap". Inside a panel it is left-aligned on the 12px inset
/// that the panel header and list rows use; in the editor area it is
/// [`centered`](Self::centered).
///
/// The primary button is filled with the accent: give an empty state one only
/// when it is the single primary action of its region.
///
/// # Usage Example
///
/// ```
/// use ui::{EmptyState, prelude::*};
///
/// let empty = EmptyState::new("No folder open")
///     .icon(IconName::Folder)
///     .description("Open a folder to browse and edit its files.")
///     .primary_action(Button::primary("open-folder", "Open folder"))
///     .action("Clone repository", None, |_, _window, _cx| {})
///     .action("Open recent", None, |_, _window, _cx| {});
/// ```
#[derive(IntoElement, RegisterComponent)]
pub struct EmptyState {
    headline: SharedString,
    icon: Option<IconName>,
    description: Option<SharedString>,
    actions: SmallVec<[EmptyStateAction; MAX_ACTIONS]>,
    primary_action: Option<Button>,
    centered: bool,
}

impl EmptyState {
    /// Creates an empty state. `headline` is one line saying what this place
    /// is for or why it is empty.
    pub fn new(headline: impl Into<SharedString>) -> Self {
        Self {
            headline: headline.into(),
            icon: None,
            description: None,
            actions: SmallVec::new(),
            primary_action: None,
            centered: false,
        }
    }

    /// A 20px muted icon above the headline.
    pub fn icon(mut self, icon: IconName) -> Self {
        self.icon = Some(icon);
        self
    }

    /// A sentence or two under the headline, in the secondary text style.
    pub fn description(mut self, description: impl Into<SharedString>) -> Self {
        self.description = Some(description.into());
        self
    }

    /// Adds an action row: its label on the left, the key cap of its binding
    /// on the right. Up to three; further calls are ignored.
    pub fn action(
        mut self,
        label: impl Into<SharedString>,
        key_binding: Option<KeyBinding>,
        on_click: impl Fn(&ClickEvent, &mut Window, &mut App) + 'static,
    ) -> Self {
        if self.actions.len() < MAX_ACTIONS {
            self.actions.push(EmptyStateAction {
                label: label.into(),
                key_binding,
                on_click: Box::new(on_click),
            });
        }
        self
    }

    /// The one button of the region, shown above the action rows. Pass a
    /// `Button::primary(..)` with its click handler.
    pub fn primary_action(mut self, button: Button) -> Self {
        self.primary_action = Some(button);
        self
    }

    /// Centres the empty state in the space it is given, for the editor area.
    /// The default is left-aligned at the top, for panels.
    pub fn centered(mut self, centered: bool) -> Self {
        self.centered = centered;
        self
    }
}

impl RenderOnce for EmptyState {
    fn render(self, _window: &mut Window, cx: &mut App) -> impl IntoElement {
        let colors = cx.theme().colors();
        let centered = self.centered;
        let text_color = colors.text;
        let muted_color = colors.text_muted;
        let hover_background = colors.ghost_element_hover;
        let row_height = metrics::row_height(cx);

        let text = v_flex()
            .px_3()
            .gap_1()
            .when(centered, |this| this.items_center())
            .when_some(self.icon, |this, icon| {
                this.child(
                    div().pb_1().child(
                        Icon::new(icon)
                            .size(IconSize::Custom(rems_from_px(20_f32)))
                            .color(Color::Muted),
                    ),
                )
            })
            // An empty headline draws no line, so a mark above the rows is not followed
            // by a blank one.
            .when(!self.headline.is_empty(), |this| {
                this.child(
                    Label::new(self.headline.clone()).when(centered, |this| this.text_center()),
                )
            })
            .when_some(self.description, |this, description| {
                this.child(
                    Label::new(description)
                        .size(LabelSize::Small)
                        .color(Color::Muted)
                        .when(centered, |this| this.text_center()),
                )
            });

        let primary_action = self.primary_action.map(|button| {
            h_flex()
                .px_3()
                .when(centered, |this| this.justify_center())
                .child(button)
        });

        let actions = (!self.actions.is_empty()).then(|| {
            v_flex().children(self.actions.into_iter().enumerate().map(|(ix, action)| {
                let focus_ring = focus_ring(cx);
                h_flex()
                    .id(("empty-state-action", ix))
                    .role(Role::Button)
                    .aria_label(action.label.clone())
                    .tab_index(0)
                    .w_full()
                    .h(row_height)
                    .px_3()
                    .gap_3()
                    // "label … key cap"; a centred row with no key cap sits under the
                    // centred text, not at the left edge of the column.
                    .map(|this| {
                        if centered && action.key_binding.is_none() {
                            this.justify_center()
                        } else {
                            this.justify_between()
                        }
                    })
                    .cursor_pointer()
                    .text_ui(cx)
                    .text_color(muted_color)
                    .hover(move |style| style.bg(hover_background).text_color(text_color))
                    .focus_visible(move |mut style: StyleRefinement| {
                        style.box_shadow = Some(focus_ring);
                        style
                    })
                    .on_click(action.on_click)
                    .child(
                        div()
                            .min_w_0()
                            .overflow_x_hidden()
                            .whitespace_nowrap()
                            .text_ellipsis()
                            .child(action.label),
                    )
                    .children(action.key_binding)
            }))
        });

        let content = v_flex()
            .id(ElementId::Name(self.headline))
            .w_full()
            .py_3()
            .gap_3()
            .when(centered, |this| this.max_w(CENTERED_MAX_WIDTH))
            .child(text)
            .children(primary_action)
            .children(actions);

        if centered {
            v_flex()
                .size_full()
                .items_center()
                .justify_center()
                .child(content)
                .into_any_element()
        } else {
            content.into_any_element()
        }
    }
}

impl Component for EmptyState {
    fn scope() -> ComponentScope {
        ComponentScope::Layout
    }

    fn description() -> &'static str {
        "The empty state: what a panel, a list or the editor area shows when \
        it has nothing to show. An icon, one line, a description, at most one \
        primary button and up to three action rows with their key caps."
    }

    fn preview(_window: &mut Window, cx: &mut App) -> AnyElement {
        use gpui::{KeybindingKeystroke, Keystroke};

        let key = |keystroke: &'static str| {
            Keystroke::parse(keystroke).ok().map(|keystroke| {
                KeyBinding::from_keystrokes(
                    vec![KeybindingKeystroke::from_keystroke(keystroke)].into(),
                    false,
                )
            })
        };
        let panel = |empty_state: EmptyState| {
            v_flex()
                .w(px(280.))
                .min_h(px(200.))
                .border_1()
                .border_color(cx.theme().colors().border)
                .bg(cx.theme().colors().background)
                .child(empty_state)
        };

        v_flex()
            .gap_6()
            .child(
                example_group(vec![
                    single_example(
                        "In a panel",
                        h_flex()
                            .items_start()
                            .gap_4()
                            .child(panel(
                                EmptyState::new("No changes")
                                    .icon(IconName::GitBranch)
                                    .description(
                                        "Files you edit in this repository are listed here.",
                                    ),
                            ))
                            .child(panel(
                                EmptyState::new("No folder open")
                                    .icon(IconName::Folder)
                                    .description("Open a folder to browse and edit its files.")
                                    .primary_action(Button::primary(
                                        "preview-empty-open-folder",
                                        "Open folder",
                                    ))
                                    .action("Clone repository", None, |_, _, _| {})
                                    .action("Open recent", key("ctrl-r"), |_, _, _| {}),
                            ))
                            .into_any_element(),
                    ),
                    single_example(
                        "Centered, in the editor area",
                        v_flex()
                            .w(px(560.))
                            .h(px(200.))
                            .border_1()
                            .border_color(cx.theme().colors().border)
                            .bg(cx.theme().colors().editor_background)
                            .child(
                                EmptyState::new("No file open")
                                    .centered(true)
                                    .action("Go to file", key("ctrl-p"), |_, _, _| {})
                                    .action("Search in files", key("ctrl-shift-f"), |_, _, _| {})
                                    .action("New chat", key("ctrl-l"), |_, _, _| {}),
                            )
                            .into_any_element(),
                    ),
                ])
                .vertical(),
            )
            .into_any_element()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn keeps_at_most_three_actions() {
        let empty_state = ["One", "Two", "Three", "Four"]
            .into_iter()
            .fold(EmptyState::new("Nothing here"), |empty_state, label| {
                empty_state.action(label, None, |_, _, _| {})
            });

        let labels = empty_state
            .actions
            .iter()
            .map(|action| action.label.as_ref())
            .collect::<Vec<_>>();
        assert_eq!(labels, ["One", "Two", "Three"]);
    }
}

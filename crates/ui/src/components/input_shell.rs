use gpui::{AnyElement, StyleRefinement};
use smallvec::SmallVec;

use crate::{IconButton, metrics, prelude::*};

/// The height of an [`InputShell`].
#[derive(Debug, Default, PartialEq, Eq, Hash, Clone, Copy)]
pub enum InputShellSize {
    /// 26px: inputs in toolbars and search rows.
    Small,
    /// 28px: inputs in forms and settings.
    #[default]
    Medium,
    /// 32px: the one prominent input of a view.
    Large,
}

impl InputShellSize {
    /// The height of an input of this size.
    pub fn height(self) -> Rems {
        metrics::input_height(match self {
            InputShellSize::Small => metrics::InputSize::Small,
            InputShellSize::Medium => metrics::InputSize::Medium,
            InputShellSize::Large => metrics::InputSize::Large,
        })
    }
}

/// The ground an [`InputShell`] is filled with. An input never has the ground
/// of the surface it sits on.
#[derive(Debug, Default, PartialEq, Eq, Hash, Clone, Copy)]
pub enum InputShellGround {
    /// The sheet ground (`editor.background`): for inputs on chrome and on
    /// raised surfaces, which is panels, toolbars, pages, popovers and modals.
    #[default]
    Sunken,
    /// The raised ground (`elevated_surface.background`): for inputs that sit
    /// on the sheet itself, like a field inside an editor or the chat
    /// transcript.
    Raised,
}

/// The box of a text input: ground, 1px line, focus and error states, a slot
/// on each side.
///
/// It is a styled container, not an editor: put the editor (or any other
/// content) in it as a child and tell it whether that content has the focus.
/// The shell is at least as tall as its size and grows with its content, so
/// a multi-line editor works; add vertical padding in that case.
///
/// Width and layout are the caller's (`.w_full()`, `.min_w(..)`, `.flex_1()`),
/// and listeners such as `.track_focus(..)` or `.on_action(..)` can be
/// attached to the shell directly.
///
/// # Usage Example
///
/// ```
/// use ui::{InputShell, InputShellSize, prelude::*};
///
/// fn search_field(editor: AnyElement, focused: bool, has_error: bool) -> InputShell {
///     InputShell::new()
///         .size(InputShellSize::Small)
///         .focused(focused)
///         .invalid(has_error)
///         .start_slot(
///             Icon::new(IconName::MagnifyingGlass)
///                 .size(IconSize::Small)
///                 .color(Color::Muted),
///         )
///         .w_full()
///         .child(editor)
/// }
/// ```
#[derive(IntoElement, RegisterComponent)]
pub struct InputShell {
    div: Div,
    size: InputShellSize,
    ground: InputShellGround,
    focused: bool,
    invalid: bool,
    disabled: bool,
    start_slot: Option<AnyElement>,
    end_slot: Option<AnyElement>,
    children: SmallVec<[AnyElement; 2]>,
}

impl InputShell {
    /// Creates a medium, sunken, empty shell.
    pub fn new() -> Self {
        Self {
            div: div(),
            size: InputShellSize::default(),
            ground: InputShellGround::default(),
            focused: false,
            invalid: false,
            disabled: false,
            start_slot: None,
            end_slot: None,
            children: SmallVec::new(),
        }
    }

    /// Sets the height of the shell.
    pub fn size(mut self, size: InputShellSize) -> Self {
        self.size = size;
        self
    }

    /// Sets the ground, according to the surface the input sits on.
    pub fn ground(mut self, ground: InputShellGround) -> Self {
        self.ground = ground;
        self
    }

    /// Whether the content has the keyboard focus: the line takes the text
    /// colour.
    pub fn focused(mut self, focused: bool) -> Self {
        self.focused = focused;
        self
    }

    /// Whether the content is invalid: the line takes the error colour, also
    /// while focused. The sentence that says why goes under the field.
    pub fn invalid(mut self, invalid: bool) -> Self {
        self.invalid = invalid;
        self
    }

    /// Whether the input is disabled: dimmed text, no focus, error or hover
    /// state.
    pub fn disabled(mut self, disabled: bool) -> Self {
        self.disabled = disabled;
        self
    }

    /// An element before the content, usually a small muted [`Icon`].
    pub fn start_slot(mut self, element: impl IntoElement) -> Self {
        self.start_slot = Some(element.into_any_element());
        self
    }

    /// An element after the content, usually an [`IconButton`].
    pub fn end_slot(mut self, element: impl IntoElement) -> Self {
        self.end_slot = Some(element.into_any_element());
        self
    }
}

impl Default for InputShell {
    fn default() -> Self {
        Self::new()
    }
}

impl Styled for InputShell {
    fn style(&mut self) -> &mut StyleRefinement {
        self.div.style()
    }
}

impl InteractiveElement for InputShell {
    fn interactivity(&mut self) -> &mut gpui::Interactivity {
        self.div.interactivity()
    }
}

impl ParentElement for InputShell {
    fn extend(&mut self, elements: impl IntoIterator<Item = AnyElement>) {
        self.children.extend(elements)
    }
}

impl RenderOnce for InputShell {
    #[allow(refining_impl_trait)]
    fn render(self, _window: &mut Window, cx: &mut App) -> Div {
        let colors = cx.theme().colors();
        let raised_ground = colors.elevated_surface_background;
        let ground = match self.ground {
            InputShellGround::Sunken => colors.editor_background,
            InputShellGround::Raised => raised_ground,
        };
        let line = if self.disabled {
            colors.border
        } else if self.invalid {
            cx.theme().status().error
        } else if self.focused {
            colors.text
        } else {
            colors.border_strong
        };
        let hover_lifts =
            self.ground == InputShellGround::Sunken && !self.disabled && !self.focused;

        self.div
            .h_flex()
            .min_h(self.size.height())
            .px_2()
            .gap_1()
            .rounded_sm()
            .bg(ground)
            .border_1()
            .border_color(line)
            .text_color(if self.disabled {
                colors.text_disabled
            } else {
                colors.text
            })
            .when(self.disabled, |this| this.cursor_not_allowed())
            .when(hover_lifts, |this| {
                this.hover(|style| style.bg(raised_ground))
            })
            .children(self.start_slot)
            .child(h_flex().flex_1().min_w_0().children(self.children))
            .children(self.end_slot)
    }
}

impl Component for InputShell {
    fn scope() -> ComponentScope {
        ComponentScope::Input
    }

    fn description() -> &'static str {
        "The input shell: the box every text input is drawn in (ground, 1px \
        line, focus, invalid and disabled states, a slot on each side), in \
        three heights. It holds an editor; it is not one."
    }

    fn preview(_window: &mut Window, cx: &mut App) -> AnyElement {
        let value = |text: &'static str| Label::new(text).size(LabelSize::Small);
        let placeholder = |text: &'static str| {
            Label::new(text)
                .size(LabelSize::Small)
                .color(Color::Placeholder)
        };
        let search_icon = || {
            Icon::new(IconName::MagnifyingGlass)
                .size(IconSize::Small)
                .color(Color::Muted)
        };
        let captioned = |caption: &'static str, shell: InputShell| {
            v_flex()
                .gap_1()
                .child(
                    Label::new(caption)
                        .size(LabelSize::Small)
                        .color(Color::Muted),
                )
                .child(shell.w(px(240.)))
        };
        let surface = |ground: gpui::Hsla| v_flex().p_3().gap_3().bg(ground);
        let chrome = cx.theme().colors().background;
        let sheet = cx.theme().colors().editor_background;

        v_flex()
            .gap_6()
            .child(
                example_group(vec![
                    single_example(
                        "Sizes and states (sunken, on chrome)",
                        h_flex()
                            .items_start()
                            .gap_4()
                            .child(
                                surface(chrome)
                                    .child(captioned(
                                        "Small, 26px",
                                        InputShell::new()
                                            .size(InputShellSize::Small)
                                            .start_slot(search_icon())
                                            .child(placeholder("Search")),
                                    ))
                                    .child(captioned(
                                        "Medium, 28px",
                                        InputShell::new().child(placeholder("Name")),
                                    ))
                                    .child(captioned(
                                        "Large, 32px",
                                        InputShell::new()
                                            .size(InputShellSize::Large)
                                            .child(placeholder("API key"))
                                            .end_slot(
                                                IconButton::new(
                                                    "preview-shell-reveal",
                                                    IconName::Eye,
                                                )
                                                .icon_size(IconSize::Small)
                                                .icon_color(Color::Muted),
                                            ),
                                    )),
                            )
                            .child(
                                surface(chrome)
                                    .child(captioned(
                                        "Rest",
                                        InputShell::new().child(value("src/main.rs")),
                                    ))
                                    .child(captioned(
                                        "Focused",
                                        InputShell::new().focused(true).child(value("src/main.rs")),
                                    ))
                                    .child(captioned(
                                        "Invalid",
                                        InputShell::new().invalid(true).child(value("not a path")),
                                    ))
                                    .child(captioned(
                                        "Disabled",
                                        InputShell::new().disabled(true).child(
                                            Label::new("Read only")
                                                .size(LabelSize::Small)
                                                .color(Color::Disabled),
                                        ),
                                    )),
                            )
                            .into_any_element(),
                    ),
                    single_example(
                        "Grounds",
                        h_flex()
                            .items_start()
                            .gap_4()
                            .child(surface(chrome).child(captioned(
                                "Sunken, on chrome",
                                InputShell::new().child(placeholder("Filter")),
                            )))
                            .child(
                                surface(sheet).child(captioned(
                                    "Raised, on the sheet",
                                    InputShell::new()
                                        .ground(InputShellGround::Raised)
                                        .child(placeholder("Ask anything")),
                                )),
                            )
                            .into_any_element(),
                    ),
                ])
                .vertical(),
            )
            .into_any_element()
    }
}

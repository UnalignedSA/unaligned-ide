use gpui::{
    Action, AnyElement, Hsla, MAX_BUTTONS_PER_SIDE, MouseButton, WindowButton, prelude::*, svg,
};
use ui::prelude::*;

/// Side of one window button, in rems (`w_5`).
const BUTTON_SIZE: f32 = 1.25;
/// Space between two window buttons, in rems (`gap_3`).
const BUTTON_GAP: f32 = 0.75;
/// Space before the first and after the last window button, in rems (`px_3`).
const GROUP_PADDING: f32 = 0.75;

/// The buttons of one side that are drawn: the desktop's layout, less what
/// the window cannot do.
pub(crate) fn drawn_buttons(
    buttons: &[Option<WindowButton>; MAX_BUTTONS_PER_SIDE],
    can_minimize: bool,
    can_maximize: bool,
) -> impl Iterator<Item = WindowButton> + '_ {
    buttons
        .iter()
        .filter_map(|button| *button)
        .filter(move |button| match button {
            WindowButton::Minimize => can_minimize,
            WindowButton::Maximize => can_maximize,
            WindowButton::Close => true,
        })
}

/// Width of a group of `button_count` window buttons, padding included.
/// A group without buttons takes no room at all.
pub(crate) fn controls_width(button_count: usize, rem_size: Pixels) -> Pixels {
    if button_count == 0 {
        return px(0.);
    }
    let count = button_count as f32;
    rem_size * (count * BUTTON_SIZE + (count - 1.) * BUTTON_GAP + 2. * GROUP_PADDING)
}

#[derive(IntoElement)]
pub struct LinuxWindowControls {
    id: &'static str,
    buttons: [Option<WindowButton>; MAX_BUTTONS_PER_SIDE],
    close_action: Box<dyn Action>,
}

impl LinuxWindowControls {
    pub fn new(
        id: &'static str,
        buttons: [Option<WindowButton>; MAX_BUTTONS_PER_SIDE],
        close_action: Box<dyn Action>,
    ) -> Self {
        Self {
            id,
            buttons,
            close_action,
        }
    }
}

impl RenderOnce for LinuxWindowControls {
    fn render(self, window: &mut Window, cx: &mut App) -> impl IntoElement {
        let is_maximized = window.is_maximized();
        let supported_controls = window.window_controls();
        let button_elements: Vec<AnyElement> = drawn_buttons(
            &self.buttons,
            supported_controls.minimize,
            supported_controls.maximize,
        )
        .map(|button| {
            create_window_button(button, button.id(), is_maximized, &*self.close_action, cx)
        })
        .collect();

        // Sizes come from the constants `controls_width` adds up, so the room
        // the title bar reserves for this group is the room it takes.
        h_flex()
            .id(self.id)
            .flex_none()
            .when(!button_elements.is_empty(), |el| {
                el.gap(rems(BUTTON_GAP))
                    .px(rems(GROUP_PADDING))
                    .on_mouse_down(MouseButton::Left, |_, _, cx| cx.stop_propagation())
                    .children(button_elements)
            })
    }
}

fn create_window_button(
    button: WindowButton,
    id: &'static str,
    is_maximized: bool,
    close_action: &dyn Action,
    cx: &mut App,
) -> AnyElement {
    match button {
        WindowButton::Minimize => {
            WindowControl::new(id, WindowControlType::Minimize, cx).into_any_element()
        }
        WindowButton::Maximize => WindowControl::new(
            id,
            if is_maximized {
                WindowControlType::Restore
            } else {
                WindowControlType::Maximize
            },
            cx,
        )
        .into_any_element(),
        WindowButton::Close => {
            WindowControl::new_close(id, WindowControlType::Close, close_action.boxed_clone(), cx)
                .into_any_element()
        }
    }
}

#[derive(Debug, PartialEq, Eq, PartialOrd, Ord, Hash, Clone, Copy)]
pub enum WindowControlType {
    Minimize,
    Restore,
    Maximize,
    Close,
}

impl WindowControlType {
    /// Returns the icon name for the window control type.
    ///
    /// Will take a [PlatformStyle] in the future to return a different
    /// icon name based on the platform.
    pub fn icon(&self) -> IconName {
        match self {
            WindowControlType::Minimize => IconName::GenericMinimize,
            WindowControlType::Restore => IconName::GenericRestore,
            WindowControlType::Maximize => IconName::GenericMaximize,
            WindowControlType::Close => IconName::GenericClose,
        }
    }
}

#[allow(unused)]
pub struct WindowControlStyle {
    background: Hsla,
    background_hover: Hsla,
    icon: Hsla,
    icon_hover: Hsla,
}

impl WindowControlStyle {
    pub fn default(cx: &mut App) -> Self {
        let colors = cx.theme().colors();

        Self {
            background: colors.ghost_element_background,
            background_hover: colors.ghost_element_hover,
            icon: colors.icon,
            icon_hover: colors.icon_muted,
        }
    }

    #[allow(unused)]
    /// Sets the background color of the control.
    pub fn background(mut self, color: impl Into<Hsla>) -> Self {
        self.background = color.into();
        self
    }

    #[allow(unused)]
    /// Sets the background color of the control when hovered.
    pub fn background_hover(mut self, color: impl Into<Hsla>) -> Self {
        self.background_hover = color.into();
        self
    }

    #[allow(unused)]
    /// Sets the color of the icon.
    pub fn icon(mut self, color: impl Into<Hsla>) -> Self {
        self.icon = color.into();
        self
    }

    #[allow(unused)]
    /// Sets the color of the icon when hovered.
    pub fn icon_hover(mut self, color: impl Into<Hsla>) -> Self {
        self.icon_hover = color.into();
        self
    }
}

#[derive(IntoElement)]
pub struct WindowControl {
    id: ElementId,
    icon: WindowControlType,
    style: WindowControlStyle,
    close_action: Option<Box<dyn Action>>,
}

impl WindowControl {
    pub fn new(id: impl Into<ElementId>, icon: WindowControlType, cx: &mut App) -> Self {
        let style = WindowControlStyle::default(cx);

        Self {
            id: id.into(),
            icon,
            style,
            close_action: None,
        }
    }

    pub fn new_close(
        id: impl Into<ElementId>,
        icon: WindowControlType,
        close_action: Box<dyn Action>,
        cx: &mut App,
    ) -> Self {
        let style = WindowControlStyle::default(cx);

        Self {
            id: id.into(),
            icon,
            style,
            close_action: Some(close_action.boxed_clone()),
        }
    }

    #[allow(unused)]
    pub fn custom_style(
        id: impl Into<ElementId>,
        icon: WindowControlType,
        style: WindowControlStyle,
    ) -> Self {
        Self {
            id: id.into(),
            icon,
            style,
            close_action: None,
        }
    }
}

impl RenderOnce for WindowControl {
    fn render(self, window: &mut Window, cx: &mut App) -> impl IntoElement {
        let enabled = match self.icon {
            WindowControlType::Minimize => window.is_minimizable(),
            WindowControlType::Restore | WindowControlType::Maximize => window.is_resizable(),
            WindowControlType::Close => true,
        };
        let icon = svg()
            .size_4()
            .flex_none()
            .path(self.icon.icon().path())
            .text_color(if enabled {
                self.style.icon
            } else {
                cx.theme().colors().icon_disabled
            })
            .when(enabled, |this| {
                this.group_hover("", |this| this.text_color(self.style.icon_hover))
            });

        h_flex()
            .id(self.id)
            .group("")
            .justify_center()
            .content_center()
            .flex_none()
            // A circle in every theme, like the desktop's own controls: named
            // radii follow the theme's corners, `rounded_full` does not.
            .rounded_full()
            .size(rems(BUTTON_SIZE))
            .when(enabled, |this| {
                this.cursor_pointer()
                    .hover(|this| this.bg(self.style.background_hover))
                    .active(|this| this.bg(self.style.background_hover))
            })
            .child(icon)
            .on_mouse_move(|_, _, cx| cx.stop_propagation())
            .on_click(move |_, window, cx| {
                cx.stop_propagation();
                if !enabled {
                    return;
                }
                match self.icon {
                    WindowControlType::Minimize => window.minimize_window(),
                    WindowControlType::Restore => window.zoom_window(),
                    WindowControlType::Maximize => window.zoom_window(),
                    WindowControlType::Close => window.dispatch_action(
                        self.close_action
                            .as_ref()
                            .expect("Use WindowControl::new_close() for close control.")
                            .boxed_clone(),
                        cx,
                    ),
                }
            })
    }
}

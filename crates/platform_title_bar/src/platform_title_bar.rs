pub mod platforms;
mod system_window_tabs;
mod title_bar_settings;

use gpui::{
    Action, AnyElement, App, Context, Decorations, Entity, Hsla, InteractiveElement, IntoElement,
    MouseButton, ParentElement, Pixels, StatefulInteractiveElement, Styled, WeakEntity, Window,
    WindowButtonLayout, WindowControlArea, div, px,
};
use project::DisableAiSettings;
use settings::Settings;
use smallvec::SmallVec;
use std::mem;
use ui::{
    prelude::*,
    utils::{TRAFFIC_LIGHT_PADDING, platform_title_bar_height},
};
use workspace::{MultiWorkspace, SidebarRenderState, SidebarSide};

use crate::platforms::{platform_linux, platform_windows};

pub use system_window_tabs::{
    DraggedWindowTab, MergeAllWindows, MoveTabToNewWindow, ShowNextWindowTab,
    ShowPreviousWindowTab, SystemWindowTabs,
};
pub use title_bar_settings::TitleBarSettings;

/// With client-side decorations, the title bar is pulled over the window frame's
/// border to avoid a transparent gap in the rounded corners, and a border in the
/// title bar's own color insets its content back by the same amount. Headers that
/// render window controls in place of the title bar use this to apply the same
/// insets as padding, so the controls stay on the same pixel.
///
/// `pad_bottom` should be false when the header already has a bottom border of
/// its own occupying that space.
pub fn apply_title_bar_insets<E: Styled + FluentBuilder>(
    header: E,
    left_window_controls: bool,
    right_window_controls: bool,
    pad_bottom: bool,
) -> E {
    let inset = theme::CLIENT_SIDE_DECORATION_BORDER;
    header
        .mt(-inset)
        .pt(inset)
        .when(pad_bottom, |header| header.pb(inset))
        .when(left_window_controls, |header| header.pl(inset))
        .when(right_window_controls, |header| header.pr(inset))
}

/// Left padding of the bar's content where no window control sits on that
/// side, in rems (`pl_2`).
const EDGE_PADDING: f32 = 0.5;

/// The platform whose title bar is drawn: the one the app was built for.
///
/// Debug builds accept `UNALIGNED_TITLE_BAR_STYLE=windows|linux|mac` to draw
/// another platform's bar (Linux with client-side decorations), so that the
/// layout of the release targets can be looked at on the development machine.
/// Only the drawing is switched: the window keeps its real behaviour.
pub fn platform_style() -> PlatformStyle {
    #[cfg(debug_assertions)]
    if let Some(style) = forced_platform_style() {
        return style;
    }
    PlatformStyle::platform()
}

#[cfg(debug_assertions)]
fn forced_platform_style() -> Option<PlatformStyle> {
    static FORCED: std::sync::OnceLock<Option<PlatformStyle>> = std::sync::OnceLock::new();
    *FORCED.get_or_init(
        || match std::env::var("UNALIGNED_TITLE_BAR_STYLE").ok()?.as_str() {
            "windows" => Some(PlatformStyle::Windows),
            "linux" => Some(PlatformStyle::Linux),
            "mac" => Some(PlatformStyle::Mac),
            _ => None,
        },
    )
}

/// Whether a debug build was asked to draw the Linux bar on another platform.
fn linux_style_is_forced() -> bool {
    #[cfg(debug_assertions)]
    if forced_platform_style() == Some(PlatformStyle::Linux)
        && PlatformStyle::platform() != PlatformStyle::Linux
    {
        return true;
    }
    false
}

/// The window's decoration mode, as the title bar draws it (see
/// [`platform_style`] for the debug-only switch).
fn window_decorations(window: &Window) -> Decorations {
    if linux_style_is_forced() {
        return Decorations::Client {
            tiling: gpui::Tiling::default(),
        };
    }
    window.window_decorations()
}

/// Debug-only stand-in for the desktop's button layout when the Linux bar is
/// drawn on another platform: `UNALIGNED_TITLE_BAR_BUTTONS` in GNOME's form
/// without spaces ("close,minimize:maximize"), by default
/// ":minimize,maximize,close".
fn forced_button_layout() -> WindowButtonLayout {
    fn side(names: &str) -> [Option<gpui::WindowButton>; gpui::MAX_BUTTONS_PER_SIDE] {
        let mut buttons = [None; gpui::MAX_BUTTONS_PER_SIDE];
        let known = names.split(',').filter_map(|name| match name {
            "minimize" => Some(gpui::WindowButton::Minimize),
            "maximize" => Some(gpui::WindowButton::Maximize),
            "close" => Some(gpui::WindowButton::Close),
            _ => None,
        });
        for (slot, button) in buttons.iter_mut().zip(known) {
            *slot = Some(button);
        }
        buttons
    }

    let spec = std::env::var("UNALIGNED_TITLE_BAR_BUTTONS")
        .unwrap_or_else(|_| ":minimize,maximize,close".to_string());
    let (left, right) = spec.split_once(':').unwrap_or(("", spec.as_str()));
    WindowButtonLayout {
        left: side(left),
        right: side(right),
    }
}

/// Everything that decides where the window controls sit and how wide they
/// are. Plain data, so the arithmetic can be checked for platforms that cannot
/// be run on the machine at hand.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct WindowControlLayout {
    pub platform_style: PlatformStyle,
    /// Native or simple fullscreen: no controls are drawn.
    pub fullscreen: bool,
    /// Linux only: the app draws the decorations itself.
    pub client_decorations: bool,
    /// Linux only: which buttons the desktop puts on which side.
    pub button_layout: Option<WindowButtonLayout>,
    /// Whether the window can be minimized (else that button is not drawn).
    pub can_minimize: bool,
    /// Whether the window can be maximized (else that button is not drawn).
    pub can_maximize: bool,
    pub rem_size: Pixels,
}

impl WindowControlLayout {
    /// The layout of `window`. `button_layout` is the user's setting
    /// (`title_bar.button_layout`), which wins over the desktop's.
    pub fn for_window(
        button_layout: Option<WindowButtonLayout>,
        window: &Window,
        cx: &App,
    ) -> Self {
        let platform_style = platform_style();
        let client_decorations = matches!(window_decorations(window), Decorations::Client { .. });
        let supported_controls = window.window_controls();
        Self {
            platform_style,
            fullscreen: window.is_fullscreen() || window.is_simple_fullscreen(),
            client_decorations,
            button_layout: effective_button_layout(
                platform_style,
                client_decorations,
                button_layout,
                cx,
            ),
            can_minimize: supported_controls.minimize,
            can_maximize: supported_controls.maximize,
            rem_size: window.rem_size(),
        }
    }

    /// Width taken by the window controls on the left and on the right.
    ///
    /// - macOS: the traffic lights' room on the left, nothing on the right.
    /// - Windows: nothing on the left, the three caption buttons on the right.
    /// - Linux, client-side decorations: the buttons the desktop's layout
    ///   puts on each side.
    /// - Linux with server-side decorations, and fullscreen everywhere: 0, 0.
    pub fn insets(&self) -> (Pixels, Pixels) {
        if self.fullscreen {
            return (px(0.), px(0.));
        }
        match self.platform_style {
            PlatformStyle::Mac => (px(TRAFFIC_LIGHT_PADDING), px(0.)),
            PlatformStyle::Windows => (px(0.), platform_windows::controls_width()),
            PlatformStyle::Linux => {
                let Some(button_layout) = self.button_layout.filter(|_| self.client_decorations)
                else {
                    return (px(0.), px(0.));
                };
                let width = |buttons| {
                    let count = platform_linux::drawn_buttons(
                        buttons,
                        self.can_minimize,
                        self.can_maximize,
                    )
                    .count();
                    platform_linux::controls_width(count, self.rem_size)
                };
                (width(&button_layout.left), width(&button_layout.right))
            }
        }
    }

    /// Distance from each end of the bar to the row that holds its content:
    /// the controls of [`Self::insets`], or the bar's own padding on a left
    /// side without controls. `left_controls` and `right_controls` are false
    /// when a sidebar open on that side draws the controls instead of the bar.
    pub fn content_insets(&self, left_controls: bool, right_controls: bool) -> (Pixels, Pixels) {
        let padding = self.rem_size * EDGE_PADDING;
        let (left, right) = self.insets();
        // A side the desktop gives buttons to is laid out as a control group
        // even when none of them can be drawn: no padding then.
        let has_left_group = match self.platform_style {
            PlatformStyle::Mac => true,
            PlatformStyle::Windows => false,
            PlatformStyle::Linux => {
                self.client_decorations
                    && self
                        .button_layout
                        .is_some_and(|layout| layout.left[0].is_some())
            }
        };
        let left = if !self.fullscreen && left_controls && has_left_group {
            left
        } else {
            padding
        };
        let right = if right_controls { right } else { px(0.) };
        (left, right)
    }
}

/// Width taken by the window controls on the left and on the right of
/// `window`'s title bar, for the current platform and decoration mode. See
/// [`WindowControlLayout::insets`].
pub fn window_control_insets(window: &Window, cx: &App) -> (Pixels, Pixels) {
    let button_layout = TitleBarSettings::get_global(cx).button_layout;
    WindowControlLayout::for_window(button_layout, window, cx).insets()
}

fn effective_button_layout(
    platform_style: PlatformStyle,
    client_decorations: bool,
    button_layout: Option<WindowButtonLayout>,
    cx: &App,
) -> Option<WindowButtonLayout> {
    if platform_style == PlatformStyle::Linux && client_decorations {
        if linux_style_is_forced() {
            return Some(forced_button_layout());
        }
        button_layout.or_else(|| cx.button_layout())
    } else {
        None
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct SidebarWindowControls {
    pub traffic_lights: bool,
    pub left: bool,
    pub right: bool,
}

impl SidebarWindowControls {
    pub fn new(sidebar_side: SidebarSide, window: &Window, cx: &App) -> Self {
        let visible = !window.is_fullscreen()
            && !window.is_simple_fullscreen()
            && TitleBarSettings::is_title_bar_visible(window, cx);
        let is_macos = cfg!(target_os = "macos");
        let on_left = sidebar_side == SidebarSide::Left;
        Self {
            traffic_lights: visible && is_macos && on_left,
            left: visible && !is_macos && on_left,
            right: visible && !is_macos && !on_left,
        }
    }
}

pub struct PlatformTitleBar {
    id: ElementId,
    platform_style: PlatformStyle,
    children: SmallVec<[AnyElement; 2]>,
    should_move: bool,
    system_window_tabs: Option<Entity<SystemWindowTabs>>,
    button_layout: Option<WindowButtonLayout>,
    multi_workspace: Option<WeakEntity<MultiWorkspace>>,
}

impl PlatformTitleBar {
    pub fn new(id: impl Into<ElementId>, cx: &mut Context<Self>) -> Self {
        let platform_style = platform_style();
        let system_window_tabs = Some(cx.new(|_cx| SystemWindowTabs::new()));

        Self {
            id: id.into(),
            platform_style,
            children: SmallVec::new(),
            should_move: false,
            system_window_tabs,
            button_layout: None,
            multi_workspace: None,
        }
    }

    pub fn without_system_window_tabs(mut self) -> Self {
        self.system_window_tabs = None;
        self
    }

    pub fn with_multi_workspace(mut self, multi_workspace: WeakEntity<MultiWorkspace>) -> Self {
        self.multi_workspace = Some(multi_workspace);
        self
    }

    pub fn set_multi_workspace(&mut self, multi_workspace: WeakEntity<MultiWorkspace>) {
        self.multi_workspace = Some(multi_workspace);
    }

    pub fn title_bar_color(&self, window: &mut Window, cx: &mut Context<Self>) -> Hsla {
        if cfg!(any(target_os = "linux", target_os = "freebsd")) {
            if window.is_window_active() && !self.should_move {
                cx.theme().colors().title_bar_background
            } else {
                cx.theme().colors().title_bar_inactive_background
            }
        } else {
            cx.theme().colors().title_bar_background
        }
    }

    pub fn set_children<T>(&mut self, children: T)
    where
        T: IntoIterator<Item = AnyElement>,
    {
        self.children = children.into_iter().collect();
    }

    pub fn set_button_layout(&mut self, button_layout: Option<WindowButtonLayout>) {
        self.button_layout = button_layout;
    }

    fn effective_button_layout(
        &self,
        decorations: &Decorations,
        cx: &App,
    ) -> Option<WindowButtonLayout> {
        effective_button_layout(
            self.platform_style,
            matches!(decorations, Decorations::Client { .. }),
            self.button_layout,
            cx,
        )
    }

    /// Distance from each end of the bar to the row its children are laid out
    /// in, as this bar will draw it (call after [`Self::set_button_layout`]).
    /// A row that centres something on the window needs both.
    pub fn content_insets(&self, window: &Window, cx: &App) -> (Pixels, Pixels) {
        let sidebar = self.sidebar_render_state(cx);
        WindowControlLayout::for_window(self.button_layout, window, cx).content_insets(
            !(sidebar.open && sidebar.side == SidebarSide::Left),
            !(sidebar.open && sidebar.side == SidebarSide::Right),
        )
    }

    pub fn init(cx: &mut App) {
        SystemWindowTabs::init(cx);
    }

    fn sidebar_render_state(&self, cx: &App) -> SidebarRenderState {
        self.multi_workspace
            .as_ref()
            .and_then(|mw| mw.upgrade())
            .map(|mw| mw.read(cx).sidebar_render_state(cx))
            .unwrap_or_default()
    }

    pub fn is_multi_workspace_enabled(cx: &App) -> bool {
        !DisableAiSettings::get_global(cx).disable_ai
    }
}

/// Renders the platform-appropriate left-side window controls (e.g. Ubuntu/GNOME close button).
///
/// Only relevant on Linux with client-side decorations when the window manager
/// places controls on the left.
pub fn render_left_window_controls(
    button_layout: Option<WindowButtonLayout>,
    close_action: Box<dyn Action>,
    window: &Window,
) -> Option<AnyElement> {
    if platform_style() != PlatformStyle::Linux {
        return None;
    }
    if !matches!(window_decorations(window), Decorations::Client { .. }) {
        return None;
    }
    let button_layout = button_layout?;
    if button_layout.left[0].is_none() {
        return None;
    }
    Some(
        platform_linux::LinuxWindowControls::new(
            "left-window-controls",
            button_layout.left,
            close_action,
        )
        .into_any_element(),
    )
}

/// Renders the platform-appropriate right-side window controls (close, minimize, maximize).
///
/// Returns `None` on Mac or when the platform doesn't need custom controls
/// (e.g. Linux with server-side decorations).
pub fn render_right_window_controls(
    button_layout: Option<WindowButtonLayout>,
    close_action: Box<dyn Action>,
    window: &Window,
) -> Option<AnyElement> {
    let decorations = window_decorations(window);
    let height = platform_title_bar_height(window);

    match platform_style() {
        PlatformStyle::Linux => {
            if !matches!(decorations, Decorations::Client { .. }) {
                return None;
            }
            let button_layout = button_layout?;
            if button_layout.right[0].is_none() {
                return None;
            }
            Some(
                platform_linux::LinuxWindowControls::new(
                    "right-window-controls",
                    button_layout.right,
                    close_action,
                )
                .into_any_element(),
            )
        }
        PlatformStyle::Windows => {
            Some(platform_windows::WindowsWindowControls::new(height).into_any_element())
        }
        PlatformStyle::Mac => None,
    }
}

impl Render for PlatformTitleBar {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let supported_controls = window.window_controls();
        let decorations = window_decorations(window);
        let height = platform_title_bar_height(window);
        let titlebar_color = self.title_bar_color(window, cx);
        let close_action = Box::new(workspace::CloseWindow);
        let children = mem::take(&mut self.children);

        let button_layout = self.effective_button_layout(&decorations, cx);
        let sidebar = self.sidebar_render_state(cx);

        let title_bar = h_flex()
            .window_control_area(WindowControlArea::Drag)
            .w_full()
            .h(height)
            .map(|this| {
                this.on_mouse_down_out(cx.listener(move |this, _ev, _window, _cx| {
                    this.should_move = false;
                }))
                .on_mouse_up(
                    gpui::MouseButton::Left,
                    cx.listener(move |this, _ev, _window, _cx| {
                        this.should_move = false;
                    }),
                )
                .on_mouse_down(
                    gpui::MouseButton::Left,
                    cx.listener(move |this, _ev, _window, _cx| {
                        this.should_move = true;
                    }),
                )
                .on_mouse_move(cx.listener(move |this, _ev, window, _| {
                    if this.should_move {
                        this.should_move = false;
                        window.start_window_move();
                    }
                }))
            })
            .map(|this| {
                // Note: On Windows the title bar behavior is handled by the platform implementation.
                this.id(self.id.clone())
                    .when(self.platform_style == PlatformStyle::Mac, |this| {
                        this.on_click(|event, window, _| {
                            if event.click_count() == 2 {
                                window.titlebar_double_click();
                            }
                        })
                    })
                    .when(
                        self.platform_style == PlatformStyle::Linux
                            && supported_controls.maximize
                            && window.is_resizable(),
                        |this| {
                            this.on_click(|event, window, _| {
                                if event.click_count() == 2 {
                                    window.zoom_window();
                                }
                            })
                        },
                    )
            })
            .map(|this| {
                let show_left_controls = !(sidebar.open && sidebar.side == SidebarSide::Left);

                // Keep in sync with `WindowControlLayout::content_insets`.
                if window.is_fullscreen() || window.is_simple_fullscreen() {
                    this.pl(rems(EDGE_PADDING))
                } else if self.platform_style == PlatformStyle::Mac && show_left_controls {
                    this.pl(px(TRAFFIC_LIGHT_PADDING))
                } else if let Some(controls) = show_left_controls
                    .then(|| {
                        render_left_window_controls(
                            button_layout,
                            close_action.as_ref().boxed_clone(),
                            window,
                        )
                    })
                    .flatten()
                {
                    this.child(controls)
                } else {
                    this.pl(rems(EDGE_PADDING))
                }
            })
            .map(|el| match decorations {
                Decorations::Server => el,
                Decorations::Client { tiling, .. } => el
                    .when(
                        !(tiling.top || tiling.right)
                            && !(sidebar.open && sidebar.side == SidebarSide::Right),
                        |el| el.rounded_tr(theme::CLIENT_SIDE_DECORATION_ROUNDING),
                    )
                    .when(
                        !(tiling.top || tiling.left)
                            && !(sidebar.open && sidebar.side == SidebarSide::Left),
                        |el| el.rounded_tl(theme::CLIENT_SIDE_DECORATION_ROUNDING),
                    )
                    // Keep in sync with `apply_title_bar_insets`.
                    .mt(-theme::CLIENT_SIDE_DECORATION_BORDER)
                    .mb(-theme::CLIENT_SIDE_DECORATION_BORDER)
                    .border(theme::CLIENT_SIDE_DECORATION_BORDER)
                    .border_color(titlebar_color),
            })
            .bg(titlebar_color)
            .content_stretch()
            .relative()
            // Flat bar closed by a 1px line; drawn as an overlay so the bar keeps
            // the height other headers align to. It is the only line between
            // the bar and the workspace, which draws none of its own.
            .child(
                div()
                    .absolute()
                    .bottom_0()
                    .left_0()
                    .right_0()
                    .h_px()
                    .bg(cx.theme().colors().border),
            )
            .child(
                div()
                    .id(self.id.clone())
                    .flex()
                    .flex_row()
                    .items_center()
                    .justify_between()
                    .overflow_x_hidden()
                    // Exactly the room the controls leave, so a child can
                    // work out where the middle of the bar is.
                    .flex_1()
                    .min_w_0()
                    .h_full()
                    .children(children),
            )
            .when(
                !window.is_fullscreen() && !window.is_simple_fullscreen(),
                |title_bar| {
                    let show_right_controls = !(sidebar.open && sidebar.side == SidebarSide::Right);

                    let title_bar = title_bar.children(
                        show_right_controls
                            .then(|| {
                                render_right_window_controls(
                                    button_layout,
                                    close_action.as_ref().boxed_clone(),
                                    window,
                                )
                            })
                            .flatten(),
                    );

                    if self.platform_style == PlatformStyle::Linux
                        && matches!(decorations, Decorations::Client { .. })
                    {
                        title_bar.when(supported_controls.window_menu, |titlebar| {
                            titlebar.on_mouse_down(MouseButton::Right, move |ev, window, _| {
                                window.show_window_menu(ev.position)
                            })
                        })
                    } else {
                        title_bar
                    }
                },
            );

        v_flex()
            .w_full()
            .child(title_bar)
            .children(self.system_window_tabs.clone())
    }
}

impl ParentElement for PlatformTitleBar {
    fn extend(&mut self, elements: impl IntoIterator<Item = AnyElement>) {
        self.children.extend(elements)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use gpui::WindowButton::{Close, Maximize, Minimize};

    const REM: Pixels = px(16.);

    fn layout(platform_style: PlatformStyle) -> WindowControlLayout {
        WindowControlLayout {
            platform_style,
            fullscreen: false,
            client_decorations: false,
            button_layout: None,
            can_minimize: true,
            can_maximize: true,
            rem_size: REM,
        }
    }

    fn linux(
        left: [Option<gpui::WindowButton>; 3],
        right: [Option<gpui::WindowButton>; 3],
    ) -> WindowControlLayout {
        WindowControlLayout {
            client_decorations: true,
            button_layout: Some(WindowButtonLayout { left, right }),
            ..layout(PlatformStyle::Linux)
        }
    }

    #[test]
    fn test_macos_insets() {
        let mac = layout(PlatformStyle::Mac);
        assert_eq!(mac.insets(), (px(TRAFFIC_LIGHT_PADDING), px(0.)));
        assert_eq!(
            mac.content_insets(true, true),
            (px(TRAFFIC_LIGHT_PADDING), px(0.))
        );
        // A sidebar open on the left holds the traffic lights.
        assert_eq!(mac.content_insets(false, true), (px(8.), px(0.)));

        let fullscreen = WindowControlLayout {
            fullscreen: true,
            ..mac
        };
        assert_eq!(fullscreen.insets(), (px(0.), px(0.)));
        assert_eq!(fullscreen.content_insets(true, true), (px(8.), px(0.)));
    }

    #[test]
    fn test_windows_insets() {
        let windows = layout(PlatformStyle::Windows);
        // Three caption buttons of 36px, whatever the UI font size.
        assert_eq!(windows.insets(), (px(0.), px(108.)));
        assert_eq!(windows.content_insets(true, true), (px(8.), px(108.)));
        assert_eq!(windows.content_insets(true, false), (px(8.), px(0.)));

        let large_font = WindowControlLayout {
            rem_size: px(20.),
            ..windows
        };
        assert_eq!(large_font.insets(), (px(0.), px(108.)));
        assert_eq!(large_font.content_insets(true, true), (px(10.), px(108.)));

        let fullscreen = WindowControlLayout {
            fullscreen: true,
            ..windows
        };
        assert_eq!(fullscreen.insets(), (px(0.), px(0.)));
        assert_eq!(fullscreen.content_insets(true, true), (px(8.), px(0.)));
    }

    #[test]
    fn test_linux_insets_follow_the_button_layout() {
        // Three buttons: 3 x 20 + 2 x 12 between + 2 x 12 around = 108.
        let right_hand = linux([None; 3], [Some(Minimize), Some(Maximize), Some(Close)]);
        assert_eq!(right_hand.insets(), (px(0.), px(108.)));
        assert_eq!(right_hand.content_insets(true, true), (px(8.), px(108.)));

        // GNOME with the buttons moved to the left.
        let left_hand = linux([Some(Close), Some(Minimize), Some(Maximize)], [None; 3]);
        assert_eq!(left_hand.insets(), (px(108.), px(0.)));
        assert_eq!(left_hand.content_insets(true, true), (px(108.), px(0.)));
        // A sidebar open on the left draws them: the bar pads instead.
        assert_eq!(left_hand.content_insets(false, true), (px(8.), px(0.)));

        // elementary OS: close on the left, maximize on the right.
        let split = linux([Some(Close), None, None], [Some(Maximize), None, None]);
        assert_eq!(split.insets(), (px(44.), px(44.)));
        assert_eq!(split.content_insets(true, true), (px(44.), px(44.)));

        // Close only (GNOME's default): 20 + 2 x 12.
        let close_only = linux([None; 3], [Some(Close), None, None]);
        assert_eq!(close_only.insets(), (px(0.), px(44.)));

        // Sizes are rems: they follow the UI font size.
        let large_font = WindowControlLayout {
            rem_size: px(20.),
            ..right_hand
        };
        assert_eq!(large_font.insets(), (px(0.), px(135.)));
    }

    #[test]
    fn test_linux_insets_skip_buttons_the_window_cannot_use() {
        let dialog = WindowControlLayout {
            can_minimize: false,
            can_maximize: false,
            ..linux([None; 3], [Some(Minimize), Some(Maximize), Some(Close)])
        };
        assert_eq!(dialog.insets(), (px(0.), px(44.)));

        // A left group whose only button cannot be drawn takes no room and
        // leaves no padding either: the bar lays it out as an empty group.
        let empty_left_group = WindowControlLayout {
            can_minimize: false,
            ..linux([Some(Minimize), None, None], [Some(Close), None, None])
        };
        assert_eq!(empty_left_group.insets(), (px(0.), px(44.)));
        assert_eq!(
            empty_left_group.content_insets(true, true),
            (px(0.), px(44.))
        );
    }

    #[test]
    fn test_linux_insets_without_client_decorations() {
        // Server-side decorations: the desktop draws the controls outside
        // the window's content.
        let server_side = WindowControlLayout {
            client_decorations: false,
            ..linux([None; 3], [Some(Minimize), Some(Maximize), Some(Close)])
        };
        assert_eq!(server_side.insets(), (px(0.), px(0.)));
        assert_eq!(server_side.content_insets(true, true), (px(8.), px(0.)));

        let fullscreen = WindowControlLayout {
            fullscreen: true,
            ..linux([Some(Close), None, None], [Some(Maximize), None, None])
        };
        assert_eq!(fullscreen.insets(), (px(0.), px(0.)));
        assert_eq!(fullscreen.content_insets(true, true), (px(8.), px(0.)));

        // No layout known at all.
        let unknown = WindowControlLayout {
            client_decorations: true,
            ..layout(PlatformStyle::Linux)
        };
        assert_eq!(unknown.insets(), (px(0.), px(0.)));
    }
}

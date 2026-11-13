use crate::{ItemHandle, MultiWorkspace, Pane, SidebarSide};
use gpui::{
    AnyView, App, Context, Decorations, ElementId, Entity, FocusHandle, Focusable, Hsla,
    IntoElement, ParentElement, Rems, Render, Role, SharedString, Styled, Subscription, WeakEntity,
    Window,
};
use settings::{SettingsContent, update_settings_file};
use std::{any::TypeId, sync::Arc};
use theme::CLIENT_SIDE_DECORATION_ROUNDING;
use ui::{
    ButtonLike, ContextMenu, IconButtonShape, IconPosition, micro_label_size, prelude::*,
    right_click_menu,
};

/// The 1px line that opens the status bar. It is part of the bar's height.
const STATUS_BAR_LINE: Pixels = px(1.);

/// How much faster than the right side the left side gives up its width.
const LEFT_TOOLS_SHRINK: f32 = 10_000.;

/// Height of every status bar item: what is left of the bar
/// (`ui::metrics::status_bar_height`, 24px at the default density) under its
/// 1px line. It has no access to the density, so a compact bar clips its
/// items by a pixel and a comfortable one centres them.
pub fn status_bar_item_height() -> Rems {
    rems_from_px(23_f32)
}

/// Height of a status bar item at the current density: the bar
/// (`ui::metrics::status_bar_height`) without its 1px line. The items of this
/// repository's own crates use it through [`status_bar_item`];
/// [`status_bar_item_height`] is the density-blind form older callers keep.
pub fn status_bar_item_height_for(cx: &App) -> Rems {
    Rems(ui::metrics::status_bar_height(cx).0 - rems_from_px(1_f32).0)
}

/// Height of the row of items: the bar without its line.
fn status_bar_row_height(window: &Window, cx: &App) -> Pixels {
    ui::metrics::status_bar_height(cx).to_pixels(window.rem_size()) - STATUS_BAR_LINE
}

/// Padding on each side of an item's content, so the contents of two
/// neighbours are 12px apart and the first one starts 8px from the window edge
/// (with [`STATUS_BAR_SIDE_PADDING`]).
const STATUS_ITEM_PADDING: Pixels = px(6.);

/// What the row adds on each side of the items.
const STATUS_BAR_SIDE_PADDING: Pixels = px(2.);

/// A status bar item: a flat button as high as the bar that follows the
/// density. Its content goes in one [`status_bar_item_content`] child, its
/// text in [`status_bar_text`], which takes the text colour while the pointer
/// is over the item.
pub fn status_bar_item(id: impl Into<ElementId>, cx: &App) -> ButtonLike {
    ButtonLike::new(id)
        .style(ButtonStyle::Subtle)
        // No padding of the button's own: `status_bar_item_content` sets it.
        .size(ButtonSize::None)
        .height(status_bar_item_height_for(cx).into())
}

/// The row inside a [`status_bar_item`]: icon and text 4px apart, padded so
/// that neighbouring items read 12px apart.
pub fn status_bar_item_content() -> Div {
    h_flex()
        .h_full()
        // `ButtonSize::None` already pads by one pixel.
        .px(STATUS_ITEM_PADDING - px(1.))
        .gap_1()
}

fn status_bar_micro_text(text: SharedString, cx: &App) -> Div {
    let colors = cx.theme().colors();
    let hover_color = colors.text;
    div()
        .flex_none()
        .whitespace_nowrap()
        .font_buffer(cx)
        .text_size(micro_label_size())
        .text_color(colors.text_muted)
        // `ButtonLike` names its group "": the text follows the item's hover.
        .group_hover("", move |style| style.text_color(hover_color))
        .child(text)
}

/// The text of a status bar item, in micro-label type: code font, 11px,
/// uppercase, muted, and the text colour while its [`status_bar_item`] is
/// hovered. For what the interface writes itself (`LN 12, COL 4`, a language,
/// a branch, a model).
pub fn status_bar_text(text: impl Into<SharedString>, cx: &App) -> Div {
    status_bar_micro_text(text.into().to_uppercase().into(), cx)
}

/// [`status_bar_text`] in a colour that says something (an error count, a low
/// balance, a call to act): it stays that colour while the item is hovered.
pub fn status_bar_text_in(text: impl Into<SharedString>, color: Hsla, cx: &App) -> Div {
    status_bar_text(text, cx)
        .text_color(color)
        .group_hover("", move |style| style.text_color(color))
}

/// Like [`status_bar_text`] but in the case it was written in: for sentences
/// that come from elsewhere (a compiler message), where capitals would change
/// the meaning of an identifier.
pub fn status_bar_message(text: impl Into<SharedString>, cx: &App) -> Div {
    status_bar_micro_text(text.into(), cx)
}

/// A flat, full-height status bar button with the button's own padding, kept
/// for the items of other crates. New items use [`status_bar_item`].
pub fn status_bar_button(id: impl Into<ElementId>) -> ButtonLike {
    ButtonLike::new(id)
        .style(ButtonStyle::Subtle)
        // `Medium` is only used for its wider horizontal padding; the height
        // is pinned to the bar.
        .size(ButtonSize::Medium)
        .height(status_bar_item_height().into())
}

/// The icon-only counterpart of [`status_bar_button`].
pub fn status_bar_icon_button(id: impl Into<ElementId>, icon: IconName) -> IconButton {
    IconButton::new(id, icon)
        .shape(IconButtonShape::Wide)
        .style(ButtonStyle::Subtle)
        .size(ButtonSize::Default)
        .icon_size(IconSize::Small)
        .icon_color(Color::Muted)
        .width(rems_from_px(28_f32))
}

/// Text of a status bar item set in the UI font (names, messages).
pub fn status_bar_label(text: impl Into<SharedString>) -> Label {
    Label::new(text)
        .size(LabelSize::Small)
        .color(Color::Muted)
        .single_line()
}

/// Text of a status bar item as a [`Label`] in micro-label type (code font,
/// 11px, uppercase, muted): `UTF-8`, `LF`, a vim mode. A label cannot follow
/// the item's hover; [`status_bar_text`] does.
pub fn status_bar_value(text: impl Into<SharedString>, cx: &App) -> Label {
    Label::new(text.into().to_uppercase())
        .size(LabelSize::Custom(micro_label_size()))
        .color(Color::Muted)
        .single_line()
        .buffer_font(cx)
}

/// An icon placed next to a label inside a [`status_bar_button`].
pub fn status_bar_icon(icon: IconName) -> Icon {
    Icon::new(icon).size(IconSize::Small).color(Color::Muted)
}

/// Describes how a status-bar item can be hidden by the user.
///
/// Every [`StatusItemView`] must either provide this (so that the user gets a
/// "Hide Button" entry in the right-click menu) or explicitly return `None`
/// to opt out. Returning `None` should be reserved for items that are
/// already conditional on some other setting exposed elsewhere (e.g., the
/// activity indicator, which disappears on its own once there's no work to
/// display).
#[derive(Clone)]
pub struct HideStatusItem {
    hide: Arc<dyn Fn(&mut SettingsContent) + Send + Sync>,
}

impl HideStatusItem {
    pub fn new(hide: impl Fn(&mut SettingsContent) + Send + Sync + 'static) -> Self {
        Self {
            hide: Arc::new(hide),
        }
    }

    /// Persists the hide by updating the user settings file.
    pub fn apply(&self, cx: &App) {
        let hide = self.hide.clone();
        let fs = <dyn fs::Fs>::global(cx);
        update_settings_file(fs, cx, move |settings, _cx| (hide)(settings));
    }
}

pub trait StatusItemView: Render {
    /// Event callback that is triggered when the active pane item changes.
    fn set_active_pane_item(
        &mut self,
        active_pane_item: Option<&dyn crate::ItemHandle>,
        window: &mut Window,
        cx: &mut Context<Self>,
    );

    /// Returns metadata describing how this item can be hidden from the
    /// status bar by writing to the user settings file.
    ///
    /// Implementors that return `None` must be inherently conditional on
    /// another user-exposed setting; otherwise, they should return `Some` so
    /// that the status bar can show a "Hide Button" entry in its
    /// right-click menu.
    fn hide_setting(&self, cx: &App) -> Option<HideStatusItem>;
}

trait StatusItemViewHandle: Send {
    fn to_any(&self) -> AnyView;
    fn set_active_pane_item(
        &self,
        active_pane_item: Option<&dyn ItemHandle>,
        window: &mut Window,
        cx: &mut App,
    );
    fn item_type(&self) -> TypeId;
    fn hide_setting(&self, cx: &App) -> Option<HideStatusItem>;
}

/// Where the threads sidebar is, for the corners of a window that draws its
/// own decorations. The status bar has no button for it: the sidebar is
/// opened by its own action.
#[derive(Default)]
struct SidebarStatus {
    open: bool,
    side: SidebarSide,
}

impl SidebarStatus {
    fn query(multi_workspace: &Option<WeakEntity<MultiWorkspace>>, cx: &App) -> Self {
        multi_workspace
            .as_ref()
            .and_then(|mw| mw.upgrade())
            .map(|mw| {
                let mw = mw.read(cx);
                let enabled = mw.multi_workspace_enabled(cx);
                Self {
                    open: mw.sidebar_open() && enabled,
                    side: mw.sidebar_side(cx),
                }
            })
            .unwrap_or_default()
    }
}

pub struct StatusBar {
    left_items: Vec<Box<dyn StatusItemViewHandle>>,
    right_items: Vec<Box<dyn StatusItemViewHandle>>,
    active_pane: Entity<Pane>,
    multi_workspace: Option<WeakEntity<MultiWorkspace>>,
    focus_handle: FocusHandle,
    _observe_active_pane: Subscription,
}

impl Focusable for StatusBar {
    fn focus_handle(&self, _cx: &App) -> FocusHandle {
        self.focus_handle.clone()
    }
}

impl Render for StatusBar {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let sidebar = SidebarStatus::query(&self.multi_workspace, cx);

        v_flex()
            .id("status-bar")
            .track_focus(&self.focus_handle)
            .key_context("StatusBar")
            // Expose the status bar as an ARIA toolbar so assistive technology
            // announces it as a toolbar and region navigation can reach its
            // controls. The controls inside form a tab group: region navigation
            // lands on the first control (per the ARIA toolbar pattern), Tab
            // steps through them, and arrow keys move between them once focus is
            // inside.
            .role(Role::Toolbar)
            .aria_label("Status bar")
            .tab_group()
            .on_key_down(
                cx.listener(|status_bar, event: &gpui::KeyDownEvent, window, cx| {
                    if event.keystroke.modifiers.modified() {
                        return;
                    }
                    match event.keystroke.key.as_str() {
                        "right" => {
                            status_bar.move_item_focus(true, window, cx);
                            cx.stop_propagation();
                        }
                        "left" => {
                            status_bar.move_item_focus(false, window, cx);
                            cx.stop_propagation();
                        }
                        _ => {}
                    }
                }),
            )
            .w_full()
            .flex_none()
            .bg(cx.theme().colors().status_bar_background)
            .map(|el| match window.window_decorations() {
                Decorations::Server => el,
                Decorations::Client { tiling, .. } => el
                    .when(
                        !(tiling.bottom || tiling.right)
                            && !(sidebar.open && sidebar.side == SidebarSide::Right),
                        |el| el.rounded_br(CLIENT_SIDE_DECORATION_ROUNDING),
                    )
                    .when(
                        !(tiling.bottom || tiling.left)
                            && !(sidebar.open && sidebar.side == SidebarSide::Left),
                        |el| el.rounded_bl(CLIENT_SIDE_DECORATION_ROUNDING),
                    )
                    // This border is to avoid a transparent gap in the rounded corners
                    .mb(px(-1.))
                    .mt({
                        #[cfg(target_os = "linux")]
                        let needs_gap_fix = {
                            // Running on Wayland and using some scaling levels other than 100% causes a
                            // 1px gap above the status bar; adding a margin avoids this.
                            gpui::guess_compositor() == "Wayland" && window.scale_factor() != 1.0
                        };
                        #[cfg(not(target_os = "linux"))]
                        let needs_gap_fix = false;
                        if needs_gap_fix { px(-1.) } else { px(0.) }
                    })
                    .border_b(px(1.0))
                    .border_color(cx.theme().colors().status_bar_background),
            })
            // The top line is a child rather than a border: with client-side
            // decorations the border colour is already used to hide the corner gap.
            .child(
                div()
                    .w_full()
                    .h(STATUS_BAR_LINE)
                    .flex_none()
                    .bg(cx.theme().colors().border),
            )
            .child(
                h_flex()
                    .w_full()
                    .h(status_bar_row_height(window, cx))
                    .px(STATUS_BAR_SIDE_PADDING)
                    .overflow_hidden()
                    .justify_between()
                    .child(self.render_left_tools(cx))
                    .child(self.render_right_tools(cx)),
            )
    }
}

impl StatusBar {
    /// When the window is too narrow for both sides, the bar gives up its
    /// middle first: the left side is clipped from its right end until nothing
    /// is left of it, and only then the right side from its left end, so the
    /// items next to the window's right edge (the AI chip, the language, the
    /// cursor position) are the last to go.
    fn render_left_tools(&self, cx: &mut Context<Self>) -> impl IntoElement {
        // No gap: the items are adjoining full-height hit targets.
        h_flex()
            .h_full()
            .min_w_0()
            // Shrinks long before the right side does.
            .flex_shrink(LEFT_TOOLS_SHRINK)
            .overflow_x_hidden()
            .children(self.left_items.iter().enumerate().map(|(index, item)| {
                render_hideable_item("status-bar-left", index, item.as_ref(), cx)
            }))
    }

    fn render_right_tools(&self, cx: &mut Context<Self>) -> impl IntoElement {
        h_flex()
            .h_full()
            .min_w_0()
            .flex_shrink(1.)
            // What does not fit leaves on the left, towards the middle of the bar.
            .justify_end()
            .overflow_x_hidden()
            .children(
                self.right_items
                    .iter()
                    .enumerate()
                    .rev()
                    .map(|(index, item)| {
                        render_hideable_item("status-bar-right", index, item.as_ref(), cx)
                    }),
            )
    }
}

fn render_hideable_item(
    side: &'static str,
    index: usize,
    item: &dyn StatusItemViewHandle,
    cx: &App,
) -> impl IntoElement {
    let view = item.to_any();
    let Some(hide) = item.hide_setting(cx) else {
        return view.into_any_element();
    };

    let menu_id: SharedString = format!("{side}-item-menu-{index}").into();
    right_click_menu(menu_id)
        .trigger(move |_is_active, _window, _cx| view)
        .menu(move |window, cx| {
            let hide = hide.clone();
            ContextMenu::build(window, cx, move |menu, _window, _cx| {
                add_hide_button_entry(menu, hide)
            })
        })
        .into_any_element()
}

/// Appends a "Hide Button" entry aligned with surrounding toggleable entries.
pub fn add_hide_button_entry(menu: ContextMenu, hide: HideStatusItem) -> ContextMenu {
    menu.toggleable_entry(
        "Hide Button",
        false,
        IconPosition::Start,
        None,
        move |_window, cx| hide.apply(cx),
    )
}

impl StatusBar {
    pub fn new(
        active_pane: &Entity<Pane>,
        multi_workspace: Option<WeakEntity<MultiWorkspace>>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Self {
        let mut this = Self {
            left_items: Default::default(),
            right_items: Default::default(),
            active_pane: active_pane.clone(),
            multi_workspace,
            focus_handle: cx.focus_handle(),
            _observe_active_pane: cx.observe_in(active_pane, window, |this, _, window, cx| {
                this.update_active_pane_item(window, cx)
            }),
        };
        this.update_active_pane_item(window, cx);
        this
    }

    pub fn set_multi_workspace(
        &mut self,
        multi_workspace: WeakEntity<MultiWorkspace>,
        cx: &mut Context<Self>,
    ) {
        self.multi_workspace = Some(multi_workspace);
        cx.notify();
    }

    pub fn add_left_item<T>(&mut self, item: Entity<T>, window: &mut Window, cx: &mut Context<Self>)
    where
        T: 'static + StatusItemView,
    {
        let active_pane_item = self.active_pane.read(cx).active_item();
        item.set_active_pane_item(active_pane_item.as_deref(), window, cx);

        self.left_items.push(Box::new(item));
        cx.notify();
    }

    pub fn item_of_type<T: StatusItemView>(&self) -> Option<Entity<T>> {
        self.left_items
            .iter()
            .chain(self.right_items.iter())
            .find_map(|item| item.to_any().downcast().ok())
    }

    pub fn position_of_item<T>(&self) -> Option<usize>
    where
        T: StatusItemView,
    {
        for (index, item) in self.left_items.iter().enumerate() {
            if item.item_type() == TypeId::of::<T>() {
                return Some(index);
            }
        }
        for (index, item) in self.right_items.iter().enumerate() {
            if item.item_type() == TypeId::of::<T>() {
                return Some(index + self.left_items.len());
            }
        }
        None
    }

    pub fn insert_item_after<T>(
        &mut self,
        position: usize,
        item: Entity<T>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) where
        T: 'static + StatusItemView,
    {
        let active_pane_item = self.active_pane.read(cx).active_item();
        item.set_active_pane_item(active_pane_item.as_deref(), window, cx);

        if position < self.left_items.len() {
            self.left_items.insert(position + 1, Box::new(item))
        } else {
            self.right_items
                .insert(position + 1 - self.left_items.len(), Box::new(item))
        }
        cx.notify()
    }

    pub fn remove_item_at(&mut self, position: usize, cx: &mut Context<Self>) {
        if position < self.left_items.len() {
            self.left_items.remove(position);
        } else {
            self.right_items.remove(position - self.left_items.len());
        }
        cx.notify();
    }

    pub fn add_right_item<T>(
        &mut self,
        item: Entity<T>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) where
        T: 'static + StatusItemView,
    {
        let active_pane_item = self.active_pane.read(cx).active_item();
        item.set_active_pane_item(active_pane_item.as_deref(), window, cx);

        self.right_items.push(Box::new(item));
        cx.notify();
    }

    pub fn set_active_pane(
        &mut self,
        active_pane: &Entity<Pane>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.active_pane = active_pane.clone();
        self._observe_active_pane = cx.observe_in(active_pane, window, |this, _, window, cx| {
            this.update_active_pane_item(window, cx)
        });
        self.update_active_pane_item(window, cx);
    }

    fn update_active_pane_item(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let active_pane_item = self.active_pane.read(cx).active_item();
        for item in self.left_items.iter().chain(&self.right_items) {
            item.set_active_pane_item(active_pane_item.as_deref(), window, cx);
        }
    }

    /// Moves focus between the interactive controls within the status bar in
    /// response to arrow keys. Navigation is clamped to the status bar so
    /// arrows move between items and stop at the ends (ARIA toolbar semantics);
    /// Tab is still used to leave the toolbar.
    fn move_item_focus(&mut self, forward: bool, window: &mut Window, cx: &mut Context<Self>) {
        let previous = window.focused(cx);
        if forward {
            window.focus_next(cx);
        } else {
            window.focus_prev(cx);
        }
        let landed_in_status_bar = window
            .focused(cx)
            .is_some_and(|handle| self.focus_handle.contains(&handle, window));
        if !landed_in_status_bar && let Some(previous) = previous {
            window.focus(&previous, cx);
        }
        cx.notify();
    }
}

impl<T: StatusItemView> StatusItemViewHandle for Entity<T> {
    fn to_any(&self) -> AnyView {
        self.clone().into()
    }

    fn set_active_pane_item(
        &self,
        active_pane_item: Option<&dyn ItemHandle>,
        window: &mut Window,
        cx: &mut App,
    ) {
        self.update(cx, |this, cx| {
            this.set_active_pane_item(active_pane_item, window, cx)
        });
    }

    fn item_type(&self) -> TypeId {
        TypeId::of::<T>()
    }

    fn hide_setting(&self, cx: &App) -> Option<HideStatusItem> {
        self.read(cx).hide_setting(cx)
    }
}

impl From<&dyn StatusItemViewHandle> for AnyView {
    fn from(val: &dyn StatusItemViewHandle) -> Self {
        val.to_any()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::Workspace;
    use fs::FakeFs;
    use gpui::{TestAppContext, VisualTestContext, size};
    use project::Project;
    use settings::SettingsStore;

    /// A status bar item of a fixed width that can be found in the window.
    struct FixedItem {
        selector: &'static str,
        width: Pixels,
    }

    impl Render for FixedItem {
        fn render(&mut self, _: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
            let selector = self.selector;
            div()
                .debug_selector(move || selector.to_string())
                .flex_none()
                .w(self.width)
                .h_full()
        }
    }

    impl StatusItemView for FixedItem {
        fn set_active_pane_item(
            &mut self,
            _: Option<&dyn ItemHandle>,
            _: &mut Window,
            _: &mut Context<Self>,
        ) {
        }

        fn hide_setting(&self, _: &App) -> Option<HideStatusItem> {
            None
        }
    }

    fn init_test(cx: &mut TestAppContext) {
        cx.update(|cx| {
            let settings_store = SettingsStore::test(cx);
            cx.set_global(settings_store);
            cx.set_global(db::AppDatabase::test_new());
            theme_settings::init(theme::LoadThemes::JustBase, cx);
        });
    }

    #[track_caller]
    fn horizontal_extent(selector: &'static str, cx: &mut VisualTestContext) -> (Pixels, Pixels) {
        let bounds = cx
            .debug_bounds(selector)
            .unwrap_or_else(|| panic!("no element with the selector {selector}"));
        (bounds.left(), bounds.right())
    }

    #[track_caller]
    fn assert_near(actual: Pixels, expected: Pixels, what: &str) {
        assert!(
            (actual - expected).abs() <= px(1.),
            "{what}: {actual:?}, expected {expected:?}"
        );
    }

    #[gpui::test]
    async fn test_status_bar_gives_up_its_middle_first(cx: &mut TestAppContext) {
        init_test(cx);
        let fs = FakeFs::new(cx.executor());
        let project = Project::test(fs, [], cx).await;
        let (workspace, cx) =
            cx.add_window_view(|window, cx| Workspace::test_new(project, window, cx));
        workspace.update_in(cx, |workspace, window, cx| {
            let left = cx.new(|_| FixedItem {
                selector: "STATUS-LEFT",
                width: px(400.),
            });
            let right = cx.new(|_| FixedItem {
                selector: "STATUS-RIGHT",
                width: px(200.),
            });
            workspace.status_bar().update(cx, |status_bar, cx| {
                status_bar.add_left_item(left, window, cx);
                status_bar.add_right_item(right, window, cx);
            });
        });

        // Room for both: each side sits against its edge, inside the side padding.
        cx.simulate_resize(size(px(1000.), px(600.)));
        cx.run_until_parked();
        let (left_start, left_end) = horizontal_extent("STATUS-LEFT", cx);
        let (right_start, right_end) = horizontal_extent("STATUS-RIGHT", cx);
        assert_near(
            left_start,
            STATUS_BAR_SIDE_PADDING,
            "left item, wide window",
        );
        assert_near(left_end, px(402.), "left item's end, wide window");
        assert_near(right_start, px(798.), "right item, wide window");
        assert_near(right_end, px(998.), "right item's end, wide window");

        // Too narrow for both: the right side keeps its place and its width,
        // the left side is what gets cut.
        cx.simulate_resize(size(px(500.), px(600.)));
        cx.run_until_parked();
        let (left_start, _) = horizontal_extent("STATUS-LEFT", cx);
        let (right_start, right_end) = horizontal_extent("STATUS-RIGHT", cx);
        assert_near(
            left_start,
            STATUS_BAR_SIDE_PADDING,
            "left item, narrow window",
        );
        assert_near(right_start, px(298.), "right item, narrow window");
        assert_near(right_end, px(498.), "right item's end, narrow window");

        // Too narrow for the right side alone: it stays against the window's
        // right edge and loses its left end.
        cx.simulate_resize(size(px(150.), px(600.)));
        cx.run_until_parked();
        let (right_start, right_end) = horizontal_extent("STATUS-RIGHT", cx);
        assert_near(right_end, px(148.), "right item's end, very narrow window");
        assert_near(right_start, px(-52.), "right item, very narrow window");
    }

    #[gpui::test]
    async fn test_status_bar_item_height_follows_the_bar(cx: &mut TestAppContext) {
        init_test(cx);
        cx.update(|cx| {
            let bar = ui::metrics::status_bar_height(cx);
            let item = status_bar_item_height_for(cx);
            assert_eq!(item.0, bar.0 - rems_from_px(1_f32).0);
        });
    }
}

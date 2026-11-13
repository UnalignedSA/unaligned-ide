use crate::Workspace;
use crate::focus_follows_mouse::FocusFollowsMouse as _;
use crate::panel_drag::panel_drag_source;
use crate::persistence::model::DockData;
use crate::status_bar::HideStatusItem;
use crate::{DraggedDock, Event, ModalLayer, Pane, WorkspaceSettings};
use anyhow::Context as _;
use client::proto;
use db::kvp::KeyValueStore;

use gpui::{
    Action, Anchor, AnyElement, AnyView, App, Axis, Context, Entity, EntityId, EventEmitter,
    FocusHandle, Focusable, IntoElement, KeyContext, MouseButton, MouseDownEvent, MouseUpEvent,
    ParentElement, Render, SharedString, StyleRefinement, Styled, Subscription, WeakEntity, Window,
    deferred, div, px,
};
use serde::{Deserialize, Serialize};
use settings::{Settings, SettingsStore, TerminalDockPosition};
use std::sync::Arc;
use ui::{ContextMenu, IconButton, TextTab, TextTabStrip, Tooltip, prelude::*, right_click_menu};
use util::ResultExt as _;

pub(crate) const RESIZE_HANDLE_SIZE: Pixels = px(6.);

pub enum PanelEvent {
    ZoomIn,
    ZoomOut,
    Activate,
    Close,
}

pub use proto::PanelId;

pub trait Panel: Focusable + EventEmitter<PanelEvent> + Render + Sized {
    fn persistent_name() -> &'static str;
    fn panel_key() -> &'static str;
    /// The `Focusable::focus_handle` root identifies the panel's subtree for containment checks
    /// and must be tracked by the panel's root element. This method returns the handle that should
    /// receive focus when the panel is activated, such as a filter, commit, or message editor; it
    /// must be a focus-tree descendant of the root or containment checks such as Zen-mode auto-close
    /// and toggle-focus will misbehave.
    fn activation_focus_handle(&self, cx: &App) -> FocusHandle {
        self.focus_handle(cx)
    }
    fn position(&self, window: &Window, cx: &App) -> DockPosition;
    fn position_is_valid(&self, position: DockPosition) -> bool;
    fn set_position(&mut self, position: DockPosition, window: &mut Window, cx: &mut Context<Self>);
    fn default_size(&self, window: &Window, cx: &App) -> Pixels;
    fn min_size(&self, _window: &Window, _cx: &App) -> Option<Pixels> {
        None
    }
    fn initial_size_state(&self, _window: &Window, _cx: &App) -> PanelSizeState {
        PanelSizeState::default()
    }
    fn size_state_changed(&mut self, _window: &mut Window, _cx: &mut Context<Self>) {}
    fn supports_flexible_size(&self) -> bool {
        false
    }
    fn has_flexible_size(&self, _window: &Window, _cx: &App) -> bool {
        false
    }
    fn set_flexible_size(
        &mut self,
        _flexible: bool,
        _window: &mut Window,
        _cx: &mut Context<Self>,
    ) {
    }
    fn icon(&self, window: &Window, cx: &App) -> Option<ui::IconName>;
    fn icon_tooltip(&self, window: &Window, cx: &App) -> Option<&'static str>;
    /// The panel's name as shown on its tab, in its header and on its drag
    /// preview. The default is the button tooltip without its "Panel" suffix
    /// ("Terminal Panel" gives "Terminal"), or the persistent name when the
    /// panel has no tooltip ("DebugPanel" gives "Debug").
    fn title(&self, window: &Window, cx: &App) -> SharedString {
        default_panel_title(
            self.icon_tooltip(window, cx)
                .unwrap_or_else(|| Self::persistent_name()),
        )
    }
    fn toggle_action(&self) -> Box<dyn Action>;
    fn icon_label(&self, _window: &Window, _: &App) -> Option<String> {
        None
    }
    fn is_zoomed(&self, _window: &Window, _cx: &App) -> bool {
        false
    }
    fn starts_open(&self, _window: &Window, _cx: &App) -> bool {
        false
    }
    fn set_zoomed(&mut self, _zoomed: bool, _window: &mut Window, _cx: &mut Context<Self>) {}
    /// Whether `set_zoomed` does something. The header of the bottom dock
    /// offers Maximize only for such a panel, so the button is never dead.
    /// The default covers the chat, which handles zoom without saying so yet.
    fn supports_zoom(&self, _window: &Window, _cx: &App) -> bool {
        self.is_agent_panel()
    }
    /// The panel's own buttons for the header of the bottom dock, drawn at the
    /// right of the tabs, before Maximize and Hide. A panel that returns them
    /// does not draw the same buttons in a row of its own.
    ///
    /// Only the bottom dock asks, and only for the panel it shows. It also
    /// asks while the workspace is being rendered (a maximized panel), so an
    /// implementation must not read the workspace entity.
    fn header_actions(
        &mut self,
        _window: &mut Window,
        _cx: &mut Context<Self>,
    ) -> Option<AnyElement> {
        None
    }
    fn set_active(&mut self, _active: bool, _window: &mut Window, _cx: &mut Context<Self>) {}
    fn pane(&self) -> Option<Entity<Pane>> {
        None
    }
    fn remote_id() -> Option<proto::PanelId> {
        None
    }
    fn activation_priority(&self) -> u32;
    fn enabled(&self, _cx: &App) -> bool {
        true
    }
    fn is_agent_panel(&self) -> bool {
        false
    }
    /// Returns metadata describing how to hide this panel's button from the
    /// status bar by writing to user settings. Implementors should return
    /// `None` if the panel button cannot be hidden through settings.
    fn hide_button_setting(&self, _: &App) -> Option<HideStatusItem> {
        None
    }
}

/// A display name from a tooltip or a persistent name: "Project Panel" and
/// "DebugPanel" lose their suffix.
pub(crate) fn default_panel_title(name: &str) -> SharedString {
    let name = name.trim();
    let name = name
        .strip_suffix("Panel")
        .map(str::trim_end)
        .filter(|name| !name.is_empty())
        .unwrap_or(name);
    SharedString::from(name.to_string())
}

pub trait PanelHandle: Send + Sync {
    fn panel_id(&self) -> EntityId;
    fn persistent_name(&self) -> &'static str;
    fn panel_key(&self) -> &'static str;
    fn position(&self, window: &Window, cx: &App) -> DockPosition;
    fn position_is_valid(&self, position: DockPosition, cx: &App) -> bool;
    fn set_position(&self, position: DockPosition, window: &mut Window, cx: &mut App);
    fn is_zoomed(&self, window: &Window, cx: &App) -> bool;
    fn set_zoomed(&self, zoomed: bool, window: &mut Window, cx: &mut App);
    /// See `Panel::supports_zoom`.
    fn supports_zoom(&self, window: &Window, cx: &App) -> bool;
    /// See `Panel::header_actions`.
    fn header_actions(&self, window: &mut Window, cx: &mut App) -> Option<AnyElement>;
    fn set_active(&self, active: bool, window: &mut Window, cx: &mut App);
    fn remote_id(&self) -> Option<proto::PanelId>;
    fn pane(&self, cx: &App) -> Option<Entity<Pane>>;
    fn default_size(&self, window: &Window, cx: &App) -> Pixels;
    fn min_size(&self, window: &Window, cx: &App) -> Option<Pixels>;
    fn initial_size_state(&self, window: &Window, cx: &App) -> PanelSizeState;
    fn size_state_changed(&self, window: &mut Window, cx: &mut App);
    fn supports_flexible_size(&self, cx: &App) -> bool;
    fn has_flexible_size(&self, window: &Window, cx: &App) -> bool;
    fn set_flexible_size(&self, flexible: bool, window: &mut Window, cx: &mut App);
    fn icon(&self, window: &Window, cx: &App) -> Option<ui::IconName>;
    fn icon_tooltip(&self, window: &Window, cx: &App) -> Option<&'static str>;
    /// See `Panel::title`.
    fn title(&self, window: &Window, cx: &App) -> SharedString;
    fn toggle_action(&self, window: &Window, cx: &App) -> Box<dyn Action>;
    fn icon_label(&self, window: &Window, cx: &App) -> Option<String>;
    fn panel_focus_handle(&self, cx: &App) -> FocusHandle;
    /// See `Panel::activation_focus_handle`.
    fn activation_focus_handle(&self, cx: &App) -> FocusHandle;
    fn to_any(&self) -> AnyView;
    fn activation_priority(&self, cx: &App) -> u32;
    fn enabled(&self, cx: &App) -> bool;
    fn is_agent_panel(&self, cx: &App) -> bool;
    fn hide_button_setting(&self, cx: &App) -> Option<HideStatusItem>;
    fn move_to_next_position(&self, window: &mut Window, cx: &mut App) {
        let current_position = self.position(window, cx);
        let next_position = [
            DockPosition::Left,
            DockPosition::Bottom,
            DockPosition::Right,
        ]
        .into_iter()
        .filter(|position| self.position_is_valid(*position, cx))
        .skip_while(|valid_position| *valid_position != current_position)
        .nth(1)
        .unwrap_or(DockPosition::Left);

        self.set_position(next_position, window, cx);
    }
}

impl<T> PanelHandle for Entity<T>
where
    T: Panel,
{
    fn panel_id(&self) -> EntityId {
        Entity::entity_id(self)
    }

    fn persistent_name(&self) -> &'static str {
        T::persistent_name()
    }

    fn panel_key(&self) -> &'static str {
        T::panel_key()
    }

    fn position(&self, window: &Window, cx: &App) -> DockPosition {
        self.read(cx).position(window, cx)
    }

    fn position_is_valid(&self, position: DockPosition, cx: &App) -> bool {
        self.read(cx).position_is_valid(position)
    }

    fn set_position(&self, position: DockPosition, window: &mut Window, cx: &mut App) {
        self.update(cx, |this, cx| this.set_position(position, window, cx))
    }

    fn is_zoomed(&self, window: &Window, cx: &App) -> bool {
        self.read(cx).is_zoomed(window, cx)
    }

    fn set_zoomed(&self, zoomed: bool, window: &mut Window, cx: &mut App) {
        self.update(cx, |this, cx| this.set_zoomed(zoomed, window, cx))
    }

    fn supports_zoom(&self, window: &Window, cx: &App) -> bool {
        self.read(cx).supports_zoom(window, cx)
    }

    fn header_actions(&self, window: &mut Window, cx: &mut App) -> Option<AnyElement> {
        self.update(cx, |this, cx| this.header_actions(window, cx))
    }

    fn set_active(&self, active: bool, window: &mut Window, cx: &mut App) {
        self.update(cx, |this, cx| this.set_active(active, window, cx))
    }

    fn pane(&self, cx: &App) -> Option<Entity<Pane>> {
        self.read(cx).pane()
    }

    fn remote_id(&self) -> Option<PanelId> {
        T::remote_id()
    }

    fn default_size(&self, window: &Window, cx: &App) -> Pixels {
        self.read(cx).default_size(window, cx)
    }

    fn min_size(&self, window: &Window, cx: &App) -> Option<Pixels> {
        self.read(cx).min_size(window, cx)
    }

    fn initial_size_state(&self, window: &Window, cx: &App) -> PanelSizeState {
        self.read(cx).initial_size_state(window, cx)
    }

    fn size_state_changed(&self, window: &mut Window, cx: &mut App) {
        self.update(cx, |this, cx| this.size_state_changed(window, cx))
    }

    fn supports_flexible_size(&self, cx: &App) -> bool {
        self.read(cx).supports_flexible_size()
    }

    fn has_flexible_size(&self, window: &Window, cx: &App) -> bool {
        self.read(cx).has_flexible_size(window, cx)
    }

    fn set_flexible_size(&self, flexible: bool, window: &mut Window, cx: &mut App) {
        self.update(cx, |this, cx| this.set_flexible_size(flexible, window, cx))
    }

    fn icon(&self, window: &Window, cx: &App) -> Option<ui::IconName> {
        self.read(cx).icon(window, cx)
    }

    fn icon_tooltip(&self, window: &Window, cx: &App) -> Option<&'static str> {
        self.read(cx).icon_tooltip(window, cx)
    }

    fn title(&self, window: &Window, cx: &App) -> SharedString {
        self.read(cx).title(window, cx)
    }

    fn toggle_action(&self, _: &Window, cx: &App) -> Box<dyn Action> {
        self.read(cx).toggle_action()
    }

    fn icon_label(&self, window: &Window, cx: &App) -> Option<String> {
        self.read(cx).icon_label(window, cx)
    }

    fn to_any(&self) -> AnyView {
        self.clone().into()
    }

    fn panel_focus_handle(&self, cx: &App) -> FocusHandle {
        self.read(cx).focus_handle(cx)
    }

    fn activation_focus_handle(&self, cx: &App) -> FocusHandle {
        self.read(cx).activation_focus_handle(cx)
    }

    fn activation_priority(&self, cx: &App) -> u32 {
        self.read(cx).activation_priority()
    }

    fn enabled(&self, cx: &App) -> bool {
        self.read(cx).enabled(cx)
    }

    fn is_agent_panel(&self, cx: &App) -> bool {
        self.read(cx).is_agent_panel()
    }

    fn hide_button_setting(&self, cx: &App) -> Option<HideStatusItem> {
        self.read(cx).hide_button_setting(cx)
    }
}

impl From<&dyn PanelHandle> for AnyView {
    fn from(val: &dyn PanelHandle) -> Self {
        val.to_any()
    }
}

/// A container with a fixed [`DockPosition`] adjacent to a certain widown edge.
/// Can contain multiple panels and show/hide itself with all contents.
pub struct Dock {
    position: DockPosition,
    panel_entries: Vec<PanelEntry>,
    workspace: WeakEntity<Workspace>,
    is_open: bool,
    active_panel_index: Option<usize>,
    focus_handle: FocusHandle,
    restoration: DockRestoreState,
    zoom_layer_open: bool,
    modal_layer: Entity<ModalLayer>,
    /// Set while the resize handle is dragged, so the sash stays highlighted
    /// even though hover styles are suspended during a drag.
    resizing: bool,
    /// Set while a panel is dragged to another dock: the sash is not drawn,
    /// because it would sit above the drop zones and swallow the drop.
    panel_drag_active: bool,
    /// Panels resized by a sash drag whose size is not stored yet: it is
    /// written once when the drag ends, not on every mouse move.
    unpersisted_panel_keys: Vec<&'static str>,
    _subscriptions: [Subscription; 2],
}

enum DockRestoreState {
    Restoring { pending: Option<DockData> },
    Finished,
}

impl DockRestoreState {
    fn pending(&self) -> Option<&DockData> {
        match self {
            Self::Restoring { pending } => pending.as_ref(),
            Self::Finished => None,
        }
    }

    fn discard_pending(&mut self) {
        if let Self::Restoring { pending } = self {
            *pending = None;
        }
    }
}

impl Focusable for Dock {
    fn focus_handle(&self, _: &App) -> FocusHandle {
        self.focus_handle.clone()
    }
}

#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub enum DockPosition {
    Left,
    Bottom,
    Right,
}

impl From<settings::DockPosition> for DockPosition {
    fn from(value: settings::DockPosition) -> Self {
        match value {
            settings::DockPosition::Left => Self::Left,
            settings::DockPosition::Bottom => Self::Bottom,
            settings::DockPosition::Right => Self::Right,
        }
    }
}

impl Into<settings::DockPosition> for DockPosition {
    fn into(self) -> settings::DockPosition {
        match self {
            Self::Left => settings::DockPosition::Left,
            Self::Bottom => settings::DockPosition::Bottom,
            Self::Right => settings::DockPosition::Right,
        }
    }
}

impl From<TerminalDockPosition> for DockPosition {
    fn from(value: TerminalDockPosition) -> Self {
        match value {
            TerminalDockPosition::Left => DockPosition::Left,
            TerminalDockPosition::Bottom => DockPosition::Bottom,
            TerminalDockPosition::Right => DockPosition::Right,
        }
    }
}

impl DockPosition {
    fn label(&self) -> &'static str {
        match self {
            Self::Left => "Left",
            Self::Bottom => "Bottom",
            Self::Right => "Right",
        }
    }

    pub fn axis(&self) -> Axis {
        match self {
            Self::Left | Self::Right => Axis::Horizontal,
            Self::Bottom => Axis::Vertical,
        }
    }
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct PanelSizeState {
    pub size: Option<Pixels>,
    #[serde(default)]
    pub flex: Option<f32>,
}

struct PanelEntry {
    panel: Arc<dyn PanelHandle>,
    size_state: PanelSizeState,
    _subscriptions: [Subscription; 4],
}

pub(crate) const PANEL_SIZE_STATE_KEY: &str = "dock_panel_size";

fn panel_uses_flexible_width(
    position: DockPosition,
    panel: &dyn PanelHandle,
    window: &Window,
    cx: &App,
) -> bool {
    position.axis() == Axis::Horizontal && panel.has_flexible_size(window, cx)
}

fn resize_panel_entry(
    position: DockPosition,
    entry: &mut PanelEntry,
    size: Option<Pixels>,
    flex: Option<f32>,
    window: &mut Window,
    cx: &mut App,
) -> (&'static str, PanelSizeState) {
    let size = size.map(|size| size.max(RESIZE_HANDLE_SIZE).round());
    let uses_flexible_width = panel_uses_flexible_width(position, entry.panel.as_ref(), window, cx);
    if uses_flexible_width {
        entry.size_state.flex = flex;
    } else {
        entry.size_state.size = size;
    }
    entry.panel.size_state_changed(window, cx);
    (entry.panel.panel_key(), entry.size_state)
}

impl Dock {
    pub fn new(
        position: DockPosition,
        modal_layer: Entity<ModalLayer>,
        window: &mut Window,
        cx: &mut Context<Workspace>,
    ) -> Entity<Self> {
        let focus_handle = cx.focus_handle();
        let workspace = cx.entity();
        let dock = cx.new(|cx| {
            let focus_subscription =
                cx.on_focus(&focus_handle, window, |dock: &mut Dock, window, cx| {
                    if let Some(active_entry) = dock.active_panel_entry() {
                        active_entry
                            .panel
                            .activation_focus_handle(cx)
                            .focus(window, cx)
                    }
                });
            let zoom_subscription = cx.subscribe(&workspace, |dock, workspace, e: &Event, cx| {
                if matches!(e, Event::ZoomChanged) {
                    let is_zoomed = workspace.read(cx).zoomed.is_some();
                    dock.zoom_layer_open = is_zoomed;
                }
            });
            Self {
                position,
                workspace: workspace.downgrade(),
                panel_entries: Default::default(),
                active_panel_index: None,
                is_open: false,
                focus_handle: focus_handle.clone(),
                _subscriptions: [focus_subscription, zoom_subscription],
                restoration: DockRestoreState::Restoring { pending: None },
                zoom_layer_open: false,
                modal_layer,
                resizing: false,
                panel_drag_active: false,
                unpersisted_panel_keys: Vec::new(),
            }
        });

        cx.on_focus_in(&focus_handle, window, {
            let dock = dock.downgrade();
            move |workspace, window, cx| {
                let Some(dock) = dock.upgrade() else {
                    return;
                };
                let Some(panel) = dock.read(cx).active_panel() else {
                    return;
                };
                if panel.is_zoomed(window, cx) {
                    workspace.zoomed = Some(panel.to_any().downgrade());
                    workspace.zoomed_position = Some(position);
                } else {
                    workspace.zoomed = None;
                    workspace.zoomed_position = None;
                }
                cx.emit(Event::ZoomChanged);
                workspace.dismiss_zoomed_items_to_reveal(Some(position), window, cx);
                workspace.update_active_view_for_followers(window, cx)
            }
        })
        .detach();

        cx.observe_in(&dock, window, move |workspace, dock, window, cx| {
            if dock.read(cx).is_open()
                && let Some(panel) = dock.read(cx).active_panel()
                && panel.is_zoomed(window, cx)
            {
                workspace.zoomed = Some(panel.to_any().downgrade());
                workspace.zoomed_position = Some(position);
                cx.emit(Event::ZoomChanged);
                return;
            }
            if workspace.zoomed_position == Some(position) {
                workspace.zoomed = None;
                workspace.zoomed_position = None;
                cx.emit(Event::ZoomChanged);
            }
        })
        .detach();

        dock
    }

    pub fn position(&self) -> DockPosition {
        self.position
    }

    pub fn is_open(&self) -> bool {
        self.is_open
    }

    fn resizable(&self, cx: &App) -> bool {
        !(self.zoom_layer_open || self.modal_layer.read(cx).has_active_modal())
    }

    pub fn panel<T: Panel>(&self) -> Option<Entity<T>> {
        self.panel_entries
            .iter()
            .find_map(|entry| entry.panel.to_any().downcast().ok())
    }

    pub fn panel_index_for_type<T: Panel>(&self) -> Option<usize> {
        self.panel_entries
            .iter()
            .position(|entry| entry.panel.to_any().downcast::<T>().is_ok())
    }

    pub fn panel_index_for_persistent_name(&self, ui_name: &str, _cx: &App) -> Option<usize> {
        self.panel_entries
            .iter()
            .position(|entry| entry.panel.persistent_name() == ui_name)
    }

    pub fn panel_index_for_proto_id(&self, panel_id: PanelId) -> Option<usize> {
        self.panel_entries
            .iter()
            .position(|entry| entry.panel.remote_id() == Some(panel_id))
    }

    pub fn panel_index_for_id(&self, panel_id: EntityId) -> Option<usize> {
        self.panel_entries
            .iter()
            .position(|entry| entry.panel.panel_id() == panel_id)
    }

    /// Called by the workspace when a panel drag starts; the dock clears the
    /// flag itself once no drag is active.
    pub(crate) fn set_panel_drag_active(&mut self, cx: &mut Context<Self>) {
        if !self.panel_drag_active {
            self.panel_drag_active = true;
            cx.notify();
        }
    }

    /// The panels of this dock, in the order of their buttons and tabs.
    pub fn panels(&self) -> impl Iterator<Item = &Arc<dyn PanelHandle>> {
        self.panel_entries.iter().map(|entry| &entry.panel)
    }

    pub fn panel_for_id(&self, panel_id: EntityId) -> Option<&Arc<dyn PanelHandle>> {
        self.panel_entries
            .iter()
            .find(|entry| entry.panel.panel_id() == panel_id)
            .map(|entry| &entry.panel)
    }

    pub fn first_enabled_panel_idx(&mut self, cx: &mut Context<Self>) -> anyhow::Result<usize> {
        self.panel_entries
            .iter()
            .position(|entry| entry.panel.enabled(cx))
            .with_context(|| {
                format!(
                    "Couldn't find any enabled panel for the {} dock.",
                    self.position.label()
                )
            })
    }

    fn active_panel_entry(&self) -> Option<&PanelEntry> {
        self.active_panel_index
            .and_then(|index| self.panel_entries.get(index))
    }

    fn active_panel_entry_mut(&mut self) -> Option<&mut PanelEntry> {
        self.active_panel_index
            .and_then(|index| self.panel_entries.get_mut(index))
    }

    pub fn active_panel_index(&self) -> Option<usize> {
        self.active_panel_index
    }

    pub fn set_open(&mut self, open: bool, window: &mut Window, cx: &mut Context<Self>) {
        if open != self.is_open {
            self.restoration.discard_pending();
        }
        self.set_open_internal(open, window, cx);
    }

    fn set_open_internal(&mut self, open: bool, window: &mut Window, cx: &mut Context<Self>) {
        if open != self.is_open {
            self.is_open = open;
            if let Some(active_panel) = self.active_panel_entry() {
                active_panel.panel.set_active(open, window, cx);
            }

            cx.notify();
        }
    }

    pub fn set_panel_zoomed(
        &mut self,
        panel: &AnyView,
        zoomed: bool,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        for entry in &mut self.panel_entries {
            if entry.panel.panel_id() == panel.entity_id() {
                if zoomed != entry.panel.is_zoomed(window, cx) {
                    entry.panel.set_zoomed(zoomed, window, cx);
                }
            } else if entry.panel.is_zoomed(window, cx) {
                entry.panel.set_zoomed(false, window, cx);
            }
        }

        self.workspace
            .update(cx, |workspace, cx| {
                workspace.serialize_workspace(window, cx);
            })
            .ok();
        cx.notify();
    }

    pub fn zoom_out(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        for entry in &mut self.panel_entries {
            if entry.panel.is_zoomed(window, cx) {
                entry.panel.set_zoomed(false, window, cx);
            }
        }
    }

    pub(crate) fn add_panel<T: Panel>(
        &mut self,
        panel: Entity<T>,
        workspace: WeakEntity<Workspace>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> usize {
        let subscriptions = [
            cx.observe(&panel, |_, _, cx| cx.notify()),
            cx.observe_global_in::<SettingsStore>(window, {
                let workspace = workspace.clone();
                let panel = panel.clone();

                move |this, window, cx| {
                    let new_position = panel.read(cx).position(window, cx);
                    if new_position == this.position {
                        return;
                    }

                    let panel_id = Entity::entity_id(&panel);
                    let Ok((new_dock, reveal)) = workspace.update(cx, |workspace, cx| {
                        if panel.is_zoomed(window, cx) {
                            workspace.zoomed_position = Some(new_position);
                        }
                        // Set by `Workspace::move_panel_to_dock`: the user asked
                        // for this move, so the panel arrives open and focused
                        // even if it was hidden in the dock it leaves.
                        let reveal = workspace.take_panel_reveal(panel_id, new_position);
                        let new_dock = match new_position {
                            DockPosition::Left => &workspace.left_dock,
                            DockPosition::Bottom => &workspace.bottom_dock,
                            DockPosition::Right => &workspace.right_dock,
                        }
                        .clone();
                        (new_dock, reveal)
                    }) else {
                        return;
                    };

                    let was_visible = this.is_open()
                        && this
                            .visible_panel()
                            .is_some_and(|active_panel| active_panel.panel_id() == panel_id);
                    let size_state = this
                        .panel_entries
                        .iter()
                        .find(|entry| entry.panel.panel_id() == panel_id)
                        .map(|entry| entry.size_state)
                        .unwrap_or_default();

                    let previous_axis = this.position.axis();
                    let next_axis = new_position.axis();
                    let axis_changed = previous_axis != next_axis;
                    let size_state = if previous_axis == next_axis {
                        size_state
                    } else {
                        PanelSizeState::default()
                    };

                    let previous_index = this.panel_index_for_id(panel_id);
                    if !this.remove_panel(&panel, window, cx) {
                        // Panel was already moved from this dock
                        return;
                    }
                    if was_visible && let Some(previous_index) = previous_index {
                        // The dock keeps showing something: the panel that
                        // followed the one that left. Decided once every panel
                        // has reacted to this settings change, so that a change
                        // that moves several panels at once (a layout preset)
                        // does not carry each neighbour along as "was shown".
                        cx.defer_in(window, move |this, window, cx| {
                            this.show_neighbor_of_moved_panel(previous_index, window, cx);
                        });
                    }

                    new_dock.update(cx, |new_dock, cx| {
                        let index =
                            new_dock.add_panel(panel.clone(), workspace.clone(), window, cx);
                        if let Some(added_panel) = new_dock.panel_for_id(panel_id).cloned() {
                            new_dock.set_panel_size_state(added_panel.as_ref(), size_state, cx);
                        }
                        if was_visible || reveal {
                            new_dock.set_open(true, window, cx);
                            new_dock.activate_panel(index, window, cx);
                        }
                        if reveal {
                            window.focus(&panel.read(cx).activation_focus_handle(cx), cx);
                        }
                    });

                    workspace
                        .update(cx, |workspace, cx| {
                            if axis_changed {
                                // The stored record is one size for both axes:
                                // without this an old height would be read back
                                // as a width after a restart.
                                workspace.persist_panel_size_state(T::panel_key(), size_state, cx);
                            }
                            workspace.serialize_workspace(window, cx);
                        })
                        .ok();
                }
            }),
            {
                let panel = panel.clone();
                let mut last_default_size = panel.read(cx).default_size(window, cx);

                cx.observe_global_in::<SettingsStore>(window, move |this, window, cx| {
                    let default_size = panel.read(cx).default_size(window, cx);
                    if default_size == last_default_size {
                        return;
                    }
                    last_default_size = default_size;

                    let panel_id = Entity::entity_id(&panel);
                    if let Some(entry) = this
                        .panel_entries
                        .iter_mut()
                        .find(|entry| entry.panel.panel_id() == panel_id)
                    {
                        entry.size_state.size = None;
                        entry.panel.size_state_changed(window, cx);
                        cx.notify();
                    }
                })
            },
            cx.subscribe_in(
                &panel,
                window,
                move |this, panel, event, window, cx| match event {
                    PanelEvent::ZoomIn => {
                        this.set_panel_zoomed(&panel.to_any(), true, window, cx);
                        if !PanelHandle::panel_focus_handle(panel, cx).contains_focused(window, cx)
                        {
                            window.focus(&panel.read(cx).activation_focus_handle(cx), cx);
                        }
                        workspace
                            .update(cx, |workspace, cx| {
                                workspace.zoomed = Some(panel.downgrade().into());
                                workspace.zoomed_position =
                                    Some(panel.read(cx).position(window, cx));
                                cx.emit(Event::ZoomChanged);
                            })
                            .ok();
                    }
                    PanelEvent::ZoomOut => {
                        this.set_panel_zoomed(&panel.to_any(), false, window, cx);
                        workspace
                            .update(cx, |workspace, cx| {
                                if workspace.zoomed_position == Some(this.position) {
                                    workspace.zoomed = None;
                                    workspace.zoomed_position = None;
                                    cx.emit(Event::ZoomChanged);
                                }
                                cx.notify();
                            })
                            .ok();
                    }
                    PanelEvent::Activate => {
                        if let Some(ix) = this
                            .panel_entries
                            .iter()
                            .position(|entry| entry.panel.panel_id() == Entity::entity_id(panel))
                        {
                            this.activate_panel(ix, window, cx);
                            this.set_open(true, window, cx);
                            window.focus(&panel.read(cx).activation_focus_handle(cx), cx);
                        }
                    }
                    PanelEvent::Close => {
                        if this
                            .visible_panel()
                            .is_some_and(|p| p.panel_id() == Entity::entity_id(panel))
                        {
                            this.set_open(false, window, cx);
                        }
                    }
                },
            ),
        ];

        let index = match self
            .panel_entries
            .binary_search_by_key(&panel.read(cx).activation_priority(), |entry| {
                entry.panel.activation_priority(cx)
            }) {
            Ok(ix) => {
                if cfg!(debug_assertions) {
                    panic!(
                        "Panels `{}` and `{}` have the same activation priority. Each panel must have a unique priority so the status bar order is deterministic.",
                        T::panel_key(),
                        self.panel_entries[ix].panel.panel_key()
                    );
                }
                ix
            }
            Err(ix) => ix,
        };
        if let Some(active_index) = self.active_panel_index.as_mut()
            && *active_index >= index
        {
            *active_index += 1;
        }
        let size_state = panel.read(cx).initial_size_state(window, cx);

        self.panel_entries.insert(
            index,
            PanelEntry {
                panel: Arc::new(panel.clone()),
                size_state,
                _subscriptions: subscriptions,
            },
        );

        self.replay_pending_serialized_state(window, cx);

        if panel.read(cx).starts_open(window, cx) {
            self.activate_panel_internal(index, window, cx);
            self.set_open_internal(true, window, cx);
        }

        cx.notify();
        index
    }

    pub(crate) fn restore_serialized_state(
        &mut self,
        serialized: DockData,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        match &mut self.restoration {
            DockRestoreState::Restoring { pending } => {
                *pending = Some(serialized);
                self.replay_pending_serialized_state(window, cx);
            }
            DockRestoreState::Finished => {
                let active_panel_missing = serialized
                    .active_panel
                    .as_deref()
                    .filter(|_| serialized.visible)
                    .is_some_and(|name| self.panel_index_for_persistent_name(name, cx).is_none());
                if !active_panel_missing {
                    self.apply_serialized_state(&serialized, window, cx);
                }
            }
        }
    }

    fn replay_pending_serialized_state(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some(serialized) = self.restoration.pending().cloned() else {
            return;
        };
        let waiting_for_active_panel = serialized
            .active_panel
            .as_deref()
            .filter(|_| serialized.visible)
            .is_some_and(|name| self.panel_index_for_persistent_name(name, cx).is_none());
        if waiting_for_active_panel {
            self.set_open_internal(serialized.visible, window, cx);
        } else {
            self.apply_serialized_state(&serialized, window, cx);
            self.restoration.discard_pending();
        }
    }

    fn apply_serialized_state(
        &mut self,
        serialized: &DockData,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if let Some(active_panel) = serialized
            .active_panel
            .as_deref()
            .filter(|_| serialized.visible)
            && let Some(idx) = self.panel_index_for_persistent_name(active_panel, cx)
        {
            self.activate_panel_internal(idx, window, cx);
        }
        if serialized.zoom
            && let Some(panel) = self.active_panel()
        {
            panel.set_zoomed(true, window, cx)
        }
        self.set_open_internal(serialized.visible, window, cx);
    }

    pub(crate) fn finish_restoration(&mut self) {
        self.restoration = DockRestoreState::Finished;
    }

    pub fn remove_panel<T: Panel>(
        &mut self,
        panel: &Entity<T>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> bool {
        if let Some(panel_ix) = self
            .panel_entries
            .iter()
            .position(|entry| entry.panel.panel_id() == Entity::entity_id(panel))
        {
            if let Some(active_panel_index) = self.active_panel_index.as_mut() {
                match panel_ix.cmp(active_panel_index) {
                    std::cmp::Ordering::Less => {
                        *active_panel_index -= 1;
                    }
                    std::cmp::Ordering::Equal => {
                        self.active_panel_index = None;
                        self.set_open(false, window, cx);
                    }
                    std::cmp::Ordering::Greater => {}
                }
            }

            self.panel_entries.remove(panel_ix);
            cx.notify();

            true
        } else {
            false
        }
    }

    /// Called after the panel shown in this dock moved to another dock, which
    /// closed this one: if the dock still has a panel with a button, it opens
    /// again on the one that followed the moved panel, or else on the one
    /// before it. A dock closes only when nothing is left to show.
    fn show_neighbor_of_moved_panel(
        &mut self,
        previous_index: usize,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        // Something else was opened here in the meantime (a panel arrived).
        if self.is_open || self.active_panel_index.is_some() {
            return;
        }
        let split = previous_index.min(self.panel_entries.len());
        let (before, after) = self.panel_entries.split_at(split);
        let neighbor = after
            .iter()
            .chain(before.iter().rev())
            .find(|entry| entry.panel.enabled(cx) && entry.panel.icon(window, cx).is_some())
            .map(|entry| entry.panel.panel_id());
        let Some(index) = neighbor.and_then(|panel_id| self.panel_index_for_id(panel_id)) else {
            return;
        };
        self.set_open(true, window, cx);
        self.activate_panel(index, window, cx);
        self.workspace
            .update(cx, |workspace, cx| {
                workspace.serialize_workspace(window, cx);
            })
            .ok();
    }

    pub fn panels_len(&self) -> usize {
        self.panel_entries.len()
    }

    pub fn has_agent_panel(&self, cx: &App) -> bool {
        self.panel_entries
            .iter()
            .any(|entry| entry.panel.is_agent_panel(cx))
    }

    pub fn activate_panel(&mut self, panel_ix: usize, window: &mut Window, cx: &mut Context<Self>) {
        if Some(panel_ix) != self.active_panel_index {
            self.restoration.discard_pending();
        }
        self.activate_panel_internal(panel_ix, window, cx);
    }

    fn activate_panel_internal(
        &mut self,
        panel_ix: usize,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if Some(panel_ix) != self.active_panel_index {
            if let Some(active_panel) = self.active_panel_entry() {
                active_panel.panel.set_active(false, window, cx);
            }

            self.active_panel_index = Some(panel_ix);
            if let Some(active_panel) = self.active_panel_entry() {
                active_panel.panel.set_active(true, window, cx);
            }

            cx.notify();
        }
    }

    pub fn visible_panel(&self) -> Option<&Arc<dyn PanelHandle>> {
        let entry = self.visible_entry()?;
        Some(&entry.panel)
    }

    pub fn active_panel(&self) -> Option<&Arc<dyn PanelHandle>> {
        let panel_entry = self.active_panel_entry()?;
        Some(&panel_entry.panel)
    }

    fn visible_entry(&self) -> Option<&PanelEntry> {
        if self.is_open {
            self.active_panel_entry()
        } else {
            None
        }
    }

    pub fn zoomed_panel(&self, window: &Window, cx: &App) -> Option<Arc<dyn PanelHandle>> {
        let entry = self.visible_entry()?;
        if entry.panel.is_zoomed(window, cx) {
            Some(entry.panel.clone())
        } else {
            None
        }
    }

    /// Panel buttons focus this before dispatching their action. A zoomed panel isn't rendered
    /// inside its dock, so the dock's own handle would be outside the frame and the action dropped.
    pub(crate) fn panel_button_focus_handle(&self, window: &Window, cx: &App) -> FocusHandle {
        self.zoomed_panel(window, cx)
            .map(|panel| panel.panel_focus_handle(cx))
            .unwrap_or_else(|| self.focus_handle.clone())
    }

    pub fn active_panel_size(&self) -> Option<PanelSizeState> {
        if self.is_open {
            self.active_panel_entry().map(|entry| entry.size_state)
        } else {
            None
        }
    }

    pub fn stored_panel_size(
        &self,
        panel: &dyn PanelHandle,
        window: &Window,
        cx: &App,
    ) -> Option<Pixels> {
        self.panel_entries
            .iter()
            .find(|entry| entry.panel.panel_id() == panel.panel_id())
            .map(|entry| {
                entry
                    .size_state
                    .size
                    .unwrap_or_else(|| entry.panel.default_size(window, cx))
            })
    }

    pub fn stored_panel_size_state(&self, panel: &dyn PanelHandle) -> Option<PanelSizeState> {
        self.panel_entries
            .iter()
            .find(|entry| entry.panel.panel_id() == panel.panel_id())
            .map(|entry| entry.size_state)
    }

    pub fn stored_active_panel_size(&self, window: &Window, cx: &App) -> Option<Pixels> {
        if self.is_open {
            self.active_panel_entry().map(|entry| {
                entry
                    .size_state
                    .size
                    .unwrap_or_else(|| entry.panel.default_size(window, cx))
            })
        } else {
            None
        }
    }

    pub fn set_panel_size_state(
        &mut self,
        panel: &dyn PanelHandle,
        size_state: PanelSizeState,
        cx: &mut Context<Self>,
    ) -> bool {
        if let Some(entry) = self
            .panel_entries
            .iter_mut()
            .find(|entry| entry.panel.panel_id() == panel.panel_id())
        {
            entry.size_state = size_state;
            cx.notify();
            true
        } else {
            false
        }
    }

    pub fn toggle_panel_flexible_size(
        &mut self,
        panel: &dyn PanelHandle,
        current_size: Option<Pixels>,
        current_flex: Option<f32>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let Some(entry) = self
            .panel_entries
            .iter_mut()
            .find(|entry| entry.panel.panel_id() == panel.panel_id())
        else {
            return;
        };
        let currently_flexible = entry.panel.has_flexible_size(window, cx);
        if currently_flexible {
            entry.size_state.size = current_size;
        } else {
            entry.size_state.flex = current_flex;
        }
        let panel_key = entry.panel.panel_key();
        let size_state = entry.size_state;
        let workspace = self.workspace.clone();
        entry
            .panel
            .set_flexible_size(!currently_flexible, window, cx);
        entry.panel.size_state_changed(window, cx);
        cx.defer(move |cx| {
            if let Some(workspace) = workspace.upgrade() {
                workspace.update(cx, |workspace, cx| {
                    workspace.persist_panel_size_state(panel_key, size_state, cx);
                });
            }
        });
        cx.notify();
    }

    fn resize_active_panel(
        &mut self,
        size: Option<Pixels>,
        flex: Option<f32>,
        persist: bool,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let position = self.position;
        if let Some(entry) = self.active_panel_entry_mut() {
            let resized = resize_panel_entry(position, entry, size, flex, window, cx);
            self.panel_sizes_changed(vec![resized], persist, cx);
        }
    }

    /// Stores the new sizes right away, or remembers which panels to store
    /// when the sash drag that is resizing them ends.
    fn panel_sizes_changed(
        &mut self,
        resized: Vec<(&'static str, PanelSizeState)>,
        persist: bool,
        cx: &mut Context<Self>,
    ) {
        if persist {
            let workspace = self.workspace.clone();
            cx.defer(move |cx| {
                if let Some(workspace) = workspace.upgrade() {
                    workspace.update(cx, |workspace, cx| {
                        for (panel_key, size_state) in resized {
                            workspace.persist_panel_size_state(panel_key, size_state, cx);
                        }
                    });
                }
            });
        } else {
            for (panel_key, _) in resized {
                if !self.unpersisted_panel_keys.contains(&panel_key) {
                    self.unpersisted_panel_keys.push(panel_key);
                }
            }
        }
        cx.notify();
    }

    /// Resizes the active panel and, when this dock is included in
    /// `resize_all_panels_in_dock`, all panels using the same sizing mode.
    pub fn resize_panel_sizes(
        &mut self,
        size: Option<Pixels>,
        flex: Option<f32>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.resize_panel_sizes_internal(size, flex, true, window, cx);
    }

    /// As `resize_panel_sizes`, for each mouse move of a sash drag: nothing is
    /// written to the database until `persist_resized_panels` is called.
    pub(crate) fn resize_panel_sizes_while_dragging(
        &mut self,
        size: Option<Pixels>,
        flex: Option<f32>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.resize_panel_sizes_internal(size, flex, false, window, cx);
    }

    fn resize_panel_sizes_internal(
        &mut self,
        size: Option<Pixels>,
        flex: Option<f32>,
        persist: bool,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if self.should_resize_all_panels(cx) {
            self.resize_all_panels(size, flex, persist, window, cx);
        } else {
            self.resize_active_panel(size, flex, persist, window, cx);
        }
    }

    /// Stores the sizes left by `resize_panel_sizes_while_dragging`. Called by
    /// the workspace once, when the sash drag ends.
    pub(crate) fn persist_resized_panels(&mut self, cx: &mut Context<Self>) {
        let panel_keys = std::mem::take(&mut self.unpersisted_panel_keys);
        let resized = self
            .panel_entries
            .iter()
            .filter(|entry| panel_keys.contains(&entry.panel.panel_key()))
            .map(|entry| (entry.panel.panel_key(), entry.size_state))
            .collect::<Vec<_>>();
        if !resized.is_empty() {
            self.panel_sizes_changed(resized, true, cx);
        }
    }

    /// Whether a sash drag left sizes that are not stored yet.
    #[cfg(test)]
    pub(crate) fn has_unpersisted_panel_sizes(&self) -> bool {
        !self.unpersisted_panel_keys.is_empty()
    }

    /// Resets the active panel and, when this dock is included in
    /// `resize_all_panels_in_dock`, all panels using the same sizing mode.
    pub fn reset_panel_sizes(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.should_resize_all_panels(cx) {
            self.reset_all_panel_sizes(window, cx);
        } else {
            self.reset_active_panel_size(window, cx);
        }
    }

    fn reset_active_panel_size(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let state = PanelSizeState::default();
        self.resize_active_panel(state.size, state.flex, true, window, cx);
    }

    fn reset_all_panel_sizes(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some(active_entry) = self.active_panel_entry() else {
            return;
        };
        let size =
            (!panel_uses_flexible_width(self.position, active_entry.panel.as_ref(), window, cx))
                .then(|| active_entry.panel.default_size(window, cx));
        self.resize_all_panels(size, None, true, window, cx);
    }

    fn should_resize_all_panels(&self, cx: &App) -> bool {
        WorkspaceSettings::get_global(cx)
            .resize_all_panels_in_dock
            .contains(&self.position)
    }

    fn resize_all_panels(
        &mut self,
        size: Option<Pixels>,
        flex: Option<f32>,
        persist: bool,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let Some(active_panel_index) = self.active_panel_index else {
            return;
        };

        let active_panel_uses_flexible_width = {
            let Some(active_entry) = self.panel_entries.get(active_panel_index) else {
                return;
            };
            panel_uses_flexible_width(self.position, active_entry.panel.as_ref(), window, cx)
        };
        let mut resized = Vec::new();
        for entry in &mut self.panel_entries {
            if panel_uses_flexible_width(self.position, entry.panel.as_ref(), window, cx)
                == active_panel_uses_flexible_width
            {
                resized.push(resize_panel_entry(
                    self.position,
                    entry,
                    size,
                    flex,
                    window,
                    cx,
                ));
            }
        }
        self.panel_sizes_changed(resized, persist, cx);
    }

    pub fn toggle_action(&self) -> Box<dyn Action> {
        match self.position {
            DockPosition::Left => crate::ToggleLeftDock.boxed_clone(),
            DockPosition::Bottom => crate::ToggleBottomDock.boxed_clone(),
            DockPosition::Right => crate::ToggleRightDock.boxed_clone(),
        }
    }

    fn dispatch_context(cx: &App) -> KeyContext {
        let mut dispatch_context = KeyContext::new_with_defaults(cx.platform_info().os);
        dispatch_context.add("Dock");

        dispatch_context
    }

    /// Brings the fixed sizes of this dock's panels under `max_size` (the room
    /// the workspace has for the dock, sash included), never under a panel's
    /// own minimum. Called when the workspace changes size.
    pub fn clamp_panel_size(&mut self, max_size: Pixels, window: &Window, cx: &mut Context<Self>) {
        let max_size = (max_size - RESIZE_HANDLE_SIZE).abs();
        let mut clamped = false;
        for entry in &mut self.panel_entries {
            let uses_flexible_width =
                panel_uses_flexible_width(self.position, entry.panel.as_ref(), window, cx);
            if uses_flexible_width {
                continue;
            }

            let size = entry
                .size_state
                .size
                .unwrap_or_else(|| entry.panel.default_size(window, cx));
            let min_size = entry
                .panel
                .min_size(window, cx)
                .unwrap_or(RESIZE_HANDLE_SIZE)
                .max(RESIZE_HANDLE_SIZE);
            let max_size = max_size.max(min_size);
            if size > max_size {
                entry.size_state.size = Some(max_size);
                clamped = true;
            }
        }
        if clamped {
            cx.notify();
        }
    }

    pub(crate) fn load_persisted_size_state(
        workspace: &Workspace,
        panel_key: &'static str,
        cx: &App,
    ) -> Option<PanelSizeState> {
        let workspace_id = workspace
            .database_id()
            .map(|id| i64::from(id).to_string())
            .or(workspace.session_id())?;
        let kvp = KeyValueStore::global(cx);
        let scope = kvp.scoped(PANEL_SIZE_STATE_KEY);
        scope
            .read(&format!("{workspace_id}:{panel_key}"))
            .log_err()
            .flatten()
            .and_then(|json| serde_json::from_str::<PanelSizeState>(&json).log_err())
    }
}

/// Thickness of the line drawn over a dock's edge while its sash is hovered or dragged.
const SASH_LINE_SIZE: Pixels = px(2.);

/// The menu opened by a right click on a panel's button or tab: where to dock
/// it, how it is sized, and whether its button is shown.
pub(crate) fn panel_context_menu(
    panel: Arc<dyn PanelHandle>,
    dock: Entity<Dock>,
    workspace: WeakEntity<Workspace>,
    dock_position: DockPosition,
    window: &mut Window,
    cx: &mut App,
) -> Entity<ContextMenu> {
    const POSITIONS: [DockPosition; 3] = [
        DockPosition::Left,
        DockPosition::Right,
        DockPosition::Bottom,
    ];

    let supports_flexible = panel.supports_flexible_size(cx);
    let currently_flexible = panel.has_flexible_size(window, cx);
    let panel_hide = panel.hide_button_setting(cx);
    ContextMenu::build(window, cx, |mut menu, _, cx| {
        let mut has_position_entries = false;
        for position in POSITIONS {
            if panel.position_is_valid(position, cx) {
                let is_current = position == dock_position;
                let panel = panel.clone();
                let workspace = workspace.clone();
                menu = menu.toggleable_entry(
                    format!("Dock {}", position.label()),
                    is_current,
                    IconPosition::Start,
                    None,
                    move |window, cx| {
                        if is_current {
                            return;
                        }
                        // Through the workspace, so the panel arrives open and
                        // focused and a failed move is reported.
                        let moved = workspace.update(cx, |workspace, cx| {
                            workspace.move_panel_to_dock(&panel, position, window, cx);
                        });
                        if moved.is_err() {
                            panel.set_position(position, window, cx);
                        }
                    },
                );
                has_position_entries = true;
            }
        }
        if supports_flexible {
            if has_position_entries {
                menu = menu.separator();
            }
            let panel_for_flex = panel.clone();
            let dock_for_flex = dock.clone();
            let workspace_for_flex = workspace.clone();
            menu = menu.toggleable_entry(
                "Flex Width",
                currently_flexible,
                IconPosition::Start,
                None,
                move |window, cx| {
                    if !currently_flexible {
                        if let Some(ws) = workspace_for_flex.upgrade() {
                            ws.update(cx, |workspace, cx| {
                                workspace.toggle_dock_panel_flexible_size(
                                    &dock_for_flex,
                                    panel_for_flex.as_ref(),
                                    window,
                                    cx,
                                );
                            });
                        }
                    }
                },
            );
            let panel_for_fixed = panel.clone();
            let dock_for_fixed = dock.clone();
            let workspace_for_fixed = workspace.clone();
            menu = menu.toggleable_entry(
                "Fixed Width",
                !currently_flexible,
                IconPosition::Start,
                None,
                move |window, cx| {
                    if currently_flexible {
                        if let Some(ws) = workspace_for_fixed.upgrade() {
                            ws.update(cx, |workspace, cx| {
                                workspace.toggle_dock_panel_flexible_size(
                                    &dock_for_fixed,
                                    panel_for_fixed.as_ref(),
                                    window,
                                    cx,
                                );
                            });
                        }
                    }
                },
            );
        }
        if let Some(hide) = panel_hide {
            menu = crate::status_bar::add_hide_button_entry(menu.separator(), hide);
        }
        menu
    })
}

impl Dock {
    /// The header of the bottom dock, the only row above its panel: one text
    /// tab per panel, then the shown panel's own buttons, Maximize and Hide.
    /// `None` for the side docks, whose panels draw their own header.
    ///
    /// The workspace draws the same header above a maximized bottom panel,
    /// which is rendered outside of its dock.
    pub fn render_header(&self, window: &mut Window, cx: &mut Context<Self>) -> Option<AnyElement> {
        if self.position != DockPosition::Bottom {
            return None;
        }
        let active_panel = self.active_panel()?.clone();

        let active_index = self.active_panel_index;
        let position = self.position;
        let dock_entity = cx.entity();
        let workspace = self.workspace.clone();
        let dock_focus_handle = self.panel_button_focus_handle(window, cx);
        let is_zoomed = active_panel.is_zoomed(window, cx);
        // A maximized panel can always be restored, whatever it declares.
        let can_zoom = is_zoomed || active_panel.supports_zoom(window, cx);

        let mut strip = TextTabStrip::new("panel-tabs");
        for (ix, entry) in self.panel_entries.iter().enumerate() {
            let is_active = Some(ix) == active_index;
            // A panel without an icon is disabled or has its button hidden
            // in the settings: it only gets a tab while it is the one shown.
            if !is_active && entry.panel.icon(window, cx).is_none() {
                continue;
            }
            let panel = entry.panel.clone();
            let title = panel.title(window, cx);
            let count = panel
                .icon_label(window, cx)
                .and_then(|count| count.parse::<usize>().ok());
            let action = panel.toggle_action(window, cx);
            let activation_focus_handle = panel.activation_focus_handle(cx);
            let dock_focus_handle = dock_focus_handle.clone();
            let dragged_panel = panel.clone();
            let drag_workspace = workspace.clone();

            // The menu wraps the tab in an element of its own, which takes the
            // tab's natural height: stretched to the strip here, so the tab
            // fills it and its marker sits on the header's line.
            strip = strip.child(
                div().h_full().flex().child(
                    right_click_menu(("panel-tab-menu", ix))
                        .menu({
                            let dock_entity = dock_entity.clone();
                            let workspace = workspace.clone();
                            move |window, cx| {
                                panel_context_menu(
                                    panel.clone(),
                                    dock_entity.clone(),
                                    workspace.clone(),
                                    position,
                                    window,
                                    cx,
                                )
                            }
                        })
                        .anchor(Anchor::BottomLeft)
                        .attach(Anchor::TopLeft)
                        .trigger(move |_, _window, _cx| {
                            let tooltip_title = title.clone();
                            let tooltip_action = action.boxed_clone();
                            let tab = TextTab::new(("panel-tab", ix), title)
                                .selected(is_active)
                                .count(count)
                                .debug_selector(move || format!("PANEL-TAB-{ix}"))
                                .tooltip(move |_window, cx| {
                                    Tooltip::for_action(tooltip_title.clone(), &*tooltip_action, cx)
                                })
                                .on_click(move |_, window, cx| {
                                    if is_active {
                                        // The panel's action toggles: on the shown
                                        // panel it would close the dock.
                                        window.focus(&activation_focus_handle, cx);
                                    } else {
                                        window.focus(&dock_focus_handle, cx);
                                        window.dispatch_action(action.boxed_clone(), cx);
                                    }
                                });
                            // Press and drag moves the panel to another dock;
                            // a plain click still switches to it.
                            panel_drag_source(tab, dragged_panel, position, drag_workspace)
                        }),
                ),
            );
        }

        if let Some(actions) = active_panel.header_actions(window, cx) {
            strip = strip.end_slot(actions);
        }

        if can_zoom {
            let label = if is_zoomed {
                "Restore Panel Size"
            } else {
                "Maximize Panel"
            };
            strip = strip.end_slot(
                IconButton::new(
                    ("panel-zoom", is_zoomed as u64),
                    if is_zoomed {
                        IconName::Minimize
                    } else {
                        IconName::Maximize
                    },
                )
                .icon_size(IconSize::Small)
                .icon_color(Color::Muted)
                .hover_color(Color::Default)
                .aria_label(label)
                .tooltip(move |_window, cx| Tooltip::for_action(label, &crate::ToggleZoom, cx))
                .on_click(cx.listener(|dock, _, window, cx| {
                    // Zoom is each panel's own business: ask the shown
                    // one, as the key binding would.
                    if let Some(panel) = dock.active_panel() {
                        window.focus(&panel.activation_focus_handle(cx), cx);
                        window.dispatch_action(crate::ToggleZoom.boxed_clone(), cx);
                    }
                })),
            );
        }

        let toggle_dock_action = self.toggle_action();
        strip = strip.end_slot(
            IconButton::new("panel-hide", IconName::Close)
                .icon_size(IconSize::Small)
                .icon_color(Color::Muted)
                .hover_color(Color::Default)
                .aria_label("Hide Panel")
                .tooltip({
                    let action = toggle_dock_action.boxed_clone();
                    move |_window, cx| Tooltip::for_action("Hide Panel", &*action, cx)
                })
                .on_click(move |_, window, cx| {
                    window.focus(&dock_focus_handle, cx);
                    window.dispatch_action(toggle_dock_action.boxed_clone(), cx);
                }),
        );

        Some(
            // The strip has no ground of its own: the same one as the headers
            // of the side panels, also over a maximized panel.
            div()
                .flex_none()
                .w_full()
                .bg(cx.theme().colors().panel_background)
                .child(strip)
                .into_any_element(),
        )
    }
}

impl Render for Dock {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let dispatch_context = Self::dispatch_context(cx);
        if !cx.has_active_drag() {
            self.resizing = false;
            self.panel_drag_active = false;
        }
        if self.visible_entry().is_some() {
            let position = self.position;
            let resizing = self.resizing;
            let accent_color = cx.theme().colors().text_accent;
            let header = self.render_header(window, cx);
            let Some(entry) = self.visible_entry() else {
                unreachable!("checked above");
            };
            let create_resize_handle = || {
                let this = cx.weak_entity();
                // The line that lights up over the dock's edge, as a sash does.
                let sash_line = div()
                    .absolute()
                    .when(resizing, |this| this.bg(accent_color))
                    .group_hover("dock-sash", |style| style.bg(accent_color));
                let sash_inset = (RESIZE_HANDLE_SIZE - SASH_LINE_SIZE) / 2.;
                let handle = div()
                    .id("resize-handle")
                    .debug_selector(|| format!("DOCK-SASH-{}", position.label().to_uppercase()))
                    .group("dock-sash")
                    .on_drag(DraggedDock(position), move |dock, _, _, cx| {
                        cx.stop_propagation();
                        this.update(cx, |this, cx| {
                            this.resizing = true;
                            cx.notify();
                        })
                        .ok();
                        cx.new(|_| dock.clone())
                    })
                    .on_mouse_down(
                        MouseButton::Left,
                        cx.listener(|_, _: &MouseDownEvent, _, cx| {
                            cx.stop_propagation();
                        }),
                    )
                    .on_mouse_up(
                        MouseButton::Left,
                        cx.listener(|dock, e: &MouseUpEvent, window, cx| {
                            if e.click_count == 2 {
                                dock.reset_panel_sizes(window, cx);
                                dock.workspace
                                    .update(cx, |workspace, cx| {
                                        workspace.serialize_workspace(window, cx);
                                    })
                                    .ok();
                                cx.stop_propagation();
                            }
                        }),
                    )
                    .occlude();
                match position {
                    DockPosition::Left => deferred(
                        handle
                            .absolute()
                            .right(-RESIZE_HANDLE_SIZE / 2.)
                            .top(px(0.))
                            .h_full()
                            .w(RESIZE_HANDLE_SIZE)
                            .cursor_col_resize()
                            .child(
                                sash_line
                                    .top_0()
                                    .bottom_0()
                                    .left(sash_inset)
                                    .w(SASH_LINE_SIZE),
                            ),
                    ),
                    DockPosition::Bottom => deferred(
                        handle
                            .absolute()
                            .top(-RESIZE_HANDLE_SIZE / 2.)
                            .left(px(0.))
                            .w_full()
                            .h(RESIZE_HANDLE_SIZE)
                            .cursor_row_resize()
                            .child(
                                sash_line
                                    .left_0()
                                    .right_0()
                                    .top(sash_inset)
                                    .h(SASH_LINE_SIZE),
                            ),
                    ),
                    DockPosition::Right => deferred(
                        handle
                            .absolute()
                            .top(px(0.))
                            .left(-RESIZE_HANDLE_SIZE / 2.)
                            .h_full()
                            .w(RESIZE_HANDLE_SIZE)
                            .cursor_col_resize()
                            .child(
                                sash_line
                                    .top_0()
                                    .bottom_0()
                                    .left(sash_inset)
                                    .w(SASH_LINE_SIZE),
                            ),
                    ),
                }
            };

            div()
                .id("dock-panel")
                .key_context(dispatch_context)
                .track_focus(&self.focus_handle(cx))
                .focus_follows_mouse(WorkspaceSettings::get_global(cx).focus_follows_mouse, cx)
                .flex()
                .bg(cx.theme().colors().panel_background)
                .border_color(cx.theme().colors().border)
                .overflow_hidden()
                .map(|this| match self.position().axis() {
                    // Width and height are always set on the workspace wrapper in
                    // render_dock, so fill whatever space the wrapper provides.
                    Axis::Horizontal => this.w_full().h_full().flex_row(),
                    Axis::Vertical => this.h_full().w_full().flex_col(),
                })
                .map(|this| match self.position() {
                    DockPosition::Left => this.border_r_1(),
                    DockPosition::Right => this.border_l_1(),
                    DockPosition::Bottom => this.border_t_1(),
                })
                .children(header)
                .child(
                    div()
                        .map(|this| match self.position().axis() {
                            Axis::Horizontal => this.w_full().h_full(),
                            // Below the tab strip, the panel takes what is left.
                            Axis::Vertical => this.w_full().flex_1().min_h_0(),
                        })
                        .child(
                            entry
                                .panel
                                .to_any()
                                .cached(StyleRefinement::default().v_flex().size_full()),
                        ),
                )
                .when(self.resizable(cx) && !self.panel_drag_active, |this| {
                    this.child(create_resize_handle())
                })
        } else {
            div()
                .id("dock-panel")
                .key_context(dispatch_context)
                .track_focus(&self.focus_handle(cx))
        }
    }
}

#[cfg(any(test, feature = "test-support"))]
pub mod test {
    use super::*;
    use gpui::{App, Context, Window, actions, div};

    pub struct TestPanel {
        pub position: DockPosition,
        pub zoomed: bool,
        pub active: bool,
        /// Keeps track of the values provided to [`Self::set_active`], to allow
        /// inspecting whether the panel was activated or deactivated.
        pub activation_history: Vec<bool>,
        pub focus_handle: FocusHandle,
        pub activation_focus_handle: Option<FocusHandle>,
        pub default_size: Pixels,
        /// The width or height the panel cannot go under, if it has one.
        pub min_size: Option<Pixels>,
        pub flexible: bool,
        pub activation_priority: u32,
        /// The docks this panel accepts; every dock when `None`.
        pub valid_positions: Option<Vec<DockPosition>>,
        /// When set, `set_position` leaves the panel where it is, as a real
        /// panel does when its setting cannot be written or is overridden.
        pub ignores_set_position: bool,
        /// The button icon. Without one the panel has no button and only gets
        /// a tab while it is the one shown.
        pub icon: Option<ui::IconName>,
    }
    actions!(test_only, [ToggleTestPanel]);

    impl EventEmitter<PanelEvent> for TestPanel {}

    impl TestPanel {
        pub fn new(position: DockPosition, activation_priority: u32, cx: &mut App) -> Self {
            Self {
                position,
                zoomed: false,
                active: false,
                activation_history: Vec::new(),
                focus_handle: cx.focus_handle(),
                activation_focus_handle: None,
                default_size: px(300.),
                min_size: None,
                flexible: false,
                activation_priority,
                valid_positions: None,
                ignores_set_position: false,
                icon: None,
            }
        }

        pub fn new_flexible(
            position: DockPosition,
            activation_priority: u32,
            cx: &mut App,
        ) -> Self {
            Self {
                flexible: true,
                ..Self::new(position, activation_priority, cx)
            }
        }

        pub fn new_with_activation_child(
            position: DockPosition,
            activation_priority: u32,
            cx: &mut App,
        ) -> Self {
            Self {
                activation_focus_handle: Some(cx.focus_handle()),
                ..Self::new(position, activation_priority, cx)
            }
        }
    }

    impl Render for TestPanel {
        fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
            div()
                .id("test")
                .track_focus(&self.focus_handle(cx))
                .children(self.activation_focus_handle.iter().map(|focus_handle| {
                    div().id("test-activation-child").track_focus(focus_handle)
                }))
        }
    }

    impl Panel for TestPanel {
        fn activation_focus_handle(&self, cx: &App) -> FocusHandle {
            self.activation_focus_handle
                .clone()
                .unwrap_or_else(|| self.focus_handle(cx))
        }

        fn persistent_name() -> &'static str {
            "TestPanel"
        }

        fn panel_key() -> &'static str {
            "TestPanel"
        }

        fn position(&self, _window: &Window, _: &App) -> super::DockPosition {
            self.position
        }

        fn position_is_valid(&self, position: super::DockPosition) -> bool {
            self.valid_positions
                .as_ref()
                .is_none_or(|valid| valid.contains(&position))
        }

        fn set_position(&mut self, position: DockPosition, _: &mut Window, cx: &mut Context<Self>) {
            if self.ignores_set_position {
                return;
            }
            self.position = position;
            cx.update_global::<SettingsStore, _>(|_, _| {});
        }

        fn default_size(&self, _window: &Window, _: &App) -> Pixels {
            self.default_size
        }

        fn min_size(&self, _window: &Window, _: &App) -> Option<Pixels> {
            self.min_size
        }

        fn initial_size_state(&self, _window: &Window, _: &App) -> PanelSizeState {
            PanelSizeState {
                size: None,
                flex: None,
            }
        }

        fn supports_flexible_size(&self) -> bool {
            self.flexible
        }

        fn has_flexible_size(&self, _window: &Window, _: &App) -> bool {
            self.flexible
        }

        fn set_flexible_size(
            &mut self,
            flexible: bool,
            _window: &mut Window,
            _cx: &mut Context<Self>,
        ) {
            self.flexible = flexible;
        }

        fn icon(&self, _window: &Window, _: &App) -> Option<ui::IconName> {
            self.icon
        }

        fn icon_tooltip(&self, _window: &Window, _cx: &App) -> Option<&'static str> {
            self.icon.map(|_| "Test Panel")
        }

        fn toggle_action(&self) -> Box<dyn Action> {
            ToggleTestPanel.boxed_clone()
        }

        fn is_zoomed(&self, _window: &Window, _: &App) -> bool {
            self.zoomed
        }

        fn set_zoomed(&mut self, zoomed: bool, _window: &mut Window, _cx: &mut Context<Self>) {
            self.zoomed = zoomed;
        }

        fn supports_zoom(&self, _window: &Window, _: &App) -> bool {
            true
        }

        fn set_active(&mut self, active: bool, _window: &mut Window, _cx: &mut Context<Self>) {
            self.active = active;
            self.activation_history.push(active);
        }

        fn activation_priority(&self) -> u32 {
            self.activation_priority
        }
    }

    impl Focusable for TestPanel {
        fn focus_handle(&self, _cx: &App) -> FocusHandle {
            self.focus_handle.clone()
        }
    }
}

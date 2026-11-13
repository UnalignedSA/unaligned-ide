//! The agent panel's History: past chats of this project, newest first, with
//! a search field on top. Click or Enter reopens a chat. The bin on hover, or
//! Backspace / Delete on the selected row while the search field is empty,
//! asks once in the row before the chat is deleted for good.

use std::sync::Arc;

use editor::actions::{Backspace, Delete};
use gpui::{AnyElement, App, Context, DismissEvent, SharedString, Task, WeakEntity, Window};
use picker::{ErasedEditor, Picker, PickerDelegate};
use ui::{
    Divider, DividerColor, EmptyState, HighlightedLabel, ListItem, ListItemSpacing, Tooltip,
    prelude::*,
};

use crate::{
    AgentPanel,
    thread_metadata_store::{ThreadId, ThreadMetadata},
    threads_archive_view::{format_history_entry_timestamp, fuzzy_match_positions},
};

pub(crate) type ChatHistory = Picker<ChatHistoryDelegate>;

pub(crate) fn chat_history(
    panel: WeakEntity<AgentPanel>,
    window: &mut Window,
    cx: &mut Context<ChatHistory>,
) -> ChatHistory {
    let delegate = ChatHistoryDelegate::new(panel, cx);
    Picker::uniform_list(delegate, window, cx)
        .show_scrollbar(true)
        .initial_width(rems(22.))
        .max_height(rems(24.))
}

struct HistoryMatch {
    chat: ThreadMetadata,
    title: SharedString,
    highlight_positions: Vec<usize>,
}

pub(crate) struct ChatHistoryDelegate {
    panel: WeakEntity<AgentPanel>,
    chats: Vec<ThreadMetadata>,
    matches: Vec<HistoryMatch>,
    query: String,
    selected_index: usize,
    active_chat: Option<ThreadId>,
    /// The chat whose row asks "Delete this chat?". Deleting is permanent, so
    /// it takes a second step: Enter or the row's Delete button.
    pending_delete: Option<ThreadId>,
}

impl ChatHistoryDelegate {
    fn new(panel: WeakEntity<AgentPanel>, cx: &mut App) -> Self {
        let (chats, active_chat) = panel
            .read_with(cx, |panel, cx| {
                (panel.history_chats(cx), panel.active_thread_id(cx))
            })
            .unwrap_or_default();
        Self::with_chats(panel, chats, active_chat)
    }

    fn with_chats(
        panel: WeakEntity<AgentPanel>,
        chats: Vec<ThreadMetadata>,
        active_chat: Option<ThreadId>,
    ) -> Self {
        let mut this = Self {
            panel,
            chats,
            matches: Vec::new(),
            query: String::new(),
            selected_index: 0,
            active_chat,
            pending_delete: None,
        };
        this.refresh_matches();
        this
    }

    fn refresh_matches(&mut self) {
        let query = self.query.trim();
        self.matches = self
            .chats
            .iter()
            .filter_map(|chat| {
                let title = chat.display_title();
                let highlight_positions = fuzzy_match_positions(query, &title)?;
                Some(HistoryMatch {
                    chat: chat.clone(),
                    title,
                    highlight_positions,
                })
            })
            .collect();
        self.selected_index = self
            .selected_index
            .min(self.matches.len().saturating_sub(1));
    }

    fn selected_chat(&self) -> Option<ThreadId> {
        self.matches
            .get(self.selected_index)
            .map(|history_match| history_match.chat.thread_id)
    }

    /// Makes the row of this chat ask for confirmation, and selects it so
    /// that Enter confirms.
    fn ask_to_delete(&mut self, thread_id: ThreadId) {
        if let Some(ix) = self
            .matches
            .iter()
            .position(|history_match| history_match.chat.thread_id == thread_id)
        {
            self.selected_index = ix;
            self.pending_delete = Some(thread_id);
        }
    }

    /// Backspace or Delete in the search field. Returns whether the key was
    /// taken: only while the field is empty, where the key has nothing to
    /// erase, and a row is selected.
    fn ask_to_delete_selected(&mut self) -> bool {
        if !self.query.is_empty() {
            return false;
        }
        let Some(thread_id) = self.selected_chat() else {
            return false;
        };
        self.ask_to_delete(thread_id);
        true
    }

    /// Withdraws the question. Returns whether there was one.
    fn cancel_delete(&mut self) -> bool {
        self.pending_delete.take().is_some()
    }

    /// Forgets the chat in this list. Returns the chat to delete for good, if
    /// it is still listed.
    fn remove(&mut self, thread_id: ThreadId) -> Option<ThreadMetadata> {
        self.pending_delete = None;
        let ix = self
            .chats
            .iter()
            .position(|chat| chat.thread_id == thread_id)?;
        let chat = self.chats.remove(ix);
        self.refresh_matches();
        Some(chat)
    }

    fn delete(&mut self, thread_id: ThreadId, window: &mut Window, cx: &mut Context<ChatHistory>) {
        let Some(chat) = self.remove(thread_id) else {
            return;
        };
        self.panel
            .update(cx, |panel, cx| panel.delete_chat(&chat, window, cx))
            .ok();
        cx.notify();
    }

    fn render_delete_question(
        &self,
        ix: usize,
        thread_id: ThreadId,
        cx: &mut Context<ChatHistory>,
    ) -> impl IntoElement {
        h_flex()
            .w_full()
            .min_w_0()
            .gap_2()
            .justify_between()
            .child(
                Label::new("Delete this chat?")
                    .size(LabelSize::Small)
                    .truncate(),
            )
            .child(
                h_flex()
                    .flex_none()
                    .gap_1()
                    .child(
                        Button::new(("confirm-delete-chat", ix), "Delete")
                            .style(ButtonStyle::Outlined)
                            .size(ButtonSize::Compact)
                            .label_size(LabelSize::Small)
                            .color(Color::Error)
                            .on_click(cx.listener(move |picker, _, window, cx| {
                                cx.stop_propagation();
                                picker.delegate.delete(thread_id, window, cx);
                            })),
                    )
                    .child(
                        Button::new(("cancel-delete-chat", ix), "Cancel")
                            .style(ButtonStyle::Outlined)
                            .size(ButtonSize::Compact)
                            .label_size(LabelSize::Small)
                            .on_click(cx.listener(|picker, _, _, cx| {
                                cx.stop_propagation();
                                picker.delegate.cancel_delete();
                                cx.notify();
                            })),
                    ),
            )
    }
}

impl PickerDelegate for ChatHistoryDelegate {
    type ListItem = ListItem;

    fn name() -> &'static str {
        "chat history"
    }

    fn placeholder_text(&self, _: &mut Window, _: &mut App) -> Arc<str> {
        "Search chats…".into()
    }

    fn no_matches_text(&self, _window: &mut Window, _cx: &mut App) -> Option<SharedString> {
        // The empty state is drawn by `render_footer`.
        None
    }

    fn match_count(&self) -> usize {
        self.matches.len()
    }

    fn selected_index(&self) -> usize {
        self.selected_index
    }

    fn set_selected_index(&mut self, ix: usize, _: &mut Window, cx: &mut Context<Picker<Self>>) {
        let ix = ix.min(self.matches.len().saturating_sub(1));
        if ix != self.selected_index {
            // The question belongs to one row: moving away withdraws it.
            self.pending_delete = None;
        }
        self.selected_index = ix;
        cx.notify();
    }

    fn update_matches(
        &mut self,
        query: String,
        _window: &mut Window,
        cx: &mut Context<Picker<Self>>,
    ) -> Task<()> {
        if query != self.query {
            self.pending_delete = None;
            self.selected_index = 0;
        }
        self.query = query;
        self.refresh_matches();
        cx.notify();
        Task::ready(())
    }

    fn confirm(&mut self, _: bool, window: &mut Window, cx: &mut Context<Picker<Self>>) {
        let Some(history_match) = self.matches.get(self.selected_index) else {
            return;
        };
        let chat = history_match.chat.clone();
        if self.pending_delete == Some(chat.thread_id) {
            // Enter answers the question of the selected row.
            self.delete(chat.thread_id, window, cx);
            return;
        }
        let panel = self.panel.clone();
        // Deferred: reopening moves focus, which must not race the dismissal.
        window.defer(cx, move |window, cx| {
            panel
                .update(cx, |panel, cx| panel.open_chat(&chat, window, cx))
                .ok();
        });
        cx.emit(DismissEvent);
    }

    fn dismissed(&mut self, _window: &mut Window, cx: &mut Context<Picker<Self>>) {
        cx.emit(DismissEvent);
    }

    /// The picker's own search strip, plus the keys of the delete question:
    /// they are caught on their way to the search field, which would
    /// otherwise swallow them.
    fn render_editor(
        &self,
        editor: &Arc<dyn ErasedEditor>,
        window: &mut Window,
        cx: &mut Context<Picker<Self>>,
    ) -> Option<Div> {
        Some(
            v_flex()
                .capture_action(cx.listener(|picker, _: &Backspace, _, cx| {
                    if picker.delegate.ask_to_delete_selected() {
                        cx.stop_propagation();
                        cx.notify();
                    }
                }))
                .capture_action(cx.listener(|picker, _: &Delete, _, cx| {
                    if picker.delegate.ask_to_delete_selected() {
                        cx.stop_propagation();
                        cx.notify();
                    }
                }))
                .capture_action(cx.listener(|picker, _: &menu::Cancel, _, cx| {
                    // Escape withdraws the question before it closes History.
                    if picker.delegate.cancel_delete() {
                        cx.stop_propagation();
                        cx.notify();
                    }
                }))
                .child(
                    h_flex()
                        .h(rems_from_px(34_f32))
                        .px_2p5()
                        .gap_1()
                        .flex_none()
                        .overflow_hidden()
                        .bg(cx.theme().colors().background)
                        .child(div().flex_1().child(editor.render(window, cx))),
                )
                .child(Divider::horizontal().color(DividerColor::Border)),
        )
    }

    fn render_footer(
        &self,
        _window: &mut Window,
        _cx: &mut Context<Picker<Self>>,
    ) -> Option<AnyElement> {
        if !self.matches.is_empty() {
            return None;
        }
        let empty_state = if self.chats.is_empty() {
            EmptyState::new("No chats yet")
                .icon(IconName::Clock)
                .description("Chats of this project are kept here.")
        } else {
            EmptyState::new("No chats match your search")
        };
        Some(div().py_2().child(empty_state).into_any_element())
    }

    fn render_match(
        &self,
        ix: usize,
        selected: bool,
        _: &mut Window,
        cx: &mut Context<Picker<Self>>,
    ) -> Option<Self::ListItem> {
        let history_match = self.matches.get(ix)?;
        let thread_id = history_match.chat.thread_id;
        let is_active = self.active_chat == Some(thread_id);
        let time = format_history_entry_timestamp(history_match.chat.updated_at);

        let item = ListItem::new(("chat-history-entry", ix))
            .inset(true)
            .spacing(ListItemSpacing::Sparse)
            .toggle_state(selected);

        if self.pending_delete == Some(thread_id) {
            return Some(item.child(self.render_delete_question(ix, thread_id, cx)));
        }

        Some(
            item.child(
                h_flex()
                    .w_full()
                    .min_w_0()
                    .gap_2()
                    .justify_between()
                    .child(
                        HighlightedLabel::new(
                            history_match.title.clone(),
                            history_match.highlight_positions.clone(),
                        )
                        .size(LabelSize::Small)
                        .color(if is_active {
                            Color::Default
                        } else {
                            Color::Muted
                        })
                        .truncate(),
                    )
                    .child(
                        Label::new(time)
                            .size(LabelSize::XSmall)
                            .color(Color::Placeholder)
                            .buffer_font(cx),
                    ),
            )
            .end_slot_on_hover(
                IconButton::new(("delete-chat", ix), IconName::Trash)
                    .icon_size(IconSize::Small)
                    .icon_color(Color::Muted)
                    .tooltip(Tooltip::text("Delete Chat…"))
                    .on_click(cx.listener(move |picker, _, _, cx| {
                        cx.stop_propagation();
                        picker.delegate.ask_to_delete(thread_id);
                        cx.notify();
                    })),
            ),
        )
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::{TimeZone as _, Utc};
    use project::WorktreePaths;
    use workspace::PathList;

    fn history(titles: &[&str]) -> ChatHistoryDelegate {
        let chats = titles
            .iter()
            .enumerate()
            .map(|(ix, title)| ThreadMetadata {
                thread_id: ThreadId::new(),
                session_id: None,
                agent_id: agent::IDE_AGENT_ID.clone(),
                title: Some(SharedString::from(title.to_string())),
                title_override: None,
                updated_at: Utc
                    .with_ymd_and_hms(2026, 11, 12, 10, ix as u32, 0)
                    .unwrap(),
                created_at: None,
                interacted_at: None,
                worktree_paths: WorktreePaths::from_folder_paths(&PathList::default()),
                remote_connection: None,
                archived: false,
            })
            .collect();
        ChatHistoryDelegate::with_chats(WeakEntity::new_invalid(), chats, None)
    }

    #[test]
    fn test_delete_asks_first_and_only_with_an_empty_search() {
        let mut history = history(&["Fix the parser", "Write tests", "Rename module"]);
        let second = history.matches[1].chat.thread_id;
        history.selected_index = 1;

        // While the search field has text, Backspace and Delete edit it.
        history.query = "t".into();
        assert!(!history.ask_to_delete_selected());
        assert_eq!(history.pending_delete, None);

        history.query.clear();
        assert!(history.ask_to_delete_selected());
        assert_eq!(history.pending_delete, Some(second));
        assert_eq!(history.chats.len(), 3, "asking deletes nothing");

        // Escape withdraws the question once, then is left to close History.
        assert!(history.cancel_delete());
        assert!(!history.cancel_delete());
        assert_eq!(history.chats.len(), 3);
    }

    #[test]
    fn test_confirmed_delete_removes_the_row_and_keeps_a_selection() {
        let mut history = history(&["Fix the parser", "Write tests", "Rename module"]);
        let last = history.matches[2].chat.thread_id;

        // The bin of a row that is not selected selects it, so Enter
        // confirms that row and no other.
        history.ask_to_delete(last);
        assert_eq!(history.selected_index, 2);
        assert_eq!(history.pending_delete, Some(last));

        let removed = history.remove(last).expect("the chat is listed");
        assert_eq!(removed.thread_id, last);
        assert_eq!(history.pending_delete, None);
        assert_eq!(history.matches.len(), 2);
        assert_eq!(history.selected_index, 1, "selection stays on a row");
        assert!(history.remove(last).is_none(), "a chat is deleted once");
    }

    #[test]
    fn test_empty_history_has_no_row_to_delete() {
        let mut history = history(&[]);
        assert!(!history.ask_to_delete_selected());
        assert_eq!(history.pending_delete, None);
    }
}

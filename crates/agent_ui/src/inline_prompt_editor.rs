use agent::ThreadStore;
use app_actions::{
    agent::ToggleModelSelector,
    editor::{MoveDown, MoveUp},
};
use collections::{HashMap, VecDeque};
use editor::actions::Paste;
use editor::display_map::{CreaseId, EditorMargins};
use editor::{AnchorRangeExt as _, MultiBufferOffset};
use editor::{
    ContextMenuOptions, Editor, EditorElement, EditorEvent, EditorMode, EditorStyle, MultiBuffer,
};
use fs::Fs;
use gpui::{
    AnyElement, App, Context, Entity, EventEmitter, FocusHandle, Focusable, Subscription,
    TextStyle, WeakEntity, Window, actions,
};
use language_model::{LanguageModel, LanguageModelRegistry};
use parking_lot::Mutex;
use picker::popover_menu::PickerPopoverMenu;
use project::Project;
use settings::{Settings, update_settings_file};
use std::fmt::Write as _;
use std::ops::Range;
use std::rc::Rc;
use std::sync::Arc;
use theme_settings::ThemeSettings;
use ui::utils::WithRemSize;
use ui::{ButtonLike, IconButtonShape, KeyBinding, PopoverMenuHandle, Tooltip, prelude::*};
use uuid::Uuid;
use workspace::Workspace;

use crate::buffer_codegen::{BufferCodegen, inline_assist_model};
use crate::completion_provider::{
    PromptCompletionProvider, PromptCompletionProviderDelegate, PromptContextType,
};
use crate::connections::{self, Backing};
use crate::language_model_selector::{LanguageModelSelector, language_model_selector};
use crate::mention_set::paste_images_as_context;
use crate::mention_set::{MentionSet, crease_for_mention};
use crate::terminal_codegen::TerminalCodegen;
use crate::{CycleFavoriteModels, CycleNextInlineAssist, CyclePreviousInlineAssist};

actions!(
    inline_assistant,
    [
        /// Does nothing: results are no longer rated. The name stays because
        /// the built-in keymaps still bind it, and an unknown action there
        /// stops the app at startup.
        ThumbsUpResult,
        /// Does nothing. See [`ThumbsUpResult`].
        ThumbsDownResult,
        /// Refines the generated result with one more instruction.
        FollowUp
    ]
);

pub struct PromptEditor<T> {
    pub editor: Entity<Editor>,
    mode: PromptEditorMode,
    mention_set: Entity<MentionSet>,
    workspace: WeakEntity<Workspace>,
    model_selector: Entity<LanguageModelSelector>,
    model_selector_menu_handle: PopoverMenuHandle<LanguageModelSelector>,
    edited_since_done: bool,
    prompt_history: VecDeque<String>,
    prompt_history_ix: Option<usize>,
    pending_prompt: String,
    _codegen_subscription: Subscription,
    editor_subscriptions: Vec<Subscription>,
    show_rate_limit_notice: bool,
    session_id: Uuid,
    /// The instruction of the last request, kept so that a text changed
    /// after a result is sent as a refinement of it.
    last_sent_prompt: Option<String>,
    /// Earlier instructions of this session, oldest first, once the user
    /// refines a result with a follow-up. The editor holds the latest one.
    follow_up_to: Vec<String>,
    /// A follow-up is being written and has not been sent yet.
    composing_follow_up: bool,
    /// Text put back by `abandon_follow_up`, so that its edit event is not
    /// mistaken for the user rewording the instruction.
    restored_prompt: Option<String>,
    _registry_subscription: Subscription,
    _phantom: std::marker::PhantomData<T>,
}

impl<T: 'static> EventEmitter<PromptEditorEvent> for PromptEditor<T> {}

/// What the prompt box is showing. Every state keeps the same two rows (the
/// instruction on top, one status row below) so the box never changes height
/// while the code streams in.
#[derive(Copy, Clone, Debug, PartialEq, Eq)]
enum BoxState {
    /// No usable model: one line and a button leading to Connections.
    Setup,
    /// Writing an instruction (first one, a reworded one or a follow-up).
    Compose,
    Generating,
    /// The result is in the buffer, waiting to be accepted or rejected.
    Review,
    /// The model answered that it cannot do what was asked.
    Declined,
    Failed,
}

impl<T: 'static> Render for PromptEditor<T> {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let ui_font_size = ThemeSettings::get_global(cx).ui_font_size(cx);
        let state = self.box_state(cx);
        let colors = cx.theme().colors();

        // In a buffer the box starts where the code starts, past the gutter.
        let (left_inset, right_inset, surface) = match &self.mode {
            PromptEditorMode::Buffer { editor_margins, .. } => {
                let editor_margins = editor_margins.lock();
                (
                    editor_margins.gutter.full_width(),
                    editor_margins.right + px(12.),
                    colors.editor_background,
                )
            }
            PromptEditorMode::Terminal { .. } => (px(8.), px(8.), colors.terminal_background),
        };

        // A raised surface on the sheet: its own ground and the strong line,
        // which turns to the text colour while the box has the focus.
        let focus_handle = self.editor.focus_handle(cx);
        let box_background = colors.elevated_surface_background;
        let border_color = if focus_handle.contains_focused(window, cx) {
            colors.text
        } else {
            colors.border_strong
        };

        let top_row = if state == BoxState::Setup {
            self.render_setup_notice(cx)
        } else {
            self.render_editor(state, window, cx)
        };
        let status_row = self.render_status_row(state, window, cx);

        div()
            .key_context("InlineAssistant")
            .capture_action(cx.listener(Self::paste))
            .block_mouse_except_scroll()
            .size_full()
            .flex()
            .flex_col()
            .justify_center()
            .py_1()
            .pl(left_inset)
            .pr(right_inset)
            .bg(surface)
            .child(
                // As wide as the text column it sits in.
                v_flex()
                    .id("inline-prompt-box")
                    .w_full()
                    .p_2()
                    .gap_0p5()
                    .bg(box_background)
                    .border_1()
                    .border_color(border_color)
                    // Without a model the instruction editor is not drawn; the box
                    // keeps its focus so Enter and Esc still reach these handlers.
                    .when(state == BoxState::Setup, |this| {
                        this.track_focus(&focus_handle)
                    })
                    .on_action(cx.listener(Self::confirm))
                    .on_action(cx.listener(Self::secondary_confirm))
                    .on_action(cx.listener(Self::cancel))
                    .on_action(cx.listener(Self::menu_cancel))
                    .on_action(cx.listener(Self::move_up))
                    .on_action(cx.listener(Self::move_down))
                    .on_action(cx.listener(|this, _: &FollowUp, window, cx| {
                        this.begin_follow_up(window, cx);
                    }))
                    .capture_action(cx.listener(Self::cycle_prev))
                    .capture_action(cx.listener(Self::cycle_next))
                    .on_action(cx.listener(|this, _: &ToggleModelSelector, window, cx| {
                        this.model_selector_menu_handle.toggle(window, cx);
                    }))
                    .on_action(cx.listener(|this, _: &CycleFavoriteModels, window, cx| {
                        this.model_selector.update(cx, |model_selector, cx| {
                            model_selector.delegate.cycle_favorite_models(window, cx);
                        });
                    }))
                    .child(top_row)
                    .child(WithRemSize::new(ui_font_size).w_full().child(status_row)),
            )
    }
}

/// One clear line for the failures a user can act on; anything else falls
/// back to the first line of the server's own message.
fn error_summary(error: &anyhow::Error) -> SharedString {
    use language_model::{
        LanguageModelCompletionError as CompletionError, ProviderErrorCategory as Category,
    };

    let known = error
        .chain()
        .find_map(|cause| cause.downcast_ref::<CompletionError>())
        .and_then(|error| match error {
            CompletionError::NoApiKey { .. } => {
                Some("No key for this model. Add one in Connections, then retry.")
            }
            CompletionError::ProviderRejection { category, .. } => match category {
                Category::Authentication | Category::Permission => {
                    Some("The key was refused. Check it in Connections, then retry.")
                }
                Category::PaymentRequired => {
                    Some("Your balance is too low. Add funds, then retry.")
                }
                Category::RateLimit => Some("Rate limit reached. Wait a moment, then retry."),
                Category::Overloaded | Category::InternalServer | Category::Timeout => {
                    Some("The model's server is not responding. Retry in a moment.")
                }
                Category::PromptTooLarge { .. } => {
                    Some("The selection is too large for this model. Select less code.")
                }
                _ => None,
            },
            CompletionError::HttpSend { .. }
            | CompletionError::ApiReadResponseError { .. }
            | CompletionError::StreamEndedUnexpectedly { .. } => {
                Some("Could not reach the model's server. Check your network, then retry.")
            }
            CompletionError::ModelUnavailable { .. } => {
                Some("This model is no longer available. Pick another one, then retry.")
            }
            _ => None,
        });
    if let Some(known) = known {
        return known.into();
    }

    let message = error.to_string();
    message
        .lines()
        .map(str::trim)
        .find(|line| !line.is_empty())
        .unwrap_or("The request failed.")
        .to_string()
        .into()
}

/// The picker behind the model's name. It shows the model the edit is really
/// sent to (`active_model`) and writes a choice to `agent.inline_assistant_model`.
fn build_model_selector(
    active_model: impl Fn(&App) -> Option<LanguageModel> + 'static,
    fs: Arc<dyn Fs>,
    focus_handle: FocusHandle,
    window: &mut Window,
    cx: &mut App,
) -> Entity<LanguageModelSelector> {
    cx.new(|cx| {
        language_model_selector(
            active_model,
            {
                let fs = fs.clone();
                move |model, cx| {
                    let provider = model.provider_id().0.to_string();
                    let model_id = model.id().0.to_string();
                    update_settings_file(fs.clone(), cx, move |settings, _cx| {
                        settings
                            .agent
                            .get_or_insert_default()
                            .set_inline_assistant_model(provider.clone(), model_id);
                    });
                }
            },
            move |model, should_be_favorite, cx| {
                crate::favorite_models::toggle_in_settings(
                    model,
                    should_be_favorite,
                    fs.clone(),
                    cx,
                );
            },
            true, // Use popover styles for picker
            focus_handle,
            window,
            cx,
        )
    })
}

impl<T: 'static> Focusable for PromptEditor<T> {
    fn focus_handle(&self, cx: &App) -> FocusHandle {
        self.editor.focus_handle(cx)
    }
}

impl<T: 'static> PromptEditor<T> {
    const MAX_LINES: u8 = 8;
    const TERMINAL_CHROME_LINES: u8 = 3;

    fn codegen_status<'a>(&'a self, cx: &'a App) -> &'a CodegenStatus {
        match &self.mode {
            PromptEditorMode::Buffer { codegen, .. } => codegen.read(cx).status(cx),
            PromptEditorMode::Terminal { codegen, .. } => &codegen.read(cx).status,
        }
    }

    /// Connecting a provider or changing the model elsewhere (agent panel,
    /// settings) must show in an open prompt box right away.
    fn observe_model_registry(cx: &mut Context<Self>) -> Subscription {
        cx.subscribe(
            &LanguageModelRegistry::global(cx),
            |_, _, _: &language_model::Event, cx| cx.notify(),
        )
    }

    fn subscribe_to_editor(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.editor_subscriptions.clear();
        self.editor_subscriptions.push(cx.subscribe_in(
            &self.editor,
            window,
            Self::handle_prompt_editor_events,
        ));
    }

    fn assign_completion_provider(&mut self, cx: &mut Context<Self>) {
        self.editor.update(cx, |editor, cx| {
            editor.set_completion_provider(Some(Rc::new(PromptCompletionProvider::new(
                PromptEditorCompletionProviderDelegate,
                cx.weak_entity(),
                self.mention_set.clone(),
                self.workspace.clone(),
            ))));
        });
    }

    pub fn set_show_cursor_when_unfocused(
        &mut self,
        show_cursor_when_unfocused: bool,
        cx: &mut Context<Self>,
    ) {
        self.editor.update(cx, |editor, cx| {
            editor.set_show_cursor_when_unfocused(show_cursor_when_unfocused, cx)
        });
    }

    pub fn unlink(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let prompt = self.prompt(cx);
        let existing_creases = self.editor.update(cx, |editor, cx| {
            extract_message_creases(editor, &self.mention_set, window, cx)
        });
        let focus = self.editor.focus_handle(cx).contains_focused(window, cx);
        let placeholder = Self::placeholder_text(&self.mode, window, cx);
        let mut creases = vec![];
        self.editor = cx.new(|cx| {
            let mut editor = Editor::auto_height(1, Self::MAX_LINES as usize, window, cx);
            editor.set_soft_wrap_mode(language::language_settings::SoftWrap::EditorWidth, cx);
            editor.set_placeholder_text(&placeholder, window, cx);
            editor.set_text(prompt, window, cx);
            creases = insert_message_creases(&mut editor, &existing_creases, window, cx);

            if focus {
                window.focus(&editor.focus_handle(cx), cx);
            }
            editor
        });

        self.mention_set.update(cx, |mention_set, _cx| {
            debug_assert_eq!(
                creases.len(),
                mention_set.creases().len(),
                "Missing creases"
            );

            let mentions = mention_set
                .clear()
                .zip(creases)
                .map(|((_, value), id)| (id, value))
                .collect::<HashMap<_, _>>();
            mention_set.set_mentions(mentions);
        });

        self.assign_completion_provider(cx);
        self.subscribe_to_editor(window, cx);
    }

    pub fn placeholder_text(mode: &PromptEditorMode, _window: &mut Window, cx: &mut App) -> String {
        match mode {
            PromptEditorMode::Buffer { codegen, .. } => {
                if codegen.read(cx).is_insertion {
                    "Describe the code. Enter to run, Esc to close."
                } else {
                    "Describe the change. Enter to run, Esc to close."
                }
            }
            PromptEditorMode::Terminal { .. } => {
                "Describe the command. Enter to run, Esc to close."
            }
        }
        .to_string()
    }

    /// The instruction sent to the model: the editor's text, preceded by the
    /// earlier instructions when this one is a follow-up. Each generation
    /// starts again from the original text, which is what lets Reject restore
    /// it exactly however many follow-ups were made.
    pub fn request_prompt(&self, cx: &App) -> String {
        let prompt = self.prompt(cx);
        if self.follow_up_to.is_empty() {
            return prompt;
        }

        let mut request = String::from(
            "Apply these instructions in order, each one refining the result of the previous ones:\n",
        );
        for (ix, instruction) in self
            .follow_up_to
            .iter()
            .chain(std::iter::once(&prompt))
            .enumerate()
        {
            writeln!(request, "{}. {}", ix + 1, instruction.trim()).ok();
        }
        request
    }

    fn needs_setup(&self, cx: &App) -> bool {
        let registry = LanguageModelRegistry::read_global(cx);
        registry
            .configuration_error(registry.inline_assistant_model(), cx)
            .is_some()
    }

    /// The model's own explanation when it declined the instruction.
    fn failure_message(&self, cx: &App) -> Option<String> {
        match &self.mode {
            PromptEditorMode::Buffer { codegen, .. } => codegen
                .read(cx)
                .active_alternative()
                .read(cx)
                .failure
                .clone(),
            PromptEditorMode::Terminal { .. } => None,
        }
    }

    fn box_state(&self, cx: &App) -> BoxState {
        match self.codegen_status(cx) {
            CodegenStatus::Pending => BoxState::Generating,
            CodegenStatus::Done if !self.edited_since_done => {
                if self.failure_message(cx).is_some() {
                    BoxState::Declined
                } else {
                    BoxState::Review
                }
            }
            _ if self.needs_setup(cx) => BoxState::Setup,
            CodegenStatus::Error(_) => BoxState::Failed,
            CodegenStatus::Idle | CodegenStatus::Done => BoxState::Compose,
        }
    }

    /// Keeps the result on screen and empties the editor for one more
    /// instruction on top of it.
    fn begin_follow_up(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.box_state(cx) != BoxState::Review {
            return;
        }

        let previous = self.prompt(cx);
        self.follow_up_to.push(previous);
        self.composing_follow_up = true;
        self.edited_since_done = true;
        self.editor.update(cx, |editor, cx| {
            editor.set_text("", window, cx);
            editor.set_placeholder_text("Add a follow-up instruction…", window, cx);
            window.focus(&editor.focus_handle(cx), cx);
        });
        cx.notify();
    }

    /// Esc while writing a follow-up goes back to Keep / Undo instead of
    /// throwing the result away.
    fn abandon_follow_up(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some(previous) = self.follow_up_to.pop() else {
            return;
        };

        let placeholder = Self::placeholder_text(&self.mode, window, cx);
        self.composing_follow_up = false;
        self.edited_since_done = false;
        self.restored_prompt = Some(previous.clone());
        self.editor.update(cx, |editor, cx| {
            editor.set_text(previous, window, cx);
            editor.set_placeholder_text(&placeholder, window, cx);
        });
        cx.notify();
    }

    /// Without a usable model the box leads to the Connections window.
    fn open_connections(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        window.dispatch_action(Box::new(crate::ManageProviders), cx);
    }

    /// A terminal block has a fixed height in terminal lines: the instruction
    /// plus the box's own padding and status row.
    fn sync_terminal_height(&mut self, cx: &mut Context<Self>) {
        let prompt_lines = self
            .editor
            .update(cx, |editor, cx| editor.max_point(cx).row().0 + 1)
            .min(Self::MAX_LINES as u32) as u8;
        let PromptEditorMode::Terminal {
            height_in_lines: current_height,
            ..
        } = &mut self.mode
        else {
            return;
        };

        let height_in_lines = prompt_lines + Self::TERMINAL_CHROME_LINES;
        if height_in_lines != *current_height {
            *current_height = height_in_lines;
            cx.emit(PromptEditorEvent::Resized { height_in_lines });
        }
    }

    pub fn prompt(&self, cx: &App) -> String {
        self.editor.read(cx).text(cx)
    }

    fn paste(&mut self, _: &Paste, window: &mut Window, cx: &mut Context<Self>) {
        if inline_assistant_model_supports_images(cx)
            && let Some(task) = paste_images_as_context(
                self.editor.clone(),
                self.mention_set.clone(),
                self.workspace.clone(),
                window,
                cx,
            )
        {
            // Capture-phase actions propagate by default, so stop the editor from also
            // pasting the clipboard's file paths as text.
            cx.stop_propagation();
            task.detach();
        }
    }

    fn handle_prompt_editor_events(
        &mut self,
        editor: &Entity<Editor>,
        event: &EditorEvent,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        match event {
            EditorEvent::Edited { .. } => {
                let snapshot = editor.update(cx, |editor, cx| editor.snapshot(window, cx));

                self.mention_set
                    .update(cx, |mention_set, _cx| mention_set.remove_invalid(&snapshot));

                if let Some(workspace) = Workspace::for_window(window, cx) {
                    workspace.update(cx, |workspace, cx| {
                        let is_via_ssh = workspace.project().read(cx).is_via_remote_server();

                        workspace
                            .client()
                            .telemetry()
                            .log_edit_event("inline assist", is_via_ssh);
                    });
                }
                let prompt = snapshot.text();
                self.sync_terminal_height(cx);
                if self
                    .restored_prompt
                    .take()
                    .is_some_and(|restored| restored == prompt)
                {
                    return;
                }
                if self
                    .prompt_history_ix
                    .is_none_or(|ix| self.prompt_history[ix] != prompt)
                {
                    self.prompt_history_ix.take();
                    self.pending_prompt = prompt;
                }

                self.edited_since_done = true;
                cx.notify();
            }
            EditorEvent::Blurred => {
                if self.show_rate_limit_notice {
                    self.show_rate_limit_notice = false;
                    cx.notify();
                }
            }
            _ => {}
        }
    }

    fn cancel(&mut self, _: &editor::actions::Cancel, window: &mut Window, cx: &mut Context<Self>) {
        match self.codegen_status(cx) {
            CodegenStatus::Pending => {
                cx.emit(PromptEditorEvent::StopRequested);
            }
            CodegenStatus::Done if self.composing_follow_up => {
                self.abandon_follow_up(window, cx);
            }
            CodegenStatus::Idle | CodegenStatus::Done | CodegenStatus::Error(_) => {
                cx.emit(PromptEditorEvent::CancelRequested);
            }
        }
    }

    // Esc resolves to `menu::Cancel` when the instruction editor is not drawn.
    fn menu_cancel(&mut self, _: &menu::Cancel, window: &mut Window, cx: &mut Context<Self>) {
        self.cancel(&editor::actions::Cancel, window, cx);
    }

    fn confirm(&mut self, _: &menu::Confirm, window: &mut Window, cx: &mut Context<Self>) {
        if self.box_state(cx) == BoxState::Setup {
            self.open_connections(window, cx);
            return;
        }
        self.handle_confirm(false, cx);
    }

    fn secondary_confirm(
        &mut self,
        _: &menu::SecondaryConfirm,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if self.box_state(cx) == BoxState::Setup {
            self.open_connections(window, cx);
            return;
        }
        let execute = matches!(self.mode, PromptEditorMode::Terminal { .. });
        self.handle_confirm(execute, cx);
    }

    fn handle_confirm(&mut self, execute: bool, cx: &mut Context<Self>) {
        match self.codegen_status(cx) {
            CodegenStatus::Idle | CodegenStatus::Error(_) => self.request_start(cx),
            CodegenStatus::Pending => {}
            CodegenStatus::Done => {
                if self.edited_since_done {
                    self.request_start(cx);
                } else {
                    cx.emit(PromptEditorEvent::ConfirmRequested { execute });
                }
            }
        }
    }

    /// Sends the instruction, unless there is nothing to send.
    fn request_start(&mut self, cx: &mut Context<Self>) {
        let prompt = self.prompt(cx);
        if prompt.trim().is_empty() {
            return;
        }
        // A text changed after a result refines that result: the instruction
        // that produced it is sent first. (`begin_follow_up` already did it.)
        let refines_result = matches!(self.codegen_status(cx), CodegenStatus::Done)
            && self.edited_since_done
            && !self.composing_follow_up;
        if let Some(previous) = self.last_sent_prompt.take()
            && refines_result
            && previous.trim() != prompt.trim()
        {
            self.follow_up_to.push(previous);
        }
        self.last_sent_prompt = Some(prompt);
        self.composing_follow_up = false;
        self.fire_started_telemetry(cx);
        cx.emit(PromptEditorEvent::StartRequested);
    }

    fn fire_started_telemetry(&self, cx: &Context<Self>) {
        let Some(model) = self.active_model(cx) else {
            return;
        };

        let model_telemetry_id = model.telemetry_id();
        let model_provider_id = model.provider_id().to_string();

        let (kind, language_name) = match &self.mode {
            PromptEditorMode::Buffer { codegen, .. } => {
                let codegen = codegen.read(cx);
                (
                    "inline",
                    codegen.language_name(cx).map(|name| name.to_string()),
                )
            }
            PromptEditorMode::Terminal { .. } => ("inline_terminal", None),
        };

        telemetry::event!(
            "Assistant Started",
            session_id = self.session_id.to_string(),
            kind = kind,
            phase = "started",
            model = model_telemetry_id,
            model_provider = model_provider_id,
            language_name = language_name,
        );
    }

    /// The model the next request of this box goes to.
    fn active_model(&self, cx: &App) -> Option<LanguageModel> {
        self.model_selector.read(cx).delegate.active_model(cx)
    }

    fn move_up(&mut self, _: &MoveUp, window: &mut Window, cx: &mut Context<Self>) {
        if let Some(ix) = self.prompt_history_ix {
            if ix > 0 {
                self.prompt_history_ix = Some(ix - 1);
                let prompt = self.prompt_history[ix - 1].as_str();
                self.editor.update(cx, |editor, cx| {
                    editor.set_text(prompt, window, cx);
                    editor.move_to_beginning(&Default::default(), window, cx);
                });
            }
        } else if !self.prompt_history.is_empty() {
            self.prompt_history_ix = Some(self.prompt_history.len() - 1);
            let prompt = self.prompt_history[self.prompt_history.len() - 1].as_str();
            self.editor.update(cx, |editor, cx| {
                editor.set_text(prompt, window, cx);
                editor.move_to_beginning(&Default::default(), window, cx);
            });
        }
    }

    fn move_down(&mut self, _: &MoveDown, window: &mut Window, cx: &mut Context<Self>) {
        if let Some(ix) = self.prompt_history_ix {
            if ix < self.prompt_history.len() - 1 {
                self.prompt_history_ix = Some(ix + 1);
                let prompt = self.prompt_history[ix + 1].as_str();
                self.editor.update(cx, |editor, cx| {
                    editor.set_text(prompt, window, cx);
                    editor.move_to_end(&Default::default(), window, cx)
                });
            } else {
                self.prompt_history_ix = None;
                let prompt = self.pending_prompt.as_str();
                self.editor.update(cx, |editor, cx| {
                    editor.set_text(prompt, window, cx);
                    editor.move_to_end(&Default::default(), window, cx)
                });
            }
        }
    }

    fn key_hint(&self, action: &dyn gpui::Action, cx: &App) -> KeyBinding {
        KeyBinding::for_action_in(action, &self.editor.focus_handle(cx), cx)
            .size(rems_from_px(10_f32))
    }

    fn render_cancel_button(
        &self,
        label: &'static str,
        style: ButtonStyle,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        Button::new("cancel", label)
            .label_size(LabelSize::Small)
            .style(style)
            .key_binding(self.key_hint(&editor::actions::Cancel, cx))
            .on_click(cx.listener(|this, _, window, cx| {
                this.cancel(&editor::actions::Cancel, window, cx);
            }))
            .into_any_element()
    }

    /// The name of the model the edit goes to; it opens the model picker.
    /// Who answers and at which price is said by its tooltip.
    fn render_model_chip(&self, window: &mut Window, cx: &mut Context<Self>) -> AnyElement {
        let model = self.active_model(cx);
        let model_name = model
            .as_ref()
            .map(|model| model.name().0)
            .unwrap_or_else(|| SharedString::from("Select a Model"));
        let provider_name = model.as_ref().and_then(|model| {
            let catalog_title = connections::catalog(cx).into_iter().find_map(|spec| {
                matches!(&spec.backing, Backing::Provider(id) if id == &model.provider_id)
                    .then_some(spec.title)
            });
            catalog_title.or_else(|| {
                LanguageModelRegistry::read_global(cx)
                    .provider(&model.provider_id)
                    .map(|provider| provider.name().0)
            })
        });
        let cost = model
            .as_ref()
            .and_then(|model| model.cost_info.as_ref())
            .map(|cost| cost.to_shared_string());
        let color = if self.model_selector_menu_handle.is_deployed() {
            Color::Default
        } else {
            Color::Muted
        };
        let show_cycle_row = self.model_selector.read(cx).delegate.favorites_count() > 1;
        let tooltip = Tooltip::element(move |_, _cx| {
            crate::ui::ModelSelectorTooltip::new()
                .provider(provider_name.clone())
                .cost(cost.clone())
                .show_cycle_row(show_cycle_row)
                .into_any_element()
        });

        PickerPopoverMenu::new(
            self.model_selector.clone(),
            ButtonLike::new("active-model").child(
                h_flex()
                    .min_w_0()
                    .gap_1()
                    .child(
                        Label::new(model_name)
                            .size(LabelSize::Small)
                            .color(color)
                            .truncate(),
                    )
                    .child(
                        Icon::new(IconName::ChevronDown)
                            .color(Color::Muted)
                            .size(IconSize::XSmall),
                    ),
            ),
            tooltip,
            gpui::Anchor::TopLeft,
            cx,
        )
        .with_handle(self.model_selector_menu_handle.clone())
        .offset(gpui::Point {
            x: px(0.0),
            y: px(2.0),
        })
        .render(window, cx)
        .into_any_element()
    }

    /// A one-line message on the left of the status row; the full text is in
    /// its tooltip when it does not fit.
    fn render_status_message(
        icon: IconName,
        color: Color,
        summary: SharedString,
        details: SharedString,
    ) -> AnyElement {
        // Starts where the instruction's text and the model's name start.
        h_flex()
            .id("status-message")
            .min_w_0()
            .gap_1p5()
            .pl_1()
            .child(Icon::new(icon).size(IconSize::XSmall).color(color))
            .child(
                Label::new(summary)
                    .size(LabelSize::Small)
                    .color(color)
                    .truncate(),
            )
            .tooltip(Tooltip::text(details))
            .into_any_element()
    }

    /// The bottom row: on the left the model (or what happened), on the
    /// right the actions of the current state. At most one of them is filled.
    fn render_status_row(
        &self,
        state: BoxState,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let is_terminal = matches!(self.mode, PromptEditorMode::Terminal { .. });
        // Only when the user listed models in `agent.inline_alternatives`.
        let cycle_controls = match &self.mode {
            PromptEditorMode::Buffer { codegen, .. }
                if codegen.read(cx).alternative_count(cx) > 1 =>
            {
                Some(self.render_cycle_controls(codegen.read(cx), cx))
            }
            _ => None,
        };

        let mut left = Vec::<AnyElement>::new();
        let mut right = Vec::<AnyElement>::new();

        match state {
            BoxState::Setup => {
                right.push(self.render_cancel_button("Cancel", ButtonStyle::Subtle, cx));
                right.push(
                    Button::new("connect", "Connect")
                        .label_size(LabelSize::Small)
                        .style(ButtonStyle::Outlined)
                        .key_binding(self.key_hint(&menu::Confirm, cx))
                        .on_click(cx.listener(|this, _, window, cx| {
                            this.open_connections(window, cx);
                        }))
                        .into_any_element(),
                );
            }
            BoxState::Compose => {
                let is_empty = self.prompt(cx).trim().is_empty();
                left.push(self.render_model_chip(window, cx));
                right.extend(cycle_controls);
                if matches!(self.codegen_status(cx), CodegenStatus::Done) {
                    // A result is in the buffer and the instruction changed.
                    right.push(self.render_cancel_button("Undo", ButtonStyle::Outlined, cx));
                    right.push(
                        Button::primary("start", "Refine")
                            .label_size(LabelSize::Small)
                            .disabled(is_empty)
                            .key_binding(self.key_hint(&menu::Confirm, cx))
                            .tooltip(Tooltip::text(
                                "Generate again with this instruction on top of the previous one",
                            ))
                            .on_click(cx.listener(|this, _, _, cx| this.request_start(cx)))
                            .into_any_element(),
                    );
                } else {
                    right.push(self.key_hint(&menu::Confirm, cx).into_any_element());
                    right.push(
                        IconButton::primary("start", IconName::Send)
                            .icon_size(IconSize::Small)
                            .disabled(is_empty)
                            .aria_label("Run")
                            .tooltip(Tooltip::text(if is_empty {
                                "Type an Instruction to Run"
                            } else {
                                "Run"
                            }))
                            .on_click(cx.listener(|this, _, _, cx| this.request_start(cx)))
                            .into_any_element(),
                    );
                }
            }
            BoxState::Generating => {
                left.push(self.render_model_name(cx));
                // Three steps on a timer that lives only while this row shows it.
                left.push(
                    h_flex()
                        .flex_none()
                        .gap_1()
                        .child(
                            Label::new("Generating")
                                .size(LabelSize::Small)
                                .color(Color::Muted),
                        )
                        .child(crate::ui::WorkingIndicator::new("inline-generating"))
                        .into_any_element(),
                );
                right.push(
                    Button::new("stop", "Stop")
                        .label_size(LabelSize::Small)
                        .style(ButtonStyle::Outlined)
                        .key_binding(self.key_hint(&editor::actions::Cancel, cx))
                        .tooltip(Tooltip::text(
                            "What was generated so far is kept for review",
                        ))
                        .on_click(
                            cx.listener(|_, _, _, cx| cx.emit(PromptEditorEvent::StopRequested)),
                        )
                        .into_any_element(),
                );
            }
            BoxState::Review => {
                left.push(self.render_model_chip(window, cx));
                right.extend(cycle_controls);
                right.push(self.render_cancel_button("Undo", ButtonStyle::Outlined, cx));
                if is_terminal {
                    right.push(
                        Button::new("run", "Run")
                            .label_size(LabelSize::Small)
                            .style(ButtonStyle::Outlined)
                            .key_binding(self.key_hint(&menu::SecondaryConfirm, cx))
                            .tooltip(Tooltip::text("Run the generated command now"))
                            .on_click(cx.listener(|_, _, _, cx| {
                                cx.emit(PromptEditorEvent::ConfirmRequested { execute: true });
                            }))
                            .into_any_element(),
                    );
                }
                right.push(
                    Button::primary("accept", "Keep")
                        .label_size(LabelSize::Small)
                        .key_binding(self.key_hint(&menu::Confirm, cx))
                        .tooltip(Tooltip::text(if is_terminal {
                            "Keep the command at the prompt without running it"
                        } else {
                            "Keep the edit. Change the instruction to refine it instead."
                        }))
                        .on_click(cx.listener(|_, _, _, cx| {
                            cx.emit(PromptEditorEvent::ConfirmRequested { execute: false });
                        }))
                        .into_any_element(),
                );
            }
            BoxState::Declined => {
                let message = SharedString::from(self.failure_message(cx).unwrap_or_default());
                let summary = message
                    .lines()
                    .map(str::trim)
                    .find(|line| !line.is_empty())
                    .unwrap_or("The model could not do this.")
                    .to_string();
                left.push(Self::render_status_message(
                    IconName::Warning,
                    Color::Muted,
                    summary.into(),
                    message,
                ));
                right.push(self.render_cancel_button("Close", ButtonStyle::Outlined, cx));
            }
            BoxState::Failed => {
                if let CodegenStatus::Error(error) = self.codegen_status(cx) {
                    left.push(Self::render_status_message(
                        IconName::XCircle,
                        Color::Error,
                        error_summary(error),
                        format!("{error:#}").into(),
                    ));
                }
                right.push(self.render_model_chip(window, cx));
                right.push(
                    Button::new("retry", "Retry")
                        .label_size(LabelSize::Small)
                        .style(ButtonStyle::Outlined)
                        .key_binding(self.key_hint(&menu::Confirm, cx))
                        .on_click(cx.listener(|this, _, _, cx| this.request_start(cx)))
                        .into_any_element(),
                );
            }
        }

        h_flex()
            .h(rems_from_px(24_f32))
            .w_full()
            .gap_2()
            .justify_between()
            .child(
                h_flex()
                    .flex_1()
                    .min_w_0()
                    .overflow_hidden()
                    .gap_2()
                    .children(left),
            )
            .child(h_flex().flex_none().gap_1().children(right))
            .into_any_element()
    }

    /// The model's name as plain text, while its picker would change nothing.
    fn render_model_name(&self, cx: &App) -> AnyElement {
        let name = self
            .active_model(cx)
            .map(|model| model.name().0)
            .unwrap_or_default();
        div()
            .min_w_0()
            .pl_1()
            .child(
                Label::new(name)
                    .size(LabelSize::Small)
                    .color(Color::Muted)
                    .truncate(),
            )
            .into_any_element()
    }

    fn cycle_prev(
        &mut self,
        _: &CyclePreviousInlineAssist,
        _: &mut Window,
        cx: &mut Context<Self>,
    ) {
        match &self.mode {
            PromptEditorMode::Buffer { codegen, .. } => {
                codegen.update(cx, |codegen, cx| codegen.cycle_prev(cx));
            }
            PromptEditorMode::Terminal { .. } => {
                // no cycle buttons in terminal mode
            }
        }
    }

    fn cycle_next(&mut self, _: &CycleNextInlineAssist, _: &mut Window, cx: &mut Context<Self>) {
        match &self.mode {
            PromptEditorMode::Buffer { codegen, .. } => {
                codegen.update(cx, |codegen, cx| codegen.cycle_next(cx));
            }
            PromptEditorMode::Terminal { .. } => {
                // no cycle buttons in terminal mode
            }
        }
    }

    fn render_cycle_controls(&self, codegen: &BufferCodegen, cx: &Context<Self>) -> AnyElement {
        let disabled = matches!(codegen.status(cx), CodegenStatus::Idle);

        let model_registry = LanguageModelRegistry::read_global(cx);
        let default_model = model_registry.default_model();
        let alternative_models = model_registry.inline_alternative_models();

        let get_model_name = |index: usize| -> String {
            let name = |model: &LanguageModel| model.name.0.to_string();

            match index {
                0 => default_model.as_ref().map_or_else(String::new, name),
                index if index <= alternative_models.len() => alternative_models
                    .get(index - 1)
                    .map_or_else(String::new, name),
                _ => String::new(),
            }
        };

        let total_models = alternative_models.len() + 1;

        if total_models <= 1 {
            return div().into_any_element();
        }

        let current_index = codegen.active_alternative;
        let prev_index = (current_index + total_models - 1) % total_models;
        let next_index = (current_index + 1) % total_models;

        let prev_model_name = get_model_name(prev_index);
        let next_model_name = get_model_name(next_index);

        h_flex()
            .child(
                IconButton::new("previous", IconName::ChevronLeft)
                    .icon_color(Color::Muted)
                    .disabled(disabled || current_index == 0)
                    .shape(IconButtonShape::Square)
                    .tooltip({
                        let focus_handle = self.editor.focus_handle(cx);
                        move |_window, cx| {
                            cx.new(|cx| {
                                let mut tooltip = Tooltip::new("Previous Alternative").key_binding(
                                    KeyBinding::for_action_in(
                                        &CyclePreviousInlineAssist,
                                        &focus_handle,
                                        cx,
                                    ),
                                );
                                if !disabled && current_index != 0 {
                                    tooltip = tooltip.meta(prev_model_name.clone());
                                }
                                tooltip
                            })
                            .into()
                        }
                    })
                    .on_click(cx.listener(|this, _, window, cx| {
                        this.cycle_prev(&CyclePreviousInlineAssist, window, cx);
                    })),
            )
            .child(
                Label::new(format!(
                    "{}/{}",
                    codegen.active_alternative + 1,
                    codegen.alternative_count(cx)
                ))
                .size(LabelSize::Small)
                .color(if disabled {
                    Color::Disabled
                } else {
                    Color::Muted
                }),
            )
            .child(
                IconButton::new("next", IconName::ChevronRight)
                    .icon_color(Color::Muted)
                    .disabled(disabled || current_index == total_models - 1)
                    .shape(IconButtonShape::Square)
                    .tooltip({
                        let focus_handle = self.editor.focus_handle(cx);
                        move |_window, cx| {
                            cx.new(|cx| {
                                let mut tooltip = Tooltip::new("Next Alternative").key_binding(
                                    KeyBinding::for_action_in(
                                        &CycleNextInlineAssist,
                                        &focus_handle,
                                        cx,
                                    ),
                                );
                                if !disabled && current_index != total_models - 1 {
                                    tooltip = tooltip.meta(next_model_name.clone());
                                }
                                tooltip
                            })
                            .into()
                        }
                    })
                    .on_click(cx.listener(|this, _, window, cx| {
                        this.cycle_next(&CycleNextInlineAssist, window, cx)
                    })),
            )
            .into_any_element()
    }

    fn render_editor(
        &mut self,
        state: BoxState,
        _window: &mut Window,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let colors = cx.theme().colors();
        let settings = ThemeSettings::get_global(cx);
        // The instruction steps back while the model is writing.
        let text_color = if state == BoxState::Generating {
            colors.text_muted
        } else {
            colors.editor_foreground
        };

        let text_style = TextStyle {
            color: text_color,
            font_family: settings.buffer_font.family.clone(),
            font_features: settings.buffer_font.features.clone(),
            font_size: settings.buffer_font_size(cx).into(),
            line_height: relative(settings.buffer_line_height.value()),
            ..Default::default()
        };

        div()
            .w_full()
            .child(EditorElement::new(
                &self.editor,
                EditorStyle {
                    background: colors.elevated_surface_background,
                    local_player: cx.theme().players().local(),
                    syntax: cx.theme().syntax().clone(),
                    text: text_style,
                    ..Default::default()
                },
            ))
            .into_any_element()
    }

    /// Shown in place of the instruction when no model can be used: there is
    /// nothing to type until one is connected. The chat says it with the
    /// same words.
    fn render_setup_notice(&self, cx: &mut Context<Self>) -> AnyElement {
        let settings = ThemeSettings::get_global(cx);
        let line_height = settings.buffer_font_size(cx) * settings.buffer_line_height.value();

        h_flex()
            .w_full()
            .h(line_height)
            .pl_1()
            .child(Label::new("No model is connected.").size(LabelSize::Small))
            .into_any_element()
    }
}

pub enum PromptEditorMode {
    Buffer {
        id: InlineAssistId,
        codegen: Entity<BufferCodegen>,
        editor_margins: Arc<Mutex<EditorMargins>>,
    },
    Terminal {
        id: TerminalInlineAssistId,
        codegen: Entity<TerminalCodegen>,
        height_in_lines: u8,
    },
}

pub enum PromptEditorEvent {
    StartRequested,
    StopRequested,
    ConfirmRequested { execute: bool },
    CancelRequested,
    Resized { height_in_lines: u8 },
}

#[derive(Copy, Clone, Default, Debug, PartialEq, Eq, Hash)]
pub struct InlineAssistId(pub usize);

impl InlineAssistId {
    pub fn post_inc(&mut self) -> InlineAssistId {
        let id = *self;
        self.0 += 1;
        id
    }
}

struct PromptEditorCompletionProviderDelegate;

fn inline_assistant_model_supports_images(cx: &App) -> bool {
    LanguageModelRegistry::read_global(cx)
        .inline_assistant_model()
        .map_or(false, |m| m.supports_images())
}

impl PromptCompletionProviderDelegate for PromptEditorCompletionProviderDelegate {
    fn supported_modes(&self, _cx: &App) -> Vec<PromptContextType> {
        vec![
            PromptContextType::File,
            PromptContextType::Symbol,
            PromptContextType::Thread,
            PromptContextType::Fetch,
            PromptContextType::Skill,
        ]
    }

    fn supports_images(&self, cx: &App) -> bool {
        inline_assistant_model_supports_images(cx)
    }

    fn available_commands(&self, _cx: &App) -> Vec<crate::completion_provider::AvailableCommand> {
        Vec::new()
    }

    fn confirm_command(&self, _cx: &mut App) {}
}

impl PromptEditor<BufferCodegen> {
    pub fn new_buffer(
        id: InlineAssistId,
        editor_margins: Arc<Mutex<EditorMargins>>,
        prompt_history: VecDeque<String>,
        prompt_buffer: Entity<MultiBuffer>,
        codegen: Entity<BufferCodegen>,
        session_id: Uuid,
        fs: Arc<dyn Fs>,
        thread_store: Entity<ThreadStore>,
        project: WeakEntity<Project>,
        workspace: WeakEntity<Workspace>,
        window: &mut Window,
        cx: &mut Context<PromptEditor<BufferCodegen>>,
    ) -> PromptEditor<BufferCodegen> {
        let codegen_subscription = cx.observe(&codegen, Self::handle_codegen_changed);
        let mode = PromptEditorMode::Buffer {
            id,
            codegen,
            editor_margins,
        };

        let prompt_editor = cx.new(|cx| {
            let mut editor = Editor::new(
                EditorMode::AutoHeight {
                    min_lines: 1,
                    max_lines: Some(Self::MAX_LINES as usize),
                },
                prompt_buffer,
                None,
                window,
                cx,
            );
            editor.set_soft_wrap_mode(language::language_settings::SoftWrap::EditorWidth, cx);
            // Since the prompt editors for all inline assistants are linked,
            // always show the cursor (even when it isn't focused) because
            // typing in one will make what you typed appear in all of them.
            editor.set_show_cursor_when_unfocused(true, cx);
            editor.set_placeholder_text(&Self::placeholder_text(&mode, window, cx), window, cx);
            editor.set_context_menu_options(ContextMenuOptions {
                min_entries_visible: 12,
                max_entries_visible: 12,
                placement: None,
            });

            editor
        });

        let mention_set = cx.new(|_cx| MentionSet::new(project, Some(thread_store.clone())));

        // The fast model for a short selection, unless the user chose one.
        let model_selector = {
            let codegen = match &mode {
                PromptEditorMode::Buffer { codegen, .. } => codegen.downgrade(),
                PromptEditorMode::Terminal { .. } => unreachable!(),
            };
            build_model_selector(
                move |cx| match codegen.upgrade() {
                    Some(codegen) => codegen.read(cx).model(cx),
                    None => LanguageModelRegistry::read_global(cx).inline_assistant_model(),
                },
                fs,
                prompt_editor.focus_handle(cx),
                window,
                cx,
            )
        };

        let mut this: PromptEditor<BufferCodegen> = PromptEditor {
            editor: prompt_editor,
            mention_set,
            workspace,
            model_selector,
            model_selector_menu_handle: PopoverMenuHandle::default(),
            edited_since_done: false,
            prompt_history,
            prompt_history_ix: None,
            pending_prompt: String::new(),
            _codegen_subscription: codegen_subscription,
            editor_subscriptions: Vec::new(),
            show_rate_limit_notice: false,
            mode,
            session_id,
            last_sent_prompt: None,
            follow_up_to: Vec::new(),
            composing_follow_up: false,
            restored_prompt: None,
            _registry_subscription: Self::observe_model_registry(cx),
            _phantom: Default::default(),
        };

        this.assign_completion_provider(cx);
        this.subscribe_to_editor(window, cx);
        this
    }

    fn handle_codegen_changed(
        &mut self,
        _codegen: Entity<BufferCodegen>,
        cx: &mut Context<PromptEditor<BufferCodegen>>,
    ) {
        match self.codegen_status(cx) {
            CodegenStatus::Idle => {
                self.editor
                    .update(cx, |editor, _| editor.set_read_only(false));
            }
            CodegenStatus::Pending => {
                self.editor
                    .update(cx, |editor, _| editor.set_read_only(true));
            }
            CodegenStatus::Done => {
                self.edited_since_done = false;
                self.editor
                    .update(cx, |editor, _| editor.set_read_only(false));
            }
            CodegenStatus::Error(_error) => {
                self.edited_since_done = false;
                self.editor
                    .update(cx, |editor, _| editor.set_read_only(false));
            }
        }
    }

    pub fn id(&self) -> InlineAssistId {
        match &self.mode {
            PromptEditorMode::Buffer { id, .. } => *id,
            PromptEditorMode::Terminal { .. } => unreachable!(),
        }
    }

    pub fn codegen(&self) -> &Entity<BufferCodegen> {
        match &self.mode {
            PromptEditorMode::Buffer { codegen, .. } => codegen,
            PromptEditorMode::Terminal { .. } => unreachable!(),
        }
    }

    pub fn mention_set(&self) -> &Entity<MentionSet> {
        &self.mention_set
    }

    pub fn editor_margins(&self) -> &Arc<Mutex<EditorMargins>> {
        match &self.mode {
            PromptEditorMode::Buffer { editor_margins, .. } => editor_margins,
            PromptEditorMode::Terminal { .. } => unreachable!(),
        }
    }
}

#[derive(Copy, Clone, Default, Debug, PartialEq, Eq, Hash)]
pub struct TerminalInlineAssistId(pub usize);

impl TerminalInlineAssistId {
    pub fn post_inc(&mut self) -> TerminalInlineAssistId {
        let id = *self;
        self.0 += 1;
        id
    }
}

impl PromptEditor<TerminalCodegen> {
    pub fn new_terminal(
        id: TerminalInlineAssistId,
        prompt_history: VecDeque<String>,
        prompt_buffer: Entity<MultiBuffer>,
        codegen: Entity<TerminalCodegen>,
        session_id: Uuid,
        fs: Arc<dyn Fs>,
        thread_store: Entity<ThreadStore>,
        project: WeakEntity<Project>,
        workspace: WeakEntity<Workspace>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Self {
        let codegen_subscription = cx.observe(&codegen, Self::handle_codegen_changed);
        let mode = PromptEditorMode::Terminal {
            id,
            codegen,
            height_in_lines: 1 + Self::TERMINAL_CHROME_LINES,
        };

        let prompt_editor = cx.new(|cx| {
            let mut editor = Editor::new(
                EditorMode::AutoHeight {
                    min_lines: 1,
                    max_lines: Some(Self::MAX_LINES as usize),
                },
                prompt_buffer,
                None,
                window,
                cx,
            );
            editor.set_soft_wrap_mode(language::language_settings::SoftWrap::EditorWidth, cx);
            editor.set_placeholder_text(&Self::placeholder_text(&mode, window, cx), window, cx);
            editor.set_context_menu_options(ContextMenuOptions {
                min_entries_visible: 12,
                max_entries_visible: 12,
                placement: None,
            });
            editor
        });

        let mention_set = cx.new(|_cx| MentionSet::new(project, Some(thread_store.clone())));

        // A command is always a small edit.
        let model_selector = build_model_selector(
            |cx| inline_assist_model(0, cx),
            fs,
            prompt_editor.focus_handle(cx),
            window,
            cx,
        );

        let mut this = Self {
            editor: prompt_editor,
            mention_set,
            workspace,
            model_selector,
            model_selector_menu_handle: PopoverMenuHandle::default(),
            edited_since_done: false,
            prompt_history,
            prompt_history_ix: None,
            pending_prompt: String::new(),
            _codegen_subscription: codegen_subscription,
            editor_subscriptions: Vec::new(),
            mode,
            show_rate_limit_notice: false,
            session_id,
            last_sent_prompt: None,
            follow_up_to: Vec::new(),
            composing_follow_up: false,
            restored_prompt: None,
            _registry_subscription: Self::observe_model_registry(cx),
            _phantom: Default::default(),
        };
        this.sync_terminal_height(cx);
        this.assign_completion_provider(cx);
        this.subscribe_to_editor(window, cx);
        this
    }

    /// Height of the terminal block that holds this prompt box, in terminal lines.
    pub fn height_in_lines(&self) -> u8 {
        match &self.mode {
            PromptEditorMode::Buffer { .. } => unreachable!(),
            PromptEditorMode::Terminal {
                height_in_lines, ..
            } => *height_in_lines,
        }
    }

    fn handle_codegen_changed(
        &mut self,
        _codegen: Entity<TerminalCodegen>,
        cx: &mut Context<Self>,
    ) {
        match &self.codegen().read(cx).status {
            CodegenStatus::Idle => {
                self.editor
                    .update(cx, |editor, _| editor.set_read_only(false));
            }
            CodegenStatus::Pending => {
                self.editor
                    .update(cx, |editor, _| editor.set_read_only(true));
            }
            CodegenStatus::Done | CodegenStatus::Error(_) => {
                self.edited_since_done = false;
                self.editor
                    .update(cx, |editor, _| editor.set_read_only(false));
            }
        }
    }

    pub fn mention_set(&self) -> &Entity<MentionSet> {
        &self.mention_set
    }

    pub fn codegen(&self) -> &Entity<TerminalCodegen> {
        match &self.mode {
            PromptEditorMode::Buffer { .. } => unreachable!(),
            PromptEditorMode::Terminal { codegen, .. } => codegen,
        }
    }

    pub fn id(&self) -> TerminalInlineAssistId {
        match &self.mode {
            PromptEditorMode::Buffer { .. } => unreachable!(),
            PromptEditorMode::Terminal { id, .. } => *id,
        }
    }
}

pub enum CodegenStatus {
    Idle,
    Pending,
    Done,
    Error(anyhow::Error),
}

/// Stored information that can be used to resurrect a context crease when creating an editor for a past message.
#[derive(Clone, Debug)]
struct MessageCrease {
    range: Range<MultiBufferOffset>,
    icon_path: SharedString,
    label: SharedString,
}

fn extract_message_creases(
    editor: &mut Editor,
    mention_set: &Entity<MentionSet>,
    window: &mut Window,
    cx: &mut Context<'_, Editor>,
) -> Vec<MessageCrease> {
    let creases = mention_set.read(cx).creases();
    let snapshot = editor.snapshot(window, cx);
    snapshot
        .crease_snapshot
        .creases()
        .filter(|(id, _)| creases.contains(id))
        .filter_map(|(_, crease)| {
            let metadata = crease.metadata()?.clone();
            Some(MessageCrease {
                range: crease.range().to_offset(snapshot.buffer()),
                label: metadata.label,
                icon_path: metadata.icon_path,
            })
        })
        .collect()
}

fn insert_message_creases(
    editor: &mut Editor,
    message_creases: &[MessageCrease],
    window: &mut Window,
    cx: &mut Context<'_, Editor>,
) -> Vec<CreaseId> {
    let buffer_snapshot = editor.buffer().read(cx).snapshot(cx);
    let creases = message_creases
        .iter()
        .map(|crease| {
            let start = buffer_snapshot.anchor_after(crease.range.start);
            let end = buffer_snapshot.anchor_before(crease.range.end);
            crease_for_mention(
                crease.label.clone(),
                crease.icon_path.clone(),
                None,
                None,
                None,
                start..end,
                cx.weak_entity(),
            )
        })
        .collect::<Vec<_>>();
    let ids = editor.insert_creases(creases.clone(), cx);
    editor.fold_creases(creases, false, window, cx);
    ids
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::terminal_codegen::TerminalCodegen;
    use agent::ThreadStore;
    use collections::VecDeque;
    use fs::FakeFs;
    use gpui::{ClipboardItem, TestAppContext, VisualTestContext};
    use language::Buffer;
    use project::Project;
    use settings::SettingsStore;
    use std::cell::RefCell;
    use std::path::{Path, PathBuf};
    use std::rc::Rc;
    use terminal::TerminalBuilder;
    use terminal::terminal_settings::CursorShape;
    use util::path;
    use util::paths::PathStyle;
    use uuid::Uuid;

    fn init_test(cx: &mut TestAppContext) {
        cx.update(|cx| {
            let settings_store = SettingsStore::test(cx);
            cx.set_global(settings_store);
            theme::init(theme::LoadThemes::JustBase, cx);
            theme_settings::init(theme::LoadThemes::JustBase, cx);
            editor::init(cx);
            release_channel::init(semver::Version::new(0, 0, 0), cx);
            language_model::LanguageModelRegistry::test(cx);
            prompt_store::init(cx);
        });
    }

    fn build_terminal_prompt_editor(
        workspace: &Entity<Workspace>,
        cx: &mut VisualTestContext,
    ) -> Entity<PromptEditor<TerminalCodegen>> {
        let thread_store = cx.update(|_window, cx| cx.new(|cx| ThreadStore::new(cx)));
        let fs = FakeFs::new(cx.executor());

        let terminal = cx.update(|_window, cx| {
            cx.new(|cx| {
                TerminalBuilder::new_display_only(
                    CursorShape::default(),
                    settings::AlternateScroll::On,
                    None,
                    0,
                    cx.background_executor(),
                    PathStyle::local(),
                )
                .subscribe(cx)
            })
        });

        let session_id = Uuid::new_v4();
        let codegen =
            cx.update(|_window, cx| cx.new(|_| TerminalCodegen::new(terminal, session_id)));

        let prompt_buffer = cx.update(|_window, cx| {
            cx.new(|cx| MultiBuffer::singleton(cx.new(|cx| Buffer::local("", cx)), cx))
        });

        let project = workspace.update(cx, |workspace, _cx| workspace.project().downgrade());

        cx.update(|window, cx| {
            cx.new(|cx| {
                PromptEditor::new_terminal(
                    TerminalInlineAssistId::default(),
                    VecDeque::new(),
                    prompt_buffer,
                    codegen,
                    session_id,
                    fs,
                    thread_store,
                    project,
                    workspace.downgrade(),
                    window,
                    cx,
                )
            })
        })
    }

    #[gpui::test]
    async fn test_secondary_confirm_emits_execute_true_in_terminal_mode(cx: &mut TestAppContext) {
        init_test(cx);

        let fs = FakeFs::new(cx.executor());
        fs.insert_tree("/project", serde_json::json!({"file": ""}))
            .await;
        let project = Project::test(fs, [Path::new(path!("/project"))], cx).await;
        let (workspace, cx) =
            cx.add_window_view(|window, cx| Workspace::test_new(project.clone(), window, cx));

        let prompt_editor = build_terminal_prompt_editor(&workspace, cx);

        // Set the codegen status to Done so that confirm logic emits ConfirmRequested.
        prompt_editor.update(cx, |editor, cx| {
            editor.codegen().update(cx, |codegen, _| {
                codegen.status = CodegenStatus::Done;
            });
            editor.edited_since_done = false;
        });

        let events: Rc<RefCell<Vec<PromptEditorEvent>>> = Rc::new(RefCell::new(Vec::new()));
        let events_clone = events.clone();
        cx.update(|_window, cx| {
            cx.subscribe(&prompt_editor, move |_, event: &PromptEditorEvent, _cx| {
                events_clone.borrow_mut().push(match event {
                    PromptEditorEvent::ConfirmRequested { execute } => {
                        PromptEditorEvent::ConfirmRequested { execute: *execute }
                    }
                    PromptEditorEvent::StartRequested => PromptEditorEvent::StartRequested,
                    PromptEditorEvent::StopRequested => PromptEditorEvent::StopRequested,
                    PromptEditorEvent::CancelRequested => PromptEditorEvent::CancelRequested,
                    PromptEditorEvent::Resized { height_in_lines } => PromptEditorEvent::Resized {
                        height_in_lines: *height_in_lines,
                    },
                });
            })
            .detach();
        });

        // Dispatch menu::SecondaryConfirm (cmd-enter).
        prompt_editor.update(cx, |editor, cx| {
            editor.handle_confirm(true, cx);
        });

        let events = events.borrow();
        assert_eq!(events.len(), 1, "Expected exactly one event");
        assert!(
            matches!(
                events[0],
                PromptEditorEvent::ConfirmRequested { execute: true }
            ),
            "Expected ConfirmRequested with execute: true, got {:?}",
            match &events[0] {
                PromptEditorEvent::ConfirmRequested { execute } =>
                    format!("ConfirmRequested {{ execute: {} }}", execute),
                _ => "other event".to_string(),
            }
        );
    }

    #[gpui::test]
    async fn test_text_changed_after_result_is_sent_as_refinement(cx: &mut TestAppContext) {
        init_test(cx);

        let fs = FakeFs::new(cx.executor());
        fs.insert_tree("/project", serde_json::json!({"file": ""}))
            .await;
        let project = Project::test(fs, [Path::new(path!("/project"))], cx).await;
        let (workspace, cx) =
            cx.add_window_view(|window, cx| Workspace::test_new(project.clone(), window, cx));
        let prompt_editor = build_terminal_prompt_editor(&workspace, cx);

        let set_prompt = |text: &str, cx: &mut VisualTestContext| {
            prompt_editor.update_in(cx, |prompt_editor, window, cx| {
                prompt_editor
                    .editor
                    .update(cx, |editor, cx| editor.set_text(text, window, cx));
            });
            cx.run_until_parked();
        };
        let finish = |cx: &mut VisualTestContext| {
            prompt_editor.update(cx, |prompt_editor, cx| {
                prompt_editor.codegen().update(cx, |codegen, cx| {
                    codegen.status = CodegenStatus::Done;
                    cx.notify();
                });
            });
            cx.run_until_parked();
        };

        // The first instruction is sent alone.
        set_prompt("list the files", cx);
        prompt_editor.update(cx, |prompt_editor, cx| {
            prompt_editor.request_start(cx);
            assert_eq!(prompt_editor.request_prompt(cx), "list the files");
        });

        // With its result on screen, the box offers Keep and Undo.
        finish(cx);
        prompt_editor.update(cx, |prompt_editor, cx| {
            assert_eq!(prompt_editor.box_state(cx), BoxState::Review);
        });

        // A changed text is a refinement: both instructions go, in order.
        set_prompt("sorted by size", cx);
        prompt_editor.update(cx, |prompt_editor, cx| {
            assert_eq!(prompt_editor.box_state(cx), BoxState::Compose);
            prompt_editor.request_start(cx);
            let request = prompt_editor.request_prompt(cx);
            assert!(request.contains("1. list the files"), "{request}");
            assert!(request.contains("2. sorted by size"), "{request}");
        });

        // Sending the same text again does not stack it on itself.
        finish(cx);
        set_prompt("sorted by size ", cx);
        prompt_editor.update(cx, |prompt_editor, cx| {
            prompt_editor.request_start(cx);
            assert_eq!(
                prompt_editor.follow_up_to,
                vec!["list the files".to_string()]
            );
        });
    }

    #[gpui::test]
    async fn test_confirm_emits_execute_false_in_terminal_mode(cx: &mut TestAppContext) {
        init_test(cx);

        let fs = FakeFs::new(cx.executor());
        fs.insert_tree("/project", serde_json::json!({"file": ""}))
            .await;
        let project = Project::test(fs, [Path::new(path!("/project"))], cx).await;
        let (workspace, cx) =
            cx.add_window_view(|window, cx| Workspace::test_new(project.clone(), window, cx));

        let prompt_editor = build_terminal_prompt_editor(&workspace, cx);

        prompt_editor.update(cx, |editor, cx| {
            editor.codegen().update(cx, |codegen, _| {
                codegen.status = CodegenStatus::Done;
            });
            editor.edited_since_done = false;
        });

        let events: Rc<RefCell<Vec<PromptEditorEvent>>> = Rc::new(RefCell::new(Vec::new()));
        let events_clone = events.clone();
        cx.update(|_window, cx| {
            cx.subscribe(&prompt_editor, move |_, event: &PromptEditorEvent, _cx| {
                events_clone.borrow_mut().push(match event {
                    PromptEditorEvent::ConfirmRequested { execute } => {
                        PromptEditorEvent::ConfirmRequested { execute: *execute }
                    }
                    PromptEditorEvent::StartRequested => PromptEditorEvent::StartRequested,
                    PromptEditorEvent::StopRequested => PromptEditorEvent::StopRequested,
                    PromptEditorEvent::CancelRequested => PromptEditorEvent::CancelRequested,
                    PromptEditorEvent::Resized { height_in_lines } => PromptEditorEvent::Resized {
                        height_in_lines: *height_in_lines,
                    },
                });
            })
            .detach();
        });

        // Dispatch menu::Confirm (enter) — should emit execute: false even in terminal mode.
        prompt_editor.update(cx, |editor, cx| {
            editor.handle_confirm(false, cx);
        });

        let events = events.borrow();
        assert_eq!(events.len(), 1, "Expected exactly one event");
        assert!(
            matches!(
                events[0],
                PromptEditorEvent::ConfirmRequested { execute: false }
            ),
            "Expected ConfirmRequested with execute: false"
        );
    }

    #[gpui::test]
    async fn test_paste_image_does_not_paste_file_paths_as_text(cx: &mut TestAppContext) {
        let mut png = std::io::Cursor::new(Vec::new());
        image::DynamicImage::new_rgb8(1, 1)
            .write_to(&mut png, image::ImageFormat::Png)
            .expect("encode png");
        let prompt = paste_into_prompt_editor(
            vec![
                gpui::ClipboardEntry::Image(gpui::Image::from_bytes(
                    gpui::ImageFormat::Png,
                    png.into_inner(),
                )),
                gpui::ClipboardEntry::ExternalPaths(gpui::ExternalPaths(
                    vec![PathBuf::from(path!("/tmp/screenshot.png"))].into(),
                )),
            ],
            cx,
        )
        .await;

        let image_uri = acp_thread::MentionUri::PastedImage {
            name: "Image".into(),
        };
        assert_eq!(prompt, format!("{} ", image_uri.as_link()));
    }

    #[gpui::test]
    async fn test_paste_non_image_file_pastes_path_as_text(cx: &mut TestAppContext) {
        let path = PathBuf::from(path!("/tmp/notes.txt"));
        let prompt = paste_into_prompt_editor(
            vec![gpui::ClipboardEntry::ExternalPaths(gpui::ExternalPaths(
                vec![path.clone()].into(),
            ))],
            cx,
        )
        .await;

        assert_eq!(prompt, path.display().to_string());
    }

    async fn paste_into_prompt_editor(
        entries: Vec<gpui::ClipboardEntry>,
        cx: &mut TestAppContext,
    ) -> String {
        init_test(cx);
        cx.update(|cx| {
            LanguageModelRegistry::global(cx).update(cx, |registry, cx| {
                let mut model = registry.default_model().expect("fake default model");
                model.supports_images = true;
                registry.set_inline_assistant_model(Some(model), cx);
            });
        });

        let fs = FakeFs::new(cx.executor());
        fs.insert_tree("/project", serde_json::json!({"file": ""}))
            .await;
        let project = Project::test(fs.clone(), [Path::new(path!("/project"))], cx).await;
        let (workspace, _) =
            cx.add_window_view(|window, cx| Workspace::test_new(project.clone(), window, cx));
        let workspace = workspace.downgrade();
        let (prompt_editor, cx) = cx.add_window_view(|window, cx| {
            let terminal = cx.new(|cx| {
                TerminalBuilder::new_display_only(
                    CursorShape::default(),
                    settings::AlternateScroll::On,
                    None,
                    0,
                    cx.background_executor(),
                    PathStyle::local(),
                )
                .subscribe(cx)
            });
            let session_id = Uuid::new_v4();
            let codegen = cx.new(|_| TerminalCodegen::new(terminal, session_id));
            let prompt_buffer =
                cx.new(|cx| MultiBuffer::singleton(cx.new(|cx| Buffer::local("", cx)), cx));
            PromptEditor::new_terminal(
                TerminalInlineAssistId::default(),
                VecDeque::new(),
                prompt_buffer,
                codegen,
                session_id,
                fs,
                cx.new(|cx| ThreadStore::new(cx)),
                project.downgrade(),
                workspace,
                window,
                cx,
            )
        });

        cx.write_to_clipboard(ClipboardItem { entries });
        prompt_editor.update_in(cx, |prompt_editor, window, cx| {
            prompt_editor.editor.focus_handle(cx).focus(window, cx);
        });
        cx.dispatch_action(Paste);
        cx.run_until_parked();

        prompt_editor.read_with(cx, |prompt_editor, cx| prompt_editor.prompt(cx))
    }
}

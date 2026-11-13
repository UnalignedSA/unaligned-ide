mod agent_configuration;
pub mod agent_connection_store;
mod agent_diff;
// Upstream's model selector of the inline prompt: no caller since the inline
// box got the chat's picker (wave 4). Kept for merges.
#[allow(dead_code)]
mod agent_model_selector;
mod agent_panel;
mod agent_registry_ui;
mod buffer_codegen;
mod chat_history;
mod completion_provider;
mod config_options;
mod connect_form;
#[allow(
    dead_code,
    reason = "`connected_count` has no reader yet: the model picker and the status bar are built on it next"
)]
mod connections;
mod context;
mod context_server_configuration;
pub(crate) mod conversation_view;
mod diagnostics;
pub mod draft_prompt_store;
mod entry_view_state;
mod external_source_prompt;
mod favorite_models;
mod inline_assistant;
mod inline_prompt_editor;
mod language_model_selector;
mod mention_set;
mod message_editor;
mod mode_selector;
mod model_search;
mod model_selector;
mod model_selector_popover;
mod profile_selector;
mod terminal_codegen;
mod terminal_inline_assistant;
pub mod terminal_thread_metadata_store;
#[cfg(any(test, feature = "test-support"))]
pub mod test_support;
mod thread_import;
pub mod thread_metadata_store;
pub mod thread_worktree_archive;

pub mod threads_archive_view;
mod ui;
mod unicode_confusables;

use std::rc::Rc;
use std::sync::Arc;

use ::ui::IconName;
use agent_client_protocol::v2::schema as acp_v2;
use agent_settings::{AgentProfileId, AgentSettings};
use anyhow::Context as _;
use command_palette_hooks::CommandPaletteFilter;
use editor::{Editor, scroll::Autoscroll};
use feature_flags::FeatureFlagAppExt as _;
use fs::Fs;
use gpui::{
    Action, App, Context, Entity, ImageSource, ReadGlobal as _, Resource, SharedString, SharedUri,
    TaskExt, Window, actions,
};
use language::{
    LanguageRegistry,
    language_settings::{AllLanguageSettings, EditPredictionProvider},
};
use language_model::{
    LanguageModel, LanguageModelId, LanguageModelProviderId, LanguageModelRegistry,
};
use project::{AgentId, DisableAiSettings, Project, ProjectPath};
use prompt_store::{self, PromptBuilder, rules_to_skills_migration};
use rope::Point;
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use settings::{
    LanguageModelSelection, Settings as _, SettingsStore, SidebarSide, ToolPermissionMode,
    update_settings_file,
};
use std::any::TypeId;
use std::path::{Path, PathBuf};
use util::{
    paths::SanitizedPath,
    rel_path::{RelPath, RelPathBuf},
};
use workspace::Workspace;

use crate::agent_configuration::ManageProfilesModal;
pub use crate::agent_connection_store::{ActiveAcpConnection, AgentConnectionStore};
pub use crate::agent_panel::{
    AgentPanel, AgentPanelEvent, AgentPanelTerminalInfo, TerminalId, ThreadTitleRegenerationResult,
};
use crate::agent_registry_ui::AgentRegistryPage;
pub use crate::inline_assistant::InlineAssistant;
pub use crate::message_editor::MessageEditorEvent;
pub use crate::thread_metadata_store::ThreadId;
pub use agent_diff::{AgentDiffPane, AgentDiffToolbar};
use app_actions;
pub use app_actions::{CreateWorktree, NewWorktreeBranchTarget, SwitchWorktree};
pub use conversation_view::open_markdown_in_workspace;
pub use conversation_view::{ConversationView, StateChange};
pub use external_source_prompt::ExternalSourcePrompt;
pub(crate) use mode_selector::ModeSelector;
pub(crate) use model_selector::ModelSelector;
pub(crate) use model_selector_popover::ModelSelectorPopover;
pub use thread_import::{
    AcpThreadImportOnboarding, CrossChannelImportOnboarding, ThreadImportModal,
    channels_with_threads, import_threads_from_other_channels,
};

pub(crate) fn resolve_agent_image(
    dest_url: &str,
    worktree_roots: &[PathBuf],
) -> Option<ImageSource> {
    if dest_url.starts_with("http://") || dest_url.starts_with("https://") {
        return Some(ImageSource::Resource(Resource::Uri(SharedUri::from(
            dest_url.to_string(),
        ))));
    }

    let path = Path::new(dest_url);
    if path.is_absolute() && path.exists() {
        return Some(ImageSource::Resource(Resource::Path(Arc::from(path))));
    }

    for root in worktree_roots {
        let absolute_path = root.join(dest_url);
        if absolute_path.exists() {
            return Some(ImageSource::Resource(Resource::Path(Arc::from(
                absolute_path.as_path(),
            ))));
        }
    }

    None
}

pub(crate) fn project_path_for_file_link(
    project: &Project,
    path: &Path,
    cx: &App,
) -> Option<ProjectPath> {
    let path_style = project.path_style(cx);
    let validate_path = |project_path: ProjectPath, relative_path: &str| {
        if project.get_open_buffer(&project_path, cx).is_none()
            && !project.entry_for_path(&project_path, cx)?.is_file()
        {
            return None;
        }
        let worktree = project.worktree_for_id(project_path.worktree_id, cx)?;
        let worktree = worktree.read(cx);
        let mut prefix = RelPathBuf::new();
        for component in relative_path.split(path_style.separators_ch()) {
            match component {
                "" | "." => {}
                ".." => {
                    let entry = worktree.entry_for_path(&prefix)?;
                    if !entry.is_dir() || entry.canonical_path.is_some() || !prefix.pop() {
                        return None;
                    }
                }
                component => prefix.push_component(component).ok()?,
            }
        }
        Some(project_path)
    };
    let path = SanitizedPath::new(path).as_path();
    if path_style.is_absolute(path) {
        let project_path = project.project_path_for_absolute_path(path, cx)?;
        let worktree = project.worktree_for_id(project_path.worktree_id, cx)?;
        let root = worktree.read(cx).abs_path();
        let root_length = root
            .to_str()?
            .trim_end_matches(path_style.separators_ch())
            .len();
        let relative_path = path
            .to_str()?
            .get(root_length..)?
            .trim_start_matches(path_style.separators_ch());
        validate_path(project_path, relative_path)
    } else {
        project.visible_worktrees(cx).find_map(|worktree| {
            let worktree = worktree.read(cx);
            [
                path.strip_prefix(worktree.root_name().as_std_path()).ok(),
                Some(path),
            ]
            .into_iter()
            .flatten()
            .find_map(|relative_path| {
                let path = RelPath::new(relative_path, path_style).ok()?.into_arc();
                validate_path(
                    ProjectPath {
                        worktree_id: worktree.id(),
                        path,
                    },
                    relative_path.to_str()?,
                )
            })
        })
    }
}

/// Opens `abs_path` in the workspace, moving the cursor to `point` when one
/// is given. Paths outside every worktree are only opened when a file exists
/// there, so broken agent links don't create empty buffers.
pub(crate) fn open_abs_path_at_point(
    workspace: &mut Workspace,
    abs_path: PathBuf,
    point: Option<Point>,
    window: &mut Window,
    cx: &mut Context<Workspace>,
) {
    let project_path = workspace.project().read_with(cx, |project, cx| {
        project_path_for_file_link(project, &abs_path, cx)
    });
    let project = workspace.project().downgrade();
    let workspace = cx.weak_entity();
    window
        .spawn(cx, async move |cx| {
            let task = if let Some(project_path) = project_path {
                workspace
                    .update_in(cx, |workspace, window, cx| {
                        workspace.open_path(project_path, None, true, window, cx)
                    })
                    .ok()
            } else {
                let path = abs_path
                    .to_str()
                    .context("file link path is not valid UTF-8")?;
                let Ok(task) =
                    project.update(cx, |project, cx| project.resolve_abs_file_link(path, cx))
                else {
                    return Ok(());
                };
                let Some(resolved_path) = task
                    .await
                    .with_context(|| format!("resolving agent file link {abs_path:?}"))?
                else {
                    log::warn!(
                        "Could not resolve agent file link to {abs_path:?}: no matching file"
                    );
                    return Ok(());
                };
                workspace
                    .update_in(cx, |workspace, window, cx| {
                        workspace.open_resolved_path(resolved_path, window, cx)
                    })
                    .ok()
            };
            let Some(task) = task else {
                return Ok(());
            };
            let item = task.await?;
            let Some(point) = point else {
                return Ok(());
            };
            let Some(editor) = item.downcast::<Editor>() else {
                return Ok(());
            };
            editor
                .update_in(cx, |editor, window, cx| {
                    if let Some(buffer) = editor.buffer().read(cx).as_singleton() {
                        let point = buffer
                            .read(cx)
                            .snapshot()
                            .point_from_external_input(point.row, point.column);
                        editor.go_to_singleton_buffer_point(point, window, cx);
                        editor.request_autoscroll(Autoscroll::center(), cx);
                    }
                })
                .ok();
            anyhow::Ok(())
        })
        .detach_and_log_err(cx);
}

pub const DEFAULT_THREAD_TITLE: &str = "New Agent Thread";

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum AgentThreadSource {
    AgentPanel,
    GitPanel,
    Sidebar,
}

impl AgentThreadSource {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::AgentPanel => "agent_panel",
            Self::GitPanel => "git_panel",
            Self::Sidebar => "sidebar",
        }
    }
}

/// The panel is one simple chat, like Cursor's side panel: one agent, two
/// modes, a model selector. Everything this gates (external agents, the agent
/// registry, profile management, sign-in, plans and upsells, the threads
/// sidebar entry) stays compiled and can be brought back by returning `false`.
pub(crate) fn simple_mode() -> bool {
    true
}

/// Whether external agents (Claude Code, Codex, Gemini CLI and any agent from
/// the `agent_servers` setting) are offered: as rows of the connection
/// catalog, as a choice when starting a chat, and in the command palette.
/// They are how a Claude or Gemini subscription is used, so this is on.
pub(crate) fn show_external_agents() -> bool {
    true
}

/// Whether agent profiles can be created and edited from the panel. Off: the
/// panel has two fixed modes, Agent and Ask.
pub(crate) fn show_profile_management() -> bool {
    false
}

/// Whether anything that needs a Zed account is shown: sign-in, plans, trial
/// and upgrade offers. Off: no Zed service stands behind this product.
pub(crate) fn show_zed_account_ui() -> bool {
    false
}

/// Whether the agent panel offers the threads sidebar. Off: the panel is one
/// chat with its history in a popover.
pub(crate) fn show_threads_sidebar_entry() -> bool {
    false
}

/// The recommended provider: first in the connection screen and in the model
/// list. Any other provider can be connected next to it, or instead of it.
pub(crate) use language_models::UNALIGNED_PROVIDER_ID;

/// The external agents that bring their own subscription: id in the public
/// agent registry, the name the product shows, icon. The single source for
/// their names: the registry and the agents themselves report other ones
/// ("Claude Agent" for `claude-acp`).
pub(crate) const SUBSCRIPTION_AGENTS: &[(&str, &str, IconName)] = &[
    (
        agent_servers::CLAUDE_AGENT_ID,
        "Claude Code",
        IconName::AiClaude,
    ),
    (agent_servers::CODEX_ID, "Codex", IconName::AiOpenAi),
    (agent_servers::GEMINI_ID, "Gemini CLI", IconName::AiGemini),
];

/// The name an agent is shown under, everywhere in the chat.
///
/// The three subscription agents always get the product's own name, whatever
/// the agent or the public registry reports. Any other agent gets the name it
/// reports, or its id when it reports none.
pub(crate) fn agent_display_name(
    agent_id: &AgentId,
    reported: Option<SharedString>,
) -> SharedString {
    let id: &str = agent_id.0.as_ref();
    if let Some((_, name, _)) = SUBSCRIPTION_AGENTS
        .iter()
        .find(|(known_id, _, _)| *known_id == id)
    {
        return SharedString::new_static(name);
    }
    reported.unwrap_or_else(|| agent_id.0.clone())
}

pub(crate) fn agent_sidebar_side(cx: &App) -> &'static str {
    match AgentSettings::get_global(cx).sidebar_side() {
        SidebarSide::Left => "left",
        SidebarSide::Right => "right",
    }
}

actions!(
    agent,
    [
        /// Toggles the "New chat with" chooser: who answers a new chat.
        ToggleNewThreadMenu,
        /// Toggles History: the past chats of this project.
        ToggleHistory,
        /// Toggles the options menu for agent settings and preferences.
        ToggleOptionsMenu,
        /// Opens the provider picker: connect Unaligned, another provider, a
        /// local model or a custom endpoint.
        ManageProviders,
        /// Toggles the profile or mode selector for switching between agent profiles.
        ToggleProfileSelector,
        /// Cycles through available session modes.
        CycleModeSelector,
        /// Cycles through favorited models in the ACP model selector.
        CycleFavoriteModels,
        /// Expands the message editor to full size.
        ExpandMessageEditor,
        /// Archives the currently selected thread.
        ArchiveSelectedThread,
        /// Removes the currently selected thread.
        RemoveSelectedThread,
        /// Renames the currently selected thread.
        RenameSelectedThread,
        /// Starts a chat conversation with follow-up enabled.
        ChatWithFollow,
        /// Cycles to the next inline assist suggestion.
        CycleNextInlineAssist,
        /// Cycles to the previous inline assist suggestion.
        CyclePreviousInlineAssist,
        /// Moves focus up in the interface.
        FocusUp,
        /// Moves focus down in the interface.
        FocusDown,
        /// Moves focus left in the interface.
        FocusLeft,
        /// Moves focus right in the interface.
        FocusRight,
        /// Opens the active thread as a markdown file.
        OpenActiveThreadAsMarkdown,
        /// Opens the agent diff view to review changes.
        OpenAgentDiff,
        /// Copies the current thread to the clipboard as JSON for debugging.
        CopyThreadToClipboard,
        /// Loads a thread from the clipboard JSON for debugging.
        LoadThreadFromClipboard,
        /// Reruns the rules-to-skills migration.
        RerunRulesToSkillsMigration,
        /// Keeps the current suggestion or change.
        Keep,
        /// Rejects the current suggestion or change.
        Reject,
        /// Rejects all suggestions or changes.
        RejectAll,
        /// Undoes the most recent reject operation, restoring the rejected changes.
        UndoLastReject,
        /// Keeps all suggestions or changes.
        KeepAll,
        /// Allow this operation only this time.
        AllowOnce,
        /// Allow this operation and remember the choice.
        AllowAlways,
        /// Reject this operation only this time.
        RejectOnce,
        /// Follows the agent's suggestions.
        Follow,
        /// Resets the trial upsell notification.
        ResetTrialUpsell,
        /// Resets the trial end upsell notification.
        ResetTrialEndUpsell,
        /// Re-enables the fast mode warning for every provider and model.
        ResetFastModeWarnings,
        /// Opens the "Add Context" menu in the message editor.
        OpenAddContextMenu,
        /// Interrupts the current generation and sends the message immediately.
        SendImmediately,
        /// Sends the next queued message immediately.
        SendNextQueuedMessage,
        /// Removes the first message from the queue (the next one to be sent).
        RemoveFirstQueuedMessage,
        /// Edits the first message in the queue (the next one to be sent).
        EditFirstQueuedMessage,
        /// Toggles steering for the first queued message: when on, it interrupts
        /// the agent at its next step instead of waiting for it to finish.
        ToggleSteerFirstQueuedMessage,
        /// Clears all messages from the queue.
        ClearMessageQueue,
        /// Opens the permission granularity dropdown for the current tool call.
        OpenPermissionDropdown,
        /// Toggles thinking mode for models that support extended thinking.
        ToggleThinkingMode,
        /// Cycles through available thinking effort levels for the current model.
        CycleThinkingEffort,
        /// Toggles the thinking effort selector menu open or closed.
        ToggleThinkingEffortMenu,
        /// Toggles fast mode for models that support it.
        ToggleFastMode,
        /// Scroll the output by one page up.
        ScrollOutputPageUp,
        /// Scroll the output by one page down.
        ScrollOutputPageDown,
        /// Scroll the output up by three lines.
        ScrollOutputLineUp,
        /// Scroll the output down by three lines.
        ScrollOutputLineDown,
        /// Scroll the output to the top.
        ScrollOutputToTop,
        /// Scroll the output to the bottom.
        ScrollOutputToBottom,
        /// Scroll the output to the previous user message.
        ScrollOutputToPreviousMessage,
        /// Scroll the output to the next user message.
        ScrollOutputToNextMessage,
        /// Toggles in-thread search over the current agent thread's contents.
        ToggleSearch,
        /// Import agent threads from the other release channels (e.g. Preview, Nightly).
        ImportThreadsFromOtherChannels,
        /// Starts a new terminal thread.
        NewTerminalThread,
    ]
);

actions!(
    dev,
    [
        /// Shows metadata for the currently active thread.
        ShowThreadMetadata,
        /// Shows metadata for all threads in the sidebar.
        ShowAllSidebarThreadMetadata,
    ]
);

/// Action to authorize a tool call with a specific permission option.
/// This is used by the permission granularity dropdown to authorize tool calls.
#[derive(Clone, PartialEq, Deserialize, JsonSchema, Action)]
#[action(namespace = agent)]
#[serde(deny_unknown_fields)]
pub struct AuthorizeToolCall {
    /// The tool call ID to authorize.
    pub tool_call_id: String,
    #[serde(default)]
    #[schemars(with = "Option<String>")]
    pub request_id: Option<acp_thread::PermissionRequestId>,
    #[serde(default)]
    pub session_id: Option<String>,
    /// The permission option ID to use.
    pub option_id: String,
    /// The kind of permission option (serialized as string).
    pub option_kind: String,
}

/// Action to select a permission granularity option from the dropdown.
/// This updates the selected granularity without triggering authorization.
#[derive(Clone, PartialEq, Deserialize, JsonSchema, Action)]
#[action(namespace = agent)]
#[serde(deny_unknown_fields)]
pub struct SelectPermissionGranularity {
    /// The tool call ID for which to select the granularity.
    pub tool_call_id: String,
    #[serde(default)]
    #[schemars(with = "Option<String>")]
    pub request_id: Option<acp_thread::PermissionRequestId>,
    #[serde(default)]
    pub session_id: Option<String>,
    /// The index of the selected granularity option.
    pub index: usize,
}

/// Action to toggle a command pattern checkbox in the permission dropdown.
#[derive(Clone, PartialEq, Deserialize, JsonSchema, Action)]
#[action(namespace = agent)]
#[serde(deny_unknown_fields)]
pub struct ToggleCommandPattern {
    /// The tool call ID for which to toggle the pattern.
    pub tool_call_id: String,
    #[serde(default)]
    #[schemars(with = "Option<String>")]
    pub request_id: Option<acp_thread::PermissionRequestId>,
    #[serde(default)]
    pub session_id: Option<String>,
    /// The index of the command pattern to toggle.
    pub pattern_index: usize,
}

/// Creates a new conversation thread, optionally based on an existing thread.
#[derive(Default, Clone, PartialEq, Deserialize, JsonSchema, Action)]
#[action(namespace = agent)]
#[serde(deny_unknown_fields)]
pub struct NewThread;

/// Starts a new chat answered by an external agent (Claude Code, Codex,
/// Gemini CLI or any agent of the `agent_servers` setting). Without an agent,
/// opens the list of agents to choose from.
#[derive(Clone, Default, PartialEq, Deserialize, JsonSchema, Action)]
#[action(namespace = agent)]
#[serde(deny_unknown_fields)]
pub struct NewExternalAgentThread {
    /// The agent id to use for the conversation. When absent, the chooser of
    /// the chat panel opens instead.
    #[serde(default, deserialize_with = "deserialize_external_agent_id")]
    agent: Option<AgentId>,
}

impl NewExternalAgentThread {
    /// A new chat with this agent.
    pub fn with_agent(agent: AgentId) -> Self {
        Self { agent: Some(agent) }
    }

    /// The agent to start the chat with; `None` opens the chooser.
    pub fn agent(&self) -> Option<&AgentId> {
        self.agent.as_ref()
    }
}

fn deserialize_external_agent_id<'de, D>(deserializer: D) -> Result<Option<AgentId>, D::Error>
where
    D: serde::Deserializer<'de>,
{
    #[derive(Deserialize)]
    #[serde(untagged)]
    enum AgentIdOrLegacyAgent {
        LegacyAgent(Agent),
        AgentId(AgentId),
    }

    Ok(
        match Option::<AgentIdOrLegacyAgent>::deserialize(deserializer)? {
            None => None,
            Some(AgentIdOrLegacyAgent::AgentId(agent_id)) => Some(agent_id),
            Some(AgentIdOrLegacyAgent::LegacyAgent(Agent::Custom { id })) => Some(id),
            Some(AgentIdOrLegacyAgent::LegacyAgent(Agent::NativeAgent)) => {
                Some(Agent::NativeAgent.id())
            }
            #[cfg(any(test, feature = "test-support"))]
            Some(AgentIdOrLegacyAgent::LegacyAgent(Agent::Stub)) => Some(Agent::Stub.id()),
        },
    )
}

#[derive(Clone, PartialEq, Deserialize, JsonSchema, Action)]
#[action(namespace = agent)]
#[serde(deny_unknown_fields)]
pub struct NewNativeAgentThreadFromSummary {
    from_session_id: acp_v2::SessionId,
}

#[derive(Debug, Default, Clone, PartialEq, Eq, Hash, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
#[non_exhaustive]
pub enum Agent {
    #[default]
    #[serde(alias = "NativeAgent", alias = "TextThread")]
    NativeAgent,
    #[serde(alias = "Custom")]
    Custom {
        #[serde(rename = "name")]
        id: AgentId,
    },
    #[cfg(any(test, feature = "test-support"))]
    Stub,
}

impl From<AgentId> for Agent {
    fn from(id: AgentId) -> Self {
        if id.as_ref() == agent::IDE_AGENT_ID.as_ref() {
            return Self::NativeAgent;
        }
        #[cfg(any(test, feature = "test-support"))]
        if id.as_ref() == "stub" {
            return Self::Stub;
        }
        Self::Custom { id }
    }
}

impl Agent {
    pub fn id(&self) -> AgentId {
        match self {
            Self::NativeAgent => agent::IDE_AGENT_ID.clone(),
            Self::Custom { id } => id.clone(),
            #[cfg(any(test, feature = "test-support"))]
            Self::Stub => "stub".into(),
        }
    }

    pub fn is_native(&self) -> bool {
        matches!(self, Self::NativeAgent)
    }

    pub fn label(&self) -> SharedString {
        match self {
            // Only the label: the id stays "Zed Agent", it is stored with
            // every thread.
            Self::NativeAgent => "Agent".into(),
            Self::Custom { id, .. } => id.0.clone(),
            #[cfg(any(test, feature = "test-support"))]
            Self::Stub => "Stub Agent".into(),
        }
    }

    pub fn icon(&self) -> Option<IconName> {
        match self {
            Self::NativeAgent => None,
            Self::Custom { .. } => Some(IconName::Sparkle),
            #[cfg(any(test, feature = "test-support"))]
            Self::Stub => None,
        }
    }

    pub fn server(
        &self,
        fs: Arc<dyn fs::Fs>,
        thread_store: Entity<agent::ThreadStore>,
    ) -> Rc<dyn agent_servers::AgentServer> {
        match self {
            Self::NativeAgent => Rc::new(agent::NativeAgentServer::new(fs, thread_store)),
            Self::Custom { id: name } => {
                Rc::new(agent_servers::CustomAgentServer::new(name.clone()))
            }
            #[cfg(any(test, feature = "test-support"))]
            Self::Stub => Rc::new(crate::test_support::StubAgentServer::default_response()),
        }
    }
}

/// Content to initialize new external agent with.
pub enum AgentInitialContent {
    ThreadSummary {
        session_id: acp_v2::SessionId,
        title: Option<SharedString>,
    },
    ContentBlock {
        blocks: Vec<acp_v2::ContentBlock>,
        auto_submit: bool,
    },
    FromExternalSource(ExternalSourcePrompt),
}

impl From<ExternalSourcePrompt> for AgentInitialContent {
    fn from(prompt: ExternalSourcePrompt) -> Self {
        Self::FromExternalSource(prompt)
    }
}

/// Opens the profile management interface for configuring agent tools and settings.
#[derive(PartialEq, Clone, Default, Debug, Deserialize, JsonSchema, Action)]
#[action(namespace = agent)]
#[serde(deny_unknown_fields)]
pub struct ManageProfiles {
    #[serde(default)]
    pub customize_tools: Option<AgentProfileId>,
}

impl ManageProfiles {
    pub fn customize_tools(profile_id: AgentProfileId) -> Self {
        Self {
            customize_tools: Some(profile_id),
        }
    }
}

// Upstream type, read only by `agent_model_selector` (see there): kept for merges.
#[allow(dead_code)]
#[derive(Clone)]
pub(crate) enum ModelUsageContext {
    InlineAssistant,
}

#[allow(dead_code)]
impl ModelUsageContext {
    pub fn model(&self, cx: &App) -> Option<LanguageModel> {
        match self {
            Self::InlineAssistant => {
                LanguageModelRegistry::read_global(cx).inline_assistant_model()
            }
        }
    }
}

pub(crate) fn humanize_token_count(count: u64) -> String {
    match count {
        0..=999 => count.to_string(),
        1000..=9999 => {
            let thousands = count / 1000;
            let hundreds = (count % 1000 + 50) / 100;
            if hundreds == 0 {
                format!("{}k", thousands)
            } else if hundreds == 10 {
                format!("{}k", thousands + 1)
            } else {
                format!("{}.{}k", thousands, hundreds)
            }
        }
        10_000..=999_999 => format!("{}k", (count + 500) / 1000),
        1_000_000..=9_999_999 => {
            let millions = count / 1_000_000;
            let hundred_thousands = (count % 1_000_000 + 50_000) / 100_000;
            if hundred_thousands == 0 {
                format!("{}M", millions)
            } else if hundred_thousands == 10 {
                format!("{}M", millions + 1)
            } else {
                format!("{}.{}M", millions, hundred_thousands)
            }
        }
        10_000_000.. => format!("{}M", (count + 500_000) / 1_000_000),
    }
}

/// Initializes the `agent` crate.
pub fn init(
    fs: Arc<dyn Fs>,
    prompt_builder: Arc<PromptBuilder>,
    language_registry: Arc<LanguageRegistry>,
    _is_new_install: bool,
    is_eval: bool,
    cx: &mut App,
) {
    agent::ThreadStore::init_global(cx);
    prompt_store::init(cx);

    cx.set_global(agent_skills::SkillsUpdatedHook(std::rc::Rc::new(|cx| {
        let workspaces: Vec<_> = workspace::AppState::global(cx)
            .workspace_store
            .read(cx)
            .workspaces()
            .cloned()
            .collect();

        for workspace in workspaces {
            workspace
                .update(cx, |workspace, cx| {
                    if let Some(panel) = workspace.panel::<AgentPanel>(cx) {
                        panel.update(cx, |panel, cx| panel.refresh_skills(cx));
                    }
                })
                .ok();
        }
    })));

    if !is_eval {
        // Initializing the language model from the user settings messes with the eval, so we only initialize them when
        // we're not running inside of the eval.
        init_language_model_settings(cx);
        connections::init(cx);
    }
    agent_panel::init(cx);
    context_server_configuration::init(language_registry, fs.clone(), cx);
    thread_metadata_store::init(cx);
    terminal_thread_metadata_store::init(cx);

    inline_assistant::init(fs.clone(), prompt_builder.clone(), cx);
    terminal_inline_assistant::init(fs.clone(), prompt_builder, cx);
    cx.observe_new(|workspace: &mut Workspace, _window, _cx| {
        workspace.register_action(
            move |workspace: &mut Workspace,
                  _: &app_actions::AcpRegistry,
                  window: &mut Window,
                  cx: &mut Context<Workspace>| {
                let existing = workspace
                    .active_pane()
                    .read(cx)
                    .items()
                    .find_map(|item| item.downcast::<AgentRegistryPage>());

                if let Some(existing) = existing {
                    existing.update(cx, |_, cx| {
                        project::AgentRegistryStore::global(cx)
                            .update(cx, |store, cx| store.refresh(cx));
                    });
                    workspace.activate_item(&existing, true, true, window, cx);
                } else {
                    let registry_page = AgentRegistryPage::new(workspace, window, cx);
                    workspace.add_item_to_active_pane(
                        Box::new(registry_page),
                        None,
                        true,
                        window,
                        cx,
                    );
                }
            },
        );
    })
    .detach();
    cx.observe_new(ManageProfilesModal::register).detach();
    cx.observe_new(connect_form::ManageProvidersModal::register)
        .detach();
    cx.observe_new(|workspace: &mut Workspace, _window, _cx| {
        workspace.register_action(
            |workspace: &mut Workspace,
             _: &ImportThreadsFromOtherChannels,
             _window: &mut Window,
             cx: &mut Context<Workspace>| {
                import_threads_from_other_channels(workspace, cx);
            },
        );
    })
    .detach();

    {
        let fs = fs.clone();
        cx.observe_new(move |workspace: &mut Workspace, _window, _cx| {
            let fs = fs.clone();
            workspace.register_action(
                move |workspace: &mut Workspace,
                      _: &RerunRulesToSkillsMigration,
                      window: &mut Window,
                      cx: &mut Context<Workspace>| {
                    rerun_rules_to_skills_migration(workspace, fs.clone(), window, cx);
                },
            );
        })
        .detach();
    }

    // Update command palette filter based on AI settings
    update_command_palette_filter(cx);

    // Watch for settings changes
    cx.observe_global::<SettingsStore>(|app_cx| {
        // When settings change, update the command palette filter
        update_command_palette_filter(app_cx);
    })
    .detach();

    cx.on_flags_ready(|_, cx| {
        update_command_palette_filter(cx);
    })
    .detach();

    // Kick off the one-time migration of non-Default Rules to global
    // Skills. Test builds keep the old feature-flag deferral because
    // server flags are never received in `gpui::test` contexts, avoiding
    // sqlite worker activity that can race with the deterministic scheduler.
    #[cfg(any(test, feature = "test-support"))]
    {
        let fs = fs.clone();
        cx.on_flags_ready(move |_, cx| {
            rules_to_skills_migration::migrate_rules_to_skills_if_needed(fs.clone(), cx);
        })
        .detach();
    }
    #[cfg(not(any(test, feature = "test-support")))]
    {
        rules_to_skills_migration::migrate_rules_to_skills_if_needed(fs, cx);
    }
}

fn rerun_rules_to_skills_migration(
    _workspace: &mut Workspace,
    fs: Arc<dyn Fs>,
    window: &mut Window,
    cx: &mut Context<Workspace>,
) {
    let workspace = cx.weak_entity();
    window
        .spawn(cx, async move |cx| {
            let migration_task = cx.update(|_window, cx| {
                rules_to_skills_migration::rerun_rules_to_skills_migration(fs, cx)
            })?;
            let result = migration_task.await?;
            log::info!("Forced rules-to-skills migration result: {result:?}");

            cx.update(|_window, cx| {
                show_rules_to_skills_migration_toast(
                    &workspace,
                    "Rules-to-skills migration rerun. Please double-check AGENTS.md and Skills for missing or duplicated prompts.",
                    cx,
                );
            })?;
            anyhow::Ok(())
        })
        .detach_and_log_err(cx);
}

fn show_rules_to_skills_migration_toast(
    workspace: &gpui::WeakEntity<Workspace>,
    message: &'static str,
    cx: &mut App,
) {
    if let Some(workspace) = workspace.upgrade() {
        workspace.update(cx, |workspace, cx| {
            struct RulesToSkillsMigrationRerunToast;
            workspace.show_toast(
                workspace::Toast::new(
                    workspace::notifications::NotificationId::unique::<
                        RulesToSkillsMigrationRerunToast,
                    >(),
                    message,
                )
                .autohide(),
                cx,
            );
        });
    }
}

fn update_command_palette_filter(cx: &mut App) {
    let disable_ai = DisableAiSettings::get_global(cx).disable_ai;
    let agent_enabled = AgentSettings::get_global(cx).enabled;

    let edit_prediction_provider = AllLanguageSettings::get_global(cx)
        .edit_predictions
        .provider;

    CommandPaletteFilter::update_global(cx, |filter, _| {
        use editor::actions::{
            AcceptEditPrediction, AcceptNextLineEditPrediction, AcceptNextWordEditPrediction,
            NextEditPrediction, PreviousEditPrediction, ShowEditPrediction, ToggleEditPrediction,
        };
        let edit_prediction_actions = [
            TypeId::of::<AcceptEditPrediction>(),
            TypeId::of::<AcceptNextWordEditPrediction>(),
            TypeId::of::<AcceptNextLineEditPrediction>(),
            TypeId::of::<ShowEditPrediction>(),
            TypeId::of::<NextEditPrediction>(),
            TypeId::of::<PreviousEditPrediction>(),
            TypeId::of::<ToggleEditPrediction>(),
        ];

        let manage_skills_action = [TypeId::of::<app_actions::assistant::ManageSkills>()];
        let skill_creator_actions = [
            TypeId::of::<app_actions::assistant::OpenSkillCreator>(),
            TypeId::of::<app_actions::assistant::CreateSkillFromUrl>(),
        ];

        if disable_ai {
            filter.hide_namespace("agent");
            filter.hide_namespace("agents");
            filter.hide_namespace("assistant");
            filter.hide_namespace("copilot");
            filter.hide_namespace("edit_prediction_onboarding");
            filter.hide_namespace("edit_prediction");

            filter.hide_action_types(&edit_prediction_actions);
            filter.hide_action_types(&[TypeId::of::<app_actions::OpenEditPredictionOnboarding>()]);
        } else {
            if agent_enabled {
                filter.show_namespace("agent");
                filter.show_namespace("agents");
                filter.show_namespace("assistant");
            } else {
                filter.hide_namespace("agent");
                filter.hide_namespace("agents");
                filter.hide_namespace("assistant");
            }

            match edit_prediction_provider {
                EditPredictionProvider::None => {
                    filter.hide_namespace("edit_prediction");
                    filter.hide_namespace("copilot");
                    filter.hide_action_types(&edit_prediction_actions);
                }
                EditPredictionProvider::Copilot => {
                    filter.show_namespace("edit_prediction");
                    filter.show_namespace("copilot");
                    filter.show_action_types(edit_prediction_actions.iter());
                }
                EditPredictionProvider::Zed
                | EditPredictionProvider::Codestral
                | EditPredictionProvider::Ollama
                | EditPredictionProvider::OpenAiCompatibleApi
                | EditPredictionProvider::Mercury => {
                    filter.show_namespace("edit_prediction");
                    filter.hide_namespace("copilot");
                    filter.show_action_types(edit_prediction_actions.iter());
                }
            }

            filter.show_namespace("edit_prediction_onboarding");
            filter.show_action_types(&[TypeId::of::<app_actions::OpenEditPredictionOnboarding>()]);

            filter.show_namespace("multi_workspace");
        }

        // Two fixed modes and no Zed account: the commands that lead to
        // profile management, to Zed's plans or to its other release
        // channels stay out of the palette. They remain bound and usable
        // from a keymap.
        if !show_profile_management() {
            filter.hide_action_types(&[
                TypeId::of::<ManageProfiles>(),
                TypeId::of::<ToggleProfileSelector>(),
            ]);
        }
        if !show_zed_account_ui() {
            filter.hide_action_types(&[
                TypeId::of::<ResetTrialUpsell>(),
                TypeId::of::<ResetTrialEndUpsell>(),
                TypeId::of::<ImportThreadsFromOtherChannels>(),
            ]);
        }
        // The chooser of the chat panel ("New chat with") and the command
        // that starts a chat with one agent exist only while external
        // agents are offered.
        if !show_external_agents() {
            filter.hide_action_types(&[
                TypeId::of::<NewExternalAgentThread>(),
                TypeId::of::<ToggleNewThreadMenu>(),
            ]);
        }
        // The registry page lists every agent of the public registry. The
        // chooser sets up the three subscription agents without it, so the
        // page stays a keymap-only command.
        filter.hide_action_types(&[TypeId::of::<app_actions::AcpRegistry>()]);

        // Skills are surfaced through the settings UI now, so this command
        // should never appear in the palette.
        filter.hide_action_types(&manage_skills_action);
        if !disable_ai {
            filter.show_action_types(skill_creator_actions.iter());
        } else {
            filter.hide_action_types(&skill_creator_actions);
        }
    });
}

fn init_language_model_settings(cx: &mut App) {
    update_active_language_model_from_settings(cx);

    cx.observe_global::<SettingsStore>(update_active_language_model_from_settings)
        .detach();
    cx.subscribe(
        &LanguageModelRegistry::global(cx),
        |registry, event: &language_model::Event, cx| match event {
            language_model::Event::ProviderStateChanged(_)
            | language_model::Event::AddedProvider(_)
            | language_model::Event::RemovedProvider(_)
            | language_model::Event::ProvidersChanged => {
                registry.update(cx, |registry, cx| registry.refresh_fallback_model(cx));
                update_active_language_model_from_settings(cx);
            }
            _ => {}
        },
    )
    .detach();
}

fn update_active_language_model_from_settings(cx: &mut App) {
    let settings = AgentSettings::get_global(cx);

    fn to_selected_model(selection: &LanguageModelSelection) -> language_model::SelectedModel {
        language_model::SelectedModel {
            provider: LanguageModelProviderId::from(selection.provider.0.clone()),
            model: LanguageModelId::from(selection.model.clone()),
        }
    }

    let should_use_fallback = SettingsStore::global(cx)
        .raw_user_settings()
        .and_then(|user| user.content.agent.as_ref())
        .and_then(|agent| agent.default_model.as_ref())
        .is_none();

    let default = settings.default_model.as_ref().map(to_selected_model);
    let inline_assistant = settings
        .inline_assistant_model
        .as_ref()
        .map(to_selected_model);
    let commit_message = settings
        .commit_message_model
        .as_ref()
        .map(to_selected_model);
    let thread_summary = settings
        .thread_summary_model
        .as_ref()
        .map(to_selected_model);
    let compaction = settings.compaction_model.as_ref().map(to_selected_model);
    let inline_alternatives = settings
        .inline_alternatives
        .iter()
        .map(to_selected_model)
        .collect::<Vec<_>>();

    LanguageModelRegistry::global(cx).update(cx, |registry, cx| {
        registry.select_default_model(default.as_ref(), cx);
        registry.select_inline_assistant_model(inline_assistant.as_ref(), cx);
        registry.select_commit_message_model(commit_message.as_ref(), cx);
        registry.select_thread_summary_model(thread_summary.as_ref(), cx);
        registry.select_compaction_model(compaction.as_ref(), cx);
        registry.select_inline_alternative_models(inline_alternatives, cx);
        registry.set_should_use_fallback(should_use_fallback);
    });
}

/// Whether the built-in agent asks before using a tool that no rule covers.
pub(crate) fn asks_before_using_tools(cx: &App) -> bool {
    AgentSettings::get_global(cx).tool_permissions.default != ToolPermissionMode::Allow
}

/// Turns the question before each tool use on or off, for every chat. Off
/// makes "allow" the default answer of every tool. The rules written for one
/// tool, the built-in refusals and the prompts that always ask still apply.
pub(crate) fn set_ask_before_using_tools(ask: bool, fs: Arc<dyn Fs>, cx: &App) {
    update_settings_file(fs, cx, move |settings, _| {
        settings
            .agent
            .get_or_insert_default()
            .tool_permissions
            .get_or_insert_default()
            .default = Some(if ask {
            ToolPermissionMode::Confirm
        } else {
            ToolPermissionMode::Allow
        });
    });
}

#[cfg(test)]
mod tests {
    use super::*;
    use agent_settings::{AgentProfileId, AgentSettings};
    use command_palette_hooks::CommandPaletteFilter;
    use editor::actions::AcceptEditPrediction;
    use gpui::{BorrowAppContext, TestAppContext, px};
    use project::DisableAiSettings;
    use settings::{
        DockPosition, NotifyWhenAgentWaiting, PlaySoundWhenAgentDone, Settings, SettingsStore,
    };

    #[gpui::test]
    fn test_agent_command_palette_visibility(cx: &mut TestAppContext) {
        // Init settings
        cx.update(|cx| {
            let store = SettingsStore::test(cx);
            cx.set_global(store);
            command_palette_hooks::init(cx);
            AgentSettings::register(cx);
            DisableAiSettings::register(cx);
            AllLanguageSettings::register(cx);
        });

        let agent_settings = AgentSettings {
            enabled: true,
            button: true,
            dock: DockPosition::Right,
            flexible: true,
            default_width: px(300.),
            default_height: px(600.),
            max_content_width: Some(px(850.)),
            default_model: None,
            subagent_model: None,
            inline_assistant_model: None,
            inline_assistant_use_streaming_tools: false,
            commit_message_model: None,
            commit_message_include_project_rules: true,
            commit_message_instructions: None,
            thread_summary_model: None,
            compaction_model: None,
            inline_alternatives: vec![],
            favorite_models: vec![],
            default_profile: AgentProfileId::default(),
            profiles: Default::default(),
            notify_when_agent_waiting: NotifyWhenAgentWaiting::default(),
            play_sound_when_agent_done: PlaySoundWhenAgentDone::Never,
            prevent_idle_sleep: true,
            single_file_review: false,
            model_parameters: vec![],
            auto_compact: agent_settings::AutoCompactSettings {
                enabled: false,
                threshold: agent_settings::AutoCompactThreshold::DEFAULT,
            },
            enable_feedback: false,
            expand_edit_card: true,
            expand_terminal_card: true,
            terminal_init_command: None,
            cancel_generation_on_terminal_stop: true,
            use_modifier_to_send: true,
            message_editor_min_lines: 1,
            tool_permissions: Default::default(),
            sandbox_permissions: Default::default(),
            show_turn_stats: false,
            show_merge_conflict_indicator: true,
            auto_generate_thread_titles: true,
            max_idle_retained_threads: 5,
            threads_sidebar: agent_settings::ThreadsSidebarSettings {
                auto_open: true,
                default_width: px(300.),
                position: settings::SidebarDockPosition::Left,
            },
            thinking_display: Default::default(),
        };

        cx.update(|cx| {
            AgentSettings::override_global(agent_settings.clone(), cx);
            DisableAiSettings::override_global(DisableAiSettings { disable_ai: false }, cx);

            // Initial update
            update_command_palette_filter(cx);
        });

        // Assert visible
        cx.update(|cx| {
            let filter = CommandPaletteFilter::try_global(cx).unwrap();
            assert!(
                !filter.is_hidden(&NewThread),
                "NewThread should be visible by default"
            );
            assert!(
                !filter.is_hidden(&NewTerminalThread),
                "NewTerminalThread should be visible by default"
            );
            assert!(
                !filter.is_hidden(&app_actions::assistant::OpenSkillCreator),
                "OpenSkillCreator should be visible by default"
            );
            assert!(
                !filter.is_hidden(&app_actions::assistant::CreateSkillFromUrl),
                "CreateSkillFromUrl should be visible by default"
            );
            assert!(
                !filter.is_hidden(&app_actions::assistant::OpenGlobalAgentsMdRules),
                "OpenGlobalAgentsMdRules should be visible by default"
            );
            assert!(
                !filter.is_hidden(&app_actions::assistant::OpenProjectAgentsMdRules),
                "OpenProjectAgentsMdRules should be visible by default"
            );
            assert!(
                filter.is_hidden(&app_actions::assistant::ManageSkills),
                "ManageSkills should be hidden even when AI is enabled"
            );
            assert!(
                !filter.is_hidden(&NewExternalAgentThread::with_agent(AgentId::from(
                    "claude-acp"
                ))),
                "NewExternalAgentThread should be visible while external agents are offered"
            );
            assert!(
                !filter.is_hidden(&ToggleNewThreadMenu),
                "the chooser's command should be visible while external agents are offered"
            );
            assert!(
                filter.is_hidden(&ToggleProfileSelector)
                    && filter.is_hidden(&ImportThreadsFromOtherChannels)
                    && filter.is_hidden(&app_actions::AcpRegistry),
                "profile, Zed channel and registry commands should stay hidden"
            );
            assert!(
                filter.is_hidden(&ManageProfiles::default()),
                "ManageProfiles should stay hidden"
            );
            assert!(
                filter.is_hidden(&ResetTrialUpsell) && filter.is_hidden(&ResetTrialEndUpsell),
                "trial upsells should stay hidden"
            );
        });

        // Disable agent
        cx.update(|cx| {
            let mut new_settings = agent_settings.clone();
            new_settings.enabled = false;
            AgentSettings::override_global(new_settings, cx);

            // Trigger update
            update_command_palette_filter(cx);
        });

        // Assert hidden
        cx.update(|cx| {
            let filter = CommandPaletteFilter::try_global(cx).unwrap();
            assert!(
                filter.is_hidden(&NewThread),
                "NewThread should be hidden when agent is disabled"
            );
            assert!(
                filter.is_hidden(&NewTerminalThread),
                "NewTerminalThread should be hidden when agent is disabled"
            );
            assert!(
                filter.is_hidden(&app_actions::assistant::OpenGlobalAgentsMdRules),
                "OpenGlobalAgentsMdRules should be hidden when agent is disabled"
            );
            assert!(
                filter.is_hidden(&app_actions::assistant::OpenProjectAgentsMdRules),
                "OpenProjectAgentsMdRules should be hidden when agent is disabled"
            );
        });

        // Test EditPredictionProvider
        // Enable EditPredictionProvider::Copilot
        cx.update(|cx| {
            cx.update_global::<SettingsStore, _>(|store, cx| {
                store.update_user_settings(cx, |s| {
                    s.project
                        .all_languages
                        .edit_predictions
                        .get_or_insert(Default::default())
                        .provider = Some(EditPredictionProvider::Copilot);
                });
            });
            update_command_palette_filter(cx);
        });

        cx.update(|cx| {
            let filter = CommandPaletteFilter::try_global(cx).unwrap();
            assert!(
                !filter.is_hidden(&AcceptEditPrediction),
                "EditPrediction should be visible when provider is Copilot"
            );
        });

        // Disable EditPredictionProvider (None)
        cx.update(|cx| {
            cx.update_global::<SettingsStore, _>(|store, cx| {
                store.update_user_settings(cx, |s| {
                    s.project
                        .all_languages
                        .edit_predictions
                        .get_or_insert(Default::default())
                        .provider = Some(EditPredictionProvider::None);
                });
            });
            update_command_palette_filter(cx);
        });

        cx.update(|cx| {
            let filter = CommandPaletteFilter::try_global(cx).unwrap();
            assert!(
                filter.is_hidden(&AcceptEditPrediction),
                "EditPrediction should be hidden when provider is None"
            );
        });

        // Disable AI entirely
        cx.update(|cx| {
            AgentSettings::override_global(agent_settings.clone(), cx);
            DisableAiSettings::override_global(DisableAiSettings { disable_ai: true }, cx);
            update_command_palette_filter(cx);
        });

        cx.update(|cx| {
            let filter = CommandPaletteFilter::try_global(cx).unwrap();
            assert!(
                filter.is_hidden(&app_actions::assistant::ManageSkills),
                "ManageSkills should be hidden when AI is disabled"
            );
            assert!(
                filter.is_hidden(&app_actions::assistant::OpenSkillCreator),
                "OpenSkillCreator should be hidden when AI is disabled"
            );
            assert!(
                filter.is_hidden(&NewThread),
                "NewThread should be hidden when AI is disabled"
            );
        });
    }

    #[test]
    fn test_deserialize_external_agent_variants() {
        assert_eq!(
            serde_json::from_str::<Agent>(r#""NativeAgent""#).unwrap(),
            Agent::NativeAgent,
        );
        assert_eq!(
            serde_json::from_str::<Agent>(r#"{"Custom":{"name":"my-agent"}}"#).unwrap(),
            Agent::Custom {
                id: "my-agent".into(),
            },
        );
    }

    #[test]
    fn test_deserialize_new_external_agent_thread() {
        let action = serde_json::from_str::<NewExternalAgentThread>(r#"{"agent":"gemini"}"#)
            .expect("should deserialize agent id");
        assert_eq!(action.agent, Some(AgentId::from("gemini")));

        let action = serde_json::from_str::<NewExternalAgentThread>(
            r#"{"agent":{"custom":{"name":"gemini"}}}"#,
        )
        .expect("should deserialize legacy custom agent payload");
        assert_eq!(action.agent, Some(AgentId::from("gemini")));

        let action = serde_json::from_str::<NewExternalAgentThread>(r#"{"agent":"NativeAgent"}"#)
            .expect("should deserialize legacy native agent payload");
        assert_eq!(action.agent, Some(Agent::NativeAgent.id()));

        // Without an agent the action opens the chooser, which is what lets
        // the command palette list it.
        let action =
            serde_json::from_str::<NewExternalAgentThread>(r#"{}"#).expect("the agent is optional");
        assert_eq!(action.agent, None);
        assert!(serde_json::from_str::<NewExternalAgentThread>(r#"{"other":1}"#).is_err());
    }
}

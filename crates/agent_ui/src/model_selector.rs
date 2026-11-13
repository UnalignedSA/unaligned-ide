use std::{rc::Rc, sync::Arc};

use acp_thread::{
    AgentModelIcon, AgentModelId, AgentModelInfo, AgentModelList, AgentModelSelector,
};

use anyhow::Result;
use app_actions::agent::OpenSettings;
use collections::{HashMap, HashSet, IndexMap};
use futures::FutureExt;
use fuzzy_nucleo::StringMatchCandidate;
use gpui::{
    Action, AsyncWindowContext, BackgroundExecutor, DismissEvent, FocusHandle, Subscription, Task,
    TaskExt, WeakEntity,
};
use itertools::Itertools;
use language_model::LanguageModelProviderId;
use picker::{Picker, PickerDelegate};
use settings::SettingsStore;
use ui::{DocumentationAside, IntoElement, prelude::*};
use util::ResultExt;

use crate::favorite_models::{
    MAX_RECENT_MODELS, ensure_recent_model, note_recent_model, recent_model_key, recent_models,
};
use crate::language_model_selector::{
    AGENTS_GROUP, AgentRow, FAVORITES_GROUP, ModelFacts, RECENT_GROUP, connected_providers,
    maker_group_title, provider_models, set_up_agents, split_by_maker,
};
use crate::model_search;
use crate::ui::{
    ModelHealth, ModelSelectorFooter, ModelSelectorHeader, ModelSelectorListItem,
    NOTHING_CONNECTED, PRICE_UNIT, connect_a_model_action, documentation_aside_side,
};

pub type ModelSelector = Picker<ModelPickerDelegate>;

/// Wide enough for a name, its price and its context size on one line, and
/// narrower than the chat column (400px by default) it opens in.
pub(crate) const PICKER_WIDTH_REMS: f32 = 23.5;

pub fn acp_model_selector(
    selector: Rc<dyn AgentModelSelector>,
    focus_handle: FocusHandle,
    window: &mut Window,
    cx: &mut Context<ModelSelector>,
) -> ModelSelector {
    let delegate = ModelPickerDelegate::new(selector, focus_handle, window, cx);
    Picker::list(delegate, window, cx)
        .show_scrollbar(true)
        .initial_width(rems(PICKER_WIDTH_REMS))
}

enum ModelPickerEntry {
    /// The label of a group, under a rule when it is not the first.
    Separator(SharedString),
    /// The label of a group that continues the connection above it (its next
    /// maker): no rule.
    SubSeparator(SharedString),
    Model(AgentModelInfo, bool),
    /// An agent with its own subscription: choosing it starts a new chat.
    Agent(AgentRow),
}

/// What the picker of the built-in agent knows besides the agent's own model
/// list: the connections behind the models, what was chosen last, and the
/// agents that could answer instead. Empty for an external agent, whose list
/// is shown as the agent gives it.
#[derive(Default)]
struct PickerLayout {
    /// What the provider registry says of each model, by the chat's model id
    /// (`provider id/model id`).
    facts: HashMap<AgentModelId, ModelFacts>,
    /// The connected providers in the order of the connection catalog.
    connections: Vec<LanguageModelProviderId>,
    /// The models chosen last, most recent first.
    recents: Vec<AgentModelId>,
    agents: Vec<AgentRow>,
}

impl PickerLayout {
    fn of_built_in_agent(cx: &mut App) -> Self {
        let recents = recent_models(cx)
            .into_iter()
            .map(|key| AgentModelId::new(key.to_string()))
            .collect();

        let mut facts = HashMap::default();
        let mut connections = Vec::new();
        for (provider, title) in connected_providers(cx) {
            connections.push(provider.id());
            for (model, model_facts) in provider_models(&*provider, &title, cx) {
                facts.insert(
                    AgentModelId::new(recent_model_key(&model).to_string()),
                    model_facts,
                );
            }
        }

        Self {
            facts,
            connections,
            recents,
            agents: set_up_agents(cx),
        }
    }
}

pub struct ModelPickerDelegate {
    selector: Rc<dyn AgentModelSelector>,
    filtered_entries: Vec<ModelPickerEntry>,
    models: Option<AgentModelList>,
    selected_index: usize,
    selected_description: Option<(usize, SharedString)>,
    selected_model: Option<AgentModelInfo>,
    favorites: HashSet<AgentModelId>,
    layout: PickerLayout,
    /// Whether the agent has answered with its model list at least once.
    models_loaded: bool,
    _refresh_models_task: Task<()>,
    _settings_subscription: Subscription,
    focus_handle: FocusHandle,
}

impl ModelPickerDelegate {
    fn new(
        selector: Rc<dyn AgentModelSelector>,
        focus_handle: FocusHandle,
        window: &mut Window,
        cx: &mut Context<ModelSelector>,
    ) -> Self {
        let rx = selector.watch(cx);
        let refresh_models_task = {
            cx.spawn_in(window, {
                async move |this, cx| {
                    async fn refresh(
                        this: &WeakEntity<Picker<ModelPickerDelegate>>,
                        cx: &mut AsyncWindowContext,
                    ) -> Result<()> {
                        let (models_task, selected_model_task) = this.update(cx, |this, cx| {
                            (
                                this.delegate.selector.list_models(cx),
                                this.delegate.selector.selected_model(cx),
                            )
                        })?;

                        let (models, selected_model) =
                            futures::join!(models_task, selected_model_task);

                        this.update_in(cx, |this, window, cx| {
                            this.delegate.models = models.ok();
                            this.delegate.selected_model = selected_model.ok();
                            this.delegate.models_loaded = true;
                            this.delegate.refresh_layout(cx);
                            this.refresh(window, cx)
                        })
                    }

                    refresh(&this, cx).await.log_err();
                    if let Some(mut rx) = rx {
                        while let Ok(()) = rx.recv().await {
                            refresh(&this, cx).await.log_err();
                        }
                    }
                }
            })
        };

        let selector_for_subscription = selector.clone();
        let settings_subscription =
            cx.observe_global_in::<SettingsStore>(window, move |picker, window, cx| {
                // Only refresh if the favorites actually changed to avoid redundant work
                // when other settings are modified (e.g., user editing settings.json)
                let new_favorites = selector_for_subscription.favorite_model_ids(cx);
                if new_favorites != picker.delegate.favorites {
                    picker.delegate.favorites = new_favorites;
                    picker.refresh(window, cx);
                }
            });
        let favorites = selector.favorite_model_ids(cx);

        Self {
            selector,
            filtered_entries: Vec::new(),
            models: None,
            selected_model: None,
            selected_index: 0,
            selected_description: None,
            favorites,
            layout: PickerLayout::default(),
            models_loaded: false,
            _refresh_models_task: refresh_models_task,
            _settings_subscription: settings_subscription,
            focus_handle,
        }
    }

    pub fn active_model(&self) -> Option<&AgentModelInfo> {
        self.selected_model.as_ref()
    }

    /// The title of the connection the current model answers through, when
    /// the model comes from the provider registry.
    pub fn active_model_connection(&self) -> Option<SharedString> {
        let model = self.selected_model.as_ref()?;
        self.layout.facts.get(&model.id)?.connection.clone()
    }

    /// Whether this is the picker of the built-in agent, whose models are
    /// those of the connected providers. An external agent brings its own
    /// list, shown as it gives it.
    fn lists_connected_providers(&self) -> bool {
        self.selector.should_render_footer()
    }

    /// Reads again what the registry says of the models, and counts the
    /// chat's current model among the recent ones.
    fn refresh_layout(&mut self, cx: &mut App) {
        if !self.lists_connected_providers() {
            return;
        }
        self.layout = PickerLayout::of_built_in_agent(cx);
        if let Some(selected) = &self.selected_model
            && self.layout.facts.contains_key(&selected.id)
            && ensure_recent_model(selected.id.to_string(), cx)
        {
            self.layout.recents.insert(0, selected.id.clone());
            self.layout.recents.truncate(MAX_RECENT_MODELS);
        }
    }

    /// Nothing is connected: the built-in agent has no model to offer and no
    /// other agent is set up.
    fn nothing_connected(&self) -> bool {
        self.lists_connected_providers()
            && self.models_loaded
            && self.models.as_ref().is_none_or(|models| models.is_empty())
            && self.layout.agents.is_empty()
    }

    pub fn favorites_count(&self) -> usize {
        self.favorites.len()
    }

    pub fn cycle_favorite_models(&mut self, window: &mut Window, cx: &mut Context<Picker<Self>>) {
        if self.favorites.is_empty() {
            return;
        }

        let Some(models) = &self.models else {
            return;
        };

        let all_models: Vec<&AgentModelInfo> = match models {
            AgentModelList::Flat(list) => list.iter().collect(),
            AgentModelList::Grouped(index_map) => index_map.values().flatten().collect(),
        };

        let favorite_models: Vec<_> = all_models
            .into_iter()
            .filter(|model| self.favorites.contains(&model.id))
            .unique_by(|model| &model.id)
            .collect();

        if favorite_models.is_empty() {
            return;
        }

        let current_id = self.selected_model.as_ref().map(|m| &m.id);

        let current_index_in_favorites = current_id
            .and_then(|id| favorite_models.iter().position(|m| &m.id == id))
            .unwrap_or(usize::MAX);

        let next_index = if current_index_in_favorites == usize::MAX {
            0
        } else {
            (current_index_in_favorites + 1) % favorite_models.len()
        };

        let next_model = favorite_models[next_index].clone();

        self.selector
            .select_model(next_model.id.clone(), cx)
            .detach_and_log_err(cx);

        self.selected_model = Some(next_model);

        // Keep the picker selection aligned with the newly-selected model
        if let Some(new_index) = self.filtered_entries.iter().position(|entry| {
            matches!(entry, ModelPickerEntry::Model(model_info, _) if self.selected_model.as_ref().is_some_and(|selected| model_info.id == selected.id))
        }) {
            self.set_selected_index(new_index, window, cx);
        } else {
            cx.notify();
        }
    }
}

impl PickerDelegate for ModelPickerDelegate {
    type ListItem = AnyElement;

    fn name() -> &'static str {
        "model selector"
    }

    fn match_count(&self) -> usize {
        self.filtered_entries.len()
    }

    fn selected_index(&self) -> usize {
        self.selected_index
    }

    fn set_selected_index(&mut self, ix: usize, _: &mut Window, cx: &mut Context<Picker<Self>>) {
        self.selected_index = ix.min(self.filtered_entries.len().saturating_sub(1));
        cx.notify();
    }

    fn can_select(&self, ix: usize, _window: &mut Window, _cx: &mut Context<Picker<Self>>) -> bool {
        match self.filtered_entries.get(ix) {
            Some(ModelPickerEntry::Model(model, _)) => model.disabled.is_none(),
            Some(ModelPickerEntry::Agent(_)) => true,
            Some(ModelPickerEntry::Separator(_) | ModelPickerEntry::SubSeparator(_)) | None => {
                false
            }
        }
    }

    fn placeholder_text(&self, _window: &mut Window, _cx: &mut App) -> Arc<str> {
        "Select a model…".into()
    }

    fn no_matches_text(&self, _window: &mut Window, _cx: &mut App) -> Option<SharedString> {
        // An empty catalogue is not a failed search: say what is missing.
        if self.nothing_connected() {
            Some(NOTHING_CONNECTED.into())
        } else {
            Some("No matches".into())
        }
    }

    fn no_matches_action(
        &self,
        _window: &mut Window,
        _cx: &mut Context<Picker<Self>>,
    ) -> Option<picker::PickerEmptyAction> {
        self.nothing_connected().then(connect_a_model_action)
    }

    fn update_matches(
        &mut self,
        query: String,
        window: &mut Window,
        cx: &mut Context<Picker<Self>>,
    ) -> Task<()> {
        let favorites = self.favorites.clone();

        cx.spawn_in(window, async move |this, cx| {
            let filtered_models = match this
                .read_with(cx, |this, cx| {
                    let query = query.clone();
                    this.delegate.models.clone().map(move |models| {
                        fuzzy_search(models, query, cx.background_executor().clone())
                    })
                })
                .ok()
                .flatten()
            {
                Some(task) => task.await,
                None => AgentModelList::Flat(vec![]),
            };

            this.update_in(cx, |this, window, cx| {
                this.delegate.filtered_entries = info_list_to_picker_entries(
                    filtered_models,
                    &favorites,
                    &this.delegate.layout,
                    &query,
                );
                // Finds the currently selected model in the list
                let new_index = this
                    .delegate
                    .selected_model
                    .as_ref()
                    .and_then(|selected| {
                        this.delegate.filtered_entries.iter().position(|entry| {
                            if let ModelPickerEntry::Model(model_info, _) = entry {
                                model_info.id == selected.id
                            } else {
                                false
                            }
                        })
                    })
                    .unwrap_or(0);
                this.set_selected_index(new_index, Some(picker::Direction::Down), true, window, cx);
                cx.notify();
            })
            .ok();
        })
    }

    fn confirm(&mut self, _secondary: bool, window: &mut Window, cx: &mut Context<Picker<Self>>) {
        if self.nothing_connected() {
            // Nothing to choose from: Enter takes the way out the empty
            // state offers.
            window.dispatch_action(Box::new(crate::ManageProviders), cx);
            cx.emit(DismissEvent);
            return;
        }

        match self.filtered_entries.get(self.selected_index) {
            Some(ModelPickerEntry::Model(model_info, _)) if model_info.disabled.is_none() => {
                let model_info = model_info.clone();
                self.selector
                    .select_model(model_info.id.clone(), cx)
                    .detach_and_log_err(cx);
                if self.layout.facts.contains_key(&model_info.id)
                    && note_recent_model(model_info.id.to_string(), cx)
                {
                    self.layout
                        .recents
                        .retain(|recent| recent != &model_info.id);
                    self.layout.recents.insert(0, model_info.id.clone());
                    self.layout.recents.truncate(MAX_RECENT_MODELS);
                    // The "Recent" group is rebuilt for the next opening.
                    cx.defer_in(window, |picker, window, cx| picker.refresh(window, cx));
                }
                self.selected_model = Some(model_info);
                let current_index = self.selected_index;
                self.set_selected_index(current_index, window, cx);

                cx.emit(DismissEvent);
            }
            Some(ModelPickerEntry::Agent(agent)) => {
                window.dispatch_action(
                    Box::new(crate::NewExternalAgentThread::with_agent(agent.id.clone())),
                    cx,
                );
                cx.emit(DismissEvent);
            }
            _ => {}
        }
    }

    fn dismissed(&mut self, window: &mut Window, cx: &mut Context<Picker<Self>>) {
        cx.defer_in(window, |picker, window, cx| {
            picker.set_query("", window, cx);
        });
    }

    fn render_match(
        &self,
        ix: usize,
        selected: bool,
        _: &mut Window,
        cx: &mut Context<Picker<Self>>,
    ) -> Option<Self::ListItem> {
        match self.filtered_entries.get(ix)? {
            ModelPickerEntry::Separator(title) => {
                // The unit of the prices, said once on the first label.
                let lists_token_prices = ix == 0
                    && self.filtered_entries.iter().any(|entry| {
                        matches!(
                            entry,
                            ModelPickerEntry::Model(model, _)
                                if model.cost.as_ref().is_some_and(|cost| cost.contains('$'))
                        )
                    });
                Some(
                    ModelSelectorHeader::new(title, ix > 0)
                        .detail(lists_token_prices.then_some(PRICE_UNIT))
                        .into_any_element(),
                )
            }
            ModelPickerEntry::SubSeparator(title) => Some(
                ModelSelectorHeader::new(title, false)
                    .sub_group(true)
                    .into_any_element(),
            ),
            ModelPickerEntry::Agent(agent) => Some(
                ModelSelectorListItem::new(ix, agent.name.clone())
                    .icon(agent.icon)
                    .secondary_line("Starts a new chat")
                    .is_focused(selected)
                    .into_any_element(),
            ),
            ModelPickerEntry::Model(model_info, is_favorite) => {
                // By id: a row may have dropped its icon (see
                // `connection_entries`), so it is not equal to the selection.
                let is_selected = self
                    .selected_model
                    .as_ref()
                    .is_some_and(|selected| selected.id == model_info.id);
                let facts = self.layout.facts.get(&model_info.id);
                let health = facts.map(|facts| facts.health).unwrap_or_default();
                let context = facts.and_then(|facts| facts.context.clone());
                // "Recent" and "Favorites" are the user's own choices: a
                // recommendation belongs under the connection.
                let in_own_group = self
                    .filtered_entries
                    .iter()
                    .position(|entry| {
                        matches!(
                            entry,
                            ModelPickerEntry::Separator(title)
                                if title != RECENT_GROUP && title != FAVORITES_GROUP
                        )
                    })
                    .is_none_or(|first_connection| ix < first_connection);
                let tag = facts.and_then(|facts| facts.tag).filter(|_| !in_own_group);

                let is_favorite = *is_favorite;
                let handle_action_click = {
                    let model_id = model_info.id.clone();
                    let selector = self.selector.clone();

                    cx.listener(move |_, _, _, cx| {
                        selector.toggle_favorite_model(model_id.clone(), !is_favorite, cx);
                    })
                };

                let model_cost = model_info.cost.clone();

                Some(
                    div()
                        .id(("model-picker-menu-child", ix))
                        .when_some(model_info.description.clone(), |this, description| {
                            this.on_hover(cx.listener(move |menu, hovered, _, cx| {
                                if *hovered {
                                    menu.delegate.selected_description =
                                        Some((ix, description.clone()));
                                } else if matches!(menu.delegate.selected_description, Some((id, _)) if id == ix) {
                                    menu.delegate.selected_description = None;
                                }
                                cx.notify();
                            }))
                        })
                        .child(
                            ModelSelectorListItem::new(ix, model_info.name.clone())
                                .map(|this| match &model_info.icon {
                                    Some(AgentModelIcon::Path(path)) => this.icon_path(path.clone()),
                                    Some(AgentModelIcon::Named(icon)) => this.icon(*icon),
                                    None => this,
                                })
                                .disabled(model_info.disabled.clone())
                                .is_selected(is_selected)
                                .is_focused(selected)
                                .is_latest(model_info.is_latest)
                                .is_favorite(is_favorite)
                                .on_toggle_favorite(handle_action_click)
                                .cost_info(model_cost)
                                .health(health)
                                .tag(tag)
                                .context_hint(context),
                        )
                        .into_any_element(),
                )
            }
        }
    }

    fn documentation_aside(
        &self,
        _window: &mut Window,
        cx: &mut Context<Picker<Self>>,
    ) -> Option<ui::DocumentationAside> {
        self.selected_description.as_ref().map(|(_, description)| {
            let description = description.clone();

            let side = documentation_aside_side(cx);

            DocumentationAside::new(
                side,
                Rc::new(move |_| Label::new(description.clone()).into_any_element()),
            )
        })
    }

    fn documentation_aside_index(&self) -> Option<usize> {
        self.selected_description.as_ref().map(|(ix, _)| *ix)
    }

    fn render_footer(
        &self,
        _window: &mut Window,
        _cx: &mut Context<Picker<Self>>,
    ) -> Option<AnyElement> {
        let focus_handle = self.focus_handle.clone();

        if !self.selector.should_render_footer() {
            return None;
        }

        // With nothing connected the empty state already offers the way out.
        if self.nothing_connected() {
            return None;
        }

        if crate::simple_mode() {
            return Some(ModelSelectorFooter::connections(focus_handle).into_any_element());
        }

        Some(ModelSelectorFooter::new(OpenSettings.boxed_clone(), focus_handle).into_any_element())
    }
}

fn info_list_to_picker_entries(
    model_list: AgentModelList,
    favorites: &HashSet<AgentModelId>,
    layout: &PickerLayout,
    query: &str,
) -> Vec<ModelPickerEntry> {
    let mut entries = match model_list {
        AgentModelList::Grouped(groups) if !layout.facts.is_empty() => {
            connection_entries(groups, favorites, layout, query.is_empty())
        }
        model_list => agent_list_entries(model_list, favorites),
    };

    let query = query.to_lowercase();
    let mut agents = layout
        .agents
        .iter()
        .filter(|agent| query.is_empty() || agent.name.to_lowercase().contains(&query))
        .peekable();
    if agents.peek().is_some() {
        entries.push(ModelPickerEntry::Separator(AGENTS_GROUP.into()));
        entries.extend(agents.cloned().map(ModelPickerEntry::Agent));
    }

    entries
}

/// The list of the built-in agent: what was chosen last, the favourites, then
/// one group per connection in catalog order. Providers are told apart by id
/// (through `layout.facts`), never by the name of the agent's groups.
///
/// While `is_browsing` (no query) the models of Unaligned are split by maker
/// and the ones that are down go last; the matches of a query keep the order
/// of their score and get no "Recent" group.
fn connection_entries(
    groups: IndexMap<acp_thread::AgentModelGroupName, Vec<AgentModelInfo>>,
    favorites: &HashSet<AgentModelId>,
    layout: &PickerLayout,
    is_browsing: bool,
) -> Vec<ModelPickerEntry> {
    #[derive(Clone, PartialEq, Eq, Hash)]
    enum Connection {
        Provider(LanguageModelProviderId),
        /// A group of the agent that no connected provider accounts for.
        Named(SharedString),
    }

    // The agent lists a model once under its provider and may repeat it in a
    // cross-provider group before ("Recommended"): the last occurrence is the
    // one under its provider, so that one decides the model's place.
    let mut seen = HashSet::default();
    let mut models = Vec::new();
    for (group_name, group) in groups.into_iter().rev() {
        for model in group.into_iter().rev() {
            if seen.insert(model.id.clone()) {
                models.push((group_name.0.clone(), model));
            }
        }
    }
    models.reverse();

    let mut entries = Vec::new();
    // Under a connection's label a row needs no icon. In the groups that mix
    // connections it says who answers, when there is more than one.
    let mixes_connections = layout.connections.len() > 1;
    let entry = |model: &AgentModelInfo, shows_icon: bool| {
        let mut model = model.clone();
        if !shows_icon {
            model.icon = None;
        }
        let is_favorite = favorites.contains(&model.id);
        ModelPickerEntry::Model(model, is_favorite)
    };
    let mixed_entry = |model: &AgentModelInfo| entry(model, mixes_connections);
    let model_entry = |model: &AgentModelInfo| entry(model, false);

    if is_browsing {
        let recents = layout
            .recents
            .iter()
            .filter_map(|recent| models.iter().find(|(_, model)| &model.id == recent))
            .take(MAX_RECENT_MODELS)
            .map(|(_, model)| mixed_entry(model))
            .collect::<Vec<_>>();
        if !recents.is_empty() {
            entries.push(ModelPickerEntry::Separator(RECENT_GROUP.into()));
            entries.extend(recents);
        }
    }

    let favorite_models = models
        .iter()
        .filter(|(_, model)| favorites.contains(&model.id))
        .map(|(_, model)| mixed_entry(model))
        .collect::<Vec<_>>();
    if !favorite_models.is_empty() {
        entries.push(ModelPickerEntry::Separator(FAVORITES_GROUP.into()));
        entries.extend(favorite_models);
    }

    let mut by_connection: IndexMap<Connection, (SharedString, Vec<AgentModelInfo>)> =
        IndexMap::default();
    for (group_name, model) in models {
        let facts = layout.facts.get(&model.id);
        let (connection, title) = match facts.and_then(|facts| facts.provider_id.clone()) {
            Some(provider_id) => {
                let title = facts
                    .and_then(|facts| facts.connection.clone())
                    .unwrap_or_else(|| group_name.clone());
                (Connection::Provider(provider_id), title)
            }
            None => (Connection::Named(group_name.clone()), group_name),
        };
        by_connection
            .entry(connection)
            .or_insert_with(|| (title, Vec::new()))
            .1
            .push(model);
    }
    by_connection.sort_by_cached_key(|connection, _| match connection {
        Connection::Provider(id) => layout
            .connections
            .iter()
            .position(|connected| connected == id)
            .unwrap_or(usize::MAX),
        Connection::Named(_) => usize::MAX,
    });

    for (connection, (title, models)) in by_connection {
        if !is_browsing {
            entries.push(ModelPickerEntry::Separator(title));
            entries.extend(models.iter().map(&model_entry));
            continue;
        }

        let is_unaligned = matches!(
            &connection,
            Connection::Provider(id) if id.0.as_ref() == crate::UNALIGNED_PROVIDER_ID
        );
        let by_maker = split_by_maker(
            models,
            |model| {
                layout
                    .facts
                    .get(&model.id)
                    .and_then(|facts| facts.org.clone())
            },
            |model| {
                layout
                    .facts
                    .get(&model.id)
                    .is_some_and(|facts| facts.health == ModelHealth::Down)
            },
        );
        let names_makers = is_unaligned && by_maker.iter().any(|(maker, _)| maker.is_some());
        if !names_makers {
            entries.push(ModelPickerEntry::Separator(title));
            for model in by_maker.iter().flat_map(|(_, models)| models) {
                entries.push(model_entry(model));
            }
            continue;
        }
        for (index, (maker, models)) in by_maker.iter().enumerate() {
            let title = maker_group_title(&title, maker.as_ref());
            entries.push(if index == 0 {
                ModelPickerEntry::Separator(title)
            } else {
                ModelPickerEntry::SubSeparator(title)
            });
            entries.extend(models.iter().map(&model_entry));
        }
    }

    entries
}

/// The list of an agent that brings its own models (or of a test): shown in
/// the agent's order and under the agent's group names.
fn agent_list_entries(
    model_list: AgentModelList,
    favorites: &HashSet<AgentModelId>,
) -> Vec<ModelPickerEntry> {
    let mut entries = Vec::new();

    let all_models: Vec<_> = match &model_list {
        AgentModelList::Flat(list) => list.iter().collect(),
        AgentModelList::Grouped(index_map) => index_map.values().flatten().collect(),
    };

    let favorite_models: Vec<_> = all_models
        .iter()
        .filter(|m| favorites.contains(&m.id))
        .unique_by(|m| &m.id)
        .collect();

    let has_favorites = !favorite_models.is_empty();
    if has_favorites {
        entries.push(ModelPickerEntry::Separator(FAVORITES_GROUP.into()));
        for model in favorite_models {
            entries.push(ModelPickerEntry::Model((*model).clone(), true));
        }
    }

    match model_list {
        AgentModelList::Flat(list) => {
            if has_favorites {
                entries.push(ModelPickerEntry::Separator("All".into()));
            }
            for model in list {
                let is_favorite = favorites.contains(&model.id);
                entries.push(ModelPickerEntry::Model(model, is_favorite));
            }
        }
        AgentModelList::Grouped(mut index_map) => {
            // The cross-provider "Recommended" group of the built-in agent
            // would repeat models that already sit under their provider, so
            // it is left out. (This is the name of a group, not of a
            // provider: providers are matched by id in `connection_entries`.)
            if crate::simple_mode() {
                index_map.shift_remove(&acp_thread::AgentModelGroupName("Recommended".into()));
            }
            for (group_name, models) in index_map {
                entries.push(ModelPickerEntry::Separator(group_name.0));
                for model in models {
                    let is_favorite = favorites.contains(&model.id);
                    entries.push(ModelPickerEntry::Model(model, is_favorite));
                }
            }
        }
    }

    entries
}

async fn fuzzy_search(
    model_list: AgentModelList,
    query: String,
    executor: BackgroundExecutor,
) -> AgentModelList {
    async fn fuzzy_search_list(
        model_list: Vec<AgentModelInfo>,
        group_name: Option<SharedString>,
        query: &str,
        executor: BackgroundExecutor,
    ) -> Vec<AgentModelInfo> {
        model_search::search(model_list, query, executor, move |index, model| {
            (
                StringMatchCandidate::new(index, model.name.clone()),
                StringMatchCandidate::new(
                    index,
                    format!(
                        "{} {}",
                        model.name,
                        group_name.as_deref().unwrap_or_default()
                    ),
                ),
            )
        })
        .await
    }

    match model_list {
        AgentModelList::Flat(model_list) => {
            AgentModelList::Flat(fuzzy_search_list(model_list, None, &query, executor).await)
        }
        AgentModelList::Grouped(index_map) => {
            let groups =
                futures::future::join_all(index_map.into_iter().map(|(group_name, models)| {
                    fuzzy_search_list(models, Some(group_name.0.clone()), &query, executor.clone())
                        .map(|results| (group_name, results))
                }))
                .await;
            AgentModelList::Grouped(IndexMap::from_iter(
                groups
                    .into_iter()
                    .filter(|(_, results)| !results.is_empty()),
            ))
        }
    }
}

#[cfg(test)]
mod tests {
    use gpui::{App, TestAppContext, VisualTestContext};
    use std::cell::RefCell;

    use super::*;

    fn create_model_list(grouped_models: Vec<(&str, Vec<&str>)>) -> AgentModelList {
        AgentModelList::Grouped(IndexMap::from_iter(grouped_models.into_iter().map(
            |(group, models)| {
                (
                    acp_thread::AgentModelGroupName(group.to_string().into()),
                    models
                        .into_iter()
                        .map(|model| acp_thread::AgentModelInfo {
                            id: AgentModelId::new(model),
                            name: model.to_string().into(),
                            description: None,
                            icon: None,
                            is_latest: false,
                            disabled: None,
                            cost: None,
                        })
                        .collect::<Vec<_>>(),
                )
            },
        )))
    }

    fn assert_models_eq(result: AgentModelList, expected: Vec<(&str, Vec<&str>)>) {
        let AgentModelList::Grouped(groups) = result else {
            panic!("Expected LanguageModelInfoList::Grouped, got {:?}", result);
        };

        assert_eq!(
            groups.len(),
            expected.len(),
            "Number of groups doesn't match"
        );

        for (i, (expected_group, expected_models)) in expected.iter().enumerate() {
            let (actual_group, actual_models) = groups.get_index(i).unwrap();
            assert_eq!(
                actual_group.0.as_ref(),
                *expected_group,
                "Group at position {} doesn't match expected group",
                i
            );
            assert_eq!(
                actual_models.len(),
                expected_models.len(),
                "Number of models in group {} doesn't match",
                expected_group
            );

            for (j, expected_model_name) in expected_models.iter().enumerate() {
                assert_eq!(
                    actual_models[j].name, *expected_model_name,
                    "Model at position {} in group {} doesn't match expected model",
                    j, expected_group
                );
            }
        }
    }

    fn create_favorites(models: Vec<&str>) -> HashSet<AgentModelId> {
        models.into_iter().map(AgentModelId::new).collect()
    }

    fn get_entry_model_ids(entries: &[ModelPickerEntry]) -> Vec<&str> {
        entries
            .iter()
            .filter_map(|entry| match entry {
                ModelPickerEntry::Model(info, _) => Some(info.id.as_ref()),
                _ => None,
            })
            .collect()
    }

    #[gpui::test]
    fn confirming_model_selects_model(cx: &mut TestAppContext) {
        cx.update(|cx| {
            let settings_store = settings::SettingsStore::test(cx);
            cx.set_global(settings_store);
            theme_settings::init(theme::LoadThemes::JustBase, cx);
            editor::init(cx);
        });

        let model_selector = Rc::new(TestModelSelector::new());

        let window_handle = cx.add_window({
            let model_selector = model_selector.clone();
            move |window, cx| {
                let selector: Rc<dyn AgentModelSelector> = model_selector;
                acp_model_selector(selector, cx.focus_handle(), window, cx)
            }
        });
        cx.run_until_parked();

        let mut cx = VisualTestContext::from_window(window_handle.into(), cx);
        window_handle
            .update(&mut cx, |picker, window, cx| {
                picker.delegate.set_selected_index(1, window, cx);
                picker.delegate.confirm(false, window, cx);
            })
            .unwrap();

        assert_eq!(
            model_selector.selected_models.borrow().as_slice(),
            &[AgentModelId::new("manual")]
        );
    }

    struct TestModelSelector {
        models: Vec<AgentModelInfo>,
        selected_model: RefCell<AgentModelInfo>,
        selected_models: RefCell<Vec<AgentModelId>>,
    }

    impl TestModelSelector {
        fn new() -> Self {
            let models = vec![
                AgentModelInfo {
                    id: AgentModelId::new("auto"),
                    name: "Auto".into(),
                    description: None,
                    icon: None,
                    is_latest: false,
                    disabled: None,
                    cost: None,
                },
                AgentModelInfo {
                    id: AgentModelId::new("manual"),
                    name: "Manual".into(),
                    description: None,
                    icon: None,
                    is_latest: false,
                    disabled: None,
                    cost: None,
                },
            ];

            Self {
                selected_model: RefCell::new(models[0].clone()),
                models,
                selected_models: RefCell::new(Vec::new()),
            }
        }
    }

    impl AgentModelSelector for TestModelSelector {
        fn list_models(&self, _cx: &mut App) -> Task<Result<AgentModelList>> {
            Task::ready(Ok(AgentModelList::Flat(self.models.clone())))
        }

        fn select_model(&self, model_id: AgentModelId, _cx: &mut App) -> Task<Result<()>> {
            self.selected_models.borrow_mut().push(model_id.clone());
            if let Some(model) = self.models.iter().find(|model| model.id == model_id) {
                *self.selected_model.borrow_mut() = model.clone();
            }
            Task::ready(Ok(()))
        }

        fn selected_model(&self, _cx: &mut App) -> Task<Result<AgentModelInfo>> {
            Task::ready(Ok(self.selected_model.borrow().clone()))
        }
    }

    fn get_entry_labels(entries: &[ModelPickerEntry]) -> Vec<&str> {
        entries
            .iter()
            .map(|entry| match entry {
                ModelPickerEntry::Model(info, _) => info.id.as_ref(),
                ModelPickerEntry::Separator(s) | ModelPickerEntry::SubSeparator(s) => s.as_ref(),
                ModelPickerEntry::Agent(agent) => agent.name.as_ref(),
            })
            .collect()
    }

    #[gpui::test]
    async fn test_fuzzy_match(cx: &mut TestAppContext) {
        let models = create_model_list(vec![
            (
                "zed",
                vec![
                    "Claude 3.7 Sonnet",
                    "Claude 3.7 Sonnet Thinking",
                    "gpt-5",
                    "gpt-5-mini",
                ],
            ),
            ("openai", vec!["gpt-3.5-turbo", "gpt-5", "gpt-5-mini"]),
            ("ollama", vec!["mistral", "deepseek"]),
        ]);

        // Results should preserve models order whenever possible.
        // In the case below, `zed/gpt-5-mini` and `openai/gpt-5-mini` have identical
        // similarity scores, but `zed/gpt-5-mini` was higher in the models list,
        // so it should appear first in the results.
        let results = fuzzy_search(models.clone(), "mini".into(), cx.executor()).await;
        assert_models_eq(
            results,
            vec![("zed", vec!["gpt-5-mini"]), ("openai", vec!["gpt-5-mini"])],
        );

        // Fuzzy search - test with specific model name
        let results = fuzzy_search(models.clone(), "mistral".into(), cx.executor()).await;
        assert_models_eq(results, vec![("ollama", vec!["mistral"])]);

        let models = create_model_list(vec![
            ("OpenAI", vec!["GPT-5.6 Sol"]),
            ("Anthropic", vec!["Claude Sonnet 4"]),
        ]);
        let results = fuzzy_search(models.clone(), "GPT 5.6".into(), cx.executor()).await;
        assert_models_eq(results, vec![("OpenAI", vec!["GPT-5.6 Sol"])]);

        let results = fuzzy_search(models.clone(), "openai 5.6".into(), cx.executor()).await;
        assert_models_eq(results, vec![("OpenAI", vec!["GPT-5.6 Sol"])]);

        let results = fuzzy_search(models, "openai".into(), cx.executor()).await;
        assert_models_eq(results, vec![("OpenAI", vec!["GPT-5.6 Sol"])]);
    }

    #[gpui::test]
    async fn test_fuzzy_match_prioritizes_names_over_group(cx: &mut TestAppContext) {
        let models = create_model_list(vec![(
            "High Provider",
            vec!["Reasoner", "Extra High", "High"],
        )]);

        let results = fuzzy_search(models, "high".into(), cx.executor()).await;
        assert_models_eq(
            results,
            vec![("High Provider", vec!["High", "Extra High", "Reasoner"])],
        );
    }

    #[gpui::test(iterations = 20)]
    async fn test_model_search_builds_candidates_in_background(executor: BackgroundExecutor) {
        let background_executor = executor.clone();
        let results = model_search::search(
            vec!["High".to_string()],
            "high",
            executor,
            move |index, name| {
                assert!(!background_executor.is_main_thread());
                (
                    StringMatchCandidate::new(index, name.clone()),
                    StringMatchCandidate::new(index, name.clone()),
                )
            },
        )
        .await;

        assert_eq!(results, ["High"]);
    }

    fn facts(
        provider: &str,
        connection: &str,
        org: Option<&str>,
        health: ModelHealth,
    ) -> ModelFacts {
        ModelFacts {
            provider_id: Some(LanguageModelProviderId::from(provider.to_string())),
            connection: Some(connection.to_string().into()),
            org: org.map(|org| org.to_string().into()),
            health,
            context: None,
            tag: None,
        }
    }

    /// The layout of a built-in agent with Unaligned and an OpenAI key
    /// connected and Claude Code set up.
    fn built_in_agent_layout() -> PickerLayout {
        let unaligned = crate::UNALIGNED_PROVIDER_ID;
        PickerLayout {
            facts: HashMap::from_iter([
                (
                    AgentModelId::new("unaligned/gpt"),
                    facts(unaligned, "Unaligned", Some("OpenAI"), ModelHealth::Up),
                ),
                (
                    AgentModelId::new("unaligned/haiku"),
                    facts(unaligned, "Unaligned", Some("Anthropic"), ModelHealth::Down),
                ),
                (
                    AgentModelId::new("unaligned/sonnet"),
                    facts(unaligned, "Unaligned", Some("Anthropic"), ModelHealth::Up),
                ),
                (
                    AgentModelId::new("openai/gpt"),
                    facts("openai", "OpenAI", None, ModelHealth::Up),
                ),
            ]),
            connections: vec![
                LanguageModelProviderId::from(unaligned.to_string()),
                LanguageModelProviderId::from("openai".to_string()),
            ],
            recents: vec![
                AgentModelId::new("unaligned/sonnet"),
                AgentModelId::new("gone/model"),
            ],
            agents: vec![AgentRow {
                id: project::AgentId::new("claude-acp"),
                name: "Claude Code".into(),
                icon: IconName::AiClaude,
            }],
        }
    }

    #[gpui::test]
    fn test_model_selector_built_in_agent_order(_cx: &mut TestAppContext) {
        // The agent lists the OpenAI key before Unaligned, repeats a model in
        // "Recommended" and names its groups after the providers' display
        // names: none of that decides the order.
        let models = create_model_list(vec![
            ("Recommended", vec!["unaligned/sonnet"]),
            ("OpenAI API", vec!["openai/gpt"]),
            (
                "Some Display Name",
                vec!["unaligned/gpt", "unaligned/haiku", "unaligned/sonnet"],
            ),
        ]);

        let entries = info_list_to_picker_entries(
            models,
            &create_favorites(vec!["openai/gpt"]),
            &built_in_agent_layout(),
            "",
        );

        assert_eq!(
            get_entry_labels(&entries),
            vec![
                "Recent",
                "unaligned/sonnet",
                "Favorites",
                "openai/gpt",
                "Unaligned · Anthropic",
                "unaligned/sonnet",
                "unaligned/haiku",
                "Unaligned · OpenAI",
                "unaligned/gpt",
                "OpenAI",
                "openai/gpt",
                "Agents with their own subscription",
                "Claude Code",
            ]
        );
        // Only the first maker of a connection sits under a rule.
        assert!(matches!(&entries[4], ModelPickerEntry::Separator(_)));
        assert!(matches!(&entries[7], ModelPickerEntry::SubSeparator(_)));
        assert!(matches!(&entries[9], ModelPickerEntry::Separator(_)));
    }

    #[gpui::test]
    fn test_model_selector_built_in_agent_query(_cx: &mut TestAppContext) {
        // What a search for "son" leaves of the list above.
        let models = create_model_list(vec![
            ("Recommended", vec!["unaligned/sonnet"]),
            (
                "Some Display Name",
                vec!["unaligned/sonnet", "unaligned/haiku"],
            ),
        ]);

        let entries = info_list_to_picker_entries(
            models,
            &create_favorites(vec![]),
            &built_in_agent_layout(),
            "son",
        );

        // No "Recent" group, no split by maker, no agent whose name does not
        // match; the score order of the matches is kept.
        assert_eq!(
            get_entry_labels(&entries),
            vec!["Unaligned", "unaligned/sonnet", "unaligned/haiku"]
        );

        let entries = info_list_to_picker_entries(
            AgentModelList::Grouped(IndexMap::default()),
            &create_favorites(vec![]),
            &built_in_agent_layout(),
            "claude",
        );
        assert_eq!(
            get_entry_labels(&entries),
            vec!["Agents with their own subscription", "Claude Code"]
        );
    }

    #[gpui::test]
    fn test_model_selector_external_agent_list_is_kept(_cx: &mut TestAppContext) {
        // An agent that brings its own models: its groups, its order, even
        // when one of its groups is called like a provider.
        let models = create_model_list(vec![
            ("Fast", vec!["flash"]),
            ("Unaligned", vec!["opus", "sonnet"]),
        ]);

        let entries = info_list_to_picker_entries(
            models,
            &create_favorites(vec![]),
            &PickerLayout::default(),
            "",
        );

        assert_eq!(
            get_entry_labels(&entries),
            vec!["Fast", "flash", "Unaligned", "opus", "sonnet"]
        );
    }

    #[gpui::test]
    fn test_favorites_section_appears_when_favorites_exist(_cx: &mut TestAppContext) {
        let models = create_model_list(vec![
            ("zed", vec!["zed/claude", "zed/gemini"]),
            ("openai", vec!["openai/gpt-5"]),
        ]);
        let favorites = create_favorites(vec!["zed/gemini"]);

        let entries = info_list_to_picker_entries(models, &favorites, &PickerLayout::default(), "");

        assert!(matches!(
            entries.first(),
            Some(ModelPickerEntry::Separator(s)) if s == FAVORITES_GROUP
        ));

        let model_ids = get_entry_model_ids(&entries);
        assert_eq!(model_ids[0], "zed/gemini");
    }

    #[gpui::test]
    fn test_no_favorites_section_when_no_favorites(_cx: &mut TestAppContext) {
        let models = create_model_list(vec![("zed", vec!["zed/claude", "zed/gemini"])]);
        let favorites = create_favorites(vec![]);

        let entries = info_list_to_picker_entries(models, &favorites, &PickerLayout::default(), "");

        assert!(matches!(
            entries.first(),
            Some(ModelPickerEntry::Separator(s)) if s == "zed"
        ));
    }

    #[gpui::test]
    fn test_models_have_correct_actions(_cx: &mut TestAppContext) {
        let models = create_model_list(vec![
            ("zed", vec!["zed/claude", "zed/gemini"]),
            ("openai", vec!["openai/gpt-5"]),
        ]);
        let favorites = create_favorites(vec!["zed/claude"]);

        let entries = info_list_to_picker_entries(models, &favorites, &PickerLayout::default(), "");

        for entry in &entries {
            if let ModelPickerEntry::Model(info, is_favorite) = entry {
                if info.id.as_ref() == "zed/claude" {
                    assert!(is_favorite, "zed/claude should be a favorite");
                } else {
                    assert!(!is_favorite, "{} should not be a favorite", info.id);
                }
            }
        }
    }

    #[gpui::test]
    fn test_favorites_appear_in_both_sections(_cx: &mut TestAppContext) {
        let models = create_model_list(vec![
            ("zed", vec!["zed/claude", "zed/gemini"]),
            ("openai", vec!["openai/gpt-5", "openai/gpt-4"]),
        ]);
        let favorites = create_favorites(vec!["zed/gemini", "openai/gpt-5"]);

        let entries = info_list_to_picker_entries(models, &favorites, &PickerLayout::default(), "");
        let model_ids = get_entry_model_ids(&entries);

        assert_eq!(model_ids[0], "zed/gemini");
        assert_eq!(model_ids[1], "openai/gpt-5");

        assert!(model_ids[2..].contains(&"zed/gemini"));
        assert!(model_ids[2..].contains(&"openai/gpt-5"));
    }

    #[gpui::test]
    fn test_favorites_are_not_duplicated_when_repeated_in_other_sections(_cx: &mut TestAppContext) {
        let models = create_model_list(vec![
            ("Recommended", vec!["zed/claude", "anthropic/claude"]),
            ("Zed", vec!["zed/claude", "zed/gpt-5"]),
            ("Antropic", vec!["anthropic/claude"]),
            ("OpenAI", vec!["openai/gpt-5"]),
        ]);

        let favorites = create_favorites(vec!["zed/claude"]);

        let entries = info_list_to_picker_entries(models, &favorites, &PickerLayout::default(), "");
        let labels = get_entry_labels(&entries);

        assert_eq!(
            labels,
            vec![
                "Favorites",
                "zed/claude",
                // The cross-provider "Recommended" group is not listed.
                "Zed",
                "zed/claude",
                "zed/gpt-5",
                "Antropic",
                "anthropic/claude",
                "OpenAI",
                "openai/gpt-5"
            ]
        );
    }

    #[gpui::test]
    fn test_flat_model_list_with_favorites(_cx: &mut TestAppContext) {
        let models = AgentModelList::Flat(vec![
            acp_thread::AgentModelInfo {
                id: AgentModelId::new("zed/claude"),
                name: "Claude".into(),
                description: None,
                icon: None,
                is_latest: false,
                disabled: None,
                cost: None,
            },
            acp_thread::AgentModelInfo {
                id: AgentModelId::new("zed/gemini"),
                name: "Gemini".into(),
                description: None,
                icon: None,
                is_latest: false,
                disabled: None,
                cost: None,
            },
        ]);
        let favorites = create_favorites(vec!["zed/gemini"]);

        let entries = info_list_to_picker_entries(models, &favorites, &PickerLayout::default(), "");

        assert!(matches!(
            entries.first(),
            Some(ModelPickerEntry::Separator(s)) if s == FAVORITES_GROUP
        ));

        assert!(entries.iter().any(|e| matches!(
            e,
            ModelPickerEntry::Separator(s) if s == "All"
        )));
    }

    #[gpui::test]
    fn test_favorites_count_returns_correct_count(_cx: &mut TestAppContext) {
        let empty_favorites: HashSet<AgentModelId> = HashSet::default();
        assert_eq!(empty_favorites.len(), 0);

        let one_favorite = create_favorites(vec!["model-a"]);
        assert_eq!(one_favorite.len(), 1);

        let multiple_favorites = create_favorites(vec!["model-a", "model-b", "model-c"]);
        assert_eq!(multiple_favorites.len(), 3);

        let with_duplicates = create_favorites(vec!["model-a", "model-a", "model-b"]);
        assert_eq!(with_duplicates.len(), 2);
    }

    #[gpui::test]
    fn test_is_favorite_flag_set_correctly_in_entries(_cx: &mut TestAppContext) {
        let models = AgentModelList::Flat(vec![
            acp_thread::AgentModelInfo {
                id: AgentModelId::new("favorite-model"),
                name: "Favorite".into(),
                description: None,
                icon: None,
                is_latest: false,
                disabled: None,
                cost: None,
            },
            acp_thread::AgentModelInfo {
                id: AgentModelId::new("regular-model"),
                name: "Regular".into(),
                description: None,
                icon: None,
                is_latest: false,
                disabled: None,
                cost: None,
            },
        ]);
        let favorites = create_favorites(vec!["favorite-model"]);

        let entries = info_list_to_picker_entries(models, &favorites, &PickerLayout::default(), "");

        for entry in &entries {
            if let ModelPickerEntry::Model(info, is_favorite) = entry {
                if info.id.as_ref() == "favorite-model" {
                    assert!(*is_favorite, "favorite-model should have is_favorite=true");
                } else if info.id.as_ref() == "regular-model" {
                    assert!(!*is_favorite, "regular-model should have is_favorite=false");
                }
            }
        }
    }
}

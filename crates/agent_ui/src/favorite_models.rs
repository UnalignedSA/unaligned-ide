use std::sync::Arc;

use agent_settings::{AgentSettings, language_model_to_selection};
use db::kvp::KeyValueStore;
use fs::Fs;
use gpui::{AppContext as _, Global, SharedString};
use language_model::LanguageModel;
use settings::{Settings as _, update_settings_file};
use ui::App;
use util::ResultExt as _;

pub fn toggle_in_settings(
    model: LanguageModel,
    should_be_favorite: bool,
    fs: Arc<dyn Fs>,
    cx: &mut App,
) {
    let current_user_selection = AgentSettings::get_global(cx)
        .default_model
        .as_ref()
        .filter(|selection| {
            selection.provider.0 == model.provider_id().0.as_ref()
                && selection.model == model.id().0.as_ref()
        })
        .cloned();

    let selection = language_model_to_selection(&model, current_user_selection.as_ref());
    update_settings_file(fs, cx, move |settings, _| {
        let agent = settings.agent.get_or_insert_default();
        if should_be_favorite {
            agent.add_favorite_model(selection.clone());
        } else {
            agent.remove_favorite_model(&selection);
        }
    });
}

/// How many models the "RECENT" group of the model picker lists.
pub(crate) const MAX_RECENT_MODELS: usize = 5;

const RECENT_MODELS_KEY: &str = "agent_recent_models";

/// The models last chosen for a chat or an inline edit, most recent first, as
/// `provider id/model id`. Favourites are a setting because the user writes
/// them; this list is written by use, so it lives in the app's key-value
/// store with the other "last used" facts.
#[derive(Default)]
struct RecentModels(Vec<SharedString>);

impl Global for RecentModels {}

/// The key of `model` in the recent list: `provider id/model id`, which is
/// also the id the chat gives to the models of the built-in agent.
pub(crate) fn recent_model_key(model: &LanguageModel) -> SharedString {
    format!("{}/{}", model.provider_id().0, model.id().0).into()
}

/// The recent models, most recent first, at most [`MAX_RECENT_MODELS`].
pub(crate) fn recent_models(cx: &mut App) -> Vec<SharedString> {
    if !cx.has_global::<RecentModels>() {
        let stored = load_recent_models(cx);
        cx.set_global(RecentModels(stored));
    }
    cx.global::<RecentModels>().0.clone()
}

/// Moves `key` (see [`recent_model_key`]) to the front of the recent models
/// and stores the list. Returns whether the list changed.
pub(crate) fn note_recent_model(key: impl Into<SharedString>, cx: &mut App) -> bool {
    let key = key.into();
    let mut recents = recent_models(cx);
    if recents.first() == Some(&key) {
        return false;
    }
    push_recent(&mut recents, key);
    cx.set_global(RecentModels(recents.clone()));
    store_recent_models(recents, cx);
    true
}

/// Adds `key` to the front of the recent models when it is not listed yet,
/// and leaves the order alone when it is: for a model that is merely the
/// current one of a chat, as opposed to one the user just chose. Returns
/// whether the list changed.
pub(crate) fn ensure_recent_model(key: impl Into<SharedString>, cx: &mut App) -> bool {
    let key = key.into();
    if recent_models(cx).contains(&key) {
        return false;
    }
    note_recent_model(key, cx)
}

fn push_recent(recents: &mut Vec<SharedString>, key: SharedString) {
    recents.retain(|recent| recent != &key);
    recents.insert(0, key);
    recents.truncate(MAX_RECENT_MODELS);
}

fn load_recent_models(cx: &App) -> Vec<SharedString> {
    // Tests share one database: what a test chose must not show up in the
    // next one, so there the list lives in memory only.
    if cfg!(test) {
        return Vec::new();
    }
    let mut recents = KeyValueStore::global(cx)
        .read_kvp(RECENT_MODELS_KEY)
        .log_err()
        .flatten()
        .and_then(|json| serde_json::from_str::<Vec<String>>(&json).log_err())
        .unwrap_or_default();
    recents.truncate(MAX_RECENT_MODELS);
    recents.into_iter().map(SharedString::from).collect()
}

fn store_recent_models(recents: Vec<SharedString>, cx: &App) {
    if cfg!(test) {
        return;
    }
    let recents = recents
        .iter()
        .map(|recent| recent.as_ref())
        .collect::<Vec<&str>>();
    let Some(json) = serde_json::to_string(&recents).log_err() else {
        return;
    };
    let kvp = KeyValueStore::global(cx);
    cx.background_spawn(async move {
        kvp.write_kvp(RECENT_MODELS_KEY.to_string(), json)
            .await
            .log_err();
    })
    .detach();
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_recent_model_selector_list_keeps_five_most_recent_first() {
        let mut recents = Vec::new();
        for key in ["a/1", "a/2", "a/3", "a/4", "a/5", "a/6"] {
            push_recent(&mut recents, key.into());
        }
        assert_eq!(recents, ["a/6", "a/5", "a/4", "a/3", "a/2"]);

        // Choosing a model again moves it to the front without repeating it.
        push_recent(&mut recents, "a/4".into());
        assert_eq!(recents, ["a/4", "a/6", "a/5", "a/3", "a/2"]);
    }
}

//! Release notes: the page of the running version's release, opened in the
//! browser. Nothing is fetched by the app itself.
//!
//! Upstream's parts are gone with its updater: the notes downloaded from its
//! own site and shown in a tab, the "updated to …" notification that followed
//! an in-place install (this app installs nothing), and its announcements.

use std::any::TypeId;

use auto_update::{ViewReleaseNotes, release_notes_url};
use command_palette_hooks::CommandPaletteFilter;
use gpui::{App, actions};
use workspace::Workspace;

actions!(
    auto_update,
    [
        /// Opens the release notes for the current version in a browser.
        ViewReleaseNotesLocally
    ]
);

pub fn init(cx: &mut App) {
    // `None` while the brand file names no releases repository: there are no
    // release notes then, and the two actions are hidden.
    let has_release_notes = release_notes_url(cx).is_some();

    cx.observe_new(move |workspace: &mut Workspace, _window, cx| {
        if has_release_notes {
            workspace.register_action(|_, _: &ViewReleaseNotesLocally, _, cx| {
                if let Some(url) = release_notes_url(cx) {
                    cx.open_url(&url);
                }
            });
        } else {
            // Done here and not in `init`: the command palette creates its
            // filter after this crate is initialized.
            CommandPaletteFilter::update_global(cx, |filter, _| {
                filter.hide_action_types(&[
                    TypeId::of::<ViewReleaseNotes>(),
                    TypeId::of::<ViewReleaseNotesLocally>(),
                ]);
            });
        }
    })
    .detach();
}

//! One organisation's notepad, as this window knows it.
//!
//! Cache-and-refetch, never a writable local store: the list on disk exists so
//! the window has something to draw before the network answers, and every
//! write still goes to the server. Access can change between reads, and a
//! local copy that disagreed with the server would be a second truth.

use std::sync::Arc;

use gpui_kit::*;

use crate::api::models::Note;
use crate::api::{ApiClient, ApiError};
use crate::store::{notes_cache_path, read_json, write_json};

#[derive(Clone, Debug, PartialEq)]
pub enum LoadState {
    Idle,
    Loading,
    Failed(String),
}

pub enum NotesEvent {
    /// A fresh list arrived; open editors compare it with what they hold.
    Refreshed,
    /// The session needs attention — the workspace routes this to sign-in.
    AuthFailed(ApiError),
}

pub struct NotesStore {
    profile_id: String,
    org_id: String,
    client: Arc<ApiClient>,
    notes: Vec<Note>,
    state: LoadState,
    /// True once the server has answered at least once. Before that the list
    /// is the cache, and an absent note may just be one made elsewhere.
    fresh: bool,
    _refresh: Option<Task<()>>,
}

impl EventEmitter<NotesEvent> for NotesStore {}

impl NotesStore {
    pub fn new(
        profile_id: &str,
        org_id: &str,
        client: Arc<ApiClient>,
        cx: &mut Context<Self>,
    ) -> Self {
        let mut notes: Vec<Note> =
            read_json(&notes_cache_path(profile_id, org_id)).unwrap_or_default();
        sort(&mut notes);
        let mut store = Self {
            profile_id: profile_id.to_owned(),
            org_id: org_id.to_owned(),
            client,
            notes,
            state: LoadState::Idle,
            fresh: false,
            _refresh: None,
        };
        store.refresh(cx);
        store
    }

    pub fn org_id(&self) -> &str {
        &self.org_id
    }

    pub fn client(&self) -> Arc<ApiClient> {
        self.client.clone()
    }

    pub fn notes(&self) -> &[Note] {
        &self.notes
    }

    pub fn get(&self, id: &str) -> Option<&Note> {
        self.notes.iter().find(|n| n.id == id)
    }

    pub fn state(&self) -> &LoadState {
        &self.state
    }

    pub fn is_fresh(&self) -> bool {
        self.fresh
    }

    pub fn refresh(&mut self, cx: &mut Context<Self>) {
        if self.state == LoadState::Loading {
            return;
        }
        self.state = LoadState::Loading;
        cx.notify();
        let client = self.client.clone();
        let org_id = self.org_id.clone();
        self._refresh = Some(cx.spawn(async move |this, cx| {
            let result = cx
                .background_spawn(async move { client.notes(&org_id) })
                .await;
            this.update(cx, |this, cx| {
                match result {
                    Ok(mut notes) => {
                        sort(&mut notes);
                        this.notes = notes;
                        this.state = LoadState::Idle;
                        this.fresh = true;
                        this.write_cache(cx);
                        cx.emit(NotesEvent::Refreshed);
                    }
                    Err(e) if e.is_auth() => {
                        this.state = LoadState::Failed(e.to_string());
                        cx.emit(NotesEvent::AuthFailed(e));
                    }
                    Err(e) => this.state = LoadState::Failed(e.to_string()),
                }
                cx.notify();
            })
            .ok();
        }));
    }

    /// The server's copy after a create or save, which is also the moment the
    /// note moves to the top of the list.
    pub fn upsert(&mut self, note: Note, cx: &mut Context<Self>) {
        match self.notes.iter_mut().find(|n| n.id == note.id) {
            Some(existing) => *existing = note,
            None => self.notes.push(note),
        }
        sort(&mut self.notes);
        self.write_cache(cx);
        cx.notify();
    }

    pub fn remove(&mut self, id: &str, cx: &mut Context<Self>) {
        self.notes.retain(|n| n.id != id);
        self.write_cache(cx);
        cx.notify();
    }

    pub fn report_auth_failure(&mut self, error: ApiError, cx: &mut Context<Self>) {
        cx.emit(NotesEvent::AuthFailed(error));
    }

    fn write_cache(&self, cx: &mut Context<Self>) {
        let path = notes_cache_path(&self.profile_id, &self.org_id);
        let notes = self.notes.clone();
        cx.background_spawn(async move {
            if let Err(e) = write_json(&path, &notes) {
                log::warn!("could not cache notes: {e}");
            }
        })
        .detach();
    }
}

/// Most recently updated first — the server's own order, kept locally so a
/// save moves the note to the top without a refetch.
fn sort(notes: &mut [Note]) {
    notes.sort_by_key(|n| std::cmp::Reverse(n.updated_at));
}

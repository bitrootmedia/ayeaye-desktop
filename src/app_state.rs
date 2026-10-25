//! App-wide state: the profiles, one API client per profile, and the saved
//! workspace layout. Lives in a GPUI global so any view can reach it.

use std::collections::HashMap;
use std::sync::Arc;

use gpui_kit::*;

use crate::api::ApiClient;
use crate::store::profiles::{Profile, ProfileList};
use crate::store::workspace::WorkspaceState;
use crate::store::{self, vault};

pub struct AppState {
    pub profiles: ProfileList,
    pub workspace: WorkspaceState,
    /// Each profile's client owns that profile's token set. There is no
    /// shared "current session": switching profile switches clients.
    clients: HashMap<String, Arc<ApiClient>>,
}

impl Global for AppState {}

impl AppState {
    pub fn init(cx: &mut App) {
        cx.set_global(AppState {
            profiles: ProfileList::load(),
            workspace: WorkspaceState::load(),
            clients: HashMap::new(),
        });
    }

    pub fn get(cx: &App) -> &AppState {
        cx.global::<AppState>()
    }

    pub fn update<R>(cx: &mut App, f: impl FnOnce(&mut AppState, &mut App) -> R) -> R {
        cx.update_global::<AppState, R>(f)
    }

    pub fn active_profile(&self) -> Option<Profile> {
        self.profiles.active().cloned()
    }

    /// The profile's client, created on first use from its stored tokens.
    /// Every rotation the server hands back is written straight to the
    /// keychain, from whichever thread received it.
    pub fn client(&mut self, profile: &Profile) -> Arc<ApiClient> {
        self.clients
            .entry(profile.id.clone())
            .or_insert_with(|| new_client(profile))
            .clone()
    }

    /// A client for a profile that is being signed in; installed as that
    /// profile's client once sign-in succeeds.
    pub fn install_client(&mut self, profile_id: &str, client: Arc<ApiClient>) {
        self.clients.insert(profile_id.to_owned(), client);
    }

    pub fn set_active(&mut self, profile_id: &str) {
        self.profiles.active = Some(profile_id.to_owned());
        self.profiles.save();
    }

    pub fn save_profile(&mut self, profile: Profile) {
        self.profiles.upsert(profile);
        self.profiles.save();
    }

    pub fn update_profile(&mut self, profile_id: &str, f: impl FnOnce(&mut Profile)) {
        self.profiles.update(profile_id, f);
        self.profiles.save();
    }

    /// Forgets the session but keeps the profile, so signing back in is one
    /// password away.
    pub fn sign_out(&mut self, profile_id: &str, cx: &mut App) {
        if let Some(client) = self.clients.remove(profile_id) {
            let id = profile_id.to_owned();
            cx.background_spawn(async move {
                crate::realtime::revoke_socket_token(&id, &client);
                client.sign_out()
            })
            .detach();
        } else {
            vault::forget_socket(profile_id);
        }
        vault::forget(profile_id);
    }

    pub fn remove_profile(&mut self, profile_id: &str, cx: &mut App) {
        self.sign_out(profile_id, cx);
        self.profiles.remove(profile_id);
        self.profiles.save();
        self.workspace.forget_profile(profile_id);
        self.workspace.save();
        store::forget_profile_cache(profile_id);
    }
}

pub fn new_client(profile: &Profile) -> Arc<ApiClient> {
    let id = profile.id.clone();
    ApiClient::new(&profile.base_url, vault::load(&profile.id), move |tokens| {
        vault::save(&id, tokens)
    })
}

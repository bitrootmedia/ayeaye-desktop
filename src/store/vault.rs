//! Session tokens, kept in the OS credential store (macOS Keychain, Windows
//! Credential Manager, the Secret Service on Linux) — never in a plist or a
//! JSON file every process you run can read.
//!
//! `AYEAYE_TOKEN_STORE=file` keeps them in a 0600 file under the config dir
//! instead. That is for development only: an unsigned binary changes code
//! signature on every rebuild, and macOS asks for the Keychain again each time.

use crate::api::session::SessionTokens;

const SERVICE: &str = "com.bitroot.ayeaye-desktop";

fn use_file() -> bool {
    std::env::var("AYEAYE_TOKEN_STORE").is_ok_and(|v| v == "file")
}

fn file_path(profile_id: &str) -> std::path::PathBuf {
    super::config_dir()
        .join("tokens")
        .join(format!("{profile_id}.json"))
}

pub fn load(profile_id: &str) -> SessionTokens {
    if use_file() {
        return super::read_json(&file_path(profile_id)).unwrap_or_default();
    }
    let entry = match keyring::Entry::new(SERVICE, profile_id) {
        Ok(entry) => entry,
        Err(e) => {
            log::error!("keychain unavailable: {e}");
            return SessionTokens::default();
        }
    };
    match entry.get_password() {
        Ok(json) => serde_json::from_str(&json).unwrap_or_default(),
        Err(keyring::Error::NoEntry) => SessionTokens::default(),
        Err(e) => {
            log::error!("could not read tokens for {profile_id}: {e}");
            SessionTokens::default()
        }
    }
}

pub fn save(profile_id: &str, tokens: &SessionTokens) {
    if !tokens.has_session() && !tokens.has_refresh() {
        return forget(profile_id);
    }
    if use_file() {
        if let Err(e) = super::write_json(&file_path(profile_id), tokens) {
            log::error!("could not save tokens: {e}");
        }
        return;
    }
    let result = keyring::Entry::new(SERVICE, profile_id)
        .and_then(|entry| entry.set_password(&serde_json::to_string(tokens).unwrap_or_default()));
    if let Err(e) = result {
        log::error!("could not save tokens for {profile_id}: {e}");
    }
}

pub fn forget(profile_id: &str) {
    if use_file() {
        let _ = std::fs::remove_file(file_path(profile_id));
        return;
    }
    if let Ok(entry) = keyring::Entry::new(SERVICE, profile_id) {
        let _ = entry.delete_credential();
    }
}

/// The personal access token the realtime socket signs in with. `WS /api/ws`
/// takes a session cookie or a PAT, and a header-mode client has no cookie,
/// so one is minted after sign-in (read scope: the socket carries no
/// content). Kept next to the session, under its own key.
#[derive(Clone, Debug, serde::Serialize, serde::Deserialize)]
pub struct SocketToken {
    pub id: String,
    pub token: String,
}

fn socket_key(profile_id: &str) -> String {
    format!("{profile_id}.socket")
}

pub fn load_socket(profile_id: &str) -> Option<SocketToken> {
    let key = socket_key(profile_id);
    if use_file() {
        return super::read_json(&file_path(&key));
    }
    let entry = keyring::Entry::new(SERVICE, &key).ok()?;
    match entry.get_password() {
        Ok(json) => serde_json::from_str(&json).ok(),
        Err(keyring::Error::NoEntry) => None,
        Err(e) => {
            log::error!("could not read the socket token for {profile_id}: {e}");
            None
        }
    }
}

pub fn save_socket(profile_id: &str, token: &SocketToken) {
    let key = socket_key(profile_id);
    if use_file() {
        if let Err(e) = super::write_json(&file_path(&key), token) {
            log::error!("could not save the socket token: {e}");
        }
        return;
    }
    let result = keyring::Entry::new(SERVICE, &key)
        .and_then(|entry| entry.set_password(&serde_json::to_string(token).unwrap_or_default()));
    if let Err(e) = result {
        log::error!("could not save the socket token for {profile_id}: {e}");
    }
}

pub fn forget_socket(profile_id: &str) {
    forget(&socket_key(profile_id));
}

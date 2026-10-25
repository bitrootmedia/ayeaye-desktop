//! Profiles: one ayeaye instance is one URL plus one credential set.
//!
//! ayeaye is single-origin, so a profile needs nothing more than its base URL;
//! self-hosted instances are the normal case and two profiles signed in at
//! once (work and personal) is expected. Tokens are not in here — they live in
//! the keychain, keyed by the profile's local id.

use serde::{Deserialize, Serialize};

use super::{config_dir, read_json, write_json};

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Profile {
    /// Local-only identity, so two profiles can share a hostname.
    pub id: String,
    pub label: String,
    /// Scheme included; `http://` is allowed.
    pub base_url: String,
    /// Kept so the switcher renders before `/api/me` answers.
    #[serde(default)]
    pub email: Option<String>,
    #[serde(default)]
    pub display_name: Option<String>,
    #[serde(default)]
    pub user_id: Option<String>,
    /// The web app drops you back into the last organisation; so do we.
    #[serde(default)]
    pub last_org_id: Option<String>,
}

impl Profile {
    pub fn new(label: &str, base_url: &str) -> Self {
        Self {
            id: new_id(),
            label: label.trim().to_owned(),
            base_url: base_url.to_owned(),
            email: None,
            display_name: None,
            user_id: None,
            last_org_id: None,
        }
    }

    pub fn host(&self) -> &str {
        let rest = self
            .base_url
            .split_once("://")
            .map(|(_, rest)| rest)
            .unwrap_or(&self.base_url);
        rest.split('/').next().unwrap_or(rest)
    }

    pub fn is_insecure(&self) -> bool {
        self.base_url.starts_with("http://")
    }

    pub fn person(&self) -> String {
        self.display_name
            .clone()
            .filter(|n| !n.trim().is_empty())
            .or_else(|| self.email.clone())
            .unwrap_or_else(|| self.label.clone())
    }
}

#[derive(Clone, Debug, Default, Serialize, Deserialize)]
pub struct ProfileList {
    #[serde(default)]
    pub profiles: Vec<Profile>,
    #[serde(default)]
    pub active: Option<String>,
}

impl ProfileList {
    fn path() -> std::path::PathBuf {
        config_dir().join("profiles.json")
    }

    pub fn load() -> Self {
        read_json(&Self::path()).unwrap_or_default()
    }

    pub fn save(&self) {
        if let Err(e) = write_json(&Self::path(), self) {
            log::error!("could not save profiles: {e}");
        }
    }

    pub fn get(&self, id: &str) -> Option<&Profile> {
        self.profiles.iter().find(|p| p.id == id)
    }

    pub fn active(&self) -> Option<&Profile> {
        self.active
            .as_deref()
            .and_then(|id| self.get(id))
            .or_else(|| self.profiles.first())
    }

    pub fn upsert(&mut self, profile: Profile) {
        match self.profiles.iter_mut().find(|p| p.id == profile.id) {
            Some(existing) => *existing = profile,
            None => self.profiles.push(profile),
        }
    }

    pub fn update(&mut self, id: &str, f: impl FnOnce(&mut Profile)) {
        if let Some(profile) = self.profiles.iter_mut().find(|p| p.id == id) {
            f(profile);
        }
    }

    pub fn remove(&mut self, id: &str) {
        self.profiles.retain(|p| p.id != id);
        if self.active.as_deref() == Some(id) {
            self.active = self.profiles.first().map(|p| p.id.clone());
        }
    }
}

fn new_id() -> String {
    use std::time::{SystemTime, UNIX_EPOCH};
    let nanos = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_nanos())
        .unwrap_or_default();
    // Unique enough for a list a person curates by hand, and readable in a
    // keychain listing.
    format!("p{:x}{:04x}", nanos, std::process::id() & 0xffff)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn host_and_person() {
        let mut p = Profile::new("Work", "https://tasks.example.com/");
        assert_eq!(p.host(), "tasks.example.com");
        assert!(!p.is_insecure());
        assert_eq!(p.person(), "Work");
        p.email = Some("ada@example.com".into());
        assert_eq!(p.person(), "ada@example.com");
        p.display_name = Some("Ada".into());
        assert_eq!(p.person(), "Ada");
    }

    #[test]
    fn removing_the_active_profile_falls_back_to_another() {
        let a = Profile::new("A", "https://a");
        let b = Profile::new("B", "https://b");
        let mut list = ProfileList::default();
        list.upsert(a.clone());
        list.upsert(b.clone());
        list.active = Some(a.id.clone());
        list.remove(&a.id);
        assert_eq!(list.active(), Some(&b));
    }
}

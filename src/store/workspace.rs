//! What was open where, so the app reopens the way it was left — the thing
//! that makes a notes app feel like a pile of text files you never lose.

use std::collections::HashMap;

use serde::{Deserialize, Serialize};

use super::{data_dir, read_json, write_json};

#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct OpenTabs {
    /// Item keys in tab order, e.g. `note:<uuid>`.
    #[serde(default)]
    pub items: Vec<String>,
    #[serde(default)]
    pub active: Option<String>,
}

#[derive(Clone, Debug, Default, Serialize, Deserialize)]
pub struct WorkspaceState {
    /// Keyed by `<profile id>/<organisation id>`.
    #[serde(default)]
    tabs: HashMap<String, OpenTabs>,
    #[serde(default = "default_true")]
    pub sidebar_open: bool,
    /// The module last shown on the rail.
    #[serde(default)]
    pub module: Option<String>,
}

fn default_true() -> bool {
    true
}

impl WorkspaceState {
    fn path() -> std::path::PathBuf {
        data_dir().join("workspace.json")
    }

    pub fn load() -> Self {
        read_json(&Self::path()).unwrap_or(Self {
            sidebar_open: true,
            ..Default::default()
        })
    }

    pub fn save(&self) {
        if let Err(e) = write_json(&Self::path(), self) {
            log::warn!("could not save workspace state: {e}");
        }
    }

    pub fn tabs(&self, profile_id: &str, org_id: &str) -> OpenTabs {
        self.tabs
            .get(&format!("{profile_id}/{org_id}"))
            .cloned()
            .unwrap_or_default()
    }

    pub fn set_tabs(&mut self, profile_id: &str, org_id: &str, tabs: OpenTabs) {
        self.tabs.insert(format!("{profile_id}/{org_id}"), tabs);
    }

    pub fn forget_profile(&mut self, profile_id: &str) {
        let prefix = format!("{profile_id}/");
        self.tabs.retain(|k, _| !k.starts_with(&prefix));
    }
}

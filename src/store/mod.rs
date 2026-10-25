//! Everything the app keeps on disk.
//!
//! - `profiles.json` (config dir): which instances you use, never a credential
//! - the OS keychain: each profile's session tokens ([`vault`])
//! - the cache dir: the last notes list per profile and organisation, so the
//!   window has something to show before the network answers
//! - `workspace.json` (data dir): open tabs per organisation, restored on launch

pub mod profiles;
pub mod vault;
pub mod workspace;

use std::path::{Path, PathBuf};

use serde::Serialize;
use serde::de::DeserializeOwned;

const APP_DIR: &str = "ayeaye-desktop";

pub fn config_dir() -> PathBuf {
    std::env::var_os("AYEAYE_CONFIG_DIR")
        .map(PathBuf::from)
        .unwrap_or_else(|| {
            dirs::config_dir()
                .unwrap_or_else(std::env::temp_dir)
                .join(APP_DIR)
        })
}

pub fn cache_dir() -> PathBuf {
    config_dir_override("cache").unwrap_or_else(|| {
        dirs::cache_dir()
            .unwrap_or_else(std::env::temp_dir)
            .join(APP_DIR)
    })
}

pub fn data_dir() -> PathBuf {
    config_dir_override("data").unwrap_or_else(|| {
        dirs::data_dir()
            .unwrap_or_else(std::env::temp_dir)
            .join(APP_DIR)
    })
}

/// With `AYEAYE_CONFIG_DIR` set (tests, a second dev instance), everything
/// lives under it so nothing touches the real profile.
fn config_dir_override(sub: &str) -> Option<PathBuf> {
    std::env::var_os("AYEAYE_CONFIG_DIR").map(|d| PathBuf::from(d).join(sub))
}

pub fn read_json<T: DeserializeOwned>(path: &Path) -> Option<T> {
    let bytes = std::fs::read(path).ok()?;
    match serde_json::from_slice(&bytes) {
        Ok(value) => Some(value),
        Err(e) => {
            log::warn!("ignoring unreadable {}: {e}", path.display());
            None
        }
    }
}

/// Write-then-rename, so a crash mid-write leaves the old file rather than
/// half of a new one.
pub fn write_json(path: &Path, value: &impl Serialize) -> std::io::Result<()> {
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)?;
    }
    // Unique per write: two threads saving the same file at once (rotated
    // tokens from parallel requests) must not rename each other's temp file.
    static WRITES: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
    let n = WRITES.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
    let tmp = path.with_extension(format!("json.{}.{n}.tmp", std::process::id()));
    std::fs::write(&tmp, serde_json::to_vec_pretty(value)?)?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(&tmp, std::fs::Permissions::from_mode(0o600))?;
    }
    std::fs::rename(tmp, path)
}

/// Cached notes: `<cache>/<profile>/<org>/notes.json`.
pub fn notes_cache_path(profile_id: &str, org_id: &str) -> PathBuf {
    cache_dir().join(profile_id).join(org_id).join("notes.json")
}

pub fn orgs_cache_path(profile_id: &str) -> PathBuf {
    cache_dir().join(profile_id).join("organisations.json")
}

pub fn forget_profile_cache(profile_id: &str) {
    let _ = std::fs::remove_dir_all(cache_dir().join(profile_id));
}

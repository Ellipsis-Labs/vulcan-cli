//! Persisted record of the last successful `vulcan agent install`.
//!
//! When the user runs `vulcan agent install --target X --scope Y`, the chosen
//! target and scope are written to `<vulcan_dir>/agent-install.json` so future
//! `vulcan update check` runs can suggest an update command that refreshes
//! skills for the right client instead of silently defaulting to claude/user.

use std::path::{Path, PathBuf};
use std::time::{SystemTime, UNIX_EPOCH};

use serde::{Deserialize, Serialize};

const STATE_FILE: &str = "agent-install.json";

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct AgentInstallState {
    pub target: String,
    pub scope: String,
    pub installed_at_unix: u64,
}

impl AgentInstallState {
    pub fn new(target: impl Into<String>, scope: impl Into<String>) -> Self {
        let installed_at_unix = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map(|d| d.as_secs())
            .unwrap_or(0);
        Self {
            target: target.into(),
            scope: scope.into(),
            installed_at_unix,
        }
    }

    pub fn path(vulcan_dir: &Path) -> PathBuf {
        vulcan_dir.join(STATE_FILE)
    }

    pub fn load(vulcan_dir: &Path) -> Option<Self> {
        let bytes = std::fs::read(Self::path(vulcan_dir)).ok()?;
        serde_json::from_slice(&bytes).ok()
    }

    pub fn save(&self, vulcan_dir: &Path) -> std::io::Result<()> {
        let path = Self::path(vulcan_dir);
        let bytes = serde_json::to_vec_pretty(self).map_err(std::io::Error::other)?;
        let tmp = path.with_extension("json.tmp");
        std::fs::write(&tmp, &bytes)?;
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            std::fs::set_permissions(&tmp, std::fs::Permissions::from_mode(0o600))?;
        }
        std::fs::rename(&tmp, &path)?;
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::TempDir;

    #[test]
    fn roundtrip_state_through_disk() {
        let dir = TempDir::new().unwrap();
        let state = AgentInstallState::new("cursor", "project");
        state.save(dir.path()).unwrap();

        let loaded = AgentInstallState::load(dir.path()).expect("state present");
        assert_eq!(loaded, state);
    }

    #[test]
    fn load_returns_none_when_missing() {
        let dir = TempDir::new().unwrap();
        assert!(AgentInstallState::load(dir.path()).is_none());
    }

    #[test]
    fn load_returns_none_on_corrupt_json() {
        let dir = TempDir::new().unwrap();
        std::fs::write(AgentInstallState::path(dir.path()), b"not json").unwrap();
        assert!(AgentInstallState::load(dir.path()).is_none());
    }
}

//! Phoenix API auth session helpers.

use crate::config::VulcanConfig;
use crate::error::VulcanError;
use phoenix_rise::api::{
    AuthSession, AuthSessionSnapshot, FileAuthSessionStore, PhoenixHttpAuthConfig,
    PhoenixHttpClient, PhoenixHttpClientBuilder,
};
use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct ApiAuthMetadata {
    pub wallet: Option<String>,
    pub wallet_name: Option<String>,
    pub logged_in_at: Option<String>,
}

pub fn auth_dir(vulcan_dir: &Path) -> PathBuf {
    vulcan_dir.join("auth")
}

pub fn session_path(vulcan_dir: &Path) -> PathBuf {
    auth_dir(vulcan_dir).join("session.json")
}

pub fn metadata_path(vulcan_dir: &Path) -> PathBuf {
    auth_dir(vulcan_dir).join("session-meta.json")
}

pub fn build_http_client(
    config: &VulcanConfig,
    vulcan_dir: &Path,
) -> Result<(PhoenixHttpClient, Option<String>), VulcanError> {
    match build_authenticated_http_client(&config.network.api_url, vulcan_dir) {
        Ok(client) => Ok((client, None)),
        Err(auth_error) => match PhoenixHttpClient::new_public(&config.network.api_url) {
            Ok(client) => Ok((client, Some(auth_error))),
            Err(err) => Err(VulcanError::api("HTTP_CLIENT_INIT_FAILED", err.to_string())),
        },
    }
}

fn build_authenticated_http_client(
    api_url: &str,
    vulcan_dir: &Path,
) -> Result<PhoenixHttpClient, String> {
    let path = session_path(vulcan_dir);
    let auth = PhoenixHttpAuthConfig::new()
        .with_file_session_store_path(path)
        .map_err(|e| e.to_string())?;
    PhoenixHttpClientBuilder::new(api_url)
        .with_auth(auth)
        .build()
        .map_err(|e| e.to_string())
}

pub fn load_session(vulcan_dir: &Path) -> Result<Option<AuthSession>, VulcanError> {
    let path = session_path(vulcan_dir);
    let store = FileAuthSessionStore::new(path);
    phoenix_rise::api::AuthSessionStore::load_session(&store)
        .map_err(|e| VulcanError::auth("API_AUTH_SESSION_LOAD_FAILED", e.to_string()))
}

pub fn load_session_snapshot(
    vulcan_dir: &Path,
) -> Result<Option<AuthSessionSnapshot>, VulcanError> {
    let Some(session) = load_session(vulcan_dir)? else {
        return Ok(None);
    };
    Ok(Some(session.snapshot()))
}

pub fn store_metadata(vulcan_dir: &Path, metadata: &ApiAuthMetadata) -> Result<(), VulcanError> {
    let dir = auth_dir(vulcan_dir);
    std::fs::create_dir_all(&dir)
        .map_err(|e| VulcanError::io("API_AUTH_DIR_CREATE_FAILED", e.to_string()))?;
    let payload = serde_json::to_vec_pretty(metadata)
        .map_err(|e| VulcanError::internal("API_AUTH_METADATA_SERIALIZE_FAILED", e.to_string()))?;
    std::fs::write(metadata_path(vulcan_dir), payload)
        .map_err(|e| VulcanError::io("API_AUTH_METADATA_WRITE_FAILED", e.to_string()))?;
    Ok(())
}

pub fn load_metadata(vulcan_dir: &Path) -> Option<ApiAuthMetadata> {
    let content = std::fs::read_to_string(metadata_path(vulcan_dir)).ok()?;
    serde_json::from_str(&content).ok()
}

pub fn clear_session(vulcan_dir: &Path) -> Result<(), VulcanError> {
    for path in [session_path(vulcan_dir), metadata_path(vulcan_dir)] {
        match std::fs::remove_file(&path) {
            Ok(()) => {}
            Err(err) if err.kind() == std::io::ErrorKind::NotFound => {}
            Err(err) => {
                return Err(VulcanError::io(
                    "API_AUTH_SESSION_CLEAR_FAILED",
                    format!("failed to remove {}: {}", path.display(), err),
                ));
            }
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn http_client_uses_session_store_before_session_exists() {
        let tmp = tempfile::tempdir().unwrap();
        let config = VulcanConfig::default();

        let (client, load_error) = build_http_client(&config, tmp.path()).unwrap();

        assert!(load_error.is_none());
        assert!(client.auth().is_some());
        assert!(!session_path(tmp.path()).exists());
    }
}

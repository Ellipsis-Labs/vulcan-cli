//! Wallet file storage — manages encrypted wallets in `~/.vulcan/wallets/`.

use anyhow::{anyhow, Result};
use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};

use crate::crypto::EncryptedData;

/// Metadata stored alongside encrypted wallet data.
#[derive(Debug, Serialize, Deserialize)]
pub struct WalletFile {
    pub name: String,
    pub public_key: String,
    pub encrypted: EncryptedData,
    pub created_at: String,
}

/// Manages the wallet directory (`~/.vulcan/wallets/`).
#[derive(Clone)]
pub struct WalletStore {
    wallets_dir: PathBuf,
}

impl WalletStore {
    /// Create a new WalletStore, ensuring the wallets directory exists.
    pub fn new(vulcan_dir: &Path) -> Result<Self> {
        let wallets_dir = vulcan_dir.join("wallets");
        std::fs::create_dir_all(&wallets_dir)?;
        Ok(Self { wallets_dir })
    }

    /// Path to a wallet file by name.
    pub fn wallet_path(&self, name: &str) -> PathBuf {
        self.wallets_dir.join(format!("{}.json", name))
    }

    /// Check if a wallet exists.
    pub fn exists(&self, name: &str) -> bool {
        self.wallet_path(name).exists()
    }

    /// Save a wallet file.
    pub fn save(&self, wallet_file: &WalletFile) -> Result<()> {
        let path = self.wallet_path(&wallet_file.name);
        let json = serde_json::to_string_pretty(wallet_file)?;
        std::fs::write(&path, json)?;

        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o600))?;
        }

        Ok(())
    }

    /// Load a wallet file by name.
    pub fn load(&self, name: &str) -> Result<WalletFile> {
        let path = self.wallet_path(name);
        if !path.exists() {
            return Err(anyhow!("Wallet '{}' not found", name));
        }
        let content = std::fs::read_to_string(&path)?;
        let wallet_file: WalletFile = serde_json::from_str(&content)?;
        Ok(wallet_file)
    }

    /// List all wallet names.
    pub fn list(&self) -> Result<Vec<String>> {
        let mut names = Vec::new();
        for entry in std::fs::read_dir(&self.wallets_dir)? {
            let entry = entry?;
            let path = entry.path();
            if path.extension().is_some_and(|ext| ext == "json") {
                if let Some(stem) = path.file_stem() {
                    names.push(stem.to_string_lossy().to_string());
                }
            }
        }
        names.sort();
        Ok(names)
    }

    /// Remove a wallet by name.
    pub fn remove(&self, name: &str) -> Result<()> {
        let path = self.wallet_path(name);
        if !path.exists() {
            return Err(anyhow!("Wallet '{}' not found", name));
        }
        std::fs::remove_file(&path)?;
        Ok(())
    }

    /// Get or set the default wallet name.
    pub fn default_wallet(&self) -> Result<Option<String>> {
        let default_path = self.wallets_dir.join("default");
        if default_path.exists() {
            let name = std::fs::read_to_string(&default_path)?.trim().to_string();
            if self.exists(&name) {
                return Ok(Some(name));
            }
        }
        Ok(None)
    }

    /// Set the default wallet.
    pub fn set_default(&self, name: &str) -> Result<()> {
        if !self.exists(name) {
            return Err(anyhow!("Wallet '{}' not found", name));
        }
        let default_path = self.wallets_dir.join("default");
        std::fs::write(default_path, name)?;
        Ok(())
    }
}

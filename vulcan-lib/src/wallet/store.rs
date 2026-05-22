//! Wallet file storage — manages encrypted wallets in `~/.vulcan/wallets/`.

use anyhow::{anyhow, Result};
use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};

use crate::crypto::EncryptedData;

/// Metadata stored alongside encrypted wallet data.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct WalletFile {
    pub name: String,
    pub public_key: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub encrypted: Option<EncryptedData>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub signer: Option<WalletSignerConfig>,
    pub created_at: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum WalletSignerConfig {
    LocalEncrypted,
    Vault {
        vault_addr: String,
        token_env: String,
        key_name: String,
    },
    AwsKms {
        key_id: String,
        region: Option<String>,
    },
    GcpKms {
        key_name: String,
    },
    Turnkey {
        api_public_key_env: String,
        api_private_key_env: String,
        organization_id: String,
        private_key_id: String,
        api_base_url: Option<String>,
    },
    Privy {
        app_id_env: String,
        app_secret_env: String,
        wallet_id: String,
        api_base_url: Option<String>,
    },
    Cdp {
        api_key_id_env: String,
        api_key_secret_env: String,
        wallet_secret_env: String,
        api_base_url: Option<String>,
    },
    Para {
        api_key_env: String,
        wallet_id: String,
        api_base_url: Option<String>,
    },
    Crossmint {
        api_key_env: String,
        wallet_locator: String,
        signer_secret_env: Option<String>,
        signer: Option<String>,
        api_base_url: Option<String>,
    },
    Dfns {
        auth_token_env: String,
        cred_id: String,
        private_key_pem_env: String,
        wallet_id: String,
        api_base_url: Option<String>,
    },
    Openfort {
        secret_key_env: String,
        account_id: String,
        wallet_secret_env: String,
        api_base_url: Option<String>,
    },
}

impl WalletSignerConfig {
    pub fn kind(&self) -> &'static str {
        match self {
            Self::LocalEncrypted => "local_encrypted",
            Self::Vault { .. } => "vault",
            Self::AwsKms { .. } => "aws_kms",
            Self::GcpKms { .. } => "gcp_kms",
            Self::Turnkey { .. } => "turnkey",
            Self::Privy { .. } => "privy",
            Self::Cdp { .. } => "cdp",
            Self::Para { .. } => "para",
            Self::Crossmint { .. } => "crossmint",
            Self::Dfns { .. } => "dfns",
            Self::Openfort { .. } => "openfort",
        }
    }

    pub fn required_env_vars(&self) -> Vec<String> {
        match self {
            Self::LocalEncrypted => Vec::new(),
            Self::Vault { token_env, .. } => vec![token_env.clone()],
            Self::AwsKms { .. } | Self::GcpKms { .. } => Vec::new(),
            Self::Turnkey {
                api_public_key_env,
                api_private_key_env,
                ..
            } => vec![api_public_key_env.clone(), api_private_key_env.clone()],
            Self::Privy {
                app_id_env,
                app_secret_env,
                ..
            } => vec![app_id_env.clone(), app_secret_env.clone()],
            Self::Cdp {
                api_key_id_env,
                api_key_secret_env,
                wallet_secret_env,
                ..
            } => vec![
                api_key_id_env.clone(),
                api_key_secret_env.clone(),
                wallet_secret_env.clone(),
            ],
            Self::Para { api_key_env, .. } => vec![api_key_env.clone()],
            Self::Crossmint {
                api_key_env,
                signer_secret_env,
                ..
            } => {
                let mut vars = vec![api_key_env.clone()];
                if let Some(env) = signer_secret_env {
                    vars.push(env.clone());
                }
                vars
            }
            Self::Dfns {
                auth_token_env,
                private_key_pem_env,
                ..
            } => vec![auth_token_env.clone(), private_key_pem_env.clone()],
            Self::Openfort {
                secret_key_env,
                wallet_secret_env,
                ..
            } => vec![secret_key_env.clone(), wallet_secret_env.clone()],
        }
    }
}

impl WalletFile {
    pub fn local_encrypted(
        name: String,
        public_key: String,
        encrypted: EncryptedData,
        created_at: String,
    ) -> Self {
        Self {
            name,
            public_key,
            encrypted: Some(encrypted),
            signer: Some(WalletSignerConfig::LocalEncrypted),
            created_at,
        }
    }

    pub fn remote(
        name: String,
        public_key: String,
        signer: WalletSignerConfig,
        created_at: String,
    ) -> Self {
        Self {
            name,
            public_key,
            encrypted: None,
            signer: Some(signer),
            created_at,
        }
    }

    pub fn signer_config(&self) -> WalletSignerConfig {
        self.signer
            .clone()
            .unwrap_or(WalletSignerConfig::LocalEncrypted)
    }

    pub fn signer_kind(&self) -> &'static str {
        self.signer_config().kind()
    }

    pub fn encrypted_data(&self) -> Result<&EncryptedData, crate::error::VulcanError> {
        self.encrypted.as_ref().ok_or_else(|| {
            crate::error::VulcanError::auth(
                "LOCAL_WALLET_REQUIRED",
                format!("Wallet '{}' is not a local encrypted wallet", self.name),
            )
        })
    }

    pub fn is_local_encrypted(&self) -> bool {
        matches!(self.signer_config(), WalletSignerConfig::LocalEncrypted)
    }
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn legacy_wallet_file_without_signer_defaults_to_local_encrypted() {
        let json = r#"{
  "name": "legacy",
  "public_key": "11111111111111111111111111111111",
  "encrypted": {
    "salt": "salt",
    "nonce": "nonce",
    "ciphertext": "ciphertext"
  },
  "created_at": "now"
}"#;

        let wallet: WalletFile = serde_json::from_str(json).unwrap();

        assert_eq!(wallet.signer_kind(), "local_encrypted");
        assert!(wallet.encrypted_data().is_ok());
    }

    #[test]
    fn remote_wallet_file_serializes_without_encrypted_data() {
        let wallet = WalletFile::remote(
            "vault".to_string(),
            "11111111111111111111111111111111".to_string(),
            WalletSignerConfig::Vault {
                vault_addr: "https://vault.example.com".to_string(),
                token_env: "VAULT_TOKEN".to_string(),
                key_name: "solana-main".to_string(),
            },
            "now".to_string(),
        );

        let value = serde_json::to_value(&wallet).unwrap();

        assert!(value.get("encrypted").is_none());
        assert_eq!(value["signer"]["type"], "vault");
    }
}

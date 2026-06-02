//! Wallet signing abstraction.
//!
//! Legacy encrypted wallet files are adapted into `solana-keychain` memory
//! signers here so transaction and message signing do not depend on raw
//! `Keypair` access at call sites.

use crate::error::VulcanError;
use crate::wallet::{Wallet, WalletFile, WalletSignerConfig};
#[cfg(feature = "aws-kms-signer")]
use solana_keychain::{AwsKmsSigner, AwsKmsSignerConfig};
use solana_keychain::{
    CdpSigner, CdpSignerConfig, CrossmintSigner, CrossmintSignerConfig, DfnsSigner,
    DfnsSignerConfig, GcpKmsSigner, GcpKmsSignerConfig, MemorySigner, OpenfortSigner,
    OpenfortSignerConfig, ParaSigner, ParaSignerConfig, PrivySigner, PrivySignerConfig,
    Signer as KeychainSigner, SolanaSigner, TurnkeySigner, TurnkeySignerConfig, VaultSigner,
    VaultSignerConfig,
};
use solana_pubkey::Pubkey;
use std::str::FromStr;
use std::sync::Arc;

type DynSigner = dyn SolanaSigner;

#[derive(Clone)]
pub struct ResolvedSigner {
    signer: Option<Arc<DynSigner>>,
    pub wallet_name: String,
    pub public_key: String,
    pub authority: Pubkey,
    pub trader_pda: Pubkey,
}

impl ResolvedSigner {
    pub async fn from_wallet_file(
        wallet_file: &WalletFile,
        password: Option<&str>,
    ) -> Result<Self, VulcanError> {
        let signer = match wallet_file.signer_config() {
            WalletSignerConfig::LocalEncrypted => {
                let password = password.ok_or_else(|| {
                    VulcanError::auth(
                        "WALLET_PASSWORD_REQUIRED",
                        "Wallet password is required for local encrypted wallet signing.",
                    )
                })?;
                let encrypted = wallet_file.encrypted_data()?;
                let wallet = Wallet::decrypt(encrypted, password)
                    .map_err(|e| VulcanError::auth("DECRYPT_FAILED", e.to_string()))?;
                return Self::from_unlocked_wallet_file(wallet_file, &wallet);
            }
            WalletSignerConfig::Vault {
                vault_addr,
                token_env,
                key_name,
            } => KeychainSigner::Vault(
                VaultSigner::from_config(VaultSignerConfig {
                    vault_addr,
                    token: env_secret(&token_env)?,
                    key_name,
                    pubkey: wallet_file.public_key.clone(),
                    http_client_config: None,
                })
                .map_err(signer_init_failed)?,
            ),
            WalletSignerConfig::AwsKms { key_id, region } => {
                #[cfg(feature = "aws-kms-signer")]
                {
                    KeychainSigner::AwsKms(
                        AwsKmsSigner::from_config(AwsKmsSignerConfig {
                            key_id,
                            public_key: wallet_file.public_key.clone(),
                            region,
                        })
                        .await
                        .map_err(signer_init_failed)?,
                    )
                }
                #[cfg(not(feature = "aws-kms-signer"))]
                {
                    let _ = (key_id, region);
                    return Err(VulcanError::auth(
                        "SIGNER_FEATURE_DISABLED",
                        "AWS KMS signer support is not enabled in this Vulcan build.",
                    ));
                }
            }
            WalletSignerConfig::GcpKms { key_name } => KeychainSigner::GcpKms(
                GcpKmsSigner::from_config(GcpKmsSignerConfig {
                    key_name,
                    public_key: wallet_file.public_key.clone(),
                })
                .await
                .map_err(signer_init_failed)?,
            ),
            WalletSignerConfig::Turnkey {
                api_public_key_env,
                api_private_key_env,
                organization_id,
                private_key_id,
                api_base_url,
            } => KeychainSigner::Turnkey(
                TurnkeySigner::from_config(TurnkeySignerConfig {
                    api_public_key: env_secret(&api_public_key_env)?,
                    api_private_key: env_secret(&api_private_key_env)?,
                    organization_id,
                    private_key_id,
                    public_key: wallet_file.public_key.clone(),
                    api_base_url,
                    http_client_config: None,
                })
                .map_err(signer_init_failed)?,
            ),
            WalletSignerConfig::Privy {
                app_id_env,
                app_secret_env,
                wallet_id,
                api_base_url,
            } => {
                let mut signer = PrivySigner::from_config(PrivySignerConfig {
                    app_id: env_secret(&app_id_env)?,
                    app_secret: env_secret(&app_secret_env)?,
                    wallet_id,
                    api_base_url,
                    http_client_config: None,
                });
                signer.init().await.map_err(signer_init_failed)?;
                KeychainSigner::Privy(signer)
            }
            WalletSignerConfig::Cdp {
                api_key_id_env,
                api_key_secret_env,
                wallet_secret_env,
                api_base_url,
            } => KeychainSigner::Cdp(
                CdpSigner::from_config(CdpSignerConfig {
                    api_key_id: env_secret(&api_key_id_env)?,
                    api_key_secret: env_secret(&api_key_secret_env)?,
                    wallet_secret: env_secret(&wallet_secret_env)?,
                    address: wallet_file.public_key.clone(),
                    api_base_url,
                    http_client_config: None,
                })
                .map_err(signer_init_failed)?,
            ),
            WalletSignerConfig::Para {
                api_key_env,
                wallet_id,
                api_base_url,
            } => {
                let mut signer = ParaSigner::from_config(ParaSignerConfig {
                    api_key: env_secret(&api_key_env)?,
                    wallet_id,
                    api_base_url,
                })
                .map_err(signer_init_failed)?;
                signer.init().await.map_err(signer_init_failed)?;
                KeychainSigner::Para(signer)
            }
            WalletSignerConfig::Crossmint {
                api_key_env,
                wallet_locator,
                signer_secret_env,
                signer,
                api_base_url,
            } => {
                let mut signer = CrossmintSigner::new(CrossmintSignerConfig {
                    api_key: env_secret(&api_key_env)?,
                    wallet_locator,
                    signer_secret: optional_env_secret(signer_secret_env.as_deref())?,
                    signer,
                    api_base_url,
                    poll_interval_ms: None,
                    max_poll_attempts: None,
                })
                .map_err(signer_init_failed)?;
                signer.init().await.map_err(signer_init_failed)?;
                KeychainSigner::Crossmint(signer)
            }
            WalletSignerConfig::Dfns {
                auth_token_env,
                cred_id,
                private_key_pem_env,
                wallet_id,
                api_base_url,
            } => {
                let mut signer = DfnsSigner::new(DfnsSignerConfig {
                    auth_token: env_secret(&auth_token_env)?,
                    cred_id,
                    private_key_pem: env_secret(&private_key_pem_env)?,
                    wallet_id,
                    api_base_url,
                    http_client_config: None,
                });
                signer.init().await.map_err(signer_init_failed)?;
                KeychainSigner::Dfns(signer)
            }
            WalletSignerConfig::Openfort {
                secret_key_env,
                account_id,
                wallet_secret_env,
                api_base_url,
            } => {
                let mut signer = OpenfortSigner::from_config(OpenfortSignerConfig {
                    secret_key: env_secret(&secret_key_env)?,
                    account_id,
                    wallet_secret: env_secret(&wallet_secret_env)?,
                    api_base_url,
                    http_client_config: None,
                })
                .map_err(signer_init_failed)?;
                signer.init().await.map_err(signer_init_failed)?;
                KeychainSigner::Openfort(signer)
            }
        };

        let signer_pubkey = signer.pubkey().to_string();
        if signer_pubkey != wallet_file.public_key {
            return Err(VulcanError::auth(
                "SIGNER_PUBKEY_MISMATCH",
                format!(
                    "Signer for wallet '{}' resolved to {}, but metadata expects {}",
                    wallet_file.name, signer_pubkey, wallet_file.public_key
                ),
            ));
        }
        let authority = Pubkey::from_str(&wallet_file.public_key)
            .map_err(|e| VulcanError::validation("INVALID_PUBKEY", e.to_string()))?;
        Ok(Self::new(
            wallet_file.name.clone(),
            authority,
            Some(Arc::new(signer) as Arc<DynSigner>),
        ))
    }

    pub fn from_unlocked_wallet_file(
        wallet_file: &WalletFile,
        wallet: &Wallet,
    ) -> Result<Self, VulcanError> {
        if wallet.address() != wallet_file.public_key {
            return Err(VulcanError::auth(
                "WALLET_PUBKEY_MISMATCH",
                format!(
                    "Wallet '{}' decrypted to {}, but metadata expects {}",
                    wallet_file.name,
                    wallet.address(),
                    wallet_file.public_key
                ),
            ));
        }

        Self::from_unlocked_wallet(wallet_file.name.clone(), wallet)
    }

    pub fn from_unlocked_wallet(wallet_name: String, wallet: &Wallet) -> Result<Self, VulcanError> {
        let authority = Pubkey::from_str(wallet.address())
            .map_err(|e| VulcanError::validation("INVALID_PUBKEY", e.to_string()))?;
        let signer = MemorySigner::from_bytes(wallet.to_bytes())
            .map_err(|e| VulcanError::auth("SIGNER_INIT_FAILED", e.to_string()))?;
        Ok(Self::new(
            wallet_name,
            authority,
            Some(Arc::new(signer) as Arc<DynSigner>),
        ))
    }

    pub fn dry_run(wallet_name: String, authority: Pubkey) -> Self {
        Self::new(wallet_name, authority, None)
    }

    pub(crate) fn from_session_parts(
        wallet_name: String,
        authority: Pubkey,
        signer: Arc<DynSigner>,
    ) -> Self {
        Self::new(wallet_name, authority, Some(signer))
    }

    fn new(wallet_name: String, authority: Pubkey, signer: Option<Arc<DynSigner>>) -> Self {
        let trader_pda = phoenix_rise::types::TraderKey::new(authority).pda();
        Self {
            signer,
            wallet_name,
            public_key: authority.to_string(),
            authority,
            trader_pda,
        }
    }

    pub fn signer(&self) -> Result<&DynSigner, VulcanError> {
        self.signer.as_deref().ok_or_else(|| {
            VulcanError::auth(
                "SIGNER_REQUIRED",
                "A live signer is required for this operation. Dry-run authority metadata cannot sign.",
            )
        })
    }

    pub(crate) fn signer_arc(&self) -> Result<Arc<DynSigner>, VulcanError> {
        self.signer.clone().ok_or_else(|| {
            VulcanError::auth(
                "SIGNER_REQUIRED",
                "A live signer is required for this operation. Dry-run authority metadata cannot sign.",
            )
        })
    }
}

fn env_secret(name: &str) -> Result<String, VulcanError> {
    std::env::var(name).map_err(|_| {
        VulcanError::auth(
            "SIGNER_SECRET_ENV_MISSING",
            format!(
                "Required signer secret environment variable '{}' is not set",
                name
            ),
        )
    })
}

fn optional_env_secret(name: Option<&str>) -> Result<Option<String>, VulcanError> {
    name.map(env_secret).transpose()
}

fn signer_init_failed(error: impl std::fmt::Display) -> VulcanError {
    VulcanError::auth("SIGNER_INIT_FAILED", error.to_string())
}

impl std::fmt::Debug for ResolvedSigner {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("ResolvedSigner")
            .field("wallet_name", &self.wallet_name)
            .field("public_key", &self.public_key)
            .field("authority", &self.authority)
            .field("trader_pda", &self.trader_pda)
            .field("signer", &self.signer.as_ref().map(|_| "[REDACTED]"))
            .finish()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn unlocked_wallet_signs_messages_through_keychain() {
        let wallet = Wallet::generate().unwrap();
        let signer = ResolvedSigner::from_unlocked_wallet("test".to_string(), &wallet).unwrap();

        let signature = signer
            .signer()
            .unwrap()
            .sign_message(b"hello")
            .await
            .unwrap();

        assert_eq!(signature.as_ref().len(), 64);
        assert_eq!(signer.public_key, wallet.address());
    }

    #[test]
    fn dry_run_signer_cannot_sign() {
        let wallet = Wallet::generate().unwrap();
        let authority = Pubkey::from_str(wallet.address()).unwrap();
        let signer = ResolvedSigner::dry_run("test".to_string(), authority);

        assert!(signer.signer().is_err());
    }

    #[test]
    fn wallet_file_metadata_mismatch_is_rejected() {
        let wallet = Wallet::generate().unwrap();
        let wallet_file = WalletFile::local_encrypted(
            "test".to_string(),
            Pubkey::new_unique().to_string(),
            wallet.encrypt("password").unwrap(),
            "now".to_string(),
        );

        let err = ResolvedSigner::from_unlocked_wallet_file(&wallet_file, &wallet).unwrap_err();
        assert_eq!(err.code, "WALLET_PUBKEY_MISMATCH");
    }

    #[cfg(not(feature = "aws-kms-signer"))]
    #[tokio::test]
    async fn aws_kms_signer_requires_feature() {
        let wallet_file = WalletFile::remote(
            "aws".to_string(),
            "11111111111111111111111111111111".to_string(),
            WalletSignerConfig::AwsKms {
                key_id: "test-key".to_string(),
                region: None,
            },
            "now".to_string(),
        );

        let err = ResolvedSigner::from_wallet_file(&wallet_file, None)
            .await
            .unwrap_err();

        assert_eq!(err.code, "SIGNER_FEATURE_DISABLED");
    }
}

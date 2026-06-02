//! Session wallet — holds an unlocked signer for the MCP session lifetime.

use crate::error::VulcanError;
use crate::wallet::{ResolvedSigner, Wallet, WalletFile};
use solana_keychain::SolanaSigner;
use solana_pubkey::Pubkey;
use std::str::FromStr;
use std::sync::Arc;
use zeroize::Zeroizing;

/// Holds an unlocked signer for the duration of an MCP session.
pub struct SessionWallet {
    signer: Arc<dyn SolanaSigner>,
    /// Raw key bytes captured for local encrypted wallets; zeroized on drop.
    _key_guard: Option<Zeroizing<Vec<u8>>>,
    pub wallet_name: String,
    pub public_key: String,
    pub authority: Pubkey,
    pub trader_pda: Pubkey,
}

impl SessionWallet {
    pub fn from_resolved(signer: ResolvedSigner) -> Result<Self, VulcanError> {
        Ok(Self {
            signer: signer.signer_arc()?,
            _key_guard: None,
            wallet_name: signer.wallet_name,
            public_key: signer.public_key,
            authority: signer.authority,
            trader_pda: signer.trader_pda,
        })
    }

    /// Create a new session wallet from a decrypted wallet and its file metadata.
    pub fn new(wallet: &Wallet, wallet_file: &WalletFile) -> Result<Self, VulcanError> {
        let authority = Pubkey::from_str(&wallet_file.public_key)
            .map_err(|e| VulcanError::validation("INVALID_PUBKEY", e.to_string()))?;
        let key_guard = Zeroizing::new(wallet.to_bytes().to_vec());
        let resolved = ResolvedSigner::from_unlocked_wallet_file(wallet_file, wallet)?;
        let trader_key = phoenix_rise::types::TraderKey::new(authority);
        let trader_pda = trader_key.pda();

        Ok(Self {
            signer: resolved.signer_arc()?,
            _key_guard: Some(key_guard),
            wallet_name: wallet_file.name.clone(),
            public_key: wallet_file.public_key.clone(),
            authority,
            trader_pda,
        })
    }

    pub fn resolved_signer(&self) -> ResolvedSigner {
        ResolvedSigner::from_session_parts(
            self.wallet_name.clone(),
            self.authority,
            self.signer.clone(),
        )
    }
}

impl std::fmt::Debug for SessionWallet {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("SessionWallet")
            .field("public_key", &self.public_key)
            .field("wallet_name", &self.wallet_name)
            .field("authority", &self.authority)
            .field("trader_pda", &self.trader_pda)
            .field("signer", &"[REDACTED]")
            .finish()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::wallet::{Wallet, WalletFile};

    #[test]
    fn debug_redacts_session_wallet_signer() {
        let wallet = Wallet::generate().unwrap();
        let wallet_file = WalletFile::local_encrypted(
            "test".to_string(),
            wallet.public_key.clone(),
            wallet.encrypt("password").unwrap(),
            "now".to_string(),
        );
        let session_wallet = SessionWallet::new(&wallet, &wallet_file).unwrap();
        let debug = format!("{:?}", session_wallet);

        assert!(debug.contains("[REDACTED]"));
        assert!(!debug.contains("signer: Some"));
    }
}

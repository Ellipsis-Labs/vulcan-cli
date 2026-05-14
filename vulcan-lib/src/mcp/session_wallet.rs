//! Session wallet — holds decrypted wallet bytes for the MCP session lifetime.

use crate::error::VulcanError;
use crate::wallet::{Wallet, WalletFile};
use solana_pubkey::Pubkey;
use std::str::FromStr;
use zeroize::Zeroize;

/// Holds decrypted wallet bytes in memory for the duration of an MCP session.
/// Bytes are zeroized on drop.
pub struct SessionWallet {
    bytes: Vec<u8>,
    pub wallet_name: String,
    pub public_key: String,
    pub authority: Pubkey,
    pub trader_pda: Pubkey,
}

impl SessionWallet {
    /// Create a new session wallet from a decrypted wallet and its file metadata.
    pub fn new(wallet: &Wallet, wallet_file: &WalletFile) -> Result<Self, VulcanError> {
        let bytes = wallet.to_bytes().to_vec();

        let authority = Pubkey::from_str(&wallet_file.public_key)
            .map_err(|e| VulcanError::validation("INVALID_PUBKEY", e.to_string()))?;

        let trader_key = phoenix_rise::types::TraderKey::new(authority);
        let trader_pda = trader_key.pda();

        Ok(Self {
            bytes,
            wallet_name: wallet_file.name.clone(),
            public_key: wallet_file.public_key.clone(),
            authority,
            trader_pda,
        })
    }

    /// Reconstruct a Wallet from the stored bytes.
    pub fn to_wallet(&self) -> Result<Wallet, VulcanError> {
        Wallet::from_bytes(&self.bytes)
            .map_err(|e| VulcanError::internal("SESSION_WALLET_ERROR", e.to_string()))
    }
}

impl Drop for SessionWallet {
    fn drop(&mut self) {
        self.bytes.zeroize();
    }
}

impl std::fmt::Debug for SessionWallet {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("SessionWallet")
            .field("public_key", &self.public_key)
            .field("wallet_name", &self.wallet_name)
            .field("authority", &self.authority)
            .field("trader_pda", &self.trader_pda)
            .field("bytes", &"[REDACTED]")
            .finish()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::wallet::{Wallet, WalletFile};

    #[test]
    fn debug_redacts_session_wallet_bytes() {
        let wallet = Wallet::generate().unwrap();
        let wallet_file = WalletFile {
            name: "test".to_string(),
            public_key: wallet.public_key.clone(),
            encrypted: wallet.encrypt("password").unwrap(),
            created_at: "now".to_string(),
        };
        let session_wallet = SessionWallet::new(&wallet, &wallet_file).unwrap();
        let debug = format!("{:?}", session_wallet);

        assert!(debug.contains("[REDACTED]"));
        assert!(!debug.contains("bytes: ["));
    }
}

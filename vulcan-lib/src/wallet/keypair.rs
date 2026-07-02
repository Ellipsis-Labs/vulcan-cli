//! Solana keypair handling
//!
//! Implements secure key management with memory zeroization on drop.
//! Extracted from quant/src/wallet/keypair.rs

use anyhow::{anyhow, Result};
use solana_sdk::signer::keypair::Keypair;
use solana_sdk::signer::Signer;
use std::path::Path;
use zeroize::{Zeroize, ZeroizeOnDrop};

use crate::crypto::{decrypt, encrypt, EncryptedData};

/// How the private key was provided
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum WalletSource {
    /// Base58 encoded string
    Base58,
    /// Byte array [u8; 64]
    ByteArray,
    /// Loaded from file
    File,
}

/// A Solana wallet (keypair)
///
/// Private key material is automatically zeroed from memory when dropped.
/// This struct intentionally does not implement Clone to prevent
/// accidental duplication of sensitive key material in memory.
#[derive(Zeroize, ZeroizeOnDrop)]
pub struct Wallet {
    /// The raw keypair bytes (64 bytes: 32 private + 32 public)
    /// Automatically zeroed on drop via ZeroizeOnDrop
    keypair_bytes: Vec<u8>,
    /// The public key (base58 encoded)
    #[zeroize(skip)]
    pub public_key: String,
}

impl Wallet {
    /// Create a wallet from a base58-encoded private key
    pub fn from_base58(base58_key: &str) -> Result<Self> {
        let mut bytes = bs58::decode(base58_key)
            .into_vec()
            .map_err(|e| anyhow!("Invalid base58 encoding: {}", e))?;

        let result = Self::from_bytes(&bytes);
        bytes.zeroize();
        result
    }

    /// Create a wallet from a byte array
    pub fn from_bytes(bytes: &[u8]) -> Result<Self> {
        if bytes.len() != 64 {
            return Err(anyhow!(
                "Invalid keypair length: expected 64 bytes, got {}",
                bytes.len()
            ));
        }

        let public_key_bytes = &bytes[32..64];
        let public_key = bs58::encode(public_key_bytes).into_string();

        Ok(Self {
            keypair_bytes: bytes.to_vec(),
            public_key,
        })
    }

    /// Load a wallet from a JSON file (Solana CLI format)
    pub fn from_file(path: &Path) -> Result<Self> {
        let content = std::fs::read_to_string(path)
            .map_err(|e| anyhow!("Failed to read keypair file: {}", e))?;

        let mut bytes: Vec<u8> = serde_json::from_str(&content)
            .map_err(|e| anyhow!("Failed to parse keypair file: {}", e))?;

        let result = Self::from_bytes(&bytes);
        bytes.zeroize();
        result
    }

    /// Generate a new random wallet
    pub fn generate() -> Result<Self> {
        let keypair = Keypair::new();
        let bytes = keypair.to_bytes();
        Self::from_bytes(&bytes)
    }

    /// Get the wallet's public key (address)
    pub fn address(&self) -> &str {
        &self.public_key
    }

    /// Get the raw keypair bytes
    pub fn to_bytes(&self) -> &[u8] {
        &self.keypair_bytes
    }

    /// Get a Solana SDK Keypair from this wallet
    pub fn to_solana_keypair(&self) -> Result<solana_sdk::signature::Keypair> {
        let keypair_bytes: [u8; 64] = self
            .keypair_bytes
            .as_slice()
            .try_into()
            .map_err(|_| anyhow!("Invalid keypair length: expected 64 bytes"))?;
        solana_sdk::signature::Keypair::try_from(&keypair_bytes[..])
            .map_err(|e| anyhow!("Failed to create Solana keypair: {}", e))
    }

    /// Encrypt the wallet with a password
    pub fn encrypt(&self, password: &str) -> Result<EncryptedData> {
        encrypt(&self.keypair_bytes, password)
    }

    /// Decrypt a wallet from encrypted data
    pub fn decrypt(encrypted: &EncryptedData, password: &str) -> Result<Self> {
        let mut bytes = decrypt(encrypted, password)?;
        let result = Self::from_bytes(&bytes);
        bytes.zeroize();
        result
    }

    /// Save encrypted wallet to a file
    pub fn save_encrypted(&self, path: &Path, password: &str) -> Result<()> {
        let encrypted = self.encrypt(password)?;
        let json = serde_json::to_string_pretty(&encrypted)?;
        std::fs::write(path, json)?;

        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o600))?;
        }

        Ok(())
    }

    /// Load encrypted wallet from a file
    pub fn load_encrypted(path: &Path, password: &str) -> Result<Self> {
        let content = std::fs::read_to_string(path)?;
        let encrypted: EncryptedData = serde_json::from_str(&content)?;
        Self::decrypt(&encrypted, password)
    }

    /// Sign a message with the wallet's private key
    pub fn sign(&self, message: &[u8]) -> Result<Vec<u8>> {
        let keypair = self.to_keypair()?;
        let signature = keypair.sign_message(message);
        Ok(signature.as_ref().to_vec())
    }

    /// Get a Keypair for use with solana_sdk Transaction signing
    pub fn to_keypair(&self) -> Result<Keypair> {
        Keypair::try_from(self.keypair_bytes.as_slice())
            .map_err(|e| anyhow!("Invalid keypair bytes: {}", e))
    }
}

impl std::fmt::Debug for Wallet {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Wallet")
            .field("public_key", &self.public_key)
            .field("keypair_bytes", &"[REDACTED]")
            .finish()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_wallet_from_bytes() {
        let mut bytes = vec![0u8; 64];
        bytes[32..64].copy_from_slice(&[1u8; 32]);

        let wallet = Wallet::from_bytes(&bytes).unwrap();
        assert!(!wallet.public_key.is_empty());
    }

    #[test]
    fn test_wallet_encrypt_decrypt() {
        let bytes = vec![42u8; 64];
        let wallet = Wallet::from_bytes(&bytes).unwrap();
        let password = "test_password";

        let encrypted = wallet.encrypt(password).unwrap();
        let decrypted = Wallet::decrypt(&encrypted, password).unwrap();

        assert_eq!(wallet.keypair_bytes, decrypted.keypair_bytes);
    }

    #[test]
    fn test_invalid_keypair_length() {
        let bytes = vec![0u8; 32];
        let result = Wallet::from_bytes(&bytes);
        assert!(result.is_err());
    }

    #[test]
    fn test_wallet_generate() {
        let wallet = Wallet::generate().unwrap();
        assert!(!wallet.public_key.is_empty());
        assert_eq!(wallet.to_bytes().len(), 64);
    }

    #[test]
    fn test_wallet_debug_redacts_keypair_bytes() {
        let wallet = Wallet::generate().unwrap();
        let debug = format!("{:?}", wallet);

        assert!(debug.contains("[REDACTED]"));
        assert!(!debug.contains("keypair_bytes: ["));
    }
}

//! AES-256-GCM encryption with Argon2id key derivation
//!
//! Extracted from quant/src/crypto/encryption.rs

use aes_gcm::{
    aead::{Aead, KeyInit},
    Aes256Gcm, Nonce,
};
use anyhow::{anyhow, Result};
use argon2::{password_hash::SaltString, Algorithm, Argon2, Params, PasswordHasher, Version};
use rand::{rngs::OsRng, RngCore};
use serde::{Deserialize, Serialize};

/// Encrypted data with metadata needed for decryption
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct EncryptedData {
    /// Salt used for key derivation (base64 encoded)
    pub salt: String,
    /// Nonce/IV for AES-GCM (base64 encoded)
    pub nonce: String,
    /// Encrypted ciphertext (base64 encoded)
    pub ciphertext: String,
}

impl EncryptedData {
    /// Serialize to a single string for storage
    #[allow(clippy::inherent_to_string)]
    pub fn to_string(&self) -> String {
        format!("encrypted:{}:{}:{}", self.salt, self.nonce, self.ciphertext)
    }

    /// Parse from a serialized string
    pub fn from_string(s: &str) -> Result<Self> {
        let s = s.strip_prefix("encrypted:").unwrap_or(s);
        let parts: Vec<&str> = s.split(':').collect();

        if parts.len() != 3 {
            return Err(anyhow!("Invalid encrypted data format"));
        }

        Ok(Self {
            salt: parts[0].to_string(),
            nonce: parts[1].to_string(),
            ciphertext: parts[2].to_string(),
        })
    }
}

/// Derive a 256-bit key from a password using Argon2id
///
/// Uses OWASP-recommended parameters:
/// - Memory: 19456 KiB (~19 MiB)
/// - Iterations: 3
/// - Parallelism: 1
fn derive_key(password: &str, salt: &[u8]) -> Result<[u8; 32]> {
    let salt_string =
        SaltString::encode_b64(salt).map_err(|e| anyhow!("Failed to encode salt: {}", e))?;

    let params = Params::new(19456, 3, 1, Some(32))
        .map_err(|e| anyhow!("Failed to create Argon2 params: {}", e))?;
    let argon2 = Argon2::new(Algorithm::Argon2id, Version::V0x13, params);

    let password_hash = argon2
        .hash_password(password.as_bytes(), &salt_string)
        .map_err(|e| anyhow!("Failed to hash password: {}", e))?;

    let hash = password_hash
        .hash
        .ok_or_else(|| anyhow!("No hash output"))?;
    let hash_bytes = hash.as_bytes();

    let mut key = [0u8; 32];
    key.copy_from_slice(&hash_bytes[..32]);

    Ok(key)
}

/// Encrypt data using AES-256-GCM with Argon2id key derivation
pub fn encrypt(plaintext: &[u8], password: &str) -> Result<EncryptedData> {
    let mut salt = [0u8; 16];
    OsRng.fill_bytes(&mut salt);

    let key = derive_key(password, &salt)?;

    let mut nonce_bytes = [0u8; 12];
    OsRng.fill_bytes(&mut nonce_bytes);
    let nonce = Nonce::from_slice(&nonce_bytes);

    let cipher =
        Aes256Gcm::new_from_slice(&key).map_err(|e| anyhow!("Failed to create cipher: {}", e))?;

    let ciphertext = cipher
        .encrypt(nonce, plaintext)
        .map_err(|e| anyhow!("Encryption failed: {}", e))?;

    Ok(EncryptedData {
        salt: base64::Engine::encode(&base64::engine::general_purpose::STANDARD, salt),
        nonce: base64::Engine::encode(&base64::engine::general_purpose::STANDARD, nonce_bytes),
        ciphertext: base64::Engine::encode(&base64::engine::general_purpose::STANDARD, &ciphertext),
    })
}

/// Decrypt data using AES-256-GCM with Argon2id key derivation
pub fn decrypt(encrypted: &EncryptedData, password: &str) -> Result<Vec<u8>> {
    use base64::Engine;

    let salt = base64::engine::general_purpose::STANDARD
        .decode(&encrypted.salt)
        .map_err(|e| anyhow!("Failed to decode salt: {}", e))?;

    let nonce_bytes = base64::engine::general_purpose::STANDARD
        .decode(&encrypted.nonce)
        .map_err(|e| anyhow!("Failed to decode nonce: {}", e))?;

    let ciphertext = base64::engine::general_purpose::STANDARD
        .decode(&encrypted.ciphertext)
        .map_err(|e| anyhow!("Failed to decode ciphertext: {}", e))?;

    let key = derive_key(password, &salt)?;

    let cipher =
        Aes256Gcm::new_from_slice(&key).map_err(|e| anyhow!("Failed to create cipher: {}", e))?;

    let nonce = Nonce::from_slice(&nonce_bytes);

    let plaintext = cipher
        .decrypt(nonce, ciphertext.as_ref())
        .map_err(|_| anyhow!("Decryption failed - invalid password or corrupted data"))?;

    Ok(plaintext)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_encrypt_decrypt_roundtrip() {
        let plaintext = b"Hello, World!";
        let password = "test_password_123";

        let encrypted = encrypt(plaintext, password).unwrap();
        let decrypted = decrypt(&encrypted, password).unwrap();

        assert_eq!(plaintext.to_vec(), decrypted);
    }

    #[test]
    fn test_wrong_password_fails() {
        let plaintext = b"Secret data";
        let password = "correct_password";
        let wrong_password = "wrong_password";

        let encrypted = encrypt(plaintext, password).unwrap();
        let result = decrypt(&encrypted, wrong_password);

        assert!(result.is_err());
    }

    #[test]
    fn test_encrypted_data_serialization() {
        let data = EncryptedData {
            salt: "salt123".to_string(),
            nonce: "nonce456".to_string(),
            ciphertext: "cipher789".to_string(),
        };

        let serialized = data.to_string();
        let deserialized = EncryptedData::from_string(&serialized).unwrap();

        assert_eq!(data.salt, deserialized.salt);
        assert_eq!(data.nonce, deserialized.nonce);
        assert_eq!(data.ciphertext, deserialized.ciphertext);
    }
}

//! Cryptographic utilities for Vulcan CLI
//!
//! Provides AES-256-GCM encryption with Argon2id key derivation.
//! Extracted from the Quant project.

mod encryption;

pub use encryption::{decrypt, encrypt, EncryptedData};

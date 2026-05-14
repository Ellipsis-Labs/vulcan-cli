//! Wallet management for Solana keypairs
//!
//! Handles loading, storing, and using Solana keypairs with encrypted storage.
//! Extracted from the Quant project.

mod keypair;
pub mod store;

pub use keypair::{Wallet, WalletSource};
pub use store::{WalletFile, WalletStore};

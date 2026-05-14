//! Wallet command execution.

use crate::cli::wallet::{ImportFormat, PrivateKeyExportFormat, WalletCommand};
use crate::context::AppContext;
use crate::error::{ErrorCategory, VulcanError};
use crate::output::{render_success, TableRenderable};
use crate::wallet::Wallet;
use crate::wallet::WalletFile;
use serde::Serialize;
use solana_pubkey::Pubkey;
use zeroize::Zeroize;

/// Derive the associated token account address (no external dependency needed).
fn spl_associated_token_address(wallet: &Pubkey, mint: &Pubkey) -> Pubkey {
    let spl_token_program =
        Pubkey::try_from("TokenkegQfeZyiNwAJbNbGKPFXCWuBvf9Ss623VQ5DA").unwrap();
    let ata_program = Pubkey::try_from("ATokenGPvbdGVxr1b2hvZbsiqW5xWH25efTNsLJA8knL").unwrap();
    let seeds = &[wallet.as_ref(), spl_token_program.as_ref(), mint.as_ref()];
    Pubkey::find_program_address(seeds, &ata_program).0
}

#[derive(Debug, Serialize)]
pub struct WalletInfo {
    pub name: String,
    pub public_key: String,
    pub is_default: bool,
}

#[derive(Debug, Serialize)]
pub struct WalletAddress {
    pub name: String,
    pub address: String,
    pub is_default: bool,
}

impl TableRenderable for WalletInfo {
    fn render_table(&self) {
        crate::output::table::render_table(
            &["Name", "Public Key", "Default"],
            vec![vec![
                self.name.clone(),
                self.public_key.clone(),
                if self.is_default {
                    "yes".into()
                } else {
                    "no".into()
                },
            ]],
        );
    }
}

#[derive(Debug, Serialize)]
pub struct WalletList {
    pub wallets: Vec<WalletInfo>,
}

impl TableRenderable for WalletList {
    fn render_table(&self) {
        if self.wallets.is_empty() {
            println!("No wallets found. Create one with: vulcan wallet create --name <NAME>");
            return;
        }
        let rows: Vec<Vec<String>> = self
            .wallets
            .iter()
            .map(|w| {
                vec![
                    w.name.clone(),
                    w.public_key.clone(),
                    if w.is_default {
                        "yes".into()
                    } else {
                        "no".into()
                    },
                ]
            })
            .collect();
        crate::output::table::render_table(&["Name", "Public Key", "Default"], rows);
    }
}

#[derive(Debug, Serialize)]
pub struct WalletCreated {
    pub name: String,
    pub public_key: String,
}

impl TableRenderable for WalletCreated {
    fn render_table(&self) {
        println!("Wallet '{}' created successfully.", self.name);
        println!("Public key: {}", self.public_key);
    }
}

#[derive(Debug, Serialize)]
pub struct WalletRemoved {
    pub name: String,
}

impl TableRenderable for WalletRemoved {
    fn render_table(&self) {
        println!("Wallet '{}' removed.", self.name);
    }
}

#[derive(Serialize)]
pub struct WalletExport {
    pub name: String,
    pub public_key: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub output_path: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub encrypted_wallet: Option<serde_json::Value>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub private_key: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub private_key_format: Option<String>,
}

impl TableRenderable for WalletExport {
    fn render_table(&self) {
        if let Some(private_key) = &self.private_key {
            println!("{private_key}");
        } else if let Some(wallet) = &self.encrypted_wallet {
            println!("{}", serde_json::to_string_pretty(wallet).unwrap());
        } else if let Some(path) = &self.output_path {
            println!("Encrypted wallet '{}' exported to {}", self.name, path);
            println!("Public key: {}", self.public_key);
        } else {
            println!("{}", self.public_key);
        }
    }
}

impl Drop for WalletExport {
    fn drop(&mut self) {
        if let Some(private_key) = &mut self.private_key {
            private_key.zeroize();
        }
    }
}

#[derive(Debug, Serialize)]
pub struct WalletBalance {
    pub name: String,
    pub address: String,
    pub sol: f64,
    pub usdc: f64,
}

impl TableRenderable for WalletBalance {
    fn render_table(&self) {
        println!("Wallet '{}'", self.name);
        println!("  Address: {}", self.address);
        println!("  SOL:     {:.9} SOL", self.sol);
        println!("  USDC:    {:.6} USDC", self.usdc);
    }
}

#[derive(Debug, Serialize)]
pub struct DefaultSet {
    pub name: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub previous: Option<String>,
}

impl TableRenderable for DefaultSet {
    fn render_table(&self) {
        println!("Default wallet set to '{}'.", self.name);
        match &self.previous {
            Some(prev) if prev != &self.name => {
                println!("(was '{prev}'. Revert with `vulcan wallet set-default {prev}`.)");
                println!(
                    "Tip: to launch a single command on a non-default wallet without changing the default, use the global `-w <name>` flag — e.g. `vulcan -w {} strategy ta start ...`",
                    self.name
                );
            }
            _ => {}
        }
    }
}

pub async fn execute(ctx: &AppContext, cmd: WalletCommand) -> Result<(), VulcanError> {
    match cmd {
        WalletCommand::Create { name } => {
            let password = crate::commands::trade::prompt_password()?;
            let result = execute_create_inner(ctx, &name, &password)?;
            render_success(ctx.output_format, &result, serde_json::Value::Null);
            Ok(())
        }

        WalletCommand::Import {
            name,
            format,
            source,
        } => {
            if ctx.wallet_store.exists(&name) {
                return Err(VulcanError::validation(
                    "WALLET_EXISTS",
                    format!("Wallet '{}' already exists", name),
                ));
            }

            let wallet = match format {
                ImportFormat::Base58 => Wallet::from_base58(&source),
                ImportFormat::Bytes => {
                    let bytes: Vec<u8> = serde_json::from_str(&source).map_err(|e| {
                        VulcanError::validation(
                            "INVALID_BYTES",
                            format!("Invalid byte array: {}", e),
                        )
                    })?;
                    Wallet::from_bytes(&bytes)
                }
                ImportFormat::File => Wallet::from_file(std::path::Path::new(&source)),
            }
            .map_err(|e| VulcanError::validation("IMPORT_FAILED", e.to_string()))?;

            let password = crate::commands::trade::prompt_password()?;

            let encrypted = wallet
                .encrypt(&password)
                .map_err(|e| VulcanError::internal("ENCRYPT_FAILED", e.to_string()))?;

            let wallet_file = WalletFile {
                name: name.clone(),
                public_key: wallet.public_key.clone(),
                encrypted,
                created_at: chrono::Utc::now().to_rfc3339(),
            };

            ctx.wallet_store.save(&wallet_file).map_err(|e| {
                VulcanError::new(
                    crate::error::ErrorCategory::Io,
                    "SAVE_FAILED",
                    e.to_string(),
                )
            })?;

            let result = WalletCreated {
                name,
                public_key: wallet.public_key.clone(),
            };
            render_success(ctx.output_format, &result, serde_json::Value::Null);
            Ok(())
        }

        WalletCommand::List => {
            let names = ctx.wallet_store.list().map_err(|e| {
                VulcanError::new(
                    crate::error::ErrorCategory::Io,
                    "LIST_FAILED",
                    e.to_string(),
                )
            })?;

            let default_name = ctx.wallet_store.default_wallet().ok().flatten();

            let wallets: Vec<WalletInfo> = names
                .into_iter()
                .map(|name| {
                    let public_key = ctx
                        .wallet_store
                        .load(&name)
                        .map(|f| f.public_key)
                        .unwrap_or_else(|_| "???".into());
                    let is_default = default_name.as_deref() == Some(&name);
                    WalletInfo {
                        name,
                        public_key,
                        is_default,
                    }
                })
                .collect();

            let result = WalletList { wallets };
            render_success(ctx.output_format, &result, serde_json::Value::Null);
            Ok(())
        }

        WalletCommand::Show { name } => {
            let wallet_file = ctx
                .wallet_store
                .load(&name)
                .map_err(|e| VulcanError::auth("WALLET_NOT_FOUND", e.to_string()))?;
            let default_name = ctx.wallet_store.default_wallet().ok().flatten();
            let is_default = default_name.as_deref() == Some(name.as_str());

            let result = WalletInfo {
                name,
                public_key: wallet_file.public_key,
                is_default,
            };
            render_success(ctx.output_format, &result, serde_json::Value::Null);
            Ok(())
        }

        WalletCommand::SetDefault { name } => {
            let previous = ctx.wallet_store.default_wallet().ok().flatten();
            ctx.wallet_store
                .set_default(&name)
                .map_err(|e| VulcanError::auth("WALLET_NOT_FOUND", e.to_string()))?;

            let result = DefaultSet { name, previous };
            render_success(ctx.output_format, &result, serde_json::Value::Null);
            Ok(())
        }

        WalletCommand::Remove { name } => {
            if !ctx.yes {
                // TODO: interactive confirmation prompt
                eprintln!("Use --yes to confirm removal of wallet '{}'", name);
                return Err(VulcanError::validation(
                    "CONFIRMATION_REQUIRED",
                    "Pass --yes to confirm wallet removal",
                ));
            }

            ctx.wallet_store
                .remove(&name)
                .map_err(|e| VulcanError::auth("WALLET_NOT_FOUND", e.to_string()))?;

            let result = WalletRemoved { name };
            render_success(ctx.output_format, &result, serde_json::Value::Null);
            Ok(())
        }

        WalletCommand::Export {
            name,
            output,
            stdout,
            private_key,
            private_key_format,
            force,
        } => {
            let wallet_file = ctx
                .wallet_store
                .load(&name)
                .map_err(|e| VulcanError::auth("WALLET_NOT_FOUND", e.to_string()))?;

            if private_key && !ctx.yes {
                return Err(VulcanError::new(
                    ErrorCategory::DangerousGate,
                    "PRIVATE_KEY_EXPORT_REQUIRES_CONFIRMATION",
                    "Plaintext private-key export requires --yes",
                ));
            }

            let wallet_json = serde_json::to_string_pretty(&wallet_file)
                .map_err(|e| VulcanError::internal("EXPORT_ENCODE_FAILED", e.to_string()))?;

            let output_path = if let Some(path) = output {
                if path.exists() && !force {
                    return Err(VulcanError::validation(
                        "OUTPUT_EXISTS",
                        format!("Output file already exists: {}", path.display()),
                    ));
                }
                if let Some(parent) = path.parent() {
                    if !parent.as_os_str().is_empty() && !parent.exists() {
                        return Err(VulcanError::validation(
                            "OUTPUT_DIR_NOT_FOUND",
                            format!("Output directory not found: {}", parent.display()),
                        ));
                    }
                }

                std::fs::write(&path, &wallet_json)
                    .map_err(|e| VulcanError::io("EXPORT_WRITE_FAILED", e.to_string()))?;

                #[cfg(unix)]
                {
                    use std::os::unix::fs::PermissionsExt;
                    std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o600))
                        .map_err(|e| VulcanError::io("EXPORT_PERMISSIONS_FAILED", e.to_string()))?;
                }

                Some(path.to_string_lossy().to_string())
            } else {
                None
            };

            let encrypted_wallet =
                if stdout {
                    Some(serde_json::from_str(&wallet_json).map_err(|e| {
                        VulcanError::internal("EXPORT_ENCODE_FAILED", e.to_string())
                    })?)
                } else {
                    None
                };

            let (private_key_value, private_key_format_value) = if private_key {
                let password = crate::commands::trade::prompt_password()?;
                let wallet = Wallet::decrypt(&wallet_file.encrypted, &password)
                    .map_err(|e| VulcanError::auth("DECRYPT_FAILED", e.to_string()))?;
                let (value, format) = match private_key_format {
                    PrivateKeyExportFormat::Base58 => (
                        bs58::encode(wallet.to_bytes()).into_string(),
                        "base58".to_string(),
                    ),
                    PrivateKeyExportFormat::Bytes => (
                        serde_json::to_string(wallet.to_bytes()).map_err(|e| {
                            VulcanError::internal("EXPORT_ENCODE_FAILED", e.to_string())
                        })?,
                        "bytes".to_string(),
                    ),
                };
                (Some(value), Some(format))
            } else {
                (None, None)
            };

            let result = WalletExport {
                name,
                public_key: wallet_file.public_key,
                output_path,
                encrypted_wallet,
                private_key: private_key_value,
                private_key_format: private_key_format_value,
            };
            render_success(ctx.output_format, &result, serde_json::Value::Null);
            Ok(())
        }

        WalletCommand::Balance { name } => {
            let wallet_name = ctx.resolved_wallet_name(name.as_deref())?;

            let wallet_file = ctx
                .wallet_store
                .load(&wallet_name)
                .map_err(|e| VulcanError::auth("WALLET_NOT_FOUND", e.to_string()))?;

            let pubkey = solana_pubkey::Pubkey::try_from(wallet_file.public_key.as_str())
                .map_err(|e| VulcanError::validation("INVALID_PUBKEY", e.to_string()))?;

            let rpc_client = ctx.rpc_client();

            // SOL balance
            let sol_lamports = rpc_client
                .get_balance(&pubkey)
                .map_err(|e| VulcanError::network("RPC_BALANCE_FAILED", e.to_string()))?;
            let sol = sol_lamports as f64 / 1_000_000_000.0;

            // USDC balance — derive the associated token account
            let usdc_mint =
                solana_pubkey::Pubkey::try_from("EPjFWdd5AufqSSqeM2qN1xzybapC8G4wEGGkZwyTDt1v")
                    .unwrap();
            let ata = spl_associated_token_address(&pubkey, &usdc_mint);

            let usdc = match rpc_client.get_token_account_balance(&ata) {
                Ok(balance) => balance.ui_amount.unwrap_or(0.0),
                Err(_) => 0.0, // No token account = 0 USDC
            };

            let result = WalletBalance {
                name: wallet_name,
                address: wallet_file.public_key,
                sol,
                usdc,
            };
            render_success(ctx.output_format, &result, serde_json::Value::Null);
            Ok(())
        }
    }
}

// ── Inner functions for MCP ────────────────────────────────────────────

pub fn execute_list_inner(ctx: &AppContext) -> Result<WalletList, VulcanError> {
    let names = ctx.wallet_store.list().map_err(|e| {
        VulcanError::new(
            crate::error::ErrorCategory::Io,
            "LIST_FAILED",
            e.to_string(),
        )
    })?;

    let default_name = ctx.wallet_store.default_wallet().ok().flatten();

    let wallets: Vec<WalletInfo> = names
        .into_iter()
        .map(|name| {
            let public_key = ctx
                .wallet_store
                .load(&name)
                .map(|f| f.public_key)
                .unwrap_or_else(|_| "???".into());
            let is_default = default_name.as_deref() == Some(&name);
            WalletInfo {
                name,
                public_key,
                is_default,
            }
        })
        .collect();

    Ok(WalletList { wallets })
}

pub fn execute_create_inner(
    ctx: &AppContext,
    name: &str,
    password: &str,
) -> Result<WalletCreated, VulcanError> {
    if ctx.wallet_store.exists(name) {
        return Err(VulcanError::validation(
            "WALLET_EXISTS",
            format!("Wallet '{}' already exists", name),
        ));
    }

    let wallet =
        Wallet::generate().map_err(|e| VulcanError::internal("KEYGEN_FAILED", e.to_string()))?;

    let encrypted = wallet
        .encrypt(password)
        .map_err(|e| VulcanError::internal("ENCRYPT_FAILED", e.to_string()))?;

    let wallet_file = WalletFile {
        name: name.to_string(),
        public_key: wallet.public_key.clone(),
        encrypted,
        created_at: chrono::Utc::now().to_rfc3339(),
    };

    ctx.wallet_store.save(&wallet_file).map_err(|e| {
        VulcanError::new(
            crate::error::ErrorCategory::Io,
            "SAVE_FAILED",
            e.to_string(),
        )
    })?;

    Ok(WalletCreated {
        name: name.to_string(),
        public_key: wallet.public_key.clone(),
    })
}

pub fn execute_address_inner(
    ctx: &AppContext,
    name: Option<&str>,
) -> Result<WalletAddress, VulcanError> {
    let wallet_name = ctx.resolved_wallet_name(name)?;
    let wallet_file = ctx
        .wallet_store
        .load(&wallet_name)
        .map_err(|e| VulcanError::auth("WALLET_NOT_FOUND", e.to_string()))?;
    let default_name = ctx.wallet_store.default_wallet().ok().flatten();
    Ok(WalletAddress {
        is_default: default_name.as_deref() == Some(wallet_name.as_str()),
        name: wallet_name,
        address: wallet_file.public_key,
    })
}

pub fn execute_balance_inner(
    ctx: &AppContext,
    name: Option<&str>,
) -> Result<WalletBalance, VulcanError> {
    let wallet_name = ctx.resolved_wallet_name(name)?;

    let wallet_file = ctx
        .wallet_store
        .load(&wallet_name)
        .map_err(|e| VulcanError::auth("WALLET_NOT_FOUND", e.to_string()))?;

    let pubkey = Pubkey::try_from(wallet_file.public_key.as_str())
        .map_err(|e| VulcanError::validation("INVALID_PUBKEY", e.to_string()))?;

    let rpc_client = ctx.rpc_client();

    let sol_lamports = rpc_client
        .get_balance(&pubkey)
        .map_err(|e| VulcanError::network("RPC_BALANCE_FAILED", e.to_string()))?;
    let sol = sol_lamports as f64 / 1_000_000_000.0;

    let usdc_mint = Pubkey::try_from("EPjFWdd5AufqSSqeM2qN1xzybapC8G4wEGGkZwyTDt1v").unwrap();
    let ata = spl_associated_token_address(&pubkey, &usdc_mint);

    let usdc = match rpc_client.get_token_account_balance(&ata) {
        Ok(balance) => balance.ui_amount.unwrap_or(0.0),
        Err(_) => 0.0,
    };

    Ok(WalletBalance {
        name: wallet_name,
        address: wallet_file.public_key,
        sol,
        usdc,
    })
}

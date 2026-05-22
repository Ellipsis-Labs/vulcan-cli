//! Wallet subcommand definitions.

use clap::{Args, Subcommand};
use std::path::PathBuf;

#[derive(Debug, Subcommand)]
pub enum WalletCommand {
    /// Generate a new Solana keypair, encrypt, and store
    Create {
        /// Name for the new wallet
        #[arg(long)]
        name: String,
    },

    /// Import a wallet from base58 private key, byte array, or Solana CLI JSON file
    Import {
        /// Name for the imported wallet
        #[arg(long)]
        name: String,

        /// Import format
        #[arg(long, value_enum, default_value = "base58")]
        format: ImportFormat,

        /// Source: base58 string, byte array, or file path (depending on --format)
        source: String,
    },

    /// List all stored wallets
    List,

    /// Show wallet details (pubkey, default status)
    Show {
        /// Wallet name
        name: String,
    },

    /// Set a wallet as the default for all commands
    #[command(alias = "use")]
    SetDefault {
        /// Wallet name
        name: String,
    },

    /// Remove a wallet from local storage
    Remove {
        /// Wallet name
        name: String,
    },

    /// Export wallet public key, encrypted backup, or plaintext private key for migration
    Export {
        /// Wallet name
        name: String,

        /// Write the encrypted wallet file to this path for backup
        #[arg(id = "export-file", long = "file", value_name = "PATH")]
        output: Option<PathBuf>,

        /// Print the encrypted wallet file to stdout for backup
        #[arg(long, conflicts_with = "export-file")]
        stdout: bool,

        /// Print the plaintext private key for migration to another wallet app (requires --yes)
        #[arg(
            id = "private-key",
            long,
            conflicts_with_all = ["export-file", "stdout"]
        )]
        private_key: bool,

        /// Plaintext private key format
        #[arg(
            long = "private-key-format",
            value_enum,
            default_value = "base58",
            requires = "private-key"
        )]
        private_key_format: PrivateKeyExportFormat,

        /// Overwrite output file if it already exists
        #[arg(long)]
        force: bool,
    },

    /// Show SOL and USDC balances for a wallet
    Balance {
        /// Wallet name (defaults to default wallet)
        name: Option<String>,
    },

    /// Register provider-backed signer wallets
    Signer {
        #[command(subcommand)]
        command: WalletSignerCommand,
    },
}

#[derive(Debug, Subcommand)]
pub enum WalletSignerCommand {
    /// Register a HashiCorp Vault transit signer
    AddVault {
        #[command(flatten)]
        common: RemoteSignerCommon,
        #[arg(long)]
        vault_addr: String,
        #[arg(long)]
        key_name: String,
        #[arg(long)]
        token_env: Option<String>,
    },
    /// Register an AWS KMS signer
    AddAwsKms {
        #[command(flatten)]
        common: RemoteSignerCommon,
        #[arg(long)]
        key_id: String,
        #[arg(long)]
        region: Option<String>,
    },
    /// Register a GCP KMS signer
    AddGcpKms {
        #[command(flatten)]
        common: RemoteSignerCommon,
        #[arg(long)]
        key_name: String,
    },
    /// Register a Turnkey signer
    AddTurnkey {
        #[command(flatten)]
        common: RemoteSignerCommon,
        #[arg(long)]
        organization_id: String,
        #[arg(long)]
        private_key_id: String,
        #[arg(long)]
        api_public_key_env: Option<String>,
        #[arg(long)]
        api_private_key_env: Option<String>,
        #[arg(long)]
        api_base_url: Option<String>,
    },
    /// Register a Privy signer
    AddPrivy {
        #[command(flatten)]
        common: RemoteSignerCommon,
        #[arg(long)]
        wallet_id: String,
        #[arg(long)]
        app_id_env: Option<String>,
        #[arg(long)]
        app_secret_env: Option<String>,
        #[arg(long)]
        api_base_url: Option<String>,
    },
    /// Register a Coinbase Developer Platform signer
    AddCdp {
        #[command(flatten)]
        common: RemoteSignerCommon,
        #[arg(long)]
        api_key_id_env: Option<String>,
        #[arg(long)]
        api_key_secret_env: Option<String>,
        #[arg(long)]
        wallet_secret_env: Option<String>,
        #[arg(long)]
        api_base_url: Option<String>,
    },
    /// Register a Para signer
    AddPara {
        #[command(flatten)]
        common: RemoteSignerCommon,
        #[arg(long)]
        wallet_id: String,
        #[arg(long)]
        api_key_env: Option<String>,
        #[arg(long)]
        api_base_url: Option<String>,
    },
    /// Register a Crossmint signer
    AddCrossmint {
        #[command(flatten)]
        common: RemoteSignerCommon,
        #[arg(long)]
        wallet_locator: String,
        #[arg(long)]
        api_key_env: Option<String>,
        #[arg(long)]
        signer_secret_env: Option<String>,
        #[arg(long)]
        signer: Option<String>,
        #[arg(long)]
        api_base_url: Option<String>,
    },
    /// Register a Dfns signer
    AddDfns {
        #[command(flatten)]
        common: RemoteSignerCommon,
        #[arg(long)]
        auth_token_env: Option<String>,
        #[arg(long)]
        cred_id: String,
        #[arg(long)]
        private_key_pem_env: Option<String>,
        #[arg(long)]
        wallet_id: String,
        #[arg(long)]
        api_base_url: Option<String>,
    },
    /// Register an Openfort backend wallet signer
    AddOpenfort {
        #[command(flatten)]
        common: RemoteSignerCommon,
        #[arg(long)]
        account_id: String,
        #[arg(long)]
        secret_key_env: Option<String>,
        #[arg(long)]
        wallet_secret_env: Option<String>,
        #[arg(long)]
        api_base_url: Option<String>,
    },
}

#[derive(Debug, Clone, Args)]
pub struct RemoteSignerCommon {
    /// Wallet record name
    #[arg(long)]
    pub name: String,
    /// Base58 Solana public key controlled by the signer provider
    #[arg(long)]
    pub public_key: String,
}

#[derive(Debug, Clone, clap::ValueEnum)]
pub enum ImportFormat {
    /// Base58 encoded private key
    Base58,
    /// Byte array (JSON)
    Bytes,
    /// Solana CLI JSON file
    File,
}

#[derive(Debug, Clone, Copy, clap::ValueEnum)]
pub enum PrivateKeyExportFormat {
    /// Base58-encoded 64-byte Solana keypair, accepted by many wallet apps
    Base58,
    /// Solana CLI JSON byte array
    Bytes,
}

#[cfg(test)]
mod tests {
    use super::{PrivateKeyExportFormat, WalletCommand, WalletSignerCommand};
    use crate::cli::{Cli, Command};
    use crate::output::OutputFormat;
    use clap::Parser;
    use std::path::PathBuf;

    #[test]
    fn export_file_flag_does_not_conflict_with_global_output_format() {
        let cli = Cli::parse_from([
            "vulcan",
            "-o",
            "json",
            "wallet",
            "export",
            "backup",
            "--file",
            "backup.json",
        ]);

        assert!(matches!(cli.output, OutputFormat::Json));
        match cli.command {
            Command::Wallet(WalletCommand::Export {
                name,
                output,
                stdout,
                private_key,
                private_key_format,
                force,
            }) => {
                assert_eq!(name, "backup");
                assert_eq!(output, Some(PathBuf::from("backup.json")));
                assert!(!stdout);
                assert!(!private_key);
                assert!(matches!(private_key_format, PrivateKeyExportFormat::Base58));
                assert!(!force);
            }
            other => panic!("unexpected command: {other:?}"),
        }
    }

    #[test]
    fn export_stdout_flag_parses_without_file_output() {
        let cli = Cli::parse_from(["vulcan", "wallet", "export", "backup", "--stdout"]);

        match cli.command {
            Command::Wallet(WalletCommand::Export {
                name,
                output,
                stdout,
                private_key,
                private_key_format,
                force,
            }) => {
                assert_eq!(name, "backup");
                assert_eq!(output, None);
                assert!(stdout);
                assert!(!private_key);
                assert!(matches!(private_key_format, PrivateKeyExportFormat::Base58));
                assert!(!force);
            }
            other => panic!("unexpected command: {other:?}"),
        }
    }

    #[test]
    fn export_private_key_flag_parses_with_format() {
        let cli = Cli::parse_from([
            "vulcan",
            "wallet",
            "export",
            "backup",
            "--private-key",
            "--private-key-format",
            "bytes",
        ]);

        match cli.command {
            Command::Wallet(WalletCommand::Export {
                name,
                output,
                stdout,
                private_key,
                private_key_format,
                force,
            }) => {
                assert_eq!(name, "backup");
                assert_eq!(output, None);
                assert!(!stdout);
                assert!(private_key);
                assert!(matches!(private_key_format, PrivateKeyExportFormat::Bytes));
                assert!(!force);
            }
            other => panic!("unexpected command: {other:?}"),
        }
    }

    #[test]
    fn signer_add_vault_parses() {
        let cli = Cli::parse_from([
            "vulcan",
            "wallet",
            "signer",
            "add-vault",
            "--name",
            "vault-main",
            "--public-key",
            "11111111111111111111111111111111",
            "--vault-addr",
            "https://vault.example.com",
            "--key-name",
            "solana-main",
            "--token-env",
            "VAULT_TOKEN",
        ]);

        match cli.command {
            Command::Wallet(WalletCommand::Signer {
                command:
                    WalletSignerCommand::AddVault {
                        common,
                        vault_addr,
                        key_name,
                        token_env,
                    },
            }) => {
                assert_eq!(common.name, "vault-main");
                assert_eq!(common.public_key, "11111111111111111111111111111111");
                assert_eq!(vault_addr, "https://vault.example.com");
                assert_eq!(key_name, "solana-main");
                assert_eq!(token_env.as_deref(), Some("VAULT_TOKEN"));
            }
            other => panic!("unexpected command: {other:?}"),
        }
    }
}

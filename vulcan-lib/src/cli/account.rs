//! Account subcommand definitions.

use clap::{ArgGroup, Subcommand};

#[derive(Debug, Subcommand)]
pub enum AccountCommand {
    /// Register a trader account on Phoenix
    #[command(group(
        ArgGroup::new("registration_code")
            .required(true)
            .args(["access_code", "referral_code", "invite_code"])
    ))]
    Register {
        /// Access code for registration
        #[arg(long)]
        access_code: Option<String>,

        /// Referral code for registration
        #[arg(long)]
        referral_code: Option<String>,

        /// Backwards-compatible alias for --access-code
        #[arg(long)]
        invite_code: Option<String>,
    },

    /// Show trader account details (PDA, subaccounts, margin mode)
    Info,

    /// List all subaccounts
    Subaccounts,

    /// Create a new subaccount
    CreateSubaccount {
        /// PDA index
        #[arg(long, default_value = "0")]
        pda_index: u8,

        /// Subaccount index
        #[arg(long)]
        subaccount_index: u8,
    },
}

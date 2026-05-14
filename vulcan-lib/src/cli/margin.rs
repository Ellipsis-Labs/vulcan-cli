//! Margin subcommand definitions.

use clap::Subcommand;

#[derive(Debug, Subcommand)]
pub enum MarginCommand {
    /// Show cross-margin health, equity, maintenance margin, available balance
    Status,

    /// Deposit USDC collateral
    Deposit {
        /// Amount in USDC
        amount: f64,
    },

    /// Withdraw USDC collateral
    Withdraw {
        /// Amount in USDC
        amount: f64,
    },

    /// Transfer collateral between subaccounts
    Transfer {
        /// Amount in USDC
        amount: f64,
        /// Source subaccount index (0 = cross-margin)
        #[arg(long)]
        from: u8,
        /// Destination subaccount index
        #[arg(long)]
        to: u8,
    },

    /// Sweep all collateral from child subaccount back to cross-margin
    TransferChildToParent {
        /// Child subaccount index to sweep
        #[arg(long)]
        child: u8,
    },

    /// Sync parent state to child subaccount
    SyncParentToChild {
        /// Child subaccount index
        #[arg(long)]
        child: u8,
    },

    /// Show leverage tier schedule for a market
    LeverageTiers {
        /// Market symbol (e.g., SOL)
        symbol: String,
    },

    /// Add collateral to an isolated position by symbol
    AddCollateral {
        /// Market symbol (e.g., SOL)
        symbol: String,
        /// Amount of USDC to add
        amount: f64,
    },
}

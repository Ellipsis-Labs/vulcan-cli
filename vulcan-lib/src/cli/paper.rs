//! Paper trading subcommand definitions.

use clap::{Subcommand, ValueEnum};

#[derive(Debug, Subcommand)]
pub enum PaperCommand {
    /// Initialize or overwrite the local paper account
    Init {
        /// Starting paper collateral balance
        #[arg(long)]
        balance: Option<f64>,

        /// Paper account currency label
        #[arg(long, default_value = "USDC")]
        currency: String,

        /// Fee in basis points charged on fills
        #[arg(long)]
        fee_bps: Option<f64>,
    },

    /// Reset local paper state
    Reset {
        /// Starting paper collateral balance
        #[arg(long)]
        balance: Option<f64>,

        /// Paper account currency label
        #[arg(long, default_value = "USDC")]
        currency: String,

        /// Fee in basis points charged on fills
        #[arg(long)]
        fee_bps: Option<f64>,
    },

    /// Show paper account status
    Status,

    /// Show paper positions
    Positions,

    /// Show open paper orders
    Orders,

    /// Show recent paper fills
    Fills {
        /// Maximum number of fills to return
        #[arg(long, default_value_t = 50)]
        limit: usize,
    },

    /// Place a paper buy order
    Buy(PaperOrderArgs),

    /// Place a paper sell order
    Sell(PaperOrderArgs),

    /// Cancel a paper order
    Cancel {
        /// Paper order ID
        order_id: String,
    },

    /// Cancel all paper orders
    CancelAll {
        /// Optional market symbol filter
        symbol: Option<String>,
    },

    /// Reconcile resting paper limit orders against live market prices
    Reconcile {
        /// Optional market symbol filter
        symbol: Option<String>,
    },

    /// Set take-profit and/or stop-loss on an existing paper position. Mirrors
    /// `trade set-tpsl` — same flag forms (single price or repeatable levels).
    SetTpsl {
        /// Market symbol (e.g., SOL)
        symbol: String,
        /// Single take-profit price covering the full position. Mutually
        /// exclusive with --tp-level.
        #[arg(long)]
        tp: Option<f64>,
        /// Single stop-loss price covering the full position. Mutually
        /// exclusive with --sl-level.
        #[arg(long)]
        sl: Option<f64>,
        /// Take-profit level as PRICE[:SIZE_TOKENS]. Repeat for laddered
        /// exits, e.g. `--tp-level 90:0.5 --tp-level 95:0.5`. SIZE is in
        /// base-asset tokens; omit for full position (single level only).
        #[arg(long = "tp-level")]
        tp_levels: Vec<String>,
        /// Stop-loss level as PRICE[:SIZE_TOKENS]. Repeatable, same format
        /// as --tp-level.
        #[arg(long = "sl-level")]
        sl_levels: Vec<String>,
    },

    /// Cancel take-profit and/or stop-loss on an existing paper position
    CancelTpsl {
        /// Market symbol (e.g., SOL)
        symbol: String,
        /// Cancel take-profit triggers
        #[arg(long)]
        tp: bool,
        /// Cancel stop-loss triggers
        #[arg(long)]
        sl: bool,
    },

    /// List active paper TP/SL triggers
    Triggers {
        /// Optional market symbol filter
        symbol: Option<String>,
    },
}

#[derive(Debug, Clone, clap::Args)]
pub struct PaperOrderArgs {
    /// Market symbol (e.g., SOL)
    pub symbol: String,

    /// Order type
    #[arg(long = "type", value_enum, default_value = "market")]
    pub order_type: PaperOrderTypeArg,

    /// Size in base lots
    #[arg(long, conflicts_with_all = ["tokens", "notional_usdc"])]
    pub size: Option<f64>,

    /// Size in base-asset tokens
    #[arg(long, conflicts_with_all = ["size", "notional_usdc"])]
    pub tokens: Option<f64>,

    /// Size as USDC notional
    #[arg(long, conflicts_with_all = ["size", "tokens"])]
    pub notional_usdc: Option<f64>,

    /// Limit price
    #[arg(long)]
    pub price: Option<f64>,

    /// Optional take-profit price attached at order time. Activates immediately
    /// on a market fill or crossing limit; stays pending on a resting limit
    /// and activates when that order fills.
    #[arg(long)]
    pub tp: Option<f64>,

    /// Optional stop-loss price attached at order time. Same semantics as --tp.
    #[arg(long)]
    pub sl: Option<f64>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, ValueEnum)]
pub enum PaperOrderTypeArg {
    Market,
    Limit,
}

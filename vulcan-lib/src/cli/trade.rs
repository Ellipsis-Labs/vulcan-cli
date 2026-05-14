//! Trade subcommand definitions.

use clap::Subcommand;

#[derive(Debug, Subcommand)]
pub enum TradeCommand {
    /// Place a market buy order
    MarketBuy {
        /// Market symbol (e.g., SOL)
        symbol: String,
        /// Order size in base lots (provide exactly one of: positional size, --tokens, --notional-usdc)
        size: Option<f64>,
        /// Order size in tokens of the base asset (e.g. 1.18 SOL)
        #[arg(long, conflicts_with_all = ["size", "notional_usdc"])]
        tokens: Option<f64>,
        /// Order size as USDC notional (market orders quoted at current mid; actual fill differs by spread + impact)
        #[arg(long, conflicts_with_all = ["size", "tokens"])]
        notional_usdc: Option<f64>,
        /// Take-profit price
        #[arg(long)]
        tp: Option<f64>,
        /// Stop-loss price
        #[arg(long)]
        sl: Option<f64>,
        /// Use isolated margin
        #[arg(long)]
        isolated: bool,
        /// USDC collateral for isolated subaccount
        #[arg(long, requires = "isolated")]
        collateral: Option<f64>,
        /// Reduce-only order
        #[arg(long)]
        reduce_only: bool,
    },

    /// Place a market sell order
    MarketSell {
        /// Market symbol (e.g., SOL)
        symbol: String,
        /// Order size in base lots (provide exactly one of: positional size, --tokens, --notional-usdc)
        size: Option<f64>,
        /// Order size in tokens of the base asset (e.g. 1.18 SOL)
        #[arg(long, conflicts_with_all = ["size", "notional_usdc"])]
        tokens: Option<f64>,
        /// Order size as USDC notional (market orders quoted at current mid; actual fill differs by spread + impact)
        #[arg(long, conflicts_with_all = ["size", "tokens"])]
        notional_usdc: Option<f64>,
        /// Take-profit price
        #[arg(long)]
        tp: Option<f64>,
        /// Stop-loss price
        #[arg(long)]
        sl: Option<f64>,
        /// Use isolated margin
        #[arg(long)]
        isolated: bool,
        /// USDC collateral for isolated subaccount
        #[arg(long, requires = "isolated")]
        collateral: Option<f64>,
        /// Reduce-only order
        #[arg(long)]
        reduce_only: bool,
    },

    /// Place a limit buy order
    LimitBuy {
        /// Market symbol (e.g., SOL)
        symbol: String,
        /// Order size in base lots
        size: f64,
        /// Limit price
        price: f64,
        /// Take-profit price
        #[arg(long)]
        tp: Option<f64>,
        /// Stop-loss price
        #[arg(long)]
        sl: Option<f64>,
        /// Use isolated margin
        #[arg(long)]
        isolated: bool,
        /// USDC collateral for isolated subaccount
        #[arg(long, requires = "isolated")]
        collateral: Option<f64>,
        /// Reduce-only order
        #[arg(long)]
        reduce_only: bool,
    },

    /// Place a limit sell order
    LimitSell {
        /// Market symbol (e.g., SOL)
        symbol: String,
        /// Order size in base lots
        size: f64,
        /// Limit price
        price: f64,
        /// Take-profit price
        #[arg(long)]
        tp: Option<f64>,
        /// Stop-loss price
        #[arg(long)]
        sl: Option<f64>,
        /// Use isolated margin
        #[arg(long)]
        isolated: bool,
        /// USDC collateral for isolated subaccount
        #[arg(long, requires = "isolated")]
        collateral: Option<f64>,
        /// Reduce-only order
        #[arg(long)]
        reduce_only: bool,
    },

    /// Cancel specific orders by ID
    Cancel {
        /// Market symbol (e.g., SOL)
        symbol: String,
        /// Order IDs to cancel
        #[arg(required = true)]
        order_ids: Vec<String>,
    },

    /// Cancel all open orders. Omit symbol to cancel across every market.
    CancelAll {
        /// Market symbol (e.g., SOL). Omit to cancel orders across all markets.
        symbol: Option<String>,
    },

    /// List open orders (optionally filter by market)
    Orders {
        /// Market symbol (e.g., SOL). Omit to list all.
        symbol: Option<String>,
    },

    /// Set take-profit and/or stop-loss on an existing position. Supports
    /// multiple levels per side and partial sizing.
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

    /// Cancel take-profit and/or stop-loss on an existing position
    CancelTpsl {
        /// Market symbol (e.g., SOL)
        symbol: String,
        /// Cancel take-profit
        #[arg(long)]
        tp: bool,
        /// Cancel stop-loss
        #[arg(long)]
        sl: bool,
    },
}

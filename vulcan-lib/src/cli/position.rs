//! Position subcommand definitions.

use clap::Subcommand;

#[derive(Debug, Subcommand)]
pub enum PositionCommand {
    /// List all open positions
    List,

    /// Detailed view of a specific position
    Show {
        /// Market symbol (e.g., SOL)
        symbol: String,
    },

    /// Close an entire position
    Close {
        /// Market symbol (e.g., SOL)
        symbol: String,
    },

    /// Close every open position across all markets
    CloseAll,

    /// Reduce a position by a specified size
    Reduce {
        /// Market symbol (e.g., SOL)
        symbol: String,
        /// Size to reduce by (in base lots)
        size: f64,
    },

    /// Attach take-profit and/or stop-loss to an existing position
    TpSl {
        /// Market symbol (e.g., SOL)
        symbol: String,
        /// Take-profit price
        #[arg(long)]
        tp: Option<f64>,
        /// Stop-loss price
        #[arg(long)]
        sl: Option<f64>,
    },
}

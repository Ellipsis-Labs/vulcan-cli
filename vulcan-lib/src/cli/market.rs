//! Market subcommand definitions.

use clap::Subcommand;

#[derive(Debug, Subcommand)]
pub enum MarketCommand {
    /// List all available perpetual markets
    List,

    /// Detailed market configuration (tick size, lot size, fees, leverage tiers)
    Info {
        /// Market symbol (e.g., SOL)
        symbol: String,
    },

    /// Current price, 24h volume, open interest, funding rate
    Ticker {
        /// Market symbol (e.g., SOL)
        symbol: String,
    },

    /// L2 orderbook snapshot
    Orderbook {
        /// Market symbol (e.g., SOL)
        symbol: String,

        /// Number of price levels to display
        #[arg(long, default_value = "10")]
        depth: usize,
    },

    /// OHLCV candle data
    Candles {
        /// Market symbol (e.g., SOL)
        symbol: String,

        /// Candle interval
        #[arg(long, default_value = "1h")]
        interval: String,

        /// Number of candles to fetch
        #[arg(long, default_value = "20")]
        limit: usize,

        /// Append derived indicator columns. Comma-separated names
        /// (e.g., `rsi,macd,atr`). Supported: sma, ema, rsi, macd, bbands, atr, vwap, adx, stoch.
        #[arg(long, value_delimiter = ',')]
        with_indicators: Vec<String>,
    },

    /// Recent trades for a market
    Trades {
        /// Market symbol (e.g., SOL)
        symbol: String,

        /// Number of trades to fetch
        #[arg(long, default_value = "20")]
        limit: usize,
    },

    /// Historical funding rate data
    FundingRates {
        /// Market symbol (e.g., SOL)
        symbol: String,

        /// Number of entries to fetch
        #[arg(long, default_value = "20")]
        limit: usize,
    },
}

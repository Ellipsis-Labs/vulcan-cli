//! Technical analysis subcommand definitions.

use clap::Subcommand;

#[derive(Debug, Subcommand)]
pub enum TaCommand {
    /// Compute a single indicator over the latest candles for a market.
    Compute {
        /// Market symbol (e.g., SOL)
        symbol: String,

        /// Indicator: sma | ema | rsi | macd | bbands | atr | vwap | adx | stoch
        #[arg(long)]
        indicator: String,

        /// Candle timeframe (1m, 5m, 15m, 1h, 4h, 1d)
        #[arg(long, default_value = "1h")]
        timeframe: String,

        /// Lookback period; falls back to the indicator's default when omitted.
        #[arg(long)]
        period: Option<usize>,

        /// Number of candles to fetch for the computation.
        #[arg(long)]
        limit: Option<usize>,

        /// Indicator-specific params as JSON object, e.g. '{"fast":12,"slow":26,"signal":9}'
        #[arg(long)]
        params: Option<String>,
    },

    /// Evaluate a trigger spec against the latest indicator value.
    Signal {
        /// Market symbol (e.g., SOL)
        symbol: String,

        /// Trigger spec as JSON, e.g. '{"indicator":"rsi","timeframe":"1h","op":"lt","threshold":30}'
        #[arg(long)]
        spec: String,
    },

    /// Multi-indicator snapshot (RSI, MACD, BBands, ATR, ADX) for market-intel reports.
    Report {
        /// Market symbol (e.g., SOL)
        symbol: String,

        /// Candle timeframe.
        #[arg(long, default_value = "1h")]
        timeframe: String,

        /// Comma-separated indicator subset (e.g. "rsi,atr,bbands"). Omit for the default bundle.
        #[arg(long, value_delimiter = ',')]
        indicators: Option<Vec<String>>,

        /// Number of recent points to include per indicator. Default omits the full
        /// series (each indicator still ships its `latest`, `signals`, and `summary`).
        /// Use `--points-limit 20` to include the last 20 bars; `0` is the same as default.
        #[arg(long)]
        points_limit: Option<usize>,
    },
}

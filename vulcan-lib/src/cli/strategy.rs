//! Strategy runner subcommand definitions.

use clap::{Args, Subcommand, ValueEnum};

#[derive(Debug, Subcommand)]
pub enum StrategyCommand {
    /// TWAP strategy runner
    #[command(subcommand)]
    Twap(TwapCommand),

    /// Grid trading strategy runner
    #[command(subcommand)]
    Grid(GridCommand),

    /// Technical-analysis-driven strategy runner — rules of (condition, action)
    #[command(subcommand)]
    Ta(TaCommand),

    /// List persisted strategy runs
    Runs {
        /// Maximum runs to show
        #[arg(long, default_value = "20")]
        limit: usize,
    },

    /// Show latest status for a strategy run
    Status {
        /// Strategy run ID
        run_id: String,

        /// Return ticks newer than this tick index
        #[arg(long)]
        since_tick: Option<u32>,

        /// Include the full ledger in JSON output
        #[arg(long, default_value = "false")]
        include_ledger: bool,
    },

    /// Show compact non-blocking monitor state for a strategy run
    Monitor {
        /// Strategy run ID
        run_id: String,

        /// Include the full ledger in JSON output
        #[arg(long, default_value = "false")]
        include_ledger: bool,
    },

    /// Inspect live grid orders against the persisted ledger without mutating state
    ReconcileGrid {
        /// Strategy run ID
        run_id: String,
    },

    /// Wait until a new tick or terminal status is observed
    WaitNextTick {
        /// Strategy run ID
        run_id: String,

        /// Return once a tick newer than this tick index is observed.
        /// Defaults to the current latest tick, so the command waits for future progress.
        #[arg(long)]
        after_tick: Option<u32>,

        /// Maximum seconds to wait before returning the current status
        #[arg(long, default_value = "90")]
        timeout_seconds: u64,

        /// Include the full ledger in JSON output
        #[arg(long, default_value = "false")]
        include_ledger: bool,
    },

    /// Show final or latest report for a strategy run
    Report {
        /// Strategy run ID
        run_id: String,
    },

    /// Request a running strategy to pause at the next safe point
    Pause {
        /// Strategy run ID
        run_id: String,
        /// Optional reason recorded in the ledger
        #[arg(long)]
        reason: Option<String>,
    },

    /// Request a running strategy to stop permanently at the next safe point
    Stop {
        /// Strategy run ID
        run_id: String,
        /// Optional reason recorded in the ledger
        #[arg(long)]
        reason: Option<String>,
    },

    /// Stop a strategy and optionally clean up live orders or positions
    Finalize {
        /// Strategy run ID
        run_id: String,

        /// Optional reason recorded in the ledger
        #[arg(long)]
        reason: Option<String>,

        /// Cancel open orders on the strategy symbol
        #[arg(long, default_value = "false")]
        cancel_orders: bool,

        /// Close any open position on the strategy symbol
        #[arg(long, default_value = "false")]
        close_position: bool,

        /// Wait for the runner to observe the stop request before cleanup
        #[arg(long, default_value = "false")]
        wait: bool,

        /// Maximum seconds to wait for the runner before cleanup
        #[arg(long, default_value = "90")]
        timeout_seconds: u64,
    },

    /// Inspect live-readiness for the active wallet without launching anything.
    /// Reports wallet identity, password availability, trader registration,
    /// collateral, and lists every blocker with a concrete remedy command.
    Preflight,

    /// Resume a paused or incomplete strategy run
    Resume {
        /// Strategy run ID
        run_id: String,

        /// Optional step to resume from
        #[arg(long)]
        from_step: Option<u32>,

        /// Do not sleep between steps; intended for tests and recovery smoke checks.
        #[arg(long, hide = true, default_value = "false")]
        no_sleep: bool,
    },
}

#[derive(Debug, Subcommand)]
pub enum TwapCommand {
    /// Start a TWAP run
    Start(TwapStartArgs),

    /// Resume a paused or incomplete TWAP run
    Resume {
        /// Strategy run ID
        run_id: String,

        /// Optional step to resume from
        #[arg(long)]
        from_step: Option<u32>,

        /// Do not sleep between steps; intended for tests and recovery smoke checks.
        #[arg(long, hide = true, default_value = "false")]
        no_sleep: bool,
    },
}

#[derive(Debug, Subcommand)]
pub enum TaCommand {
    /// Start a TA strategy run from a config file or JSON string
    Start(TaStartArgs),

    /// Resume a paused or incomplete TA strategy run
    Resume {
        /// Strategy run ID
        run_id: String,

        /// Do not sleep between ticks; intended for tests and recovery smoke checks.
        #[arg(long, hide = true, default_value = "false")]
        no_sleep: bool,
    },
}

#[derive(Debug, Clone, Args)]
pub struct TaStartArgs {
    /// Path to a JSON config file. Provide either --config-file or --config-json.
    #[arg(long, conflicts_with = "config_json")]
    pub config_file: Option<String>,

    /// Inline JSON config (mutually exclusive with --config-file).
    #[arg(long, conflicts_with = "config_file")]
    pub config_json: Option<String>,

    /// Execution mode
    #[arg(long, value_enum, default_value = "paper")]
    pub mode: StrategyModeArg,

    /// Maximum ticks to run; ignored when --run-until-stopped is set.
    #[arg(long, default_value = "60")]
    pub max_ticks: u32,

    /// Keep running until paused/stopped (overrides --max-ticks).
    #[arg(long, default_value = "false")]
    pub run_until_stopped: bool,

    /// Optional run label
    #[arg(long)]
    pub run_label: Option<String>,

    /// Guardrail max total live notional
    #[arg(long)]
    pub max_total_notional_usdc: Option<f64>,

    /// Guardrail max per-step live notional
    #[arg(long)]
    pub max_step_notional_usdc: Option<f64>,

    /// Guardrail mark-price drift threshold from run start
    #[arg(long)]
    pub max_price_drift_bps: Option<f64>,

    /// Guardrail max position notional divided by equity
    #[arg(long)]
    pub max_exposure_ratio: Option<f64>,

    /// Reconcile attempts per live firing
    #[arg(long)]
    pub reconcile_attempts: Option<u32>,

    /// Reconcile delay (ms) per live firing
    #[arg(long)]
    pub reconcile_delay_ms: Option<u64>,

    /// Do not sleep between ticks; intended for tests and smoke checks.
    #[arg(long, hide = true, default_value = "false")]
    pub no_sleep: bool,

    /// Start the runner in the background and return immediately with the run ID.
    #[arg(long, default_value = "false")]
    pub detached: bool,
}

#[derive(Debug, Subcommand)]
pub enum GridCommand {
    /// Start a grid trading run
    Start(Box<GridStartArgs>),

    /// Resume a paused or incomplete grid run
    Resume {
        /// Strategy run ID
        run_id: String,

        /// Optional step to resume from
        #[arg(long)]
        from_step: Option<u32>,

        /// Do not sleep between steps; intended for tests and recovery smoke checks.
        #[arg(long, hide = true, default_value = "false")]
        no_sleep: bool,
    },
}

#[derive(Debug, Clone, Args)]
pub struct TwapStartArgs {
    /// Market symbol, e.g. SOL
    #[arg(long)]
    pub symbol: String,

    /// Buy or sell
    #[arg(long, value_enum)]
    pub side: StrategySideArg,

    /// Total TWAP notional in USDC
    #[arg(long, conflicts_with = "tokens")]
    pub notional_usdc: Option<f64>,

    /// Total TWAP size in base-asset tokens
    #[arg(long, conflicts_with = "notional_usdc")]
    pub tokens: Option<f64>,

    /// Number of slices
    #[arg(long)]
    pub slices: u32,

    /// Seconds between slices
    #[arg(long, default_value = "60")]
    pub interval_seconds: u64,

    /// Execution mode
    #[arg(long, value_enum, default_value = "paper")]
    pub mode: StrategyModeArg,

    /// Margin mode for live execution
    #[arg(long, value_enum, default_value = "cross")]
    pub margin_mode: StrategyMarginModeArg,

    /// USDC collateral to transfer when opening the first isolated live order
    #[arg(long)]
    pub isolated_collateral: Option<f64>,

    /// Optional run label
    #[arg(long)]
    pub run_label: Option<String>,

    /// Guardrail max total live notional
    #[arg(long)]
    pub max_total_notional_usdc: Option<f64>,

    /// Guardrail max per-step live notional
    #[arg(long)]
    pub max_step_notional_usdc: Option<f64>,

    /// Guardrail mark-price drift threshold from run start
    #[arg(long)]
    pub max_price_drift_bps: Option<f64>,

    /// Guardrail max position notional divided by equity
    #[arg(long)]
    pub max_exposure_ratio: Option<f64>,

    /// Number of history reconciliation attempts per live step
    #[arg(long)]
    pub reconcile_attempts: Option<u32>,

    /// Milliseconds between history reconciliation attempts
    #[arg(long)]
    pub reconcile_delay_ms: Option<u64>,

    /// Do not sleep between slices; intended for tests and smoke checks.
    #[arg(long, hide = true, default_value = "false")]
    pub no_sleep: bool,

    /// Start the runner in the background and return immediately with the run ID.
    #[arg(long, default_value = "false")]
    pub detached: bool,
}

#[derive(Debug, Clone, Args)]
pub struct GridStartArgs {
    /// Market symbol, e.g. SOL
    #[arg(long)]
    pub symbol: String,

    /// Lower grid boundary price. Required unless --center-on-mark is set.
    #[arg(long)]
    pub lower_price: Option<f64>,

    /// Upper grid boundary price. Required unless --center-on-mark is set.
    #[arg(long)]
    pub upper_price: Option<f64>,

    /// Center the grid on the current mark price at launch. Requires --width-pct.
    #[arg(long, default_value = "false")]
    pub center_on_mark: bool,

    /// Half-width of the grid as a percentage of mark (e.g. 1.0 = ±1%). Requires --center-on-mark.
    #[arg(long)]
    pub width_pct: Option<f64>,

    /// Number of buy levels below mark and sell levels above mark
    #[arg(long)]
    pub levels_per_side: u32,

    /// Order size per level in base-asset tokens
    #[arg(long, conflicts_with = "size_lots_per_level")]
    pub tokens_per_level: Option<f64>,

    /// Order size per level in base lots
    #[arg(long, conflicts_with = "tokens_per_level")]
    pub size_lots_per_level: Option<u64>,

    /// Fully custom bid level as PRICE:SIZE_LOTS[:TP][:SL]. Repeatable.
    #[arg(long = "bid-level")]
    pub bid_levels: Vec<String>,

    /// Fully custom ask level as PRICE:SIZE_LOTS[:TP][:SL]. Repeatable.
    #[arg(long = "ask-level")]
    pub ask_levels: Vec<String>,

    /// Distance from entry to take-profit for generated levels.
    #[arg(long)]
    pub take_profit_spacing: Option<f64>,

    /// Distance from entry to stop-loss for generated levels.
    #[arg(long)]
    pub stop_loss_spacing: Option<f64>,

    /// Seconds between maintenance ticks
    #[arg(long, default_value = "60")]
    pub interval_seconds: u64,

    /// Maximum ticks to run, including the initial placement tick.
    #[arg(long, default_value = "60")]
    pub ticks: u32,

    /// Keep running maintenance ticks until paused or stopped.
    #[arg(long, default_value = "false")]
    pub run_until_stopped: bool,

    /// Seconds without a tick before status reports this run as stale.
    #[arg(long)]
    pub stale_after_seconds: Option<u64>,

    /// Execution mode
    #[arg(long, value_enum, default_value = "paper")]
    pub mode: StrategyModeArg,

    /// Margin mode for live execution
    #[arg(long, value_enum, default_value = "cross")]
    pub margin_mode: StrategyMarginModeArg,

    /// USDC collateral to transfer when opening the first isolated live order
    #[arg(long)]
    pub isolated_collateral: Option<f64>,

    /// Optional run label
    #[arg(long)]
    pub run_label: Option<String>,

    /// Whether live multi-limit orders may slide to the top of book if crossing.
    #[arg(long, default_value = "false")]
    pub slide: bool,

    /// Guardrail max total live notional
    #[arg(long)]
    pub max_total_notional_usdc: Option<f64>,

    /// Guardrail max per-step live notional
    #[arg(long)]
    pub max_step_notional_usdc: Option<f64>,

    /// Guardrail mark-price drift threshold from run start
    #[arg(long)]
    pub max_price_drift_bps: Option<f64>,

    /// Guardrail max position notional divided by equity
    #[arg(long)]
    pub max_exposure_ratio: Option<f64>,

    /// Number of history/order reconciliation attempts per live step
    #[arg(long)]
    pub reconcile_attempts: Option<u32>,

    /// Milliseconds between reconciliation attempts
    #[arg(long)]
    pub reconcile_delay_ms: Option<u64>,

    /// Do not sleep between ticks; intended for tests and smoke checks.
    #[arg(long, hide = true, default_value = "false")]
    pub no_sleep: bool,

    /// Start the runner in the background and return immediately with the run ID.
    #[arg(long, default_value = "false")]
    pub detached: bool,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, ValueEnum)]
pub enum StrategyModeArg {
    Paper,
    DryRun,
    ConfirmEach,
    AutoExecute,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, ValueEnum)]
pub enum StrategySideArg {
    Buy,
    Sell,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, ValueEnum)]
pub enum StrategyMarginModeArg {
    Cross,
    Isolated,
}

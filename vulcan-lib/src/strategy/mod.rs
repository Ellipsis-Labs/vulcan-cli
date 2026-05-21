//! First-class strategy runners and persisted run logs.

pub mod execution;
pub mod grid;
pub mod log;
pub mod preflight;
pub mod reconcile;
pub mod runner;
pub mod safety;
pub mod ta;
pub mod ta_rule;
pub mod twap;
pub mod types;
pub mod watchdog;

pub use grid::{GridRunConfig, GridRunResult};
pub use log::{
    clear_control_request, default_strategy_runs_dir, list_run_summaries, load_run_ledger,
    load_run_report, read_control_request, read_run_ticks, run_ledger_path, run_lock_path,
    write_control_request, write_run_ledger, write_run_report, write_tick,
};
pub use safety::StrategySafetyPolicy;
pub use twap::{TwapRunConfig, TwapRunResult};
pub use types::{
    StrategyAccountHealth, StrategyControlAction, StrategyControlRequest, StrategyExecution,
    StrategyLedgerSlice, StrategyLedgerStep, StrategyMarginMode, StrategyMarketSnapshot,
    StrategyMode, StrategyNextTick, StrategyPlannedAction, StrategyPositionSnapshot,
    StrategyProgress, StrategyRunLedger, StrategyRunReport, StrategyRunSummary, StrategySide,
    StrategySliceStatus, StrategyTick, StrategyWalletContext,
};

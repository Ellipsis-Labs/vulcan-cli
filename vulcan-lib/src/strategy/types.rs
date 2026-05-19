//! Shared strategy runner types.

use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum StrategyMode {
    Paper,
    DryRun,
    ConfirmEach,
    AutoExecute,
}

impl StrategyMode {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Paper => "paper",
            Self::DryRun => "dry_run",
            Self::ConfirmEach => "confirm_each",
            Self::AutoExecute => "auto_execute",
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum StrategySide {
    Buy,
    Sell,
}

impl StrategySide {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Buy => "buy",
            Self::Sell => "sell",
        }
    }
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum StrategyMarginMode {
    #[default]
    Cross,
    Isolated,
}

impl StrategyMarginMode {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Cross => "cross",
            Self::Isolated => "isolated",
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum StrategyControlAction {
    Pause,
    Stop,
}

impl StrategyControlAction {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Pause => "pause",
            Self::Stop => "stop",
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct StrategyControlRequest {
    pub action: StrategyControlAction,
    pub requested_at: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub reason: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct StrategyWalletContext {
    pub name: String,
    pub address: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct StrategyMarketSnapshot {
    pub mark_price: f64,
    pub mid_price: f64,
    pub funding_rate: f64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct StrategyPlannedAction {
    pub action: String,
    pub order_type: String,
    pub slice_notional_usdc: Option<f64>,
    pub slice_tokens: Option<f64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub executable_notional_usdc: Option<f64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub executable_tokens: Option<f64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub executable_base_lots: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub rounding_note: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct StrategyExecution {
    pub action: String,
    pub fill_id: Option<String>,
    pub tx_signature: Option<String>,
    pub fill_price: Option<f64>,
    pub fill_tokens: Option<f64>,
    pub fill_notional_usdc: Option<f64>,
    pub fee: Option<f64>,
    pub note: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct StrategyProgress {
    pub slices_filled: u32,
    pub cumulative_tokens: f64,
    pub cumulative_notional_usdc: f64,
    pub vwap: Option<f64>,
    pub cumulative_fees: f64,
    pub remaining_slices: u32,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct StrategyPositionSnapshot {
    pub side: Option<String>,
    pub size_tokens: f64,
    pub size_lots: Option<u64>,
    pub mark_price: Option<f64>,
    pub notional_usdc: f64,
    pub unrealized_pnl: f64,
    pub exposure_ratio: f64,
    /// Live-only fields populated from the Phoenix positions endpoint. All None for paper.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub entry_price: Option<f64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub liquidation_price: Option<f64>,
    /// Distance from current mark to liquidation, in basis points (10000 = 100%).
    /// Sign convention: positive = mark is "safe side" of liq (above liq for longs,
    /// below liq for shorts). Negative = already past liq (shouldn't happen but possible mid-tick).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub distance_to_liq_bps: Option<f64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub unrealized_pnl_pct: Option<f64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub initial_margin_usdc: Option<f64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub maintenance_margin_usdc: Option<f64>,
    /// Leverage = abs(notional) / equity; mirrors `exposure_ratio` but named clearly.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub leverage: Option<f64>,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct StrategyAccountHealth {
    pub equity: Option<f64>,
    pub collateral: Option<String>,
    pub available_balance: Option<String>,
    pub risk_state: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct StrategyNextTick {
    pub next_tick_at: Option<String>,
    pub delay_seconds: Option<u64>,
    pub stop_reason: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct StrategyTick {
    pub run_id: String,
    pub strategy: String,
    pub mode: StrategyMode,
    pub symbol: String,
    pub side: StrategySide,
    pub tick: u32,
    pub slices_total: u32,
    pub timestamp: String,
    pub elapsed_ms: u128,
    pub market: StrategyMarketSnapshot,
    pub planned_action: StrategyPlannedAction,
    pub execution: StrategyExecution,
    pub progress: StrategyProgress,
    pub position: StrategyPositionSnapshot,
    /// Position snapshot captured *after* this tick's action executed. Only
    /// populated when a rule fired and produced a fill; otherwise `None` and the
    /// post-tick state equals `position`. Lets agents see the immediate
    /// post-fill state instead of waiting for the next tick to re-snapshot.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub position_after_action: Option<StrategyPositionSnapshot>,
    pub account_health: StrategyAccountHealth,
    pub next_tick: StrategyNextTick,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct StrategyRunSummary {
    pub run_id: String,
    pub strategy: String,
    pub mode: StrategyMode,
    pub symbol: String,
    pub side: StrategySide,
    pub started_at: String,
    pub completed_at: Option<String>,
    pub status: String,
    pub slices_completed: u32,
    pub slices_total: u32,
    pub cumulative_notional_usdc: f64,
    pub cumulative_tokens: f64,
    pub vwap: Option<f64>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum StrategySliceStatus {
    Planned,
    Ready,
    Submitted,
    Resting,
    Filled,
    Partial,
    Replaced,
    Cancelled,
    Finalized,
    Skipped,
    Paused,
    Failed,
}

impl StrategySliceStatus {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Planned => "planned",
            Self::Ready => "ready",
            Self::Submitted => "submitted",
            Self::Resting => "resting",
            Self::Filled => "filled",
            Self::Partial => "partial",
            Self::Replaced => "replaced",
            Self::Cancelled => "cancelled",
            Self::Finalized => "finalized",
            Self::Skipped => "skipped",
            Self::Paused => "paused",
            Self::Failed => "failed",
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct StrategyLedgerTotals {
    pub planned_notional_usdc: Option<f64>,
    pub planned_tokens: Option<f64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub planned_base_lots: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub executable_notional_usdc: Option<f64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub executable_tokens: Option<f64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub executable_base_lots: Option<u64>,
    pub submitted_notional_usdc: f64,
    pub filled_notional_usdc: f64,
    pub filled_tokens: f64,
    pub vwap: Option<f64>,
    pub fees: f64,
    pub filled_count: u32,
    pub partial_count: u32,
    pub submitted_count: u32,
    pub skipped_count: u32,
    pub paused_count: u32,
    pub failed_count: u32,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct StrategyLedgerSlice {
    pub index: u32,
    pub scheduled_at: String,
    pub planned_notional_usdc: Option<f64>,
    pub planned_tokens: Option<f64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub executable_notional_usdc: Option<f64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub executable_tokens: Option<f64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub executable_base_lots: Option<u64>,
    pub status: StrategySliceStatus,
    pub tx_signature: Option<String>,
    pub fill_id: Option<String>,
    pub fill_price: Option<f64>,
    pub fill_tokens: Option<f64>,
    pub fill_notional_usdc: Option<f64>,
    pub fee: Option<f64>,
    pub submitted_at: Option<String>,
    pub filled_at: Option<String>,
    pub updated_at: String,
    pub pause_reason: Option<String>,
    pub error_code: Option<String>,
    pub note: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub rounding_note: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub metadata: Option<serde_json::Value>,
}

pub type StrategyLedgerStep = StrategyLedgerSlice;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct StrategyRunLedger {
    pub run_id: String,
    pub strategy: String,
    pub run_label: Option<String>,
    pub wallet: Option<StrategyWalletContext>,
    pub symbol: String,
    pub side: StrategySide,
    pub mode: StrategyMode,
    pub started_at: String,
    pub completed_at: Option<String>,
    pub status: String,
    pub interval_seconds: u64,
    #[serde(default)]
    pub run_until_stopped: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub stale_after_seconds: Option<u64>,
    pub slices_total: u32,
    pub totals: StrategyLedgerTotals,
    pub slices: Vec<StrategyLedgerSlice>,
    pub last_updated_at: String,
    pub pause_reason: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub safety_policy: Option<serde_json::Value>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct StrategyRunReport {
    pub run_id: String,
    pub summary: StrategyRunSummary,
    pub config: serde_json::Value,
    pub ticks: Vec<StrategyTick>,
    pub log_path: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub ledger: Option<StrategyRunLedger>,
    pub notes: Vec<String>,
}

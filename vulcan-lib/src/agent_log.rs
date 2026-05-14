//! Local agent action/session log.
//!
//! The log is append-only JSONL and intentionally redacted by default. It is
//! meant to preserve what the agent did and observed, not to replace exchange
//! history or expose raw tool arguments.

use crate::error::VulcanError;
use chrono::Utc;
use rand::RngCore;
use serde::{Deserialize, Serialize};
use serde_json::{Map, Value};
use std::collections::BTreeSet;
use std::collections::HashSet;
use std::fs::{self, File, OpenOptions};
use std::io::{BufRead, BufReader, Write};
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};

const DEFAULT_SUMMARY_LIMIT: usize = 50;
const DEFAULT_RETAIN_SESSIONS: usize = 20;
const DEFAULT_MAX_FILE_MB: u64 = 25;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AgentLogRecord {
    pub timestamp: String,
    pub session_id: String,
    pub correlation_id: String,
    pub surface: String,
    pub name: String,
    pub dangerous: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub acknowledged: Option<bool>,
    pub outcome: String,
    pub duration_ms: u128,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub error_code: Option<String>,
    #[serde(default, skip_serializing_if = "Value::is_null")]
    pub args: Value,
    #[serde(default, skip_serializing_if = "Value::is_null")]
    pub summary: Value,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ArchivedSessionSummary {
    pub archived_at: String,
    pub session_id: String,
    pub summary: AgentSessionSummary,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AgentSessionSummary {
    pub total_records: usize,
    pub sessions: Vec<String>,
    pub tx_signatures: Vec<String>,
    pub errors: Vec<ActionErrorSummary>,
    pub latest_positions: Vec<PositionSummary>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub latest_margin: Option<MarginSummary>,
    pub recent_actions: Vec<ActionSummary>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ActionSummary {
    pub timestamp: String,
    pub session_id: String,
    pub name: String,
    pub outcome: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub symbol: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub side: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub error_code: Option<String>,
    pub tx_signatures: Vec<String>,
    pub paper_fill_ids: Vec<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub mode: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ActionErrorSummary {
    pub timestamp: String,
    pub name: String,
    pub code: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct PositionSummary {
    pub symbol: String,
    pub side: String,
    pub size: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub unrealized_pnl: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub mark_price: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct MarginSummary {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub risk_state: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub portfolio_value: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub unrealized_pnl: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub collateral_balance: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AgentPositionSessionReport {
    pub generated_at: String,
    pub log_path: String,
    pub session: String,
    pub limit: usize,
    pub live: LiveTraderReport,
    pub session_summary: AgentSessionSummary,
    pub metrics: SessionReportMetrics,
    pub recent_position_changes: Vec<PositionChangeSummary>,
    pub inferred_fills: Vec<InferredFillSummary>,
    pub notes: Vec<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct LiveTraderReport {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub margin: Option<MarginSummary>,
    pub positions: Vec<PositionSummary>,
    pub open_orders: usize,
    pub reduce_only_orders: usize,
    pub take_profit_orders: usize,
    pub stop_loss_orders: usize,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SessionReportMetrics {
    pub actions: ActionCountSummary,
    pub modes: ModeCountSummary,
    pub tx_count: usize,
    pub paper_fill_count: usize,
    pub error_count: usize,
    pub open_position_win_rate: WinRateSummary,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct ActionCountSummary {
    pub successful_trades: usize,
    pub market_trades: usize,
    pub limit_trades: usize,
    pub cancels: usize,
    pub position_closes: usize,
    pub position_reductions: usize,
    pub tpsl_updates: usize,
    pub margin_actions: usize,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct ModeCountSummary {
    pub live_actions: usize,
    pub dry_run_actions: usize,
    pub paper_actions: usize,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct WinRateSummary {
    pub basis: String,
    pub winners: usize,
    pub losers: usize,
    pub flat: usize,
    pub sample_size: usize,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub rate: Option<f64>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PositionChangeSummary {
    pub timestamp: String,
    pub name: String,
    pub outcome: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub symbol: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub side: Option<String>,
    pub tx_signatures: Vec<String>,
    pub paper_fill_ids: Vec<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub mode: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct InferredFillSummary {
    pub timestamp: String,
    pub name: String,
    pub basis: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub symbol: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub side: Option<String>,
    pub tx_signatures: Vec<String>,
    pub paper_fill_ids: Vec<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub mode: Option<String>,
}

#[derive(Debug, Clone)]
pub struct AgentLogSink {
    path: PathBuf,
    summary_archive_path: PathBuf,
    session_id: String,
    verbose: bool,
    capture_position_snapshots: bool,
    retain_sessions: usize,
    max_file_bytes: u64,
    archive_session_summaries: bool,
    lock: Arc<Mutex<()>>,
}

impl AgentLogSink {
    #[allow(clippy::too_many_arguments)]
    pub fn new(
        path: PathBuf,
        session_id: String,
        verbose: bool,
        capture_position_snapshots: bool,
        retain_sessions: usize,
        max_file_bytes: u64,
        archive_session_summaries: bool,
    ) -> Self {
        let summary_archive_path = default_summary_archive_path(&path);
        Self {
            path,
            summary_archive_path,
            session_id,
            verbose,
            capture_position_snapshots,
            retain_sessions,
            max_file_bytes,
            archive_session_summaries,
            lock: Arc::new(Mutex::new(())),
        }
    }

    pub fn path(&self) -> &Path {
        &self.path
    }

    pub fn session_id(&self) -> &str {
        &self.session_id
    }

    pub fn summary_archive_path(&self) -> &Path {
        &self.summary_archive_path
    }

    #[allow(clippy::too_many_arguments)]
    pub fn record_call(
        &self,
        surface: &str,
        name: &str,
        args: &Value,
        dangerous: bool,
        acknowledged: Option<bool>,
        outcome: &str,
        duration_ms: u128,
        result: Option<&Value>,
        error_code: Option<&str>,
    ) {
        let record = AgentLogRecord {
            timestamp: Utc::now().to_rfc3339(),
            session_id: self.session_id.clone(),
            correlation_id: new_correlation_id(),
            surface: surface.to_string(),
            name: name.to_string(),
            dangerous,
            acknowledged,
            outcome: outcome.to_string(),
            duration_ms,
            error_code: error_code.map(str::to_string),
            args: summarize_args(args),
            summary: result
                .map(|value| summarize_result_with_options(value, self.capture_position_snapshots))
                .unwrap_or(Value::Null),
        };

        if let Err(err) = self.append(&record) {
            if self.verbose {
                eprintln!("[agent-log] write failed: {err}");
            }
        }
    }

    fn append(&self, record: &AgentLogRecord) -> Result<(), VulcanError> {
        let _guard = self.lock.lock().ok();
        ensure_parent(&self.path)?;
        let mut file = open_append(&self.path)?;
        let line = serde_json::to_string(record)
            .map_err(|e| VulcanError::internal("AGENT_LOG_SERIALIZE_FAILED", e.to_string()))?;
        writeln!(file, "{line}")
            .map_err(|e| VulcanError::io("AGENT_LOG_WRITE_FAILED", e.to_string()))?;
        drop(file);
        self.apply_retention()?;
        Ok(())
    }

    fn apply_retention(&self) -> Result<(), VulcanError> {
        if self.retain_sessions == 0 && self.max_file_bytes == 0 {
            return Ok(());
        }
        let records = read_all(&self.path)?;
        let (kept, removed) = retain_records(records, self.retain_sessions, self.max_file_bytes)?;
        if removed.is_empty() {
            return Ok(());
        }

        if self.archive_session_summaries {
            archive_removed_sessions(&self.summary_archive_path, &removed)?;
        }
        rewrite_log(&self.path, &kept)?;
        Ok(())
    }
}

pub fn default_log_path(vulcan_dir: &Path) -> PathBuf {
    vulcan_dir.join("agent-actions.jsonl")
}

pub fn default_summary_archive_path(log_path: &Path) -> PathBuf {
    log_path
        .parent()
        .unwrap_or_else(|| Path::new("."))
        .join("agent-session-summaries.jsonl")
}

pub fn new_session_id() -> String {
    format!(
        "{}-{:016x}",
        Utc::now().format("%Y%m%dT%H%M%SZ"),
        rand::thread_rng().next_u64()
    )
}

pub fn new_correlation_id() -> String {
    format!("{:016x}", rand::thread_rng().next_u64())
}

pub fn summarize_args(args: &Value) -> Value {
    let Value::Object(map) = args else {
        return Value::Null;
    };

    let mut out = Map::new();
    for (key, value) in map {
        match key.as_str() {
            "symbol" | "side" | "order_type" | "scope" | "include" | "reduce_only" | "isolated"
            | "slide" => {
                out.insert(key.clone(), value.clone());
            }
            "bids" | "asks" | "order_ids" | "tp_levels" | "sl_levels" => {
                out.insert(
                    key.clone(),
                    serde_json::json!({
                        "provided": true,
                        "count": value.as_array().map(Vec::len)
                    }),
                );
            }
            "size" | "tokens" | "notional_usdc" | "price" | "tp" | "sl" | "collateral"
            | "amount" | "access_code" | "referral_code" | "invite_code" | "api_key"
            | "password" => {
                out.insert(key.clone(), serde_json::json!({ "provided": true }));
            }
            "acknowledged" => {}
            other => {
                out.insert(other.to_string(), serde_json::json!({ "provided": true }));
            }
        }
    }
    Value::Object(out)
}

pub fn summarize_result(value: &Value) -> Value {
    summarize_result_with_options(value, true)
}

pub fn summarize_result_with_options(value: &Value, capture_position_snapshots: bool) -> Value {
    let mut summary = Map::new();

    let tx_signatures = collect_string_values(value, "tx_signature");
    if !tx_signatures.is_empty() {
        summary.insert(
            "tx_signatures".to_string(),
            serde_json::json!(tx_signatures),
        );
    }

    let paper_fill_ids = collect_string_values(value, "fill_id");
    if !paper_fill_ids.is_empty() {
        summary.insert(
            "paper_fill_ids".to_string(),
            serde_json::json!(paper_fill_ids),
        );
    }

    for key in ["action", "symbol", "side", "order_type", "mode"] {
        if let Some(v) = find_string_value(value, key) {
            summary.insert(key.to_string(), Value::String(v));
        }
    }

    if let Some(v) = find_bool_value(value, "dry_run") {
        summary.insert("dry_run".to_string(), Value::Bool(v));
    }

    if capture_position_snapshots {
        let positions = collect_positions(value);
        if !positions.is_empty() {
            summary.insert(
                "position_count".to_string(),
                serde_json::json!(positions.len()),
            );
            summary.insert("positions".to_string(), serde_json::json!(positions));
        }

        if let Some(margin) = find_margin(value) {
            summary.insert("margin".to_string(), serde_json::json!(margin));
        }
    }

    if let Some(order_count) = find_array_count(value, "orders") {
        summary.insert("order_count".to_string(), serde_json::json!(order_count));
    }

    if summary.is_empty() {
        Value::Null
    } else {
        Value::Object(summary)
    }
}

pub fn read_recent(path: &Path, limit: usize) -> Result<Vec<AgentLogRecord>, VulcanError> {
    let mut records = read_all(path)?;
    let keep = limit.min(records.len());
    Ok(records.split_off(records.len().saturating_sub(keep)))
}

pub fn read_all(path: &Path) -> Result<Vec<AgentLogRecord>, VulcanError> {
    if !path.exists() {
        return Ok(Vec::new());
    }
    let file =
        File::open(path).map_err(|e| VulcanError::io("AGENT_LOG_READ_FAILED", e.to_string()))?;
    let reader = BufReader::new(file);
    let mut records = Vec::new();
    for line in reader.lines() {
        let line = line.map_err(|e| VulcanError::io("AGENT_LOG_READ_FAILED", e.to_string()))?;
        if line.trim().is_empty() {
            continue;
        }
        if let Ok(record) = serde_json::from_str::<AgentLogRecord>(&line) {
            records.push(record);
        }
    }
    Ok(records)
}

pub fn summarize_records(records: &[AgentLogRecord]) -> AgentSessionSummary {
    let mut sessions = BTreeSet::new();
    let mut tx_signatures = BTreeSet::new();
    let mut errors = Vec::new();
    let mut latest_positions = Vec::new();
    let mut latest_margin = None;
    let mut recent_actions = Vec::new();

    for record in records {
        sessions.insert(record.session_id.clone());
        let txs = summary_tx_signatures(&record.summary);
        let paper_fill_ids = summary_paper_fill_ids(&record.summary);
        for sig in &txs {
            tx_signatures.insert(sig.clone());
        }
        if let Some(code) = &record.error_code {
            errors.push(ActionErrorSummary {
                timestamp: record.timestamp.clone(),
                name: record.name.clone(),
                code: code.clone(),
            });
        }
        if let Some(positions) = summary_positions(&record.summary) {
            latest_positions = positions;
        }
        if let Some(margin) = summary_margin(&record.summary) {
            latest_margin = Some(margin);
        }
        recent_actions.push(ActionSummary {
            timestamp: record.timestamp.clone(),
            session_id: record.session_id.clone(),
            name: record.name.clone(),
            outcome: record.outcome.clone(),
            symbol: summary_string(&record.summary, "symbol")
                .or_else(|| arg_string(&record.args, "symbol")),
            side: summary_string(&record.summary, "side")
                .or_else(|| arg_string(&record.args, "side")),
            error_code: record.error_code.clone(),
            tx_signatures: txs,
            paper_fill_ids,
            mode: record_mode(record),
        });
    }

    AgentSessionSummary {
        total_records: records.len(),
        sessions: sessions.into_iter().collect(),
        tx_signatures: tx_signatures.into_iter().collect(),
        errors,
        latest_positions,
        latest_margin,
        recent_actions,
    }
}

pub fn summary_markdown(summary: &AgentSessionSummary) -> String {
    let mut out = String::new();
    out.push_str("# Vulcan Agent Session Summary\n\n");
    out.push_str(&format!("- Records: {}\n", summary.total_records));
    out.push_str(&format!("- Sessions: {}\n", summary.sessions.len()));
    out.push_str(&format!(
        "- Transaction signatures: {}\n",
        summary.tx_signatures.len()
    ));
    out.push_str(&format!("- Errors: {}\n", summary.errors.len()));

    if let Some(margin) = &summary.latest_margin {
        out.push_str("\n## Latest Margin\n\n");
        if let Some(v) = &margin.risk_state {
            out.push_str(&format!("- Risk state: {v}\n"));
        }
        if let Some(v) = &margin.portfolio_value {
            out.push_str(&format!("- Portfolio value: {v}\n"));
        }
        if let Some(v) = &margin.unrealized_pnl {
            out.push_str(&format!("- Unrealized PnL: {v}\n"));
        }
    }

    if !summary.latest_positions.is_empty() {
        out.push_str("\n## Latest Positions\n\n");
        for position in &summary.latest_positions {
            out.push_str(&format!(
                "- {} {} size {}",
                position.symbol, position.side, position.size
            ));
            if let Some(pnl) = &position.unrealized_pnl {
                out.push_str(&format!(" PnL {pnl}"));
            }
            out.push('\n');
        }
    }

    if !summary.recent_actions.is_empty() {
        out.push_str("\n## Recent Actions\n\n");
        for action in summary.recent_actions.iter().rev().take(10).rev() {
            out.push_str(&format!(
                "- {} {} {}",
                action.timestamp, action.name, action.outcome
            ));
            if let Some(symbol) = &action.symbol {
                out.push_str(&format!(" {symbol}"));
            }
            if !action.tx_signatures.is_empty() {
                out.push_str(&format!(" tx={}", action.tx_signatures.join(",")));
            }
            if !action.paper_fill_ids.is_empty() {
                out.push_str(&format!(" paper_fill={}", action.paper_fill_ids.join(",")));
            }
            if let Some(code) = &action.error_code {
                out.push_str(&format!(" error={code}"));
            }
            if let Some(mode) = &action.mode {
                out.push_str(&format!(" mode={mode}"));
            }
            out.push('\n');
        }
    }

    out
}

pub fn build_position_session_report(
    records: &[AgentLogRecord],
    live_portfolio: Option<&Value>,
    live_error: Option<String>,
    log_path: &Path,
    session: String,
    limit: usize,
) -> AgentPositionSessionReport {
    let session_summary = summarize_records(records);
    let live = live_portfolio
        .map(extract_live_trader_report)
        .unwrap_or_default();
    let live = LiveTraderReport {
        error: live_error,
        ..live
    };
    let mode_counts = count_modes(records);
    let paper_fill_count = records
        .iter()
        .flat_map(|record| summary_paper_fill_ids(&record.summary))
        .collect::<BTreeSet<_>>()
        .len();
    let metrics = SessionReportMetrics {
        actions: count_actions(records),
        modes: mode_counts,
        tx_count: session_summary.tx_signatures.len(),
        paper_fill_count,
        error_count: session_summary.errors.len(),
        open_position_win_rate: open_position_win_rate(&live.positions),
    };
    let recent_position_changes = position_changes(records);
    let inferred_fills = inferred_fills(records);
    let notes = vec![
        "Live margin, positions, and open orders come from the trader API at report time."
            .to_string(),
        "Session actions, transaction counts, and inferred fills come from the local redacted agent log."
            .to_string(),
        "Win rate is based on currently open positions with parseable unrealized PnL, not realized closed-trade history."
            .to_string(),
        "Limit-order fills are not authoritative until trade history is available; market trades with transaction signatures are reported as inferred fills."
            .to_string(),
        "Paper actions and paper PnL are local simulation results and are reported separately from live trading."
            .to_string(),
    ];

    AgentPositionSessionReport {
        generated_at: Utc::now().to_rfc3339(),
        log_path: log_path.to_string_lossy().to_string(),
        session,
        limit,
        live,
        session_summary,
        metrics,
        recent_position_changes,
        inferred_fills,
        notes,
    }
}

pub fn position_report_markdown(report: &AgentPositionSessionReport) -> String {
    let mut out = String::new();
    out.push_str("# Vulcan Position / Session Report\n\n");
    out.push_str(&format!("- Generated: {}\n", report.generated_at));
    out.push_str(&format!("- Session filter: {}\n", report.session));
    out.push_str(&format!(
        "- Records analyzed: {}\n",
        report.session_summary.total_records
    ));
    out.push_str(&format!(
        "- Transactions observed: {}\n",
        report.metrics.tx_count
    ));
    out.push_str(&format!(
        "- Paper fills observed: {}\n",
        report.metrics.paper_fill_count
    ));
    out.push_str(&format!(
        "- Errors observed: {}\n",
        report.metrics.error_count
    ));

    out.push_str("\n## Live Trader State\n\n");
    if let Some(error) = &report.live.error {
        out.push_str(&format!("- Live state error: {error}\n"));
    }
    if let Some(margin) = &report.live.margin {
        if let Some(v) = &margin.risk_state {
            out.push_str(&format!("- Risk state: {v}\n"));
        }
        if let Some(v) = &margin.portfolio_value {
            out.push_str(&format!("- Portfolio value: {v}\n"));
        }
        if let Some(v) = &margin.unrealized_pnl {
            out.push_str(&format!("- Unrealized PnL: {v}\n"));
        }
        if let Some(v) = &margin.collateral_balance {
            out.push_str(&format!("- Collateral: {v}\n"));
        }
    }
    out.push_str(&format!("- Open orders: {}\n", report.live.open_orders));
    out.push_str(&format!(
        "- TP/SL orders: {} TP, {} SL\n",
        report.live.take_profit_orders, report.live.stop_loss_orders
    ));

    if !report.live.positions.is_empty() {
        out.push_str("\n## Live Positions\n\n");
        for position in &report.live.positions {
            out.push_str(&format!(
                "- {} {} size {}",
                position.symbol, position.side, position.size
            ));
            if let Some(pnl) = &position.unrealized_pnl {
                out.push_str(&format!(" uPnL {pnl}"));
            }
            if let Some(mark) = &position.mark_price {
                out.push_str(&format!(" mark {mark}"));
            }
            out.push('\n');
        }
    }

    out.push_str("\n## Session Metrics\n\n");
    out.push_str(&format!(
        "- Successful trades: {} ({} market, {} limit)\n",
        report.metrics.actions.successful_trades,
        report.metrics.actions.market_trades,
        report.metrics.actions.limit_trades
    ));
    out.push_str(&format!(
        "- Mode split: {} live, {} dry-run, {} paper\n",
        report.metrics.modes.live_actions,
        report.metrics.modes.dry_run_actions,
        report.metrics.modes.paper_actions
    ));
    out.push_str(&format!(
        "- Cancels / closes / reductions: {} / {} / {}\n",
        report.metrics.actions.cancels,
        report.metrics.actions.position_closes,
        report.metrics.actions.position_reductions
    ));
    let win = &report.metrics.open_position_win_rate;
    match win.rate {
        Some(rate) => out.push_str(&format!(
            "- Open-position win rate: {:.1}% ({}/{} by {})\n",
            rate * 100.0,
            win.winners,
            win.sample_size,
            win.basis
        )),
        None => out.push_str("- Open-position win rate: unavailable\n"),
    }

    if !report.inferred_fills.is_empty() {
        out.push_str("\n## Inferred Fills\n\n");
        for fill in report.inferred_fills.iter().rev().take(10).rev() {
            out.push_str(&format!("- {} {}", fill.timestamp, fill.name));
            if let Some(symbol) = &fill.symbol {
                out.push_str(&format!(" {symbol}"));
            }
            if let Some(side) = &fill.side {
                out.push_str(&format!(" {side}"));
            }
            if !fill.tx_signatures.is_empty() {
                out.push_str(&format!(" tx={}", fill.tx_signatures.join(",")));
            }
            if !fill.paper_fill_ids.is_empty() {
                out.push_str(&format!(" paper_fill={}", fill.paper_fill_ids.join(",")));
            }
            if let Some(mode) = &fill.mode {
                out.push_str(&format!(" mode={mode}"));
            }
            out.push('\n');
        }
    }

    if !report.recent_position_changes.is_empty() {
        out.push_str("\n## Recent Position Changes\n\n");
        for change in report.recent_position_changes.iter().rev().take(10).rev() {
            out.push_str(&format!(
                "- {} {} {}",
                change.timestamp, change.name, change.outcome
            ));
            if let Some(symbol) = &change.symbol {
                out.push_str(&format!(" {symbol}"));
            }
            if !change.tx_signatures.is_empty() {
                out.push_str(&format!(" tx={}", change.tx_signatures.join(",")));
            }
            if !change.paper_fill_ids.is_empty() {
                out.push_str(&format!(" paper_fill={}", change.paper_fill_ids.join(",")));
            }
            if let Some(mode) = &change.mode {
                out.push_str(&format!(" mode={mode}"));
            }
            out.push('\n');
        }
    }

    out.push_str("\n## Notes\n\n");
    for note in &report.notes {
        out.push_str(&format!("- {note}\n"));
    }

    out
}

fn ensure_parent(path: &Path) -> Result<(), VulcanError> {
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent)
            .map_err(|e| VulcanError::io("AGENT_LOG_DIR_FAILED", e.to_string()))?;
    }
    Ok(())
}

fn open_append(path: &Path) -> Result<File, VulcanError> {
    let file = OpenOptions::new()
        .create(true)
        .append(true)
        .open(path)
        .map_err(|e| VulcanError::io("AGENT_LOG_OPEN_FAILED", e.to_string()))?;
    set_private_permissions(path);
    Ok(file)
}

#[cfg(unix)]
fn set_private_permissions(path: &Path) {
    use std::os::unix::fs::PermissionsExt;
    let _ = fs::set_permissions(path, fs::Permissions::from_mode(0o600));
}

#[cfg(not(unix))]
fn set_private_permissions(_path: &Path) {}

fn collect_string_values(value: &Value, key: &str) -> Vec<String> {
    let mut values = BTreeSet::new();
    collect_string_values_inner(value, key, &mut values);
    values.into_iter().collect()
}

fn collect_string_values_inner(value: &Value, key: &str, values: &mut BTreeSet<String>) {
    match value {
        Value::Object(map) => {
            if let Some(s) = map.get(key).and_then(Value::as_str) {
                values.insert(s.to_string());
            }
            for v in map.values() {
                collect_string_values_inner(v, key, values);
            }
        }
        Value::Array(items) => {
            for item in items {
                collect_string_values_inner(item, key, values);
            }
        }
        _ => {}
    }
}

fn find_string_value(value: &Value, key: &str) -> Option<String> {
    match value {
        Value::Object(map) => map
            .get(key)
            .and_then(Value::as_str)
            .map(str::to_string)
            .or_else(|| map.values().find_map(|v| find_string_value(v, key))),
        Value::Array(items) => items.iter().find_map(|v| find_string_value(v, key)),
        _ => None,
    }
}

fn find_bool_value(value: &Value, key: &str) -> Option<bool> {
    match value {
        Value::Object(map) => map
            .get(key)
            .and_then(Value::as_bool)
            .or_else(|| map.values().find_map(|v| find_bool_value(v, key))),
        Value::Array(items) => items.iter().find_map(|v| find_bool_value(v, key)),
        _ => None,
    }
}

fn collect_positions(value: &Value) -> Vec<PositionSummary> {
    let mut out = Vec::new();
    collect_positions_inner(value, &mut out);
    out
}

fn collect_positions_inner(value: &Value, out: &mut Vec<PositionSummary>) {
    match value {
        Value::Object(map) => {
            if let (Some(symbol), Some(side), Some(size)) = (
                string_field(map, "symbol"),
                string_field(map, "side"),
                string_field(map, "size").or_else(|| string_field(map, "size_tokens")),
            ) {
                out.push(PositionSummary {
                    symbol,
                    side,
                    size,
                    unrealized_pnl: string_field(map, "unrealized_pnl"),
                    mark_price: string_field(map, "mark_price"),
                });
            }
            for v in map.values() {
                collect_positions_inner(v, out);
            }
        }
        Value::Array(items) => {
            for item in items {
                collect_positions_inner(item, out);
            }
        }
        _ => {}
    }
}

fn find_margin(value: &Value) -> Option<MarginSummary> {
    match value {
        Value::Object(map) => {
            let margin = MarginSummary {
                risk_state: string_field(map, "risk_state"),
                portfolio_value: string_field(map, "portfolio_value"),
                unrealized_pnl: string_field(map, "unrealized_pnl"),
                collateral_balance: string_field(map, "collateral_balance"),
            };
            if margin.risk_state.is_some()
                || margin.portfolio_value.is_some()
                || margin.unrealized_pnl.is_some()
                || margin.collateral_balance.is_some()
            {
                return Some(margin);
            }
            map.values().find_map(find_margin)
        }
        Value::Array(items) => items.iter().find_map(find_margin),
        _ => None,
    }
}

fn find_array_count(value: &Value, key: &str) -> Option<usize> {
    match value {
        Value::Object(map) => map
            .get(key)
            .and_then(Value::as_array)
            .map(Vec::len)
            .or_else(|| map.values().find_map(|v| find_array_count(v, key))),
        Value::Array(items) => items.iter().find_map(|v| find_array_count(v, key)),
        _ => None,
    }
}

fn string_field(map: &Map<String, Value>, key: &str) -> Option<String> {
    map.get(key)
        .and_then(Value::as_str)
        .map(str::to_string)
        .or_else(|| map.get(key).and_then(Value::as_f64).map(|v| v.to_string()))
}

fn summary_tx_signatures(summary: &Value) -> Vec<String> {
    summary
        .get("tx_signatures")
        .and_then(Value::as_array)
        .map(|items| {
            items
                .iter()
                .filter_map(Value::as_str)
                .map(str::to_string)
                .collect()
        })
        .unwrap_or_default()
}

fn summary_paper_fill_ids(summary: &Value) -> Vec<String> {
    summary
        .get("paper_fill_ids")
        .and_then(Value::as_array)
        .map(|items| {
            items
                .iter()
                .filter_map(Value::as_str)
                .map(str::to_string)
                .collect()
        })
        .unwrap_or_default()
}

fn summary_positions(summary: &Value) -> Option<Vec<PositionSummary>> {
    serde_json::from_value(summary.get("positions")?.clone()).ok()
}

fn summary_margin(summary: &Value) -> Option<MarginSummary> {
    serde_json::from_value(summary.get("margin")?.clone()).ok()
}

fn summary_string(summary: &Value, key: &str) -> Option<String> {
    summary.get(key).and_then(Value::as_str).map(str::to_string)
}

fn arg_string(args: &Value, key: &str) -> Option<String> {
    args.get(key).and_then(Value::as_str).map(str::to_string)
}

fn record_mode(record: &AgentLogRecord) -> Option<String> {
    if summary_string(&record.summary, "mode").as_deref() == Some("paper")
        || record.name.starts_with("vulcan_paper")
        || record.name.starts_with("paper")
    {
        Some("paper".to_string())
    } else if record
        .summary
        .get("dry_run")
        .and_then(Value::as_bool)
        .unwrap_or(false)
    {
        Some("dry_run".to_string())
    } else if record.dangerous || !summary_tx_signatures(&record.summary).is_empty() {
        Some("live".to_string())
    } else {
        None
    }
}

fn extract_live_trader_report(value: &Value) -> LiveTraderReport {
    let orders = collect_order_reports(value);
    LiveTraderReport {
        margin: find_margin(value),
        positions: collect_positions(value),
        open_orders: orders.len(),
        reduce_only_orders: orders.iter().filter(|o| o.reduce_only).count(),
        take_profit_orders: orders.iter().filter(|o| o.is_take_profit).count(),
        stop_loss_orders: orders.iter().filter(|o| o.is_stop_loss).count(),
        error: None,
    }
}

#[derive(Debug)]
struct OrderReport {
    reduce_only: bool,
    is_take_profit: bool,
    is_stop_loss: bool,
}

fn collect_order_reports(value: &Value) -> Vec<OrderReport> {
    let mut out = Vec::new();
    collect_order_reports_inner(value, &mut out);
    out
}

fn collect_order_reports_inner(value: &Value, out: &mut Vec<OrderReport>) {
    match value {
        Value::Object(map) => {
            if map.contains_key("order_id") {
                out.push(OrderReport {
                    reduce_only: bool_field(map, "reduce_only"),
                    is_take_profit: bool_field(map, "is_take_profit"),
                    is_stop_loss: bool_field(map, "is_stop_loss"),
                });
            }
            for v in map.values() {
                collect_order_reports_inner(v, out);
            }
        }
        Value::Array(items) => {
            for item in items {
                collect_order_reports_inner(item, out);
            }
        }
        _ => {}
    }
}

fn bool_field(map: &Map<String, Value>, key: &str) -> bool {
    map.get(key).and_then(Value::as_bool).unwrap_or(false)
}

fn count_actions(records: &[AgentLogRecord]) -> ActionCountSummary {
    let mut counts = ActionCountSummary::default();
    for record in records.iter().filter(|record| record.outcome == "ok") {
        let name = record.name.as_str();
        if is_trade_record(record) {
            counts.successful_trades += 1;
            match arg_string(&record.args, "order_type")
                .or_else(|| summary_string(&record.summary, "order_type"))
                .as_deref()
            {
                Some("market") => counts.market_trades += 1,
                Some("limit") => counts.limit_trades += 1,
                _ if name.contains("market_") => counts.market_trades += 1,
                _ if name.contains("limit_") => counts.limit_trades += 1,
                _ => {}
            }
        }
        if name.contains("cancel") {
            counts.cancels += 1;
        }
        if name.contains("position_close") || name.contains("position.close") {
            counts.position_closes += 1;
        }
        if name.contains("position_reduce") || name.contains("position.reduce") {
            counts.position_reductions += 1;
        }
        if name.contains("tpsl") || name.contains("tp_sl") {
            counts.tpsl_updates += 1;
        }
        if name.contains("margin_") || name.contains("margin.") {
            counts.margin_actions += 1;
        }
    }
    counts
}

fn count_modes(records: &[AgentLogRecord]) -> ModeCountSummary {
    let mut counts = ModeCountSummary::default();
    for record in records {
        match record_mode(record).as_deref() {
            Some("paper") => counts.paper_actions += 1,
            Some("dry_run") => counts.dry_run_actions += 1,
            Some("live") => counts.live_actions += 1,
            _ => {}
        }
    }
    counts
}

fn is_trade_record(record: &AgentLogRecord) -> bool {
    let name = record.name.as_str();
    name == "vulcan_trade"
        || name.contains("trade.market_")
        || name.contains("trade.limit_")
        || name.contains("trade_multi_limit")
        || name.contains("paper_trade")
        || name.contains("paper.buy")
        || name.contains("paper.sell")
}

fn open_position_win_rate(positions: &[PositionSummary]) -> WinRateSummary {
    let mut winners = 0;
    let mut losers = 0;
    let mut flat = 0;
    for position in positions {
        let Some(pnl) = position
            .unrealized_pnl
            .as_deref()
            .and_then(parse_decimalish)
        else {
            continue;
        };
        if pnl > 0.0 {
            winners += 1;
        } else if pnl < 0.0 {
            losers += 1;
        } else {
            flat += 1;
        }
    }
    let sample_size = winners + losers + flat;
    WinRateSummary {
        basis: "open_unrealized_pnl".to_string(),
        winners,
        losers,
        flat,
        sample_size,
        rate: (sample_size > 0).then_some(winners as f64 / sample_size as f64),
    }
}

fn parse_decimalish(value: &str) -> Option<f64> {
    let cleaned: String = value
        .chars()
        .filter(|c| c.is_ascii_digit() || matches!(c, '.' | '-'))
        .collect();
    if cleaned.is_empty() || cleaned == "-" || cleaned == "." {
        None
    } else {
        cleaned.parse().ok()
    }
}

fn position_changes(records: &[AgentLogRecord]) -> Vec<PositionChangeSummary> {
    records
        .iter()
        .filter(|record| {
            record.outcome == "ok"
                && (is_trade_record(record)
                    || record.name.contains("position_")
                    || record.name.contains("position."))
        })
        .map(|record| PositionChangeSummary {
            timestamp: record.timestamp.clone(),
            name: record.name.clone(),
            outcome: record.outcome.clone(),
            symbol: summary_string(&record.summary, "symbol")
                .or_else(|| arg_string(&record.args, "symbol")),
            side: summary_string(&record.summary, "side")
                .or_else(|| arg_string(&record.args, "side")),
            tx_signatures: summary_tx_signatures(&record.summary),
            paper_fill_ids: summary_paper_fill_ids(&record.summary),
            mode: record_mode(record),
        })
        .collect()
}

fn inferred_fills(records: &[AgentLogRecord]) -> Vec<InferredFillSummary> {
    records
        .iter()
        .filter(|record| {
            record.outcome == "ok"
                && is_trade_record(record)
                && ((has_market_order_basis(record)
                    && !summary_tx_signatures(&record.summary).is_empty())
                    || !summary_paper_fill_ids(&record.summary).is_empty())
        })
        .map(|record| InferredFillSummary {
            timestamp: record.timestamp.clone(),
            name: record.name.clone(),
            basis: if !summary_paper_fill_ids(&record.summary).is_empty() {
                "paper_fill".to_string()
            } else {
                "successful_market_trade_with_tx_signature".to_string()
            },
            symbol: summary_string(&record.summary, "symbol")
                .or_else(|| arg_string(&record.args, "symbol")),
            side: summary_string(&record.summary, "side")
                .or_else(|| arg_string(&record.args, "side")),
            tx_signatures: summary_tx_signatures(&record.summary),
            paper_fill_ids: summary_paper_fill_ids(&record.summary),
            mode: record_mode(record),
        })
        .collect()
}

fn has_market_order_basis(record: &AgentLogRecord) -> bool {
    matches!(
        arg_string(&record.args, "order_type")
            .or_else(|| summary_string(&record.summary, "order_type"))
            .as_deref(),
        Some("market")
    ) || record.name.contains("market_")
}

pub fn default_summary_limit() -> usize {
    DEFAULT_SUMMARY_LIMIT
}

pub fn default_retain_sessions() -> usize {
    DEFAULT_RETAIN_SESSIONS
}

pub fn default_max_file_mb() -> u64 {
    DEFAULT_MAX_FILE_MB
}

pub fn mb_to_bytes(mb: u64) -> u64 {
    mb.saturating_mul(1024).saturating_mul(1024)
}

fn retain_records(
    records: Vec<AgentLogRecord>,
    retain_sessions: usize,
    max_file_bytes: u64,
) -> Result<(Vec<AgentLogRecord>, Vec<AgentLogRecord>), VulcanError> {
    let mut recent_sessions = Vec::<String>::new();
    for record in records.iter().rev() {
        if !recent_sessions.contains(&record.session_id) {
            recent_sessions.push(record.session_id.clone());
        }
        if retain_sessions > 0 && recent_sessions.len() >= retain_sessions {
            break;
        }
    }
    let keep_sessions: HashSet<String> = if retain_sessions == 0 {
        records.iter().map(|r| r.session_id.clone()).collect()
    } else {
        recent_sessions.into_iter().collect()
    };

    let mut kept = Vec::new();
    let mut removed = Vec::new();
    for record in records {
        if keep_sessions.contains(&record.session_id) {
            kept.push(record);
        } else {
            removed.push(record);
        }
    }

    if max_file_bytes > 0 {
        let mut bytes = 0u64;
        let mut size_kept_reversed = Vec::new();
        let mut size_removed = Vec::new();
        for record in kept.into_iter().rev() {
            let line_len = serde_json::to_string(&record)
                .map_err(|e| VulcanError::internal("AGENT_LOG_SERIALIZE_FAILED", e.to_string()))?
                .len() as u64
                + 1;
            if bytes.saturating_add(line_len) <= max_file_bytes || size_kept_reversed.is_empty() {
                bytes = bytes.saturating_add(line_len);
                size_kept_reversed.push(record);
            } else {
                size_removed.push(record);
            }
        }
        size_kept_reversed.reverse();
        size_removed.reverse();
        removed.extend(size_removed);
        kept = size_kept_reversed;
    }

    Ok((kept, removed))
}

fn rewrite_log(path: &Path, records: &[AgentLogRecord]) -> Result<(), VulcanError> {
    ensure_parent(path)?;
    let tmp = path.with_extension("jsonl.tmp");
    {
        let mut file = File::create(&tmp)
            .map_err(|e| VulcanError::io("AGENT_LOG_REWRITE_FAILED", e.to_string()))?;
        for record in records {
            let line = serde_json::to_string(record)
                .map_err(|e| VulcanError::internal("AGENT_LOG_SERIALIZE_FAILED", e.to_string()))?;
            writeln!(file, "{line}")
                .map_err(|e| VulcanError::io("AGENT_LOG_REWRITE_FAILED", e.to_string()))?;
        }
    }
    fs::rename(&tmp, path)
        .map_err(|e| VulcanError::io("AGENT_LOG_REWRITE_FAILED", e.to_string()))?;
    set_private_permissions(path);
    Ok(())
}

fn archive_removed_sessions(path: &Path, records: &[AgentLogRecord]) -> Result<(), VulcanError> {
    let mut sessions = Vec::<String>::new();
    for record in records {
        if !sessions.contains(&record.session_id) {
            sessions.push(record.session_id.clone());
        }
    }
    if sessions.is_empty() {
        return Ok(());
    }

    ensure_parent(path)?;
    let mut file = open_append(path)?;
    for session_id in sessions {
        let session_records: Vec<AgentLogRecord> = records
            .iter()
            .filter(|record| record.session_id == session_id)
            .cloned()
            .collect();
        let archive = ArchivedSessionSummary {
            archived_at: Utc::now().to_rfc3339(),
            session_id,
            summary: summarize_records(&session_records),
        };
        let line = serde_json::to_string(&archive)
            .map_err(|e| VulcanError::internal("AGENT_LOG_SERIALIZE_FAILED", e.to_string()))?;
        writeln!(file, "{line}")
            .map_err(|e| VulcanError::io("AGENT_LOG_ARCHIVE_FAILED", e.to_string()))?;
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn redacts_sensitive_args() {
        let args = serde_json::json!({
            "symbol": "SOL",
            "side": "buy",
            "notional_usdc": 100,
            "access_code": "SECRET",
            "password": "WALLET_SECRET",
            "VULCAN_WALLET_PASSWORD": "ENV_SECRET",
            "private_key": "PRIVATE_SECRET",
            "refresh_token": "TOKEN_SECRET",
            "acknowledged": true
        });
        let redacted = summarize_args(&args);
        assert_eq!(redacted["symbol"], "SOL");
        assert_eq!(redacted["notional_usdc"]["provided"], true);
        assert!(redacted.get("acknowledged").is_none());
        assert_ne!(redacted["access_code"], "SECRET");
        let encoded = serde_json::to_string(&redacted).unwrap();
        assert!(!encoded.contains("WALLET_SECRET"));
        assert!(!encoded.contains("ENV_SECRET"));
        assert!(!encoded.contains("PRIVATE_SECRET"));
        assert!(!encoded.contains("TOKEN_SECRET"));
    }

    #[test]
    fn summarizes_positions_and_tx() {
        let value = serde_json::json!({
            "tx_signature": "abc",
            "positions": [{
                "symbol": "SOL",
                "side": "Long",
                "size": "1.0",
                "unrealized_pnl": "2.3",
                "mark_price": "150"
            }]
        });
        let summary = summarize_result(&value);
        assert_eq!(summary["tx_signatures"][0], "abc");
        assert_eq!(summary["position_count"], 1);
        assert_eq!(summary["positions"][0]["symbol"], "SOL");
    }

    #[test]
    fn summarizes_paper_mode_and_fill_ids() {
        let value = serde_json::json!({
            "mode": "paper",
            "fill": {
                "fill_id": "paper-fill-1",
                "symbol": "SOL",
                "side": "buy",
                "order_type": "market",
                "size_tokens": 1.0,
                "unrealized_pnl": "0"
            }
        });
        let summary = summarize_result(&value);
        assert_eq!(summary["mode"], "paper");
        assert_eq!(summary["paper_fill_ids"][0], "paper-fill-1");
        assert_eq!(summary["positions"][0]["size"], "1");
    }

    #[test]
    fn can_skip_position_snapshot_summaries() {
        let value = serde_json::json!({
            "tx_signature": "abc",
            "positions": [{
                "symbol": "SOL",
                "side": "Long",
                "size": "1.0",
                "unrealized_pnl": "2.3"
            }]
        });
        let summary = summarize_result_with_options(&value, false);
        assert_eq!(summary["tx_signatures"][0], "abc");
        assert!(summary.get("positions").is_none());
        assert!(summary.get("position_count").is_none());
    }

    #[test]
    fn appends_and_reads_jsonl() {
        let tmp = tempfile::tempdir().unwrap();
        let path = tmp.path().join("agent-actions.jsonl");
        let sink = AgentLogSink::new(
            path.clone(),
            "session".into(),
            true,
            true,
            20,
            mb_to_bytes(25),
            true,
        );
        sink.record_call(
            "mcp",
            "vulcan_market_ticker",
            &serde_json::json!({ "symbol": "SOL" }),
            false,
            None,
            "ok",
            12,
            Some(&serde_json::json!({ "symbol": "SOL" })),
            None,
        );
        let records = read_recent(&path, 10).unwrap();
        assert_eq!(records.len(), 1);
        assert_eq!(records[0].session_id, "session");
    }

    #[test]
    fn aggregates_summary() {
        let records = vec![AgentLogRecord {
            timestamp: "2026-01-01T00:00:00Z".into(),
            session_id: "s1".into(),
            correlation_id: "c1".into(),
            surface: "mcp".into(),
            name: "vulcan_trade".into(),
            dangerous: true,
            acknowledged: Some(true),
            outcome: "ok".into(),
            duration_ms: 1,
            error_code: None,
            args: serde_json::json!({ "symbol": "SOL", "side": "buy" }),
            summary: serde_json::json!({ "tx_signatures": ["abc"], "symbol": "SOL" }),
        }];
        let summary = summarize_records(&records);
        assert_eq!(summary.total_records, 1);
        assert_eq!(summary.tx_signatures, vec!["abc".to_string()]);
        assert_eq!(summary.recent_actions[0].symbol.as_deref(), Some("SOL"));
    }

    #[test]
    fn retention_keeps_recent_sessions_and_archives_removed() {
        let tmp = tempfile::tempdir().unwrap();
        let path = tmp.path().join("agent-actions.jsonl");

        for session in ["s1", "s2", "s3"] {
            let sink = AgentLogSink::new(
                path.clone(),
                session.into(),
                true,
                true,
                2,
                mb_to_bytes(25),
                true,
            );
            sink.record_call(
                "mcp",
                "vulcan_trade",
                &serde_json::json!({ "symbol": "SOL" }),
                true,
                Some(true),
                "ok",
                1,
                Some(&serde_json::json!({ "tx_signature": session })),
                None,
            );
        }

        let records = read_all(&path).unwrap();
        assert_eq!(records.len(), 2);
        assert_eq!(records[0].session_id, "s2");
        assert_eq!(records[1].session_id, "s3");

        let archive = fs::read_to_string(default_summary_archive_path(&path)).unwrap();
        assert!(archive.contains("\"session_id\":\"s1\""));
    }

    #[test]
    fn retention_caps_file_size_but_keeps_latest_record() {
        let tmp = tempfile::tempdir().unwrap();
        let path = tmp.path().join("agent-actions.jsonl");
        let sink = AgentLogSink::new(path.clone(), "session".into(), true, true, 20, 10, false);

        sink.record_call(
            "mcp",
            "vulcan_market_ticker",
            &serde_json::json!({ "symbol": "SOL" }),
            false,
            None,
            "ok",
            1,
            Some(&serde_json::json!({ "symbol": "SOL" })),
            None,
        );
        sink.record_call(
            "mcp",
            "vulcan_market_ticker",
            &serde_json::json!({ "symbol": "BTC" }),
            false,
            None,
            "ok",
            1,
            Some(&serde_json::json!({ "symbol": "BTC" })),
            None,
        );

        let records = read_all(&path).unwrap();
        assert_eq!(records.len(), 1);
        assert_eq!(records[0].args["symbol"], "BTC");
    }

    #[test]
    fn builds_position_session_report_from_live_state_and_logs() {
        let records = vec![AgentLogRecord {
            timestamp: "2026-01-01T00:00:00Z".into(),
            session_id: "s1".into(),
            correlation_id: "c1".into(),
            surface: "mcp".into(),
            name: "vulcan_trade".into(),
            dangerous: true,
            acknowledged: Some(true),
            outcome: "ok".into(),
            duration_ms: 1,
            error_code: None,
            args: serde_json::json!({
                "symbol": "SOL",
                "side": "buy",
                "order_type": "market"
            }),
            summary: serde_json::json!({
                "tx_signatures": ["abc"],
                "symbol": "SOL",
                "side": "buy",
                "order_type": "market"
            }),
        }];
        let live = serde_json::json!({
            "margin": {
                "risk_state": "Healthy",
                "portfolio_value": "1000",
                "unrealized_pnl": "12.5"
            },
            "positions": {
                "positions": [{
                    "symbol": "SOL",
                    "side": "Long",
                    "size": "1.0",
                    "unrealized_pnl": "12.5",
                    "mark_price": "150"
                }]
            },
            "orders": {
                "orders": [{
                    "symbol": "SOL",
                    "side": "Ask",
                    "order_id": "1",
                    "price": "200",
                    "size_remaining": "1",
                    "initial_size": "1",
                    "reduce_only": true,
                    "is_take_profit": true,
                    "is_stop_loss": false
                }]
            }
        });

        let report = build_position_session_report(
            &records,
            Some(&live),
            None,
            Path::new("/tmp/agent-actions.jsonl"),
            "all".to_string(),
            50,
        );

        assert_eq!(report.metrics.actions.successful_trades, 1);
        assert_eq!(report.metrics.actions.market_trades, 1);
        assert_eq!(report.inferred_fills.len(), 1);
        assert_eq!(report.live.open_orders, 1);
        assert_eq!(report.live.take_profit_orders, 1);
        assert_eq!(report.metrics.open_position_win_rate.winners, 1);
    }

    #[test]
    fn report_separates_paper_activity_from_live_activity() {
        let records = vec![AgentLogRecord {
            timestamp: "2026-01-01T00:00:00Z".into(),
            session_id: "s1".into(),
            correlation_id: "c1".into(),
            surface: "mcp".into(),
            name: "vulcan_paper_trade".into(),
            dangerous: false,
            acknowledged: None,
            outcome: "ok".into(),
            duration_ms: 1,
            error_code: None,
            args: serde_json::json!({
                "symbol": "SOL",
                "side": "buy",
                "order_type": "market"
            }),
            summary: serde_json::json!({
                "mode": "paper",
                "paper_fill_ids": ["paper-fill-1"],
                "symbol": "SOL",
                "side": "buy",
                "order_type": "market"
            }),
        }];

        let report = build_position_session_report(
            &records,
            None,
            None,
            Path::new("/tmp/agent-actions.jsonl"),
            "all".to_string(),
            50,
        );

        assert_eq!(report.metrics.modes.paper_actions, 1);
        assert_eq!(report.metrics.modes.live_actions, 0);
        assert_eq!(report.metrics.paper_fill_count, 1);
        assert_eq!(report.inferred_fills[0].mode.as_deref(), Some("paper"));
    }
}

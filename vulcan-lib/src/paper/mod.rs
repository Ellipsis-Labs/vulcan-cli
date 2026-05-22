//! Local paper trading engine for Phoenix perpetuals.
//!
//! Paper trading uses live market data but stores all account state locally. It
//! never signs or submits Solana transactions.

use crate::commands::market::{execute_info_inner, execute_orderbook_inner, execute_ticker_inner};
use crate::commands::trade::{TpSlInput, TpSlSize};
use crate::context::AppContext;
use crate::error::VulcanError;
use chrono::Utc;
use rand::RngCore;
use serde::{Deserialize, Serialize};
use std::fs;
use std::path::{Path, PathBuf};

const DEFAULT_BALANCE: f64 = 10_000.0;
const DEFAULT_FEE_BPS: f64 = 5.0;

fn default_next_trigger_id() -> u64 {
    1
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PaperState {
    pub mode: String,
    pub currency: String,
    pub starting_balance: f64,
    pub balance: f64,
    pub fee_bps: f64,
    pub created_at: String,
    pub updated_at: String,
    pub next_order_id: u64,
    pub next_fill_id: u64,
    #[serde(default = "default_next_trigger_id")]
    pub next_trigger_id: u64,
    /// RFC3339 timestamp of the last time triggers were evaluated against
    /// market data. Used by `reconcile` to fetch historical candles for the
    /// gap window and replay missed crossings. `None` on a fresh init or for
    /// state files written before candle replay was added.
    #[serde(default)]
    pub last_evaluated_at: Option<String>,
    pub positions: Vec<PaperPosition>,
    pub orders: Vec<PaperOrder>,
    pub fills: Vec<PaperFill>,
    #[serde(default)]
    pub triggers: Vec<PaperTrigger>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PaperPosition {
    pub symbol: String,
    pub side: String,
    pub size_tokens: f64,
    pub size_lots: u64,
    pub entry_price: f64,
    pub mark_price: f64,
    pub unrealized_pnl: f64,
    pub realized_pnl: f64,
    /// Active take-profit triggers, projected from `PaperState.triggers` on
    /// every read of this position. Never persisted in `paper-state.json`
    /// (always empty in storage); populated by `paper::positions(..)`.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub tp_levels: Vec<PaperPositionTpSl>,
    /// Active stop-loss triggers. Same semantics as `tp_levels`.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub sl_levels: Vec<PaperPositionTpSl>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PaperPositionTpSl {
    pub trigger_id: String,
    pub price: f64,
    pub size_tokens: f64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PaperOrder {
    pub order_id: String,
    pub symbol: String,
    pub side: String,
    pub price: f64,
    pub size_tokens: f64,
    pub size_lots: u64,
    pub created_at: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PaperFill {
    pub fill_id: String,
    pub order_id: Option<String>,
    pub symbol: String,
    pub side: String,
    pub order_type: String,
    pub price: f64,
    pub size_tokens: f64,
    pub size_lots: u64,
    pub fee: f64,
    pub realized_pnl: f64,
    pub timestamp: String,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum PaperTriggerKind {
    TakeProfit,
    StopLoss,
}

impl PaperTriggerKind {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::TakeProfit => "take_profit",
            Self::StopLoss => "stop_loss",
        }
    }
}

/// A take-profit or stop-loss attached to a paper position. Mirrors the live
/// `ConditionalTriggerView` shape closely enough that callers can compare paper
/// and live behavior side by side.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PaperTrigger {
    pub trigger_id: String,
    pub symbol: String,
    pub kind: PaperTriggerKind,
    pub position_side: String,
    pub trigger_price: f64,
    pub size_tokens: f64,
    pub size_lots: u64,
    /// `None` once the trigger is active against an open position. `Some(order_id)`
    /// while still parented to a resting paper limit order — re-parented to the
    /// position when that order fills (used by Phase 4 order-time TP/SL).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub parent_order_id: Option<String>,
    pub created_at: String,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize)]
pub enum PaperSide {
    Buy,
    Sell,
}

impl PaperSide {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Buy => "buy",
            Self::Sell => "sell",
        }
    }

    fn signed(self, size_tokens: f64) -> f64 {
        match self {
            Self::Buy => size_tokens,
            Self::Sell => -size_tokens,
        }
    }
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize)]
pub enum PaperOrderType {
    Market,
    Limit,
}

impl PaperOrderType {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Market => "market",
            Self::Limit => "limit",
        }
    }
}

#[derive(Debug, Clone, Serialize)]
pub struct PaperStatus {
    pub mode: String,
    pub path: String,
    pub currency: String,
    pub starting_balance: f64,
    pub balance: f64,
    pub equity: f64,
    pub position_notional_usdc: f64,
    pub exposure_ratio: f64,
    pub unrealized_pnl: f64,
    pub realized_pnl: f64,
    pub fees_paid: f64,
    pub open_positions: usize,
    pub open_orders: usize,
    pub triggers: usize,
    pub fills: usize,
}

#[derive(Debug, Clone, Serialize)]
pub struct PaperInitResult {
    pub mode: String,
    pub path: String,
    pub state: PaperStatus,
}

#[derive(Debug, Clone, Serialize)]
pub struct PaperOrderResult {
    pub mode: String,
    pub action: String,
    pub symbol: String,
    pub side: String,
    pub order_type: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub order_id: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub fill: Option<PaperFill>,
    /// Order-time TP/SL triggers attached by this order (active or pending).
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub attached_triggers: Vec<PaperTrigger>,
    /// TP/SL triggers that fired as a side effect of this order's market refresh.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub trigger_fills: Vec<PaperFill>,
    pub state: PaperStatus,
}

#[derive(Debug, Clone, Serialize)]
pub struct PaperCancelResult {
    pub mode: String,
    pub cancelled: usize,
    pub order_ids: Vec<String>,
    pub state: PaperStatus,
}

#[derive(Debug, Clone, Serialize)]
pub struct PaperReconcileResult {
    pub mode: String,
    pub checked_orders: usize,
    pub fills: Vec<PaperFill>,
    pub state: PaperStatus,
}

#[derive(Debug, Clone, Serialize)]
pub struct PaperPositionsResult {
    pub mode: String,
    pub currency: String,
    pub equity: f64,
    pub position_notional_usdc: f64,
    pub exposure_ratio: f64,
    pub positions: Vec<PaperPosition>,
}

#[derive(Debug, Clone, Serialize)]
pub struct PaperOrdersResult {
    pub mode: String,
    pub orders: Vec<PaperOrder>,
}

#[derive(Debug, Clone, Serialize)]
pub struct PaperFillsResult {
    pub mode: String,
    pub fills: Vec<PaperFill>,
}

#[derive(Debug, Clone, Serialize)]
pub struct PaperSetTpSlResult {
    pub mode: String,
    pub symbol: String,
    pub position_side: String,
    pub tp_levels: Vec<PaperTpSlLevel>,
    pub sl_levels: Vec<PaperTpSlLevel>,
    pub triggers: Vec<PaperTrigger>,
    pub state: PaperStatus,
}

#[derive(Debug, Clone, Serialize)]
pub struct PaperTpSlLevel {
    pub trigger_id: String,
    pub price: f64,
    pub size_tokens: f64,
    pub size_lots: u64,
}

#[derive(Debug, Clone, Serialize)]
pub struct PaperCancelTpSlResult {
    pub mode: String,
    pub symbol: String,
    pub cancelled: usize,
    pub trigger_ids: Vec<String>,
    pub state: PaperStatus,
}

#[derive(Debug, Clone, Serialize)]
pub struct PaperTriggersResult {
    pub mode: String,
    pub triggers: Vec<PaperTrigger>,
}

#[derive(Debug, Clone, Copy)]
pub struct PaperSizeInput {
    pub size_lots: Option<f64>,
    pub tokens: Option<f64>,
    pub notional_usdc: Option<f64>,
}

#[derive(Debug, Clone)]
pub struct PaperOrderRequest {
    pub symbol: String,
    pub side: PaperSide,
    pub order_type: PaperOrderType,
    pub size: PaperSizeInput,
    pub price: Option<f64>,
    /// Optional take-profit price attached at order time. Activates immediately
    /// when the entry fills; sits pending if the order rests.
    pub tp: Option<f64>,
    /// Optional stop-loss price attached at order time. Same semantics as `tp`.
    pub sl: Option<f64>,
}

#[derive(Debug, Clone, Copy)]
struct ResolvedPaperSize {
    lots: u64,
    tokens: f64,
}

#[derive(Debug, Clone, Copy)]
struct PaperMarketPrice {
    mark: f64,
    bid: Option<f64>,
    ask: Option<f64>,
}

pub fn default_state_path(vulcan_dir: &Path) -> PathBuf {
    vulcan_dir.join("paper-state.json")
}

impl PaperState {
    pub fn new(balance: f64, currency: String, fee_bps: f64) -> Self {
        let now = Utc::now().to_rfc3339();
        Self {
            mode: "paper".to_string(),
            currency,
            starting_balance: balance,
            balance,
            fee_bps,
            created_at: now.clone(),
            updated_at: now,
            next_order_id: 1,
            next_fill_id: 1,
            next_trigger_id: 1,
            last_evaluated_at: None,
            positions: Vec::new(),
            orders: Vec::new(),
            fills: Vec::new(),
            triggers: Vec::new(),
        }
    }

    fn touch(&mut self) {
        self.updated_at = Utc::now().to_rfc3339();
    }

    fn next_order_id(&mut self) -> String {
        let id = format!(
            "paper-order-{}-{:08x}",
            self.next_order_id,
            rand::thread_rng().next_u32()
        );
        self.next_order_id += 1;
        id
    }

    fn next_fill_id(&mut self) -> String {
        let id = format!(
            "paper-fill-{}-{:08x}",
            self.next_fill_id,
            rand::thread_rng().next_u32()
        );
        self.next_fill_id += 1;
        id
    }

    fn next_trigger_id(&mut self) -> String {
        let id = format!(
            "paper-trigger-{}-{:08x}",
            self.next_trigger_id,
            rand::thread_rng().next_u32()
        );
        self.next_trigger_id += 1;
        id
    }
}

pub fn initial_balance_or_default(balance: Option<f64>) -> f64 {
    balance.unwrap_or(DEFAULT_BALANCE)
}

pub fn fee_bps_or_default(fee_bps: Option<f64>) -> f64 {
    fee_bps.unwrap_or(DEFAULT_FEE_BPS)
}

pub fn init_state(
    vulcan_dir: &Path,
    balance: Option<f64>,
    currency: Option<String>,
    fee_bps: Option<f64>,
) -> Result<(PathBuf, PaperState), VulcanError> {
    let balance = initial_balance_or_default(balance);
    if !balance.is_finite() || balance <= 0.0 {
        return Err(VulcanError::validation(
            "INVALID_PAPER_BALANCE",
            "paper balance must be positive",
        ));
    }
    let fee_bps = fee_bps_or_default(fee_bps);
    if !fee_bps.is_finite() || fee_bps < 0.0 {
        return Err(VulcanError::validation(
            "INVALID_PAPER_FEE",
            "paper fee bps must be non-negative",
        ));
    }
    let path = default_state_path(vulcan_dir);
    let state = PaperState::new(
        balance,
        currency.unwrap_or_else(|| "USDC".to_string()),
        fee_bps,
    );
    save_state(&path, &state)?;
    Ok((path, state))
}

pub fn load_state(vulcan_dir: &Path) -> Result<(PathBuf, PaperState), VulcanError> {
    let path = default_state_path(vulcan_dir);
    if !path.exists() {
        return Err(VulcanError::config(
            "PAPER_NOT_INITIALIZED",
            "Paper account not initialized. Run `vulcan paper init` first.",
        ));
    }
    let text = fs::read_to_string(&path)
        .map_err(|e| VulcanError::io("PAPER_STATE_READ_FAILED", e.to_string()))?;
    let state = serde_json::from_str(&text)
        .map_err(|e| VulcanError::io("PAPER_STATE_PARSE_FAILED", e.to_string()))?;
    Ok((path, state))
}

pub fn save_state(path: &Path, state: &PaperState) -> Result<(), VulcanError> {
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent)
            .map_err(|e| VulcanError::io("PAPER_STATE_DIR_FAILED", e.to_string()))?;
    }
    let text = serde_json::to_string_pretty(state)
        .map_err(|e| VulcanError::internal("PAPER_STATE_SERIALIZE_FAILED", e.to_string()))?;
    fs::write(path, text)
        .map_err(|e| VulcanError::io("PAPER_STATE_WRITE_FAILED", e.to_string()))?;
    set_private_permissions(path);
    Ok(())
}

#[cfg(unix)]
fn set_private_permissions(path: &Path) {
    use std::os::unix::fs::PermissionsExt;
    let _ = fs::set_permissions(path, fs::Permissions::from_mode(0o600));
}

#[cfg(not(unix))]
fn set_private_permissions(_path: &Path) {}

/// RAII guard that holds an exclusive advisory flock on the paper-state lock file.
/// Concurrent `vulcan` processes (e.g. parallel strategy runners) block here
/// until the previous writer releases the lock.  The guard is intentionally
/// opaque — callers just hold it in scope until the matching `save_state` call
/// returns, then drop it.
#[cfg(unix)]
pub struct PaperStateLock {
    _file: std::fs::File,
}

#[cfg(unix)]
impl PaperStateLock {
    pub fn acquire(vulcan_dir: &Path) -> Result<Self, VulcanError> {
        use std::fs::OpenOptions;
        use std::os::unix::io::AsRawFd;

        extern "C" {
            fn flock(fd: std::ffi::c_int, operation: std::ffi::c_int) -> std::ffi::c_int;
        }
        const LOCK_EX: std::ffi::c_int = 2;

        let lock_path = vulcan_dir.join("paper-state.lock");
        let file = OpenOptions::new()
            .write(true)
            .create(true)
            .open(&lock_path)
            .map_err(|e| VulcanError::io("PAPER_LOCK_OPEN_FAILED", e.to_string()))?;
        let ret = unsafe { flock(file.as_raw_fd(), LOCK_EX) };
        if ret != 0 {
            return Err(VulcanError::io(
                "PAPER_LOCK_ACQUIRE_FAILED",
                std::io::Error::last_os_error().to_string(),
            ));
        }
        Ok(PaperStateLock { _file: file })
    }
}

#[cfg(unix)]
impl Drop for PaperStateLock {
    fn drop(&mut self) {
        use std::os::unix::io::AsRawFd;
        extern "C" {
            fn flock(fd: std::ffi::c_int, operation: std::ffi::c_int) -> std::ffi::c_int;
        }
        const LOCK_UN: std::ffi::c_int = 8;
        unsafe { flock(self._file.as_raw_fd(), LOCK_UN) };
    }
}

/// Acquires the paper-state exclusive lock, then loads the state.
/// Hold the returned `PaperStateLock` in scope until after `save_state` completes.
/// On non-Unix platforms this is equivalent to `load_state` with no locking.
#[cfg(unix)]
pub fn locked_load_state(
    vulcan_dir: &Path,
) -> Result<(PathBuf, PaperState, PaperStateLock), VulcanError> {
    let lock = PaperStateLock::acquire(vulcan_dir)?;
    let (path, state) = load_state(vulcan_dir)?;
    Ok((path, state, lock))
}

#[cfg(not(unix))]
pub fn locked_load_state(vulcan_dir: &Path) -> Result<(PathBuf, PaperState, ()), VulcanError> {
    let (path, state) = load_state(vulcan_dir)?;
    Ok((path, state, ()))
}

pub async fn mark_state(
    ctx: &AppContext,
    mut state: PaperState,
) -> Result<PaperState, VulcanError> {
    let symbols: Vec<String> = state.positions.iter().map(|p| p.symbol.clone()).collect();
    for symbol in symbols {
        let price = fetch_paper_price(ctx, &symbol).await?;
        refresh_position_marks(&mut state, &symbol, price.mark);
    }
    state.touch();
    Ok(state)
}

pub async fn status(
    ctx: &AppContext,
    path: &Path,
    state: PaperState,
) -> Result<PaperStatus, VulcanError> {
    let state = mark_state(ctx, state).await?;
    Ok(status_from_state(path, &state))
}

pub async fn place_order(
    ctx: &AppContext,
    state_path: &Path,
    mut state: PaperState,
    request: PaperOrderRequest,
) -> Result<(PaperOrderResult, PaperState), VulcanError> {
    let symbol = request.symbol.as_str();
    let side = request.side;
    let order_type = request.order_type;
    let price = request.price;
    let market = fetch_paper_price(ctx, symbol).await?;
    let limit_or_mark = price.unwrap_or(market.mark);
    let resolved = resolve_size(ctx, symbol, request.size, limit_or_mark).await?;
    let mut fill = None;
    let mut order_id = None;

    match order_type {
        PaperOrderType::Market => {
            let fill_price = match side {
                PaperSide::Buy => market.ask.unwrap_or(market.mark),
                PaperSide::Sell => market.bid.unwrap_or(market.mark),
            };
            fill = Some(apply_fill(
                &mut state,
                None,
                symbol,
                side,
                PaperOrderType::Market,
                fill_price,
                resolved,
            ));
        }
        PaperOrderType::Limit => {
            let price = price.ok_or_else(|| {
                VulcanError::validation("MISSING_PRICE", "paper limit orders require price")
            })?;
            let crosses = limit_crosses(side, price, market);
            if crosses {
                fill = Some(apply_fill(
                    &mut state,
                    None,
                    symbol,
                    side,
                    PaperOrderType::Limit,
                    price,
                    resolved,
                ));
            } else {
                let id = state.next_order_id();
                state.orders.push(PaperOrder {
                    order_id: id.clone(),
                    symbol: symbol.to_string(),
                    side: side.as_str().to_string(),
                    price,
                    size_tokens: resolved.tokens,
                    size_lots: resolved.lots,
                    created_at: Utc::now().to_rfc3339(),
                });
                order_id = Some(id);
            }
        }
    }

    let attached_triggers = if request.tp.is_some() || request.sl.is_some() {
        let (reference_price, parent_id) = if let Some(f) = &fill {
            (f.price, None)
        } else if let Some(id) = &order_id {
            (
                price.ok_or_else(|| {
                    VulcanError::validation(
                        "MISSING_PRICE",
                        "resting limit order has no price for TP/SL validation",
                    )
                })?,
                Some(id.clone()),
            )
        } else {
            return Err(VulcanError::internal(
                "PAPER_ORDER_NO_OUTCOME",
                "paper order produced neither fill nor resting order",
            ));
        };
        let now = Utc::now().to_rfc3339();
        build_order_time_triggers(
            &mut state,
            symbol,
            side,
            request.tp,
            request.sl,
            reference_price,
            resolved.lots,
            resolved.tokens,
            parent_id,
            &now,
        )?
    } else {
        Vec::new()
    };

    let trigger_fills = refresh_and_evaluate(&mut state, symbol, market.mark);
    state.touch();
    save_state(state_path, &state)?;
    let result = PaperOrderResult {
        mode: "paper".to_string(),
        action: if fill.is_some() {
            "filled".to_string()
        } else {
            "resting".to_string()
        },
        symbol: symbol.to_string(),
        side: side.as_str().to_string(),
        order_type: order_type.as_str().to_string(),
        order_id,
        fill,
        attached_triggers,
        trigger_fills,
        state: status_from_state(state_path, &state),
    };
    Ok((result, state))
}

pub async fn reconcile(
    ctx: &AppContext,
    state_path: &Path,
    mut state: PaperState,
    symbol_filter: Option<&str>,
) -> Result<(PaperReconcileResult, PaperState), VulcanError> {
    let mut fills = Vec::new();
    let checked_orders = state
        .orders
        .iter()
        .filter(|o| symbol_filter.map(|s| s == o.symbol).unwrap_or(true))
        .count();

    let symbol_match = |s: &str| symbol_filter.map(|f| f == s).unwrap_or(true);
    let mut symbols: Vec<String> = Vec::new();
    for o in &state.orders {
        if symbol_match(&o.symbol) && !symbols.contains(&o.symbol) {
            symbols.push(o.symbol.clone());
        }
    }
    for t in &state.triggers {
        if t.parent_order_id.is_none() && symbol_match(&t.symbol) && !symbols.contains(&t.symbol) {
            symbols.push(t.symbol.clone());
        }
    }

    // If there's a meaningful gap since the last evaluation, replay historical
    // candles so triggers that crossed between sessions fire at their actual
    // prices (with the candle's close time as the fill timestamp). Falls back
    // to "current mark only" on first run or if the candle API doesn't answer.
    let now_ms = chrono::Utc::now().timestamp_millis();
    let replay_since_ms: Option<i64> = state.last_evaluated_at.as_deref().and_then(|s| {
        chrono::DateTime::parse_from_rfc3339(s)
            .ok()
            .map(|dt| dt.timestamp_millis())
    });

    let mut markets: Vec<(String, PaperMarketPrice)> = Vec::with_capacity(symbols.len());
    for symbol in &symbols {
        if let Some(since_ms) = replay_since_ms {
            // Only replay if the gap is at least one minute — saves an API
            // call on rapid back-to-back reconciles.
            if now_ms - since_ms > 60_000 {
                let candles = fetch_replay_candles(ctx, symbol, since_ms, now_ms).await;
                let replay_fills = replay_candle_triggers(&mut state, symbol, &candles);
                fills.extend(replay_fills);
            }
        }
        let market = fetch_paper_price(ctx, symbol).await?;
        let trigger_fills = refresh_and_evaluate(&mut state, symbol, market.mark);
        fills.extend(trigger_fills);
        markets.push((symbol.clone(), market));
    }

    let market_for = |sym: &str| markets.iter().find(|(s, _)| s == sym).map(|(_, m)| *m);

    let mut remaining = Vec::new();
    for order in std::mem::take(&mut state.orders) {
        if !symbol_match(&order.symbol) {
            remaining.push(order);
            continue;
        }
        let market = market_for(&order.symbol).expect("market cached for matched symbol");
        let side = parse_side(&order.side)?;
        if limit_crosses(side, order.price, market) {
            let fill = apply_fill(
                &mut state,
                Some(order.order_id.clone()),
                &order.symbol,
                side,
                PaperOrderType::Limit,
                order.price,
                ResolvedPaperSize {
                    lots: order.size_lots,
                    tokens: order.size_tokens,
                },
            );
            fills.push(fill);
            // Re-parent any TP/SL that was attached to this resting limit —
            // now that the position exists, the trigger is active.
            for trig in state.triggers.iter_mut() {
                if trig.parent_order_id.as_deref() == Some(order.order_id.as_str()) {
                    trig.parent_order_id = None;
                }
            }
            // Position changed — re-evaluate triggers on this symbol.
            let trigger_fills = refresh_and_evaluate(&mut state, &order.symbol, market.mark);
            fills.extend(trigger_fills);
        } else {
            remaining.push(order);
        }
    }

    state.orders = remaining;
    state.touch();
    save_state(state_path, &state)?;
    let result = PaperReconcileResult {
        mode: "paper".to_string(),
        checked_orders,
        fills,
        state: status_from_state(state_path, &state),
    };
    Ok((result, state))
}

pub fn cancel(
    state_path: &Path,
    mut state: PaperState,
    order_id: &str,
) -> Result<(PaperCancelResult, PaperState), VulcanError> {
    let mut removed_ids = Vec::new();

    let order_matched = state.orders.iter().any(|o| o.order_id == order_id);
    if order_matched {
        state.orders.retain(|o| o.order_id != order_id);
        removed_ids.push(order_id.to_string());
        // Cascade: pending TP/SL triggers attached to this order.
        state.triggers.retain(|t| {
            if t.parent_order_id.as_deref() == Some(order_id) {
                removed_ids.push(t.trigger_id.clone());
                false
            } else {
                true
            }
        });
    } else {
        let before = state.triggers.len();
        state.triggers.retain(|t| t.trigger_id != order_id);
        if state.triggers.len() < before {
            removed_ids.push(order_id.to_string());
        }
    }

    if removed_ids.is_empty() {
        return Err(VulcanError::validation(
            "PAPER_ORDER_NOT_FOUND",
            format!("No paper order or trigger found with id {}", order_id),
        ));
    }
    let cancelled = removed_ids.len();
    state.touch();
    save_state(state_path, &state)?;
    let result = PaperCancelResult {
        mode: "paper".to_string(),
        cancelled,
        order_ids: removed_ids,
        state: status_from_state(state_path, &state),
    };
    Ok((result, state))
}

pub fn cancel_all(
    state_path: &Path,
    mut state: PaperState,
    symbol: Option<&str>,
) -> Result<(PaperCancelResult, PaperState), VulcanError> {
    let mut order_ids = Vec::new();
    state.orders.retain(|o| {
        let should_cancel = symbol.map(|s| s == o.symbol).unwrap_or(true);
        if should_cancel {
            order_ids.push(o.order_id.clone());
        }
        !should_cancel
    });
    // Cascade: pending TP/SL triggers attached to any cancelled order.
    let cancelled_set: std::collections::HashSet<&str> =
        order_ids.iter().map(|s| s.as_str()).collect();
    let mut cascade_ids = Vec::new();
    state.triggers.retain(|t| {
        let parented = t
            .parent_order_id
            .as_deref()
            .map(|p| cancelled_set.contains(p))
            .unwrap_or(false);
        if parented {
            cascade_ids.push(t.trigger_id.clone());
            false
        } else {
            true
        }
    });
    let mut all_ids = order_ids;
    all_ids.extend(cascade_ids);
    let cancelled = all_ids.len();
    state.touch();
    save_state(state_path, &state)?;
    let result = PaperCancelResult {
        mode: "paper".to_string(),
        cancelled,
        order_ids: all_ids,
        state: status_from_state(state_path, &state),
    };
    Ok((result, state))
}

pub async fn set_tpsl(
    ctx: &AppContext,
    state_path: &Path,
    mut state: PaperState,
    symbol: &str,
    tp_inputs: Vec<TpSlInput>,
    sl_inputs: Vec<TpSlInput>,
) -> Result<(PaperSetTpSlResult, PaperState), VulcanError> {
    if tp_inputs.is_empty() && sl_inputs.is_empty() {
        return Err(VulcanError::validation(
            "NO_TP_SL",
            "Specify at least one TP or SL level",
        ));
    }

    let symbol_upper = symbol.to_ascii_uppercase();
    let position = state
        .positions
        .iter()
        .find(|p| p.symbol.to_ascii_uppercase() == symbol_upper)
        .cloned()
        .ok_or_else(|| {
            VulcanError::validation(
                "NO_POSITION",
                format!(
                    "No open paper position for '{}'. TP/SL requires an existing position.",
                    symbol
                ),
            )
        })?;

    let position_lots = position.size_lots;
    let position_side = position.side.clone();
    let entry_price = position.entry_price;

    let resolved_tp =
        resolve_paper_tpsl_levels(ctx, &symbol_upper, &tp_inputs, position_lots).await?;
    let resolved_sl =
        resolve_paper_tpsl_levels(ctx, &symbol_upper, &sl_inputs, position_lots).await?;

    for level in &resolved_tp {
        validate_tpsl_direction(
            PaperTriggerKind::TakeProfit,
            &position_side,
            entry_price,
            level.price,
        )?;
    }
    for level in &resolved_sl {
        validate_tpsl_direction(
            PaperTriggerKind::StopLoss,
            &position_side,
            entry_price,
            level.price,
        )?;
    }

    let total_tp: u64 = resolved_tp.iter().map(|l| l.size_lots).sum();
    let total_sl: u64 = resolved_sl.iter().map(|l| l.size_lots).sum();
    if total_tp > position_lots {
        return Err(VulcanError::validation(
            "TP_SIZE_EXCEEDS_POSITION",
            format!(
                "Sum of TP level sizes ({} lots) exceeds position size ({} lots)",
                total_tp, position_lots
            ),
        ));
    }
    if total_sl > position_lots {
        return Err(VulcanError::validation(
            "SL_SIZE_EXCEEDS_POSITION",
            format!(
                "Sum of SL level sizes ({} lots) exceeds position size ({} lots)",
                total_sl, position_lots
            ),
        ));
    }

    let now = Utc::now().to_rfc3339();
    let mut tp_out = Vec::with_capacity(resolved_tp.len());
    let mut sl_out = Vec::with_capacity(resolved_sl.len());
    for level in &resolved_tp {
        let trigger_id = state.next_trigger_id();
        tp_out.push(PaperTpSlLevel {
            trigger_id: trigger_id.clone(),
            price: level.price,
            size_tokens: level.size_tokens,
            size_lots: level.size_lots,
        });
        state.triggers.push(PaperTrigger {
            trigger_id,
            symbol: symbol_upper.clone(),
            kind: PaperTriggerKind::TakeProfit,
            position_side: position_side.clone(),
            trigger_price: level.price,
            size_tokens: level.size_tokens,
            size_lots: level.size_lots,
            parent_order_id: None,
            created_at: now.clone(),
        });
    }
    for level in &resolved_sl {
        let trigger_id = state.next_trigger_id();
        sl_out.push(PaperTpSlLevel {
            trigger_id: trigger_id.clone(),
            price: level.price,
            size_tokens: level.size_tokens,
            size_lots: level.size_lots,
        });
        state.triggers.push(PaperTrigger {
            trigger_id,
            symbol: symbol_upper.clone(),
            kind: PaperTriggerKind::StopLoss,
            position_side: position_side.clone(),
            trigger_price: level.price,
            size_tokens: level.size_tokens,
            size_lots: level.size_lots,
            parent_order_id: None,
            created_at: now.clone(),
        });
    }

    state.touch();
    save_state(state_path, &state)?;

    let active_triggers: Vec<PaperTrigger> = state
        .triggers
        .iter()
        .filter(|t| t.symbol == symbol_upper)
        .cloned()
        .collect();

    let result = PaperSetTpSlResult {
        mode: "paper".to_string(),
        symbol: symbol_upper,
        position_side,
        tp_levels: tp_out,
        sl_levels: sl_out,
        triggers: active_triggers,
        state: status_from_state(state_path, &state),
    };
    Ok((result, state))
}

pub fn cancel_tpsl(
    state_path: &Path,
    mut state: PaperState,
    symbol: &str,
    cancel_tp: bool,
    cancel_sl: bool,
) -> Result<(PaperCancelTpSlResult, PaperState), VulcanError> {
    if !cancel_tp && !cancel_sl {
        return Err(VulcanError::validation(
            "TPSL_CANCEL_NOTHING",
            "Set at least one of --tp or --sl to cancel",
        ));
    }
    let symbol_upper = symbol.to_ascii_uppercase();
    let mut trigger_ids = Vec::new();
    state.triggers.retain(|t| {
        if t.symbol != symbol_upper {
            return true;
        }
        let matches = match t.kind {
            PaperTriggerKind::TakeProfit => cancel_tp,
            PaperTriggerKind::StopLoss => cancel_sl,
        };
        if matches {
            trigger_ids.push(t.trigger_id.clone());
            false
        } else {
            true
        }
    });

    let cancelled = trigger_ids.len();
    state.touch();
    save_state(state_path, &state)?;
    let result = PaperCancelTpSlResult {
        mode: "paper".to_string(),
        symbol: symbol_upper,
        cancelled,
        trigger_ids,
        state: status_from_state(state_path, &state),
    };
    Ok((result, state))
}

pub fn triggers(state: &PaperState, symbol_filter: Option<&str>) -> PaperTriggersResult {
    let filter_upper = symbol_filter.map(|s| s.to_ascii_uppercase());
    let triggers: Vec<PaperTrigger> = state
        .triggers
        .iter()
        .filter(|t| {
            filter_upper
                .as_deref()
                .map(|f| t.symbol == f)
                .unwrap_or(true)
        })
        .cloned()
        .collect();
    PaperTriggersResult {
        mode: "paper".to_string(),
        triggers,
    }
}

#[derive(Debug, Clone, Copy)]
struct ResolvedPaperTpSlLevel {
    price: f64,
    size_tokens: f64,
    size_lots: u64,
}

async fn resolve_paper_tpsl_levels(
    ctx: &AppContext,
    symbol: &str,
    inputs: &[TpSlInput],
    position_lots: u64,
) -> Result<Vec<ResolvedPaperTpSlLevel>, VulcanError> {
    if inputs.is_empty() {
        return Ok(Vec::new());
    }
    let full_count = inputs
        .iter()
        .filter(|i| matches!(i.size, TpSlSize::Full))
        .count();
    if inputs.len() > 1 && full_count > 0 {
        return Err(VulcanError::validation(
            "TPSL_FULL_WITH_MULTI_LEVEL",
            "When using multiple TP/SL levels on a side, every level must specify an explicit size",
        ));
    }

    let info = execute_info_inner(ctx, symbol).await?;
    let multiplier = 10f64.powi(info.base_lots_decimals as i32);

    let mut out = Vec::with_capacity(inputs.len());
    for inp in inputs {
        if !inp.price.is_finite() || inp.price <= 0.0 {
            return Err(VulcanError::validation(
                "TPSL_PRICE_INVALID",
                format!("TP/SL price must be positive, got {}", inp.price),
            ));
        }
        let (lots, tokens) = match inp.size {
            TpSlSize::Full => (position_lots, position_lots as f64 / multiplier),
            TpSlSize::Lots(n) => (n, n as f64 / multiplier),
            TpSlSize::Tokens(t) => {
                if !t.is_finite() || t <= 0.0 {
                    return Err(VulcanError::validation(
                        "TPSL_SIZE_INVALID",
                        format!("TP/SL token size must be positive, got {}", t),
                    ));
                }
                let lots = (t * multiplier).round() as u64;
                (lots, lots as f64 / multiplier)
            }
        };
        if lots == 0 {
            return Err(VulcanError::validation(
                "TPSL_SIZE_TOO_SMALL",
                format!("Resolved TP/SL size is 0 base lots for price {}", inp.price),
            ));
        }
        out.push(ResolvedPaperTpSlLevel {
            price: inp.price,
            size_tokens: tokens,
            size_lots: lots,
        });
    }
    Ok(out)
}

/// Validate and attach order-time TP/SL to `state.triggers`. Pushes one
/// trigger per provided side and returns them in `[tp, sl]` order.
///
/// `reference_price` is what the validator compares against — the fill price
/// for filled orders, the limit price for resting limits. `parent_order_id`
/// is `None` for an immediately-filled entry (trigger is active) or
/// `Some(order_id)` for a still-resting limit (trigger is pending and will be
/// re-parented on fill).
#[allow(clippy::too_many_arguments)]
fn build_order_time_triggers(
    state: &mut PaperState,
    symbol: &str,
    side: PaperSide,
    tp: Option<f64>,
    sl: Option<f64>,
    reference_price: f64,
    size_lots: u64,
    size_tokens: f64,
    parent_order_id: Option<String>,
    now: &str,
) -> Result<Vec<PaperTrigger>, VulcanError> {
    if tp.is_none() && sl.is_none() {
        return Ok(Vec::new());
    }

    let target_side = match side {
        PaperSide::Buy => "long",
        PaperSide::Sell => "short",
    };

    // Reduce-only rejection: order-time TP/SL on an order against an
    // opposite-side existing position would close or flip — live's bracket
    // ticket fails this case. Make the user explicit: place the order, then
    // attach TP/SL with `paper set-tpsl`.
    if let Some(existing) = state
        .positions
        .iter()
        .find(|p| p.symbol.eq_ignore_ascii_case(symbol))
    {
        if existing.side != target_side {
            return Err(VulcanError::validation(
                "TPSL_ORDER_REDUCES",
                format!(
                    "Order-time TP/SL only works when opening or extending a position. This {} on {} would reduce/flip an existing {} position. Place the order first, then use `paper set-tpsl`.",
                    side.as_str(),
                    symbol,
                    existing.side
                ),
            ));
        }
    }

    if let Some(tp_price) = tp {
        validate_tpsl_direction(
            PaperTriggerKind::TakeProfit,
            target_side,
            reference_price,
            tp_price,
        )?;
    }
    if let Some(sl_price) = sl {
        validate_tpsl_direction(
            PaperTriggerKind::StopLoss,
            target_side,
            reference_price,
            sl_price,
        )?;
    }

    let mut out = Vec::new();
    if let Some(tp_price) = tp {
        let trigger = PaperTrigger {
            trigger_id: state.next_trigger_id(),
            symbol: symbol.to_string(),
            kind: PaperTriggerKind::TakeProfit,
            position_side: target_side.to_string(),
            trigger_price: tp_price,
            size_tokens,
            size_lots,
            parent_order_id: parent_order_id.clone(),
            created_at: now.to_string(),
        };
        state.triggers.push(trigger.clone());
        out.push(trigger);
    }
    if let Some(sl_price) = sl {
        let trigger = PaperTrigger {
            trigger_id: state.next_trigger_id(),
            symbol: symbol.to_string(),
            kind: PaperTriggerKind::StopLoss,
            position_side: target_side.to_string(),
            trigger_price: sl_price,
            size_tokens,
            size_lots,
            parent_order_id,
            created_at: now.to_string(),
        };
        state.triggers.push(trigger.clone());
        out.push(trigger);
    }

    Ok(out)
}

fn validate_tpsl_direction(
    kind: PaperTriggerKind,
    position_side: &str,
    entry_price: f64,
    trigger_price: f64,
) -> Result<(), VulcanError> {
    let ok = match (kind, position_side) {
        (PaperTriggerKind::TakeProfit, "long") => trigger_price > entry_price,
        (PaperTriggerKind::StopLoss, "long") => trigger_price < entry_price,
        (PaperTriggerKind::TakeProfit, "short") => trigger_price < entry_price,
        (PaperTriggerKind::StopLoss, "short") => trigger_price > entry_price,
        _ => true,
    };
    if !ok {
        let (which, relation) = match (kind, position_side) {
            (PaperTriggerKind::TakeProfit, "long") => ("take-profit", "above"),
            (PaperTriggerKind::StopLoss, "long") => ("stop-loss", "below"),
            (PaperTriggerKind::TakeProfit, "short") => ("take-profit", "below"),
            (PaperTriggerKind::StopLoss, "short") => ("stop-loss", "above"),
            _ => ("trigger", "different from"),
        };
        return Err(VulcanError::validation(
            "TPSL_WRONG_DIRECTION",
            format!(
                "{} {} must be {} entry price {} for {} position",
                which, trigger_price, relation, entry_price, position_side
            ),
        ));
    }
    Ok(())
}

pub fn positions(state: &PaperState) -> PaperPositionsResult {
    let unrealized_pnl = state
        .positions
        .iter()
        .map(|p| p.unrealized_pnl)
        .sum::<f64>();
    let equity = state.balance + unrealized_pnl;
    let position_notional_usdc = position_notional_usdc(&state.positions);

    let project = |p: &PaperPosition, kind: PaperTriggerKind| -> Vec<PaperPositionTpSl> {
        state
            .triggers
            .iter()
            .filter(|t| t.symbol == p.symbol && t.kind == kind && t.parent_order_id.is_none())
            .map(|t| PaperPositionTpSl {
                trigger_id: t.trigger_id.clone(),
                price: t.trigger_price,
                size_tokens: t.size_tokens,
            })
            .collect()
    };
    let positions: Vec<PaperPosition> = state
        .positions
        .iter()
        .map(|p| PaperPosition {
            tp_levels: project(p, PaperTriggerKind::TakeProfit),
            sl_levels: project(p, PaperTriggerKind::StopLoss),
            ..p.clone()
        })
        .collect();

    PaperPositionsResult {
        mode: "paper".to_string(),
        currency: state.currency.clone(),
        equity,
        position_notional_usdc,
        exposure_ratio: exposure_ratio(position_notional_usdc, equity),
        positions,
    }
}

pub fn orders(state: &PaperState) -> PaperOrdersResult {
    PaperOrdersResult {
        mode: "paper".to_string(),
        orders: state.orders.clone(),
    }
}

pub fn fills(state: &PaperState, limit: usize) -> PaperFillsResult {
    let keep = limit.min(state.fills.len());
    PaperFillsResult {
        mode: "paper".to_string(),
        fills: state.fills[state.fills.len().saturating_sub(keep)..].to_vec(),
    }
}

fn status_from_state(path: &Path, state: &PaperState) -> PaperStatus {
    let unrealized_pnl = state.positions.iter().map(|p| p.unrealized_pnl).sum();
    let realized_pnl = state.fills.iter().map(|f| f.realized_pnl).sum::<f64>();
    let fees_paid = state.fills.iter().map(|f| f.fee).sum();
    let equity = state.balance + unrealized_pnl;
    let position_notional_usdc = position_notional_usdc(&state.positions);
    PaperStatus {
        mode: "paper".to_string(),
        path: path.to_string_lossy().to_string(),
        currency: state.currency.clone(),
        starting_balance: state.starting_balance,
        balance: state.balance,
        equity,
        position_notional_usdc,
        exposure_ratio: exposure_ratio(position_notional_usdc, equity),
        unrealized_pnl,
        realized_pnl,
        fees_paid,
        open_positions: state.positions.len(),
        open_orders: state.orders.len(),
        triggers: state.triggers.len(),
        fills: state.fills.len(),
    }
}

fn position_notional_usdc(positions: &[PaperPosition]) -> f64 {
    positions
        .iter()
        .map(|position| position.size_tokens * position.mark_price)
        .sum()
}

fn exposure_ratio(position_notional_usdc: f64, equity: f64) -> f64 {
    if equity.abs() < f64::EPSILON {
        0.0
    } else {
        position_notional_usdc / equity
    }
}

/// Fetch hourly candles for `symbol` covering the gap between `since` and
/// `until`. Returns `Ok(empty)` if there's nothing to replay or if the API
/// call fails — reconcile should degrade gracefully to "current mark only".
///
/// The lookback is capped at 30 days to avoid asking the API for ancient
/// data; gaps larger than that just lose the missed window beyond the cap.
async fn fetch_replay_candles(
    ctx: &AppContext,
    symbol: &str,
    since_ms: i64,
    until_ms: i64,
) -> Vec<ReplayCandle> {
    if until_ms <= since_ms {
        return Vec::new();
    }
    const MAX_LOOKBACK_MS: i64 = 30 * 24 * 60 * 60 * 1000;
    let effective_since = since_ms.max(until_ms - MAX_LOOKBACK_MS);
    let params = phoenix_rise::CandlesQueryParams::new(symbol, phoenix_rise::Timeframe::Hour1)
        .with_start_time(effective_since)
        .with_end_time(until_ms);
    let Ok(raw) = ctx.http_client.get_candles(params).await else {
        return Vec::new();
    };
    let mut candles: Vec<ReplayCandle> = raw
        .into_iter()
        .map(|c| {
            let close_time = if c.time > 1_000_000_000_000 {
                c.time / 1000
            } else {
                c.time
            };
            ReplayCandle {
                close_time,
                open: c.open,
                high: c.high,
                low: c.low,
                close: c.close,
            }
        })
        .collect();
    candles.sort_by_key(|c| c.close_time);
    candles
}

async fn fetch_paper_price(
    ctx: &AppContext,
    symbol: &str,
) -> Result<PaperMarketPrice, VulcanError> {
    let ticker = execute_ticker_inner(ctx, symbol).await?;
    let book = execute_orderbook_inner(ctx, symbol, 1).await.ok();
    Ok(PaperMarketPrice {
        mark: ticker.mark_price,
        bid: book.as_ref().and_then(|b| b.bids.first()).map(|l| l.price),
        ask: book.as_ref().and_then(|b| b.asks.first()).map(|l| l.price),
    })
}

async fn resolve_size(
    ctx: &AppContext,
    symbol: &str,
    input: PaperSizeInput,
    reference_price: f64,
) -> Result<ResolvedPaperSize, VulcanError> {
    let provided = [
        input.size_lots.is_some(),
        input.tokens.is_some(),
        input.notional_usdc.is_some(),
    ]
    .into_iter()
    .filter(|v| *v)
    .count();
    if provided != 1 {
        return Err(VulcanError::validation(
            "AMBIGUOUS_SIZE",
            "provide exactly one of size, tokens, or notional_usdc",
        ));
    }
    let info = execute_info_inner(ctx, symbol).await?;
    let multiplier = 10f64.powi(info.base_lots_decimals as i32);
    let tokens = if let Some(lots) = input.size_lots {
        lots / multiplier
    } else if let Some(tokens) = input.tokens {
        tokens
    } else {
        input.notional_usdc.unwrap() / reference_price
    };
    if !tokens.is_finite() || tokens <= 0.0 {
        return Err(VulcanError::validation(
            "INVALID_SIZE",
            "paper order size must be positive",
        ));
    }
    let lots = (tokens * multiplier).round() as u64;
    if lots == 0 {
        return Err(VulcanError::validation(
            "SIZE_TOO_SMALL",
            "paper order size rounds to zero base lots",
        ));
    }
    Ok(ResolvedPaperSize {
        lots,
        tokens: lots as f64 / multiplier,
    })
}

fn apply_fill(
    state: &mut PaperState,
    order_id: Option<String>,
    symbol: &str,
    side: PaperSide,
    order_type: PaperOrderType,
    price: f64,
    size: ResolvedPaperSize,
) -> PaperFill {
    let fee = price * size.tokens * state.fee_bps / 10_000.0;
    let realized_pnl = apply_position_fill(state, symbol, side, price, size);
    state.balance += realized_pnl - fee;
    let fill = PaperFill {
        fill_id: state.next_fill_id(),
        order_id,
        symbol: symbol.to_string(),
        side: side.as_str().to_string(),
        order_type: order_type.as_str().to_string(),
        price,
        size_tokens: size.tokens,
        size_lots: size.lots,
        fee,
        realized_pnl,
        timestamp: Utc::now().to_rfc3339(),
    };
    state.fills.push(fill.clone());
    fill
}

fn apply_position_fill(
    state: &mut PaperState,
    symbol: &str,
    side: PaperSide,
    price: f64,
    size: ResolvedPaperSize,
) -> f64 {
    // Net positions in integer lots. Tokens are a derived view: doing the
    // arithmetic in floats accumulates rounding drift across many opposite-
    // side fills, and a non-exact `signed_total.abs() < f64::EPSILON` check
    // misses residues ~1e-10 that nevertheless round to zero lots — leaving
    // a phantom position with `size_lots: 0` but `size_tokens > 0`.
    let signed_new_lots: i128 = match side {
        PaperSide::Buy => size.lots as i128,
        PaperSide::Sell => -(size.lots as i128),
    };
    // tokens-per-lot for this market — invariant across fills on the same
    // symbol (same `base_lots_decimals`), so we use it to project the
    // position's net lots back to a clean token count.
    let tokens_per_lot = if size.lots == 0 {
        0.0
    } else {
        size.tokens / size.lots as f64
    };

    let Some(idx) = state.positions.iter().position(|p| p.symbol == symbol) else {
        state.positions.push(PaperPosition {
            symbol: symbol.to_string(),
            side: if signed_new_lots >= 0 { "long" } else { "short" }.to_string(),
            size_tokens: size.tokens,
            size_lots: size.lots,
            entry_price: price,
            mark_price: price,
            unrealized_pnl: 0.0,
            realized_pnl: 0.0,
            tp_levels: Vec::new(),
            sl_levels: Vec::new(),
        });
        return 0.0;
    };

    let position = &mut state.positions[idx];
    let signed_old_lots: i128 = if position.side == "long" {
        position.size_lots as i128
    } else {
        -(position.size_lots as i128)
    };
    let opposite_sign = signed_old_lots.signum() != signed_new_lots.signum()
        && signed_old_lots != 0
        && signed_new_lots != 0;
    let closed_lots: u64 = if opposite_sign {
        signed_old_lots.abs().min(signed_new_lots.abs()) as u64
    } else {
        0
    };
    let closed_tokens = closed_lots as f64 * tokens_per_lot;
    let realized = if closed_lots > 0 {
        if signed_old_lots > 0 {
            (price - position.entry_price) * closed_tokens
        } else {
            (position.entry_price - price) * closed_tokens
        }
    } else {
        0.0
    };
    let signed_total_lots = signed_old_lots + signed_new_lots;
    position.realized_pnl += realized;

    if signed_total_lots == 0 {
        state.positions.remove(idx);
        return realized;
    }

    if signed_old_lots.signum() == signed_new_lots.signum() {
        let old_abs = signed_old_lots.unsigned_abs() as f64;
        let new_abs = signed_new_lots.unsigned_abs() as f64;
        position.entry_price =
            (old_abs * position.entry_price + new_abs * price) / (old_abs + new_abs);
    } else if signed_total_lots.signum() != signed_old_lots.signum() {
        position.entry_price = price;
    }

    let total_abs_lots = signed_total_lots.unsigned_abs() as u64;
    position.side = if signed_total_lots > 0 { "long" } else { "short" }.to_string();
    position.size_lots = total_abs_lots;
    position.size_tokens = total_abs_lots as f64 * tokens_per_lot;
    position.mark_price = price;
    position.unrealized_pnl = unrealized_for(position, price);
    realized
}

fn refresh_position_marks(state: &mut PaperState, symbol: &str, mark: f64) {
    for position in state.positions.iter_mut().filter(|p| p.symbol == symbol) {
        position.mark_price = mark;
        position.unrealized_pnl = unrealized_for(position, mark);
    }
}

/// Refresh marks for `symbol` and fire any TP/SL triggers whose price the mark
/// has crossed. Bumps `last_evaluated_at` so a subsequent reconcile knows the
/// time window it needs to replay. Caller is responsible for persisting state.
fn refresh_and_evaluate(state: &mut PaperState, symbol: &str, mark: f64) -> Vec<PaperFill> {
    refresh_position_marks(state, symbol, mark);
    let fills = evaluate_triggers(state, symbol, mark);
    state.last_evaluated_at = Some(Utc::now().to_rfc3339());
    fills
}

/// Fire any active triggers on `symbol` that have been crossed by `mark`.
/// Fills are applied at the trigger price (paper always fills at the
/// configured trigger, never at the live bid/ask), capped by the remaining
/// position. When a trigger closes the position, any remaining **active**
/// triggers on the same symbol are purged as orphans; pending triggers
/// (attached to still-resting limit orders) survive and arm when their
/// parent order eventually fills.
fn evaluate_triggers(state: &mut PaperState, symbol: &str, mark: f64) -> Vec<PaperFill> {
    if !mark.is_finite() || mark <= 0.0 {
        return Vec::new();
    }
    let mut fills = Vec::new();
    let candidate_ids: Vec<String> = state
        .triggers
        .iter()
        .filter(|t| t.symbol == symbol && t.parent_order_id.is_none())
        .map(|t| t.trigger_id.clone())
        .collect();

    for trigger_id in candidate_ids {
        let Some(idx) = state
            .triggers
            .iter()
            .position(|t| t.trigger_id == trigger_id)
        else {
            continue;
        };
        let trigger = state.triggers[idx].clone();
        if !trigger_crossed(&trigger, mark) {
            continue;
        }

        let Some(position) = state.positions.iter().find(|p| p.symbol == symbol) else {
            state.triggers.remove(idx);
            continue;
        };

        let close_lots = trigger.size_lots.min(position.size_lots);
        if close_lots == 0 {
            state.triggers.remove(idx);
            continue;
        }
        let close_tokens = trigger.size_tokens.min(position.size_tokens);
        let close_side = match position.side.as_str() {
            "long" => PaperSide::Sell,
            "short" => PaperSide::Buy,
            _ => continue,
        };

        state.triggers.remove(idx);
        let fill = apply_fill(
            state,
            Some(trigger.trigger_id.clone()),
            symbol,
            close_side,
            PaperOrderType::Limit,
            trigger.trigger_price,
            ResolvedPaperSize {
                lots: close_lots,
                tokens: close_tokens,
            },
        );
        fills.push(fill);

        if !state.positions.iter().any(|p| p.symbol == symbol) {
            state
                .triggers
                .retain(|t| t.symbol != symbol || t.parent_order_id.is_some());
            break;
        }
    }

    fills
}

/// One historical price bar used for trigger replay. Times are seconds since
/// the Unix epoch (matches Phoenix's `ApiCandle.time`).
#[derive(Debug, Clone, Copy)]
pub(crate) struct ReplayCandle {
    pub close_time: i64,
    pub open: f64,
    pub high: f64,
    pub low: f64,
    pub close: f64,
}

/// Walk a chronologically-ordered slice of candles and fire every active
/// trigger that the candle's `[low, high]` range covered, provided the
/// trigger existed at or before the candle's close. Mirrors the live keeper
/// model: a trigger fires when the market visits its price, not when paper
/// next happens to look.
///
/// Within a single candle we use the bullish/bearish heuristic to decide
/// which of several crossable triggers fires first: a bullish candle
/// (close greater than open) is more likely to have visited high prices
/// before lows, so we process triggers in descending price order. A bearish
/// candle uses ascending order. Approximate, but it's the best HLOC alone
/// can offer.
fn replay_candle_triggers(
    state: &mut PaperState,
    symbol: &str,
    candles: &[ReplayCandle],
) -> Vec<PaperFill> {
    let mut fills = Vec::new();
    for candle in candles {
        let crossable_ids: Vec<String> = {
            let mut v: Vec<(String, f64)> = state
                .triggers
                .iter()
                .filter(|t| {
                    t.symbol == symbol
                        && t.parent_order_id.is_none()
                        && created_at_seconds(t).is_none_or(|ts| ts <= candle.close_time)
                        && t.trigger_price >= candle.low
                        && t.trigger_price <= candle.high
                })
                .map(|t| (t.trigger_id.clone(), t.trigger_price))
                .collect();
            // Bullish: highs likely visited first → process descending.
            // Bearish: lows first → ascending. Use sort_by for stable tie-break.
            let bullish = candle.close >= candle.open;
            v.sort_by(|a, b| {
                if bullish {
                    b.1.partial_cmp(&a.1).unwrap_or(std::cmp::Ordering::Equal)
                } else {
                    a.1.partial_cmp(&b.1).unwrap_or(std::cmp::Ordering::Equal)
                }
            });
            v.into_iter().map(|(id, _)| id).collect()
        };

        for trigger_id in crossable_ids {
            let Some(idx) = state
                .triggers
                .iter()
                .position(|t| t.trigger_id == trigger_id)
            else {
                continue;
            };
            let trigger = state.triggers[idx].clone();
            let Some(position) = state.positions.iter().find(|p| p.symbol == symbol) else {
                state.triggers.remove(idx);
                continue;
            };
            let close_lots = trigger.size_lots.min(position.size_lots);
            if close_lots == 0 {
                state.triggers.remove(idx);
                continue;
            }
            let close_tokens = trigger.size_tokens.min(position.size_tokens);
            let close_side = match position.side.as_str() {
                "long" => PaperSide::Sell,
                "short" => PaperSide::Buy,
                _ => continue,
            };
            state.triggers.remove(idx);
            let mut fill = apply_fill(
                state,
                Some(trigger.trigger_id.clone()),
                symbol,
                close_side,
                PaperOrderType::Limit,
                trigger.trigger_price,
                ResolvedPaperSize {
                    lots: close_lots,
                    tokens: close_tokens,
                },
            );
            // Stamp the fill with the candle's close time, not "now", so the
            // history accurately reflects when the trigger crossed.
            if let Some(ts) = chrono::DateTime::from_timestamp(candle.close_time, 0) {
                fill.timestamp = ts.to_rfc3339();
                if let Some(last) = state.fills.last_mut() {
                    last.timestamp = fill.timestamp.clone();
                }
            }
            fills.push(fill);

            if !state.positions.iter().any(|p| p.symbol == symbol) {
                state
                    .triggers
                    .retain(|t| t.symbol != symbol || t.parent_order_id.is_some());
                break;
            }
        }
    }
    fills
}

/// Parse a trigger's `created_at` (RFC3339) to a Unix-second timestamp. Used
/// to filter replay so a trigger never fires against candles that closed
/// before it existed.
fn created_at_seconds(trigger: &PaperTrigger) -> Option<i64> {
    chrono::DateTime::parse_from_rfc3339(&trigger.created_at)
        .ok()
        .map(|dt| dt.timestamp())
}

fn trigger_crossed(trigger: &PaperTrigger, mark: f64) -> bool {
    match (trigger.kind, trigger.position_side.as_str()) {
        (PaperTriggerKind::TakeProfit, "long") => mark >= trigger.trigger_price,
        (PaperTriggerKind::StopLoss, "long") => mark <= trigger.trigger_price,
        (PaperTriggerKind::TakeProfit, "short") => mark <= trigger.trigger_price,
        (PaperTriggerKind::StopLoss, "short") => mark >= trigger.trigger_price,
        _ => false,
    }
}

fn unrealized_for(position: &PaperPosition, mark: f64) -> f64 {
    if position.side == "long" {
        (mark - position.entry_price) * position.size_tokens
    } else {
        (position.entry_price - mark) * position.size_tokens
    }
}

fn limit_crosses(side: PaperSide, price: f64, market: PaperMarketPrice) -> bool {
    match side {
        PaperSide::Buy => market.ask.unwrap_or(market.mark) <= price,
        PaperSide::Sell => market.bid.unwrap_or(market.mark) >= price,
    }
}

fn parse_side(side: &str) -> Result<PaperSide, VulcanError> {
    match side {
        "buy" => Ok(PaperSide::Buy),
        "sell" => Ok(PaperSide::Sell),
        _ => Err(VulcanError::validation(
            "INVALID_SIDE",
            format!("invalid paper side {}", side),
        )),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn paper_position_nets_and_realizes_pnl() {
        let mut state = PaperState::new(10_000.0, "USDC".to_string(), 0.0);
        apply_fill(
            &mut state,
            None,
            "SOL",
            PaperSide::Buy,
            PaperOrderType::Market,
            100.0,
            ResolvedPaperSize {
                lots: 100,
                tokens: 1.0,
            },
        );
        apply_fill(
            &mut state,
            None,
            "SOL",
            PaperSide::Sell,
            PaperOrderType::Market,
            110.0,
            ResolvedPaperSize {
                lots: 50,
                tokens: 0.5,
            },
        );
        assert_eq!(state.positions.len(), 1);
        assert_eq!(state.positions[0].side, "long");
        assert_eq!(state.positions[0].size_tokens, 0.5);
        assert!((state.balance - 10_005.0).abs() < 0.0001);
        assert!((state.fills[1].realized_pnl - 5.0).abs() < 0.0001);
    }

    #[test]
    fn legacy_paper_state_json_loads_with_default_triggers() {
        // A state file written before the TP/SL feature has no `triggers` or
        // `next_trigger_id` fields. It must still deserialize cleanly.
        let legacy = r#"{
            "mode": "paper",
            "currency": "USDC",
            "starting_balance": 10000.0,
            "balance": 10000.0,
            "fee_bps": 5.0,
            "created_at": "2025-01-01T00:00:00Z",
            "updated_at": "2025-01-01T00:00:00Z",
            "next_order_id": 1,
            "next_fill_id": 1,
            "positions": [],
            "orders": [],
            "fills": []
        }"#;
        let state: PaperState = serde_json::from_str(legacy).expect("legacy state must load");
        assert!(state.triggers.is_empty());
        assert_eq!(state.next_trigger_id, 1);
    }

    fn seed_long_position(state: &mut PaperState) {
        apply_fill(
            state,
            None,
            "SOL",
            PaperSide::Buy,
            PaperOrderType::Market,
            100.0,
            ResolvedPaperSize {
                lots: 100,
                tokens: 1.0,
            },
        );
    }

    fn push_trigger(state: &mut PaperState, kind: PaperTriggerKind, price: f64) -> String {
        let trigger_id = state.next_trigger_id();
        state.triggers.push(PaperTrigger {
            trigger_id: trigger_id.clone(),
            symbol: "SOL".to_string(),
            kind,
            position_side: "long".to_string(),
            trigger_price: price,
            size_tokens: 0.5,
            size_lots: 50,
            parent_order_id: None,
            created_at: "2023-01-01T00:00:00Z".to_string(),
        });
        trigger_id
    }

    #[test]
    fn cancel_tpsl_removes_only_matching_kind() {
        let tmp = tempfile::tempdir().unwrap();
        let path = tmp.path().join("paper-state.json");
        let mut state = PaperState::new(10_000.0, "USDC".to_string(), 0.0);
        seed_long_position(&mut state);
        let tp_id = push_trigger(&mut state, PaperTriggerKind::TakeProfit, 110.0);
        let sl_id = push_trigger(&mut state, PaperTriggerKind::StopLoss, 90.0);
        save_state(&path, &state).unwrap();

        let (result, state_after) = cancel_tpsl(&path, state, "sol", true, false).unwrap();
        assert_eq!(result.cancelled, 1);
        assert_eq!(result.trigger_ids, vec![tp_id]);
        assert_eq!(state_after.triggers.len(), 1);
        assert_eq!(state_after.triggers[0].trigger_id, sl_id);
    }

    #[test]
    fn cancel_tpsl_requires_at_least_one_side() {
        let tmp = tempfile::tempdir().unwrap();
        let path = tmp.path().join("paper-state.json");
        let state = PaperState::new(10_000.0, "USDC".to_string(), 0.0);
        let err = cancel_tpsl(&path, state, "SOL", false, false).unwrap_err();
        assert!(err.to_string().contains("at least one"));
    }

    #[test]
    fn triggers_filters_by_symbol() {
        let mut state = PaperState::new(10_000.0, "USDC".to_string(), 0.0);
        seed_long_position(&mut state);
        push_trigger(&mut state, PaperTriggerKind::TakeProfit, 110.0);
        state.triggers.push(PaperTrigger {
            trigger_id: "paper-trigger-99".to_string(),
            symbol: "ETH".to_string(),
            kind: PaperTriggerKind::TakeProfit,
            position_side: "long".to_string(),
            trigger_price: 3500.0,
            size_tokens: 1.0,
            size_lots: 100,
            parent_order_id: None,
            created_at: "2023-01-01T00:00:00Z".to_string(),
        });
        let all = triggers(&state, None);
        assert_eq!(all.triggers.len(), 2);
        let sol = triggers(&state, Some("sol"));
        assert_eq!(sol.triggers.len(), 1);
        assert_eq!(sol.triggers[0].symbol, "SOL");
    }

    #[test]
    fn cancel_extends_to_trigger_ids() {
        let tmp = tempfile::tempdir().unwrap();
        let path = tmp.path().join("paper-state.json");
        let mut state = PaperState::new(10_000.0, "USDC".to_string(), 0.0);
        seed_long_position(&mut state);
        let tp_id = push_trigger(&mut state, PaperTriggerKind::TakeProfit, 110.0);
        save_state(&path, &state).unwrap();

        let (result, state_after) = cancel(&path, state, &tp_id).unwrap();
        assert_eq!(result.cancelled, 1);
        assert!(state_after.triggers.is_empty());
    }

    #[test]
    fn direction_validation_rejects_tp_below_entry_on_long() {
        let err =
            validate_tpsl_direction(PaperTriggerKind::TakeProfit, "long", 100.0, 90.0).unwrap_err();
        assert!(err.to_string().contains("above"));
    }

    #[test]
    fn direction_validation_rejects_sl_above_entry_on_long() {
        let err =
            validate_tpsl_direction(PaperTriggerKind::StopLoss, "long", 100.0, 110.0).unwrap_err();
        assert!(err.to_string().contains("below"));
    }

    #[test]
    fn direction_validation_short_position_mirrors_long() {
        validate_tpsl_direction(PaperTriggerKind::TakeProfit, "short", 100.0, 90.0).unwrap();
        validate_tpsl_direction(PaperTriggerKind::StopLoss, "short", 100.0, 110.0).unwrap();
        assert!(
            validate_tpsl_direction(PaperTriggerKind::TakeProfit, "short", 100.0, 110.0).is_err()
        );
        assert!(validate_tpsl_direction(PaperTriggerKind::StopLoss, "short", 100.0, 90.0).is_err());
    }

    fn seed_short_position(state: &mut PaperState) {
        apply_fill(
            state,
            None,
            "SOL",
            PaperSide::Sell,
            PaperOrderType::Market,
            100.0,
            ResolvedPaperSize {
                lots: 100,
                tokens: 1.0,
            },
        );
    }

    #[test]
    fn long_tp_fires_when_mark_crosses_above() {
        let mut state = PaperState::new(10_000.0, "USDC".to_string(), 0.0);
        seed_long_position(&mut state);
        let tp_id = push_trigger(&mut state, PaperTriggerKind::TakeProfit, 110.0);

        let fills = evaluate_triggers(&mut state, "SOL", 109.99);
        assert!(fills.is_empty(), "TP must not fire below trigger price");
        assert_eq!(state.triggers.len(), 1);

        let fills = evaluate_triggers(&mut state, "SOL", 110.0);
        assert_eq!(fills.len(), 1);
        assert_eq!(fills[0].order_id.as_deref(), Some(tp_id.as_str()));
        assert!((fills[0].price - 110.0).abs() < f64::EPSILON);
        assert!(state.triggers.is_empty());
    }

    #[test]
    fn long_sl_fires_when_mark_crosses_below() {
        let mut state = PaperState::new(10_000.0, "USDC".to_string(), 0.0);
        seed_long_position(&mut state);
        push_trigger(&mut state, PaperTriggerKind::StopLoss, 90.0);

        assert!(evaluate_triggers(&mut state, "SOL", 90.01).is_empty());
        let fills = evaluate_triggers(&mut state, "SOL", 90.0);
        assert_eq!(fills.len(), 1);
        assert!((fills[0].realized_pnl - (-5.0)).abs() < 0.0001); // 0.5 SOL × (90-100)
    }

    #[test]
    fn short_triggers_use_inverted_direction() {
        let mut state = PaperState::new(10_000.0, "USDC".to_string(), 0.0);
        seed_short_position(&mut state);
        state.triggers.push(PaperTrigger {
            trigger_id: "t-tp".to_string(),
            symbol: "SOL".to_string(),
            kind: PaperTriggerKind::TakeProfit,
            position_side: "short".to_string(),
            trigger_price: 90.0,
            size_tokens: 0.5,
            size_lots: 50,
            parent_order_id: None,
            created_at: "2023-01-01T00:00:00Z".to_string(),
        });
        state.triggers.push(PaperTrigger {
            trigger_id: "t-sl".to_string(),
            symbol: "SOL".to_string(),
            kind: PaperTriggerKind::StopLoss,
            position_side: "short".to_string(),
            trigger_price: 110.0,
            size_tokens: 0.5,
            size_lots: 50,
            parent_order_id: None,
            created_at: "2023-01-01T00:00:00Z".to_string(),
        });

        let fills_tp = evaluate_triggers(&mut state, "SOL", 89.5);
        assert_eq!(fills_tp.len(), 1);
        assert_eq!(fills_tp[0].order_id.as_deref(), Some("t-tp"));
        assert_eq!(fills_tp[0].side, "buy"); // closing a short

        let fills_sl = evaluate_triggers(&mut state, "SOL", 110.5);
        assert_eq!(fills_sl.len(), 1);
        assert_eq!(fills_sl[0].order_id.as_deref(), Some("t-sl"));
    }

    #[test]
    fn trigger_size_is_capped_at_remaining_position() {
        let mut state = PaperState::new(10_000.0, "USDC".to_string(), 0.0);
        seed_long_position(&mut state); // 1.0 SOL, 100 lots
                                        // Push an oversized SL (would close 2 SOL but position is only 1).
        state.triggers.push(PaperTrigger {
            trigger_id: "oversized".to_string(),
            symbol: "SOL".to_string(),
            kind: PaperTriggerKind::StopLoss,
            position_side: "long".to_string(),
            trigger_price: 90.0,
            size_tokens: 2.0,
            size_lots: 200,
            parent_order_id: None,
            created_at: "2023-01-01T00:00:00Z".to_string(),
        });
        let fills = evaluate_triggers(&mut state, "SOL", 90.0);
        assert_eq!(fills.len(), 1);
        assert_eq!(fills[0].size_lots, 100, "must cap at remaining position");
        assert!(state.positions.is_empty(), "position fully closed");
    }

    #[test]
    fn orphan_triggers_purged_when_position_fully_closes() {
        let mut state = PaperState::new(10_000.0, "USDC".to_string(), 0.0);
        seed_long_position(&mut state);
        // SL closes the whole position at 90; an additional TP at 200 should be purged.
        state.triggers.push(PaperTrigger {
            trigger_id: "sl".to_string(),
            symbol: "SOL".to_string(),
            kind: PaperTriggerKind::StopLoss,
            position_side: "long".to_string(),
            trigger_price: 90.0,
            size_tokens: 1.0,
            size_lots: 100,
            parent_order_id: None,
            created_at: "2023-01-01T00:00:00Z".to_string(),
        });
        state.triggers.push(PaperTrigger {
            trigger_id: "tp".to_string(),
            symbol: "SOL".to_string(),
            kind: PaperTriggerKind::TakeProfit,
            position_side: "long".to_string(),
            trigger_price: 200.0,
            size_tokens: 1.0,
            size_lots: 100,
            parent_order_id: None,
            created_at: "2023-01-01T00:00:00Z".to_string(),
        });
        let fills = evaluate_triggers(&mut state, "SOL", 85.0);
        assert_eq!(fills.len(), 1);
        assert!(state.positions.is_empty());
        assert!(state.triggers.is_empty(), "orphan TP purged");
    }

    #[test]
    fn pending_triggers_survive_position_close() {
        // Regression: when an active SL fires and closes the position to zero,
        // the orphan-purge must not wipe pending triggers attached to a
        // separate resting limit on the same symbol. Those belong to the
        // resting order's eventual fill, not to the now-closed position.
        let mut state = PaperState::new(10_000.0, "USDC".to_string(), 0.0);
        seed_long_position(&mut state);
        // Active SL sized to the full 1.0 SOL long — closes the position when it fires.
        let active_sl = state.next_trigger_id();
        state.triggers.push(PaperTrigger {
            trigger_id: active_sl,
            symbol: "SOL".to_string(),
            kind: PaperTriggerKind::StopLoss,
            position_side: "long".to_string(),
            trigger_price: 90.0,
            size_tokens: 1.0,
            size_lots: 100,
            parent_order_id: None,
            created_at: "2023-01-01T00:00:00Z".to_string(),
        });
        // A resting limit and its pending TP/SL on the same symbol. These
        // belong to the next leg's fill; the purge must leave them alone.
        let resting_id = state.next_order_id();
        state.orders.push(PaperOrder {
            order_id: resting_id.clone(),
            symbol: "SOL".to_string(),
            side: "buy".to_string(),
            price: 95.0,
            size_tokens: 0.5,
            size_lots: 50,
            created_at: "2023-01-01T00:00:00Z".to_string(),
        });
        for kind in [PaperTriggerKind::TakeProfit, PaperTriggerKind::StopLoss] {
            let trigger_id = state.next_trigger_id();
            state.triggers.push(PaperTrigger {
                trigger_id,
                symbol: "SOL".to_string(),
                kind,
                position_side: "long".to_string(),
                trigger_price: if matches!(kind, PaperTriggerKind::TakeProfit) {
                    110.0
                } else {
                    85.0
                },
                size_tokens: 0.5,
                size_lots: 50,
                parent_order_id: Some(resting_id.clone()),
                created_at: "2023-01-01T00:00:00Z".to_string(),
            });
        }

        let fills = evaluate_triggers(&mut state, "SOL", 89.0);
        assert_eq!(fills.len(), 1, "active SL fires once");
        assert!(state.positions.is_empty(), "position closed by SL");
        assert_eq!(
            state.triggers.len(),
            2,
            "pending triggers on resting order must survive"
        );
        for t in &state.triggers {
            assert_eq!(
                t.parent_order_id.as_deref(),
                Some(resting_id.as_str()),
                "surviving triggers still parented to the resting order"
            );
        }
        assert_eq!(state.orders.len(), 1, "resting order still present");
    }

    #[test]
    fn netting_in_lots_handles_float_residue() {
        // Regression: many mixed-side buys/sells used to accumulate float drift
        // in `size_tokens`, leaving residues (~1e-10) that exceeded
        // `f64::EPSILON` (~2.2e-16) but rounded to zero lots — producing a
        // phantom position with size_lots: 0 and size_tokens > 0. After
        // refactoring `apply_position_fill` to net in integer lots, any
        // sequence that nets to zero lots must remove the position cleanly.
        let mut state = PaperState::new(1_000_000.0, "USDC".to_string(), 0.0);
        // Sequence whose lot-net is exactly zero across many opposite-side
        // fills at the same price (so realized PnL is also zero). Chosen to
        // accumulate float error in the old token-based netting path.
        let pairs: [(PaperSide, u64); 12] = [
            (PaperSide::Buy, 185),
            (PaperSide::Sell, 207),
            (PaperSide::Buy, 152),
            (PaperSide::Sell, 112),
            (PaperSide::Buy, 162),
            (PaperSide::Sell, 469),
            (PaperSide::Buy, 67),
            (PaperSide::Sell, 251),
            (PaperSide::Buy, 467),
            (PaperSide::Sell, 51),
            (PaperSide::Buy, 348),
            (PaperSide::Sell, 25),
        ];
        // Net lots: +185 -207 +152 -112 +162 -469 +67 -251 +467 -51 +348 -25 = 266
        // Make the last fill close the net to zero exactly.
        let net: i128 = pairs
            .iter()
            .map(|(s, l)| match s {
                PaperSide::Buy => *l as i128,
                PaperSide::Sell => -(*l as i128),
            })
            .sum();
        let closer_side = if net > 0 {
            PaperSide::Sell
        } else {
            PaperSide::Buy
        };
        for (side, lots) in pairs {
            apply_fill(
                &mut state,
                None,
                "SOL",
                side,
                PaperOrderType::Market,
                5.0,
                ResolvedPaperSize {
                    lots,
                    tokens: lots as f64 / 100.0,
                },
            );
        }
        let closer_lots = net.unsigned_abs() as u64;
        apply_fill(
            &mut state,
            None,
            "SOL",
            closer_side,
            PaperOrderType::Market,
            5.0,
            ResolvedPaperSize {
                lots: closer_lots,
                tokens: closer_lots as f64 / 100.0,
            },
        );
        assert!(
            state.positions.is_empty(),
            "position must close cleanly when lot-net is zero, got {:?}",
            state.positions
        );
    }

    #[test]
    fn pending_triggers_survive_candle_replay_position_close() {
        // Same regression for the candle-replay path: a candle that crosses
        // the active SL must close the position without sweeping pending
        // triggers attached to a resting limit on the same symbol.
        let mut state = PaperState::new(10_000.0, "USDC".to_string(), 0.0);
        seed_long_position(&mut state);
        // Full-size active SL — fires on candle low <= 90, closing the 1.0 SOL long.
        let active_sl = state.next_trigger_id();
        state.triggers.push(PaperTrigger {
            trigger_id: active_sl,
            symbol: "SOL".to_string(),
            kind: PaperTriggerKind::StopLoss,
            position_side: "long".to_string(),
            trigger_price: 90.0,
            size_tokens: 1.0,
            size_lots: 100,
            parent_order_id: None,
            created_at: "2023-01-01T00:00:00Z".to_string(),
        });
        let resting_id = state.next_order_id();
        state.orders.push(PaperOrder {
            order_id: resting_id.clone(),
            symbol: "SOL".to_string(),
            side: "buy".to_string(),
            price: 95.0,
            size_tokens: 0.5,
            size_lots: 50,
            created_at: "2023-01-01T00:00:00Z".to_string(),
        });
        let pending_id = state.next_trigger_id();
        state.triggers.push(PaperTrigger {
            trigger_id: pending_id.clone(),
            symbol: "SOL".to_string(),
            kind: PaperTriggerKind::TakeProfit,
            position_side: "long".to_string(),
            trigger_price: 110.0,
            size_tokens: 0.5,
            size_lots: 50,
            parent_order_id: Some(resting_id.clone()),
            created_at: "2023-01-01T00:00:00Z".to_string(),
        });

        let candles = vec![candle(1_700_000_000, 100.0, 102.0, 85.0, 95.0)];
        let fills = replay_candle_triggers(&mut state, "SOL", &candles);
        assert_eq!(fills.len(), 1, "active SL fires once on candle replay");
        assert!(state.positions.is_empty(), "position closed by replay SL");
        assert_eq!(state.triggers.len(), 1, "pending TP survives");
        assert_eq!(state.triggers[0].trigger_id, pending_id);
        assert_eq!(
            state.triggers[0].parent_order_id.as_deref(),
            Some(resting_id.as_str())
        );
        assert_eq!(state.orders.len(), 1, "resting order still present");
    }

    fn candle(close_time: i64, open: f64, high: f64, low: f64, close: f64) -> ReplayCandle {
        ReplayCandle {
            close_time,
            open,
            high,
            low,
            close,
        }
    }

    fn push_trigger_at(
        state: &mut PaperState,
        kind: PaperTriggerKind,
        side: &str,
        price: f64,
        created_at: &str,
    ) -> String {
        let id = state.next_trigger_id();
        state.triggers.push(PaperTrigger {
            trigger_id: id.clone(),
            symbol: "SOL".to_string(),
            kind,
            position_side: side.to_string(),
            trigger_price: price,
            size_tokens: 0.5,
            size_lots: 50,
            parent_order_id: None,
            created_at: created_at.to_string(),
        });
        id
    }

    #[test]
    fn replay_fires_trigger_when_candle_range_covers_price() {
        let mut state = PaperState::new(10_000.0, "USDC".to_string(), 0.0);
        seed_long_position(&mut state);
        push_trigger_at(
            &mut state,
            PaperTriggerKind::StopLoss,
            "long",
            90.0,
            "2023-01-01T00:00:00Z",
        );
        // Candle low touches 85 — passes through 90 → fire SL at 90.
        let candles = vec![candle(1_700_000_000, 100.0, 102.0, 85.0, 95.0)];
        let fills = replay_candle_triggers(&mut state, "SOL", &candles);
        assert_eq!(fills.len(), 1);
        assert!((fills[0].price - 90.0).abs() < f64::EPSILON);
        // Fill timestamp must reflect the candle close, not "now".
        assert!(fills[0].timestamp.starts_with("2023-11-14"));
        // Trigger size was 0.5 (from push_trigger_at), position was 1.0 — half
        // closed, half remains.
        assert_eq!(state.positions.len(), 1);
        assert!((state.positions[0].size_tokens - 0.5).abs() < 1e-6);
    }

    #[test]
    fn replay_skips_trigger_when_range_does_not_cover() {
        let mut state = PaperState::new(10_000.0, "USDC".to_string(), 0.0);
        seed_long_position(&mut state);
        push_trigger_at(
            &mut state,
            PaperTriggerKind::StopLoss,
            "long",
            85.0,
            "2025-01-01T00:00:00Z",
        );
        // Candle low is 90 — never visited 85.
        let candles = vec![candle(1_700_000_000, 100.0, 105.0, 90.0, 95.0)];
        let fills = replay_candle_triggers(&mut state, "SOL", &candles);
        assert!(fills.is_empty());
        assert_eq!(state.triggers.len(), 1);
    }

    #[test]
    fn replay_respects_trigger_created_at() {
        let mut state = PaperState::new(10_000.0, "USDC".to_string(), 0.0);
        seed_long_position(&mut state);
        // Trigger created AFTER the candle's close time → must not fire.
        let candle_close_ts = 1_700_000_000_i64;
        let after_candle = chrono::DateTime::from_timestamp(candle_close_ts + 3600, 0)
            .unwrap()
            .to_rfc3339();
        push_trigger_at(
            &mut state,
            PaperTriggerKind::StopLoss,
            "long",
            90.0,
            &after_candle,
        );
        let candles = vec![candle(candle_close_ts, 100.0, 102.0, 85.0, 95.0)];
        let fills = replay_candle_triggers(&mut state, "SOL", &candles);
        assert!(
            fills.is_empty(),
            "trigger that didn't exist yet must not fire"
        );
        assert_eq!(state.triggers.len(), 1);
    }

    #[test]
    fn bullish_candle_fires_higher_priced_triggers_first() {
        // Long position, both TP@108 and SL@92 inside a bullish candle's range.
        // In a bullish candle (close > open) we assume highs visited before
        // lows, so the long's TP should fire before its SL — TP fires first,
        // closes the position, SL becomes an orphan and gets purged.
        let mut state = PaperState::new(10_000.0, "USDC".to_string(), 0.0);
        // Use lots that match the candle-range fully so the first fire closes.
        apply_fill(
            &mut state,
            None,
            "SOL",
            PaperSide::Buy,
            PaperOrderType::Market,
            100.0,
            ResolvedPaperSize {
                lots: 100,
                tokens: 1.0,
            },
        );
        state.triggers.push(PaperTrigger {
            trigger_id: "tp".to_string(),
            symbol: "SOL".to_string(),
            kind: PaperTriggerKind::TakeProfit,
            position_side: "long".to_string(),
            trigger_price: 108.0,
            size_tokens: 1.0,
            size_lots: 100,
            parent_order_id: None,
            created_at: "2023-01-01T00:00:00Z".to_string(),
        });
        state.triggers.push(PaperTrigger {
            trigger_id: "sl".to_string(),
            symbol: "SOL".to_string(),
            kind: PaperTriggerKind::StopLoss,
            position_side: "long".to_string(),
            trigger_price: 92.0,
            size_tokens: 1.0,
            size_lots: 100,
            parent_order_id: None,
            created_at: "2023-01-01T00:00:00Z".to_string(),
        });
        let bullish = vec![candle(1_700_000_000, 100.0, 115.0, 85.0, 110.0)];
        let fills = replay_candle_triggers(&mut state, "SOL", &bullish);
        assert_eq!(fills.len(), 1);
        assert_eq!(
            fills[0].order_id.as_deref(),
            Some("tp"),
            "TP must fire first in bullish"
        );
        assert!(state.positions.is_empty());
        assert!(state.triggers.is_empty(), "orphan SL purged");
    }

    #[test]
    fn bearish_candle_fires_lower_priced_triggers_first() {
        let mut state = PaperState::new(10_000.0, "USDC".to_string(), 0.0);
        apply_fill(
            &mut state,
            None,
            "SOL",
            PaperSide::Buy,
            PaperOrderType::Market,
            100.0,
            ResolvedPaperSize {
                lots: 100,
                tokens: 1.0,
            },
        );
        state.triggers.push(PaperTrigger {
            trigger_id: "tp".to_string(),
            symbol: "SOL".to_string(),
            kind: PaperTriggerKind::TakeProfit,
            position_side: "long".to_string(),
            trigger_price: 108.0,
            size_tokens: 1.0,
            size_lots: 100,
            parent_order_id: None,
            created_at: "2023-01-01T00:00:00Z".to_string(),
        });
        state.triggers.push(PaperTrigger {
            trigger_id: "sl".to_string(),
            symbol: "SOL".to_string(),
            kind: PaperTriggerKind::StopLoss,
            position_side: "long".to_string(),
            trigger_price: 92.0,
            size_tokens: 1.0,
            size_lots: 100,
            parent_order_id: None,
            created_at: "2023-01-01T00:00:00Z".to_string(),
        });
        // Close < open → bearish; lows visited first.
        let bearish = vec![candle(1_700_000_000, 110.0, 115.0, 85.0, 90.0)];
        let fills = replay_candle_triggers(&mut state, "SOL", &bearish);
        assert_eq!(fills.len(), 1);
        assert_eq!(
            fills[0].order_id.as_deref(),
            Some("sl"),
            "SL fires first in bearish"
        );
    }

    #[test]
    fn replay_walks_multiple_candles_in_order() {
        let mut state = PaperState::new(10_000.0, "USDC".to_string(), 0.0);
        seed_long_position(&mut state); // 1 SOL long at 100
                                        // Two TP levels, multi-level laddered.
        state.triggers.push(PaperTrigger {
            trigger_id: "tp1".to_string(),
            symbol: "SOL".to_string(),
            kind: PaperTriggerKind::TakeProfit,
            position_side: "long".to_string(),
            trigger_price: 105.0,
            size_tokens: 0.5,
            size_lots: 50,
            parent_order_id: None,
            created_at: "2023-01-01T00:00:00Z".to_string(),
        });
        state.triggers.push(PaperTrigger {
            trigger_id: "tp2".to_string(),
            symbol: "SOL".to_string(),
            kind: PaperTriggerKind::TakeProfit,
            position_side: "long".to_string(),
            trigger_price: 110.0,
            size_tokens: 0.5,
            size_lots: 50,
            parent_order_id: None,
            created_at: "2023-01-01T00:00:00Z".to_string(),
        });
        let candles = vec![
            // Hour 1 — visits 105 but not 110.
            candle(1_700_000_000, 100.0, 106.0, 99.0, 103.0),
            // Hour 2 — visits 110.
            candle(1_700_003_600, 103.0, 112.0, 102.0, 111.0),
        ];
        let fills = replay_candle_triggers(&mut state, "SOL", &candles);
        assert_eq!(fills.len(), 2);
        assert_eq!(fills[0].order_id.as_deref(), Some("tp1"));
        assert_eq!(fills[1].order_id.as_deref(), Some("tp2"));
        assert!(state.positions.is_empty(), "1.0 - 0.5 - 0.5 = 0, closed");
    }

    #[test]
    fn pending_triggers_on_resting_orders_are_skipped() {
        let mut state = PaperState::new(10_000.0, "USDC".to_string(), 0.0);
        seed_long_position(&mut state);
        // Phase 4-shaped pending trigger — should NOT fire until parent fills.
        state.triggers.push(PaperTrigger {
            trigger_id: "pending".to_string(),
            symbol: "SOL".to_string(),
            kind: PaperTriggerKind::StopLoss,
            position_side: "long".to_string(),
            trigger_price: 95.0,
            size_tokens: 0.5,
            size_lots: 50,
            parent_order_id: Some("paper-order-99".to_string()),
            created_at: "2023-01-01T00:00:00Z".to_string(),
        });
        let fills = evaluate_triggers(&mut state, "SOL", 80.0);
        assert!(fills.is_empty());
        assert_eq!(state.triggers.len(), 1);
    }

    #[test]
    fn build_order_time_triggers_attaches_active_when_no_parent() {
        let mut state = PaperState::new(10_000.0, "USDC".to_string(), 0.0);
        let attached = build_order_time_triggers(
            &mut state,
            "SOL",
            PaperSide::Buy,
            Some(110.0),
            Some(90.0),
            100.0,
            100,
            1.0,
            None,
            "2025-01-01T00:00:00Z",
        )
        .unwrap();
        assert_eq!(attached.len(), 2);
        assert!(attached.iter().all(|t| t.parent_order_id.is_none()));
        assert_eq!(state.triggers.len(), 2);
        assert_eq!(attached[0].kind, PaperTriggerKind::TakeProfit);
        assert_eq!(attached[0].position_side, "long");
    }

    #[test]
    fn build_order_time_triggers_marks_pending_when_parented() {
        let mut state = PaperState::new(10_000.0, "USDC".to_string(), 0.0);
        let attached = build_order_time_triggers(
            &mut state,
            "SOL",
            PaperSide::Buy,
            Some(110.0),
            None,
            100.0,
            100,
            1.0,
            Some("paper-order-7".to_string()),
            "2025-01-01T00:00:00Z",
        )
        .unwrap();
        assert_eq!(attached.len(), 1);
        assert_eq!(
            attached[0].parent_order_id.as_deref(),
            Some("paper-order-7")
        );
    }

    #[test]
    fn build_order_time_triggers_rejects_reduce_only() {
        let mut state = PaperState::new(10_000.0, "USDC".to_string(), 0.0);
        seed_long_position(&mut state);
        // A sell on a long would reduce — order-time TP/SL must reject.
        let err = build_order_time_triggers(
            &mut state,
            "SOL",
            PaperSide::Sell,
            Some(90.0),
            None,
            100.0,
            50,
            0.5,
            None,
            "2025-01-01T00:00:00Z",
        )
        .unwrap_err();
        assert!(err.to_string().contains("reduce/flip"));
        assert!(state.triggers.is_empty());
    }

    #[test]
    fn build_order_time_triggers_validates_direction() {
        let mut state = PaperState::new(10_000.0, "USDC".to_string(), 0.0);
        // Long with TP below entry must error before anything is pushed.
        let err = build_order_time_triggers(
            &mut state,
            "SOL",
            PaperSide::Buy,
            Some(95.0),
            None,
            100.0,
            100,
            1.0,
            None,
            "2025-01-01T00:00:00Z",
        )
        .unwrap_err();
        assert!(err.to_string().contains("above"));
        assert!(state.triggers.is_empty());
    }

    #[test]
    fn cancel_cascades_to_pending_triggers() {
        let tmp = tempfile::tempdir().unwrap();
        let path = tmp.path().join("paper-state.json");
        let mut state = PaperState::new(10_000.0, "USDC".to_string(), 0.0);
        state.orders.push(PaperOrder {
            order_id: "paper-order-1".to_string(),
            symbol: "SOL".to_string(),
            side: "buy".to_string(),
            price: 100.0,
            size_tokens: 1.0,
            size_lots: 100,
            created_at: "2023-01-01T00:00:00Z".to_string(),
        });
        state.triggers.push(PaperTrigger {
            trigger_id: "paper-trigger-1".to_string(),
            symbol: "SOL".to_string(),
            kind: PaperTriggerKind::TakeProfit,
            position_side: "long".to_string(),
            trigger_price: 110.0,
            size_tokens: 1.0,
            size_lots: 100,
            parent_order_id: Some("paper-order-1".to_string()),
            created_at: "2023-01-01T00:00:00Z".to_string(),
        });
        // Unrelated active trigger that must survive.
        state.triggers.push(PaperTrigger {
            trigger_id: "paper-trigger-2".to_string(),
            symbol: "ETH".to_string(),
            kind: PaperTriggerKind::StopLoss,
            position_side: "long".to_string(),
            trigger_price: 3000.0,
            size_tokens: 1.0,
            size_lots: 100,
            parent_order_id: None,
            created_at: "2023-01-01T00:00:00Z".to_string(),
        });
        save_state(&path, &state).unwrap();

        let (result, state_after) = cancel(&path, state, "paper-order-1").unwrap();
        assert_eq!(result.cancelled, 2, "order + its pending trigger");
        assert!(result.order_ids.contains(&"paper-order-1".to_string()));
        assert!(result.order_ids.contains(&"paper-trigger-1".to_string()));
        assert_eq!(state_after.orders.len(), 0);
        assert_eq!(state_after.triggers.len(), 1);
        assert_eq!(state_after.triggers[0].trigger_id, "paper-trigger-2");
    }

    #[test]
    fn cancel_all_cascades_to_pending_triggers_for_symbol() {
        let tmp = tempfile::tempdir().unwrap();
        let path = tmp.path().join("paper-state.json");
        let mut state = PaperState::new(10_000.0, "USDC".to_string(), 0.0);
        state.orders.push(PaperOrder {
            order_id: "paper-order-1".to_string(),
            symbol: "SOL".to_string(),
            side: "buy".to_string(),
            price: 100.0,
            size_tokens: 1.0,
            size_lots: 100,
            created_at: "2023-01-01T00:00:00Z".to_string(),
        });
        state.triggers.push(PaperTrigger {
            trigger_id: "pending".to_string(),
            symbol: "SOL".to_string(),
            kind: PaperTriggerKind::TakeProfit,
            position_side: "long".to_string(),
            trigger_price: 110.0,
            size_tokens: 1.0,
            size_lots: 100,
            parent_order_id: Some("paper-order-1".to_string()),
            created_at: "2023-01-01T00:00:00Z".to_string(),
        });
        save_state(&path, &state).unwrap();

        let (result, state_after) = cancel_all(&path, state, Some("SOL")).unwrap();
        assert_eq!(result.cancelled, 2);
        assert!(state_after.orders.is_empty());
        assert!(state_after.triggers.is_empty());
    }

    #[test]
    fn positions_projects_active_triggers_only() {
        let mut state = PaperState::new(10_000.0, "USDC".to_string(), 0.0);
        seed_long_position(&mut state); // SOL long
                                        // Active TP, active SL on the position.
        push_trigger(&mut state, PaperTriggerKind::TakeProfit, 110.0);
        push_trigger(&mut state, PaperTriggerKind::StopLoss, 90.0);
        // Pending trigger on a still-resting limit — must NOT project onto position.
        state.triggers.push(PaperTrigger {
            trigger_id: "pending".to_string(),
            symbol: "SOL".to_string(),
            kind: PaperTriggerKind::TakeProfit,
            position_side: "long".to_string(),
            trigger_price: 120.0,
            size_tokens: 0.5,
            size_lots: 50,
            parent_order_id: Some("paper-order-99".to_string()),
            created_at: "2023-01-01T00:00:00Z".to_string(),
        });

        let result = positions(&state);
        assert_eq!(result.positions.len(), 1);
        let p = &result.positions[0];
        assert_eq!(p.tp_levels.len(), 1, "pending trigger excluded");
        assert!((p.tp_levels[0].price - 110.0).abs() < f64::EPSILON);
        assert_eq!(p.sl_levels.len(), 1);
        assert!((p.sl_levels[0].price - 90.0).abs() < f64::EPSILON);
    }

    #[test]
    fn status_includes_trigger_count() {
        let tmp = tempfile::tempdir().unwrap();
        let path = tmp.path().join("paper-state.json");
        let mut state = PaperState::new(10_000.0, "USDC".to_string(), 0.0);
        seed_long_position(&mut state);
        push_trigger(&mut state, PaperTriggerKind::TakeProfit, 110.0);
        push_trigger(&mut state, PaperTriggerKind::StopLoss, 90.0);
        save_state(&path, &state).unwrap();
        let status = status_from_state(&path, &state);
        assert_eq!(status.triggers, 2);
    }

    #[test]
    fn cancel_trigger_id_does_not_cascade() {
        let tmp = tempfile::tempdir().unwrap();
        let path = tmp.path().join("paper-state.json");
        let mut state = PaperState::new(10_000.0, "USDC".to_string(), 0.0);
        state.orders.push(PaperOrder {
            order_id: "paper-order-1".to_string(),
            symbol: "SOL".to_string(),
            side: "buy".to_string(),
            price: 100.0,
            size_tokens: 1.0,
            size_lots: 100,
            created_at: "2023-01-01T00:00:00Z".to_string(),
        });
        state.triggers.push(PaperTrigger {
            trigger_id: "pending".to_string(),
            symbol: "SOL".to_string(),
            kind: PaperTriggerKind::TakeProfit,
            position_side: "long".to_string(),
            trigger_price: 110.0,
            size_tokens: 1.0,
            size_lots: 100,
            parent_order_id: Some("paper-order-1".to_string()),
            created_at: "2023-01-01T00:00:00Z".to_string(),
        });
        save_state(&path, &state).unwrap();

        let (result, state_after) = cancel(&path, state, "pending").unwrap();
        assert_eq!(result.cancelled, 1);
        assert_eq!(state_after.orders.len(), 1, "parent order untouched");
        assert!(state_after.triggers.is_empty());
    }

    #[test]
    fn paper_position_flips_side() {
        let mut state = PaperState::new(10_000.0, "USDC".to_string(), 0.0);
        apply_fill(
            &mut state,
            None,
            "SOL",
            PaperSide::Buy,
            PaperOrderType::Market,
            100.0,
            ResolvedPaperSize {
                lots: 100,
                tokens: 1.0,
            },
        );
        apply_fill(
            &mut state,
            None,
            "SOL",
            PaperSide::Sell,
            PaperOrderType::Market,
            90.0,
            ResolvedPaperSize {
                lots: 200,
                tokens: 2.0,
            },
        );
        assert_eq!(state.positions[0].side, "short");
        assert_eq!(state.positions[0].size_tokens, 1.0);
        assert_eq!(state.positions[0].entry_price, 90.0);
        assert!((state.balance - 9_990.0).abs() < 0.0001);
    }
}

#[cfg(test)]
mod proptests {
    use super::*;
    use proptest::prelude::*;

    const SYMBOL: &str = "SOL";
    const STARTING_BALANCE: f64 = 1_000_000.0;
    const LOTS_PER_TOKEN: f64 = 100.0; // matches the test-only ResolvedPaperSize convention

    /// One synthetic order in a random sequence.
    #[derive(Debug, Clone)]
    struct SynthOrder {
        side: PaperSide,
        tokens: f64,
        price: f64,
    }

    fn arb_order() -> impl Strategy<Value = SynthOrder> {
        (
            prop::bool::ANY,
            10u32..=500,   // tokens × 100 = lots (keeps lots integer)
            500u32..=2000, // price × 100 (keeps prices in [5.00, 20.00])
        )
            .prop_map(|(buy, lots_x100, px_x100)| SynthOrder {
                side: if buy { PaperSide::Buy } else { PaperSide::Sell },
                tokens: lots_x100 as f64 / LOTS_PER_TOKEN,
                price: px_x100 as f64 / 100.0,
            })
    }

    fn arb_orders(max_len: usize) -> impl Strategy<Value = Vec<SynthOrder>> {
        prop::collection::vec(arb_order(), 0..=max_len)
    }

    fn apply_synth(state: &mut PaperState, order: &SynthOrder) -> PaperFill {
        let lots = (order.tokens * LOTS_PER_TOKEN).round() as u64;
        apply_fill(
            state,
            None,
            SYMBOL,
            order.side,
            PaperOrderType::Market,
            order.price,
            ResolvedPaperSize {
                lots,
                tokens: lots as f64 / LOTS_PER_TOKEN,
            },
        )
    }

    proptest! {
        /// For any random sequence of buy/sell fills with zero fees, the running
        /// balance equals starting_balance + sum of realized PnL. Money is not
        /// created or destroyed by the netting machinery.
        #[test]
        fn balance_conservation_no_fees(orders in arb_orders(50)) {
            let mut state = PaperState::new(STARTING_BALANCE, "USDC".to_string(), 0.0);
            let mut realized_total = 0.0;
            for o in &orders {
                let f = apply_synth(&mut state, o);
                realized_total += f.realized_pnl;
            }
            let expected = STARTING_BALANCE + realized_total;
            prop_assert!(
                (state.balance - expected).abs() < 1e-6,
                "balance {} != expected {} after {} orders (realized total {})",
                state.balance, expected, orders.len(), realized_total
            );
            // Fees must be zero with fee_bps=0.
            let fees: f64 = state.fills.iter().map(|f| f.fee).sum();
            prop_assert!(fees.abs() < 1e-9, "expected zero fees, got {}", fees);
        }

        /// With a non-zero fee rate, balance == starting + sum(realized) - sum(fees).
        #[test]
        fn balance_conservation_with_fees(orders in arb_orders(30)) {
            let fee_bps = 5.0;
            let mut state = PaperState::new(STARTING_BALANCE, "USDC".to_string(), fee_bps);
            for o in &orders {
                apply_synth(&mut state, o);
            }
            let realized: f64 = state.fills.iter().map(|f| f.realized_pnl).sum();
            let fees: f64 = state.fills.iter().map(|f| f.fee).sum();
            let expected = STARTING_BALANCE + realized - fees;
            prop_assert!(
                (state.balance - expected).abs() < 1e-6,
                "balance {} != expected {}", state.balance, expected
            );
        }

        /// After any sequence of fills, the position state stays sane:
        /// - At most one position per symbol.
        /// - If present, size_tokens > 0 (zero-size positions are removed).
        /// - size_lots / multiplier ≈ size_tokens.
        #[test]
        fn position_invariants(orders in arb_orders(50)) {
            let mut state = PaperState::new(STARTING_BALANCE, "USDC".to_string(), 0.0);
            for o in &orders {
                apply_synth(&mut state, o);
            }
            prop_assert!(state.positions.len() <= 1, "more than one position for {}", SYMBOL);
            for p in &state.positions {
                prop_assert!(p.size_tokens > 0.0, "phantom zero-size position");
                prop_assert!(p.size_lots > 0, "phantom zero-lot position");
                let expected_tokens = p.size_lots as f64 / LOTS_PER_TOKEN;
                prop_assert!(
                    (p.size_tokens - expected_tokens).abs() < 1e-6,
                    "size_tokens {} != size_lots/{} = {}",
                    p.size_tokens, LOTS_PER_TOKEN, expected_tokens
                );
            }
        }

        /// Trigger evaluation is idempotent at a fixed mark: a second call after
        /// the first must produce no additional fills.
        #[test]
        fn trigger_eval_idempotent(
            orders in arb_orders(20),
            mark_x100 in 500u32..=2000u32,
        ) {
            let mut state = PaperState::new(STARTING_BALANCE, "USDC".to_string(), 0.0);
            for o in &orders {
                apply_synth(&mut state, o);
            }
            // Seed a handful of triggers at random prices.
            for px in [600u32, 800, 1000, 1200, 1500] {
                if let Some(pos) = state.positions.first().cloned() {
                    let trigger_id = state.next_trigger_id();
                    state.triggers.push(PaperTrigger {
                        trigger_id,
                        symbol: SYMBOL.to_string(),
                        kind: if px as f64 / 100.0 > pos.entry_price {
                            PaperTriggerKind::TakeProfit
                        } else {
                            PaperTriggerKind::StopLoss
                        },
                        position_side: pos.side.clone(),
                        trigger_price: px as f64 / 100.0,
                        size_tokens: 0.1,
                        size_lots: 10,
                        parent_order_id: None,
                        created_at: "2023-01-01T00:00:00Z".to_string(),
                    });
                }
            }
            let mark = mark_x100 as f64 / 100.0;
            let _first = evaluate_triggers(&mut state, SYMBOL, mark);
            let second = evaluate_triggers(&mut state, SYMBOL, mark);
            prop_assert!(
                second.is_empty(),
                "second evaluate_triggers at the same mark produced {} extra fills",
                second.len()
            );
        }

        /// Orphan-trigger invariant: every persisted trigger either has a
        /// matching position OR a matching resting limit order.
        #[test]
        fn no_orphan_triggers(orders in arb_orders(30)) {
            let mut state = PaperState::new(STARTING_BALANCE, "USDC".to_string(), 0.0);
            for o in &orders {
                apply_synth(&mut state, o);
            }
            // Seed triggers tied to the position (if any) and verify the
            // evaluator purges them once the position closes.
            if let Some(pos) = state.positions.first().cloned() {
                for kind in [PaperTriggerKind::TakeProfit, PaperTriggerKind::StopLoss] {
                    let trigger_id = state.next_trigger_id();
                    state.triggers.push(PaperTrigger {
                        trigger_id,
                        symbol: SYMBOL.to_string(),
                        kind,
                        position_side: pos.side.clone(),
                        trigger_price: pos.entry_price,
                        size_tokens: pos.size_tokens,
                        size_lots: pos.size_lots,
                        parent_order_id: None,
                        created_at: "2023-01-01T00:00:00Z".to_string(),
                    });
                }
                // Mark at entry will fire something (TP fires at mark==entry on
                // longs, SL fires at mark==entry on shorts — depending on side).
                // After firing, position may close; any remaining trigger on
                // this symbol with parent_order_id=None must still have a
                // corresponding position.
                let _ = evaluate_triggers(&mut state, SYMBOL, pos.entry_price);
            }
            for t in &state.triggers {
                if t.parent_order_id.is_none() {
                    let has_position = state.positions.iter().any(|p| p.symbol == t.symbol);
                    prop_assert!(
                        has_position,
                        "active trigger {} on {} but no position",
                        t.trigger_id, t.symbol
                    );
                }
                // Pending triggers must reference an existing order.
                if let Some(parent) = &t.parent_order_id {
                    let has_order = state.orders.iter().any(|o| &o.order_id == parent);
                    prop_assert!(
                        has_order,
                        "pending trigger {} references missing order {}",
                        t.trigger_id, parent
                    );
                }
            }
        }

        /// State round-trips through JSON without loss: serialize → deserialize
        /// gives an equivalent state.
        #[test]
        fn json_roundtrip(orders in arb_orders(30)) {
            let mut state = PaperState::new(STARTING_BALANCE, "USDC".to_string(), 5.0);
            for o in &orders {
                apply_synth(&mut state, o);
            }
            let text = serde_json::to_string(&state).expect("serialize");
            let restored: PaperState =
                serde_json::from_str(&text).expect("deserialize");
            prop_assert!((restored.balance - state.balance).abs() < 1e-9);
            prop_assert_eq!(restored.positions.len(), state.positions.len());
            prop_assert_eq!(restored.fills.len(), state.fills.len());
            prop_assert_eq!(restored.next_order_id, state.next_order_id);
            prop_assert_eq!(restored.next_fill_id, state.next_fill_id);
            prop_assert_eq!(restored.next_trigger_id, state.next_trigger_id);
        }

        /// Cancelling all orders cleans every pending trigger that referenced
        /// one of them. Active triggers (parent=None) are untouched.
        #[test]
        fn cancel_all_cascades_pending(num_orders in 1usize..=8) {
            let tmp = tempfile::tempdir().expect("tempdir");
            let path = tmp.path().join("paper-state.json");
            let mut state = PaperState::new(STARTING_BALANCE, "USDC".to_string(), 0.0);

            // Plant some pending triggers attached to resting orders, plus an
            // unrelated active trigger that must survive.
            for i in 0..num_orders {
                let order_id = format!("paper-order-{}", i);
                state.orders.push(PaperOrder {
                    order_id: order_id.clone(),
                    symbol: SYMBOL.to_string(),
                    side: "buy".to_string(),
                    price: 10.0,
                    size_tokens: 0.5,
                    size_lots: 50,
                    created_at: "2023-01-01T00:00:00Z".to_string(),
                });
                let trigger_id = state.next_trigger_id();
                state.triggers.push(PaperTrigger {
                    trigger_id,
                    symbol: SYMBOL.to_string(),
                    kind: PaperTriggerKind::TakeProfit,
                    position_side: "long".to_string(),
                    trigger_price: 12.0,
                    size_tokens: 0.5,
                    size_lots: 50,
                    parent_order_id: Some(order_id),
                    created_at: "2023-01-01T00:00:00Z".to_string(),
                });
            }
            // Active trigger that must survive.
            state.positions.push(PaperPosition {
                symbol: "ETH".to_string(),
                side: "long".to_string(),
                size_tokens: 1.0,
                size_lots: 100,
                entry_price: 3000.0,
                mark_price: 3000.0,
                unrealized_pnl: 0.0,
                realized_pnl: 0.0,
                tp_levels: vec![],
                sl_levels: vec![],
            });
            let survivor_id = state.next_trigger_id();
            state.triggers.push(PaperTrigger {
                trigger_id: survivor_id.clone(),
                symbol: "ETH".to_string(),
                kind: PaperTriggerKind::TakeProfit,
                position_side: "long".to_string(),
                trigger_price: 3500.0,
                size_tokens: 1.0,
                size_lots: 100,
                parent_order_id: None,
                created_at: "2023-01-01T00:00:00Z".to_string(),
            });
            save_state(&path, &state).expect("save");

            let (result, state_after) =
                cancel_all(&path, state, Some(SYMBOL)).expect("cancel_all");
            prop_assert_eq!(result.cancelled, num_orders * 2);
            prop_assert_eq!(state_after.orders.len(), 0);
            prop_assert_eq!(state_after.triggers.len(), 1);
            prop_assert_eq!(state_after.triggers[0].trigger_id.as_str(), survivor_id.as_str());
        }
    }
}

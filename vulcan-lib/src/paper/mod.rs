//! Local paper trading engine for Phoenix perpetuals.
//!
//! Paper trading uses live market data but stores all account state locally. It
//! never signs or submits Solana transactions.

use crate::commands::market::{execute_info_inner, execute_orderbook_inner, execute_ticker_inner};
use crate::context::AppContext;
use crate::error::VulcanError;
use chrono::Utc;
use rand::RngCore;
use serde::{Deserialize, Serialize};
use std::fs;
use std::path::{Path, PathBuf};

const DEFAULT_BALANCE: f64 = 10_000.0;
const DEFAULT_FEE_BPS: f64 = 5.0;

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
    pub positions: Vec<PaperPosition>,
    pub orders: Vec<PaperOrder>,
    pub fills: Vec<PaperFill>,
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
            positions: Vec::new(),
            orders: Vec::new(),
            fills: Vec::new(),
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

    refresh_position_marks(&mut state, symbol, market.mark);
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
    let mut remaining = Vec::new();
    let mut fills = Vec::new();
    let checked_orders = state
        .orders
        .iter()
        .filter(|o| symbol_filter.map(|s| s == o.symbol).unwrap_or(true))
        .count();

    for order in std::mem::take(&mut state.orders) {
        if symbol_filter.map(|s| s != order.symbol).unwrap_or(false) {
            remaining.push(order);
            continue;
        }
        let market = fetch_paper_price(ctx, &order.symbol).await?;
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
            refresh_position_marks(&mut state, &order.symbol, market.mark);
            fills.push(fill);
        } else {
            refresh_position_marks(&mut state, &order.symbol, market.mark);
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
    let before = state.orders.len();
    state.orders.retain(|o| o.order_id != order_id);
    let cancelled = before - state.orders.len();
    if cancelled == 0 {
        return Err(VulcanError::validation(
            "PAPER_ORDER_NOT_FOUND",
            format!("No paper order found with id {}", order_id),
        ));
    }
    state.touch();
    save_state(state_path, &state)?;
    let result = PaperCancelResult {
        mode: "paper".to_string(),
        cancelled,
        order_ids: vec![order_id.to_string()],
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
    let cancelled = order_ids.len();
    state.touch();
    save_state(state_path, &state)?;
    let result = PaperCancelResult {
        mode: "paper".to_string(),
        cancelled,
        order_ids,
        state: status_from_state(state_path, &state),
    };
    Ok((result, state))
}

pub fn positions(state: &PaperState) -> PaperPositionsResult {
    let unrealized_pnl = state
        .positions
        .iter()
        .map(|p| p.unrealized_pnl)
        .sum::<f64>();
    let equity = state.balance + unrealized_pnl;
    let position_notional_usdc = position_notional_usdc(&state.positions);
    PaperPositionsResult {
        mode: "paper".to_string(),
        currency: state.currency.clone(),
        equity,
        position_notional_usdc,
        exposure_ratio: exposure_ratio(position_notional_usdc, equity),
        positions: state.positions.clone(),
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
    let signed_new = side.signed(size.tokens);
    let Some(idx) = state.positions.iter().position(|p| p.symbol == symbol) else {
        state.positions.push(PaperPosition {
            symbol: symbol.to_string(),
            side: position_side(signed_new).to_string(),
            size_tokens: signed_new.abs(),
            size_lots: size.lots,
            entry_price: price,
            mark_price: price,
            unrealized_pnl: 0.0,
            realized_pnl: 0.0,
        });
        return 0.0;
    };

    let position = &mut state.positions[idx];
    let signed_old = signed_position_qty(position);
    let realized = if signed_old.signum() != signed_new.signum() {
        let closed = signed_old.abs().min(signed_new.abs());
        if signed_old > 0.0 {
            (price - position.entry_price) * closed
        } else {
            (position.entry_price - price) * closed
        }
    } else {
        0.0
    };
    let signed_total = signed_old + signed_new;
    position.realized_pnl += realized;

    if signed_total.abs() < f64::EPSILON {
        state.positions.remove(idx);
        return realized;
    }

    if signed_old.signum() == signed_new.signum() {
        position.entry_price = ((signed_old.abs() * position.entry_price)
            + (signed_new.abs() * price))
            / signed_total.abs();
    } else if signed_total.signum() != signed_old.signum() {
        position.entry_price = price;
    }

    position.side = position_side(signed_total).to_string();
    position.size_tokens = signed_total.abs();
    position.size_lots = size_tokens_to_lots(position.size_tokens, size.tokens, size.lots);
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

fn unrealized_for(position: &PaperPosition, mark: f64) -> f64 {
    if position.side == "long" {
        (mark - position.entry_price) * position.size_tokens
    } else {
        (position.entry_price - mark) * position.size_tokens
    }
}

fn signed_position_qty(position: &PaperPosition) -> f64 {
    if position.side == "long" {
        position.size_tokens
    } else {
        -position.size_tokens
    }
}

fn position_side(signed_qty: f64) -> &'static str {
    if signed_qty >= 0.0 {
        "long"
    } else {
        "short"
    }
}

fn size_tokens_to_lots(position_tokens: f64, fill_tokens: f64, fill_lots: u64) -> u64 {
    ((position_tokens / fill_tokens) * fill_lots as f64).round() as u64
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

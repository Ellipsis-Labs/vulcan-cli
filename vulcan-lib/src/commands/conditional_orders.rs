//! Read-side support for Phoenix conditional orders (multi-level TP/SL).
//!
//! Phoenix's v1 trader-state endpoint surfaces TP/SL triggers alongside
//! position rows. Vulcan uses that HTTP snapshot for read and display paths.

use crate::commands::trade::OrderInfo;
use crate::commands::trader_state::{
    fetch_trader_state_snapshot, TraderStateSnapshot, TraderStateTrigger, TraderStateTriggerRow,
};
use crate::context::AppContext;
use crate::error::VulcanError;
use phoenix_rise::accounts::owned::{ConditionalOrderCollection, StopLossTradeSide};
use phoenix_rise::ix::constants::get_conditional_orders_address;
use phoenix_rise::math::{BaseLots, Side as RiseSide, Ticks};
use phoenix_rise::api::PhoenixMetadata;
use solana_pubkey::Pubkey;
use solana_commitment_config::CommitmentConfig;
use std::collections::{HashMap, HashSet};

/// Fetch and decode the on-chain `ConditionalOrderCollection` for one trader.
/// Returns `Ok(None)` when the account does not exist (no conditional orders
/// have ever been placed for this trader).
///
/// Prefer the trader-state trigger projection for read/display paths. This RPC
/// decoder is kept for cancellation when trader-state omits cancellable API IDs.
pub async fn fetch_conditional_orders(
    ctx: &AppContext,
    trader_pda: Pubkey,
) -> Result<Option<ConditionalOrderCollection>, VulcanError> {
    let pda = get_conditional_orders_address(&trader_pda)
        .map_err(|e| VulcanError::api("CONDITIONAL_ORDERS_PDA_FAILED", e.to_string()))?;
    let rpc = ctx.rpc_client_async();

    let account = rpc
        .get_account_with_commitment(&pda, CommitmentConfig::confirmed())
        .await
        .map_err(|e| VulcanError::network("RPC_GET_ACCOUNT_FAILED", e.to_string()))?
        .value;

    let Some(account) = account else {
        return Ok(None);
    };

    let collection = ConditionalOrderCollection::try_from_account_bytes(&account.data)
        .map_err(|e| VulcanError::api("CONDITIONAL_ORDERS_DECODE_FAILED", e.to_string()))?;
    Ok(Some(collection))
}

/// Fetch projected conditional triggers directly from v1 trader-state.
pub async fn fetch_conditional_triggers_for_authority(
    ctx: &AppContext,
    authority: &Pubkey,
    pda_index: u8,
    subaccount_index: u8,
    metadata: &PhoenixMetadata,
) -> Result<Vec<ConditionalTriggerView>, VulcanError> {
    let response = fetch_trader_state_snapshot(ctx, authority, pda_index).await?;
    Ok(project_trader_state_triggers_for_subaccount(
        &response.snapshot,
        subaccount_index,
        metadata,
    ))
}

#[derive(Debug, Clone)]
pub struct TraderStateLimitOrderView {
    pub subaccount_index: u8,
    pub symbol: String,
    pub side: RiseSide,
    pub order_sequence_number: String,
    pub order_sequence_number_u64: u64,
    pub price_ticks: u64,
    pub price: f64,
    pub size_remaining_tokens: f64,
    pub initial_size_tokens: f64,
    pub reduce_only: bool,
    pub is_stop_loss: bool,
}

#[derive(Debug, Clone)]
pub struct TraderStateOrderData {
    pub limit_orders: Vec<TraderStateLimitOrderView>,
    pub conditional_triggers: Vec<ConditionalTriggerView>,
    pub has_unparsed_limit_orders: bool,
}

/// Fetch all order-like rows from v1 trader-state for one subaccount. This
/// covers regular limit orders plus conditional TP/SL triggers in one trader
/// state API call.
pub async fn fetch_trader_state_order_data(
    ctx: &AppContext,
    authority: &Pubkey,
    pda_index: u8,
    subaccount_index: u8,
    metadata: &PhoenixMetadata,
) -> Result<TraderStateOrderData, VulcanError> {
    let response = fetch_trader_state_snapshot(ctx, authority, pda_index).await?;
    Ok(project_trader_state_order_data_for_subaccount(
        &response.snapshot,
        subaccount_index,
        metadata,
    ))
}

/// Fetch all order-like rows from v1 trader-state across every subaccount.
/// This covers regular limit orders plus conditional TP/SL triggers without
/// narrowing the account into an HTTP view shape.
pub async fn fetch_all_trader_state_order_data(
    ctx: &AppContext,
    authority: &Pubkey,
    pda_index: u8,
    metadata: &PhoenixMetadata,
) -> Result<TraderStateOrderData, VulcanError> {
    let response = fetch_trader_state_snapshot(ctx, authority, pda_index).await?;
    Ok(project_all_trader_state_order_data(
        &response.snapshot,
        metadata,
    ))
}

pub(crate) fn project_trader_state_order_data_for_subaccount(
    snapshot: &TraderStateSnapshot,
    subaccount_index: u8,
    metadata: &PhoenixMetadata,
) -> TraderStateOrderData {
    let (limit_orders, has_unparsed_limit_orders) =
        project_trader_state_limit_orders(snapshot, subaccount_index, metadata);
    let conditional_triggers =
        project_trader_state_triggers_for_subaccount(snapshot, subaccount_index, metadata);

    TraderStateOrderData {
        limit_orders,
        conditional_triggers,
        has_unparsed_limit_orders,
    }
}

pub(crate) fn project_all_trader_state_order_data(
    snapshot: &TraderStateSnapshot,
    metadata: &PhoenixMetadata,
) -> TraderStateOrderData {
    let mut limit_orders = Vec::new();
    let mut conditional_triggers = Vec::new();
    let mut has_unparsed_limit_orders = false;

    for subaccount in &snapshot.subaccounts {
        let (orders, has_unparsed) =
            project_trader_state_limit_orders(snapshot, subaccount.subaccount_index, metadata);
        limit_orders.extend(orders);
        has_unparsed_limit_orders |= has_unparsed;
        conditional_triggers.extend(project_trader_state_triggers_for_subaccount(
            snapshot,
            subaccount.subaccount_index,
            metadata,
        ));
    }

    TraderStateOrderData {
        limit_orders,
        conditional_triggers,
        has_unparsed_limit_orders,
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum TriggerKind {
    StopLoss,
    TakeProfit,
}

#[derive(Debug, Clone)]
pub struct ConditionalTriggerView {
    pub subaccount_index: u8,
    pub symbol: String,
    pub kind: TriggerKind,
    pub trigger_price: f64,
    pub size_tokens: f64,
    pub side: StopLossTradeSide,
    pub order_id: String,
}

pub(crate) fn project_trader_state_triggers_for_subaccount(
    snapshot: &TraderStateSnapshot,
    subaccount_index: u8,
    metadata: &PhoenixMetadata,
) -> Vec<ConditionalTriggerView> {
    let Some(subaccount) = snapshot
        .subaccounts
        .iter()
        .find(|s| s.subaccount_index == subaccount_index)
    else {
        return Vec::new();
    };

    let mut position_lots_by_symbol = HashMap::new();
    for position in &subaccount.positions {
        if let Some(lots) = parse_abs_lots(&position.base_position_lots) {
            position_lots_by_symbol.insert(position.symbol.to_ascii_uppercase(), lots);
        }
    }

    let mut out = Vec::new();
    let mut seen = HashSet::new();
    let mut projection = StateTriggerProjection {
        out: &mut out,
        seen: &mut seen,
        metadata,
        position_lots_by_symbol: &position_lots_by_symbol,
        subaccount_index: subaccount.subaccount_index,
    };

    for position in &subaccount.positions {
        projection.append(
            &position.symbol,
            TriggerKind::TakeProfit,
            &position.take_profit_triggers,
        );
        projection.append(
            &position.symbol,
            TriggerKind::StopLoss,
            &position.stop_loss_triggers,
        );
        projection.append(
            &position.symbol,
            TriggerKind::TakeProfit,
            &position.conditional_take_profit_triggers,
        );
        projection.append(
            &position.symbol,
            TriggerKind::StopLoss,
            &position.conditional_stop_loss_triggers,
        );
    }

    for triggers in &subaccount.triggers {
        projection.append(
            &triggers.symbol,
            TriggerKind::TakeProfit,
            &triggers.take_profit_triggers,
        );
        projection.append(
            &triggers.symbol,
            TriggerKind::StopLoss,
            &triggers.stop_loss_triggers,
        );
        projection.append(
            &triggers.symbol,
            TriggerKind::TakeProfit,
            &triggers.conditional_take_profit_triggers,
        );
        projection.append(
            &triggers.symbol,
            TriggerKind::StopLoss,
            &triggers.conditional_stop_loss_triggers,
        );
    }

    out
}

pub(crate) fn project_trader_state_limit_orders(
    snapshot: &TraderStateSnapshot,
    subaccount_index: u8,
    metadata: &PhoenixMetadata,
) -> (Vec<TraderStateLimitOrderView>, bool) {
    let Some(subaccount) = snapshot
        .subaccounts
        .iter()
        .find(|s| s.subaccount_index == subaccount_index)
    else {
        return (Vec::new(), false);
    };

    let mut out = Vec::new();
    let mut has_unparsed = false;

    for market_orders in &subaccount.orders {
        let symbol_upper = market_orders.symbol.to_ascii_uppercase();
        let Some(calc) = metadata.get_market_calculator(&symbol_upper) else {
            has_unparsed |= market_orders
                .orders
                .iter()
                .any(|row| trigger_status_is_open(&row.status));
            continue;
        };

        for row in &market_orders.orders {
            if !trigger_status_is_open(&row.status) {
                continue;
            }

            let parsed = parse_u64(&row.order_sequence_number)
                .zip(parse_u64(&row.price_ticks))
                .zip(parse_u64(&row.size_remaining_lots))
                .zip(parse_u64(&row.initial_size_lots));
            let Some((
                ((order_sequence_number_u64, price_ticks), size_remaining_lots),
                initial_size_lots,
            )) = parsed
            else {
                has_unparsed = true;
                continue;
            };

            let price = row
                .price_usd
                .as_deref()
                .and_then(|p| p.parse::<f64>().ok())
                .filter(|p| p.is_finite() && *p > 0.0)
                .unwrap_or_else(|| calc.ticks_to_price(Ticks::new(price_ticks)));

            out.push(TraderStateLimitOrderView {
                subaccount_index,
                symbol: symbol_upper.clone(),
                side: api_side_to_rise_side(row.side),
                order_sequence_number: row.order_sequence_number.clone(),
                order_sequence_number_u64,
                price_ticks,
                price,
                size_remaining_tokens: calc.base_lots_to_units(BaseLots::new(size_remaining_lots)),
                initial_size_tokens: calc.base_lots_to_units(BaseLots::new(initial_size_lots)),
                reduce_only: row.reduce_only,
                is_stop_loss: row.is_stop_loss,
            });
        }
    }

    (out, has_unparsed)
}

struct StateTriggerProjection<'a> {
    out: &'a mut Vec<ConditionalTriggerView>,
    seen: &'a mut HashSet<(String, TriggerKind, String)>,
    metadata: &'a PhoenixMetadata,
    position_lots_by_symbol: &'a HashMap<String, u64>,
    subaccount_index: u8,
}

impl StateTriggerProjection<'_> {
    fn append(&mut self, symbol: &str, kind: TriggerKind, rows: &[TraderStateTriggerRow]) {
        let symbol_upper = symbol.to_ascii_uppercase();
        let Some(calc) = self.metadata.get_market_calculator(&symbol_upper) else {
            return;
        };
        let position_lots = self
            .position_lots_by_symbol
            .get(&symbol_upper)
            .copied()
            .unwrap_or_default();

        for row in rows {
            if !trigger_status_is_open(&row.status) {
                continue;
            }

            let order_id = row
                .order_id(kind)
                .map(str::to_string)
                .unwrap_or_else(|| fallback_state_order_id(&symbol_upper, kind, &row.trigger));
            if !self
                .seen
                .insert((symbol_upper.clone(), kind, order_id.clone()))
            {
                continue;
            }

            let Some(trigger_price_ticks) = parse_u64(&row.trigger.trigger_price_ticks) else {
                continue;
            };
            let Some(size_lots) = state_trigger_size_lots(row, position_lots) else {
                continue;
            };

            self.out.push(ConditionalTriggerView {
                subaccount_index: self.subaccount_index,
                symbol: symbol_upper.clone(),
                kind,
                trigger_price: calc.ticks_to_price(Ticks::new(trigger_price_ticks)),
                size_tokens: calc.base_lots_to_units(BaseLots::new(size_lots)),
                side: match row.trigger.side {
                    phoenix_rise::types::prelude::Side::Bid => StopLossTradeSide::Bid,
                    phoenix_rise::types::prelude::Side::Ask => StopLossTradeSide::Ask,
                },
                order_id,
            });
        }
    }
}

fn api_side_to_rise_side(side: phoenix_rise::types::prelude::Side) -> RiseSide {
    match side {
        phoenix_rise::types::prelude::Side::Bid => RiseSide::Bid,
        phoenix_rise::types::prelude::Side::Ask => RiseSide::Ask,
    }
}

impl TraderStateTriggerRow {
    fn order_id(&self, kind: TriggerKind) -> Option<&str> {
        match kind {
            TriggerKind::TakeProfit => self
                .conditional_take_profit_id
                .as_deref()
                .or(self.take_profit_id.as_deref()),
            TriggerKind::StopLoss => self
                .conditional_stop_loss_id
                .as_deref()
                .or(self.stop_loss_id.as_deref()),
        }
    }
}

fn fallback_state_order_id(
    symbol: &str,
    kind: TriggerKind,
    trigger: &TraderStateTrigger,
) -> String {
    let kind_label = match kind {
        TriggerKind::TakeProfit => "tp",
        TriggerKind::StopLoss => "sl",
    };
    format!(
        "state:{}:{}:{}",
        symbol, kind_label, trigger.trigger_price_ticks
    )
}

fn trigger_status_is_open(status: &str) -> bool {
    let status = status.trim();
    status.is_empty()
        || status.eq_ignore_ascii_case("active")
        || status.eq_ignore_ascii_case("open")
}

fn state_trigger_size_lots(row: &TraderStateTriggerRow, position_lots: u64) -> Option<u64> {
    if let Some(fillable_size_lots) = row
        .trigger
        .fillable_size_lots
        .as_deref()
        .and_then(parse_u64)
    {
        let filled_size_lots = row
            .trigger
            .filled_size_lots
            .as_deref()
            .and_then(parse_u64)
            .unwrap_or_default();
        let remaining = fillable_size_lots.saturating_sub(filled_size_lots);
        if remaining > 0 {
            return Some(remaining);
        }
    }

    if let Some(max_size_lots) = row.trigger.max_size_lots.as_deref().and_then(parse_u64) {
        let filled_size_lots = row
            .trigger
            .filled_size_lots
            .as_deref()
            .and_then(parse_u64)
            .unwrap_or_default();
        let remaining = max_size_lots.saturating_sub(filled_size_lots);
        if remaining > 0 {
            return Some(remaining);
        }
    }

    if row.trigger.use_percent.unwrap_or(false) {
        if let Some(percent) = row.trigger.percent.filter(|p| (1..=100).contains(p)) {
            let lots = position_lots.saturating_mul(percent) / 100;
            if lots > 0 {
                return Some(lots);
            }
        }
    }

    (position_lots > 0).then_some(position_lots)
}

fn parse_u64(value: &str) -> Option<u64> {
    value.parse().ok()
}

fn parse_abs_lots(value: &str) -> Option<u64> {
    value.parse::<i128>().ok().map(|v| {
        let lots = v.unsigned_abs();
        lots.min(u64::MAX as u128) as u64
    })
}

/// Convert projected triggers into `OrderInfo` entries for merging into
/// `vulcan_trade_orders` output.
pub fn triggers_to_order_infos(
    triggers: &[ConditionalTriggerView],
    symbol_filter: Option<&str>,
) -> Vec<OrderInfo> {
    let filter_upper = symbol_filter.map(|s| s.to_ascii_uppercase());
    triggers
        .iter()
        .filter(|t| match &filter_upper {
            Some(s) => &t.symbol == s,
            None => true,
        })
        .map(|t| OrderInfo {
            subaccount_index: (t.subaccount_index > 0).then_some(t.subaccount_index),
            symbol: t.symbol.clone(),
            side: match t.side {
                StopLossTradeSide::Bid => "Buy".to_string(),
                StopLossTradeSide::Ask => "Sell".to_string(),
            },
            order_id: t.order_id.clone(),
            price: format_price(t.trigger_price),
            size_remaining: format_size(t.size_tokens),
            initial_size: format_size(t.size_tokens),
            reduce_only: true,
            is_stop_loss: matches!(t.kind, TriggerKind::StopLoss),
            is_take_profit: matches!(t.kind, TriggerKind::TakeProfit),
        })
        .collect()
}

pub fn trader_state_limit_orders_to_order_infos(
    orders: &[TraderStateLimitOrderView],
    symbol_filter: Option<&str>,
) -> Vec<OrderInfo> {
    let filter_upper = symbol_filter.map(|s| s.to_ascii_uppercase());
    orders
        .iter()
        .filter(|o| match &filter_upper {
            Some(s) => &o.symbol == s,
            None => true,
        })
        .map(|o| OrderInfo {
            subaccount_index: (o.subaccount_index > 0).then_some(o.subaccount_index),
            symbol: o.symbol.clone(),
            side: match o.side {
                RiseSide::Bid => "Buy".to_string(),
                RiseSide::Ask => "Sell".to_string(),
            },
            order_id: o.order_sequence_number.clone(),
            price: format_price(o.price),
            size_remaining: format_size(o.size_remaining_tokens),
            initial_size: format_size(o.initial_size_tokens),
            reduce_only: o.reduce_only,
            is_stop_loss: o.is_stop_loss,
            is_take_profit: false,
        })
        .collect()
}

fn format_price(p: f64) -> String {
    format!("{:.6}", p)
}

fn format_size(s: f64) -> String {
    format!("{:.6}", s)
}

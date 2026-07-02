//! Temporary v1 trader-state HTTP support.
//!
//! This module is a workaround until `phoenix-rise`'s Rust SDK exposes
//! `GET /v1/trader/state/{authority}` directly. Keep endpoint-specific
//! deserialization and fetch logic here so callers can be swapped to the SDK
//! API once `phoenix-rise` adds this route.

use crate::commands::conditional_orders::{
    project_all_trader_state_order_data, project_trader_state_order_data_for_subaccount,
    ConditionalTriggerView, TraderStateLimitOrderView, TraderStateOrderData,
};
use crate::context::AppContext;
use crate::error::VulcanError;
use phoenix_rise::api::{
    decimal_from_signed_base_lots, LimitOrder as SdkLimitOrder, PhoenixMetadata,
    Position as SdkPosition, SubaccountState, Trader, TraderKey,
};
use phoenix_rise::math::{
    PerpAssetMetadata, QuoteLots, SignedBaseLots, SignedQuoteLots, Ticks, TraderPortfolio,
};
use phoenix_rise::types::prelude::{
    CooldownStatus, Decimal as UiDecimal, MarketStatsUpdate, Side as ApiSide,
    TraderStateCapabilities,
};
use serde::{Deserialize, Serialize};
use solana_pubkey::Pubkey;
use std::collections::{BTreeSet, HashMap};

const TRADER_STATE_PATH_PREFIX: &str = "/v1/trader/state";

#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct TraderStateResponse {
    #[serde(default)]
    pub(crate) trader_pda_index: u8,
    #[serde(default)]
    pub(crate) slot: u64,
    pub(crate) snapshot: TraderStateSnapshot,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct TraderStateSnapshot {
    #[serde(default)]
    pub(crate) capabilities: Option<TraderStateCapabilities>,
    #[serde(default)]
    pub(crate) maker_fee_override_multiplier: f64,
    #[serde(default)]
    pub(crate) taker_fee_override_multiplier: f64,
    #[serde(default)]
    pub(crate) subaccounts: Vec<TraderStateSubaccount>,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct TraderStateSubaccount {
    pub(crate) subaccount_index: u8,
    #[serde(default)]
    pub(crate) sequence: u64,
    #[serde(default)]
    pub(crate) collateral: String,
    #[serde(default)]
    pub(crate) capabilities: Option<TraderStateCapabilities>,
    #[serde(default)]
    pub(crate) cooldown_status: Option<CooldownStatus>,
    #[serde(default)]
    pub(crate) positions: Vec<TraderStatePosition>,
    #[serde(default)]
    pub(crate) orders: Vec<TraderStateMarketOrders>,
    #[serde(default)]
    pub(crate) triggers: Vec<TraderStatePositionTriggers>,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct TraderStatePosition {
    pub(crate) symbol: String,
    pub(crate) base_position_lots: String,
    #[serde(default)]
    pub(crate) entry_price_ticks: String,
    #[serde(default)]
    pub(crate) entry_price_usd: String,
    #[serde(default)]
    pub(crate) virtual_quote_position_lots: String,
    #[serde(default)]
    pub(crate) unsettled_funding_quote_lots: String,
    #[serde(default)]
    pub(crate) accumulated_funding_quote_lots: String,
    #[serde(default)]
    pub(crate) take_profit_triggers: Vec<TraderStateTriggerRow>,
    #[serde(default)]
    pub(crate) stop_loss_triggers: Vec<TraderStateTriggerRow>,
    #[serde(default)]
    pub(crate) conditional_take_profit_triggers: Vec<TraderStateTriggerRow>,
    #[serde(default)]
    pub(crate) conditional_stop_loss_triggers: Vec<TraderStateTriggerRow>,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct TraderStatePositionTriggers {
    pub(crate) symbol: String,
    #[serde(default)]
    pub(crate) take_profit_triggers: Vec<TraderStateTriggerRow>,
    #[serde(default)]
    pub(crate) stop_loss_triggers: Vec<TraderStateTriggerRow>,
    #[serde(default)]
    pub(crate) conditional_take_profit_triggers: Vec<TraderStateTriggerRow>,
    #[serde(default)]
    pub(crate) conditional_stop_loss_triggers: Vec<TraderStateTriggerRow>,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct TraderStateMarketOrders {
    pub(crate) symbol: String,
    #[serde(default)]
    pub(crate) orders: Vec<TraderStateLimitOrderRow>,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct TraderStateLimitOrderRow {
    pub(crate) order_sequence_number: String,
    pub(crate) side: ApiSide,
    pub(crate) price_ticks: String,
    #[serde(default)]
    pub(crate) price_usd: Option<String>,
    pub(crate) size_remaining_lots: String,
    pub(crate) initial_size_lots: String,
    #[serde(default)]
    pub(crate) reduce_only: bool,
    #[serde(default)]
    pub(crate) is_stop_loss: bool,
    #[serde(default)]
    pub(crate) status: String,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct TraderStateTriggerRow {
    #[serde(default)]
    pub(crate) take_profit_id: Option<String>,
    #[serde(default)]
    pub(crate) stop_loss_id: Option<String>,
    #[serde(default)]
    pub(crate) conditional_take_profit_id: Option<String>,
    #[serde(default)]
    pub(crate) conditional_stop_loss_id: Option<String>,
    pub(crate) trigger: TraderStateTrigger,
    #[serde(default)]
    pub(crate) status: String,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct TraderStateTrigger {
    pub(crate) trigger_price_ticks: String,
    pub(crate) side: ApiSide,
    #[serde(default)]
    pub(crate) max_size_lots: Option<String>,
    #[serde(default)]
    pub(crate) fillable_size_lots: Option<String>,
    #[serde(default)]
    pub(crate) filled_size_lots: Option<String>,
    #[serde(default)]
    pub(crate) use_percent: Option<bool>,
    #[serde(default)]
    pub(crate) percent: Option<u64>,
}

impl TraderStateResponse {
    pub(crate) fn find_position(
        &self,
        symbol: &str,
    ) -> Option<(&TraderStateSubaccount, &TraderStatePosition)> {
        self.snapshot.find_position(symbol)
    }

    pub(crate) fn find_position_in_subaccount(
        &self,
        subaccount_index: u8,
        symbol: &str,
    ) -> Option<(&TraderStateSubaccount, &TraderStatePosition)> {
        self.snapshot
            .find_position_in_subaccount(subaccount_index, symbol)
    }

    pub(crate) fn find_subaccount(&self, subaccount_index: u8) -> Option<&TraderStateSubaccount> {
        self.snapshot.find_subaccount(subaccount_index)
    }

    pub(crate) fn has_cross_margin_subaccount(&self) -> bool {
        self.find_subaccount(0).is_some()
    }

    pub(crate) fn to_sdk_trader_for_isolated_builder(&self, authority: Pubkey) -> Trader {
        let mut trader = Trader::new(TraderKey::new_with_idx(authority, self.trader_pda_index, 0));
        trader.last_slot = self.slot;
        trader.capabilities = self.snapshot.capabilities.clone();
        trader.maker_fee_override_multiplier = self.snapshot.maker_fee_override_multiplier;
        trader.taker_fee_override_multiplier = self.snapshot.taker_fee_override_multiplier;

        for subaccount in &self.snapshot.subaccounts {
            trader.subaccounts.insert(
                subaccount.subaccount_index,
                subaccount.to_sdk_subaccount_state(),
            );
        }

        trader
    }
}

impl TraderStateSnapshot {
    pub(crate) fn find_position(
        &self,
        symbol: &str,
    ) -> Option<(&TraderStateSubaccount, &TraderStatePosition)> {
        let symbol_upper = symbol.to_ascii_uppercase();
        self.subaccounts.iter().find_map(|subaccount| {
            subaccount
                .positions
                .iter()
                .find(|position| position.symbol.to_ascii_uppercase() == symbol_upper)
                .map(|position| (subaccount, position))
        })
    }

    pub(crate) fn find_position_in_subaccount(
        &self,
        subaccount_index: u8,
        symbol: &str,
    ) -> Option<(&TraderStateSubaccount, &TraderStatePosition)> {
        let symbol_upper = symbol.to_ascii_uppercase();
        self.find_subaccount(subaccount_index)
            .and_then(|subaccount| {
                subaccount
                    .positions
                    .iter()
                    .find(|position| position.symbol.to_ascii_uppercase() == symbol_upper)
                    .map(|position| (subaccount, position))
            })
    }

    pub(crate) fn find_subaccount(&self, subaccount_index: u8) -> Option<&TraderStateSubaccount> {
        self.subaccounts
            .iter()
            .find(|subaccount| subaccount.subaccount_index == subaccount_index)
    }

    fn referenced_symbols(&self) -> BTreeSet<String> {
        let mut symbols = BTreeSet::new();
        for subaccount in &self.subaccounts {
            for position in &subaccount.positions {
                symbols.insert(position.symbol.to_ascii_uppercase());
            }
            for orders in &subaccount.orders {
                symbols.insert(orders.symbol.to_ascii_uppercase());
            }
            for triggers in &subaccount.triggers {
                symbols.insert(triggers.symbol.to_ascii_uppercase());
            }
        }
        symbols
    }
}

impl TraderStateSubaccount {
    pub(crate) fn has_positions_except(&self, symbol: &str) -> bool {
        let symbol_upper = symbol.to_ascii_uppercase();
        self.positions.iter().any(|position| {
            position.symbol.to_ascii_uppercase() != symbol_upper && position.base_lots() != 0
        })
    }

    pub(crate) fn has_open_order_like_rows(&self) -> bool {
        count_open_limit_orders(self) > 0 || count_open_conditional_orders(self) > 0
    }

    pub(crate) fn sweep_blocker(&self, closing_symbol: Option<&str>) -> Option<String> {
        if let Some(symbol) = closing_symbol {
            if self.has_positions_except(symbol) {
                return Some("subaccount has another open position".to_string());
            }
        } else if self
            .positions
            .iter()
            .any(|position| position.base_lots() != 0)
        {
            return Some("subaccount has an open position".to_string());
        }

        if self.has_open_order_like_rows() {
            return Some("subaccount has open limit or conditional orders".to_string());
        }

        None
    }

    fn to_sdk_subaccount_state(&self) -> SubaccountState {
        let mut subaccount = SubaccountState {
            subaccount_index: self.subaccount_index,
            sequence: self.sequence,
            collateral: SignedQuoteLots::new(parse_i64(&self.collateral)),
            capabilities: self.capabilities.clone(),
            cooldown_status: self.cooldown_status.clone(),
            ..Default::default()
        };

        for position in &self.positions {
            subaccount
                .positions
                .insert(position.symbol.clone(), position.to_sdk_position());
        }

        for market_orders in &self.orders {
            for order in &market_orders.orders {
                let osn = parse_u64(&order.order_sequence_number);
                subaccount.orders.insert(
                    (market_orders.symbol.clone(), osn),
                    order.to_sdk_limit_order(&market_orders.symbol),
                );
            }
        }

        subaccount
    }
}

impl TraderStatePosition {
    pub(crate) fn base_lots(&self) -> i64 {
        parse_i64(&self.base_position_lots)
    }

    pub(crate) fn abs_base_lots(&self) -> u64 {
        self.base_lots().unsigned_abs()
    }

    pub(crate) fn is_long(&self) -> bool {
        self.base_lots() >= 0
    }

    fn to_sdk_position(&self) -> SdkPosition {
        SdkPosition {
            symbol: self.symbol.clone(),
            position_sequence_number: 0,
            base_position_lots: self.base_lots(),
            entry_price_ticks: parse_i64(&self.entry_price_ticks),
            entry_price_usd: self.entry_price_usd.parse().unwrap_or_default(),
            virtual_quote_position_lots: parse_i64(&self.virtual_quote_position_lots),
            unsettled_funding_quote_lots: parse_i64(&self.unsettled_funding_quote_lots),
            accumulated_funding_quote_lots: parse_i64(&self.accumulated_funding_quote_lots),
            take_profit_triggers: Vec::new(),
            stop_loss_triggers: Vec::new(),
        }
    }
}

impl TraderStateLimitOrderRow {
    fn to_sdk_limit_order(&self, symbol: &str) -> SdkLimitOrder {
        SdkLimitOrder {
            symbol: symbol.to_string(),
            order_sequence_number: parse_u64(&self.order_sequence_number),
            side: format!("{:?}", self.side),
            order_type: String::new(),
            conditional_kind: None,
            price_ticks: parse_i64(&self.price_ticks),
            price_usd: self
                .price_usd
                .as_deref()
                .and_then(|price| price.parse().ok())
                .unwrap_or_default(),
            size_remaining_lots: parse_u64(&self.size_remaining_lots),
            initial_size_lots: parse_u64(&self.initial_size_lots),
            reduce_only: self.reduce_only,
            is_stop_loss: self.is_stop_loss,
            status: self.status.clone(),
        }
    }
}

fn parse_i64(value: &str) -> i64 {
    value.parse().unwrap_or_default()
}

fn parse_u64(value: &str) -> u64 {
    value.parse().unwrap_or_default()
}

fn parse_open_u64(value: &str) -> Option<u64> {
    value.parse().ok()
}

fn status_is_open(status: &str) -> bool {
    let status = status.trim();
    status.is_empty()
        || status.eq_ignore_ascii_case("active")
        || status.eq_ignore_ascii_case("open")
}

#[derive(Debug, Clone)]
#[allow(dead_code)]
pub(crate) struct ComputedTraderView {
    pub(crate) flags: u16,
    pub(crate) state: String,
    pub(crate) trader_key: String,
    pub(crate) trader_pda_index: u8,
    pub(crate) trader_subaccount_index: u8,
    pub(crate) authority: String,
    pub(crate) collateral_balance: UiDecimal,
    pub(crate) effective_collateral: UiDecimal,
    pub(crate) effective_collateral_for_withdrawals: UiDecimal,
    pub(crate) unrealized_pnl: UiDecimal,
    pub(crate) discounted_unrealized_pnl: UiDecimal,
    pub(crate) unsettled_funding_owed: UiDecimal,
    pub(crate) accumulated_funding: UiDecimal,
    pub(crate) portfolio_value: UiDecimal,
    pub(crate) maintenance_margin: UiDecimal,
    pub(crate) cancel_margin: UiDecimal,
    pub(crate) initial_margin: UiDecimal,
    pub(crate) initial_margin_for_withdrawals: UiDecimal,
    pub(crate) risk_state: String,
    pub(crate) risk_tier: String,
    pub(crate) positions: Vec<ComputedPositionView>,
    pub(crate) limit_orders: Vec<TraderStateLimitOrderView>,
    pub(crate) conditional_triggers: Vec<ConditionalTriggerView>,
    pub(crate) num_open_limit_orders: usize,
    pub(crate) num_open_conditional_orders: usize,
    pub(crate) maker_fee_override_multiplier: f64,
    pub(crate) taker_fee_override_multiplier: f64,
    pub(crate) max_positions: u64,
    pub(crate) last_deposit_slot: u64,
    pub(crate) is_in_active_traders: bool,
}

#[derive(Debug, Clone)]
#[allow(dead_code)]
pub(crate) struct ComputedPositionView {
    pub(crate) symbol: String,
    pub(crate) position_size: UiDecimal,
    pub(crate) virtual_quote_position: UiDecimal,
    pub(crate) entry_price: UiDecimal,
    pub(crate) unrealized_pnl: UiDecimal,
    pub(crate) discounted_unrealized_pnl: UiDecimal,
    pub(crate) position_value: UiDecimal,
    pub(crate) initial_margin: UiDecimal,
    pub(crate) maintenance_margin: UiDecimal,
    pub(crate) liquidation_price: UiDecimal,
    pub(crate) take_profit_price: Option<UiDecimal>,
    pub(crate) stop_loss_price: Option<UiDecimal>,
    pub(crate) unsettled_funding: UiDecimal,
    pub(crate) accumulated_funding: UiDecimal,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
struct TraderStateQuery {
    pda_index: u8,
}

#[derive(Debug, Clone)]
pub(crate) struct TraderStateBundle {
    pub(crate) state: TraderStateResponse,
    pub(crate) views: Vec<ComputedTraderView>,
    pub(crate) order_data: TraderStateOrderData,
}

pub(crate) async fn fetch_computed_trader_views(
    ctx: &AppContext,
    authority: &Pubkey,
) -> Result<Vec<ComputedTraderView>, VulcanError> {
    Ok(fetch_trader_state_bundle(ctx, authority, 0).await?.views)
}

pub(crate) async fn fetch_trader_state_bundle(
    ctx: &AppContext,
    authority: &Pubkey,
    pda_index: u8,
) -> Result<TraderStateBundle, VulcanError> {
    let state = fetch_trader_state_snapshot(ctx, authority, pda_index).await?;
    compute_trader_state_bundle(ctx, *authority, state).await
}

async fn compute_trader_state_bundle(
    ctx: &AppContext,
    authority: Pubkey,
    state: TraderStateResponse,
) -> Result<TraderStateBundle, VulcanError> {
    let mut metadata = ctx.metadata().await?.clone();
    hydrate_metadata_for_trader_state(ctx, &mut metadata, &state).await?;
    let views = compute_trader_views_from_state(&authority, &state, &metadata)?;
    let order_data = project_all_trader_state_order_data(&state.snapshot, &metadata);
    Ok(TraderStateBundle {
        state,
        views,
        order_data,
    })
}

pub(crate) async fn fetch_trader_state_snapshot(
    ctx: &AppContext,
    authority: &Pubkey,
    pda_index: u8,
) -> Result<TraderStateResponse, VulcanError> {
    ctx.http_client
        .get_json_with_query(
            &trader_state_path(authority),
            &TraderStateQuery { pda_index },
        )
        .await
        .map_err(|e| VulcanError::api("TRADER_STATE_FETCH_FAILED", e.to_string()))
}

pub(crate) async fn try_fetch_trader_state_snapshot(
    ctx: &AppContext,
    authority: &Pubkey,
    pda_index: u8,
) -> Result<Option<TraderStateResponse>, VulcanError> {
    match ctx
        .http_client
        .get_json_with_query(
            &trader_state_path(authority),
            &TraderStateQuery { pda_index },
        )
        .await
    {
        Ok(state) => Ok(Some(state)),
        Err(err) if err.status() == Some(404) => Ok(None),
        Err(err) => Err(VulcanError::api(
            "TRADER_STATE_FETCH_FAILED",
            err.to_string(),
        )),
    }
}

fn trader_state_path(authority: &Pubkey) -> String {
    format!("{TRADER_STATE_PATH_PREFIX}/{authority}")
}

async fn hydrate_metadata_for_trader_state(
    ctx: &AppContext,
    metadata: &mut PhoenixMetadata,
    state: &TraderStateResponse,
) -> Result<(), VulcanError> {
    let missing_symbols: Vec<String> = state
        .snapshot
        .referenced_symbols()
        .into_iter()
        .filter(|symbol| !metadata.has_perp_asset_metadata(symbol))
        .collect();

    let mut unresolved = Vec::new();
    for symbol in &missing_symbols {
        let price = match ctx.http_client.markets().get_mark_price(symbol).await {
            Ok(response) => response.mark_price.map(|p| p.price),
            Err(err) => {
                unresolved.push(format!("{}: {}", symbol, err));
                continue;
            }
        };
        let Some(price) = price else {
            unresolved.push(format!("{}: mark price unavailable", symbol));
            continue;
        };
        let stats = MarketStatsUpdate {
            symbol: symbol.clone(),
            open_interest: 0.0,
            mark_price: price,
            mid_price: price,
            oracle_price: price,
            prev_day_mark_price: price,
            day_volume_usd: 0.0,
            funding_rate: 0.0,
        };
        if let Err(err) = metadata.apply_market_stats(&stats) {
            unresolved.push(format!("{}: {}", symbol, err));
        }
    }

    if !unresolved.is_empty() {
        ctx.hydrate_metadata_from_perp_asset_map_rpc(metadata)
            .await
            .map_err(|rpc_err| {
                VulcanError::api(
                    "MARKET_METADATA_HYDRATION_FAILED",
                    format!(
                        "Failed to hydrate mark prices via API ({}) and RPC fallback failed: {}",
                        unresolved.join("; "),
                        rpc_err
                    ),
                )
            })?;
    }

    let still_missing: Vec<_> = missing_symbols
        .into_iter()
        .filter(|symbol| !metadata.has_perp_asset_metadata(symbol))
        .collect();
    if !still_missing.is_empty() {
        return Err(VulcanError::api(
            "MARKET_METADATA_HYDRATION_FAILED",
            format!(
                "Missing market metadata for symbols: {}",
                still_missing.join(", ")
            ),
        ));
    }

    Ok(())
}

pub(crate) fn compute_trader_views_from_state(
    authority: &Pubkey,
    state: &TraderStateResponse,
    metadata: &PhoenixMetadata,
) -> Result<Vec<ComputedTraderView>, VulcanError> {
    let mut out = Vec::new();
    for subaccount in &state.snapshot.subaccounts {
        let sdk_subaccount = subaccount.to_sdk_subaccount_state();
        let mut portfolio = sdk_subaccount.to_trader_portfolio();
        portfolio.authority = *authority;
        portfolio.trader_pda_index = state.trader_pda_index;
        portfolio.trader_subaccount_index = subaccount.subaccount_index;

        let margin = portfolio
            .compute_margin(metadata.all_perp_asset_metadata())
            .map_err(|e| VulcanError::api("TRADER_VIEW_COMPUTE_FAILED", e.to_string()))?;

        let capabilities = subaccount
            .capabilities
            .as_ref()
            .or(state.snapshot.capabilities.as_ref());
        let state_label = capabilities
            .map(|c| c.state.clone())
            .unwrap_or_else(|| "unknown".to_string());
        let flags = capabilities.map(|c| c.flags).unwrap_or_default();
        let trader_key = TraderKey::new_with_idx(
            *authority,
            state.trader_pda_index,
            subaccount.subaccount_index,
        );
        let positions = compute_position_views(subaccount, &portfolio, &margin, metadata);
        let order_data = project_trader_state_order_data_for_subaccount(
            &state.snapshot,
            subaccount.subaccount_index,
            metadata,
        );
        let num_open_limit_orders = order_data.limit_orders.len();
        let num_open_conditional_orders = order_data.conditional_triggers.len();
        let last_deposit_slot = subaccount
            .cooldown_status
            .as_ref()
            .map(|s| s.last_deposit_slot)
            .unwrap_or_default();

        out.push(ComputedTraderView {
            flags,
            state: state_label.clone(),
            trader_key: trader_key.pda().to_string(),
            trader_pda_index: state.trader_pda_index,
            trader_subaccount_index: subaccount.subaccount_index,
            authority: authority.to_string(),
            collateral_balance: signed_quote_decimal(margin.quote_lot_collateral),
            effective_collateral: signed_quote_decimal(margin.effective_collateral()),
            effective_collateral_for_withdrawals: signed_quote_decimal(
                margin.effective_collateral_for_withdrawals(),
            ),
            unrealized_pnl: signed_quote_decimal(margin.margin.unrealized_pnl),
            discounted_unrealized_pnl: signed_quote_decimal(
                margin.margin.discounted_unrealized_pnl,
            ),
            unsettled_funding_owed: signed_quote_decimal(margin.margin.unsettled_funding),
            accumulated_funding: signed_quote_decimal(margin.margin.accumulated_funding),
            portfolio_value: signed_quote_decimal(margin.portfolio_value()),
            maintenance_margin: quote_decimal(margin.margin.maintenance_margin),
            cancel_margin: quote_decimal(margin.margin.cancel_margin),
            initial_margin: quote_decimal(margin.margin.initial_margin),
            initial_margin_for_withdrawals: quote_decimal(
                margin.margin.initial_margin_for_withdrawals,
            ),
            risk_state: margin
                .risk_state()
                .map(|state| format!("{state:?}"))
                .unwrap_or_else(|e| format!("Unknown({e})")),
            risk_tier: margin
                .risk_tier()
                .map(|tier| format!("{tier:?}"))
                .unwrap_or_else(|e| format!("Unknown({e})")),
            positions,
            limit_orders: order_data.limit_orders,
            conditional_triggers: order_data.conditional_triggers,
            num_open_limit_orders,
            num_open_conditional_orders,
            maker_fee_override_multiplier: state.snapshot.maker_fee_override_multiplier,
            taker_fee_override_multiplier: state.snapshot.taker_fee_override_multiplier,
            max_positions: 0,
            last_deposit_slot,
            is_in_active_traders: !state_label.eq_ignore_ascii_case("cold"),
        });
    }

    out.sort_by_key(|view| view.trader_subaccount_index);
    Ok(out)
}

fn compute_position_views(
    subaccount: &TraderStateSubaccount,
    portfolio: &TraderPortfolio,
    margin: &phoenix_rise::math::TraderPortfolioMargin,
    metadata: &PhoenixMetadata,
) -> Vec<ComputedPositionView> {
    let mut out = Vec::new();
    for position in &subaccount.positions {
        let symbol_upper = position.symbol.to_ascii_uppercase();
        let market_margin = margin
            .positions
            .get(&position.symbol)
            .or_else(|| margin.positions.get(&symbol_upper));
        let position_margin = market_margin.map(|m| m.margin).unwrap_or_default();
        let position_size = metadata
            .get_perp_asset_metadata(&symbol_upper)
            .map(|asset| {
                decimal_from_signed_base_lots(
                    SignedBaseLots::new(position.base_lots()),
                    asset.base_lot_decimals(),
                )
            })
            .unwrap_or_else(|| UiDecimal::from_i64_with_decimals(position.base_lots(), 0));

        out.push(ComputedPositionView {
            symbol: symbol_upper.clone(),
            position_size,
            virtual_quote_position: signed_quote_decimal(SignedQuoteLots::new(parse_i64(
                &position.virtual_quote_position_lots,
            ))),
            entry_price: price_decimal_from_str(&position.entry_price_usd),
            unrealized_pnl: signed_quote_decimal(position_margin.unrealized_pnl),
            discounted_unrealized_pnl: signed_quote_decimal(
                position_margin.discounted_unrealized_pnl,
            ),
            position_value: signed_quote_decimal(position_margin.position_value),
            initial_margin: quote_decimal(position_margin.initial_margin),
            maintenance_margin: quote_decimal(position_margin.maintenance_margin),
            liquidation_price: liquidation_price_decimal(
                portfolio,
                metadata,
                &symbol_upper,
                position.is_long(),
            )
            .unwrap_or_else(|| UiDecimal::from_i64_with_decimals(-1, 0)),
            take_profit_price: first_trigger_price_decimal(
                metadata,
                &symbol_upper,
                &position.take_profit_triggers,
            )
            .or_else(|| {
                first_trigger_price_decimal(
                    metadata,
                    &symbol_upper,
                    &position.conditional_take_profit_triggers,
                )
            }),
            stop_loss_price: first_trigger_price_decimal(
                metadata,
                &symbol_upper,
                &position.stop_loss_triggers,
            )
            .or_else(|| {
                first_trigger_price_decimal(
                    metadata,
                    &symbol_upper,
                    &position.conditional_stop_loss_triggers,
                )
            }),
            unsettled_funding: signed_quote_decimal(SignedQuoteLots::new(parse_i64(
                &position.unsettled_funding_quote_lots,
            ))),
            accumulated_funding: signed_quote_decimal(SignedQuoteLots::new(parse_i64(
                &position.accumulated_funding_quote_lots,
            ))),
        });
    }
    out.sort_by(|a, b| a.symbol.cmp(&b.symbol));
    out
}

fn liquidation_price_decimal(
    portfolio: &TraderPortfolio,
    metadata: &PhoenixMetadata,
    symbol: &str,
    is_long: bool,
) -> Option<UiDecimal> {
    let symbol_upper = symbol.to_ascii_uppercase();
    let asset = metadata.get_perp_asset_metadata(&symbol_upper)?;
    let calc = metadata.get_market_calculator(&symbol_upper)?;
    let current_ticks = asset.mark_price.as_inner();
    if current_ticks == 0 {
        return None;
    }

    if portfolio_is_below_maintenance(
        portfolio,
        metadata.all_perp_asset_metadata(),
        &symbol_upper,
        current_ticks,
    )? {
        return Some(price_decimal_from_f64(
            calc.ticks_to_price(Ticks::new(current_ticks)),
        ));
    }

    let liquidation_ticks = if is_long {
        if !portfolio_is_below_maintenance(
            portfolio,
            metadata.all_perp_asset_metadata(),
            &symbol_upper,
            1,
        )? {
            return None;
        }
        let mut low = 1_u64;
        let mut high = current_ticks;
        while low + 1 < high {
            let mid = low + (high - low) / 2;
            if portfolio_is_below_maintenance(
                portfolio,
                metadata.all_perp_asset_metadata(),
                &symbol_upper,
                mid,
            )? {
                low = mid;
            } else {
                high = mid;
            }
        }
        high
    } else {
        let max_ticks = u32::MAX as u64;
        let mut high = current_ticks
            .saturating_mul(2)
            .max(current_ticks + 1)
            .min(max_ticks);
        let mut found = false;
        for _ in 0..48 {
            if portfolio_is_below_maintenance(
                portfolio,
                metadata.all_perp_asset_metadata(),
                &symbol_upper,
                high,
            )? {
                found = true;
                break;
            }
            let next = high.saturating_mul(2).min(max_ticks);
            if next == high {
                return None;
            }
            high = next;
        }
        if !found {
            return None;
        }
        let mut low = current_ticks;
        while low + 1 < high {
            let mid = low + (high - low) / 2;
            if portfolio_is_below_maintenance(
                portfolio,
                metadata.all_perp_asset_metadata(),
                &symbol_upper,
                mid,
            )? {
                high = mid;
            } else {
                low = mid;
            }
        }
        high
    };

    Some(price_decimal_from_f64(
        calc.ticks_to_price(Ticks::new(liquidation_ticks)),
    ))
}

fn portfolio_is_below_maintenance(
    portfolio: &TraderPortfolio,
    base_metadata: &HashMap<String, PerpAssetMetadata>,
    symbol: &str,
    mark_price_ticks: u64,
) -> Option<bool> {
    let mut metadata = base_metadata.clone();
    let asset = metadata.get_mut(symbol)?;
    asset.set_mark_price(Ticks::new(mark_price_ticks));
    let margin = portfolio.compute_margin(&metadata).ok()?;
    let maintenance = SignedQuoteLots::new(margin.margin.maintenance_margin.as_inner() as i64);
    Some(margin.effective_collateral() <= maintenance)
}

fn count_open_limit_orders(subaccount: &TraderStateSubaccount) -> usize {
    subaccount
        .orders
        .iter()
        .map(|market_orders| {
            market_orders
                .orders
                .iter()
                .filter(|order| status_is_open(&order.status))
                .count()
        })
        .sum()
}

fn count_open_conditional_orders(subaccount: &TraderStateSubaccount) -> usize {
    let mut count = 0;
    for position in &subaccount.positions {
        count += count_open_trigger_rows(&position.take_profit_triggers);
        count += count_open_trigger_rows(&position.stop_loss_triggers);
        count += count_open_trigger_rows(&position.conditional_take_profit_triggers);
        count += count_open_trigger_rows(&position.conditional_stop_loss_triggers);
    }
    for triggers in &subaccount.triggers {
        count += count_open_trigger_rows(&triggers.take_profit_triggers);
        count += count_open_trigger_rows(&triggers.stop_loss_triggers);
        count += count_open_trigger_rows(&triggers.conditional_take_profit_triggers);
        count += count_open_trigger_rows(&triggers.conditional_stop_loss_triggers);
    }
    count
}

fn count_open_trigger_rows(rows: &[TraderStateTriggerRow]) -> usize {
    rows.iter()
        .filter(|row| status_is_open(&row.status))
        .count()
}

fn first_trigger_price_decimal(
    metadata: &PhoenixMetadata,
    symbol: &str,
    rows: &[TraderStateTriggerRow],
) -> Option<UiDecimal> {
    let calc = metadata.get_market_calculator(symbol)?;
    rows.iter()
        .filter(|row| status_is_open(&row.status))
        .find_map(|row| {
            let ticks = parse_open_u64(&row.trigger.trigger_price_ticks)?;
            Some(price_decimal_from_f64(
                calc.ticks_to_price(Ticks::new(ticks)),
            ))
        })
}

fn signed_quote_decimal(value: SignedQuoteLots) -> UiDecimal {
    UiDecimal::from_i64_with_decimals(value.as_inner(), 0)
}

fn quote_decimal(value: QuoteLots) -> UiDecimal {
    UiDecimal::from_i64_with_decimals(value.as_inner() as i64, 0)
}

fn price_decimal_from_str(value: &str) -> UiDecimal {
    value
        .parse::<f64>()
        .ok()
        .map(price_decimal_from_f64)
        .unwrap_or_else(|| UiDecimal::from_i64_with_decimals(0, 6))
}

fn price_decimal_from_f64(value: f64) -> UiDecimal {
    if !value.is_finite() {
        return UiDecimal::from_i64_with_decimals(-1, 0);
    }
    let scaled = value * 1_000_000.0;
    if scaled > i64::MAX as f64 || scaled < i64::MIN as f64 {
        return UiDecimal::from_i64_with_decimals(-1, 0);
    }
    UiDecimal::from_i64_with_decimals(scaled.round() as i64, 6)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn trader_state_fixture() -> TraderStateResponse {
        serde_json::from_str(
            r#"{
              "traderPdaIndex": 0,
              "slot": 123,
              "snapshot": {
                "subaccounts": [
                  {
                    "subaccountIndex": 0,
                    "collateral": "1000000",
                    "positions": [
                      {
                        "symbol": "SOL",
                        "basePositionLots": "10",
                        "entryPriceTicks": "100",
                        "entryPriceUsd": "10",
                        "virtualQuotePositionLots": "0",
                        "unsettledFundingQuoteLots": "0",
                        "accumulatedFundingQuoteLots": "0"
                      }
                    ]
                  },
                  {
                    "subaccountIndex": 1,
                    "collateral": "500000",
                    "positions": [
                      {
                        "symbol": "SOL",
                        "basePositionLots": "-7",
                        "entryPriceTicks": "100",
                        "entryPriceUsd": "10",
                        "virtualQuotePositionLots": "0",
                        "unsettledFundingQuoteLots": "0",
                        "accumulatedFundingQuoteLots": "0",
                        "conditionalStopLossTriggers": [
                          {
                            "conditionalStopLossId": "csl-1-0-lt",
                            "status": "active",
                            "trigger": {
                              "triggerPriceTicks": "90",
                              "side": "bid",
                              "maxSizeLots": "7"
                            }
                          }
                        ]
                      }
                    ],
                    "orders": [
                      {
                        "symbol": "BTC",
                        "orders": [
                          {
                            "orderSequenceNumber": "42",
                            "side": "ask",
                            "priceTicks": "1000",
                            "sizeRemainingLots": "5",
                            "initialSizeLots": "5",
                            "status": "open"
                          }
                        ]
                      }
                    ],
                    "triggers": [
                      {
                        "symbol": "ETH",
                        "takeProfitTriggers": [
                          {
                            "takeProfitId": "ctp-2-0-gt",
                            "status": "active",
                            "trigger": {
                              "triggerPriceTicks": "120",
                              "side": "ask",
                              "maxSizeLots": "3"
                            }
                          }
                        ]
                      }
                    ]
                  }
                ]
              }
            }"#,
        )
        .expect("fixture should deserialize")
    }

    #[test]
    fn trader_state_find_position_can_target_subaccount() {
        let state = trader_state_fixture();
        let (_, cross_position) = state
            .find_position_in_subaccount(0, "SOL")
            .expect("cross SOL position");
        let (_, isolated_position) = state
            .find_position_in_subaccount(1, "SOL")
            .expect("isolated SOL position");

        assert_eq!(cross_position.base_lots(), 10);
        assert_eq!(isolated_position.base_lots(), -7);
    }

    #[test]
    fn trader_state_referenced_symbols_includes_orders_and_triggers() {
        let state = trader_state_fixture();
        let symbols: Vec<_> = state.snapshot.referenced_symbols().into_iter().collect();

        assert_eq!(symbols, vec!["BTC", "ETH", "SOL"]);
    }

    #[test]
    fn sweep_blocker_detects_open_order_like_rows() {
        let state = trader_state_fixture();
        let subaccount = state.find_subaccount(1).expect("subaccount");

        assert_eq!(
            subaccount.sweep_blocker(Some("SOL")),
            Some("subaccount has open limit or conditional orders".to_string())
        );
    }

    #[test]
    fn trader_state_path_uses_v1_state_endpoint() {
        let authority = Pubkey::new_from_array([7; 32]);

        assert_eq!(
            trader_state_path(&authority),
            format!("/v1/trader/state/{authority}")
        );
    }
}

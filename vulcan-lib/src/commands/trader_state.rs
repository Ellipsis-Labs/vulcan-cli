//! Temporary v1 trader-state HTTP support.
//!
//! This module is a workaround until `phoenix-rise`'s Rust SDK exposes
//! `GET /v1/trader/state/{authority}` directly. Keep endpoint-specific
//! deserialization and fetch logic here so callers can be swapped to the SDK
//! API once `phoenix-rise` adds this route.

use crate::context::AppContext;
use crate::error::VulcanError;
use phoenix_rise::math::Side as RiseSide;
use serde::Deserialize;
use solana_pubkey::Pubkey;

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct TraderStateResponse {
    pub(crate) snapshot: TraderStateSnapshot,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct TraderStateSnapshot {
    #[serde(default)]
    pub(crate) subaccounts: Vec<TraderStateSubaccount>,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct TraderStateSubaccount {
    pub(crate) subaccount_index: u8,
    #[serde(default)]
    pub(crate) positions: Vec<TraderStatePosition>,
    #[serde(default)]
    pub(crate) orders: Vec<TraderStateMarketOrders>,
    #[serde(default)]
    pub(crate) triggers: Vec<TraderStatePositionTriggers>,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct TraderStatePosition {
    pub(crate) symbol: String,
    pub(crate) base_position_lots: String,
    #[serde(default)]
    pub(crate) take_profit_triggers: Vec<TraderStateTriggerRow>,
    #[serde(default)]
    pub(crate) stop_loss_triggers: Vec<TraderStateTriggerRow>,
    #[serde(default)]
    pub(crate) conditional_take_profit_triggers: Vec<TraderStateTriggerRow>,
    #[serde(default)]
    pub(crate) conditional_stop_loss_triggers: Vec<TraderStateTriggerRow>,
}

#[derive(Debug, Deserialize)]
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

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct TraderStateMarketOrders {
    pub(crate) symbol: String,
    #[serde(default)]
    pub(crate) orders: Vec<TraderStateLimitOrderRow>,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct TraderStateLimitOrderRow {
    pub(crate) order_sequence_number: String,
    pub(crate) side: RiseSide,
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

#[derive(Debug, Deserialize)]
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

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct TraderStateTrigger {
    pub(crate) trigger_price_ticks: String,
    pub(crate) side: RiseSide,
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

pub(crate) async fn fetch_trader_state_snapshot(
    ctx: &AppContext,
    authority: &Pubkey,
    pda_index: u8,
) -> Result<TraderStateResponse, VulcanError> {
    let url = format!(
        "{}/v1/trader/state/{}",
        ctx.config.network.api_url.trim_end_matches('/'),
        authority
    );
    let response = ctx
        .raw_http_client
        .get(&url)
        .query(&[("pdaIndex", pda_index.to_string())])
        .send()
        .await
        .map_err(|e| VulcanError::network("TRADER_STATE_FETCH_FAILED", e.to_string()))?;

    let status = response.status();
    let body = response
        .text()
        .await
        .map_err(|e| VulcanError::network("TRADER_STATE_FETCH_FAILED", e.to_string()))?;

    if !status.is_success() {
        return Err(VulcanError::api(
            "TRADER_STATE_FETCH_FAILED",
            format!("{} returned HTTP {}: {}", url, status, body),
        ));
    }

    serde_json::from_str(&body)
        .map_err(|e| VulcanError::api("TRADER_STATE_DECODE_FAILED", e.to_string()))
}

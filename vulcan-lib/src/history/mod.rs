//! Live trader history adapters backed by Phoenix/Rise HTTP APIs.

use crate::context::AppContext;
use crate::error::VulcanError;
use phoenix_rise::types::prelude::{
    CollateralHistoryQueryParams, FundingHistoryQueryParams, OrderHistoryQueryParams,
    PnlQueryParams, PnlResolution, TradeHistoryQueryParams,
};
use serde::Serialize;
use solana_pubkey::Pubkey;
use std::str::FromStr;

#[derive(Debug, Clone, Serialize)]
pub struct HistoryResult {
    pub history_type: String,
    pub symbol: Option<String>,
    pub limit: i64,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub cursor: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub next_cursor: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub prev_cursor: Option<String>,
    pub has_more: bool,
    pub count: usize,
    pub data: serde_json::Value,
}

pub async fn trades(
    ctx: &AppContext,
    symbol: Option<&str>,
    limit: i64,
    cursor: Option<&str>,
) -> Result<HistoryResult, VulcanError> {
    let authority = default_authority(ctx)?;
    let mut params = TradeHistoryQueryParams::new().with_limit(limit);
    if let Some(symbol) = symbol {
        params = params.with_market_symbol(symbol.to_ascii_uppercase());
    }
    if let Some(cursor) = cursor {
        params = params.with_cursor(cursor);
    }
    let response = ctx
        .http_client
        .trades()
        .get_trader_trade_history(&authority, params)
        .await
        .map_err(|e| VulcanError::api("TRADE_HISTORY_FAILED", e.to_string()))?;
    Ok(HistoryResult {
        history_type: "trades".to_string(),
        symbol: symbol.map(|s| s.to_ascii_uppercase()),
        limit,
        cursor: cursor.map(str::to_string),
        next_cursor: response.next_cursor,
        prev_cursor: response.prev_cursor,
        has_more: response.has_more,
        count: response.data.len(),
        data: serde_json::to_value(response.data).unwrap_or(serde_json::Value::Null),
    })
}

pub async fn orders(
    ctx: &AppContext,
    symbol: Option<&str>,
    limit: i64,
    cursor: Option<&str>,
) -> Result<HistoryResult, VulcanError> {
    let authority = default_authority(ctx)?;
    let mut params = OrderHistoryQueryParams::new(limit);
    if let Some(symbol) = symbol {
        params = params.with_market_symbol(symbol.to_ascii_uppercase());
    }
    if let Some(cursor) = cursor {
        params = params.with_cursor(cursor);
    }
    let response = ctx
        .http_client
        .orders()
        .get_trader_order_history(&authority, params)
        .await
        .map_err(|e| VulcanError::api("ORDER_HISTORY_FAILED", e.to_string()))?;
    Ok(HistoryResult {
        history_type: "orders".to_string(),
        symbol: symbol.map(|s| s.to_ascii_uppercase()),
        limit,
        cursor: cursor.map(str::to_string),
        next_cursor: response.next_cursor,
        prev_cursor: response.prev_cursor,
        has_more: response.has_more,
        count: response.data.len(),
        data: serde_json::to_value(response.data).unwrap_or(serde_json::Value::Null),
    })
}

pub async fn collateral(
    ctx: &AppContext,
    limit: i64,
    cursor: Option<&str>,
) -> Result<HistoryResult, VulcanError> {
    let authority = default_authority(ctx)?;
    let mut params = CollateralHistoryQueryParams::new(limit);
    if let Some(cursor) = cursor {
        params = params.with_next_cursor(cursor);
    }
    let response = ctx
        .http_client
        .collateral()
        .get_user_collateral_history(&authority, params)
        .await
        .map_err(|e| VulcanError::api("COLLATERAL_HISTORY_FAILED", e.to_string()))?;
    Ok(HistoryResult {
        history_type: "collateral".to_string(),
        symbol: None,
        limit,
        cursor: cursor.map(str::to_string),
        next_cursor: response.next_cursor,
        prev_cursor: response.prev_cursor,
        has_more: response.has_more,
        count: response.data.len(),
        data: serde_json::to_value(response.data).unwrap_or(serde_json::Value::Null),
    })
}

pub async fn funding(
    ctx: &AppContext,
    symbol: Option<&str>,
    limit: i64,
    cursor: Option<&str>,
) -> Result<HistoryResult, VulcanError> {
    let authority = default_authority(ctx)?;
    let mut params = FundingHistoryQueryParams::new().with_limit(limit);
    if let Some(symbol) = symbol {
        params = params.with_symbol(symbol.to_ascii_uppercase());
    }
    if let Some(cursor) = cursor {
        params = params.with_cursor(cursor);
    }
    let response = ctx
        .http_client
        .funding()
        .get_user_funding_history(&authority, params)
        .await
        .map_err(|e| VulcanError::api("FUNDING_HISTORY_FAILED", e.to_string()))?;
    Ok(HistoryResult {
        history_type: "funding".to_string(),
        symbol: symbol.map(|s| s.to_ascii_uppercase()),
        limit,
        cursor: cursor.map(str::to_string),
        next_cursor: response.next_cursor,
        prev_cursor: response.prev_cursor,
        has_more: response.has_more,
        count: response.events.len(),
        data: serde_json::to_value(response.events).unwrap_or(serde_json::Value::Null),
    })
}

pub async fn pnl(
    ctx: &AppContext,
    resolution: &str,
    limit: i64,
) -> Result<HistoryResult, VulcanError> {
    let authority = default_authority(ctx)?;
    let resolution = PnlResolution::from_str(resolution)
        .map_err(|e| VulcanError::validation("INVALID_PNL_RESOLUTION", e))?;
    let params = PnlQueryParams::new(resolution).with_limit(limit);
    let response = ctx
        .http_client
        .traders()
        .get_trader_pnl(&authority, params)
        .await
        .map_err(|e| VulcanError::api("PNL_HISTORY_FAILED", e.to_string()))?;
    Ok(HistoryResult {
        history_type: "pnl".to_string(),
        symbol: None,
        limit,
        cursor: None,
        next_cursor: None,
        prev_cursor: None,
        has_more: false,
        count: response.len(),
        data: serde_json::to_value(response).unwrap_or(serde_json::Value::Null),
    })
}

fn default_authority(ctx: &AppContext) -> Result<Pubkey, VulcanError> {
    crate::commands::trade::resolve_authority(ctx)
}

//! Shared live history reconciliation helpers for strategy runners.

use crate::context::AppContext;
use crate::error::VulcanError;
use crate::strategy::types::StrategyExecution;

#[allow(dead_code)]
pub(crate) enum ReconcileOutcome {
    Matched(StrategyExecution),
    Missing,
    Ambiguous(usize),
    Pending,
    Failed(String),
}

/// Find a fill record by order sequence number. Used by strategy runners
/// (grid) where the original placement transaction won't appear in trade
/// history — resting limit orders fill on separate counterparty match
/// transactions, but every fill record references the same
/// `order_sequence_number`.
///
/// Phoenix encodes bid order ids in the high half of u64 (the orderbook
/// returns them as unsigned strings), while trade history's
/// `order_sequence_number: i64` stores the same value wrapped to signed.
/// Normalize both to u64 before comparing so bids and asks both reconcile.
pub(crate) async fn reconcile_fill_by_order_id(
    ctx: &AppContext,
    symbol: &str,
    order_id: &str,
) -> Result<ReconcileOutcome, VulcanError> {
    let Some(needle) = normalize_order_id(order_id) else {
        return Ok(ReconcileOutcome::Missing);
    };
    let history = crate::history::trades(ctx, Some(symbol), 50, None).await?;
    let Some(records) = history.data.as_array() else {
        return Ok(ReconcileOutcome::Missing);
    };
    let matches: Vec<&serde_json::Value> = records
        .iter()
        .filter(|record| record_matches_order_seq(record, needle))
        .collect();
    if matches.is_empty() {
        return Ok(ReconcileOutcome::Missing);
    }
    let record = matches[0];
    let fill_price = find_number_for_keys(
        record,
        &["fill_price", "price", "avg_price", "average_price"],
    );
    let fill_tokens = find_number_for_keys(
        record,
        &[
            "fill_tokens",
            "tokens",
            "base_tokens",
            "size_tokens",
            "base_lots_delta",
            "baseLotsdelta",
            "quantity",
            "qty",
            "size",
        ],
    )
    .map(f64::abs);
    let fill_notional = find_number_for_keys(
        record,
        &[
            "fill_notional_usdc",
            "notional_usdc",
            "quote_quantity",
            "quote_qty",
            "notional",
        ],
    )
    .or_else(|| fill_price.zip(fill_tokens).map(|(p, t)| p * t));
    let fee = find_number_for_keys(record, &["fees", "fee", "taker_fee", "maker_fee"]);
    let fill_signature = find_string_for_keys(
        record,
        &["signature", "tx_signature", "transaction_signature"],
    );
    let fill_id = find_string_for_keys(record, &["fill_id", "fillId", "trade_id", "tradeId", "id"]);
    Ok(ReconcileOutcome::Matched(StrategyExecution {
        action: "grid_level_reconcile".to_string(),
        fill_id,
        tx_signature: fill_signature,
        fill_price,
        fill_tokens,
        fill_notional_usdc: fill_notional,
        fee,
        note: None,
    }))
}

fn record_matches_order_seq(record: &serde_json::Value, needle: u64) -> bool {
    for key in [
        "orderSequenceNumber",
        "order_sequence_number",
        "orderId",
        "order_id",
    ] {
        if let Some(value) = find_string_for_keys(record, &[key]) {
            if normalize_order_id(&value) == Some(needle) {
                return true;
            }
        }
    }
    false
}

fn normalize_order_id(s: &str) -> Option<u64> {
    if let Ok(n) = s.parse::<u64>() {
        return Some(n);
    }
    if let Ok(n) = s.parse::<i64>() {
        return Some(n as u64);
    }
    None
}

pub(crate) async fn reconcile_fill_by_tx(
    ctx: &AppContext,
    symbol: &str,
    tx_signature: &str,
    fallback: &StrategyExecution,
) -> Result<ReconcileOutcome, VulcanError> {
    let history = crate::history::trades(ctx, Some(symbol), 20, None).await?;
    let Some(records) = history.data.as_array() else {
        return Ok(ReconcileOutcome::Missing);
    };
    let matches: Vec<&serde_json::Value> = records
        .iter()
        .filter(|record| value_contains_string(record, tx_signature))
        .collect();
    if matches.is_empty() {
        return Ok(ReconcileOutcome::Missing);
    }
    if matches.len() > 1 {
        return Ok(ReconcileOutcome::Ambiguous(matches.len()));
    }
    let record = matches[0];
    let fill_price = find_number_for_keys(
        record,
        &[
            "fill_price",
            "price",
            "avg_price",
            "average_price",
            "mark_price",
        ],
    )
    .or(fallback.fill_price);
    let fill_tokens = find_number_for_keys(
        record,
        &[
            "fill_tokens",
            "tokens",
            "base_tokens",
            "size_tokens",
            "quantity",
            "qty",
            "size",
        ],
    )
    .or(fallback.fill_tokens);
    let fill_notional = find_number_for_keys(
        record,
        &[
            "fill_notional_usdc",
            "notional_usdc",
            "quote_quantity",
            "quote_qty",
            "notional",
        ],
    )
    .or_else(|| {
        fill_tokens
            .zip(fill_price)
            .map(|(tokens, price)| tokens * price)
    })
    .or(fallback.fill_notional_usdc);
    let fee = find_number_for_keys(record, &["fee", "fees", "taker_fee", "maker_fee"]);
    Ok(ReconcileOutcome::Matched(StrategyExecution {
        action: fallback.action.clone(),
        fill_id: find_string_for_keys(record, &["fill_id", "trade_id", "id", "order_id"]),
        tx_signature: Some(tx_signature.to_string()),
        fill_price,
        fill_tokens,
        fill_notional_usdc: fill_notional,
        fee,
        note: None,
    }))
}

pub(crate) fn value_contains_string(value: &serde_json::Value, needle: &str) -> bool {
    match value {
        serde_json::Value::String(s) => s == needle,
        serde_json::Value::Array(values) => values
            .iter()
            .any(|value| value_contains_string(value, needle)),
        serde_json::Value::Object(map) => map
            .values()
            .any(|value| value_contains_string(value, needle)),
        _ => false,
    }
}

pub(crate) fn find_number_for_keys(value: &serde_json::Value, keys: &[&str]) -> Option<f64> {
    match value {
        serde_json::Value::Object(map) => {
            for key in keys {
                if let Some(value) = map.get(*key).and_then(json_number) {
                    return Some(value);
                }
            }
            map.values()
                .find_map(|value| find_number_for_keys(value, keys))
        }
        serde_json::Value::Array(values) => values
            .iter()
            .find_map(|value| find_number_for_keys(value, keys)),
        _ => None,
    }
}

pub(crate) fn find_string_for_keys(value: &serde_json::Value, keys: &[&str]) -> Option<String> {
    match value {
        serde_json::Value::Object(map) => {
            for key in keys {
                if let Some(value) = map.get(*key).and_then(json_string) {
                    return Some(value);
                }
            }
            map.values()
                .find_map(|value| find_string_for_keys(value, keys))
        }
        serde_json::Value::Array(values) => values
            .iter()
            .find_map(|value| find_string_for_keys(value, keys)),
        _ => None,
    }
}

fn json_number(value: &serde_json::Value) -> Option<f64> {
    value
        .as_f64()
        .or_else(|| value.as_str().and_then(|s| s.parse::<f64>().ok()))
}

fn json_string(value: &serde_json::Value) -> Option<String> {
    value
        .as_str()
        .map(str::to_string)
        .or_else(|| value.as_i64().map(|n| n.to_string()))
        .or_else(|| value.as_u64().map(|n| n.to_string()))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn reconciliation_helpers_find_nested_values() {
        let value = serde_json::json!({
            "tx_signature": "abc",
            "fill": { "price": "42.5", "quantity": 2.0, "trade_id": "t1" }
        });
        assert!(value_contains_string(&value, "abc"));
        assert_eq!(find_number_for_keys(&value, &["price"]), Some(42.5));
        assert_eq!(
            find_string_for_keys(&value, &["trade_id"]),
            Some("t1".to_string())
        );
    }

    #[test]
    fn normalize_order_id_handles_unsigned_and_signed_wrap() {
        // Asks fit in i64 directly.
        assert_eq!(normalize_order_id("15467"), Some(15467));
        // Bids are in the high half of u64. Orderbook returns the unsigned form,
        // trade history serializes them as signed i64 (wrapped negative).
        assert_eq!(
            normalize_order_id("18446744073709536149"),
            Some(18446744073709536149)
        );
        assert_eq!(normalize_order_id("-15467"), Some(18446744073709536149));
        assert_eq!(normalize_order_id("not-a-number"), None);
    }

    #[test]
    fn record_matches_order_seq_unifies_signed_and_unsigned() {
        // Bid fill: trade history records `orderSequenceNumber` as a signed i64
        // (wraps negative for high-half u64 values).
        let bid_fill = serde_json::json!({
            "orderSequenceNumber": -15467,
            "price": "535.59",
        });
        assert!(record_matches_order_seq(&bid_fill, 18446744073709536149));
        // Ask fill: positive sequence number on both sides.
        let ask_fill = serde_json::json!({
            "orderSequenceNumber": 15467,
            "price": "536.69",
        });
        assert!(record_matches_order_seq(&ask_fill, 15467));
        // Non-matching record returns false.
        assert!(!record_matches_order_seq(&ask_fill, 99999));
    }
}

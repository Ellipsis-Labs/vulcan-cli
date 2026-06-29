//! Shared execution helpers for strategy runners.

use crate::cli::paper::{PaperOrderArgs, PaperOrderTypeArg};
use crate::commands::{paper, trade};
use crate::context::AppContext;
use crate::error::VulcanError;
use crate::paper::PaperSide;
use crate::strategy::types::{
    StrategyExecution, StrategyMarginMode, StrategyMarketSnapshot, StrategySide,
};
use phoenix_rise::ix::types::Side;

pub(crate) async fn execute_paper_market(
    ctx: &AppContext,
    symbol: &str,
    side: StrategySide,
    tokens: Option<f64>,
    notional_usdc: Option<f64>,
) -> Result<StrategyExecution, VulcanError> {
    let args = PaperOrderArgs {
        symbol: symbol.to_string(),
        order_type: PaperOrderTypeArg::Market,
        size: None,
        tokens,
        notional_usdc,
        price: None,
        tp: None,
        sl: None,
    };
    let side = match side {
        StrategySide::Buy => PaperSide::Buy,
        StrategySide::Sell => PaperSide::Sell,
    };
    let result = paper::order(ctx, side, args).await?;
    let fill = result.fill.as_ref();
    Ok(StrategyExecution {
        action: result.action,
        fill_id: fill.map(|fill| fill.fill_id.clone()),
        tx_signature: None,
        fill_price: fill.map(|fill| fill.price),
        fill_tokens: fill.map(|fill| fill.size_tokens),
        fill_notional_usdc: fill.map(|fill| fill.price * fill.size_tokens),
        fee: fill.map(|fill| fill.fee),
        note: None,
    })
}

pub(crate) fn dry_run_market(
    side: StrategySide,
    tokens: Option<f64>,
    notional_usdc: Option<f64>,
    market: &StrategyMarketSnapshot,
) -> StrategyExecution {
    let fill_tokens = tokens.or_else(|| notional_usdc.map(|notional| notional / market.mark_price));
    let fill_notional =
        notional_usdc.or_else(|| fill_tokens.map(|tokens| tokens * market.mark_price));
    StrategyExecution {
        action: format!("dry_run_{}_market", side.as_str()),
        fill_id: None,
        tx_signature: None,
        fill_price: Some(market.mark_price),
        fill_tokens,
        fill_notional_usdc: fill_notional,
        fee: Some(0.0),
        note: Some("dry run: no paper or live order submitted".to_string()),
    }
}

pub(crate) struct LiveMarketRequest<'a> {
    pub(crate) symbol: &'a str,
    pub(crate) side: StrategySide,
    pub(crate) tokens: Option<f64>,
    pub(crate) notional_usdc: Option<f64>,
    pub(crate) market: &'a StrategyMarketSnapshot,
    pub(crate) margin_mode: StrategyMarginMode,
    pub(crate) isolated_collateral: Option<f64>,
}

#[allow(dead_code)]
pub(crate) async fn execute_live_market(
    ctx: &AppContext,
    request: LiveMarketRequest<'_>,
) -> Result<StrategyExecution, VulcanError> {
    let spec = if let Some(tokens) = request.tokens {
        trade::SizeSpec::Tokens(tokens)
    } else if let Some(notional) = request.notional_usdc {
        trade::SizeSpec::Notional(notional)
    } else {
        return Err(VulcanError::validation(
            "MISSING_STRATEGY_SIZE",
            "provide either notional_usdc or tokens",
        ));
    };
    let resolved = trade::resolve_base_lots(ctx, request.symbol, spec).await?;
    let side_sdk = match request.side {
        StrategySide::Buy => Side::Bid,
        StrategySide::Sell => Side::Ask,
    };
    let mut order = trade::execute_market_order_inner(
        ctx,
        request.symbol,
        resolved.base_lots,
        side_sdk,
        None,
        None,
        request.margin_mode == StrategyMarginMode::Isolated,
        request.isolated_collateral,
        false,
    )
    .await?;
    order.quoted_mark = resolved.quoted_mark;

    let fill_tokens = base_lots_to_tokens(ctx, request.symbol, order.size)
        .await
        .ok();
    let fill_price = order.quoted_mark.or(Some(request.market.mark_price));
    let mut execution = StrategyExecution {
        action: order.action,
        fill_id: None,
        tx_signature: order.tx_signature.clone(),
        fill_price,
        fill_tokens,
        fill_notional_usdc: fill_tokens
            .zip(fill_price)
            .map(|(tokens, price)| tokens * price),
        fee: None,
        note: None,
    };

    if order.dry_run {
        execution.action = format!("dry_run_live_{}_market", request.side.as_str());
        execution.fee = Some(0.0);
        execution.note = Some("dry run: no live order submitted".to_string());
    }
    Ok(execution)
}

pub(crate) async fn execute_live_market_base_lots(
    ctx: &AppContext,
    symbol: &str,
    side: StrategySide,
    base_lots: u64,
    market: &StrategyMarketSnapshot,
    margin_mode: StrategyMarginMode,
    isolated_collateral: Option<f64>,
) -> Result<StrategyExecution, VulcanError> {
    let side_sdk = match side {
        StrategySide::Buy => Side::Bid,
        StrategySide::Sell => Side::Ask,
    };
    let order = trade::execute_market_order_inner(
        ctx,
        symbol,
        base_lots as f64,
        side_sdk,
        None,
        None,
        margin_mode == StrategyMarginMode::Isolated,
        isolated_collateral,
        false,
    )
    .await?;
    let fill_tokens = base_lots_to_tokens(ctx, symbol, order.size).await.ok();
    let fill_price = Some(market.mark_price);
    let mut execution = StrategyExecution {
        action: order.action,
        fill_id: None,
        tx_signature: order.tx_signature.clone(),
        fill_price,
        fill_tokens,
        fill_notional_usdc: fill_tokens
            .zip(fill_price)
            .map(|(tokens, price)| tokens * price),
        fee: None,
        note: None,
    };
    if order.dry_run {
        execution.action = format!("dry_run_live_{}_market", side.as_str());
        execution.fee = Some(0.0);
        execution.note = Some("dry run: no live order submitted".to_string());
    }
    Ok(execution)
}

pub(crate) async fn base_lots_to_tokens(
    ctx: &AppContext,
    symbol: &str,
    base_lots: f64,
) -> Result<f64, VulcanError> {
    let metadata = ctx.metadata().await?;
    let market = metadata.get_market(symbol).ok_or_else(|| {
        VulcanError::validation("UNKNOWN_MARKET", format!("Unknown market: {}", symbol))
    })?;
    Ok(base_lots / 10f64.powi(market.base_lots_decimals as i32))
}

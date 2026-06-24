//! Position command execution.

use crate::cli::position::PositionCommand;
use crate::context::AppContext;
use crate::error::VulcanError;
use crate::output::{render_success, TableRenderable};
use phoenix_rise::Side;
use serde::Serialize;
use solana_pubkey::Pubkey;

// ── Result types ────────────────────────────────────────────────────────

#[derive(Debug, Serialize)]
pub struct PositionListResult {
    pub positions: Vec<PositionInfo>,
}

#[derive(Debug, Serialize)]
pub struct PositionInfo {
    pub symbol: String,
    pub side: String,
    pub size: String,
    pub entry_price: String,
    pub mark_price: String,
    pub unrealized_pnl: String,
    /// Unrealized PnL as a percentage. For isolated positions, denominator is
    /// the isolated subaccount collateral. For cross positions, denominator is
    /// the entry-time initial margin (current initial_margin * entry / mark).
    /// Matches the phoenix-v2-frontend formula.
    pub unrealized_pnl_pct: String,
    pub liquidation_price: String,
    pub initial_margin: String,
    pub maintenance_margin: String,
    /// Collateral assigned to this position's subaccount. Only populated for
    /// isolated positions; for cross positions the collateral is shared across
    /// the cross-margin subaccount and not attributable to a single position.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub subaccount_collateral: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub subaccount_index: Option<u8>,
}

impl TableRenderable for PositionListResult {
    fn render_table(&self) {
        if self.positions.is_empty() {
            println!("No open positions.");
            return;
        }
        let rows: Vec<Vec<String>> = self
            .positions
            .iter()
            .map(|p| {
                vec![
                    p.symbol.clone(),
                    p.side.clone(),
                    p.size.clone(),
                    p.entry_price.clone(),
                    p.mark_price.clone(),
                    p.unrealized_pnl.clone(),
                    p.unrealized_pnl_pct.clone(),
                    p.liquidation_price.clone(),
                ]
            })
            .collect();
        crate::output::table::render_table(
            &[
                "Symbol",
                "Side",
                "Size",
                "Entry",
                "Mark",
                "PnL",
                "PnL %",
                "Liq Price",
            ],
            rows,
        );
    }
}

#[derive(Debug, Serialize)]
pub struct PositionDetailResult {
    pub symbol: String,
    pub side: String,
    pub size: String,
    pub entry_price: String,
    pub unrealized_pnl: String,
    pub discounted_unrealized_pnl: String,
    pub position_value: String,
    pub initial_margin: String,
    pub maintenance_margin: String,
    pub liquidation_price: String,
    pub take_profit_price: Option<String>,
    pub stop_loss_price: Option<String>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub tp_levels: Vec<TpSlLevel>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub sl_levels: Vec<TpSlLevel>,
    pub unsettled_funding: String,
    pub accumulated_funding: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub subaccount_index: Option<u8>,
}

#[derive(Debug, Serialize)]
pub struct TpSlLevel {
    pub price: f64,
    pub size: f64,
}

impl TableRenderable for PositionDetailResult {
    fn render_table(&self) {
        println!("Position: {} {}", self.symbol, self.side);
        println!("  Size: {}", self.size);
        println!("  Entry price: {}", self.entry_price);
        println!("  Unrealized PnL: {}", self.unrealized_pnl);
        println!("  Position value: {}", self.position_value);
        println!("  Initial margin: {}", self.initial_margin);
        println!("  Maintenance margin: {}", self.maintenance_margin);
        println!("  Liquidation price: {}", self.liquidation_price);
        if let Some(tp) = &self.take_profit_price {
            println!("  Take profit: {}", tp);
        }
        if let Some(sl) = &self.stop_loss_price {
            println!("  Stop loss: {}", sl);
        }
        if !self.tp_levels.is_empty() {
            println!("  TP levels:");
            for lvl in &self.tp_levels {
                println!("    - {} @ {}", lvl.size, lvl.price);
            }
        }
        if !self.sl_levels.is_empty() {
            println!("  SL levels:");
            for lvl in &self.sl_levels {
                println!("    - {} @ {}", lvl.size, lvl.price);
            }
        }
        println!("  Unsettled funding: {}", self.unsettled_funding);
        println!("  Accumulated funding: {}", self.accumulated_funding);
    }
}

#[derive(Debug, Serialize)]
pub struct CloseResult {
    pub symbol: String,
    pub side_closed: String,
    pub size_closed: String,
    pub dry_run: bool,
    pub tx_signature: Option<String>,
    pub num_instructions: usize,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub swept_subaccount: Option<u8>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub sweep_skipped_reason: Option<String>,
}

impl TableRenderable for CloseResult {
    fn render_table(&self) {
        if self.dry_run {
            println!(
                "[DRY RUN] Would close {} {} position (size: {})",
                self.symbol, self.side_closed, self.size_closed
            );
        } else {
            println!(
                "Closed {} {} position (size: {})",
                self.symbol, self.side_closed, self.size_closed
            );
        }
        if let Some(sub) = self.swept_subaccount {
            if self.dry_run {
                println!(
                    "  [DRY RUN] Would sweep collateral from subaccount {} back to cross-margin",
                    sub
                );
            } else {
                println!(
                    "  Swept collateral from subaccount {} back to cross-margin",
                    sub
                );
            }
        }
        if let Some(reason) = &self.sweep_skipped_reason {
            println!("  Collateral sweep skipped: {}", reason);
        }
        if let Some(sig) = &self.tx_signature {
            println!("  Tx: {}", sig);
        }
    }
}

#[derive(Debug, Serialize)]
pub struct CloseAllResult {
    pub closed: Vec<CloseResult>,
    pub total_closed: usize,
    pub dry_run: bool,
}

impl TableRenderable for CloseAllResult {
    fn render_table(&self) {
        if self.closed.is_empty() {
            println!("No open positions to close.");
            return;
        }
        if self.dry_run {
            println!("[DRY RUN] Would close {} positions:", self.total_closed);
        } else {
            println!("Closed {} positions:", self.total_closed);
        }
        for r in &self.closed {
            print!("  {} {} {}", r.symbol, r.side_closed, r.size_closed);
            if let Some(sig) = &r.tx_signature {
                print!("  tx: {}", sig);
            }
            println!();
        }
    }
}

#[derive(Debug, Serialize)]
pub struct TpSlResult {
    pub symbol: String,
    pub tp: Option<f64>,
    pub sl: Option<f64>,
    pub dry_run: bool,
    pub tx_signature: Option<String>,
    pub num_instructions: usize,
}

impl TableRenderable for TpSlResult {
    fn render_table(&self) {
        if self.dry_run {
            print!("[DRY RUN] Would attach");
        } else {
            print!("Attached");
        }
        if let Some(tp) = self.tp {
            print!(" TP=${:.2}", tp);
        }
        if let Some(sl) = self.sl {
            print!(" SL=${:.2}", sl);
        }
        println!(" to {} position", self.symbol);
        if let Some(sig) = &self.tx_signature {
            println!("  Tx: {}", sig);
        }
    }
}

// ── Helpers ─────────────────────────────────────────────────────────────

/// Fetch all locally computed trader views for the active wallet.
/// MCP requests use the pre-unlocked session wallet; CLI falls back to default wallet.
pub(crate) async fn get_all_computed_trader_views(
    ctx: &AppContext,
) -> Result<
    (
        Vec<crate::commands::trader_state::ComputedTraderView>,
        String,
    ),
    VulcanError,
> {
    let (bundle, wallet_name) = get_all_trader_state_bundle(ctx).await?;
    let traders = bundle.views;

    if traders.is_empty() {
        return Err(VulcanError::api(
            "NO_TRADER_ACCOUNT",
            "No registered trader account found. Use 'vulcan account register' first.",
        ));
    }

    Ok((traders, wallet_name))
}

/// Fetch the full v1 trader-state bundle for the active wallet.
pub(crate) async fn get_all_trader_state_bundle(
    ctx: &AppContext,
) -> Result<(crate::commands::trader_state::TraderStateBundle, String), VulcanError> {
    let (wallet_name, authority) = crate::commands::trade::resolve_authority_name(ctx)?;
    let bundle =
        crate::commands::trader_state::fetch_trader_state_bundle(ctx, &authority, 0).await?;

    if bundle.views.is_empty() {
        return Err(VulcanError::api(
            "NO_TRADER_ACCOUNT",
            "No registered trader account found. Use 'vulcan account register' first.",
        ));
    }

    Ok((bundle, wallet_name))
}

/// Fetch the cross-margin computed trader view for the default wallet.
#[allow(dead_code)]
async fn get_trader_view(
    ctx: &AppContext,
) -> Result<(crate::commands::trader_state::ComputedTraderView, String), VulcanError> {
    let (traders, wallet_name) = get_all_computed_trader_views(ctx).await?;

    let trader = traders
        .into_iter()
        .find(|t| t.trader_subaccount_index == 0)
        .ok_or_else(|| {
            VulcanError::api(
                "NO_TRADER_ACCOUNT",
                "No cross-margin trader account found. Use 'vulcan account register' first.",
            )
        })?;

    Ok((trader, wallet_name))
}

fn resolve_authority(ctx: &AppContext) -> Result<Pubkey, VulcanError> {
    crate::commands::trade::resolve_authority(ctx)
}

fn position_side(size_ui: &str) -> &str {
    if size_ui.starts_with('-') {
        "Short"
    } else {
        "Long"
    }
}

fn format_liq_price(ui: &str) -> String {
    // If the liquidation price is negative, it's effectively unreachable
    if ui.starts_with('-') {
        "N/A".to_string()
    } else {
        ui.to_string()
    }
}

/// Compute unrealized PnL as a percentage following the phoenix-v2-frontend
/// formula. Isolated positions use the subaccount collateral as the
/// denominator; cross positions use the entry-time initial margin, derived by
/// rebasing the current `initial_margin` from mark price back to entry price:
/// `entry_initial_margin = initial_margin * entry / mark`.
fn unrealized_pnl_pct(
    pnl: Option<f64>,
    initial_margin: Option<f64>,
    entry: Option<f64>,
    mark: Option<f64>,
    iso_collateral: Option<f64>,
) -> String {
    let na = || "—".to_string();
    let Some(pnl) = pnl else { return na() };
    let denom = if let Some(c) = iso_collateral {
        c
    } else if let (Some(im), Some(e), Some(m)) = (initial_margin, entry, mark) {
        if m.abs() <= f64::EPSILON {
            return na();
        }
        im * e / m
    } else {
        return na();
    };
    if denom.abs() <= f64::EPSILON {
        return na();
    }
    format!("{:+.2}%", (pnl / denom) * 100.0)
}

// ── Execution ───────────────────────────────────────────────────────────

pub async fn execute_list_inner(ctx: &AppContext) -> Result<PositionListResult, VulcanError> {
    let (bundle, _) = get_all_trader_state_bundle(ctx).await?;
    Ok(project_positions(&bundle.views))
}

pub(crate) fn project_positions(
    traders: &[crate::commands::trader_state::ComputedTraderView],
) -> PositionListResult {
    let mut positions: Vec<PositionInfo> = Vec::new();
    for trader in traders {
        let is_isolated = trader.trader_subaccount_index != 0;
        let margin_label = if is_isolated { " [iso]" } else { "" };
        let iso_collateral: Option<f64> = if is_isolated {
            trader.collateral_balance.ui.parse::<f64>().ok()
        } else {
            None
        };
        for p in &trader.positions {
            let entry_f = p.entry_price.ui.parse::<f64>().ok();
            let size_abs = p
                .position_size
                .ui
                .parse::<f64>()
                .ok()
                .map(f64::abs)
                .filter(|size| *size > f64::EPSILON);
            let value_abs = p.position_value.ui.parse::<f64>().ok().map(f64::abs);
            let mark_f = match (size_abs, value_abs) {
                (Some(s), Some(v)) => Some(v / s),
                _ => None,
            };
            let mark = mark_f
                .map(|m| format!("{:.4}", m))
                .unwrap_or_else(|| "—".to_string());
            let pnl_f = p.unrealized_pnl.ui.parse::<f64>().ok();
            let init_margin_f = p.initial_margin.ui.parse::<f64>().ok();
            let pnl_pct = unrealized_pnl_pct(
                pnl_f,
                init_margin_f,
                entry_f,
                mark_f,
                if is_isolated { iso_collateral } else { None },
            );
            let sub_idx = if is_isolated {
                Some(trader.trader_subaccount_index)
            } else {
                None
            };
            positions.push(PositionInfo {
                symbol: format!("{}{}", p.symbol, margin_label),
                side: position_side(&p.position_size.ui).to_string(),
                size: p.position_size.ui.clone(),
                entry_price: p.entry_price.ui.clone(),
                mark_price: mark,
                unrealized_pnl: p.unrealized_pnl.ui.clone(),
                unrealized_pnl_pct: pnl_pct,
                initial_margin: p.initial_margin.ui.clone(),
                subaccount_collateral: iso_collateral.map(|c| format!("{:.6}", c)),
                liquidation_price: format_liq_price(&p.liquidation_price.ui),
                maintenance_margin: p.maintenance_margin.ui.clone(),
                subaccount_index: sub_idx,
            });
        }
    }
    PositionListResult { positions }
}

pub async fn execute_show_inner(
    ctx: &AppContext,
    symbol: &str,
) -> Result<PositionDetailResult, VulcanError> {
    let (bundle, _) = get_all_trader_state_bundle(ctx).await?;
    let traders = &bundle.views;

    let symbol_upper = symbol.to_ascii_uppercase();
    let (trader_view, pos) = traders
        .iter()
        .find_map(|t| {
            t.positions
                .iter()
                .find(|p| p.symbol.to_ascii_uppercase() == symbol_upper)
                .map(|p| (t, p))
        })
        .ok_or_else(|| {
            VulcanError::validation("NO_POSITION", format!("No open position for '{}'", symbol))
        })?;

    let sub_idx = if trader_view.trader_subaccount_index == 0 {
        None
    } else {
        Some(trader_view.trader_subaccount_index)
    };

    let (mut tp_levels, mut sl_levels) = (Vec::new(), Vec::new());
    for t in trader_view
        .conditional_triggers
        .iter()
        .filter(|t| t.symbol.eq_ignore_ascii_case(&pos.symbol))
    {
        let level = TpSlLevel {
            price: t.trigger_price,
            size: t.size_tokens,
        };
        match t.kind {
            crate::commands::conditional_orders::TriggerKind::TakeProfit => tp_levels.push(level),
            crate::commands::conditional_orders::TriggerKind::StopLoss => sl_levels.push(level),
        }
    }

    Ok(PositionDetailResult {
        symbol: pos.symbol.clone(),
        side: position_side(&pos.position_size.ui).to_string(),
        size: pos.position_size.ui.clone(),
        entry_price: pos.entry_price.ui.clone(),
        unrealized_pnl: pos.unrealized_pnl.ui.clone(),
        discounted_unrealized_pnl: pos.discounted_unrealized_pnl.ui.clone(),
        position_value: pos.position_value.ui.clone(),
        initial_margin: pos.initial_margin.ui.clone(),
        maintenance_margin: pos.maintenance_margin.ui.clone(),
        liquidation_price: format_liq_price(&pos.liquidation_price.ui),
        take_profit_price: pos.take_profit_price.as_ref().map(|d| d.ui.clone()),
        stop_loss_price: pos.stop_loss_price.as_ref().map(|d| d.ui.clone()),
        tp_levels,
        sl_levels,
        unsettled_funding: pos.unsettled_funding.ui.clone(),
        accumulated_funding: pos.accumulated_funding.ui.clone(),
        subaccount_index: sub_idx,
    })
}

pub async fn execute_close_inner(
    ctx: &AppContext,
    symbol: &str,
) -> Result<CloseResult, VulcanError> {
    execute_close_target_inner(ctx, symbol, None).await
}

async fn execute_close_target_inner(
    ctx: &AppContext,
    symbol: &str,
    target_subaccount: Option<u8>,
) -> Result<CloseResult, VulcanError> {
    let (wallet_name, authority) = crate::commands::trade::resolve_authority_name(ctx)?;
    let trader_state =
        crate::commands::trader_state::fetch_trader_state_snapshot(ctx, &authority, 0).await?;

    let symbol_upper = symbol.to_ascii_uppercase();

    let (subaccount, pos) = if let Some(subaccount_index) = target_subaccount {
        trader_state
            .find_position_in_subaccount(subaccount_index, &symbol_upper)
            .ok_or_else(|| {
                VulcanError::validation(
                    "NO_POSITION",
                    format!(
                        "No open position for '{}' in subaccount {}",
                        symbol, subaccount_index
                    ),
                )
            })?
    } else {
        trader_state.find_position(&symbol_upper).ok_or_else(|| {
            VulcanError::validation("NO_POSITION", format!("No open position for '{}'", symbol))
        })?
    };

    let subaccount_index = subaccount.subaccount_index;
    let is_long = pos.is_long();
    let close_side = if is_long { Side::Ask } else { Side::Bid };
    let abs_size = pos.abs_base_lots();
    let size_str = pos.base_position_lots.clone();
    let side_closed_str = if is_long { "Long" } else { "Short" }.to_string();

    let (wallet, authority, _) =
        crate::commands::trade::resolve_wallet_and_pda(ctx, Some(&wallet_name)).await?;

    let builder = ctx.tx_builder().await?;

    let mut ixs = if subaccount_index == 0 {
        // Cross-margin: use standard market order
        let trader_pda = phoenix_rise::types::TraderKey::derive_pda(&authority, 0, 0);
        let ticket = phoenix_rise::MarketOrderTicket::builder()
            .authority(authority)
            .trader_account(trader_pda)
            .symbol(symbol)
            .side(close_side)
            .num_base_lots(abs_size)
            .build()
            .map_err(|e| VulcanError::api("BUILD_CLOSE_FAILED", e.to_string()))?;

        builder
            .place_market_order(ticket)
            .await
            .map_err(|e| VulcanError::api("BUILD_CLOSE_FAILED", e.to_string()))?
    } else {
        let trader_pda = phoenix_rise::types::TraderKey::derive_pda(
            &authority,
            trader_state.trader_pda_index,
            subaccount_index,
        );
        let ticket = phoenix_rise::MarketOrderTicket::builder()
            .authority(authority)
            .trader_account(trader_pda)
            .symbol(symbol)
            .side(close_side)
            .num_base_lots(abs_size)
            .subaccount_index(subaccount_index)
            .build()
            .map_err(|e| VulcanError::api("BUILD_CLOSE_FAILED", e.to_string()))?;

        builder
            .place_market_order(ticket)
            .await
            .map_err(|e| VulcanError::api("BUILD_CLOSE_FAILED", e.to_string()))?
    };

    // Auto-sweep only when the child has no other position/order-like state.
    let (swept_subaccount, sweep_skipped_reason) = if subaccount_index > 0 {
        if let Some(reason) = subaccount.sweep_blocker(Some(&symbol_upper)) {
            (None, Some(reason))
        } else {
            let child_pda = phoenix_rise::types::TraderKey::derive_pda(
                &authority,
                trader_state.trader_pda_index,
                subaccount_index,
            );
            let parent_pda = phoenix_rise::types::TraderKey::derive_pda(
                &authority,
                trader_state.trader_pda_index,
                0,
            );
            let sweep_ixs = builder
                .build_transfer_collateral_child_to_parent(authority, child_pda, parent_pda)
                .map_err(|e| VulcanError::api("BUILD_SWEEP_FAILED", e.to_string()))?;
            ixs.extend(sweep_ixs);
            (Some(subaccount_index), None)
        }
    } else {
        (None, None)
    };

    let num_ixs = ixs.len();
    let sig = crate::commands::trade::send_or_dry_run(ctx, ixs, &wallet).await?;

    Ok(CloseResult {
        symbol: symbol.to_string(),
        side_closed: side_closed_str,
        size_closed: size_str,
        dry_run: ctx.dry_run,
        tx_signature: sig,
        num_instructions: num_ixs,
        swept_subaccount,
        sweep_skipped_reason,
    })
}

pub async fn execute_close_all_inner(ctx: &AppContext) -> Result<CloseAllResult, VulcanError> {
    let (_, authority) = crate::commands::trade::resolve_authority_name(ctx)?;
    let trader_state =
        crate::commands::trader_state::fetch_trader_state_snapshot(ctx, &authority, 0).await?;

    let mut targets: Vec<(u8, String)> = trader_state
        .snapshot
        .subaccounts
        .iter()
        .flat_map(|subaccount| {
            subaccount.positions.iter().map(|position| {
                (
                    subaccount.subaccount_index,
                    position.symbol.to_ascii_uppercase(),
                )
            })
        })
        .collect();
    targets.sort();
    targets.dedup();

    let mut closed = Vec::with_capacity(targets.len());
    for (subaccount_index, symbol) in &targets {
        let result = execute_close_target_inner(ctx, symbol, Some(*subaccount_index)).await?;
        closed.push(result);
    }

    let total_closed = closed.len();
    Ok(CloseAllResult {
        closed,
        total_closed,
        dry_run: ctx.dry_run,
    })
}

pub async fn execute_reduce_inner(
    ctx: &AppContext,
    symbol: &str,
    size: f64,
) -> Result<CloseResult, VulcanError> {
    let (wallet_name, authority) = crate::commands::trade::resolve_authority_name(ctx)?;
    let trader_state =
        crate::commands::trader_state::fetch_trader_state_snapshot(ctx, &authority, 0).await?;

    let symbol_upper = symbol.to_ascii_uppercase();
    let (subaccount, pos) = trader_state.find_position(&symbol_upper).ok_or_else(|| {
        VulcanError::validation("NO_POSITION", format!("No open position for '{}'", symbol))
    })?;

    let subaccount_index = subaccount.subaccount_index;
    let is_long = pos.is_long();
    let reduce_side = if is_long { Side::Ask } else { Side::Bid };
    let side_str = if is_long { "Long" } else { "Short" }.to_string();
    let num_base_lots = size as u64;

    let (wallet, authority, _) =
        crate::commands::trade::resolve_wallet_and_pda(ctx, Some(&wallet_name)).await?;

    let builder = ctx.tx_builder().await?;
    let ixs = if subaccount_index == 0 {
        let trader_pda = phoenix_rise::types::TraderKey::derive_pda(&authority, 0, 0);
        let ticket = phoenix_rise::MarketOrderTicket::builder()
            .authority(authority)
            .trader_account(trader_pda)
            .symbol(symbol)
            .side(reduce_side)
            .num_base_lots(num_base_lots)
            .build()
            .map_err(|e| VulcanError::api("BUILD_REDUCE_FAILED", e.to_string()))?;

        builder
            .place_market_order(ticket)
            .await
            .map_err(|e| VulcanError::api("BUILD_REDUCE_FAILED", e.to_string()))?
    } else {
        let trader_pda = phoenix_rise::types::TraderKey::derive_pda(
            &authority,
            trader_state.trader_pda_index,
            subaccount_index,
        );
        let ticket = phoenix_rise::MarketOrderTicket::builder()
            .authority(authority)
            .trader_account(trader_pda)
            .symbol(symbol)
            .side(reduce_side)
            .num_base_lots(num_base_lots)
            .subaccount_index(subaccount_index)
            .build()
            .map_err(|e| VulcanError::api("BUILD_REDUCE_FAILED", e.to_string()))?;

        builder
            .place_market_order(ticket)
            .await
            .map_err(|e| VulcanError::api("BUILD_REDUCE_FAILED", e.to_string()))?
    };

    let num_ixs = ixs.len();
    let sig = crate::commands::trade::send_or_dry_run(ctx, ixs, &wallet).await?;

    Ok(CloseResult {
        symbol: symbol.to_string(),
        side_closed: side_str,
        size_closed: format!("{}", size),
        dry_run: ctx.dry_run,
        tx_signature: sig,
        num_instructions: num_ixs,
        swept_subaccount: None,
        sweep_skipped_reason: None,
    })
}

pub async fn execute_tp_sl_inner(
    ctx: &AppContext,
    symbol: &str,
    tp: Option<f64>,
    sl: Option<f64>,
) -> Result<TpSlResult, VulcanError> {
    if tp.is_none() && sl.is_none() {
        return Err(VulcanError::validation(
            "NO_TP_SL",
            "At least one of --tp or --sl must be specified",
        ));
    }

    let (wallet_name, authority) = crate::commands::trade::resolve_authority_name(ctx)?;
    let trader_state =
        crate::commands::trader_state::fetch_trader_state_snapshot(ctx, &authority, 0).await?;

    let symbol_upper = symbol.to_ascii_uppercase();
    let (subaccount, pos) = trader_state.find_position(&symbol_upper).ok_or_else(|| {
        VulcanError::validation("NO_POSITION", format!("No open position for '{}'", symbol))
    })?;

    let subaccount_index = subaccount.subaccount_index;
    let is_long = pos.is_long();
    let primary_side = if is_long { Side::Bid } else { Side::Ask };

    let (wallet, authority, _) =
        crate::commands::trade::resolve_wallet_and_pda(ctx, Some(&wallet_name)).await?;

    let trader_pda = phoenix_rise::types::TraderKey::derive_pda(
        &authority,
        trader_state.trader_pda_index,
        subaccount_index,
    );

    let builder = ctx.tx_builder().await?;
    let bracket = crate::commands::trade::sized_bracket_leg_orders(tp, sl, pos.abs_base_lots())
        .ok_or_else(|| {
            VulcanError::validation("NO_TP_SL", "At least one of --tp or --sl must be specified")
        })?;

    let mut ixs = crate::commands::trade::conditional_orders_init_ixs_if_needed(
        ctx, &builder, authority, trader_pda,
    )
    .await?;
    let bracket_ixs = builder
        .build_bracket_leg_orders(authority, trader_pda, symbol, primary_side, &bracket)
        .map_err(|e| VulcanError::api("BUILD_TPSL_FAILED", e.to_string()))?;
    ixs.extend(bracket_ixs);

    let num_ixs = ixs.len();
    let sig = crate::commands::trade::send_or_dry_run(ctx, ixs, &wallet).await?;

    Ok(TpSlResult {
        symbol: symbol.to_string(),
        tp,
        sl,
        dry_run: ctx.dry_run,
        tx_signature: sig,
        num_instructions: num_ixs,
    })
}

pub async fn execute(ctx: &AppContext, cmd: PositionCommand) -> Result<(), VulcanError> {
    match cmd {
        PositionCommand::List => {
            let result = execute_list_inner(ctx).await?;
            render_success(ctx.output_format, &result, serde_json::Value::Null);

            if ctx.watch {
                let authority = resolve_authority(ctx)?;
                crate::watch::watch_loop(
                    ctx,
                    crate::watch::WatchKind::TraderState(authority),
                    || async {
                        let result = execute_list_inner(ctx).await?;
                        render_success(ctx.output_format, &result, serde_json::Value::Null);
                        Ok(())
                    },
                )
                .await?;
            }
            Ok(())
        }

        PositionCommand::Show { symbol } => {
            let result = execute_show_inner(ctx, &symbol).await?;
            render_success(
                ctx.output_format,
                &result,
                serde_json::json!({ "command": "position show", "symbol": symbol }),
            );

            if ctx.watch {
                let authority = resolve_authority(ctx)?;
                crate::watch::watch_loop(
                    ctx,
                    crate::watch::WatchKind::TraderState(authority),
                    || {
                        let symbol = symbol.clone();
                        async move {
                            let result = execute_show_inner(ctx, &symbol).await?;
                            render_success(
                                ctx.output_format,
                                &result,
                                serde_json::json!({ "command": "position show", "symbol": symbol }),
                            );
                            Ok(())
                        }
                    },
                )
                .await?;
            }
            Ok(())
        }

        PositionCommand::Close { symbol } => {
            if !ctx.yes && !ctx.dry_run {
                return Err(VulcanError::validation(
                    "CONFIRMATION_REQUIRED",
                    "Pass --yes to confirm position close, or --dry-run to simulate",
                ));
            }

            let result = execute_close_inner(ctx, &symbol).await?;

            render_success(
                ctx.output_format,
                &result,
                serde_json::json!({ "command": "position close", "symbol": symbol, "dry_run": ctx.dry_run }),
            );
            Ok(())
        }

        PositionCommand::CloseAll => {
            if !ctx.yes && !ctx.dry_run {
                return Err(VulcanError::validation(
                    "CONFIRMATION_REQUIRED",
                    "Pass --yes to confirm closing all positions, or --dry-run to simulate",
                ));
            }

            let result = execute_close_all_inner(ctx).await?;

            render_success(
                ctx.output_format,
                &result,
                serde_json::json!({ "command": "position close-all", "dry_run": ctx.dry_run }),
            );
            Ok(())
        }

        PositionCommand::Reduce { symbol, size } => {
            if !ctx.yes && !ctx.dry_run {
                return Err(VulcanError::validation(
                    "CONFIRMATION_REQUIRED",
                    "Pass --yes to confirm position reduce, or --dry-run to simulate",
                ));
            }
            let result = execute_reduce_inner(ctx, &symbol, size).await?;
            render_success(
                ctx.output_format,
                &result,
                serde_json::json!({ "command": "position reduce", "symbol": symbol, "dry_run": ctx.dry_run }),
            );
            Ok(())
        }

        PositionCommand::TpSl { symbol, tp, sl } => {
            if !ctx.yes && !ctx.dry_run {
                return Err(VulcanError::validation(
                    "CONFIRMATION_REQUIRED",
                    "Pass --yes to confirm TP/SL, or --dry-run to simulate",
                ));
            }
            let result = execute_tp_sl_inner(ctx, &symbol, tp, sl).await?;
            render_success(
                ctx.output_format,
                &result,
                serde_json::json!({ "command": "position tp-sl", "symbol": symbol, "dry_run": ctx.dry_run }),
            );
            Ok(())
        }
    }
}

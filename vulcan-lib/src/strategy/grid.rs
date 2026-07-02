//! Grid trading strategy runner.

use crate::cli::paper::{PaperOrderArgs, PaperOrderTypeArg};
use crate::cli::strategy::GridStartArgs;
use crate::commands::{market, paper, trade};
use crate::context::AppContext;
use crate::error::VulcanError;
use crate::paper::PaperSide;
use crate::strategy::log::{
    clear_control_request, default_strategy_runs_dir, read_control_request, write_run_ledger,
    write_run_report,
};
use crate::strategy::reconcile::{reconcile_fill_by_order_id, ReconcileOutcome};
use crate::strategy::runner::{emit_strategy_tick, recompute_ledger_totals, record_strategy_event};
use crate::strategy::safety::{
    validate_live_margin_feasibility, StrategyMarginFeasibility, StrategySafetyPolicy,
};
use crate::strategy::types::{
    StrategyAccountHealth, StrategyControlAction, StrategyControlRequest, StrategyExecution,
    StrategyLedgerSlice, StrategyLedgerTotals, StrategyMarginMode, StrategyMarketSnapshot,
    StrategyMode, StrategyNextTick, StrategyPlannedAction, StrategyPositionSnapshot,
    StrategyProgress, StrategyRunLedger, StrategyRunReport, StrategyRunSummary, StrategySide,
    StrategySliceStatus, StrategyTick, StrategyWalletContext,
};
use chrono::Utc;
use phoenix_rise::ix::types::Side;
use rand::RngCore;
use serde::{Deserialize, Serialize};
use std::collections::{BTreeMap, HashSet};
use std::fs::{self, OpenOptions};
use std::io::Write;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::Instant;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct GridRunConfig {
    pub run_id: String,
    pub run_label: Option<String>,
    pub wallet: Option<StrategyWalletContext>,
    pub symbol: String,
    pub mode: StrategyMode,
    #[serde(default)]
    pub margin_mode: StrategyMarginMode,
    pub isolated_collateral: Option<f64>,
    pub lower_price: f64,
    pub upper_price: f64,
    pub levels_per_side: u32,
    pub tokens_per_level: Option<f64>,
    pub size_lots_per_level: Option<u64>,
    pub price_drift_guard_bps: Option<f64>,
    pub take_profit_spacing: Option<f64>,
    pub stop_loss_spacing: Option<f64>,
    pub interval_seconds: u64,
    pub ticks: u32,
    #[serde(default)]
    pub run_until_stopped: bool,
    pub stale_after_seconds: Option<u64>,
    pub slide: bool,
    pub levels: Vec<GridLevel>,
    pub margin_feasibility: Option<StrategyMarginFeasibility>,
    #[serde(default)]
    pub safety_policy: StrategySafetyPolicy,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct GridLevel {
    pub id: String,
    pub entry_side: StrategySide,
    pub entry_price: f64,
    pub size_lots: u64,
    pub tokens: f64,
    pub take_profit_price: Option<f64>,
    pub stop_loss_price: Option<f64>,
    pub source_level_id: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct GridRunResult {
    pub report: StrategyRunReport,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct GridDetachedRunResult {
    pub run_id: String,
    pub strategy: String,
    pub status: String,
    pub message: String,
}

pub async fn run_grid(ctx: &AppContext, args: GridStartArgs) -> Result<GridRunResult, VulcanError> {
    let (config, no_sleep) = build_grid_config(ctx, args).await?;
    let run_id = config.run_id.clone();
    match run_grid_config(ctx, config, no_sleep, None, None, Vec::new()).await {
        Ok(result) => Ok(result),
        Err(err) => {
            mark_run_failed(ctx, &run_id, &err);
            Err(err)
        }
    }
}

pub async fn run_grid_detached(
    ctx: Arc<AppContext>,
    args: GridStartArgs,
) -> Result<GridDetachedRunResult, VulcanError> {
    let (config, no_sleep) = build_grid_config(ctx.as_ref(), args).await?;
    let run_id = config.run_id.clone();
    let task_run_id = run_id.clone();
    crate::strategy::watchdog::spawn_watchdog(
        crate::strategy::log::default_strategy_runs_dir(&ctx.vulcan_dir),
        run_id.clone(),
    );
    tokio::spawn(async move {
        if let Err(err) =
            run_grid_config(ctx.as_ref(), config, no_sleep, None, None, Vec::new()).await
        {
            eprintln!("[strategy:{task_run_id}] detached grid failed: {err}");
            mark_run_failed(ctx.as_ref(), &task_run_id, &err);
            record_strategy_event(
                ctx.as_ref(),
                "strategy.grid.detached_error",
                &serde_json::json!({
                    "run_id": task_run_id,
                    "error": {
                        "category": err.category,
                        "code": err.code,
                        "message": err.message,
                    },
                }),
                Some(err.code.as_str()),
            );
        }
    });
    Ok(GridDetachedRunResult {
        run_id,
        strategy: "grid".to_string(),
        status: "started".to_string(),
        message:
            "Grid started in detached mode. Poll vulcan_strategy_status with run_id for ticks."
                .to_string(),
    })
}

fn mark_run_failed(ctx: &AppContext, run_id: &str, err: &VulcanError) {
    let runs_dir = default_strategy_runs_dir(&ctx.vulcan_dir);
    let Ok(mut ledger) = crate::strategy::log::load_run_ledger(&runs_dir, run_id) else {
        return;
    };
    let now = Utc::now().to_rfc3339();
    ledger.status = "failed".to_string();
    ledger.pause_reason = Some(format!(
        "detached runner exited with {}: {}",
        err.code, err.message
    ));
    ledger.completed_at = Some(now.clone());
    ledger.last_updated_at = now;
    let _ = crate::strategy::log::write_run_ledger(&runs_dir, &ledger);
}

pub async fn resume_grid(
    ctx: &AppContext,
    run_id: &str,
    from_step: Option<u32>,
    no_sleep: bool,
) -> Result<GridRunResult, VulcanError> {
    let runs_dir = default_strategy_runs_dir(&ctx.vulcan_dir);
    let report = crate::strategy::log::load_run_report(&runs_dir, run_id)?;
    let ledger = crate::strategy::log::load_run_ledger(&runs_dir, run_id)?;
    clear_control_request(&runs_dir, run_id)?;
    if ledger.strategy != "grid" {
        return Err(VulcanError::validation(
            "UNSUPPORTED_STRATEGY_RESUME",
            format!("resume_grid supports grid runs, got {}", ledger.strategy),
        ));
    }
    if ledger.status == "stopped" {
        return Err(VulcanError::validation(
            "STRATEGY_ALREADY_COMPLETE",
            format!("grid run {} is already {}", run_id, ledger.status),
        ));
    }
    let mut config: GridRunConfig = serde_json::from_value(report.config).map_err(|e| {
        VulcanError::io(
            "STRATEGY_CONFIG_PARSE_FAILED",
            format!("failed to parse grid config: {e}"),
        )
    })?;
    let ticks = crate::strategy::log::read_run_ticks(&runs_dir, run_id).unwrap_or_default();
    let resume_step = from_step.unwrap_or(ticks.len() as u32 + 1).max(1);
    if config.ticks < resume_step {
        config.ticks = resume_step + 59;
    }
    run_grid_config(
        ctx,
        config,
        no_sleep,
        Some(resume_step),
        Some(ledger),
        ticks,
    )
    .await
}

async fn build_grid_config(
    ctx: &AppContext,
    args: GridStartArgs,
) -> Result<(GridRunConfig, bool), VulcanError> {
    let mode = StrategyMode::from(args.mode);
    if matches!(mode, StrategyMode::ConfirmEach | StrategyMode::AutoExecute)
        && !ctx.yes
        && !ctx.dry_run
    {
        return Err(VulcanError::validation(
            "CONFIRMATION_REQUIRED",
            "Pass --yes to confirm live grid execution. Grid levels are still recorded in the strategy ledger.",
        ));
    }
    let symbol = args.symbol.to_ascii_uppercase();
    let margin_mode = StrategyMarginMode::from(args.margin_mode);
    validate_margin_config(margin_mode, args.isolated_collateral)?;
    validate_grid_range_args(&args)?;
    if args.ticks == 0 {
        return Err(VulcanError::validation(
            "INVALID_GRID_TICKS",
            "ticks must be greater than zero",
        ));
    }
    if matches!(mode, StrategyMode::ConfirmEach | StrategyMode::AutoExecute) && !ctx.dry_run {
        crate::commands::trade::resolve_wallet_and_pda(ctx, None).await?;
    }
    let (info_result, ticker_result) = tokio::join!(
        market::execute_info_inner(ctx, &symbol),
        market::execute_ticker_inner(ctx, &symbol),
    );
    let info = info_result?;
    if info.isolated_only && margin_mode == StrategyMarginMode::Cross {
        return Err(VulcanError::validation(
            "ISOLATED_ONLY_MARKET",
            format!(
                "{} is isolated-only. Use --margin-mode isolated for live grid execution.",
                symbol
            ),
        ));
    }
    let ticker = ticker_result?;
    let (lower_price, upper_price) = resolve_grid_bounds(&args, ticker.mark_price)?;
    if lower_price <= 0.0 || upper_price <= 0.0 || lower_price >= upper_price {
        return Err(VulcanError::validation(
            "INVALID_GRID_RANGE",
            "lower-price and upper-price must be positive and lower-price must be below upper-price",
        ));
    }
    let base_lot_scale = 10f64.powi(info.base_lots_decimals as i32);
    let safety_policy = StrategySafetyPolicy::with_overrides(
        args.max_total_notional_usdc,
        args.max_step_notional_usdc,
        args.max_price_drift_bps,
        args.max_exposure_ratio,
        args.reconcile_attempts,
        args.reconcile_delay_ms,
    )?;
    let levels = build_levels(
        ctx,
        &symbol,
        &args,
        lower_price,
        upper_price,
        ticker.mark_price,
        base_lot_scale,
    )
    .await?;
    if levels.is_empty() {
        return Err(VulcanError::validation(
            "EMPTY_GRID",
            "grid must contain at least one bid or ask level",
        ));
    }
    let total_notional = levels
        .iter()
        .map(|level| level.tokens * level.entry_price)
        .sum::<f64>();
    let max_level_notional = levels
        .iter()
        .map(|level| level.tokens * level.entry_price)
        .fold(0.0_f64, f64::max);
    safety_policy.validate_plan(Some(total_notional), Some(max_level_notional))?;
    validate_grid_bounds(ticker.mark_price, lower_price, upper_price)?;
    let margin_feasibility =
        if matches!(mode, StrategyMode::ConfirmEach | StrategyMode::AutoExecute) {
            Some(
                validate_live_margin_feasibility(
                    ctx,
                    &symbol,
                    total_notional,
                    Some(levels.iter().map(|level| level.size_lots).sum()),
                    args.isolated_collateral,
                )
                .await?,
            )
        } else {
            None
        };

    Ok((
        GridRunConfig {
            run_id: new_run_id(),
            run_label: args.run_label.clone(),
            wallet: strategy_wallet_context(ctx),
            symbol,
            mode,
            margin_mode,
            isolated_collateral: args.isolated_collateral,
            lower_price,
            upper_price,
            levels_per_side: args.levels_per_side,
            tokens_per_level: args.tokens_per_level,
            size_lots_per_level: args.size_lots_per_level,
            price_drift_guard_bps: args.max_price_drift_bps,
            take_profit_spacing: args.take_profit_spacing,
            stop_loss_spacing: args.stop_loss_spacing,
            interval_seconds: args.interval_seconds,
            ticks: args.ticks,
            run_until_stopped: args.run_until_stopped,
            stale_after_seconds: args.stale_after_seconds,
            slide: args.slide,
            levels,
            margin_feasibility,
            safety_policy,
        },
        args.no_sleep,
    ))
}

async fn run_grid_config(
    ctx: &AppContext,
    config: GridRunConfig,
    no_sleep: bool,
    start_step: Option<u32>,
    existing_ledger: Option<StrategyRunLedger>,
    mut ticks: Vec<StrategyTick>,
) -> Result<GridRunResult, VulcanError> {
    let started_at = existing_ledger
        .as_ref()
        .map(|ledger| ledger.started_at.clone())
        .unwrap_or_else(|| Utc::now().to_rfc3339());
    let started = Instant::now();
    let runs_dir = default_strategy_runs_dir(&ctx.vulcan_dir);
    let mut ledger = existing_ledger.unwrap_or_else(|| build_ledger(&config, &started_at));
    let _run_lock =
        StrategyRunLock::acquire(&runs_dir, &config.run_id, stale_after_seconds(&config))?;
    ledger.status = "running".to_string();
    ledger.pause_reason = None;
    ledger.run_until_stopped = config.run_until_stopped;
    ledger.stale_after_seconds = Some(stale_after_seconds(&config));
    write_run_ledger(&runs_dir, &ledger)?;

    let mut progress = progress_from_ledger(&ledger);
    let mut paused = false;
    let mut stopped = false;
    let first_tick = start_step.unwrap_or(1);

    let mut tick_number = first_tick;
    loop {
        if !config.run_until_stopped && tick_number > config.ticks {
            break;
        }
        if apply_control_request(&runs_dir, &mut ledger)? {
            stopped = ledger.status == "stopped";
            paused = ledger.status == "paused";
            break;
        }
        _run_lock.touch();
        let market = market_snapshot(ctx, &config.symbol).await?;
        if config.price_drift_guard_bps.is_some() {
            if let Err(err) = config
                .safety_policy
                .validate_market(config_center_price(&config), &market)
            {
                paused = true;
                ledger.status = "paused".to_string();
                ledger.pause_reason = Some(err.message);
            }
        }
        if !paused
            && (market.mark_price < config.lower_price || market.mark_price > config.upper_price)
        {
            paused = true;
            ledger.status = "paused".to_string();
            ledger.pause_reason = Some(format!(
                "mark price {:.6} outside grid range [{:.6}, {:.6}]",
                market.mark_price, config.lower_price, config.upper_price
            ));
        }

        let outcome = if paused {
            GridTickOutcome::paused(
                ledger
                    .pause_reason
                    .clone()
                    .unwrap_or_else(|| "paused".to_string()),
            )
        } else if tick_number == 1 && ledger.totals.submitted_count == 0 {
            place_initial_grid(ctx, &config, &mut ledger, &market).await?
        } else {
            maintain_grid(ctx, &config, &mut ledger, &market).await?
        };

        progress.cumulative_tokens = ledger.totals.filled_tokens;
        progress.cumulative_notional_usdc = ledger.totals.filled_notional_usdc;
        progress.cumulative_fees = ledger.totals.fees;
        progress.slices_filled = ledger.totals.filled_count;
        progress.remaining_slices = if config.run_until_stopped {
            0
        } else {
            config.ticks.saturating_sub(tick_number)
        };
        progress.vwap = ledger.totals.vwap;

        recompute_ledger_totals(&mut ledger);
        if let Some(reason) = outcome.pause_reason.clone() {
            paused = true;
            ledger.status = "paused".to_string();
            ledger.pause_reason = Some(reason);
        }
        write_run_ledger(&runs_dir, &ledger)?;

        let (position, account_health) = account_snapshot(ctx, &config, market.mark_price).await?;
        if !paused {
            if let Err(err) = config.safety_policy.validate_account(&account_health) {
                paused = true;
                ledger.status = "paused".to_string();
                ledger.pause_reason = Some(err.message);
            }
            if let Err(err) = config
                .safety_policy
                .validate_exposure(position.exposure_ratio)
            {
                paused = true;
                ledger.status = "paused".to_string();
                ledger.pause_reason = Some(err.message);
            }
        }

        let finite_limit_reached = !config.run_until_stopped && tick_number == config.ticks;
        let final_tick = finite_limit_reached || paused;
        let tick = StrategyTick {
            run_id: config.run_id.clone(),
            strategy: "grid".to_string(),
            mode: config.mode,
            symbol: config.symbol.clone(),
            side: StrategySide::Buy,
            tick: tick_number,
            // For grid, `slices_total` mirrors the ledger slice count (initial
            // planned levels + replacement slices recorded over the run). Keeping
            // it aligned with `summary.slices_total` prevents two same-named
            // fields from carrying different units in the same response. The
            // planned tick budget is derivable from
            // `progress.remaining_slices` + current tick number for finite runs;
            // run-until-stopped grids have no fixed denominator.
            slices_total: ledger.slices_total,
            timestamp: Utc::now().to_rfc3339(),
            elapsed_ms: started.elapsed().as_millis(),
            market,
            planned_action: outcome.planned_action,
            execution: outcome.execution,
            progress: progress.clone(),
            position,
            position_after_action: None,
            account_health,
            next_tick: StrategyNextTick {
                next_tick_at: if final_tick {
                    None
                } else {
                    Some(
                        (Utc::now() + chrono::Duration::seconds(config.interval_seconds as i64))
                            .to_rfc3339(),
                    )
                },
                delay_seconds: (!final_tick).then_some(config.interval_seconds),
                stop_reason: if paused {
                    ledger.pause_reason.clone()
                } else {
                    finite_limit_reached.then(|| "completed".to_string())
                },
            },
        };
        emit_strategy_tick(ctx, &runs_dir, &tick, render_grid_tick)?;
        ticks.push(tick);
        write_run_ledger(&runs_dir, &ledger)?;

        if paused {
            break;
        }
        if apply_control_request(&runs_dir, &mut ledger)? {
            stopped = ledger.status == "stopped";
            paused = ledger.status == "paused";
            break;
        }
        if final_tick {
            break;
        }
        if !no_sleep
            && config.interval_seconds > 0
            && wait_interval_or_shutdown(config.interval_seconds).await
        {
            paused = true;
            ledger.status = "paused".to_string();
            ledger.pause_reason = Some("runner interrupted by local shutdown signal".to_string());
            ledger.last_updated_at = Utc::now().to_rfc3339();
            write_run_ledger(&runs_dir, &ledger)?;
            break;
        }
        tick_number = tick_number.saturating_add(1);
    }

    if !paused && !stopped {
        ledger.status = "completed".to_string();
    }
    if config.mode == StrategyMode::DryRun && ledger.status == "completed" {
        mark_unfilled_levels_finalized(&mut ledger);
    }
    ledger.completed_at = Some(Utc::now().to_rfc3339());
    ledger.last_updated_at = Utc::now().to_rfc3339();
    recompute_ledger_totals(&mut ledger);
    write_run_ledger(&runs_dir, &ledger)?;

    let report = build_report(&config, &ledger, ticks, &runs_dir, &started_at);
    write_run_report(&runs_dir, &report)?;
    record_strategy_event(ctx, "strategy.grid.complete", &report, None);
    Ok(GridRunResult { report })
}

fn mark_unfilled_levels_finalized(ledger: &mut StrategyRunLedger) {
    let now = Utc::now().to_rfc3339();
    for slice in &mut ledger.slices {
        if !matches!(
            slice.status,
            StrategySliceStatus::Filled
                | StrategySliceStatus::Partial
                | StrategySliceStatus::Replaced
                | StrategySliceStatus::Cancelled
                | StrategySliceStatus::Finalized
                | StrategySliceStatus::Skipped
                | StrategySliceStatus::Failed
        ) {
            slice.status = StrategySliceStatus::Finalized;
            slice.updated_at = now.clone();
        }
    }
    ledger.last_updated_at = now;
}

async fn build_levels(
    ctx: &AppContext,
    symbol: &str,
    args: &GridStartArgs,
    lower_price: f64,
    upper_price: f64,
    mark_price: f64,
    base_lot_scale: f64,
) -> Result<Vec<GridLevel>, VulcanError> {
    if !args.bid_levels.is_empty() || !args.ask_levels.is_empty() {
        let mut levels = Vec::new();
        for input in &args.bid_levels {
            levels.push(
                parse_custom_level(ctx, symbol, input, StrategySide::Buy, base_lot_scale).await?,
            );
        }
        for input in &args.ask_levels {
            levels.push(
                parse_custom_level(ctx, symbol, input, StrategySide::Sell, base_lot_scale).await?,
            );
        }
        return Ok(levels);
    }
    if args.levels_per_side == 0 {
        return Err(VulcanError::validation(
            "INVALID_GRID_LEVELS",
            "levels-per-side must be greater than zero",
        ));
    }
    let size_lots = size_lots_per_level(args, base_lot_scale)?;
    let tokens = size_lots as f64 / base_lot_scale;
    let spacing = (upper_price - lower_price) / (args.levels_per_side * 2) as f64;
    if spacing <= 0.0 {
        return Err(VulcanError::validation(
            "INVALID_GRID_SPACING",
            "grid spacing must be positive",
        ));
    }
    let points = (0..=(args.levels_per_side * 2))
        .map(|i| lower_price + spacing * i as f64)
        .collect::<Vec<_>>();
    let mut bids = points
        .iter()
        .copied()
        .filter(|price| *price < mark_price)
        .collect::<Vec<_>>();
    bids.sort_by(|a, b| b.partial_cmp(a).unwrap_or(std::cmp::Ordering::Equal));
    bids.truncate(args.levels_per_side as usize);
    let mut asks = points
        .iter()
        .copied()
        .filter(|price| *price > mark_price)
        .collect::<Vec<_>>();
    asks.sort_by(|a, b| a.partial_cmp(b).unwrap_or(std::cmp::Ordering::Equal));
    asks.truncate(args.levels_per_side as usize);
    if bids.len() != args.levels_per_side as usize || asks.len() != args.levels_per_side as usize {
        return Err(VulcanError::validation(
            "GRID_MARK_OUT_OF_RANGE",
            "current mark price must leave enough grid levels on both sides",
        ));
    }
    let metadata = ctx.metadata().await?;
    let calc = metadata.get_market_calculator(symbol).ok_or_else(|| {
        VulcanError::validation("UNKNOWN_MARKET", format!("Unknown market: {}", symbol))
    })?;
    let mut levels = Vec::new();
    for (idx, price) in bids.into_iter().enumerate() {
        let price = snap_price_with_calc(calc, price)?;
        levels.push(generated_level(
            idx,
            StrategySide::Buy,
            price,
            size_lots,
            tokens,
            args.take_profit_spacing,
            args.stop_loss_spacing,
            calc,
        )?);
    }
    for (idx, price) in asks.into_iter().enumerate() {
        let price = snap_price_with_calc(calc, price)?;
        levels.push(generated_level(
            idx,
            StrategySide::Sell,
            price,
            size_lots,
            tokens,
            args.take_profit_spacing,
            args.stop_loss_spacing,
            calc,
        )?);
    }
    Ok(levels)
}

fn size_lots_per_level(args: &GridStartArgs, base_lot_scale: f64) -> Result<u64, VulcanError> {
    let lots = if let Some(lots) = args.size_lots_per_level {
        lots
    } else if let Some(tokens) = args.tokens_per_level {
        if tokens <= 0.0 {
            return Err(VulcanError::validation(
                "INVALID_GRID_SIZE",
                "tokens-per-level must be positive",
            ));
        }
        (tokens * base_lot_scale).round() as u64
    } else {
        return Err(VulcanError::validation(
            "MISSING_GRID_SIZE",
            "provide either --tokens-per-level or --size-lots-per-level",
        ));
    };
    if lots == 0 {
        return Err(VulcanError::validation(
            "SIZE_TOO_SMALL",
            "grid level size resolves to less than one base lot",
        ));
    }
    Ok(lots)
}

#[allow(clippy::too_many_arguments)]
fn generated_level(
    idx: usize,
    side: StrategySide,
    price: f64,
    size_lots: u64,
    tokens: f64,
    tp_spacing: Option<f64>,
    sl_spacing: Option<f64>,
    calc: &phoenix_rise::math::MarketCalculator,
) -> Result<GridLevel, VulcanError> {
    // TP/SL are derived from `price ± spacing` where spacing is in dollars, so
    // the result isn't guaranteed to be tick-aligned even when `price` is.
    // Snap defensively so Phoenix accepts the bracket leg and so any local
    // display matches the on-chain resting price.
    let take_profit_price = match (side, tp_spacing) {
        (_, Some(spacing)) if spacing <= 0.0 => {
            return Err(VulcanError::validation(
                "INVALID_GRID_TPSL",
                "take-profit spacing must be positive",
            ));
        }
        (StrategySide::Buy, Some(spacing)) => Some(snap_price_with_calc(calc, price + spacing)?),
        (StrategySide::Sell, Some(spacing)) => Some(snap_price_with_calc(calc, price - spacing)?),
        _ => None,
    };
    let stop_loss_price = match (side, sl_spacing) {
        (_, Some(spacing)) if spacing <= 0.0 => {
            return Err(VulcanError::validation(
                "INVALID_GRID_TPSL",
                "stop-loss spacing must be positive",
            ));
        }
        (StrategySide::Buy, Some(spacing)) => Some(snap_price_with_calc(calc, price - spacing)?),
        (StrategySide::Sell, Some(spacing)) => Some(snap_price_with_calc(calc, price + spacing)?),
        _ => None,
    };
    validate_tpsl(side, price, take_profit_price, stop_loss_price)?;
    Ok(GridLevel {
        id: format!("{}-{}", side.as_str(), idx + 1),
        entry_side: side,
        entry_price: price,
        size_lots,
        tokens,
        take_profit_price,
        stop_loss_price,
        source_level_id: None,
    })
}

async fn parse_custom_level(
    ctx: &AppContext,
    symbol: &str,
    input: &str,
    side: StrategySide,
    base_lot_scale: f64,
) -> Result<GridLevel, VulcanError> {
    let parts = input.split(':').collect::<Vec<_>>();
    if !(2..=4).contains(&parts.len()) {
        return Err(VulcanError::validation(
            "GRID_LEVEL_PARSE",
            format!("level expects PRICE:SIZE_LOTS[:TP][:SL], got '{}'", input),
        ));
    }
    let price = parts[0].parse::<f64>().map_err(|_| {
        VulcanError::validation("GRID_LEVEL_PARSE", format!("invalid price in '{}'", input))
    })?;
    let size_lots = parts[1].parse::<u64>().map_err(|_| {
        VulcanError::validation("GRID_LEVEL_PARSE", format!("invalid size in '{}'", input))
    })?;
    if size_lots == 0 {
        return Err(VulcanError::validation(
            "INVALID_GRID_SIZE",
            "custom level size must be greater than zero",
        ));
    }
    let tp = optional_part(parts.get(2))?;
    let sl = optional_part(parts.get(3))?;
    validate_tpsl(side, price, tp, sl)?;
    Ok(GridLevel {
        id: format!("{}-custom-{}", side.as_str(), price),
        entry_side: side,
        entry_price: validate_tick_price(ctx, symbol, price).await?,
        size_lots,
        tokens: size_lots as f64 / base_lot_scale,
        take_profit_price: tp,
        stop_loss_price: sl,
        source_level_id: None,
    })
}

fn optional_part(value: Option<&&str>) -> Result<Option<f64>, VulcanError> {
    match value.map(|v| v.trim()).filter(|v| !v.is_empty()) {
        Some(value) => value.parse::<f64>().map(Some).map_err(|_| {
            VulcanError::validation(
                "GRID_LEVEL_PARSE",
                format!("invalid TP/SL value '{}'", value),
            )
        }),
        None => Ok(None),
    }
}

// Snap a derived price (from center_on_mark/width_pct generation, replacement
// spacing, or TP/SL spacing) to the market's tick. Strict validation is reserved
// for user-supplied prices where silent snapping would mask typos; see
// validate_tick_price.
fn snap_price_with_calc(
    calc: &phoenix_rise::math::MarketCalculator,
    price: f64,
) -> Result<f64, VulcanError> {
    if price <= 0.0 {
        return Err(VulcanError::validation(
            "INVALID_GRID_PRICE",
            "grid prices must be positive",
        ));
    }
    let ticks = calc.price_to_ticks(price).map_err(|e| {
        VulcanError::validation(
            "INVALID_GRID_PRICE_TICK",
            format!("price {} is not valid: {}", price, e),
        )
    })?;
    Ok(calc.ticks_to_price(ticks))
}

async fn validate_tick_price(
    ctx: &AppContext,
    symbol: &str,
    price: f64,
) -> Result<f64, VulcanError> {
    if price <= 0.0 {
        return Err(VulcanError::validation(
            "INVALID_GRID_PRICE",
            "grid prices must be positive",
        ));
    }
    let metadata = ctx.metadata().await?;
    let calc = metadata.get_market_calculator(symbol).ok_or_else(|| {
        VulcanError::validation("UNKNOWN_MARKET", format!("Unknown market: {}", symbol))
    })?;
    let ticks = calc.price_to_ticks(price).map_err(|e| {
        VulcanError::validation(
            "INVALID_GRID_PRICE_TICK",
            format!("price {} is not valid for {}: {}", price, symbol, e),
        )
    })?;
    let canonical = calc.ticks_to_price(ticks);
    if (canonical - price).abs() > 1e-8 {
        return Err(VulcanError::validation(
            "INVALID_GRID_PRICE_TICK",
            format!(
                "price {:.8} is not tick-aligned for {}; nearest tick price is {:.8}",
                price, symbol, canonical
            ),
        ));
    }
    Ok(canonical)
}

fn validate_tpsl(
    side: StrategySide,
    entry: f64,
    tp: Option<f64>,
    sl: Option<f64>,
) -> Result<(), VulcanError> {
    if let Some(tp) = tp {
        let valid = match side {
            StrategySide::Buy => tp > entry,
            StrategySide::Sell => tp < entry,
        };
        if !valid {
            return Err(VulcanError::validation(
                "INVALID_GRID_TP",
                "buy level TP must be above entry and sell level TP must be below entry",
            ));
        }
    }
    if let Some(sl) = sl {
        let valid = match side {
            StrategySide::Buy => sl < entry,
            StrategySide::Sell => sl > entry,
        };
        if !valid {
            return Err(VulcanError::validation(
                "INVALID_GRID_SL",
                "buy level SL must be below entry and sell level SL must be above entry",
            ));
        }
    }
    Ok(())
}

fn validate_margin_config(
    margin_mode: StrategyMarginMode,
    isolated_collateral: Option<f64>,
) -> Result<(), VulcanError> {
    if margin_mode == StrategyMarginMode::Cross && isolated_collateral.is_some() {
        return Err(VulcanError::validation(
            "INVALID_MARGIN_MODE",
            "--isolated-collateral requires --margin-mode isolated",
        ));
    }
    if isolated_collateral
        .map(|value| value <= 0.0)
        .unwrap_or(false)
    {
        return Err(VulcanError::validation(
            "INVALID_ISOLATED_COLLATERAL",
            "isolated collateral must be positive",
        ));
    }
    Ok(())
}

async fn place_initial_grid(
    ctx: &AppContext,
    config: &GridRunConfig,
    ledger: &mut StrategyRunLedger,
    market: &StrategyMarketSnapshot,
) -> Result<GridTickOutcome, VulcanError> {
    let mut execution = StrategyExecution {
        action: "grid_initial_place".to_string(),
        fill_price: Some(market.mark_price),
        note: Some(tpsl_note(config)),
        ..Default::default()
    };
    match config.mode {
        StrategyMode::Paper => {
            let mut resting = 0_u32;
            let mut filled = 0_u32;
            for level in &config.levels {
                let result = place_paper_level(ctx, config, level).await?;
                if result.fill.is_some() {
                    filled += 1;
                    update_level_ledger(
                        ledger,
                        &level.id,
                        LevelLedgerUpdate {
                            status: StrategySliceStatus::Filled,
                            order_id: result.order_id,
                            fill_id: result.fill.as_ref().map(|fill| fill.fill_id.clone()),
                            fill_price: result.fill.as_ref().map(|fill| fill.price),
                            fill_tokens: result.fill.as_ref().map(|fill| fill.size_tokens),
                            fill_notional_usdc: result
                                .fill
                                .as_ref()
                                .map(|fill| fill.price * fill.size_tokens),
                            fee: result.fill.as_ref().map(|fill| fill.fee),
                            note: Some("paper grid level filled immediately".to_string()),
                        },
                    );
                } else {
                    resting += 1;
                    update_level_ledger(
                        ledger,
                        &level.id,
                        LevelLedgerUpdate {
                            status: StrategySliceStatus::Resting,
                            order_id: result.order_id,
                            note: Some(tpsl_note_for_level(level, false)),
                            ..LevelLedgerUpdate::new(StrategySliceStatus::Resting)
                        },
                    );
                }
            }
            execution.note = Some(format!(
                "paper grid placed: {} resting, {} filled. {}",
                resting,
                filled,
                tpsl_note(config)
            ));
        }
        StrategyMode::DryRun => {
            for level in &config.levels {
                update_level_ledger(
                    ledger,
                    &level.id,
                    LevelLedgerUpdate {
                        fee: Some(0.0),
                        note: Some(format!("dry run: {}", tpsl_note_for_level(level, false))),
                        ..LevelLedgerUpdate::new(StrategySliceStatus::Submitted)
                    },
                );
            }
            execution.action = "dry_run_grid_initial_place".to_string();
        }
        StrategyMode::ConfirmEach | StrategyMode::AutoExecute => {
            if config.margin_mode == StrategyMarginMode::Isolated {
                let mut submitted = 0_usize;
                let mut first_signature = None;
                for (index, level) in config.levels.iter().enumerate() {
                    let result = place_live_isolated_level(
                        ctx,
                        config,
                        level,
                        if index == 0 {
                            config.isolated_collateral
                        } else {
                            None
                        },
                    )
                    .await?;
                    if first_signature.is_none() {
                        first_signature = result.tx_signature.clone();
                    }
                    update_level_ledger(
                        ledger,
                        &level.id,
                        LevelLedgerUpdate {
                            note: Some(tpsl_note_for_level(level, true)),
                            ..LevelLedgerUpdate::new(StrategySliceStatus::Resting)
                        },
                    );
                    set_level_tx_signature(ledger, &level.id, result.tx_signature.clone());
                    submitted += 1;
                }
                execution.tx_signature = first_signature;
                execution.note = Some(format!(
                    "live isolated grid submitted {} individual limit orders. {}",
                    submitted,
                    tpsl_note(config)
                ));
                let just_placed: HashSet<String> =
                    config.levels.iter().map(|level| level.id.clone()).collect();
                if let Ok(observation) =
                    reconcile_live_level_orders(ctx, config, ledger, &just_placed).await
                {
                    execution.note = Some(format!(
                        "{} {}",
                        execution.note.unwrap_or_default(),
                        observation.summary()
                    ));
                }
            } else {
                let bids = config
                    .levels
                    .iter()
                    .filter(|level| level.entry_side == StrategySide::Buy)
                    .map(|level| trade::MultiLimitLeg {
                        price: level.entry_price,
                        size_lots: level.size_lots,
                        tp: None,
                        sl: None,
                    })
                    .collect::<Vec<_>>();
                let asks = config
                    .levels
                    .iter()
                    .filter(|level| level.entry_side == StrategySide::Sell)
                    .map(|level| trade::MultiLimitLeg {
                        price: level.entry_price,
                        size_lots: level.size_lots,
                        tp: None,
                        sl: None,
                    })
                    .collect::<Vec<_>>();
                let result = trade::execute_multi_limit_order_inner(
                    ctx,
                    &config.symbol,
                    bids,
                    asks,
                    config.slide,
                )
                .await?;
                execution.tx_signature = result.tx_signature.clone();
                for level in &config.levels {
                    update_level_ledger(
                        ledger,
                        &level.id,
                        LevelLedgerUpdate {
                            note: Some(tpsl_note_for_level(level, true)),
                            ..LevelLedgerUpdate::new(StrategySliceStatus::Resting)
                        },
                    );
                    set_level_tx_signature(ledger, &level.id, result.tx_signature.clone());
                }
                let just_placed: HashSet<String> =
                    config.levels.iter().map(|level| level.id.clone()).collect();
                let observation = reconcile_live_level_orders(ctx, config, ledger, &just_placed)
                    .await
                    .ok();
                execution.note = Some(format!(
                    "live cross-margin multi-limit submitted {} bids and {} asks. {}{}",
                    result.bids.len(),
                    result.asks.len(),
                    tpsl_note(config),
                    observation
                        .map(|observation| format!(" {}", observation.summary()))
                        .unwrap_or_default()
                ));
            }
        }
    }
    recompute_ledger_totals(ledger);
    Ok(GridTickOutcome {
        planned_action: planned_action(config, "initial_grid"),
        execution,
        pause_reason: None,
    })
}

async fn maintain_grid(
    ctx: &AppContext,
    config: &GridRunConfig,
    ledger: &mut StrategyRunLedger,
    _market: &StrategyMarketSnapshot,
) -> Result<GridTickOutcome, VulcanError> {
    match config.mode {
        StrategyMode::Paper => maintain_paper_grid(ctx, config, ledger).await,
        StrategyMode::DryRun => Ok(GridTickOutcome {
            planned_action: planned_action(config, "maintenance"),
            execution: StrategyExecution {
                action: "dry_run_grid_maintenance".to_string(),
                fee: Some(0.0),
                note: Some("dry run: no paper or live replacements submitted".to_string()),
                ..Default::default()
            },
            pause_reason: None,
        }),
        StrategyMode::ConfirmEach | StrategyMode::AutoExecute => {
            let mut observation =
                reconcile_live_level_orders(ctx, config, ledger, &HashSet::new()).await?;
            let mut placed = 0_usize;
            let mut tx_signature = None;
            let mut just_placed: HashSet<String> = HashSet::new();
            if !observation.missing_levels.is_empty() {
                // Only replace levels that the reconciler has authoritatively
                // transitioned to Filled via trade history. Levels that are
                // merely missing from the book (e.g. rejected at submission,
                // never rested) stay in `missing_levels` but keep their
                // pre-reconcile status — replacing them would stack a fresh
                // order at the flipped price every tick.
                let metadata = ctx.metadata().await?;
                let calc = metadata
                    .get_market_calculator(&config.symbol)
                    .ok_or_else(|| {
                        VulcanError::validation(
                            "UNKNOWN_MARKET",
                            format!("Unknown market: {}", &config.symbol),
                        )
                    })?;
                let replacements = observation
                    .missing_levels
                    .iter()
                    .filter(|level| {
                        level_status(ledger, &level.id) == Some(StrategySliceStatus::Filled)
                    })
                    .filter_map(|level| replacement_level(config, level, calc))
                    .collect::<Vec<_>>();
                if !replacements.is_empty() {
                    just_placed = replacements.iter().map(|level| level.id.clone()).collect();
                    let result =
                        place_live_replacements(ctx, config, ledger, &replacements).await?;
                    placed = result.placed;
                    tx_signature = result.tx_signature;
                    observation =
                        reconcile_live_level_orders(ctx, config, ledger, &just_placed).await?;
                }
            }
            Ok(GridTickOutcome {
                planned_action: planned_action(config, "maintenance"),
                execution: StrategyExecution {
                    action: "live_grid_maintenance_replace".to_string(),
                    tx_signature,
                    note: Some(format!(
                        "{} TP/SL activation remains pending until fills can be mapped authoritatively.",
                        observation.maintenance_summary(placed, &just_placed),
                    )),
                    ..Default::default()
                },
                pause_reason: None,
            })
        }
    }
}

async fn maintain_paper_grid(
    ctx: &AppContext,
    config: &GridRunConfig,
    ledger: &mut StrategyRunLedger,
) -> Result<GridTickOutcome, VulcanError> {
    let result = paper::reconcile(ctx, Some(&config.symbol)).await?;
    let metadata = ctx.metadata().await?;
    let calc = metadata
        .get_market_calculator(&config.symbol)
        .ok_or_else(|| {
            VulcanError::validation(
                "UNKNOWN_MARKET",
                format!("Unknown market: {}", &config.symbol),
            )
        })?;
    let mut replacements = Vec::new();
    let level_by_order = paper_order_level_map(ledger);
    for fill in &result.fills {
        if let Some(level_id) = level_by_order.get(&fill.order_id.clone().unwrap_or_default()) {
            if let Some(level) = config.levels.iter().find(|level| &level.id == level_id) {
                update_level_ledger(
                    ledger,
                    &level.id,
                    LevelLedgerUpdate {
                        status: StrategySliceStatus::Filled,
                        order_id: fill.order_id.clone(),
                        fill_id: Some(fill.fill_id.clone()),
                        fill_price: Some(fill.price),
                        fill_tokens: Some(fill.size_tokens),
                        fill_notional_usdc: Some(fill.price * fill.size_tokens),
                        fee: Some(fill.fee),
                        note: Some("paper grid level filled during reconciliation".to_string()),
                    },
                );
                if let Some(replacement) = replacement_level(config, level, calc) {
                    replacements.push(replacement);
                }
            }
        }
    }
    let mut placed = 0;
    for replacement in replacements {
        let result = place_paper_level(ctx, config, &replacement).await?;
        let metadata = level_metadata(&replacement, result.order_id.clone(), None);
        ledger.slices.push(StrategyLedgerSlice {
            index: ledger.slices.len() as u32 + 1,
            scheduled_at: Utc::now().to_rfc3339(),
            planned_notional_usdc: Some(replacement.tokens * replacement.entry_price),
            planned_tokens: Some(replacement.tokens),
            executable_notional_usdc: Some(replacement.tokens * replacement.entry_price),
            executable_tokens: Some(replacement.tokens),
            executable_base_lots: Some(replacement.size_lots),
            status: if result.fill.is_some() {
                StrategySliceStatus::Filled
            } else {
                StrategySliceStatus::Resting
            },
            tx_signature: None,
            fill_id: result.fill.as_ref().map(|fill| fill.fill_id.clone()),
            fill_price: result.fill.as_ref().map(|fill| fill.price),
            fill_tokens: result.fill.as_ref().map(|fill| fill.size_tokens),
            fill_notional_usdc: result
                .fill
                .as_ref()
                .map(|fill| fill.price * fill.size_tokens),
            fee: result.fill.as_ref().map(|fill| fill.fee),
            submitted_at: Some(Utc::now().to_rfc3339()),
            filled_at: result.fill.as_ref().map(|_| Utc::now().to_rfc3339()),
            updated_at: Utc::now().to_rfc3339(),
            pause_reason: None,
            error_code: None,
            note: Some("paper replacement level".to_string()),
            rounding_note: None,
            metadata: Some(metadata),
        });
        ledger.slices_total = ledger.slices.len() as u32;
        placed += 1;
    }
    recompute_ledger_totals(ledger);
    Ok(GridTickOutcome {
        planned_action: planned_action(config, "maintenance"),
        execution: StrategyExecution {
            action: "paper_grid_maintenance".to_string(),
            fill_tokens: Some(
                result
                    .fills
                    .iter()
                    .map(|fill| fill.size_tokens)
                    .sum::<f64>(),
            ),
            fill_notional_usdc: Some(
                result
                    .fills
                    .iter()
                    .map(|fill| fill.size_tokens * fill.price)
                    .sum::<f64>(),
            ),
            fee: Some(result.fills.iter().map(|fill| fill.fee).sum()),
            note: Some(format!(
                "paper reconciliation filled {} levels and placed {} replacements",
                result.fills.len(),
                placed
            )),
            ..Default::default()
        },
        pause_reason: None,
    })
}

fn replacement_level(
    config: &GridRunConfig,
    level: &GridLevel,
    calc: &phoenix_rise::math::MarketCalculator,
) -> Option<GridLevel> {
    let spacing = (config.upper_price - config.lower_price) / (config.levels_per_side * 2) as f64;
    let (side, raw_price) = match level.entry_side {
        StrategySide::Buy => (StrategySide::Sell, level.entry_price + spacing),
        StrategySide::Sell => (StrategySide::Buy, level.entry_price - spacing),
    };
    // Snap to Phoenix tick so the price we store matches the price that will rest
    // on the orderbook exactly. Without this, `live_order_matches_level`'s strict
    // (< 1e-6) price comparison fails and the order_id never gets harvested,
    // stranding the level in a "missing forever" retry loop.
    let price = snap_price_with_calc(calc, raw_price).ok()?;
    if price < config.lower_price || price > config.upper_price {
        return None;
    }
    let mut replacement = level.clone();
    replacement.id = format!(
        "{}-repl-{}-{}",
        side.as_str(),
        price,
        Utc::now().timestamp_millis()
    );
    replacement.entry_side = side;
    replacement.entry_price = price;
    replacement.source_level_id = Some(level.id.clone());
    // Snap derived TP/SL to tick (spacing is in dollars, so `price ± spacing`
    // may not land on a tick even when `price` does).
    replacement.take_profit_price = match (side, config.take_profit_spacing) {
        (StrategySide::Buy, Some(spacing)) => {
            Some(snap_price_with_calc(calc, price + spacing).ok()?)
        }
        (StrategySide::Sell, Some(spacing)) => {
            Some(snap_price_with_calc(calc, price - spacing).ok()?)
        }
        _ => None,
    };
    replacement.stop_loss_price = match (side, config.stop_loss_spacing) {
        (StrategySide::Buy, Some(spacing)) => {
            Some(snap_price_with_calc(calc, price - spacing).ok()?)
        }
        (StrategySide::Sell, Some(spacing)) => {
            Some(snap_price_with_calc(calc, price + spacing).ok()?)
        }
        _ => None,
    };
    validate_tpsl(
        side,
        replacement.entry_price,
        replacement.take_profit_price,
        replacement.stop_loss_price,
    )
    .ok()?;
    Some(replacement)
}

async fn place_paper_level(
    ctx: &AppContext,
    config: &GridRunConfig,
    level: &GridLevel,
) -> Result<crate::paper::PaperOrderResult, VulcanError> {
    let args = PaperOrderArgs {
        symbol: config.symbol.clone(),
        order_type: PaperOrderTypeArg::Limit,
        size: Some(level.size_lots as f64),
        tokens: None,
        notional_usdc: None,
        price: Some(level.entry_price),
        tp: level.take_profit_price,
        sl: level.stop_loss_price,
    };
    let side = match level.entry_side {
        StrategySide::Buy => PaperSide::Buy,
        StrategySide::Sell => PaperSide::Sell,
    };
    paper::order(ctx, side, args).await
}

async fn place_live_isolated_level(
    ctx: &AppContext,
    config: &GridRunConfig,
    level: &GridLevel,
    collateral: Option<f64>,
) -> Result<trade::OrderResult, VulcanError> {
    let side = match level.entry_side {
        StrategySide::Buy => Side::Bid,
        StrategySide::Sell => Side::Ask,
    };
    trade::execute_limit_order_inner(
        ctx,
        &config.symbol,
        level.size_lots as f64,
        level.entry_price,
        side,
        level.take_profit_price,
        level.stop_loss_price,
        true,
        collateral,
        None,
        false,
    )
    .await
}

struct LevelLedgerUpdate {
    status: StrategySliceStatus,
    order_id: Option<String>,
    fill_id: Option<String>,
    fill_price: Option<f64>,
    fill_tokens: Option<f64>,
    fill_notional_usdc: Option<f64>,
    fee: Option<f64>,
    note: Option<String>,
}

impl LevelLedgerUpdate {
    fn new(status: StrategySliceStatus) -> Self {
        Self {
            status,
            order_id: None,
            fill_id: None,
            fill_price: None,
            fill_tokens: None,
            fill_notional_usdc: None,
            fee: None,
            note: None,
        }
    }
}

fn update_level_ledger(ledger: &mut StrategyRunLedger, level_id: &str, update: LevelLedgerUpdate) {
    let now = Utc::now().to_rfc3339();
    if let Some(slice) = ledger.slices.iter_mut().find(|slice| {
        slice
            .metadata
            .as_ref()
            .and_then(|metadata| metadata.get("level_id"))
            .and_then(|value| value.as_str())
            == Some(level_id)
    }) {
        slice.status = update.status;
        slice.fill_id = update.fill_id;
        slice.fill_price = update.fill_price;
        slice.fill_tokens = update.fill_tokens;
        slice.fill_notional_usdc = update.fill_notional_usdc;
        slice.fee = update.fee;
        slice.note = update.note;
        if matches!(
            update.status,
            StrategySliceStatus::Submitted
                | StrategySliceStatus::Resting
                | StrategySliceStatus::Filled
        ) {
            slice.submitted_at.get_or_insert_with(|| now.clone());
        }
        if matches!(
            update.status,
            StrategySliceStatus::Filled | StrategySliceStatus::Partial
        ) {
            slice.filled_at = Some(now.clone());
        }
        if let Some(metadata) = slice
            .metadata
            .as_mut()
            .and_then(|value| value.as_object_mut())
        {
            if let Some(order_id) = update.order_id {
                metadata.insert("order_id".to_string(), serde_json::Value::String(order_id));
            }
        }
        slice.updated_at = now;
    }
}

fn paper_order_level_map(ledger: &StrategyRunLedger) -> BTreeMap<String, String> {
    ledger
        .slices
        .iter()
        .filter_map(|slice| {
            let metadata = slice.metadata.as_ref()?;
            let order_id = metadata.get("order_id")?.as_str()?.to_string();
            let level_id = metadata.get("level_id")?.as_str()?.to_string();
            Some((order_id, level_id))
        })
        .collect()
}

fn build_ledger(config: &GridRunConfig, started_at: &str) -> StrategyRunLedger {
    let slices = config
        .levels
        .iter()
        .enumerate()
        .map(|(index, level)| StrategyLedgerSlice {
            index: index as u32 + 1,
            scheduled_at: started_at.to_string(),
            planned_notional_usdc: Some(level.tokens * level.entry_price),
            planned_tokens: Some(level.tokens),
            executable_notional_usdc: Some(level.tokens * level.entry_price),
            executable_tokens: Some(level.tokens),
            executable_base_lots: Some(level.size_lots),
            status: StrategySliceStatus::Planned,
            tx_signature: None,
            fill_id: None,
            fill_price: None,
            fill_tokens: None,
            fill_notional_usdc: None,
            fee: None,
            submitted_at: None,
            filled_at: None,
            updated_at: started_at.to_string(),
            pause_reason: None,
            error_code: None,
            note: Some(tpsl_note_for_level(level, false)),
            rounding_note: None,
            metadata: Some(level_metadata(level, None, None)),
        })
        .collect::<Vec<_>>();
    let planned_notional = config
        .levels
        .iter()
        .map(|level| level.tokens * level.entry_price)
        .sum::<f64>();
    let planned_tokens = config.levels.iter().map(|level| level.tokens).sum::<f64>();
    StrategyRunLedger {
        run_id: config.run_id.clone(),
        strategy: "grid".to_string(),
        run_label: config.run_label.clone(),
        wallet: config.wallet.clone(),
        symbol: config.symbol.clone(),
        side: StrategySide::Buy,
        mode: config.mode,
        started_at: started_at.to_string(),
        completed_at: None,
        status: "running".to_string(),
        interval_seconds: config.interval_seconds,
        run_until_stopped: config.run_until_stopped,
        stale_after_seconds: Some(stale_after_seconds(config)),
        slices_total: config.levels.len() as u32,
        totals: StrategyLedgerTotals {
            planned_notional_usdc: Some(planned_notional),
            planned_tokens: Some(planned_tokens),
            planned_base_lots: Some(config.levels.iter().map(|level| level.size_lots).sum()),
            executable_notional_usdc: Some(planned_notional),
            executable_tokens: Some(planned_tokens),
            executable_base_lots: Some(config.levels.iter().map(|level| level.size_lots).sum()),
            ..Default::default()
        },
        slices,
        last_updated_at: started_at.to_string(),
        pause_reason: None,
        safety_policy: Some(
            serde_json::to_value(&config.safety_policy).unwrap_or(serde_json::Value::Null),
        ),
    }
}

fn set_level_tx_signature(
    ledger: &mut StrategyRunLedger,
    level_id: &str,
    tx_signature: Option<String>,
) {
    if let Some(tx_signature) = tx_signature {
        if let Some(slice) = ledger.slices.iter_mut().find(|slice| {
            slice
                .metadata
                .as_ref()
                .and_then(|metadata| metadata.get("level_id"))
                .and_then(|value| value.as_str())
                == Some(level_id)
        }) {
            slice.tx_signature = Some(tx_signature);
        }
    }
}

#[derive(Debug, Clone)]
struct LiveGridObservation {
    resting: usize,
    total: usize,
    missing: Vec<String>,
    missing_levels: Vec<GridLevel>,
    harvested: Vec<HarvestedOrderId>,
}

#[derive(Debug, Clone)]
struct HarvestedOrderId {
    level_id: String,
    order_id: String,
    order_price: String,
    order_size_remaining: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct GridLiveOrderInspection {
    pub resting: usize,
    pub total: usize,
    pub missing: Vec<String>,
    pub summary: String,
}

impl LiveGridObservation {
    fn summary(&self) -> String {
        if self.missing.is_empty() {
            format!(
                "Observed {}/{} planned live levels resting.",
                self.resting, self.total
            )
        } else {
            format!(
                "Observed {}/{} planned live levels resting; missing {}: {}.",
                self.resting,
                self.total,
                self.missing.len(),
                self.missing.join(", ")
            )
        }
    }

    /// Maintenance-tick summary that distinguishes a newly-placed replacement
    /// (awaiting orderbook propagation) from a level that is genuinely missing
    /// from the book. The plain `summary()` lumps both into "missing", which
    /// inverts cause and effect when the runner itself just submitted the
    /// "missing" order in this same tick.
    fn maintenance_summary(&self, placed: usize, just_placed: &HashSet<String>) -> String {
        let (awaiting, truly_missing): (Vec<&GridLevel>, Vec<&GridLevel>) = self
            .missing_levels
            .iter()
            .partition(|level| just_placed.contains(&level.id));
        let mut parts = vec![format!(
            "Reconciled {}/{} planned live levels resting.",
            self.resting, self.total
        )];
        parts.push(format!("Placed {} replacement order(s).", placed));
        if !awaiting.is_empty() {
            parts.push(format!(
                "Awaiting propagation for {}: {}.",
                awaiting.len(),
                awaiting
                    .iter()
                    .map(|level| level.id.clone())
                    .collect::<Vec<_>>()
                    .join(", ")
            ));
        }
        if !truly_missing.is_empty() {
            parts.push(format!(
                "Unresolved missing {}: {}.",
                truly_missing.len(),
                truly_missing
                    .iter()
                    .map(|level| format!(
                        "{} {} @ {:.2}",
                        level.entry_side.as_str(),
                        level.id,
                        level.entry_price
                    ))
                    .collect::<Vec<_>>()
                    .join(", ")
            ));
        }
        parts.join(" ")
    }

    fn inspection(&self) -> GridLiveOrderInspection {
        GridLiveOrderInspection {
            resting: self.resting,
            total: self.total,
            missing: self.missing.clone(),
            summary: self.summary(),
        }
    }
}

pub async fn inspect_live_grid_orders(
    ctx: &AppContext,
    run_id: &str,
) -> Result<GridLiveOrderInspection, VulcanError> {
    let runs_dir = default_strategy_runs_dir(&ctx.vulcan_dir);
    let ledger = crate::strategy::load_run_ledger(&runs_dir, run_id)?;
    if ledger.strategy != "grid" {
        return Err(VulcanError::validation(
            "STRATEGY_RECONCILE_UNSUPPORTED",
            "live order inspection is only supported for grid runs",
        ));
    }
    observe_live_level_orders(ctx, &ledger.symbol, &ledger)
        .await
        .map(|observation| observation.inspection())
}

async fn reconcile_live_level_orders(
    ctx: &AppContext,
    config: &GridRunConfig,
    ledger: &mut StrategyRunLedger,
    exclude_levels: &HashSet<String>,
) -> Result<LiveGridObservation, VulcanError> {
    let observation = observe_live_level_orders(ctx, &config.symbol, ledger).await?;

    // Persist freshly-harvested order_ids so later ticks can match by id exactly
    // instead of by side+price+size, which can collide if duplicate orders ever
    // share the same signature on the book.
    for harvested in &observation.harvested {
        set_level_order_id(ledger, &harvested.level_id, harvested.order_id.clone());
        set_level_note(
            ledger,
            &harvested.level_id,
            format!(
                "live order resting id={} price={} size={}",
                harvested.order_id, harvested.order_price, harvested.order_size_remaining
            ),
        );
    }

    for level in submitted_live_levels(ledger) {
        if !observation
            .missing_levels
            .iter()
            .any(|missing| missing.id == level.id)
        {
            continue;
        }

        if exclude_levels.contains(&level.id) {
            set_level_note(
                ledger,
                &level.id,
                "live order submitted in current maintenance tick; awaiting orderbook propagation, retry next tick".to_string(),
            );
            continue;
        }

        // A level that was never harvested as resting (no order_id) might have
        // been rejected at submission or never propagated to the book.
        // Replacing it would stack a fresh flipped order without ever closing
        // a prior position, so keep its pre-reconcile status and retry next
        // tick. Once the orderbook reports it as resting, future ticks will
        // harvest the order_id and the path below takes over.
        let Some(order_id) = level_order_id(ledger, &level.id) else {
            set_level_note(
                ledger,
                &level.id,
                "live order missing from book; no harvested order_id, retrying next tick"
                    .to_string(),
            );
            continue;
        };

        // A resting limit order can only leave the orderbook via fill (the
        // grid runner does not cancel mid-run), so a level that was once
        // harvested and is now missing is authoritatively filled. Limit
        // fills are guaranteed at the limit price, so fall back to the
        // level's known entry_price/tokens when trade history hasn't caught
        // up yet — the replacement gate must not depend on that secondary
        // signal.
        let reconciled = reconcile_level_with_history(ctx, config, &level, &order_id).await;
        let update = match reconciled {
            Some(execution) => LevelLedgerUpdate {
                fill_id: execution.fill_id,
                fill_price: execution.fill_price.or(Some(level.entry_price)),
                fill_tokens: execution.fill_tokens.or(Some(level.tokens)),
                fill_notional_usdc: execution
                    .fill_notional_usdc
                    .or(Some(level.entry_price * level.tokens)),
                fee: execution.fee,
                note: Some(format!(
                    "live order id={} missing from book; matched authoritative trade history",
                    order_id
                )),
                ..LevelLedgerUpdate::new(StrategySliceStatus::Filled)
            },
            None => LevelLedgerUpdate {
                fill_id: None,
                fill_price: Some(level.entry_price),
                fill_tokens: Some(level.tokens),
                fill_notional_usdc: Some(level.entry_price * level.tokens),
                fee: None,
                note: Some(format!(
                    "live order id={} missing from book; promoted to filled at limit price (trade history match pending)",
                    order_id
                )),
                ..LevelLedgerUpdate::new(StrategySliceStatus::Filled)
            },
        };
        update_level_ledger(ledger, &level.id, update);
    }

    Ok(observation)
}

async fn reconcile_level_with_history(
    ctx: &AppContext,
    config: &GridRunConfig,
    _level: &GridLevel,
    order_id: &str,
) -> Option<StrategyExecution> {
    let attempts = config.safety_policy.reconcile_attempts.max(1);
    let delay_ms = config.safety_policy.reconcile_delay_ms.max(1);
    for attempt in 1..=attempts {
        if attempt > 1 {
            tokio::time::sleep(std::time::Duration::from_millis(delay_ms)).await;
        }
        match reconcile_fill_by_order_id(ctx, &config.symbol, order_id).await {
            Ok(ReconcileOutcome::Matched(execution)) => return Some(execution),
            Ok(_) => continue,
            Err(_) => continue,
        }
    }
    None
}

async fn observe_live_level_orders(
    ctx: &AppContext,
    symbol: &str,
    ledger: &StrategyRunLedger,
) -> Result<LiveGridObservation, VulcanError> {
    let orders = trade::execute_orders_inner(ctx, Some(symbol)).await?;
    let mut resting = 0_usize;
    let mut missing = Vec::new();
    let mut missing_levels = Vec::new();
    let mut used_order_indexes = Vec::new();
    let mut harvested = Vec::new();

    let active_levels = submitted_live_levels(ledger);
    // First pass: bind levels with known order_ids to their exact orders.
    // This stops a heuristic side+price+size match from accidentally claiming
    // an order that already belongs to another level.
    for level in &active_levels {
        let Some(known_id) = level_order_id(ledger, &level.id) else {
            continue;
        };
        if let Some((order_index, _order)) =
            orders.orders.iter().enumerate().find(|(index, order)| {
                !used_order_indexes.contains(index) && order.order_id == known_id
            })
        {
            used_order_indexes.push(order_index);
            resting += 1;
        }
    }
    // Second pass: levels without a known order_id (initial placement, or fresh
    // replacements before harvest). Use heuristic match and capture the
    // exchange-assigned order_id so subsequent ticks can match by ID exactly.
    for level in &active_levels {
        if level_order_id(ledger, &level.id).is_some() {
            // Already handled in the first pass — either matched (counted in
            // `resting`) or genuinely missing (handled below).
            continue;
        }
        if let Some((order_index, order)) =
            orders.orders.iter().enumerate().find(|(index, order)| {
                !used_order_indexes.contains(index) && live_order_matches_level(order, level)
            })
        {
            used_order_indexes.push(order_index);
            resting += 1;
            harvested.push(HarvestedOrderId {
                level_id: level.id.clone(),
                order_id: order.order_id.clone(),
                order_price: order.price.clone(),
                order_size_remaining: order.size_remaining.clone(),
            });
        }
    }
    // Final pass: any level not accounted for is missing from the book.
    for level in &active_levels {
        let known_id = level_order_id(ledger, &level.id);
        let matched_by_known_id =
            known_id.as_deref().is_some_and(|id| {
                orders.orders.iter().enumerate().any(|(index, order)| {
                    used_order_indexes.contains(&index) && order.order_id == id
                })
            });
        let matched_by_harvest = harvested.iter().any(|h| h.level_id == level.id);
        if matched_by_known_id || matched_by_harvest {
            continue;
        }
        missing.push(format!(
            "{} {} @ {:.2}",
            level.entry_side.as_str(),
            level.id,
            level.entry_price
        ));
        missing_levels.push(level.clone());
    }

    Ok(LiveGridObservation {
        resting,
        total: active_levels.len(),
        missing,
        missing_levels,
        harvested,
    })
}

fn level_order_id(ledger: &StrategyRunLedger, level_id: &str) -> Option<String> {
    let slice = ledger.slices.iter().find(|slice| {
        slice
            .metadata
            .as_ref()
            .and_then(|metadata| metadata.get("level_id"))
            .and_then(|value| value.as_str())
            == Some(level_id)
    })?;
    slice
        .metadata
        .as_ref()
        .and_then(|metadata| metadata.get("order_id"))
        .and_then(|value| value.as_str())
        .map(String::from)
}

fn level_status(ledger: &StrategyRunLedger, level_id: &str) -> Option<StrategySliceStatus> {
    ledger
        .slices
        .iter()
        .find(|slice| {
            slice
                .metadata
                .as_ref()
                .and_then(|metadata| metadata.get("level_id"))
                .and_then(|value| value.as_str())
                == Some(level_id)
        })
        .map(|slice| slice.status)
}

#[derive(Debug)]
struct LiveReplacementResult {
    placed: usize,
    tx_signature: Option<String>,
}

async fn place_live_replacements(
    ctx: &AppContext,
    config: &GridRunConfig,
    ledger: &mut StrategyRunLedger,
    replacements: &[GridLevel],
) -> Result<LiveReplacementResult, VulcanError> {
    if replacements.is_empty() {
        return Ok(LiveReplacementResult {
            placed: 0,
            tx_signature: None,
        });
    }

    let mut first_signature = None;
    if config.margin_mode == StrategyMarginMode::Isolated {
        for replacement in replacements {
            let result = place_live_isolated_level(ctx, config, replacement, None).await?;
            if first_signature.is_none() {
                first_signature = result.tx_signature.clone();
            }
            append_live_level_slice(ledger, replacement, result.tx_signature.clone());
        }
    } else {
        let bids = replacements
            .iter()
            .filter(|level| level.entry_side == StrategySide::Buy)
            .map(|level| trade::MultiLimitLeg {
                price: level.entry_price,
                size_lots: level.size_lots,
                tp: None,
                sl: None,
            })
            .collect::<Vec<_>>();
        let asks = replacements
            .iter()
            .filter(|level| level.entry_side == StrategySide::Sell)
            .map(|level| trade::MultiLimitLeg {
                price: level.entry_price,
                size_lots: level.size_lots,
                tp: None,
                sl: None,
            })
            .collect::<Vec<_>>();
        let result =
            trade::execute_multi_limit_order_inner(ctx, &config.symbol, bids, asks, config.slide)
                .await?;
        first_signature = result.tx_signature.clone();
        for replacement in replacements {
            append_live_level_slice(ledger, replacement, result.tx_signature.clone());
        }
    }

    recompute_ledger_totals(ledger);
    Ok(LiveReplacementResult {
        placed: replacements.len(),
        tx_signature: first_signature,
    })
}

fn append_live_level_slice(
    ledger: &mut StrategyRunLedger,
    level: &GridLevel,
    tx_signature: Option<String>,
) {
    let now = Utc::now().to_rfc3339();
    let metadata = level_metadata(level, None, tx_signature.clone());
    ledger.slices.push(StrategyLedgerSlice {
        index: ledger.slices.len() as u32 + 1,
        scheduled_at: now.clone(),
        planned_notional_usdc: Some(level.tokens * level.entry_price),
        planned_tokens: Some(level.tokens),
        executable_notional_usdc: Some(level.tokens * level.entry_price),
        executable_tokens: Some(level.tokens),
        executable_base_lots: Some(level.size_lots),
        status: StrategySliceStatus::Resting,
        tx_signature,
        fill_id: None,
        fill_price: None,
        fill_tokens: None,
        fill_notional_usdc: None,
        fee: None,
        submitted_at: Some(now.clone()),
        filled_at: None,
        updated_at: now,
        pause_reason: None,
        error_code: None,
        note: Some(format!(
            "live replacement level from {}",
            level.source_level_id.as_deref().unwrap_or("unknown")
        )),
        rounding_note: None,
        metadata: Some(metadata),
    });
    ledger.slices_total = ledger.slices.len() as u32;
}

fn submitted_live_levels(ledger: &StrategyRunLedger) -> Vec<GridLevel> {
    ledger
        .slices
        .iter()
        .filter(|slice| {
            matches!(
                slice.status,
                StrategySliceStatus::Ready
                    | StrategySliceStatus::Submitted
                    | StrategySliceStatus::Resting
            )
        })
        .filter_map(|slice| slice.metadata.as_ref().and_then(grid_level_from_metadata))
        .collect()
}

fn grid_level_from_metadata(metadata: &serde_json::Value) -> Option<GridLevel> {
    let entry_side = match metadata.get("entry_side")?.as_str()? {
        "buy" => StrategySide::Buy,
        "sell" => StrategySide::Sell,
        _ => return None,
    };
    Some(GridLevel {
        id: metadata.get("level_id")?.as_str()?.to_string(),
        entry_side,
        entry_price: metadata.get("entry_price")?.as_f64()?,
        size_lots: metadata.get("size_lots")?.as_u64()?,
        tokens: metadata.get("tokens")?.as_f64()?,
        take_profit_price: metadata
            .get("take_profit_price")
            .and_then(|value| value.as_f64()),
        stop_loss_price: metadata
            .get("stop_loss_price")
            .and_then(|value| value.as_f64()),
        source_level_id: metadata
            .get("source_level_id")
            .and_then(|value| value.as_str())
            .map(|value| value.to_string()),
    })
}

fn live_order_matches_level(order: &trade::OrderInfo, level: &GridLevel) -> bool {
    let side_matches = match level.entry_side {
        StrategySide::Buy => order.side.eq_ignore_ascii_case("buy"),
        StrategySide::Sell => order.side.eq_ignore_ascii_case("sell"),
    };
    let price_matches = parse_f64(&order.price)
        .map(|price| (price - level.entry_price).abs() < 0.000001)
        .unwrap_or(false);
    let size_matches = parse_f64(&order.size_remaining)
        .or_else(|| parse_f64(&order.initial_size))
        .map(|size| (size - level.tokens).abs() < 0.000001)
        .unwrap_or(true);
    side_matches && price_matches && size_matches
}

fn set_level_order_id(ledger: &mut StrategyRunLedger, level_id: &str, order_id: String) {
    if let Some(slice) = level_slice_mut(ledger, level_id) {
        if let Some(metadata) = slice
            .metadata
            .as_mut()
            .and_then(|value| value.as_object_mut())
        {
            metadata.insert("order_id".to_string(), serde_json::Value::String(order_id));
        }
    }
}

fn set_level_note(ledger: &mut StrategyRunLedger, level_id: &str, note: String) {
    if let Some(slice) = level_slice_mut(ledger, level_id) {
        slice.status = StrategySliceStatus::Resting;
        slice.note = Some(note);
        slice.updated_at = Utc::now().to_rfc3339();
    }
}

fn level_slice_mut<'a>(
    ledger: &'a mut StrategyRunLedger,
    level_id: &str,
) -> Option<&'a mut StrategyLedgerSlice> {
    ledger.slices.iter_mut().find(|slice| {
        slice
            .metadata
            .as_ref()
            .and_then(|metadata| metadata.get("level_id"))
            .and_then(|value| value.as_str())
            == Some(level_id)
    })
}

fn level_metadata(
    level: &GridLevel,
    order_id: Option<String>,
    tx_signature: Option<String>,
) -> serde_json::Value {
    let mut value = serde_json::json!({
        "level_id": level.id,
        "entry_side": level.entry_side.as_str(),
        "entry_price": level.entry_price,
        "size_lots": level.size_lots,
        "tokens": level.tokens,
        "take_profit_price": level.take_profit_price,
        "stop_loss_price": level.stop_loss_price,
        "source_level_id": level.source_level_id,
    });
    if let Some(object) = value.as_object_mut() {
        if let Some(order_id) = order_id {
            object.insert("order_id".to_string(), serde_json::Value::String(order_id));
        }
        if let Some(tx_signature) = tx_signature {
            object.insert(
                "tx_signature".to_string(),
                serde_json::Value::String(tx_signature),
            );
        }
    }
    value
}

fn planned_action(config: &GridRunConfig, action: &str) -> StrategyPlannedAction {
    StrategyPlannedAction {
        action: action.to_string(),
        order_type: "limit".to_string(),
        slice_notional_usdc: Some(
            config
                .levels
                .iter()
                .map(|level| level.tokens * level.entry_price)
                .sum(),
        ),
        slice_tokens: Some(config.levels.iter().map(|level| level.tokens).sum()),
        executable_notional_usdc: Some(
            config
                .levels
                .iter()
                .map(|level| level.tokens * level.entry_price)
                .sum(),
        ),
        executable_tokens: Some(config.levels.iter().map(|level| level.tokens).sum()),
        executable_base_lots: Some(config.levels.iter().map(|level| level.size_lots).sum()),
        rounding_note: Some(format!("{} grid levels planned", config.levels.len())),
    }
}

struct GridTickOutcome {
    planned_action: StrategyPlannedAction,
    execution: StrategyExecution,
    pause_reason: Option<String>,
}

impl GridTickOutcome {
    fn paused(reason: String) -> Self {
        Self {
            planned_action: StrategyPlannedAction {
                action: "paused".to_string(),
                order_type: "limit".to_string(),
                slice_notional_usdc: None,
                slice_tokens: None,
                executable_notional_usdc: None,
                executable_tokens: None,
                executable_base_lots: None,
                rounding_note: Some(reason.clone()),
            },
            execution: StrategyExecution {
                action: "paused".to_string(),
                note: Some(reason.clone()),
                ..Default::default()
            },
            pause_reason: Some(reason),
        }
    }
}

fn progress_from_ledger(ledger: &StrategyRunLedger) -> StrategyProgress {
    StrategyProgress {
        slices_filled: ledger.totals.filled_count,
        cumulative_tokens: ledger.totals.filled_tokens,
        cumulative_notional_usdc: ledger.totals.filled_notional_usdc,
        vwap: ledger.totals.vwap,
        cumulative_fees: ledger.totals.fees,
        remaining_slices: ledger
            .slices_total
            .saturating_sub(ledger.totals.filled_count),
    }
}

fn build_report(
    config: &GridRunConfig,
    ledger: &StrategyRunLedger,
    ticks: Vec<StrategyTick>,
    runs_dir: &std::path::Path,
    started_at: &str,
) -> StrategyRunReport {
    let notes = vec![
        format!("Margin mode: {}.", config.margin_mode.as_str()),
        match config.price_drift_guard_bps {
            Some(bps) => format!("Center-relative drift guard: {:.2} bps.", bps),
            None => "Center-relative drift guard disabled; grid bounds are the price guard."
                .to_string(),
        },
        if config.run_until_stopped {
            "Lifecycle: run until stopped; local process must stay awake for maintenance.".to_string()
        } else {
            format!("Lifecycle: finite run for {} ticks.", config.ticks)
        },
        "Per-level TP/SL intent is persisted in level metadata; live TP/SL is not reported active until an actual bracket action is submitted.".to_string(),
    ];
    StrategyRunReport {
        run_id: config.run_id.clone(),
        summary: StrategyRunSummary {
            run_id: config.run_id.clone(),
            strategy: "grid".to_string(),
            mode: config.mode,
            symbol: config.symbol.clone(),
            side: StrategySide::Buy,
            started_at: started_at.to_string(),
            completed_at: ledger.completed_at.clone(),
            status: ledger.status.clone(),
            slices_completed: ledger.totals.filled_count + ledger.totals.partial_count,
            slices_total: ledger.slices_total,
            cumulative_notional_usdc: ledger.totals.filled_notional_usdc,
            cumulative_tokens: ledger.totals.filled_tokens,
            vwap: ledger.totals.vwap,
        },
        config: serde_json::to_value(config).unwrap_or(serde_json::Value::Null),
        ticks,
        log_path: crate::strategy::log::run_log_path(runs_dir, &config.run_id)
            .to_string_lossy()
            .to_string(),
        ledger: Some(ledger.clone()),
        notes,
    }
}

async fn market_snapshot(
    ctx: &AppContext,
    symbol: &str,
) -> Result<StrategyMarketSnapshot, VulcanError> {
    let ticker = market::execute_ticker_inner(ctx, symbol).await?;
    Ok(StrategyMarketSnapshot {
        mark_price: ticker.mark_price,
        mid_price: ticker.mid_price,
        funding_rate: ticker.funding_rate,
    })
}

async fn account_snapshot(
    ctx: &AppContext,
    config: &GridRunConfig,
    fallback_mark: f64,
) -> Result<(StrategyPositionSnapshot, StrategyAccountHealth), VulcanError> {
    match config.mode {
        StrategyMode::Paper => {
            let status = paper::status(ctx).await?;
            let (_, state) = crate::paper::load_state(&ctx.vulcan_dir)?;
            let positions = crate::paper::positions(&state);
            let position = positions
                .positions
                .iter()
                .find(|position| position.symbol == config.symbol)
                .map(|position| {
                    let exposure = if status.equity.abs() < f64::EPSILON {
                        0.0
                    } else {
                        (position.size_tokens * position.mark_price) / status.equity
                    };
                    StrategyPositionSnapshot {
                        side: Some(position.side.clone()),
                        size_tokens: position.size_tokens,
                        size_lots: Some(position.size_lots),
                        mark_price: Some(position.mark_price),
                        notional_usdc: position.size_tokens * position.mark_price,
                        unrealized_pnl: position.unrealized_pnl,
                        exposure_ratio: exposure,
                        leverage: Some(exposure),
                        ..Default::default()
                    }
                })
                .unwrap_or_default();
            Ok((
                position,
                StrategyAccountHealth {
                    equity: Some(status.equity),
                    collateral: None,
                    available_balance: Some(format!("{:.6}", status.balance)),
                    risk_state: Some("paper".to_string()),
                },
            ))
        }
        StrategyMode::DryRun => Ok((
            StrategyPositionSnapshot::default(),
            StrategyAccountHealth::default(),
        )),
        StrategyMode::ConfirmEach | StrategyMode::AutoExecute => {
            let margin = crate::commands::margin::execute_status_inner(ctx).await?;
            let positions = crate::commands::position::execute_list_inner(ctx).await?;
            let equity = parse_f64(&margin.effective_collateral);
            let position = positions
                .positions
                .iter()
                .find(|position| position.symbol.eq_ignore_ascii_case(&config.symbol))
                .map(|position| {
                    let size_tokens = parse_f64(&position.size).unwrap_or(0.0).abs();
                    let mark_price = parse_f64(&position.mark_price).unwrap_or(fallback_mark);
                    let notional_usdc = size_tokens * mark_price;
                    let exposure = equity
                        .filter(|equity| equity.abs() >= f64::EPSILON)
                        .map(|equity| notional_usdc / equity)
                        .unwrap_or_default();
                    let entry_price = parse_f64(&position.entry_price);
                    let liquidation_price = parse_f64(&position.liquidation_price);
                    let distance_to_liq_bps = liquidation_price.and_then(|liq| {
                        if mark_price.abs() < f64::EPSILON || liq.abs() < f64::EPSILON {
                            return None;
                        }
                        let is_long = position.side.eq_ignore_ascii_case("long")
                            || position.side.eq_ignore_ascii_case("buy");
                        let signed = if is_long {
                            (mark_price - liq) / mark_price
                        } else {
                            (liq - mark_price) / mark_price
                        };
                        Some(signed * 10_000.0)
                    });
                    let unrealized_pnl_pct = parse_f64(
                        &position
                            .unrealized_pnl_pct
                            .trim_end_matches('%')
                            .replace(',', ""),
                    );
                    StrategyPositionSnapshot {
                        side: Some(position.side.clone()),
                        size_tokens,
                        size_lots: None,
                        mark_price: Some(mark_price),
                        notional_usdc,
                        unrealized_pnl: parse_f64(&position.unrealized_pnl).unwrap_or(0.0),
                        exposure_ratio: exposure,
                        entry_price,
                        liquidation_price,
                        distance_to_liq_bps,
                        unrealized_pnl_pct,
                        initial_margin_usdc: parse_f64(&position.initial_margin),
                        maintenance_margin_usdc: parse_f64(&position.maintenance_margin),
                        leverage: Some(exposure),
                    }
                })
                .unwrap_or_default();
            Ok((
                position,
                StrategyAccountHealth {
                    equity,
                    collateral: Some(margin.collateral_balance),
                    available_balance: Some(margin.available_to_withdraw),
                    risk_state: Some(margin.risk_state),
                },
            ))
        }
    }
}

fn parse_f64(value: &str) -> Option<f64> {
    let cleaned: String = value
        .chars()
        .filter(|ch| ch.is_ascii_digit() || matches!(ch, '.' | '-' | '+'))
        .collect();
    cleaned.parse::<f64>().ok()
}

fn config_center_price(config: &GridRunConfig) -> f64 {
    (config.lower_price + config.upper_price) / 2.0
}

fn validate_grid_bounds(mark: f64, lower: f64, upper: f64) -> Result<(), VulcanError> {
    if mark <= lower || mark >= upper {
        return Err(VulcanError::validation(
            "GRID_MARK_OUT_OF_RANGE",
            format!(
                "current mark price {:.6} must be inside grid range [{:.6}, {:.6}]",
                mark, lower, upper
            ),
        ));
    }
    Ok(())
}

fn validate_grid_range_args(args: &GridStartArgs) -> Result<(), VulcanError> {
    let has_explicit = args.lower_price.is_some() || args.upper_price.is_some();
    if args.center_on_mark {
        if has_explicit {
            return Err(VulcanError::validation(
                "INVALID_GRID_RANGE",
                "--center-on-mark cannot be combined with --lower-price/--upper-price",
            ));
        }
        if args.width_pct.is_none() {
            return Err(VulcanError::validation(
                "INVALID_GRID_RANGE",
                "--center-on-mark requires --width-pct",
            ));
        }
    } else {
        if args.width_pct.is_some() {
            return Err(VulcanError::validation(
                "INVALID_GRID_RANGE",
                "--width-pct requires --center-on-mark",
            ));
        }
        if args.lower_price.is_none() || args.upper_price.is_none() {
            return Err(VulcanError::validation(
                "INVALID_GRID_RANGE",
                "specify --lower-price and --upper-price, or use --center-on-mark with --width-pct",
            ));
        }
    }
    Ok(())
}

fn resolve_grid_bounds(args: &GridStartArgs, mark_price: f64) -> Result<(f64, f64), VulcanError> {
    if args.center_on_mark {
        let width_pct = args
            .width_pct
            .expect("validated by validate_grid_range_args");
        if width_pct <= 0.0 || width_pct >= 100.0 {
            return Err(VulcanError::validation(
                "INVALID_GRID_RANGE",
                "--width-pct must be greater than 0 and less than 100",
            ));
        }
        let half = width_pct / 100.0;
        Ok((mark_price * (1.0 - half), mark_price * (1.0 + half)))
    } else {
        Ok((
            args.lower_price
                .expect("validated by validate_grid_range_args"),
            args.upper_price
                .expect("validated by validate_grid_range_args"),
        ))
    }
}

fn tpsl_note(config: &GridRunConfig) -> String {
    let with_tpsl = config
        .levels
        .iter()
        .filter(|level| level.take_profit_price.is_some() || level.stop_loss_price.is_some())
        .count();
    if with_tpsl == 0 {
        "no per-level TP/SL intent configured".to_string()
    } else if matches!(
        config.mode,
        StrategyMode::ConfirmEach | StrategyMode::AutoExecute
    ) {
        format!(
            "{} levels have TP/SL intent; live brackets are pending until fills are mapped",
            with_tpsl
        )
    } else {
        format!("{} levels have TP/SL intent", with_tpsl)
    }
}

fn tpsl_note_for_level(level: &GridLevel, live: bool) -> String {
    match (level.take_profit_price, level.stop_loss_price, live) {
        (None, None, _) => "no TP/SL configured".to_string(),
        (tp, sl, true) => format!("TP={:?} SL={:?} intent pending live fill mapping", tp, sl),
        (tp, sl, false) => format!("TP={:?} SL={:?}", tp, sl),
    }
}

fn apply_control_request(
    runs_dir: &std::path::Path,
    ledger: &mut StrategyRunLedger,
) -> Result<bool, VulcanError> {
    let Some(request) = read_control_request(runs_dir, &ledger.run_id)? else {
        return Ok(false);
    };
    match request.action {
        StrategyControlAction::Pause => {
            ledger.status = "paused".to_string();
            ledger.pause_reason = Some(control_reason(&request));
            ledger.last_updated_at = Utc::now().to_rfc3339();
            write_run_ledger(runs_dir, ledger)?;
            clear_control_request(runs_dir, &ledger.run_id)?;
            Ok(true)
        }
        StrategyControlAction::Stop => {
            ledger.status = "stopped".to_string();
            ledger.completed_at = Some(Utc::now().to_rfc3339());
            ledger.pause_reason = Some(control_reason(&request));
            ledger.last_updated_at = Utc::now().to_rfc3339();
            write_run_ledger(runs_dir, ledger)?;
            clear_control_request(runs_dir, &ledger.run_id)?;
            Ok(true)
        }
    }
}

fn control_reason(request: &StrategyControlRequest) -> String {
    request
        .reason
        .clone()
        .unwrap_or_else(|| format!("{} requested", request.action.as_str()))
}

fn stale_after_seconds(config: &GridRunConfig) -> u64 {
    config
        .stale_after_seconds
        .unwrap_or_else(|| (config.interval_seconds.saturating_mul(2)).max(180))
}

struct StrategyRunLock {
    path: PathBuf,
}

impl StrategyRunLock {
    fn acquire(
        runs_dir: &Path,
        run_id: &str,
        stale_after_seconds: u64,
    ) -> Result<Self, VulcanError> {
        fs::create_dir_all(runs_dir)
            .map_err(|e| VulcanError::io("STRATEGY_RUN_DIR_FAILED", e.to_string()))?;
        let path = runs_dir.join(format!("{run_id}.lock"));
        match OpenOptions::new().write(true).create_new(true).open(&path) {
            Ok(mut file) => {
                writeln!(
                    file,
                    "pid={}\nstarted_at={}",
                    std::process::id(),
                    Utc::now().to_rfc3339()
                )
                .map_err(|e| VulcanError::io("STRATEGY_LOCK_WRITE_FAILED", e.to_string()))?;
                Ok(Self { path })
            }
            Err(err) if err.kind() == std::io::ErrorKind::AlreadyExists => {
                if lock_is_stale(&path, stale_after_seconds) {
                    fs::remove_file(&path).map_err(|e| {
                        VulcanError::io("STRATEGY_LOCK_CLEAR_FAILED", e.to_string())
                    })?;
                    return Self::acquire(runs_dir, run_id, stale_after_seconds);
                }
                Err(VulcanError::validation(
                    "STRATEGY_RUN_LOCKED",
                    format!(
                        "strategy run {run_id} already has an active local runner lock; stop that process or wait for the lock to become stale"
                    ),
                ))
            }
            Err(err) => Err(VulcanError::io(
                "STRATEGY_LOCK_CREATE_FAILED",
                err.to_string(),
            )),
        }
    }

    fn touch(&self) {
        let _ = fs::write(
            &self.path,
            format!(
                "pid={}\nupdated_at={}\n",
                std::process::id(),
                Utc::now().to_rfc3339()
            ),
        );
    }
}

impl Drop for StrategyRunLock {
    fn drop(&mut self) {
        let _ = fs::remove_file(&self.path);
    }
}

fn lock_is_stale(path: &Path, stale_after_seconds: u64) -> bool {
    let Ok(metadata) = fs::metadata(path) else {
        return true;
    };
    let Ok(modified) = metadata.modified() else {
        return false;
    };
    modified
        .elapsed()
        .map(|elapsed| elapsed.as_secs() > stale_after_seconds)
        .unwrap_or(false)
}

async fn wait_interval_or_shutdown(seconds: u64) -> bool {
    #[cfg(unix)]
    {
        let mut term = match tokio::signal::unix::signal(
            tokio::signal::unix::SignalKind::terminate(),
        ) {
            Ok(signal) => signal,
            Err(_) => {
                tokio::select! {
                    _ = tokio::time::sleep(std::time::Duration::from_secs(seconds)) => return false,
                    _ = tokio::signal::ctrl_c() => return true,
                }
            }
        };
        tokio::select! {
            _ = tokio::time::sleep(std::time::Duration::from_secs(seconds)) => false,
            _ = tokio::signal::ctrl_c() => true,
            _ = term.recv() => true,
        }
    }
    #[cfg(not(unix))]
    {
        tokio::select! {
            _ = tokio::time::sleep(std::time::Duration::from_secs(seconds)) => false,
            _ = tokio::signal::ctrl_c() => true,
        }
    }
}

fn strategy_wallet_context(ctx: &AppContext) -> Option<StrategyWalletContext> {
    if let Some(session_wallet) = &ctx.session_wallet {
        return Some(StrategyWalletContext {
            name: session_wallet.wallet_name.clone(),
            address: session_wallet.public_key.clone(),
        });
    }
    let name = ctx.wallet_store.default_wallet().ok().flatten()?;
    let wallet = ctx.wallet_store.load(&name).ok()?;
    Some(StrategyWalletContext {
        name,
        address: wallet.public_key,
    })
}

fn new_run_id() -> String {
    format!(
        "grid-{}-{:08x}",
        Utc::now().format("%Y%m%dT%H%M%SZ"),
        rand::thread_rng().next_u32()
    )
}

fn render_grid_tick(tick: &StrategyTick) {
    println!("Grid Tick {} {}", tick.tick, tick.symbol);
    crate::output::table::render_table(
        &["Field", "Value"],
        vec![
            vec!["Action".to_string(), tick.execution.action.clone()],
            vec![
                "Planned".to_string(),
                tick.planned_action
                    .executable_notional_usdc
                    .map(|v| format!("{:.4} USDC", v))
                    .unwrap_or_else(|| "n/a".to_string()),
            ],
            vec![
                "Fill".to_string(),
                format!(
                    "{} tokens @ {} = {} USDC",
                    tick.execution
                        .fill_tokens
                        .map(|v| format!("{:.6}", v))
                        .unwrap_or_else(|| "n/a".to_string()),
                    tick.execution
                        .fill_price
                        .map(|v| format!("{:.6}", v))
                        .unwrap_or_else(|| "n/a".to_string()),
                    tick.execution
                        .fill_notional_usdc
                        .map(|v| format!("{:.4}", v))
                        .unwrap_or_else(|| "n/a".to_string())
                ),
            ],
            vec![
                "Tx".to_string(),
                tick.execution
                    .tx_signature
                    .clone()
                    .unwrap_or_else(|| "n/a".to_string()),
            ],
            vec![
                "Position".to_string(),
                format!(
                    "{} {:.6} tokens | notional {:.4} | uPnL {:+.4} | exposure {:.2}x",
                    tick.position.side.as_deref().unwrap_or("none"),
                    tick.position.size_tokens,
                    tick.position.notional_usdc,
                    tick.position.unrealized_pnl,
                    tick.position.exposure_ratio
                ),
            ],
            vec![
                "Next".to_string(),
                tick.next_tick
                    .stop_reason
                    .clone()
                    .or_else(|| {
                        tick.next_tick
                            .delay_seconds
                            .map(|delay| format!("in {}s", delay))
                    })
                    .unwrap_or_else(|| "n/a".to_string()),
            ],
        ],
    );
    if let Some(note) = &tick.execution.note {
        println!("Note: {}", note);
    }
    println!();
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::cli::strategy::StrategyModeArg;

    #[test]
    fn validates_tpsl_direction() {
        validate_tpsl(StrategySide::Buy, 100.0, Some(110.0), Some(90.0)).unwrap();
        validate_tpsl(StrategySide::Sell, 100.0, Some(90.0), Some(110.0)).unwrap();
        assert_eq!(
            validate_tpsl(StrategySide::Buy, 100.0, Some(90.0), None)
                .unwrap_err()
                .code,
            "INVALID_GRID_TP"
        );
        assert_eq!(
            validate_tpsl(StrategySide::Sell, 100.0, None, Some(90.0))
                .unwrap_err()
                .code,
            "INVALID_GRID_SL"
        );
    }

    #[test]
    fn size_lots_from_tokens_rounds_to_base_lots() {
        let args = GridStartArgs {
            symbol: "SOL".to_string(),
            lower_price: Some(90.0),
            upper_price: Some(110.0),
            center_on_mark: false,
            width_pct: None,
            levels_per_side: 2,
            tokens_per_level: Some(0.5),
            size_lots_per_level: None,
            bid_levels: vec![],
            ask_levels: vec![],
            take_profit_spacing: None,
            stop_loss_spacing: None,
            interval_seconds: 60,
            ticks: 1,
            run_until_stopped: false,
            stale_after_seconds: None,
            mode: StrategyModeArg::DryRun,
            margin_mode: crate::cli::strategy::StrategyMarginModeArg::Cross,
            isolated_collateral: None,
            run_label: None,
            slide: false,
            max_total_notional_usdc: None,
            max_step_notional_usdc: None,
            max_price_drift_bps: None,
            max_exposure_ratio: None,
            reconcile_attempts: None,
            reconcile_delay_ms: None,
            no_sleep: true,
            detached: false,
        };
        assert_eq!(size_lots_per_level(&args, 100.0).unwrap(), 50);
    }

    #[test]
    fn rejects_out_of_range_mark() {
        assert_eq!(
            validate_grid_bounds(120.0, 90.0, 110.0).unwrap_err().code,
            "GRID_MARK_OUT_OF_RANGE"
        );
    }

    #[test]
    fn snap_price_with_calc_quantizes_to_nearest_tick() {
        use phoenix_rise::math::{MarketCalculator, QuoteLotsPerBaseLotPerTick};

        // base_lot_decimals=2, tick_size=100 → 1 tick = $0.01 (ZEC-like).
        let calc = MarketCalculator::new(2, QuoteLotsPerBaseLotPerTick::new(100));

        // The exact failure from the ZEC issue log: 512.7309 is not tick-aligned.
        let snapped = snap_price_with_calc(&calc, 512.7309).unwrap();
        assert!(
            (snapped - 512.73).abs() < 1e-9,
            "expected 512.73, got {}",
            snapped
        );

        // Already-aligned price round-trips unchanged.
        let already = snap_price_with_calc(&calc, 512.73).unwrap();
        assert!((already - 512.73).abs() < 1e-9);

        // Rounds up when the unaligned residual is past the halfway point.
        let up = snap_price_with_calc(&calc, 512.736).unwrap();
        assert!((up - 512.74).abs() < 1e-9, "expected 512.74, got {}", up);

        // Non-positive prices are rejected with the same code grid validation uses.
        assert_eq!(
            snap_price_with_calc(&calc, 0.0).unwrap_err().code,
            "INVALID_GRID_PRICE"
        );
        assert_eq!(
            snap_price_with_calc(&calc, -1.0).unwrap_err().code,
            "INVALID_GRID_PRICE"
        );
    }

    fn args_with_bounds(
        lower: Option<f64>,
        upper: Option<f64>,
        center_on_mark: bool,
        width_pct: Option<f64>,
    ) -> GridStartArgs {
        GridStartArgs {
            symbol: "SOL".to_string(),
            lower_price: lower,
            upper_price: upper,
            center_on_mark,
            width_pct,
            levels_per_side: 2,
            tokens_per_level: Some(0.5),
            size_lots_per_level: None,
            bid_levels: vec![],
            ask_levels: vec![],
            take_profit_spacing: None,
            stop_loss_spacing: None,
            interval_seconds: 60,
            ticks: 1,
            run_until_stopped: false,
            stale_after_seconds: None,
            mode: StrategyModeArg::DryRun,
            margin_mode: crate::cli::strategy::StrategyMarginModeArg::Cross,
            isolated_collateral: None,
            run_label: None,
            slide: false,
            max_total_notional_usdc: None,
            max_step_notional_usdc: None,
            max_price_drift_bps: None,
            max_exposure_ratio: None,
            reconcile_attempts: None,
            reconcile_delay_ms: None,
            no_sleep: true,
            detached: false,
        }
    }

    #[test]
    fn center_on_mark_computes_bounds_from_width_pct() {
        let args = args_with_bounds(None, None, true, Some(1.0));
        validate_grid_range_args(&args).unwrap();
        let (lower, upper) = resolve_grid_bounds(&args, 100.0).unwrap();
        assert!((lower - 99.0).abs() < 1e-9);
        assert!((upper - 101.0).abs() < 1e-9);
    }

    #[test]
    fn explicit_bounds_take_precedence_when_provided() {
        let args = args_with_bounds(Some(90.0), Some(110.0), false, None);
        validate_grid_range_args(&args).unwrap();
        let (lower, upper) = resolve_grid_bounds(&args, 100.0).unwrap();
        assert_eq!((lower, upper), (90.0, 110.0));
    }

    #[test]
    fn center_on_mark_requires_width_pct() {
        let args = args_with_bounds(None, None, true, None);
        assert_eq!(
            validate_grid_range_args(&args).unwrap_err().code,
            "INVALID_GRID_RANGE"
        );
    }

    #[test]
    fn center_on_mark_rejects_explicit_bounds() {
        let args = args_with_bounds(Some(90.0), Some(110.0), true, Some(1.0));
        assert_eq!(
            validate_grid_range_args(&args).unwrap_err().code,
            "INVALID_GRID_RANGE"
        );
    }

    #[test]
    fn width_pct_requires_center_on_mark() {
        let args = args_with_bounds(Some(90.0), Some(110.0), false, Some(1.0));
        assert_eq!(
            validate_grid_range_args(&args).unwrap_err().code,
            "INVALID_GRID_RANGE"
        );
    }

    #[test]
    fn missing_bounds_without_center_on_mark_fails() {
        let args = args_with_bounds(None, None, false, None);
        assert_eq!(
            validate_grid_range_args(&args).unwrap_err().code,
            "INVALID_GRID_RANGE"
        );
    }

    #[test]
    fn width_pct_out_of_range_rejected() {
        let args = args_with_bounds(None, None, true, Some(150.0));
        assert_eq!(
            resolve_grid_bounds(&args, 100.0).unwrap_err().code,
            "INVALID_GRID_RANGE"
        );
    }

    #[test]
    fn replacement_flips_side_and_keeps_tpsl_intent() {
        let level = GridLevel {
            id: "buy-1".to_string(),
            entry_side: StrategySide::Buy,
            entry_price: 100.0,
            size_lots: 10,
            tokens: 0.1,
            take_profit_price: Some(102.0),
            stop_loss_price: Some(98.0),
            source_level_id: None,
        };
        let config = GridRunConfig {
            run_id: "grid-test".to_string(),
            run_label: None,
            wallet: None,
            symbol: "SOL".to_string(),
            mode: StrategyMode::DryRun,
            margin_mode: StrategyMarginMode::Cross,
            isolated_collateral: None,
            lower_price: 90.0,
            upper_price: 110.0,
            levels_per_side: 5,
            tokens_per_level: None,
            size_lots_per_level: Some(10),
            price_drift_guard_bps: None,
            take_profit_spacing: Some(2.0),
            stop_loss_spacing: Some(2.0),
            interval_seconds: 60,
            ticks: 1,
            run_until_stopped: false,
            stale_after_seconds: None,
            slide: false,
            levels: vec![level.clone()],
            margin_feasibility: None,
            safety_policy: StrategySafetyPolicy::default(),
        };
        use phoenix_rise::math::{MarketCalculator, QuoteLotsPerBaseLotPerTick};
        let calc = MarketCalculator::new(2, QuoteLotsPerBaseLotPerTick::new(100));
        let replacement = replacement_level(&config, &level, &calc).unwrap();
        assert_eq!(replacement.entry_side, StrategySide::Sell);
        assert_eq!(replacement.take_profit_price, Some(100.0));
        assert_eq!(replacement.stop_loss_price, Some(104.0));
        assert_eq!(replacement.source_level_id, Some("buy-1".to_string()));
    }

    #[test]
    fn replacement_snaps_to_tick() {
        use phoenix_rise::math::{MarketCalculator, QuoteLotsPerBaseLotPerTick};
        // 1 tick = $0.01 (ZEC-like market).
        let calc = MarketCalculator::new(2, QuoteLotsPerBaseLotPerTick::new(100));
        // Reproduces the ZEC failure mode: a sell at 570.71 fills, and the raw
        // replacement price (570.71 - (581.77 - 559.65) / 6 = 567.0233...) has
        // sub-tick residue. Without snapping, the level's stored entry_price
        // would never match the tick-aligned price resting on Phoenix.
        let level = GridLevel {
            id: "sell-1".to_string(),
            entry_side: StrategySide::Sell,
            entry_price: 570.71,
            size_lots: 1,
            tokens: 0.01,
            take_profit_price: None,
            stop_loss_price: None,
            source_level_id: None,
        };
        let config = GridRunConfig {
            run_id: "grid-test".to_string(),
            run_label: None,
            wallet: None,
            symbol: "ZEC".to_string(),
            mode: StrategyMode::AutoExecute,
            margin_mode: StrategyMarginMode::Cross,
            isolated_collateral: None,
            lower_price: 559.65,
            upper_price: 581.77,
            levels_per_side: 3,
            tokens_per_level: None,
            size_lots_per_level: Some(1),
            price_drift_guard_bps: None,
            take_profit_spacing: None,
            stop_loss_spacing: None,
            interval_seconds: 60,
            ticks: 1,
            run_until_stopped: false,
            stale_after_seconds: None,
            slide: true,
            levels: vec![level.clone()],
            margin_feasibility: None,
            safety_policy: StrategySafetyPolicy::default(),
        };
        let replacement = replacement_level(&config, &level, &calc).unwrap();
        // raw price = 570.71 - (581.77 - 559.65) / 6 = 570.71 - 3.6866... = 567.0233...,
        // which must snap to a $0.01 tick.
        let snapped_cents = (replacement.entry_price * 100.0).round() as i64;
        assert_eq!(snapped_cents as f64 / 100.0, replacement.entry_price);
        // Sanity: still inside grid bounds and on the correct side.
        assert_eq!(replacement.entry_side, StrategySide::Buy);
        assert!(replacement.entry_price >= config.lower_price);
        assert!(replacement.entry_price <= config.upper_price);
    }

    #[test]
    fn live_order_matching_uses_side_price_and_size() {
        let level = GridLevel {
            id: "buy-1".to_string(),
            entry_side: StrategySide::Buy,
            entry_price: 562.25,
            size_lots: 1,
            tokens: 0.01,
            take_profit_price: None,
            stop_loss_price: None,
            source_level_id: None,
        };
        let order = trade::OrderInfo {
            subaccount_index: None,
            symbol: "ZEC".to_string(),
            side: "Buy".to_string(),
            order_id: "15320".to_string(),
            price: "562.25".to_string(),
            size_remaining: "0.01".to_string(),
            initial_size: "0.01".to_string(),
            reduce_only: false,
            is_stop_loss: false,
            is_take_profit: false,
        };
        assert!(live_order_matches_level(&order, &level));

        let wrong_side = trade::OrderInfo {
            side: "Sell".to_string(),
            ..order
        };
        assert!(!live_order_matches_level(&wrong_side, &level));
    }

    fn level_slice(
        level_id: &str,
        side: StrategySide,
        price: f64,
        status: StrategySliceStatus,
    ) -> StrategyLedgerSlice {
        let side_str = match side {
            StrategySide::Buy => "buy",
            StrategySide::Sell => "sell",
        };
        let metadata = serde_json::json!({
            "level_id": level_id,
            "entry_side": side_str,
            "entry_price": price,
            "size_lots": 1u64,
            "tokens": 0.01,
        });
        StrategyLedgerSlice {
            index: 1,
            scheduled_at: "2026-01-01T00:00:00Z".to_string(),
            planned_notional_usdc: Some(price * 0.01),
            planned_tokens: Some(0.01),
            executable_notional_usdc: Some(price * 0.01),
            executable_tokens: Some(0.01),
            executable_base_lots: Some(1),
            status,
            tx_signature: Some("tx-stub".to_string()),
            fill_id: None,
            fill_price: None,
            fill_tokens: None,
            fill_notional_usdc: None,
            fee: None,
            submitted_at: Some("2026-01-01T00:00:00Z".to_string()),
            filled_at: None,
            updated_at: "2026-01-01T00:00:00Z".to_string(),
            pause_reason: None,
            error_code: None,
            note: None,
            rounding_note: None,
            metadata: Some(metadata),
        }
    }

    fn ledger_with_slices(slices: Vec<StrategyLedgerSlice>) -> StrategyRunLedger {
        let total = slices.len() as u32;
        StrategyRunLedger {
            run_id: "grid-test".to_string(),
            strategy: "grid".to_string(),
            run_label: None,
            wallet: None,
            symbol: "ZEC".to_string(),
            side: StrategySide::Buy,
            mode: StrategyMode::AutoExecute,
            started_at: "2026-01-01T00:00:00Z".to_string(),
            completed_at: None,
            status: "running".to_string(),
            interval_seconds: 60,
            run_until_stopped: true,
            stale_after_seconds: None,
            slices_total: total,
            totals: StrategyLedgerTotals::default(),
            slices,
            last_updated_at: "2026-01-01T00:00:00Z".to_string(),
            pause_reason: None,
            safety_policy: None,
        }
    }

    #[test]
    fn level_status_reads_slice_status() {
        let ledger = ledger_with_slices(vec![
            level_slice(
                "buy-1",
                StrategySide::Buy,
                100.0,
                StrategySliceStatus::Resting,
            ),
            level_slice(
                "buy-2",
                StrategySide::Buy,
                98.0,
                StrategySliceStatus::Filled,
            ),
        ]);
        assert_eq!(
            level_status(&ledger, "buy-1"),
            Some(StrategySliceStatus::Resting)
        );
        assert_eq!(
            level_status(&ledger, "buy-2"),
            Some(StrategySliceStatus::Filled)
        );
        assert_eq!(level_status(&ledger, "buy-3"), None);
    }

    #[test]
    fn level_order_id_round_trips_through_metadata() {
        let mut ledger = ledger_with_slices(vec![level_slice(
            "buy-1",
            StrategySide::Buy,
            534.49,
            StrategySliceStatus::Resting,
        )]);
        assert_eq!(level_order_id(&ledger, "buy-1"), None);
        set_level_order_id(&mut ledger, "buy-1", "18446744073709536149".to_string());
        assert_eq!(
            level_order_id(&ledger, "buy-1"),
            Some("18446744073709536149".to_string())
        );
    }

    #[test]
    fn missing_unconfirmed_level_does_not_yield_replacement() {
        // Regression: an initial bid that never rested (e.g. rejected for
        // crossing the spread) stays in observation.missing_levels every tick.
        // The maintenance filter must drop it because its status is still
        // Resting (no authoritative trade-history match), otherwise the runner
        // submits a fresh flipped replacement every minute and the book stacks.
        let ledger = ledger_with_slices(vec![
            level_slice(
                "buy-1",
                StrategySide::Buy,
                534.65,
                StrategySliceStatus::Resting,
            ),
            level_slice(
                "buy-2",
                StrategySide::Buy,
                532.65,
                StrategySliceStatus::Filled,
            ),
        ]);
        let missing = [
            GridLevel {
                id: "buy-1".to_string(),
                entry_side: StrategySide::Buy,
                entry_price: 534.65,
                size_lots: 1,
                tokens: 0.01,
                take_profit_price: None,
                stop_loss_price: None,
                source_level_id: None,
            },
            GridLevel {
                id: "buy-2".to_string(),
                entry_side: StrategySide::Buy,
                entry_price: 532.65,
                size_lots: 1,
                tokens: 0.01,
                take_profit_price: None,
                stop_loss_price: None,
                source_level_id: None,
            },
        ];
        let kept: Vec<&GridLevel> = missing
            .iter()
            .filter(|level| level_status(&ledger, &level.id) == Some(StrategySliceStatus::Filled))
            .collect();
        assert_eq!(kept.len(), 1);
        assert_eq!(kept[0].id, "buy-2");
    }

    fn level(id: &str, side: StrategySide, price: f64) -> GridLevel {
        GridLevel {
            id: id.to_string(),
            entry_side: side,
            entry_price: price,
            size_lots: 1,
            tokens: 0.01,
            take_profit_price: None,
            stop_loss_price: None,
            source_level_id: None,
        }
    }

    #[test]
    fn maintenance_summary_separates_pending_propagation_from_unresolved() {
        // The runner just placed `buy-repl-555` as a replacement for a filled
        // sell. The observation re-runs against the book before the new order
        // has propagated, so it still appears in `missing_levels`. The summary
        // must label it as "awaiting propagation", not "missing", because the
        // runner itself is the cause. A truly unresolved level (no
        // just_placed entry) stays in the "unresolved missing" bucket.
        let just_placed: HashSet<String> = ["buy-repl-555".to_string()].into_iter().collect();
        let observation = LiveGridObservation {
            resting: 5,
            total: 7,
            missing: vec![
                "buy buy-repl-555 @ 555.15".to_string(),
                "buy buy-3 @ 530.00".to_string(),
            ],
            missing_levels: vec![
                level("buy-repl-555", StrategySide::Buy, 555.15),
                level("buy-3", StrategySide::Buy, 530.00),
            ],
            harvested: Vec::new(),
        };
        let note = observation.maintenance_summary(1, &just_placed);
        assert!(note.contains("Reconciled 5/7"));
        assert!(note.contains("Placed 1 replacement"));
        assert!(note.contains("Awaiting propagation for 1: buy-repl-555"));
        assert!(note.contains("Unresolved missing 1"));
        assert!(note.contains("buy buy-3 @ 530.00"));
        // The plain summary would have called the just-placed order "missing";
        // the maintenance summary must not.
        assert!(!note.contains("missing 2"));
    }

    #[test]
    fn maintenance_summary_omits_empty_buckets() {
        let observation = LiveGridObservation {
            resting: 6,
            total: 6,
            missing: Vec::new(),
            missing_levels: Vec::new(),
            harvested: Vec::new(),
        };
        let note = observation.maintenance_summary(0, &HashSet::new());
        assert_eq!(
            note,
            "Reconciled 6/6 planned live levels resting. Placed 0 replacement order(s)."
        );
    }
}

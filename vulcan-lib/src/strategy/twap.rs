//! TWAP strategy runner.

use crate::cli::strategy::{
    StrategyMarginModeArg, StrategyModeArg, StrategySideArg, TwapStartArgs,
};
use crate::commands::{margin, market, paper, position};
use crate::context::AppContext;
use crate::error::VulcanError;
use crate::strategy::execution::{
    base_lots_to_tokens, dry_run_market, execute_live_market_base_lots, execute_paper_market,
};
use crate::strategy::log::{
    clear_control_request, default_strategy_runs_dir, read_control_request, write_run_ledger,
    write_run_report,
};
use crate::strategy::reconcile::{reconcile_fill_by_tx, ReconcileOutcome};
use crate::strategy::runner::{
    emit_strategy_tick, mark_step_ready, recompute_ledger_totals, record_strategy_event,
    update_ledger_step,
};
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
use rand::RngCore;
use serde::{Deserialize, Serialize};
use std::sync::Arc;
use std::time::Instant;

impl From<StrategyModeArg> for StrategyMode {
    fn from(value: StrategyModeArg) -> Self {
        match value {
            StrategyModeArg::Paper => Self::Paper,
            StrategyModeArg::DryRun => Self::DryRun,
            StrategyModeArg::ConfirmEach => Self::ConfirmEach,
            StrategyModeArg::AutoExecute => Self::AutoExecute,
        }
    }
}

impl From<StrategySideArg> for StrategySide {
    fn from(value: StrategySideArg) -> Self {
        match value {
            StrategySideArg::Buy => Self::Buy,
            StrategySideArg::Sell => Self::Sell,
        }
    }
}

impl From<StrategyMarginModeArg> for StrategyMarginMode {
    fn from(value: StrategyMarginModeArg) -> Self {
        match value {
            StrategyMarginModeArg::Cross => Self::Cross,
            StrategyMarginModeArg::Isolated => Self::Isolated,
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TwapRunConfig {
    pub run_id: String,
    pub run_label: Option<String>,
    pub wallet: Option<StrategyWalletContext>,
    pub symbol: String,
    pub side: StrategySide,
    pub mode: StrategyMode,
    #[serde(default)]
    pub margin_mode: StrategyMarginMode,
    pub isolated_collateral: Option<f64>,
    pub total_notional_usdc: Option<f64>,
    pub total_tokens: Option<f64>,
    pub estimated_executable_notional_usdc: Option<f64>,
    pub estimated_executable_tokens: Option<f64>,
    pub estimated_base_lots: Option<u64>,
    pub estimated_rounding_delta_usdc: Option<f64>,
    pub margin_feasibility: Option<StrategyMarginFeasibility>,
    pub slices: u32,
    pub interval_seconds: u64,
    #[serde(default)]
    pub safety_policy: StrategySafetyPolicy,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TwapRunResult {
    pub report: StrategyRunReport,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TwapDetachedRunResult {
    pub run_id: String,
    pub strategy: String,
    pub status: String,
    pub message: String,
}

pub async fn run_twap(ctx: &AppContext, args: TwapStartArgs) -> Result<TwapRunResult, VulcanError> {
    let (config, no_sleep) = build_twap_config(ctx, args).await?;
    run_twap_config(ctx, config, no_sleep, None, None, Vec::new()).await
}

pub async fn run_twap_detached(
    ctx: Arc<AppContext>,
    args: TwapStartArgs,
) -> Result<TwapDetachedRunResult, VulcanError> {
    let (config, no_sleep) = build_twap_config(ctx.as_ref(), args).await?;
    let run_id = config.run_id.clone();
    let task_run_id = run_id.clone();
    crate::strategy::watchdog::spawn_watchdog(
        crate::strategy::log::default_strategy_runs_dir(&ctx.vulcan_dir),
        run_id.clone(),
    );
    tokio::spawn(async move {
        if let Err(err) =
            run_twap_config(ctx.as_ref(), config, no_sleep, None, None, Vec::new()).await
        {
            eprintln!("[strategy:{task_run_id}] detached TWAP failed: {err}");
            record_strategy_event(
                ctx.as_ref(),
                "strategy.twap.detached_error",
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
    Ok(TwapDetachedRunResult {
        run_id,
        strategy: "twap".to_string(),
        status: "started".to_string(),
        message:
            "TWAP started in detached mode. Poll vulcan_strategy_status with run_id for ticks."
                .to_string(),
    })
}

async fn build_twap_config(
    ctx: &AppContext,
    args: TwapStartArgs,
) -> Result<(TwapRunConfig, bool), VulcanError> {
    let mode = StrategyMode::from(args.mode);
    if matches!(mode, StrategyMode::ConfirmEach | StrategyMode::AutoExecute)
        && !ctx.yes
        && !ctx.dry_run
    {
        return Err(VulcanError::validation(
            "CONFIRMATION_REQUIRED",
            "Pass --yes to confirm live TWAP execution. Each slice is still recorded in the strategy ledger.",
        ));
    }
    if args.slices == 0 {
        return Err(VulcanError::validation(
            "INVALID_SLICES",
            "slices must be greater than zero",
        ));
    }
    if args.notional_usdc.is_none() && args.tokens.is_none() {
        return Err(VulcanError::validation(
            "MISSING_TWAP_SIZE",
            "provide either --notional-usdc or --tokens",
        ));
    }
    if args.notional_usdc.map(|v| v <= 0.0).unwrap_or(false)
        || args.tokens.map(|v| v <= 0.0).unwrap_or(false)
    {
        return Err(VulcanError::validation(
            "INVALID_TWAP_SIZE",
            "TWAP size must be positive",
        ));
    }
    if matches!(mode, StrategyMode::ConfirmEach | StrategyMode::AutoExecute) && !ctx.dry_run {
        crate::commands::trade::resolve_wallet_and_pda(ctx, None).map_err(|err| {
            if err.code == "WALLET_PASSWORD_REQUIRED" {
                VulcanError::auth(
                    "WALLET_PASSWORD_REQUIRED",
                    "Live strategy was not started and no ledger was created. Wallet password is required for live signing, and non-interactive agent shells cannot answer wallet prompts. Agents should not ask the user to run this TWAP manually as a `!` shell workaround. If MCP is already configured (check with `vulcan agent mcp doctor --target <claude|cursor|codex|agentskills> --scope user`): restart the agent client so Vulcan inherits the env. If MCP is NOT yet configured: run `vulcan agent mcp install --target <…> --scope user --dangerous` and restart. `vulcan agent live-ready` is a status check, not an installer. CLI fallback: set VULCAN_WALLET_PASSWORD before launching the CLI command.",
                )
            } else if err.code == "DECRYPT_FAILED" {
                VulcanError::auth(
                    err.code.clone(),
                    format!(
                        "{} Live strategy was not started and no ledger was created. Agents should not ask the user to run this TWAP manually as a `!` shell workaround. If MCP is already configured (check with `vulcan agent mcp doctor --target <claude|cursor|codex|agentskills> --scope user`): restart the agent client. If MCP is NOT yet configured: run `vulcan agent mcp install --target <…> --scope user --dangerous` and restart. `vulcan agent live-ready` is a status check, not an installer. CLI fallback: set VULCAN_WALLET_PASSWORD before launching the CLI command.",
                        err.message
                    ),
                )
            } else {
                err
            }
        })?;
    }

    let run_id = new_run_id();
    let symbol = args.symbol.to_ascii_uppercase();
    let side = StrategySide::from(args.side);
    let margin_mode = StrategyMarginMode::from(args.margin_mode);
    validate_margin_config(margin_mode, args.isolated_collateral)?;
    let safety_policy = StrategySafetyPolicy::with_overrides(
        args.max_total_notional_usdc,
        args.max_step_notional_usdc,
        args.max_price_drift_bps,
        args.max_exposure_ratio,
        args.reconcile_attempts,
        args.reconcile_delay_ms,
    )?;
    let mut config = TwapRunConfig {
        run_id: run_id.clone(),
        run_label: args.run_label.clone(),
        wallet: strategy_wallet_context(ctx),
        symbol: symbol.clone(),
        side,
        mode,
        margin_mode,
        isolated_collateral: args.isolated_collateral,
        total_notional_usdc: args.notional_usdc,
        total_tokens: args.tokens,
        estimated_executable_notional_usdc: None,
        estimated_executable_tokens: None,
        estimated_base_lots: None,
        estimated_rounding_delta_usdc: None,
        margin_feasibility: None,
        slices: args.slices,
        interval_seconds: args.interval_seconds,
        safety_policy,
    };
    let ticker = market::execute_ticker_inner(ctx, &symbol).await?;
    let estimated_base_lots = planned_total_base_lots(ctx, &config, ticker.mark_price).await?;
    let estimated_tokens = base_lots_to_tokens(ctx, &symbol, estimated_base_lots as f64).await?;
    let estimated_notional = estimated_tokens * ticker.mark_price;
    config.estimated_base_lots = Some(estimated_base_lots);
    config.estimated_executable_tokens = Some(estimated_tokens);
    config.estimated_executable_notional_usdc = Some(estimated_notional);
    config.estimated_rounding_delta_usdc = args
        .notional_usdc
        .map(|requested| estimated_notional - requested);
    config.safety_policy.validate_plan(
        Some(estimated_notional),
        Some(estimated_notional / args.slices as f64),
    )?;
    if matches!(mode, StrategyMode::ConfirmEach | StrategyMode::AutoExecute) {
        config.margin_feasibility = Some(
            validate_live_margin_feasibility(
                ctx,
                &symbol,
                estimated_notional,
                Some(estimated_base_lots),
                config.isolated_collateral,
            )
            .await?,
        );
    }
    Ok((config, args.no_sleep))
}

pub async fn resume_twap(
    ctx: &AppContext,
    run_id: &str,
    from_step: Option<u32>,
    no_sleep: bool,
) -> Result<TwapRunResult, VulcanError> {
    let runs_dir = default_strategy_runs_dir(&ctx.vulcan_dir);
    let report = crate::strategy::log::load_run_report(&runs_dir, run_id)?;
    let mut ledger = crate::strategy::log::load_run_ledger(&runs_dir, run_id)?;
    clear_control_request(&runs_dir, run_id)?;
    if ledger.strategy != "twap" {
        return Err(VulcanError::validation(
            "UNSUPPORTED_STRATEGY_RESUME",
            format!(
                "resume currently supports twap runs, got {}",
                ledger.strategy
            ),
        ));
    }
    if matches!(
        ledger.mode,
        StrategyMode::ConfirmEach | StrategyMode::AutoExecute
    ) && !ctx.yes
        && !ctx.dry_run
    {
        return Err(VulcanError::validation(
            "CONFIRMATION_REQUIRED",
            "Pass --yes to resume a live strategy run.",
        ));
    }
    if matches!(
        ledger.mode,
        StrategyMode::ConfirmEach | StrategyMode::AutoExecute
    ) {
        let current_wallet = strategy_wallet_context(ctx);
        if current_wallet.as_ref().map(|wallet| &wallet.address)
            != ledger.wallet.as_ref().map(|wallet| &wallet.address)
        {
            return Err(VulcanError::validation(
                "STRATEGY_WALLET_MISMATCH",
                "current default wallet does not match the wallet recorded in the strategy ledger",
            ));
        }
    }
    let config: TwapRunConfig = serde_json::from_value(report.config.clone()).map_err(|e| {
        VulcanError::validation(
            "STRATEGY_CONFIG_PARSE_FAILED",
            format!("failed to parse TWAP config from report: {}", e),
        )
    })?;
    normalize_submitted_paused_live_slices(&mut ledger);
    let ticks = crate::strategy::log::read_run_ticks(&runs_dir, run_id).unwrap_or_default();
    let start_step = from_step
        .or_else(|| crate::strategy::runner::next_incomplete_step(&ledger))
        .ok_or_else(|| {
            VulcanError::validation(
                "STRATEGY_ALREADY_COMPLETE",
                "no incomplete steps remain in this strategy run",
            )
        })?;
    run_twap_config(ctx, config, no_sleep, Some(start_step), Some(ledger), ticks).await
}

async fn run_twap_config(
    ctx: &AppContext,
    config: TwapRunConfig,
    no_sleep: bool,
    start_step: Option<u32>,
    existing_ledger: Option<StrategyRunLedger>,
    mut ticks: Vec<StrategyTick>,
) -> Result<TwapRunResult, VulcanError> {
    let mode = config.mode;
    let side = config.side;
    let symbol = config.symbol.clone();
    let run_id = config.run_id.clone();
    let args_slices = config.slices;
    let started_at = existing_ledger
        .as_ref()
        .map(|ledger| ledger.started_at.clone())
        .unwrap_or_else(|| Utc::now().to_rfc3339());
    let started = Instant::now();
    let runs_dir = default_strategy_runs_dir(&ctx.vulcan_dir);
    let mut ledger = existing_ledger.unwrap_or_else(|| build_ledger(&config, &started_at));
    if start_step.is_none() {
        let initial_market = market_snapshot(ctx, &symbol).await?;
        apply_lot_aware_plan(ctx, &config, &mut ledger, initial_market.mark_price).await?;
    }
    ledger.status = "running".to_string();
    ledger.pause_reason = None;
    write_run_ledger(&runs_dir, &ledger)?;
    let mut progress = StrategyProgress {
        slices_filled: ledger.totals.filled_count,
        cumulative_tokens: ledger.totals.filled_tokens,
        cumulative_notional_usdc: ledger.totals.filled_notional_usdc,
        vwap: ledger.totals.vwap,
        cumulative_fees: ledger.totals.fees,
        remaining_slices: args_slices.saturating_sub(ledger.totals.filled_count),
    };
    let mut paused = false;
    let mut stopped = false;

    for tick_number in start_step.unwrap_or(1)..=args_slices {
        if apply_control_request(&runs_dir, &mut ledger)? {
            stopped = ledger.status == "stopped";
            paused = ledger.status == "paused";
            break;
        }
        mark_step_ready(&mut ledger, tick_number);
        write_run_ledger(&runs_dir, &ledger)?;

        let market = market_snapshot(ctx, &symbol).await?;
        let sizing =
            twap_step_sizing(ctx, &config, &ledger, tick_number, market.mark_price).await?;
        apply_step_sizing(&mut ledger, tick_number, &sizing);
        write_run_ledger(&runs_dir, &ledger)?;
        let planned = planned_action(&config, &sizing);
        let outcome = match match mode {
            StrategyMode::Paper => execute_paper_slice(ctx, &config, &sizing)
                .await
                .map(SliceOutcome::filled),
            StrategyMode::DryRun => Ok(SliceOutcome::filled(dry_run_execution(
                &config, &sizing, &market,
            ))),
            StrategyMode::ConfirmEach | StrategyMode::AutoExecute => {
                let isolated_collateral = if config.margin_mode == StrategyMarginMode::Isolated
                    && ledger.totals.submitted_count == 0
                {
                    config.isolated_collateral
                } else {
                    None
                };
                execute_live_slice(ctx, &config, &sizing, &market, isolated_collateral).await
            }
        } {
            Ok(outcome) => outcome,
            Err(err) => SliceOutcome::paused(
                StrategyExecution {
                    action: "error".to_string(),
                    note: Some(format!("{}: {}", err.code, err.message)),
                    ..Default::default()
                },
                format!("{}: {}", err.code, err.message),
            ),
        };
        let execution = outcome.execution;
        if let (Some(tokens), Some(notional)) =
            (execution.fill_tokens, execution.fill_notional_usdc)
        {
            progress.slices_filled += 1;
            progress.cumulative_tokens += tokens;
            progress.cumulative_notional_usdc += notional;
            progress.vwap = Some(progress.cumulative_notional_usdc / progress.cumulative_tokens);
        }
        if let Some(fee) = execution.fee {
            progress.cumulative_fees += fee;
        }
        progress.remaining_slices = args_slices.saturating_sub(tick_number);

        update_ledger_step(&mut ledger, tick_number, outcome.status, &execution);
        if let Some(reason) = outcome.pause_reason {
            paused = true;
            ledger.status = "paused".to_string();
            ledger.pause_reason = Some(reason);
        }
        recompute_ledger_totals(&mut ledger);
        write_run_ledger(&runs_dir, &ledger)?;

        let (position, account_health) = match mode {
            StrategyMode::Paper => paper_position_snapshot(ctx, &symbol).await?,
            StrategyMode::DryRun => (
                StrategyPositionSnapshot::default(),
                StrategyAccountHealth::default(),
            ),
            StrategyMode::ConfirmEach => {
                match live_position_snapshot(ctx, &symbol, market.mark_price).await {
                    Ok(snapshot) => snapshot,
                    Err(err) if paused => (
                        StrategyPositionSnapshot::default(),
                        StrategyAccountHealth {
                            risk_state: Some(format!("snapshot_error: {}", err.code)),
                            ..Default::default()
                        },
                    ),
                    Err(err) => return Err(err),
                }
            }
            StrategyMode::AutoExecute => {
                match live_position_snapshot(ctx, &symbol, market.mark_price).await {
                    Ok(snapshot) => snapshot,
                    Err(err) => (
                        StrategyPositionSnapshot::default(),
                        StrategyAccountHealth {
                            risk_state: Some(format!("snapshot_error: {}", err.code)),
                            ..Default::default()
                        },
                    ),
                }
            }
        };
        let final_tick = tick_number == args_slices || paused;
        let tick = StrategyTick {
            run_id: run_id.clone(),
            strategy: "twap".to_string(),
            mode,
            symbol: symbol.clone(),
            side,
            tick: tick_number,
            slices_total: args_slices,
            timestamp: Utc::now().to_rfc3339(),
            elapsed_ms: started.elapsed().as_millis(),
            market,
            planned_action: planned,
            execution,
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
                    (tick_number == args_slices).then(|| "completed".to_string())
                },
            },
        };
        emit_strategy_tick(ctx, &runs_dir, &tick, render_live_tick)?;
        ticks.push(tick);

        if paused {
            break;
        }
        if apply_control_request(&runs_dir, &mut ledger)? {
            stopped = ledger.status == "stopped";
            paused = ledger.status == "paused";
            break;
        }
        if !final_tick && !no_sleep && config.interval_seconds > 0 {
            tokio::time::sleep(std::time::Duration::from_secs(config.interval_seconds)).await;
        }
    }

    if !paused && !stopped {
        ledger.status = "completed".to_string();
    }
    ledger.completed_at = Some(Utc::now().to_rfc3339());
    ledger.last_updated_at = Utc::now().to_rfc3339();
    recompute_ledger_totals(&mut ledger);
    write_run_ledger(&runs_dir, &ledger)?;

    let summary = StrategyRunSummary {
        run_id: run_id.clone(),
        strategy: "twap".to_string(),
        mode,
        symbol,
        side,
        started_at,
        completed_at: ledger.completed_at.clone(),
        status: ledger.status.clone(),
        slices_completed: progress.slices_filled,
        slices_total: args_slices,
        cumulative_notional_usdc: progress.cumulative_notional_usdc,
        cumulative_tokens: progress.cumulative_tokens,
        vwap: progress.vwap,
    };
    let notes = vec![
        format_conversion_note(&config, &ledger.totals),
        format!("Margin mode: {}.", config.margin_mode.as_str()),
        "Paper fills come from local paper state; live fills advance after transaction submission and authoritative history reconciliation continues in the background when the indexer lags.".to_string(),
    ];
    let report = StrategyRunReport {
        run_id: run_id.clone(),
        summary,
        config: serde_json::to_value(&config).unwrap_or(serde_json::Value::Null),
        ticks,
        log_path: crate::strategy::log::run_log_path(&runs_dir, &run_id)
            .to_string_lossy()
            .to_string(),
        ledger: Some(ledger),
        notes,
    };
    write_run_report(&runs_dir, &report)?;
    record_strategy_event(ctx, "strategy.twap.complete", &report, None);
    Ok(TwapRunResult { report })
}

fn strategy_wallet_context(ctx: &AppContext) -> Option<StrategyWalletContext> {
    if let Some(wallet) = &ctx.session_wallet {
        return Some(StrategyWalletContext {
            name: wallet.wallet_name.clone(),
            address: wallet.public_key.clone(),
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
        "twap-{}-{:08x}",
        Utc::now().format("%Y%m%dT%H%M%SZ"),
        rand::thread_rng().next_u32()
    )
}

fn normalize_submitted_paused_live_slices(ledger: &mut StrategyRunLedger) {
    if !matches!(
        ledger.mode,
        StrategyMode::ConfirmEach | StrategyMode::AutoExecute
    ) {
        return;
    }
    let now = Utc::now().to_rfc3339();
    let mut changed = false;
    for slice in &mut ledger.slices {
        if slice.status == StrategySliceStatus::Paused
            && slice.tx_signature.is_some()
            && (slice.fill_tokens.is_some() || slice.fill_notional_usdc.is_some())
        {
            slice.status = StrategySliceStatus::Filled;
            slice.filled_at.get_or_insert_with(|| now.clone());
            slice.updated_at = now.clone();
            slice.note = Some(format!(
                "{}; normalized as filled before resume to avoid duplicate submission",
                slice
                    .note
                    .clone()
                    .unwrap_or_else(|| "live slice submitted before pause".to_string())
            ));
            changed = true;
        }
    }
    if changed {
        ledger.status = "running".to_string();
        ledger.pause_reason = None;
        ledger.last_updated_at = now;
        recompute_ledger_totals(ledger);
    }
}

#[derive(Debug)]
struct SliceOutcome {
    execution: StrategyExecution,
    status: StrategySliceStatus,
    pause_reason: Option<String>,
}

impl SliceOutcome {
    fn filled(execution: StrategyExecution) -> Self {
        Self {
            execution,
            status: StrategySliceStatus::Filled,
            pause_reason: None,
        }
    }

    fn paused(mut execution: StrategyExecution, reason: impl Into<String>) -> Self {
        let reason = reason.into();
        if execution.note.is_none() {
            execution.note = Some(reason.clone());
        }
        Self {
            execution,
            status: StrategySliceStatus::Paused,
            pause_reason: Some(reason),
        }
    }
}

#[derive(Debug, Clone)]
struct TwapStepSizing {
    planned_notional_usdc: Option<f64>,
    planned_tokens: Option<f64>,
    executable_notional_usdc: Option<f64>,
    executable_tokens: Option<f64>,
    executable_base_lots: Option<u64>,
    rounding_note: Option<String>,
}

async fn apply_lot_aware_plan(
    ctx: &AppContext,
    config: &TwapRunConfig,
    ledger: &mut StrategyRunLedger,
    mark_price: f64,
) -> Result<(), VulcanError> {
    let mut executable_notional = 0.0;
    let mut executable_tokens = 0.0;
    let target_lots = planned_total_base_lots(ctx, config, mark_price).await?;
    let mut executable_lots = 0;
    let mut planned_lots = target_lots;
    let cap_lots = step_cap_base_lots(ctx, config, mark_price).await?;
    for slice in &mut ledger.slices {
        let remaining_steps = config.slices.saturating_sub(slice.index).saturating_add(1);
        let wanted_lots = planned_lots.div_ceil(remaining_steps as u64);
        let lots = cap_lots
            .map(|cap| wanted_lots.min(cap))
            .unwrap_or(wanted_lots);
        planned_lots = planned_lots.saturating_sub(lots);
        let mut sizing = sizing_from_lots(ctx, config, lots, mark_price).await?;
        if let Some(cap) = cap_lots {
            if wanted_lots > cap {
                sizing.rounding_note = Some(format!(
                    "catch-up wanted {} lots but max_step_notional_usdc allows {} lots; this run may underfill",
                    wanted_lots, cap
                ));
            }
        }
        slice.executable_notional_usdc = sizing.executable_notional_usdc;
        slice.executable_tokens = sizing.executable_tokens;
        slice.executable_base_lots = sizing.executable_base_lots;
        slice.rounding_note = sizing.rounding_note;
        executable_notional += sizing.executable_notional_usdc.unwrap_or(0.0);
        executable_tokens += sizing.executable_tokens.unwrap_or(0.0);
        executable_lots += lots;
    }
    ledger.totals.planned_base_lots = Some(target_lots);
    ledger.totals.executable_notional_usdc = Some(executable_notional);
    ledger.totals.executable_tokens = Some(executable_tokens);
    ledger.totals.executable_base_lots = Some(executable_lots);
    Ok(())
}

async fn twap_step_sizing(
    ctx: &AppContext,
    config: &TwapRunConfig,
    ledger: &StrategyRunLedger,
    tick_number: u32,
    mark_price: f64,
) -> Result<TwapStepSizing, VulcanError> {
    let remaining_steps = config.slices.saturating_sub(tick_number).saturating_add(1);
    let filled_lots = ledger
        .slices
        .iter()
        .filter(|slice| {
            slice.index < tick_number
                && matches!(
                    slice.status,
                    StrategySliceStatus::Filled | StrategySliceStatus::Partial
                )
        })
        .filter_map(|slice| slice.executable_base_lots)
        .sum::<u64>();
    let target_lots = planned_total_base_lots(ctx, config, mark_price).await?;
    let remaining_lots = target_lots.saturating_sub(filled_lots);
    let uncapped_lots = remaining_lots.div_ceil(remaining_steps as u64);
    let cap_lots = step_cap_base_lots(ctx, config, mark_price).await?;
    let lots = cap_lots
        .map(|cap| uncapped_lots.min(cap))
        .unwrap_or(uncapped_lots);
    let mut sizing = sizing_from_lots(ctx, config, lots, mark_price).await?;
    if let Some(cap) = cap_lots {
        if uncapped_lots > cap {
            sizing.rounding_note = Some(format!(
                "catch-up wanted {} lots but max_step_notional_usdc allows {} lots; this run may underfill",
                uncapped_lots, cap
            ));
        }
    }
    Ok(sizing)
}

async fn planned_total_base_lots(
    ctx: &AppContext,
    config: &TwapRunConfig,
    mark_price: f64,
) -> Result<u64, VulcanError> {
    let decimals = base_lots_decimals(ctx, &config.symbol).await?;
    let scale = 10f64.powi(decimals as i32);
    let lots = if let Some(tokens) = config.total_tokens {
        (tokens * scale).round()
    } else if let Some(notional) = config.total_notional_usdc {
        ((notional / mark_price) * scale).round()
    } else {
        0.0
    };
    if lots < 1.0 {
        return Err(VulcanError::validation(
            "SIZE_TOO_SMALL",
            "TWAP target resolves to less than one base lot",
        ));
    }
    Ok(lots as u64)
}

async fn step_cap_base_lots(
    ctx: &AppContext,
    config: &TwapRunConfig,
    mark_price: f64,
) -> Result<Option<u64>, VulcanError> {
    if !matches!(config.mode, StrategyMode::AutoExecute) {
        return Ok(None);
    }
    let decimals = base_lots_decimals(ctx, &config.symbol).await?;
    let scale = 10f64.powi(decimals as i32);
    let lot_notional = mark_price / scale;
    let lots = (config.safety_policy.max_step_notional_usdc / lot_notional).floor() as u64;
    if lots == 0 {
        return Err(VulcanError::validation(
            "MAX_STEP_BELOW_LOT_SIZE",
            format!(
                "max_step_notional_usdc {:.4} is below one base lot for {} at current mark ({:.4} USDC)",
                config.safety_policy.max_step_notional_usdc,
                config.symbol,
                lot_notional
            ),
        ));
    }
    Ok(Some(lots))
}

async fn sizing_from_lots(
    ctx: &AppContext,
    config: &TwapRunConfig,
    lots: u64,
    mark_price: f64,
) -> Result<TwapStepSizing, VulcanError> {
    let tokens = base_lots_to_tokens(ctx, &config.symbol, lots as f64).await?;
    let notional = tokens * mark_price;
    let planned_notional_usdc = config
        .total_notional_usdc
        .map(|notional| notional / config.slices as f64);
    let planned_tokens = config
        .total_tokens
        .map(|tokens| tokens / config.slices as f64);
    let planned_value = planned_notional_usdc.or_else(|| planned_tokens.map(|t| t * mark_price));
    let rounding_note = planned_value.and_then(|planned| {
        let delta = notional - planned;
        (delta.abs() > 0.0001).then(|| {
            format!(
                "lot-aware executable size is {:.4} USDC ({:.6} tokens, {} base lots), {:+.4} USDC vs even slice",
                notional, tokens, lots, delta
            )
        })
    });
    Ok(TwapStepSizing {
        planned_notional_usdc,
        planned_tokens,
        executable_notional_usdc: Some(notional),
        executable_tokens: Some(tokens),
        executable_base_lots: Some(lots),
        rounding_note,
    })
}

async fn base_lots_decimals(ctx: &AppContext, symbol: &str) -> Result<i8, VulcanError> {
    let metadata = ctx.metadata().await?;
    let market = metadata.get_market(symbol).ok_or_else(|| {
        VulcanError::validation("UNKNOWN_MARKET", format!("Unknown market: {}", symbol))
    })?;
    Ok(market.base_lots_decimals)
}

fn apply_step_sizing(ledger: &mut StrategyRunLedger, tick_number: u32, sizing: &TwapStepSizing) {
    if let Some(slice) = ledger
        .slices
        .iter_mut()
        .find(|slice| slice.index == tick_number)
    {
        slice.executable_notional_usdc = sizing.executable_notional_usdc;
        slice.executable_tokens = sizing.executable_tokens;
        slice.executable_base_lots = sizing.executable_base_lots;
        slice.rounding_note = sizing.rounding_note.clone();
        slice.planned_notional_usdc = sizing.planned_notional_usdc;
        slice.planned_tokens = sizing.planned_tokens;
    }
}

fn render_live_tick(tick: &StrategyTick) {
    println!(
        "TWAP Tick {}/{} {}",
        tick.tick, tick.slices_total, tick.symbol
    );
    crate::output::table::render_table(
        &["Field", "Value"],
        vec![
            vec![
                "Action".to_string(),
                format!("{} {}", tick.side.as_str(), tick.execution.action),
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
                "Cumulative".to_string(),
                format!(
                    "{:.4} USDC / {:.6} tokens | VWAP {}",
                    tick.progress.cumulative_notional_usdc,
                    tick.progress.cumulative_tokens,
                    tick.progress
                        .vwap
                        .map(|v| format!("{:.6}", v))
                        .unwrap_or_else(|| "n/a".to_string())
                ),
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
    if let Some(note) = tick
        .planned_action
        .rounding_note
        .as_ref()
        .or(tick.execution.note.as_ref())
    {
        println!("Note: {}", note);
    }
    println!();
}

fn format_conversion_note(config: &TwapRunConfig, totals: &StrategyLedgerTotals) -> String {
    let requested = if let Some(notional) = config.total_notional_usdc {
        format!("{:.4} USDC", notional)
    } else if let Some(tokens) = config.total_tokens {
        format!("{:.6} {}", tokens, config.symbol)
    } else {
        "unknown target".to_string()
    };
    format!(
        "TWAP conversion: requested {} resolves to {} planned base lots; executable plan is {} base lots across {} ticks ({} / {}).",
        requested,
        totals
            .planned_base_lots
            .map(|v| v.to_string())
            .unwrap_or_else(|| "n/a".to_string()),
        totals
            .executable_base_lots
            .map(|v| v.to_string())
            .unwrap_or_else(|| "n/a".to_string()),
        config.slices,
        totals
            .executable_tokens
            .map(|v| format!("{:.6} {}", v, config.symbol))
            .unwrap_or_else(|| "n/a".to_string()),
        totals
            .executable_notional_usdc
            .map(|v| format!("{:.4} USDC", v))
            .unwrap_or_else(|| "n/a".to_string())
    )
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

fn build_ledger(config: &TwapRunConfig, started_at: &str) -> StrategyRunLedger {
    let started = chrono::DateTime::parse_from_rfc3339(started_at)
        .map(|dt| dt.with_timezone(&Utc))
        .unwrap_or_else(|_| Utc::now());
    let slice_notional = config
        .total_notional_usdc
        .map(|notional| notional / config.slices as f64);
    let slice_tokens = config
        .total_tokens
        .map(|tokens| tokens / config.slices as f64);
    let slices = (1..=config.slices)
        .map(|index| {
            let scheduled_at = started
                + chrono::Duration::seconds(config.interval_seconds as i64 * (index - 1) as i64);
            StrategyLedgerSlice {
                index,
                scheduled_at: scheduled_at.to_rfc3339(),
                planned_notional_usdc: slice_notional,
                planned_tokens: slice_tokens,
                executable_notional_usdc: None,
                executable_tokens: None,
                executable_base_lots: None,
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
                note: None,
                rounding_note: None,
                metadata: None,
            }
        })
        .collect();
    StrategyRunLedger {
        run_id: config.run_id.clone(),
        strategy: "twap".to_string(),
        run_label: config.run_label.clone(),
        wallet: config.wallet.clone(),
        symbol: config.symbol.clone(),
        side: config.side,
        mode: config.mode,
        started_at: started_at.to_string(),
        completed_at: None,
        status: "running".to_string(),
        interval_seconds: config.interval_seconds,
        run_until_stopped: false,
        stale_after_seconds: None,
        slices_total: config.slices,
        totals: StrategyLedgerTotals {
            planned_notional_usdc: config.total_notional_usdc,
            planned_tokens: config.total_tokens,
            planned_base_lots: config.estimated_base_lots,
            executable_notional_usdc: config.estimated_executable_notional_usdc,
            executable_tokens: config.estimated_executable_tokens,
            executable_base_lots: config.estimated_base_lots,
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

fn planned_action(config: &TwapRunConfig, sizing: &TwapStepSizing) -> StrategyPlannedAction {
    StrategyPlannedAction {
        action: format!("{}_slice", config.side.as_str()),
        order_type: "market".to_string(),
        slice_notional_usdc: sizing.planned_notional_usdc,
        slice_tokens: sizing.planned_tokens,
        executable_notional_usdc: sizing.executable_notional_usdc,
        executable_tokens: sizing.executable_tokens,
        executable_base_lots: sizing.executable_base_lots,
        rounding_note: sizing.rounding_note.clone(),
    }
}

async fn execute_paper_slice(
    ctx: &AppContext,
    config: &TwapRunConfig,
    sizing: &TwapStepSizing,
) -> Result<StrategyExecution, VulcanError> {
    execute_paper_market(
        ctx,
        &config.symbol,
        config.side,
        sizing.executable_tokens,
        None,
    )
    .await
}

fn dry_run_execution(
    config: &TwapRunConfig,
    sizing: &TwapStepSizing,
    market: &StrategyMarketSnapshot,
) -> StrategyExecution {
    dry_run_market(
        config.side,
        sizing.executable_tokens,
        sizing.executable_notional_usdc,
        market,
    )
}

async fn execute_live_slice(
    ctx: &AppContext,
    config: &TwapRunConfig,
    sizing: &TwapStepSizing,
    market: &StrategyMarketSnapshot,
    isolated_collateral: Option<f64>,
) -> Result<SliceOutcome, VulcanError> {
    let base_lots = sizing.executable_base_lots.ok_or_else(|| {
        VulcanError::validation(
            "MISSING_EXECUTABLE_SIZE",
            "TWAP step did not resolve to base lots",
        )
    })?;
    let mut execution = execute_live_market_base_lots(
        ctx,
        &config.symbol,
        config.side,
        base_lots,
        market,
        config.margin_mode,
        isolated_collateral,
    )
    .await?;
    if execution.note.is_none() {
        execution.note = sizing.rounding_note.clone();
    }

    if execution.action.starts_with("dry_run_live_") {
        return Ok(SliceOutcome::filled(execution));
    }

    let Some(tx_signature) = execution.tx_signature.clone() else {
        return Ok(SliceOutcome::paused(
            execution,
            "live slice submitted without a transaction signature; pausing",
        ));
    };

    execution.note = Some(format!(
        "submitted tx {}; reconciling in background",
        tx_signature
    ));
    spawn_background_reconciliation(ctx, config, tx_signature, execution.clone());
    Ok(SliceOutcome::filled(execution))
}

fn spawn_background_reconciliation(
    ctx: &AppContext,
    config: &TwapRunConfig,
    tx_signature: String,
    fallback: StrategyExecution,
) {
    let ctx = Arc::new(ctx.clone());
    let symbol = config.symbol.clone();
    let run_id = config.run_id.clone();
    let attempts = config.safety_policy.reconcile_attempts.max(1);
    let delay_ms = config.safety_policy.reconcile_delay_ms.max(1);
    tokio::spawn(async move {
        for attempt in 1..=attempts {
            if attempt > 1 {
                tokio::time::sleep(std::time::Duration::from_millis(delay_ms)).await;
            }
            match reconcile_fill_by_tx(ctx.as_ref(), &symbol, &tx_signature, &fallback).await {
                Ok(ReconcileOutcome::Matched(execution)) => {
                    record_strategy_event(
                        ctx.as_ref(),
                        "strategy.twap.background_reconcile_matched",
                        &serde_json::json!({
                            "run_id": run_id,
                            "symbol": symbol,
                            "tx_signature": tx_signature,
                            "execution": execution,
                        }),
                        None,
                    );
                    return;
                }
                Ok(ReconcileOutcome::Missing | ReconcileOutcome::Pending) => {}
                Ok(ReconcileOutcome::Ambiguous(matches)) => {
                    record_strategy_event(
                        ctx.as_ref(),
                        "strategy.twap.background_reconcile_ambiguous",
                        &serde_json::json!({
                            "run_id": run_id,
                            "symbol": symbol,
                            "tx_signature": tx_signature,
                            "matches": matches,
                        }),
                        Some("STRATEGY_RECONCILE_AMBIGUOUS"),
                    );
                    return;
                }
                Ok(ReconcileOutcome::Failed(reason)) => {
                    record_strategy_event(
                        ctx.as_ref(),
                        "strategy.twap.background_reconcile_failed",
                        &serde_json::json!({
                            "run_id": run_id,
                            "symbol": symbol,
                            "tx_signature": tx_signature,
                            "reason": reason,
                        }),
                        Some("STRATEGY_RECONCILE_FAILED"),
                    );
                    return;
                }
                Err(err) => {
                    let code = err.code.clone();
                    record_strategy_event(
                        ctx.as_ref(),
                        "strategy.twap.background_reconcile_error",
                        &serde_json::json!({
                            "run_id": run_id,
                            "symbol": symbol,
                            "tx_signature": tx_signature,
                            "error": {
                                "category": err.category,
                                "code": err.code,
                                "message": err.message,
                            },
                        }),
                        Some(code.as_str()),
                    );
                    return;
                }
            }
        }
        record_strategy_event(
            ctx.as_ref(),
            "strategy.twap.background_reconcile_pending",
            &serde_json::json!({
                "run_id": run_id,
                "symbol": symbol,
                "tx_signature": tx_signature,
                "attempts": attempts,
            }),
            None,
        );
    });
}

pub(crate) async fn paper_position_snapshot_pub(
    ctx: &AppContext,
    symbol: &str,
) -> Result<(StrategyPositionSnapshot, StrategyAccountHealth), VulcanError> {
    paper_position_snapshot(ctx, symbol).await
}

pub(crate) async fn live_position_snapshot_pub(
    ctx: &AppContext,
    symbol: &str,
    fallback_mark: f64,
) -> Result<(StrategyPositionSnapshot, StrategyAccountHealth), VulcanError> {
    live_position_snapshot(ctx, symbol, fallback_mark).await
}

async fn paper_position_snapshot(
    ctx: &AppContext,
    symbol: &str,
) -> Result<(StrategyPositionSnapshot, StrategyAccountHealth), VulcanError> {
    let status = paper::status(ctx).await?;
    let (_, state) = crate::paper::load_state(&ctx.vulcan_dir)?;
    let positions = crate::paper::positions(&state);
    let position = positions
        .positions
        .iter()
        .find(|position| position.symbol == symbol)
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
    let health = StrategyAccountHealth {
        equity: Some(status.equity),
        collateral: None,
        available_balance: Some(format!("{:.6}", status.balance)),
        risk_state: Some("paper".to_string()),
    };
    Ok((position, health))
}

async fn live_position_snapshot(
    ctx: &AppContext,
    symbol: &str,
    fallback_mark: f64,
) -> Result<(StrategyPositionSnapshot, StrategyAccountHealth), VulcanError> {
    let margin = margin::execute_status_inner(ctx).await?;
    let positions = position::execute_list_inner(ctx).await?;
    let equity = parse_f64(&margin.effective_collateral);
    let symbol_upper = symbol.to_ascii_uppercase();
    let position = positions
        .positions
        .iter()
        .find(|position| {
            position
                .symbol
                .split_whitespace()
                .next()
                .unwrap_or(position.symbol.as_str())
                .eq_ignore_ascii_case(&symbol_upper)
        })
        .map(|position| {
            let size_tokens = parse_f64(&position.size).unwrap_or(0.0).abs();
            let mark_price = parse_f64(&position.mark_price).or(Some(fallback_mark));
            let mark_resolved = mark_price.unwrap_or(fallback_mark);
            let notional_usdc = size_tokens * mark_resolved;
            let exposure_ratio = if let Some(equity) = equity {
                if equity.abs() < f64::EPSILON {
                    0.0
                } else {
                    notional_usdc / equity
                }
            } else {
                0.0
            };
            let entry_price = parse_f64(&position.entry_price);
            let liquidation_price = parse_f64(&position.liquidation_price);
            let distance_to_liq_bps = liquidation_price.and_then(|liq| {
                if mark_resolved.abs() < f64::EPSILON || liq.abs() < f64::EPSILON {
                    return None;
                }
                let is_long = position.side.eq_ignore_ascii_case("long")
                    || position.side.eq_ignore_ascii_case("buy");
                // Long: safer when mark > liq. Short: safer when mark < liq.
                let signed = if is_long {
                    (mark_resolved - liq) / mark_resolved
                } else {
                    (liq - mark_resolved) / mark_resolved
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
                mark_price,
                notional_usdc,
                unrealized_pnl: parse_f64(&position.unrealized_pnl).unwrap_or(0.0),
                exposure_ratio,
                entry_price,
                liquidation_price,
                distance_to_liq_bps,
                unrealized_pnl_pct,
                initial_margin_usdc: parse_f64(&position.initial_margin),
                maintenance_margin_usdc: parse_f64(&position.maintenance_margin),
                leverage: Some(exposure_ratio),
            }
        })
        .unwrap_or_default();
    let health = StrategyAccountHealth {
        equity,
        collateral: Some(margin.collateral_balance),
        available_balance: Some(margin.available_to_withdraw),
        risk_state: Some(margin.risk_state),
    };
    Ok((position, health))
}

fn parse_f64(value: &str) -> Option<f64> {
    let cleaned: String = value
        .chars()
        .filter(|ch| ch.is_ascii_digit() || matches!(ch, '.' | '-' | '+'))
        .collect();
    cleaned.parse::<f64>().ok()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn dry_run_execution_sizes_notional_slice() {
        let config = TwapRunConfig {
            run_id: "run".to_string(),
            run_label: None,
            wallet: None,
            symbol: "SOL".to_string(),
            side: StrategySide::Buy,
            mode: StrategyMode::DryRun,
            margin_mode: StrategyMarginMode::Cross,
            isolated_collateral: None,
            total_notional_usdc: Some(100.0),
            total_tokens: None,
            estimated_executable_notional_usdc: None,
            estimated_executable_tokens: None,
            estimated_base_lots: None,
            estimated_rounding_delta_usdc: None,
            margin_feasibility: None,
            slices: 4,
            interval_seconds: 30,
            safety_policy: StrategySafetyPolicy::default(),
        };
        let market = StrategyMarketSnapshot {
            mark_price: 50.0,
            mid_price: 50.0,
            funding_rate: 0.0,
        };
        let sizing = TwapStepSizing {
            planned_notional_usdc: Some(25.0),
            planned_tokens: None,
            executable_notional_usdc: Some(25.0),
            executable_tokens: Some(0.5),
            executable_base_lots: Some(50),
            rounding_note: None,
        };
        let execution = dry_run_execution(&config, &sizing, &market);
        assert_eq!(execution.fill_notional_usdc, Some(25.0));
        assert_eq!(execution.fill_tokens, Some(0.5));
    }

    #[test]
    fn ledger_builds_one_planned_slice_per_twap_slice() {
        let config = TwapRunConfig {
            run_id: "run".to_string(),
            run_label: Some("test".to_string()),
            wallet: None,
            symbol: "ZEC".to_string(),
            side: StrategySide::Buy,
            mode: StrategyMode::Paper,
            margin_mode: StrategyMarginMode::Cross,
            isolated_collateral: None,
            total_notional_usdc: Some(120.0),
            total_tokens: None,
            estimated_executable_notional_usdc: None,
            estimated_executable_tokens: None,
            estimated_base_lots: None,
            estimated_rounding_delta_usdc: None,
            margin_feasibility: None,
            slices: 12,
            interval_seconds: 3600,
            safety_policy: StrategySafetyPolicy::default(),
        };
        let ledger = build_ledger(&config, "2026-05-09T00:00:00Z");
        assert_eq!(ledger.slices.len(), 12);
        assert_eq!(ledger.slices[0].planned_notional_usdc, Some(10.0));
        assert_eq!(ledger.slices[0].status, StrategySliceStatus::Planned);
        assert_eq!(ledger.totals.planned_notional_usdc, Some(120.0));
    }

    #[test]
    fn ledger_totals_count_filled_and_paused_slices() {
        let config = TwapRunConfig {
            run_id: "run".to_string(),
            run_label: None,
            wallet: None,
            symbol: "SOL".to_string(),
            side: StrategySide::Buy,
            mode: StrategyMode::Paper,
            margin_mode: StrategyMarginMode::Cross,
            isolated_collateral: None,
            total_notional_usdc: Some(100.0),
            total_tokens: None,
            estimated_executable_notional_usdc: None,
            estimated_executable_tokens: None,
            estimated_base_lots: None,
            estimated_rounding_delta_usdc: None,
            margin_feasibility: None,
            slices: 2,
            interval_seconds: 60,
            safety_policy: StrategySafetyPolicy::default(),
        };
        let mut ledger = build_ledger(&config, "2026-05-09T00:00:00Z");
        update_ledger_step(
            &mut ledger,
            1,
            StrategySliceStatus::Filled,
            &StrategyExecution {
                action: "paper".to_string(),
                fill_id: Some("fill".to_string()),
                tx_signature: None,
                fill_price: Some(50.0),
                fill_tokens: Some(1.0),
                fill_notional_usdc: Some(50.0),
                fee: Some(0.1),
                note: None,
            },
        );
        update_ledger_step(
            &mut ledger,
            2,
            StrategySliceStatus::Paused,
            &StrategyExecution {
                action: "market-buy".to_string(),
                fill_id: None,
                tx_signature: Some("tx".to_string()),
                fill_price: None,
                fill_tokens: None,
                fill_notional_usdc: None,
                fee: None,
                note: Some("missing history".to_string()),
            },
        );
        recompute_ledger_totals(&mut ledger);
        assert_eq!(ledger.totals.filled_count, 1);
        assert_eq!(ledger.totals.paused_count, 1);
        assert_eq!(ledger.totals.filled_notional_usdc, 50.0);
        assert_eq!(ledger.totals.vwap, Some(50.0));
    }

    #[test]
    fn resume_normalizes_submitted_paused_live_slice_as_filled() {
        let config = TwapRunConfig {
            run_id: "run".to_string(),
            run_label: None,
            wallet: None,
            symbol: "SOL".to_string(),
            side: StrategySide::Buy,
            mode: StrategyMode::AutoExecute,
            margin_mode: StrategyMarginMode::Cross,
            isolated_collateral: None,
            total_notional_usdc: Some(100.0),
            total_tokens: None,
            estimated_executable_notional_usdc: None,
            estimated_executable_tokens: None,
            estimated_base_lots: None,
            estimated_rounding_delta_usdc: None,
            margin_feasibility: None,
            slices: 2,
            interval_seconds: 60,
            safety_policy: StrategySafetyPolicy::default(),
        };
        let mut ledger = build_ledger(&config, "2026-05-09T00:00:00Z");
        update_ledger_step(
            &mut ledger,
            1,
            StrategySliceStatus::Paused,
            &StrategyExecution {
                action: "market_buy".to_string(),
                fill_id: None,
                tx_signature: Some("tx".to_string()),
                fill_price: Some(97.45),
                fill_tokens: Some(0.11),
                fill_notional_usdc: Some(10.7195),
                fee: None,
                note: Some("trade history reconciliation is pending".to_string()),
            },
        );

        normalize_submitted_paused_live_slices(&mut ledger);

        assert_eq!(ledger.slices[0].status, StrategySliceStatus::Filled);
        assert_eq!(ledger.totals.filled_count, 1);
        assert_eq!(
            crate::strategy::runner::next_incomplete_step(&ledger),
            Some(2)
        );
    }
}

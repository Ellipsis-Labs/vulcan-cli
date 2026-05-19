//! Technical-analysis-driven strategy runner.
//!
//! Sibling to `twap.rs` / `grid.rs`. Each tick:
//! 1. Read pause/stop control request.
//! 2. Snapshot market + position state.
//! 3. Compute every distinct indicator referenced by the rule set (deduped).
//! 4. Walk rules top-down; the first rule whose condition fires (and is not
//!    cooldown-locked / filter-skipped) executes its action and ends the tick.
//! 5. Update ledger + emit `StrategyTick`.

use crate::commands::market;
use crate::context::AppContext;
use crate::error::VulcanError;
use crate::indicators::{self, IndicatorRequest, IndicatorSeries};
use crate::strategy::execution::{
    dry_run_market, execute_live_market_base_lots, execute_paper_market,
};
use crate::strategy::log::{
    clear_control_request, default_strategy_runs_dir, read_control_request, write_run_ledger,
    write_run_report,
};
use crate::strategy::runner::{emit_strategy_tick, record_strategy_event};
use crate::strategy::safety::StrategySafetyPolicy;
use crate::strategy::ta_rule::{
    Action, CooldownPolicy, CooldownState, EvaluationContext, IndicatorRef, Rule, SizeSpec,
    TaStrategyConfig,
};
use crate::strategy::twap::{live_position_snapshot_pub, paper_position_snapshot_pub};
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
use std::collections::BTreeMap;
use std::sync::Arc;
use std::time::Instant;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TaRunConfig {
    pub run_id: String,
    pub run_label: Option<String>,
    pub wallet: Option<StrategyWalletContext>,
    pub mode: StrategyMode,
    pub max_ticks: u32,
    #[serde(default)]
    pub run_until_stopped: bool,
    pub strategy_config: TaStrategyConfig,
    #[serde(default)]
    pub safety_policy: StrategySafetyPolicy,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TaRunResult {
    pub report: StrategyRunReport,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TaDetachedRunResult {
    pub run_id: String,
    pub strategy: String,
    pub status: String,
    pub message: String,
}

#[derive(Debug, Clone)]
pub struct TaStartInput {
    pub config: TaStrategyConfig,
    pub mode: StrategyMode,
    pub max_ticks: u32,
    pub run_until_stopped: bool,
    pub run_label: Option<String>,
    pub no_sleep: bool,
    pub safety_overrides: SafetyOverrides,
}

#[derive(Debug, Clone, Default)]
pub struct SafetyOverrides {
    pub max_total_notional_usdc: Option<f64>,
    pub max_step_notional_usdc: Option<f64>,
    pub max_price_drift_bps: Option<f64>,
    pub max_exposure_ratio: Option<f64>,
    pub reconcile_attempts: Option<u32>,
    pub reconcile_delay_ms: Option<u64>,
}

pub async fn run_ta(ctx: &AppContext, input: TaStartInput) -> Result<TaRunResult, VulcanError> {
    let (config, no_sleep) = build_ta_config(ctx, input).await?;
    let run_id = config.run_id.clone();
    let runs_dir = default_strategy_runs_dir(&ctx.vulcan_dir);
    match run_ta_config(ctx, config, no_sleep, None, Vec::new()).await {
        Ok(result) => Ok(result),
        Err(err) => {
            // Mark the run as errored so it's visible from monitor/status and resumable.
            if let Ok(mut ledger) = crate::strategy::log::load_run_ledger(&runs_dir, &run_id) {
                ledger.status = "errored".to_string();
                ledger.completed_at = Some(Utc::now().to_rfc3339());
                ledger.pause_reason = Some(format!("{}: {}", err.code, err.message));
                ledger.last_updated_at = Utc::now().to_rfc3339();
                let _ = write_run_ledger(&runs_dir, &ledger);
                // Best-effort: also write a final report so resume works.
                if let Ok(report) = crate::strategy::log::load_run_report(&runs_dir, &run_id) {
                    let mut final_report = report;
                    final_report.summary.status = "errored".to_string();
                    final_report.summary.completed_at = ledger.completed_at.clone();
                    final_report.ledger = Some(ledger);
                    final_report.notes.push(format!(
                        "Run terminated with error {}: {}",
                        err.code, err.message
                    ));
                    let _ = write_run_report(&runs_dir, &final_report);
                }
            }
            Err(err)
        }
    }
}

pub async fn run_ta_detached(
    ctx: Arc<AppContext>,
    input: TaStartInput,
) -> Result<TaDetachedRunResult, VulcanError> {
    let (config, no_sleep) = build_ta_config(ctx.as_ref(), input).await?;
    let run_id = config.run_id.clone();
    let task_run_id = run_id.clone();
    crate::strategy::watchdog::spawn_watchdog(
        default_strategy_runs_dir(&ctx.vulcan_dir),
        run_id.clone(),
    );
    tokio::spawn(async move {
        if let Err(err) = run_ta_config(ctx.as_ref(), config, no_sleep, None, Vec::new()).await {
            eprintln!("[strategy:{task_run_id}] detached TA strategy failed: {err}");
            record_strategy_event(
                ctx.as_ref(),
                "strategy.ta.detached_error",
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
    Ok(TaDetachedRunResult {
        run_id,
        strategy: "ta".to_string(),
        status: "started".to_string(),
        message:
            "TA strategy started in detached mode. Poll vulcan_strategy_status with run_id for ticks."
                .to_string(),
    })
}

async fn build_ta_config(
    ctx: &AppContext,
    input: TaStartInput,
) -> Result<(TaRunConfig, bool), VulcanError> {
    if input.config.rules.is_empty() {
        return Err(VulcanError::validation(
            "INVALID_TA_STRATEGY_CONFIG",
            "rules must contain at least one rule",
        ));
    }
    if matches!(
        input.mode,
        StrategyMode::ConfirmEach | StrategyMode::AutoExecute
    ) && !ctx.yes
        && !ctx.dry_run
    {
        return Err(VulcanError::validation(
            "CONFIRMATION_REQUIRED",
            "Pass --yes to confirm live TA strategy execution. Every firing is recorded in the strategy ledger.",
        ));
    }
    if matches!(
        input.mode,
        StrategyMode::ConfirmEach | StrategyMode::AutoExecute
    ) && !ctx.dry_run
    {
        // Run the full live-readiness check up front. Failing fast here means we
        // never allocate a run id or write a ledger for a strategy that can't
        // possibly execute live — and the error message lists every blocker with
        // a concrete remedy command for the agent to act on.
        let readiness = crate::strategy::preflight::check_live_readiness(ctx).await;
        if !readiness.ready {
            return Err(readiness.into_error());
        }
    }
    for rule in &input.config.rules {
        if let Action::Reduce { fraction } = rule.action {
            if !(fraction > 0.0 && fraction <= 1.0) {
                return Err(VulcanError::validation(
                    "INVALID_RULE",
                    format!(
                        "rule '{}': Reduce.fraction must be in (0, 1], got {}",
                        rule.name, fraction
                    ),
                ));
            }
        }
        if let Action::Open { size, .. } = &rule.action {
            if !size_positive(size) {
                return Err(VulcanError::validation(
                    "INVALID_RULE",
                    format!("rule '{}': Open size must be positive", rule.name),
                ));
            }
        }
    }
    let safety = StrategySafetyPolicy::with_overrides(
        input.safety_overrides.max_total_notional_usdc,
        input.safety_overrides.max_step_notional_usdc,
        input.safety_overrides.max_price_drift_bps,
        input.safety_overrides.max_exposure_ratio,
        input.safety_overrides.reconcile_attempts,
        input.safety_overrides.reconcile_delay_ms,
    )?;
    let run_id = new_run_id();
    let symbol_upper = input.config.symbol.to_ascii_uppercase();
    let strategy_config = TaStrategyConfig {
        symbol: symbol_upper,
        ..input.config
    };
    let config = TaRunConfig {
        run_id,
        run_label: input.run_label,
        wallet: strategy_wallet_context(ctx),
        mode: input.mode,
        max_ticks: input.max_ticks,
        run_until_stopped: input.run_until_stopped,
        strategy_config,
        safety_policy: safety,
    };
    Ok((config, input.no_sleep))
}

pub async fn resume_ta(
    ctx: &AppContext,
    run_id: &str,
    no_sleep: bool,
) -> Result<TaRunResult, VulcanError> {
    let runs_dir = default_strategy_runs_dir(&ctx.vulcan_dir);
    let report = crate::strategy::log::load_run_report(&runs_dir, run_id)?;
    let ledger = crate::strategy::log::load_run_ledger(&runs_dir, run_id)?;
    clear_control_request(&runs_dir, run_id)?;
    if ledger.strategy != "ta" {
        return Err(VulcanError::validation(
            "UNSUPPORTED_STRATEGY_RESUME",
            format!("resume requested for ta but ledger is {}", ledger.strategy),
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
            "Pass --yes to resume a live TA strategy run.",
        ));
    }
    let config: TaRunConfig = serde_json::from_value(report.config.clone()).map_err(|e| {
        VulcanError::validation(
            "STRATEGY_CONFIG_PARSE_FAILED",
            format!("failed to parse TA strategy config from report: {}", e),
        )
    })?;
    let ticks = crate::strategy::log::read_run_ticks(&runs_dir, run_id).unwrap_or_default();
    run_ta_config(ctx, config, no_sleep, Some(ledger), ticks).await
}

async fn run_ta_config(
    ctx: &AppContext,
    config: TaRunConfig,
    no_sleep: bool,
    existing_ledger: Option<StrategyRunLedger>,
    mut ticks: Vec<StrategyTick>,
) -> Result<TaRunResult, VulcanError> {
    let mode = config.mode;
    let symbol = config.strategy_config.symbol.clone();
    let run_id = config.run_id.clone();
    let started_at = existing_ledger
        .as_ref()
        .map(|l| l.started_at.clone())
        .unwrap_or_else(|| Utc::now().to_rfc3339());
    let started = Instant::now();
    let runs_dir = default_strategy_runs_dir(&ctx.vulcan_dir);

    let mut ledger = existing_ledger.unwrap_or_else(|| build_ledger(&config, &started_at));
    ledger.status = "running".to_string();
    ledger.pause_reason = None;
    write_run_ledger(&runs_dir, &ledger)?;

    // Cooldown state persists across ticks; on resume, rehydrate from the last slice's metadata.
    let mut cooldown: CooldownState = ledger
        .slices
        .iter()
        .rev()
        .find_map(|s| s.metadata.as_ref())
        .and_then(|m| m.get("cooldown_state"))
        .and_then(|v| serde_json::from_value::<CooldownState>(v.clone()).ok())
        .unwrap_or_default();

    let mut progress = StrategyProgress {
        slices_filled: ledger.totals.filled_count,
        cumulative_tokens: ledger.totals.filled_tokens,
        cumulative_notional_usdc: ledger.totals.filled_notional_usdc,
        vwap: ledger.totals.vwap,
        cumulative_fees: ledger.totals.fees,
        remaining_slices: 0,
    };

    let mut paused = false;
    let mut stopped = false;
    let max_ticks = if config.run_until_stopped {
        u32::MAX
    } else {
        config.max_ticks
    };
    let start_tick = ticks.last().map(|t| t.tick + 1).unwrap_or(1);

    let indicator_refs =
        crate::strategy::ta_rule::gather_indicator_refs(&config.strategy_config.rules);
    let cadence = config.strategy_config.resolved_cadence();
    let mut previous_prices = ticks.last().map(|tick| PriceHistory {
        mark: tick.market.mark_price,
        mid: tick.market.mid_price,
        close: tick.market.mark_price,
    });

    for tick_number in start_tick..=max_ticks {
        if apply_control_request(&runs_dir, &mut ledger)? {
            stopped = ledger.status == "stopped";
            paused = ledger.status == "paused";
            break;
        }

        // Transient errors on market/position snapshots should NOT kill the run.
        // Record a `skipped` ledger entry, sleep, and try again next tick.
        let market_snap = match market_snapshot(ctx, &symbol).await {
            Ok(m) => m,
            Err(err) => {
                ledger
                    .slices
                    .push(transient_error_slice(tick_number, "market snapshot", &err));
                ledger.totals.submitted_count += 1;
                ledger.totals.skipped_count += 1;
                ledger.last_updated_at = Utc::now().to_rfc3339();
                write_run_ledger(&runs_dir, &ledger)?;
                write_partial_report(
                    ctx,
                    &runs_dir,
                    &config,
                    &ledger,
                    &ticks,
                    &progress,
                    cooldown.total_firings,
                )?;
                let delay = cadence.delay_seconds(Utc::now().timestamp().max(0) as u64);
                if !no_sleep && delay > 0 {
                    tokio::time::sleep(std::time::Duration::from_secs(delay)).await;
                }
                continue;
            }
        };
        let (position, account_health) = match mode {
            StrategyMode::Paper => match paper_position_snapshot_pub(ctx, &symbol).await {
                Ok(snap) => snap,
                Err(err) => {
                    let mut snap = (
                        StrategyPositionSnapshot::default(),
                        StrategyAccountHealth::default(),
                    );
                    snap.1.risk_state = Some(format!("snapshot_error: {}", err.code));
                    snap
                }
            },
            StrategyMode::DryRun => (
                StrategyPositionSnapshot::default(),
                StrategyAccountHealth::default(),
            ),
            StrategyMode::ConfirmEach | StrategyMode::AutoExecute => {
                live_position_snapshot_pub(ctx, &symbol, market_snap.mark_price)
                    .await
                    .unwrap_or_else(|err| {
                        let health = StrategyAccountHealth {
                            risk_state: Some(format!("snapshot_error: {}", err.code)),
                            ..Default::default()
                        };
                        (StrategyPositionSnapshot::default(), health)
                    })
            }
        };

        if matches!(mode, StrategyMode::ConfirmEach | StrategyMode::AutoExecute) {
            // TA strategies expect price to move over a run, so we compare against
            // the previous tick's mark rather than the launch mark. On the very
            // first tick `previous_prices` is None and the drift check is skipped.
            if let Some(prev_mark) = previous_prices.as_ref().map(|p| p.mark) {
                config
                    .safety_policy
                    .validate_market_rolling(prev_mark, &market_snap)?;
            }
            config.safety_policy.validate_account(&account_health)?;
        }

        let indicator_cache = match compute_indicator_cache(ctx, &symbol, &indicator_refs).await {
            Ok(cache) => cache,
            Err(err) => {
                ledger.slices.push(transient_error_slice(
                    tick_number,
                    "indicator computation",
                    &err,
                ));
                ledger.totals.submitted_count += 1;
                ledger.totals.skipped_count += 1;
                ledger.last_updated_at = Utc::now().to_rfc3339();
                write_run_ledger(&runs_dir, &ledger)?;
                write_partial_report(
                    ctx,
                    &runs_dir,
                    &config,
                    &ledger,
                    &ticks,
                    &progress,
                    cooldown.total_firings,
                )?;
                let delay = cadence.delay_seconds(Utc::now().timestamp().max(0) as u64);
                if !no_sleep && delay > 0 {
                    tokio::time::sleep(std::time::Duration::from_secs(delay)).await;
                }
                continue;
            }
        };

        let (close_price, previous_close_price) = close_price_pair(
            &indicator_cache,
            previous_prices.as_ref(),
            market_snap.mark_price,
        );
        let attach_isolated_collateral = should_attach_isolated_collateral(&config, &ledger, mode);

        let eval = EvaluationContext {
            indicators: &indicator_cache,
            mark_price: market_snap.mark_price,
            previous_mark_price: previous_prices.as_ref().map(|p| p.mark),
            mid_price: market_snap.mid_price,
            previous_mid_price: previous_prices.as_ref().map(|p| p.mid),
            close_price,
            previous_close_price,
            position: &position,
        };

        // Walk rules; first firing wins. Always update cooldown state for every rule
        // so condition-resets are tracked even when an earlier rule already fired.
        let mut firing: Option<FiringRecord> = None;
        for rule in &config.strategy_config.rules {
            let outcome = rule.evaluate(&eval, &cooldown, tick_number)?;
            update_cooldown_for(&mut cooldown, rule, &outcome);
            if outcome.fired && firing.is_none() {
                firing = Some(FiringRecord {
                    rule_name: rule.name.clone(),
                    action: rule.action.clone(),
                });
            }
        }

        let (planned_action, execution, status, fired_side) = if let Some(record) = firing.as_ref()
        {
            cooldown.total_firings += 1;
            {
                let entry = cooldown.entry_mut(&record.rule_name);
                entry.firings += 1;
                entry.last_fired_tick = Some(tick_number);
                // Lock if this rule uses UntilConditionResets cooldown.
                if let Some(rule) = config
                    .strategy_config
                    .rules
                    .iter()
                    .find(|r| r.name == record.rule_name)
                {
                    if matches!(rule.cooldown, CooldownPolicy::UntilConditionResets) {
                        entry.locked = true;
                    }
                }
            }
            match dispatch_action(
                ctx,
                &config,
                &record.action,
                &record.rule_name,
                &symbol,
                &market_snap,
                &position,
                &account_health,
                mode,
                progress.cumulative_notional_usdc,
                attach_isolated_collateral,
            )
            .await
            {
                Ok((planned, execution, status, side)) => (planned, execution, status, side),
                Err(err) => {
                    let planned = StrategyPlannedAction {
                        action: record.rule_name.clone(),
                        order_type: "market".to_string(),
                        slice_notional_usdc: None,
                        slice_tokens: None,
                        executable_notional_usdc: None,
                        executable_tokens: None,
                        executable_base_lots: None,
                        rounding_note: Some(format!("rule '{}' action failed", record.rule_name)),
                    };
                    let execution = StrategyExecution {
                        action: "error".to_string(),
                        note: Some(format!("{}: {}", err.code, err.message)),
                        ..Default::default()
                    };
                    (
                        planned,
                        execution,
                        StrategySliceStatus::Failed,
                        StrategySide::Buy,
                    )
                }
            }
        } else {
            (
                StrategyPlannedAction {
                    action: "no_rule_fired".to_string(),
                    order_type: "none".to_string(),
                    slice_notional_usdc: None,
                    slice_tokens: None,
                    executable_notional_usdc: None,
                    executable_tokens: None,
                    executable_base_lots: None,
                    rounding_note: None,
                },
                StrategyExecution {
                    action: "no_rule_fired".to_string(),
                    ..Default::default()
                },
                StrategySliceStatus::Skipped,
                StrategySide::Buy,
            )
        };

        if let (Some(tokens), Some(notional)) =
            (execution.fill_tokens, execution.fill_notional_usdc)
        {
            progress.slices_filled += 1;
            progress.cumulative_tokens += tokens;
            progress.cumulative_notional_usdc += notional;
            if progress.cumulative_tokens.abs() > f64::EPSILON {
                progress.vwap =
                    Some(progress.cumulative_notional_usdc / progress.cumulative_tokens.abs());
            }
        }
        if let Some(fee) = execution.fee {
            progress.cumulative_fees += fee;
        }

        let slice = StrategyLedgerSlice {
            index: tick_number,
            scheduled_at: Utc::now().to_rfc3339(),
            planned_notional_usdc: planned_action.slice_notional_usdc,
            planned_tokens: planned_action.slice_tokens,
            executable_notional_usdc: planned_action.executable_notional_usdc,
            executable_tokens: planned_action.executable_tokens,
            executable_base_lots: planned_action.executable_base_lots,
            status,
            tx_signature: execution.tx_signature.clone(),
            fill_id: execution.fill_id.clone(),
            fill_price: execution.fill_price,
            fill_tokens: execution.fill_tokens,
            fill_notional_usdc: execution.fill_notional_usdc,
            fee: execution.fee,
            submitted_at: execution
                .tx_signature
                .as_ref()
                .map(|_| Utc::now().to_rfc3339()),
            filled_at: if matches!(status, StrategySliceStatus::Filled) {
                Some(Utc::now().to_rfc3339())
            } else {
                None
            },
            updated_at: Utc::now().to_rfc3339(),
            pause_reason: None,
            error_code: if matches!(status, StrategySliceStatus::Failed) {
                Some("RULE_ACTION_FAILED".to_string())
            } else {
                None
            },
            note: firing
                .as_ref()
                .map(|f| format!("rule fired: {}", f.rule_name))
                .or_else(|| execution.note.clone()),
            rounding_note: planned_action.rounding_note.clone(),
            metadata: Some(serde_json::json!({
                "rule_name": firing.as_ref().map(|f| f.rule_name.clone()),
                "executed_side": fired_side.as_str(),
                "cooldown_state": cooldown,
            })),
        };
        ledger.slices.push(slice);
        ledger.totals.submitted_count += 1;
        if matches!(status, StrategySliceStatus::Filled) {
            ledger.totals.filled_count += 1;
            ledger.totals.filled_tokens += execution.fill_tokens.unwrap_or(0.0);
            ledger.totals.filled_notional_usdc += execution.fill_notional_usdc.unwrap_or(0.0);
            ledger.totals.fees += execution.fee.unwrap_or(0.0);
            if ledger.totals.filled_tokens.abs() > f64::EPSILON {
                ledger.totals.vwap =
                    Some(ledger.totals.filled_notional_usdc / ledger.totals.filled_tokens.abs());
            }
        } else if matches!(status, StrategySliceStatus::Skipped) {
            ledger.totals.skipped_count += 1;
        } else if matches!(status, StrategySliceStatus::Failed) {
            ledger.totals.failed_count += 1;
        }
        ledger.last_updated_at = Utc::now().to_rfc3339();
        write_run_ledger(&runs_dir, &ledger)?;

        // Re-snapshot position when a rule actually filled, so the tick records
        // post-execution state alongside the pre-execution `position` the rule
        // saw. Without this the very tick that fires reports stale (pre-fill)
        // position data and agents have to wait one tick to see the trade.
        let position_after_action = if matches!(status, StrategySliceStatus::Filled) {
            match mode {
                StrategyMode::Paper => paper_position_snapshot_pub(ctx, &symbol)
                    .await
                    .ok()
                    .map(|(snap, _)| snap),
                StrategyMode::ConfirmEach | StrategyMode::AutoExecute => {
                    live_position_snapshot_pub(ctx, &symbol, market_snap.mark_price)
                        .await
                        .ok()
                        .map(|(snap, _)| snap)
                }
                StrategyMode::DryRun => None,
            }
        } else {
            None
        };

        let final_tick = !config.run_until_stopped && tick_number >= config.max_ticks;
        let stop_reason = if let Some(max) = config.strategy_config.max_firings {
            if max > 0 && cooldown.total_firings >= max {
                Some(format!("max_firings {} reached; stopping", max))
            } else {
                None
            }
        } else {
            None
        };
        let firings_cap_hit = stop_reason.is_some();

        let tick = StrategyTick {
            run_id: run_id.clone(),
            strategy: "ta".to_string(),
            mode,
            symbol: symbol.clone(),
            side: fired_side,
            tick: tick_number,
            slices_total: effective_slices_total(&config),
            timestamp: Utc::now().to_rfc3339(),
            elapsed_ms: started.elapsed().as_millis(),
            market: market_snap,
            planned_action,
            execution,
            progress: progress.clone(),
            position,
            position_after_action,
            account_health,
            next_tick: {
                let delay = cadence.delay_seconds(Utc::now().timestamp().max(0) as u64);
                StrategyNextTick {
                    next_tick_at: if final_tick || firings_cap_hit {
                        None
                    } else {
                        Some((Utc::now() + chrono::Duration::seconds(delay as i64)).to_rfc3339())
                    },
                    delay_seconds: (!final_tick && !firings_cap_hit).then_some(delay),
                    stop_reason: stop_reason
                        .clone()
                        .or_else(|| final_tick.then(|| "max_ticks_reached".to_string())),
                }
            },
        };
        emit_strategy_tick(ctx, &runs_dir, &tick, render_live_tick)?;
        previous_prices = Some(PriceHistory {
            mark: tick.market.mark_price,
            mid: tick.market.mid_price,
            close: close_price.unwrap_or(tick.market.mark_price),
        });
        ticks.push(tick);

        // Write a partial report so mid-run `strategy report <id>` works and
        // crashes leave behind a resumable artifact.
        write_partial_report(
            ctx,
            &runs_dir,
            &config,
            &ledger,
            &ticks,
            &progress,
            cooldown.total_firings,
        )?;

        if firings_cap_hit {
            stopped = true;
            ledger.status = "stopped".to_string();
            ledger.pause_reason = stop_reason.clone();
            break;
        }
        if final_tick {
            break;
        }
        if apply_control_request(&runs_dir, &mut ledger)? {
            stopped = ledger.status == "stopped";
            paused = ledger.status == "paused";
            break;
        }
        let delay = cadence.delay_seconds(Utc::now().timestamp().max(0) as u64);
        if !no_sleep && delay > 0 {
            tokio::time::sleep(std::time::Duration::from_secs(delay)).await;
        }
    }

    if !paused && !stopped {
        ledger.status = "completed".to_string();
    }
    ledger.completed_at = Some(Utc::now().to_rfc3339());
    ledger.last_updated_at = Utc::now().to_rfc3339();
    write_run_ledger(&runs_dir, &ledger)?;

    let last_side = ticks.last().map(|t| t.side).unwrap_or(StrategySide::Buy);
    let summary = StrategyRunSummary {
        run_id: run_id.clone(),
        strategy: "ta".to_string(),
        mode,
        symbol: symbol.clone(),
        side: last_side,
        started_at,
        completed_at: ledger.completed_at.clone(),
        status: ledger.status.clone(),
        slices_completed: progress.slices_filled,
        slices_total: effective_slices_total(&config),
        cumulative_notional_usdc: progress.cumulative_notional_usdc,
        cumulative_tokens: progress.cumulative_tokens,
        vwap: progress.vwap,
    };
    let notes = vec![
        format!(
            "TA strategy executed {} rules over {} ticks; {} firings total.",
            config.strategy_config.rules.len(),
            ticks.len(),
            cooldown.total_firings
        ),
        format!(
            "Margin mode: {}.",
            config.strategy_config.margin_mode.as_str()
        ),
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
    record_strategy_event(ctx, "strategy.ta.complete", &report, None);
    Ok(TaRunResult { report })
}

// ── Action dispatch ────────────────────────────────────────────────────────

#[allow(clippy::too_many_arguments)]
async fn dispatch_action(
    ctx: &AppContext,
    config: &TaRunConfig,
    action: &Action,
    rule_name: &str,
    symbol: &str,
    market: &StrategyMarketSnapshot,
    position: &StrategyPositionSnapshot,
    account_health: &StrategyAccountHealth,
    mode: StrategyMode,
    cumulative_notional_usdc: f64,
    attach_isolated_collateral: bool,
) -> Result<
    (
        StrategyPlannedAction,
        StrategyExecution,
        StrategySliceStatus,
        StrategySide,
    ),
    VulcanError,
> {
    match action {
        Action::NoOp => {
            let planned = StrategyPlannedAction {
                action: format!("noop:{rule_name}"),
                order_type: "none".to_string(),
                slice_notional_usdc: None,
                slice_tokens: None,
                executable_notional_usdc: None,
                executable_tokens: None,
                executable_base_lots: None,
                rounding_note: None,
            };
            let execution = StrategyExecution {
                action: format!("noop:{rule_name}"),
                note: Some("rule fired with NoOp action".to_string()),
                ..Default::default()
            };
            Ok((
                planned,
                execution,
                StrategySliceStatus::Skipped,
                StrategySide::Buy,
            ))
        }
        Action::Open { side, size } => {
            let (tokens, lots) = resolve_open_size(ctx, symbol, size, market).await?;
            let tokens = apply_position_cap(config, position, *side, tokens);
            if tokens.abs() < f64::EPSILON {
                let planned = StrategyPlannedAction {
                    action: format!("open_{}:{rule_name}", side.as_str()),
                    order_type: "market".to_string(),
                    slice_notional_usdc: Some(0.0),
                    slice_tokens: Some(0.0),
                    executable_notional_usdc: Some(0.0),
                    executable_tokens: Some(0.0),
                    executable_base_lots: Some(0),
                    rounding_note: Some(format!(
                        "rule '{}' clamped by max_concurrent_position_tokens cap",
                        rule_name
                    )),
                };
                let execution = StrategyExecution {
                    action: format!("open_skipped:{rule_name}"),
                    note: Some("position cap blocked Open".to_string()),
                    ..Default::default()
                };
                return Ok((planned, execution, StrategySliceStatus::Skipped, *side));
            }
            let planned = StrategyPlannedAction {
                action: format!("open_{}:{rule_name}", side.as_str()),
                order_type: "market".to_string(),
                slice_notional_usdc: Some(tokens * market.mark_price),
                slice_tokens: Some(tokens),
                executable_notional_usdc: Some(tokens * market.mark_price),
                executable_tokens: Some(tokens),
                executable_base_lots: Some(lots),
                rounding_note: None,
            };
            validate_open_safety(
                config,
                mode,
                market,
                position,
                account_health,
                *side,
                tokens,
                cumulative_notional_usdc,
            )?;
            let isolated_collateral = if attach_isolated_collateral {
                config.strategy_config.isolated_collateral
            } else {
                None
            };
            let execution = execute_market(
                ctx,
                config,
                mode,
                symbol,
                *side,
                tokens,
                lots,
                market,
                isolated_collateral,
            )
            .await?;
            Ok((planned, execution, StrategySliceStatus::Filled, *side))
        }
        Action::Close => {
            let (side, tokens, lots_opt) = match close_target(position) {
                Some(v) => v,
                None => {
                    let planned = StrategyPlannedAction {
                        action: format!("close:{rule_name}"),
                        order_type: "market".to_string(),
                        slice_notional_usdc: Some(0.0),
                        slice_tokens: Some(0.0),
                        executable_notional_usdc: Some(0.0),
                        executable_tokens: Some(0.0),
                        executable_base_lots: Some(0),
                        rounding_note: Some("no position to close".to_string()),
                    };
                    let execution = StrategyExecution {
                        action: format!("close_skipped:{rule_name}"),
                        note: Some("no position to close".to_string()),
                        ..Default::default()
                    };
                    return Ok((
                        planned,
                        execution,
                        StrategySliceStatus::Skipped,
                        StrategySide::Buy,
                    ));
                }
            };
            let lots = lots_from_position(ctx, symbol, lots_opt.unwrap_or(0), tokens).await?;
            let planned = StrategyPlannedAction {
                action: format!("close:{rule_name}"),
                order_type: "market".to_string(),
                slice_notional_usdc: Some(tokens * market.mark_price),
                slice_tokens: Some(tokens),
                executable_notional_usdc: Some(tokens * market.mark_price),
                executable_tokens: Some(tokens),
                executable_base_lots: Some(lots),
                rounding_note: None,
            };
            let execution =
                execute_market(ctx, config, mode, symbol, side, tokens, lots, market, None).await?;
            Ok((planned, execution, StrategySliceStatus::Filled, side))
        }
        Action::Reduce { fraction } => {
            let (side, tokens_full, lots_opt) = match close_target(position) {
                Some(v) => v,
                None => {
                    let planned = StrategyPlannedAction {
                        action: format!("reduce:{rule_name}"),
                        order_type: "market".to_string(),
                        slice_notional_usdc: Some(0.0),
                        slice_tokens: Some(0.0),
                        executable_notional_usdc: Some(0.0),
                        executable_tokens: Some(0.0),
                        executable_base_lots: Some(0),
                        rounding_note: Some("no position to reduce".to_string()),
                    };
                    let execution = StrategyExecution {
                        action: format!("reduce_skipped:{rule_name}"),
                        note: Some("no position to reduce".to_string()),
                        ..Default::default()
                    };
                    return Ok((
                        planned,
                        execution,
                        StrategySliceStatus::Skipped,
                        StrategySide::Buy,
                    ));
                }
            };
            let tokens = tokens_full * fraction;
            let lots_hint = lots_opt
                .map(|full| ((full as f64) * fraction).round().max(1.0) as u64)
                .unwrap_or(0);
            let lots = lots_from_position(ctx, symbol, lots_hint, tokens).await?;
            let planned = StrategyPlannedAction {
                action: format!("reduce:{rule_name}"),
                order_type: "market".to_string(),
                slice_notional_usdc: Some(tokens * market.mark_price),
                slice_tokens: Some(tokens),
                executable_notional_usdc: Some(tokens * market.mark_price),
                executable_tokens: Some(tokens),
                executable_base_lots: Some(lots),
                rounding_note: Some(format!("reduce by fraction {:.4}", fraction)),
            };
            let execution =
                execute_market(ctx, config, mode, symbol, side, tokens, lots, market, None).await?;
            Ok((planned, execution, StrategySliceStatus::Filled, side))
        }
    }
}

#[allow(clippy::too_many_arguments)]
async fn execute_market(
    ctx: &AppContext,
    config: &TaRunConfig,
    mode: StrategyMode,
    symbol: &str,
    side: StrategySide,
    tokens: f64,
    lots: u64,
    market: &StrategyMarketSnapshot,
    isolated_collateral: Option<f64>,
) -> Result<StrategyExecution, VulcanError> {
    match mode {
        StrategyMode::Paper => execute_paper_market(ctx, symbol, side, Some(tokens), None).await,
        StrategyMode::DryRun => Ok(dry_run_market(
            side,
            Some(tokens),
            Some(tokens * market.mark_price),
            market,
        )),
        StrategyMode::ConfirmEach | StrategyMode::AutoExecute => {
            execute_live_market_base_lots(
                ctx,
                symbol,
                side,
                lots,
                market,
                config.strategy_config.margin_mode,
                isolated_collateral,
            )
            .await
        }
    }
}

async fn resolve_open_size(
    ctx: &AppContext,
    symbol: &str,
    size: &SizeSpec,
    market: &StrategyMarketSnapshot,
) -> Result<(f64, u64), VulcanError> {
    let decimals = base_lots_decimals(ctx, symbol).await?;
    let scale = 10f64.powi(decimals as i32);
    let (tokens, lots) = match size {
        SizeSpec::Tokens(t) => {
            let lots = (t * scale).round().max(1.0) as u64;
            (lots as f64 / scale, lots)
        }
        SizeSpec::Lots(l) => {
            let lots = *l;
            (lots as f64 / scale, lots)
        }
        SizeSpec::Notional(n) => {
            let lots = ((n / market.mark_price) * scale).round().max(1.0) as u64;
            (lots as f64 / scale, lots)
        }
    };
    Ok((tokens, lots))
}

fn close_target(position: &StrategyPositionSnapshot) -> Option<(StrategySide, f64, Option<u64>)> {
    if position.size_tokens.abs() < f64::EPSILON {
        return None;
    }
    let side = match position.side.as_deref() {
        Some(s) if s.eq_ignore_ascii_case("long") || s.eq_ignore_ascii_case("buy") => {
            StrategySide::Sell
        }
        Some(s) if s.eq_ignore_ascii_case("short") || s.eq_ignore_ascii_case("sell") => {
            StrategySide::Buy
        }
        _ => return None,
    };
    Some((side, position.size_tokens.abs(), position.size_lots))
}

async fn lots_from_position(
    ctx: &AppContext,
    symbol: &str,
    lots_hint: u64,
    tokens: f64,
) -> Result<u64, VulcanError> {
    if lots_hint > 0 {
        return Ok(lots_hint);
    }
    let decimals = base_lots_decimals(ctx, symbol).await?;
    let scale = 10f64.powi(decimals as i32);
    Ok((tokens * scale).round().max(1.0) as u64)
}

async fn base_lots_decimals(ctx: &AppContext, symbol: &str) -> Result<i8, VulcanError> {
    let metadata = ctx.metadata().await?;
    let market = metadata.get_market(symbol).ok_or_else(|| {
        VulcanError::validation("UNKNOWN_MARKET", format!("Unknown market: {}", symbol))
    })?;
    Ok(market.base_lots_decimals)
}

fn apply_position_cap(
    config: &TaRunConfig,
    position: &StrategyPositionSnapshot,
    side: StrategySide,
    tokens: f64,
) -> f64 {
    let Some(cap) = config.strategy_config.max_concurrent_position_tokens else {
        return tokens;
    };
    let current = position.size_tokens.abs();
    let same_side = matches!(
        (side, position.side.as_deref()),
        (StrategySide::Buy, Some(s)) if s.eq_ignore_ascii_case("long") || s.eq_ignore_ascii_case("buy")
    ) || matches!(
        (side, position.side.as_deref()),
        (StrategySide::Sell, Some(s)) if s.eq_ignore_ascii_case("short") || s.eq_ignore_ascii_case("sell")
    );
    if !same_side {
        return tokens;
    }
    let projected = current + tokens;
    if projected <= cap {
        tokens
    } else {
        (cap - current).max(0.0)
    }
}

// ── Cooldown / firing helpers ──────────────────────────────────────────────

struct FiringRecord {
    rule_name: String,
    action: Action,
}

struct PriceHistory {
    mark: f64,
    mid: f64,
    close: f64,
}

fn update_cooldown_for(
    state: &mut CooldownState,
    rule: &Rule,
    outcome: &crate::strategy::ta_rule::RuleEvalOutcome,
) {
    let entry = state.entry_mut(&rule.name);
    if matches!(rule.cooldown, CooldownPolicy::UntilConditionResets)
        && entry.locked
        && !outcome.condition_true
    {
        entry.locked = false;
    }
    entry.condition_held_last_tick = outcome.condition_true;
}

// ── Indicator cache ────────────────────────────────────────────────────────

fn close_price_pair(
    cache: &BTreeMap<String, IndicatorSeries>,
    previous_prices: Option<&PriceHistory>,
    fallback_mark: f64,
) -> (Option<f64>, Option<f64>) {
    let closes: Vec<f64> = cache
        .values()
        .next()
        .map(|series| {
            series
                .points
                .iter()
                .filter_map(|p| p.values.get("close").copied().filter(|v| !v.is_nan()))
                .collect()
        })
        .unwrap_or_default();
    let current = closes.last().copied().or(Some(fallback_mark));
    let previous = closes
        .iter()
        .rev()
        .nth(1)
        .copied()
        .or_else(|| previous_prices.map(|p| p.close));
    (current, previous)
}

fn should_attach_isolated_collateral(
    config: &TaRunConfig,
    ledger: &StrategyRunLedger,
    mode: StrategyMode,
) -> bool {
    matches!(mode, StrategyMode::ConfirmEach | StrategyMode::AutoExecute)
        && config.strategy_config.margin_mode == StrategyMarginMode::Isolated
        && config.strategy_config.isolated_collateral.is_some()
        && !ledger
            .slices
            .iter()
            .any(|slice| slice.tx_signature.is_some())
}

#[allow(clippy::too_many_arguments)]
fn validate_open_safety(
    config: &TaRunConfig,
    mode: StrategyMode,
    market: &StrategyMarketSnapshot,
    position: &StrategyPositionSnapshot,
    account_health: &StrategyAccountHealth,
    side: StrategySide,
    tokens: f64,
    cumulative_notional_usdc: f64,
) -> Result<(), VulcanError> {
    if !matches!(mode, StrategyMode::ConfirmEach | StrategyMode::AutoExecute) {
        return Ok(());
    }
    let step_notional = (tokens * market.mark_price).abs();
    config.safety_policy.validate_plan(
        Some(cumulative_notional_usdc.abs() + step_notional),
        Some(step_notional),
    )?;
    config
        .safety_policy
        .validate_exposure(projected_exposure_ratio(
            position,
            account_health,
            side,
            tokens,
            market,
        ))?;
    Ok(())
}

fn projected_exposure_ratio(
    position: &StrategyPositionSnapshot,
    account_health: &StrategyAccountHealth,
    side: StrategySide,
    tokens: f64,
    market: &StrategyMarketSnapshot,
) -> f64 {
    let Some(equity) = account_health
        .equity
        .filter(|value| value.abs() >= f64::EPSILON)
    else {
        return position.exposure_ratio;
    };
    let current_tokens = signed_position_tokens(position);
    let delta = match side {
        StrategySide::Buy => tokens.abs(),
        StrategySide::Sell => -tokens.abs(),
    };
    ((current_tokens + delta).abs() * market.mark_price) / equity
}

fn signed_position_tokens(position: &StrategyPositionSnapshot) -> f64 {
    match position.side.as_deref() {
        Some(side) if side.eq_ignore_ascii_case("long") || side.eq_ignore_ascii_case("buy") => {
            position.size_tokens.abs()
        }
        Some(side) if side.eq_ignore_ascii_case("short") || side.eq_ignore_ascii_case("sell") => {
            -position.size_tokens.abs()
        }
        _ => 0.0,
    }
}

async fn compute_indicator_cache(
    ctx: &AppContext,
    symbol: &str,
    refs: &[IndicatorRef],
) -> Result<BTreeMap<String, IndicatorSeries>, VulcanError> {
    let mut cache: BTreeMap<String, IndicatorSeries> = BTreeMap::new();
    for r in refs {
        let key = r.cache_key();
        if cache.contains_key(&key) {
            continue;
        }
        let request = IndicatorRequest {
            kind: r.kind,
            period: r.period,
            params: r.params.clone(),
        };
        let series = indicators::compute(ctx, symbol, &r.timeframe, &request, None).await?;
        cache.insert(key, series);
    }
    Ok(cache)
}

/// Build a `skipped` ledger entry for a transient error so the run stays
/// resumable instead of crashing.
fn transient_error_slice(tick: u32, stage: &str, err: &VulcanError) -> StrategyLedgerSlice {
    let now = Utc::now().to_rfc3339();
    StrategyLedgerSlice {
        index: tick,
        scheduled_at: now.clone(),
        planned_notional_usdc: None,
        planned_tokens: None,
        executable_notional_usdc: None,
        executable_tokens: None,
        executable_base_lots: None,
        status: StrategySliceStatus::Skipped,
        tx_signature: None,
        fill_id: None,
        fill_price: None,
        fill_tokens: None,
        fill_notional_usdc: None,
        fee: None,
        submitted_at: None,
        filled_at: None,
        updated_at: now,
        pause_reason: Some(err.message.clone()),
        error_code: Some(err.code.clone()),
        note: Some(format!(
            "transient error during {}: {} ({})",
            stage, err.message, err.code
        )),
        rounding_note: None,
        metadata: Some(serde_json::json!({ "transient_error_stage": stage })),
    }
}

/// Write a snapshot of the run report so mid-run `strategy report` works and
/// a crash leaves behind a resumable artifact.
fn write_partial_report(
    ctx: &AppContext,
    runs_dir: &std::path::Path,
    config: &TaRunConfig,
    ledger: &StrategyRunLedger,
    ticks: &[StrategyTick],
    progress: &StrategyProgress,
    total_firings: u32,
) -> Result<(), VulcanError> {
    let _ = ctx;
    let last_side = ticks.last().map(|t| t.side).unwrap_or(StrategySide::Buy);
    let summary = StrategyRunSummary {
        run_id: config.run_id.clone(),
        strategy: "ta".to_string(),
        mode: config.mode,
        symbol: config.strategy_config.symbol.clone(),
        side: last_side,
        started_at: ledger.started_at.clone(),
        completed_at: ledger.completed_at.clone(),
        status: ledger.status.clone(),
        slices_completed: progress.slices_filled,
        slices_total: effective_slices_total(config),
        cumulative_notional_usdc: progress.cumulative_notional_usdc,
        cumulative_tokens: progress.cumulative_tokens,
        vwap: progress.vwap,
    };
    let notes = vec![format!(
        "Partial report at tick {}; {} firings.",
        ticks.last().map(|t| t.tick).unwrap_or(0),
        total_firings
    )];
    let report = StrategyRunReport {
        run_id: config.run_id.clone(),
        summary,
        config: serde_json::to_value(config).unwrap_or(serde_json::Value::Null),
        ticks: ticks.to_vec(),
        log_path: crate::strategy::log::run_log_path(runs_dir, &config.run_id)
            .to_string_lossy()
            .to_string(),
        ledger: Some(ledger.clone()),
        notes,
    };
    write_run_report(runs_dir, &report)
}

// ── Plumbing ───────────────────────────────────────────────────────────────

/// `run_until_stopped` runs have no fixed end, so we emit `u32::MAX` as a
/// sentinel — display code already hides it from `progress N/<total>` strings.
/// Without this, ticks carry the unrelated CLI default `--max-ticks 60` and
/// look like a TWAP slice plan that doesn't apply to TA.
fn effective_slices_total(config: &TaRunConfig) -> u32 {
    if config.run_until_stopped {
        u32::MAX
    } else {
        config.max_ticks
    }
}

fn size_positive(size: &SizeSpec) -> bool {
    match size {
        SizeSpec::Tokens(v) | SizeSpec::Notional(v) => *v > 0.0,
        SizeSpec::Lots(v) => *v > 0,
    }
}

fn new_run_id() -> String {
    format!(
        "ta-{}-{:08x}",
        Utc::now().format("%Y%m%dT%H%M%SZ"),
        rand::thread_rng().next_u32()
    )
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

fn build_ledger(config: &TaRunConfig, started_at: &str) -> StrategyRunLedger {
    let cadence = config.strategy_config.resolved_cadence();
    StrategyRunLedger {
        run_id: config.run_id.clone(),
        strategy: "ta".to_string(),
        run_label: config.run_label.clone(),
        wallet: config.wallet.clone(),
        symbol: config.strategy_config.symbol.clone(),
        side: StrategySide::Buy,
        mode: config.mode,
        started_at: started_at.to_string(),
        completed_at: None,
        status: "running".to_string(),
        interval_seconds: cadence.nominal_interval_seconds(),
        run_until_stopped: config.run_until_stopped,
        stale_after_seconds: None,
        slices_total: effective_slices_total(config),
        totals: StrategyLedgerTotals::default(),
        slices: Vec::new(),
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

fn render_live_tick(tick: &StrategyTick) {
    println!(
        "TA Tick {}{} {} [status={}]",
        tick.tick,
        if tick.slices_total > 0 && tick.slices_total < u32::MAX {
            format!("/{}", tick.slices_total)
        } else {
            String::new()
        },
        tick.symbol,
        tick.next_tick
            .stop_reason
            .clone()
            .unwrap_or_else(|| "running".to_string())
    );
    crate::output::table::render_table(
        &["Field", "Value"],
        vec![
            vec![
                "Action".to_string(),
                format!("{} {}", tick.side.as_str(), tick.planned_action.action),
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
                    "{:.4} USDC / {:.6} tokens | VWAP {} | fees {:.4}",
                    tick.progress.cumulative_notional_usdc,
                    tick.progress.cumulative_tokens,
                    tick.progress
                        .vwap
                        .map(|v| format!("{:.6}", v))
                        .unwrap_or_else(|| "n/a".to_string()),
                    tick.progress.cumulative_fees,
                ),
            ],
            vec![
                "Position".to_string(),
                format!(
                    "{} {:.6} tokens | notional ${:.4} | entry {} | mark {}",
                    tick.position.side.as_deref().unwrap_or("none"),
                    tick.position.size_tokens,
                    tick.position.notional_usdc,
                    tick.position
                        .entry_price
                        .map(|v| format!("{:.6}", v))
                        .unwrap_or_else(|| "n/a".to_string()),
                    tick.position
                        .mark_price
                        .map(|v| format!("{:.6}", v))
                        .unwrap_or_else(|| "n/a".to_string()),
                ),
            ],
            vec![
                "P/L".to_string(),
                format!(
                    "uPnL {:+.4} USDC{} | account equity {}",
                    tick.position.unrealized_pnl,
                    tick.position
                        .unrealized_pnl_pct
                        .map(|pct| format!(" ({:+.2}%)", pct))
                        .unwrap_or_default(),
                    tick.account_health
                        .equity
                        .map(|v| format!("${:.4}", v))
                        .unwrap_or_else(|| "n/a".to_string()),
                ),
            ],
            vec![
                "Risk".to_string(),
                format!(
                    "leverage {:.2}x | liq {} | distance {} | margin {}",
                    tick.position
                        .leverage
                        .unwrap_or(tick.position.exposure_ratio),
                    tick.position
                        .liquidation_price
                        .map(|v| format!("${:.4}", v))
                        .unwrap_or_else(|| "n/a".to_string()),
                    tick.position
                        .distance_to_liq_bps
                        .map(|bps| {
                            let pct = bps / 100.0;
                            let flag = if bps < 200.0 {
                                " [CRITICAL]"
                            } else if bps < 500.0 {
                                " [CLOSE]"
                            } else {
                                ""
                            };
                            format!("{:+.0}bps ({:+.2}%){}", bps, pct, flag)
                        })
                        .unwrap_or_else(|| "n/a".to_string()),
                    tick.account_health
                        .risk_state
                        .as_deref()
                        .unwrap_or("unknown"),
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
    if let Some(note) = tick.execution.note.as_ref() {
        println!("Note: {}", note);
    }
    println!();
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::strategy::ta_rule::TaStrategyConfig;

    fn config_with(max_ticks: u32, run_until_stopped: bool) -> TaRunConfig {
        TaRunConfig {
            run_id: "test".to_string(),
            run_label: None,
            wallet: None,
            mode: StrategyMode::Paper,
            max_ticks,
            run_until_stopped,
            strategy_config: TaStrategyConfig {
                symbol: "SOL".to_string(),
                interval_seconds: 60,
                cadence: None,
                margin_mode: StrategyMarginMode::default(),
                isolated_collateral: None,
                max_concurrent_position_tokens: None,
                max_firings: None,
                rules: Vec::new(),
            },
            safety_policy: StrategySafetyPolicy::default(),
        }
    }

    #[test]
    fn slices_total_uses_max_ticks_for_bounded_runs() {
        let config = config_with(60, false);
        assert_eq!(effective_slices_total(&config), 60);
    }

    #[test]
    fn slices_total_is_sentinel_for_run_until_stopped() {
        // u32::MAX is the sentinel the display layer hides from `N/total`
        // output; without this we emit the CLI default 60 and the response
        // reads like a TWAP slice plan.
        let config = config_with(60, true);
        assert_eq!(effective_slices_total(&config), u32::MAX);
    }

    fn skeleton_tick() -> StrategyTick {
        StrategyTick {
            run_id: "r".to_string(),
            strategy: "ta".to_string(),
            mode: StrategyMode::Paper,
            symbol: "SOL".to_string(),
            side: StrategySide::Buy,
            tick: 1,
            slices_total: u32::MAX,
            timestamp: "2026-05-19T00:00:00Z".to_string(),
            elapsed_ms: 0,
            market: StrategyMarketSnapshot {
                mark_price: 0.0,
                mid_price: 0.0,
                funding_rate: 0.0,
            },
            planned_action: StrategyPlannedAction {
                action: "noop".to_string(),
                order_type: "noop".to_string(),
                slice_notional_usdc: None,
                slice_tokens: None,
                executable_notional_usdc: None,
                executable_tokens: None,
                executable_base_lots: None,
                rounding_note: None,
            },
            execution: StrategyExecution::default(),
            progress: StrategyProgress::default(),
            position: StrategyPositionSnapshot::default(),
            position_after_action: None,
            account_health: StrategyAccountHealth::default(),
            next_tick: StrategyNextTick {
                next_tick_at: None,
                delay_seconds: None,
                stop_reason: None,
            },
        }
    }

    #[test]
    fn position_after_action_is_skipped_when_none() {
        let tick = skeleton_tick();
        let json = serde_json::to_string(&tick).unwrap();
        assert!(
            !json.contains("position_after_action"),
            "None should be omitted from serialized tick: {}",
            json
        );
    }

    #[test]
    fn position_after_action_round_trips_when_some() {
        let mut tick = skeleton_tick();
        tick.position_after_action = Some(StrategyPositionSnapshot {
            side: Some("short".to_string()),
            size_tokens: 0.5,
            ..Default::default()
        });
        let json = serde_json::to_string(&tick).unwrap();
        assert!(json.contains("\"position_after_action\""));
        let back: StrategyTick = serde_json::from_str(&json).unwrap();
        let after = back.position_after_action.unwrap();
        assert_eq!(after.side.as_deref(), Some("short"));
        assert_eq!(after.size_tokens, 0.5);
    }

    #[test]
    fn position_after_action_defaults_to_none_for_legacy_payloads() {
        // Older ledgers / reports written before this field existed must still
        // deserialize cleanly. Strip the field and confirm it comes back as None.
        let mut tick = skeleton_tick();
        tick.position_after_action = Some(StrategyPositionSnapshot::default());
        let mut value: serde_json::Value = serde_json::to_value(&tick).unwrap();
        value
            .as_object_mut()
            .unwrap()
            .remove("position_after_action");
        let back: StrategyTick = serde_json::from_value(value).unwrap();
        assert!(back.position_after_action.is_none());
    }
}

//! Strategy runner command execution.

use crate::cli::strategy::{GridCommand, StrategyCommand, TaCommand, TaStartArgs, TwapCommand};
use crate::context::AppContext;
use crate::error::VulcanError;
use crate::output::{render_success, TableRenderable};
use crate::strategy::{
    clear_control_request, default_strategy_runs_dir, list_run_summaries, load_run_ledger,
    load_run_report, read_run_ticks, write_control_request, write_run_ledger, GridRunResult,
    StrategyControlAction, StrategyControlRequest, StrategyRunLedger, StrategyRunReport,
    StrategyRunSummary, StrategyTick, TwapRunResult,
};
use chrono::Utc;
use std::fs;
use std::sync::Arc;

#[derive(Debug, serde::Serialize)]
pub struct StrategyRunsResult {
    pub runs: Vec<StrategyRunSummary>,
}

#[derive(Debug, serde::Serialize)]
pub struct StrategyStatusResult {
    pub summary: StrategyRunSummary,
    pub latest_tick: Option<StrategyTick>,
    pub new_ticks: Vec<StrategyTick>,
    pub next_tick: Option<crate::strategy::types::StrategyNextTick>,
    pub lifecycle: StrategyLifecycleStatus,
    pub terminal: bool,
    pub timed_out: bool,
    /// True when the run is terminal AND `latest_tick.position` reports a non-flat
    /// position. The snapshot is frozen at the last tick and may not reflect chain
    /// state if positions were closed after the runner exited. Consumers must call
    /// `vulcan_position_list` for authoritative current state before sizing trades.
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub position_snapshot_stale: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub position_snapshot_warning: Option<String>,
    pub ledger: Option<StrategyRunLedger>,
}

#[derive(Debug, serde::Serialize)]
pub struct StrategyMonitorResult {
    pub summary: StrategyRunSummary,
    pub last_observed_tick: Option<u32>,
    pub last_observed_at: Option<String>,
    pub seconds_since_last_tick: Option<u64>,
    pub expected_next_tick_at: Option<String>,
    pub runner_lock: StrategyRunnerLockStatus,
    pub lifecycle: StrategyLifecycleStatus,
    pub terminal: bool,
    pub event_summary: StrategyMonitorEventSummary,
    pub ledger: Option<StrategyRunLedger>,
}

#[derive(Debug, serde::Serialize)]
pub struct StrategyRunnerLockStatus {
    pub present: bool,
    pub path: Option<String>,
    pub pid: Option<u32>,
    pub updated_at: Option<String>,
    pub age_seconds: Option<u64>,
}

#[derive(Debug, serde::Serialize)]
pub struct StrategyMonitorEventSummary {
    pub latest_action: Option<String>,
    pub latest_note: Option<String>,
    pub total_ticks: usize,
    pub filled_count: u32,
    pub open_count: u32,
    pub failed_count: u32,
}

#[derive(Debug, serde::Serialize)]
pub struct StrategyLifecycleStatus {
    pub stale: bool,
    pub seconds_since_last_tick: Option<u64>,
    pub stale_after_seconds: Option<u64>,
    pub recovery_hint: Option<String>,
}

#[derive(Debug, serde::Serialize)]
pub struct StrategyControlResult {
    pub run_id: String,
    pub action: StrategyControlAction,
    pub status: String,
    pub message: String,
}

#[derive(Debug, serde::Serialize)]
pub struct StrategyFinalizeResult {
    pub run_id: String,
    pub status: String,
    pub stop: StrategyControlResult,
    pub waited: bool,
    pub wait_timed_out: bool,
    pub cancel_orders: Option<crate::commands::trade::CancelResult>,
    pub close_position: Option<crate::commands::position::CloseResult>,
    pub close_position_note: Option<String>,
    pub monitor: StrategyMonitorResult,
}

pub async fn execute(ctx: &AppContext, cmd: StrategyCommand) -> Result<(), VulcanError> {
    match cmd {
        StrategyCommand::Twap(TwapCommand::Start(args)) => {
            if args.detached {
                let result =
                    crate::strategy::twap::run_twap_detached(Arc::new(ctx.clone()), args).await?;
                render_success(
                    ctx.output_format,
                    &result,
                    serde_json::json!({ "strategy": "twap", "detached": true }),
                );
            } else {
                let result = crate::strategy::twap::run_twap(ctx, args).await?;
                render_success(
                    ctx.output_format,
                    &result,
                    serde_json::json!({ "strategy": "twap" }),
                );
            }
        }
        StrategyCommand::Twap(TwapCommand::Resume {
            run_id,
            from_step,
            no_sleep,
        }) => {
            clear_control_request(&default_strategy_runs_dir(&ctx.vulcan_dir), &run_id)?;
            let result =
                crate::strategy::twap::resume_twap(ctx, &run_id, from_step, no_sleep).await?;
            render_success(
                ctx.output_format,
                &result,
                serde_json::json!({ "strategy": "twap", "resumed": true }),
            );
        }
        StrategyCommand::Grid(GridCommand::Start(args)) => {
            let args = *args;
            if args.detached {
                let result =
                    crate::strategy::grid::run_grid_detached(Arc::new(ctx.clone()), args).await?;
                render_success(
                    ctx.output_format,
                    &result,
                    serde_json::json!({ "strategy": "grid", "detached": true }),
                );
            } else {
                let result = crate::strategy::grid::run_grid(ctx, args).await?;
                render_success(
                    ctx.output_format,
                    &result,
                    serde_json::json!({ "strategy": "grid" }),
                );
            }
        }
        StrategyCommand::Grid(GridCommand::Resume {
            run_id,
            from_step,
            no_sleep,
        }) => {
            clear_control_request(&default_strategy_runs_dir(&ctx.vulcan_dir), &run_id)?;
            let result =
                crate::strategy::grid::resume_grid(ctx, &run_id, from_step, no_sleep).await?;
            render_success(
                ctx.output_format,
                &result,
                serde_json::json!({ "strategy": "grid", "resumed": true }),
            );
        }
        StrategyCommand::Ta(TaCommand::Start(args)) => {
            let input = build_ta_start_input(args)?;
            if input.detached_requested {
                let result =
                    crate::strategy::ta::run_ta_detached(Arc::new(ctx.clone()), input.input)
                        .await?;
                render_success(
                    ctx.output_format,
                    &result,
                    serde_json::json!({ "strategy": "ta", "detached": true }),
                );
            } else {
                let result = crate::strategy::ta::run_ta(ctx, input.input).await?;
                render_success(
                    ctx.output_format,
                    &result,
                    serde_json::json!({ "strategy": "ta" }),
                );
            }
        }
        StrategyCommand::Ta(TaCommand::Resume { run_id, no_sleep }) => {
            clear_control_request(&default_strategy_runs_dir(&ctx.vulcan_dir), &run_id)?;
            let result = crate::strategy::ta::resume_ta(ctx, &run_id, no_sleep).await?;
            render_success(
                ctx.output_format,
                &result,
                serde_json::json!({ "strategy": "ta", "resumed": true }),
            );
        }
        StrategyCommand::Runs { limit } => {
            let result = runs(ctx, limit)?;
            render_success(ctx.output_format, &result, serde_json::Value::Null);
        }
        StrategyCommand::Preflight => {
            let result = crate::strategy::preflight::check_live_readiness(ctx).await;
            render_success(ctx.output_format, &result, serde_json::Value::Null);
        }
        StrategyCommand::Status {
            run_id,
            since_tick,
            include_ledger,
        } => {
            let result = status(ctx, &run_id, since_tick, include_ledger)?;
            render_success(ctx.output_format, &result, serde_json::Value::Null);
        }
        StrategyCommand::Monitor {
            run_id,
            include_ledger,
        } => {
            let result = monitor(ctx, &run_id, include_ledger)?;
            render_success(ctx.output_format, &result, serde_json::Value::Null);
        }
        StrategyCommand::ReconcileGrid { run_id } => {
            let result = crate::strategy::grid::inspect_live_grid_orders(ctx, &run_id).await?;
            render_success(ctx.output_format, &result, serde_json::Value::Null);
        }
        StrategyCommand::WaitNextTick {
            run_id,
            after_tick,
            timeout_seconds,
            include_ledger,
        } => {
            let result =
                wait_next_tick(ctx, &run_id, after_tick, timeout_seconds, include_ledger).await?;
            render_success(ctx.output_format, &result, serde_json::Value::Null);
        }
        StrategyCommand::Report { run_id } => {
            let result = report(ctx, &run_id)?;
            render_success(ctx.output_format, &result, serde_json::Value::Null);
        }
        StrategyCommand::Pause { run_id, reason } => {
            let result = request_control(ctx, &run_id, StrategyControlAction::Pause, reason)?;
            render_success(ctx.output_format, &result, serde_json::Value::Null);
        }
        StrategyCommand::Stop { run_id, reason } => {
            let result = request_control(ctx, &run_id, StrategyControlAction::Stop, reason)?;
            render_success(ctx.output_format, &result, serde_json::Value::Null);
        }
        StrategyCommand::Finalize {
            run_id,
            reason,
            cancel_orders,
            close_position,
            wait,
            timeout_seconds,
        } => {
            let result = finalize(
                ctx,
                &run_id,
                reason,
                cancel_orders,
                close_position,
                wait,
                timeout_seconds,
            )
            .await?;
            render_success(ctx.output_format, &result, serde_json::Value::Null);
        }
        StrategyCommand::Resume {
            run_id,
            from_step,
            no_sleep,
        } => {
            clear_control_request(&default_strategy_runs_dir(&ctx.vulcan_dir), &run_id)?;
            let ledger = load_run_ledger(&default_strategy_runs_dir(&ctx.vulcan_dir), &run_id)?;
            match ledger.strategy.as_str() {
                "twap" => {
                    let result =
                        crate::strategy::twap::resume_twap(ctx, &run_id, from_step, no_sleep)
                            .await?;
                    render_success(
                        ctx.output_format,
                        &result,
                        serde_json::json!({ "strategy": "twap", "resumed": true }),
                    );
                }
                "grid" => {
                    let result =
                        crate::strategy::grid::resume_grid(ctx, &run_id, from_step, no_sleep)
                            .await?;
                    render_success(
                        ctx.output_format,
                        &result,
                        serde_json::json!({ "strategy": "grid", "resumed": true }),
                    );
                }
                other => {
                    return Err(VulcanError::validation(
                        "UNSUPPORTED_STRATEGY_RESUME",
                        format!("resume does not support strategy {}", other),
                    ));
                }
            }
        }
    }
    Ok(())
}

pub fn request_control(
    ctx: &AppContext,
    run_id: &str,
    action: StrategyControlAction,
    reason: Option<String>,
) -> Result<StrategyControlResult, VulcanError> {
    let runs_dir = default_strategy_runs_dir(&ctx.vulcan_dir);
    let mut ledger = load_run_ledger(&runs_dir, run_id)?;
    if matches!(ledger.status.as_str(), "completed" | "stopped" | "failed") {
        return Err(VulcanError::validation(
            "STRATEGY_NOT_RUNNING",
            format!("strategy run {} is already {}", run_id, ledger.status),
        ));
    }
    let request = StrategyControlRequest {
        action,
        requested_at: Utc::now().to_rfc3339(),
        reason,
    };
    write_control_request(&runs_dir, run_id, &request)?;
    ledger.status = format!("{}_requested", action.as_str());
    ledger.pause_reason = request.reason.clone();
    ledger.last_updated_at = request.requested_at.clone();
    write_run_ledger(&runs_dir, &ledger)?;
    Ok(StrategyControlResult {
        run_id: run_id.to_string(),
        action,
        status: ledger.status,
        message: format!(
            "{} requested; runner will apply it at the next safe point",
            action.as_str()
        ),
    })
}

pub async fn finalize(
    ctx: &AppContext,
    run_id: &str,
    reason: Option<String>,
    cancel_orders: bool,
    close_position: bool,
    wait: bool,
    timeout_seconds: u64,
) -> Result<StrategyFinalizeResult, VulcanError> {
    if (cancel_orders || close_position) && !ctx.yes && !ctx.dry_run {
        return Err(VulcanError::validation(
            "CONFIRMATION_REQUIRED",
            "Pass --yes to confirm strategy cleanup, or --dry-run to simulate",
        ));
    }

    let stop = match request_control(ctx, run_id, StrategyControlAction::Stop, reason) {
        Ok(result) => result,
        Err(err) if err.code == "STRATEGY_NOT_RUNNING" => {
            let ledger = load_run_ledger(&default_strategy_runs_dir(&ctx.vulcan_dir), run_id)?;
            StrategyControlResult {
                run_id: run_id.to_string(),
                action: StrategyControlAction::Stop,
                status: ledger.status,
                message: "strategy was already terminal; continuing cleanup".to_string(),
            }
        }
        Err(err) => return Err(err),
    };

    let wait_status = if wait {
        // Anchor at the tick already on disk so the wait blocks until the
        // runner emits a NEW tick or reaches terminal — not on pre-existing
        // ticks. wait_next_tick's default-None now returns all known ticks.
        let anchor = read_run_ticks(&default_strategy_runs_dir(&ctx.vulcan_dir), run_id)
            .unwrap_or_default()
            .last()
            .map(|tick| tick.tick);
        Some(wait_next_tick(ctx, run_id, anchor, timeout_seconds, false).await?)
    } else {
        None
    };
    let wait_timed_out = wait_status
        .as_ref()
        .map(|status| status.timed_out)
        .unwrap_or(false);

    let runs_dir = default_strategy_runs_dir(&ctx.vulcan_dir);
    let mut ledger = load_run_ledger(&runs_dir, run_id)?;
    let mut cancel_result = None;
    let mut close_result = None;
    let mut close_note = None;

    if cancel_orders {
        cancel_result =
            Some(crate::commands::trade::execute_cancel_all_inner(ctx, &ledger.symbol).await?);
        mark_open_grid_slices_cancelled(&mut ledger);
    }

    if close_position {
        match crate::commands::position::execute_close_inner(ctx, &ledger.symbol).await {
            Ok(result) => {
                close_result = Some(result);
            }
            Err(err) if err.code == "NO_POSITION" => {
                close_note = Some(err.message);
            }
            Err(err) => return Err(err),
        }
    }

    ledger.status = "finalized".to_string();
    ledger
        .completed_at
        .get_or_insert_with(|| Utc::now().to_rfc3339());
    ledger.last_updated_at = Utc::now().to_rfc3339();
    ledger.pause_reason = Some("Strategy finalized by explicit cleanup command".to_string());
    recompute_finalize_totals(&mut ledger);
    write_run_ledger(&runs_dir, &ledger)?;

    let monitor = monitor(ctx, run_id, true)?;
    Ok(StrategyFinalizeResult {
        run_id: run_id.to_string(),
        status: ledger.status,
        stop,
        waited: wait,
        wait_timed_out,
        cancel_orders: cancel_result,
        close_position: close_result,
        close_position_note: close_note,
        monitor,
    })
}

pub fn runs(ctx: &AppContext, limit: usize) -> Result<StrategyRunsResult, VulcanError> {
    let runs_dir = default_strategy_runs_dir(&ctx.vulcan_dir);
    Ok(StrategyRunsResult {
        runs: list_run_summaries(&runs_dir, limit)?,
    })
}

pub fn status(
    ctx: &AppContext,
    run_id: &str,
    since_tick: Option<u32>,
    include_ledger: bool,
) -> Result<StrategyStatusResult, VulcanError> {
    status_inner(ctx, run_id, since_tick, include_ledger, false)
}

pub fn monitor(
    ctx: &AppContext,
    run_id: &str,
    include_ledger: bool,
) -> Result<StrategyMonitorResult, VulcanError> {
    let runs_dir = default_strategy_runs_dir(&ctx.vulcan_dir);
    let ledger = load_run_ledger(&runs_dir, run_id).ok();
    let summary = if let Some(ledger) = &ledger {
        summary_from_ledger(ledger)
    } else {
        load_run_report(&runs_dir, run_id)?.summary
    };
    let ticks = read_run_ticks(&runs_dir, run_id).unwrap_or_default();
    let latest_tick = ticks.last();
    let terminal = is_terminal_status(&summary.status);
    let lifecycle = lifecycle_status(&summary, ledger.as_ref(), latest_tick, terminal);
    let runner_lock = runner_lock_status(&runs_dir, run_id);
    let event_summary = monitor_event_summary(ledger.as_ref(), &ticks);
    Ok(StrategyMonitorResult {
        summary,
        last_observed_tick: latest_tick.map(|tick| tick.tick),
        last_observed_at: latest_tick
            .map(|tick| tick.timestamp.clone())
            .or_else(|| ledger.as_ref().map(|ledger| ledger.last_updated_at.clone())),
        seconds_since_last_tick: lifecycle.seconds_since_last_tick,
        expected_next_tick_at: latest_tick.and_then(|tick| tick.next_tick.next_tick_at.clone()),
        runner_lock,
        lifecycle,
        terminal,
        event_summary,
        ledger: include_ledger.then_some(ledger).flatten(),
    })
}

pub async fn wait_next_tick(
    ctx: &AppContext,
    run_id: &str,
    after_tick: Option<u32>,
    timeout_seconds: u64,
    include_ledger: bool,
) -> Result<StrategyStatusResult, VulcanError> {
    // Default to 0 so a fresh poll returns every tick already on disk, including
    // tick 1 if the detached runner raced ahead of the agent's first call.
    // Callers that want "from-now" semantics must pass after_tick explicitly.
    let observed_tick = after_tick.unwrap_or(0);
    let deadline = std::time::Instant::now()
        .checked_add(std::time::Duration::from_secs(timeout_seconds))
        .unwrap_or_else(std::time::Instant::now);

    loop {
        let result = status_inner(ctx, run_id, Some(observed_tick), include_ledger, false)?;
        if result.terminal || !result.new_ticks.is_empty() {
            return Ok(result);
        }
        if std::time::Instant::now() >= deadline {
            return status_inner(ctx, run_id, Some(observed_tick), include_ledger, true);
        }
        tokio::time::sleep(std::time::Duration::from_millis(750)).await;
    }
}

fn runner_lock_status(runs_dir: &std::path::Path, run_id: &str) -> StrategyRunnerLockStatus {
    let path = crate::strategy::run_lock_path(runs_dir, run_id);
    if !path.exists() {
        return StrategyRunnerLockStatus {
            present: false,
            path: Some(path.to_string_lossy().to_string()),
            pid: None,
            updated_at: None,
            age_seconds: None,
        };
    }
    let updated_at = fs::metadata(&path)
        .and_then(|metadata| metadata.modified())
        .ok()
        .map(chrono::DateTime::<Utc>::from)
        .map(|timestamp| timestamp.to_rfc3339());
    let age_seconds = updated_at.as_deref().and_then(seconds_since);
    let pid = fs::read_to_string(&path)
        .ok()
        .and_then(|text| parse_lock_pid(&text));
    StrategyRunnerLockStatus {
        present: true,
        path: Some(path.to_string_lossy().to_string()),
        pid,
        updated_at,
        age_seconds,
    }
}

fn parse_lock_pid(text: &str) -> Option<u32> {
    text.lines()
        .find_map(|line| line.strip_prefix("pid="))
        .and_then(|pid| pid.trim().parse().ok())
}

fn monitor_event_summary(
    ledger: Option<&StrategyRunLedger>,
    ticks: &[StrategyTick],
) -> StrategyMonitorEventSummary {
    let latest_tick = ticks.last();
    let (filled_count, open_count, failed_count) = ledger
        .map(|ledger| {
            let open = ledger
                .slices
                .iter()
                .filter(|slice| !is_closed_slice_status(slice.status))
                .count() as u32;
            (ledger.totals.filled_count, open, ledger.totals.failed_count)
        })
        .unwrap_or((0, 0, 0));
    StrategyMonitorEventSummary {
        latest_action: latest_tick.map(|tick| tick.execution.action.clone()),
        latest_note: latest_tick.and_then(|tick| tick.execution.note.clone()),
        total_ticks: ticks.len(),
        filled_count,
        open_count,
        failed_count,
    }
}

fn mark_open_grid_slices_cancelled(ledger: &mut StrategyRunLedger) {
    if ledger.strategy != "grid" {
        return;
    }
    let now = Utc::now().to_rfc3339();
    for slice in &mut ledger.slices {
        if !is_closed_slice_status(slice.status) {
            slice.status = crate::strategy::StrategySliceStatus::Cancelled;
            slice.note = Some("cancelled during strategy finalize".to_string());
            slice.updated_at = now.clone();
        }
    }
    ledger.last_updated_at = now;
}

fn recompute_finalize_totals(ledger: &mut StrategyRunLedger) {
    crate::strategy::runner::recompute_ledger_totals(ledger);
}

fn is_closed_slice_status(status: crate::strategy::StrategySliceStatus) -> bool {
    matches!(
        status,
        crate::strategy::StrategySliceStatus::Filled
            | crate::strategy::StrategySliceStatus::Replaced
            | crate::strategy::StrategySliceStatus::Cancelled
            | crate::strategy::StrategySliceStatus::Finalized
            | crate::strategy::StrategySliceStatus::Skipped
            | crate::strategy::StrategySliceStatus::Failed
    )
}

fn status_inner(
    ctx: &AppContext,
    run_id: &str,
    since_tick: Option<u32>,
    include_ledger: bool,
    timed_out: bool,
) -> Result<StrategyStatusResult, VulcanError> {
    let runs_dir = default_strategy_runs_dir(&ctx.vulcan_dir);
    let ledger = load_run_ledger(&runs_dir, run_id).ok();
    let summary = if let Some(ledger) = &ledger {
        summary_from_ledger(ledger)
    } else {
        load_run_report(&runs_dir, run_id)?.summary
    };
    let ticks = read_run_ticks(&runs_dir, run_id).unwrap_or_default();
    let latest_tick = ticks.last().cloned();
    let new_ticks = since_tick
        .map(|since_tick| {
            ticks
                .iter()
                .filter(|tick| tick.tick > since_tick)
                .cloned()
                .collect()
        })
        .unwrap_or_default();
    let next_tick = latest_tick.as_ref().map(|tick| tick.next_tick.clone());
    let terminal = is_terminal_status(&summary.status);
    let lifecycle = lifecycle_status(&summary, ledger.as_ref(), latest_tick.as_ref(), terminal);
    let (position_snapshot_stale, position_snapshot_warning) =
        stale_position_warning(latest_tick.as_ref(), terminal);
    Ok(StrategyStatusResult {
        summary,
        latest_tick,
        new_ticks,
        next_tick,
        lifecycle,
        terminal,
        timed_out,
        position_snapshot_stale,
        position_snapshot_warning,
        ledger: include_ledger.then_some(ledger).flatten(),
    })
}

fn stale_position_warning(
    latest_tick: Option<&StrategyTick>,
    terminal: bool,
) -> (bool, Option<String>) {
    if !terminal {
        return (false, None);
    }
    let Some(tick) = latest_tick else {
        return (false, None);
    };
    if tick.position.side.is_none() && tick.position.size_tokens == 0.0 {
        return (false, None);
    }
    let warning = format!(
        "latest_tick.position is a frozen snapshot from tick {} at {} and the run is {}; \
         the chain position may have changed since (e.g. closed in another session). \
         Call vulcan_position_list for authoritative current state before sizing trades.",
        tick.tick, tick.timestamp, "terminal"
    );
    (true, Some(warning))
}

fn lifecycle_status(
    summary: &StrategyRunSummary,
    ledger: Option<&StrategyRunLedger>,
    latest_tick: Option<&StrategyTick>,
    terminal: bool,
) -> StrategyLifecycleStatus {
    let stale_after_seconds = ledger.map(|ledger| {
        ledger
            .stale_after_seconds
            .unwrap_or_else(|| (ledger.interval_seconds.saturating_mul(2)).max(180))
    });
    let last_seen_at = latest_tick
        .map(|tick| tick.timestamp.as_str())
        .or_else(|| ledger.map(|ledger| ledger.last_updated_at.as_str()));
    let seconds_since_last_tick = last_seen_at.and_then(seconds_since);
    let stale = !terminal
        && stale_after_seconds
            .zip(seconds_since_last_tick)
            .map(|(threshold, elapsed)| elapsed > threshold)
            .unwrap_or(false);
    let recovery_hint = stale.then(|| {
        format!(
            "No strategy tick observed for {}s; reconcile and resume with `vulcan strategy resume {} --yes` before assuming replacements are active.",
            seconds_since_last_tick.unwrap_or_default(),
            summary.run_id
        )
    });
    StrategyLifecycleStatus {
        stale,
        seconds_since_last_tick,
        stale_after_seconds,
        recovery_hint,
    }
}

fn seconds_since(timestamp: &str) -> Option<u64> {
    let timestamp = chrono::DateTime::parse_from_rfc3339(timestamp).ok()?;
    let elapsed = Utc::now().signed_duration_since(timestamp.with_timezone(&Utc));
    elapsed.num_seconds().try_into().ok()
}

fn is_terminal_status(status: &str) -> bool {
    matches!(
        status,
        "completed" | "paused" | "stopped" | "failed" | "finalized"
    )
}

pub fn report(ctx: &AppContext, run_id: &str) -> Result<StrategyRunReport, VulcanError> {
    let runs_dir = default_strategy_runs_dir(&ctx.vulcan_dir);
    match load_run_report(&runs_dir, run_id) {
        Ok(mut report) => {
            if report.ledger.is_none() {
                report.ledger = load_run_ledger(&runs_dir, run_id).ok();
            }
            Ok(report)
        }
        Err(report_err) => {
            let ledger = load_run_ledger(&runs_dir, run_id).map_err(|_| report_err)?;
            let ticks = read_run_ticks(&runs_dir, run_id).unwrap_or_default();
            Ok(StrategyRunReport {
                run_id: run_id.to_string(),
                summary: summary_from_ledger(&ledger),
                config: serde_json::Value::Null,
                ticks,
                log_path: crate::strategy::log::run_log_path(&runs_dir, run_id)
                    .to_string_lossy()
                    .to_string(),
                ledger: Some(ledger),
                notes: vec!["Interim report built from live ledger; final report is written when the strategy completes.".to_string()],
            })
        }
    }
}

fn summary_from_ledger(ledger: &StrategyRunLedger) -> StrategyRunSummary {
    StrategyRunSummary {
        run_id: ledger.run_id.clone(),
        strategy: ledger.strategy.clone(),
        mode: ledger.mode,
        symbol: ledger.symbol.clone(),
        side: ledger.side,
        started_at: ledger.started_at.clone(),
        completed_at: ledger.completed_at.clone(),
        status: ledger.status.clone(),
        slices_completed: ledger.totals.filled_count + ledger.totals.partial_count,
        slices_total: ledger.slices_total,
        cumulative_notional_usdc: ledger.totals.filled_notional_usdc,
        cumulative_tokens: ledger.totals.filled_tokens,
        vwap: ledger.totals.vwap,
    }
}

impl TableRenderable for TwapRunResult {
    fn render_table(&self) {
        self.report.render_table();
    }
}

impl TableRenderable for GridRunResult {
    fn render_table(&self) {
        self.report.render_table();
    }
}

impl TableRenderable for crate::strategy::ta::TaRunResult {
    fn render_table(&self) {
        self.report.render_table();
    }
}

impl TableRenderable for crate::strategy::ta::TaDetachedRunResult {
    fn render_table(&self) {
        println!("Strategy Started: {}", self.run_id);
        println!("─────────────────────────────────────────");
        println!("  Strategy: {}", self.strategy);
        println!("  Status:   {}", self.status);
        println!("  Note:     {}", self.message);
    }
}

impl TableRenderable for crate::strategy::twap::TwapDetachedRunResult {
    fn render_table(&self) {
        println!("Strategy Started: {}", self.run_id);
        println!("─────────────────────────────────────────");
        println!("  Strategy: {}", self.strategy);
        println!("  Status:   {}", self.status);
        println!("  Message:  {}", self.message);
    }
}

impl TableRenderable for crate::strategy::grid::GridDetachedRunResult {
    fn render_table(&self) {
        println!("Strategy Started: {}", self.run_id);
        println!("─────────────────────────────────────────");
        println!("  Strategy: {}", self.strategy);
        println!("  Status:   {}", self.status);
        println!("  Message:  {}", self.message);
    }
}

impl TableRenderable for crate::strategy::grid::GridLiveOrderInspection {
    fn render_table(&self) {
        println!("Grid Reconciliation");
        println!("─────────────────────────────────────────");
        println!("  {}", self.summary);
        if !self.missing.is_empty() {
            println!("  Missing:");
            for missing in &self.missing {
                println!("    - {}", missing);
            }
        }
    }
}

impl TableRenderable for StrategyControlResult {
    fn render_table(&self) {
        println!("Strategy Control");
        println!("────────────────");
        println!("  Run ID:  {}", self.run_id);
        println!("  Action:  {}", self.action.as_str());
        println!("  Status:  {}", self.status);
        println!("  Message: {}", self.message);
    }
}

impl TableRenderable for StrategyFinalizeResult {
    fn render_table(&self) {
        println!("Strategy Finalized: {}", self.run_id);
        println!("─────────────────────────────────────────");
        println!("  Status: {}", self.status);
        println!("  Stop:   {}", self.stop.message);
        if self.waited {
            println!(
                "  Wait:   {}",
                if self.wait_timed_out {
                    "timed out before terminal tick"
                } else {
                    "observed runner progress"
                }
            );
        }
        if let Some(cancel) = &self.cancel_orders {
            println!(
                "  Cancel: {} order(s){}",
                cancel.cancelled_ids.len(),
                cancel
                    .tx_signature
                    .as_ref()
                    .map(|sig| format!(" tx {}", sig))
                    .unwrap_or_default()
            );
        }
        if let Some(close) = &self.close_position {
            println!(
                "  Close:  {} {}{}",
                close.side_closed,
                close.size_closed,
                close
                    .tx_signature
                    .as_ref()
                    .map(|sig| format!(" tx {}", sig))
                    .unwrap_or_default()
            );
        } else if let Some(note) = &self.close_position_note {
            println!("  Close:  {}", note);
        }
        println!();
        self.monitor.render_table();
    }
}

impl TableRenderable for StrategyRunReport {
    fn render_table(&self) {
        if self.summary.strategy == "ta" {
            render_ta_report(self);
            return;
        }
        println!("Strategy Run: {}", self.run_id);
        println!("─────────────────────────────────────────");
        println!("  Strategy: {}", self.summary.strategy);
        println!("  Mode:     {}", self.summary.mode.as_str());
        if let Some(wallet) = self
            .ledger
            .as_ref()
            .and_then(|ledger| ledger.wallet.as_ref())
        {
            println!("  Wallet:   {} ({})", wallet.name, wallet.address);
        } else {
            println!("  Wallet:   none");
        }
        println!("  Symbol:   {}", self.summary.symbol);
        println!("  Side:     {}", self.summary.side.as_str());
        println!("  Status:   {}", self.summary.status);
        println!(
            "  Filled:   {:.6} tokens / {:.4} USDC",
            self.summary.cumulative_tokens, self.summary.cumulative_notional_usdc
        );
        if let Some(vwap) = self.summary.vwap {
            println!("  VWAP:     {:.6}", vwap);
        }
        println!(
            "  Slices:   {}/{}",
            self.summary.slices_completed, self.summary.slices_total
        );
        println!("  Log:      {}", self.log_path);
        if let Some(ledger) = &self.ledger {
            println!();
            render_ledger(ledger);
        }
        if let Some(tick) = self.ticks.last() {
            println!();
            render_tick(tick);
        }
    }
}

fn render_ta_report(report: &StrategyRunReport) {
    println!("TA Strategy Run: {}", report.run_id);
    println!("─────────────────────────────────────────");
    if let Some(label) = report.ledger.as_ref().and_then(|l| l.run_label.as_ref()) {
        println!("  Label:    {}", label);
    }
    println!("  Mode:     {}", report.summary.mode.as_str());
    if let Some(wallet) = report.ledger.as_ref().and_then(|l| l.wallet.as_ref()) {
        println!("  Wallet:   {} ({})", wallet.name, wallet.address);
    }
    println!("  Symbol:   {}", report.summary.symbol);
    let status_label = format_ta_status(&report.summary.status, report.ledger.as_ref());
    println!("  Status:   {}", status_label);
    println!("  Started:  {}", report.summary.started_at);
    if let Some(completed) = &report.summary.completed_at {
        println!("  Ended:    {}", completed);
    }
    if let Some(ledger) = &report.ledger {
        let cadence_desc = report
            .config
            .get("strategy_config")
            .and_then(|sc| sc.get("cadence"))
            .and_then(|c| {
                serde_json::from_value::<crate::strategy::ta_rule::Cadence>(c.clone()).ok()
            })
            .map(|c| c.describe())
            .unwrap_or_else(|| format!("fixed {}s", ledger.interval_seconds));
        println!("  Cadence:  {}", cadence_desc);
        if let Some(reason) = &ledger.pause_reason {
            println!("  Reason:   {}", reason);
        }
    }
    println!("  Log:      {}", report.log_path);

    let ledger = match &report.ledger {
        Some(l) => l,
        None => return,
    };

    // Fills table — one row per filled slice (rule firing that actually executed).
    let fills: Vec<&crate::strategy::types::StrategyLedgerSlice> = ledger
        .slices
        .iter()
        .filter(|s| {
            matches!(
                s.status,
                crate::strategy::StrategySliceStatus::Filled
                    | crate::strategy::StrategySliceStatus::Partial
            ) && s.fill_tokens.is_some()
        })
        .collect();

    println!();
    if fills.is_empty() {
        println!("Fills: none yet (firings without executed market orders are normal during warmup or NoOp rules)");
    } else {
        println!("Fills: {} (of {} ticks)", fills.len(), ledger.slices.len());
        let rows: Vec<Vec<String>> = fills
            .iter()
            .map(|slice| {
                let rule = slice
                    .metadata
                    .as_ref()
                    .and_then(|m| m.get("rule_name"))
                    .and_then(|v| v.as_str())
                    .map(|s| s.to_string())
                    .unwrap_or_else(|| "-".into());
                let side = infer_fill_side(slice);
                vec![
                    slice.index.to_string(),
                    rule,
                    side,
                    slice
                        .fill_tokens
                        .map(|v| format!("{:.6}", v))
                        .unwrap_or_else(|| "-".into()),
                    slice
                        .fill_price
                        .map(|v| format!("{:.6}", v))
                        .unwrap_or_else(|| "-".into()),
                    slice
                        .fee
                        .map(|v| format!("${:.4}", v))
                        .unwrap_or_else(|| "-".into()),
                    slice
                        .fill_notional_usdc
                        .map(|v| format!("${:.4}", v))
                        .unwrap_or_else(|| "-".into()),
                    slice
                        .tx_signature
                        .as_ref()
                        .map(|tx| abbreviate(tx))
                        .unwrap_or_else(|| "paper".into()),
                ]
            })
            .collect();
        crate::output::table::render_table(
            &[
                "Tick", "Rule", "Side", "Tokens", "Price", "Fee", "Notional", "Tx",
            ],
            rows,
        );
    }

    // Performance summary — TA-meaningful: tick count vs firings vs fills are different.
    let total_ticks = ledger.slices.len() as u32;
    let total_firings: u32 = ledger
        .slices
        .iter()
        .filter(|s| {
            s.metadata
                .as_ref()
                .and_then(|m| m.get("rule_name"))
                .and_then(|v| v.as_str())
                .is_some()
        })
        .count() as u32;
    let no_rule_ticks = total_ticks.saturating_sub(total_firings);
    let perf = compute_ta_performance(&fills);
    println!();
    println!("Performance");
    crate::output::table::render_table(
        &["Field", "Value"],
        vec![
            vec!["Total ticks".to_string(), total_ticks.to_string()],
            vec!["Rule firings".to_string(), total_firings.to_string()],
            vec!["Fills".to_string(), ledger.totals.filled_count.to_string()],
            vec!["No-rule ticks".to_string(), no_rule_ticks.to_string()],
            vec![
                "Failed firings".to_string(),
                ledger.totals.failed_count.to_string(),
            ],
            vec![
                "Volume traded".to_string(),
                format!(
                    "{:.4} USDC over {:.6} tokens",
                    perf.total_traded_notional, perf.total_traded_tokens
                ),
            ],
            vec!["Fees".to_string(), format!("${:.4}", perf.total_fees)],
            vec![
                "Net token delta".to_string(),
                format!("{:+.6} tokens", perf.net_token_delta),
            ],
            vec![
                "Cash flow (fills only)".to_string(),
                format!(
                    "{:+.4} USDC (positive = received, negative = spent)",
                    perf.cash_flow
                ),
            ],
            vec![
                "Realized P/L estimate".to_string(),
                format!(
                    "{:+.4} USDC{}",
                    perf.cash_flow - perf.total_fees,
                    if perf.net_token_delta.abs() > 1e-9 {
                        format!(
                            " (with open {:+.6}-token position — mark-to-market not included)",
                            perf.net_token_delta
                        )
                    } else {
                        String::new()
                    }
                ),
            ],
        ],
    );

    // Tick-level firing breakdown — useful when a strategy has many ticks but few fills.
    let firings_by_rule = count_firings_by_rule(ledger);
    if !firings_by_rule.is_empty() {
        println!();
        println!("Firings by rule");
        let rows: Vec<Vec<String>> = firings_by_rule
            .into_iter()
            .map(|(rule, count)| vec![rule, count.to_string()])
            .collect();
        crate::output::table::render_table(&["Rule", "Firings"], rows);
    }

    // Live risk snapshot from the most recent tick — only useful when there's an open position.
    if let Some(tick) = report.ticks.last() {
        if tick.position.size_tokens.abs() > f64::EPSILON {
            println!();
            println!("Live Position Risk");
            crate::output::table::render_table(
                &["Field", "Value"],
                vec![
                    vec![
                        "Side".to_string(),
                        tick.position
                            .side
                            .clone()
                            .unwrap_or_else(|| "none".to_string()),
                    ],
                    vec![
                        "Size".to_string(),
                        format!("{:.6} tokens", tick.position.size_tokens),
                    ],
                    vec![
                        "Entry / Mark".to_string(),
                        format!(
                            "{} / {}",
                            tick.position
                                .entry_price
                                .map(|v| format!("${:.4}", v))
                                .unwrap_or_else(|| "n/a".into()),
                            tick.position
                                .mark_price
                                .map(|v| format!("${:.4}", v))
                                .unwrap_or_else(|| "n/a".into()),
                        ),
                    ],
                    vec![
                        "Notional".to_string(),
                        format!("${:.4}", tick.position.notional_usdc),
                    ],
                    vec![
                        "uPnL".to_string(),
                        format!(
                            "{:+.4} USDC{}",
                            tick.position.unrealized_pnl,
                            tick.position
                                .unrealized_pnl_pct
                                .map(|pct| format!(" ({:+.2}%)", pct))
                                .unwrap_or_default(),
                        ),
                    ],
                    vec![
                        "Leverage".to_string(),
                        format!(
                            "{:.2}x",
                            tick.position
                                .leverage
                                .unwrap_or(tick.position.exposure_ratio)
                        ),
                    ],
                    vec![
                        "Liquidation".to_string(),
                        tick.position
                            .liquidation_price
                            .map(|v| format!("${:.4}", v))
                            .unwrap_or_else(|| "n/a".to_string()),
                    ],
                    vec![
                        "Distance to liq".to_string(),
                        tick.position
                            .distance_to_liq_bps
                            .map(|bps| {
                                let pct = bps / 100.0;
                                let flag = if bps < 200.0 {
                                    " [CRITICAL]"
                                } else if bps < 500.0 {
                                    " [CLOSE]"
                                } else if bps < 1500.0 {
                                    " [moderate]"
                                } else {
                                    ""
                                };
                                format!("{:+.0}bps ({:+.2}%){}", bps, pct, flag)
                            })
                            .unwrap_or_else(|| "n/a".to_string()),
                    ],
                    vec![
                        "Margin (initial / maint)".to_string(),
                        format!(
                            "{} / {}",
                            tick.position
                                .initial_margin_usdc
                                .map(|v| format!("${:.4}", v))
                                .unwrap_or_else(|| "n/a".into()),
                            tick.position
                                .maintenance_margin_usdc
                                .map(|v| format!("${:.4}", v))
                                .unwrap_or_else(|| "n/a".into()),
                        ),
                    ],
                    vec![
                        "Account".to_string(),
                        format!(
                            "equity {} | risk {}",
                            tick.account_health
                                .equity
                                .map(|v| format!("${:.4}", v))
                                .unwrap_or_else(|| "n/a".into()),
                            tick.account_health
                                .risk_state
                                .as_deref()
                                .unwrap_or("unknown"),
                        ),
                    ],
                ],
            );
        }
    }

    if !report.notes.is_empty() {
        println!();
        println!("Notes");
        for n in &report.notes {
            println!("  • {}", n);
        }
    }
}

fn format_ta_status(status: &str, ledger: Option<&StrategyRunLedger>) -> String {
    let base = status.to_string();
    if status == "errored" || status == "paused" {
        if let Some(reason) = ledger.and_then(|l| l.pause_reason.as_ref()) {
            return format!("{} — {}", base, reason);
        }
    }
    base
}

fn infer_fill_side(slice: &crate::strategy::types::StrategyLedgerSlice) -> String {
    if let Some(meta) = &slice.metadata {
        if let Some(side) = meta.get("executed_side").and_then(|v| v.as_str()) {
            return side.to_string();
        }
    }
    "-".into()
}

struct TaPerformance {
    total_traded_tokens: f64,
    total_traded_notional: f64,
    total_fees: f64,
    net_token_delta: f64,
    cash_flow: f64,
}

fn compute_ta_performance(fills: &[&crate::strategy::types::StrategyLedgerSlice]) -> TaPerformance {
    let mut total_traded_tokens = 0.0;
    let mut total_traded_notional = 0.0;
    let mut total_fees = 0.0;
    let mut net_token_delta = 0.0;
    let mut cash_flow = 0.0;
    for slice in fills {
        let tokens = slice.fill_tokens.unwrap_or(0.0);
        let notional = slice.fill_notional_usdc.unwrap_or(0.0);
        let fee = slice.fee.unwrap_or(0.0);
        total_traded_tokens += tokens.abs();
        total_traded_notional += notional.abs();
        total_fees += fee;
        let side = infer_fill_side(slice);
        match side.as_str() {
            "buy" => {
                net_token_delta += tokens.abs();
                cash_flow -= notional.abs();
            }
            "sell" => {
                net_token_delta -= tokens.abs();
                cash_flow += notional.abs();
            }
            _ => {}
        }
    }
    TaPerformance {
        total_traded_tokens,
        total_traded_notional,
        total_fees,
        net_token_delta,
        cash_flow,
    }
}

fn count_firings_by_rule(ledger: &StrategyRunLedger) -> Vec<(String, u32)> {
    let mut counts: std::collections::BTreeMap<String, u32> = std::collections::BTreeMap::new();
    for slice in &ledger.slices {
        if let Some(rule) = slice
            .metadata
            .as_ref()
            .and_then(|m| m.get("rule_name"))
            .and_then(|v| v.as_str())
        {
            *counts.entry(rule.to_string()).or_insert(0) += 1;
        }
    }
    counts.into_iter().collect()
}

impl TableRenderable for StrategyRunsResult {
    fn render_table(&self) {
        if self.runs.is_empty() {
            println!("No strategy runs found.");
            return;
        }
        let rows = self
            .runs
            .iter()
            .map(|run| {
                vec![
                    run.run_id.clone(),
                    run.strategy.clone(),
                    run.mode.as_str().to_string(),
                    run.symbol.clone(),
                    run.status.clone(),
                    format!("{}/{}", run.slices_completed, run.slices_total),
                    format!("{:.4}", run.cumulative_notional_usdc),
                ]
            })
            .collect();
        crate::output::table::render_table(
            &[
                "Run ID", "Strategy", "Mode", "Symbol", "Status", "Steps", "Notional",
            ],
            rows,
        );
    }
}

impl TableRenderable for StrategyStatusResult {
    fn render_table(&self) {
        println!("Strategy Status: {}", self.summary.run_id);
        println!("─────────────────────────────────────────");
        println!("  Status: {}", self.summary.status);
        println!(
            "  Progress: {}/{} steps",
            self.summary.slices_completed, self.summary.slices_total
        );
        println!(
            "  Filled: {:.6} tokens / {:.4} USDC",
            self.summary.cumulative_tokens, self.summary.cumulative_notional_usdc
        );
        if let Some(elapsed) = self.lifecycle.seconds_since_last_tick {
            println!(
                "  Heartbeat: last tick {}s ago{}",
                elapsed,
                self.lifecycle
                    .stale_after_seconds
                    .map(|threshold| format!(" (stale after {}s)", threshold))
                    .unwrap_or_default()
            );
        }
        if self.lifecycle.stale {
            if let Some(hint) = &self.lifecycle.recovery_hint {
                println!("  Stale: {}", hint);
            }
        }
        if let Some(warning) = &self.position_snapshot_warning {
            println!("  Position snapshot: {}", warning);
        }
        if let Some(ledger) = &self.ledger {
            println!();
            render_ledger(ledger);
        }
        if self.timed_out {
            println!();
            println!("Wait timed out before a new tick or terminal status was observed.");
        }
        if !self.new_ticks.is_empty() {
            println!();
            println!("New ticks: {}", self.new_ticks.len());
            for tick in &self.new_ticks {
                render_tick(tick);
            }
            return;
        }
        if let Some(tick) = &self.latest_tick {
            println!();
            render_tick(tick);
        }
    }
}

impl TableRenderable for StrategyMonitorResult {
    fn render_table(&self) {
        println!("Strategy Monitor: {}", self.summary.run_id);
        println!("─────────────────────────────────────────");
        println!("  Strategy: {}", self.summary.strategy);
        println!("  Status:   {}", self.summary.status);
        println!("  Terminal: {}", self.terminal);
        if let Some(tick) = self.last_observed_tick {
            println!(
                "  Last Tick: {} at {} ({}s ago)",
                tick,
                self.last_observed_at.as_deref().unwrap_or("unknown"),
                self.seconds_since_last_tick.unwrap_or_default()
            );
        } else if let Some(last) = &self.last_observed_at {
            println!("  Last Update: {}", last);
        }
        if let Some(next) = &self.expected_next_tick_at {
            println!("  Expected Next: {}", next);
        }
        println!(
            "  Runner Lock: {}{}",
            if self.runner_lock.present {
                "present"
            } else {
                "missing"
            },
            self.runner_lock
                .age_seconds
                .map(|age| format!(" ({}s old)", age))
                .unwrap_or_default()
        );
        if self.lifecycle.stale {
            if let Some(hint) = &self.lifecycle.recovery_hint {
                println!("  Stale: {}", hint);
            }
        }
        println!(
            "  Events: {} ticks | {} filled | {} open | {} failed",
            self.event_summary.total_ticks,
            self.event_summary.filled_count,
            self.event_summary.open_count,
            self.event_summary.failed_count
        );
        if let Some(action) = &self.event_summary.latest_action {
            println!("  Latest Action: {}", action);
        }
        if let Some(note) = &self.event_summary.latest_note {
            println!("  Latest Note: {}", note);
        }
        if let Some(ledger) = &self.ledger {
            println!();
            render_ledger(ledger);
        }
    }
}

fn render_ledger(ledger: &StrategyRunLedger) {
    let open = ledger
        .slices
        .iter()
        .filter(|slice| !is_closed_slice_status(slice.status))
        .count();
    println!(
        "Ledger: {} {} ({} filled, {} open)",
        ledger.slices_total,
        step_label_plural(ledger),
        ledger.totals.filled_count,
        open
    );
    println!(
        "  Target: {} | cadence {}s",
        target_label(ledger),
        ledger.interval_seconds
    );
    println!(
        "  Totals: {:.4} USDC / {:.6} tokens | VWAP {} | fees {:.4}",
        ledger.totals.filled_notional_usdc,
        ledger.totals.filled_tokens,
        ledger
            .totals
            .vwap
            .map(|v| format!("{:.6}", v))
            .unwrap_or_else(|| "n/a".to_string()),
        ledger.totals.fees
    );
    if let Some(executable) = ledger.totals.executable_notional_usdc {
        println!(
            "  Lot plan: executable {:.4} USDC / {:.6} tokens",
            executable,
            ledger.totals.executable_tokens.unwrap_or_default()
        );
    }
    if let Some(reason) = &ledger.pause_reason {
        println!("  Pause:  {}", reason);
    }
    println!("  {}:", title_case(step_label_plural(ledger)));
    for line in ledger_lines(ledger) {
        println!("    {}", line);
    }
}

fn target_label(ledger: &StrategyRunLedger) -> String {
    if let Some(notional) = ledger.totals.planned_notional_usdc {
        format!("${:.4} {}", notional, ledger.symbol)
    } else if let Some(tokens) = ledger.totals.planned_tokens {
        format!("{:.6} {}", tokens, ledger.symbol)
    } else {
        ledger.symbol.clone()
    }
}

fn step_label_plural(ledger: &StrategyRunLedger) -> &'static str {
    if ledger.strategy == "twap" {
        "slices"
    } else {
        "steps"
    }
}

fn step_label_singular(ledger: &StrategyRunLedger) -> &'static str {
    if ledger.strategy == "twap" {
        "slice"
    } else {
        "step"
    }
}

fn title_case(label: &str) -> String {
    let mut chars = label.chars();
    match chars.next() {
        Some(first) => first.to_uppercase().collect::<String>() + chars.as_str(),
        None => String::new(),
    }
}

fn ledger_lines(ledger: &StrategyRunLedger) -> Vec<String> {
    let mut lines = Vec::new();
    let mut collapsed_pending = 0usize;
    for slice in &ledger.slices {
        let is_quiet_pending =
            matches!(slice.status, crate::strategy::StrategySliceStatus::Planned)
                && slice.index > 4
                && slice.index < ledger.slices_total;
        if is_quiet_pending {
            collapsed_pending += 1;
            continue;
        }
        if collapsed_pending > 0 {
            lines.push(format!(
                "... +{} planned {}",
                collapsed_pending,
                step_label_plural(ledger)
            ));
            collapsed_pending = 0;
        }
        lines.push(format_step(slice, step_label_singular(ledger)));
    }
    if collapsed_pending > 0 {
        lines.push(format!(
            "... +{} planned {}",
            collapsed_pending,
            step_label_plural(ledger)
        ));
    }
    lines
}

fn format_step(slice: &crate::strategy::types::StrategyLedgerSlice, label: &str) -> String {
    let planned = if let Some(notional) = slice.planned_notional_usdc {
        format!("${:.4}", notional)
    } else if let Some(tokens) = slice.planned_tokens {
        format!("{:.6} tokens", tokens)
    } else {
        "size n/a".to_string()
    };
    let mut line = format!(
        "[{}] {} {} {} scheduled {}",
        slice.status.as_str(),
        label,
        slice.index,
        planned,
        slice.scheduled_at
    );
    if let Some(price) = slice.fill_price {
        line.push_str(&format!(" fill @ {:.6}", price));
    }
    if let Some(notional) = slice.fill_notional_usdc {
        line.push_str(&format!(" ({:.4} USDC)", notional));
    } else if let Some(notional) = slice.executable_notional_usdc {
        line.push_str(&format!(" executable {:.4} USDC", notional));
    }
    if let Some(lots) = slice.executable_base_lots {
        line.push_str(&format!(" {} base lots", lots));
    }
    if let Some(tx) = &slice.tx_signature {
        line.push_str(&format!(" tx {}", tx));
    }
    if let Some(order_id) = slice
        .metadata
        .as_ref()
        .and_then(|metadata| metadata.get("order_id"))
        .and_then(|value| value.as_str())
    {
        line.push_str(&format!(" order {}", abbreviate(order_id)));
    }
    if let Some(reason) = &slice.pause_reason {
        line.push_str(&format!(" pause {}", reason));
    }
    if let Some(note) = &slice.note {
        line.push_str(&format!(" note {}", note));
    }
    if let Some(note) = &slice.rounding_note {
        line.push_str(&format!(" rounding {}", note));
    }
    line
}

fn abbreviate(value: &str) -> String {
    if value.len() <= 12 {
        value.to_string()
    } else {
        format!("{}...{}", &value[..6], &value[value.len() - 6..])
    }
}

fn render_tick(tick: &StrategyTick) {
    println!("Latest Tick {}", tick.tick);
    println!(
        "  Planned:  {} | executable {}",
        tick.planned_action
            .slice_notional_usdc
            .map(|v| format!("{:.4} USDC", v))
            .unwrap_or_else(|| "n/a".to_string()),
        tick.planned_action
            .executable_notional_usdc
            .map(|v| format!("{:.4} USDC", v))
            .unwrap_or_else(|| "n/a".to_string())
    );
    println!(
        "  Fill:     {} @ {} | tokens={} notional={} fee={}",
        tick.execution.action.as_str(),
        tick.execution
            .fill_price
            .map(|v| format!("{:.6}", v))
            .unwrap_or_else(|| "n/a".to_string()),
        tick.execution
            .fill_tokens
            .map(|v| format!("{:.6}", v))
            .unwrap_or_else(|| "n/a".to_string()),
        tick.execution
            .fill_notional_usdc
            .map(|v| format!("{:.4}", v))
            .unwrap_or_else(|| "n/a".to_string()),
        tick.execution
            .fee
            .map(|v| format!("{:.4}", v))
            .unwrap_or_else(|| "n/a".to_string())
    );
    if let Some(tx) = &tick.execution.tx_signature {
        println!("  Tx:       {}", tx);
    }
    println!(
        "  Progress: {:.6} tokens / {:.4} USDC | VWAP {} | fees {:.4}",
        tick.progress.cumulative_tokens,
        tick.progress.cumulative_notional_usdc,
        tick.progress
            .vwap
            .map(|v| format!("{:.6}", v))
            .unwrap_or_else(|| "n/a".to_string()),
        tick.progress.cumulative_fees
    );
    println!(
        "  Position: {} {:.6} tokens | notional {:.4} | uPnL {:+.4} | exposure {:.2}x",
        tick.position.side.as_deref().unwrap_or("none"),
        tick.position.size_tokens,
        tick.position.notional_usdc,
        tick.position.unrealized_pnl,
        tick.position.exposure_ratio
    );
}

struct PreparedTaInput {
    input: crate::strategy::ta::TaStartInput,
    detached_requested: bool,
}

fn build_ta_start_input(args: TaStartArgs) -> Result<PreparedTaInput, VulcanError> {
    let raw_config = match (args.config_file.clone(), args.config_json.clone()) {
        (Some(_), Some(_)) => {
            return Err(VulcanError::validation(
                "INVALID_TA_STRATEGY_CONFIG",
                "pass either --config-file or --config-json, not both",
            ));
        }
        (Some(path), None) => fs::read_to_string(&path)
            .map_err(|e| VulcanError::io("CONFIG_FILE_READ_FAILED", e.to_string()))?,
        (None, Some(json)) => json,
        (None, None) => {
            return Err(VulcanError::validation(
                "INVALID_TA_STRATEGY_CONFIG",
                "provide --config-file <path> or --config-json '<json>'",
            ));
        }
    };
    let config: crate::strategy::ta_rule::TaStrategyConfig = serde_json::from_str(&raw_config)
        .map_err(|e| {
            VulcanError::validation(
                "INVALID_TA_STRATEGY_CONFIG",
                format!("could not parse TA strategy config JSON: {e}"),
            )
        })?;
    let mode = match args.mode {
        crate::cli::strategy::StrategyModeArg::Paper => crate::strategy::types::StrategyMode::Paper,
        crate::cli::strategy::StrategyModeArg::DryRun => {
            crate::strategy::types::StrategyMode::DryRun
        }
        crate::cli::strategy::StrategyModeArg::ConfirmEach => {
            crate::strategy::types::StrategyMode::ConfirmEach
        }
        crate::cli::strategy::StrategyModeArg::AutoExecute => {
            crate::strategy::types::StrategyMode::AutoExecute
        }
    };
    let input = crate::strategy::ta::TaStartInput {
        config,
        mode,
        max_ticks: args.max_ticks,
        run_until_stopped: args.run_until_stopped,
        run_label: args.run_label,
        no_sleep: args.no_sleep,
        safety_overrides: crate::strategy::ta::SafetyOverrides {
            max_total_notional_usdc: args.max_total_notional_usdc,
            max_step_notional_usdc: args.max_step_notional_usdc,
            max_price_drift_bps: args.max_price_drift_bps,
            max_exposure_ratio: args.max_exposure_ratio,
            reconcile_attempts: args.reconcile_attempts,
            reconcile_delay_ms: args.reconcile_delay_ms,
        },
    };
    Ok(PreparedTaInput {
        input,
        detached_requested: args.detached,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn lifecycle_status_marks_running_grid_stale_after_threshold() {
        let summary = StrategyRunSummary {
            run_id: "grid-test".to_string(),
            strategy: "grid".to_string(),
            mode: crate::strategy::StrategyMode::Paper,
            symbol: "SOL".to_string(),
            side: crate::strategy::StrategySide::Buy,
            started_at: Utc::now().to_rfc3339(),
            completed_at: None,
            status: "running".to_string(),
            slices_completed: 0,
            slices_total: 0,
            cumulative_notional_usdc: 0.0,
            cumulative_tokens: 0.0,
            vwap: None,
        };
        let ledger = StrategyRunLedger {
            run_id: summary.run_id.clone(),
            strategy: "grid".to_string(),
            run_label: None,
            wallet: None,
            symbol: summary.symbol.clone(),
            side: summary.side,
            mode: summary.mode,
            started_at: summary.started_at.clone(),
            completed_at: None,
            status: "running".to_string(),
            interval_seconds: 60,
            run_until_stopped: true,
            stale_after_seconds: Some(1),
            slices_total: 0,
            totals: crate::strategy::types::StrategyLedgerTotals::default(),
            slices: Vec::new(),
            last_updated_at: (Utc::now() - chrono::Duration::seconds(5)).to_rfc3339(),
            pause_reason: None,
            safety_policy: None,
        };

        let lifecycle = lifecycle_status(&summary, Some(&ledger), None, false);

        assert!(lifecycle.stale);
        assert_eq!(lifecycle.stale_after_seconds, Some(1));
        assert!(lifecycle
            .recovery_hint
            .unwrap()
            .contains("vulcan strategy resume"));
    }

    fn tick_with_position(tick: u32, side: Option<&str>, size_tokens: f64) -> StrategyTick {
        StrategyTick {
            run_id: "run".to_string(),
            strategy: "grid".to_string(),
            mode: crate::strategy::StrategyMode::AutoExecute,
            symbol: "ZEC".to_string(),
            side: crate::strategy::StrategySide::Buy,
            tick,
            slices_total: 0,
            timestamp: "2026-05-18T00:00:00Z".to_string(),
            elapsed_ms: 0,
            market: crate::strategy::types::StrategyMarketSnapshot {
                mark_price: 0.0,
                mid_price: 0.0,
                funding_rate: 0.0,
            },
            planned_action: crate::strategy::types::StrategyPlannedAction {
                action: "noop".to_string(),
                order_type: "noop".to_string(),
                slice_notional_usdc: None,
                slice_tokens: None,
                executable_notional_usdc: None,
                executable_tokens: None,
                executable_base_lots: None,
                rounding_note: None,
            },
            execution: crate::strategy::types::StrategyExecution::default(),
            progress: crate::strategy::types::StrategyProgress::default(),
            position: crate::strategy::types::StrategyPositionSnapshot {
                side: side.map(|s| s.to_string()),
                size_tokens,
                ..Default::default()
            },
            position_after_action: None,
            account_health: crate::strategy::types::StrategyAccountHealth::default(),
            next_tick: crate::strategy::types::StrategyNextTick {
                next_tick_at: None,
                delay_seconds: None,
                stop_reason: None,
            },
        }
    }

    #[test]
    fn stale_position_warning_fires_on_terminal_with_open_position() {
        let tick = tick_with_position(22, Some("short"), 0.38);
        let (stale, warning) = stale_position_warning(Some(&tick), true);
        assert!(stale);
        let warning = warning.expect("warning populated");
        assert!(warning.contains("vulcan_position_list"));
        assert!(warning.contains("tick 22"));
    }

    #[test]
    fn stale_position_warning_silent_when_flat() {
        let tick = tick_with_position(22, None, 0.0);
        let (stale, warning) = stale_position_warning(Some(&tick), true);
        assert!(!stale);
        assert!(warning.is_none());
    }

    #[test]
    fn stale_position_warning_silent_when_running() {
        let tick = tick_with_position(22, Some("short"), 0.38);
        let (stale, warning) = stale_position_warning(Some(&tick), false);
        assert!(!stale);
        assert!(warning.is_none());
    }
}

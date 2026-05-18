//! Generic strategy ledger lifecycle helpers.

use crate::context::AppContext;
use crate::error::VulcanError;
use crate::output::OutputFormat;
use crate::strategy::log::write_tick;
use crate::strategy::types::{
    StrategyExecution, StrategyLedgerTotals, StrategyRunLedger, StrategySliceStatus, StrategyTick,
};
use chrono::Utc;
use serde::Serialize;
use std::path::Path;

pub(crate) fn mark_step_ready(ledger: &mut StrategyRunLedger, index: u32) {
    let now = Utc::now().to_rfc3339();
    if let Some(step) = ledger.slices.iter_mut().find(|step| step.index == index) {
        step.status = StrategySliceStatus::Ready;
        step.updated_at = now.clone();
    }
    ledger.last_updated_at = now;
}

pub(crate) fn update_ledger_step(
    ledger: &mut StrategyRunLedger,
    index: u32,
    status: StrategySliceStatus,
    execution: &StrategyExecution,
) {
    let now = Utc::now().to_rfc3339();
    if let Some(step) = ledger.slices.iter_mut().find(|step| step.index == index) {
        step.status = status;
        step.tx_signature = execution.tx_signature.clone();
        step.fill_id = execution.fill_id.clone();
        step.fill_price = execution.fill_price;
        step.fill_tokens = execution.fill_tokens;
        step.fill_notional_usdc = execution.fill_notional_usdc;
        step.fee = execution.fee;
        step.note = execution.note.clone();
        if execution.action == "error" {
            step.error_code = execution
                .note
                .as_deref()
                .and_then(|note| note.split_once(':').map(|(code, _)| code.to_string()));
        }
        step.updated_at = now.clone();
        if execution.tx_signature.is_some() {
            step.submitted_at.get_or_insert_with(|| now.clone());
        }
        if matches!(status, StrategySliceStatus::Resting) {
            step.submitted_at.get_or_insert_with(|| now.clone());
        }
        if matches!(
            status,
            StrategySliceStatus::Filled | StrategySliceStatus::Partial
        ) {
            step.filled_at = Some(now.clone());
        }
        if matches!(status, StrategySliceStatus::Paused) {
            step.pause_reason = execution.note.clone();
        }
    }
    ledger.last_updated_at = now;
}

pub(crate) fn recompute_ledger_totals(ledger: &mut StrategyRunLedger) {
    let mut totals = StrategyLedgerTotals {
        planned_notional_usdc: ledger.totals.planned_notional_usdc,
        planned_tokens: ledger.totals.planned_tokens,
        planned_base_lots: ledger.totals.planned_base_lots,
        ..Default::default()
    };
    for step in &ledger.slices {
        if matches!(
            step.status,
            StrategySliceStatus::Submitted
                | StrategySliceStatus::Resting
                | StrategySliceStatus::Filled
                | StrategySliceStatus::Partial
                | StrategySliceStatus::Paused
        ) {
            totals.submitted_count += 1;
            totals.submitted_notional_usdc += step
                .executable_notional_usdc
                .or(step.planned_notional_usdc)
                .unwrap_or(0.0);
        }
        totals.executable_notional_usdc = Some(
            totals.executable_notional_usdc.unwrap_or(0.0)
                + step.executable_notional_usdc.unwrap_or(0.0),
        );
        totals.executable_tokens =
            Some(totals.executable_tokens.unwrap_or(0.0) + step.executable_tokens.unwrap_or(0.0));
        totals.executable_base_lots =
            Some(totals.executable_base_lots.unwrap_or(0) + step.executable_base_lots.unwrap_or(0));
        match step.status {
            StrategySliceStatus::Filled => totals.filled_count += 1,
            StrategySliceStatus::Partial => totals.partial_count += 1,
            StrategySliceStatus::Skipped => totals.skipped_count += 1,
            StrategySliceStatus::Paused => totals.paused_count += 1,
            StrategySliceStatus::Failed => totals.failed_count += 1,
            _ => {}
        }
        totals.filled_notional_usdc += step.fill_notional_usdc.unwrap_or(0.0);
        totals.filled_tokens += step.fill_tokens.unwrap_or(0.0);
        totals.fees += step.fee.unwrap_or(0.0);
    }
    if totals.filled_tokens.abs() > f64::EPSILON {
        totals.vwap = Some(totals.filled_notional_usdc / totals.filled_tokens);
    }
    ledger.totals = totals;
}

pub(crate) fn emit_strategy_tick<F>(
    ctx: &AppContext,
    runs_dir: &Path,
    tick: &StrategyTick,
    render_table: F,
) -> Result<(), VulcanError>
where
    F: FnOnce(&StrategyTick),
{
    write_tick(runs_dir, tick)?;
    record_strategy_event(ctx, &format!("strategy.{}.tick", tick.strategy), tick, None);
    if ctx.output_format == OutputFormat::Table {
        render_table(tick);
    } else {
        render_strategy_tick_diagnostic(tick);
    }
    Ok(())
}

pub(crate) fn record_strategy_event<T: Serialize>(
    ctx: &AppContext,
    name: &str,
    value: &T,
    error_code: Option<&str>,
) {
    let Some(log) = &ctx.agent_log else {
        return;
    };
    let result = serde_json::to_value(value).ok();
    log.record_call(
        "strategy",
        name,
        &serde_json::Value::Null,
        false,
        None,
        if error_code.is_some() { "error" } else { "ok" },
        0,
        result.as_ref(),
        error_code,
    );
}

fn render_strategy_tick_diagnostic(tick: &StrategyTick) {
    eprintln!(
        "[{}:{}] tick {} {} {} {} planned={} executable={} base_lots={}",
        tick.strategy,
        tick.run_id,
        tick.tick,
        format_tick_progress(tick),
        tick.side.as_str(),
        tick.symbol,
        format_optional_usdc(tick.planned_action.slice_notional_usdc),
        format_optional_usdc(tick.planned_action.executable_notional_usdc),
        tick.planned_action
            .executable_base_lots
            .map(|v| v.to_string())
            .unwrap_or_else(|| "n/a".to_string())
    );
    eprintln!(
        "[{}:{}] fill={} price={} tokens={} notional={} fee={}",
        tick.strategy,
        tick.run_id,
        tick.execution.action,
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
            .map(|v| format!("{:.4} USDC", v))
            .unwrap_or_else(|| "n/a".to_string()),
        tick.execution
            .fee
            .map(|v| format!("{:.4} USDC", v))
            .unwrap_or_else(|| "n/a".to_string())
    );
    eprintln!(
        "[{}:{}] progress={:.4} USDC/{:.6} tokens remaining={} exposure={:.2}x",
        tick.strategy,
        tick.run_id,
        tick.progress.cumulative_notional_usdc,
        tick.progress.cumulative_tokens,
        tick.progress.remaining_slices,
        tick.position.exposure_ratio
    );
    if let Some(note) = tick
        .planned_action
        .rounding_note
        .as_ref()
        .or(tick.execution.note.as_ref())
    {
        eprintln!("[{}:{}] note={}", tick.strategy, tick.run_id, note);
    }
    if let Some(stop_reason) = &tick.next_tick.stop_reason {
        eprintln!("[{}:{}] stop={}", tick.strategy, tick.run_id, stop_reason);
    } else if let Some(delay) = tick.next_tick.delay_seconds {
        eprintln!("[{}:{}] next={}s", tick.strategy, tick.run_id, delay);
    }
}

fn format_optional_usdc(value: Option<f64>) -> String {
    value
        .map(|v| format!("{:.4} USDC", v))
        .unwrap_or_else(|| "n/a".to_string())
}

fn format_tick_progress(tick: &StrategyTick) -> String {
    // For grid runs, `slices_total` mirrors the ledger slice count rather than a
    // tick denominator, so "tick N/M" would suggest progress through a fixed
    // schedule that doesn't exist. Print the slice count explicitly instead.
    if tick.strategy == "grid" {
        format!("(slices: {})", tick.slices_total)
    } else if tick.slices_total > 0 && tick.slices_total < u32::MAX {
        format!("of {}", tick.slices_total)
    } else {
        String::new()
    }
}

#[allow(dead_code)]
pub(crate) fn next_incomplete_step(ledger: &StrategyRunLedger) -> Option<u32> {
    ledger
        .slices
        .iter()
        .find(|step| {
            !matches!(
                step.status,
                StrategySliceStatus::Filled
                    | StrategySliceStatus::Replaced
                    | StrategySliceStatus::Cancelled
                    | StrategySliceStatus::Finalized
                    | StrategySliceStatus::Skipped
                    | StrategySliceStatus::Failed
            )
        })
        .map(|step| step.index)
}

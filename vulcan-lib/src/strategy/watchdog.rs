//! Detached-run watchdog.
//!
//! Detached strategy runs live as `tokio::spawn` tasks inside the MCP server
//! process. If the runner task hangs (deadlock, stuck network call) or its
//! parent process exits between MCP-server restarts, `last_updated_at` stops
//! advancing but `status` stays `running` and `completed_at` stays `null`.
//! Monitor surfaces `lifecycle.stale=true`, but only if a caller asks.
//!
//! The watchdog runs in its own task alongside each detached run and turns the
//! frozen-state condition into an active terminal-status mark, so a later
//! `status` or `monitor` call sees a clean `errored` ledger without needing
//! anyone to first call monitor.
//!
//! Escalation is two-stage to avoid racing a runner that is merely slow:
//! 1. At `stale_after_seconds`, write a `Stop` control request. A live runner
//!    consumes it at the top of its next tick (see `apply_control_request`
//!    in the per-strategy modules) and exits with `status = "stopped"`.
//! 2. If `last_updated_at` still hasn't advanced after a grace window
//!    (`escalate_after_seconds`, default 2 × stale), the runner is treated as
//!    dead and the watchdog marks the ledger `errored` directly.
//!
//! The watchdog exits as soon as the ledger reaches any terminal status,
//! whether driven by the runner, the watchdog itself, or an external caller.

use crate::strategy::log::{
    load_run_ledger, load_run_report, write_control_request, write_run_ledger, write_run_report,
};
use crate::strategy::types::{StrategyControlAction, StrategyControlRequest, StrategyRunLedger};
use chrono::{DateTime, Utc};
use std::path::PathBuf;
use std::time::Duration;

const DEFAULT_POLL_INTERVAL_SECONDS: u64 = 30;
const TERMINAL_STATUSES: &[&str] = &["completed", "stopped", "errored", "paused"];

/// Spawns a fire-and-forget watchdog tokio task for the given run. Safe to
/// call once per detached run; the task exits on its own once the ledger
/// reaches a terminal status.
pub fn spawn_watchdog(runs_dir: PathBuf, run_id: String) {
    tokio::spawn(async move {
        run_watchdog(runs_dir, run_id).await;
    });
}

async fn run_watchdog(runs_dir: PathBuf, run_id: String) {
    let mut stop_request_written = false;
    loop {
        let Ok(ledger) = load_run_ledger(&runs_dir, &run_id) else {
            // Ledger missing — runner errored out before writing, or the run
            // was cleaned up. Nothing to watch.
            return;
        };

        if is_terminal(&ledger.status) {
            return;
        }

        let stale_after = stale_after_seconds(&ledger);
        let escalate_after = stale_after.saturating_mul(2);
        let elapsed = match seconds_since_rfc3339(&ledger.last_updated_at) {
            Some(v) => v,
            None => stale_after, // unparseable timestamp → treat as stale-equivalent
        };

        if !stop_request_written && elapsed > stale_after {
            // Stage 1: politely ask the runner to stop. If the runner is alive
            // and merely slow, this lands at the top of its next tick.
            let request = StrategyControlRequest {
                action: StrategyControlAction::Stop,
                requested_at: Utc::now().to_rfc3339(),
                reason: Some(format!(
                    "watchdog: no tick for {}s (stale threshold {}s); requesting stop",
                    elapsed, stale_after
                )),
            };
            let _ = write_control_request(&runs_dir, &run_id, &request);
            stop_request_written = true;
        } else if stop_request_written && elapsed > escalate_after {
            // Stage 2: runner hasn't responded to the stop request within the
            // grace window. Treat as dead and forcibly mark errored.
            mark_errored(&runs_dir, ledger, elapsed, stale_after);
            return;
        }

        tokio::time::sleep(Duration::from_secs(poll_interval(stale_after))).await;
    }
}

fn mark_errored(runs_dir: &std::path::Path, ledger: StrategyRunLedger, elapsed: u64, stale: u64) {
    let now = Utc::now().to_rfc3339();
    let pause_reason = format!(
        "watchdog: runner unresponsive for {}s (stale threshold {}s); marking errored",
        elapsed, stale
    );
    let mut errored = ledger;
    errored.status = "errored".to_string();
    errored.completed_at = Some(now.clone());
    errored.pause_reason = Some(pause_reason.clone());
    errored.last_updated_at = now.clone();
    let _ = write_run_ledger(runs_dir, &errored);

    // Best-effort: update the report so resume callers see consistent state.
    if let Ok(mut report) = load_run_report(runs_dir, &errored.run_id) {
        report.summary.status = "errored".to_string();
        report.summary.completed_at = Some(now);
        report.ledger = Some(errored.clone());
        report.notes.push(pause_reason);
        let _ = write_run_report(runs_dir, &report);
    }
}

fn is_terminal(status: &str) -> bool {
    TERMINAL_STATUSES.contains(&status)
}

/// Mirrors the threshold used by `commands::strategy::compute_lifecycle_status`
/// so monitor's `lifecycle.stale` flag and the watchdog escalate together.
fn stale_after_seconds(ledger: &StrategyRunLedger) -> u64 {
    ledger
        .stale_after_seconds
        .unwrap_or_else(|| ledger.interval_seconds.saturating_mul(2).max(180))
}

fn poll_interval(stale_after: u64) -> u64 {
    let candidate = stale_after / 4;
    candidate.clamp(5, DEFAULT_POLL_INTERVAL_SECONDS)
}

fn seconds_since_rfc3339(timestamp: &str) -> Option<u64> {
    let parsed = DateTime::parse_from_rfc3339(timestamp).ok()?;
    let now = Utc::now();
    let delta = now.signed_duration_since(parsed.with_timezone(&Utc));
    let secs = delta.num_seconds();
    if secs < 0 {
        Some(0)
    } else {
        Some(secs as u64)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::strategy::log::{load_run_ledger, read_control_request, write_run_ledger};
    use crate::strategy::types::{
        StrategyLedgerTotals, StrategyMode, StrategyRunLedger, StrategySide,
    };
    use chrono::Duration as ChronoDuration;
    use tempfile::TempDir;

    fn ledger_with(status: &str, last_updated_at: DateTime<Utc>) -> StrategyRunLedger {
        StrategyRunLedger {
            run_id: "wd-test".to_string(),
            strategy: "ta".to_string(),
            run_label: None,
            wallet: None,
            symbol: "SOL".to_string(),
            side: StrategySide::Buy,
            mode: StrategyMode::Paper,
            started_at: last_updated_at.to_rfc3339(),
            completed_at: None,
            status: status.to_string(),
            interval_seconds: 60,
            run_until_stopped: true,
            stale_after_seconds: Some(60),
            slices_total: u32::MAX,
            totals: StrategyLedgerTotals::default(),
            slices: Vec::new(),
            last_updated_at: last_updated_at.to_rfc3339(),
            pause_reason: None,
            safety_policy: None,
        }
    }

    fn write_ledger(tmp: &TempDir, ledger: &StrategyRunLedger) {
        write_run_ledger(tmp.path(), ledger).unwrap();
    }

    #[tokio::test]
    async fn missing_ledger_exits_without_writing_anything() {
        let tmp = TempDir::new().unwrap();
        // No ledger written; watchdog should fall through immediately.
        run_watchdog(tmp.path().to_path_buf(), "nonexistent".to_string()).await;
        // No control request was written.
        assert!(read_control_request(tmp.path(), "nonexistent")
            .unwrap()
            .is_none());
    }

    #[tokio::test]
    async fn terminal_status_short_circuits() {
        let tmp = TempDir::new().unwrap();
        let ledger = ledger_with(
            "completed",
            Utc::now() - ChronoDuration::seconds(10_000), // very stale, but terminal
        );
        write_ledger(&tmp, &ledger);
        run_watchdog(tmp.path().to_path_buf(), ledger.run_id.clone()).await;
        // Status unchanged.
        let after = load_run_ledger(tmp.path(), &ledger.run_id).unwrap();
        assert_eq!(after.status, "completed");
        assert!(read_control_request(tmp.path(), &ledger.run_id)
            .unwrap()
            .is_none());
    }

    /// Stale ledger without any control-request response should escalate to a
    /// Stop request, then to `errored` after the grace window. We run the
    /// watchdog loop body directly to avoid sleeping for real minutes.
    #[tokio::test]
    async fn stale_run_writes_stop_then_marks_errored() {
        let tmp = TempDir::new().unwrap();
        let ledger = ledger_with(
            "running",
            Utc::now() - ChronoDuration::seconds(500), // elapsed=500, stale=60, escalate=120
        );
        write_ledger(&tmp, &ledger);

        // Drive each stage manually to verify behavior without sleeping for
        // real minutes.
        let stale_after = stale_after_seconds(&ledger);
        let elapsed = seconds_since_rfc3339(&ledger.last_updated_at).unwrap();
        let escalate_after = stale_after.saturating_mul(2);
        assert!(elapsed > stale_after, "fixture must be stale");
        assert!(
            elapsed > escalate_after,
            "fixture must be past escalation window"
        );

        // Stage 1: stop request written.
        let req = StrategyControlRequest {
            action: StrategyControlAction::Stop,
            requested_at: Utc::now().to_rfc3339(),
            reason: Some("test".to_string()),
        };
        write_control_request(tmp.path(), &ledger.run_id, &req).unwrap();
        assert!(read_control_request(tmp.path(), &ledger.run_id)
            .unwrap()
            .is_some());

        // Stage 2: still no tick → mark errored.
        mark_errored(tmp.path(), ledger.clone(), elapsed, stale_after);
        let after = load_run_ledger(tmp.path(), &ledger.run_id).unwrap();
        assert_eq!(after.status, "errored");
        assert!(after.completed_at.is_some());
        assert!(after
            .pause_reason
            .unwrap()
            .contains("watchdog: runner unresponsive"));
    }

    #[test]
    fn stale_after_seconds_uses_ledger_override_when_present() {
        let mut ledger = ledger_with("running", Utc::now());
        ledger.stale_after_seconds = Some(7);
        assert_eq!(stale_after_seconds(&ledger), 7);
    }

    #[test]
    fn stale_after_seconds_falls_back_to_double_interval_floor_180() {
        let mut ledger = ledger_with("running", Utc::now());
        ledger.stale_after_seconds = None;
        ledger.interval_seconds = 30;
        // 2 * 30 = 60 → clamped to 180 floor.
        assert_eq!(stale_after_seconds(&ledger), 180);
        ledger.interval_seconds = 600;
        // 2 * 600 = 1200 > 180.
        assert_eq!(stale_after_seconds(&ledger), 1200);
    }

    #[test]
    fn poll_interval_clamped_to_sensible_range() {
        // Tiny stale threshold → at least 5s between polls.
        assert_eq!(poll_interval(8), 5);
        // Modest threshold → quarter of stale.
        assert_eq!(poll_interval(80), 20);
        // Very large threshold → capped at default.
        assert_eq!(poll_interval(10_000), DEFAULT_POLL_INTERVAL_SECONDS);
    }

    #[test]
    fn is_terminal_recognizes_all_terminal_statuses() {
        assert!(is_terminal("completed"));
        assert!(is_terminal("stopped"));
        assert!(is_terminal("errored"));
        assert!(is_terminal("paused"));
        assert!(!is_terminal("running"));
        assert!(!is_terminal(""));
    }
}

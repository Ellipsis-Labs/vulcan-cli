//! Persistence helpers for strategy run logs.

use crate::error::VulcanError;
use crate::strategy::types::{
    StrategyControlRequest, StrategyRunLedger, StrategyRunReport, StrategyRunSummary, StrategyTick,
};
use std::fs::{self, File, OpenOptions};
use std::io::{BufRead, BufReader, Write};
use std::path::{Path, PathBuf};

pub fn default_strategy_runs_dir(vulcan_dir: &Path) -> PathBuf {
    vulcan_dir.join("strategy-runs")
}

pub fn run_log_path(runs_dir: &Path, run_id: &str) -> PathBuf {
    runs_dir.join(format!("{run_id}.jsonl"))
}

pub fn run_summary_path(runs_dir: &Path, run_id: &str) -> PathBuf {
    runs_dir.join(format!("{run_id}.summary.json"))
}

pub fn run_ledger_path(runs_dir: &Path, run_id: &str) -> PathBuf {
    runs_dir.join(format!("{run_id}.ledger.json"))
}

pub fn run_control_path(runs_dir: &Path, run_id: &str) -> PathBuf {
    runs_dir.join(format!("{run_id}.control.json"))
}

pub fn run_lock_path(runs_dir: &Path, run_id: &str) -> PathBuf {
    runs_dir.join(format!("{run_id}.lock"))
}

pub fn write_tick(runs_dir: &Path, tick: &StrategyTick) -> Result<(), VulcanError> {
    fs::create_dir_all(runs_dir)
        .map_err(|e| VulcanError::io("STRATEGY_RUN_DIR_FAILED", e.to_string()))?;
    let path = run_log_path(runs_dir, &tick.run_id);
    let mut file = OpenOptions::new()
        .create(true)
        .append(true)
        .open(&path)
        .map_err(|e| VulcanError::io("STRATEGY_RUN_LOG_OPEN_FAILED", e.to_string()))?;
    let line = serde_json::to_string(tick)
        .map_err(|e| VulcanError::internal("STRATEGY_TICK_SERIALIZE_FAILED", e.to_string()))?;
    writeln!(file, "{line}")
        .map_err(|e| VulcanError::io("STRATEGY_RUN_LOG_WRITE_FAILED", e.to_string()))?;
    set_private_permissions(&path);
    Ok(())
}

pub fn write_run_report(runs_dir: &Path, report: &StrategyRunReport) -> Result<(), VulcanError> {
    fs::create_dir_all(runs_dir)
        .map_err(|e| VulcanError::io("STRATEGY_RUN_DIR_FAILED", e.to_string()))?;
    let path = run_summary_path(runs_dir, &report.run_id);
    let text = serde_json::to_string_pretty(report)
        .map_err(|e| VulcanError::internal("STRATEGY_REPORT_SERIALIZE_FAILED", e.to_string()))?;
    fs::write(&path, text)
        .map_err(|e| VulcanError::io("STRATEGY_REPORT_WRITE_FAILED", e.to_string()))?;
    set_private_permissions(&path);
    Ok(())
}

pub fn write_run_ledger(runs_dir: &Path, ledger: &StrategyRunLedger) -> Result<(), VulcanError> {
    fs::create_dir_all(runs_dir)
        .map_err(|e| VulcanError::io("STRATEGY_RUN_DIR_FAILED", e.to_string()))?;
    let path = run_ledger_path(runs_dir, &ledger.run_id);
    let text = serde_json::to_string_pretty(ledger)
        .map_err(|e| VulcanError::internal("STRATEGY_LEDGER_SERIALIZE_FAILED", e.to_string()))?;
    fs::write(&path, text)
        .map_err(|e| VulcanError::io("STRATEGY_LEDGER_WRITE_FAILED", e.to_string()))?;
    set_private_permissions(&path);
    Ok(())
}

pub fn write_control_request(
    runs_dir: &Path,
    run_id: &str,
    request: &StrategyControlRequest,
) -> Result<(), VulcanError> {
    fs::create_dir_all(runs_dir)
        .map_err(|e| VulcanError::io("STRATEGY_RUN_DIR_FAILED", e.to_string()))?;
    let path = run_control_path(runs_dir, run_id);
    let text = serde_json::to_string_pretty(request)
        .map_err(|e| VulcanError::internal("STRATEGY_CONTROL_SERIALIZE_FAILED", e.to_string()))?;
    fs::write(&path, text)
        .map_err(|e| VulcanError::io("STRATEGY_CONTROL_WRITE_FAILED", e.to_string()))?;
    set_private_permissions(&path);
    Ok(())
}

pub fn read_control_request(
    runs_dir: &Path,
    run_id: &str,
) -> Result<Option<StrategyControlRequest>, VulcanError> {
    let path = run_control_path(runs_dir, run_id);
    if !path.exists() {
        return Ok(None);
    }
    let text = fs::read_to_string(&path)
        .map_err(|e| VulcanError::io("STRATEGY_CONTROL_READ_FAILED", e.to_string()))?;
    serde_json::from_str(&text)
        .map(Some)
        .map_err(|e| VulcanError::io("STRATEGY_CONTROL_PARSE_FAILED", e.to_string()))
}

pub fn clear_control_request(runs_dir: &Path, run_id: &str) -> Result<(), VulcanError> {
    let path = run_control_path(runs_dir, run_id);
    if path.exists() {
        fs::remove_file(&path)
            .map_err(|e| VulcanError::io("STRATEGY_CONTROL_CLEAR_FAILED", e.to_string()))?;
    }
    Ok(())
}

pub fn load_run_ledger(runs_dir: &Path, run_id: &str) -> Result<StrategyRunLedger, VulcanError> {
    let path = run_ledger_path(runs_dir, run_id);
    let text = fs::read_to_string(&path)
        .map_err(|e| VulcanError::io("STRATEGY_LEDGER_READ_FAILED", e.to_string()))?;
    serde_json::from_str(&text)
        .map_err(|e| VulcanError::io("STRATEGY_LEDGER_PARSE_FAILED", e.to_string()))
}

pub fn load_run_report(runs_dir: &Path, run_id: &str) -> Result<StrategyRunReport, VulcanError> {
    let path = run_summary_path(runs_dir, run_id);
    let text = fs::read_to_string(&path)
        .map_err(|e| VulcanError::io("STRATEGY_REPORT_READ_FAILED", e.to_string()))?;
    serde_json::from_str(&text)
        .map_err(|e| VulcanError::io("STRATEGY_REPORT_PARSE_FAILED", e.to_string()))
}

pub fn read_run_ticks(runs_dir: &Path, run_id: &str) -> Result<Vec<StrategyTick>, VulcanError> {
    let path = run_log_path(runs_dir, run_id);
    let file = File::open(&path)
        .map_err(|e| VulcanError::io("STRATEGY_RUN_LOG_READ_FAILED", e.to_string()))?;
    let reader = BufReader::new(file);
    let mut ticks = Vec::new();
    for line in reader.lines() {
        let line =
            line.map_err(|e| VulcanError::io("STRATEGY_RUN_LOG_READ_FAILED", e.to_string()))?;
        if line.trim().is_empty() {
            continue;
        }
        let tick = serde_json::from_str(&line)
            .map_err(|e| VulcanError::io("STRATEGY_RUN_LOG_PARSE_FAILED", e.to_string()))?;
        ticks.push(tick);
    }
    Ok(ticks)
}

pub fn list_run_summaries(
    runs_dir: &Path,
    limit: usize,
) -> Result<Vec<StrategyRunSummary>, VulcanError> {
    if !runs_dir.exists() {
        return Ok(Vec::new());
    }
    let mut entries = Vec::new();
    for entry in fs::read_dir(runs_dir)
        .map_err(|e| VulcanError::io("STRATEGY_RUN_DIR_READ_FAILED", e.to_string()))?
    {
        let entry =
            entry.map_err(|e| VulcanError::io("STRATEGY_RUN_DIR_READ_FAILED", e.to_string()))?;
        let path = entry.path();
        if path.extension().and_then(|e| e.to_str()) != Some("json") {
            continue;
        }
        if !path
            .file_name()
            .and_then(|name| name.to_str())
            .map(|name| name.ends_with(".summary.json"))
            .unwrap_or(false)
        {
            continue;
        }
        let text = fs::read_to_string(&path)
            .map_err(|e| VulcanError::io("STRATEGY_REPORT_READ_FAILED", e.to_string()))?;
        let report: StrategyRunReport = serde_json::from_str(&text)
            .map_err(|e| VulcanError::io("STRATEGY_REPORT_PARSE_FAILED", e.to_string()))?;
        entries.push(report.summary);
    }
    entries.sort_by(|a, b| b.started_at.cmp(&a.started_at));
    entries.truncate(limit);
    Ok(entries)
}

#[cfg(unix)]
fn set_private_permissions(path: &Path) {
    use std::os::unix::fs::PermissionsExt;
    let _ = fs::set_permissions(path, fs::Permissions::from_mode(0o600));
}

#[cfg(not(unix))]
fn set_private_permissions(_path: &Path) {}

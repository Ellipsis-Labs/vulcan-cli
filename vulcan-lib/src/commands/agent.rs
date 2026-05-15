//! Agent integration commands — install and inspect bundled Agent Skills.

use crate::agent_log::{
    build_position_session_report, default_log_path, read_recent, summarize_records,
    AgentLogRecord, AgentPositionSessionReport, AgentSessionSummary,
};
use crate::cli::agent::{AgentCommand, AgentLogCommand, AgentMcpCommand, AgentScope, AgentTarget};
use crate::commands::portfolio::{execute_snapshot_inner, PortfolioSections};
use crate::commands::status::StatusReport;
use crate::commands::wallet::WalletBalance;
use crate::context::AppContext;
use crate::error::VulcanError;
use crate::output::{render_success, TableRenderable};
use serde::Serialize;
use std::fs;
use std::io::{self, BufRead, Write};
use std::path::{Path, PathBuf};

#[derive(Debug, Clone, Copy)]
struct SkillSpec {
    name: &'static str,
    contents: &'static str,
}

const SKILLS: &[SkillSpec] = &[
    SkillSpec {
        name: "vulcan",
        contents: include_str!("../../../skills/vulcan/SKILL.md"),
    },
    SkillSpec {
        name: "vulcan-risk-management",
        contents: include_str!("../../../skills/vulcan-risk-management/SKILL.md"),
    },
    SkillSpec {
        name: "vulcan-error-recovery",
        contents: include_str!("../../../skills/vulcan-error-recovery/SKILL.md"),
    },
    SkillSpec {
        name: "vulcan-trade-execution",
        contents: include_str!("../../../skills/vulcan-trade-execution/SKILL.md"),
    },
    SkillSpec {
        name: "vulcan-lot-size-calculator",
        contents: include_str!("../../../skills/vulcan-lot-size-calculator/SKILL.md"),
    },
    SkillSpec {
        name: "vulcan-tpsl-management",
        contents: include_str!("../../../skills/vulcan-tpsl-management/SKILL.md"),
    },
    SkillSpec {
        name: "vulcan-twap-execution",
        contents: include_str!("../../../skills/vulcan-twap-execution/SKILL.md"),
    },
    SkillSpec {
        name: "vulcan-grid-trading",
        contents: include_str!("../../../skills/vulcan-grid-trading/SKILL.md"),
    },
    SkillSpec {
        name: "vulcan-market-intel",
        contents: include_str!("../../../skills/vulcan-market-intel/SKILL.md"),
    },
    SkillSpec {
        name: "vulcan-portfolio-intel",
        contents: include_str!("../../../skills/vulcan-portfolio-intel/SKILL.md"),
    },
    SkillSpec {
        name: "vulcan-margin-operations",
        contents: include_str!("../../../skills/vulcan-margin-operations/SKILL.md"),
    },
    SkillSpec {
        name: "vulcan-onboarding",
        contents: include_str!("../../../skills/vulcan-onboarding/SKILL.md"),
    },
    SkillSpec {
        name: "vulcan-position-management",
        contents: include_str!("../../../skills/vulcan-position-management/SKILL.md"),
    },
    SkillSpec {
        name: "vulcan-technical-analysis",
        contents: include_str!("../../../skills/vulcan-technical-analysis/SKILL.md"),
    },
    SkillSpec {
        name: "vulcan-ta-strategy",
        contents: include_str!("../../../skills/vulcan-ta-strategy/SKILL.md"),
    },
    SkillSpec {
        name: "vulcan-quickstart",
        contents: include_str!("../../../skills/vulcan-quickstart/SKILL.md"),
    },
    SkillSpec {
        name: "vulcan-execution-modes",
        contents: include_str!("../../../skills/vulcan-execution-modes/SKILL.md"),
    },
];

#[derive(Debug, Serialize)]
pub struct AgentInstallResult {
    pub target: String,
    pub scope: String,
    pub directory: String,
    pub dry_run: bool,
    pub force: bool,
    pub installed: usize,
    pub unchanged: usize,
    pub skipped: usize,
    pub files: Vec<SkillInstallStatus>,
    pub mcp_config: String,
    pub next_steps: Vec<String>,
}

#[derive(Debug, Serialize)]
pub struct SkillInstallStatus {
    pub skill: String,
    pub path: String,
    pub status: String,
}

#[derive(Debug, Serialize)]
pub struct AgentDoctorResult {
    pub target: String,
    pub scope: String,
    pub directory: String,
    pub installed: usize,
    pub missing: usize,
    pub skills: Vec<SkillDoctorStatus>,
}

#[derive(Debug, Serialize)]
pub struct AgentHealthResult {
    pub status: StatusReport,
    pub connections: AgentConnectionHealth,
    pub agent_skills: Vec<AgentDoctorResult>,
    pub mcp: Vec<AgentMcpDoctorResult>,
    pub live_agent_ready: Vec<AgentLiveAgentReadiness>,
    pub api_auth: crate::commands::auth::ApiAuthStatus,
    pub api_auth_auto_login: crate::commands::auth::ApiAuthAutoLoginResult,
    pub wallets: AgentWalletHealth,
    pub live_readiness: AgentLiveReadiness,
    pub paper: AgentPaperHealth,
    pub next_steps: Vec<String>,
}

#[derive(Debug, Serialize)]
pub struct AgentConnectionHealth {
    pub phoenix_api_ok: bool,
    pub phoenix_api_url: String,
    pub phoenix_markets: Option<usize>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub phoenix_error: Option<String>,
    pub solana_rpc_ok: bool,
    pub solana_rpc_url: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub solana_version: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub solana_error: Option<String>,
}

#[derive(Debug, Serialize)]
pub struct AgentWalletHealth {
    pub count: usize,
    pub default_wallet: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub default_wallet_address: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub default_balance: Option<WalletBalance>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub balance_error: Option<String>,
    pub wallet_funds_label: String,
}

#[derive(Debug, Serialize)]
pub struct AgentLiveReadiness {
    pub has_default_wallet: bool,
    pub wallet_has_sol: bool,
    pub wallet_has_usdc: bool,
    pub trader_registered: bool,
    pub invite_code_required: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub deposited_collateral_usdc: Option<String>,
    pub can_attempt_live_trading: bool,
    pub note: String,
}

#[derive(Debug, Serialize)]
pub struct AgentPaperHealth {
    pub initialized: bool,
    pub path: String,
}

#[derive(Debug, Serialize)]
pub struct SkillDoctorStatus {
    pub skill: String,
    pub path: String,
    pub installed: bool,
    pub current: bool,
}

#[derive(Debug, Serialize)]
pub struct AgentLogShowResult {
    pub path: String,
    pub session: String,
    pub limit: usize,
    pub records: Vec<AgentLogRecord>,
}

#[derive(Debug, Serialize)]
pub struct AgentLogSummaryResult {
    pub path: String,
    pub session: String,
    pub limit: usize,
    pub summary: AgentSessionSummary,
}

#[derive(Debug, Serialize)]
pub struct AgentLogReportResult {
    pub report: AgentPositionSessionReport,
}

#[derive(Debug, Serialize)]
pub struct AgentMcpConfigResult {
    pub target: String,
    pub dangerous: bool,
    pub groups: Option<Vec<String>>,
    pub config: serde_json::Value,
    pub warnings: Vec<String>,
    pub manual_install_command: Option<String>,
    pub restart_required: bool,
    pub next_steps: Vec<String>,
}

#[derive(Debug, Serialize, Clone)]
pub struct AgentMcpDoctorResult {
    pub target: String,
    pub scope: String,
    pub path: String,
    /// Dotted JSON path inside `path` where the vulcan entry lives.
    /// Claude/user nests under `projects."<cwd>".mcpServers.vulcan`;
    /// other targets use top-level `mcpServers.vulcan`. Disambiguates the
    /// `scope` label, which refers to install scope rather than JSON nesting.
    pub storage_pointer: String,
    pub exists: bool,
    pub configured: bool,
    pub dangerous_enabled: bool,
    pub password_env_present: bool,
    pub wallet_env_name: Option<String>,
    pub warnings: Vec<String>,
    pub manual_install_command: String,
    pub restart_required: bool,
    pub restart_instructions: String,
}

#[derive(Debug, Serialize)]
pub struct AgentMcpInstallResult {
    pub target: String,
    pub scope: String,
    pub path: String,
    pub dry_run: bool,
    pub dangerous: bool,
    pub installed: bool,
    pub config: serde_json::Value,
    pub wallet_name: Option<String>,
    pub wallet_address: Option<String>,
    pub password_env_written: bool,
    pub placeholder_password: bool,
    pub manual_install_command: String,
    pub restart_required: bool,
    pub restart_instructions: String,
    pub warnings: Vec<String>,
    pub next_steps: Vec<String>,
}

#[derive(Debug, Serialize)]
pub struct AgentMcpSetWalletResult {
    pub target: String,
    pub scope: String,
    pub path: String,
    pub dry_run: bool,
    pub wallet_name: String,
    pub wallet_address: String,
    pub previous_wallet_name: Option<String>,
    pub updated: bool,
    pub password_env_written: bool,
    pub dangerous_newly_enabled: bool,
    pub config: serde_json::Value,
    pub restart_required: bool,
    pub restart_instructions: String,
    pub warnings: Vec<String>,
}

#[derive(Debug, Serialize)]
pub struct AgentLiveAgentReadiness {
    pub target: String,
    pub target_source: String,
    pub scope: String,
    pub mcp_configured: bool,
    pub dangerous_enabled: bool,
    pub password_env_present: bool,
    pub wallet_env_name: Option<String>,
    pub cli_env_unlock_present: bool,
    pub recommended_execution_path: String,
    pub blocking_reason: Option<String>,
    pub resolution_summary: String,
    pub install_command: String,
    pub doctor_command: String,
    pub restart_instructions: String,
    pub config_path: String,
}

impl TableRenderable for AgentInstallResult {
    fn render_table(&self) {
        println!("Vulcan Agent Skills");
        println!("─────────────────────────────────────────");
        println!("  Target:    {}", self.target);
        println!("  Scope:     {}", self.scope);
        println!("  Directory: {}", self.directory);
        if self.dry_run {
            println!("  Mode:      dry run");
        }
        println!();
        for file in &self.files {
            println!("  {:<12} {} -> {}", file.status, file.skill, file.path);
        }
        println!();
        if self.dry_run {
            println!(
                "  Summary: {} planned, {} unchanged, {} skipped",
                self.installed, self.unchanged, self.skipped
            );
        } else {
            println!(
                "  Summary: {} installed, {} unchanged, {} skipped",
                self.installed, self.unchanged, self.skipped
            );
        }
        println!();
        println!("MCP config snippet:");
        println!("{}", self.mcp_config);
        println!("Next steps:");
        for step in &self.next_steps {
            println!("  - {step}");
        }
    }
}

impl TableRenderable for AgentDoctorResult {
    fn render_table(&self) {
        println!("Vulcan Agent Skills Doctor");
        println!("─────────────────────────────────────────");
        println!("  Target:    {}", self.target);
        println!("  Scope:     {}", self.scope);
        println!("  Directory: {}", self.directory);
        println!();
        for skill in &self.skills {
            let status = if skill.current {
                "current"
            } else if skill.installed {
                "different"
            } else {
                "missing"
            };
            println!("  {:<10} {} -> {}", status, skill.skill, skill.path);
        }
        println!();
        println!(
            "  Summary: {} installed, {} missing",
            self.installed, self.missing
        );
    }
}

impl TableRenderable for AgentMcpConfigResult {
    fn render_table(&self) {
        println!("Vulcan MCP Config");
        println!("─────────────────────────────────────────");
        println!("  Target:    {}", self.target);
        println!("  Dangerous: {}", self.dangerous);
        println!();
        println!("{}", serde_json::to_string_pretty(&self.config).unwrap());
        if !self.warnings.is_empty() {
            println!();
            println!("Warnings:");
            for warning in &self.warnings {
                println!("  - {warning}");
            }
        }
        if let Some(command) = &self.manual_install_command {
            println!();
            println!("Manual install:");
            println!("  {command}");
        }
        println!("Next steps:");
        for step in &self.next_steps {
            println!("  - {step}");
        }
    }
}

impl TableRenderable for AgentMcpDoctorResult {
    fn render_table(&self) {
        println!("Vulcan MCP Doctor");
        println!("─────────────────────────────────────────");
        println!("  Target:      {}", self.target);
        println!("  Scope:       {}", self.scope);
        println!("  Path:        {}", self.path);
        println!("  Storage:     {}", self.storage_pointer);
        println!("  Exists:      {}", self.exists);
        println!("  Configured:  {}", self.configured);
        println!("  Dangerous:   {}", self.dangerous_enabled);
        println!("  Password env: {}", self.password_env_present);
        if let Some(wallet_name) = &self.wallet_env_name {
            println!("  Wallet env:  {wallet_name}");
        }
        for warning in &self.warnings {
            println!("  Warning:     {warning}");
        }
        println!("  Install:     {}", self.manual_install_command);
        println!("  Restart:     {}", self.restart_instructions);
    }
}

impl TableRenderable for AgentMcpInstallResult {
    fn render_table(&self) {
        println!("Vulcan MCP Install");
        println!("─────────────────────────────────────────");
        println!("  Target:    {}", self.target);
        println!("  Scope:     {}", self.scope);
        println!("  Path:      {}", self.path);
        println!(
            "  Mode:      {}",
            if self.dry_run { "dry run" } else { "write" }
        );
        println!("  Installed: {}", self.installed);
        if let Some(wallet_name) = &self.wallet_name {
            println!("  Wallet:    {wallet_name}");
        }
        if let Some(wallet_address) = &self.wallet_address {
            println!("  Address:   {wallet_address}");
        }
        println!("  Password env written: {}", self.password_env_written);
        println!("  Placeholder password: {}", self.placeholder_password);
        if !self.warnings.is_empty() {
            println!("Warnings:");
            for warning in &self.warnings {
                println!("  - {warning}");
            }
        }
        println!("Next steps:");
        for step in &self.next_steps {
            println!("  - {step}");
        }
    }
}

impl TableRenderable for AgentMcpSetWalletResult {
    fn render_table(&self) {
        println!("Vulcan MCP Wallet");
        println!("─────────────────────────────────────────");
        println!("  Target:    {}", self.target);
        println!("  Scope:     {}", self.scope);
        println!("  Path:      {}", self.path);
        println!(
            "  Mode:      {}",
            if self.dry_run { "dry run" } else { "write" }
        );
        if let Some(previous) = &self.previous_wallet_name {
            println!("  Previous:  {previous}");
        }
        println!("  Wallet:    {}", self.wallet_name);
        println!("  Address:   {}", self.wallet_address);
        println!("  Updated:   {}", self.updated);
        println!("  Password env written: {}", self.password_env_written);
        if self.dangerous_newly_enabled {
            println!("  --allow-dangerous: newly added");
        }
        for warning in &self.warnings {
            println!("  Warning:   {warning}");
        }
        println!("  Restart:   {}", self.restart_instructions);
    }
}

impl TableRenderable for AgentLiveAgentReadiness {
    fn render_table(&self) {
        println!("Vulcan Live Agent Readiness");
        println!("─────────────────────────────────────────");
        println!("  Target:      {} ({})", self.target, self.target_source);
        println!("  Scope:       {}", self.scope);
        println!("  Path:        {}", self.config_path);
        println!(
            "  MCP ready:   {}",
            self.mcp_configured && self.dangerous_enabled && self.password_env_present
        );
        if let Some(wallet_name) = &self.wallet_env_name {
            println!("  Wallet:      {wallet_name}");
        }
        println!("  CLI env:     {}", self.cli_env_unlock_present);
        println!("  Route:       {}", self.recommended_execution_path);
        if let Some(reason) = &self.blocking_reason {
            println!("  Blocked:     {reason}");
        }
        println!();
        println!("{}", self.resolution_summary);
        println!("  Doctor:  {}", self.doctor_command);
        println!("  Install: {}", self.install_command);
        println!("  Restart: {}", self.restart_instructions);
    }
}

impl TableRenderable for AgentHealthResult {
    fn render_table(&self) {
        println!("Vulcan Agent Health");
        println!("─────────────────────────────────────────");
        println!(
            "  API:        {}",
            if self.connections.phoenix_api_ok {
                "OK"
            } else {
                "Needs attention"
            }
        );
        println!(
            "  RPC:        {}",
            if self.connections.solana_rpc_ok {
                "OK"
            } else {
                "Needs attention"
            }
        );
        println!(
            "  Wallets:    {} found{}",
            self.wallets.count,
            self.wallets
                .default_wallet
                .as_ref()
                .map(|name| format!(", default={name}"))
                .unwrap_or_default()
        );
        if let Some(balance) = &self.wallets.default_balance {
            println!(
                "  Wallet funds: {:.6} USDC, {:.9} SOL",
                balance.usdc, balance.sol
            );
        }
        if let Some(error) = &self.wallets.balance_error {
            println!("  Balance:    {}", error);
        }
        println!(
            "  Trader:     {}{}",
            if self.status.trader.registered {
                "registered"
            } else {
                "not registered"
            },
            self.live_readiness
                .deposited_collateral_usdc
                .as_ref()
                .map(|c| format!(", deposited collateral={c} USDC"))
                .unwrap_or_default()
        );
        if self.live_readiness.invite_code_required {
            println!("  Invite:     required for registration");
        }
        println!(
            "  Skills:     {} targets checked, {} missing files",
            self.agent_skills.len(),
            self.agent_skills
                .iter()
                .map(|doctor| doctor.missing)
                .sum::<usize>()
        );
        for doctor in &self.agent_skills {
            println!(
                "             {}: {}/{} installed",
                doctor.target,
                doctor.installed,
                doctor.skills.len()
            );
        }
        println!(
            "  Paper:      {} ({})",
            if self.paper.initialized {
                "initialized"
            } else {
                "not initialized"
            },
            self.paper.path
        );
        println!();
        println!("Recommended next steps:");
        for step in &self.next_steps {
            println!("  - {step}");
        }
    }
}

impl TableRenderable for AgentLogShowResult {
    fn render_table(&self) {
        println!("Vulcan Agent Action Log");
        println!("─────────────────────────────────────────");
        println!("  Path:    {}", self.path);
        println!("  Session: {}", self.session);
        println!("  Records: {}", self.records.len());
        println!();
        for record in &self.records {
            println!(
                "  {} {:<28} {:<7} {}",
                record.timestamp, record.name, record.outcome, record.correlation_id
            );
        }
    }
}

impl TableRenderable for AgentLogSummaryResult {
    fn render_table(&self) {
        println!("Vulcan Agent Session Summary");
        println!("─────────────────────────────────────────");
        println!("  Path:     {}", self.path);
        println!("  Session:  {}", self.session);
        println!("  Records:  {}", self.summary.total_records);
        println!("  Sessions: {}", self.summary.sessions.len());
        println!("  Txs:      {}", self.summary.tx_signatures.len());
        println!("  Errors:   {}", self.summary.errors.len());
        if let Some(margin) = &self.summary.latest_margin {
            println!();
            println!("Latest margin:");
            if let Some(v) = &margin.risk_state {
                println!("  Risk state: {}", v);
            }
            if let Some(v) = &margin.unrealized_pnl {
                println!("  Unrealized PnL: {}", v);
            }
        }
        if !self.summary.latest_positions.is_empty() {
            println!();
            println!("Latest positions:");
            for p in &self.summary.latest_positions {
                println!("  {} {} size {}", p.symbol, p.side, p.size);
            }
        }
    }
}

impl TableRenderable for AgentLogReportResult {
    fn render_table(&self) {
        let report = &self.report;
        println!("Vulcan Position / Session Report");
        println!("─────────────────────────────────────────");
        println!("  Generated: {}", report.generated_at);
        println!("  Session:   {}", report.session);
        println!("  Records:   {}", report.session_summary.total_records);
        println!("  Txs:       {}", report.metrics.tx_count);
        println!("  Paper fills: {}", report.metrics.paper_fill_count);
        println!("  Errors:    {}", report.metrics.error_count);
        if let Some(margin) = &report.live.margin {
            println!();
            println!("Live trader:");
            if let Some(v) = &margin.risk_state {
                println!("  Risk state: {}", v);
            }
            if let Some(v) = &margin.portfolio_value {
                println!("  Portfolio value: {}", v);
            }
            if let Some(v) = &margin.unrealized_pnl {
                println!("  Unrealized PnL: {}", v);
            }
        }
        if let Some(error) = &report.live.error {
            println!("  Live state error: {}", error);
        }
        println!("  Open orders: {}", report.live.open_orders);
        println!(
            "  TP/SL orders: {} TP, {} SL",
            report.live.take_profit_orders, report.live.stop_loss_orders
        );
        println!();
        println!("Session metrics:");
        println!(
            "  Trades: {} ({} market, {} limit)",
            report.metrics.actions.successful_trades,
            report.metrics.actions.market_trades,
            report.metrics.actions.limit_trades
        );
        println!(
            "  Modes: {} live, {} dry-run, {} paper",
            report.metrics.modes.live_actions,
            report.metrics.modes.dry_run_actions,
            report.metrics.modes.paper_actions
        );
        println!(
            "  Cancels/closes/reductions: {}/{}/{}",
            report.metrics.actions.cancels,
            report.metrics.actions.position_closes,
            report.metrics.actions.position_reductions
        );
        match report.metrics.open_position_win_rate.rate {
            Some(rate) => println!(
                "  Open-position win rate: {:.1}% ({}/{})",
                rate * 100.0,
                report.metrics.open_position_win_rate.winners,
                report.metrics.open_position_win_rate.sample_size
            ),
            None => println!("  Open-position win rate: unavailable"),
        }
        if !report.live.positions.is_empty() {
            println!();
            println!("Live positions:");
            for p in &report.live.positions {
                println!(
                    "  {} {} size {} uPnL {}",
                    p.symbol,
                    p.side,
                    p.size,
                    p.unrealized_pnl.as_deref().unwrap_or("n/a")
                );
            }
        }
        if !report.inferred_fills.is_empty() {
            println!();
            println!("Inferred fills:");
            for fill in report.inferred_fills.iter().rev().take(10).rev() {
                println!(
                    "  {} {} {} txs={}",
                    fill.timestamp,
                    fill.symbol.as_deref().unwrap_or("-"),
                    fill.side.as_deref().unwrap_or("-"),
                    fill.tx_signatures.len()
                );
            }
        }
    }
}

pub async fn execute(ctx: &AppContext, cmd: AgentCommand) -> Result<(), VulcanError> {
    match cmd {
        AgentCommand::Install {
            target,
            scope,
            dir,
            force,
        } => {
            let result = install_skills(target, scope, dir, force, ctx.dry_run)?;
            render_success(ctx.output_format, &result, serde_json::Value::Null);
        }
        AgentCommand::Doctor { target, scope, dir } => {
            let result = doctor(target, scope, dir)?;
            render_success(ctx.output_format, &result, serde_json::Value::Null);
        }
        AgentCommand::Health { target, scope, dir } => {
            let result = health(ctx, target, scope, dir).await?;
            render_success(ctx.output_format, &result, serde_json::Value::Null);
        }
        AgentCommand::LiveReady { target, scope } => {
            let target = target.unwrap_or_else(infer_agent_target);
            let result = live_agent_readiness(target, scope, None)?;
            render_success(ctx.output_format, &result, serde_json::Value::Null);
        }
        AgentCommand::Mcp(cmd) => match cmd {
            AgentMcpCommand::PrintConfig(args) => {
                let result = mcp_print_config(args.target, args.dangerous, args.groups)?;
                render_success(ctx.output_format, &result, serde_json::Value::Null);
            }
            AgentMcpCommand::Doctor(args) => {
                let result = mcp_doctor(args.target, args.scope, args.path)?;
                render_success(ctx.output_format, &result, serde_json::Value::Null);
            }
            AgentMcpCommand::Install(args) => {
                let result = if args.repair {
                    mcp_repair(args.target, args.scope, args.path)?
                } else {
                    mcp_install(
                        Some(ctx),
                        args.target,
                        args.scope,
                        args.path,
                        args.dangerous,
                        args.groups,
                        args.force,
                        ctx.dry_run,
                    )?
                };
                render_success(ctx.output_format, &result, serde_json::Value::Null);
            }
            AgentMcpCommand::SetWallet(args) => {
                let result = mcp_set_wallet(
                    ctx,
                    args.target,
                    args.scope,
                    args.path,
                    &args.wallet,
                    ctx.dry_run,
                )?;
                render_success(ctx.output_format, &result, serde_json::Value::Null);
            }
            AgentMcpCommand::Diagnose(args) => {
                let result = mcp_diagnose(args.target, args.scope, args.path).await?;
                render_success(ctx.output_format, &result, serde_json::Value::Null);
            }
        },
        AgentCommand::Log(cmd) => match cmd {
            AgentLogCommand::Show { limit, session } => {
                let result = log_show(ctx, limit, session)?;
                render_success(ctx.output_format, &result, serde_json::Value::Null);
            }
            AgentLogCommand::Summary { limit, session } => {
                let result = log_summary(ctx, limit, session)?;
                render_success(ctx.output_format, &result, serde_json::Value::Null);
            }
            AgentLogCommand::Report { limit, session } => {
                let result = log_report(ctx, limit, session).await?;
                render_success(ctx.output_format, &result, serde_json::Value::Null);
            }
        },
    }
    Ok(())
}

pub async fn health(
    ctx: &AppContext,
    target: Option<AgentTarget>,
    scope: AgentScope,
    dir: Option<PathBuf>,
) -> Result<AgentHealthResult, VulcanError> {
    let status = crate::commands::status::execute_inner(ctx).await?;
    let connections = AgentConnectionHealth {
        phoenix_api_ok: status.api.ok,
        phoenix_api_url: status.config.api_url.clone(),
        phoenix_markets: status.api.markets,
        phoenix_error: status.api.error.clone(),
        solana_rpc_ok: status.rpc.ok,
        solana_rpc_url: status.config.rpc_url.clone(),
        solana_version: status.rpc.version.clone(),
        solana_error: status.rpc.error.clone(),
    };
    let use_dir_override = target.is_some();
    let agent_skills = health_skill_targets(target)
        .into_iter()
        .map(|target| {
            let dir = if use_dir_override { dir.clone() } else { None };
            doctor(target, scope, dir)
        })
        .collect::<Result<Vec<_>, _>>()?;
    let mcp = health_skill_targets(target)
        .into_iter()
        .map(|target| mcp_doctor(target, scope, None))
        .collect::<Result<Vec<_>, _>>()?;
    let live_agent_ready = health_skill_targets(target)
        .into_iter()
        .map(|target| live_agent_readiness(target, scope, None))
        .collect::<Result<Vec<_>, _>>()?;
    let wallet_names = ctx
        .wallet_store
        .list()
        .map_err(|e| VulcanError::config("WALLET_LIST_FAILED", e.to_string()))?;
    let default_wallet = ctx
        .wallet_store
        .default_wallet()
        .map_err(|e| VulcanError::config("WALLET_DEFAULT_FAILED", e.to_string()))?;
    let (default_balance, balance_error) = match default_wallet.as_deref() {
        Some(_) => match crate::commands::wallet::execute_balance_inner(ctx, None) {
            Ok(balance) => (Some(balance), None),
            Err(err) => (None, Some(format!("{}: {}", err.code, err.message))),
        },
        None => (None, None),
    };
    let default_wallet_address = default_wallet
        .as_deref()
        .and_then(|name| ctx.wallet_store.load(name).ok())
        .map(|wallet| wallet.public_key);
    let paper_path = crate::paper::default_state_path(&ctx.vulcan_dir);
    let paper = AgentPaperHealth {
        initialized: paper_path.exists(),
        path: paper_path.to_string_lossy().to_string(),
    };
    let wallets = AgentWalletHealth {
        count: wallet_names.len(),
        default_wallet,
        default_wallet_address,
        default_balance,
        balance_error,
        wallet_funds_label:
            "Wallet SOL/USDC funds before deposit; deposited trader collateral is separate."
                .to_string(),
    };
    let live_readiness = live_readiness_from_health(&status, &wallets);
    let api_auth_auto_login = crate::commands::auth::auto_login_if_possible(ctx).await;
    let api_auth = api_auth_auto_login.status.clone();
    let next_steps = health_next_steps(
        &status,
        &agent_skills,
        &mcp,
        &wallets,
        &paper,
        &live_readiness,
        &api_auth,
    );
    Ok(AgentHealthResult {
        status,
        connections,
        agent_skills,
        mcp,
        live_agent_ready,
        api_auth,
        api_auth_auto_login,
        wallets,
        live_readiness,
        paper,
        next_steps,
    })
}

fn live_readiness_from_health(
    status: &StatusReport,
    wallets: &AgentWalletHealth,
) -> AgentLiveReadiness {
    let wallet_has_sol = wallets
        .default_balance
        .as_ref()
        .map(|balance| balance.sol > 0.0)
        .unwrap_or(false);
    let wallet_has_usdc = wallets
        .default_balance
        .as_ref()
        .map(|balance| balance.usdc > 0.0)
        .unwrap_or(false);
    let has_default_wallet = wallets.default_wallet.is_some();
    let trader_registered = status.trader.registered;
    AgentLiveReadiness {
        has_default_wallet,
        wallet_has_sol,
        wallet_has_usdc,
        trader_registered,
        invite_code_required: has_default_wallet && !trader_registered,
        deposited_collateral_usdc: status.trader.collateral.clone(),
        can_attempt_live_trading: status.api.ok
            && status.rpc.ok
            && has_default_wallet
            && trader_registered,
        note: "Wallet funds are SOL/USDC held at the wallet address. Deposited collateral is USDC already moved into the Phoenix trader account.".to_string(),
    }
}

fn health_next_steps(
    status: &StatusReport,
    agent_skills: &[AgentDoctorResult],
    mcp: &[AgentMcpDoctorResult],
    wallets: &AgentWalletHealth,
    paper: &AgentPaperHealth,
    live_readiness: &AgentLiveReadiness,
    api_auth: &crate::commands::auth::ApiAuthStatus,
) -> Vec<String> {
    let mut steps = Vec::new();
    if !paper.initialized {
        steps.push("Try paper trading first: vulcan paper init --balance 10000".to_string());
    } else {
        steps.push("Continue paper trading: vulcan paper status".to_string());
    }
    let missing_targets: Vec<&AgentDoctorResult> = agent_skills
        .iter()
        .filter(|doctor| doctor.missing > 0)
        .collect();
    if !missing_targets.is_empty() {
        steps.push(format!(
            "Install or refresh missing agent skills: {}",
            missing_targets
                .iter()
                .map(|doctor| format!(
                    "vulcan agent install --target {} --scope {}",
                    doctor.target, doctor.scope
                ))
                .collect::<Vec<_>>()
                .join("; ")
        ));
    }
    if wallets.count == 0 {
        steps.push("Create a wallet when ready for live setup: vulcan setup".to_string());
    } else if wallets.default_wallet.is_none() {
        steps.push("Set a default wallet: vulcan wallet set-default <NAME>".to_string());
    }
    if live_readiness.invite_code_required {
        steps.push("Registration requires an invite/access code: vulcan account register --access-code <CODE> --yes".to_string());
    }
    if status.trader.registered {
        steps.push("Wallet funds and deposited collateral are separate. Check deposited collateral and positions: vulcan portfolio -o json".to_string());
    }
    let live_mcp_ready = mcp
        .iter()
        .any(|mcp| mcp.configured && mcp.dangerous_enabled && mcp.password_env_present);
    if live_readiness.can_attempt_live_trading && !live_mcp_ready {
        let install_commands = mcp
            .iter()
            .map(|mcp| mcp.manual_install_command.clone())
            .collect::<Vec<_>>()
            .join("; ");
        steps.push(format!(
            "Set up dangerous MCP for agent-driven live trading: {}",
            install_commands
        ));
    }
    if api_auth.reauth_required {
        if live_mcp_ready {
            steps.push("Log in to the Phoenix API from MCP with vulcan_auth_login.".to_string());
        } else if wallets.default_wallet.is_some() {
            steps.push(
                "Log in to the Phoenix API from an interactive terminal: vulcan auth login"
                    .to_string(),
            );
        }
    }
    if !status.api.ok {
        steps.push("Check Phoenix API connectivity or override --api-url".to_string());
    }
    if !status.rpc.ok {
        steps.push("Check Solana RPC connectivity or override --rpc-url".to_string());
    }
    steps
}

fn health_skill_targets(target: Option<AgentTarget>) -> Vec<AgentTarget> {
    match target {
        Some(target) => vec![target],
        None => vec![
            AgentTarget::Cursor,
            AgentTarget::Claude,
            AgentTarget::Codex,
            AgentTarget::Agentskills,
        ],
    }
}

pub fn log_show(
    ctx: &AppContext,
    limit: usize,
    session: Option<String>,
) -> Result<AgentLogShowResult, VulcanError> {
    let (records, session_label, path) = load_filtered_records(ctx, limit, session)?;
    Ok(AgentLogShowResult {
        path: path.to_string_lossy().to_string(),
        session: session_label,
        limit,
        records,
    })
}

pub fn log_summary(
    ctx: &AppContext,
    limit: usize,
    session: Option<String>,
) -> Result<AgentLogSummaryResult, VulcanError> {
    let (records, session_label, path) = load_filtered_records(ctx, limit, session)?;
    let summary = summarize_records(&records);
    Ok(AgentLogSummaryResult {
        path: path.to_string_lossy().to_string(),
        session: session_label,
        limit,
        summary,
    })
}

pub async fn log_report(
    ctx: &AppContext,
    limit: usize,
    session: Option<String>,
) -> Result<AgentLogReportResult, VulcanError> {
    let (records, session_label, path) = load_filtered_records(ctx, limit, session)?;
    let live = execute_snapshot_inner(ctx, PortfolioSections::all())
        .await
        .and_then(|snapshot| {
            serde_json::to_value(snapshot)
                .map_err(|e| VulcanError::internal("AGENT_REPORT_SERIALIZE_FAILED", e.to_string()))
        });
    let (live_value, live_error) = match live {
        Ok(value) => (Some(value), None),
        Err(err) => (None, Some(format!("{}: {}", err.code, err.message))),
    };
    let report = build_position_session_report(
        &records,
        live_value.as_ref(),
        live_error,
        &path,
        session_label,
        limit,
    );
    Ok(AgentLogReportResult { report })
}

fn load_filtered_records(
    ctx: &AppContext,
    limit: usize,
    session: Option<String>,
) -> Result<(Vec<AgentLogRecord>, String, PathBuf), VulcanError> {
    let path = ctx
        .agent_log
        .as_ref()
        .map(|log| log.path().to_path_buf())
        .unwrap_or_else(|| default_log_path(&ctx.vulcan_dir));
    let configured_limit = ctx.config.agent_log.max_summary_entries.max(limit).max(1);
    let read_limit = if session.is_some() {
        configured_limit.saturating_mul(10)
    } else {
        configured_limit
    };
    let mut records = read_recent(&path, read_limit)?;
    let session_label = match session {
        Some(s) if s == "current" => {
            records.retain(|r| r.session_id == ctx.session_id);
            "current".to_string()
        }
        Some(s) => {
            records.retain(|r| r.session_id == s);
            s
        }
        None => "all".to_string(),
    };
    let keep = limit.min(records.len());
    let records = records.split_off(records.len().saturating_sub(keep));
    Ok((records, session_label, path))
}

pub fn mcp_print_config(
    target: AgentTarget,
    dangerous: bool,
    groups: Option<Vec<String>>,
) -> Result<AgentMcpConfigResult, VulcanError> {
    let scope = AgentScope::User;
    Ok(AgentMcpConfigResult {
        target: target_label(target).to_string(),
        dangerous,
        groups: groups.clone(),
        config: mcp_config_value(dangerous, groups),
        warnings: mcp_warnings(dangerous),
        manual_install_command: dangerous.then(|| mcp_install_command(target, scope, dangerous)),
        restart_required: dangerous,
        next_steps: mcp_next_steps(target, dangerous),
    })
}

/// Non-interactive command-path repair. Reads the existing vulcan entry,
/// replaces `command` (now absolute via `current_exe()`) and `args` (rebuilt
/// from the existing config's dangerous + groups state), preserves `env`
/// untouched. Used to migrate older bare-name installs without re-entering
/// the wallet password.
pub fn mcp_repair(
    target: AgentTarget,
    scope: AgentScope,
    path: Option<PathBuf>,
) -> Result<AgentMcpInstallResult, VulcanError> {
    let path = path.unwrap_or(mcp_config_path(target, scope)?);
    let pointer = mcp_server_pointer(target, scope, None)?;
    let content = fs::read_to_string(&path).map_err(|e| {
        VulcanError::io(
            "MCP_CONFIG_READ_FAILED",
            format!("could not read {}: {}", path.display(), e),
        )
    })?;
    let mut value: serde_json::Value = serde_json::from_str(&content).map_err(|e| {
        VulcanError::validation(
            "MCP_CONFIG_INVALID_JSON",
            format!("{} is not valid JSON: {}", path.display(), e),
        )
    })?;
    let existing = find_server_at_pointer(&value, &pointer)
        .cloned()
        .ok_or_else(|| {
            VulcanError::validation(
                "MCP_CONFIG_NOT_INSTALLED",
                format!(
                    "no Vulcan server entry at /{} in {}. Run `{}` first to do a fresh install.",
                    pointer.join("/"),
                    path.display(),
                    mcp_install_command(target, scope, true)
                ),
            )
        })?;

    // Reconstruct args from the existing entry's flags so we preserve the
    // user's --dangerous and --groups choices.
    let existing_args: Vec<String> = existing
        .get("args")
        .and_then(|v| v.as_array())
        .map(|a| {
            a.iter()
                .filter_map(|x| x.as_str().map(str::to_string))
                .collect()
        })
        .unwrap_or_default();
    let dangerous = existing_args.iter().any(|a| a == "--allow-dangerous");
    let groups_value: Option<String> = existing_args
        .iter()
        .position(|a| a == "--groups")
        .and_then(|i| existing_args.get(i + 1).cloned());
    let groups = groups_value.map(|s| {
        s.split(',')
            .map(|p| p.trim().to_string())
            .filter(|p| !p.is_empty())
            .collect::<Vec<_>>()
    });

    // Rebuild server with abs-path command + preserved env.
    let mut repaired = mcp_server_value_with_password(dangerous, groups, None, None);
    if let Some(env) = existing.get("env").cloned() {
        repaired
            .as_object_mut()
            .expect("server is object")
            .insert("env".to_string(), env);
    }

    insert_server_at_pointer(&mut value, &pointer, repaired.clone(), false)?;
    let payload = serde_json::to_vec_pretty(&value)
        .map_err(|e| VulcanError::internal("MCP_CONFIG_SERIALIZE_FAILED", e.to_string()))?;
    atomic_write(&path, &payload)?;
    verify_server_at_pointer(&path, &pointer)?;

    let env_password_present = repaired
        .get("env")
        .and_then(|e| e.get("VULCAN_WALLET_PASSWORD"))
        .and_then(|v| v.as_str())
        .map(|s| !s.is_empty() && s != "your-password")
        .unwrap_or(false);
    let wallet_name = repaired
        .get("env")
        .and_then(|e| e.get("VULCAN_WALLET_NAME"))
        .and_then(|v| v.as_str())
        .map(str::to_string);

    Ok(AgentMcpInstallResult {
        target: target_label(target).to_string(),
        scope: scope_label(scope).to_string(),
        path: path.to_string_lossy().to_string(),
        dry_run: false,
        dangerous,
        installed: true,
        config: redact_mcp_config(value),
        wallet_name,
        wallet_address: None,
        password_env_written: env_password_present,
        placeholder_password: dangerous && !env_password_present,
        manual_install_command: mcp_install_command(target, scope, dangerous),
        restart_required: true,
        restart_instructions: restart_instructions(target),
        warnings: vec![
            "Repair only updates `command` and `args`. Existing `env` (wallet name + password) was preserved verbatim.".to_string(),
        ],
        next_steps: vec![
            "Run `vulcan agent mcp diagnose` to confirm spawn + handshake succeed.".to_string(),
            mcp_next_steps(target, dangerous).into_iter().next().unwrap_or_default(),
        ],
    })
}

#[derive(Debug, Serialize)]
pub struct AgentMcpDiagnoseResult {
    pub target: String,
    pub scope: String,
    pub path: String,
    pub configured: bool,
    pub command: Option<String>,
    pub command_is_absolute: bool,
    pub command_exists_on_disk: bool,
    pub spawn_ok: bool,
    pub spawn_error: Option<String>,
    pub handshake_ok: bool,
    pub handshake_error: Option<String>,
    pub vulcan_tools_count: usize,
    pub sample_tool_names: Vec<String>,
    pub stderr_tail: Option<String>,
    pub verdict: String,
    pub remedies: Vec<String>,
    pub passed: bool,
}

impl TableRenderable for AgentMcpDiagnoseResult {
    fn render_table(&self) {
        println!(
            "Vulcan MCP Diagnose: {}",
            if self.passed { "PASS" } else { "FAIL" }
        );
        println!("─────────────────────────────────────────");
        println!("  Target:           {}", self.target);
        println!("  Scope:            {}", self.scope);
        println!("  Path:             {}", self.path);
        println!("  Configured:       {}", self.configured);
        println!(
            "  Command:          {} (absolute={}, exists={})",
            self.command.as_deref().unwrap_or("-"),
            self.command_is_absolute,
            self.command_exists_on_disk
        );
        println!(
            "  Spawn:            {}{}",
            if self.spawn_ok { "ok" } else { "failed" },
            self.spawn_error
                .as_ref()
                .map(|e| format!(" — {e}"))
                .unwrap_or_default()
        );
        println!(
            "  Handshake:        {}{}",
            if self.handshake_ok { "ok" } else { "failed" },
            self.handshake_error
                .as_ref()
                .map(|e| format!(" — {e}"))
                .unwrap_or_default()
        );
        println!(
            "  Vulcan tools:     {} (sample: {})",
            self.vulcan_tools_count,
            self.sample_tool_names.join(", ")
        );
        println!("  Verdict:          {}", self.verdict);
        if !self.remedies.is_empty() {
            println!();
            println!("Remedies:");
            for r in &self.remedies {
                println!("  • {}", r);
            }
        }
        if let Some(tail) = &self.stderr_tail {
            println!();
            println!("Server stderr tail:");
            for line in tail.lines() {
                println!("  {}", line);
            }
        }
    }
}

/// End-to-end probe: read the installed MCP entry, simulate an agent client
/// spawning the server with that exact command/args/env, run the JSON-RPC
/// handshake, and confirm vulcan_* tools are listed. This is the test we wish
/// Claude Code itself would run before silently dropping a failed MCP server.
pub async fn mcp_diagnose(
    target: AgentTarget,
    scope: AgentScope,
    path: Option<PathBuf>,
) -> Result<AgentMcpDiagnoseResult, VulcanError> {
    let path = path.unwrap_or(mcp_config_path(target, scope)?);
    let pointer = mcp_server_pointer(target, scope, None)?;
    let mut remedies = Vec::new();

    // Step 1: read the server entry from the configured file at the right
    // pointer. If it's missing here, agents won't see it either.
    let configured_value: Option<serde_json::Value> = if path.exists() {
        let content = fs::read_to_string(&path).ok();
        content
            .and_then(|c| serde_json::from_str::<serde_json::Value>(&c).ok())
            .and_then(|v| find_server_at_pointer(&v, &pointer).cloned())
    } else {
        None
    };
    let configured = configured_value.is_some();
    if !configured {
        remedies.push(format!(
            "Run `{}` to install the Vulcan MCP entry.",
            mcp_install_command(target, scope, true)
        ));
        return Ok(AgentMcpDiagnoseResult {
            target: target_label(target).to_string(),
            scope: scope_label(scope).to_string(),
            path: path.to_string_lossy().to_string(),
            configured: false,
            command: None,
            command_is_absolute: false,
            command_exists_on_disk: false,
            spawn_ok: false,
            spawn_error: Some(format!(
                "no Vulcan server entry at /{} in {}",
                pointer.join("/"),
                path.display()
            )),
            handshake_ok: false,
            handshake_error: None,
            vulcan_tools_count: 0,
            sample_tool_names: Vec::new(),
            stderr_tail: None,
            verdict: "Vulcan MCP server is not configured for this target/scope.".to_string(),
            remedies,
            passed: false,
        });
    }

    let server = configured_value.unwrap();
    let command = server
        .get("command")
        .and_then(|v| v.as_str())
        .map(str::to_string);
    let args: Vec<String> = server
        .get("args")
        .and_then(|v| v.as_array())
        .map(|a| {
            a.iter()
                .filter_map(|x| x.as_str().map(str::to_string))
                .collect()
        })
        .unwrap_or_default();
    let env_map: std::collections::HashMap<String, String> = server
        .get("env")
        .and_then(|v| v.as_object())
        .map(|m| {
            m.iter()
                .filter_map(|(k, v)| v.as_str().map(|s| (k.clone(), s.to_string())))
                .collect()
        })
        .unwrap_or_default();

    let command_str = command.clone().unwrap_or_default();
    let command_is_absolute = std::path::Path::new(&command_str).is_absolute();
    let command_exists_on_disk = if command_is_absolute {
        std::path::Path::new(&command_str).exists()
    } else {
        // Bare names like "vulcan" — Claude Code resolves these against ITS
        // own PATH (often a stripped one from Finder/Spotlight launches), not
        // the user's shell PATH. We can't reliably check that here; flag it.
        remedies.push(format!(
            "MCP `command` is the bare name `{command_str}`. Claude Code may inherit a stripped PATH that does not include shim dirs (mise, nvm, etc.) and silently fail to spawn the server. Re-run `vulcan agent mcp install --target {} --scope {} --dangerous` to write an absolute path.",
            target_label(target),
            scope_label(scope)
        ));
        true // can't disprove without trying
    };

    // Step 2: spawn the server with the configured command/args/env (no PATH
    // inheritance — exactly what a fresh agent process gets, modulo whatever
    // launcher-injected PATH it sees) and run the handshake.
    let (spawn_ok, spawn_error, handshake_ok, handshake_error, tools, stderr_tail) =
        spawn_and_handshake(&command_str, &args, &env_map).await;

    let vulcan_tools: Vec<String> = tools
        .iter()
        .filter(|n| n.starts_with("vulcan_"))
        .cloned()
        .collect();
    let sample_tool_names = vulcan_tools.iter().take(5).cloned().collect();

    let passed = configured && spawn_ok && handshake_ok && !vulcan_tools.is_empty();
    let verdict = if passed {
        format!(
            "An agent client launching this MCP server will see {} vulcan_* tools after restart.",
            vulcan_tools.len()
        )
    } else if !spawn_ok {
        format!(
            "Spawn failed: {}",
            spawn_error.as_deref().unwrap_or("unknown")
        )
    } else if !handshake_ok {
        format!(
            "Server spawned but JSON-RPC handshake failed: {}",
            handshake_error.as_deref().unwrap_or("unknown")
        )
    } else if vulcan_tools.is_empty() {
        "Server replied to tools/list but returned zero vulcan_* tools.".to_string()
    } else {
        "Diagnose did not pass.".to_string()
    };
    if !passed && remedies.is_empty() {
        remedies.push(
            "Re-run install: `vulcan agent mcp install --target ... --scope ... --dangerous`."
                .to_string(),
        );
    }

    Ok(AgentMcpDiagnoseResult {
        target: target_label(target).to_string(),
        scope: scope_label(scope).to_string(),
        path: path.to_string_lossy().to_string(),
        configured,
        command,
        command_is_absolute,
        command_exists_on_disk,
        spawn_ok,
        spawn_error,
        handshake_ok,
        handshake_error,
        vulcan_tools_count: vulcan_tools.len(),
        sample_tool_names,
        stderr_tail,
        verdict,
        remedies,
        passed,
    })
}

async fn spawn_and_handshake(
    command: &str,
    args: &[String],
    env: &std::collections::HashMap<String, String>,
) -> (
    bool,
    Option<String>,
    bool,
    Option<String>,
    Vec<String>,
    Option<String>,
) {
    use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
    use tokio::process::Command;
    use tokio::time::{timeout, Duration};

    let mut cmd = Command::new(command);
    cmd.args(args);
    // Mirror the env Claude Code would pass: only the explicit env block from
    // the MCP entry, no parent env. This is the strictest possible test and
    // matches the worst-case launcher.
    cmd.env_clear();
    for (k, v) in env {
        cmd.env(k, v);
    }
    // Preserve a minimal PATH so the server itself can run subprocesses if
    // needed; the spawn lookup of `command` is by absolute path so PATH does
    // not affect step 1.
    cmd.env("PATH", "/usr/local/bin:/usr/bin:/bin");
    cmd.env("HOME", std::env::var("HOME").unwrap_or_default());
    cmd.stdin(std::process::Stdio::piped());
    cmd.stdout(std::process::Stdio::piped());
    cmd.stderr(std::process::Stdio::piped());

    let mut child = match cmd.spawn() {
        Ok(c) => c,
        Err(e) => {
            return (
                false,
                Some(format!("spawn `{command}` failed: {e}")),
                false,
                None,
                Vec::new(),
                None,
            );
        }
    };

    let mut stdin = child.stdin.take().expect("piped stdin");
    let stdout = child.stdout.take().expect("piped stdout");
    let stderr = child.stderr.take().expect("piped stderr");

    let handshake = async {
        let init = r#"{"jsonrpc":"2.0","id":1,"method":"initialize","params":{"protocolVersion":"2024-11-05","capabilities":{},"clientInfo":{"name":"vulcan-diagnose","version":"0"}}}"#;
        let initialized = r#"{"jsonrpc":"2.0","method":"notifications/initialized"}"#;
        let tools_list = r#"{"jsonrpc":"2.0","id":2,"method":"tools/list"}"#;
        stdin.write_all(init.as_bytes()).await?;
        stdin.write_all(b"\n").await?;
        stdin.write_all(initialized.as_bytes()).await?;
        stdin.write_all(b"\n").await?;
        stdin.write_all(tools_list.as_bytes()).await?;
        stdin.write_all(b"\n").await?;
        stdin.flush().await?;
        Ok::<(), std::io::Error>(())
    };

    let mut stdout_reader = BufReader::new(stdout);
    let mut stderr_reader = BufReader::new(stderr);

    let mut tools: Vec<String> = Vec::new();
    let mut handshake_ok = false;
    let mut handshake_error: Option<String> = None;
    let mut stderr_lines: Vec<String> = Vec::new();

    let probe = async {
        if let Err(e) = handshake.await {
            return Err(format!("write handshake: {e}"));
        }
        // Read up to ~5 JSON-RPC frames (init reply, optional notifications,
        // tools/list reply) with a hard timeout.
        for _ in 0..10 {
            let mut line = String::new();
            match stdout_reader.read_line(&mut line).await {
                Ok(0) => break,
                Ok(_) => {
                    let trimmed = line.trim();
                    if trimmed.is_empty() {
                        continue;
                    }
                    if let Ok(msg) = serde_json::from_str::<serde_json::Value>(trimmed) {
                        if let Some(result) = msg.get("result") {
                            if let Some(arr) = result.get("tools").and_then(|t| t.as_array()) {
                                for t in arr {
                                    if let Some(name) = t.get("name").and_then(|n| n.as_str()) {
                                        tools.push(name.to_string());
                                    }
                                }
                                handshake_ok = true;
                                break;
                            } else if msg.get("id").and_then(|i| i.as_i64()) == Some(1) {
                                // initialize reply — keep reading for tools/list reply.
                                continue;
                            }
                        }
                        if let Some(err) = msg.get("error") {
                            return Err(format!("server returned JSON-RPC error: {err}"));
                        }
                    }
                }
                Err(e) => return Err(format!("read response: {e}")),
            }
        }
        Ok(())
    };

    let stderr_collector = async {
        for _ in 0..50 {
            let mut line = String::new();
            match stderr_reader.read_line(&mut line).await {
                Ok(0) => break,
                Ok(_) => stderr_lines.push(line.trim_end().to_string()),
                Err(_) => break,
            }
        }
    };

    match timeout(Duration::from_secs(8), async {
        let probe_res = probe.await;
        let _ = tokio::time::timeout(Duration::from_millis(100), stderr_collector).await;
        probe_res
    })
    .await
    {
        Ok(Ok(())) => {}
        Ok(Err(e)) => handshake_error = Some(e),
        Err(_) => {
            handshake_error = Some(
                "JSON-RPC handshake timed out after 8s; server may have crashed at startup"
                    .to_string(),
            );
        }
    }

    let _ = child.kill().await;
    let _ = child.wait().await;

    let stderr_tail = if stderr_lines.is_empty() {
        None
    } else {
        Some(
            stderr_lines
                .iter()
                .rev()
                .take(15)
                .cloned()
                .collect::<Vec<_>>()
                .into_iter()
                .rev()
                .collect::<Vec<_>>()
                .join("\n"),
        )
    };

    (
        true,
        None,
        handshake_ok,
        handshake_error,
        tools,
        stderr_tail,
    )
}

pub fn mcp_doctor(
    target: AgentTarget,
    scope: AgentScope,
    path: Option<PathBuf>,
) -> Result<AgentMcpDoctorResult, VulcanError> {
    let path = path.unwrap_or(mcp_config_path(target, scope)?);
    let pointer = mcp_server_pointer(target, scope, None)?;
    let (configured, dangerous_enabled, password_env_present, wallet_env_name, mut warnings) =
        inspect_mcp_config_at(&path, &pointer);
    if !configured {
        warnings.push("Vulcan MCP server is not configured at this path.".to_string());
    }
    // Migration warning: if a legacy ~/.claude/settings.json with mcpServers exists,
    // Claude Code ignores it. Tell the user to re-run install so the new ~/.claude.json
    // path takes effect.
    if matches!((target, scope), (AgentTarget::Claude, AgentScope::User)) {
        if let Some(home) = dirs::home_dir() {
            let legacy = home.join(".claude/settings.json");
            if legacy != path && legacy.exists() {
                let (legacy_configured, ..) = inspect_mcp_config(&legacy);
                if legacy_configured {
                    warnings.push(format!(
                        "Legacy MCP config at {} contains a `mcpServers` block — Claude Code ignores it. The current installer writes to ~/.claude.json instead; you can safely remove the legacy `mcpServers` key from settings.json.",
                        legacy.display()
                    ));
                }
            }
        }
    }
    Ok(AgentMcpDoctorResult {
        target: target_label(target).to_string(),
        scope: scope_label(scope).to_string(),
        path: path.to_string_lossy().to_string(),
        storage_pointer: format_pointer(&pointer),
        exists: path.exists(),
        configured,
        dangerous_enabled,
        password_env_present,
        wallet_env_name,
        warnings,
        manual_install_command: mcp_install_command(target, scope, true),
        restart_required: configured,
        restart_instructions: restart_instructions(target),
    })
}

pub fn live_agent_readiness(
    target: AgentTarget,
    scope: AgentScope,
    path: Option<PathBuf>,
) -> Result<AgentLiveAgentReadiness, VulcanError> {
    let doctor = mcp_doctor(target, scope, path)?;
    let cli_env_unlock_present = std::env::var("VULCAN_WALLET_PASSWORD")
        .map(|value| !value.is_empty())
        .unwrap_or(false);
    let mcp_ready = doctor.configured && doctor.dangerous_enabled && doctor.password_env_present;
    let (recommended_execution_path, blocking_reason) = if mcp_ready {
        ("mcp".to_string(), None)
    } else if cli_env_unlock_present {
        ("cli_env".to_string(), None)
    } else {
        let reason = if !doctor.configured {
            "mcp_not_configured"
        } else if !doctor.dangerous_enabled {
            "mcp_dangerous_not_enabled"
        } else {
            "mcp_wallet_password_missing_or_placeholder"
        };
        ("not_ready".to_string(), Some(reason.to_string()))
    };
    let resolution_summary = if mcp_ready {
        format!(
            "Live signing is ready for {} through MCP. Restart is only needed after config changes.",
            display_target_name(target)
        )
    } else if cli_env_unlock_present {
        "Live CLI fallback can sign because VULCAN_WALLET_PASSWORD is present. MCP remains recommended for agent-driven live trading.".to_string()
    } else {
        format!(
            "To enable live signing in {}, run `{}`, then {}.",
            display_target_name(target),
            mcp_install_command(target, scope, true),
            restart_instructions(target)
        )
    };
    Ok(AgentLiveAgentReadiness {
        target: target_label(target).to_string(),
        target_source: "explicit_or_default".to_string(),
        scope: scope_label(scope).to_string(),
        mcp_configured: doctor.configured,
        dangerous_enabled: doctor.dangerous_enabled,
        password_env_present: doctor.password_env_present,
        wallet_env_name: doctor.wallet_env_name,
        cli_env_unlock_present,
        recommended_execution_path,
        blocking_reason,
        resolution_summary,
        install_command: mcp_install_command(target, scope, true),
        doctor_command: mcp_doctor_command(target, scope),
        restart_instructions: restart_instructions(target),
        config_path: doctor.path,
    })
}

#[allow(clippy::too_many_arguments)]
pub fn mcp_install(
    ctx: Option<&AppContext>,
    target: AgentTarget,
    scope: AgentScope,
    path: Option<PathBuf>,
    dangerous: bool,
    groups: Option<Vec<String>>,
    force: bool,
    dry_run: bool,
) -> Result<AgentMcpInstallResult, VulcanError> {
    mcp_install_with_project(
        ctx, target, scope, path, None, dangerous, groups, force, dry_run,
    )
}

/// Internal install entry point that takes an explicit `project_path` override —
/// only consulted for targets that key the server under a project (Claude/user).
/// Tests use this to avoid relying on `std::env::current_dir()`.
#[allow(clippy::too_many_arguments)]
pub fn mcp_install_with_project(
    ctx: Option<&AppContext>,
    target: AgentTarget,
    scope: AgentScope,
    path: Option<PathBuf>,
    project_path: Option<PathBuf>,
    dangerous: bool,
    groups: Option<Vec<String>>,
    force: bool,
    dry_run: bool,
) -> Result<AgentMcpInstallResult, VulcanError> {
    let path = path.unwrap_or(mcp_config_path(target, scope)?);
    let pointer = mcp_server_pointer(target, scope, project_path.as_deref())?;
    let install_secret = resolve_mcp_install_secret(ctx, dangerous, dry_run)?;
    let server = mcp_server_value_with_password(
        dangerous,
        groups.clone(),
        install_secret
            .as_ref()
            .map(|secret| secret.password.expose()),
        install_secret
            .as_ref()
            .map(|secret| secret.wallet_name.as_str()),
    );
    let config_to_write = if path.exists() {
        let content = fs::read_to_string(&path)
            .map_err(|e| VulcanError::io("MCP_CONFIG_READ_FAILED", e.to_string()))?;
        let mut value: serde_json::Value = serde_json::from_str(&content).map_err(|e| {
            VulcanError::validation(
                "MCP_CONFIG_INVALID_JSON",
                format!("{} is not valid JSON: {}", path.display(), e),
            )
        })?;
        if value.as_object().is_none() {
            if force {
                build_config_with_server(&pointer, server.clone())
            } else {
                return Err(VulcanError::validation(
                    "MCP_CONFIG_UNSUPPORTED_SHAPE",
                    "existing MCP config is not a JSON object; pass --force to replace it",
                ));
            }
        } else {
            insert_server_at_pointer(&mut value, &pointer, server.clone(), force)?;
            value
        }
    } else {
        build_config_with_server(&pointer, server.clone())
    };
    if !dry_run {
        if let Some(parent) = path.parent() {
            fs::create_dir_all(parent)
                .map_err(|e| VulcanError::io("MCP_CONFIG_DIR_CREATE_FAILED", e.to_string()))?;
        }
        let payload = serde_json::to_vec_pretty(&config_to_write)
            .map_err(|e| VulcanError::internal("MCP_CONFIG_SERIALIZE_FAILED", e.to_string()))?;
        atomic_write(&path, &payload)?;
        // Post-install verification: read back and assert the vulcan server is at the
        // expected pointer. This catches both "wrote to the wrong file" regressions
        // and concurrent rewriters (e.g. Claude Code rewriting ~/.claude.json while
        // we were mid-write).
        verify_server_at_pointer(&path, &pointer)?;
    }
    let config = redact_mcp_config(config_to_write.clone());
    let password_env_written = dangerous
        && !dry_run
        && install_secret
            .as_ref()
            .map(|secret| !secret.password.is_empty())
            .unwrap_or(false);
    let placeholder_password = dangerous && !password_env_written;
    Ok(AgentMcpInstallResult {
        target: target_label(target).to_string(),
        scope: scope_label(scope).to_string(),
        path: path.to_string_lossy().to_string(),
        dry_run,
        dangerous,
        installed: !dry_run,
        config,
        wallet_name: install_secret
            .as_ref()
            .map(|secret| secret.wallet_name.clone()),
        wallet_address: install_secret
            .as_ref()
            .map(|secret| secret.wallet_address.clone()),
        password_env_written,
        placeholder_password,
        manual_install_command: mcp_install_command(target, scope, dangerous),
        restart_required: !dry_run,
        restart_instructions: restart_instructions(target),
        warnings: mcp_warnings(dangerous),
        next_steps: mcp_next_steps(target, dangerous),
    })
}

pub fn mcp_set_wallet(
    ctx: &AppContext,
    target: AgentTarget,
    scope: AgentScope,
    path: Option<PathBuf>,
    wallet_name: &str,
    dry_run: bool,
) -> Result<AgentMcpSetWalletResult, VulcanError> {
    let path = path.unwrap_or(mcp_config_path(target, scope)?);
    let pointer = mcp_server_pointer(target, scope, None)?;
    let wallet_secret = resolve_mcp_wallet_switch_secret(ctx, wallet_name, dry_run)?;

    let content = fs::read_to_string(&path)
        .map_err(|e| VulcanError::io("MCP_CONFIG_READ_FAILED", e.to_string()))?;
    let mut config: serde_json::Value = serde_json::from_str(&content).map_err(|e| {
        VulcanError::validation(
            "MCP_CONFIG_INVALID_JSON",
            format!("{} is not valid JSON: {}", path.display(), e),
        )
    })?;

    let mutation = set_mcp_config_wallet_at(
        &mut config,
        &pointer,
        wallet_name,
        (!wallet_secret.password.is_empty()).then_some(wallet_secret.password.expose()),
    )
    .map_err(|err| {
        if err.code == "MCP_CONFIG_NOT_INSTALLED" {
            VulcanError::validation(
                err.code,
                format!(
                    "No Vulcan MCP server found in {}. Run `{}` first.",
                    path.display(),
                    mcp_install_command(target, scope, true)
                ),
            )
        } else {
            err
        }
    })?;
    let SetWalletMutation {
        previous_wallet_name,
        dangerous_newly_enabled,
    } = mutation;
    let password_env_written = find_server_at_pointer(&config, &pointer)
        .and_then(|server| server.get("env"))
        .and_then(|env| env.as_object())
        .map(|env| {
            env.get("VULCAN_WALLET_PASSWORD")
                .and_then(|value| value.as_str())
                .map(|value| value != "your-password")
                .unwrap_or(false)
        })
        .unwrap_or(false)
        && !dry_run;

    if !dry_run {
        let payload = serde_json::to_vec_pretty(&config)
            .map_err(|e| VulcanError::internal("MCP_CONFIG_SERIALIZE_FAILED", e.to_string()))?;
        atomic_write(&path, &payload)?;
        verify_server_at_pointer(&path, &pointer)?;
    }

    let mut warnings = Vec::new();
    if dry_run {
        warnings.push(
            "Dry run did not prompt for or validate the wallet password, and did not write MCP config.".to_string(),
        );
    } else if !password_env_written {
        warnings.push(
            "VULCAN_WALLET_PASSWORD is not set in the MCP config; live signing will need a password at MCP startup or a reinstall.".to_string(),
        );
    }
    if dangerous_newly_enabled {
        warnings.push(
            "Added --allow-dangerous to the MCP server args. After restart, this agent will see live trading tools (orders, deposits, withdrawals, cancellations).".to_string(),
        );
    }

    Ok(AgentMcpSetWalletResult {
        target: target_label(target).to_string(),
        scope: scope_label(scope).to_string(),
        path: path.to_string_lossy().to_string(),
        dry_run,
        wallet_name: wallet_name.to_string(),
        wallet_address: wallet_secret.wallet_address,
        previous_wallet_name,
        updated: !dry_run,
        password_env_written,
        dangerous_newly_enabled,
        config: redact_mcp_config(config),
        restart_required: !dry_run,
        restart_instructions: restart_instructions(target),
        warnings,
    })
}

#[cfg(test)]
fn set_mcp_config_wallet(
    config: &mut serde_json::Value,
    wallet_name: &str,
    password: Option<&str>,
) -> Result<SetWalletMutation, VulcanError> {
    set_mcp_config_wallet_at(
        config,
        &["mcpServers".to_string(), "vulcan".to_string()],
        wallet_name,
        password,
    )
}

struct SetWalletMutation {
    previous_wallet_name: Option<String>,
    dangerous_newly_enabled: bool,
}

fn set_mcp_config_wallet_at(
    config: &mut serde_json::Value,
    pointer: &[String],
    wallet_name: &str,
    password: Option<&str>,
) -> Result<SetWalletMutation, VulcanError> {
    let server = find_server_at_pointer_mut(config, pointer).ok_or_else(|| {
        VulcanError::validation("MCP_CONFIG_NOT_INSTALLED", "No Vulcan MCP server found")
    })?;
    let server_object = server.as_object_mut().ok_or_else(|| {
        VulcanError::validation(
            "MCP_CONFIG_UNSUPPORTED_SHAPE",
            "existing Vulcan MCP server entry is not a JSON object",
        )
    })?;
    let dangerous_newly_enabled = ensure_allow_dangerous_arg(server_object)?;
    let env = server_object
        .entry("env")
        .or_insert_with(|| serde_json::json!({}));
    let env_object = env.as_object_mut().ok_or_else(|| {
        VulcanError::validation(
            "MCP_CONFIG_UNSUPPORTED_SHAPE",
            "existing Vulcan MCP env value is not a JSON object",
        )
    })?;
    let previous_wallet_name = env_object
        .get("VULCAN_WALLET_NAME")
        .and_then(|value| value.as_str())
        .map(|value| value.to_string());
    env_object.insert(
        "VULCAN_WALLET_NAME".to_string(),
        serde_json::Value::String(wallet_name.to_string()),
    );
    if let Some(password) = password {
        env_object.insert(
            "VULCAN_WALLET_PASSWORD".to_string(),
            serde_json::Value::String(password.to_string()),
        );
    }
    Ok(SetWalletMutation {
        previous_wallet_name,
        dangerous_newly_enabled,
    })
}

/// Wiring up a wallet for live MCP signing implies the user wants dangerous
/// tools exposed; otherwise the password env is unreachable. Inserts
/// `--allow-dangerous` into the server's `args` array if absent. Idempotent.
/// Returns true when the flag was newly added.
fn ensure_allow_dangerous_arg(
    server_object: &mut serde_json::Map<String, serde_json::Value>,
) -> Result<bool, VulcanError> {
    let args_value = server_object
        .entry("args")
        .or_insert_with(|| serde_json::Value::Array(vec![serde_json::json!("mcp")]));
    let args_array = args_value.as_array_mut().ok_or_else(|| {
        VulcanError::validation(
            "MCP_CONFIG_UNSUPPORTED_SHAPE",
            "existing Vulcan MCP args value is not a JSON array",
        )
    })?;
    if args_array
        .iter()
        .any(|v| v.as_str() == Some("--allow-dangerous"))
    {
        return Ok(false);
    }
    // Insert directly after the leading `mcp` subcommand so the canonical
    // ordering matches a fresh `mcp_install --dangerous` write.
    let insert_at = if args_array
        .first()
        .and_then(|v| v.as_str())
        .map(|s| s == "mcp")
        .unwrap_or(false)
    {
        1
    } else {
        args_array.len()
    };
    args_array.insert(insert_at, serde_json::json!("--allow-dangerous"));
    Ok(true)
}

fn resolve_mcp_wallet_switch_secret(
    ctx: &AppContext,
    wallet_name: &str,
    dry_run: bool,
) -> Result<McpInstallSecret, VulcanError> {
    let wallet_file = ctx
        .wallet_store
        .load(wallet_name)
        .map_err(|e| VulcanError::auth("WALLET_NOT_FOUND", e.to_string()))?;
    if dry_run {
        return Ok(McpInstallSecret {
            wallet_name: wallet_name.to_string(),
            wallet_address: wallet_file.public_key,
            password: crate::secrets::SecretString::new(String::new()),
        });
    }

    use std::io::IsTerminal;
    if !io::stdin().is_terminal() {
        return Err(VulcanError::auth(
            "MCP_SET_WALLET_REQUIRES_TTY",
            "Switching the MCP wallet must be run interactively so Vulcan can read and validate the selected wallet password before writing MCP config.",
        ));
    }
    eprint!("Wallet password for '{}': ", wallet_name);
    io::stderr()
        .flush()
        .map_err(|e| VulcanError::io("FLUSH_FAILED", e.to_string()))?;
    let password = rpassword::read_password()
        .map_err(|e| VulcanError::io("PASSWORD_READ_FAILED", e.to_string()))?;
    crate::wallet::Wallet::decrypt(&wallet_file.encrypted, &password)
        .map_err(|e| VulcanError::auth("DECRYPT_FAILED", e.to_string()))?;

    Ok(McpInstallSecret {
        wallet_name: wallet_name.to_string(),
        wallet_address: wallet_file.public_key,
        password: crate::secrets::SecretString::new(password),
    })
}

fn mcp_config_value(dangerous: bool, groups: Option<Vec<String>>) -> serde_json::Value {
    mcp_config_value_with_password(dangerous, groups, None, None)
}

fn mcp_config_value_with_password(
    dangerous: bool,
    groups: Option<Vec<String>>,
    password: Option<&str>,
    wallet_name: Option<&str>,
) -> serde_json::Value {
    serde_json::json!({
        "mcpServers": {
            "vulcan": mcp_server_value_with_password(dangerous, groups, password, wallet_name)
        }
    })
}

/// Build a top-of-document object containing the vulcan server at the given pointer.
/// Used when no config file exists yet, or when `--force` is replacing a non-object root.
fn build_config_with_server(pointer: &[String], server: serde_json::Value) -> serde_json::Value {
    // Walk the pointer in reverse, wrapping the value in nested objects.
    let mut value = server;
    for key in pointer.iter().rev() {
        let mut obj = serde_json::Map::new();
        obj.insert(key.clone(), value);
        value = serde_json::Value::Object(obj);
    }
    value
}

/// Insert `server` at `pointer` inside `value`. Creates any missing intermediate
/// objects. Fails when an intermediate or terminal slot already holds a non-object
/// value, unless `force` is set (then it's overwritten).
fn insert_server_at_pointer(
    value: &mut serde_json::Value,
    pointer: &[String],
    server: serde_json::Value,
    force: bool,
) -> Result<(), VulcanError> {
    if pointer.is_empty() {
        return Err(VulcanError::internal(
            "MCP_POINTER_EMPTY",
            "MCP server pointer cannot be empty",
        ));
    }
    let mut current = value;
    let last_idx = pointer.len() - 1;
    for (i, key) in pointer.iter().enumerate() {
        if i == last_idx {
            let obj = current.as_object_mut().ok_or_else(|| {
                VulcanError::validation(
                    "MCP_CONFIG_UNSUPPORTED_SHAPE",
                    "parent of MCP server entry is not a JSON object",
                )
            })?;
            obj.insert(key.clone(), server);
            return Ok(());
        }
        // Intermediate: ensure object, then descend.
        let obj = current.as_object_mut().ok_or_else(|| {
            VulcanError::validation(
                "MCP_CONFIG_UNSUPPORTED_SHAPE",
                "intermediate value on the way to MCP server entry is not a JSON object",
            )
        })?;
        let entry = obj
            .entry(key.clone())
            .or_insert_with(|| serde_json::json!({}));
        if !entry.is_object() {
            if force {
                *entry = serde_json::json!({});
            } else {
                return Err(VulcanError::validation(
                    "MCP_CONFIG_UNSUPPORTED_SHAPE",
                    format!(
                        "existing value at `{}` is not a JSON object; pass --force to replace it",
                        key
                    ),
                ));
            }
        }
        current = entry;
    }
    Ok(())
}

/// Look up an existing server at `pointer` (read-only). Returns None if missing.
fn find_server_at_pointer<'a>(
    value: &'a serde_json::Value,
    pointer: &[String],
) -> Option<&'a serde_json::Value> {
    let mut current = value;
    for key in pointer {
        current = current.as_object()?.get(key)?;
    }
    Some(current)
}

fn find_server_at_pointer_mut<'a>(
    value: &'a mut serde_json::Value,
    pointer: &[String],
) -> Option<&'a mut serde_json::Value> {
    let mut current = value;
    for key in pointer {
        current = current.as_object_mut()?.get_mut(key)?;
    }
    Some(current)
}

/// Write `bytes` to `path` atomically: write to a sibling tempfile, then rename.
/// Atomicity matters here because Claude Code itself rewrites ~/.claude.json on
/// session events (numStartups, recent projects, etc.) — a partial write at the
/// real path could be picked up mid-flight.
fn atomic_write(path: &Path, bytes: &[u8]) -> Result<(), VulcanError> {
    let parent = path.parent().unwrap_or_else(|| Path::new("."));
    let file_name = path.file_name().and_then(|n| n.to_str()).ok_or_else(|| {
        VulcanError::io(
            "MCP_CONFIG_WRITE_FAILED",
            format!("invalid path {}", path.display()),
        )
    })?;
    let tmp_path = parent.join(format!(".{file_name}.vulcan-tmp"));

    // Create the temp file with mode 0600 *at open time* so the contents are
    // never readable by other local users — not even briefly between create
    // and chmod. On non-Unix platforms we fall back to a plain write; the OS
    // ACL story there is different (e.g. Windows %APPDATA% is per-user by
    // default) and not addressable from this layer.
    #[cfg(unix)]
    {
        use std::io::Write;
        use std::os::unix::fs::OpenOptionsExt;
        let mut tmp = std::fs::OpenOptions::new()
            .create(true)
            .write(true)
            .truncate(true)
            .mode(0o600)
            .open(&tmp_path)
            .map_err(|e| VulcanError::io("MCP_CONFIG_WRITE_FAILED", e.to_string()))?;
        tmp.write_all(bytes)
            .map_err(|e| VulcanError::io("MCP_CONFIG_WRITE_FAILED", e.to_string()))?;
        tmp.sync_all()
            .map_err(|e| VulcanError::io("MCP_CONFIG_WRITE_FAILED", e.to_string()))?;
    }
    #[cfg(not(unix))]
    fs::write(&tmp_path, bytes)
        .map_err(|e| VulcanError::io("MCP_CONFIG_WRITE_FAILED", e.to_string()))?;

    fs::rename(&tmp_path, path)
        .map_err(|e| VulcanError::io("MCP_CONFIG_WRITE_FAILED", e.to_string()))?;
    Ok(())
}

/// Read `path` back after install and assert `pointer` resolves to a JSON object
/// (the vulcan server entry). Catches "wrote to the wrong file" regressions and
/// concurrent rewrites by other processes.
fn verify_server_at_pointer(path: &Path, pointer: &[String]) -> Result<(), VulcanError> {
    let content = fs::read_to_string(path).map_err(|e| {
        VulcanError::io(
            "MCP_VERIFY_READ_FAILED",
            format!(
                "could not read back {} after install: {}",
                path.display(),
                e
            ),
        )
    })?;
    let value: serde_json::Value = serde_json::from_str(&content).map_err(|e| {
        VulcanError::internal(
            "MCP_VERIFY_INVALID_JSON",
            format!("read-back of {} is not JSON: {}", path.display(), e),
        )
    })?;
    if find_server_at_pointer(&value, pointer).is_none() {
        return Err(VulcanError::internal(
            "MCP_VERIFY_NOT_FOUND",
            format!(
                "wrote to {} but vulcan server is not present at /{} on read-back; another process may have overwritten the file",
                path.display(),
                pointer.join("/")
            ),
        ));
    }
    Ok(())
}

fn mcp_server_value_with_password(
    dangerous: bool,
    groups: Option<Vec<String>>,
    password: Option<&str>,
    wallet_name: Option<&str>,
) -> serde_json::Value {
    let mut args = vec!["mcp".to_string()];
    if dangerous {
        args.push("--allow-dangerous".to_string());
    }
    if let Some(groups) = groups.filter(|groups| !groups.is_empty()) {
        args.push("--groups".to_string());
        args.push(groups.join(","));
    }
    let mut server = serde_json::json!({
        "command": resolve_vulcan_command(),
        "args": args
    });
    if dangerous {
        server.as_object_mut().unwrap().insert(
            "env".to_string(),
            serde_json::json!({
                "VULCAN_WALLET_NAME": wallet_name.unwrap_or("your-wallet"),
                "VULCAN_WALLET_PASSWORD": password.unwrap_or("your-password")
            }),
        );
    }
    server
}

/// Pick the value to write as `command` in the MCP config. We prefer the
/// absolute path of the currently-running `vulcan` binary (`current_exe()`)
/// because Claude Code on macOS — especially when launched from Finder /
/// Spotlight rather than a login shell — inherits a stripped PATH that often
/// does not include shim directories like `~/.local/share/mise/shims`. A bare
/// `"command": "vulcan"` then spawns with ENOENT and Claude Code silently
/// drops the MCP server, so `vulcan_*` tools never appear in the session.
///
/// Falls back to the bare name only if `current_exe()` fails (rare; e.g. the
/// installer was launched in a way that lost its argv0).
fn resolve_vulcan_command() -> String {
    match std::env::current_exe() {
        Ok(path) => path.to_string_lossy().to_string(),
        Err(_) => "vulcan".to_string(),
    }
}

#[derive(Debug)]
struct McpInstallSecret {
    wallet_name: String,
    wallet_address: String,
    /// Plaintext wallet password held in memory only long enough to write it
    /// into the MCP config. Wrapped so it can't leak via `{:?}` log lines,
    /// panic backtraces, or accidental serialization, and zeroized on drop.
    password: crate::secrets::SecretString,
}

fn resolve_mcp_install_secret(
    ctx: Option<&AppContext>,
    dangerous: bool,
    dry_run: bool,
) -> Result<Option<McpInstallSecret>, VulcanError> {
    if !dangerous || dry_run {
        return Ok(None);
    }

    use std::io::IsTerminal;
    if !io::stdin().is_terminal() {
        return Err(VulcanError::auth(
            "MCP_DANGEROUS_INSTALL_REQUIRES_TTY",
            "Dangerous MCP install must be run interactively so Vulcan can select a wallet, read the wallet password, validate decryption, and write ready-to-use MCP config. Agents should ask the user to run `vulcan agent mcp install --target <agent> --scope user --dangerous` outside auto-mode.",
        ));
    }
    let Some(ctx) = ctx else {
        return Err(VulcanError::internal(
            "MCP_INSTALL_CONTEXT_REQUIRED",
            "dangerous MCP install requires application context",
        ));
    };

    let wallets = ctx
        .wallet_store
        .list()
        .map_err(|e| VulcanError::config("WALLET_LIST_FAILED", e.to_string()))?;
    if wallets.is_empty() {
        return Err(VulcanError::config(
            "NO_WALLETS",
            "No wallets found. Create or import a wallet before dangerous MCP install.",
        ));
    }
    let default_wallet = ctx
        .wallet_store
        .default_wallet()
        .map_err(|e| VulcanError::config("WALLET_DEFAULT_FAILED", e.to_string()))?;
    let wallet_name = prompt_wallet_selection(&wallets, default_wallet.as_deref())?;
    let wallet_file = ctx
        .wallet_store
        .load(&wallet_name)
        .map_err(|e| VulcanError::auth("WALLET_NOT_FOUND", e.to_string()))?;
    eprint!("Wallet password for '{}': ", wallet_name);
    io::stderr()
        .flush()
        .map_err(|e| VulcanError::io("FLUSH_FAILED", e.to_string()))?;
    let password = rpassword::read_password()
        .map_err(|e| VulcanError::io("PASSWORD_READ_FAILED", e.to_string()))?;
    crate::wallet::Wallet::decrypt(&wallet_file.encrypted, &password)
        .map_err(|e| VulcanError::auth("DECRYPT_FAILED", e.to_string()))?;

    Ok(Some(McpInstallSecret {
        wallet_name,
        wallet_address: wallet_file.public_key,
        password: crate::secrets::SecretString::new(password),
    }))
}

fn prompt_wallet_selection(
    wallets: &[String],
    default_wallet: Option<&str>,
) -> Result<String, VulcanError> {
    println!("Select wallet for dangerous MCP live signing:");
    for (idx, wallet) in wallets.iter().enumerate() {
        let default_marker = if Some(wallet.as_str()) == default_wallet {
            " (default)"
        } else {
            ""
        };
        println!("  {}) {}{}", idx + 1, wallet, default_marker);
    }
    let default_idx = default_wallet
        .and_then(|default| wallets.iter().position(|wallet| wallet == default))
        .unwrap_or(0);
    print!("Choice [{}]: ", default_idx + 1);
    io::stdout()
        .flush()
        .map_err(|e| VulcanError::io("FLUSH_FAILED", e.to_string()))?;
    let mut input = String::new();
    io::stdin()
        .lock()
        .read_line(&mut input)
        .map_err(|e| VulcanError::io("READ_FAILED", e.to_string()))?;
    let trimmed = input.trim();
    let idx = if trimmed.is_empty() {
        default_idx
    } else {
        trimmed
            .parse::<usize>()
            .map_err(|_| {
                VulcanError::validation(
                    "INVALID_WALLET_SELECTION",
                    "Wallet choice must be a number",
                )
            })?
            .checked_sub(1)
            .ok_or_else(|| {
                VulcanError::validation(
                    "INVALID_WALLET_SELECTION",
                    "Wallet choice must be at least 1",
                )
            })?
    };
    wallets.get(idx).cloned().ok_or_else(|| {
        VulcanError::validation("INVALID_WALLET_SELECTION", "Wallet choice is out of range")
    })
}

fn redact_mcp_config(mut config: serde_json::Value) -> serde_json::Value {
    if let Some(password) = config
        .get_mut("mcpServers")
        .and_then(|servers| servers.get_mut("vulcan"))
        .and_then(|server| server.get_mut("env"))
        .and_then(|env| env.get_mut("VULCAN_WALLET_PASSWORD"))
    {
        if password
            .as_str()
            .is_some_and(|value| value != "your-password")
        {
            *password = serde_json::Value::String("[REDACTED]".to_string());
        }
    }
    config
}

fn mcp_config_path(target: AgentTarget, scope: AgentScope) -> Result<PathBuf, VulcanError> {
    let home = dirs::home_dir()
        .ok_or_else(|| VulcanError::config("HOME_NOT_FOUND", "Could not determine home dir"))?;
    Ok(match (target, scope) {
        (AgentTarget::Cursor, AgentScope::Project) => PathBuf::from(".cursor/mcp.json"),
        (AgentTarget::Cursor, AgentScope::User) => home.join(".cursor/mcp.json"),
        (AgentTarget::Claude, AgentScope::Project) => PathBuf::from(".mcp.json"),
        // Claude Code's user-scope MCP registry lives in ~/.claude.json under
        // `projects.<cwd>.mcpServers.<name>`. This file is its main runtime
        // state file (also tracks numStartups, tipsHistory, etc.); we touch
        // only the nested vulcan entry via `mcp_server_pointer`.
        (AgentTarget::Claude, AgentScope::User) => home.join(".claude.json"),
        (AgentTarget::Codex, AgentScope::Project) => PathBuf::from(".codex/mcp.json"),
        (AgentTarget::Codex, AgentScope::User) => home.join(".codex/mcp.json"),
        (AgentTarget::Agentskills, AgentScope::Project) => PathBuf::from(".agents/mcp.json"),
        (AgentTarget::Agentskills, AgentScope::User) => home.join(".agents/mcp.json"),
    })
}

/// Render a JSON-pointer segment list as a dotted path, quoting segments that
/// contain characters outside `[A-Za-z0-9_-]`. Example: `["projects", "/Users/x",
/// "mcpServers", "vulcan"]` → `projects."/Users/x".mcpServers.vulcan`.
fn format_pointer(pointer: &[String]) -> String {
    pointer
        .iter()
        .map(|seg| {
            let needs_quote = seg.is_empty()
                || !seg
                    .chars()
                    .all(|c| c.is_ascii_alphanumeric() || c == '_' || c == '-');
            if needs_quote {
                format!("\"{seg}\"")
            } else {
                seg.clone()
            }
        })
        .collect::<Vec<_>>()
        .join(".")
}

/// Where the `vulcan` MCP server entry lives inside the JSON file at `mcp_config_path`.
/// Claude/user nests under `projects.<cwd>.mcpServers`; everyone else uses top-level
/// `mcpServers`. `project_path` is the absolute cwd to key into `projects` — only
/// consulted for Claude/user; ignored for the rest.
fn mcp_server_pointer(
    target: AgentTarget,
    scope: AgentScope,
    project_path: Option<&Path>,
) -> Result<Vec<String>, VulcanError> {
    match (target, scope) {
        (AgentTarget::Claude, AgentScope::User) => {
            let cwd = match project_path {
                Some(p) => p.to_path_buf(),
                None => std::env::current_dir()
                    .map_err(|e| VulcanError::io("CWD_LOOKUP_FAILED", e.to_string()))?,
            };
            let cwd_str = cwd.to_string_lossy().to_string();
            Ok(vec![
                "projects".to_string(),
                cwd_str,
                "mcpServers".to_string(),
                "vulcan".to_string(),
            ])
        }
        _ => Ok(vec!["mcpServers".to_string(), "vulcan".to_string()]),
    }
}

fn infer_agent_target() -> AgentTarget {
    if std::env::var("CLAUDECODE").is_ok()
        || std::env::var("CLAUDE_CODE").is_ok()
        || std::env::var("ANTHROPIC_MODEL").is_ok()
    {
        AgentTarget::Claude
    } else if std::env::var("CURSOR_TRACE_ID").is_ok()
        || std::env::var("CURSOR_AGENT").is_ok()
        || std::env::var("CURSOR_SESSION_ID").is_ok()
    {
        AgentTarget::Cursor
    } else {
        AgentTarget::Claude
    }
}

fn display_target_name(target: AgentTarget) -> &'static str {
    match target {
        AgentTarget::Claude => "Claude Code",
        AgentTarget::Cursor => "Cursor",
        AgentTarget::Codex => "Codex",
        AgentTarget::Agentskills => "your agentskills.io-compatible agent",
    }
}

fn mcp_install_command(target: AgentTarget, scope: AgentScope, dangerous: bool) -> String {
    let mut command = format!(
        "vulcan agent mcp install --target {} --scope {}",
        target_label(target),
        scope_label(scope)
    );
    if dangerous {
        command.push_str(" --dangerous");
    }
    command
}

fn mcp_doctor_command(target: AgentTarget, scope: AgentScope) -> String {
    format!(
        "vulcan agent mcp doctor --target {} --scope {} -o json",
        target_label(target),
        scope_label(scope)
    )
}

fn restart_instructions(target: AgentTarget) -> String {
    match target {
        AgentTarget::Claude => "restart Claude Code so it reloads MCP settings".to_string(),
        AgentTarget::Cursor => {
            "restart Cursor or start a new agent chat so it reloads MCP settings".to_string()
        }
        AgentTarget::Codex => "restart Codex so it reloads MCP settings".to_string(),
        AgentTarget::Agentskills => {
            "restart your agentskills.io-compatible agent so it reloads MCP settings".to_string()
        }
    }
}

fn inspect_mcp_config(path: &Path) -> (bool, bool, bool, Option<String>, Vec<String>) {
    inspect_mcp_config_at(path, &["mcpServers".to_string(), "vulcan".to_string()])
}

fn inspect_mcp_config_at(
    path: &Path,
    pointer: &[String],
) -> (bool, bool, bool, Option<String>, Vec<String>) {
    let mut warnings = Vec::new();
    let Ok(content) = fs::read_to_string(path) else {
        return (false, false, false, None, warnings);
    };
    let Ok(value) = serde_json::from_str::<serde_json::Value>(&content) else {
        warnings.push("MCP config exists but is not valid JSON.".to_string());
        return (false, false, false, None, warnings);
    };
    let server = find_server_at_pointer(&value, pointer);
    let configured = server.is_some();
    let args = server
        .and_then(|server| server.get("args"))
        .and_then(|args| args.as_array())
        .cloned()
        .unwrap_or_default();
    let dangerous_enabled = args
        .iter()
        .any(|arg| arg.as_str() == Some("--allow-dangerous"));
    let password_env_present = server
        .and_then(|server| server.get("env"))
        .and_then(|env| env.get("VULCAN_WALLET_PASSWORD"))
        .and_then(|value| value.as_str())
        .map(|value| !value.is_empty() && value != "your-password")
        .unwrap_or(false);
    let wallet_env_name = server
        .and_then(|server| server.get("env"))
        .and_then(|env| env.get("VULCAN_WALLET_NAME"))
        .and_then(|value| value.as_str())
        .filter(|value| !value.is_empty() && *value != "your-wallet")
        .map(str::to_string);
    if dangerous_enabled && !password_env_present {
        warnings.push(
            "Dangerous MCP is enabled but VULCAN_WALLET_PASSWORD is missing or placeholder."
                .to_string(),
        );
    }
    if dangerous_enabled && wallet_env_name.is_none() {
        warnings.push(
            "Dangerous MCP is enabled but VULCAN_WALLET_NAME is missing or placeholder; it will fall back to the default wallet."
                .to_string(),
        );
    }
    (
        configured,
        dangerous_enabled,
        password_env_present,
        wallet_env_name,
        warnings,
    )
}

fn mcp_warnings(dangerous: bool) -> Vec<String> {
    if dangerous {
        vec![
            "Dangerous MCP exposes live trading tools.".to_string(),
            "VULCAN_WALLET_NAME selects the wallet MCP unlocks; VULCAN_WALLET_PASSWORD may be stored in plaintext agent config after interactive install.".to_string(),
        ]
    } else {
        vec!["Read-only/paper MCP does not unlock a wallet for live signing.".to_string()]
    }
}

fn mcp_next_steps(target: AgentTarget, dangerous: bool) -> Vec<String> {
    let mut steps = vec![match target {
        AgentTarget::Claude => "Restart Claude Code so it reloads MCP settings.".to_string(),
        AgentTarget::Cursor => {
            "Restart Cursor or start a new agent chat so it reloads MCP settings.".to_string()
        }
        AgentTarget::Codex => "Restart Codex so it reloads MCP settings.".to_string(),
        AgentTarget::Agentskills => {
            "Restart your agentskills.io-compatible agent so it reloads MCP settings.".to_string()
        }
    }];
    if dangerous {
        steps.push(
            "If this was a dry run or print-config output, run the install interactively so Vulcan can validate and write the wallet password.".to_string(),
        );
        steps.push("After restart, live tools can sign through the MCP session wallet; Phoenix API auth may refresh automatically for higher rate limits.".to_string());
    }
    steps
}

pub fn install_skills(
    target: AgentTarget,
    scope: AgentScope,
    dir: Option<PathBuf>,
    force: bool,
    dry_run: bool,
) -> Result<AgentInstallResult, VulcanError> {
    let base_dir = skills_dir(target, scope, dir)?;
    let mut files = Vec::with_capacity(SKILLS.len());
    let mut installed = 0usize;
    let mut unchanged = 0usize;
    let mut skipped = 0usize;

    if !dry_run {
        fs::create_dir_all(&base_dir)
            .map_err(|e| VulcanError::io("SKILLS_DIR_CREATE_FAILED", e.to_string()))?;
    }

    for skill in SKILLS {
        let skill_dir = base_dir.join(skill.name);
        let skill_file = skill_dir.join("SKILL.md");
        let existing = fs::read_to_string(&skill_file).ok();

        let status = match existing {
            Some(current) if current == skill.contents => {
                unchanged += 1;
                "unchanged"
            }
            Some(_) if !force => {
                skipped += 1;
                "skipped"
            }
            Some(_) => {
                installed += 1;
                if !dry_run {
                    write_skill(&skill_dir, &skill_file, skill.contents)?;
                }
                if dry_run {
                    "would_update"
                } else {
                    "updated"
                }
            }
            None => {
                installed += 1;
                if !dry_run {
                    write_skill(&skill_dir, &skill_file, skill.contents)?;
                }
                if dry_run {
                    "would_create"
                } else {
                    "created"
                }
            }
        };

        files.push(SkillInstallStatus {
            skill: skill.name.to_string(),
            path: skill_file.to_string_lossy().to_string(),
            status: status.to_string(),
        });
    }

    Ok(AgentInstallResult {
        target: target_label(target).to_string(),
        scope: scope_label(scope).to_string(),
        directory: base_dir.to_string_lossy().to_string(),
        dry_run,
        force,
        installed,
        unchanged,
        skipped,
        files,
        mcp_config: mcp_config_snippet(),
        next_steps: next_steps(target),
    })
}

pub fn doctor(
    target: AgentTarget,
    scope: AgentScope,
    dir: Option<PathBuf>,
) -> Result<AgentDoctorResult, VulcanError> {
    let base_dir = skills_dir(target, scope, dir)?;
    let mut skills = Vec::with_capacity(SKILLS.len());
    let mut installed = 0usize;
    let mut missing = 0usize;

    for skill in SKILLS {
        let skill_file = base_dir.join(skill.name).join("SKILL.md");
        let existing = fs::read_to_string(&skill_file).ok();
        let is_installed = existing.is_some();
        if is_installed {
            installed += 1;
        } else {
            missing += 1;
        }
        skills.push(SkillDoctorStatus {
            skill: skill.name.to_string(),
            path: skill_file.to_string_lossy().to_string(),
            installed: is_installed,
            current: existing
                .as_deref()
                .map(|content| content == skill.contents)
                .unwrap_or(false),
        });
    }

    Ok(AgentDoctorResult {
        target: target_label(target).to_string(),
        scope: scope_label(scope).to_string(),
        directory: base_dir.to_string_lossy().to_string(),
        installed,
        missing,
        skills,
    })
}

pub fn default_skills_dir(target: AgentTarget, scope: AgentScope) -> Result<PathBuf, VulcanError> {
    match scope {
        AgentScope::Project => Ok(match target {
            AgentTarget::Claude => PathBuf::from(".claude/skills"),
            AgentTarget::Cursor => PathBuf::from(".cursor/skills"),
            AgentTarget::Codex => PathBuf::from(".codex/skills"),
            AgentTarget::Agentskills => PathBuf::from(".agents/skills"),
        }),
        AgentScope::User => {
            let home = dirs::home_dir().ok_or_else(|| {
                VulcanError::config("HOME_NOT_FOUND", "Could not determine home directory")
            })?;
            Ok(match target {
                AgentTarget::Claude => home.join(".claude/skills"),
                AgentTarget::Cursor => home.join(".cursor/skills"),
                AgentTarget::Codex => home.join(".codex/skills"),
                AgentTarget::Agentskills => home.join(".agents/skills"),
            })
        }
    }
}

fn skills_dir(
    target: AgentTarget,
    scope: AgentScope,
    dir: Option<PathBuf>,
) -> Result<PathBuf, VulcanError> {
    Ok(dir.unwrap_or(default_skills_dir(target, scope)?))
}

fn write_skill(skill_dir: &Path, skill_file: &Path, contents: &str) -> Result<(), VulcanError> {
    fs::create_dir_all(skill_dir)
        .map_err(|e| VulcanError::io("SKILL_DIR_CREATE_FAILED", e.to_string()))?;
    fs::write(skill_file, contents)
        .map_err(|e| VulcanError::io("SKILL_WRITE_FAILED", e.to_string()))?;
    Ok(())
}

fn target_label(target: AgentTarget) -> &'static str {
    match target {
        AgentTarget::Claude => "claude",
        AgentTarget::Cursor => "cursor",
        AgentTarget::Codex => "codex",
        AgentTarget::Agentskills => "agentskills",
    }
}

fn scope_label(scope: AgentScope) -> &'static str {
    match scope {
        AgentScope::User => "user",
        AgentScope::Project => "project",
    }
}

fn mcp_config_snippet() -> String {
    r#"{
  "mcpServers": {
    "vulcan": {
      "command": "vulcan",
      "args": ["mcp", "--allow-dangerous"],
      "env": {
        "VULCAN_WALLET_PASSWORD": "your-password"
      }
    }
  }
}"#
    .to_string()
}

fn next_steps(target: AgentTarget) -> Vec<String> {
    let restart = match target {
        AgentTarget::Claude => "Restart Claude Code so it reloads installed skills.",
        AgentTarget::Cursor => {
            "Restart Cursor or start a new agent chat so it reloads installed skills."
        }
        AgentTarget::Codex => "Restart Codex so it reloads installed skills.",
        AgentTarget::Agentskills => {
            "Restart your agentskills.io-compatible agent so it reloads installed skills."
        }
    };
    vec![
        restart.to_string(),
        "Run `vulcan agent doctor` to inspect installed skill files.".to_string(),
        "Run `vulcan agent-context` or read `vulcan://context` before trading.".to_string(),
    ]
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn skills_array_matches_repo_directory() {
        // Guard against the silent footgun where someone adds a `skills/<name>/SKILL.md`
        // to the repo but forgets to register it in the `SKILLS` array above. Without
        // this test, `cargo build` succeeds but `vulcan agent install` quietly omits the
        // new skill, the agent's local skill dir never receives it, and the agent gets
        // "Unknown skill: <name>" at runtime.
        let skills_root = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
            .parent()
            .expect("vulcan-lib has a workspace parent")
            .join("skills");
        let mut on_disk: Vec<String> = std::fs::read_dir(&skills_root)
            .unwrap_or_else(|e| panic!("read {} failed: {e}", skills_root.display()))
            .filter_map(|entry| entry.ok())
            .filter(|entry| entry.path().is_dir())
            .filter(|entry| entry.path().join("SKILL.md").exists())
            .filter_map(|entry| entry.file_name().to_str().map(str::to_string))
            .collect();
        on_disk.sort();

        let mut in_array: Vec<String> = SKILLS.iter().map(|s| s.name.to_string()).collect();
        in_array.sort();

        let missing_from_array: Vec<&String> =
            on_disk.iter().filter(|n| !in_array.contains(n)).collect();
        let missing_from_disk: Vec<&String> =
            in_array.iter().filter(|n| !on_disk.contains(n)).collect();

        assert!(
            missing_from_array.is_empty() && missing_from_disk.is_empty(),
            "SKILLS array drift detected.\n  Has SKILL.md on disk but missing from `SKILLS` array \
             (add `SkillSpec` entry for each, with `include_str!(...)`): {:?}\n  In `SKILLS` array \
             but missing on disk (rename or remove): {:?}",
            missing_from_array,
            missing_from_disk,
        );
    }

    #[test]
    fn project_paths_follow_agent_conventions() {
        assert_eq!(
            default_skills_dir(AgentTarget::Claude, AgentScope::Project).unwrap(),
            PathBuf::from(".claude/skills")
        );
        assert_eq!(
            default_skills_dir(AgentTarget::Cursor, AgentScope::Project).unwrap(),
            PathBuf::from(".cursor/skills")
        );
        assert_eq!(
            default_skills_dir(AgentTarget::Agentskills, AgentScope::Project).unwrap(),
            PathBuf::from(".agents/skills")
        );
    }

    #[test]
    fn dry_run_does_not_write_files() {
        let tmp = tempfile::tempdir().unwrap();
        let dir = tmp.path().join("skills");
        let result = install_skills(
            AgentTarget::Claude,
            AgentScope::Project,
            Some(dir.clone()),
            false,
            true,
        )
        .unwrap();

        assert!(!dir.exists());
        assert_eq!(result.installed, SKILLS.len());
        assert!(result.files.iter().all(|f| f.status == "would_create"));
    }

    #[test]
    fn install_is_idempotent_and_preserves_local_changes_without_force() {
        let tmp = tempfile::tempdir().unwrap();
        let dir = tmp.path().join("skills");

        let first = install_skills(
            AgentTarget::Claude,
            AgentScope::Project,
            Some(dir.clone()),
            false,
            false,
        )
        .unwrap();
        assert_eq!(first.installed, SKILLS.len());

        let second = install_skills(
            AgentTarget::Claude,
            AgentScope::Project,
            Some(dir.clone()),
            false,
            false,
        )
        .unwrap();
        assert_eq!(second.unchanged, SKILLS.len());

        fs::write(dir.join("vulcan/SKILL.md"), "local edit").unwrap();
        let third = install_skills(
            AgentTarget::Claude,
            AgentScope::Project,
            Some(dir.clone()),
            false,
            false,
        )
        .unwrap();
        assert_eq!(third.skipped, 1);

        let forced = install_skills(
            AgentTarget::Claude,
            AgentScope::Project,
            Some(dir.clone()),
            true,
            false,
        )
        .unwrap();
        assert_eq!(forced.installed, 1);
        assert_eq!(
            fs::read_to_string(dir.join("vulcan/SKILL.md")).unwrap(),
            SKILLS[0].contents
        );
    }

    #[test]
    fn mcp_config_defaults_to_read_only() {
        let config = mcp_print_config(AgentTarget::Cursor, false, None).unwrap();
        let server = &config.config["mcpServers"]["vulcan"];

        // `command` is the absolute path of the current vulcan binary (so
        // PATH-stripped launchers can still find it). The fallback when
        // current_exe() fails is the bare name. Either is acceptable.
        let cmd = server["command"]
            .as_str()
            .expect("command should be string");
        assert!(
            cmd == "vulcan" || cmd.ends_with("/vulcan") || cmd.contains("vulcan"),
            "unexpected command value: {cmd}"
        );
        assert_eq!(server["args"], serde_json::json!(["mcp"]));
        assert!(server.get("env").is_none());
    }

    #[test]
    fn mcp_config_dangerous_includes_placeholder_env_and_groups() {
        let config = mcp_print_config(
            AgentTarget::Cursor,
            true,
            Some(vec!["market".to_string(), "auth".to_string()]),
        )
        .unwrap();
        let server = &config.config["mcpServers"]["vulcan"];

        assert_eq!(
            server["args"],
            serde_json::json!(["mcp", "--allow-dangerous", "--groups", "market,auth"])
        );
        assert_eq!(server["env"]["VULCAN_WALLET_PASSWORD"], "your-password");
        assert_eq!(server["env"]["VULCAN_WALLET_NAME"], "your-wallet");
    }

    #[test]
    fn claude_user_install_writes_to_projects_map_in_claude_json() {
        // Claude Code reads MCP from ~/.claude.json under
        // `projects.<cwd>.mcpServers.<name>`. Older versions of our installer
        // wrote to `~/.claude/settings.json` which Claude Code ignores, leaving
        // agents unable to call vulcan_* tools after restart. Lock in the
        // correct path.
        let tmp = tempfile::tempdir().unwrap();
        let path = tmp.path().join(".claude.json");
        let project_path = PathBuf::from("/abs/path/to/workspace");

        // dangerous=false so the test doesn't need an interactive TTY; the
        // pointer-routing behavior is identical between read-only and dangerous.
        let result = mcp_install_with_project(
            None,
            AgentTarget::Claude,
            AgentScope::User,
            Some(path.clone()),
            Some(project_path.clone()),
            false,
            None,
            false,
            false,
        )
        .unwrap();
        assert!(result.installed);

        let written: serde_json::Value =
            serde_json::from_str(&fs::read_to_string(&path).unwrap()).unwrap();
        let projects = written
            .get("projects")
            .expect("expected `projects` key in ~/.claude.json");
        let entry = projects
            .get(project_path.to_string_lossy().as_ref())
            .expect("expected per-project entry for this cwd");
        let cmd = entry["mcpServers"]["vulcan"]["command"]
            .as_str()
            .expect("command should be string");
        assert!(
            cmd.contains("vulcan"),
            "vulcan server should be nested under projects.<cwd>.mcpServers, not at the top level (got command={cmd})"
        );
        // And NOT at top level — Claude Code only reads the per-project location.
        assert!(
            written.get("mcpServers").is_none(),
            "top-level mcpServers must not be created for Claude/user installs (Claude Code does not read it)"
        );
    }

    #[test]
    fn claude_user_install_preserves_unrelated_keys_and_other_projects() {
        // ~/.claude.json holds Claude Code's whole runtime state (numStartups,
        // tipsHistory, recent projects, other projects' mcpServers …). The
        // installer must merge non-destructively.
        let tmp = tempfile::tempdir().unwrap();
        let path = tmp.path().join(".claude.json");
        let our_project = PathBuf::from("/abs/path/to/this-workspace");
        let other_project = "/some/other/project";
        let existing = serde_json::json!({
            "numStartups": 42,
            "tipsHistory": { "shown": ["foo"] },
            "projects": {
                other_project: {
                    "mcpServers": {
                        "gmail": { "command": "/path/to/gmail-mcp" }
                    },
                    "lastUsed": "2026-05-01T00:00:00Z"
                }
            }
        });
        fs::write(&path, serde_json::to_vec_pretty(&existing).unwrap()).unwrap();

        let _ = mcp_install_with_project(
            None,
            AgentTarget::Claude,
            AgentScope::User,
            Some(path.clone()),
            Some(our_project.clone()),
            false,
            None,
            false,
            false,
        )
        .unwrap();

        let written: serde_json::Value =
            serde_json::from_str(&fs::read_to_string(&path).unwrap()).unwrap();
        assert_eq!(written["numStartups"], 42, "non-mcp top-level key wiped");
        assert_eq!(
            written["tipsHistory"]["shown"][0], "foo",
            "nested non-mcp key wiped"
        );
        assert_eq!(
            written["projects"][other_project]["mcpServers"]["gmail"]["command"],
            "/path/to/gmail-mcp",
            "another project's MCP entry was clobbered"
        );
        assert_eq!(
            written["projects"][other_project]["lastUsed"], "2026-05-01T00:00:00Z",
            "another project's non-mcp keys were clobbered"
        );
        let cmd = written["projects"][our_project.to_string_lossy().as_ref()]["mcpServers"]
            ["vulcan"]["command"]
            .as_str()
            .expect("our vulcan server should be present");
        assert!(
            cmd.contains("vulcan"),
            "vulcan server not inserted at the right path (got command={cmd})"
        );
    }

    #[test]
    fn non_claude_targets_still_use_top_level_mcp_servers() {
        // Backward-compat guard: Cursor / Codex / Agentskills must keep writing
        // `mcpServers.<name>` at the top of their respective config files.
        // Anything else breaks every existing user who has those agents wired up.
        for target in [
            AgentTarget::Cursor,
            AgentTarget::Codex,
            AgentTarget::Agentskills,
        ] {
            let tmp = tempfile::tempdir().unwrap();
            let path = tmp.path().join("mcp.json");
            let _ = mcp_install_with_project(
                None,
                target,
                AgentScope::User,
                Some(path.clone()),
                None,
                false,
                None,
                false,
                false,
            )
            .unwrap_or_else(|e| panic!("install failed for {target:?}: {e}"));
            let written: serde_json::Value =
                serde_json::from_str(&fs::read_to_string(&path).unwrap()).unwrap();
            let cmd = written["mcpServers"]["vulcan"]["command"]
                .as_str()
                .expect("vulcan entry should be present at top level");
            assert!(
                cmd.contains("vulcan"),
                "{target:?} should keep its top-level mcpServers schema (got command={cmd})"
            );
            assert!(
                written.get("projects").is_none(),
                "{target:?} should NOT use the projects-map layout"
            );
        }
    }

    #[test]
    fn mcp_install_merges_existing_config() {
        let tmp = tempfile::tempdir().unwrap();
        let path = tmp.path().join("mcp.json");
        fs::write(
            &path,
            serde_json::json!({
                "mcpServers": {
                    "other": { "command": "other" }
                }
            })
            .to_string(),
        )
        .unwrap();

        let result = mcp_install(
            None,
            AgentTarget::Cursor,
            AgentScope::Project,
            Some(path.clone()),
            false,
            None,
            false,
            false,
        )
        .unwrap();

        assert!(result.installed);
        let merged: serde_json::Value =
            serde_json::from_str(&fs::read_to_string(path).unwrap()).unwrap();
        assert_eq!(merged["mcpServers"]["other"]["command"], "other");
        let vulcan_cmd = merged["mcpServers"]["vulcan"]["command"]
            .as_str()
            .expect("vulcan should have been added");
        assert!(
            vulcan_cmd.contains("vulcan"),
            "unexpected merged vulcan command: {vulcan_cmd}"
        );
    }

    #[test]
    fn mcp_doctor_detects_dangerous_placeholder_password() {
        let tmp = tempfile::tempdir().unwrap();
        let path = tmp.path().join("mcp.json");
        fs::write(&path, mcp_config_value(true, None).to_string()).unwrap();

        let result = mcp_doctor(AgentTarget::Cursor, AgentScope::Project, Some(path)).unwrap();

        assert!(result.configured);
        assert!(result.dangerous_enabled);
        assert!(!result.password_env_present);
        assert!(result.wallet_env_name.is_none());
        assert!(!result.warnings.is_empty());
    }

    #[test]
    fn live_ready_returns_target_specific_resolution() {
        let tmp = tempfile::tempdir().unwrap();
        let path = tmp.path().join("mcp.json");
        let result =
            live_agent_readiness(AgentTarget::Claude, AgentScope::User, Some(path)).unwrap();

        assert_eq!(result.target, "claude");
        assert_eq!(result.recommended_execution_path, "not_ready");
        assert!(result.install_command.contains("--target claude"));
        assert!(result.restart_instructions.contains("Claude Code"));
    }

    #[test]
    fn mcp_redacts_real_password_in_returned_config() {
        let config =
            mcp_config_value_with_password(true, None, Some("super-secret-password"), Some("main"));
        let redacted = redact_mcp_config(config);

        assert_eq!(
            redacted["mcpServers"]["vulcan"]["env"]["VULCAN_WALLET_PASSWORD"],
            "[REDACTED]"
        );
        assert!(!serde_json::to_string(&redacted)
            .unwrap()
            .contains("super-secret-password"));
        assert_eq!(
            redacted["mcpServers"]["vulcan"]["env"]["VULCAN_WALLET_NAME"],
            "main"
        );
    }

    #[test]
    fn dry_run_dangerous_install_uses_placeholder_not_real_secret() {
        // Use Cursor (top-level mcpServers schema) to keep this test focused on
        // the placeholder-vs-real-secret behavior of dry-run installs, not the
        // per-target JSON layout. Claude/User's nested-projects layout is covered
        // by `claude_user_install_writes_to_projects_map_in_claude_json`.
        let tmp = tempfile::tempdir().unwrap();
        let path = tmp.path().join("mcp.json");
        let result = mcp_install(
            None,
            AgentTarget::Cursor,
            AgentScope::User,
            Some(path.clone()),
            true,
            None,
            false,
            true,
        )
        .unwrap();

        assert!(!result.installed);
        assert!(!path.exists());
        assert!(result.placeholder_password);
        assert!(!result.password_env_written);
        assert_eq!(
            result.config["mcpServers"]["vulcan"]["env"]["VULCAN_WALLET_PASSWORD"],
            "your-password"
        );
        assert_eq!(
            result.config["mcpServers"]["vulcan"]["env"]["VULCAN_WALLET_NAME"],
            "your-wallet"
        );
    }

    #[test]
    fn mcp_set_wallet_preserves_password_when_none_supplied() {
        let mut config =
            mcp_config_value_with_password(true, None, Some("super-secret-password"), Some("old"));
        let mutation = set_mcp_config_wallet(&mut config, "new-wallet", None).unwrap();
        assert_eq!(mutation.previous_wallet_name.as_deref(), Some("old"));
        // Already-dangerous config: no new flag added.
        assert!(!mutation.dangerous_newly_enabled);
        assert_eq!(
            config["mcpServers"]["vulcan"]["env"]["VULCAN_WALLET_NAME"],
            "new-wallet"
        );
        assert_eq!(
            config["mcpServers"]["vulcan"]["env"]["VULCAN_WALLET_PASSWORD"],
            "super-secret-password"
        );
        // --allow-dangerous still present exactly once.
        let args = config["mcpServers"]["vulcan"]["args"].as_array().unwrap();
        assert_eq!(
            args.iter()
                .filter(|v| v.as_str() == Some("--allow-dangerous"))
                .count(),
            1
        );
    }

    #[test]
    fn mcp_set_wallet_can_update_matching_password_env() {
        let mut config =
            mcp_config_value_with_password(true, None, Some("old-password"), Some("old"));
        let mutation =
            set_mcp_config_wallet(&mut config, "new-wallet", Some("new-password")).unwrap();
        assert_eq!(mutation.previous_wallet_name.as_deref(), Some("old"));
        assert!(!mutation.dangerous_newly_enabled);
        assert_eq!(
            config["mcpServers"]["vulcan"]["env"]["VULCAN_WALLET_NAME"],
            "new-wallet"
        );
        assert_eq!(
            config["mcpServers"]["vulcan"]["env"]["VULCAN_WALLET_PASSWORD"],
            "new-password"
        );
    }

    #[test]
    fn mcp_set_wallet_adds_allow_dangerous_when_missing() {
        // Start from a non-dangerous install: args is just ["mcp"].
        let mut config = mcp_config_value(false, None);
        let mutation = set_mcp_config_wallet(&mut config, "prodguy", Some("new-password")).unwrap();
        assert!(mutation.previous_wallet_name.is_none());
        assert!(mutation.dangerous_newly_enabled);
        let args = config["mcpServers"]["vulcan"]["args"].as_array().unwrap();
        // Canonical ordering: --allow-dangerous immediately after the leading subcommand.
        assert_eq!(args[0].as_str(), Some("mcp"));
        assert_eq!(args[1].as_str(), Some("--allow-dangerous"));
        assert_eq!(
            config["mcpServers"]["vulcan"]["env"]["VULCAN_WALLET_NAME"],
            "prodguy"
        );
        assert_eq!(
            config["mcpServers"]["vulcan"]["env"]["VULCAN_WALLET_PASSWORD"],
            "new-password"
        );
    }

    #[test]
    fn mcp_set_wallet_is_idempotent_for_allow_dangerous() {
        let mut config = mcp_config_value(false, None);
        let first = set_mcp_config_wallet(&mut config, "prodguy", Some("p1")).unwrap();
        assert!(first.dangerous_newly_enabled);
        let second = set_mcp_config_wallet(&mut config, "prodguy2", Some("p2")).unwrap();
        assert!(!second.dangerous_newly_enabled);
        let args = config["mcpServers"]["vulcan"]["args"].as_array().unwrap();
        assert_eq!(
            args.iter()
                .filter(|v| v.as_str() == Some("--allow-dangerous"))
                .count(),
            1
        );
    }

    #[test]
    fn mcp_set_wallet_inserts_allow_dangerous_before_groups() {
        // args ["mcp", "--groups", "trade"] — flag should land at index 1.
        let mut config = mcp_config_value(false, Some(vec!["trade".to_string()]));
        let mutation = set_mcp_config_wallet(&mut config, "prodguy", Some("password")).unwrap();
        assert!(mutation.dangerous_newly_enabled);
        let args = config["mcpServers"]["vulcan"]["args"].as_array().unwrap();
        assert_eq!(args[0].as_str(), Some("mcp"));
        assert_eq!(args[1].as_str(), Some("--allow-dangerous"));
        assert_eq!(args[2].as_str(), Some("--groups"));
        assert_eq!(args[3].as_str(), Some("trade"));
    }
}

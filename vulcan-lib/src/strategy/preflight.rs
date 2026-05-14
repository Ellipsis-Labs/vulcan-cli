//! Live-readiness pre-flight for strategy runners.
//!
//! Designed to fail fast *before* a run ID is allocated or a ledger is written.
//! Surfaces every blocker at once with a concrete remedy command for each, so an
//! agent reading the failure can act on it without guessing.

use crate::context::AppContext;
use crate::error::VulcanError;
use crate::output::TableRenderable;
use serde::Serialize;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum WalletSource {
    /// MCP session wallet (pre-decrypted at server start).
    SessionWallet,
    /// `-w <name>` CLI flag.
    CliFlag,
    /// `VULCAN_WALLET_NAME` env var promoted into ctx.wallet_override.
    EnvVar,
    /// Default wallet from `vulcan wallet set-default`.
    Default,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum PasswordSource {
    /// Available via MCP session wallet — no password prompt needed.
    SessionWallet,
    /// Read from VULCAN_WALLET_PASSWORD env var.
    EnvVar,
    /// Not available — would require interactive prompt the agent can't answer.
    Missing,
}

#[derive(Debug, Clone, Serialize)]
pub struct WalletInfo {
    pub name: String,
    pub address: String,
    pub source: WalletSource,
}

#[derive(Debug, Clone, Serialize)]
pub struct Blocker {
    pub code: &'static str,
    pub message: String,
    pub remedy: String,
}

#[derive(Debug, Clone, Serialize)]
pub struct McpTargetState {
    pub target: String,
    pub scope: String,
    pub configured: bool,
    pub dangerous_enabled: bool,
    pub password_env_present: bool,
    pub wallet_env_name: Option<String>,
}

#[derive(Debug, Clone, Serialize)]
pub struct LiveReadiness {
    pub wallet: Option<WalletInfo>,
    pub password_source: PasswordSource,
    pub api_authenticated: bool,
    pub trader_registered: bool,
    pub collateral_usdc: Option<f64>,
    pub available_to_withdraw_usdc: Option<f64>,
    pub open_positions: u32,
    pub open_orders: u32,
    pub risk_state: Option<String>,
    /// Per-target MCP setup state — does any target have a working live config?
    pub mcp_targets: Vec<McpTargetState>,
    /// True if at least one MCP target has dangerous_enabled + password_env_present.
    /// When true, the agent should switch to MCP-mode rather than try CLI tricks.
    pub mcp_live_ready_target: Option<String>,
    pub blockers: Vec<Blocker>,
    /// Human-readable one-line verdict.
    pub verdict: String,
    /// True iff `blockers.is_empty()` — strategies can proceed to live execution.
    pub ready: bool,
}

impl TableRenderable for LiveReadiness {
    fn render_table(&self) {
        println!(
            "Live Readiness: {}",
            if self.ready { "READY" } else { "NOT READY" }
        );
        println!("─────────────────────────────────────────");
        if let Some(w) = &self.wallet {
            println!(
                "  Wallet:        {} ({})  [source: {:?}]",
                w.name, w.address, w.source
            );
        } else {
            println!("  Wallet:        not resolved");
        }
        println!("  Password:      {:?}", self.password_source);
        println!("  API auth:      {}", self.api_authenticated);
        println!(
            "  Trader:        {}",
            if self.trader_registered {
                "registered"
            } else {
                "not registered"
            }
        );
        if let Some(v) = self.collateral_usdc {
            println!("  Collateral:    ${:.4}", v);
        }
        if let Some(v) = self.available_to_withdraw_usdc {
            println!("  Free margin:   ${:.4}", v);
        }
        println!(
            "  Positions / orders: {} / {}",
            self.open_positions, self.open_orders
        );
        if let Some(rs) = &self.risk_state {
            println!("  Risk state:    {}", rs);
        }
        println!("  Verdict:       {}", self.verdict);
        if !self.mcp_targets.is_empty() {
            println!();
            println!("MCP Targets");
            let rows: Vec<Vec<String>> = self
                .mcp_targets
                .iter()
                .map(|t| {
                    vec![
                        t.target.clone(),
                        t.scope.clone(),
                        bool_str(t.configured),
                        bool_str(t.dangerous_enabled),
                        bool_str(t.password_env_present),
                        t.wallet_env_name.clone().unwrap_or_else(|| "-".into()),
                    ]
                })
                .collect();
            crate::output::table::render_table(
                &[
                    "Target",
                    "Scope",
                    "Configured",
                    "Dangerous",
                    "Password env",
                    "Wallet env",
                ],
                rows,
            );
            if let Some(target) = &self.mcp_live_ready_target {
                println!(
                    "  Live signing is configured via MCP for target `{}`.",
                    target
                );
            }
        }
        if !self.blockers.is_empty() {
            println!();
            println!("Blockers ({}):", self.blockers.len());
            for b in &self.blockers {
                println!("  [{}] {}", b.code, b.message);
                println!("      remedy: {}", b.remedy);
            }
        }
    }
}

fn bool_str(b: bool) -> String {
    if b {
        "yes".into()
    } else {
        "no".into()
    }
}

impl LiveReadiness {
    /// Convert a NotReady readiness into a `VulcanError` with every blocker and remedy
    /// inlined in the message. Use this when a strategy refuses to launch.
    pub fn into_error(self) -> VulcanError {
        let wallet_line = self
            .wallet
            .as_ref()
            .map(|w| format!("Active wallet: {} ({})", w.name, w.address))
            .unwrap_or_else(|| "Active wallet: not resolved".to_string());
        let mut lines = vec![
            "Live strategy was not started and no ledger was created.".to_string(),
            wallet_line,
            format!("Verdict: {}", self.verdict),
            "Blockers:".to_string(),
        ];
        for b in &self.blockers {
            lines.push(format!("  • [{}] {}", b.code, b.message));
            lines.push(format!("      remedy: {}", b.remedy));
        }
        lines.push(
            "Run `vulcan strategy preflight -o json` to inspect this again without launching."
                .to_string(),
        );
        VulcanError::validation("LIVE_READINESS_FAILED", lines.join("\n"))
    }
}

/// Check whether the current context is ready to launch a live strategy run.
/// Read-only: makes API calls but does not allocate run IDs, write ledgers, or
/// submit transactions.
pub async fn check_live_readiness(ctx: &AppContext) -> LiveReadiness {
    let mut blockers = Vec::new();

    let wallet = resolve_wallet_info(ctx);
    if wallet.is_none() {
        blockers.push(Blocker {
            code: "NO_WALLET",
            message: "No wallet selected. Strategies need a wallet for live signing.".to_string(),
            remedy: "Pass `-w <name>`, set VULCAN_WALLET_NAME, or run `vulcan wallet set-default <NAME>`.".to_string(),
        });
    }

    let password_source = resolve_password_source(ctx);
    let mcp_targets = scan_mcp_targets();
    let mcp_live_ready_target = mcp_targets
        .iter()
        .find(|t| t.configured && t.dangerous_enabled && t.password_env_present)
        .map(|t| t.target.clone());

    if matches!(password_source, PasswordSource::Missing) {
        let remedy = if let Some(target) = &mcp_live_ready_target {
            format!(
                "MCP is already configured with the wallet password for target `{}`. \
                 This shell is running Vulcan in CLI mode, which does not inherit that password. \
                 SAFE remedy (preferred): restart your agent client so it talks to Vulcan through MCP — \
                 the existing MCP session will sign without prompting. \
                 ALTERNATE remedy: run the launch command yourself from a terminal where \
                 VULCAN_WALLET_PASSWORD is already exported. \
                 DO NOT scrape the password out of MCP config files — that is an unredacted-secret \
                 inspection and is forbidden by the runtime contract.",
                target
            )
        } else {
            "No MCP target has live signing configured. Either (a) run `vulcan agent live-ready \
             --target <claude|cursor|codex|agentskills> --scope user -o json` to wire MCP, or \
             (b) export VULCAN_WALLET_PASSWORD before launching the CLI from your own shell. \
             DO NOT read password values from any config file."
                .to_string()
        };
        blockers.push(Blocker {
            code: "WALLET_PASSWORD_REQUIRED",
            message: "Wallet password is required for live signing. Non-interactive agent shells cannot answer prompts.".to_string(),
            remedy,
        });
    }

    // API auth — informational. Public mode still works for reads; this just
    // tells the agent whether the higher rate budget is available.
    let api_authenticated = api_auth_active(ctx);

    // Margin status — uses ctx.wallet_override automatically via resolve_authority_name.
    let mut trader_registered = false;
    let mut collateral_usdc: Option<f64> = None;
    let mut available_to_withdraw_usdc: Option<f64> = None;
    let mut risk_state: Option<String> = None;
    match crate::commands::margin::execute_status_inner(ctx).await {
        Ok(margin) => {
            trader_registered = true;
            collateral_usdc = parse_money(&margin.collateral_balance);
            available_to_withdraw_usdc = parse_money(&margin.available_to_withdraw);
            risk_state = Some(margin.risk_state.clone());
            if let Some(cap) = collateral_usdc {
                if cap <= 0.0 {
                    blockers.push(Blocker {
                        code: "NO_COLLATERAL",
                        message: "Trader account has zero collateral. Live orders will fail margin checks."
                            .to_string(),
                        remedy: "Deposit USDC: `vulcan margin deposit --amount <USDC>` (you can also fund the wallet first via `vulcan wallet balance` to confirm SOL/USDC on-chain).".to_string(),
                    });
                }
            }
        }
        Err(err) => match err.code.as_str() {
            "NO_TRADER_ACCOUNT" => {
                blockers.push(Blocker {
                    code: "TRADER_NOT_REGISTERED",
                    message: "Wallet is not registered as a Phoenix trader.".to_string(),
                    remedy: "Run `vulcan account register --invite-code <CODE>` (ask for an invite code if needed).".to_string(),
                });
            }
            _ => {
                blockers.push(Blocker {
                    code: "MARGIN_STATUS_FETCH_FAILED",
                    message: format!(
                        "Could not fetch margin status: {} ({})",
                        err.message, err.code
                    ),
                    remedy: "Run `vulcan status` to check API/RPC connectivity, then `vulcan margin status -o json` for details.".to_string(),
                });
            }
        },
    }

    // Positions / orders — informational.
    let (open_positions, open_orders) = position_and_order_counts(ctx).await;

    let verdict = if blockers.is_empty() {
        match (collateral_usdc, available_to_withdraw_usdc) {
            (Some(_), Some(free)) if free < 1.0 => {
                "Ready, but free margin is below $1 USDC — live orders will likely be size-limited."
                    .to_string()
            }
            _ => "Ready for live strategy execution.".to_string(),
        }
    } else {
        format!("Not ready: {} blocker(s).", blockers.len())
    };
    let ready = blockers.is_empty();

    LiveReadiness {
        wallet,
        password_source,
        api_authenticated,
        trader_registered,
        collateral_usdc,
        available_to_withdraw_usdc,
        open_positions,
        open_orders,
        risk_state,
        mcp_targets,
        mcp_live_ready_target,
        blockers,
        verdict,
        ready,
    }
}

/// Scan the four well-known MCP targets and report which ones have live signing set up.
fn scan_mcp_targets() -> Vec<McpTargetState> {
    use crate::cli::agent::{AgentScope, AgentTarget};
    let targets = [
        AgentTarget::Claude,
        AgentTarget::Cursor,
        AgentTarget::Codex,
        AgentTarget::Agentskills,
    ];
    let mut out = Vec::with_capacity(targets.len());
    for target in targets {
        if let Ok(doctor) = crate::commands::agent::mcp_doctor(target, AgentScope::User, None) {
            out.push(McpTargetState {
                target: doctor.target,
                scope: doctor.scope,
                configured: doctor.configured,
                dangerous_enabled: doctor.dangerous_enabled,
                password_env_present: doctor.password_env_present,
                wallet_env_name: doctor.wallet_env_name,
            });
        }
    }
    out
}

// ── Helpers ────────────────────────────────────────────────────────────────

fn resolve_wallet_info(ctx: &AppContext) -> Option<WalletInfo> {
    if let Some(sw) = &ctx.session_wallet {
        return Some(WalletInfo {
            name: sw.wallet_name.clone(),
            address: sw.public_key.clone(),
            source: WalletSource::SessionWallet,
        });
    }
    // Determine which override flag is responsible — CLI flag vs env var. Both end up
    // in `wallet_override`, so we infer source from VULCAN_WALLET_NAME presence.
    if let Some(name) = ctx.wallet_override.as_ref() {
        if ctx.wallet_store.exists(name) {
            if let Ok(wallet) = ctx.wallet_store.load(name) {
                let env_match = std::env::var("VULCAN_WALLET_NAME")
                    .ok()
                    .map(|v| v.trim().eq(name.trim()))
                    .unwrap_or(false);
                let source = if env_match {
                    WalletSource::EnvVar
                } else {
                    WalletSource::CliFlag
                };
                return Some(WalletInfo {
                    name: name.clone(),
                    address: wallet.public_key,
                    source,
                });
            }
        }
    }
    let default_name = ctx.wallet_store.default_wallet().ok().flatten()?;
    let wallet = ctx.wallet_store.load(&default_name).ok()?;
    Some(WalletInfo {
        name: default_name,
        address: wallet.public_key,
        source: WalletSource::Default,
    })
}

fn resolve_password_source(ctx: &AppContext) -> PasswordSource {
    if ctx.session_wallet.is_some() {
        return PasswordSource::SessionWallet;
    }
    if std::env::var("VULCAN_WALLET_PASSWORD")
        .ok()
        .filter(|v| !v.is_empty())
        .is_some()
    {
        return PasswordSource::EnvVar;
    }
    PasswordSource::Missing
}

fn api_auth_active(ctx: &AppContext) -> bool {
    ctx.api_auth_error.is_none()
}

async fn position_and_order_counts(ctx: &AppContext) -> (u32, u32) {
    let positions = crate::commands::position::execute_list_inner(ctx)
        .await
        .map(|p| p.positions.len() as u32)
        .unwrap_or(0);
    let orders = crate::commands::trade::execute_orders_inner(ctx, None)
        .await
        .map(|o| o.orders.len() as u32)
        .unwrap_or(0);
    (positions, orders)
}

fn parse_money(value: &str) -> Option<f64> {
    let cleaned: String = value
        .chars()
        .filter(|ch| ch.is_ascii_digit() || matches!(ch, '.' | '-' | '+'))
        .collect();
    cleaned.parse::<f64>().ok()
}

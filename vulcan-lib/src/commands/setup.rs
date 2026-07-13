//! Interactive setup wizard — wallet creation, config, registration, and connectivity check.

use crate::cli::agent::{AgentScope, AgentTarget};
use crate::commands::account;
use crate::commands::agent;
use crate::config::VulcanConfig;
use crate::context::AppContext;
use crate::error::VulcanError;
use crate::wallet::{Wallet, WalletFile, WalletStore};
use solana_pubkey::Pubkey;
use std::io::{self, BufRead, Write};
use std::path::Path;
use std::str::FromStr;

// ── Prompt helpers ─────────────────────────────────────────────────────

fn prompt(msg: &str) -> Result<String, VulcanError> {
    print!("{msg}");
    io::stdout()
        .flush()
        .map_err(|e| VulcanError::io("FLUSH_FAILED", e.to_string()))?;
    let mut input = String::new();
    io::stdin()
        .lock()
        .read_line(&mut input)
        .map_err(|e| VulcanError::io("READ_FAILED", e.to_string()))?;
    Ok(input.trim().to_string())
}

fn prompt_yn(msg: &str, default: bool) -> Result<bool, VulcanError> {
    let hint = if default { "Y/n" } else { "y/N" };
    let input = prompt(&format!("{msg} [{hint}] "))?;
    Ok(match input.to_lowercase().as_str() {
        "y" | "yes" => true,
        "n" | "no" => false,
        _ => default,
    })
}

fn prompt_agent_target() -> Result<AgentTarget, VulcanError> {
    println!("  Which agent should receive the skills?");
    println!("    1) Cursor");
    println!("    2) Claude Code");
    println!("    3) OpenAI Codex");
    println!("    4) Generic agentskills.io directory");
    println!();

    let choice = prompt("  Choice [1]: ")?;
    Ok(match choice.as_str() {
        "" | "1" => AgentTarget::Cursor,
        "2" => AgentTarget::Claude,
        "3" => AgentTarget::Codex,
        "4" => AgentTarget::Agentskills,
        _ => {
            return Err(VulcanError::validation(
                "INVALID_CHOICE",
                "Please enter 1, 2, 3, or 4",
            ));
        }
    })
}

fn prompt_agent_scope() -> Result<AgentScope, VulcanError> {
    println!("  Install scope:");
    println!("    1) User — available across projects");
    println!("    2) Project — install into this repository");
    println!();

    let choice = prompt("  Choice [1]: ")?;
    Ok(match choice.as_str() {
        "" | "1" => AgentScope::User,
        "2" => AgentScope::Project,
        _ => {
            return Err(VulcanError::validation(
                "INVALID_CHOICE",
                "Please enter 1 or 2",
            ));
        }
    })
}

fn target_label_for_setup(target: AgentTarget) -> &'static str {
    match target {
        AgentTarget::Claude => "claude",
        AgentTarget::Cursor => "cursor",
        AgentTarget::Codex => "codex",
        AgentTarget::Agentskills => "agentskills",
    }
}

fn scope_label_for_setup(scope: AgentScope) -> &'static str {
    match scope {
        AgentScope::User => "user",
        AgentScope::Project => "project",
    }
}

fn prompt_password(msg: &str) -> Result<String, VulcanError> {
    // Use rpassword if available, otherwise fall back to regular prompt
    print!("{msg}");
    io::stdout()
        .flush()
        .map_err(|e| VulcanError::io("FLUSH_FAILED", e.to_string()))?;
    rpassword::read_password().map_err(|e| VulcanError::io("PASSWORD_READ_FAILED", e.to_string()))
}

fn step_header(n: u8, total: u8, label: &str) {
    println!("  [{n}/{total}] {label}");
    println!("  {}", "─".repeat(label.len() + 6));
}

// ── Banner ─────────────────────────────────────────────────────────────

fn print_banner() {
    let b = "\x1b[38;2;255;100;0m"; // Phoenix orange
    let bold = "\x1b[1m";
    let dim = "\x1b[2m";
    let r = "\x1b[0m";

    println!();
    println!("  {b}{bold}██╗   ██╗██╗   ██╗██╗      ██████╗ █████╗ ███╗   ██╗{r}");
    println!("  {b}{bold}██║   ██║██║   ██║██║     ██╔════╝██╔══██╗████╗  ██║{r}");
    println!("  {b}{bold}██║   ██║██║   ██║██║     ██║     ███████║██╔██╗ ██║{r}");
    println!("  {b}{bold}╚██╗ ██╔╝██║   ██║██║     ██║     ██╔══██║██║╚██╗██║{r}");
    println!("  {b}{bold} ╚████╔╝ ╚██████╔╝███████╗╚██████╗██║  ██║██║ ╚████║{r}");
    println!("  {b}{bold}  ╚═══╝   ╚═════╝ ╚══════╝ ╚═════╝╚═╝  ╚═╝╚═╝  ╚═══╝{r}");
    println!();
    println!("  {b}╭──────────────────────────────────────────────────────╮{r}");
    println!(
        "  {b}│{r}  {bold}Phoenix Perpetuals DEX{r} {dim}— AI-native CLI for Solana{r}  {b}│{r}"
    );
    println!("  {b}╰──────────────────────────────────────────────────────╯{r}");
    println!();
}

// ── Execution ──────────────────────────────────────────────────────────

pub async fn execute(ctx: &AppContext) -> Result<(), VulcanError> {
    print_banner();

    let total = 6;
    let wallet_store = &ctx.wallet_store;

    // ── Step 1: Wallet ─────────────────────────────────────────────────
    step_header(1, total, "Wallet");

    let wallets = wallet_store
        .list()
        .map_err(|e| VulcanError::io("WALLET_LIST_FAILED", e.to_string()))?;

    if !wallets.is_empty() {
        let default = wallet_store
            .default_wallet()
            .map_err(|e| VulcanError::io("WALLET_DEFAULT_FAILED", e.to_string()))?;
        let default_label = default.as_deref().unwrap_or("none");
        println!(
            "  ✓ {} wallet(s) found (default: {})",
            wallets.len(),
            default_label
        );
        for name in &wallets {
            let marker = if Some(name.as_str()) == default.as_deref() {
                "→"
            } else {
                " "
            };
            println!("    {marker} {name}");
        }
        println!();

        if !prompt_yn("  Add another wallet?", false)? {
            println!();
            return finish_setup(ctx, None, None).await;
        }
        println!();
    }

    let (wallet_name, address, wallet) = setup_wallet(wallet_store)?;

    // Set as default if it's the only wallet or user wants it
    let wallets_after = wallet_store
        .list()
        .map_err(|e| VulcanError::io("WALLET_LIST_FAILED", e.to_string()))?;
    if wallets_after.len() == 1 {
        wallet_store
            .set_default(&wallet_name)
            .map_err(|e| VulcanError::io("SET_DEFAULT_FAILED", e.to_string()))?;
        println!("  ✓ Set as default wallet");
    } else if prompt_yn("  Set as default wallet?", true)? {
        wallet_store
            .set_default(&wallet_name)
            .map_err(|e| VulcanError::io("SET_DEFAULT_FAILED", e.to_string()))?;
        println!("  ✓ Set as default wallet");
    }

    println!("    Address: {address}");
    println!();

    finish_setup(ctx, Some(wallet_name.clone()), Some((wallet_name, wallet))).await
}

fn setup_wallet(wallet_store: &WalletStore) -> Result<(String, String, Wallet), VulcanError> {
    let name = prompt("  Wallet name: ")?;
    if name.is_empty() {
        return Err(VulcanError::validation(
            "EMPTY_NAME",
            "Wallet name cannot be empty",
        ));
    }
    if wallet_store.exists(&name) {
        return Err(VulcanError::validation(
            "WALLET_EXISTS",
            format!("Wallet '{}' already exists", name),
        ));
    }

    println!();
    println!("  How would you like to set up your wallet?");
    println!("    1) Generate a new keypair");
    println!("    2) Import from base58 private key");
    println!("    3) Import from Solana keypair file (JSON)");
    println!();

    let choice = prompt("  Choice [1]: ")?;
    let choice = if choice.is_empty() {
        "1".to_string()
    } else {
        choice
    };

    let wallet = match choice.as_str() {
        "1" => {
            println!();
            println!("  Generating new keypair...");
            Wallet::generate().map_err(|e| VulcanError::internal("KEYGEN_FAILED", e.to_string()))?
        }
        "2" => {
            let key = prompt("  Enter base58 private key: ")?;
            Wallet::from_base58(&key)
                .map_err(|e| VulcanError::validation("INVALID_KEY", e.to_string()))?
        }
        "3" => {
            let path_str = prompt("  Path to keypair file: ")?;
            let path = Path::new(&path_str);
            if !path.exists() {
                return Err(VulcanError::validation(
                    "FILE_NOT_FOUND",
                    format!("File not found: {}", path_str),
                ));
            }
            Wallet::from_file(path)
                .map_err(|e| VulcanError::validation("INVALID_FILE", e.to_string()))?
        }
        _ => {
            return Err(VulcanError::validation(
                "INVALID_CHOICE",
                "Please enter 1, 2, or 3",
            ));
        }
    };

    let address = wallet.public_key.clone();

    // Encrypt with password
    println!();
    let password = prompt_password("  Set encryption password: ")?;
    if password.is_empty() {
        return Err(VulcanError::validation(
            "EMPTY_PASSWORD",
            "Password cannot be empty",
        ));
    }
    let password_confirm = prompt_password("  Confirm password: ")?;
    if password != password_confirm {
        return Err(VulcanError::validation(
            "PASSWORD_MISMATCH",
            "Passwords do not match",
        ));
    }

    let encrypted = wallet
        .encrypt(&password)
        .map_err(|e| VulcanError::internal("ENCRYPT_FAILED", e.to_string()))?;

    let wallet_file = WalletFile::local_encrypted(
        name.clone(),
        address.clone(),
        encrypted,
        chrono::Utc::now().to_rfc3339(),
    );

    wallet_store
        .save(&wallet_file)
        .map_err(|e| VulcanError::io("WALLET_SAVE_FAILED", e.to_string()))?;

    println!();
    if choice == "1" {
        println!("  ✓ Wallet created");
        println!();
        println!("  ⚠ Back up your wallet file. If lost, your funds cannot be recovered.");
        println!("    File: {}", wallet_store.wallet_path(&name).display());
    } else {
        println!("  ✓ Wallet imported");
    }

    Ok((name, address, wallet))
}

async fn finish_setup(
    ctx: &AppContext,
    registration_wallet: Option<String>,
    added_wallet_for_auth: Option<(String, Wallet)>,
) -> Result<(), VulcanError> {
    let total = 6;
    let wallet_store = &ctx.wallet_store;

    // ── Step 2: Config ─────────────────────────────────────────────────
    step_header(2, total, "Configuration");

    let config_path = VulcanConfig::path();
    if config_path.exists() {
        println!("  ✓ Config file found");
        println!("    Path: {}", config_path.display());
        println!();

        if prompt_yn("  Reconfigure?", false)? {
            println!();
            configure()?;
        }
    } else {
        println!("  ○ No config file found — using defaults");
        println!();

        if prompt_yn("  Customize configuration?", false)? {
            println!();
            configure()?;
        } else {
            // Save defaults
            let config = VulcanConfig::default();
            config
                .save()
                .map_err(|e| VulcanError::io("CONFIG_SAVE_FAILED", e.to_string()))?;
            println!("  ✓ Default config saved to {}", config_path.display());
        }
    }

    println!();

    let setup_ctx = AppContext::new(
        ctx.output_format,
        ctx.dry_run,
        true,
        ctx.verbose,
        ctx.watch,
        None,
        None,
        None,
    )
    .map_err(|e| VulcanError::config("INIT_FAILED", e.to_string()))?;

    // ── Step 3: Verify Connection ──────────────────────────────────────
    step_header(3, total, "Verify Connection");

    let config = &setup_ctx.config;
    println!("  ○ API: {}", config.network.api_url);
    println!("  ○ RPC: {}", config.network.rpc_url);
    println!("  ○ Phoenix API Auth: wallet-signed session optional");
    println!();
    println!("  Run `vulcan market list` to verify API connectivity.");

    println!();

    // ── Step 4: Agent Skills ───────────────────────────────────────────
    step_header(4, total, "Agent Skills");

    println!("  Vulcan can install bundled Agent Skills for better discovery in AI agents.");
    println!(
        "  The installer writes SKILL.md files only; it prints an MCP config snippet for you."
    );
    println!();

    if prompt_yn("  Install agent skills now?", true)? {
        let target = prompt_agent_target()?;
        let scope = prompt_agent_scope()?;
        let skills_dir = agent::default_skills_dir(target, scope)?;
        println!();
        println!("  Skills directory:");
        println!("    {}", skills_dir.display());
        println!();
        if prompt_yn("  Continue?", true)? {
            let result = agent::install_skills(target, scope, Some(skills_dir), false, false)?;
            println!("  ✓ Agent skills installed");
            println!(
                "    {} installed or updated, {} unchanged, {} skipped",
                result.installed, result.unchanged, result.skipped
            );
            if result.skipped > 0 {
                println!("    Some files had local changes. Re-run with `vulcan agent install --force` to overwrite.");
            }
            println!();
            println!("  MCP config snippet:");
            println!("{}", result.mcp_config);
            println!();
            if prompt_yn("  Install read-only/paper MCP config now?", true)? {
                let mcp = agent::mcp_install(
                    Some(&setup_ctx),
                    target,
                    scope,
                    None,
                    false,
                    None,
                    false,
                    false,
                )?;
                println!("  ✓ MCP config installed");
                println!("    {}", mcp.path);
                println!("    Restart your agent client so it reloads MCP settings.");
                println!();
            }
            println!("  For live agent trading, run:");
            println!(
                "    vulcan agent mcp install --target {} --scope {} --dangerous",
                target_label_for_setup(target),
                scope_label_for_setup(scope)
            );
            println!("  Then replace the VULCAN_WALLET_PASSWORD placeholder deliberately.");
            println!();
        } else {
            println!("  Skipped agent skill installation.");
            println!("  You can run `vulcan agent install --target cursor --scope user` later.");
        }
    } else {
        println!("  Skipped agent skill installation.");
        println!("  You can run `vulcan agent install --target cursor --scope user` later.");
    }

    println!();

    let has_wallet_for_api_login = registration_wallet.is_some()
        || setup_ctx
            .wallet_store
            .default_wallet()
            .ok()
            .flatten()
            .is_some();

    if has_wallet_for_api_login {
        let result = if let Some((wallet_name, wallet)) = added_wallet_for_auth.as_ref() {
            crate::commands::auth::login_with_wallet(&setup_ctx, wallet, Some(wallet_name.clone()))
                .await
        } else {
            crate::commands::auth::login_default_wallet(&setup_ctx).await
        };
        match result {
            Ok(ok) => {
                println!(
                    "  ✓ Phoenix API session stored at {}",
                    ok.status.session_path
                );
                println!("    {}", ok.note);
            }
            Err(err) => {
                println!(
                    "  ○ Skipped Phoenix API login ({}: {})",
                    err.code, err.message
                );
                println!("    Run `vulcan auth login` later if you need authenticated reads.");
            }
        }
        println!();
    }

    // ── Step 5: Trader Registration ────────────────────────────────────
    step_header(5, total, "Trader Registration");

    maybe_register_trader(&setup_ctx, registration_wallet.as_deref()).await?;

    println!();

    // ── Step 6: Next Steps ─────────────────────────────────────────────
    step_header(6, total, "Next Steps");

    let default_wallet = wallet_store.default_wallet().ok().flatten();

    if default_wallet.is_some() {
        if let Some(wallet_name) = registration_wallet.as_deref() {
            if default_wallet.as_deref() != Some(wallet_name) {
                println!("  ○ Make the newly added wallet active for future commands:");
                println!("    vulcan wallet set-default {wallet_name}");
                println!();
            }
        }
        println!("  ○ Deposit collateral:");
        println!("    vulcan margin deposit <AMOUNT> --yes");
        println!();
        println!("  ○ Check your setup:");
        println!("    vulcan status");
    } else {
        println!("  ○ Create or import a wallet:");
        println!("    vulcan wallet create --name <NAME>");
        println!("    vulcan wallet import --name <NAME> --format base58 <KEY>");
    }

    println!();
    println!("  ────────────────────────────────────────");
    println!("  ✓ Setup complete! You're ready to go.");
    println!();
    println!("  Quick start:");
    println!("    vulcan market list              Browse markets");
    println!("    vulcan market ticker SOL          Live price");
    println!("    vulcan trade market-buy SOL 1 --dry-run");
    println!();

    Ok(())
}

async fn maybe_register_trader(
    ctx: &AppContext,
    registration_wallet: Option<&str>,
) -> Result<(), VulcanError> {
    let fallback_wallet = ctx
        .wallet_store
        .default_wallet()
        .map_err(|e| VulcanError::io("WALLET_DEFAULT_FAILED", e.to_string()))?;
    let wallet_name = registration_wallet.map(str::to_string).or(fallback_wallet);

    let Some(wallet_name) = wallet_name else {
        println!("  ○ No default wallet configured — skipping registration.");
        return Ok(());
    };

    let wallet_file = ctx
        .wallet_store
        .load(&wallet_name)
        .map_err(|e| VulcanError::auth("WALLET_NOT_FOUND", e.to_string()))?;
    let authority = Pubkey::from_str(&wallet_file.public_key)
        .map_err(|e| VulcanError::validation("INVALID_PUBKEY", e.to_string()))?;

    if registration_wallet.is_some() {
        println!("  ○ Newly added wallet: {wallet_name}");
    } else {
        println!("  ○ Default wallet: {wallet_name}");
    }
    println!("    Address: {}", wallet_file.public_key);

    match account::trader_onboarding_status(ctx, &authority).await? {
        phoenix_rise::api::ReferralActivationTraderStatus::Activated => {
            let trader_key = phoenix_rise::api::TraderKey::new(authority);
            println!("  ✓ Trader account already registered and onboarded");
            println!("    Trader PDA: {}", trader_key.pda());
            return Ok(());
        }
        phoenix_rise::api::ReferralActivationTraderStatus::Registered => {
            let trader_key = phoenix_rise::api::TraderKey::new(authority);
            println!("  ○ Trader account is registered but still needs onboarding.");
            println!("    Trader PDA: {}", trader_key.pda());
        }
        phoenix_rise::api::ReferralActivationTraderStatus::Missing => {
            println!("  ○ Trader account is not registered.");
        }
    }

    println!("    Registration submits an on-chain transaction and requires SOL for fees.");
    println!();

    if !prompt_yn("  Register now?", true)? {
        println!("  Skipped registration.");
        return Ok(());
    }

    println!();
    println!("  If you have a referral code, enter it now.");
    println!("  Press Enter to skip and register without a code.");
    let code = prompt("  Referral code (optional): ")?;
    let referral_code = if code.is_empty() { None } else { Some(code) };

    let result = account::execute_register_wallet_inner(ctx, &wallet_name, referral_code).await?;
    println!("  ✓ Trader account registered");
    println!("    Trader PDA: {}", result.trader_pda);
    if let Some(sig) = result.tx_signature {
        println!("    Tx: {sig}");
    }

    Ok(())
}

fn configure() -> Result<(), VulcanError> {
    let mut config = VulcanConfig::load()
        .map_err(|e| VulcanError::config("CONFIG_LOAD_FAILED", e.to_string()))?;

    let rpc = prompt(&format!("  Solana RPC URL [{}]: ", config.network.rpc_url))?;
    if !rpc.is_empty() {
        config.network.rpc_url = rpc;
    }

    let api = prompt(&format!("  Phoenix API URL [{}]: ", config.network.api_url))?;
    if !api.is_empty() {
        config.network.api_url = api;
    }

    let slippage = prompt(&format!(
        "  Default slippage (bps) [{}]: ",
        config.trading.default_slippage_bps
    ))?;
    if !slippage.is_empty() {
        config.trading.default_slippage_bps = slippage.parse().map_err(|_| {
            VulcanError::validation("INVALID_SLIPPAGE", "Slippage must be a number")
        })?;
    }

    config
        .save()
        .map_err(|e| VulcanError::io("CONFIG_SAVE_FAILED", e.to_string()))?;

    println!("  ✓ Config saved to {}", VulcanConfig::path().display());

    Ok(())
}

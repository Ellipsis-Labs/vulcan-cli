//! Margin command execution.

use crate::cli::margin::MarginCommand;
use crate::context::AppContext;
use crate::error::VulcanError;
use crate::output::{render_success, TableRenderable};
use serde::Serialize;
use solana_pubkey::Pubkey;

// ── Result types ────────────────────────────────────────────────────────

#[derive(Debug, Serialize)]
pub struct MarginStatusResult {
    pub collateral_balance: String,
    pub effective_collateral: String,
    pub portfolio_value: String,
    pub unrealized_pnl: String,
    pub initial_margin: String,
    pub maintenance_margin: String,
    pub risk_state: String,
    pub risk_tier: String,
    pub available_to_withdraw: String,
    pub num_positions: usize,
    pub num_open_orders: usize,
}

impl TableRenderable for MarginStatusResult {
    fn render_table(&self) {
        println!("Margin Status:");
        println!("  Collateral balance: {}", self.collateral_balance);
        println!("  Effective collateral: {}", self.effective_collateral);
        println!("  Portfolio value: {}", self.portfolio_value);
        println!("  Unrealized PnL: {}", self.unrealized_pnl);
        println!("  Initial margin: {}", self.initial_margin);
        println!("  Maintenance margin: {}", self.maintenance_margin);
        println!("  Risk state: {}", self.risk_state);
        println!("  Risk tier: {}", self.risk_tier);
        println!("  Available to withdraw: {}", self.available_to_withdraw);
        println!("  Open positions: {}", self.num_positions);
        println!("  Open orders: {}", self.num_open_orders);
    }
}

#[derive(Debug, Serialize)]
pub struct DepositWithdrawResult {
    pub action: String,
    pub amount: f64,
    pub dry_run: bool,
    pub tx_signature: Option<String>,
    pub num_instructions: usize,
}

impl TableRenderable for DepositWithdrawResult {
    fn render_table(&self) {
        if self.dry_run {
            println!(
                "[DRY RUN] Would {} ${:.2} USDC ({} instructions)",
                self.action, self.amount, self.num_instructions
            );
        } else {
            println!("{}ed ${:.2} USDC", self.action, self.amount);
            if let Some(sig) = &self.tx_signature {
                println!("  Tx: {}", sig);
            }
        }
    }
}

#[derive(Debug, Serialize)]
pub struct TransferResult {
    pub from_subaccount: u8,
    pub to_subaccount: u8,
    pub amount: f64,
    pub dry_run: bool,
    pub tx_signature: Option<String>,
    pub num_instructions: usize,
}

impl TableRenderable for TransferResult {
    fn render_table(&self) {
        if self.dry_run {
            println!(
                "[DRY RUN] Would transfer ${:.2} USDC from subaccount {} to subaccount {}",
                self.amount, self.from_subaccount, self.to_subaccount
            );
        } else {
            println!(
                "Transferred ${:.2} USDC from subaccount {} to subaccount {}",
                self.amount, self.from_subaccount, self.to_subaccount
            );
        }
        if let Some(sig) = &self.tx_signature {
            println!("  Tx: {}", sig);
        }
    }
}

#[derive(Debug, Serialize)]
pub struct SweepResult {
    pub child_subaccount: u8,
    pub action: String,
    pub dry_run: bool,
    pub tx_signature: Option<String>,
    pub num_instructions: usize,
}

impl TableRenderable for SweepResult {
    fn render_table(&self) {
        if self.dry_run {
            println!(
                "[DRY RUN] Would {} subaccount {}",
                self.action, self.child_subaccount
            );
        } else {
            println!("{}d subaccount {}", self.action, self.child_subaccount);
        }
        if let Some(sig) = &self.tx_signature {
            println!("  Tx: {}", sig);
        }
    }
}

#[derive(Debug, Serialize)]
pub struct LeverageTiersResult {
    pub symbol: String,
    pub tiers: Vec<LeverageTierInfo>,
}

#[derive(Debug, Serialize)]
pub struct LeverageTierInfo {
    pub max_leverage: String,
    pub max_size: String,
}

impl TableRenderable for LeverageTiersResult {
    fn render_table(&self) {
        println!("Leverage tiers for {}:", self.symbol);
        let rows: Vec<Vec<String>> = self
            .tiers
            .iter()
            .map(|t| vec![t.max_leverage.clone(), t.max_size.clone()])
            .collect();
        crate::output::table::render_table(&["Max Leverage", "Max Size"], rows);
    }
}

// ── Helpers ─────────────────────────────────────────────────────────────

fn resolve_authority(ctx: &AppContext) -> Result<Pubkey, VulcanError> {
    crate::commands::trade::resolve_authority(ctx)
}

// ── Execution ───────────────────────────────────────────────────────────

pub async fn execute_status_inner(ctx: &AppContext) -> Result<MarginStatusResult, VulcanError> {
    let (_, authority) = crate::commands::trade::resolve_authority_name(ctx)?;

    let traders = ctx
        .http_client
        .get_traders(&authority)
        .await
        .map_err(|e| VulcanError::api("TRADERS_FETCH_FAILED", e.to_string()))?;

    let trader = traders
        .iter()
        .find(|t| t.trader_subaccount_index == 0)
        .ok_or_else(|| {
            VulcanError::api(
                "NO_TRADER_ACCOUNT",
                "No registered trader account found. Use 'vulcan account register' first.",
            )
        })?;

    Ok(project_margin_status(trader))
}

pub(crate) fn project_margin_status(
    trader: &phoenix_rise::types::TraderView,
) -> MarginStatusResult {
    let total_orders: usize = trader.limit_orders.values().map(|v| v.len()).sum();
    let eff_for_w: f64 = trader
        .effective_collateral_for_withdrawals
        .ui
        .parse()
        .unwrap_or(0.0);
    let im_for_w: f64 = trader
        .initial_margin_for_withdrawals
        .ui
        .parse()
        .unwrap_or(0.0);
    let available_to_withdraw = format!("{:.6}", (eff_for_w - im_for_w).max(0.0));
    MarginStatusResult {
        collateral_balance: trader.collateral_balance.ui.clone(),
        effective_collateral: trader.effective_collateral.ui.clone(),
        portfolio_value: trader.portfolio_value.ui.clone(),
        unrealized_pnl: trader.unrealized_pnl.ui.clone(),
        initial_margin: trader.initial_margin.ui.clone(),
        maintenance_margin: trader.maintenance_margin.ui.clone(),
        risk_state: format!("{:?}", trader.risk_state),
        risk_tier: format!("{:?}", trader.risk_tier),
        available_to_withdraw,
        num_positions: trader.positions.len(),
        num_open_orders: total_orders,
    }
}

pub async fn execute_deposit_withdraw_inner(
    ctx: &AppContext,
    amount: f64,
    is_deposit: bool,
) -> Result<DepositWithdrawResult, VulcanError> {
    let (wallet, authority, trader_pda) =
        crate::commands::trade::resolve_wallet_and_pda(ctx, None)?;

    let builder = ctx.tx_builder().await?;

    let ixs = if is_deposit {
        builder
            .build_deposit_funds(authority, trader_pda, amount)
            .map_err(|e| VulcanError::api("BUILD_DEPOSIT_FAILED", e.to_string()))?
    } else {
        builder
            .build_withdraw_funds(authority, trader_pda, amount)
            .map_err(|e| VulcanError::api("BUILD_WITHDRAW_FAILED", e.to_string()))?
    };

    let num_ixs = ixs.len();
    let sig = crate::commands::trade::send_or_dry_run(ctx, ixs, &wallet).await?;

    let action = if is_deposit { "deposit" } else { "withdraw" };
    Ok(DepositWithdrawResult {
        action: action.to_string(),
        amount,
        dry_run: ctx.dry_run,
        tx_signature: sig,
        num_instructions: num_ixs,
    })
}

pub async fn execute_transfer_inner(
    ctx: &AppContext,
    amount: f64,
    from: u8,
    to: u8,
) -> Result<TransferResult, VulcanError> {
    let (wallet, authority, _) = crate::commands::trade::resolve_wallet_and_pda(ctx, None)?;
    let builder = ctx.tx_builder().await?;

    let src_pda = phoenix_rise::types::TraderKey::derive_pda(&authority, 0, from);
    let dst_pda = phoenix_rise::types::TraderKey::derive_pda(&authority, 0, to);

    let ixs = builder
        .build_transfer_collateral(authority, src_pda, dst_pda, amount)
        .map_err(|e| VulcanError::api("BUILD_TRANSFER_FAILED", e.to_string()))?;

    let num_ixs = ixs.len();
    let sig = crate::commands::trade::send_or_dry_run(ctx, ixs, &wallet).await?;

    Ok(TransferResult {
        from_subaccount: from,
        to_subaccount: to,
        amount,
        dry_run: ctx.dry_run,
        tx_signature: sig,
        num_instructions: num_ixs,
    })
}

pub async fn execute_transfer_child_to_parent_inner(
    ctx: &AppContext,
    child: u8,
) -> Result<SweepResult, VulcanError> {
    let (wallet, authority, _) = crate::commands::trade::resolve_wallet_and_pda(ctx, None)?;
    let builder = ctx.tx_builder().await?;

    let child_pda = phoenix_rise::types::TraderKey::derive_pda(&authority, 0, child);
    let parent_pda = phoenix_rise::types::TraderKey::derive_pda(&authority, 0, 0);

    let ixs = builder
        .build_transfer_collateral_child_to_parent(authority, child_pda, parent_pda)
        .map_err(|e| VulcanError::api("BUILD_SWEEP_FAILED", e.to_string()))?;

    let num_ixs = ixs.len();
    let sig = crate::commands::trade::send_or_dry_run(ctx, ixs, &wallet).await?;

    Ok(SweepResult {
        child_subaccount: child,
        action: "sweep".to_string(),
        dry_run: ctx.dry_run,
        tx_signature: sig,
        num_instructions: num_ixs,
    })
}

pub async fn execute_sync_parent_to_child_inner(
    ctx: &AppContext,
    child: u8,
) -> Result<SweepResult, VulcanError> {
    let (wallet, authority, _) = crate::commands::trade::resolve_wallet_and_pda(ctx, None)?;
    let builder = ctx.tx_builder().await?;

    let parent_pda = phoenix_rise::types::TraderKey::derive_pda(&authority, 0, 0);
    let child_pda = phoenix_rise::types::TraderKey::derive_pda(&authority, 0, child);

    let ixs = builder
        .build_sync_parent_to_child(authority, parent_pda, child_pda)
        .map_err(|e| VulcanError::api("BUILD_SYNC_FAILED", e.to_string()))?;

    let num_ixs = ixs.len();
    let sig = crate::commands::trade::send_or_dry_run(ctx, ixs, &wallet).await?;

    Ok(SweepResult {
        child_subaccount: child,
        action: "sync parent-to-child".to_string(),
        dry_run: ctx.dry_run,
        tx_signature: sig,
        num_instructions: num_ixs,
    })
}

pub async fn execute_add_collateral_inner(
    ctx: &AppContext,
    symbol: &str,
    amount: f64,
) -> Result<TransferResult, VulcanError> {
    let symbol_upper = symbol.to_ascii_uppercase();

    // Fetch all trader views to find the isolated subaccount for this symbol
    let (wallet, authority, _) = crate::commands::trade::resolve_wallet_and_pda(ctx, None)?;

    let traders = ctx
        .http_client
        .get_traders(&authority)
        .await
        .map_err(|e| VulcanError::api("TRADERS_FETCH_FAILED", e.to_string()))?;

    // Find the isolated subaccount that holds a position in this symbol
    let iso_view = traders
        .iter()
        .find(|t| {
            t.trader_subaccount_index > 0
                && t.positions
                    .iter()
                    .any(|p| p.symbol.to_ascii_uppercase() == symbol_upper)
        })
        .ok_or_else(|| {
            VulcanError::validation(
                "NO_ISOLATED_POSITION",
                format!("No isolated position found for '{}'", symbol),
            )
        })?;

    let sub_idx = iso_view.trader_subaccount_index;
    let builder = ctx.tx_builder().await?;

    let src_pda = phoenix_rise::types::TraderKey::derive_pda(&authority, 0, 0);
    let dst_pda = phoenix_rise::types::TraderKey::derive_pda(&authority, 0, sub_idx);

    let ixs = builder
        .build_transfer_collateral(authority, src_pda, dst_pda, amount)
        .map_err(|e| VulcanError::api("BUILD_TRANSFER_FAILED", e.to_string()))?;

    let num_ixs = ixs.len();
    let sig = crate::commands::trade::send_or_dry_run(ctx, ixs, &wallet).await?;

    Ok(TransferResult {
        from_subaccount: 0,
        to_subaccount: sub_idx,
        amount,
        dry_run: ctx.dry_run,
        tx_signature: sig,
        num_instructions: num_ixs,
    })
}

pub async fn execute_leverage_tiers_inner(
    ctx: &AppContext,
    symbol: &str,
) -> Result<LeverageTiersResult, VulcanError> {
    let metadata = ctx.metadata().await?;
    let symbol_upper = symbol.to_ascii_uppercase();
    let market = metadata.get_market(&symbol_upper).ok_or_else(|| {
        VulcanError::validation("UNKNOWN_MARKET", format!("Unknown market: {}", symbol))
    })?;

    let tiers: Vec<LeverageTierInfo> = market
        .leverage_tiers
        .iter()
        .map(|t| {
            let max_size_str = {
                let size = t.max_size_base_lots as f64
                    / 10f64.powi(market.base_lots_decimals.max(0) as i32);
                format!("{:.4}", size)
            };
            LeverageTierInfo {
                max_leverage: format!("{:.1}x", t.max_leverage),
                max_size: max_size_str,
            }
        })
        .collect();

    Ok(LeverageTiersResult {
        symbol: symbol_upper,
        tiers,
    })
}

pub async fn execute(ctx: &AppContext, cmd: MarginCommand) -> Result<(), VulcanError> {
    match cmd {
        MarginCommand::Status => {
            let result = execute_status_inner(ctx).await?;
            render_success(ctx.output_format, &result, serde_json::Value::Null);

            if ctx.watch {
                let authority = resolve_authority(ctx)?;
                crate::watch::watch_loop(
                    ctx,
                    crate::watch::WatchKind::TraderState(authority),
                    || async {
                        let result = execute_status_inner(ctx).await?;
                        render_success(ctx.output_format, &result, serde_json::Value::Null);
                        Ok(())
                    },
                )
                .await?;
            }
            Ok(())
        }

        MarginCommand::Deposit { amount } => execute_deposit_withdraw(ctx, amount, true).await,

        MarginCommand::Withdraw { amount } => execute_deposit_withdraw(ctx, amount, false).await,

        MarginCommand::Transfer { amount, from, to } => {
            if !ctx.yes && !ctx.dry_run {
                return Err(VulcanError::validation(
                    "CONFIRMATION_REQUIRED",
                    "Pass --yes to confirm transfer, or --dry-run to simulate",
                ));
            }
            let result = execute_transfer_inner(ctx, amount, from, to).await?;
            render_success(
                ctx.output_format,
                &result,
                serde_json::json!({ "command": "margin transfer", "amount": amount, "dry_run": ctx.dry_run }),
            );
            Ok(())
        }

        MarginCommand::TransferChildToParent { child } => {
            if !ctx.yes && !ctx.dry_run {
                return Err(VulcanError::validation(
                    "CONFIRMATION_REQUIRED",
                    "Pass --yes to confirm sweep, or --dry-run to simulate",
                ));
            }
            let result = execute_transfer_child_to_parent_inner(ctx, child).await?;
            render_success(
                ctx.output_format,
                &result,
                serde_json::json!({ "command": "margin transfer-child-to-parent", "dry_run": ctx.dry_run }),
            );
            Ok(())
        }

        MarginCommand::SyncParentToChild { child } => {
            if !ctx.yes && !ctx.dry_run {
                return Err(VulcanError::validation(
                    "CONFIRMATION_REQUIRED",
                    "Pass --yes to confirm sync, or --dry-run to simulate",
                ));
            }
            let result = execute_sync_parent_to_child_inner(ctx, child).await?;
            render_success(
                ctx.output_format,
                &result,
                serde_json::json!({ "command": "margin sync-parent-to-child", "dry_run": ctx.dry_run }),
            );
            Ok(())
        }

        MarginCommand::AddCollateral { symbol, amount } => {
            if !ctx.yes && !ctx.dry_run {
                return Err(VulcanError::validation(
                    "CONFIRMATION_REQUIRED",
                    "Pass --yes to confirm collateral addition, or --dry-run to simulate",
                ));
            }
            let result = execute_add_collateral_inner(ctx, &symbol, amount).await?;
            render_success(
                ctx.output_format,
                &result,
                serde_json::json!({ "command": "margin add-collateral", "symbol": symbol, "amount": amount, "dry_run": ctx.dry_run }),
            );
            Ok(())
        }

        MarginCommand::LeverageTiers { symbol } => {
            let result = execute_leverage_tiers_inner(ctx, &symbol).await?;
            render_success(
                ctx.output_format,
                &result,
                serde_json::json!({ "command": "margin leverage-tiers", "symbol": symbol }),
            );
            Ok(())
        }
    }
}

async fn execute_deposit_withdraw(
    ctx: &AppContext,
    amount: f64,
    is_deposit: bool,
) -> Result<(), VulcanError> {
    if !ctx.yes && !ctx.dry_run {
        let action = if is_deposit { "deposit" } else { "withdrawal" };
        return Err(VulcanError::validation(
            "CONFIRMATION_REQUIRED",
            format!("Pass --yes to confirm {}, or --dry-run to simulate", action),
        ));
    }

    let result = execute_deposit_withdraw_inner(ctx, amount, is_deposit).await?;
    let action = &result.action;

    render_success(
        ctx.output_format,
        &result,
        serde_json::json!({ "command": format!("margin {}", action), "amount": amount, "dry_run": ctx.dry_run }),
    );
    Ok(())
}

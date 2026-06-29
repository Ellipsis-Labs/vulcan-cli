//! Account command execution.

use crate::cli::account::AccountCommand;
use crate::context::AppContext;
use crate::error::VulcanError;
use crate::output::{render_success, TableRenderable};
use serde::Serialize;
use solana_pubkey::Pubkey;
use std::str::FromStr;

// ── Result types ────────────────────────────────────────────────────────

#[derive(Debug, Serialize)]
pub struct RegisterResult {
    pub authority: String,
    pub trader_pda: String,
    pub dry_run: bool,
    pub tx_signature: Option<String>,
}

pub enum RegistrationCode {
    Access(String),
    Referral(String),
}

impl TableRenderable for RegisterResult {
    fn render_table(&self) {
        if self.dry_run {
            println!("[DRY RUN] Would register trader account:");
        } else {
            println!("Trader account registered:");
        }
        println!("  Authority: {}", self.authority);
        println!("  Trader PDA: {}", self.trader_pda);
        if let Some(sig) = &self.tx_signature {
            println!("  Tx: {}", sig);
        }
    }
}

#[derive(Debug, Serialize)]
pub struct AccountInfoResult {
    pub authority: String,
    pub trader_key: String,
    pub pda_index: u8,
    pub subaccount_index: u8,
    pub state: String,
    pub collateral_balance: String,
    pub portfolio_value: String,
    pub risk_state: String,
    pub risk_tier: String,
    pub num_positions: usize,
    pub num_open_orders: usize,
    pub max_positions: u64,
}

impl TableRenderable for AccountInfoResult {
    fn render_table(&self) {
        println!("Account Info:");
        println!("  Authority: {}", self.authority);
        println!("  Trader key: {}", self.trader_key);
        println!("  PDA index: {}", self.pda_index);
        println!("  Subaccount index: {}", self.subaccount_index);
        println!("  State: {}", self.state);
        println!("  Collateral: {}", self.collateral_balance);
        println!("  Portfolio value: {}", self.portfolio_value);
        println!("  Risk state: {}", self.risk_state);
        println!("  Risk tier: {}", self.risk_tier);
        println!("  Positions: {}/{}", self.num_positions, self.max_positions);
        println!("  Open orders: {}", self.num_open_orders);
    }
}

#[derive(Debug, Serialize)]
pub struct SubaccountListResult {
    pub authority: String,
    pub subaccounts: Vec<SubaccountInfo>,
}

#[derive(Debug, Serialize)]
pub struct SubaccountInfo {
    pub trader_key: String,
    pub pda_index: u8,
    pub subaccount_index: u8,
    pub state: String,
    pub collateral_balance: String,
    pub num_positions: usize,
    pub margin_type: String,
}

impl TableRenderable for SubaccountListResult {
    fn render_table(&self) {
        if self.subaccounts.is_empty() {
            println!("No subaccounts found.");
            return;
        }
        let rows: Vec<Vec<String>> = self
            .subaccounts
            .iter()
            .map(|s| {
                vec![
                    format!("{}", s.subaccount_index),
                    s.margin_type.clone(),
                    s.state.clone(),
                    s.collateral_balance.clone(),
                    s.num_positions.to_string(),
                ]
            })
            .collect();
        crate::output::table::render_table(
            &["Subaccount", "Type", "State", "Collateral", "Positions"],
            rows,
        );
    }
}

// ── Helpers ─────────────────────────────────────────────────────────────

fn resolve_authority(ctx: &AppContext) -> Result<(String, Pubkey), VulcanError> {
    crate::commands::trade::resolve_authority_name(ctx)
}

// ── Execution ───────────────────────────────────────────────────────────

pub async fn execute(ctx: &AppContext, cmd: AccountCommand) -> Result<(), VulcanError> {
    match cmd {
        AccountCommand::Register {
            access_code,
            referral_code,
            invite_code,
        } => {
            let (wallet_name, authority) = resolve_authority(ctx)?;
            let code = match (access_code, referral_code, invite_code) {
                (Some(code), None, None) | (None, None, Some(code)) => {
                    RegistrationCode::Access(code)
                }
                (None, Some(code), None) => RegistrationCode::Referral(code),
                _ => {
                    return Err(VulcanError::validation(
                        "REGISTRATION_CODE_CONFLICT",
                        "Provide exactly one of --access-code, --referral-code, or --invite-code",
                    ));
                }
            };
            let result = register_authority(ctx, &wallet_name, authority, code).await?;

            render_success(ctx.output_format, &result, serde_json::Value::Null);
            Ok(())
        }

        AccountCommand::Info => {
            let (_, authority) = resolve_authority(ctx)?;

            let traders =
                crate::commands::trader_state::fetch_computed_trader_views(ctx, &authority).await?;

            let trader = traders
                .iter()
                .find(|t| t.trader_subaccount_index == 0)
                .ok_or_else(|| {
                    VulcanError::api(
                        "NO_TRADER_ACCOUNT",
                        "No registered trader account found. Use 'vulcan account register' first.",
                    )
                })?;

            let result = AccountInfoResult {
                authority: trader.authority.clone(),
                trader_key: trader.trader_key.clone(),
                pda_index: trader.trader_pda_index,
                subaccount_index: trader.trader_subaccount_index,
                state: trader.state.clone(),
                collateral_balance: trader.collateral_balance.ui.clone(),
                portfolio_value: trader.portfolio_value.ui.clone(),
                risk_state: trader.risk_state.clone(),
                risk_tier: trader.risk_tier.clone(),
                num_positions: trader.positions.len(),
                num_open_orders: trader.num_open_limit_orders + trader.num_open_conditional_orders,
                max_positions: trader.max_positions,
            };

            render_success(ctx.output_format, &result, serde_json::Value::Null);
            Ok(())
        }

        AccountCommand::Subaccounts => {
            let (_, authority) = resolve_authority(ctx)?;

            let traders =
                crate::commands::trader_state::fetch_computed_trader_views(ctx, &authority).await?;

            let subaccounts: Vec<SubaccountInfo> = traders
                .iter()
                .map(|t| SubaccountInfo {
                    trader_key: t.trader_key.clone(),
                    pda_index: t.trader_pda_index,
                    subaccount_index: t.trader_subaccount_index,
                    state: t.state.clone(),
                    collateral_balance: t.collateral_balance.ui.clone(),
                    num_positions: t.positions.len(),
                    margin_type: if t.trader_subaccount_index == 0 {
                        "Cross".to_string()
                    } else {
                        "Isolated".to_string()
                    },
                })
                .collect();

            let result = SubaccountListResult {
                authority: authority.to_string(),
                subaccounts,
            };

            render_success(ctx.output_format, &result, serde_json::Value::Null);
            Ok(())
        }

        AccountCommand::CreateSubaccount {
            pda_index,
            subaccount_index,
        } => {
            if subaccount_index == 0 {
                return Err(VulcanError::validation(
                    "INVALID_SUBACCOUNT",
                    "Subaccount index 0 is reserved for cross-margin. Use 1+ for isolated.",
                ));
            }

            let (wallet_name, authority) = resolve_authority(ctx)?;
            let builder = ctx.tx_builder().await?;

            let state = crate::commands::trader_state::try_fetch_trader_state_snapshot(
                ctx, &authority, pda_index,
            )
            .await?;
            let state = state.ok_or_else(|| {
                VulcanError::api(
                    "NO_PARENT_TRADER_ACCOUNT",
                    "Register the cross-margin trader account before creating isolated subaccounts.",
                )
            })?;
            if !state.has_cross_margin_subaccount() {
                return Err(VulcanError::api(
                    "NO_PARENT_TRADER_ACCOUNT",
                    "Register the cross-margin trader account before creating isolated subaccounts.",
                ));
            }
            if state.find_subaccount(subaccount_index).is_some() {
                return Err(VulcanError::validation(
                    "SUBACCOUNT_EXISTS",
                    format!("Subaccount {} is already registered.", subaccount_index),
                ));
            }

            let mut ixs = builder
                .build_register_trader(authority, pda_index, subaccount_index)
                .map_err(|e| VulcanError::api("BUILD_REGISTER_FAILED", e.to_string()))?;
            let parent_pda = phoenix_rise::types::TraderKey::derive_pda(&authority, pda_index, 0);
            let child_pda =
                phoenix_rise::types::TraderKey::derive_pda(&authority, pda_index, subaccount_index);
            ixs.extend(
                builder
                    .build_sync_parent_to_child(authority, parent_pda, child_pda)
                    .map_err(|e| VulcanError::api("BUILD_SYNC_FAILED", e.to_string()))?,
            );

            let (wallet, _, _) =
                crate::commands::trade::resolve_wallet_and_pda(ctx, Some(&wallet_name)).await?;

            let sig = crate::commands::trade::send_or_dry_run(ctx, ixs, &wallet).await?;

            let trader_key = phoenix_rise::types::TraderKey::new_with_idx(
                authority,
                pda_index,
                subaccount_index,
            );
            let result = RegisterResult {
                authority: authority.to_string(),
                trader_pda: trader_key.pda().to_string(),
                dry_run: ctx.dry_run,
                tx_signature: sig,
            };

            render_success(ctx.output_format, &result, serde_json::Value::Null);
            Ok(())
        }
    }
}

// ── Inner functions for MCP ────────────────────────────────────────────

pub async fn execute_info_inner(ctx: &AppContext) -> Result<AccountInfoResult, VulcanError> {
    let (_, authority) = resolve_authority(ctx)?;

    let traders =
        crate::commands::trader_state::fetch_computed_trader_views(ctx, &authority).await?;

    let trader = traders
        .iter()
        .find(|t| t.trader_subaccount_index == 0)
        .ok_or_else(|| {
            VulcanError::api(
                "NO_TRADER_ACCOUNT",
                "No registered trader account found. Use 'vulcan account register' first.",
            )
        })?;

    Ok(AccountInfoResult {
        authority: trader.authority.clone(),
        trader_key: trader.trader_key.clone(),
        pda_index: trader.trader_pda_index,
        subaccount_index: trader.trader_subaccount_index,
        state: trader.state.clone(),
        collateral_balance: trader.collateral_balance.ui.clone(),
        portfolio_value: trader.portfolio_value.ui.clone(),
        risk_state: trader.risk_state.clone(),
        risk_tier: trader.risk_tier.clone(),
        num_positions: trader.positions.len(),
        num_open_orders: trader.num_open_limit_orders + trader.num_open_conditional_orders,
        max_positions: trader.max_positions,
    })
}

pub async fn execute_register_inner(
    ctx: &AppContext,
    invite_code: &str,
) -> Result<RegisterResult, VulcanError> {
    let (wallet_name, authority) = resolve_authority(ctx)?;
    register_authority(
        ctx,
        &wallet_name,
        authority,
        RegistrationCode::Access(invite_code.to_string()),
    )
    .await
}

pub async fn execute_register_with_code_inner(
    ctx: &AppContext,
    code: RegistrationCode,
) -> Result<RegisterResult, VulcanError> {
    let (wallet_name, authority) = resolve_authority(ctx)?;
    register_authority(ctx, &wallet_name, authority, code).await
}

pub async fn execute_register_wallet_with_code_inner(
    ctx: &AppContext,
    wallet_name: &str,
    code: RegistrationCode,
) -> Result<RegisterResult, VulcanError> {
    let wallet_file = ctx
        .wallet_store
        .load(wallet_name)
        .map_err(|e| VulcanError::auth("WALLET_NOT_FOUND", e.to_string()))?;
    let authority = Pubkey::from_str(&wallet_file.public_key)
        .map_err(|e| VulcanError::validation("INVALID_PUBKEY", e.to_string()))?;
    register_authority(ctx, wallet_name, authority, code).await
}

async fn is_cross_margin_registered(
    ctx: &AppContext,
    authority: &Pubkey,
) -> Result<bool, VulcanError> {
    Ok(
        crate::commands::trader_state::try_fetch_trader_state_snapshot(ctx, authority, 0)
            .await?
            .is_some_and(|state| state.has_cross_margin_subaccount()),
    )
}

async fn activate_registration_code(
    ctx: &AppContext,
    authority: &Pubkey,
    code: &RegistrationCode,
) -> Result<(), VulcanError> {
    match code {
        RegistrationCode::Access(code) => {
            ctx.http_client
                .invite()
                .activate_invite(authority, code.as_str())
                .await
                .map_err(|e| VulcanError::api("REGISTER_API_FAILED", e.to_string()))?;
        }
        RegistrationCode::Referral(referral_code) => {
            ctx.http_client
                .invite()
                .activate_referral(authority, referral_code.as_str())
                .await
                .map_err(|e| VulcanError::api("REGISTER_API_FAILED", e.to_string()))?;
        }
    }
    Ok(())
}

async fn register_authority(
    ctx: &AppContext,
    wallet_name: &str,
    authority: Pubkey,
    code: RegistrationCode,
) -> Result<RegisterResult, VulcanError> {
    let already_registered = is_cross_margin_registered(ctx, &authority).await?;

    let sig = if already_registered {
        eprintln!("Trader account already registered, skipping registration.");
        None
    } else if ctx.dry_run {
        None
    } else {
        activate_registration_code(ctx, &authority, &code).await?;

        if is_cross_margin_registered(ctx, &authority).await? {
            eprintln!(
                "Trader account registered after code activation; skipping on-chain transaction."
            );
            None
        } else {
            let builder = ctx.tx_builder().await?;
            let ixs = builder
                .build_register_trader(authority, 0, 0)
                .map_err(|e| VulcanError::api("BUILD_REGISTER_FAILED", e.to_string()))?;

            let (wallet, _, _) =
                crate::commands::trade::resolve_wallet_and_pda(ctx, Some(wallet_name)).await?;

            match crate::commands::trade::send_or_dry_run(ctx, ixs, &wallet).await {
                Ok(sig) => sig,
                Err(err)
                    if err.category == crate::error::ErrorCategory::TxFailed
                        && is_cross_margin_registered(ctx, &authority).await? =>
                {
                    eprintln!(
                        "Registration transaction failed, but trader account is now registered."
                    );
                    None
                }
                Err(err) => return Err(err),
            }
        }
    };

    let trader_key = phoenix_rise::types::TraderKey::new(authority);
    Ok(RegisterResult {
        authority: authority.to_string(),
        trader_pda: trader_key.pda().to_string(),
        dry_run: ctx.dry_run,
        tx_signature: sig,
    })
}

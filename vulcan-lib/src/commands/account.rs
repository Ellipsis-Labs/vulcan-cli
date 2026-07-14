//! Account command execution.

use crate::cli::account::AccountCommand;
use crate::context::AppContext;
use crate::error::VulcanError;
use crate::output::{render_success, TableRenderable};
use crate::wallet::ResolvedSigner;
use phoenix_rise::accounts::owned::Permission;
use phoenix_rise::accounts::permission::TRADER_ONBOARDING_PERMISSION;
use phoenix_rise::api::{
    fetch_referral_activation_trader_status, ActivateReferralTxRequest,
    ReferralActivationTraderStatus, TraderKey,
};
use phoenix_rise::ix::onboard_trader_delegated::{
    create_onboard_trader_delegated_ix, OnboardTraderDelegatedParams,
};
use phoenix_rise::ix::register_trader::{create_register_trader_ix, RegisterTraderParams};
use serde::Serialize;
use solana_keychain::SignTransactionResult;
use solana_pubkey::Pubkey;
use solana_rpc_client_api::config::RpcSimulateTransactionConfig;
use std::str::FromStr;

const CROSS_MARGIN_MAX_POSITIONS: u32 = 128;
/// Used when the user registers without providing a referral code; the
/// activate-tx endpoint requires one.
const DEFAULT_REFERRAL_CODE: &str = "VULCAN";
const LAMPORTS_PER_SOL: u64 = 1_000_000_000;
const TRADER_HEADER_LEN: usize = 224;
const TRADER_POSITION_MAP_PREFIX_LEN: usize = 16;
const TRADER_POSITION_ENTRY_LEN: usize = 40;

// ── Result types ────────────────────────────────────────────────────────

#[derive(Debug, Serialize)]
pub struct RegisterResult {
    pub authority: String,
    pub trader_pda: String,
    pub dry_run: bool,
    pub tx_signature: Option<String>,
}

pub(crate) async fn trader_onboarding_status(
    ctx: &AppContext,
    authority: &Pubkey,
) -> Result<ReferralActivationTraderStatus, VulcanError> {
    let trader = TraderKey::new(*authority);
    let rpc_client = ctx.rpc_client_async();
    fetch_referral_activation_trader_status(&rpc_client, &trader.pda())
        .await
        .map_err(|e| VulcanError::network("TRADER_STATUS_FAILED", e.to_string()))
}

fn trader_account_size(max_positions: u32) -> Result<usize, VulcanError> {
    let position_entries_len = TRADER_POSITION_ENTRY_LEN
        .checked_mul(max_positions as usize)
        .ok_or_else(|| {
            VulcanError::validation(
                "TRADER_ACCOUNT_SIZE_OVERFLOW",
                "Trader account position map size overflowed",
            )
        })?;
    TRADER_HEADER_LEN
        .checked_add(TRADER_POSITION_MAP_PREFIX_LEN)
        .and_then(|prefix_len| prefix_len.checked_add(position_entries_len))
        .ok_or_else(|| {
            VulcanError::validation(
                "TRADER_ACCOUNT_SIZE_OVERFLOW",
                "Trader account size overflowed",
            )
        })
}

fn format_sol_lamports(lamports: u64) -> String {
    let whole = lamports / LAMPORTS_PER_SOL;
    let fractional = lamports % LAMPORTS_PER_SOL;
    let mut value = format!("{whole}.{fractional:09}");
    while value.contains('.') && value.ends_with('0') {
        value.pop();
    }
    if value.ends_with('.') {
        value.push('0');
    }
    value
}

async fn ensure_sol_for_cross_trader_rent(
    ctx: &AppContext,
    payer: Pubkey,
    max_positions: u32,
) -> Result<(), VulcanError> {
    let account_size = trader_account_size(max_positions)?;
    let rpc = ctx.rpc_client_async();
    let required_lamports = rpc
        .get_minimum_balance_for_rent_exemption(account_size)
        .await
        .map_err(|e| VulcanError::network("REGISTRATION_RENT_FETCH_FAILED", e.to_string()))?;
    let balance_lamports = rpc
        .get_balance(&payer)
        .await
        .map_err(|e| VulcanError::network("RPC_BALANCE_FAILED", e.to_string()))?;

    if balance_lamports < required_lamports {
        return Err(VulcanError::validation(
            "INSUFFICIENT_SOL_FOR_REGISTRATION",
            format!(
                "Wallet {payer} has {} SOL, but registering the cross-margin trader account needs at least {} SOL for rent exemption ({} bytes). Fund the wallet with SOL and retry.",
                format_sol_lamports(balance_lamports),
                format_sol_lamports(required_lamports),
                account_size,
            ),
        ));
    }

    Ok(())
}

async fn simulate_onboarding_transaction(
    ctx: &AppContext,
    ixs: &[solana_sdk::instruction::Instruction],
    fee_payer: Pubkey,
) -> Result<(), VulcanError> {
    let tx = solana_sdk::transaction::Transaction::new_with_payer(ixs, Some(&fee_payer));
    let result = ctx
        .rpc_client_async()
        .simulate_transaction_with_config(
            &tx,
            RpcSimulateTransactionConfig {
                sig_verify: false,
                replace_recent_blockhash: true,
                commitment: Some(solana_commitment_config::CommitmentConfig::confirmed()),
                ..RpcSimulateTransactionConfig::default()
            },
        )
        .await
        .map_err(|e| VulcanError::network("REGISTRATION_SIMULATION_RPC_FAILED", e.to_string()))?;

    if let Some(err) = result.value.err {
        let logs = result
            .value
            .logs
            .unwrap_or_default()
            .into_iter()
            .rev()
            .take(8)
            .collect::<Vec<_>>()
            .into_iter()
            .rev()
            .collect::<Vec<_>>();
        let logs = if logs.is_empty() {
            String::new()
        } else {
            format!(" Logs: {}", logs.join(" | "))
        };
        return Err(VulcanError::tx_failed(
            "REGISTRATION_SIMULATION_FAILED",
            format!(
                "Registration transaction simulation failed for payer {fee_payer}: {err:?}.{logs}"
            ),
        ));
    }

    Ok(())
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
        AccountCommand::Register { referral_code } => {
            let (wallet_name, authority) = resolve_authority(ctx)?;
            let result = register_authority(ctx, &wallet_name, authority, referral_code).await?;

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
            let parent_pda = phoenix_rise::api::TraderKey::derive_pda(&authority, pda_index, 0);
            let child_pda =
                phoenix_rise::api::TraderKey::derive_pda(&authority, pda_index, subaccount_index);
            ixs.extend(
                builder
                    .build_sync_parent_to_child(authority, parent_pda, child_pda)
                    .map_err(|e| VulcanError::api("BUILD_SYNC_FAILED", e.to_string()))?,
            );

            let (wallet, _, _) =
                crate::commands::trade::resolve_wallet_and_pda(ctx, Some(&wallet_name)).await?;

            let sig = crate::commands::trade::send_or_dry_run(ctx, ixs, &wallet).await?;

            let trader_key =
                phoenix_rise::api::TraderKey::new_with_idx(authority, pda_index, subaccount_index);
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
    referral_code: Option<String>,
) -> Result<RegisterResult, VulcanError> {
    let (wallet_name, authority) = resolve_authority(ctx)?;
    register_authority(ctx, &wallet_name, authority, referral_code).await
}

pub async fn execute_register_wallet_inner(
    ctx: &AppContext,
    wallet_name: &str,
    referral_code: Option<String>,
) -> Result<RegisterResult, VulcanError> {
    let wallet_file = ctx
        .wallet_store
        .load(wallet_name)
        .map_err(|e| VulcanError::auth("WALLET_NOT_FOUND", e.to_string()))?;
    let authority = Pubkey::from_str(&wallet_file.public_key)
        .map_err(|e| VulcanError::validation("INVALID_PUBKEY", e.to_string()))?;
    register_authority(ctx, wallet_name, authority, referral_code).await
}

async fn sign_onboarding_transaction_for_api(
    ctx: &AppContext,
    wallet: &ResolvedSigner,
    ixs: Vec<solana_sdk::instruction::Instruction>,
    trader_onboarder: Pubkey,
) -> Result<(String, String, Pubkey), VulcanError> {
    let signer = wallet.signer()?;
    let fee_payer = signer.pubkey();
    if fee_payer != wallet.authority {
        return Err(VulcanError::auth(
            "SIGNER_PUBKEY_MISMATCH",
            format!(
                "Signer pubkey {} does not match active wallet authority {}",
                fee_payer, wallet.authority
            ),
        ));
    }

    simulate_onboarding_transaction(ctx, &ixs, fee_payer).await?;

    let recent_blockhash = ctx
        .rpc_client()
        .get_latest_blockhash()
        .map_err(|e| VulcanError::network("BLOCKHASH_FAILED", e.to_string()))?;
    let mut tx = solana_sdk::transaction::Transaction::new_with_payer(&ixs, Some(&fee_payer));
    tx.message.recent_blockhash = recent_blockhash;

    let signed = signer
        .sign_transaction(&mut tx)
        .await
        .map_err(|e| VulcanError::auth("TX_SIGN_FAILED", e.to_string()))?;
    if matches!(signed, SignTransactionResult::Partial(_)) {
        validate_partial_onboarding_signatures(&tx, &trader_onboarder)?;
    }

    let (transaction, _) = signed.into_signed_transaction();
    Ok((transaction, recent_blockhash.to_string(), fee_payer))
}

/// The onboard instruction lists the API's trader onboarder as a required
/// signer; the API fills that slot server-side after submission, so a locally
/// partial signature is the expected state. Only a missing signature for any
/// other key is fatal.
fn validate_partial_onboarding_signatures(
    tx: &solana_sdk::transaction::Transaction,
    trader_onboarder: &Pubkey,
) -> Result<(), VulcanError> {
    let num_required = tx.message.header.num_required_signatures as usize;
    for (position, key) in tx
        .message
        .account_keys
        .iter()
        .take(num_required)
        .enumerate()
    {
        let missing = tx
            .signatures
            .get(position)
            .is_none_or(|sig| *sig == solana_sdk::signature::Signature::default());
        if missing && key != trader_onboarder {
            return Err(VulcanError::auth(
                "PARTIAL_SIGNATURE",
                format!(
                    "Onboarding transaction is missing a signature for {key}, which is not the API trader onboarder {trader_onboarder}."
                ),
            ));
        }
    }
    Ok(())
}

fn parse_api_pubkey(value: &str, field: &str) -> Result<Pubkey, VulcanError> {
    Pubkey::from_str(value).map_err(|e| {
        VulcanError::api(
            "REGISTER_API_INVALID_PUBKEY",
            format!("Invalid {field} pubkey: {e}"),
        )
    })
}

fn parse_api_pubkey_list(values: &[String], field: &str) -> Result<Vec<Pubkey>, VulcanError> {
    values
        .iter()
        .map(|value| parse_api_pubkey(value, field))
        .collect()
}

async fn fetch_permission_account(
    ctx: &AppContext,
    permission_account: &Pubkey,
) -> Result<Permission, VulcanError> {
    let rpc = ctx.rpc_client_async();
    let account = rpc
        .get_account_with_commitment(permission_account, rpc.commitment())
        .await
        .map_err(|e| VulcanError::network("PERMISSION_FETCH_FAILED", e.to_string()))?
        .value
        .ok_or_else(|| {
            VulcanError::api(
                "PERMISSION_ACCOUNT_MISSING",
                format!("Permission account does not exist: {permission_account}"),
            )
        })?;
    Permission::try_from_account_bytes(&account.data)
        .map_err(|e| VulcanError::api("PERMISSION_DECODE_FAILED", e.to_string()))
}

fn validate_permission(
    permission: &Permission,
    expected_risk_authority: Pubkey,
    expected_onboarder: Pubkey,
) -> Result<(), VulcanError> {
    if permission.permission_authority != expected_risk_authority {
        return Err(VulcanError::api(
            "PERMISSION_AUTHORITY_MISMATCH",
            format!(
                "Permission authority mismatch: expected {expected_risk_authority}, got {}",
                permission.permission_authority
            ),
        ));
    }
    if permission.delegated_key != expected_onboarder {
        return Err(VulcanError::api(
            "PERMISSION_DELEGATE_MISMATCH",
            format!(
                "Permission delegated key mismatch: expected {expected_onboarder}, got {}",
                permission.delegated_key
            ),
        ));
    }
    if permission.permission & TRADER_ONBOARDING_PERMISSION != TRADER_ONBOARDING_PERMISSION {
        return Err(VulcanError::api(
            "PERMISSION_MISSING_ONBOARDING",
            "Permission account is missing trader-onboarding permission",
        ));
    }
    if permission.allowed_signer_actions == 0 {
        return Err(VulcanError::api(
            "PERMISSION_EXHAUSTED",
            "Permission account has no remaining signer actions",
        ));
    }
    Ok(())
}

async fn submit_referral_activation_tx(
    ctx: &AppContext,
    wallet_name: &str,
    authority: Pubkey,
    referral_code: String,
    status: ReferralActivationTraderStatus,
) -> Result<Option<String>, VulcanError> {
    let trader = TraderKey::new(authority);
    let (wallet, _, _) =
        crate::commands::trade::resolve_wallet_and_pda(ctx, Some(wallet_name)).await?;
    let permission_response = ctx
        .http_client
        .invite()
        .get_referral_activation_permission()
        .await
        .map_err(|e| VulcanError::api("REFERRAL_PERMISSION_FAILED", e.to_string()))?;
    let trader_onboarder =
        parse_api_pubkey(&permission_response.trader_onboarder, "trader_onboarder")?;
    let risk_authority = parse_api_pubkey(&permission_response.risk_authority, "risk_authority")?;
    let permission_account = parse_api_pubkey(
        &permission_response.permission_account,
        "permission_account",
    )?;
    let permission = fetch_permission_account(ctx, &permission_account).await?;
    validate_permission(&permission, risk_authority, trader_onboarder)?;

    let keys = ctx.metadata().await?.keys();
    let global_trader_index =
        parse_api_pubkey_list(&keys.global_trader_index, "global_trader_index")?;
    let active_trader_buffer =
        parse_api_pubkey_list(&keys.active_trader_buffer, "active_trader_buffer")?;

    let mut ixs = Vec::<solana_sdk::instruction::Instruction>::new();
    if status.should_include_register_trader() {
        ensure_sol_for_cross_trader_rent(ctx, authority, CROSS_MARGIN_MAX_POSITIONS).await?;

        let register_params = RegisterTraderParams::builder()
            .payer(authority)
            .trader(authority)
            .trader_account(trader.pda())
            .max_positions(CROSS_MARGIN_MAX_POSITIONS as u64)
            .trader_pda_index(0)
            .subaccount_index(0)
            .build()
            .map_err(|e| VulcanError::api("BUILD_REGISTER_FAILED", e.to_string()))?;
        ixs.push(
            create_register_trader_ix(register_params)
                .map_err(|e| VulcanError::api("BUILD_REGISTER_FAILED", e.to_string()))?
                .into(),
        );
    }

    let onboard_params = OnboardTraderDelegatedParams::builder()
        .authority(trader_onboarder)
        .permission_account(permission_account)
        .trader_account(trader.pda())
        .global_trader_index(global_trader_index)
        .active_trader_buffer(active_trader_buffer)
        .build()
        .map_err(|e| VulcanError::api("BUILD_REFERRAL_ONBOARD_FAILED", e.to_string()))?;
    ixs.push(
        create_onboard_trader_delegated_ix(onboard_params)
            .map_err(|e| VulcanError::api("BUILD_REFERRAL_ONBOARD_FAILED", e.to_string()))?
            .into(),
    );

    let (transaction, recent_blockhash, _) =
        sign_onboarding_transaction_for_api(ctx, &wallet, ixs, trader_onboarder).await?;
    let response = ctx
        .http_client
        .invite()
        .activate_referral_tx(&ActivateReferralTxRequest {
            referral_code,
            trader_authority: authority.to_string(),
            trader_pda_index: Some(0),
            trader_subaccount_index: Some(0),
            recent_blockhash,
            transaction,
        })
        .await
        .map_err(|e| VulcanError::api("REFERRAL_ACTIVATE_TX_FAILED", e.to_string()))?;

    Ok(response.signature)
}

async fn register_authority(
    ctx: &AppContext,
    wallet_name: &str,
    authority: Pubkey,
    referral_code: Option<String>,
) -> Result<RegisterResult, VulcanError> {
    let status = trader_onboarding_status(ctx, &authority).await?;

    let sig = if matches!(status, ReferralActivationTraderStatus::Activated) {
        eprintln!("Trader account already registered and onboarded, skipping registration.");
        None
    } else if ctx.dry_run {
        None
    } else {
        let code = referral_code
            .map(|code| code.trim().to_string())
            .filter(|code| !code.is_empty())
            .unwrap_or_else(|| DEFAULT_REFERRAL_CODE.to_string());
        submit_referral_activation_tx(ctx, wallet_name, authority, code, status).await?
    };

    let trader_key = TraderKey::new(authority);
    Ok(RegisterResult {
        authority: authority.to_string(),
        trader_pda: trader_key.pda().to_string(),
        dry_run: ctx.dry_run,
        tx_signature: sig,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use solana_sdk::signature::Signature;

    fn two_signer_onboarding_tx(
        authority: Pubkey,
        onboarder: Pubkey,
    ) -> solana_sdk::transaction::Transaction {
        let program_id = Pubkey::new_unique();
        let ix = solana_sdk::instruction::Instruction {
            program_id,
            accounts: vec![
                solana_sdk::instruction::AccountMeta::new(authority, true),
                solana_sdk::instruction::AccountMeta::new_readonly(onboarder, true),
            ],
            data: vec![],
        };
        solana_sdk::transaction::Transaction::new_with_payer(&[ix], Some(&authority))
    }

    #[test]
    fn partial_signature_missing_only_onboarder_is_accepted() {
        let authority = Pubkey::new_unique();
        let onboarder = Pubkey::new_unique();
        let mut tx = two_signer_onboarding_tx(authority, onboarder);
        tx.signatures = vec![Signature::from([1u8; 64]), Signature::default()];

        assert!(validate_partial_onboarding_signatures(&tx, &onboarder).is_ok());
    }

    #[test]
    fn partial_signature_missing_authority_is_rejected() {
        let authority = Pubkey::new_unique();
        let onboarder = Pubkey::new_unique();
        let mut tx = two_signer_onboarding_tx(authority, onboarder);
        tx.signatures = vec![Signature::default(), Signature::default()];

        let err = validate_partial_onboarding_signatures(&tx, &onboarder)
            .expect_err("missing authority signature must be rejected");
        assert!(err.to_string().contains(&authority.to_string()));
    }

    #[test]
    fn trader_account_size_matches_rise_layout() {
        assert_eq!(
            trader_account_size(CROSS_MARGIN_MAX_POSITIONS).expect("cross account size"),
            5_360
        );
        assert_eq!(trader_account_size(1).expect("isolated account size"), 280);
    }

    #[test]
    fn format_sol_lamports_trims_fractional_padding() {
        assert_eq!(format_sol_lamports(0), "0.0");
        assert_eq!(format_sol_lamports(1), "0.000000001");
        assert_eq!(format_sol_lamports(1_500_000_000), "1.5");
        assert_eq!(format_sol_lamports(2_000_000_000), "2.0");
    }
}

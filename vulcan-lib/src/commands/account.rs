//! Account command execution.

use crate::cli::account::AccountCommand;
use crate::context::AppContext;
use crate::error::VulcanError;
use crate::output::{render_success, TableRenderable};
use crate::wallet::ResolvedSigner;
use phoenix_rise::accounts::owned::Permission;
use phoenix_rise::accounts::permission::TRADER_ONBOARDING_PERMISSION;
use phoenix_rise::api::{
    fetch_referral_activation_trader_status, ActivateReferralTxRequest, ApiInstructionResponse,
    BuildRegisterIxsRequest, ReferralActivationTraderStatus, SendRegisterIxsRequest, TraderKey,
};
use phoenix_rise::ix::onboard_trader_delegated::{
    create_onboard_trader_delegated_ix, OnboardTraderDelegatedParams,
};
use phoenix_rise::ix::register_trader::{create_register_trader_ix, RegisterTraderParams};
use serde::Serialize;
use solana_keychain::SignTransactionResult;
use solana_pubkey::Pubkey;
use std::str::FromStr;

const CROSS_MARGIN_MAX_POSITIONS: u32 = 128;

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

fn parse_registration_code(
    access_code: Option<String>,
    referral_code: Option<String>,
    invite_code: Option<String>,
) -> Result<Option<RegistrationCode>, VulcanError> {
    match (access_code, referral_code, invite_code) {
        (None, None, None) => Ok(None),
        (Some(code), None, None) | (None, None, Some(code)) => {
            Ok(Some(RegistrationCode::Access(code)))
        }
        (None, Some(code), None) => Ok(Some(RegistrationCode::Referral(code))),
        _ => Err(VulcanError::validation(
            "REGISTRATION_CODE_CONFLICT",
            "Provide at most one of --access-code, --referral-code, or --invite-code",
        )),
    }
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
            let code = parse_registration_code(access_code, referral_code, invite_code)?;
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
    invite_code: &str,
) -> Result<RegisterResult, VulcanError> {
    let (wallet_name, authority) = resolve_authority(ctx)?;
    register_authority(
        ctx,
        &wallet_name,
        authority,
        Some(RegistrationCode::Access(invite_code.to_string())),
    )
    .await
}

pub async fn execute_register_with_code_inner(
    ctx: &AppContext,
    code: Option<RegistrationCode>,
) -> Result<RegisterResult, VulcanError> {
    let (wallet_name, authority) = resolve_authority(ctx)?;
    register_authority(ctx, &wallet_name, authority, code).await
}

pub async fn execute_register_wallet_with_code_inner(
    ctx: &AppContext,
    wallet_name: &str,
    code: Option<RegistrationCode>,
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

async fn activate_access_code(
    ctx: &AppContext,
    authority: &Pubkey,
    code: &str,
) -> Result<(), VulcanError> {
    ctx.http_client
        .invite()
        .activate_invite(authority, code)
        .await
        .map_err(|e| VulcanError::api("REGISTER_API_FAILED", e.to_string()))?;
    Ok(())
}

async fn submit_local_register_tx(
    ctx: &AppContext,
    wallet_name: &str,
    authority: Pubkey,
) -> Result<Option<String>, VulcanError> {
    let builder = ctx.tx_builder().await?;
    let ixs = builder
        .build_register_trader(authority, 0, 0)
        .map_err(|e| VulcanError::api("BUILD_REGISTER_FAILED", e.to_string()))?;

    let (wallet, _, _) =
        crate::commands::trade::resolve_wallet_and_pda(ctx, Some(wallet_name)).await?;

    match crate::commands::trade::send_or_dry_run(ctx, ixs, &wallet).await {
        Ok(sig) => Ok(sig),
        Err(err)
            if err.category == crate::error::ErrorCategory::TxFailed
                && is_cross_margin_registered(ctx, &authority).await? =>
        {
            eprintln!("Registration transaction failed, but trader account is now registered.");
            Ok(None)
        }
        Err(err) => Err(err),
    }
}

fn api_instruction_to_solana(
    api_instruction: &ApiInstructionResponse,
) -> Result<solana_sdk::instruction::Instruction, VulcanError> {
    Ok(solana_sdk::instruction::Instruction {
        program_id: Pubkey::from_str(&api_instruction.program_id).map_err(|e| {
            VulcanError::api(
                "REGISTER_API_INVALID_IX",
                format!("Invalid program id: {e}"),
            )
        })?,
        accounts: api_instruction
            .keys
            .iter()
            .map(|account| {
                Ok(solana_sdk::instruction::AccountMeta {
                    pubkey: Pubkey::from_str(&account.pubkey).map_err(|e| {
                        VulcanError::api(
                            "REGISTER_API_INVALID_IX",
                            format!("Invalid account pubkey: {e}"),
                        )
                    })?,
                    is_signer: account.is_signer,
                    is_writable: account.is_writable,
                })
            })
            .collect::<Result<Vec<_>, VulcanError>>()?,
        data: api_instruction.data.clone(),
    })
}

async fn sign_onboarding_transaction_for_api(
    ctx: &AppContext,
    wallet: &ResolvedSigner,
    ixs: Vec<solana_sdk::instruction::Instruction>,
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
        return Err(VulcanError::auth(
            "PARTIAL_SIGNATURE",
            "Transaction was only partially signed; onboarding requires a complete authority signature.",
        ));
    }

    let (transaction, _) = signed.into_signed_transaction();
    Ok((transaction, recent_blockhash.to_string(), fee_payer))
}

async fn submit_builder_onboarding_tx(
    ctx: &AppContext,
    wallet_name: &str,
    authority: Pubkey,
) -> Result<Option<String>, VulcanError> {
    let (wallet, _, _) =
        crate::commands::trade::resolve_wallet_and_pda(ctx, Some(wallet_name)).await?;
    let fee_payer = wallet.signer()?.pubkey();
    if fee_payer != wallet.authority {
        return Err(VulcanError::auth(
            "SIGNER_PUBKEY_MISMATCH",
            format!(
                "Signer pubkey {} does not match active wallet authority {}",
                fee_payer, wallet.authority
            ),
        ));
    }
    let built = ctx
        .http_client
        .exchange()
        .build_register_ixs(&BuildRegisterIxsRequest {
            trader_authority: authority.to_string(),
            tx_fee_payer: fee_payer.to_string(),
            max_positions: Some(CROSS_MARGIN_MAX_POSITIONS),
        })
        .await
        .map_err(|e| VulcanError::api("BUILD_REGISTER_API_FAILED", e.to_string()))?;
    let ixs = built
        .instructions
        .iter()
        .map(api_instruction_to_solana)
        .collect::<Result<Vec<_>, _>>()?;
    let (transaction, _, fee_payer) =
        sign_onboarding_transaction_for_api(ctx, &wallet, ixs).await?;

    let submitted = ctx
        .http_client
        .exchange()
        .send_register_ixs(&SendRegisterIxsRequest {
            transaction,
            trader_authority: authority.to_string(),
            tx_fee_payer: fee_payer.to_string(),
            max_positions: Some(built.max_positions),
            trader_pda_index: Some(0),
            trader_subaccount_index: Some(0),
        })
        .await
        .map_err(|e| VulcanError::api("SEND_REGISTER_API_FAILED", e.to_string()))?;
    Ok(Some(submitted.signature))
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
        sign_onboarding_transaction_for_api(ctx, &wallet, ixs).await?;
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
    code: Option<RegistrationCode>,
) -> Result<RegisterResult, VulcanError> {
    let status = trader_onboarding_status(ctx, &authority).await?;

    let sig = if matches!(status, ReferralActivationTraderStatus::Activated) {
        eprintln!("Trader account already registered and onboarded, skipping registration.");
        None
    } else if ctx.dry_run {
        None
    } else {
        match code {
            Some(RegistrationCode::Access(code)) => {
                activate_access_code(ctx, &authority, code.as_str()).await?;

                if is_cross_margin_registered(ctx, &authority).await? {
                    eprintln!(
                        "Trader account registered after code activation; skipping on-chain transaction."
                    );
                    None
                } else {
                    submit_local_register_tx(ctx, wallet_name, authority).await?
                }
            }
            Some(RegistrationCode::Referral(referral_code)) => {
                submit_referral_activation_tx(ctx, wallet_name, authority, referral_code, status)
                    .await?
            }
            None => submit_builder_onboarding_tx(ctx, wallet_name, authority).await?,
        }
    };

    let trader_key = TraderKey::new(authority);
    Ok(RegisterResult {
        authority: authority.to_string(),
        trader_pda: trader_key.pda().to_string(),
        dry_run: ctx.dry_run,
        tx_signature: sig,
    })
}

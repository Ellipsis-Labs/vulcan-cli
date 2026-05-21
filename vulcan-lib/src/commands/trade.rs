//! Trade command execution.

use crate::cli::trade::TradeCommand;
use crate::context::AppContext;
use crate::error::VulcanError;
use crate::output::{render_success, TableRenderable};
use phoenix_rise::math::{SignedQuoteLots, WrapperNum};
use phoenix_rise::types::trader_state::LimitOrder as SdkLimitOrder;
use phoenix_rise::types::{
    Position as SdkPosition, SubaccountState, Trader, TraderKey, TraderView,
};
use phoenix_rise::{BracketLeg, BracketLegOrders, BracketLegSize, IsolatedCollateralFlow, Side};
use serde::Serialize;
use solana_pubkey::Pubkey;
use solana_sdk::signer::Signer;
use std::collections::BTreeMap;
use std::str::FromStr;
use std::sync::Arc;

const DEFAULT_CONDITIONAL_ORDERS_CAPACITY: u8 = 8;

// ── Result types ────────────────────────────────────────────────────────

#[derive(Debug, Serialize)]
pub struct OrderResult {
    pub action: String,
    pub symbol: String,
    pub side: String,
    pub size: f64,
    pub price: Option<f64>,
    pub tp: Option<f64>,
    pub sl: Option<f64>,
    pub dry_run: bool,
    pub tx_signature: Option<String>,
    pub num_instructions: usize,
    /// Mid price at quote time, set when sizing was specified as notional_usdc.
    /// Actual fill price may differ by spread and market impact.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub quoted_mark: Option<f64>,
}

impl TableRenderable for OrderResult {
    fn render_table(&self) {
        if self.dry_run {
            println!("[DRY RUN] Would place {} order:", self.action);
        } else {
            println!("Order placed:");
        }
        println!("  Symbol: {}", self.symbol);
        println!("  Side: {}", self.side);
        println!("  Size: {} base lots", self.size);
        if let Some(p) = self.price {
            println!("  Price: ${:.2}", p);
        }
        if let Some(tp) = self.tp {
            println!("  Take profit: ${:.2}", tp);
        }
        if let Some(sl) = self.sl {
            println!("  Stop loss: ${:.2}", sl);
        }
        println!("  Instructions: {}", self.num_instructions);
        if let Some(sig) = &self.tx_signature {
            println!("  Tx: {}", sig);
        }
    }
}

/// Single leg of a multi-limit order. Each leg may carry its own optional
/// per-leg take-profit / stop-loss; when set, the multi-limit submission falls
/// back to building N individual limit-order instructions (each with its own
/// bracket) bundled into one transaction.
#[derive(Debug, Clone)]
pub struct MultiLimitLeg {
    pub price: f64,
    pub size_lots: u64,
    pub tp: Option<f64>,
    pub sl: Option<f64>,
}

#[derive(Debug, Serialize)]
pub struct MultiLimitOrderEntry {
    pub side: String,
    pub price: f64,
    pub size: u64,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub tp: Option<f64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub sl: Option<f64>,
}

#[derive(Debug, Serialize)]
pub struct MultiLimitOrderResult {
    pub action: String,
    pub symbol: String,
    pub bids: Vec<MultiLimitOrderEntry>,
    pub asks: Vec<MultiLimitOrderEntry>,
    pub slide: bool,
    /// True when any leg carried tp/sl and the submission expanded to per-leg
    /// limit-order instructions instead of the single multi-limit instruction.
    pub per_leg_brackets: bool,
    pub dry_run: bool,
    pub tx_signature: Option<String>,
    pub num_instructions: usize,
}

impl TableRenderable for MultiLimitOrderResult {
    fn render_table(&self) {
        if self.dry_run {
            println!("[DRY RUN] Would place multi-limit order:");
        } else {
            println!("Multi-limit order placed:");
        }
        println!("  Symbol: {}", self.symbol);
        println!("  Bids: {}", self.bids.len());
        for b in &self.bids {
            print!("    ${:.4} × {} lots", b.price, b.size);
            if let Some(tp) = b.tp {
                print!("  tp=${:.4}", tp);
            }
            if let Some(sl) = b.sl {
                print!("  sl=${:.4}", sl);
            }
            println!();
        }
        println!("  Asks: {}", self.asks.len());
        for a in &self.asks {
            print!("    ${:.4} × {} lots", a.price, a.size);
            if let Some(tp) = a.tp {
                print!("  tp=${:.4}", tp);
            }
            if let Some(sl) = a.sl {
                print!("  sl=${:.4}", sl);
            }
            println!();
        }
        println!("  Slide: {}", self.slide);
        if self.per_leg_brackets {
            println!("  Per-leg brackets: yes");
        }
        println!("  Instructions: {}", self.num_instructions);
        if let Some(sig) = &self.tx_signature {
            println!("  Tx: {}", sig);
        }
    }
}

#[derive(Debug, Serialize)]
pub struct CancelResult {
    pub symbol: String,
    pub cancelled_ids: Vec<String>,
    pub dry_run: bool,
    pub tx_signature: Option<String>,
    pub num_instructions: usize,
}

impl TableRenderable for CancelResult {
    fn render_table(&self) {
        if self.dry_run {
            println!(
                "[DRY RUN] Would cancel {} orders on {}",
                self.cancelled_ids.len(),
                self.symbol
            );
        } else {
            println!(
                "Cancelled {} orders on {}",
                self.cancelled_ids.len(),
                self.symbol
            );
        }
        if let Some(sig) = &self.tx_signature {
            println!("  Tx: {}", sig);
        }
    }
}

#[derive(Debug, Serialize)]
pub struct CancelAllMarketsResult {
    pub per_market: Vec<CancelResult>,
    pub total_cancelled: usize,
    pub markets_touched: usize,
    pub dry_run: bool,
}

impl TableRenderable for CancelAllMarketsResult {
    fn render_table(&self) {
        if self.per_market.is_empty() {
            println!("No open orders to cancel.");
            return;
        }
        if self.dry_run {
            println!(
                "[DRY RUN] Would cancel {} orders across {} markets:",
                self.total_cancelled, self.markets_touched
            );
        } else {
            println!(
                "Cancelled {} orders across {} markets:",
                self.total_cancelled, self.markets_touched
            );
        }
        for r in &self.per_market {
            print!("  {} — {} orders", r.symbol, r.cancelled_ids.len());
            if let Some(sig) = &r.tx_signature {
                print!("  tx: {}", sig);
            }
            println!();
        }
    }
}

#[derive(Debug, Serialize)]
pub struct OrderInfo {
    pub symbol: String,
    pub side: String,
    pub order_id: String,
    pub price: String,
    pub size_remaining: String,
    pub initial_size: String,
    pub reduce_only: bool,
    pub is_stop_loss: bool,
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub is_take_profit: bool,
}

#[derive(Debug, Serialize)]
pub struct OrdersResult {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub symbol: Option<String>,
    pub orders: Vec<OrderInfo>,
}

impl TableRenderable for OrdersResult {
    fn render_table(&self) {
        if self.orders.is_empty() {
            match &self.symbol {
                Some(s) => println!("No open orders for {}.", s),
                None => println!("No open orders."),
            }
            return;
        }
        let show_symbol = self.symbol.is_none();
        let mut headers = vec!["Order ID", "Side", "Price", "Remaining", "Initial", "Flags"];
        if show_symbol {
            headers.insert(0, "Symbol");
        }
        let rows: Vec<Vec<String>> = self
            .orders
            .iter()
            .map(|o| {
                let mut flags = Vec::new();
                if o.reduce_only {
                    flags.push("RO");
                }
                if o.is_stop_loss {
                    flags.push("SL");
                }
                if o.is_take_profit {
                    flags.push("TP");
                }
                let mut row = Vec::new();
                if show_symbol {
                    row.push(o.symbol.clone());
                }
                row.extend([
                    o.order_id.clone(),
                    o.side.clone(),
                    o.price.clone(),
                    o.size_remaining.clone(),
                    o.initial_size.clone(),
                    flags.join(","),
                ]);
                row
            })
            .collect();
        crate::output::table::render_table(&headers, rows);
    }
}

// ── Helpers ─────────────────────────────────────────────────────────────

/// Get wallet password from VULCAN_WALLET_PASSWORD env var, or prompt via stderr.
pub fn prompt_password() -> Result<String, VulcanError> {
    if let Ok(pw) = std::env::var("VULCAN_WALLET_PASSWORD") {
        return Ok(pw);
    }
    use std::io::IsTerminal;
    if !std::io::stdin().is_terminal() {
        return Err(VulcanError::auth(
            "WALLET_PASSWORD_REQUIRED",
            "Wallet password is required for live signing. Non-interactive agent shells cannot answer wallet prompts. If MCP is already configured (check with `vulcan agent mcp doctor --target <claude|cursor|codex|agentskills> --scope user`): restart the agent client so Vulcan inherits the env. If MCP is NOT yet configured: run `vulcan agent mcp install --target <…> --scope user --dangerous` and restart the client. `vulcan agent live-ready --target <…> --scope user -o json` reports readiness but does not install anything. CLI fallback requires VULCAN_WALLET_PASSWORD to be set before starting the command.",
        ));
    }
    eprint!("Wallet password: ");
    rpassword::read_password().map_err(|e| {
        VulcanError::io(
            "PASSWORD_READ_FAILED",
            format!(
                "{}. Set VULCAN_WALLET_PASSWORD or run from an interactive terminal.",
                e
            ),
        )
    })
}

/// Resolve the wallet and trader PDA for trading commands.
/// If a session wallet is available (MCP mode), use it directly.
/// Per-call `wallet_override` wins over the global CLI `--wallet` flag, which wins over the default wallet.
pub fn resolve_wallet_and_pda(
    ctx: &AppContext,
    wallet_override: Option<&str>,
) -> Result<(crate::wallet::Wallet, Pubkey, Pubkey), VulcanError> {
    // MCP session wallet path — no password prompt needed
    if let Some(sw) = &ctx.session_wallet {
        let wallet = sw.to_wallet()?;
        return Ok((wallet, sw.authority, sw.trader_pda));
    }

    let chosen_name = wallet_override
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .map(str::to_string)
        .or_else(|| ctx.wallet_override.clone());

    let wallet_name = match chosen_name {
        Some(name) => {
            if !ctx.wallet_store.exists(&name) {
                return Err(VulcanError::auth(
                    "WALLET_NOT_FOUND",
                    format!(
                        "Wallet '{}' not found. Use `vulcan wallet list` for stored names.",
                        name
                    ),
                ));
            }
            name
        }
        None => ctx
            .wallet_store
            .default_wallet()
            .map_err(|e| VulcanError::config("CONFIG_ERROR", e.to_string()))?
            .ok_or_else(|| {
                VulcanError::config(
                    "NO_DEFAULT_WALLET",
                    "No default wallet set. Use 'vulcan wallet set-default <NAME>' or pass --wallet <NAME>",
                )
            })?,
    };

    let wallet_file = ctx
        .wallet_store
        .load(&wallet_name)
        .map_err(|e| VulcanError::auth("WALLET_NOT_FOUND", e.to_string()))?;

    let authority = Pubkey::from_str(&wallet_file.public_key)
        .map_err(|e| VulcanError::validation("INVALID_PUBKEY", e.to_string()))?;

    // Default trader PDA: pda_index=0, subaccount_index=0 (cross-margin)
    let trader_key = phoenix_rise::types::TraderKey::new(authority);
    let trader_pda = trader_key.pda();

    if ctx.dry_run {
        let wallet = dry_run_wallet_for_authority(authority)?;
        return Ok((wallet, authority, trader_pda));
    }

    let password = prompt_password()?;
    let wallet = crate::wallet::Wallet::decrypt(&wallet_file.encrypted, &password)
        .map_err(|e| VulcanError::auth("DECRYPT_FAILED", e.to_string()))?;

    Ok((wallet, authority, trader_pda))
}

fn dry_run_wallet_for_authority(authority: Pubkey) -> Result<crate::wallet::Wallet, VulcanError> {
    let mut keypair_bytes = vec![0u8; 64];
    keypair_bytes[32..64].copy_from_slice(authority.as_ref());
    crate::wallet::Wallet::from_bytes(&keypair_bytes)
        .map_err(|e| VulcanError::internal("DRY_RUN_WALLET_FAILED", e.to_string()))
}

/// Resolve the active authority pubkey (no decryption needed).
/// MCP requests use the pre-unlocked session wallet; CLI falls back to `--wallet` or the default wallet.
pub(crate) fn resolve_authority_name(ctx: &AppContext) -> Result<(String, Pubkey), VulcanError> {
    if let Some(sw) = &ctx.session_wallet {
        return Ok((sw.wallet_name.clone(), sw.authority));
    }

    let wallet_name = if let Some(name) = &ctx.wallet_override {
        if !ctx.wallet_store.exists(name) {
            return Err(VulcanError::auth(
                "WALLET_NOT_FOUND",
                format!(
                    "Wallet '{}' from --wallet not found. Use `vulcan wallet list` for stored names.",
                    name
                ),
            ));
        }
        name.clone()
    } else {
        ctx.wallet_store
            .default_wallet()
            .map_err(|e| VulcanError::config("CONFIG_ERROR", e.to_string()))?
            .ok_or_else(|| {
                VulcanError::config(
                    "NO_DEFAULT_WALLET",
                    "No default wallet set. Use 'vulcan wallet set-default <NAME>' or pass --wallet <NAME>",
                )
            })?
    };

    let wallet_file = ctx
        .wallet_store
        .load(&wallet_name)
        .map_err(|e| VulcanError::auth("WALLET_NOT_FOUND", e.to_string()))?;

    let authority = Pubkey::from_str(&wallet_file.public_key)
        .map_err(|e| VulcanError::validation("INVALID_PUBKEY", e.to_string()))?;
    Ok((wallet_name, authority))
}

/// Resolve the active authority pubkey (no decryption needed).
pub(crate) fn resolve_authority(ctx: &AppContext) -> Result<Pubkey, VulcanError> {
    resolve_authority_name(ctx).map(|(_, authority)| authority)
}

/// Convert HTTP API TraderViews into the SDK Trader struct needed for isolated orders.
pub fn trader_from_views(authority: Pubkey, pda_index: u8, views: &[TraderView]) -> Trader {
    let key = TraderKey::new_with_idx(authority, pda_index, 0);
    let mut trader = Trader::new(key);

    for view in views {
        let collateral_f64: f64 = view.collateral_balance.value as f64
            / 10f64.powi(view.collateral_balance.decimals as i32);
        let collateral_quote_lots = (collateral_f64 * 1_000_000.0) as i64;

        let mut subaccount = SubaccountState {
            subaccount_index: view.trader_subaccount_index,
            collateral: SignedQuoteLots::new(collateral_quote_lots),
            ..Default::default()
        };

        // Convert positions
        for pos_view in &view.positions {
            let base_lots: i64 = pos_view.position_size.value;
            let entry_ticks: i64 = pos_view.entry_price.value;
            let entry_usd = pos_view
                .entry_price
                .ui
                .parse()
                .unwrap_or(phoenix_rise::Decimal::ZERO);

            let position = SdkPosition {
                symbol: pos_view.symbol.clone(),
                base_position_lots: base_lots,
                entry_price_ticks: entry_ticks,
                entry_price_usd: entry_usd,
                virtual_quote_position_lots: 0,
                unsettled_funding_quote_lots: 0,
                accumulated_funding_quote_lots: 0,
            };
            subaccount
                .positions
                .insert(pos_view.symbol.clone(), position);
        }

        // Convert limit orders
        for (symbol, orders) in &view.limit_orders {
            for order in orders {
                let osn: u64 = order.order_sequence_number.parse().unwrap_or(0);
                let sdk_order = SdkLimitOrder {
                    symbol: symbol.clone(),
                    order_sequence_number: osn,
                    side: format!("{:?}", order.side),
                    order_type: String::new(),
                    price_ticks: order.price.value,
                    price_usd: order
                        .price
                        .ui
                        .parse()
                        .unwrap_or(phoenix_rise::Decimal::ZERO),
                    size_remaining_lots: order.trade_size_remaining.value.unsigned_abs(),
                    initial_size_lots: order.initial_trade_size.value.unsigned_abs(),
                    reduce_only: order.is_reduce_only,
                    is_stop_loss: order.is_stop_loss,
                    status: "Open".to_string(),
                };
                subaccount.orders.insert((symbol.clone(), osn), sdk_order);
            }
        }

        trader
            .subaccounts
            .insert(view.trader_subaccount_index, subaccount);
    }

    trader
}

pub fn bracket_leg_orders(
    tp: Option<f64>,
    sl: Option<f64>,
) -> Option<phoenix_rise::BracketLegOrders> {
    if tp.is_none() && sl.is_none() {
        return None;
    }

    Some(phoenix_rise::BracketLegOrders {
        take_profit: tp.map(BracketLeg::new),
        stop_loss: sl.map(BracketLeg::new),
    })
}

/// Build a bracket where each leg is sized to an explicit base-lot amount.
/// The SDK rejects this on limit orders (`UnsupportedLimitBracketLegSizing`),
/// but market orders and `set_tpsl` need it so the on-chain `max_size`
/// matches the actual position and renders correctly in Phoenix UI.
pub fn sized_bracket_leg_orders(
    tp: Option<f64>,
    sl: Option<f64>,
    base_lots: u64,
) -> Option<BracketLegOrders> {
    if tp.is_none() && sl.is_none() {
        return None;
    }
    let leg = |price: f64| BracketLeg::new(price).with_size(BracketLegSize::BaseLots(base_lots));
    Some(BracketLegOrders {
        take_profit: tp.map(leg),
        stop_loss: sl.map(leg),
    })
}

/// Default compute-unit budget for a single Phoenix tx. Sized for a typical
/// place/cancel/position ix plus the activate-trader overhead.
pub const DEFAULT_CU_LIMIT: u32 = 400_000;

/// Solana's hard cap on per-tx requested compute units.
pub const MAX_CU_LIMIT: u32 = 1_400_000;

/// Build, optionally sign, and submit a transaction with the default CU budget.
pub async fn send_or_dry_run(
    ctx: &AppContext,
    ixs: Vec<solana_sdk::instruction::Instruction>,
    wallet: &crate::wallet::Wallet,
) -> Result<Option<String>, VulcanError> {
    send_or_dry_run_with_cu_limit(ctx, ixs, wallet, DEFAULT_CU_LIMIT).await
}

/// Build, optionally sign, and submit a transaction with a caller-supplied CU
/// budget. Used by bundles that exceed the default (e.g. multi-leg
/// `place_limit_order_with_conditionals`, where each leg consumes ~145k CUs).
pub async fn send_or_dry_run_with_cu_limit(
    ctx: &AppContext,
    ixs: Vec<solana_sdk::instruction::Instruction>,
    wallet: &crate::wallet::Wallet,
    cu_limit: u32,
) -> Result<Option<String>, VulcanError> {
    if ctx.dry_run {
        return Ok(None);
    }

    let keypair = wallet
        .to_solana_keypair()
        .map_err(|e| VulcanError::auth("KEYPAIR_ERROR", e.to_string()))?;

    let rpc_client = ctx.rpc_client();

    let recent_blockhash = rpc_client
        .get_latest_blockhash()
        .map_err(|e| VulcanError::network("BLOCKHASH_FAILED", e.to_string()))?;

    let cu_limit = cu_limit.min(MAX_CU_LIMIT);
    let mut all_ixs = Vec::with_capacity(ixs.len() + 1);
    all_ixs.push(
        solana_sdk::compute_budget::ComputeBudgetInstruction::set_compute_unit_limit(cu_limit),
    );
    all_ixs.extend(ixs);

    let tx = solana_sdk::transaction::Transaction::new_signed_with_payer(
        &all_ixs,
        Some(&keypair.pubkey()),
        &[&keypair],
        recent_blockhash,
    );

    let sig = rpc_client
        .send_and_confirm_transaction(&tx)
        .map_err(|e| VulcanError::tx_failed("TX_SEND_FAILED", e.to_string()))?;

    Ok(Some(sig.to_string()))
}

/// Build a conditional-orders account init instruction when the PDA is missing
/// or exists as the zero-sized system account left behind by some isolated
/// subaccount flows.
pub(crate) async fn conditional_orders_init_ixs_if_needed(
    ctx: &AppContext,
    builder: &phoenix_rise::PhoenixTxBuilder<'_>,
    authority: Pubkey,
    trader_pda: Pubkey,
) -> Result<Vec<solana_sdk::instruction::Instruction>, VulcanError> {
    let conditional_orders = phoenix_rise::get_conditional_orders_address(&trader_pda);
    let rpc_client = ctx.rpc_client_async();
    let response = rpc_client
        .get_account_with_commitment(&conditional_orders, rpc_client.commitment())
        .await
        .map_err(|e| VulcanError::network("CONDITIONAL_ORDERS_FETCH_FAILED", e.to_string()))?;

    let needs_init = response
        .value
        .as_ref()
        .map(|account| account.data.is_empty())
        .unwrap_or(true);

    if !needs_init {
        return Ok(Vec::new());
    }

    builder
        .build_create_conditional_orders_account(
            authority,
            authority,
            trader_pda,
            DEFAULT_CONDITIONAL_ORDERS_CAPACITY,
        )
        .map_err(|e| VulcanError::api("BUILD_CONDITIONAL_ORDERS_INIT_FAILED", e.to_string()))
}

fn insert_before_first_order_or_conditional_ix(
    ixs: &mut Vec<solana_sdk::instruction::Instruction>,
    init_ixs: Vec<solana_sdk::instruction::Instruction>,
) {
    if init_ixs.is_empty() {
        return;
    }

    let insert_at = ixs
        .iter()
        .position(is_order_or_position_conditional_ix)
        .unwrap_or(ixs.len());
    ixs.splice(insert_at..insert_at, init_ixs);
}

fn is_order_or_position_conditional_ix(ix: &solana_sdk::instruction::Instruction) -> bool {
    ix.program_id == phoenix_rise::phoenix_rise_ix::PHOENIX_PROGRAM_ID
        && (ix
            .data
            .starts_with(&phoenix_rise::phoenix_rise_ix::place_market_order_discriminant())
            || ix
                .data
                .starts_with(&phoenix_rise::phoenix_rise_ix::place_limit_order_discriminant())
            || ix.data.starts_with(
                &phoenix_rise::phoenix_rise_ix::place_position_conditional_order_discriminant(),
            ))
}

// ── TP/SL level input ───────────────────────────────────────────────────

/// How a caller wants to size a single TP or SL leg.
#[derive(Debug, Clone, Copy)]
pub enum TpSlSize {
    /// Cover the full current position. Only valid when this is the only level
    /// on a given side — multi-level requires explicit per-leg sizes.
    Full,
    /// Explicit base lots.
    Lots(u64),
    /// Tokens of the base asset (e.g., 0.5 SOL). Resolved against the market's
    /// `base_lots_decimals`.
    Tokens(f64),
}

/// User-facing TP or SL leg before resolution against the live position.
#[derive(Debug, Clone, Copy)]
pub struct TpSlInput {
    pub price: f64,
    pub size: TpSlSize,
}

impl TpSlInput {
    pub fn full(price: f64) -> Self {
        Self {
            price,
            size: TpSlSize::Full,
        }
    }
}

/// Parse CLI inputs for one side of TP or SL into a single `Vec<TpSlInput>`.
///
/// Accepts either the legacy single-price flag (`--tp`/`--sl`) or the
/// repeatable level flag (`--tp-level`/`--sl-level`) with `PRICE[:SIZE]`
/// entries. The two forms are mutually exclusive.
pub fn parse_cli_tpsl_levels(
    single_flag: &str,
    level_flag: &str,
    single: Option<f64>,
    levels: &[String],
) -> Result<Vec<TpSlInput>, VulcanError> {
    if single.is_some() && !levels.is_empty() {
        return Err(VulcanError::validation(
            "TPSL_FLAG_CONFLICT",
            format!("{} and {} cannot be combined", single_flag, level_flag),
        ));
    }
    if let Some(price) = single {
        return Ok(vec![TpSlInput::full(price)]);
    }
    levels
        .iter()
        .map(|s| parse_tpsl_level_str(s, level_flag))
        .collect()
}

fn parse_tpsl_level_str(s: &str, flag: &str) -> Result<TpSlInput, VulcanError> {
    let mut parts = s.splitn(2, ':');
    let price_str = parts.next().unwrap_or("");
    let price: f64 = price_str.trim().parse().map_err(|_| {
        VulcanError::validation(
            "TPSL_LEVEL_PARSE",
            format!("{} expects PRICE[:SIZE_TOKENS], got '{}'", flag, s),
        )
    })?;
    let size = match parts.next() {
        None => TpSlSize::Full,
        Some(sz) => {
            let tokens: f64 = sz.trim().parse().map_err(|_| {
                VulcanError::validation(
                    "TPSL_LEVEL_PARSE",
                    format!("{} expects PRICE[:SIZE_TOKENS], got '{}'", flag, s),
                )
            })?;
            TpSlSize::Tokens(tokens)
        }
    };
    Ok(TpSlInput { price, size })
}

// ── Size resolution ─────────────────────────────────────────────────────

/// Caller-supplied way to express order size.
#[derive(Debug, Clone, Copy)]
pub enum SizeSpec {
    /// Already in base lots — no conversion.
    Lots(f64),
    /// Tokens of the base asset (e.g., 1.18 SOL).
    Tokens(f64),
    /// USDC notional. Quoted against current mid; actual fill differs by spread + impact.
    Notional(f64),
}

#[derive(Debug, Clone, Copy)]
pub struct ResolvedSize {
    pub base_lots: f64,
    pub quoted_mark: Option<f64>,
}

/// Pick exactly one of the three size sources, mapping to a SizeSpec.
/// Returns `validation` errors for ambiguous or missing input.
pub fn size_spec_from_inputs(
    size: Option<f64>,
    tokens: Option<f64>,
    notional_usdc: Option<f64>,
) -> Result<SizeSpec, VulcanError> {
    let count = size.is_some() as u8 + tokens.is_some() as u8 + notional_usdc.is_some() as u8;
    if count == 0 {
        return Err(VulcanError::validation(
            "MISSING_SIZE",
            "One of `size`, `tokens`, or `notional_usdc` is required.",
        ));
    }
    if count > 1 {
        return Err(VulcanError::validation(
            "AMBIGUOUS_SIZE",
            "Provide only one of `size`, `tokens`, or `notional_usdc`.",
        ));
    }
    if let Some(n) = size {
        return Ok(SizeSpec::Lots(n));
    }
    if let Some(n) = tokens {
        return Ok(SizeSpec::Tokens(n));
    }
    Ok(SizeSpec::Notional(notional_usdc.unwrap()))
}

/// Resolve a SizeSpec into base lots, fetching market metadata and a quote
/// price as needed. Adds zero round trips for `Lots`, one (cached) metadata
/// read for `Tokens`, and metadata + a mid-price snapshot fetched in parallel
/// for `Notional`.
pub async fn resolve_base_lots(
    ctx: &AppContext,
    symbol: &str,
    spec: SizeSpec,
) -> Result<ResolvedSize, VulcanError> {
    match spec {
        SizeSpec::Lots(n) => Ok(ResolvedSize {
            base_lots: n,
            quoted_mark: None,
        }),
        SizeSpec::Tokens(n) => {
            let lots = tokens_to_base_lots(ctx, symbol, n).await?;
            Ok(ResolvedSize {
                base_lots: lots,
                quoted_mark: None,
            })
        }
        SizeSpec::Notional(notional) => {
            if notional <= 0.0 {
                return Err(VulcanError::validation(
                    "SIZE_TOO_SMALL",
                    "notional_usdc must be positive.",
                ));
            }
            let (mark_result, metadata_result) = tokio::join!(
                crate::commands::market::fetch_market_quote_price(ctx, symbol),
                ctx.metadata(),
            );
            let mark = mark_result?;
            metadata_result?;
            if mark <= 0.0 {
                return Err(VulcanError::api(
                    "INVALID_MARK_PRICE",
                    format!("Mid price for {} is non-positive: {}", symbol, mark),
                ));
            }
            let tokens = notional / mark;
            let lots = tokens_to_base_lots(ctx, symbol, tokens).await?;
            Ok(ResolvedSize {
                base_lots: lots,
                quoted_mark: Some(mark),
            })
        }
    }
}

/// Resolve a SizeSpec into base lots using the supplied limit price for the
/// `Notional` branch. Unlike `resolve_base_lots`, this never fetches a mark
/// price — the caller's limit price is authoritative for the limit order it is
/// about to place, which is also the price the user expects their notional to
/// be denominated against.
pub async fn resolve_base_lots_with_limit_price(
    ctx: &AppContext,
    symbol: &str,
    spec: SizeSpec,
    limit_price: f64,
) -> Result<ResolvedSize, VulcanError> {
    match spec {
        SizeSpec::Lots(n) => Ok(ResolvedSize {
            base_lots: n,
            quoted_mark: None,
        }),
        SizeSpec::Tokens(n) => {
            let lots = tokens_to_base_lots(ctx, symbol, n).await?;
            Ok(ResolvedSize {
                base_lots: lots,
                quoted_mark: None,
            })
        }
        SizeSpec::Notional(notional) => {
            if notional <= 0.0 {
                return Err(VulcanError::validation(
                    "SIZE_TOO_SMALL",
                    "notional_usdc must be positive.",
                ));
            }
            if limit_price <= 0.0 {
                return Err(VulcanError::validation(
                    "INVALID_PRICE",
                    "limit price must be positive.",
                ));
            }
            let tokens = notional / limit_price;
            let lots = tokens_to_base_lots(ctx, symbol, tokens).await?;
            Ok(ResolvedSize {
                base_lots: lots,
                quoted_mark: Some(limit_price),
            })
        }
    }
}

async fn tokens_to_base_lots(
    ctx: &AppContext,
    symbol: &str,
    tokens: f64,
) -> Result<f64, VulcanError> {
    if tokens <= 0.0 {
        return Err(VulcanError::validation(
            "SIZE_TOO_SMALL",
            "tokens must be positive.",
        ));
    }
    let metadata = ctx.metadata().await?;
    let market = metadata.get_market(symbol).ok_or_else(|| {
        VulcanError::validation("UNKNOWN_MARKET", format!("Unknown market: {}", symbol))
    })?;
    let decimals = market.base_lots_decimals as i32;
    let lots = (tokens * 10f64.powi(decimals)).floor();
    if lots < 1.0 {
        return Err(VulcanError::validation(
            "SIZE_TOO_SMALL",
            format!(
                "Resolved size is < 1 base lot for {} (got {} tokens, {} decimals)",
                symbol, tokens, decimals
            ),
        ));
    }
    Ok(lots)
}

// ── Execution ───────────────────────────────────────────────────────────

pub async fn execute(ctx: &AppContext, cmd: TradeCommand) -> Result<(), VulcanError> {
    match cmd {
        TradeCommand::MarketBuy {
            symbol,
            size,
            tokens,
            notional_usdc,
            tp,
            sl,
            isolated,
            collateral,
            reduce_only,
        } => {
            let spec = size_spec_from_inputs(size, tokens, notional_usdc)?;
            execute_market_order(
                ctx,
                &symbol,
                spec,
                Side::Bid,
                tp,
                sl,
                isolated,
                collateral,
                reduce_only,
            )
            .await
        }
        TradeCommand::MarketSell {
            symbol,
            size,
            tokens,
            notional_usdc,
            tp,
            sl,
            isolated,
            collateral,
            reduce_only,
        } => {
            let spec = size_spec_from_inputs(size, tokens, notional_usdc)?;
            execute_market_order(
                ctx,
                &symbol,
                spec,
                Side::Ask,
                tp,
                sl,
                isolated,
                collateral,
                reduce_only,
            )
            .await
        }
        TradeCommand::LimitBuy {
            symbol,
            size,
            price,
            tp,
            sl,
            isolated,
            collateral,
            reduce_only,
        } => {
            execute_limit_order(
                ctx,
                &symbol,
                size,
                price,
                Side::Bid,
                tp,
                sl,
                isolated,
                collateral,
                reduce_only,
            )
            .await
        }
        TradeCommand::LimitSell {
            symbol,
            size,
            price,
            tp,
            sl,
            isolated,
            collateral,
            reduce_only,
        } => {
            execute_limit_order(
                ctx,
                &symbol,
                size,
                price,
                Side::Ask,
                tp,
                sl,
                isolated,
                collateral,
                reduce_only,
            )
            .await
        }
        TradeCommand::Cancel { symbol, order_ids } => execute_cancel(ctx, &symbol, order_ids).await,
        TradeCommand::CancelAll { symbol } => match symbol {
            Some(s) => execute_cancel_all(ctx, &s).await,
            None => execute_cancel_all_markets(ctx).await,
        },
        TradeCommand::Orders { symbol } => execute_orders(ctx, symbol.as_deref()).await,
        TradeCommand::SetTpsl {
            symbol,
            tp,
            sl,
            tp_levels,
            sl_levels,
        } => {
            let tp_inputs = parse_cli_tpsl_levels("--tp", "--tp-level", tp, &tp_levels)?;
            let sl_inputs = parse_cli_tpsl_levels("--sl", "--sl-level", sl, &sl_levels)?;
            execute_set_tpsl(ctx, &symbol, tp_inputs, sl_inputs).await
        }
        TradeCommand::CancelTpsl { symbol, tp, sl } => {
            execute_cancel_tpsl(ctx, &symbol, tp, sl).await
        }
    }
}

#[allow(clippy::too_many_arguments)]
pub async fn execute_market_order_inner(
    ctx: &AppContext,
    symbol: &str,
    size: f64,
    side: Side,
    tp: Option<f64>,
    sl: Option<f64>,
    isolated: bool,
    collateral: Option<f64>,
    _reduce_only: bool,
) -> Result<OrderResult, VulcanError> {
    let (wallet, authority, trader_pda) = resolve_wallet_and_pda(ctx, None)?;
    let builder = ctx.tx_builder().await?;
    let num_base_lots = size as u64;

    // Pin bracket size to the order's base lots so the resulting conditional
    // orders carry an explicit max_size. The SDK default (size_percent: 100)
    // stores max_size = 0, which Phoenix UI renders as "size 0".
    let bracket = sized_bracket_leg_orders(tp, sl, num_base_lots);

    let ixs = if isolated {
        // Fetch full trader state for isolated order building
        let traders = ctx
            .http_client
            .get_traders(&authority)
            .await
            .map_err(|e| VulcanError::api("TRADERS_FETCH_FAILED", e.to_string()))?;

        let trader = trader_from_views(authority, 0, &traders);
        let conditional_init_target = trader
            .get_or_create_isolated_subaccount_key(symbol)
            .and_then(|sub_key| {
                let creates_subaccount = !trader.subaccount_exists(sub_key.subaccount_index);
                if creates_subaccount || bracket.is_some() {
                    Some(sub_key.pda())
                } else {
                    None
                }
            });

        let collateral_flow = collateral.map(|c| IsolatedCollateralFlow::TransferFromCrossMargin {
            collateral: (c * 1_000_000.0) as u64,
        });

        let mut ixs = builder
            .build_isolated_market_order(
                &trader,
                symbol,
                side,
                num_base_lots,
                collateral_flow,
                true, // allow_cross_and_isolated
                bracket.as_ref(),
            )
            .map_err(|e| VulcanError::api("BUILD_ORDER_FAILED", e.to_string()))?;

        if let Some(trader_pda) = conditional_init_target {
            let init_ixs =
                conditional_orders_init_ixs_if_needed(ctx, &builder, authority, trader_pda).await?;
            insert_before_first_order_or_conditional_ix(&mut ixs, init_ixs);
        }

        ixs
    } else {
        // Check for isolated-only markets
        let metadata = ctx.metadata().await?;
        if metadata.is_isolated_only(symbol) {
            return Err(VulcanError::validation(
                "ISOLATED_ONLY_MARKET",
                format!(
                    "{} is isolated-only. Use --isolated --collateral <AMOUNT>.",
                    symbol
                ),
            ));
        }

        let mut ticket_builder = phoenix_rise::MarketOrderTicket::builder()
            .authority(authority)
            .trader_account(trader_pda)
            .symbol(symbol)
            .side(side)
            .num_base_lots(num_base_lots);

        if let Some(bracket) = bracket {
            let rpc_client = Arc::new(ctx.rpc_client_async());
            ticket_builder = ticket_builder
                .bracket_leg_ticket(phoenix_rise::BracketLegTicket::new(rpc_client, bracket));
        }

        let ticket = ticket_builder
            .build()
            .map_err(|e| VulcanError::api("BUILD_ORDER_FAILED", e.to_string()))?;

        builder
            .place_market_order(ticket)
            .await
            .map_err(|e| VulcanError::api("BUILD_ORDER_FAILED", e.to_string()))?
    };

    let num_ixs = ixs.len();
    let sig = send_or_dry_run(ctx, ixs, &wallet).await?;

    let side_str = match side {
        Side::Bid => "buy",
        Side::Ask => "sell",
    };

    Ok(OrderResult {
        action: format!("market-{}", side_str),
        symbol: symbol.to_string(),
        side: side_str.to_string(),
        size,
        price: None,
        tp,
        sl,
        dry_run: ctx.dry_run,
        tx_signature: sig,
        num_instructions: num_ixs,
        quoted_mark: None,
    })
}

#[allow(clippy::too_many_arguments)]
async fn execute_market_order(
    ctx: &AppContext,
    symbol: &str,
    spec: SizeSpec,
    side: Side,
    tp: Option<f64>,
    sl: Option<f64>,
    isolated: bool,
    collateral: Option<f64>,
    reduce_only: bool,
) -> Result<(), VulcanError> {
    if !ctx.yes && !ctx.dry_run {
        return Err(VulcanError::validation(
            "CONFIRMATION_REQUIRED",
            "Pass --yes to confirm trade, or --dry-run to simulate",
        ));
    }

    let resolved = resolve_base_lots(ctx, symbol, spec).await?;

    let mut result = execute_market_order_inner(
        ctx,
        symbol,
        resolved.base_lots,
        side,
        tp,
        sl,
        isolated,
        collateral,
        reduce_only,
    )
    .await?;
    result.quoted_mark = resolved.quoted_mark;

    let side_str = &result.side;
    render_success(
        ctx.output_format,
        &result,
        serde_json::json!({
            "command": format!("trade market-{}", side_str),
            "symbol": symbol,
            "dry_run": ctx.dry_run,
        }),
    );
    Ok(())
}

#[allow(clippy::too_many_arguments)]
pub async fn execute_limit_order_inner(
    ctx: &AppContext,
    symbol: &str,
    size: f64,
    price: f64,
    side: Side,
    tp: Option<f64>,
    sl: Option<f64>,
    isolated: bool,
    collateral: Option<f64>,
    _reduce_only: bool,
) -> Result<OrderResult, VulcanError> {
    let (wallet, authority, trader_pda) = resolve_wallet_and_pda(ctx, None)?;
    let builder = ctx.tx_builder().await?;
    let num_base_lots = size as u64;

    let bracket = bracket_leg_orders(tp, sl);

    let ixs = if isolated {
        let traders = ctx
            .http_client
            .get_traders(&authority)
            .await
            .map_err(|e| VulcanError::api("TRADERS_FETCH_FAILED", e.to_string()))?;

        let trader = trader_from_views(authority, 0, &traders);
        let conditional_init_target = trader
            .get_or_create_isolated_subaccount_key(symbol)
            .and_then(|sub_key| {
                if !trader.subaccount_exists(sub_key.subaccount_index) {
                    Some(sub_key.pda())
                } else {
                    None
                }
            });

        let collateral_flow = collateral.map(|c| IsolatedCollateralFlow::TransferFromCrossMargin {
            collateral: (c * 1_000_000.0) as u64,
        });

        let mut ixs = builder
            .build_isolated_limit_order(
                &trader,
                symbol,
                side,
                price,
                num_base_lots,
                collateral_flow,
                true, // allow_cross_and_isolated
            )
            .map_err(|e| VulcanError::api("BUILD_ORDER_FAILED", e.to_string()))?;

        if let Some(trader_pda) = conditional_init_target {
            let init_ixs =
                conditional_orders_init_ixs_if_needed(ctx, &builder, authority, trader_pda).await?;
            insert_before_first_order_or_conditional_ix(&mut ixs, init_ixs);
        }

        ixs
    } else {
        let metadata = ctx.metadata().await?;
        if metadata.is_isolated_only(symbol) {
            return Err(VulcanError::validation(
                "ISOLATED_ONLY_MARKET",
                format!(
                    "{} is isolated-only. Use --isolated --collateral <AMOUNT>.",
                    symbol
                ),
            ));
        }

        let mut ticket_builder = phoenix_rise::LimitOrderTicket::builder()
            .authority(authority)
            .trader_account(trader_pda)
            .symbol(symbol)
            .side(side)
            .price(price)
            .num_base_lots(num_base_lots);

        if let Some(bracket) = bracket {
            let rpc_client = Arc::new(ctx.rpc_client_async());
            ticket_builder = ticket_builder
                .bracket_leg_ticket(phoenix_rise::BracketLegTicket::new(rpc_client, bracket));
        }

        let ticket = ticket_builder
            .build()
            .map_err(|e| VulcanError::api("BUILD_ORDER_FAILED", e.to_string()))?;

        builder
            .place_limit_order(ticket)
            .await
            .map_err(|e| VulcanError::api("BUILD_ORDER_FAILED", e.to_string()))?
    };

    let num_ixs = ixs.len();
    let sig = send_or_dry_run(ctx, ixs, &wallet).await?;

    let side_str = match side {
        Side::Bid => "buy",
        Side::Ask => "sell",
    };

    Ok(OrderResult {
        action: format!("limit-{}", side_str),
        symbol: symbol.to_string(),
        side: side_str.to_string(),
        size,
        price: Some(price),
        tp,
        sl,
        dry_run: ctx.dry_run,
        tx_signature: sig,
        num_instructions: num_ixs,
        quoted_mark: None,
    })
}

pub async fn execute_multi_limit_order_inner(
    ctx: &AppContext,
    symbol: &str,
    bids: Vec<MultiLimitLeg>,
    asks: Vec<MultiLimitLeg>,
    slide: bool,
) -> Result<MultiLimitOrderResult, VulcanError> {
    let (wallet, authority, trader_pda) = resolve_wallet_and_pda(ctx, None)?;
    let builder = ctx.tx_builder().await?;

    let metadata = ctx.metadata().await?;
    if metadata.is_isolated_only(symbol) {
        return Err(VulcanError::validation(
            "ISOLATED_ONLY_MARKET",
            format!(
                "{} is isolated-only. Multi-limit orders are not supported for isolated markets.",
                symbol
            ),
        ));
    }

    let any_bracket = bids
        .iter()
        .chain(asks.iter())
        .any(|leg| leg.tp.is_some() || leg.sl.is_some());

    let ixs = if any_bracket {
        // Per-leg bracket path: Phoenix's multi_limit instruction does not
        // accept conditional orders, so when any leg has tp/sl we expand to N
        // individual `place_limit_order_with_conditionals` ixs bundled into a
        // single transaction. `slide` is ignored on this path (the per-leg
        // limit ix is post-only by default and Phoenix's limit builder does
        // not expose a slide option).
        let rpc_client = Arc::new(ctx.rpc_client_async());
        let mut all_ixs: Vec<solana_sdk::instruction::Instruction> = Vec::new();
        for (legs, side) in [(&bids, Side::Bid), (&asks, Side::Ask)] {
            for leg in legs.iter() {
                let mut ticket_builder = phoenix_rise::LimitOrderTicket::builder()
                    .authority(authority)
                    .trader_account(trader_pda)
                    .symbol(symbol)
                    .side(side)
                    .price(leg.price)
                    .num_base_lots(leg.size_lots);

                if let Some(bracket) = bracket_leg_orders(leg.tp, leg.sl) {
                    ticket_builder = ticket_builder.bracket_leg_ticket(
                        phoenix_rise::BracketLegTicket::new(rpc_client.clone(), bracket),
                    );
                }

                let ticket = ticket_builder
                    .build()
                    .map_err(|e| VulcanError::api("BUILD_ORDER_FAILED", e.to_string()))?;

                let leg_ixs = builder
                    .place_limit_order(ticket)
                    .await
                    .map_err(|e| VulcanError::api("BUILD_ORDER_FAILED", e.to_string()))?;
                all_ixs.extend(leg_ixs);
            }
        }
        all_ixs
    } else {
        let bid_tuples: Vec<(f64, u64)> = bids.iter().map(|l| (l.price, l.size_lots)).collect();
        let ask_tuples: Vec<(f64, u64)> = asks.iter().map(|l| (l.price, l.size_lots)).collect();
        builder
            .build_multi_limit_order(
                authority,
                trader_pda,
                symbol,
                &bid_tuples,
                &ask_tuples,
                slide,
            )
            .map_err(|e| VulcanError::api("BUILD_MULTI_ORDER_FAILED", e.to_string()))?
    };

    let num_ixs = ixs.len();
    let sig = if any_bracket {
        // Each place_limit_order_with_conditionals ix burns ~145k CUs; size the
        // tx budget so all N legs fit (capped at Solana's 1.4M per-tx max).
        let cu_limit = ((num_ixs as u32).saturating_mul(175_000)).saturating_add(100_000);
        send_or_dry_run_with_cu_limit(ctx, ixs, &wallet, cu_limit).await?
    } else {
        send_or_dry_run(ctx, ixs, &wallet).await?
    };

    let bid_entries: Vec<MultiLimitOrderEntry> = bids
        .iter()
        .map(|l| MultiLimitOrderEntry {
            side: "buy".to_string(),
            price: l.price,
            size: l.size_lots,
            tp: l.tp,
            sl: l.sl,
        })
        .collect();

    let ask_entries: Vec<MultiLimitOrderEntry> = asks
        .iter()
        .map(|l| MultiLimitOrderEntry {
            side: "sell".to_string(),
            price: l.price,
            size: l.size_lots,
            tp: l.tp,
            sl: l.sl,
        })
        .collect();

    Ok(MultiLimitOrderResult {
        action: "multi-limit".to_string(),
        symbol: symbol.to_string(),
        bids: bid_entries,
        asks: ask_entries,
        slide,
        per_leg_brackets: any_bracket,
        dry_run: ctx.dry_run,
        tx_signature: sig,
        num_instructions: num_ixs,
    })
}

#[allow(clippy::too_many_arguments)]
async fn execute_limit_order(
    ctx: &AppContext,
    symbol: &str,
    size: f64,
    price: f64,
    side: Side,
    tp: Option<f64>,
    sl: Option<f64>,
    isolated: bool,
    collateral: Option<f64>,
    reduce_only: bool,
) -> Result<(), VulcanError> {
    if !ctx.yes && !ctx.dry_run {
        return Err(VulcanError::validation(
            "CONFIRMATION_REQUIRED",
            "Pass --yes to confirm trade, or --dry-run to simulate",
        ));
    }

    let result = execute_limit_order_inner(
        ctx,
        symbol,
        size,
        price,
        side,
        tp,
        sl,
        isolated,
        collateral,
        reduce_only,
    )
    .await?;

    let side_str = &result.side;
    render_success(
        ctx.output_format,
        &result,
        serde_json::json!({
            "command": format!("trade limit-{}", side_str),
            "symbol": symbol,
            "dry_run": ctx.dry_run,
        }),
    );
    Ok(())
}

struct LimitOrderCancelSelection {
    order_ids: Vec<String>,
    cancel_ids: Vec<phoenix_rise::CancelId>,
    needs_fallback: bool,
}

async fn api_limit_order_cancel_selection(
    ctx: &AppContext,
    authority: &Pubkey,
    symbol_upper: &str,
    requested_order_ids: Option<&[String]>,
) -> Result<LimitOrderCancelSelection, VulcanError> {
    let metadata = ctx.metadata().await?;
    let order_data = crate::commands::conditional_orders::fetch_trader_state_order_data(
        ctx, authority, 0, 0, metadata,
    )
    .await?;

    let selected: Vec<_> = order_data
        .limit_orders
        .iter()
        .filter(|o| o.symbol.eq_ignore_ascii_case(symbol_upper))
        .filter(|o| match requested_order_ids {
            Some(ids) => ids.contains(&o.order_sequence_number),
            None => true,
        })
        .collect();

    let missing_requested = requested_order_ids
        .map(|ids| {
            ids.iter()
                .any(|id| !selected.iter().any(|o| &o.order_sequence_number == id))
        })
        .unwrap_or(false);
    let needs_fallback = match requested_order_ids {
        Some(_) => missing_requested,
        None => order_data.has_unparsed_limit_orders,
    };

    Ok(LimitOrderCancelSelection {
        order_ids: selected
            .iter()
            .map(|o| o.order_sequence_number.clone())
            .collect(),
        cancel_ids: selected
            .iter()
            .map(|o| phoenix_rise::CancelId::new(o.price_ticks, o.order_sequence_number_u64))
            .collect(),
        needs_fallback,
    })
}

async fn legacy_limit_order_cancel_selection(
    ctx: &AppContext,
    authority: &Pubkey,
    symbol_upper: &str,
    requested_order_ids: Option<&[String]>,
) -> Result<LimitOrderCancelSelection, VulcanError> {
    let traders = ctx
        .http_client
        .get_traders(authority)
        .await
        .map_err(|e| VulcanError::api("TRADERS_FETCH_FAILED", e.to_string()))?;

    let trader = traders
        .iter()
        .find(|t| t.trader_subaccount_index == 0)
        .ok_or_else(|| {
            VulcanError::api("NO_TRADER_ACCOUNT", "No registered trader account found")
        })?;

    let orders = trader
        .limit_orders
        .get(symbol_upper)
        .cloned()
        .unwrap_or_default();

    let metadata = ctx.metadata().await?;
    let calc = metadata
        .get_market_calculator(symbol_upper)
        .ok_or_else(|| {
            VulcanError::validation(
                "UNKNOWN_MARKET",
                format!("Unknown market: {}", symbol_upper),
            )
        })?;

    let selected: Vec<_> = orders
        .iter()
        .filter(|o| match requested_order_ids {
            Some(ids) => ids.contains(&o.order_sequence_number),
            None => true,
        })
        .collect();

    Ok(LimitOrderCancelSelection {
        order_ids: selected
            .iter()
            .map(|o| o.order_sequence_number.clone())
            .collect(),
        cancel_ids: selected
            .iter()
            .map(|o| {
                let price_f64 = o.price.value as f64 / 10f64.powi(o.price.decimals as i32);
                let ticks = calc.price_to_ticks(price_f64).unwrap_or_default();
                phoenix_rise::CancelId::new(
                    ticks.into(),
                    o.order_sequence_number.parse::<u64>().unwrap_or(0),
                )
            })
            .collect(),
        needs_fallback: false,
    })
}

pub async fn execute_cancel_inner(
    ctx: &AppContext,
    symbol: &str,
    order_ids: Vec<String>,
) -> Result<CancelResult, VulcanError> {
    let (wallet, authority, trader_pda) = resolve_wallet_and_pda(ctx, None)?;
    let symbol_upper = symbol.to_ascii_uppercase();

    let selection = match api_limit_order_cancel_selection(
        ctx,
        &authority,
        &symbol_upper,
        Some(order_ids.as_slice()),
    )
    .await
    {
        Ok(selection) if !selection.needs_fallback => selection,
        _ => {
            legacy_limit_order_cancel_selection(
                ctx,
                &authority,
                &symbol_upper,
                Some(order_ids.as_slice()),
            )
            .await?
        }
    };

    if selection.cancel_ids.is_empty() {
        return Err(VulcanError::validation(
            "INVALID_ORDER_IDS",
            "No matching open orders found for the provided IDs",
        ));
    }

    let builder = ctx.tx_builder().await?;
    let ixs = builder
        .build_cancel_orders(authority, trader_pda, symbol, selection.cancel_ids)
        .map_err(|e| VulcanError::api("BUILD_CANCEL_FAILED", e.to_string()))?;

    let num_ixs = ixs.len();
    let sig = send_or_dry_run(ctx, ixs, &wallet).await?;

    Ok(CancelResult {
        symbol: symbol.to_string(),
        cancelled_ids: selection.order_ids,
        dry_run: ctx.dry_run,
        tx_signature: sig,
        num_instructions: num_ixs,
    })
}

async fn execute_cancel(
    ctx: &AppContext,
    symbol: &str,
    order_ids: Vec<String>,
) -> Result<(), VulcanError> {
    if !ctx.yes && !ctx.dry_run {
        return Err(VulcanError::validation(
            "CONFIRMATION_REQUIRED",
            "Pass --yes to confirm cancellation, or --dry-run to simulate",
        ));
    }

    let result = execute_cancel_inner(ctx, symbol, order_ids).await?;

    render_success(
        ctx.output_format,
        &result,
        serde_json::json!({ "command": "trade cancel", "symbol": symbol, "dry_run": ctx.dry_run }),
    );
    Ok(())
}

pub async fn execute_cancel_all_inner(
    ctx: &AppContext,
    symbol: &str,
) -> Result<CancelResult, VulcanError> {
    let (wallet, authority, trader_pda) = resolve_wallet_and_pda(ctx, None)?;
    let symbol_upper = symbol.to_ascii_uppercase();

    let selection =
        match api_limit_order_cancel_selection(ctx, &authority, &symbol_upper, None).await {
            Ok(selection) if !selection.needs_fallback => selection,
            _ => legacy_limit_order_cancel_selection(ctx, &authority, &symbol_upper, None).await?,
        };

    if selection.cancel_ids.is_empty() {
        return Ok(CancelResult {
            symbol: symbol.to_string(),
            cancelled_ids: vec![],
            dry_run: ctx.dry_run,
            tx_signature: None,
            num_instructions: 0,
        });
    }

    let builder = ctx.tx_builder().await?;
    let ixs = builder
        .build_cancel_orders(authority, trader_pda, symbol, selection.cancel_ids)
        .map_err(|e| VulcanError::api("BUILD_CANCEL_FAILED", e.to_string()))?;

    let num_ixs = ixs.len();
    let sig = send_or_dry_run(ctx, ixs, &wallet).await?;

    Ok(CancelResult {
        symbol: symbol.to_string(),
        cancelled_ids: selection.order_ids,
        dry_run: ctx.dry_run,
        tx_signature: sig,
        num_instructions: num_ixs,
    })
}

async fn execute_cancel_all(ctx: &AppContext, symbol: &str) -> Result<(), VulcanError> {
    if !ctx.yes && !ctx.dry_run {
        return Err(VulcanError::validation(
            "CONFIRMATION_REQUIRED",
            "Pass --yes to confirm cancellation, or --dry-run to simulate",
        ));
    }

    let result = execute_cancel_all_inner(ctx, symbol).await?;

    render_success(
        ctx.output_format,
        &result,
        serde_json::json!({ "command": "trade cancel-all", "symbol": symbol, "dry_run": ctx.dry_run }),
    );
    Ok(())
}

pub async fn execute_cancel_all_markets_inner(
    ctx: &AppContext,
) -> Result<CancelAllMarketsResult, VulcanError> {
    let orders = execute_orders_inner(ctx, None).await?;

    let mut symbols: Vec<String> = orders
        .orders
        .iter()
        .map(|o| o.symbol.to_ascii_uppercase())
        .collect();
    symbols.sort();
    symbols.dedup();

    let mut per_market = Vec::with_capacity(symbols.len());
    let mut total_cancelled = 0usize;
    for symbol in &symbols {
        let result = execute_cancel_all_inner(ctx, symbol).await?;
        total_cancelled += result.cancelled_ids.len();
        per_market.push(result);
    }

    let markets_touched = per_market
        .iter()
        .filter(|r| !r.cancelled_ids.is_empty())
        .count();

    Ok(CancelAllMarketsResult {
        per_market,
        total_cancelled,
        markets_touched,
        dry_run: ctx.dry_run,
    })
}

async fn execute_cancel_all_markets(ctx: &AppContext) -> Result<(), VulcanError> {
    if !ctx.yes && !ctx.dry_run {
        return Err(VulcanError::validation(
            "CONFIRMATION_REQUIRED",
            "Pass --yes to confirm cancelling orders across all markets, or --dry-run to simulate",
        ));
    }

    let result = execute_cancel_all_markets_inner(ctx).await?;

    render_success(
        ctx.output_format,
        &result,
        serde_json::json!({ "command": "trade cancel-all", "scope": "all-markets", "dry_run": ctx.dry_run }),
    );
    Ok(())
}

pub async fn execute_orders_inner(
    ctx: &AppContext,
    symbol: Option<&str>,
) -> Result<OrdersResult, VulcanError> {
    let authority = resolve_authority(ctx)?;

    match execute_orders_inner_api(ctx, &authority, symbol).await {
        Ok(result) => Ok(result),
        Err(_) => execute_orders_inner_legacy(ctx, &authority, symbol).await,
    }
}

async fn execute_orders_inner_api(
    ctx: &AppContext,
    authority: &Pubkey,
    symbol: Option<&str>,
) -> Result<OrdersResult, VulcanError> {
    let metadata = ctx.metadata().await?;
    let order_data = crate::commands::conditional_orders::fetch_trader_state_order_data(
        ctx, authority, 0, 0, metadata,
    )
    .await?;

    let mut orders = crate::commands::conditional_orders::trader_state_limit_orders_to_order_infos(
        &order_data.limit_orders,
        symbol,
    );
    orders.extend(
        crate::commands::conditional_orders::triggers_to_order_infos(
            &order_data.conditional_triggers,
            symbol,
        ),
    );

    Ok(OrdersResult {
        symbol: symbol.map(|s| s.to_ascii_uppercase()),
        orders,
    })
}

async fn execute_orders_inner_legacy(
    ctx: &AppContext,
    authority: &Pubkey,
    symbol: Option<&str>,
) -> Result<OrdersResult, VulcanError> {
    let traders = ctx
        .http_client
        .get_traders(authority)
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

    let mut result = project_orders(trader, symbol);

    let metadata = ctx.metadata().await?;
    let triggers =
        crate::commands::conditional_orders::fetch_conditional_triggers(ctx, trader, metadata)
            .await?;
    let extra = crate::commands::conditional_orders::triggers_to_order_infos(&triggers, symbol);
    result.orders.extend(extra);

    Ok(result)
}

pub(crate) fn project_orders(
    trader: &phoenix_rise::types::TraderView,
    symbol: Option<&str>,
) -> OrdersResult {
    let order_infos = match symbol {
        Some(sym) => {
            let symbol_upper = sym.to_ascii_uppercase();
            let orders = trader
                .limit_orders
                .get(&symbol_upper)
                .cloned()
                .unwrap_or_default();
            orders_to_infos(&symbol_upper, &orders)
        }
        None => {
            let mut all = Vec::new();
            for (sym, orders) in &trader.limit_orders {
                all.extend(orders_to_infos(sym, orders));
            }
            all
        }
    };

    OrdersResult {
        symbol: symbol.map(|s| s.to_ascii_uppercase()),
        orders: order_infos,
    }
}

fn orders_to_infos(symbol: &str, orders: &[phoenix_rise::types::LimitOrder]) -> Vec<OrderInfo> {
    orders
        .iter()
        .map(|o| {
            let side = match o.side {
                phoenix_rise::types::Side::Bid => "Buy",
                phoenix_rise::types::Side::Ask => "Sell",
            };
            OrderInfo {
                symbol: symbol.to_string(),
                side: side.to_string(),
                order_id: o.order_sequence_number.clone(),
                price: o.price.ui.clone(),
                size_remaining: o.trade_size_remaining.ui.clone(),
                initial_size: o.initial_trade_size.ui.clone(),
                reduce_only: o.is_reduce_only,
                is_stop_loss: o.is_stop_loss,
                is_take_profit: false,
            }
        })
        .collect()
}

async fn execute_orders(ctx: &AppContext, symbol: Option<&str>) -> Result<(), VulcanError> {
    let result = execute_orders_inner(ctx, symbol).await?;

    render_success(
        ctx.output_format,
        &result,
        serde_json::json!({ "command": "trade orders", "symbol": symbol }),
    );

    if ctx.watch {
        let authority = resolve_authority(ctx)?;
        let sym = symbol.map(|s| s.to_string());
        crate::watch::watch_loop(ctx, crate::watch::WatchKind::TraderState(authority), || {
            let sym = sym.clone();
            async move {
                let result = execute_orders_inner(ctx, sym.as_deref()).await?;
                render_success(
                    ctx.output_format,
                    &result,
                    serde_json::json!({ "command": "trade orders", "symbol": sym }),
                );
                Ok(())
            }
        })
        .await?;
    }

    Ok(())
}

// ── TP/SL result types ─────────────────────────────────────────────────

#[derive(Debug, Serialize)]
pub struct TpSlLevelOut {
    pub price: f64,
    pub size_lots: u64,
}

#[derive(Debug, Serialize)]
pub struct SetTpSlResult {
    pub symbol: String,
    pub side: String,
    pub tp_levels: Vec<TpSlLevelOut>,
    pub sl_levels: Vec<TpSlLevelOut>,
    pub dry_run: bool,
    pub tx_signature: Option<String>,
    pub num_instructions: usize,
}

impl TableRenderable for SetTpSlResult {
    fn render_table(&self) {
        if self.dry_run {
            println!(
                "[DRY RUN] Would set TP/SL on {} {} position:",
                self.symbol, self.side
            );
        } else {
            println!("TP/SL set on {} {} position:", self.symbol, self.side);
        }
        for lvl in &self.tp_levels {
            println!(
                "  Take profit: ${:.4} ({} base lots)",
                lvl.price, lvl.size_lots
            );
        }
        for lvl in &self.sl_levels {
            println!(
                "  Stop loss:   ${:.4} ({} base lots)",
                lvl.price, lvl.size_lots
            );
        }
        println!("  Instructions: {}", self.num_instructions);
        if let Some(sig) = &self.tx_signature {
            println!("  Tx: {}", sig);
        }
    }
}

#[derive(Debug, Serialize)]
pub struct CancelTpSlResult {
    pub symbol: String,
    pub cancelled_tp: bool,
    pub cancelled_sl: bool,
    pub dry_run: bool,
    pub tx_signature: Option<String>,
    pub num_instructions: usize,
}

impl TableRenderable for CancelTpSlResult {
    fn render_table(&self) {
        let mut legs = Vec::new();
        if self.cancelled_tp {
            legs.push("TP");
        }
        if self.cancelled_sl {
            legs.push("SL");
        }
        if self.dry_run {
            println!(
                "[DRY RUN] Would cancel {} on {}:",
                legs.join("/"),
                self.symbol
            );
        } else {
            println!("Cancelled {} on {}:", legs.join("/"), self.symbol);
        }
        println!("  Instructions: {}", self.num_instructions);
        if let Some(sig) = &self.tx_signature {
            println!("  Tx: {}", sig);
        }
    }
}

// ── set-tpsl ───────────────────────────────────────────────────────────

pub async fn execute_set_tpsl_inner(
    ctx: &AppContext,
    symbol: &str,
    tp_levels: Vec<TpSlInput>,
    sl_levels: Vec<TpSlInput>,
) -> Result<SetTpSlResult, VulcanError> {
    if tp_levels.is_empty() && sl_levels.is_empty() {
        return Err(VulcanError::validation(
            "NO_TP_SL",
            "Specify at least one TP or SL level",
        ));
    }

    let (wallet, authority, _) = resolve_wallet_and_pda(ctx, None)?;
    let symbol_upper = symbol.to_ascii_uppercase();

    // Fetch trader state to detect position side
    let traders = ctx
        .http_client
        .get_traders(&authority)
        .await
        .map_err(|e| VulcanError::api("TRADERS_FETCH_FAILED", e.to_string()))?;

    let (trader_view, pos) = traders
        .iter()
        .find_map(|t| {
            t.positions
                .iter()
                .find(|p| p.symbol.to_ascii_uppercase() == symbol_upper)
                .map(|p| (t, p))
        })
        .ok_or_else(|| {
            VulcanError::validation(
                "NO_POSITION",
                format!(
                    "No open position for '{}'. TP/SL requires an existing position.",
                    symbol
                ),
            )
        })?;

    let is_long = !pos.position_size.ui.starts_with('-');
    let primary_side = if is_long { Side::Bid } else { Side::Ask };
    let side_str = if is_long { "Long" } else { "Short" };
    let position_lots = pos.position_size.value.unsigned_abs();
    let trader_pda = TraderKey::derive_pda(
        &authority,
        trader_view.trader_pda_index,
        trader_view.trader_subaccount_index,
    );

    // Resolve every input level to an explicit base-lot size. We pin sizes
    // (rather than using the SDK default `size_percent: 100`) so on-chain
    // `max_size` is non-zero and Phoenix UI / keepers behave correctly.
    let resolved_tp = resolve_tpsl_levels(ctx, &symbol_upper, &tp_levels, position_lots).await?;
    let resolved_sl = resolve_tpsl_levels(ctx, &symbol_upper, &sl_levels, position_lots).await?;

    let total_tp: u64 = resolved_tp.iter().map(|l| l.size_lots).sum();
    let total_sl: u64 = resolved_sl.iter().map(|l| l.size_lots).sum();
    if total_tp > position_lots {
        return Err(VulcanError::validation(
            "TP_SIZE_EXCEEDS_POSITION",
            format!(
                "Sum of TP level sizes ({} lots) exceeds position size ({} lots)",
                total_tp, position_lots
            ),
        ));
    }
    if total_sl > position_lots {
        return Err(VulcanError::validation(
            "SL_SIZE_EXCEEDS_POSITION",
            format!(
                "Sum of SL level sizes ({} lots) exceeds position size ({} lots)",
                total_sl, position_lots
            ),
        ));
    }

    let builder = ctx.tx_builder().await?;
    let mut all_ixs =
        conditional_orders_init_ixs_if_needed(ctx, &builder, authority, trader_pda).await?;
    for level in &resolved_tp {
        let bracket = BracketLegOrders {
            take_profit: Some(
                BracketLeg::new(level.price).with_size(BracketLegSize::BaseLots(level.size_lots)),
            ),
            stop_loss: None,
        };
        let ixs = builder
            .build_bracket_leg_orders(authority, trader_pda, &symbol_upper, primary_side, &bracket)
            .map_err(|e| VulcanError::api("BUILD_TPSL_FAILED", e.to_string()))?;
        all_ixs.extend(ixs);
    }
    for level in &resolved_sl {
        let bracket = BracketLegOrders {
            take_profit: None,
            stop_loss: Some(
                BracketLeg::new(level.price).with_size(BracketLegSize::BaseLots(level.size_lots)),
            ),
        };
        let ixs = builder
            .build_bracket_leg_orders(authority, trader_pda, &symbol_upper, primary_side, &bracket)
            .map_err(|e| VulcanError::api("BUILD_TPSL_FAILED", e.to_string()))?;
        all_ixs.extend(ixs);
    }

    let num_ixs = all_ixs.len();
    let sig = send_or_dry_run(ctx, all_ixs, &wallet).await?;

    Ok(SetTpSlResult {
        symbol: symbol_upper,
        side: side_str.to_string(),
        tp_levels: resolved_tp,
        sl_levels: resolved_sl,
        dry_run: ctx.dry_run,
        tx_signature: sig,
        num_instructions: num_ixs,
    })
}

async fn resolve_tpsl_levels(
    ctx: &AppContext,
    symbol: &str,
    inputs: &[TpSlInput],
    position_lots: u64,
) -> Result<Vec<TpSlLevelOut>, VulcanError> {
    if inputs.is_empty() {
        return Ok(Vec::new());
    }
    let full_count = inputs
        .iter()
        .filter(|i| matches!(i.size, TpSlSize::Full))
        .count();
    if inputs.len() > 1 && full_count > 0 {
        return Err(VulcanError::validation(
            "TPSL_FULL_WITH_MULTI_LEVEL",
            "When using multiple TP/SL levels on a side, every level must specify an explicit size",
        ));
    }
    let mut out = Vec::with_capacity(inputs.len());
    for inp in inputs {
        let size_lots = match inp.size {
            TpSlSize::Full => position_lots,
            TpSlSize::Lots(n) => n,
            TpSlSize::Tokens(t) => tokens_to_base_lots(ctx, symbol, t).await? as u64,
        };
        if size_lots == 0 {
            return Err(VulcanError::validation(
                "TPSL_SIZE_TOO_SMALL",
                format!("Resolved TP/SL size is 0 base lots for price {}", inp.price),
            ));
        }
        out.push(TpSlLevelOut {
            price: inp.price,
            size_lots,
        });
    }
    Ok(out)
}

async fn execute_set_tpsl(
    ctx: &AppContext,
    symbol: &str,
    tp_levels: Vec<TpSlInput>,
    sl_levels: Vec<TpSlInput>,
) -> Result<(), VulcanError> {
    if !ctx.yes && !ctx.dry_run {
        return Err(VulcanError::validation(
            "CONFIRMATION_REQUIRED",
            "Pass --yes to confirm, or --dry-run to simulate",
        ));
    }

    let result = execute_set_tpsl_inner(ctx, symbol, tp_levels, sl_levels).await?;
    let symbol_upper = result.symbol.clone();

    render_success(
        ctx.output_format,
        &result,
        serde_json::json!({
            "command": "trade set-tpsl",
            "symbol": symbol_upper,
            "dry_run": ctx.dry_run,
        }),
    );
    Ok(())
}

// ── cancel-tpsl ────────────────────────────────────────────────────────

pub async fn execute_cancel_tpsl_inner(
    ctx: &AppContext,
    symbol: &str,
    cancel_tp: bool,
    cancel_sl: bool,
) -> Result<CancelTpSlResult, VulcanError> {
    if !cancel_tp && !cancel_sl {
        return Err(VulcanError::validation(
            "NO_TP_SL",
            "Specify at least one of --tp or --sl to cancel",
        ));
    }

    let (wallet, authority, _) = resolve_wallet_and_pda(ctx, None)?;
    let symbol_upper = symbol.to_ascii_uppercase();

    let traders = ctx
        .http_client
        .get_traders(&authority)
        .await
        .map_err(|e| VulcanError::api("TRADERS_FETCH_FAILED", e.to_string()))?;

    let (trader, pos) = traders
        .iter()
        .find_map(|t| {
            t.positions
                .iter()
                .find(|p| p.symbol.to_ascii_uppercase() == symbol_upper)
                .map(|p| (t, p))
        })
        .ok_or_else(|| {
            VulcanError::validation("NO_POSITION", format!("No open position for '{}'", symbol))
        })?;

    let trader_pda = TraderKey::derive_pda(
        &authority,
        trader.trader_pda_index,
        trader.trader_subaccount_index,
    );
    let is_long = !pos.position_size.ui.starts_with('-');

    // Prefer trader-state API IDs for multi-level TP/SL cancellation. They
    // encode the on-chain conditional-order index, avoiding an RPC account
    // fetch in the normal case. Fall back to the RPC decoder if the API ID
    // shape is missing or invalid.
    let api_cond_ixs = match build_api_conditional_cancel_ixs(
        ctx,
        trader,
        authority,
        trader_pda,
        &symbol_upper,
        cancel_tp,
        cancel_sl,
    )
    .await
    {
        Ok(Some(ixs)) if !ixs.is_empty() => Some(ixs),
        Ok(_) | Err(_) => None,
    };

    let ixs = if let Some(ixs) = api_cond_ixs {
        ixs
    } else {
        let cond_ixs = build_rpc_conditional_cancel_ixs(
            ctx,
            trader,
            authority,
            trader_pda,
            &symbol_upper,
            is_long,
            cancel_tp,
            cancel_sl,
        )
        .await?;
        if !cond_ixs.is_empty() {
            cond_ixs
        } else {
            let builder = ctx.tx_builder().await?;
            // Fall back to the legacy single-SL/TP cancel path.
            // For longs: TP triggers GreaterThan, SL triggers LessThan
            // For shorts: TP triggers LessThan, SL triggers GreaterThan
            let mut ixs = Vec::new();
            if cancel_tp {
                let tp_direction = if is_long {
                    phoenix_rise::Direction::GreaterThan
                } else {
                    phoenix_rise::Direction::LessThan
                };
                let tp_ixs = builder
                    .build_cancel_bracket_leg(authority, trader_pda, &symbol_upper, tp_direction)
                    .map_err(|e| VulcanError::api("BUILD_CANCEL_TP_FAILED", e.to_string()))?;
                ixs.extend(tp_ixs);
            }
            if cancel_sl {
                let sl_direction = if is_long {
                    phoenix_rise::Direction::LessThan
                } else {
                    phoenix_rise::Direction::GreaterThan
                };
                let sl_ixs = builder
                    .build_cancel_bracket_leg(authority, trader_pda, &symbol_upper, sl_direction)
                    .map_err(|e| VulcanError::api("BUILD_CANCEL_SL_FAILED", e.to_string()))?;
                ixs.extend(sl_ixs);
            }
            ixs
        }
    };

    let num_ixs = ixs.len();
    let sig = send_or_dry_run(ctx, ixs, &wallet).await?;

    Ok(CancelTpSlResult {
        symbol: symbol_upper,
        cancelled_tp: cancel_tp,
        cancelled_sl: cancel_sl,
        dry_run: ctx.dry_run,
        tx_signature: sig,
        num_instructions: num_ixs,
    })
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum ConditionalTriggerDirection {
    Greater,
    Less,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct ParsedConditionalOrderId {
    conditional_order_index: u8,
    direction: ConditionalTriggerDirection,
}

fn parse_conditional_order_id(
    order_id: &str,
    expected_kind: crate::commands::conditional_orders::TriggerKind,
    expected_asset_id: u32,
) -> Option<ParsedConditionalOrderId> {
    let mut parts = order_id.split('-');
    let prefix = parts.next()?;
    let asset_id = parts.next()?.parse::<u32>().ok()?;
    let conditional_order_index = parts.next()?.parse::<u8>().ok()?;
    let direction = match parts.next()? {
        "gt" => ConditionalTriggerDirection::Greater,
        "lt" => ConditionalTriggerDirection::Less,
        _ => return None,
    };
    if parts.next().is_some() {
        return None;
    }

    let expected_prefix = match expected_kind {
        crate::commands::conditional_orders::TriggerKind::TakeProfit => "ctp",
        crate::commands::conditional_orders::TriggerKind::StopLoss => "csl",
    };
    if prefix != expected_prefix || asset_id != expected_asset_id {
        return None;
    }

    Some(ParsedConditionalOrderId {
        conditional_order_index,
        direction,
    })
}

/// Build per-index cancel instructions from trader-state API trigger IDs.
/// Returns `Ok(None)` when an ID cannot be parsed/validated, signaling that
/// the caller should use the RPC conditional-order decoder fallback.
async fn build_api_conditional_cancel_ixs(
    ctx: &AppContext,
    trader: &TraderView,
    authority: Pubkey,
    trader_pda: Pubkey,
    symbol_upper: &str,
    cancel_tp: bool,
    cancel_sl: bool,
) -> Result<Option<Vec<solana_sdk::instruction::Instruction>>, VulcanError> {
    let metadata = ctx.metadata().await?;
    let market = metadata.get_market(symbol_upper).ok_or_else(|| {
        VulcanError::validation(
            "UNKNOWN_MARKET",
            format!("Unknown market: {}", symbol_upper),
        )
    })?;
    let market_asset_id = market.asset_id;
    let orderbook = Pubkey::from_str(&market.market_pubkey)
        .map_err(|e| VulcanError::validation("INVALID_MARKET_PUBKEY", e.to_string()))?;

    let triggers = crate::commands::conditional_orders::fetch_conditional_triggers_for_authority(
        ctx,
        &authority,
        trader.trader_pda_index,
        trader.trader_subaccount_index,
        metadata,
    )
    .await?;
    let mut by_index: BTreeMap<u8, (bool, bool)> = BTreeMap::new();

    for trigger in triggers
        .iter()
        .filter(|t| t.symbol.eq_ignore_ascii_case(symbol_upper))
    {
        let should_cancel = match trigger.kind {
            crate::commands::conditional_orders::TriggerKind::TakeProfit => cancel_tp,
            crate::commands::conditional_orders::TriggerKind::StopLoss => cancel_sl,
        };
        if !should_cancel {
            continue;
        }

        let Some(parsed) =
            parse_conditional_order_id(&trigger.order_id, trigger.kind, market_asset_id)
        else {
            return Ok(None);
        };

        let entry = by_index
            .entry(parsed.conditional_order_index)
            .or_insert((false, false));
        match parsed.direction {
            ConditionalTriggerDirection::Greater => entry.0 = true,
            ConditionalTriggerDirection::Less => entry.1 = true,
        }
    }

    let mut ixs = Vec::new();
    for (index, (disable_first, disable_second)) in by_index {
        if !disable_first && !disable_second {
            continue;
        }
        ixs.push(build_cancel_conditional_order_ix(
            authority,
            trader_pda,
            orderbook,
            index,
            disable_first,
            disable_second,
        )?);
    }

    Ok(Some(ixs))
}

/// Build per-index cancel instructions against the `ConditionalOrderCollection`
/// for every active leg matching the requested direction(s). Returns an empty
/// vec when the collection account does not exist (caller falls back to the
/// legacy single-SL/TP cancel path).
///
/// Disable mapping (matches the on-chain `cancel_trigger(disable_greater,
/// disable_less)`):
/// - `disable_first`  → `greater_trigger_order` (long TP / short SL)
/// - `disable_second` → `less_trigger_order`    (long SL / short TP)
#[allow(clippy::too_many_arguments)]
async fn build_rpc_conditional_cancel_ixs(
    ctx: &AppContext,
    trader: &TraderView,
    authority: Pubkey,
    trader_pda: Pubkey,
    symbol_upper: &str,
    is_long: bool,
    cancel_tp: bool,
    cancel_sl: bool,
) -> Result<Vec<solana_sdk::instruction::Instruction>, VulcanError> {
    let Some(collection) =
        crate::commands::conditional_orders::fetch_conditional_orders(ctx, trader).await?
    else {
        return Ok(Vec::new());
    };

    let metadata = ctx.metadata().await?;
    let market = metadata.get_market(symbol_upper).ok_or_else(|| {
        VulcanError::validation(
            "UNKNOWN_MARKET",
            format!("Unknown market: {}", symbol_upper),
        )
    })?;
    let market_asset_id = market.asset_id;
    let orderbook = Pubkey::from_str(&market.market_pubkey)
        .map_err(|e| VulcanError::validation("INVALID_MARKET_PUBKEY", e.to_string()))?;

    let mut ixs = Vec::new();
    for (index, order) in collection.active_orders() {
        if order.asset_id != market_asset_id {
            continue;
        }
        let greater_active = order.greater_trigger_order.is_active;
        let less_active = order.less_trigger_order.is_active;

        // Map (cancel_tp, cancel_sl) onto (disable_first, disable_second) by
        // position side, then mask against actually-active legs.
        let (cancel_greater, cancel_less) = if is_long {
            (cancel_tp, cancel_sl)
        } else {
            (cancel_sl, cancel_tp)
        };
        let disable_first = cancel_greater && greater_active;
        let disable_second = cancel_less && less_active;
        if !disable_first && !disable_second {
            continue;
        }

        ixs.push(build_cancel_conditional_order_ix(
            authority,
            trader_pda,
            orderbook,
            index,
            disable_first,
            disable_second,
        )?);
    }
    Ok(ixs)
}

fn build_cancel_conditional_order_ix(
    authority: Pubkey,
    trader_pda: Pubkey,
    orderbook: Pubkey,
    conditional_order_index: u8,
    disable_first: bool,
    disable_second: bool,
) -> Result<solana_sdk::instruction::Instruction, VulcanError> {
    let params = phoenix_rise::CancelConditionalOrderParams::builder()
        .trader_account(trader_pda)
        .trader_wallet(authority)
        .orderbook(orderbook)
        .conditional_order_index(conditional_order_index)
        .disable_first(disable_first)
        .disable_second(disable_second)
        .build()
        .map_err(|e| VulcanError::api("BUILD_CANCEL_COND_FAILED", e.to_string()))?;
    let ix = phoenix_rise::phoenix_rise_ix::create_cancel_conditional_order_ix(params)
        .map_err(|e| VulcanError::api("BUILD_CANCEL_COND_FAILED", e.to_string()))?;
    Ok(ix.into())
}

async fn execute_cancel_tpsl(
    ctx: &AppContext,
    symbol: &str,
    cancel_tp: bool,
    cancel_sl: bool,
) -> Result<(), VulcanError> {
    if !ctx.yes && !ctx.dry_run {
        return Err(VulcanError::validation(
            "CONFIRMATION_REQUIRED",
            "Pass --yes to confirm, or --dry-run to simulate",
        ));
    }

    let result = execute_cancel_tpsl_inner(ctx, symbol, cancel_tp, cancel_sl).await?;
    let symbol_upper = result.symbol.clone();

    render_success(
        ctx.output_format,
        &result,
        serde_json::json!({
            "command": "trade cancel-tpsl",
            "symbol": symbol_upper,
            "dry_run": ctx.dry_run,
        }),
    );
    Ok(())
}

#[cfg(test)]
mod conditional_orders_init_tests {
    use super::*;

    fn test_ix(program_id: Pubkey, data: Vec<u8>) -> solana_sdk::instruction::Instruction {
        solana_sdk::instruction::Instruction {
            program_id,
            accounts: Vec::new(),
            data,
        }
    }

    fn phoenix_ix(discriminant: [u8; 8]) -> solana_sdk::instruction::Instruction {
        test_ix(
            phoenix_rise::phoenix_rise_ix::PHOENIX_PROGRAM_ID,
            discriminant.to_vec(),
        )
    }

    #[test]
    fn inserts_init_before_first_market_order() {
        let setup_ix = test_ix(Pubkey::new_unique(), vec![1]);
        let init_ix = test_ix(Pubkey::new_unique(), vec![2]);
        let market_ix =
            phoenix_ix(phoenix_rise::phoenix_rise_ix::place_market_order_discriminant());
        let conditional_ix = phoenix_ix(
            phoenix_rise::phoenix_rise_ix::place_position_conditional_order_discriminant(),
        );
        let mut ixs = vec![setup_ix.clone(), market_ix.clone(), conditional_ix.clone()];

        insert_before_first_order_or_conditional_ix(&mut ixs, vec![init_ix.clone()]);

        assert_eq!(ixs[0].data, setup_ix.data);
        assert_eq!(ixs[1].data, init_ix.data);
        assert_eq!(ixs[2].data, market_ix.data);
        assert_eq!(ixs[3].data, conditional_ix.data);
    }

    #[test]
    fn inserts_init_before_first_position_conditional_when_no_order_ix_exists() {
        let setup_ix = test_ix(Pubkey::new_unique(), vec![1]);
        let init_ix = test_ix(Pubkey::new_unique(), vec![2]);
        let conditional_ix = phoenix_ix(
            phoenix_rise::phoenix_rise_ix::place_position_conditional_order_discriminant(),
        );
        let mut ixs = vec![setup_ix.clone(), conditional_ix.clone()];

        insert_before_first_order_or_conditional_ix(&mut ixs, vec![init_ix.clone()]);

        assert_eq!(ixs[0].data, setup_ix.data);
        assert_eq!(ixs[1].data, init_ix.data);
        assert_eq!(ixs[2].data, conditional_ix.data);
    }
}

#[cfg(test)]
mod size_spec_tests {
    use super::*;

    #[test]
    fn lots_only_returns_lots_variant() {
        let spec = size_spec_from_inputs(Some(118.0), None, None).unwrap();
        assert!(matches!(spec, SizeSpec::Lots(n) if (n - 118.0).abs() < f64::EPSILON));
    }

    #[test]
    fn tokens_only_returns_tokens_variant() {
        let spec = size_spec_from_inputs(None, Some(1.18), None).unwrap();
        assert!(matches!(spec, SizeSpec::Tokens(n) if (n - 1.18).abs() < f64::EPSILON));
    }

    #[test]
    fn notional_only_returns_notional_variant() {
        let spec = size_spec_from_inputs(None, None, Some(100.0)).unwrap();
        assert!(matches!(spec, SizeSpec::Notional(n) if (n - 100.0).abs() < f64::EPSILON));
    }

    #[test]
    fn missing_all_three_is_validation_error() {
        let err = size_spec_from_inputs(None, None, None).unwrap_err();
        assert_eq!(err.code, "MISSING_SIZE");
    }

    #[test]
    fn providing_two_at_once_is_ambiguous() {
        let err = size_spec_from_inputs(Some(100.0), Some(1.0), None).unwrap_err();
        assert_eq!(err.code, "AMBIGUOUS_SIZE");
        let err = size_spec_from_inputs(Some(100.0), None, Some(50.0)).unwrap_err();
        assert_eq!(err.code, "AMBIGUOUS_SIZE");
        let err = size_spec_from_inputs(None, Some(1.0), Some(50.0)).unwrap_err();
        assert_eq!(err.code, "AMBIGUOUS_SIZE");
    }

    #[test]
    fn providing_all_three_is_ambiguous() {
        let err = size_spec_from_inputs(Some(100.0), Some(1.0), Some(50.0)).unwrap_err();
        assert_eq!(err.code, "AMBIGUOUS_SIZE");
    }
}

#[cfg(test)]
mod tpsl_input_tests {
    use super::*;

    #[test]
    fn legacy_single_tp_resolves_to_full_position() {
        let inputs = parse_cli_tpsl_levels("--tp", "--tp-level", Some(90.0), &Vec::new()).unwrap();
        assert_eq!(inputs.len(), 1);
        assert!((inputs[0].price - 90.0).abs() < f64::EPSILON);
        assert!(matches!(inputs[0].size, TpSlSize::Full));
    }

    #[test]
    fn level_with_explicit_size_parses_tokens() {
        let inputs = parse_cli_tpsl_levels(
            "--tp",
            "--tp-level",
            None,
            &["90:0.5".to_string(), "95:0.25".to_string()],
        )
        .unwrap();
        assert_eq!(inputs.len(), 2);
        assert!(matches!(inputs[0].size, TpSlSize::Tokens(t) if (t - 0.5).abs() < f64::EPSILON));
        assert!(matches!(inputs[1].size, TpSlSize::Tokens(t) if (t - 0.25).abs() < f64::EPSILON));
    }

    #[test]
    fn level_without_size_defaults_to_full() {
        let inputs =
            parse_cli_tpsl_levels("--sl", "--sl-level", None, &["140".to_string()]).unwrap();
        assert_eq!(inputs.len(), 1);
        assert!(matches!(inputs[0].size, TpSlSize::Full));
    }

    #[test]
    fn combining_single_and_levels_is_rejected() {
        let err = parse_cli_tpsl_levels("--tp", "--tp-level", Some(90.0), &["95:0.5".to_string()])
            .unwrap_err();
        assert_eq!(err.code, "TPSL_FLAG_CONFLICT");
    }

    #[test]
    fn unparseable_level_is_validation_error() {
        let err = parse_cli_tpsl_levels("--tp", "--tp-level", None, &["not-a-number".to_string()])
            .unwrap_err();
        assert_eq!(err.code, "TPSL_LEVEL_PARSE");
        let err =
            parse_cli_tpsl_levels("--tp", "--tp-level", None, &["90:not-a-number".to_string()])
                .unwrap_err();
        assert_eq!(err.code, "TPSL_LEVEL_PARSE");
    }
}

#[cfg(test)]
mod conditional_cancel_id_tests {
    use super::*;
    use crate::commands::conditional_orders::TriggerKind;

    #[test]
    fn parses_conditional_api_id_index_and_direction() {
        let parsed = parse_conditional_order_id("ctp-10-1-gt", TriggerKind::TakeProfit, 10)
            .expect("valid conditional TP id");
        assert_eq!(parsed.conditional_order_index, 1);
        assert_eq!(parsed.direction, ConditionalTriggerDirection::Greater);

        let parsed = parse_conditional_order_id("csl-10-2-lt", TriggerKind::StopLoss, 10)
            .expect("valid conditional SL id");
        assert_eq!(parsed.conditional_order_index, 2);
        assert_eq!(parsed.direction, ConditionalTriggerDirection::Less);
    }

    #[test]
    fn rejects_ids_that_should_use_rpc_fallback() {
        assert!(parse_conditional_order_id("tp-10-1-gt", TriggerKind::TakeProfit, 10).is_none());
        assert!(parse_conditional_order_id("ctp-11-1-gt", TriggerKind::TakeProfit, 10).is_none());
        assert!(
            parse_conditional_order_id("ctp-10-not-index-gt", TriggerKind::TakeProfit, 10)
                .is_none()
        );
        assert!(
            parse_conditional_order_id("ctp-10-1-unknown", TriggerKind::TakeProfit, 10).is_none()
        );
    }
}

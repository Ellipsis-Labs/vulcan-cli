//! MCP server handler — implements rmcp ServerHandler.

use crate::agent_log::{
    build_position_session_report, default_log_path, position_report_markdown, read_recent,
    summarize_records, summary_markdown,
};
use crate::cli::strategy::{
    GridStartArgs, StrategyMarginModeArg, StrategyModeArg, StrategySideArg, TwapStartArgs,
};
use crate::commands;
use crate::commands::portfolio::{execute_snapshot_inner, PortfolioSections};
use crate::context::AppContext;
use crate::mcp::registry::{self, ToolDef};
use crate::paper::{PaperOrderType, PaperSide, PaperSizeInput};
use phoenix_rise::Side;
use rmcp::model::*;
use rmcp::ServerHandler;
use serde_json::Value;
use std::sync::Arc;

/// Vulcan MCP server — exposes trading tools over stdio.
pub struct VulcanMcpServer {
    ctx: Arc<AppContext>,
    tools: Vec<&'static ToolDef>,
    allow_dangerous: bool,
}

impl VulcanMcpServer {
    pub fn new(ctx: Arc<AppContext>, allow_dangerous: bool, groups: Option<Vec<String>>) -> Self {
        let tools = registry::tools_for_groups(&groups);
        Self {
            ctx,
            tools,
            allow_dangerous,
        }
    }

    /// Hide dangerous tools from `tools/list` when the server was started
    /// without `--allow-dangerous`. They're still gated at invocation time,
    /// but exposing them in the list invites agents to attempt calls that
    /// will be rejected. Kraken's `-s market,paper` pattern: read-only first,
    /// dangerous tools require the user to explicitly widen the args.
    fn visible_tools(&self) -> Vec<&'static ToolDef> {
        if self.allow_dangerous {
            self.tools.clone()
        } else {
            self.tools
                .iter()
                .copied()
                .filter(|t| !t.dangerous)
                .collect()
        }
    }

    /// Convert a ToolDef into an rmcp Tool model.
    fn to_rmcp_tool(def: &ToolDef) -> Tool {
        let schema = (def.schema)();
        let schema_obj: serde_json::Map<String, Value> = match schema {
            Value::Object(m) => m,
            _ => serde_json::Map::new(),
        };
        Tool::new(def.name, def.description, Arc::new(schema_obj))
    }

    /// Dispatch a tool call to the appropriate command inner function.
    async fn dispatch(&self, name: &str, args: &Value) -> Result<Value, crate::error::VulcanError> {
        match name {
            // ── Market ──────────────────────────────────────────────────
            "vulcan_market_list" => {
                let result = commands::market::execute_list_inner(&self.ctx).await?;
                Ok(serde_json::to_value(result).unwrap())
            }
            "vulcan_market_ticker" => {
                let symbol = arg_str(args, "symbol")?;
                let result = commands::market::execute_ticker_inner(&self.ctx, &symbol).await?;
                Ok(serde_json::to_value(result).unwrap())
            }
            "vulcan_market_info" => {
                let symbol = arg_str(args, "symbol")?;
                let result = commands::market::execute_info_inner(&self.ctx, &symbol).await?;
                Ok(serde_json::to_value(result).unwrap())
            }
            "vulcan_market_orderbook" => {
                let symbol = arg_str(args, "symbol")?;
                let depth = arg_usize_or(args, "depth", 10);
                let result =
                    commands::market::execute_orderbook_inner(&self.ctx, &symbol, depth).await?;
                Ok(serde_json::to_value(result).unwrap())
            }
            "vulcan_market_candles" => {
                let symbol = arg_str(args, "symbol")?;
                let interval = arg_str_or(args, "interval", "1h");
                let limit = arg_usize_or(args, "limit", 50);
                let result =
                    commands::market::execute_candles_inner(&self.ctx, &symbol, &interval, limit)
                        .await?;
                Ok(serde_json::to_value(result).unwrap())
            }

            // ── Technical analysis ──────────────────────────────────────
            "vulcan_ta_compute" => {
                let symbol = arg_str(args, "symbol")?;
                let indicator = arg_str(args, "indicator")?;
                let timeframe = arg_str_or(args, "timeframe", "1h");
                let period = args
                    .get("period")
                    .and_then(|v| v.as_u64())
                    .map(|v| v as usize);
                let limit = args
                    .get("limit")
                    .and_then(|v| v.as_u64())
                    .map(|v| v as usize);
                let params = arg_params_map(args, "params")?;
                let kind =
                    <crate::indicators::IndicatorKind as std::str::FromStr>::from_str(&indicator)?;
                let request = crate::indicators::IndicatorRequest {
                    kind,
                    period,
                    params,
                };
                let series =
                    crate::indicators::compute(&self.ctx, &symbol, &timeframe, &request, limit)
                        .await?;
                Ok(serde_json::to_value(series).unwrap())
            }
            "vulcan_ta_signal" => {
                let symbol = arg_str(args, "symbol")?;
                let spec_value = args.get("spec").cloned().ok_or_else(|| {
                    crate::error::VulcanError::validation(
                        "MISSING_ARG",
                        "Required argument 'spec' is missing",
                    )
                })?;
                let spec: crate::indicators::TriggerSpec = serde_json::from_value(spec_value)
                    .map_err(|e| {
                        crate::error::VulcanError::validation(
                            "INVALID_TRIGGER_SPEC",
                            format!("could not parse 'spec': {e}"),
                        )
                    })?;
                let outcome =
                    crate::indicators::evaluate_trigger(&self.ctx, &symbol, &spec).await?;
                Ok(serde_json::to_value(outcome).unwrap())
            }
            "vulcan_ta_report" => {
                let symbol = arg_str(args, "symbol")?;
                let timeframe = arg_str_or(args, "timeframe", "1h");
                let report = crate::indicators::report(&self.ctx, &symbol, &timeframe).await?;
                Ok(serde_json::to_value(report).unwrap())
            }

            // ── Trade ───────────────────────────────────────────────────
            "vulcan_trade" => self.dispatch_trade(args).await,
            "vulcan_trade_multi_limit" => {
                let symbol = arg_str(args, "symbol")?;
                let slide = arg_bool_or(args, "slide", false);
                let bids = arg_order_array(args, "bids")?;
                let asks = arg_order_array(args, "asks")?;
                let result = commands::trade::execute_multi_limit_order_inner(
                    &self.ctx, &symbol, bids, asks, slide,
                )
                .await?;
                Ok(serde_json::to_value(result).unwrap())
            }
            "vulcan_trade_orders" => {
                let symbol = arg_str_opt(args, "symbol");
                let result =
                    commands::trade::execute_orders_inner(&self.ctx, symbol.as_deref()).await?;
                Ok(serde_json::to_value(result).unwrap())
            }
            "vulcan_trade_cancel" => {
                let scope = arg_str(args, "scope")?;
                match scope.as_str() {
                    "ids" => {
                        let symbol = arg_str(args, "symbol")?;
                        let order_ids = arg_str_array(args, "order_ids")?;
                        let result =
                            commands::trade::execute_cancel_inner(&self.ctx, &symbol, order_ids)
                                .await?;
                        Ok(serde_json::to_value(result).unwrap())
                    }
                    "all" => {
                        let symbol = arg_str(args, "symbol")?;
                        let result =
                            commands::trade::execute_cancel_all_inner(&self.ctx, &symbol).await?;
                        Ok(serde_json::to_value(result).unwrap())
                    }
                    "all-markets" => {
                        let result =
                            commands::trade::execute_cancel_all_markets_inner(&self.ctx).await?;
                        Ok(serde_json::to_value(result).unwrap())
                    }
                    "tpsl" => {
                        let symbol = arg_str(args, "symbol")?;
                        let cancel_tp = arg_bool_or(args, "tp", false);
                        let cancel_sl = arg_bool_or(args, "sl", false);
                        let result = commands::trade::execute_cancel_tpsl_inner(
                            &self.ctx, &symbol, cancel_tp, cancel_sl,
                        )
                        .await?;
                        Ok(serde_json::to_value(result).unwrap())
                    }
                    other => Err(crate::error::VulcanError::validation(
                        "INVALID_CANCEL_SCOPE",
                        format!(
                            "scope must be 'ids', 'all', 'all-markets', or 'tpsl', got '{}'",
                            other
                        ),
                    )),
                }
            }

            "vulcan_trade_set_tpsl" => {
                let symbol = arg_str(args, "symbol")?;
                let tp_inputs = arg_tpsl_levels(args, "tp", "tp_levels")?;
                let sl_inputs = arg_tpsl_levels(args, "sl", "sl_levels")?;
                let result = commands::trade::execute_set_tpsl_inner(
                    &self.ctx, &symbol, tp_inputs, sl_inputs,
                )
                .await?;
                Ok(serde_json::to_value(result).unwrap())
            }
            // ── Position ────────────────────────────────────────────────
            "vulcan_position_list" => {
                let result = commands::position::execute_list_inner(&self.ctx).await?;
                Ok(serde_json::to_value(result).unwrap())
            }
            "vulcan_position_show" => {
                let symbol = arg_str(args, "symbol")?;
                let result = commands::position::execute_show_inner(&self.ctx, &symbol).await?;
                Ok(serde_json::to_value(result).unwrap())
            }
            "vulcan_position_close" => {
                let symbol = arg_str(args, "symbol")?;
                let result = commands::position::execute_close_inner(&self.ctx, &symbol).await?;
                Ok(serde_json::to_value(result).unwrap())
            }
            "vulcan_position_close_all" => {
                let result = commands::position::execute_close_all_inner(&self.ctx).await?;
                Ok(serde_json::to_value(result).unwrap())
            }

            // ── Margin ──────────────────────────────────────────────────
            "vulcan_margin_status" => {
                let result = commands::margin::execute_status_inner(&self.ctx).await?;
                Ok(serde_json::to_value(result).unwrap())
            }
            "vulcan_margin_deposit" => {
                let amount = arg_f64(args, "amount")?;
                let result =
                    commands::margin::execute_deposit_withdraw_inner(&self.ctx, amount, true)
                        .await?;
                Ok(serde_json::to_value(result).unwrap())
            }
            "vulcan_margin_withdraw" => {
                let amount = arg_f64(args, "amount")?;
                let result =
                    commands::margin::execute_deposit_withdraw_inner(&self.ctx, amount, false)
                        .await?;
                Ok(serde_json::to_value(result).unwrap())
            }
            "vulcan_margin_transfer" => {
                let from = arg_u8(args, "from_subaccount")?;
                let to = arg_u8(args, "to_subaccount")?;
                let amount = arg_f64(args, "amount")?;
                let result =
                    commands::margin::execute_transfer_inner(&self.ctx, amount, from, to).await?;
                Ok(serde_json::to_value(result).unwrap())
            }
            "vulcan_margin_add_collateral" => {
                let symbol = arg_str(args, "symbol")?;
                let amount = arg_f64(args, "amount")?;
                let result =
                    commands::margin::execute_add_collateral_inner(&self.ctx, &symbol, amount)
                        .await?;
                Ok(serde_json::to_value(result).unwrap())
            }
            "vulcan_margin_transfer_child_to_parent" => {
                let child = arg_u8(args, "child_subaccount")?;
                let result =
                    commands::margin::execute_transfer_child_to_parent_inner(&self.ctx, child)
                        .await?;
                Ok(serde_json::to_value(result).unwrap())
            }
            "vulcan_margin_sync_parent_to_child" => {
                let child = arg_u8(args, "child_subaccount")?;
                let result =
                    commands::margin::execute_sync_parent_to_child_inner(&self.ctx, child).await?;
                Ok(serde_json::to_value(result).unwrap())
            }
            "vulcan_margin_leverage_tiers" => {
                let symbol = arg_str(args, "symbol")?;
                let result =
                    commands::margin::execute_leverage_tiers_inner(&self.ctx, &symbol).await?;
                Ok(serde_json::to_value(result).unwrap())
            }

            // ── Position (new) ──────────────────────────────────────────
            "vulcan_position_reduce" => {
                let symbol = arg_str(args, "symbol")?;
                let size = arg_f64(args, "size")?;
                let result =
                    commands::position::execute_reduce_inner(&self.ctx, &symbol, size).await?;
                Ok(serde_json::to_value(result).unwrap())
            }
            "vulcan_position_tp_sl" => {
                let symbol = arg_str(args, "symbol")?;
                let tp = arg_f64_opt(args, "tp");
                let sl = arg_f64_opt(args, "sl");
                let result =
                    commands::position::execute_tp_sl_inner(&self.ctx, &symbol, tp, sl).await?;
                Ok(serde_json::to_value(result).unwrap())
            }

            // ── History ─────────────────────────────────────────────────
            "vulcan_history" => {
                let history_type = arg_str(args, "type")?;
                let symbol = arg_str_opt(args, "symbol");
                let limit = arg_i64_or(args, "limit", 20);
                let cursor = arg_str_opt(args, "cursor");
                let resolution = arg_str_or(args, "resolution", "1h");
                let result = match history_type.as_str() {
                    "trades" => {
                        crate::history::trades(
                            &self.ctx,
                            symbol.as_deref(),
                            limit,
                            cursor.as_deref(),
                        )
                        .await?
                    }
                    "orders" => {
                        crate::history::orders(
                            &self.ctx,
                            symbol.as_deref(),
                            limit,
                            cursor.as_deref(),
                        )
                        .await?
                    }
                    "collateral" => {
                        crate::history::collateral(&self.ctx, limit, cursor.as_deref()).await?
                    }
                    "funding" => {
                        crate::history::funding(
                            &self.ctx,
                            symbol.as_deref(),
                            limit,
                            cursor.as_deref(),
                        )
                        .await?
                    }
                    "pnl" => crate::history::pnl(&self.ctx, &resolution, limit).await?,
                    other => {
                        return Err(crate::error::VulcanError::validation(
                            "INVALID_HISTORY_TYPE",
                            format!("unknown history type {}", other),
                        ));
                    }
                };
                Ok(serde_json::to_value(result).unwrap())
            }

            // ── Status ────────────────────────────────────────────────────
            "vulcan_status" => {
                let result = commands::status::execute_inner(&self.ctx).await?;
                Ok(serde_json::to_value(result).unwrap())
            }

            // ── Update ──────────────────────────────────────────────────
            "vulcan_update_check" => {
                let force = arg_bool_or(args, "force_refresh", false);
                let result = commands::update::execute_check_inner(&self.ctx, force).await?;
                Ok(serde_json::to_value(result).unwrap())
            }

            // ── Wallet ────────────────────────────────────────────────────
            "vulcan_wallet_create" => {
                let name = arg_str(args, "name")?;
                let password = arg_str(args, "password")?;
                let result = commands::wallet::execute_create_inner(&self.ctx, &name, &password)?;
                Ok(serde_json::to_value(result).unwrap())
            }
            "vulcan_wallet_list" => {
                let result = commands::wallet::execute_list_inner(&self.ctx)?;
                Ok(serde_json::to_value(result).unwrap())
            }
            "vulcan_wallet_address" => {
                let name = arg_str_opt(args, "name");
                let result = commands::wallet::execute_address_inner(&self.ctx, name.as_deref())?;
                Ok(serde_json::to_value(result).unwrap())
            }
            "vulcan_wallet_balance" => {
                let name = arg_str_opt(args, "name");
                let result = commands::wallet::execute_balance_inner(&self.ctx, name.as_deref())?;
                Ok(serde_json::to_value(result).unwrap())
            }

            // ── Portfolio ─────────────────────────────────────────────────
            "vulcan_portfolio" => {
                let include = arg_str_array_opt(args, "include").unwrap_or_default();
                let sections = commands::portfolio::PortfolioSections::from_include(&include)?;
                let result =
                    commands::portfolio::execute_snapshot_inner(&self.ctx, sections).await?;
                Ok(serde_json::to_value(result).unwrap())
            }

            // ── Paper trading ───────────────────────────────────────────
            "vulcan_paper_init" => {
                let result = commands::paper::init(
                    &self.ctx,
                    arg_f64_opt(args, "balance"),
                    arg_str_opt(args, "currency"),
                    arg_f64_opt(args, "fee_bps"),
                )
                .await?;
                Ok(serde_json::to_value(result).unwrap())
            }
            "vulcan_paper_status" => {
                let result = commands::paper::status(&self.ctx).await?;
                Ok(serde_json::to_value(result).unwrap())
            }
            "vulcan_paper_positions" => {
                let (_, state) = crate::paper::load_state(&self.ctx.vulcan_dir)?;
                Ok(serde_json::to_value(crate::paper::positions(&state)).unwrap())
            }
            "vulcan_paper_orders" => {
                let (_, state) = crate::paper::load_state(&self.ctx.vulcan_dir)?;
                Ok(serde_json::to_value(crate::paper::orders(&state)).unwrap())
            }
            "vulcan_paper_fills" => {
                let limit = arg_usize_or(args, "limit", 50);
                let (_, state) = crate::paper::load_state(&self.ctx.vulcan_dir)?;
                Ok(serde_json::to_value(crate::paper::fills(&state, limit)).unwrap())
            }
            "vulcan_paper_trade" => self.dispatch_paper_trade(args).await,
            "vulcan_paper_cancel" => {
                let order_id = arg_str(args, "order_id")?;
                let result = commands::paper::cancel(&self.ctx, &order_id)?;
                Ok(serde_json::to_value(result).unwrap())
            }
            "vulcan_paper_cancel_all" => {
                let symbol = arg_str_opt(args, "symbol");
                let result = commands::paper::cancel_all(&self.ctx, symbol.as_deref())?;
                Ok(serde_json::to_value(result).unwrap())
            }
            "vulcan_paper_reconcile" => {
                let symbol = arg_str_opt(args, "symbol");
                let result = commands::paper::reconcile(&self.ctx, symbol.as_deref()).await?;
                Ok(serde_json::to_value(result).unwrap())
            }

            // ── Strategy runners ───────────────────────────────────────
            "vulcan_strategy_twap_start" => {
                let detached = arg_bool_or(args, "detached", false);
                let args = TwapStartArgs {
                    symbol: arg_str(args, "symbol")?,
                    side: parse_strategy_side(args)?,
                    notional_usdc: arg_f64_opt(args, "notional_usdc"),
                    tokens: arg_f64_opt(args, "tokens"),
                    slices: arg_u32(args, "slices")?,
                    interval_seconds: arg_u64_or(args, "interval_seconds", 60),
                    mode: parse_strategy_mode(args)?,
                    margin_mode: parse_strategy_margin_mode(args)?,
                    isolated_collateral: arg_f64_opt(args, "isolated_collateral"),
                    run_label: arg_str_opt(args, "run_label"),
                    max_total_notional_usdc: arg_f64_opt(args, "max_total_notional_usdc"),
                    max_step_notional_usdc: arg_f64_opt(args, "max_step_notional_usdc"),
                    max_price_drift_bps: arg_f64_opt(args, "max_price_drift_bps"),
                    max_exposure_ratio: arg_f64_opt(args, "max_exposure_ratio"),
                    reconcile_attempts: arg_u32_opt(args, "reconcile_attempts"),
                    reconcile_delay_ms: arg_u64_opt(args, "reconcile_delay_ms"),
                    no_sleep: arg_bool_or(args, "no_sleep", false),
                    detached,
                };
                if detached {
                    let result =
                        crate::strategy::twap::run_twap_detached(self.ctx.clone(), args).await?;
                    Ok(serde_json::to_value(result).unwrap())
                } else {
                    let result = crate::strategy::twap::run_twap(&self.ctx, args).await?;
                    Ok(serde_json::to_value(result).unwrap())
                }
            }
            "vulcan_strategy_grid_start" => {
                let detached = arg_bool_or(args, "detached", false);
                let args = GridStartArgs {
                    symbol: arg_str(args, "symbol")?,
                    lower_price: arg_f64_opt(args, "lower_price"),
                    upper_price: arg_f64_opt(args, "upper_price"),
                    center_on_mark: arg_bool_or(args, "center_on_mark", false),
                    width_pct: arg_f64_opt(args, "width_pct"),
                    levels_per_side: arg_u32(args, "levels_per_side")?,
                    tokens_per_level: arg_f64_opt(args, "tokens_per_level"),
                    size_lots_per_level: arg_u64_opt(args, "size_lots_per_level"),
                    bid_levels: arg_str_array_opt(args, "bid_levels").unwrap_or_default(),
                    ask_levels: arg_str_array_opt(args, "ask_levels").unwrap_or_default(),
                    take_profit_spacing: arg_f64_opt(args, "take_profit_spacing"),
                    stop_loss_spacing: arg_f64_opt(args, "stop_loss_spacing"),
                    interval_seconds: arg_u64_or(args, "interval_seconds", 60),
                    ticks: arg_u32_opt(args, "ticks").unwrap_or(60),
                    run_until_stopped: arg_bool_or(args, "run_until_stopped", false),
                    stale_after_seconds: arg_u64_opt(args, "stale_after_seconds"),
                    mode: parse_strategy_mode(args)?,
                    margin_mode: parse_strategy_margin_mode(args)?,
                    isolated_collateral: arg_f64_opt(args, "isolated_collateral"),
                    run_label: arg_str_opt(args, "run_label"),
                    slide: arg_bool_or(args, "slide", false),
                    max_total_notional_usdc: arg_f64_opt(args, "max_total_notional_usdc"),
                    max_step_notional_usdc: arg_f64_opt(args, "max_step_notional_usdc"),
                    max_price_drift_bps: arg_f64_opt(args, "max_price_drift_bps"),
                    max_exposure_ratio: arg_f64_opt(args, "max_exposure_ratio"),
                    reconcile_attempts: arg_u32_opt(args, "reconcile_attempts"),
                    reconcile_delay_ms: arg_u64_opt(args, "reconcile_delay_ms"),
                    no_sleep: arg_bool_or(args, "no_sleep", false),
                    detached,
                };
                if detached {
                    let result =
                        crate::strategy::grid::run_grid_detached(self.ctx.clone(), args).await?;
                    Ok(serde_json::to_value(result).unwrap())
                } else {
                    let result = crate::strategy::grid::run_grid(&self.ctx, args).await?;
                    Ok(serde_json::to_value(result).unwrap())
                }
            }
            "vulcan_strategy_preflight" => {
                let result = crate::strategy::preflight::check_live_readiness(&self.ctx).await;
                Ok(serde_json::to_value(result).unwrap())
            }
            "vulcan_strategy_ta_start" => {
                let config_value = args.get("config").cloned().ok_or_else(|| {
                    crate::error::VulcanError::validation(
                        "MISSING_ARG",
                        "Required argument 'config' (TA strategy config object) is missing",
                    )
                })?;
                let config: crate::strategy::ta_rule::TaStrategyConfig =
                    serde_json::from_value(config_value).map_err(|e| {
                        crate::error::VulcanError::validation(
                            "INVALID_TA_STRATEGY_CONFIG",
                            format!("could not parse 'config': {e}"),
                        )
                    })?;
                let mode_arg = parse_strategy_mode(args)?;
                let mode = match mode_arg {
                    crate::cli::strategy::StrategyModeArg::Paper => {
                        crate::strategy::types::StrategyMode::Paper
                    }
                    crate::cli::strategy::StrategyModeArg::DryRun => {
                        crate::strategy::types::StrategyMode::DryRun
                    }
                    crate::cli::strategy::StrategyModeArg::ConfirmEach => {
                        crate::strategy::types::StrategyMode::ConfirmEach
                    }
                    crate::cli::strategy::StrategyModeArg::AutoExecute => {
                        crate::strategy::types::StrategyMode::AutoExecute
                    }
                };
                let input = crate::strategy::ta::TaStartInput {
                    config,
                    mode,
                    max_ticks: arg_u32_opt(args, "max_ticks").unwrap_or(60),
                    run_until_stopped: arg_bool_or(args, "run_until_stopped", false),
                    run_label: arg_str_opt(args, "run_label"),
                    no_sleep: arg_bool_or(args, "no_sleep", false),
                    safety_overrides: crate::strategy::ta::SafetyOverrides {
                        max_total_notional_usdc: arg_f64_opt(args, "max_total_notional_usdc"),
                        max_step_notional_usdc: arg_f64_opt(args, "max_step_notional_usdc"),
                        max_price_drift_bps: arg_f64_opt(args, "max_price_drift_bps"),
                        max_exposure_ratio: arg_f64_opt(args, "max_exposure_ratio"),
                        reconcile_attempts: arg_u32_opt(args, "reconcile_attempts"),
                        reconcile_delay_ms: arg_u64_opt(args, "reconcile_delay_ms"),
                    },
                };
                let detached = arg_bool_or(args, "detached", false);
                if detached {
                    let result =
                        crate::strategy::ta::run_ta_detached(self.ctx.clone(), input).await?;
                    Ok(serde_json::to_value(result).unwrap())
                } else {
                    let result = crate::strategy::ta::run_ta(&self.ctx, input).await?;
                    Ok(serde_json::to_value(result).unwrap())
                }
            }
            "vulcan_strategy_runs" => {
                let result = commands::strategy::runs(&self.ctx, arg_usize_or(args, "limit", 20))?;
                Ok(serde_json::to_value(result).unwrap())
            }
            "vulcan_strategy_status" => {
                let run_id = arg_str(args, "run_id")?;
                let result = commands::strategy::status(
                    &self.ctx,
                    &run_id,
                    arg_u32_opt(args, "since_tick"),
                    arg_bool_or(args, "include_ledger", false),
                )?;
                Ok(serde_json::to_value(result).unwrap())
            }
            "vulcan_strategy_monitor" => {
                let run_id = arg_str(args, "run_id")?;
                let result = commands::strategy::monitor(
                    &self.ctx,
                    &run_id,
                    arg_bool_or(args, "include_ledger", false),
                )?;
                Ok(serde_json::to_value(result).unwrap())
            }
            "vulcan_strategy_reconcile_grid" => {
                let run_id = arg_str(args, "run_id")?;
                let result =
                    crate::strategy::grid::inspect_live_grid_orders(&self.ctx, &run_id).await?;
                Ok(serde_json::to_value(result).unwrap())
            }
            "vulcan_strategy_wait_next_tick" => {
                let run_id = arg_str(args, "run_id")?;
                let result = commands::strategy::wait_next_tick(
                    &self.ctx,
                    &run_id,
                    arg_u32_opt(args, "after_tick"),
                    arg_u64_or(args, "timeout_seconds", 90),
                    arg_bool_or(args, "include_ledger", false),
                )
                .await?;
                Ok(serde_json::to_value(result).unwrap())
            }
            "vulcan_strategy_report" => {
                let run_id = arg_str(args, "run_id")?;
                let result = commands::strategy::report(&self.ctx, &run_id)?;
                Ok(serde_json::to_value(result).unwrap())
            }
            "vulcan_strategy_pause" => {
                let run_id = arg_str(args, "run_id")?;
                let result = commands::strategy::request_control(
                    &self.ctx,
                    &run_id,
                    crate::strategy::StrategyControlAction::Pause,
                    arg_str_opt(args, "reason"),
                )?;
                Ok(serde_json::to_value(result).unwrap())
            }
            "vulcan_strategy_stop" => {
                let run_id = arg_str(args, "run_id")?;
                let result = commands::strategy::request_control(
                    &self.ctx,
                    &run_id,
                    crate::strategy::StrategyControlAction::Stop,
                    arg_str_opt(args, "reason"),
                )?;
                Ok(serde_json::to_value(result).unwrap())
            }
            "vulcan_strategy_finalize" => {
                let run_id = arg_str(args, "run_id")?;
                let result = commands::strategy::finalize(
                    &self.ctx,
                    &run_id,
                    arg_str_opt(args, "reason"),
                    arg_bool_or(args, "cancel_orders", false),
                    arg_bool_or(args, "close_position", false),
                    arg_bool_or(args, "wait", false),
                    arg_u64_or(args, "timeout_seconds", 90),
                )
                .await?;
                Ok(serde_json::to_value(result).unwrap())
            }
            "vulcan_strategy_resume" => {
                let run_id = arg_str(args, "run_id")?;
                let runs_dir = crate::strategy::default_strategy_runs_dir(&self.ctx.vulcan_dir);
                let ledger = crate::strategy::load_run_ledger(&runs_dir, &run_id)?;
                let result = match ledger.strategy.as_str() {
                    "twap" => serde_json::to_value(
                        crate::strategy::twap::resume_twap(
                            &self.ctx,
                            &run_id,
                            arg_u32_opt(args, "from_step"),
                            arg_bool_or(args, "no_sleep", false),
                        )
                        .await?,
                    )
                    .unwrap(),
                    "grid" => serde_json::to_value(
                        crate::strategy::grid::resume_grid(
                            &self.ctx,
                            &run_id,
                            arg_u32_opt(args, "from_step"),
                            arg_bool_or(args, "no_sleep", false),
                        )
                        .await?,
                    )
                    .unwrap(),
                    other => {
                        return Err(crate::error::VulcanError::validation(
                            "UNSUPPORTED_STRATEGY_RESUME",
                            format!("resume does not support strategy {}", other),
                        ));
                    }
                };
                Ok(serde_json::to_value(result).unwrap())
            }

            // ── Account ───────────────────────────────────────────────────
            "vulcan_account_info" => {
                let result = commands::account::execute_info_inner(&self.ctx).await?;
                Ok(serde_json::to_value(result).unwrap())
            }
            "vulcan_account_register" => {
                let code = match (
                    arg_str_opt(args, "access_code"),
                    arg_str_opt(args, "referral_code"),
                    arg_str_opt(args, "invite_code"),
                ) {
                    (Some(code), None, None) | (None, None, Some(code)) => {
                        commands::account::RegistrationCode::Access(code)
                    }
                    (None, Some(code), None) => commands::account::RegistrationCode::Referral(code),
                    _ => {
                        return Err(crate::error::VulcanError::validation(
                            "REGISTRATION_CODE_CONFLICT",
                            "Provide exactly one of access_code, referral_code, or invite_code",
                        ));
                    }
                };
                let result =
                    commands::account::execute_register_with_code_inner(&self.ctx, code).await?;
                Ok(serde_json::to_value(result).unwrap())
            }

            // ── Auth ─────────────────────────────────────────────────────
            "vulcan_auth_status" => {
                let result = commands::auth::status(&self.ctx);
                Ok(serde_json::to_value(result).unwrap())
            }
            "vulcan_auth_login" => {
                let Some(session_wallet) = &self.ctx.session_wallet else {
                    return Err(crate::error::VulcanError::auth(
                        "MCP_SESSION_WALLET_REQUIRED",
                        "vulcan_auth_login requires MCP --allow-dangerous with an unlocked session wallet",
                    ));
                };
                let wallet = session_wallet.to_wallet()?;
                let wallet_name = self.ctx.wallet_store.default_wallet().ok().flatten();
                let result =
                    commands::auth::login_with_wallet(&self.ctx, &wallet, wallet_name).await?;
                Ok(serde_json::to_value(result).unwrap())
            }
            "vulcan_auth_logout" => {
                let result = commands::auth::logout(&self.ctx)?;
                Ok(serde_json::to_value(result).unwrap())
            }

            _ => Err(crate::error::VulcanError::validation(
                "UNKNOWN_TOOL",
                format!("Unknown tool: {}", name),
            )),
        }
    }

    async fn dispatch_trade(&self, args: &Value) -> Result<Value, crate::error::VulcanError> {
        let symbol = arg_str(args, "symbol")?;
        let side = parse_side(args, "side")?;
        let order_type = arg_str(args, "order_type")?;
        let tp = arg_f64_opt(args, "tp");
        let sl = arg_f64_opt(args, "sl");
        let isolated = arg_bool_or(args, "isolated", false);
        let collateral = arg_f64_opt(args, "collateral");
        let reduce_only = arg_bool_or(args, "reduce_only", false);

        match order_type.as_str() {
            "market" => {
                let spec = commands::trade::size_spec_from_inputs(
                    arg_f64_opt(args, "size"),
                    arg_f64_opt(args, "tokens"),
                    arg_f64_opt(args, "notional_usdc"),
                )?;
                let resolved = commands::trade::resolve_base_lots(&self.ctx, &symbol, spec).await?;
                let mut result = commands::trade::execute_market_order_inner(
                    &self.ctx,
                    &symbol,
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
                Ok(serde_json::to_value(result).unwrap())
            }
            "limit" => {
                let size = arg_f64(args, "size")?;
                let price = arg_f64(args, "price")?;
                let result = commands::trade::execute_limit_order_inner(
                    &self.ctx,
                    &symbol,
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
                Ok(serde_json::to_value(result).unwrap())
            }
            other => Err(crate::error::VulcanError::validation(
                "INVALID_ORDER_TYPE",
                format!("order_type must be 'market' or 'limit', got '{}'", other),
            )),
        }
    }

    async fn dispatch_paper_trade(&self, args: &Value) -> Result<Value, crate::error::VulcanError> {
        let symbol = arg_str(args, "symbol")?;
        let side = match arg_str(args, "side")?.as_str() {
            "buy" => PaperSide::Buy,
            "sell" => PaperSide::Sell,
            other => {
                return Err(crate::error::VulcanError::validation(
                    "INVALID_SIDE",
                    format!("side must be buy or sell, got {}", other),
                ));
            }
        };
        let order_type = match arg_str_or(args, "order_type", "market").as_str() {
            "market" => PaperOrderType::Market,
            "limit" => PaperOrderType::Limit,
            other => {
                return Err(crate::error::VulcanError::validation(
                    "INVALID_ORDER_TYPE",
                    format!("order_type must be market or limit, got {}", other),
                ));
            }
        };
        let (path, state) = crate::paper::load_state(&self.ctx.vulcan_dir)?;
        let (result, _) = crate::paper::place_order(
            &self.ctx,
            &path,
            state,
            crate::paper::PaperOrderRequest {
                symbol,
                side,
                order_type,
                size: PaperSizeInput {
                    size_lots: arg_f64_opt(args, "size"),
                    tokens: arg_f64_opt(args, "tokens"),
                    notional_usdc: arg_f64_opt(args, "notional_usdc"),
                },
                price: arg_f64_opt(args, "price"),
            },
        )
        .await?;
        Ok(serde_json::to_value(result).unwrap())
    }
}

// ── Embedded agent resources ────────────────────────────────────────────
static RESOURCES: &[(&str, &str, &str)] = &[
    // ── Context ────────────────────────────────────────────────────────
    (
        "vulcan://context",
        "Vulcan Runtime Context for AI Agents",
        include_str!("../../../CONTEXT.md"),
    ),
    // ── Agent resources ────────────────────────────────────────────────
    (
        "vulcan://agents/system",
        "Vulcan System Prompt",
        include_str!("../../../agents/system.md"),
    ),
    (
        "vulcan://agents/error-catalog",
        "Error Catalog — codes, categories, and recovery hints",
        include_str!("../../../agents/error-catalog.json"),
    ),
    (
        "vulcan://agents/tool-catalog",
        "Tool Catalog — machine-readable MCP and CLI command contract",
        include_str!("../../../agents/tool-catalog.json"),
    ),
    // ── Skills ─────────────────────────────────────────────────────────
    (
        "vulcan://skills/index",
        "Skills Index — all available workflow skills",
        include_str!("../../../skills/INDEX.md"),
    ),
    (
        "vulcan://skills/vulcan",
        "Vulcan setup and broad agent discovery entry point",
        include_str!("../../../skills/vulcan/SKILL.md"),
    ),
    (
        "vulcan://skills/vulcan-risk-management",
        "Pre-trade risk checks, leverage tiers, margin health, when to warn",
        include_str!("../../../skills/vulcan-risk-management/SKILL.md"),
    ),
    (
        "vulcan://skills/vulcan-error-recovery",
        "Error category routing, tx_failed recovery, network error handling",
        include_str!("../../../skills/vulcan-error-recovery/SKILL.md"),
    ),
    (
        "vulcan://skills/vulcan-trade-execution",
        "Safe order execution with pre-trade checks and post-trade verification",
        include_str!("../../../skills/vulcan-trade-execution/SKILL.md"),
    ),
    (
        "vulcan://skills/vulcan-lot-size-calculator",
        "Convert desired token amounts to base lots with worked examples",
        include_str!("../../../skills/vulcan-lot-size-calculator/SKILL.md"),
    ),
    (
        "vulcan://skills/vulcan-tpsl-management",
        "Take-profit and stop-loss: direction rules, constraints, set/cancel flows",
        include_str!("../../../skills/vulcan-tpsl-management/SKILL.md"),
    ),
    (
        "vulcan://skills/vulcan-market-intel",
        "Ticker, orderbook, candles, market info, and pre-trade analysis",
        include_str!("../../../skills/vulcan-market-intel/SKILL.md"),
    ),
    (
        "vulcan://skills/vulcan-portfolio-intel",
        "Portfolio snapshot: margin status, positions, orders, funding rates",
        include_str!("../../../skills/vulcan-portfolio-intel/SKILL.md"),
    ),
    (
        "vulcan://skills/vulcan-margin-operations",
        "Deposit, withdraw, transfer, isolated margin, collateral management",
        include_str!("../../../skills/vulcan-margin-operations/SKILL.md"),
    ),
    (
        "vulcan://skills/vulcan-position-management",
        "List, show, close, reduce positions and manage TP/SL",
        include_str!("../../../skills/vulcan-position-management/SKILL.md"),
    ),
    (
        "vulcan://skills/vulcan-onboarding",
        "New user setup: wallet creation, registration, first deposit",
        include_str!("../../../skills/vulcan-onboarding/SKILL.md"),
    ),
    (
        "vulcan://skills/vulcan-twap-execution",
        "Execute large orders as time-weighted slices to reduce market impact",
        include_str!("../../../skills/vulcan-twap-execution/SKILL.md"),
    ),
    (
        "vulcan://skills/vulcan-grid-trading",
        "Grid trading with layered limit orders across a price range",
        include_str!("../../../skills/vulcan-grid-trading/SKILL.md"),
    ),
];

fn resource_mime_type(uri: &str) -> &'static str {
    if uri.ends_with("-catalog") {
        "application/json"
    } else {
        "text/markdown"
    }
}

impl ServerHandler for VulcanMcpServer {
    fn get_info(&self) -> ServerInfo {
        ServerInfo::new(
            ServerCapabilities::builder()
                .enable_tools()
                .enable_resources()
                .build(),
        )
        .with_server_info(Implementation::from_build_env())
        .with_instructions(
            "Vulcan MCP server for Phoenix Perpetuals DEX on Solana. \
             Use market tools for price data, trade tools for order management, \
             position tools to monitor open positions, and margin tools for collateral. \
             Use wallet tools to create local encrypted wallets and show deposit addresses. \
             Use paper tools for local simulations with live prices and no real funds. \
             Report every trade, order, cancellation, strategy slice, paper fill, dry-run action, and transaction signature as soon as observed; do not batch execution updates into mid-run/end summaries. \
             Use strategy tools for curated strategy runs with Vulcan-owned tick logs, ledgers, launch-time margin feasibility checks, and wait-next-tick monitoring; present modes as Paper mode, Live mode with confirmation required, Live mode with automatic execution, and Plan mode with dry run. Ask for margin mode, triggers, and safety guardrails before launch. Live confirm_each, auto_execute, and resume are dangerous. \
             Use history tools for Phoenix/Rise trader fills, orders, collateral, funding, and PnL. \
             Dangerous tools (trades, deposits, withdrawals, cancellations) require \
             acknowledged=true. The size field is in base lots; market orders also \
             accept tokens or notional_usdc. \
             Read vulcan://context for the full runtime contract. \
             Read vulcan://agents/tool-catalog for the machine-readable tool contract. \
             Read vulcan://agent/health for first-run setup, Phoenix API/Solana RPC, paper, wallet, deposited collateral, and skill checks. \
             Read vulcan://agent/session-summary for recent redacted agent actions. \
             Read vulcan://agent/position-report for a live trader/session performance report. \
             Read vulcan://skills/index for goal-oriented workflow skills.",
        )
    }

    #[allow(deprecated)]
    #[allow(clippy::manual_async_fn)]
    fn list_resources(
        &self,
        _request: Option<PaginatedRequestParams>,
        _context: rmcp::service::RequestContext<rmcp::service::RoleServer>,
    ) -> impl std::future::Future<Output = Result<ListResourcesResult, ErrorData>> + Send + '_ {
        async {
            let mut resources: Vec<Resource> = RESOURCES
                .iter()
                .map(|(uri, name, _)| {
                    Annotated::new(
                        RawResource::new(*uri, *name).with_mime_type(resource_mime_type(uri)),
                        None,
                    )
                })
                .collect();
            resources.push(Annotated::new(
                RawResource::new(
                    "vulcan://agent/health",
                    "Agent Health — first-run setup, paper, wallet, registration, and skill checks",
                )
                .with_mime_type("application/json"),
                None,
            ));
            resources.push(Annotated::new(
                RawResource::new(
                    "vulcan://agent/session-summary",
                    "Agent Session Summary — recent redacted actions, PnL, and transactions",
                )
                .with_mime_type("text/markdown"),
                None,
            ));
            resources.push(Annotated::new(
                RawResource::new(
                    "vulcan://agent/position-report",
                    "Agent Position Report — live trader state plus session actions",
                )
                .with_mime_type("text/markdown"),
                None,
            ));
            Ok(ListResourcesResult::with_all_items(resources))
        }
    }

    #[allow(deprecated)]
    #[allow(clippy::manual_async_fn)]
    fn read_resource(
        &self,
        request: ReadResourceRequestParams,
        _context: rmcp::service::RequestContext<rmcp::service::RoleServer>,
    ) -> impl std::future::Future<Output = Result<ReadResourceResult, ErrorData>> + Send + '_ {
        async move {
            let uri = request.uri.as_str();
            if uri == "vulcan://agent/health" {
                let health = commands::agent::health(
                    &self.ctx,
                    None,
                    crate::cli::agent::AgentScope::User,
                    None,
                )
                .await
                .map_err(|e| {
                    ErrorData::internal_error(format!("Could not build agent health: {}", e), None)
                })?;
                let text = serde_json::to_string_pretty(&health).map_err(|e| {
                    ErrorData::internal_error(
                        format!("Could not serialize agent health: {}", e),
                        None,
                    )
                })?;
                return Ok(ReadResourceResult::new(vec![ResourceContents::text(
                    text,
                    uri.to_string(),
                )
                .with_mime_type("application/json")]));
            }
            if uri == "vulcan://agent/session-summary" {
                let path = self
                    .ctx
                    .agent_log
                    .as_ref()
                    .map(|log| log.path().to_path_buf())
                    .unwrap_or_else(|| default_log_path(&self.ctx.vulcan_dir));
                let limit = self.ctx.config.agent_log.max_summary_entries.max(1);
                let records = read_recent(&path, limit).map_err(|e| {
                    ErrorData::internal_error(format!("Could not read agent log: {}", e), None)
                })?;
                let summary = summarize_records(&records);
                let text = summary_markdown(&summary);
                return Ok(ReadResourceResult::new(vec![ResourceContents::text(
                    text,
                    uri.to_string(),
                )
                .with_mime_type("text/markdown")]));
            }
            if uri == "vulcan://agent/position-report" {
                let path = self
                    .ctx
                    .agent_log
                    .as_ref()
                    .map(|log| log.path().to_path_buf())
                    .unwrap_or_else(|| default_log_path(&self.ctx.vulcan_dir));
                let limit = self.ctx.config.agent_log.max_summary_entries.max(1);
                let records = read_recent(&path, limit).map_err(|e| {
                    ErrorData::internal_error(format!("Could not read agent log: {}", e), None)
                })?;
                let live = execute_snapshot_inner(&self.ctx, PortfolioSections::all())
                    .await
                    .and_then(|snapshot| {
                        serde_json::to_value(snapshot).map_err(|e| {
                            crate::error::VulcanError::internal(
                                "AGENT_REPORT_SERIALIZE_FAILED",
                                e.to_string(),
                            )
                        })
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
                    "all".to_string(),
                    limit,
                );
                let text = position_report_markdown(&report);
                return Ok(ReadResourceResult::new(vec![ResourceContents::text(
                    text,
                    uri.to_string(),
                )
                .with_mime_type("text/markdown")]));
            }
            let content = RESOURCES.iter().find(|(u, _, _)| *u == uri);
            match content {
                Some((uri, _, text)) => Ok(ReadResourceResult::new(vec![ResourceContents::text(
                    *text, *uri,
                )
                .with_mime_type(resource_mime_type(uri))])),
                None => Err(ErrorData::resource_not_found(
                    format!("Unknown resource: {}", uri),
                    None,
                )),
            }
        }
    }

    #[allow(deprecated)]
    #[allow(clippy::manual_async_fn)]
    fn list_tools(
        &self,
        _request: Option<PaginatedRequestParams>,
        _context: rmcp::service::RequestContext<rmcp::service::RoleServer>,
    ) -> impl std::future::Future<Output = Result<ListToolsResult, ErrorData>> + Send + '_ {
        async {
            let tools: Vec<Tool> = self
                .visible_tools()
                .iter()
                .map(|t| Self::to_rmcp_tool(t))
                .collect();
            Ok(ListToolsResult::with_all_items(tools))
        }
    }

    #[allow(deprecated)]
    #[allow(clippy::manual_async_fn)]
    fn call_tool(
        &self,
        request: CallToolRequestParams,
        _context: rmcp::service::RequestContext<rmcp::service::RoleServer>,
    ) -> impl std::future::Future<Output = Result<CallToolResult, ErrorData>> + Send + '_ {
        async move {
            let started = std::time::Instant::now();
            let name = request.name.as_ref();
            let args = match &request.arguments {
                Some(map) => Value::Object(map.clone()),
                None => Value::Object(serde_json::Map::new()),
            };

            // Audit log to stderr (tool name + arg keys only, never values)
            let arg_keys: Vec<&String> = match &args {
                Value::Object(m) => m.keys().collect(),
                _ => vec![],
            };
            eprintln!("[mcp] call_tool: {} args={:?}", name, arg_keys);

            // Find tool definition
            let tool_def = self.tools.iter().find(|t| t.name == name);
            let tool_def = match tool_def {
                Some(t) => t,
                None => {
                    if let Some(log) = &self.ctx.agent_log {
                        log.record_call(
                            "mcp",
                            name,
                            &args,
                            false,
                            None,
                            "error",
                            started.elapsed().as_millis(),
                            None,
                            Some("UNKNOWN_TOOL"),
                        );
                    }
                    return Ok(CallToolResult::error(vec![Content::text(format!(
                        "Unknown tool: {}",
                        name
                    ))]));
                }
            };

            // Dangerous command gating
            let mut acknowledged_for_log = None;
            let call_dangerous = tool_def.dangerous || is_dynamic_dangerous(name, &args);
            if call_dangerous {
                if !self.allow_dangerous {
                    if let Some(log) = &self.ctx.agent_log {
                        log.record_call(
                            "mcp",
                            name,
                            &args,
                            call_dangerous,
                            Some(false),
                            "blocked",
                            started.elapsed().as_millis(),
                            None,
                            Some("DANGEROUS_NOT_ALLOWED"),
                        );
                    }
                    return Ok(CallToolResult::error(vec![Content::text(
                        "This tool is dangerous and --allow-dangerous was not set on the server.",
                    )]));
                }
                let acknowledged = acknowledged_arg(&args);
                acknowledged_for_log = Some(acknowledged);
                if !acknowledged {
                    if let Some(log) = &self.ctx.agent_log {
                        log.record_call(
                            "mcp",
                            name,
                            &args,
                            call_dangerous,
                            acknowledged_for_log,
                            "blocked",
                            started.elapsed().as_millis(),
                            None,
                            Some("DANGEROUS_GATE"),
                        );
                    }
                    return Ok(CallToolResult::error(vec![Content::text(
                        "Dangerous operation requires acknowledged=true. \
                         This is a real financial transaction. Set acknowledged=true to proceed.",
                    )]));
                }
            }

            // Dispatch
            match self.dispatch(name, &args).await {
                Ok(result) => {
                    if let Some(log) = &self.ctx.agent_log {
                        log.record_call(
                            "mcp",
                            name,
                            &args,
                            call_dangerous,
                            acknowledged_for_log,
                            "ok",
                            started.elapsed().as_millis(),
                            Some(&result),
                            None,
                        );
                    }
                    let text = serde_json::to_string_pretty(&result).unwrap_or_default();
                    Ok(CallToolResult::success(vec![Content::text(text)]))
                }
                Err(e) => {
                    eprintln!("[mcp] error: {}", e);
                    if let Some(log) = &self.ctx.agent_log {
                        log.record_call(
                            "mcp",
                            name,
                            &args,
                            call_dangerous,
                            acknowledged_for_log,
                            "error",
                            started.elapsed().as_millis(),
                            None,
                            Some(&e.code),
                        );
                    }
                    let error_json = serde_json::json!({
                        "category": e.category.to_string(),
                        "code": e.code,
                        "message": e.message,
                        "retryable": e.category.is_retryable(),
                    });
                    Ok(CallToolResult::error(vec![Content::text(
                        serde_json::to_string_pretty(&error_json).unwrap_or(e.message.clone()),
                    )]))
                }
            }
        }
    }
}

// ── Arg extraction helpers ──────────────────────────────────────────────

fn parse_side(args: &Value, key: &str) -> Result<Side, crate::error::VulcanError> {
    let raw = arg_str(args, key)?;
    match raw.to_ascii_lowercase().as_str() {
        "buy" | "bid" | "long" => Ok(Side::Bid),
        "sell" | "ask" | "short" => Ok(Side::Ask),
        other => Err(crate::error::VulcanError::validation(
            "INVALID_SIDE",
            format!("side must be 'buy' or 'sell', got '{}'", other),
        )),
    }
}

fn parse_strategy_side(args: &Value) -> Result<StrategySideArg, crate::error::VulcanError> {
    match arg_str(args, "side")?.as_str() {
        "buy" => Ok(StrategySideArg::Buy),
        "sell" => Ok(StrategySideArg::Sell),
        other => Err(crate::error::VulcanError::validation(
            "INVALID_SIDE",
            format!("side must be buy or sell, got {}", other),
        )),
    }
}

fn parse_strategy_mode(args: &Value) -> Result<StrategyModeArg, crate::error::VulcanError> {
    match arg_str_or(args, "mode", "paper").as_str() {
        "paper" => Ok(StrategyModeArg::Paper),
        "dry_run" => Ok(StrategyModeArg::DryRun),
        "confirm_each" => Ok(StrategyModeArg::ConfirmEach),
        "auto_execute" => Ok(StrategyModeArg::AutoExecute),
        other => Err(crate::error::VulcanError::validation(
            "INVALID_STRATEGY_MODE",
            format!("unsupported strategy mode {}", other),
        )),
    }
}

fn parse_strategy_margin_mode(
    args: &Value,
) -> Result<StrategyMarginModeArg, crate::error::VulcanError> {
    match arg_str_or(args, "margin_mode", "cross").as_str() {
        "cross" => Ok(StrategyMarginModeArg::Cross),
        "isolated" => Ok(StrategyMarginModeArg::Isolated),
        other => Err(crate::error::VulcanError::validation(
            "INVALID_STRATEGY_MARGIN_MODE",
            format!("margin_mode must be cross or isolated, got {}", other),
        )),
    }
}

fn is_dynamic_dangerous(name: &str, args: &Value) -> bool {
    ((name == "vulcan_strategy_twap_start"
        || name == "vulcan_strategy_grid_start"
        || name == "vulcan_strategy_ta_start")
        && matches!(
            arg_str_or(args, "mode", "paper").as_str(),
            "confirm_each" | "auto_execute"
        ))
        || name == "vulcan_strategy_resume"
        || (name == "vulcan_strategy_finalize"
            && (arg_bool_or(args, "cancel_orders", false)
                || arg_bool_or(args, "close_position", false)))
}

fn acknowledged_arg(args: &Value) -> bool {
    args.get("acknowledged")
        .map(|v| {
            v.as_bool().unwrap_or_else(|| {
                // Some MCP clients send "true" as a string instead of a JSON boolean.
                v.as_str()
                    .map(|s| s.eq_ignore_ascii_case("true"))
                    .unwrap_or(false)
            })
        })
        .unwrap_or(false)
}

fn arg_str(args: &Value, key: &str) -> Result<String, crate::error::VulcanError> {
    args.get(key)
        .and_then(|v| v.as_str())
        .map(|s| s.to_string())
        .ok_or_else(|| {
            crate::error::VulcanError::validation(
                "MISSING_ARG",
                format!("Required argument '{}' is missing or not a string", key),
            )
        })
}

fn arg_str_opt(args: &Value, key: &str) -> Option<String> {
    args.get(key)
        .and_then(|v| v.as_str())
        .map(|s| s.to_string())
}

fn arg_str_or(args: &Value, key: &str, default: &str) -> String {
    args.get(key)
        .and_then(|v| v.as_str())
        .unwrap_or(default)
        .to_string()
}

fn arg_f64(args: &Value, key: &str) -> Result<f64, crate::error::VulcanError> {
    args.get(key)
        .and_then(|v| {
            v.as_f64()
                .or_else(|| v.as_str().and_then(|s| s.parse::<f64>().ok()))
        })
        .ok_or_else(|| {
            crate::error::VulcanError::validation(
                "MISSING_ARG",
                format!("Required argument '{}' is missing or not a number", key),
            )
        })
}

fn arg_f64_opt(args: &Value, key: &str) -> Option<f64> {
    args.get(key).and_then(|v| {
        v.as_f64()
            .or_else(|| v.as_str().and_then(|s| s.parse::<f64>().ok()))
    })
}

fn arg_usize_or(args: &Value, key: &str, default: usize) -> usize {
    args.get(key)
        .and_then(|v| {
            v.as_u64()
                .or_else(|| v.as_str().and_then(|s| s.parse::<u64>().ok()))
        })
        .map(|v| v as usize)
        .unwrap_or(default)
}

fn arg_i64_or(args: &Value, key: &str, default: i64) -> i64 {
    args.get(key)
        .and_then(|v| {
            v.as_i64()
                .or_else(|| v.as_str().and_then(|s| s.parse::<i64>().ok()))
        })
        .unwrap_or(default)
}

fn arg_u64_or(args: &Value, key: &str, default: u64) -> u64 {
    args.get(key)
        .and_then(|v| {
            v.as_u64()
                .or_else(|| v.as_str().and_then(|s| s.parse::<u64>().ok()))
        })
        .unwrap_or(default)
}

fn arg_u64_opt(args: &Value, key: &str) -> Option<u64> {
    args.get(key).and_then(|v| {
        v.as_u64()
            .or_else(|| v.as_str().and_then(|s| s.parse::<u64>().ok()))
    })
}

fn arg_u32_opt(args: &Value, key: &str) -> Option<u32> {
    args.get(key).and_then(|v| {
        v.as_u64()
            .or_else(|| v.as_str().and_then(|s| s.parse::<u64>().ok()))
            .map(|v| v as u32)
    })
}

fn arg_params_map(
    args: &Value,
    key: &str,
) -> Result<std::collections::BTreeMap<String, f64>, crate::error::VulcanError> {
    use std::collections::BTreeMap;
    let Some(value) = args.get(key) else {
        return Ok(BTreeMap::new());
    };
    if value.is_null() {
        return Ok(BTreeMap::new());
    }
    let obj = value.as_object().ok_or_else(|| {
        crate::error::VulcanError::validation(
            "INVALID_INDICATOR_PARAMS",
            format!("'{key}' must be a JSON object of name→number"),
        )
    })?;
    let mut out = BTreeMap::new();
    for (k, v) in obj {
        let n = v.as_f64().ok_or_else(|| {
            crate::error::VulcanError::validation(
                "INVALID_INDICATOR_PARAMS",
                format!("'{key}.{k}' must be a number"),
            )
        })?;
        out.insert(k.clone(), n);
    }
    Ok(out)
}

fn arg_bool_or(args: &Value, key: &str, default: bool) -> bool {
    args.get(key)
        .and_then(|v| {
            v.as_bool()
                .or_else(|| v.as_str().map(|s| s.eq_ignore_ascii_case("true")))
        })
        .unwrap_or(default)
}

fn arg_u8(args: &Value, key: &str) -> Result<u8, crate::error::VulcanError> {
    args.get(key)
        .and_then(|v| {
            v.as_u64()
                .or_else(|| v.as_str().and_then(|s| s.parse::<u64>().ok()))
        })
        .map(|v| v as u8)
        .ok_or_else(|| {
            crate::error::VulcanError::validation(
                "MISSING_ARG",
                format!("Required argument '{}' is missing or not a number", key),
            )
        })
}

fn arg_u32(args: &Value, key: &str) -> Result<u32, crate::error::VulcanError> {
    args.get(key)
        .and_then(|v| {
            v.as_u64()
                .or_else(|| v.as_str().and_then(|s| s.parse::<u64>().ok()))
        })
        .map(|v| v as u32)
        .ok_or_else(|| {
            crate::error::VulcanError::validation(
                "MISSING_ARG",
                format!("Required argument '{}' is missing or not a number", key),
            )
        })
}

fn arg_order_array(args: &Value, key: &str) -> Result<Vec<(f64, u64)>, crate::error::VulcanError> {
    args.get(key)
        .and_then(|v| v.as_array())
        .map(|arr| {
            arr.iter()
                .filter_map(|entry| {
                    let price = entry.get("price").and_then(|v| v.as_f64())?;
                    let size = entry
                        .get("size")
                        .and_then(|v| v.as_u64().or_else(|| v.as_f64().map(|f| f as u64)))?;
                    Some((price, size))
                })
                .collect()
        })
        .ok_or_else(|| {
            crate::error::VulcanError::validation(
                "MISSING_ARG",
                format!(
                    "Required argument '{}' is missing or not an array of {{price, size}} objects",
                    key
                ),
            )
        })
}

fn arg_str_array_opt(args: &Value, key: &str) -> Option<Vec<String>> {
    args.get(key).and_then(|v| v.as_array()).map(|arr| {
        arr.iter()
            .filter_map(|v| v.as_str().map(|s| s.to_string()))
            .collect()
    })
}

fn arg_str_array(args: &Value, key: &str) -> Result<Vec<String>, crate::error::VulcanError> {
    args.get(key)
        .and_then(|v| v.as_array())
        .map(|arr| {
            arr.iter()
                .filter_map(|v| v.as_str().map(|s| s.to_string()))
                .collect()
        })
        .ok_or_else(|| {
            crate::error::VulcanError::validation(
                "MISSING_ARG",
                format!("Required argument '{}' is missing or not an array", key),
            )
        })
}

/// Resolve a single side of TP/SL input from MCP args. Accepts either:
///   - `single_key` as a number (legacy: covers full position), or
///   - `levels_key` as an array of `{ price, size?, size_lots? }` objects.
///
/// The two are mutually exclusive. Returns an empty Vec if neither is set.
fn arg_tpsl_levels(
    args: &Value,
    single_key: &str,
    levels_key: &str,
) -> Result<Vec<commands::trade::TpSlInput>, crate::error::VulcanError> {
    let single = args.get(single_key).and_then(|v| v.as_f64());
    let levels = args.get(levels_key).and_then(|v| v.as_array());

    if single.is_some() && levels.is_some() {
        return Err(crate::error::VulcanError::validation(
            "TPSL_FLAG_CONFLICT",
            format!("'{}' and '{}' cannot be combined", single_key, levels_key),
        ));
    }
    if let Some(price) = single {
        return Ok(vec![commands::trade::TpSlInput::full(price)]);
    }
    let Some(arr) = levels else {
        return Ok(Vec::new());
    };
    let mut out = Vec::with_capacity(arr.len());
    for (idx, entry) in arr.iter().enumerate() {
        let price = entry.get("price").and_then(|v| v.as_f64()).ok_or_else(|| {
            crate::error::VulcanError::validation(
                "TPSL_LEVEL_INVALID",
                format!("'{}'[{}] missing required number 'price'", levels_key, idx),
            )
        })?;
        let size_tokens = entry.get("size").and_then(|v| v.as_f64());
        let size_lots = entry
            .get("size_lots")
            .and_then(|v| v.as_u64().or_else(|| v.as_f64().map(|f| f as u64)));
        let size = match (size_tokens, size_lots) {
            (Some(_), Some(_)) => {
                return Err(crate::error::VulcanError::validation(
                    "TPSL_LEVEL_INVALID",
                    format!(
                        "'{}'[{}] cannot specify both 'size' and 'size_lots'",
                        levels_key, idx
                    ),
                ));
            }
            (Some(t), None) => commands::trade::TpSlSize::Tokens(t),
            (None, Some(n)) => commands::trade::TpSlSize::Lots(n),
            (None, None) => commands::trade::TpSlSize::Full,
        };
        out.push(commands::trade::TpSlInput { price, size });
    }
    Ok(out)
}

#[cfg(test)]
mod tests {
    #[test]
    fn tool_catalog_cli_version_matches_crate() {
        let catalog: serde_json::Value =
            serde_json::from_str(include_str!("../../../agents/tool-catalog.json"))
                .expect("tool-catalog.json must be valid JSON");
        let catalog_version = catalog
            .get("cli_version")
            .and_then(|v| v.as_str())
            .expect("tool-catalog.json must have a string `cli_version`");
        assert_eq!(
            catalog_version,
            env!("CARGO_PKG_VERSION"),
            "agents/tool-catalog.json `cli_version` is out of sync with vulcan-lib Cargo.toml — bump it on every release"
        );
    }
}

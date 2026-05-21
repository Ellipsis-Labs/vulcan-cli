//! Paper trading command execution.

use crate::cli::paper::{PaperCommand, PaperOrderArgs, PaperOrderTypeArg};
use crate::commands::trade::{parse_cli_tpsl_levels, TpSlInput};
use crate::context::AppContext;
use crate::error::VulcanError;
use crate::output::{render_success, TableRenderable};
use crate::paper::{
    self, PaperCancelResult, PaperCancelTpSlResult, PaperFillsResult, PaperInitResult,
    PaperOrderResult, PaperOrderType, PaperOrdersResult, PaperPositionsResult,
    PaperReconcileResult, PaperSetTpSlResult, PaperSide, PaperSizeInput, PaperStatus,
    PaperTriggersResult,
};

pub async fn execute(ctx: &AppContext, cmd: PaperCommand) -> Result<(), VulcanError> {
    match cmd {
        PaperCommand::Init {
            balance,
            currency,
            fee_bps,
        }
        | PaperCommand::Reset {
            balance,
            currency,
            fee_bps,
        } => {
            let result = init(ctx, balance, Some(currency), fee_bps).await?;
            render_success(
                ctx.output_format,
                &result,
                serde_json::json!({ "mode": "paper" }),
            );
        }
        PaperCommand::Status => {
            let result = status(ctx).await?;
            render_success(
                ctx.output_format,
                &result,
                serde_json::json!({ "mode": "paper" }),
            );
        }
        PaperCommand::Positions => {
            let (_, state) = paper::load_state(&ctx.vulcan_dir)?;
            let result = paper::positions(&state);
            render_success(
                ctx.output_format,
                &result,
                serde_json::json!({ "mode": "paper" }),
            );
        }
        PaperCommand::Orders => {
            let (_, state) = paper::load_state(&ctx.vulcan_dir)?;
            let result = paper::orders(&state);
            render_success(
                ctx.output_format,
                &result,
                serde_json::json!({ "mode": "paper" }),
            );
        }
        PaperCommand::Fills { limit } => {
            let (_, state) = paper::load_state(&ctx.vulcan_dir)?;
            let result = paper::fills(&state, limit);
            render_success(
                ctx.output_format,
                &result,
                serde_json::json!({ "mode": "paper" }),
            );
        }
        PaperCommand::Buy(args) => {
            let result = order(ctx, PaperSide::Buy, args).await?;
            render_success(
                ctx.output_format,
                &result,
                serde_json::json!({ "mode": "paper" }),
            );
        }
        PaperCommand::Sell(args) => {
            let result = order(ctx, PaperSide::Sell, args).await?;
            render_success(
                ctx.output_format,
                &result,
                serde_json::json!({ "mode": "paper" }),
            );
        }
        PaperCommand::Cancel { order_id } => {
            let result = cancel(ctx, &order_id)?;
            render_success(
                ctx.output_format,
                &result,
                serde_json::json!({ "mode": "paper" }),
            );
        }
        PaperCommand::CancelAll { symbol } => {
            let result = cancel_all(ctx, symbol.as_deref())?;
            render_success(
                ctx.output_format,
                &result,
                serde_json::json!({ "mode": "paper" }),
            );
        }
        PaperCommand::Reconcile { symbol } => {
            let result = reconcile(ctx, symbol.as_deref()).await?;
            render_success(
                ctx.output_format,
                &result,
                serde_json::json!({ "mode": "paper" }),
            );
        }
        PaperCommand::SetTpsl {
            symbol,
            tp,
            sl,
            tp_levels,
            sl_levels,
        } => {
            let tp_inputs = parse_cli_tpsl_levels("--tp", "--tp-level", tp, &tp_levels)?;
            let sl_inputs = parse_cli_tpsl_levels("--sl", "--sl-level", sl, &sl_levels)?;
            let result = set_tpsl(ctx, &symbol, tp_inputs, sl_inputs).await?;
            render_success(
                ctx.output_format,
                &result,
                serde_json::json!({ "mode": "paper" }),
            );
        }
        PaperCommand::CancelTpsl { symbol, tp, sl } => {
            let result = cancel_tpsl(ctx, &symbol, tp, sl)?;
            render_success(
                ctx.output_format,
                &result,
                serde_json::json!({ "mode": "paper" }),
            );
        }
        PaperCommand::Triggers { symbol } => {
            let (_, state) = paper::load_state(&ctx.vulcan_dir)?;
            let result = paper::triggers(&state, symbol.as_deref());
            render_success(
                ctx.output_format,
                &result,
                serde_json::json!({ "mode": "paper" }),
            );
        }
    }
    Ok(())
}

pub async fn init(
    ctx: &AppContext,
    balance: Option<f64>,
    currency: Option<String>,
    fee_bps: Option<f64>,
) -> Result<PaperInitResult, VulcanError> {
    let (path, state) = paper::init_state(&ctx.vulcan_dir, balance, currency, fee_bps)?;
    let status = paper::status(ctx, &path, state).await?;
    Ok(PaperInitResult {
        mode: "paper".to_string(),
        path: path.to_string_lossy().to_string(),
        state: status,
    })
}

pub async fn status(ctx: &AppContext) -> Result<PaperStatus, VulcanError> {
    let (path, state) = paper::load_state(&ctx.vulcan_dir)?;
    paper::status(ctx, &path, state).await
}

pub async fn order(
    ctx: &AppContext,
    side: PaperSide,
    args: PaperOrderArgs,
) -> Result<PaperOrderResult, VulcanError> {
    let (path, state) = paper::load_state(&ctx.vulcan_dir)?;
    let order_type = match args.order_type {
        PaperOrderTypeArg::Market => PaperOrderType::Market,
        PaperOrderTypeArg::Limit => PaperOrderType::Limit,
    };
    let (result, _) = paper::place_order(
        ctx,
        &path,
        state,
        paper::PaperOrderRequest {
            symbol: args.symbol,
            side,
            order_type,
            size: PaperSizeInput {
                size_lots: args.size,
                tokens: args.tokens,
                notional_usdc: args.notional_usdc,
            },
            price: args.price,
            tp: args.tp,
            sl: args.sl,
        },
    )
    .await?;
    Ok(result)
}

pub fn cancel(ctx: &AppContext, order_id: &str) -> Result<PaperCancelResult, VulcanError> {
    let (path, state) = paper::load_state(&ctx.vulcan_dir)?;
    let (result, _) = paper::cancel(&path, state, order_id)?;
    Ok(result)
}

pub fn cancel_all(
    ctx: &AppContext,
    symbol: Option<&str>,
) -> Result<PaperCancelResult, VulcanError> {
    let (path, state) = paper::load_state(&ctx.vulcan_dir)?;
    let (result, _) = paper::cancel_all(&path, state, symbol)?;
    Ok(result)
}

pub async fn reconcile(
    ctx: &AppContext,
    symbol: Option<&str>,
) -> Result<PaperReconcileResult, VulcanError> {
    let (path, state) = paper::load_state(&ctx.vulcan_dir)?;
    let (result, _) = paper::reconcile(ctx, &path, state, symbol).await?;
    Ok(result)
}

pub async fn set_tpsl(
    ctx: &AppContext,
    symbol: &str,
    tp_inputs: Vec<TpSlInput>,
    sl_inputs: Vec<TpSlInput>,
) -> Result<PaperSetTpSlResult, VulcanError> {
    let (path, state) = paper::load_state(&ctx.vulcan_dir)?;
    let (result, _) = paper::set_tpsl(ctx, &path, state, symbol, tp_inputs, sl_inputs).await?;
    Ok(result)
}

pub fn cancel_tpsl(
    ctx: &AppContext,
    symbol: &str,
    cancel_tp: bool,
    cancel_sl: bool,
) -> Result<PaperCancelTpSlResult, VulcanError> {
    let (path, state) = paper::load_state(&ctx.vulcan_dir)?;
    let (result, _) = paper::cancel_tpsl(&path, state, symbol, cancel_tp, cancel_sl)?;
    Ok(result)
}

pub fn triggers(
    ctx: &AppContext,
    symbol: Option<&str>,
) -> Result<PaperTriggersResult, VulcanError> {
    let (_, state) = paper::load_state(&ctx.vulcan_dir)?;
    Ok(paper::triggers(&state, symbol))
}

impl TableRenderable for PaperInitResult {
    fn render_table(&self) {
        println!("[PAPER] initialized");
        println!("  Path: {}", self.path);
        self.state.render_table();
    }
}

impl TableRenderable for PaperStatus {
    fn render_table(&self) {
        println!("[PAPER] Account");
        println!("─────────────────────────────────────────");
        println!("  Balance:     {:.4} {}", self.balance, self.currency);
        println!("  Equity:      {:.4} {}", self.equity, self.currency);
        println!(
            "  Pos notional:{:>9.4} {}",
            self.position_notional_usdc, self.currency
        );
        println!("  Exposure:    {:.2}x", self.exposure_ratio);
        println!("  Realized:    {:+.4}", self.realized_pnl);
        println!("  Unrealized:  {:+.4}", self.unrealized_pnl);
        println!("  Fees paid:   {:.4}", self.fees_paid);
        println!("  Positions:   {}", self.open_positions);
        println!("  Open orders: {}", self.open_orders);
        println!("  Triggers:    {}", self.triggers);
        println!("  Fills:       {}", self.fills);
    }
}

impl TableRenderable for PaperOrderResult {
    fn render_table(&self) {
        println!(
            "[PAPER] {} {} {} {}",
            self.action, self.order_type, self.side, self.symbol
        );
        if let Some(order_id) = &self.order_id {
            println!("  Order ID: {}", order_id);
        }
        if let Some(fill) = &self.fill {
            println!(
                "  Fill: {} {} @ {:.4}, fee {:.4}, realized {:+.4}",
                fill.size_tokens, fill.symbol, fill.price, fill.fee, fill.realized_pnl
            );
        }
        for trig in &self.attached_triggers {
            let parent = trig
                .parent_order_id
                .as_deref()
                .map(|id| format!("pending on {}", id))
                .unwrap_or_else(|| "active".to_string());
            println!(
                "  Attached {}: {} @ {:.4} ({})",
                trig.kind.as_str(),
                trig.trigger_id,
                trig.trigger_price,
                parent
            );
        }
        for fill in &self.trigger_fills {
            println!(
                "  Trigger fill: {} {} {} @ {:.4}, fee {:.4}, realized {:+.4}",
                fill.order_id.as_deref().unwrap_or("?"),
                fill.size_tokens,
                fill.symbol,
                fill.price,
                fill.fee,
                fill.realized_pnl
            );
        }
        println!();
        self.state.render_table();
    }
}

impl TableRenderable for PaperCancelResult {
    fn render_table(&self) {
        println!("[PAPER] Cancelled {} orders", self.cancelled);
        for id in &self.order_ids {
            println!("  {}", id);
        }
        println!();
        self.state.render_table();
    }
}

impl TableRenderable for PaperReconcileResult {
    fn render_table(&self) {
        println!(
            "[PAPER] Reconciled {} orders, {} fills",
            self.checked_orders,
            self.fills.len()
        );
        for fill in &self.fills {
            println!(
                "  {} {} {} @ {:.4}",
                fill.fill_id, fill.side, fill.symbol, fill.price
            );
        }
        println!();
        self.state.render_table();
    }
}

impl TableRenderable for PaperPositionsResult {
    fn render_table(&self) {
        println!("[PAPER] Positions");
        if self.positions.is_empty() {
            println!("No paper positions.");
            return;
        }
        let rows: Vec<Vec<String>> = self
            .positions
            .iter()
            .map(|p| {
                let notional = p.size_tokens * p.mark_price;
                let exposure = if self.equity.abs() < f64::EPSILON {
                    0.0
                } else {
                    notional / self.equity
                };
                let tpsl = match (p.tp_levels.len(), p.sl_levels.len()) {
                    (0, 0) => "—".to_string(),
                    (t, s) => format!("{}tp/{}sl", t, s),
                };
                vec![
                    p.symbol.clone(),
                    p.side.clone(),
                    format!("{:.6}", p.size_tokens),
                    format!("{:.2}", notional),
                    format!("{:.2}x", exposure),
                    format!("{:.4}", p.entry_price),
                    format!("{:.4}", p.mark_price),
                    format!("{:+.4}", p.unrealized_pnl),
                    tpsl,
                ]
            })
            .collect();
        crate::output::table::render_table(
            &[
                "Symbol", "Side", "Tokens", "Notional", "Exposure", "Entry", "Mark", "uPnL",
                "TP/SL",
            ],
            rows,
        );
    }
}

impl TableRenderable for PaperOrdersResult {
    fn render_table(&self) {
        println!("[PAPER] Open Orders");
        if self.orders.is_empty() {
            println!("No paper orders.");
            return;
        }
        let rows: Vec<Vec<String>> = self
            .orders
            .iter()
            .map(|o| {
                vec![
                    o.order_id.clone(),
                    o.symbol.clone(),
                    o.side.clone(),
                    format!("{:.4}", o.price),
                    format!("{:.6}", o.size_tokens),
                ]
            })
            .collect();
        crate::output::table::render_table(
            &["Order ID", "Symbol", "Side", "Price", "Tokens"],
            rows,
        );
    }
}

impl TableRenderable for PaperFillsResult {
    fn render_table(&self) {
        println!("[PAPER] Fills");
        if self.fills.is_empty() {
            println!("No paper fills.");
            return;
        }
        let rows: Vec<Vec<String>> = self
            .fills
            .iter()
            .map(|f| {
                vec![
                    f.timestamp.clone(),
                    f.fill_id.clone(),
                    f.symbol.clone(),
                    f.side.clone(),
                    f.order_type.clone(),
                    format!("{:.4}", f.price),
                    format!("{:.6}", f.size_tokens),
                    format!("{:+.4}", f.realized_pnl),
                ]
            })
            .collect();
        crate::output::table::render_table(
            &[
                "Time", "Fill ID", "Symbol", "Side", "Type", "Price", "Tokens", "PnL",
            ],
            rows,
        );
    }
}

impl TableRenderable for PaperSetTpSlResult {
    fn render_table(&self) {
        println!(
            "[PAPER] TP/SL set on {} ({} position)",
            self.symbol, self.position_side
        );
        if !self.tp_levels.is_empty() {
            println!("  Take-profit:");
            for level in &self.tp_levels {
                println!(
                    "    {} @ {:.4}  size {:.6} ({} lots)",
                    level.trigger_id, level.price, level.size_tokens, level.size_lots
                );
            }
        }
        if !self.sl_levels.is_empty() {
            println!("  Stop-loss:");
            for level in &self.sl_levels {
                println!(
                    "    {} @ {:.4}  size {:.6} ({} lots)",
                    level.trigger_id, level.price, level.size_tokens, level.size_lots
                );
            }
        }
        println!();
        self.state.render_table();
    }
}

impl TableRenderable for PaperCancelTpSlResult {
    fn render_table(&self) {
        println!(
            "[PAPER] Cancelled {} TP/SL triggers on {}",
            self.cancelled, self.symbol
        );
        for id in &self.trigger_ids {
            println!("  {}", id);
        }
        println!();
        self.state.render_table();
    }
}

impl TableRenderable for PaperTriggersResult {
    fn render_table(&self) {
        println!("[PAPER] Triggers");
        if self.triggers.is_empty() {
            println!("No paper triggers.");
            return;
        }
        let rows: Vec<Vec<String>> = self
            .triggers
            .iter()
            .map(|t| {
                vec![
                    t.trigger_id.clone(),
                    t.symbol.clone(),
                    t.kind.as_str().to_string(),
                    t.position_side.clone(),
                    format!("{:.4}", t.trigger_price),
                    format!("{:.6}", t.size_tokens),
                    t.parent_order_id
                        .clone()
                        .unwrap_or_else(|| "position".to_string()),
                ]
            })
            .collect();
        crate::output::table::render_table(
            &[
                "Trigger ID",
                "Symbol",
                "Kind",
                "Side",
                "Price",
                "Tokens",
                "Parent",
            ],
            rows,
        );
    }
}

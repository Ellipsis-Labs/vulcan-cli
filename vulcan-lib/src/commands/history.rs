//! History command execution.

use crate::cli::history::HistoryCommand;
use crate::context::AppContext;
use crate::error::VulcanError;
use crate::history::HistoryResult;
use crate::output::{render_success, TableRenderable};

pub async fn execute(ctx: &AppContext, cmd: HistoryCommand) -> Result<(), VulcanError> {
    let result = execute_inner(ctx, cmd).await?;
    render_success(ctx.output_format, &result, serde_json::Value::Null);
    Ok(())
}

pub async fn execute_inner(
    ctx: &AppContext,
    cmd: HistoryCommand,
) -> Result<HistoryResult, VulcanError> {
    match cmd {
        HistoryCommand::Trades {
            symbol,
            limit,
            cursor,
        } => crate::history::trades(ctx, symbol.as_deref(), limit, cursor.as_deref()).await,
        HistoryCommand::Orders {
            symbol,
            limit,
            cursor,
        } => crate::history::orders(ctx, symbol.as_deref(), limit, cursor.as_deref()).await,
        HistoryCommand::Collateral { limit, cursor } => {
            crate::history::collateral(ctx, limit, cursor.as_deref()).await
        }
        HistoryCommand::Funding {
            symbol,
            limit,
            cursor,
        } => crate::history::funding(ctx, symbol.as_deref(), limit, cursor.as_deref()).await,
        HistoryCommand::Pnl { resolution, limit } => {
            crate::history::pnl(ctx, &resolution, limit).await
        }
    }
}

impl TableRenderable for HistoryResult {
    fn render_table(&self) {
        println!("History: {}", self.history_type);
        println!("─────────────────────────────────────────");
        if let Some(symbol) = &self.symbol {
            println!("  Symbol: {}", symbol);
        }
        println!("  Count:  {}", self.count);
        println!("  More:   {}", self.has_more);
        if let Some(cursor) = &self.next_cursor {
            println!("  Next:   {}", cursor);
        }
        if let Some(cursor) = &self.prev_cursor {
            println!("  Prev:   {}", cursor);
        }
        println!();
        match &self.data {
            serde_json::Value::Array(items) if items.is_empty() => {
                println!("No history rows.");
            }
            serde_json::Value::Array(items) => {
                for item in items.iter().take(10) {
                    println!(
                        "{}",
                        serde_json::to_string(item).unwrap_or_else(|_| "{}".to_string())
                    );
                }
                if items.len() > 10 {
                    println!("... {} more rows omitted in table output", items.len() - 10);
                }
            }
            other => println!(
                "{}",
                serde_json::to_string(other).unwrap_or_else(|_| "{}".to_string())
            ),
        }
    }
}

//! Portfolio snapshot — combined margin / positions / orders in one fetch.

use crate::cli::portfolio::PortfolioArgs;
use crate::commands::conditional_orders::{fetch_conditional_triggers, triggers_to_order_infos};
use crate::commands::margin::{project_margin_status, MarginStatusResult};
use crate::commands::position::{get_all_trader_views, project_positions, PositionListResult};
use crate::commands::trade::{project_orders, OrdersResult};
use crate::context::AppContext;
use crate::error::VulcanError;
use crate::output::{render_success, TableRenderable};
use serde::Serialize;

#[derive(Debug, Clone, Copy, Default)]
pub struct PortfolioSections {
    pub(crate) margin: bool,
    pub(crate) positions: bool,
    pub(crate) orders: bool,
}

impl PortfolioSections {
    pub fn all() -> Self {
        Self {
            margin: true,
            positions: true,
            orders: true,
        }
    }

    pub fn from_include(include: &[String]) -> Result<Self, VulcanError> {
        if include.is_empty() {
            return Ok(Self::all());
        }
        let mut sections = Self::default();
        for name in include {
            match name.as_str() {
                "margin" => sections.margin = true,
                "positions" => sections.positions = true,
                "orders" => sections.orders = true,
                other => {
                    return Err(VulcanError::validation(
                        "INVALID_INCLUDE",
                        format!(
                            "Unknown portfolio section '{}'. Valid: margin, positions, orders",
                            other
                        ),
                    ));
                }
            }
        }
        Ok(sections)
    }
}

#[derive(Debug, Default, Serialize)]
pub struct PortfolioSnapshot {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub margin: Option<MarginStatusResult>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub positions: Option<PositionListResult>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub orders: Option<OrdersResult>,
    /// Per-subaccount summaries for any non-cross (isolated) subaccounts.
    /// Empty when the trader has no isolated positions.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub isolated_subaccounts: Option<Vec<IsolatedSubaccountSummary>>,
    /// Aggregated totals across cross-margin (subaccount 0) and every isolated
    /// subaccount. Surfaced whenever the margin section is requested so the
    /// agent doesn't undercount collateral parked in isolated subaccounts.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub totals: Option<AccountTotals>,
}

#[derive(Debug, Clone, Serialize)]
pub struct IsolatedSubaccountSummary {
    pub subaccount_index: u8,
    /// Symbol of the position held by this isolated subaccount, if any.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub symbol: Option<String>,
    pub collateral_balance: String,
    pub effective_collateral: String,
    pub portfolio_value: String,
    pub unrealized_pnl: String,
    pub initial_margin: String,
    pub maintenance_margin: String,
}

#[derive(Debug, Clone, Serialize)]
pub struct AccountTotals {
    /// Sum of `collateral_balance` across cross and every isolated subaccount.
    pub total_collateral: String,
    /// Sum of `portfolio_value` (collateral + unrealized PnL) across cross and
    /// every isolated subaccount. This is the "true" account value.
    pub total_account_value: String,
    /// Sum of unrealized PnL across all subaccounts.
    pub total_unrealized_pnl: String,
    pub num_subaccounts: usize,
}

impl TableRenderable for PortfolioSnapshot {
    fn render_table(&self) {
        if let Some(m) = &self.margin {
            m.render_table();
            println!();
        }
        if let Some(t) = &self.totals {
            println!("Account Totals (cross + isolated):");
            println!("  Total collateral: {}", t.total_collateral);
            println!("  Total account value: {}", t.total_account_value);
            println!("  Total unrealized PnL: {}", t.total_unrealized_pnl);
            println!("  Subaccounts: {}", t.num_subaccounts);
            println!();
        }
        if let Some(isos) = &self.isolated_subaccounts {
            if !isos.is_empty() {
                println!("Isolated Subaccounts:");
                for iso in isos {
                    let label = iso.symbol.as_deref().unwrap_or("(empty)");
                    println!(
                        "  #{} {}: collateral={} portfolio_value={} uPnL={} init_margin={} maint_margin={}",
                        iso.subaccount_index,
                        label,
                        iso.collateral_balance,
                        iso.portfolio_value,
                        iso.unrealized_pnl,
                        iso.initial_margin,
                        iso.maintenance_margin,
                    );
                }
                println!();
            }
        }
        if let Some(p) = &self.positions {
            println!("Positions:");
            p.render_table();
            println!();
        }
        if let Some(o) = &self.orders {
            println!("Open orders:");
            o.render_table();
        }
    }
}

pub async fn execute_snapshot_inner(
    ctx: &AppContext,
    sections: PortfolioSections,
) -> Result<PortfolioSnapshot, VulcanError> {
    if !(sections.margin || sections.positions || sections.orders) {
        return Ok(PortfolioSnapshot::default());
    }

    let (traders, _) = get_all_trader_views(ctx).await?;

    let trader_xm = if sections.margin || sections.orders {
        Some(find_cross_margin(&traders)?)
    } else {
        None
    };

    // Fetch conditional TP/SL triggers once when we'll need them for either
    // margin counting or the orders section. This uses v1 trader-state, not RPC.
    let conditional_extras = if let Some(trader) = trader_xm {
        let metadata = ctx.metadata().await?;
        Some(fetch_conditional_triggers(ctx, trader, metadata).await?)
    } else {
        None
    };

    let mut margin = sections
        .margin
        .then(|| project_margin_status(trader_xm.unwrap()));
    if let (Some(m), Some(triggers)) = (margin.as_mut(), conditional_extras.as_ref()) {
        m.num_open_orders += triggers.len();
    }

    let mut orders = sections
        .orders
        .then(|| project_orders(trader_xm.unwrap(), None));
    if let (Some(o), Some(triggers)) = (orders.as_mut(), conditional_extras.as_ref()) {
        o.orders.extend(triggers_to_order_infos(triggers, None));
    }

    let (isolated_subaccounts, totals) = if sections.margin {
        let isos = collect_isolated_subaccounts(&traders);
        let totals = compute_totals(&traders);
        ((!isos.is_empty()).then_some(isos), Some(totals))
    } else {
        (None, None)
    };

    Ok(PortfolioSnapshot {
        margin,
        orders,
        positions: sections.positions.then(|| project_positions(&traders)),
        isolated_subaccounts,
        totals,
    })
}

fn collect_isolated_subaccounts(
    traders: &[phoenix_rise::types::TraderView],
) -> Vec<IsolatedSubaccountSummary> {
    let mut out = Vec::new();
    for t in traders {
        if t.trader_subaccount_index == 0 {
            continue;
        }
        let symbol = t.positions.first().map(|p| p.symbol.clone());
        out.push(IsolatedSubaccountSummary {
            subaccount_index: t.trader_subaccount_index,
            symbol,
            collateral_balance: t.collateral_balance.ui.clone(),
            effective_collateral: t.effective_collateral.ui.clone(),
            portfolio_value: t.portfolio_value.ui.clone(),
            unrealized_pnl: t.unrealized_pnl.ui.clone(),
            initial_margin: t.initial_margin.ui.clone(),
            maintenance_margin: t.maintenance_margin.ui.clone(),
        });
    }
    out
}

fn compute_totals(traders: &[phoenix_rise::types::TraderView]) -> AccountTotals {
    let parse = |s: &str| -> f64 { s.parse::<f64>().unwrap_or(0.0) };
    let mut total_collateral = 0.0_f64;
    let mut total_account_value = 0.0_f64;
    let mut total_unrealized_pnl = 0.0_f64;
    for t in traders {
        total_collateral += parse(&t.collateral_balance.ui);
        total_account_value += parse(&t.portfolio_value.ui);
        total_unrealized_pnl += parse(&t.unrealized_pnl.ui);
    }
    AccountTotals {
        total_collateral: format!("{:.6}", total_collateral),
        total_account_value: format!("{:.6}", total_account_value),
        total_unrealized_pnl: format!("{:.6}", total_unrealized_pnl),
        num_subaccounts: traders.len(),
    }
}

fn find_cross_margin(
    traders: &[phoenix_rise::types::TraderView],
) -> Result<&phoenix_rise::types::TraderView, VulcanError> {
    traders
        .iter()
        .find(|t| t.trader_subaccount_index == 0)
        .ok_or_else(|| {
            VulcanError::api(
                "NO_TRADER_ACCOUNT",
                "No registered trader account found. Use 'vulcan account register' first.",
            )
        })
}

pub async fn execute(ctx: &AppContext, args: PortfolioArgs) -> Result<(), VulcanError> {
    let sections = PortfolioSections::from_include(&args.include)?;
    let snapshot = execute_snapshot_inner(ctx, sections).await?;
    render_success(
        ctx.output_format,
        &snapshot,
        serde_json::json!({ "command": "portfolio" }),
    );
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn empty_include_returns_all_sections() {
        let s = PortfolioSections::from_include(&[]).unwrap();
        assert!(s.margin && s.positions && s.orders);
    }

    #[test]
    fn explicit_subset_only_enables_named_sections() {
        let s = PortfolioSections::from_include(&["margin".into(), "orders".into()]).unwrap();
        assert!(s.margin);
        assert!(!s.positions);
        assert!(s.orders);
    }

    #[test]
    fn unknown_section_is_validation_error() {
        let err = PortfolioSections::from_include(&["bogus".into()]).unwrap_err();
        assert_eq!(err.code, "INVALID_INCLUDE");
    }

    #[test]
    fn snapshot_serializes_only_present_sections() {
        let snapshot = PortfolioSnapshot::default();
        let json = serde_json::to_value(&snapshot).unwrap();
        assert_eq!(json, serde_json::json!({}));
    }
}

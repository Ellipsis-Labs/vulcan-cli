//! Shared safety policy checks for live strategy runners.

use crate::commands::{margin, market};
use crate::context::AppContext;
use crate::error::VulcanError;
use crate::strategy::types::{StrategyAccountHealth, StrategyMarketSnapshot};
use serde::{Deserialize, Serialize};

const DEFAULT_MAX_TOTAL_NOTIONAL_USDC: f64 = 1_000.0;
const DEFAULT_MAX_STEP_NOTIONAL_USDC: f64 = 100.0;
const DEFAULT_MAX_PRICE_DRIFT_BPS: f64 = 100.0;
const DEFAULT_MAX_EXPOSURE_RATIO: f64 = 3.0;
const DEFAULT_RECONCILE_ATTEMPTS: u32 = 3;
const DEFAULT_RECONCILE_DELAY_MS: u64 = 750;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct StrategySafetyPolicy {
    pub max_total_notional_usdc: f64,
    pub max_step_notional_usdc: f64,
    pub max_price_drift_bps: f64,
    pub max_exposure_ratio: f64,
    pub reconcile_attempts: u32,
    pub reconcile_delay_ms: u64,
    pub require_authoritative_reconciliation: bool,
    pub stop_on_error: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct StrategyMarginFeasibility {
    pub symbol: String,
    pub planned_notional_usdc: f64,
    pub planned_base_lots: Option<u64>,
    pub max_leverage: f64,
    pub required_initial_margin_usdc: f64,
    pub available_margin_usdc: f64,
    pub max_safe_notional_usdc: f64,
    pub note: String,
}

impl Default for StrategySafetyPolicy {
    fn default() -> Self {
        Self {
            max_total_notional_usdc: DEFAULT_MAX_TOTAL_NOTIONAL_USDC,
            max_step_notional_usdc: DEFAULT_MAX_STEP_NOTIONAL_USDC,
            max_price_drift_bps: DEFAULT_MAX_PRICE_DRIFT_BPS,
            max_exposure_ratio: DEFAULT_MAX_EXPOSURE_RATIO,
            reconcile_attempts: DEFAULT_RECONCILE_ATTEMPTS,
            reconcile_delay_ms: DEFAULT_RECONCILE_DELAY_MS,
            require_authoritative_reconciliation: true,
            stop_on_error: true,
        }
    }
}

impl StrategySafetyPolicy {
    pub fn with_overrides(
        max_total_notional_usdc: Option<f64>,
        max_step_notional_usdc: Option<f64>,
        max_price_drift_bps: Option<f64>,
        max_exposure_ratio: Option<f64>,
        reconcile_attempts: Option<u32>,
        reconcile_delay_ms: Option<u64>,
    ) -> Result<Self, VulcanError> {
        let mut policy = Self::default();
        if let Some(value) = max_total_notional_usdc {
            policy.max_total_notional_usdc = positive("max_total_notional_usdc", value)?;
        }
        if let Some(value) = max_step_notional_usdc {
            policy.max_step_notional_usdc = positive("max_step_notional_usdc", value)?;
        }
        if let Some(value) = max_price_drift_bps {
            policy.max_price_drift_bps = positive("max_price_drift_bps", value)?;
        }
        if let Some(value) = max_exposure_ratio {
            policy.max_exposure_ratio = positive("max_exposure_ratio", value)?;
        }
        if let Some(value) = reconcile_attempts {
            if value == 0 {
                return Err(VulcanError::validation(
                    "INVALID_SAFETY_POLICY",
                    "reconcile_attempts must be greater than zero",
                ));
            }
            policy.reconcile_attempts = value;
        }
        if let Some(value) = reconcile_delay_ms {
            policy.reconcile_delay_ms = value;
        }
        Ok(policy)
    }

    pub fn validate_plan(
        &self,
        total_notional_usdc: Option<f64>,
        step_notional_usdc: Option<f64>,
    ) -> Result<(), VulcanError> {
        if let Some(total) = total_notional_usdc {
            if total > self.max_total_notional_usdc {
                return Err(VulcanError::validation(
                    "STRATEGY_SAFETY_LIMIT",
                    format!(
                        "total notional {:.4} exceeds max_total_notional_usdc {:.4}",
                        total, self.max_total_notional_usdc
                    ),
                ));
            }
        }
        if let Some(step) = step_notional_usdc {
            if step > self.max_step_notional_usdc {
                return Err(VulcanError::validation(
                    "STRATEGY_SAFETY_LIMIT",
                    format!(
                        "step notional {:.4} exceeds max_step_notional_usdc {:.4}",
                        step, self.max_step_notional_usdc
                    ),
                ));
            }
        }
        Ok(())
    }

    pub fn validate_market(
        &self,
        initial_mark: f64,
        market: &StrategyMarketSnapshot,
    ) -> Result<(), VulcanError> {
        if initial_mark <= 0.0 {
            return Ok(());
        }
        let drift_bps = ((market.mark_price - initial_mark).abs() / initial_mark) * 10_000.0;
        if drift_bps > self.max_price_drift_bps {
            return Err(VulcanError::validation(
                "STRATEGY_PRICE_DRIFT",
                format!(
                    "price drift {:.2} bps exceeds max_price_drift_bps {:.2}",
                    drift_bps, self.max_price_drift_bps
                ),
            ));
        }
        Ok(())
    }

    /// Rolling per-tick drift check intended for strategies (e.g. TA) where the
    /// price is expected to move over the run. Compares the current mark to the
    /// previous tick's mark rather than a fixed launch-time anchor, so trending
    /// markets do not exhaust the drift budget. Still catches single-tick gaps
    /// and feed glitches.
    pub fn validate_market_rolling(
        &self,
        previous_mark: f64,
        market: &StrategyMarketSnapshot,
    ) -> Result<(), VulcanError> {
        if previous_mark <= 0.0 {
            return Ok(());
        }
        let drift_bps = ((market.mark_price - previous_mark).abs() / previous_mark) * 10_000.0;
        if drift_bps > self.max_price_drift_bps {
            return Err(VulcanError::validation(
                "STRATEGY_PRICE_DRIFT",
                format!(
                    "single-tick price drift {:.2} bps exceeds max_price_drift_bps {:.2}",
                    drift_bps, self.max_price_drift_bps
                ),
            ));
        }
        Ok(())
    }

    pub fn validate_account(&self, health: &StrategyAccountHealth) -> Result<(), VulcanError> {
        if let Some(risk_state) = &health.risk_state {
            let risk_lower = risk_state.to_ascii_lowercase();
            if !(risk_lower.contains("healthy") || risk_lower == "paper") {
                return Err(VulcanError::validation(
                    "STRATEGY_RISK_STATE",
                    format!("risk state is not healthy: {}", risk_state),
                ));
            }
        }
        Ok(())
    }

    pub fn validate_exposure(&self, exposure_ratio: f64) -> Result<(), VulcanError> {
        if exposure_ratio > self.max_exposure_ratio {
            return Err(VulcanError::validation(
                "STRATEGY_EXPOSURE_LIMIT",
                format!(
                    "exposure ratio {:.4} exceeds max_exposure_ratio {:.4}",
                    exposure_ratio, self.max_exposure_ratio
                ),
            ));
        }
        Ok(())
    }
}

pub async fn validate_live_margin_feasibility(
    ctx: &AppContext,
    symbol: &str,
    planned_notional_usdc: f64,
    planned_base_lots: Option<u64>,
    isolated_collateral: Option<f64>,
) -> Result<StrategyMarginFeasibility, VulcanError> {
    if ctx.dry_run {
        return Ok(StrategyMarginFeasibility::dry_run(
            symbol,
            planned_notional_usdc,
            planned_base_lots,
        ));
    }

    let info = market::execute_info_inner(ctx, symbol).await?;
    let max_leverage = max_leverage_for_size(&info.leverage_tiers, planned_base_lots);
    let required_initial_margin_usdc = planned_notional_usdc / max_leverage.max(1.0);
    let available_margin_usdc = if let Some(collateral) = isolated_collateral {
        collateral
    } else {
        let margin = margin::execute_status_inner(ctx).await?;
        parse_f64(&margin.available_to_withdraw)
            .or_else(|| {
                parse_f64(&margin.effective_collateral)
                    .zip(parse_f64(&margin.initial_margin))
                    .map(|(effective, initial)| (effective - initial).max(0.0))
            })
            .unwrap_or(0.0)
    };
    let max_safe_notional_usdc = available_margin_usdc * max_leverage.max(1.0);
    if required_initial_margin_usdc > available_margin_usdc {
        return Err(VulcanError::validation(
            "STRATEGY_MARGIN_FEASIBILITY",
            format!(
                "{} strategy requires at least {:.4} USDC initial margin for {:.4} USDC notional at {:.2}x max leverage; available margin is {:.4} USDC. Estimated max safe notional is {:.4} USDC.",
                symbol,
                required_initial_margin_usdc,
                planned_notional_usdc,
                max_leverage,
                available_margin_usdc,
                max_safe_notional_usdc
            ),
        ));
    }

    Ok(StrategyMarginFeasibility {
        symbol: symbol.to_string(),
        planned_notional_usdc,
        planned_base_lots,
        max_leverage,
        required_initial_margin_usdc,
        available_margin_usdc,
        max_safe_notional_usdc,
        note: "live strategy margin feasibility passed at launch".to_string(),
    })
}

impl StrategyMarginFeasibility {
    fn dry_run(symbol: &str, planned_notional_usdc: f64, planned_base_lots: Option<u64>) -> Self {
        Self {
            symbol: symbol.to_string(),
            planned_notional_usdc,
            planned_base_lots,
            max_leverage: 1.0,
            required_initial_margin_usdc: 0.0,
            available_margin_usdc: 0.0,
            max_safe_notional_usdc: 0.0,
            note: "dry run: live margin feasibility was not evaluated".to_string(),
        }
    }
}

fn max_leverage_for_size(
    tiers: &[market::LeverageTierInfo],
    planned_base_lots: Option<u64>,
) -> f64 {
    let Some(planned_base_lots) = planned_base_lots else {
        return tiers.first().map(|tier| tier.max_leverage).unwrap_or(1.0);
    };
    tiers
        .iter()
        .find(|tier| planned_base_lots <= tier.max_size_base_lots)
        .or_else(|| tiers.last())
        .map(|tier| tier.max_leverage)
        .unwrap_or(1.0)
}

fn parse_f64(value: &str) -> Option<f64> {
    let cleaned = value
        .trim()
        .trim_start_matches('$')
        .trim_end_matches(" USDC")
        .replace(',', "");
    cleaned.parse::<f64>().ok()
}

fn positive(name: &str, value: f64) -> Result<f64, VulcanError> {
    if value <= 0.0 {
        return Err(VulcanError::validation(
            "INVALID_SAFETY_POLICY",
            format!("{} must be positive", name),
        ));
    }
    Ok(value)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn plan_limit_reports_large_auto_execute() {
        let policy = StrategySafetyPolicy::default();
        let err = policy.validate_plan(Some(2_000.0), Some(10.0)).unwrap_err();
        assert_eq!(err.code, "STRATEGY_SAFETY_LIMIT");
    }

    fn snap(mark: f64) -> StrategyMarketSnapshot {
        StrategyMarketSnapshot {
            mark_price: mark,
            mid_price: mark,
            funding_rate: 0.0,
        }
    }

    #[test]
    fn rolling_drift_passes_for_trend_under_threshold() {
        // 100 bps default; previous tick at 100, current at 100.5 → 50 bps drift.
        let policy = StrategySafetyPolicy::default();
        policy.validate_market_rolling(100.0, &snap(100.5)).unwrap();
    }

    #[test]
    fn rolling_drift_trips_on_single_tick_gap() {
        // previous 100 → current 105 → 500 bps single-tick gap.
        let policy = StrategySafetyPolicy::default();
        let err = policy
            .validate_market_rolling(100.0, &snap(105.0))
            .unwrap_err();
        assert_eq!(err.code, "STRATEGY_PRICE_DRIFT");
        assert!(err.message.contains("single-tick"));
    }

    #[test]
    fn rolling_drift_skipped_for_non_positive_baseline() {
        // First-tick condition (no baseline yet) is modeled by previous_mark <= 0.
        let policy = StrategySafetyPolicy::default();
        policy.validate_market_rolling(0.0, &snap(100.0)).unwrap();
        policy.validate_market_rolling(-1.0, &snap(100.0)).unwrap();
    }

    #[test]
    fn rolling_drift_does_not_compound_over_run() {
        // Two consecutive 50-bps moves should both pass: under the run-anchored
        // semantics, the second tick would have shown 100 bps total drift and
        // the third tick (150 bps cumulative) would have tripped. Rolling
        // baseline only sees 50 bps per step.
        let policy = StrategySafetyPolicy::default();
        policy.validate_market_rolling(100.0, &snap(100.5)).unwrap();
        policy.validate_market_rolling(100.5, &snap(101.0)).unwrap();
        policy.validate_market_rolling(101.0, &snap(101.5)).unwrap();
    }

    #[test]
    fn leverage_tier_matches_planned_base_lots() {
        let tiers = vec![
            market::LeverageTierInfo {
                max_leverage: 15.0,
                max_size_base_lots: 100,
            },
            market::LeverageTierInfo {
                max_leverage: 10.0,
                max_size_base_lots: 1_000,
            },
        ];
        assert_eq!(max_leverage_for_size(&tiers, Some(50)), 15.0);
        assert_eq!(max_leverage_for_size(&tiers, Some(500)), 10.0);
        assert_eq!(max_leverage_for_size(&tiers, Some(5_000)), 10.0);
    }
}

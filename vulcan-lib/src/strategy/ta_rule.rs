//! Declarative TA-strategy rule model.
//!
//! Each rule has a `Condition` (boolean expression over indicators / price / constants)
//! and an `Action` (open / close / reduce). The condition is evaluated each tick against
//! freshly computed indicators; if it fires and the rule's cooldown is clear, the action
//! is dispatched.

use crate::error::VulcanError;
use crate::indicators::{IndicatorKind, IndicatorPoint, IndicatorSeries};
use crate::strategy::types::{StrategyMarginMode, StrategyPositionSnapshot, StrategySide};
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;

// ── Public types ───────────────────────────────────────────────────────────

/// A reference to a single indicator series. Two refs with the same kind, timeframe,
/// period, params, and key produce the same series — the runner uses this to dedupe
/// indicator fetches across rules per tick.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct IndicatorRef {
    pub kind: IndicatorKind,
    pub timeframe: String,
    #[serde(default)]
    pub period: Option<usize>,
    /// Indicator-specific parameters (e.g. MACD `fast`/`slow`/`signal`).
    #[serde(default)]
    pub params: BTreeMap<String, f64>,
    /// Optional series key override (default = `kind.primary_key()`).
    #[serde(default)]
    pub key: Option<String>,
}

impl IndicatorRef {
    /// Stable string key for caching identical references across rules.
    pub fn cache_key(&self) -> String {
        let mut params: Vec<(String, f64)> =
            self.params.iter().map(|(k, v)| (k.clone(), *v)).collect();
        params.sort_by(|a, b| a.0.cmp(&b.0));
        let params_str: Vec<String> = params.iter().map(|(k, v)| format!("{k}={v}")).collect();
        format!(
            "{:?}|{}|p={}|key={}|{}",
            self.kind,
            self.timeframe,
            self.period
                .map(|p| p.to_string())
                .unwrap_or_else(|| "default".to_string()),
            self.key.clone().unwrap_or_else(|| "primary".to_string()),
            params_str.join(",")
        )
    }

    pub fn output_key(&self) -> String {
        self.key
            .clone()
            .unwrap_or_else(|| self.kind.primary_key().to_string())
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum PriceField {
    /// Current close (latest candle close on the rule's primary timeframe).
    Close,
    /// Current mark price from the live snapshot.
    Mark,
    /// Current mid price from the live snapshot.
    Mid,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ValueSource {
    Constant(f64),
    Indicator(IndicatorRef),
    Price(PriceField),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum CompareOp {
    Lt,
    Lte,
    Gt,
    Gte,
    Eq,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum CrossDirection {
    Above,
    Below,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Condition {
    All(Vec<Condition>),
    Any(Vec<Condition>),
    Not(Box<Condition>),
    Compare {
        left: ValueSource,
        op: CompareOp,
        right: ValueSource,
    },
    Crosses {
        left: ValueSource,
        right: ValueSource,
        direction: CrossDirection,
    },
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SizeSpec {
    Tokens(f64),
    Lots(u64),
    Notional(f64),
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "snake_case", tag = "kind")]
pub enum Action {
    /// Open or add to a position with a fresh market order.
    Open { side: StrategySide, size: SizeSpec },
    /// Close the current position entirely with an opposite-side market order.
    Close,
    /// Reduce the current position by `fraction` (0.0 < fraction <= 1.0).
    Reduce { fraction: f64 },
    /// Record-only firing — useful for debugging or "watch" rules.
    NoOp,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "snake_case")]
pub enum PositionFilter {
    Long,
    Short,
    Flat,
    NotFlat,
    #[default]
    Any,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
#[serde(rename_all = "snake_case")]
pub enum CooldownPolicy {
    /// No cooldown — fires whenever the condition is true. Use with care.
    None,
    /// Default: once fired, the rule is locked out until the condition evaluates `false`
    /// for at least one tick, then re-armed.
    #[default]
    UntilConditionResets,
    /// Fires at most once for the lifetime of the run.
    OncePerRun,
    /// Wait at least N ticks after firing.
    MinTicks { ticks: u32 },
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Rule {
    pub name: String,
    pub when: Condition,
    #[serde(rename = "do")]
    pub action: Action,
    #[serde(default)]
    pub cooldown: CooldownPolicy,
    /// Pre-filter on position state — skip this rule if it doesn't match.
    #[serde(default)]
    pub only_if_position: PositionFilter,
}

/// How the runner schedules the next tick.
///
/// `Fixed` sleeps a constant number of seconds between ticks (current default).
/// `CandleClose` aligns wake-ups to candle boundaries on the given timeframe —
/// the runner sleeps until `next_candle_close + grace_seconds`. This makes
/// strategies behave the same in backtests and live, and avoids polling
/// candles before the bar has closed.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "snake_case", tag = "kind")]
pub enum Cadence {
    Fixed {
        interval_seconds: u64,
    },
    CandleClose {
        timeframe: String,
        #[serde(default)]
        grace_seconds: Option<u64>,
    },
}

impl Default for Cadence {
    fn default() -> Self {
        Self::Fixed {
            interval_seconds: 60,
        }
    }
}

impl Cadence {
    /// Compute the seconds to sleep before the next tick, given the current
    /// unix epoch seconds. For `CandleClose`, returns the delay to the next
    /// bar boundary plus the configured grace period.
    pub fn delay_seconds(&self, now_unix_seconds: u64) -> u64 {
        match self {
            Cadence::Fixed { interval_seconds } => *interval_seconds,
            Cadence::CandleClose {
                timeframe,
                grace_seconds,
            } => {
                let tf = timeframe_seconds(timeframe).unwrap_or(60);
                let grace = grace_seconds.unwrap_or(5);
                let remainder = now_unix_seconds % tf;
                let to_next = if remainder == 0 { tf } else { tf - remainder };
                to_next + grace
            }
        }
    }

    /// Representative "tick interval" used for the ledger's persisted
    /// `interval_seconds` field (informational; the runner always computes
    /// real delays via `delay_seconds`).
    pub fn nominal_interval_seconds(&self) -> u64 {
        match self {
            Cadence::Fixed { interval_seconds } => *interval_seconds,
            Cadence::CandleClose { timeframe, .. } => timeframe_seconds(timeframe).unwrap_or(60),
        }
    }

    pub fn describe(&self) -> String {
        match self {
            Cadence::Fixed { interval_seconds } => {
                format!("fixed {}s", interval_seconds)
            }
            Cadence::CandleClose {
                timeframe,
                grace_seconds,
            } => format!(
                "candle close {} (+{}s grace)",
                timeframe,
                grace_seconds.unwrap_or(5)
            ),
        }
    }
}

fn timeframe_seconds(tf: &str) -> Option<u64> {
    match tf {
        "1m" => Some(60),
        "5m" => Some(300),
        "15m" => Some(900),
        "1h" => Some(3600),
        "4h" => Some(14_400),
        "1d" => Some(86_400),
        _ => None,
    }
}

/// Configuration for a `vulcan strategy ta` run.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TaStrategyConfig {
    pub symbol: String,
    /// Back-compat fixed cadence in seconds (used when `cadence` is omitted).
    #[serde(default = "default_interval_seconds")]
    pub interval_seconds: u64,
    /// Optional richer cadence spec. When present, overrides `interval_seconds`.
    #[serde(default)]
    pub cadence: Option<Cadence>,
    #[serde(default)]
    pub margin_mode: StrategyMarginMode,
    #[serde(default)]
    pub isolated_collateral: Option<f64>,
    /// Hard cap: never let an Open push absolute position size past this many tokens.
    #[serde(default)]
    pub max_concurrent_position_tokens: Option<f64>,
    /// Stop the runner after N firings of any rule (defense against runaway logic). 0 = unlimited.
    #[serde(default)]
    pub max_firings: Option<u32>,
    pub rules: Vec<Rule>,
}

impl TaStrategyConfig {
    pub fn resolved_cadence(&self) -> Cadence {
        self.cadence.clone().unwrap_or(Cadence::Fixed {
            interval_seconds: self.interval_seconds,
        })
    }
}

fn default_interval_seconds() -> u64 {
    60
}

// ── Evaluation ─────────────────────────────────────────────────────────────

/// Snapshot fed to `evaluate_condition` each tick.
#[derive(Debug)]
pub struct EvaluationContext<'a> {
    /// Cache keyed by `IndicatorRef::cache_key()`.
    pub indicators: &'a BTreeMap<String, IndicatorSeries>,
    pub mark_price: f64,
    pub previous_mark_price: Option<f64>,
    pub mid_price: f64,
    pub previous_mid_price: Option<f64>,
    /// Latest candle close on the *first* timeframe seen in the rule set; used by
    /// `ValueSource::Price(Close)`. Optional; if missing, Price(Close) yields NaN.
    pub close_price: Option<f64>,
    pub previous_close_price: Option<f64>,
    pub position: &'a StrategyPositionSnapshot,
}

#[derive(Debug, Clone, Serialize)]
pub struct RuleFiring {
    pub rule_name: String,
    pub fired: bool,
    pub reason: Option<String>,
    pub skipped_by_filter: Option<String>,
    pub skipped_by_cooldown: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct CooldownState {
    /// Per-rule state, keyed by rule name.
    #[serde(default)]
    pub rules: BTreeMap<String, RuleCooldownState>,
    #[serde(default)]
    pub total_firings: u32,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct RuleCooldownState {
    /// True if the rule fired on a prior tick and is currently armed-but-locked.
    pub locked: bool,
    /// Last tick number on which the rule fired.
    pub last_fired_tick: Option<u32>,
    /// Total firings of this rule.
    pub firings: u32,
    /// True if the condition evaluated `true` on the most recent tick (for `UntilConditionResets`).
    pub condition_held_last_tick: bool,
}

impl CooldownState {
    pub fn rule_state(&self, name: &str) -> RuleCooldownState {
        self.rules.get(name).cloned().unwrap_or_default()
    }

    pub fn entry_mut(&mut self, name: &str) -> &mut RuleCooldownState {
        self.rules.entry(name.to_string()).or_default()
    }

    /// Test-only alias kept for backwards-compat with the original signature used in tests.
    #[cfg(test)]
    fn entry(&mut self, name: &str) -> &mut RuleCooldownState {
        self.entry_mut(name)
    }
}

/// Outcome of evaluating one rule on one tick.
pub struct RuleEvalOutcome {
    pub fired: bool,
    /// True if the rule's *condition* alone evaluated to true (ignoring filters/cooldowns).
    pub condition_true: bool,
    pub skipped_reason: Option<String>,
}

impl Rule {
    pub fn evaluate(
        &self,
        eval: &EvaluationContext<'_>,
        state: &CooldownState,
        tick_number: u32,
    ) -> Result<RuleEvalOutcome, VulcanError> {
        // Cooldown / position filter checks happen first so we don't waste cycles on
        // condition evaluation when we'd skip anyway. Condition evaluation is still
        // computed (cheaply, from the cache) so callers can record whether the
        // condition holds for cooldown bookkeeping next tick.
        let filter_skip = match (self.only_if_position, position_state(eval.position)) {
            (PositionFilter::Any, _) => None,
            (PositionFilter::Long, PositionState::Long) => None,
            (PositionFilter::Short, PositionState::Short) => None,
            (PositionFilter::Flat, PositionState::Flat) => None,
            (PositionFilter::NotFlat, PositionState::Long | PositionState::Short) => None,
            (filter, current) => Some(format!(
                "rule {:?} requires {:?} position, current state is {:?}",
                self.name, filter, current
            )),
        };

        let condition_true = evaluate_condition(&self.when, eval)?;

        let rule_state = state.rule_state(&self.name);
        let cooldown_skip = match &self.cooldown {
            CooldownPolicy::None => None,
            CooldownPolicy::UntilConditionResets => {
                if rule_state.locked {
                    Some(format!(
                        "cooldown UntilConditionResets: waiting for `{}` condition to evaluate false",
                        self.name
                    ))
                } else {
                    None
                }
            }
            CooldownPolicy::OncePerRun => {
                if rule_state.firings > 0 {
                    Some("cooldown OncePerRun: already fired this run".to_string())
                } else {
                    None
                }
            }
            CooldownPolicy::MinTicks { ticks } => match rule_state.last_fired_tick {
                Some(last) if tick_number.saturating_sub(last) < *ticks => Some(format!(
                    "cooldown MinTicks: {} of {} ticks elapsed since last firing",
                    tick_number.saturating_sub(last),
                    ticks
                )),
                _ => None,
            },
        };

        let (fired, skipped_reason) = if filter_skip.is_some() {
            (false, filter_skip)
        } else if cooldown_skip.is_some() {
            (false, cooldown_skip)
        } else if condition_true {
            (true, None)
        } else {
            (false, Some("condition evaluated false".to_string()))
        };

        Ok(RuleEvalOutcome {
            fired,
            condition_true,
            skipped_reason,
        })
    }
}

pub fn evaluate_condition(
    condition: &Condition,
    eval: &EvaluationContext<'_>,
) -> Result<bool, VulcanError> {
    match condition {
        Condition::All(children) => {
            for child in children {
                if !evaluate_condition(child, eval)? {
                    return Ok(false);
                }
            }
            Ok(true)
        }
        Condition::Any(children) => {
            for child in children {
                if evaluate_condition(child, eval)? {
                    return Ok(true);
                }
            }
            Ok(false)
        }
        Condition::Not(child) => Ok(!evaluate_condition(child, eval)?),
        Condition::Compare { left, op, right } => {
            let l = resolve_latest(left, eval)?;
            let r = resolve_latest(right, eval)?;
            let (Some(l), Some(r)) = (l, r) else {
                return Ok(false); // missing data => condition false
            };
            Ok(match op {
                CompareOp::Lt => l < r,
                CompareOp::Lte => l <= r,
                CompareOp::Gt => l > r,
                CompareOp::Gte => l >= r,
                CompareOp::Eq => (l - r).abs() < f64::EPSILON,
            })
        }
        Condition::Crosses {
            left,
            right,
            direction,
        } => {
            let (l_curr, l_prev) = resolve_pair(left, eval)?;
            let (r_curr, r_prev) = resolve_pair(right, eval)?;
            let (Some(l_curr), Some(l_prev), Some(r_curr), Some(r_prev)) =
                (l_curr, l_prev, r_curr, r_prev)
            else {
                return Ok(false);
            };
            let prev_diff = l_prev - r_prev;
            let curr_diff = l_curr - r_curr;
            Ok(match direction {
                CrossDirection::Above => prev_diff <= 0.0 && curr_diff > 0.0,
                CrossDirection::Below => prev_diff >= 0.0 && curr_diff < 0.0,
            })
        }
    }
}

/// Walk a rule set and collect the distinct `IndicatorRef`s. Used by the runner
/// to compute each indicator at most once per tick.
pub fn gather_indicator_refs(rules: &[Rule]) -> Vec<IndicatorRef> {
    let mut seen = BTreeMap::new();
    for rule in rules {
        walk_condition_indicators(&rule.when, &mut seen);
    }
    seen.into_values().collect()
}

fn walk_condition_indicators(c: &Condition, out: &mut BTreeMap<String, IndicatorRef>) {
    match c {
        Condition::All(children) | Condition::Any(children) => {
            for child in children {
                walk_condition_indicators(child, out);
            }
        }
        Condition::Not(child) => walk_condition_indicators(child, out),
        Condition::Compare { left, right, .. } | Condition::Crosses { left, right, .. } => {
            collect_indicator(left, out);
            collect_indicator(right, out);
        }
    }
}

fn collect_indicator(src: &ValueSource, out: &mut BTreeMap<String, IndicatorRef>) {
    if let ValueSource::Indicator(r) = src {
        out.entry(r.cache_key()).or_insert_with(|| r.clone());
    }
}

// ── Internal helpers ───────────────────────────────────────────────────────

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum PositionState {
    Long,
    Short,
    Flat,
}

fn position_state(p: &StrategyPositionSnapshot) -> PositionState {
    if p.size_tokens.abs() < f64::EPSILON {
        return PositionState::Flat;
    }
    match p.side.as_deref() {
        Some(s) if s.eq_ignore_ascii_case("long") || s.eq_ignore_ascii_case("buy") => {
            PositionState::Long
        }
        Some(s) if s.eq_ignore_ascii_case("short") || s.eq_ignore_ascii_case("sell") => {
            PositionState::Short
        }
        _ => PositionState::Flat,
    }
}

/// Latest non-NaN value of a `ValueSource`. Returns `None` if the indicator series
/// hasn't produced a value yet (warmup) or the cache is empty.
fn resolve_latest(
    src: &ValueSource,
    eval: &EvaluationContext<'_>,
) -> Result<Option<f64>, VulcanError> {
    match src {
        ValueSource::Constant(v) => Ok(Some(*v)),
        ValueSource::Price(PriceField::Mark) => Ok(Some(eval.mark_price)),
        ValueSource::Price(PriceField::Mid) => Ok(Some(eval.mid_price)),
        ValueSource::Price(PriceField::Close) => Ok(eval.close_price),
        ValueSource::Indicator(r) => Ok(indicator_latest_two(eval, r)?.map(|(curr, _)| curr)),
    }
}

/// (latest, previous) pair of non-NaN values. For `Crosses` semantics — needs two.
fn resolve_pair(
    src: &ValueSource,
    eval: &EvaluationContext<'_>,
) -> Result<(Option<f64>, Option<f64>), VulcanError> {
    match src {
        ValueSource::Constant(v) => Ok((Some(*v), Some(*v))),
        ValueSource::Price(PriceField::Mark) => {
            Ok((Some(eval.mark_price), eval.previous_mark_price))
        }
        ValueSource::Price(PriceField::Mid) => Ok((Some(eval.mid_price), eval.previous_mid_price)),
        ValueSource::Price(PriceField::Close) => Ok((eval.close_price, eval.previous_close_price)),
        ValueSource::Indicator(r) => {
            let pair = indicator_latest_two(eval, r)?;
            Ok(pair.map_or((None, None), |(c, p)| (Some(c), Some(p))))
        }
    }
}

/// Pull (latest, previous) non-NaN values for the indicator's series key from the
/// cache. Returns Ok(None) if either is missing (warmup or short series).
fn indicator_latest_two(
    eval: &EvaluationContext<'_>,
    r: &IndicatorRef,
) -> Result<Option<(f64, f64)>, VulcanError> {
    let series = eval.indicators.get(&r.cache_key()).ok_or_else(|| {
        VulcanError::internal(
            "TA_RULE_INDICATOR_NOT_CACHED",
            format!(
                "indicator {} not present in evaluation cache; gather_indicator_refs missed it",
                r.cache_key()
            ),
        )
    })?;
    let key = r.output_key();
    let valued: Vec<f64> = series
        .points
        .iter()
        .filter_map(|p: &IndicatorPoint| p.values.get(&key).copied().filter(|v| !v.is_nan()))
        .collect();
    if valued.len() < 2 {
        if let Some(&v) = valued.last() {
            // We have only one value — that's "latest" but no "previous".
            // For Compare callers, this is fine via resolve_latest. For Crosses,
            // we report missing previous.
            return Ok(Some((v, f64::NAN)));
        }
        return Ok(None);
    }
    let curr = valued[valued.len() - 1];
    let prev = valued[valued.len() - 2];
    Ok(Some((curr, prev)))
}

// ── Tests ──────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;
    use crate::indicators::{IndicatorPoint, IndicatorSummary};

    fn synth_series(kind: IndicatorKind, key: &str, values: &[f64]) -> IndicatorSeries {
        let points: Vec<IndicatorPoint> = values
            .iter()
            .enumerate()
            .map(|(i, &v)| {
                let mut m = BTreeMap::new();
                m.insert(key.to_string(), v);
                IndicatorPoint {
                    time: format!("t{}", i),
                    values: m,
                }
            })
            .collect();
        IndicatorSeries {
            kind,
            symbol: "TEST".into(),
            timeframe: "1h".into(),
            period: 14,
            points,
            summary: IndicatorSummary {
                primary_key: key.to_string(),
                latest: values.last().copied(),
                min: None,
                max: None,
                mean: None,
                verdict: String::new(),
            },
        }
    }

    fn cache(refs: &[(IndicatorRef, IndicatorSeries)]) -> BTreeMap<String, IndicatorSeries> {
        let mut m = BTreeMap::new();
        for (r, s) in refs {
            m.insert(r.cache_key(), s.clone());
        }
        m
    }

    fn rsi_ref() -> IndicatorRef {
        IndicatorRef {
            kind: IndicatorKind::Rsi,
            timeframe: "1h".into(),
            period: Some(14),
            params: BTreeMap::new(),
            key: None,
        }
    }

    fn ema_ref(period: usize) -> IndicatorRef {
        IndicatorRef {
            kind: IndicatorKind::Ema,
            timeframe: "1h".into(),
            period: Some(period),
            params: BTreeMap::new(),
            key: None,
        }
    }

    fn empty_position() -> StrategyPositionSnapshot {
        StrategyPositionSnapshot::default()
    }

    fn eval_ctx<'a>(
        indicators: &'a BTreeMap<String, IndicatorSeries>,
        position: &'a StrategyPositionSnapshot,
    ) -> EvaluationContext<'a> {
        EvaluationContext {
            indicators,
            mark_price: 100.0,
            previous_mark_price: None,
            mid_price: 100.0,
            previous_mid_price: None,
            close_price: Some(100.0),
            previous_close_price: None,
            position,
        }
    }

    #[test]
    fn compare_indicator_lt_constant() {
        let r = rsi_ref();
        let series = synth_series(IndicatorKind::Rsi, "rsi", &[40.0, 35.0, 28.0]);
        let indicators = cache(&[(r.clone(), series)]);
        let pos = empty_position();
        let eval = eval_ctx(&indicators, &pos);

        let cond = Condition::Compare {
            left: ValueSource::Indicator(r),
            op: CompareOp::Lt,
            right: ValueSource::Constant(30.0),
        };
        assert!(evaluate_condition(&cond, &eval).unwrap());
    }

    #[test]
    fn all_short_circuits_on_false() {
        let r = rsi_ref();
        let series = synth_series(IndicatorKind::Rsi, "rsi", &[50.0]);
        let indicators = cache(&[(r.clone(), series)]);
        let pos = empty_position();
        let eval = eval_ctx(&indicators, &pos);

        let cond = Condition::All(vec![
            Condition::Compare {
                left: ValueSource::Indicator(r.clone()),
                op: CompareOp::Lt,
                right: ValueSource::Constant(30.0), // false
            },
            // Even though this is well-formed, the All should short-circuit.
            Condition::Compare {
                left: ValueSource::Constant(1.0),
                op: CompareOp::Lt,
                right: ValueSource::Constant(2.0),
            },
        ]);
        assert!(!evaluate_condition(&cond, &eval).unwrap());
    }

    #[test]
    fn any_returns_true_if_one_holds() {
        let r = rsi_ref();
        let series = synth_series(IndicatorKind::Rsi, "rsi", &[50.0]);
        let indicators = cache(&[(r.clone(), series)]);
        let pos = empty_position();
        let eval = eval_ctx(&indicators, &pos);

        let cond = Condition::Any(vec![
            Condition::Compare {
                left: ValueSource::Indicator(r),
                op: CompareOp::Lt,
                right: ValueSource::Constant(30.0), // false
            },
            Condition::Compare {
                left: ValueSource::Constant(1.0),
                op: CompareOp::Lt,
                right: ValueSource::Constant(2.0), // true
            },
        ]);
        assert!(evaluate_condition(&cond, &eval).unwrap());
    }

    #[test]
    fn empty_all_is_true_empty_any_is_false() {
        let indicators = BTreeMap::new();
        let pos = empty_position();
        let eval = eval_ctx(&indicators, &pos);
        assert!(evaluate_condition(&Condition::All(vec![]), &eval).unwrap());
        assert!(!evaluate_condition(&Condition::Any(vec![]), &eval).unwrap());
    }

    #[test]
    fn not_flips_truth() {
        let indicators = BTreeMap::new();
        let pos = empty_position();
        let eval = eval_ctx(&indicators, &pos);
        let inner = Condition::Compare {
            left: ValueSource::Constant(1.0),
            op: CompareOp::Lt,
            right: ValueSource::Constant(2.0),
        };
        assert!(!evaluate_condition(&Condition::Not(Box::new(inner)), &eval).unwrap());
    }

    #[test]
    fn crosses_above_fires_on_sign_flip_not_before() {
        let fast = ema_ref(9);
        let slow = ema_ref(21);
        // Fast was below slow at t-1, above slow at t. Crosses above should fire.
        let fast_series = synth_series(IndicatorKind::Ema, "ema", &[99.0, 100.0, 102.0]);
        let slow_series = synth_series(IndicatorKind::Ema, "ema", &[100.0, 101.0, 101.5]);
        let indicators = cache(&[(fast.clone(), fast_series), (slow.clone(), slow_series)]);
        let pos = empty_position();
        let eval = eval_ctx(&indicators, &pos);

        let cond = Condition::Crosses {
            left: ValueSource::Indicator(fast.clone()),
            right: ValueSource::Indicator(slow.clone()),
            direction: CrossDirection::Above,
        };
        assert!(evaluate_condition(&cond, &eval).unwrap());

        // No crossing if prior bar was already above.
        let fast_series = synth_series(IndicatorKind::Ema, "ema", &[101.0, 102.0]);
        let slow_series = synth_series(IndicatorKind::Ema, "ema", &[100.0, 100.5]);
        let indicators = cache(&[(fast.clone(), fast_series), (slow.clone(), slow_series)]);
        let eval = eval_ctx(&indicators, &pos);
        assert!(!evaluate_condition(&cond, &eval).unwrap());
    }

    #[test]
    fn crosses_below_fires_only_on_downward_flip() {
        let fast = ema_ref(9);
        let slow = ema_ref(21);
        // prev bar: fast(101) >= slow(100); curr bar: fast(99) < slow(100) → downward cross.
        let fast_series = synth_series(IndicatorKind::Ema, "ema", &[101.0, 99.0]);
        let slow_series = synth_series(IndicatorKind::Ema, "ema", &[100.0, 100.0]);
        let indicators = cache(&[(fast.clone(), fast_series), (slow.clone(), slow_series)]);
        let pos = empty_position();
        let eval = eval_ctx(&indicators, &pos);

        let cond = Condition::Crosses {
            left: ValueSource::Indicator(fast),
            right: ValueSource::Indicator(slow),
            direction: CrossDirection::Below,
        };
        assert!(evaluate_condition(&cond, &eval).unwrap());
    }

    #[test]
    fn price_crosses_use_previous_price() {
        let slow = ema_ref(21);
        let slow_series = synth_series(IndicatorKind::Ema, "ema", &[100.0, 100.0]);
        let indicators = cache(&[(slow.clone(), slow_series)]);
        let pos = empty_position();
        let eval = EvaluationContext {
            indicators: &indicators,
            mark_price: 101.0,
            previous_mark_price: Some(99.0),
            mid_price: 101.0,
            previous_mid_price: Some(99.0),
            close_price: Some(101.0),
            previous_close_price: Some(99.0),
            position: &pos,
        };

        let cond = Condition::Crosses {
            left: ValueSource::Price(PriceField::Close),
            right: ValueSource::Indicator(slow),
            direction: CrossDirection::Above,
        };
        assert!(evaluate_condition(&cond, &eval).unwrap());
    }

    #[test]
    fn position_filter_long_skips_when_flat() {
        let r = rsi_ref();
        let series = synth_series(IndicatorKind::Rsi, "rsi", &[28.0]);
        let indicators = cache(&[(r.clone(), series)]);
        let pos = empty_position(); // flat
        let eval = eval_ctx(&indicators, &pos);
        let state = CooldownState::default();

        let rule = Rule {
            name: "exit-long".into(),
            when: Condition::Compare {
                left: ValueSource::Indicator(r),
                op: CompareOp::Lt,
                right: ValueSource::Constant(30.0),
            },
            action: Action::Close,
            cooldown: CooldownPolicy::None,
            only_if_position: PositionFilter::Long,
        };
        let outcome = rule.evaluate(&eval, &state, 1).unwrap();
        assert!(!outcome.fired);
        assert!(outcome.condition_true);
        assert!(outcome.skipped_reason.unwrap().contains("Long"));
    }

    #[test]
    fn cooldown_once_per_run_locks_after_first_fire() {
        let r = rsi_ref();
        let series = synth_series(IndicatorKind::Rsi, "rsi", &[28.0]);
        let indicators = cache(&[(r.clone(), series)]);
        let pos = empty_position();
        let eval = eval_ctx(&indicators, &pos);

        let rule = Rule {
            name: "once".into(),
            when: Condition::Compare {
                left: ValueSource::Indicator(r),
                op: CompareOp::Lt,
                right: ValueSource::Constant(30.0),
            },
            action: Action::NoOp,
            cooldown: CooldownPolicy::OncePerRun,
            only_if_position: PositionFilter::Any,
        };

        let mut state = CooldownState::default();
        // First firing:
        let outcome = rule.evaluate(&eval, &state, 1).unwrap();
        assert!(outcome.fired);
        state.entry(&rule.name).firings = 1;
        // Second tick: should be locked even though condition still true.
        let outcome = rule.evaluate(&eval, &state, 2).unwrap();
        assert!(!outcome.fired);
        assert!(outcome.skipped_reason.unwrap().contains("OncePerRun"));
    }

    #[test]
    fn cooldown_min_ticks_releases_after_n() {
        let r = rsi_ref();
        let series = synth_series(IndicatorKind::Rsi, "rsi", &[28.0]);
        let indicators = cache(&[(r.clone(), series)]);
        let pos = empty_position();
        let eval = eval_ctx(&indicators, &pos);

        let rule = Rule {
            name: "throttled".into(),
            when: Condition::Compare {
                left: ValueSource::Indicator(r),
                op: CompareOp::Lt,
                right: ValueSource::Constant(30.0),
            },
            action: Action::NoOp,
            cooldown: CooldownPolicy::MinTicks { ticks: 3 },
            only_if_position: PositionFilter::Any,
        };

        let mut state = CooldownState::default();
        state.entry(&rule.name).last_fired_tick = Some(2);
        // Tick 3: only 1 tick elapsed → still locked.
        assert!(!rule.evaluate(&eval, &state, 3).unwrap().fired);
        // Tick 5: 3 ticks elapsed → unlocked.
        assert!(rule.evaluate(&eval, &state, 5).unwrap().fired);
    }

    #[test]
    fn gather_indicator_refs_dedupes_across_rules() {
        let rsi = rsi_ref();
        let ema9 = ema_ref(9);
        let ema21 = ema_ref(21);
        let rules = vec![
            Rule {
                name: "a".into(),
                when: Condition::All(vec![
                    Condition::Compare {
                        left: ValueSource::Indicator(rsi.clone()),
                        op: CompareOp::Gt,
                        right: ValueSource::Constant(50.0),
                    },
                    Condition::Crosses {
                        left: ValueSource::Indicator(ema9.clone()),
                        right: ValueSource::Indicator(ema21.clone()),
                        direction: CrossDirection::Above,
                    },
                ]),
                action: Action::NoOp,
                cooldown: CooldownPolicy::default(),
                only_if_position: PositionFilter::default(),
            },
            Rule {
                name: "b".into(),
                when: Condition::Compare {
                    left: ValueSource::Indicator(rsi.clone()), // duplicate
                    op: CompareOp::Lt,
                    right: ValueSource::Constant(30.0),
                },
                action: Action::NoOp,
                cooldown: CooldownPolicy::default(),
                only_if_position: PositionFilter::default(),
            },
        ];
        let refs = gather_indicator_refs(&rules);
        assert_eq!(
            refs.len(),
            3,
            "expected 3 distinct indicators, got {refs:#?}"
        );
    }

    #[test]
    fn cadence_fixed_returns_constant() {
        let c = Cadence::Fixed {
            interval_seconds: 30,
        };
        assert_eq!(c.delay_seconds(0), 30);
        assert_eq!(c.delay_seconds(123_456), 30);
    }

    #[test]
    fn cadence_candle_close_aligns_to_next_boundary() {
        let c = Cadence::CandleClose {
            timeframe: "1h".into(),
            grace_seconds: Some(5),
        };
        // 1h = 3600s. At unix=10, next boundary is 3600 → delay 3590 + 5 grace.
        assert_eq!(c.delay_seconds(10), 3595);
        // Already on a boundary: jump a full bar plus grace, not zero.
        assert_eq!(c.delay_seconds(3600), 3605);
        // 15m timeframe at 14:23:45 → next 14:30:00. unix=14*3600+23*60+45 → remainder 1425 → to_next 1725 → delay 1730 (with 5s grace)
        let c15 = Cadence::CandleClose {
            timeframe: "15m".into(),
            grace_seconds: Some(5),
        };
        let now = 14 * 3600 + 23 * 60 + 45;
        let remainder = now % 900;
        let to_next = 900 - remainder;
        assert_eq!(c15.delay_seconds(now as u64), (to_next + 5) as u64);
    }

    #[test]
    fn cadence_defaults_to_fixed_60s() {
        assert!(matches!(
            Cadence::default(),
            Cadence::Fixed {
                interval_seconds: 60
            }
        ));
    }

    #[test]
    fn action_serde_round_trip() {
        for a in [
            Action::Open {
                side: StrategySide::Buy,
                size: SizeSpec::Tokens(1.0),
            },
            Action::Close,
            Action::Reduce { fraction: 0.5 },
            Action::NoOp,
        ] {
            let json = serde_json::to_string(&a).unwrap();
            let _back: Action = serde_json::from_str(&json).unwrap();
        }
    }
}

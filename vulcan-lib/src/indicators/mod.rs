//! Technical analysis indicators on Phoenix candle data.
//!
//! Wraps the `kand` crate behind a Vulcan-native API so `kand` types
//! never leak through the public surface. All indicators compute
//! over `Vec<CandleRow>` returned by `commands::market::execute_candles_inner`.

pub mod batch;
pub mod trigger;

use crate::commands::market;
use crate::context::AppContext;
use crate::error::VulcanError;
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;
use std::str::FromStr;

pub use trigger::{Op, TriggerOutcome, TriggerSpec};

/// Indicators currently supported.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum IndicatorKind {
    Sma,
    Ema,
    Rsi,
    Macd,
    Bbands,
    Atr,
    Vwap,
    Adx,
    Stoch,
}

impl IndicatorKind {
    /// Default period used when the caller omits one.
    pub fn default_period(self) -> usize {
        match self {
            Self::Sma | Self::Ema => 20,
            Self::Rsi | Self::Atr | Self::Adx => 14,
            Self::Bbands => 20,
            Self::Stoch => 14,
            Self::Macd | Self::Vwap => 0, // not used; macd has its own triplet, vwap is cumulative
        }
    }

    /// The "primary" output series key used by triggers and the latest-value summary.
    pub fn primary_key(self) -> &'static str {
        match self {
            Self::Sma => "sma",
            Self::Ema => "ema",
            Self::Rsi => "rsi",
            Self::Macd => "macd",
            Self::Bbands => "middle",
            Self::Atr => "atr",
            Self::Vwap => "vwap",
            Self::Adx => "adx",
            Self::Stoch => "k",
        }
    }

    /// Minimum number of candles required to produce at least one non-NaN value.
    pub fn min_candles(self, period: usize) -> usize {
        match self {
            Self::Sma | Self::Ema | Self::Rsi | Self::Atr | Self::Bbands => period + 1,
            Self::Macd => 35, // slow_period(26) + signal_period(9)
            Self::Adx => period * 2 + 1,
            Self::Stoch => period + 3,
            Self::Vwap => 1,
        }
    }
}

impl FromStr for IndicatorKind {
    type Err = VulcanError;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        match s.to_ascii_lowercase().as_str() {
            "sma" => Ok(Self::Sma),
            "ema" => Ok(Self::Ema),
            "rsi" => Ok(Self::Rsi),
            "macd" => Ok(Self::Macd),
            "bbands" | "bollinger" | "bb" => Ok(Self::Bbands),
            "atr" => Ok(Self::Atr),
            "vwap" => Ok(Self::Vwap),
            "adx" => Ok(Self::Adx),
            "stoch" | "stochastic" => Ok(Self::Stoch),
            other => Err(VulcanError::validation(
                "INVALID_INDICATOR",
                format!("unknown indicator: {other} (supported: sma, ema, rsi, macd, bbands, atr, vwap, adx, stoch)"),
            )),
        }
    }
}

/// Request to compute a single indicator.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct IndicatorRequest {
    pub kind: IndicatorKind,
    /// Lookback period; falls back to `kind.default_period()` when omitted.
    pub period: Option<usize>,
    /// Indicator-specific parameters (e.g. MACD fast/slow/signal, BBands dev).
    #[serde(default)]
    pub params: BTreeMap<String, f64>,
}

impl IndicatorRequest {
    pub fn new(kind: IndicatorKind) -> Self {
        Self {
            kind,
            period: None,
            params: BTreeMap::new(),
        }
    }

    pub fn with_period(mut self, period: usize) -> Self {
        self.period = Some(period);
        self
    }

    pub fn period_or_default(&self) -> usize {
        self.period.unwrap_or_else(|| self.kind.default_period())
    }

    pub fn param(&self, key: &str) -> Option<f64> {
        self.params.get(key).copied()
    }
}

/// One bar's worth of indicator output. Multi-line indicators (MACD, BBands…)
/// populate several keys.
#[derive(Debug, Clone, Serialize)]
pub struct IndicatorPoint {
    pub time: String,
    pub values: BTreeMap<String, f64>,
}

impl IndicatorPoint {
    pub fn primary(&self, key: &str) -> Option<f64> {
        self.values.get(key).copied().filter(|v| !v.is_nan())
    }
}

#[derive(Debug, Clone, Serialize)]
pub struct IndicatorSummary {
    pub primary_key: String,
    pub latest: Option<f64>,
    pub min: Option<f64>,
    pub max: Option<f64>,
    pub mean: Option<f64>,
    /// One-line agent-friendly read of the current state.
    pub verdict: String,
}

#[derive(Debug, Clone, Serialize)]
pub struct IndicatorSeries {
    pub kind: IndicatorKind,
    pub symbol: String,
    pub timeframe: String,
    pub period: usize,
    /// Named values from the most recent bar — hoisted from `points.last().values` so
    /// callers don't have to scan the series for current state (e.g. `bbands.upper`,
    /// `macd.signal`). Empty when no non-NaN data is available.
    #[serde(default)]
    pub latest: BTreeMap<String, f64>,
    /// Derived per-kind state flags (RSI state, BBands position_in_band, ADX
    /// trend_strength, MACD recent_cross, …). Computed from the full series, so they
    /// stay valid even when `points` is trimmed in `vulcan_ta_report`.
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub signals: BTreeMap<String, serde_json::Value>,
    pub points: Vec<IndicatorPoint>,
    pub summary: IndicatorSummary,
}

impl IndicatorSeries {
    /// Build a series from raw points, auto-deriving `summary`, `latest`, and `signals`.
    pub fn from_points(
        kind: IndicatorKind,
        symbol: String,
        timeframe: String,
        period: usize,
        points: Vec<IndicatorPoint>,
    ) -> Self {
        let summary = build_summary(kind, &points);
        let latest = build_latest_snapshot(&points);
        let signals = build_signals(kind, &points);
        Self {
            kind,
            symbol,
            timeframe,
            period,
            latest,
            signals,
            points,
            summary,
        }
    }
}

impl crate::output::TableRenderable for IndicatorSeries {
    fn render_table(&self) {
        println!(
            "{} {} ({} period={}) — {}",
            self.symbol,
            indicator_label(self.kind),
            self.timeframe,
            self.period,
            self.summary.verdict
        );
        if let Some(v) = self.summary.latest {
            println!("latest {}: {:.4}", self.summary.primary_key, v);
        }
        let signals = format_signals(&self.signals);
        if !signals.is_empty() {
            println!("signals: {}", signals);
        }
        if self.points.is_empty() {
            return;
        }

        // Render the last N points (cap at 20 to keep tables small).
        let take = self.points.len().min(20);
        let start = self.points.len() - take;
        let mut keys: Vec<String> = self
            .points
            .last()
            .map(|p| p.values.keys().cloned().collect())
            .unwrap_or_default();
        keys.sort();

        let mut headers: Vec<&str> = vec!["Time"];
        for k in &keys {
            headers.push(k.as_str());
        }
        let rows: Vec<Vec<String>> = self.points[start..]
            .iter()
            .map(|p| {
                let mut row = vec![p.time.clone()];
                for k in &keys {
                    let cell = p.values.get(k).map_or("-".to_string(), |v| {
                        if v.is_nan() {
                            "-".to_string()
                        } else {
                            format!("{:.4}", v)
                        }
                    });
                    row.push(cell);
                }
                row
            })
            .collect();
        crate::output::table::render_table(&headers, rows);
    }
}

fn indicator_label(kind: IndicatorKind) -> &'static str {
    match kind {
        IndicatorKind::Sma => "SMA",
        IndicatorKind::Ema => "EMA",
        IndicatorKind::Rsi => "RSI",
        IndicatorKind::Macd => "MACD",
        IndicatorKind::Bbands => "BBands",
        IndicatorKind::Atr => "ATR",
        IndicatorKind::Vwap => "VWAP",
        IndicatorKind::Adx => "ADX",
        IndicatorKind::Stoch => "Stochastic",
    }
}

/// How many candles to fetch by default when the caller doesn't specify.
const DEFAULT_FETCH_LIMIT: usize = 200;

/// Compute a single indicator over the latest candle window for `symbol`.
pub async fn compute(
    ctx: &AppContext,
    symbol: &str,
    timeframe: &str,
    request: &IndicatorRequest,
    limit: Option<usize>,
) -> Result<IndicatorSeries, VulcanError> {
    let period = request.period_or_default();
    let min = request.kind.min_candles(period);
    let fetch_limit = limit.unwrap_or(DEFAULT_FETCH_LIMIT).max(min + 5);

    let candles = market::execute_candles_inner(ctx, symbol, timeframe, fetch_limit).await?;
    if candles.candles.len() < min {
        return Err(VulcanError::validation(
            "INDICATOR_WARMUP_INSUFFICIENT",
            format!(
                "{} needs at least {} candles for period {}, got {}",
                indicator_label(request.kind),
                min,
                period,
                candles.candles.len()
            ),
        ));
    }

    let points = batch::compute_points(&candles.candles, request)?;
    Ok(IndicatorSeries::from_points(
        request.kind,
        candles.symbol,
        candles.interval,
        period,
        points,
    ))
}

/// Evaluate a trigger spec: fetch candles, compute, compare the latest value.
pub async fn evaluate_trigger(
    ctx: &AppContext,
    symbol: &str,
    spec: &TriggerSpec,
) -> Result<TriggerOutcome, VulcanError> {
    let request = IndicatorRequest {
        kind: spec.indicator,
        period: spec.period,
        params: spec.params.clone(),
    };
    let series = compute(ctx, symbol, &spec.timeframe, &request, None).await?;
    let outcome = trigger::evaluate(spec, &series)?;
    Ok(outcome)
}

/// Render the per-kind signal map as `key=value` pairs joined by `, ` for CLI output.
/// Numbers print with 4 decimals; strings print as-is; other JSON types fall through to
/// their compact serialization (rare in practice — signals are limited to numbers/strings).
fn format_signals(signals: &BTreeMap<String, serde_json::Value>) -> String {
    use serde_json::Value;
    let mut parts = Vec::with_capacity(signals.len());
    for (k, v) in signals {
        let rendered = match v {
            Value::Number(n) => n
                .as_f64()
                .map(|f| format!("{:.4}", f))
                .unwrap_or_else(|| v.to_string()),
            Value::String(s) => s.clone(),
            other => other.to_string(),
        };
        parts.push(format!("{}={}", k, rendered));
    }
    parts.join(", ")
}

fn build_latest_snapshot(points: &[IndicatorPoint]) -> BTreeMap<String, f64> {
    let Some(last) = points.last() else {
        return BTreeMap::new();
    };
    last.values
        .iter()
        .filter(|(_, v)| !v.is_nan())
        .map(|(k, v)| (k.clone(), *v))
        .collect()
}

fn round4(v: f64) -> f64 {
    (v * 10_000.0).round() / 10_000.0
}

fn get_finite(point: &IndicatorPoint, key: &str) -> Option<f64> {
    point.values.get(key).copied().filter(|v| !v.is_nan())
}

fn build_signals(
    kind: IndicatorKind,
    points: &[IndicatorPoint],
) -> BTreeMap<String, serde_json::Value> {
    use serde_json::{json, Value};
    let mut out: BTreeMap<String, Value> = BTreeMap::new();
    let Some(last) = points.last() else {
        return out;
    };
    let close = get_finite(last, "close");

    match kind {
        IndicatorKind::Rsi => {
            if let Some(v) = get_finite(last, "rsi") {
                let state = match v {
                    x if x >= 70.0 => "overbought",
                    x if x >= 55.0 => "bullish",
                    x if x > 45.0 => "neutral",
                    x if x > 30.0 => "bearish",
                    _ => "oversold",
                };
                out.insert("state".into(), json!(state));
            }
        }
        IndicatorKind::Macd => {
            if let Some(hist) = get_finite(last, "hist") {
                let momentum = if hist > 0.0 {
                    "bullish"
                } else if hist < 0.0 {
                    "bearish"
                } else {
                    "flat"
                };
                out.insert("momentum".into(), json!(momentum));
                // Look at the last 5 bars for a sign change in hist.
                let recent: Vec<f64> = points
                    .iter()
                    .rev()
                    .take(5)
                    .filter_map(|p| get_finite(p, "hist"))
                    .collect();
                let cross = recent.windows(2).find_map(|w| {
                    // recent[0] is newest; w[0] newer than w[1].
                    if w[1] <= 0.0 && w[0] > 0.0 {
                        Some("bullish")
                    } else if w[1] >= 0.0 && w[0] < 0.0 {
                        Some("bearish")
                    } else {
                        None
                    }
                });
                if let Some(c) = cross {
                    out.insert("recent_cross".into(), json!(c));
                }
            }
        }
        IndicatorKind::Bbands => {
            if let (Some(u), Some(l), Some(m), Some(c)) = (
                get_finite(last, "upper"),
                get_finite(last, "lower"),
                get_finite(last, "middle"),
                close,
            ) {
                let width = u - l;
                let pos = if width > 0.0 { (c - l) / width } else { 0.5 };
                let state = if c >= u {
                    "above_upper"
                } else if pos >= 0.8 {
                    "near_upper"
                } else if c <= l {
                    "below_lower"
                } else if pos <= 0.2 {
                    "near_lower"
                } else {
                    "inside"
                };
                out.insert("state".into(), json!(state));
                out.insert("position_in_band".into(), json!(round4(pos)));
                if m > 0.0 {
                    out.insert("width_pct".into(), json!(round4(width / m * 100.0)));
                }
            }
        }
        IndicatorKind::Atr => {
            if let (Some(v), Some(c)) = (get_finite(last, "atr"), close) {
                if c > 0.0 {
                    out.insert("atr_pct_of_price".into(), json!(round4(v / c * 100.0)));
                }
            }
        }
        IndicatorKind::Adx => {
            if let Some(v) = get_finite(last, "adx") {
                let strength = match v {
                    x if x >= 50.0 => "very_strong",
                    x if x >= 25.0 => "strong",
                    x if x >= 20.0 => "emerging",
                    _ => "weak",
                };
                out.insert("trend_strength".into(), json!(strength));
            }
        }
        IndicatorKind::Stoch => {
            if let Some(kv) = get_finite(last, "k") {
                let state = if kv >= 80.0 {
                    "overbought"
                } else if kv <= 20.0 {
                    "oversold"
                } else {
                    "neutral"
                };
                out.insert("state".into(), json!(state));
                if let Some(dv) = get_finite(last, "d") {
                    out.insert("k_minus_d".into(), json!(round4(kv - dv)));
                }
            }
        }
        IndicatorKind::Sma | IndicatorKind::Ema | IndicatorKind::Vwap => {
            if let (Some(v), Some(c)) = (get_finite(last, kind.primary_key()), close) {
                let state = if c > v {
                    "price_above"
                } else if c < v {
                    "price_below"
                } else {
                    "price_at"
                };
                out.insert("state".into(), json!(state));
            }
        }
    }
    out
}

fn build_summary(kind: IndicatorKind, points: &[IndicatorPoint]) -> IndicatorSummary {
    let primary_key = kind.primary_key().to_string();
    let values: Vec<f64> = points
        .iter()
        .filter_map(|p| p.primary(&primary_key))
        .collect();

    let latest = values.last().copied();
    let min = values.iter().copied().fold(None, |acc: Option<f64>, v| {
        Some(acc.map_or(v, |m| m.min(v)))
    });
    let max = values.iter().copied().fold(None, |acc: Option<f64>, v| {
        Some(acc.map_or(v, |m| m.max(v)))
    });
    let mean = if values.is_empty() {
        None
    } else {
        Some(values.iter().sum::<f64>() / values.len() as f64)
    };

    let verdict = verdict_for(kind, latest, mean, points);

    IndicatorSummary {
        primary_key,
        latest,
        min,
        max,
        mean,
        verdict,
    }
}

fn verdict_for(
    kind: IndicatorKind,
    latest: Option<f64>,
    _mean: Option<f64>,
    points: &[IndicatorPoint],
) -> String {
    let Some(v) = latest else {
        return "insufficient data".to_string();
    };
    match kind {
        IndicatorKind::Rsi => match v {
            x if x >= 70.0 => format!("overbought (RSI {:.1})", x),
            x if x <= 30.0 => format!("oversold (RSI {:.1})", x),
            x if x >= 55.0 => format!("bullish bias (RSI {:.1})", x),
            x if x <= 45.0 => format!("bearish bias (RSI {:.1})", x),
            x => format!("neutral (RSI {:.1})", x),
        },
        IndicatorKind::Macd => {
            let hist = points
                .last()
                .and_then(|p| p.values.get("hist").copied())
                .unwrap_or(0.0);
            if hist > 0.0 {
                format!("bullish momentum (hist {:.4})", hist)
            } else if hist < 0.0 {
                format!("bearish momentum (hist {:.4})", hist)
            } else {
                "flat momentum".to_string()
            }
        }
        IndicatorKind::Bbands => {
            let upper = points
                .last()
                .and_then(|p| p.values.get("upper").copied())
                .unwrap_or(f64::NAN);
            let lower = points
                .last()
                .and_then(|p| p.values.get("lower").copied())
                .unwrap_or(f64::NAN);
            let close = points
                .last()
                .and_then(|p| p.values.get("close").copied())
                .unwrap_or(f64::NAN);
            if !close.is_nan() && !upper.is_nan() && !lower.is_nan() {
                if close >= upper {
                    format!("price at/above upper band ({:.2} ≥ {:.2})", close, upper)
                } else if close <= lower {
                    format!("price at/below lower band ({:.2} ≤ {:.2})", close, lower)
                } else {
                    format!("inside band ({:.2}–{:.2})", lower, upper)
                }
            } else {
                format!("middle {:.2}", v)
            }
        }
        IndicatorKind::Adx => match v {
            x if x >= 25.0 => format!("strong trend (ADX {:.1})", x),
            x if x >= 20.0 => format!("emerging trend (ADX {:.1})", x),
            x => format!("range-bound (ADX {:.1})", x),
        },
        IndicatorKind::Stoch => {
            let d = points
                .last()
                .and_then(|p| p.values.get("d").copied())
                .unwrap_or(f64::NAN);
            match v {
                x if x >= 80.0 => format!("overbought (K {:.1} / D {:.1})", x, d),
                x if x <= 20.0 => format!("oversold (K {:.1} / D {:.1})", x, d),
                x => format!("neutral (K {:.1} / D {:.1})", x, d),
            }
        }
        IndicatorKind::Atr => format!("ATR {:.4}", v),
        IndicatorKind::Vwap => format!("VWAP {:.2}", v),
        IndicatorKind::Sma | IndicatorKind::Ema => format!("{} {:.4}", indicator_label(kind), v),
    }
}

/// Bundle of indicators for the `ta report` command — RSI, MACD, BBands, ATR by default.
#[derive(Debug, Clone, Serialize)]
pub struct IndicatorReport {
    pub symbol: String,
    pub timeframe: String,
    pub indicators: Vec<IndicatorSeries>,
}

impl crate::output::TableRenderable for IndicatorReport {
    fn render_table(&self) {
        println!(
            "Technical analysis report: {} ({})\n",
            self.symbol, self.timeframe
        );
        for series in &self.indicators {
            let latest = series
                .summary
                .latest
                .map(|v| format!("{:.4}", v))
                .unwrap_or_else(|| "-".to_string());
            println!(
                "  {:<10} latest={:<12} {}",
                indicator_label(series.kind),
                latest,
                series.summary.verdict
            );
            let signals = format_signals(&series.signals);
            if !signals.is_empty() {
                println!("             signals: {}", signals);
            }
        }
    }
}

pub const DEFAULT_REPORT_KINDS: &[IndicatorKind] = &[
    IndicatorKind::Rsi,
    IndicatorKind::Macd,
    IndicatorKind::Bbands,
    IndicatorKind::Atr,
    IndicatorKind::Adx,
];

/// Build the bundled report. `points_limit`:
/// - `None` → omit per-indicator `points` entirely (default; agent-friendly).
/// - `Some(n)` → keep the last `n` points per indicator (`n = 0` is the same as `None`).
///
/// `latest`, `signals`, and `summary` are always computed from the full untrimmed
/// series, so callers can drop `points` and still see current state.
pub async fn report(
    ctx: &AppContext,
    symbol: &str,
    timeframe: &str,
    kinds: Option<&[IndicatorKind]>,
    points_limit: Option<usize>,
) -> Result<IndicatorReport, VulcanError> {
    let kinds = kinds.unwrap_or(DEFAULT_REPORT_KINDS);
    let mut indicators = Vec::with_capacity(kinds.len());
    for kind in kinds {
        let request = IndicatorRequest::new(*kind);
        match compute(ctx, symbol, timeframe, &request, None).await {
            Ok(mut series) => {
                series.points = match points_limit {
                    None | Some(0) => Vec::new(),
                    Some(n) if n >= series.points.len() => series.points,
                    Some(n) => {
                        let cut = series.points.len() - n;
                        series.points.split_off(cut)
                    }
                };
                indicators.push(series);
            }
            // Skip indicators that didn't have enough warmup rather than failing the whole report.
            Err(e) if e.code == "INDICATOR_WARMUP_INSUFFICIENT" => continue,
            Err(e) => return Err(e),
        }
    }
    Ok(IndicatorReport {
        symbol: symbol.to_string(),
        timeframe: timeframe.to_string(),
        indicators,
    })
}

#[cfg(test)]
mod report_shape_tests {
    use super::*;

    fn point(time: &str, pairs: &[(&str, f64)]) -> IndicatorPoint {
        let mut values = BTreeMap::new();
        for (k, v) in pairs {
            values.insert((*k).to_string(), *v);
        }
        IndicatorPoint {
            time: time.to_string(),
            values,
        }
    }

    #[test]
    fn latest_snapshot_drops_nan_and_uses_last_point() {
        let points = vec![
            point("t0", &[("rsi", 50.0)]),
            point("t1", &[("rsi", f64::NAN), ("close", 100.0)]),
            point("t2", &[("rsi", 32.5), ("close", 99.4)]),
        ];
        let snap = build_latest_snapshot(&points);
        assert_eq!(snap.get("rsi"), Some(&32.5));
        assert_eq!(snap.get("close"), Some(&99.4));
        assert_eq!(snap.len(), 2);

        let empty = build_latest_snapshot(&[]);
        assert!(empty.is_empty());
    }

    #[test]
    fn rsi_signals_state_thresholds() {
        let cases = [
            (15.0, "oversold"),
            (40.0, "bearish"),
            (50.0, "neutral"),
            (60.0, "bullish"),
            (75.0, "overbought"),
        ];
        for (v, expected) in cases {
            let pts = vec![point("t", &[("rsi", v)])];
            let sigs = build_signals(IndicatorKind::Rsi, &pts);
            assert_eq!(
                sigs.get("state").and_then(|s| s.as_str()),
                Some(expected),
                "rsi={} expected state={}",
                v,
                expected
            );
        }
    }

    #[test]
    fn bbands_signals_position_and_state() {
        // close pierces below lower
        let pts = vec![point(
            "t",
            &[
                ("upper", 110.0),
                ("middle", 100.0),
                ("lower", 90.0),
                ("close", 88.0),
            ],
        )];
        let sigs = build_signals(IndicatorKind::Bbands, &pts);
        assert_eq!(sigs.get("state").unwrap().as_str(), Some("below_lower"));
        // (88-90)/(110-90) = -0.1
        let pos = sigs.get("position_in_band").unwrap().as_f64().unwrap();
        assert!((pos - -0.1).abs() < 1e-6, "pos={}", pos);
        // width / middle * 100 = 20 / 100 * 100 = 20
        let width = sigs.get("width_pct").unwrap().as_f64().unwrap();
        assert!((width - 20.0).abs() < 1e-6);

        // inside band
        let pts2 = vec![point(
            "t",
            &[
                ("upper", 110.0),
                ("middle", 100.0),
                ("lower", 90.0),
                ("close", 100.0),
            ],
        )];
        let sigs2 = build_signals(IndicatorKind::Bbands, &pts2);
        assert_eq!(sigs2.get("state").unwrap().as_str(), Some("inside"));
    }

    #[test]
    fn adx_trend_strength_buckets() {
        let cases = [
            (10.0, "weak"),
            (22.0, "emerging"),
            (30.0, "strong"),
            (60.0, "very_strong"),
        ];
        for (v, expected) in cases {
            let pts = vec![point("t", &[("adx", v)])];
            let sigs = build_signals(IndicatorKind::Adx, &pts);
            assert_eq!(
                sigs.get("trend_strength").and_then(|s| s.as_str()),
                Some(expected),
                "adx={} expected={}",
                v,
                expected
            );
        }
    }

    #[test]
    fn atr_signals_pct_of_price() {
        let pts = vec![point("t", &[("atr", 5.0), ("close", 100.0)])];
        let sigs = build_signals(IndicatorKind::Atr, &pts);
        let pct = sigs.get("atr_pct_of_price").unwrap().as_f64().unwrap();
        assert!((pct - 5.0).abs() < 1e-6, "pct={}", pct);

        // missing close → no pct
        let pts_no_close = vec![point("t", &[("atr", 5.0)])];
        let sigs2 = build_signals(IndicatorKind::Atr, &pts_no_close);
        assert!(!sigs2.contains_key("atr_pct_of_price"));
    }

    #[test]
    fn macd_recent_cross_detects_sign_flip_within_window() {
        let pts: Vec<IndicatorPoint> = [-0.3_f64, -0.1, 0.05, 0.2]
            .iter()
            .enumerate()
            .map(|(i, h)| point(&format!("t{}", i), &[("macd", 0.0), ("hist", *h)]))
            .collect();
        let sigs = build_signals(IndicatorKind::Macd, &pts);
        assert_eq!(sigs.get("momentum").unwrap().as_str(), Some("bullish"));
        assert_eq!(sigs.get("recent_cross").unwrap().as_str(), Some("bullish"));
    }

    #[test]
    fn from_points_populates_latest_signals_and_summary() {
        let pts = vec![point("t", &[("rsi", 28.0), ("close", 100.0)])];
        let series =
            IndicatorSeries::from_points(IndicatorKind::Rsi, "TEST".into(), "1h".into(), 14, pts);
        assert_eq!(series.latest.get("rsi"), Some(&28.0));
        assert_eq!(
            series.signals.get("state").and_then(|s| s.as_str()),
            Some("oversold")
        );
        assert_eq!(series.summary.latest, Some(28.0));
        assert!(series.summary.verdict.contains("oversold"));
    }
}

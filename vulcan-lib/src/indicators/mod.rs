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
    pub points: Vec<IndicatorPoint>,
    pub summary: IndicatorSummary,
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
    let summary = build_summary(request.kind, &points);

    Ok(IndicatorSeries {
        kind: request.kind,
        symbol: candles.symbol,
        timeframe: candles.interval,
        period,
        points,
        summary,
    })
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
        }
    }
}

pub async fn report(
    ctx: &AppContext,
    symbol: &str,
    timeframe: &str,
) -> Result<IndicatorReport, VulcanError> {
    let kinds = [
        IndicatorKind::Rsi,
        IndicatorKind::Macd,
        IndicatorKind::Bbands,
        IndicatorKind::Atr,
        IndicatorKind::Adx,
    ];
    let mut indicators = Vec::with_capacity(kinds.len());
    for kind in kinds {
        let request = IndicatorRequest::new(kind);
        match compute(ctx, symbol, timeframe, &request, None).await {
            Ok(series) => indicators.push(series),
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

//! Trigger specs: serializable conditions agents and strategy runners can
//! attach to indicator output to gate decisions.

use super::{IndicatorKind, IndicatorSeries};
use crate::error::VulcanError;
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Op {
    Lt,
    Lte,
    Gt,
    Gte,
    CrossesAbove,
    CrossesBelow,
}

impl Op {
    pub fn as_str(&self) -> &'static str {
        match self {
            Self::Lt => "<",
            Self::Lte => "<=",
            Self::Gt => ">",
            Self::Gte => ">=",
            Self::CrossesAbove => "crosses_above",
            Self::CrossesBelow => "crosses_below",
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TriggerSpec {
    pub indicator: IndicatorKind,
    pub timeframe: String,
    #[serde(default)]
    pub period: Option<usize>,
    pub op: Op,
    pub threshold: f64,
    /// Optional series key override (e.g. "hist" for MACD histogram, "k"/"d" for stoch).
    #[serde(default)]
    pub key: Option<String>,
    /// Indicator-specific parameters forwarded to compute().
    #[serde(default)]
    pub params: BTreeMap<String, f64>,
}

#[derive(Debug, Clone, Serialize)]
pub struct TriggerOutcome {
    pub fired: bool,
    pub value: Option<f64>,
    pub prev_value: Option<f64>,
    pub threshold: f64,
    pub op: Op,
    pub key: String,
    pub evaluated_at: String,
}

impl crate::output::TableRenderable for TriggerOutcome {
    fn render_table(&self) {
        let v = self
            .value
            .map(|v| format!("{:.4}", v))
            .unwrap_or_else(|| "-".to_string());
        println!(
            "trigger {} {} {} → {} (value={}, at={})",
            self.key,
            self.op.as_str(),
            self.threshold,
            if self.fired { "FIRED" } else { "not fired" },
            v,
            self.evaluated_at,
        );
    }
}

pub fn evaluate(
    spec: &TriggerSpec,
    series: &IndicatorSeries,
) -> Result<TriggerOutcome, VulcanError> {
    let key = spec
        .key
        .clone()
        .unwrap_or_else(|| spec.indicator.primary_key().to_string());

    // Pull the last two non-NaN values for the requested key.
    let valued: Vec<(usize, f64)> = series
        .points
        .iter()
        .enumerate()
        .filter_map(|(i, p)| {
            p.values
                .get(&key)
                .copied()
                .filter(|v| !v.is_nan())
                .map(|v| (i, v))
        })
        .collect();
    let (latest_idx, latest) = match valued.last().copied() {
        Some(v) => v,
        None => {
            return Err(VulcanError::validation(
                "INDICATOR_KEY_MISSING",
                format!("indicator series has no values for key '{}'", key),
            ));
        }
    };
    let prev = valued.iter().rev().nth(1).map(|(_, v)| *v);

    let fired = match spec.op {
        Op::Lt => latest < spec.threshold,
        Op::Lte => latest <= spec.threshold,
        Op::Gt => latest > spec.threshold,
        Op::Gte => latest >= spec.threshold,
        Op::CrossesAbove => prev.is_some_and(|p| p <= spec.threshold && latest > spec.threshold),
        Op::CrossesBelow => prev.is_some_and(|p| p >= spec.threshold && latest < spec.threshold),
    };

    let evaluated_at = series
        .points
        .get(latest_idx)
        .map(|p| p.time.clone())
        .unwrap_or_default();

    Ok(TriggerOutcome {
        fired,
        value: Some(latest),
        prev_value: prev,
        threshold: spec.threshold,
        op: spec.op,
        key,
        evaluated_at,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::indicators::{IndicatorPoint, IndicatorSummary};

    fn series(values: &[f64]) -> IndicatorSeries {
        let points: Vec<IndicatorPoint> = values
            .iter()
            .enumerate()
            .map(|(i, &v)| {
                let mut m = BTreeMap::new();
                m.insert("rsi".to_string(), v);
                IndicatorPoint {
                    time: format!("t{}", i),
                    values: m,
                }
            })
            .collect();
        IndicatorSeries {
            kind: IndicatorKind::Rsi,
            symbol: "TEST".into(),
            timeframe: "1h".into(),
            period: 14,
            points,
            summary: IndicatorSummary {
                primary_key: "rsi".into(),
                latest: values.last().copied(),
                min: None,
                max: None,
                mean: None,
                verdict: String::new(),
            },
        }
    }

    #[test]
    fn lt_fires_when_below_threshold() {
        let s = series(&[40.0, 35.0, 28.0]);
        let spec = TriggerSpec {
            indicator: IndicatorKind::Rsi,
            timeframe: "1h".into(),
            period: None,
            op: Op::Lt,
            threshold: 30.0,
            key: None,
            params: Default::default(),
        };
        let out = evaluate(&spec, &s).unwrap();
        assert!(out.fired);
        assert_eq!(out.value, Some(28.0));
    }

    #[test]
    fn crosses_above_requires_prior_below() {
        let s = series(&[60.0, 65.0, 72.0]);
        let spec = TriggerSpec {
            indicator: IndicatorKind::Rsi,
            timeframe: "1h".into(),
            period: None,
            op: Op::CrossesAbove,
            threshold: 70.0,
            key: None,
            params: Default::default(),
        };
        let out = evaluate(&spec, &s).unwrap();
        assert!(out.fired);

        // No crossing if previous already above:
        let s = series(&[71.0, 72.0]);
        let out = evaluate(&spec, &s).unwrap();
        assert!(!out.fired);
    }
}

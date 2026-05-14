//! Batch computation of indicators over a slice of candles.
//!
//! Each helper allocates the output buffers kand expects, runs the indicator,
//! and packs the results into a `Vec<IndicatorPoint>` aligned 1:1 with the input.

use super::{IndicatorKind, IndicatorPoint, IndicatorRequest};
use crate::commands::market::CandleRow;
use crate::error::VulcanError;
use std::collections::BTreeMap;

type Float = f64;

pub fn compute_points(
    candles: &[CandleRow],
    request: &IndicatorRequest,
) -> Result<Vec<IndicatorPoint>, VulcanError> {
    match request.kind {
        IndicatorKind::Sma => sma(candles, request.period_or_default()),
        IndicatorKind::Ema => ema(candles, request.period_or_default()),
        IndicatorKind::Rsi => rsi(candles, request.period_or_default()),
        IndicatorKind::Macd => macd(candles, request),
        IndicatorKind::Bbands => bbands(candles, request),
        IndicatorKind::Atr => atr(candles, request.period_or_default()),
        IndicatorKind::Vwap => vwap(candles),
        IndicatorKind::Adx => adx(candles, request.period_or_default()),
        IndicatorKind::Stoch => stoch(candles, request),
    }
}

fn closes(candles: &[CandleRow]) -> Vec<Float> {
    candles.iter().map(|c| c.close).collect()
}

fn highs(candles: &[CandleRow]) -> Vec<Float> {
    candles.iter().map(|c| c.high).collect()
}

fn lows(candles: &[CandleRow]) -> Vec<Float> {
    candles.iter().map(|c| c.low).collect()
}

fn volumes(candles: &[CandleRow]) -> Vec<Float> {
    candles.iter().map(|c| c.volume.unwrap_or(0.0)).collect()
}

fn empty_points(candles: &[CandleRow]) -> Vec<IndicatorPoint> {
    candles
        .iter()
        .map(|c| IndicatorPoint {
            time: c.time.clone(),
            values: BTreeMap::from([("close".to_string(), c.close)]),
        })
        .collect()
}

fn map_kand_err(code: &'static str, err: impl std::fmt::Display) -> VulcanError {
    VulcanError::internal(code, format!("kand error: {err}"))
}

fn sma(candles: &[CandleRow], period: usize) -> Result<Vec<IndicatorPoint>, VulcanError> {
    let input = closes(candles);
    let mut out = vec![Float::NAN; input.len()];
    kand::ta::ohlcv::sma::sma(&input, period, &mut out)
        .map_err(|e| map_kand_err("SMA_FAILED", e))?;
    let mut points = empty_points(candles);
    for (p, v) in points.iter_mut().zip(out.iter()) {
        p.values.insert("sma".to_string(), *v);
    }
    Ok(points)
}

fn ema(candles: &[CandleRow], period: usize) -> Result<Vec<IndicatorPoint>, VulcanError> {
    let input = closes(candles);
    let mut out = vec![Float::NAN; input.len()];
    kand::ta::ohlcv::ema::ema(&input, period, None, &mut out)
        .map_err(|e| map_kand_err("EMA_FAILED", e))?;
    let mut points = empty_points(candles);
    for (p, v) in points.iter_mut().zip(out.iter()) {
        p.values.insert("ema".to_string(), *v);
    }
    Ok(points)
}

fn rsi(candles: &[CandleRow], period: usize) -> Result<Vec<IndicatorPoint>, VulcanError> {
    let input = closes(candles);
    let n = input.len();
    let mut out_rsi = vec![Float::NAN; n];
    let mut out_gain = vec![Float::NAN; n];
    let mut out_loss = vec![Float::NAN; n];
    kand::ta::ohlcv::rsi::rsi(&input, period, &mut out_rsi, &mut out_gain, &mut out_loss)
        .map_err(|e| map_kand_err("RSI_FAILED", e))?;
    let mut points = empty_points(candles);
    for (p, v) in points.iter_mut().zip(out_rsi.iter()) {
        p.values.insert("rsi".to_string(), *v);
    }
    Ok(points)
}

fn macd(
    candles: &[CandleRow],
    request: &IndicatorRequest,
) -> Result<Vec<IndicatorPoint>, VulcanError> {
    let fast = request.param("fast").unwrap_or(12.0) as usize;
    let slow = request.param("slow").unwrap_or(26.0) as usize;
    let signal = request.param("signal").unwrap_or(9.0) as usize;
    let input = closes(candles);
    let n = input.len();
    let mut macd_line = vec![Float::NAN; n];
    let mut sig_line = vec![Float::NAN; n];
    let mut hist = vec![Float::NAN; n];
    let mut fast_ema = vec![Float::NAN; n];
    let mut slow_ema = vec![Float::NAN; n];
    kand::ta::ohlcv::macd::macd(
        &input,
        fast,
        slow,
        signal,
        &mut macd_line,
        &mut sig_line,
        &mut hist,
        &mut fast_ema,
        &mut slow_ema,
    )
    .map_err(|e| map_kand_err("MACD_FAILED", e))?;
    let mut points = empty_points(candles);
    for (i, p) in points.iter_mut().enumerate() {
        p.values.insert("macd".to_string(), macd_line[i]);
        p.values.insert("signal".to_string(), sig_line[i]);
        p.values.insert("hist".to_string(), hist[i]);
    }
    Ok(points)
}

fn bbands(
    candles: &[CandleRow],
    request: &IndicatorRequest,
) -> Result<Vec<IndicatorPoint>, VulcanError> {
    let period = request.period_or_default();
    let dev_up = request.param("dev_up").unwrap_or(2.0);
    let dev_down = request.param("dev_down").unwrap_or(2.0);
    let input = closes(candles);
    let n = input.len();
    let mut upper = vec![Float::NAN; n];
    let mut middle = vec![Float::NAN; n];
    let mut lower = vec![Float::NAN; n];
    let mut sma_buf = vec![Float::NAN; n];
    let mut var_buf = vec![Float::NAN; n];
    let mut sum_buf = vec![Float::NAN; n];
    let mut sum_sq_buf = vec![Float::NAN; n];
    kand::ta::ohlcv::bbands::bbands(
        &input,
        period,
        dev_up,
        dev_down,
        &mut upper,
        &mut middle,
        &mut lower,
        &mut sma_buf,
        &mut var_buf,
        &mut sum_buf,
        &mut sum_sq_buf,
    )
    .map_err(|e| map_kand_err("BBANDS_FAILED", e))?;
    let mut points = empty_points(candles);
    for (i, p) in points.iter_mut().enumerate() {
        p.values.insert("upper".to_string(), upper[i]);
        p.values.insert("middle".to_string(), middle[i]);
        p.values.insert("lower".to_string(), lower[i]);
        p.values.insert("close".to_string(), candles[i].close);
    }
    Ok(points)
}

fn atr(candles: &[CandleRow], period: usize) -> Result<Vec<IndicatorPoint>, VulcanError> {
    let h = highs(candles);
    let l = lows(candles);
    let c = closes(candles);
    let n = h.len();
    let mut out = vec![Float::NAN; n];
    kand::ta::ohlcv::atr::atr(&h, &l, &c, period, &mut out)
        .map_err(|e| map_kand_err("ATR_FAILED", e))?;
    let mut points = empty_points(candles);
    for (p, v) in points.iter_mut().zip(out.iter()) {
        p.values.insert("atr".to_string(), *v);
    }
    Ok(points)
}

fn vwap(candles: &[CandleRow]) -> Result<Vec<IndicatorPoint>, VulcanError> {
    let h = highs(candles);
    let l = lows(candles);
    let c = closes(candles);
    let v = volumes(candles);
    let n = h.len();
    let mut out_vwap = vec![Float::NAN; n];
    let mut cum_pv = vec![Float::NAN; n];
    let mut cum_vol = vec![Float::NAN; n];
    kand::ta::ohlcv::vwap::vwap(&h, &l, &c, &v, &mut out_vwap, &mut cum_pv, &mut cum_vol)
        .map_err(|e| map_kand_err("VWAP_FAILED", e))?;
    let mut points = empty_points(candles);
    for (p, v) in points.iter_mut().zip(out_vwap.iter()) {
        p.values.insert("vwap".to_string(), *v);
    }
    Ok(points)
}

fn adx(candles: &[CandleRow], period: usize) -> Result<Vec<IndicatorPoint>, VulcanError> {
    let h = highs(candles);
    let l = lows(candles);
    let c = closes(candles);
    let n = h.len();
    let mut out_adx = vec![Float::NAN; n];
    let mut plus_dm = vec![Float::NAN; n];
    let mut minus_dm = vec![Float::NAN; n];
    let mut tr = vec![Float::NAN; n];
    kand::ta::ohlcv::adx::adx(
        &h,
        &l,
        &c,
        period,
        &mut out_adx,
        &mut plus_dm,
        &mut minus_dm,
        &mut tr,
    )
    .map_err(|e| map_kand_err("ADX_FAILED", e))?;
    let mut points = empty_points(candles);
    for (p, v) in points.iter_mut().zip(out_adx.iter()) {
        p.values.insert("adx".to_string(), *v);
    }
    Ok(points)
}

fn stoch(
    candles: &[CandleRow],
    request: &IndicatorRequest,
) -> Result<Vec<IndicatorPoint>, VulcanError> {
    let k_period = request.period_or_default();
    let k_slow = request.param("k_slow").unwrap_or(3.0) as usize;
    let d_period = request.param("d").unwrap_or(3.0) as usize;
    let h = highs(candles);
    let l = lows(candles);
    let c = closes(candles);
    let n = h.len();
    let mut fast_k = vec![Float::NAN; n];
    let mut k = vec![Float::NAN; n];
    let mut d = vec![Float::NAN; n];
    kand::ta::ohlcv::stoch::stoch(
        &h,
        &l,
        &c,
        k_period,
        k_slow,
        d_period,
        &mut fast_k,
        &mut k,
        &mut d,
    )
    .map_err(|e| map_kand_err("STOCH_FAILED", e))?;
    let mut points = empty_points(candles);
    for (i, p) in points.iter_mut().enumerate() {
        p.values.insert("k".to_string(), k[i]);
        p.values.insert("d".to_string(), d[i]);
    }
    Ok(points)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn make_candles(closes: &[f64]) -> Vec<CandleRow> {
        closes
            .iter()
            .enumerate()
            .map(|(i, &c)| CandleRow {
                time: format!("t{}", i),
                open: c,
                high: c + 1.0,
                low: c - 1.0,
                close: c,
                volume: Some(1000.0),
            })
            .collect()
    }

    #[test]
    fn sma_matches_arithmetic_mean() {
        let candles = make_candles(&[1.0, 2.0, 3.0, 4.0, 5.0, 6.0, 7.0]);
        let points = sma(&candles, 3).unwrap();
        // SMA at index 2 = mean(1,2,3) = 2.0
        let v = points[2].values.get("sma").copied().unwrap();
        assert!((v - 2.0).abs() < 1e-9, "got {v}");
        // SMA at index 6 = mean(5,6,7) = 6.0
        let v = points[6].values.get("sma").copied().unwrap();
        assert!((v - 6.0).abs() < 1e-9, "got {v}");
    }

    #[test]
    fn rsi_in_range() {
        // Strictly rising series → RSI tends toward 100.
        let closes: Vec<f64> = (1..=30).map(|i| i as f64).collect();
        let candles = make_candles(&closes);
        let points = rsi(&candles, 14).unwrap();
        let last = points.last().unwrap().values.get("rsi").copied().unwrap();
        assert!(
            last > 99.0,
            "expected RSI ≈ 100 for monotonic series, got {last}"
        );
    }
}

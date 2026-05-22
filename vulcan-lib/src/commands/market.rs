//! Market command execution.

use crate::cli::market::MarketCommand;
use crate::context::AppContext;
use crate::error::VulcanError;
use crate::output::{render_success, TableRenderable};
use phoenix_rise::phoenix_rise_types::{
    candles::{ApiCandle, CandlesQueryParams, Timeframe},
    http_error::PhoenixHttpError,
};
use serde::{de::DeserializeOwned, Serialize};

// ── Result types ────────────────────────────────────────────────────────

#[derive(Debug, Serialize)]
pub struct MarketListResult {
    pub markets: Vec<MarketSummary>,
}

#[derive(Debug, Serialize)]
pub struct MarketSummary {
    pub symbol: String,
    pub status: String,
    pub taker_fee: f64,
    pub maker_fee: f64,
    pub max_leverage: f64,
    pub isolated_only: bool,
}

impl TableRenderable for MarketListResult {
    fn render_table(&self) {
        if self.markets.is_empty() {
            println!("No markets found.");
            return;
        }
        let rows: Vec<Vec<String>> = self
            .markets
            .iter()
            .map(|m| {
                vec![
                    m.symbol.clone(),
                    m.status.clone(),
                    format!("{:.2}%", m.taker_fee * 100.0),
                    format!("{:.2}%", m.maker_fee * 100.0),
                    format!("{:.0}x", m.max_leverage),
                    if m.isolated_only {
                        "yes".into()
                    } else {
                        "no".into()
                    },
                ]
            })
            .collect();
        crate::output::table::render_table(
            &[
                "Symbol",
                "Status",
                "Taker Fee",
                "Maker Fee",
                "Max Leverage",
                "Isolated Only",
            ],
            rows,
        );
    }
}

#[derive(Debug, Serialize)]
pub struct MarketInfoResult {
    pub symbol: String,
    pub status: String,
    pub market_pubkey: String,
    pub tick_size: u64,
    pub base_lots_decimals: i8,
    pub taker_fee: f64,
    pub maker_fee: f64,
    pub funding_interval_seconds: u32,
    pub funding_period_seconds: u32,
    pub max_funding_rate_per_interval: f64,
    pub isolated_only: bool,
    pub leverage_tiers: Vec<LeverageTierInfo>,
}

#[derive(Debug, Serialize)]
pub struct LeverageTierInfo {
    pub max_leverage: f64,
    pub max_size_base_lots: u64,
}

impl TableRenderable for MarketInfoResult {
    fn render_table(&self) {
        println!("Market: {}", self.symbol);
        println!("Status: {}", self.status);
        println!("Pubkey: {}", self.market_pubkey);
        println!("Tick size: {}", self.tick_size);
        println!("Base lots decimals: {}", self.base_lots_decimals);
        println!("Taker fee: {:.4}%", self.taker_fee * 100.0);
        println!("Maker fee: {:.4}%", self.maker_fee * 100.0);
        println!(
            "Funding: every {}s / period {}s",
            self.funding_interval_seconds, self.funding_period_seconds
        );
        println!(
            "Max funding rate/interval: {:.6}%",
            self.max_funding_rate_per_interval * 100.0
        );
        println!("Isolated only: {}", self.isolated_only);
        if !self.leverage_tiers.is_empty() {
            println!("\nLeverage tiers:");
            let rows: Vec<Vec<String>> = self
                .leverage_tiers
                .iter()
                .map(|t| {
                    vec![
                        format!("{:.0}x", t.max_leverage),
                        t.max_size_base_lots.to_string(),
                    ]
                })
                .collect();
            crate::output::table::render_table(&["Max Leverage", "Max Size (base lots)"], rows);
        }
    }
}

#[derive(Debug, Serialize)]
pub struct TickerResult {
    pub symbol: String,
    pub mark_price: f64,
    pub mid_price: f64,
    pub oracle_price: f64,
    pub prev_day_price: f64,
    pub change_24h_pct: f64,
    pub volume_24h_usd: f64,
    pub open_interest: f64,
    pub funding_rate: f64,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub warnings: Vec<String>,
}

impl TableRenderable for TickerResult {
    fn render_table(&self) {
        let rows = vec![vec![
            self.symbol.clone(),
            format!("${:.2}", self.mark_price),
            format!("{:+.2}%", self.change_24h_pct),
            format!("${:.0}", self.volume_24h_usd),
            format!("{:.0}", self.open_interest),
            format!("{:+.4}%", self.funding_rate * 100.0),
        ]];
        crate::output::table::render_table(
            &[
                "Symbol",
                "Mark Price",
                "24h Change",
                "24h Volume",
                "Open Interest",
                "Funding Rate",
            ],
            rows,
        );
        for warning in &self.warnings {
            eprintln!("warning: {warning}");
        }
    }
}

#[derive(Debug, Serialize)]
pub struct CandlesResult {
    pub symbol: String,
    pub interval: String,
    pub candles: Vec<CandleRow>,
    /// Optional indicator overlays, present when the caller asked for `--with-indicators`.
    /// One entry per requested indicator, each aligned 1:1 with `candles` by index.
    #[serde(skip_serializing_if = "Vec::is_empty", default)]
    pub indicators: Vec<crate::indicators::IndicatorSeries>,
}

#[derive(Debug, Serialize)]
pub struct CandleRow {
    pub time: String,
    pub open: f64,
    pub high: f64,
    pub low: f64,
    pub close: f64,
    pub volume: Option<f64>,
}

impl TableRenderable for CandlesResult {
    fn render_table(&self) {
        // Collect indicator columns: (header, per-row formatted value).
        let mut extra_headers: Vec<String> = Vec::new();
        let mut extra_cols: Vec<Vec<String>> = Vec::new();
        for series in &self.indicators {
            let key = series.summary.primary_key.clone();
            let header = format!("{}({})", key, series.period);
            let col: Vec<String> = series
                .points
                .iter()
                .map(|p| match p.values.get(&key).copied() {
                    Some(v) if !v.is_nan() => format!("{:.4}", v),
                    _ => "-".to_string(),
                })
                .collect();
            extra_headers.push(header);
            extra_cols.push(col);
        }

        let mut headers: Vec<&str> = vec!["Time", "Open", "High", "Low", "Close", "Volume"];
        for h in &extra_headers {
            headers.push(h.as_str());
        }

        let rows: Vec<Vec<String>> = self
            .candles
            .iter()
            .enumerate()
            .map(|(i, c)| {
                let mut row = vec![
                    c.time.clone(),
                    format!("{:.2}", c.open),
                    format!("{:.2}", c.high),
                    format!("{:.2}", c.low),
                    format!("{:.2}", c.close),
                    c.volume.map_or("-".into(), |v| format!("{:.0}", v)),
                ];
                for col in &extra_cols {
                    row.push(col.get(i).cloned().unwrap_or_else(|| "-".into()));
                }
                row
            })
            .collect();
        crate::output::table::render_table(&headers, rows);

        for series in &self.indicators {
            println!("  {}", series.summary.verdict);
        }
    }
}

#[derive(Debug, Serialize)]
pub struct OrderbookResult {
    pub symbol: String,
    pub mid_price: Option<f64>,
    pub spread: Option<f64>,
    pub bids: Vec<OrderbookLevel>,
    pub asks: Vec<OrderbookLevel>,
}

#[derive(Debug, Serialize)]
pub struct OrderbookLevel {
    pub price: f64,
    pub quantity: f64,
}

impl TableRenderable for OrderbookResult {
    fn render_table(&self) {
        if self.bids.is_empty() && self.asks.is_empty() {
            println!("Orderbook is empty for {}.", self.symbol);
            return;
        }

        if let (Some(mid), Some(spread)) = (self.mid_price, self.spread) {
            let spread_bps = if mid > 0.0 {
                (spread / mid) * 10000.0
            } else {
                0.0
            };
            println!(
                "{} — mid: {:.4}  spread: {:.1}bps\n",
                self.symbol, mid, spread_bps
            );
        }

        println!("  Asks:");
        let ask_rows: Vec<Vec<String>> = self
            .asks
            .iter()
            .rev()
            .map(|l| vec![format!("{:.4}", l.price), format!("{:.4}", l.quantity)])
            .collect();
        crate::output::table::render_table(&["Price", "Quantity"], ask_rows);

        println!("\n  Bids:");
        let bid_rows: Vec<Vec<String>> = self
            .bids
            .iter()
            .map(|l| vec![format!("{:.4}", l.price), format!("{:.4}", l.quantity)])
            .collect();
        crate::output::table::render_table(&["Price", "Quantity"], bid_rows);
    }
}

struct TickerCandle {
    time: i64,
    open: f64,
    close: f64,
    volume: Option<f64>,
}

struct Ticker24hSnapshot {
    prev_day_price: f64,
    change_24h_pct: f64,
    volume_24h_usd: f64,
}

fn map_phoenix_http_err(code: &'static str, err: PhoenixHttpError) -> VulcanError {
    match err {
        PhoenixHttpError::RateLimited {
            retry_after_seconds,
            attempts,
            ref message,
            ..
        } => VulcanError::rate_limit(
            code,
            format!(
                "rate limited after {attempts} attempt(s){}: {message}. \
                 Authenticated requests get a higher budget — \
                 run `vulcan auth login` to sign in, or wait ~30-60s before retrying.",
                retry_after_seconds
                    .map(|s| format!(" (retry_after={s}s)"))
                    .unwrap_or_default()
            ),
        ),
        PhoenixHttpError::Authentication { ref message, .. } => VulcanError::auth(
            "API_AUTH_REQUIRED",
            format!("authentication failed: {message}. Run `vulcan auth login` to sign in."),
        ),
        PhoenixHttpError::RequestFailed(e) => VulcanError::network(code, e.to_string()),
        other => VulcanError::api(code, other.to_string()),
    }
}

fn api_url(ctx: &AppContext, path: &str) -> String {
    format!(
        "{}/{}",
        ctx.config.network.api_url.trim_end_matches('/'),
        path.trim_start_matches('/')
    )
}

async fn fetch_api_json<T: DeserializeOwned>(
    ctx: &AppContext,
    path: &str,
    code: &'static str,
) -> Result<T, VulcanError> {
    let url = api_url(ctx, path);
    let max_attempts: u32 = 3;
    let mut attempt: u32 = 0;
    loop {
        attempt += 1;
        let response = ctx
            .raw_http_client
            .get(&url)
            .send()
            .await
            .map_err(|e| VulcanError::network(code, e.to_string()))?;

        let status = response.status();
        let retry_after_seconds = response
            .headers()
            .get(reqwest::header::RETRY_AFTER)
            .and_then(|v| v.to_str().ok())
            .and_then(|v| v.parse::<u64>().ok());
        let body = response
            .text()
            .await
            .map_err(|e| VulcanError::network(code, e.to_string()))?;

        if status.is_success() {
            return serde_json::from_str(&body).map_err(|e| VulcanError::api(code, e.to_string()));
        }

        if status.as_u16() == 429 {
            if attempt < max_attempts {
                let delay = std::time::Duration::from_secs(
                    retry_after_seconds.unwrap_or(2u64.saturating_mul(attempt as u64)),
                )
                .min(std::time::Duration::from_secs(10));
                tokio::time::sleep(delay).await;
                continue;
            }
            return Err(VulcanError::rate_limit(
                code,
                format!(
                    "{} returned HTTP 429 (rate limited) after {} attempt(s): {}. \
                     Remedy: wait ~30-60s before retrying, \
                     or run `vulcan auth login` for the higher authenticated rate budget.",
                    url, attempt, body
                ),
            ));
        }

        return Err(VulcanError::api(
            code,
            format!("{} returned HTTP {}: {}", url, status, body),
        ));
    }
}

pub(crate) async fn fetch_market_stats(
    ctx: &AppContext,
    symbol: &str,
) -> Result<phoenix_rise::types::MarketView, VulcanError> {
    let symbol_upper = symbol.to_ascii_uppercase();
    fetch_api_json(
        ctx,
        &format!("/market/{symbol_upper}/stats"),
        "MARKET_STATS_FETCH_FAILED",
    )
    .await
}

pub(crate) async fn fetch_market_quote_price(
    ctx: &AppContext,
    symbol: &str,
) -> Result<f64, VulcanError> {
    let stats = fetch_market_stats(ctx, symbol).await?;
    market_quote_price(&stats).ok_or_else(|| {
        VulcanError::api(
            "NO_MARKET_PRICE",
            format!("No mid or mark price available for {}", symbol),
        )
    })
}

fn market_quote_price(stats: &phoenix_rise::types::MarketView) -> Option<f64> {
    stats
        .market
        .l2_orderbook
        .mid
        .or_else(|| stats.market.mark_price.map(|p| p.price))
        .filter(|price| price.is_finite() && *price > 0.0)
}

fn decimal_to_f64(decimal: &phoenix_rise::types::Decimal) -> f64 {
    decimal.ui.parse().unwrap_or_else(|_| {
        let scale = 10f64.powi(decimal.decimals as i32);
        decimal.value as f64 / scale
    })
}

async fn fetch_ticker_candles(
    ctx: &AppContext,
    symbol: &str,
) -> Result<Vec<TickerCandle>, VulcanError> {
    let params =
        CandlesQueryParams::new(symbol.to_ascii_uppercase(), Timeframe::Hour1).with_limit(25);
    let candles: Vec<ApiCandle> = ctx
        .http_client
        .candles()
        .get_candles(params)
        .await
        .map_err(|e| map_phoenix_http_err("TICKER_CANDLES_FETCH_FAILED", e))?;
    Ok(candles
        .into_iter()
        .map(|c| TickerCandle {
            time: c.time,
            open: c.open,
            close: c.close,
            volume: c.volume,
        })
        .collect())
}

fn derive_24h_snapshot(mark_price: f64, mut candles: Vec<TickerCandle>) -> Ticker24hSnapshot {
    candles.sort_by_key(|c| c.time);

    let recent: Vec<&TickerCandle> = candles.iter().rev().take(24).collect();
    let prev_day_price = recent
        .last()
        .map(|c| c.open)
        .filter(|price| price.is_finite() && *price > 0.0)
        .unwrap_or(mark_price);
    let change_24h_pct = if prev_day_price > 0.0 {
        ((mark_price - prev_day_price) / prev_day_price) * 100.0
    } else {
        0.0
    };
    let volume_24h_usd = recent
        .iter()
        .map(|c| c.volume.unwrap_or(0.0) * c.close)
        .sum();

    Ticker24hSnapshot {
        prev_day_price,
        change_24h_pct,
        volume_24h_usd,
    }
}

// ── Execution ───────────────────────────────────────────────────────────

pub async fn execute_list_inner(ctx: &AppContext) -> Result<MarketListResult, VulcanError> {
    let markets = ctx
        .http_client
        .get_markets()
        .await
        .map_err(|e| VulcanError::api("MARKETS_FETCH_FAILED", e.to_string()))?;

    Ok(MarketListResult {
        markets: markets
            .iter()
            .map(|m| MarketSummary {
                symbol: m.symbol.clone(),
                status: format!("{:?}", m.market_status),
                taker_fee: m.taker_fee,
                maker_fee: m.maker_fee,
                max_leverage: m
                    .leverage_tiers
                    .first()
                    .map(|t| t.max_leverage)
                    .unwrap_or(1.0),
                isolated_only: m.isolated_only,
            })
            .collect(),
    })
}

pub async fn execute_info_inner(
    ctx: &AppContext,
    symbol: &str,
) -> Result<MarketInfoResult, VulcanError> {
    let market = ctx
        .http_client
        .get_market(symbol)
        .await
        .map_err(|e| VulcanError::api("MARKET_FETCH_FAILED", e.to_string()))?;

    Ok(MarketInfoResult {
        symbol: market.symbol.clone(),
        status: format!("{:?}", market.market_status),
        market_pubkey: market.market_pubkey.clone(),
        tick_size: market.tick_size,
        base_lots_decimals: market.base_lots_decimals,
        taker_fee: market.taker_fee,
        maker_fee: market.maker_fee,
        funding_interval_seconds: market.funding_interval_seconds,
        funding_period_seconds: market.funding_period_seconds,
        max_funding_rate_per_interval: market.max_funding_rate_per_interval,
        isolated_only: market.isolated_only,
        leverage_tiers: market
            .leverage_tiers
            .iter()
            .map(|t| LeverageTierInfo {
                max_leverage: t.max_leverage,
                max_size_base_lots: t.max_size_base_lots,
            })
            .collect(),
    })
}

pub async fn execute_ticker_inner(
    ctx: &AppContext,
    symbol: &str,
) -> Result<TickerResult, VulcanError> {
    let (stats_result, candles_result) = tokio::join!(
        fetch_market_stats(ctx, symbol),
        fetch_ticker_candles(ctx, symbol)
    );
    let stats = stats_result?;
    let mark_price = stats
        .market
        .mark_price
        .map(|p| p.price)
        .or(stats.market.l2_orderbook.mid)
        .filter(|price| price.is_finite() && *price > 0.0)
        .ok_or_else(|| {
            VulcanError::api(
                "NO_MARKET_PRICE",
                format!("No mark or mid price available for {}", symbol),
            )
        })?;
    let mid_price = stats.market.l2_orderbook.mid.unwrap_or(mark_price);
    let oracle_price = stats
        .market
        .spot_price
        .map(|p| p.price)
        .unwrap_or(mark_price);

    let mut warnings: Vec<String> = Vec::new();
    let candles = match candles_result {
        Ok(c) => c,
        Err(err) => {
            warnings.push(format!(
                "24h stats unavailable ({}): {}. prev_day_price, change_24h_pct, and volume_24h_usd \
                 are degraded to defaults. Remedy: {}",
                err.code,
                err.message,
                remedy_for_code(&err.code),
            ));
            Vec::new()
        }
    };
    let derived = derive_24h_snapshot(mark_price, candles);

    Ok(TickerResult {
        symbol: stats.market.symbol.clone(),
        mark_price,
        mid_price,
        oracle_price,
        prev_day_price: derived.prev_day_price,
        change_24h_pct: derived.change_24h_pct,
        volume_24h_usd: derived.volume_24h_usd,
        open_interest: decimal_to_f64(&stats.market.open_interest),
        funding_rate: stats.market.current_funding_rate_percentage / 100.0,
        warnings,
    })
}

fn remedy_for_code(code: &str) -> &'static str {
    match code {
        "TICKER_CANDLES_FETCH_FAILED" => {
            "candles endpoint rate-limited or unreachable; \
             run `vulcan auth login` for the higher authenticated rate budget, \
             or wait ~30-60s before retrying"
        }
        "MARKET_STATS_FETCH_FAILED" => {
            "market stats endpoint rate-limited or unreachable; \
             wait ~30-60s before retrying"
        }
        "API_AUTH_REQUIRED" => "run `vulcan auth login` to sign in",
        _ => "see error message; retry after the underlying condition clears",
    }
}

pub async fn execute_orderbook_inner(
    ctx: &AppContext,
    symbol: &str,
    depth: usize,
) -> Result<OrderbookResult, VulcanError> {
    let stats = fetch_market_stats(ctx, symbol).await?;
    let book = &stats.market.l2_orderbook;

    let bids: Vec<OrderbookLevel> = book
        .bids
        .iter()
        .take(depth)
        .map(|&(price, qty)| OrderbookLevel {
            price,
            quantity: qty,
        })
        .collect();

    let asks: Vec<OrderbookLevel> = book
        .asks
        .iter()
        .take(depth)
        .map(|&(price, qty)| OrderbookLevel {
            price,
            quantity: qty,
        })
        .collect();

    let spread = match (bids.first(), asks.first()) {
        (Some(b), Some(a)) => Some(a.price - b.price),
        _ => None,
    };

    Ok(OrderbookResult {
        symbol: stats.market.symbol.clone(),
        mid_price: book.mid,
        spread,
        bids,
        asks,
    })
}

pub async fn execute_candles_inner(
    ctx: &AppContext,
    symbol: &str,
    interval: &str,
    limit: usize,
) -> Result<CandlesResult, VulcanError> {
    let timeframe: phoenix_rise::Timeframe = interval
        .parse()
        .map_err(|e: String| VulcanError::validation("INVALID_INTERVAL", e))?;
    let params = phoenix_rise::CandlesQueryParams::new(symbol, timeframe);
    let candles = ctx
        .http_client
        .get_candles(params)
        .await
        .map_err(|e| VulcanError::api("CANDLES_FETCH_FAILED", e.to_string()))?;

    let candles_to_show: Vec<_> = candles.into_iter().rev().take(limit).rev().collect();

    Ok(CandlesResult {
        symbol: symbol.to_string(),
        interval: interval.to_string(),
        candles: candles_to_show
            .iter()
            .map(|c| {
                let secs = if c.time > 1_000_000_000_000 {
                    c.time / 1000
                } else {
                    c.time
                };
                let dt = chrono::DateTime::from_timestamp(secs, 0)
                    .map(|t| t.format("%Y-%m-%d %H:%M").to_string())
                    .unwrap_or_else(|| c.time.to_string());
                CandleRow {
                    time: dt,
                    open: c.open,
                    high: c.high,
                    low: c.low,
                    close: c.close,
                    volume: c.volume,
                }
            })
            .collect(),
        indicators: Vec::new(),
    })
}

/// Attach indicator overlays to a `CandlesResult`. The series share the same
/// time index as `result.candles`, so callers can render them as extra columns.
pub fn attach_indicators(
    result: &mut CandlesResult,
    requested: &[String],
) -> Result<(), VulcanError> {
    use crate::indicators::{IndicatorKind, IndicatorRequest};
    use std::str::FromStr;

    let mut series_vec = Vec::with_capacity(requested.len());
    for name in requested {
        let name = name.trim();
        if name.is_empty() {
            continue;
        }
        let kind = IndicatorKind::from_str(name)?;
        let request = IndicatorRequest::new(kind);
        let period = request.period_or_default();
        let min = kind.min_candles(period);
        if result.candles.len() < min {
            return Err(VulcanError::validation(
                "INDICATOR_WARMUP_INSUFFICIENT",
                format!(
                    "{} needs at least {} candles for period {}, got {}. Increase --limit.",
                    name,
                    min,
                    period,
                    result.candles.len()
                ),
            ));
        }
        let points = crate::indicators::batch::compute_points(&result.candles, &request)?;
        series_vec.push(crate::indicators::IndicatorSeries::from_points(
            kind,
            result.symbol.clone(),
            result.interval.clone(),
            period,
            points,
        ));
    }
    result.indicators = series_vec;
    Ok(())
}

pub async fn execute(ctx: &AppContext, cmd: MarketCommand) -> Result<(), VulcanError> {
    match cmd {
        MarketCommand::List => {
            let result = execute_list_inner(ctx).await?;
            render_success(ctx.output_format, &result, serde_json::Value::Null);
            Ok(())
        }

        MarketCommand::Info { symbol } => {
            let result = execute_info_inner(ctx, &symbol).await?;
            render_success(ctx.output_format, &result, serde_json::Value::Null);
            Ok(())
        }

        MarketCommand::Ticker { symbol } => {
            let result = execute_ticker_inner(ctx, &symbol).await?;
            render_success(
                ctx.output_format,
                &result,
                serde_json::json!({ "command": "market ticker", "symbol": symbol }),
            );

            if ctx.watch {
                crate::watch::watch_loop(
                    ctx,
                    crate::watch::WatchKind::Market(symbol.clone()),
                    || {
                        let symbol = symbol.clone();
                        async move {
                            let result = execute_ticker_inner(ctx, &symbol).await?;
                            render_success(
                                ctx.output_format,
                                &result,
                                serde_json::json!({ "command": "market ticker", "symbol": symbol }),
                            );
                            Ok(())
                        }
                    },
                )
                .await?;
            }
            Ok(())
        }

        MarketCommand::Orderbook { symbol, depth } => {
            let result = execute_orderbook_inner(ctx, &symbol, depth).await?;
            render_success(
                ctx.output_format,
                &result,
                serde_json::json!({ "command": "market orderbook", "symbol": symbol, "depth": depth }),
            );

            if ctx.watch {
                crate::watch::watch_loop(ctx, crate::watch::WatchKind::Orderbook(symbol.clone()), || {
                    let symbol = symbol.clone();
                    async move {
                        let result = execute_orderbook_inner(ctx, &symbol, depth).await?;
                        render_success(
                            ctx.output_format,
                            &result,
                            serde_json::json!({ "command": "market orderbook", "symbol": symbol, "depth": depth }),
                        );
                        Ok(())
                    }
                }).await?;
            }
            Ok(())
        }

        MarketCommand::Candles {
            symbol,
            interval,
            limit,
            with_indicators,
        } => {
            let mut result = execute_candles_inner(ctx, &symbol, &interval, limit).await?;
            if !with_indicators.is_empty() {
                attach_indicators(&mut result, &with_indicators)?;
            }
            render_success(
                ctx.output_format,
                &result,
                serde_json::json!({ "command": "market candles", "symbol": symbol, "interval": interval }),
            );
            Ok(())
        }

        MarketCommand::Trades { symbol, limit } => {
            let _ = (symbol, limit);
            Err(VulcanError::internal(
                "NOT_IMPLEMENTED",
                "market trades not yet implemented (needs WebSocket trades stream)",
            ))
        }

        MarketCommand::FundingRates { symbol, limit } => {
            let _ = (symbol, limit);
            Err(VulcanError::internal(
                "NOT_IMPLEMENTED",
                "market funding-rates not yet implemented (needs history endpoint with no auth)",
            ))
        }
    }
}

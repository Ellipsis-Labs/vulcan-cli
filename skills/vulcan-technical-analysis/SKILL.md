---
name: vulcan-technical-analysis
version: 1.0.0
description: "Technical indicators (SMA, EMA, RSI, MACD, BBands, ATR, VWAP, ADX, Stoch) and trigger evaluation over Phoenix candle history."
metadata:
  openclaw:
    category: "finance"
  requires:
    bins: ["vulcan"]
    skills: ["vulcan", "vulcan-market-intel"]
---

# vulcan-technical-analysis

Use this skill when you need a *derived* read on price — momentum, trend strength, volatility, or mean-reversion — rather than just raw candles. Backed by the `kand` TA library; data comes from the same Phoenix candle endpoint as `vulcan_market_candles`.

## When To Use Which Indicator

| Question | Indicator | Default period | Primary key |
| --- | --- | --- | --- |
| Is price trending up or down? | `sma`, `ema` | 20 | `sma` / `ema` |
| Is momentum overbought or oversold? | `rsi` | 14 | `rsi` |
| Is momentum accelerating or fading? | `macd` | (12/26/9) | `macd`, `signal`, `hist` |
| How volatile is price relative to itself? | `bbands` | 20 | `upper`, `middle`, `lower` |
| What is realized volatility (absolute)? | `atr` | 14 | `atr` |
| Where is the volume-weighted average? | `vwap` | n/a | `vwap` |
| How strong is the trend (any direction)? | `adx` | 14 | `adx` |
| Is short-term momentum at an extreme? | `stoch` | 14 (k_slow=3, d=3) | `k`, `d` |

## Three Tools

### 1. Compute a single indicator

```
vulcan_ta_compute → { symbol: "SOL", indicator: "rsi", timeframe: "1h", period: 14 }
vulcan_ta_compute → { symbol: "SOL", indicator: "macd", timeframe: "4h", params: { fast: 12, slow: 26, signal: 9 } }
vulcan_ta_compute → { symbol: "SOL", indicator: "bbands", timeframe: "1h", params: { dev_up: 2, dev_down: 2 } }
```

Returns the full series aligned to the candle window plus a `summary.verdict` line. Use `summary.latest` for the latest reading.

### 2. Evaluate a trigger spec

```
vulcan_ta_signal → {
  symbol: "SOL",
  spec: { indicator: "rsi", timeframe: "1h", op: "lt", threshold: 30 }
}
```

Ops: `lt`, `lte`, `gt`, `gte`, `crosses_above`, `crosses_below`. The `crosses_*` ops require two consecutive non-NaN values on opposite sides of the threshold. Use the optional `key` field to target a non-primary series (e.g. `"key": "hist"` for the MACD histogram, `"key": "d"` for the Stochastic %D line).

### 3. Bundled report

```
vulcan_ta_report → { symbol: "SOL", timeframe: "1h" }
```

One call returns RSI + MACD + BBands + ATR + ADX. Ideal for the market-intel handoff: a single string per indicator agents can paste into a summary.

## Trigger Patterns

Mean-reversion long entry:

```
spec: { indicator: "rsi", timeframe: "1h", op: "lt", threshold: 30 }
```

Trend-following exit (momentum fading):

```
spec: { indicator: "macd", timeframe: "1h", op: "crosses_below", threshold: 0, key: "hist" }
```

Volatility-expansion pause for a TWAP/grid:

```
# pause if ATR doubles versus where it was when you started
spec: { indicator: "atr", timeframe: "15m", op: "gt", threshold: <2x launch ATR> }
```

Strong-trend gate (avoid mean-reversion entries in trending markets):

```
spec: { indicator: "adx", timeframe: "1h", op: "lt", threshold: 25 }
```

## Notes And Gotchas

- **Warmup**: each indicator needs N candles before it produces a non-NaN value. The tool fetches `max(limit, min_warmup + 5)` candles automatically; if the API returns fewer, you'll see `INDICATOR_WARMUP_INSUFFICIENT` — increase `limit` or use a shorter `period`.
- **Timeframe selection**: 1m/5m are noisy for RSI/MACD; prefer 15m/1h for swing decisions and 4h/1d for trend context.
- **NaN handling**: leading NaNs are expected. Triggers only fire on valid (non-NaN) latest values.
- **For a full TA-driven strategy** with declarative rules (open/close/reduce on conditions like EMA cross, RSI mean-reversion, MACD trend follow), use the `vulcan-ta-strategy` skill — that runner owns the loop, persists state, and handles cooldowns automatically. This skill is for ad-hoc indicator reads and trigger evaluation.

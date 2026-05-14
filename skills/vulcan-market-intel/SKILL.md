---
name: vulcan-market-intel
version: 1.0.0
description: "Ticker, orderbook, candles, market info, and pre-trade analysis patterns."
metadata:
  openclaw:
    category: "finance"
  requires:
    bins: ["vulcan"]
    skills: ["vulcan"]
---

# vulcan-market-intel

Use this skill for:
- Getting current price and funding rate
- Analyzing orderbook depth and spread
- Fetching historical candles
- Pre-trade market research

## List All Markets

```
vulcan_market_list → {}
```

Returns all active perpetual markets with fees, leverage info, and trading status.

## Get Price and Funding Rate

```
vulcan_market_ticker → { symbol: "SOL" }
```

Key fields: mark_price, index_price, funding_rate, volume_24h, change_24h.

## Get Market Configuration

```
vulcan_market_info → { symbol: "SOL" }
```

Key fields: base_lots_decimals, tick_size, taker_fee, maker_fee, leverage_tiers, funding_params.

## Orderbook Analysis

```
vulcan_market_orderbook → { symbol: "SOL", depth: 10 }
```

Key fields: bids, asks, mid_price, spread.

Use this for:
- **Spread check**: Wide spread (>10bps) means higher implicit cost.
- **Slippage estimation**: If order size exceeds liquidity at top levels, expect slippage.
- **Market depth**: How much liquidity is available at each price level.

## Historical Candles

```
vulcan_market_candles → { symbol: "SOL", interval: "1h", limit: 24 }
```

Intervals: `1m`, `5m`, `15m`, `1h`, `4h`, `1d`. Default: `1h`, limit: 50.

## Technical Analysis Overlay

Raw candles tell you what happened; indicators tell you the *shape* of what happened. For trend/momentum/volatility context, prefer one of:

```
vulcan_ta_report   → { symbol: "SOL", timeframe: "1h" }     # RSI + MACD + BBands + ATR + ADX, one call
vulcan_ta_compute  → { symbol: "SOL", indicator: "rsi", timeframe: "1h", period: 14 }
```

Or fold a couple of overlays directly into the candle table with `--with-indicators`:

```
vulcan market candles SOL --interval 1h --limit 50 --with-indicators rsi,macd
```

See `vulcan-technical-analysis` for indicator semantics and trigger patterns.

## Pre-Trade Analysis Pattern

Before placing a trade, gather comprehensive market context:

```
1. vulcan_market_info      → { symbol }    # lot sizes, fees, leverage
2. vulcan_market_ticker    → { symbol }    # current price, funding rate
3. vulcan_market_orderbook → { symbol }    # spread, depth, slippage
4. vulcan_market_candles   → { symbol, interval: "1h", limit: 24 }  # recent price action
5. vulcan_ta_report        → { symbol, timeframe: "1h" }            # momentum + volatility verdict
```

Summarize for the user: current price, 24h change, funding rate, spread, liquidity depth, and the indicator verdict (e.g. "RSI oversold, MACD bearish, ADX 31 → strong trend continuation risk before mean reversion").

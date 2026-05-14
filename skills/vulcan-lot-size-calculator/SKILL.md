---
name: vulcan-lot-size-calculator
version: 1.0.0
description: "Convert desired token amounts to base lots — the most common agent mistake."
metadata:
  openclaw:
    category: "finance"
  requires:
    bins: ["vulcan"]
    skills: ["vulcan"]
---

# vulcan-lot-size-calculator

Use this skill for:
- Converting a desired token amount to base lots before placing an order
- Converting a USD notional to base lots when you need lot-level precision
- Understanding why lot sizes differ per market

## Why This Matters

Vulcan's `size` field is in **base lots**, not tokens or USD. Getting this wrong means trading 100x more or less than intended.

For market orders, prefer `tokens` or `notional_usdc` when you do not need lot-level control. Use this skill when passing `size`, placing limit orders, or checking exact lot math.

## Step-by-Step Calculation

### Step 1: Fetch market info

```
vulcan_market_info → { symbol: "SOL" }
```

Extract `base_lots_decimals` from the response.

### Step 2: Convert tokens to base lots

```
base_lots = desired_tokens * 10^base_lots_decimals
```

### Step 3: Pass base lots to trade tool

```
vulcan_trade → { symbol: "SOL", side: "buy", order_type: "market", size: <base_lots>, acknowledged: true }
```

Market-order shortcut:

```
vulcan_trade → { symbol: "SOL", side: "buy", order_type: "market", tokens: 0.5, acknowledged: true }
vulcan_trade → { symbol: "SOL", side: "buy", order_type: "market", notional_usdc: 100, acknowledged: true }
```

## Worked Examples

### SOL (base_lots_decimals = 2)

| Want | Calculation | Base lots |
|------|-------------|-----------|
| 0.1 SOL | 0.1 * 10^2 = 0.1 * 100 | 10 |
| 0.5 SOL | 0.5 * 100 | 50 |
| 1 SOL | 1 * 100 | 100 |
| 5 SOL | 5 * 100 | 500 |

### BTC (base_lots_decimals = 4)

| Want | Calculation | Base lots |
|------|-------------|-----------|
| 0.001 BTC | 0.001 * 10^4 = 0.001 * 10000 | 10 |
| 0.01 BTC | 0.01 * 10000 | 100 |
| 0.1 BTC | 0.1 * 10000 | 1000 |

### ETH (base_lots_decimals = 3)

| Want | Calculation | Base lots |
|------|-------------|-----------|
| 0.01 ETH | 0.01 * 10^3 = 0.01 * 1000 | 10 |
| 0.1 ETH | 0.1 * 1000 | 100 |
| 1 ETH | 1 * 1000 | 1000 |

## Converting from USD Notional

To trade a specific USD amount via base lots:

1. Get current price: `vulcan_market_ticker → { symbol }`
2. Calculate tokens: `desired_tokens = usd_amount / mark_price`
3. Calculate base lots: `base_lots = desired_tokens * 10^base_lots_decimals`

Example: $100 worth of SOL at $150/SOL, decimals=2:
```
tokens = 100 / 150 = 0.6667
base_lots = 0.6667 * 100 = 66.67 → round to 67
```

For market orders, `notional_usdc` performs this sizing internally and quotes at current mid; actual fill can differ by spread and impact.

## Common Mistakes

1. **Passing token amount as size** — If you want 0.5 SOL and pass `size: 0.5`, you'll get 0.005 SOL (0.5 base lots at decimals=2). Always multiply.

2. **Using the wrong decimals** — Each market has different `base_lots_decimals`. SOL=2, BTC=4, ETH=3. Always fetch fresh from `vulcan_market_info`.

3. **Not rounding** — Base lots must be whole numbers. Round to nearest integer after calculation.

4. **Caching decimals across markets** — Different markets have different decimals. Fetch per-market.

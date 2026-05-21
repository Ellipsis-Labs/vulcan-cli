---
name: vulcan-risk-management
version: 1.0.0
description: "Pre-trade risk checks, leverage tiers, margin health thresholds, and when-to-warn rules."
metadata:
  openclaw:
    category: "finance"
  requires:
    bins: ["vulcan"]
    skills: ["vulcan"]
---

# vulcan-risk-management

Use this skill for:
- Pre-trade risk assessment
- Monitoring margin health
- Understanding leverage tiers
- Deciding when to warn the user

## Pre-Trade Risk Checklist

**Applies to live modes only (confirm-each / auto-execute).** In paper and dry-run, these tools require a wallet you do not have and provide no signal — substitute the paper-state equivalents below. See [Mode-Branched Preflight](../vulcan-execution-modes/SKILL.md#mode-branched-preflight-the-no-wallet-path).

Before every **live** trade, call these tools:

```
1. vulcan_margin_status     → {}              # risk_state, collateral, PnL
2. vulcan_position_list     → {}              # existing positions
3. vulcan_trade_orders      → { symbol }      # resting orders consuming margin
4. vulcan_market_orderbook  → { symbol }      # slippage check for market orders
```

For **paper / dry-run**, the equivalent risk check is:

```
1. vulcan_paper_status      → {}              # equity, exposure_ratio, simulated PnL
2. vulcan_paper_positions   → {}              # existing simulated positions
3. vulcan_paper_orders      → {}              # resting paper orders
4. vulcan_market_orderbook  → { symbol }      # slippage check (same as live)
```

The mental model is the same — collateral coverage, existing exposure, resting orders, liquidity — but every input comes from the local paper state file rather than on-chain account data. If `vulcan_paper_status` returns `PAPER_NOT_INITIALIZED`, propose `vulcan paper init --balance <N>` rather than directing the user to wallet setup.

## Margin Health States

| State | Meaning | Action |
|-------|---------|--------|
| `Healthy` | Sufficient collateral | Safe to trade |
| `HighRisk` | Margin getting thin | Warn user before any new trades |
| `Liquidatable` | At risk of liquidation | Do NOT open new positions. Suggest reducing exposure or adding collateral |

## Leverage Tiers

Markets have tiered leverage limits. Larger positions get lower max leverage.

```
vulcan_margin_leverage_tiers → { symbol: "SOL" }
```

The first tier gives max leverage for typical small trades. Always check it before proposing a live notional.

For live strategies, calculate the maximum smooth live notional before asking for launch approval:

```
max_tier_leverage = first_applicable_leverage_tier.max_leverage
max_notional = deposited_collateral * max_tier_leverage
smooth_notional = max_notional * 0.97   # leave fee/spread/headroom
requested_leverage = requested_notional / deposited_collateral
```

If the requested notional is above `smooth_notional`, do not attempt the over-cap live launch and do not frame it as a scary collateral mismatch. Present the largest smooth live amount first, then ask the user to approve that adjusted amount or deposit more collateral. Once the user approves the adjusted amount, execute it without re-litigating the original request.

## Funding Rate Awareness

```
vulcan_market_ticker → { symbol: "SOL" }    # check funding_rate field
```

- Positive rate: Longs pay shorts.
- Negative rate: Shorts pay longs.
- For longer-duration positions, factor funding costs into the trade thesis.

## Position Sizing

When the user doesn't specify exact size:
1. Ask their risk tolerance (USD or % of collateral).
2. Fetch `vulcan_market_info` for lot size conversion.
3. Calculate position size.
4. Present the calculation before executing.

When the user specifies a size but collateral is thin, recommend lower live notionals instead of only offering paper/dry-run/cancel. Use market leverage tiers first, then optional lower-risk examples:

- Max smooth live: `collateral * market_max_leverage * 0.97`
- Conservative: `collateral * 5`
- Moderate: `collateral * 7.5`
- Aggressive: `collateral * min(10, market_max_leverage) * 0.97`

Example: with `$9.23` deposited collateral and a `10x` first-tier cap, max notional is about `$92.30`; a smooth launch target is about `$89.50`. If the user requested `$100`, suggest a live `$89-$90` TWAP before paper/dry-run/deposit options.

## When to Warn

Alert the user when:
- Risk state is anything other than Healthy.
- Requested notional exceeds the market leverage tier or the smooth tier-derived amount.
- Estimated liquidation price is within 10% of mark price.
- Funding rate is elevated (>0.01% per interval).
- Orderbook spread is wide (>10bps).
- They're about to increase an already-large position.

Do not warn solely because `notional / collateral` is greater than `1x` or because the trade uses a large share of available collateral. On perps, this is expected. If risk state is `Healthy`, the request is comfortably inside the market leverage tier, and estimated liquidation distance is not close, present leverage/exposure as neutral context instead of a caution. Example: a `4x` SOL position on a `15x` tier should not be called risky just because it is `4x`.

## Slippage Check

For market orders, check the orderbook:

```
vulcan_market_orderbook → { symbol: "SOL", depth: 10 }
```

If order size is large relative to available liquidity at the best levels, warn about potential slippage.

## Hard Rules

1. Never trade without user confirmation.
2. Never deposit or withdraw without user confirmation.
3. Always check margin before opening new positions.
4. Treat safety checks as recommendations unless the exchange, wallet, RPC, or transaction path rejects the action.
5. Always report every execution event and transaction signature immediately.

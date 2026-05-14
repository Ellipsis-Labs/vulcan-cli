---
name: vulcan-trade-execution
version: 1.0.0
description: "Execute perpetual futures orders with pre-trade checks and post-trade verification."
metadata:
  openclaw:
    category: "finance"
  requires:
    bins: ["vulcan"]
    skills: ["vulcan", "vulcan-execution-modes", "vulcan-lot-size-calculator", "vulcan-risk-management", "vulcan-error-recovery"]
---

# vulcan-trade-execution

Use this skill for:
- Placing market or limit orders on Phoenix DEX
- Attaching TP/SL to new orders
- Cancelling orders
- The complete safe order flow

Do not use this skill to manually split a scheduled or multi-slice strategy when a Vulcan runner can express it. Route standard TWAP requests to `vulcan-twap-execution` and `vulcan_strategy_twap_start`; use raw `vulcan_trade` loops only for explicitly approved manual fallbacks or unsupported strategy shapes.

## Safe Market Order Flow

### 1. Gather market context

```
vulcan_market_ticker → { symbol: "SOL" }     # current price, funding rate
vulcan_margin_status → {}                     # available collateral, risk state
vulcan_position_list → {}                     # existing positions
vulcan_trade_orders  → { symbol: "SOL" }     # existing resting orders
```

Call `vulcan_market_info` too when using base-lot `size`, placing limit orders, or checking fees/leverage tiers.

### 2. Choose size input

For market orders, provide exactly one of:
- `notional_usdc` — human-friendly USDC sizing, quoted at mid; actual fill differs by spread and impact.
- `tokens` — base-asset amount, with lot conversion handled internally.
- `size` — base lots, for precise control.

When using `size`, extract `base_lots_decimals` from `vulcan_market_info`:
```
base_lots = desired_tokens * 10^base_lots_decimals
```
Example: Want 0.5 SOL, decimals=2 → 0.5 * 100 = 50 base lots.

### 3. Validate against constraints

- Ensure `vulcan_margin_status` shows risk_state = Healthy.
- Check leverage tiers — larger positions have lower max leverage.
- Factor in existing positions (same-side increases exposure, opposite-side reduces).
- For market orders, check `vulcan_market_orderbook` when size may be large relative to visible liquidity and warn about slippage.

### 4. Confirm with user

Present: symbol, direction, size input (`size`, `tokens`, or `notional_usdc`) with approximate token/notional amount, order type, estimated fees, mark price, existing positions, and any TP/SL.

In confirm-each mode, wait for explicit user approval before executing. In auto-execute mode, log the trade details, execute immediately, and still report every order, fill, cancellation, and signature as soon as the agent observes it.

### 5. Execute

```
vulcan_trade → { symbol: "SOL", side: "buy", order_type: "market", notional_usdc: 100, acknowledged: true }
vulcan_trade → { symbol: "SOL", side: "buy", order_type: "market", tokens: 0.5, acknowledged: true }
vulcan_trade → { symbol: "SOL", side: "buy", order_type: "market", size: 50, acknowledged: true }
```

### 6. Verify

```
vulcan_position_list → {}    # confirm position opened
```

Report the transaction signature to the user.

## Market Order with TP/SL

Attach take-profit and/or stop-loss at order time:

```
vulcan_trade → {
  symbol: "SOL",
  side: "buy",
  order_type: "market",
  notional_usdc: 100,
  tp: 160.0,
  sl: 140.0,
  acknowledged: true
}
```

**Direction rules:**
- Long (buy): TP must be above entry, SL must be below entry.
- Short (sell): TP must be below entry, SL must be above entry.

**Constraints:**
- Same-call TP/SL is supported on market orders and cross-margin limit orders.
- Do not rely on same-call TP/SL for isolated limit orders; attach TP/SL after the position exists with `vulcan_trade_set_tpsl` or `vulcan_position_tp_sl`.
- TP/SL only works when opening or extending a position. Fails if the order reduces a position (entire tx rolls back).
- `vulcan_position_show` is the authoritative source for position TP/SL. Conditional trigger legs may also appear in portfolio/order views as reduce-only trigger orders; do not treat them as normal resting limits.

## Limit Orders

```
vulcan_trade → {
  symbol: "SOL",
  side: "buy",
  order_type: "limit",
  size: 50,
  price: 145.00,
  acknowledged: true
}
```

Limit orders rest on the book. They pay maker fees (typically lower). After placing, verify with:

```
vulcan_trade_orders → { symbol: "SOL" }    # confirm order on book
```

## Isolated Margin Orders

For markets requiring isolated margin, or when you want dedicated collateral:

```
vulcan_trade → {
  symbol: "SOL",
  side: "buy",
  order_type: "market",
  size: 50,
  isolated: true,
  collateral: 100.0,
  acknowledged: true
}
```

## Reduce-Only Orders

To ensure an order only reduces (never increases) a position:

```
vulcan_trade → {
  symbol: "SOL",
  side: "sell",
  order_type: "market",
  size: 25,
  reduce_only: true,
  acknowledged: true
}
```

## Cancel Orders

```
vulcan_trade_orders → { symbol: "SOL" }                                              # get order IDs
vulcan_trade_cancel → { symbol: "SOL", scope: "ids", order_ids: ["id1"], acknowledged: true }
vulcan_trade_cancel → { symbol: "SOL", scope: "all", acknowledged: true }            # all orders for one market
vulcan_trade_cancel → { scope: "all-markets", acknowledged: true }                   # all orders across every market
```

## Hard Rules

- Never execute orders without explicit user approval (unless in auto-execute mode).
- Route failures by `.error.category`.
- On `tx_failed`, check position state before retrying.

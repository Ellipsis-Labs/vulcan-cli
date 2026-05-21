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

Mode-agnostic (always safe):

```
vulcan_market_ticker → { symbol: "SOL" }     # current price, funding rate
vulcan_market_info   → { symbol: "SOL" }     # tick_size, base_lots_decimals (needed for size/limit/leverage)
```

Then ask for the execution mode (see [`vulcan-execution-modes`](../vulcan-execution-modes/SKILL.md#how-to-format-the-question)). After the user picks a mode, branch the wallet-bound checks:

- **Paper / Dry-Run** — run `vulcan_paper_status` (handle `PAPER_NOT_INITIALIZED` by proposing `vulcan paper init --balance <N>`). Do **not** call `vulcan_margin_status`, `vulcan_position_list`, `vulcan_trade_orders`, `vulcan_portfolio_*`, or `vulcan_wallet_list`. Paper has its own state file and never resolves a wallet.
- **Confirm-Each / Auto-Execute** — run the live checklist:
  ```
  vulcan_strategy_preflight → { wallet }    # must report READY
  vulcan_margin_status      → {}            # available collateral, risk state
  vulcan_position_list      → {}            # existing positions
  vulcan_trade_orders       → { symbol }    # existing resting orders
  ```

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

- For live modes, ensure `vulcan_margin_status` shows risk_state = Healthy. In paper/dry-run, validate against `vulcan_paper_status.equity` and `exposure_ratio` instead — same intent, no wallet needed.
- Check leverage tiers — larger positions have lower max leverage. (Paper applies a simulated tier from market info; live reads `vulcan_margin_leverage_tiers`.)
- Factor in existing positions (same-side increases exposure, opposite-side reduces). Use `vulcan_paper_positions` in paper, `vulcan_position_list` live.
- For market orders, check `vulcan_market_orderbook` when size may be large relative to visible liquidity and warn about slippage. Safe in any mode.

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

For live modes:
```
vulcan_position_list → {}    # confirm position opened
```

For paper:
```
vulcan_paper_positions → {}  # confirm simulated position opened
vulcan_paper_fills     → {}  # see the simulated fill
```

Report the transaction signature (live) or the paper fill id (paper) to the user.

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

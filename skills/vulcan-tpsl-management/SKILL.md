---
name: vulcan-tpsl-management
version: 1.0.0
description: "Take-profit and stop-loss: direction rules, constraints, and set/cancel flows."
metadata:
  openclaw:
    category: "finance"
  requires:
    bins: ["vulcan"]
    skills: ["vulcan"]
---

# vulcan-tpsl-management

Use this skill for:
- Setting TP/SL when opening a position
- Attaching TP/SL to an existing position
- Modifying or cancelling TP/SL
- Understanding TP/SL constraints and gotchas

## Direction Rules

### Long positions (buy)

- Take-profit price MUST be **above** entry price.
- Stop-loss price MUST be **below** entry price.

### Short positions (sell)

- Take-profit price MUST be **below** entry price.
- Stop-loss price MUST be **above** entry price.

## Setting TP/SL at Order Time

Attach to market orders:

```
vulcan_trade → {
  symbol: "SOL",
  side: "buy",
  order_type: "market",
  size: 50,
  tp: 160.0, sl: 140.0,
  acknowledged: true
}
```

Same-call TP/SL is also supported for cross-margin limit orders. Do not rely on same-call TP/SL for isolated limit orders; use `vulcan_trade_set_tpsl` or `vulcan_position_tp_sl` after the position exists.

**Critical constraint**: TP/SL at order time only works when **opening or extending** a position. If the order **reduces** an existing position, the entire transaction rolls back (the order does not execute either).

## Setting TP/SL on Existing Position

### Method 1: Trade tool

```
vulcan_trade_set_tpsl → { symbol: "SOL", tp: 160.0, sl: 140.0, acknowledged: true }
```

### Method 2: Position tool

```
vulcan_position_tp_sl → { symbol: "SOL", tp: 160.0, sl: 140.0, acknowledged: true }
```

Both auto-detect position side. You can set just TP, just SL, or both.

## Multi-level TP/SL (laddered exits)

Pass `tp_levels` / `sl_levels` instead of `tp` / `sl` to attach multiple
trigger orders at different prices, each closing a portion of the position.
Sum of level sizes per side must not exceed the current position.

```
vulcan_trade_set_tpsl → {
  symbol: "SOL",
  tp_levels: [
    { price: 160.0, size: 0.5 },   # close 0.5 SOL at $160
    { price: 170.0, size: 0.5 }    # close 0.5 SOL at $170
  ],
  sl_levels: [{ price: 140.0 }],   # full position at $140 (only valid as single level)
  acknowledged: true
}
```

Each level accepts `size` (base-asset tokens) or `size_lots` (base lots),
mutually exclusive. Omit both only when there is exactly one level on that
side — it then defaults to the full position.

CLI equivalent:

```
vulcan trade set-tpsl SOL \
  --tp-level 160:0.5 --tp-level 170:0.5 \
  --sl-level 140 \
  --yes
```

## Modifying TP/SL

`set_tpsl` *appends* new trigger orders — it does not replace existing ones.
To change an existing TP/SL, cancel first, then set:

```
vulcan_trade_cancel   → { symbol: "SOL", scope: "tpsl", tp: true, acknowledged: true }
vulcan_trade_set_tpsl → { symbol: "SOL", tp: 165.0, acknowledged: true }
```

## Cancelling TP/SL

```
vulcan_trade_cancel → { symbol: "SOL", scope: "tpsl", tp: true, sl: true, acknowledged: true }
```

Set `tp: true` to cancel take-profit, `sl: true` to cancel stop-loss, or both.

## Viewing TP/SL

TP/SL are **trigger orders**. `vulcan_position_show` is the authoritative source for the position's TP/SL.

```
vulcan_position_show → { symbol: "SOL" }
# Look for: take_profit_price, stop_loss_price
```

Conditional trigger legs may also appear in portfolio or order views as reduce-only trigger orders (for example, `cond:` order IDs). Do not treat those as normal resting limits.

## Common Mistakes

1. **Wrong direction**: TP must be on the profitable side. For longs, TP > entry. For shorts, TP < entry.
2. **Setting on a reduce order**: TP/SL fails if the order reduces a position. Use `vulcan_trade_set_tpsl` on the existing position instead.
3. **Treating trigger legs as normal limits**: If conditional TP/SL legs appear in order views, they are reduce-only trigger orders. Check `vulcan_position_show` for the position-level TP/SL state.

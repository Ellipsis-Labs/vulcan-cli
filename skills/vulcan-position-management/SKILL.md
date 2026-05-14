---
name: vulcan-position-management
version: 1.0.0
description: "List, show, close, reduce positions and manage TP/SL on existing positions."
metadata:
  openclaw:
    category: "finance"
  requires:
    bins: ["vulcan"]
    skills: ["vulcan"]
---

# vulcan-position-management

Use this skill for:
- Viewing open positions
- Closing or reducing positions
- Attaching TP/SL to existing positions
- Monitoring position PnL and liquidation price

## List All Positions

```
vulcan_position_list → {}
```

Returns all open positions across cross and isolated subaccounts. Each row carries: symbol (with ` [iso]` suffix for isolated), side, size, entry price, mark price, unrealized PnL, `unrealized_pnl_pct` (precomputed; isolated uses subaccount collateral as denominator, cross uses entry-time initial margin), `initial_margin`, `maintenance_margin`, `liquidation_price`, and — for isolated only — `subaccount_collateral` and `subaccount_index`.

## Show Position Detail

```
vulcan_position_show → { symbol: "SOL" }
```

Returns detailed info: PnL, margin, liquidation price, TP/SL prices, subaccount info.

## Close Entire Position

```
vulcan_position_close → { symbol: "SOL", acknowledged: true }
```

Closes via market order on the opposite side. Verify with:

```
vulcan_position_list → {}    # confirm position is gone
```

## Close Every Position (Emergency Flatten)

```
vulcan_position_close_all → { acknowledged: true }
```

Closes every open position across all markets and subaccounts (one market order per position). Pair with `vulcan_trade_cancel → { scope: "all-markets", acknowledged: true }` first if you also want to wipe resting orders. Verify with `vulcan_position_list → {}`.

## Reduce Position

Partially reduce a position by a specified size (in base lots):

```
vulcan_position_reduce → { symbol: "SOL", size: 25, acknowledged: true }
```

## Attach TP/SL to Existing Position

Two tools can set TP/SL on an existing position:

### Using position tool (protective trigger orders)

```
vulcan_position_tp_sl → { symbol: "SOL", tp: 160.0, sl: 140.0, acknowledged: true }
```

### Using trade tool (set/modify)

```
vulcan_trade_set_tpsl → { symbol: "SOL", tp: 160.0, sl: 140.0, acknowledged: true }
```

Both auto-detect position side. Direction rules:
- Long: TP > current price, SL < current price.
- Short: TP < current price, SL > current price.

You can set just TP, just SL, or both.

## Cancel TP/SL

```
vulcan_trade_cancel → { symbol: "SOL", scope: "tpsl", tp: true, sl: true, acknowledged: true }
```

Set `tp: true` to cancel take-profit, `sl: true` to cancel stop-loss, or both.

## View TP/SL

TP/SL prices in `vulcan_position_show` are the authoritative position-level state.

```
vulcan_position_show → { symbol: "SOL" }
# Look for take_profit_price and stop_loss_price fields
```

Conditional trigger legs may also appear in portfolio or order views as reduce-only trigger orders. Treat those as implementation details of the protective orders, not normal resting limits.

## Position Management Flow

1. Review positions: `vulcan_position_list`
2. Get details on specific position: `vulcan_position_show → { symbol }`
3. If needed, adjust TP/SL: `vulcan_trade_set_tpsl → { symbol, tp?, sl?, acknowledged: true }`
4. If needed, reduce: `vulcan_position_reduce → { symbol, size, acknowledged: true }`
5. If needed, close: `vulcan_position_close → { symbol, acknowledged: true }`

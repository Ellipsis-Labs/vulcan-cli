---
name: vulcan-grid-trading
version: 1.0.0
description: "Grid trading with layered limit orders across a price range on Phoenix DEX perpetuals."
metadata:
  openclaw:
    category: "finance"
  requires:
    bins: ["vulcan"]
    skills: ["vulcan", "vulcan-execution-modes", "vulcan-trade-execution", "vulcan-lot-size-calculator", "vulcan-risk-management", "vulcan-error-recovery"]
---

# vulcan-grid-trading

Use this skill for:
- Placing a grid of limit orders across a price range
- Profiting from sideways/ranging markets on perpetual futures
- Managing grid state (filled orders, replacements)
- Running a market-making-like strategy

## Core Concept

Grid trading places buy limit orders below the current price and sell limit orders above it at fixed intervals. When a buy fills, a corresponding sell is placed one grid level higher. When a sell fills, a corresponding buy is placed one grid level lower. Profit comes from capturing the spread at each level.

On perpetual futures (Phoenix DEX), this means opening and closing positions at grid levels. Funding rate costs/income also factor into profitability.

Grid maintenance is a long-running loop. Prefer Vulcan's native grid runner (`vulcan strategy grid start` or `vulcan_strategy_grid_start`) when available. It owns the loop, checks live margin feasibility before launch, persists tick logs and level ledgers, supports Paper mode, Plan mode with dry run, Live mode with confirmation required, and Live mode with automatic execution, and records per-level TP/SL intent. Do not write a custom bot or scheduler script if the native runner, MCP tools, or app loop can run the maintenance cycle directly.

Before placing or maintaining a grid, pick one of the execution modes defined in `vulcan-execution-modes` — load that skill for the canonical labels, the question format, and the follow-up checklist per mode. The five modes are Observe (read-only), Paper (`--mode paper`), Dry-Run (`--mode dry_run`), Confirm-Each (`--mode confirm_each`), and Auto-Execute (`--mode auto_execute`). Prefer a structured question UI when available and also ask whether the user wants cross-margin or isolated-margin execution, then collect the per-mode follow-up plus safety guardrails before launch. Cross-margin live grids use batched multi-limit placement; isolated live grids submit individual isolated limit orders and can transfer optional collateral on the first order.

## Grid Parameters

Define with the user before starting:
- **Symbol**: e.g., SOL
- **Price range**: lower bound to upper bound (e.g., 140–160)
- **Grid levels**: number of orders per side (e.g., 5 buy + 5 sell = 10 total)
- **Size per level**: in tokens (agent converts to base lots)
- **Optional TP/SL per level**: either global spacing or explicit custom levels as `PRICE:SIZE_LOTS[:TP][:SL]`

Grid spacing = (upper - lower) / total_levels

## Pre-Grid Checks

```
1. vulcan_market_info      → { symbol: "SOL" }    # base_lots_decimals, tick_size, fees
2. vulcan_market_ticker    → { symbol: "SOL" }    # current price (center the grid)
3. vulcan_market_orderbook → { symbol: "SOL" }    # spread, depth
4. vulcan_margin_status    → {}                    # enough collateral for worst case?
5. vulcan_position_list    → {}                    # existing positions in this market
```

## Calculate Grid Levels

```
spacing = (upper_bound - lower_bound) / total_levels
```

Example: Range 140–160, 10 levels, current price 150:
```
spacing = (160 - 140) / 10 = 2.0
Buy levels:  148, 146, 144, 142, 140
Sell levels: 152, 154, 156, 158, 160
```

Ensure all prices are valid multiples of `tick_size` from `vulcan_market_info`.

## Calculate Size Per Level

```
size_per_level_lots = desired_tokens_per_level * 10^base_lots_decimals
```

## Margin Estimation

Worst case: all buy orders fill (max long position) or all sell orders fill (max short position). Calculate margin required:

```
max_position_lots = size_per_level_lots * levels_per_side
```

Check this against leverage tiers and available collateral.

## Confirm with User

Present the full grid before placing:
- Price range, grid levels, spacing
- Size per level (base lots + token equivalent)
- Total margin required (worst case)
- Estimated fees per round-trip
- Funding rate exposure
- Get explicit approval for the entire grid.

## Place the Grid

Prefer the native runner:

```
vulcan_strategy_grid_start → {
  symbol: "SOL",
  lower_price: 140,
  upper_price: 160,
  levels_per_side: 5,
  tokens_per_level: 0.5,
  take_profit_spacing: 2,
  stop_loss_spacing: 4,
  mode: "paper",
  margin_mode: "cross",
  run_until_stopped: true,
  detached: true
}
```

When the user describes the grid relative to the current price (e.g. "±1% around mark"), prefer `center_on_mark: true` with `width_pct` instead of absolute bounds. The runner re-anchors the grid to the mark at submission time, so price drift between approval and launch doesn't invalidate the run:

```
vulcan_strategy_grid_start → {
  symbol: "SOL",
  center_on_mark: true,
  width_pct: 1.0,
  levels_per_side: 5,
  tokens_per_level: 0.5,
  mode: "paper",
  margin_mode: "cross"
}
```

For raw agent-managed fallback, use `vulcan_trade_multi_limit` to place all grid orders in a single transaction. This is much faster than placing orders individually.

```
vulcan_trade_multi_limit → {
  symbol: "SOL",
  bids: [
    { price: 148.00, size: 50 },
    { price: 146.00, size: 50 },
    { price: 144.00, size: 50 },
    { price: 142.00, size: 50 },
    { price: 140.00, size: 50 }
  ],
  asks: [
    { price: 152.00, size: 50 },
    { price: 154.00, size: 50 },
    { price: 156.00, size: 50 },
    { price: 158.00, size: 50 },
    { price: 160.00, size: 50 }
  ],
  slide: false,
  acknowledged: true
}
```

### Verify all orders placed

```
vulcan_trade_orders → { symbol: "SOL" }
```

## Grid Maintenance Loop

Periodically check for fills and replace completed orders:

Display every maintenance tick with the shared tick contract: strategy, symbol, tick number, mode, timestamp, market snapshot, account or paper state, missing/filled grid levels, planned replacements, guardrail checks or trigger outcomes, execution result, and next tick timing. Live grid maintenance treats missing resting levels as filled and submits flipped replacement orders on later ticks; TP/SL activation remains pending until fills can be mapped authoritatively.

**Launch and monitoring contract for multi-tick MCP runs: see [CONTEXT.md § Strategy Monitoring (Detached Runs)](../../CONTEXT.md#strategy-monitoring-detached-runs).** For long-lived grids, that means `run_until_stopped: true` plus `detached: true`. Grid-specific tick narration:

- Most ticks are no-fill maintenance — send a concise heartbeat per tick, do not go silent.
- Report fills/replacements as compact tables with full transaction signatures.
- Per-level TP/SL is intent only until a bracket action is recorded; never claim it is active just because it was specified in the plan.

Vulcan's grid runner is a local process, not an always-on daemon. If the machine sleeps, the process exits, or networking is unavailable, resting orders remain live but monitoring/replacements stop. `vulcan_strategy_monitor` exposes lifecycle staleness and runner-lock state; if `lifecycle.stale` is true, stop and tell the user to resume/reconcile instead of claiming the grid is maintained.

In live native grid mode, per-level TP/SL is persisted as intent until the runner submits and records an actual TP/SL/bracket action. Never tell the user TP/SL is active just because it was specified in the grid plan.

### 1. Check open orders

```
vulcan_trade_orders → { symbol: "SOL" }
```

Compare against the expected grid. Missing orders = filled.

### 2. Check position

```
vulcan_position_show → { symbol: "SOL" }
```

### 3. Replace filled orders

- For each filled **buy** at price P: queue a **sell** at P + spacing.
- For each filled **sell** at price P: queue a **buy** at P - spacing.

Batch all replacement orders into a single `vulcan_trade_multi_limit` call:

```
vulcan_trade_multi_limit → {
  symbol: "SOL",
  bids: [{ price: <P - spacing>, size: 50 }, ...],
  asks: [{ price: <P + spacing>, size: 50 }, ...],
  slide: false,
  acknowledged: true
}
```

### 4. Check margin health

```
vulcan_margin_status → {}
```

If risk_state is not Healthy, pause grid maintenance and alert user.

### 5. Repeat at regular intervals

Suggested check interval: 60 seconds for cron-style agent loops. Use shorter intervals only when the active agent client explicitly supports sub-minute wakeups. If the user asks for a cadence below the scheduler minimum, stop and ask for explicit approval before rounding or changing the cadence.

## Grid Shutdown

Prefer the native finalize flow for strategy-owned shutdown:

```
vulcan_strategy_finalize → { run_id: "<RUN_ID>", cancel_orders: true, close_position: true, wait: true, acknowledged: true }
```

Fallback manual cleanup cancels all grid orders:

```
vulcan_trade_cancel → { symbol: "SOL", scope: "all", acknowledged: true }
```

Then optionally close any remaining position:

```
vulcan_position_close → { symbol: "SOL", acknowledged: true }
```

## Risk Considerations

- **Trending markets**: Grid trading profits in ranging markets but loses in strong trends. If price drops below the entire grid, you accumulate a large long position at a loss. If price rises above, you're fully short.
- **Price guard**: Grid bounds are the default price guard. Do not add a center-relative drift guard unless the user explicitly asks for one; wide grids are often intentionally far from center at launch.
- **Funding rates**: On perpetuals, holding a position incurs funding payments. Check `vulcan_market_ticker` for the funding rate — a high funding rate can erode grid profits.
- **Margin**: All resting limit orders consume margin. A wide grid with many levels can lock up significant collateral.
- **Slippage on replacement**: Replacement orders may not fill at exactly the grid level if the market moves fast.

## Hard Rules

1. Never place a live grid without explicit user approval for the full grid plan.
2. Always dry-run the grid math and present to user before placing.
3. Check margin status before placing and during maintenance.
4. Cancel the entire grid before adjusting parameters — never leave orphaned orders.
5. Track total grid P&L (sum of all fill spreads minus fees and funding).
6. Set price boundaries — if price moves outside the grid range, pause and alert.
7. Report every placed order, cancellation, fill, maintenance action, and transaction signature immediately.
8. Ask for cross-margin vs isolated-margin before launching a live grid. If isolated, confirm any `isolated_collateral` transfer amount.
9. Do not generate one-off scripts for grid maintenance unless the user explicitly asks for durable/reusable automation or the agent loop cannot safely execute the plan.

---
name: vulcan-ta-strategy
version: 1.0.0
description: "TA-driven strategy runner — declarative rules (Condition → Action) over indicators, price, and boolean composition."
metadata:
  openclaw:
    category: "finance"
  requires:
    bins: ["vulcan"]
    skills: ["vulcan", "vulcan-execution-modes", "vulcan-technical-analysis", "vulcan-risk-management"]
---

# vulcan-ta-strategy

A first-class strategy runner (sibling to TWAP and grid). Each tick the runner evaluates a list of declarative rules and the first one whose condition fires (and is not cooldown-locked) executes its action. Use this skill to express **any** strategy that fits the shape `rules: [{ when: <condition>, do: <action> }, …]`.

## When To Use TA Strategy Vs TWAP / Grid

| Use case | Runner |
| --- | --- |
| "Execute this fixed-size order in N slices over T minutes" | TWAP |
| "Place layered buy/sell orders across a price range" | Grid |
| "Open / close / reduce based on what the chart is doing" | **TA strategy** |

If your strategy is fundamentally about *timing* or *triggers* (EMA cross, RSI mean-reversion, MACD trend follow, Bollinger breakout, multi-confirmation entries), this is the runner.

## CLI

```
vulcan strategy ta start --config-file strategy.json --mode paper --max-ticks 60
vulcan strategy ta start --config-json '{...}' --mode auto-execute --yes
vulcan strategy ta start --config-file strategy.json --run-until-stopped --detached
vulcan strategy ta resume <RUN_ID>
```

Use `vulcan strategy status <run_id>` / `monitor` / `wait-next-tick` / `pause` / `stop` / `finalize` for control — these are strategy-kind-agnostic and work the same as for TWAP/grid.

### Choosing a non-default wallet

Pass the wallet name with the global `-w <name>` flag on every command — **do not** flip the default wallet (`vulcan wallet set-default <name>` / `vulcan wallet use <name>`) just to launch a strategy on a different wallet. Default-flips persist across all future sessions and surprise the next operator.

```
vulcan -w prodguy strategy preflight
vulcan -w prodguy strategy ta start --config-file s.json --mode auto-execute --yes
vulcan -w prodguy strategy status <RUN_ID>
```

Preflight's `Wallet:` line shows the resolved source — `CliFlag` (from `-w`) or `Default` — so you can confirm the right wallet is active before launching.

## MCP

`vulcan_strategy_ta_start` accepts the same config as a structured object. Live modes require `acknowledged: true`. Status/monitor/control tools are shared with TWAP and grid.

**Launch and monitoring contract for multi-tick MCP runs: see [CONTEXT.md § Strategy Monitoring (Detached Runs)](../../CONTEXT.md#strategy-monitoring-detached-runs).** TA-specific tick narration: report which rule fired (or "no rule matched") per tick, plus the indicator values that drove the decision. The runner records both, so the agent's job is to surface them — not to evaluate triggers in agent code.

## Config Shape

```json
{
  "symbol": "SOL",
  "interval_seconds": 60,
  "margin_mode": "cross",
  "max_concurrent_position_tokens": 5.0,
  "max_firings": 50,
  "rules": [
    {
      "name": "entry-long",
      "when": { "all": [ <condition>, <condition>, ... ] },
      "do":   { "kind": "open", "side": "buy", "size": { "tokens": 1.0 } },
      "cooldown": "until_condition_resets",
      "only_if_position": "flat"
    }
  ]
}
```

### `cadence` — When the runner ticks

Two modes. Set one or fall back to the legacy fixed-interval default.

```json
"cadence": { "kind": "fixed", "interval_seconds": 60 }
"cadence": { "kind": "candle_close", "timeframe": "15m", "grace_seconds": 5 }
```

- **`fixed`** — sleep `interval_seconds` between ticks. Mid-bar sampling; backtests and live runs see different bar histories depending on how the schedule lands.
- **`candle_close`** — sleep until the next `timeframe` boundary plus `grace_seconds` (default 5). The runner wakes up *just after* a new bar closes, so every tick sees a freshly settled candle. **Strongly preferred** when your indicators use the same timeframe — strategy logic becomes deterministic and equivalent across paper / dry-run / live / backtest.

If `cadence` is omitted, the legacy `interval_seconds` top-level field is used as `Fixed`. Pick `candle_close` with the *shortest* indicator timeframe in your rule set so all rules see fresh data and you don't burn the rate budget polling stale bars.

### `when` — Conditions

A boolean tree:

- `{ "all": [<c1>, <c2>, ...] }` — AND. Empty `all` is `true`.
- `{ "any": [<c1>, <c2>, ...] }` — OR. Empty `any` is `false`.
- `{ "not": <c> }` — negate.
- `{ "compare": { "left": <v>, "op": "lt|lte|gt|gte|eq", "right": <v> } }` — scalar comparison of two `ValueSource`s.
- `{ "crosses": { "left": <v>, "right": <v>, "direction": "above|below" } }` — `left` crosses above/below `right` on the latest bar. Both sides must produce at least two non-NaN values (so prefer `indicator` for crossing legs; `price` only has a single point).

`ValueSource`:

- `{ "constant": 30.0 }`
- `{ "indicator": { "kind": "rsi", "timeframe": "1h", "period": 14, "params": {...}, "key": "..." } }`
- `{ "price": "close" | "mark" | "mid" }`

Indicator `kind` values: `sma`, `ema`, `rsi`, `macd`, `bbands`, `atr`, `vwap`, `adx`, `stoch`. Key defaults to the indicator's primary (`rsi`, `ema`, `middle`, `atr`, …). For MACD use `key: "macd"` (default), `"signal"`, or `"hist"`. For Stochastic use `"k"` (default) or `"d"`.

### `do` — Actions

Tagged enum with `kind`:

- `{ "kind": "open", "side": "buy"|"sell", "size": <SizeSpec> }` — submit a fresh market order.
- `{ "kind": "close" }` — emit opposite-side market order sized to the current position.
- `{ "kind": "reduce", "fraction": 0.5 }` — close `fraction` of the current position. `0 < fraction <= 1`.
- `{ "kind": "no_op" }` — record-only firing (useful as a watch / debugging rule).

`SizeSpec`:

- `{ "tokens": 1.0 }` — base-asset tokens.
- `{ "lots": 100 }` — explicit base lots.
- `{ "notional": 250.0 }` — USDC notional, converted to lots at the current mark.

### `cooldown`

- `"none"` — fires whenever the condition is true. Use with care.
- `"until_condition_resets"` (default) — fires once, then locked until the condition flips false for one tick.
- `"once_per_run"` — fires at most once.
- `{ "min_ticks": { "ticks": 5 } }` — wait at least N ticks after firing.

### `only_if_position`

Pre-filter: `"any"` (default), `"long"`, `"short"`, `"flat"`, `"not_flat"`. The rule is skipped (without consuming a firing) when the current position state doesn't match.

## Recipe 1 — EMA-cross trend follow

```json
{
  "symbol": "SOL", "interval_seconds": 300,
  "max_concurrent_position_tokens": 5.0,
  "rules": [
    {
      "name": "entry-long",
      "when": {
        "all": [
          { "crosses": {
              "left":  { "indicator": { "kind": "ema", "timeframe": "1h", "period": 9 } },
              "right": { "indicator": { "kind": "ema", "timeframe": "1h", "period": 21 } },
              "direction": "above" } },
          { "compare": {
              "left":  { "indicator": { "kind": "adx", "timeframe": "1h", "period": 14 } },
              "op": "gt",
              "right": { "constant": 20.0 } } }
        ]
      },
      "do": { "kind": "open", "side": "buy", "size": { "tokens": 1.0 } },
      "only_if_position": "flat"
    },
    {
      "name": "exit-on-cross-down",
      "when": { "crosses": {
        "left":  { "indicator": { "kind": "ema", "timeframe": "1h", "period": 9 } },
        "right": { "indicator": { "kind": "ema", "timeframe": "1h", "period": 21 } },
        "direction": "below" } },
      "do": { "kind": "close" },
      "only_if_position": "long"
    }
  ]
}
```

Entries gated by ADX > 20 to avoid taking crosses in chop. Exits fire on the opposite cross.

## Recipe 2 — RSI mean-reversion

```json
{
  "symbol": "SOL", "interval_seconds": 60,
  "max_concurrent_position_tokens": 2.0,
  "rules": [
    {
      "name": "buy-oversold",
      "when": { "compare": {
        "left":  { "indicator": { "kind": "rsi", "timeframe": "15m", "period": 14 } },
        "op": "lt", "right": { "constant": 30.0 } } },
      "do": { "kind": "open", "side": "buy", "size": { "notional": 200.0 } },
      "cooldown": "until_condition_resets",
      "only_if_position": "flat"
    },
    {
      "name": "sell-when-neutral",
      "when": { "compare": {
        "left":  { "indicator": { "kind": "rsi", "timeframe": "15m", "period": 14 } },
        "op": "gt", "right": { "constant": 55.0 } } },
      "do": { "kind": "close" },
      "only_if_position": "long"
    }
  ]
}
```

## Recipe 3 — MACD + ADX confirmation entry, half-out at MACD flip

```json
{
  "symbol": "SOL", "interval_seconds": 300,
  "rules": [
    {
      "name": "entry",
      "when": { "all": [
        { "compare": {
          "left":  { "indicator": { "kind": "macd", "timeframe": "1h", "key": "hist" } },
          "op": "gt", "right": { "constant": 0.0 } } },
        { "compare": {
          "left":  { "indicator": { "kind": "adx", "timeframe": "1h", "period": 14 } },
          "op": "gt", "right": { "constant": 25.0 } } }
      ] },
      "do": { "kind": "open", "side": "buy", "size": { "tokens": 0.5 } },
      "only_if_position": "flat"
    },
    {
      "name": "trim-on-macd-flip",
      "when": { "crosses": {
        "left":  { "indicator": { "kind": "macd", "timeframe": "1h", "key": "hist" } },
        "right": { "constant": 0.0 },
        "direction": "below" } },
      "do": { "kind": "reduce", "fraction": 0.5 },
      "only_if_position": "long"
    }
  ]
}
```

## Live Monitoring — uPnL / Leverage / Liquidation Risk

Every live tick records position risk in the standard tick log and surfaces it in `vulcan strategy report <run_id>`. The "Risk" row on each tick shows:

```
Risk    leverage 2.34x | liq $147.2150 | distance +382bps (+3.82%) [CLOSE] | margin healthy
```

- **Leverage** — `notional / equity`. Cross-margin uses account equity; isolated uses subaccount collateral.
- **Liquidation price** — pulled from the Phoenix positions endpoint. `n/a` in paper mode.
- **Distance to liq** — signed bps from current mark. Positive = mark is on the safe side of liq (above for longs, below for shorts). Annotated `[CRITICAL]` under 200bps, `[CLOSE]` under 500bps, `[moderate]` under 1500bps.
- **Margin state** — Phoenix `risk_state` string (`healthy` / `warning` / `danger` / etc.) or `paper` for paper mode.

The `P/L` row shows uPnL in USDC and as a percentage (when live), plus account equity. The `Live Position Risk` section of the final report consolidates all of this when the run ends with an open position.

Agents should treat `distance < 500bps` or `risk_state != healthy` as a strong signal to add an explicit Close rule or finalize the run.

## Notes And Gotchas

- **Price-drift guardrail is *per tick*, not run-anchored**: live modes call `validate_market_rolling` against the **previous tick's mark**, so `max_price_drift_bps` clamps single-tick gaps / feed glitches and lets multi-bar trends accumulate without tripping. The first tick has no baseline and is skipped. Set the override only to harden against gap risk on a per-tick basis (e.g. `200` = halt on a 2% move in one tick).
- **`slices_total` on `run_until_stopped` runs**: the field is emitted as `u32::MAX` and hidden from `progress N/total` displays. There is no TWAP-style slice plan — use `progress.cumulative_*` and the firings count instead.
- **`position_after_action`**: on ticks where a rule actually fills, a second position snapshot is captured *after* execution and emitted in the tick alongside the pre-action `position`. Use `position_after_action` when you need the post-fill state immediately; `position` remains the view the rule's condition evaluated against.
- **Indicator dedupe**: rules sharing the same `IndicatorRef` only fetch + compute once per tick. Compose freely without worrying about API hits.
- **First-firing-wins**: rule order matters. Place the most specific / highest-priority rules first.
- **Warmup**: each indicator needs N candles before producing values; the runner will fail the tick with `INDICATOR_WARMUP_INSUFFICIENT` if the API returns too few. Bump timeframes from `1m` to `15m`/`1h` if you see this.
- **Position cap**: `max_concurrent_position_tokens` clamps `Open` actions to never push absolute position size past the cap. Opposite-side opens (flip semantics) are NOT clamped — they reduce the existing position.
- **Live mode**: requires `--yes` (or `acknowledged: true` via MCP) plus a wallet password. Use `paper` to validate the rule set first.
- **No bracket TP/SL in v1**: `Open` does not attach TP/SL. Express stops as separate rules with `Close` / `Reduce` gated on price or indicator conditions — that's more flexible than fixed-price stops anyway.
- **Cooldown state persists across resume**: the latest tick's metadata carries the full `CooldownState`, so a paused-then-resumed run continues exactly where it left off.

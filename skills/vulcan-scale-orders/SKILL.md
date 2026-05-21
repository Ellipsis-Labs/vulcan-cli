---
name: vulcan-scale-orders
version: 1.0.0
description: "Scale (laddered) limit orders: place a one-sided multi-limit entry across a price range, then attach laddered TP/SL. Suggests levels from technical analysis when the user does not provide a range."
metadata:
  openclaw:
    category: "finance"
  requires:
    bins: ["vulcan"]
    skills: ["vulcan", "vulcan-execution-modes", "vulcan-trade-execution", "vulcan-lot-size-calculator", "vulcan-tpsl-management", "vulcan-technical-analysis", "vulcan-market-intel", "vulcan-risk-management", "vulcan-error-recovery"]
---

# vulcan-scale-orders

Use this skill for:
- Entering a position with N limit orders distributed across a price range ("scale in" / laddered entry).
- Exiting an existing position with laddered limits ("scale out").
- Attaching laddered TP/SL to the resulting position after fills.
- Generating sensible levels from TA when the user does not specify a range.

Do **not** use this skill for:
- Two-sided market-making with automatic replacement on fill → use `vulcan-grid-trading`.
- Time-sliced execution at the touch / mark price → use `vulcan-twap-execution`.
- Single-shot limit or market orders → use `vulcan-trade-execution`.

## Core Concept

A scale order places `N` post-only limit orders on **one side** between a `lower_price` and `upper_price`. Unlike a grid, there is no replacement loop: once a level fills, that slice of the entry is done. Profit and risk come from the eventual aggregate position, managed afterward with TP/SL.

Typical use cases:
- **Scale long entry**: ladder buys from current price down through a support zone.
- **Scale short entry**: ladder sells from current price up through a resistance zone.
- **Scale exit**: ladder reduce-only sells above current (long) or buys below current (short).

## Required Inputs

Collect with the user before placing:

| Field | Required | Notes |
| --- | --- | --- |
| `symbol` | yes | e.g. `SOL` |
| `side` | yes | `buy` for long entry / short exit, `sell` for short entry / long exit |
| `lower_price`, `upper_price` | yes (or TA-suggested) | If omitted, **derive from TA and ask the user to confirm before placing** — see [Prompting Pattern](#prompting-pattern). |
| `levels` | yes | Number of orders (typical 3–8). |
| `total_tokens` | yes | Total base-asset size across all levels. Converted to base lots per level. |
| `distribution` | optional | `uniform` (default), `front_weighted`, `back_weighted`, `geometric:r=<ratio>`. See [Sizing Distributions](#sizing-distributions). |
| `tp_levels`, `sl_levels` | optional | Laddered exits to attach **after** fills. See [Attaching TP/SL](#attaching-tpsl). |
| `reduce_only` | optional | `true` when scaling out of an existing position. |
| `margin_mode` | optional | `cross` (default for multi-limit) vs `isolated`. Isolated requires per-order placement — see [Isolated Margin](#isolated-margin). |

Before placing, choose a mode via [`vulcan-execution-modes`](../vulcan-execution-modes/SKILL.md) (Paper / Dry-Run / Confirm-Each / Auto-Execute). Scale orders are typically a single batch placement, so Confirm-Each at the batch level (one approval covers the whole ladder) is standard.

**Pick the mode before running any wallet-bound preflight.** The [Pre-Trade Checks](#pre-trade-checks) below branch on mode — in paper or dry-run, never call `vulcan_margin_status`, `vulcan_position_list`, `vulcan_trade_orders`, or any other wallet-resolving tool. See [Mode-Branched Preflight](../vulcan-execution-modes/SKILL.md#mode-branched-preflight-the-no-wallet-path).

## Prompting Pattern

The agent **must** prompt the user for missing or unconfirmed inputs before placing. This is non-negotiable for the **price range** — whether you suggested it from TA or the user gave one. Use the host's structured-question facility if it has one (e.g. `AskUserQuestion`); otherwise ask in plain text and wait for an explicit reply.

### Decision tree

1. **User gave a complete range** (`lower_price` + `upper_price`):
   - Restate it back in the confirmation table ([Confirm With User](#confirm-with-user)).
   - Ask: *"Confirm placing the ladder with `lower=<L>` / `upper=<U>` / `levels=<N>` / `total_tokens=<T>`? (yes / change / cancel)"*
   - Do **not** silently accept; even a user-given range needs an explicit "yes" before placement (unless mode is auto-execute, where the plan is still echoed before placing).

2. **User gave a partial range** (only one bound, only `levels`, only `total_tokens`, only "scale in around mark", etc.):
   - Fill the gaps from TA using the [TA-Based Level Suggestion](#ta-based-level-suggestion-when-user-gives-no-range) recipe.
   - Present the **completed** plan and ask the user to confirm or override each filled-in field.

3. **User gave no range at all** ("scale in to SOL longs", "ladder a short on ETH"):
   - Run the TA suggestion recipe, present the suggestion compactly with the reasoning (which indicator drove each bound), and ask for confirmation.

### Required confirmation question

After producing a suggested or restated plan, ask the user a single, focused question. Example phrasing:

```
Suggested scale-long range for SOL (1h TA):
  upper = 149.80   (mark 150.00 − 0.25·ATR)
  lower = 143.20   (BBand lower; ATR=2.7, recent swing low 142.8)
  levels = 5, spacing = 1.65, distribution = uniform, total = 2.5 SOL

  Sanity: ADX 18 (no strong trend), RSI 42 (neutral) — OK to scale long.

How would you like to proceed?
  1. Confirm as-is and place
  2. Tighten the range (e.g. only down to 145)
  3. Widen the range (e.g. down to BBand-2σ)
  4. Change levels / size / distribution
  5. Cancel
```

If a structured-question tool is available, use these as discrete options. If not, ask in chat and wait. **Never** place the ladder until the user answers option 1 (or supplies a modified plan that you then echo back for one more confirmation).

### What "explicit approval" means

- A bare *"go"*, *"yes"*, *"do it"*, *"place it"*, or *"confirm"* after you presented the full table counts.
- *"Sounds good, but tighten the lower bound to 145"* is **not** approval — restate the modified plan and ask again.
- Silence, an unrelated reply, or an emoji is **not** approval.
- In auto-execute mode, the user's prior consent to auto-execute counts as approval for the placement, but you still echo the full plan immediately before placing and report the tx signature on completion.

### Re-prompt triggers

Even after initial approval, re-prompt the user before placing if **any** of these are now true:
- Mark price moved more than `0.5 × atr` between suggestion and placement (the range may now be silly).
- `vulcan_margin_status.risk_state` is not `Healthy`.
- Existing position size on this symbol changed (new fills landed).
- Any sanity warning newly fired (ADX crossed 30, RSI past extreme).

Show what changed and ask whether to proceed, adjust, or cancel.

## Pre-Trade Checks

Run **mode-agnostic** market checks first — they are read-only and safe in every mode:

```
1. vulcan_market_info      → { symbol }    # base_lots_decimals, tick_size, min lot
2. vulcan_market_ticker    → { symbol }    # current price for centering / TA
3. vulcan_market_orderbook → { symbol }    # confirm side of book is empty above/below your range
```

Then ask the user for the execution mode (per [`vulcan-execution-modes`](../vulcan-execution-modes/SKILL.md#how-to-format-the-question)). Only after the mode is set should you run wallet-bound checks.

**Paper / Dry-Run:** skip the wallet-bound block. The only paper-side check is:

```
vulcan_paper_status → {}     # if PAPER_NOT_INITIALIZED, propose `vulcan paper init --balance <N>`
```

Do not call `vulcan_margin_status`, `vulcan_position_list`, `vulcan_trade_orders`, `vulcan_portfolio_*`, or `vulcan_wallet_list` in paper/dry-run.

**Confirm-Each / Auto-Execute:** also run:

```
4. vulcan_strategy_preflight → { wallet }   # must report READY
5. vulcan_margin_status      → {}           # worst-case margin coverage
6. vulcan_position_list      → {}           # existing exposure in this market
7. vulcan_trade_orders       → { symbol }   # avoid colliding with existing resting orders
```

For buy ladders, `upper_price` must be **at or below** the best ask (or `slide: true` is required). For sell ladders, `lower_price` must be **at or above** the best bid. Otherwise post-only placement rejects.

## TA-Based Level Suggestion (when user gives no range)

When the user says "scale in to SOL longs" with no prices, **propose** a range and confirm. Do not place until they approve.

Recipe:

1. Get current mark from `vulcan_market_ticker`.
2. Get a volatility / trend snapshot from `vulcan_ta_report → { symbol, timeframe: "1h" }`. Note `bbands.upper / .lower`, `atr`, `rsi`, `adx`.
3. Pick the range:
   - **Long entry (`side: buy`)**:
     - `upper_price` = current mark (or `mark − 0.25 × atr` for a small buffer below).
     - `lower_price` = `max(bbands.lower, mark − 2 × atr, recent_swing_low)`.
   - **Short entry (`side: sell`)**:
     - `lower_price` = current mark (or `mark + 0.25 × atr`).
     - `upper_price` = `min(bbands.upper, mark + 2 × atr, recent_swing_high)`.
   - `recent_swing_low / high`: pull from `vulcan_market_candles` over the last 50–100 bars at the chosen timeframe.
4. Pick `levels` from range width:
   - Spacing should be ≥ `0.25 × atr` so fills aren't all clustered into noise.
   - `levels = clamp( round( (upper − lower) / (0.5 × atr) ), 3, 8 )`.
5. Snap every price to the market `tick_size` from `vulcan_market_info`.
6. Sanity gate before suggesting:
   - If `adx > 30` and the range is **against the trend** (e.g. scaling longs into a strong downtrend), flag this and ask the user to confirm — they may be catching a falling knife.
   - If `rsi` already < 25 (long entry) or > 75 (short entry), the range may be too late / too early — surface it.

Present the suggestion compactly:

```
Suggested scale-long range for SOL (1h TA):
  upper = 149.80  (mark 150.00 − 0.25·ATR)
  lower = 143.20  (BBand lower; ATR=2.7, swing low 142.8)
  levels = 5, spacing = 1.65, distribution = uniform
  ADX 18 (no strong trend), RSI 42 (neutral). Confirm to place?
```

## Calculating Levels

```
spacing = (upper_price − lower_price) / (levels − 1)
```

Buy ladder (descending — best price first is closest to current):
```
prices = [ upper, upper − spacing, upper − 2·spacing, …, lower ]
```

Sell ladder (ascending):
```
prices = [ lower, lower + spacing, …, upper ]
```

Every price must be a valid multiple of `tick_size`.

### Sizing Distributions

`tokens_per_level` is then converted to base lots:
```
size_lots = tokens_per_level × 10^base_lots_decimals
```

- **uniform** (default): `tokens_per_level = total_tokens / levels` at every price.
- **front_weighted**: more size at the price closest to current mark — conservative; better fill probability, worse average if the range fully fills.
- **back_weighted**: more size at the far end (lower price for buys, higher for sells) — aggressive; pays off if price runs deep into your range, but tail risk if not all fill.
- **geometric:r=R** (`R != 1`): each level's size is `R ×` the previous level's. `R = 1.5` for back-weighted (long entry into deep support), `R = 1/1.5` for front-weighted. Pick `R` so the sum matches `total_tokens`:
  ```
  s0 = total_tokens × (1 − R) / (1 − R^levels)   # geometric series scaling
  size_i = s0 × R^i                              # i = 0 .. levels−1
  ```

If any computed `size_lots < min_lot_size` from `vulcan_market_info`, reduce `levels` or increase `total_tokens` and re-suggest. Never silently round to zero.

## Margin Estimation

Worst case is **all levels fill** → full `total_tokens` long or short at the average ladder price.

```
avg_fill_price ≈ Σ (price_i × size_i) / Σ size_i
notional      ≈ avg_fill_price × total_tokens
initial_margin = notional / max_leverage_at_this_size
```

Get `max_leverage_at_this_size` from `vulcan_margin_leverage_tiers` for the resulting position size. Confirm `vulcan_margin_status.available_collateral ≥ initial_margin × safety_buffer (1.2×)` before placing.

Route any leverage / margin warnings through [`vulcan-risk-management`](../vulcan-risk-management/SKILL.md).

## Confirm With User

Always present the full plan as a compact table before placing:

```
Scale long SOL — 5 levels, uniform, total 2.5 SOL

  level  price    tokens   lots    cum_tokens
  1      149.80   0.50     50      0.50
  2      148.15   0.50     50      1.00
  3      146.50   0.50     50      1.50
  4      144.85   0.50     50      2.00
  5      143.20   0.50     50      2.50

  Worst-case avg fill ≈ 146.50, notional ≈ $366.25
  Worst-case initial margin ≈ $73 (5× tier), available collateral $480 ✓
  No TP/SL attached at order time — laddered TP/SL will be set post-fill.
```

Wait for explicit approval. In auto-execute mode, log the same plan immediately, then place, and report the tx signature as soon as it lands.

## Place The Ladder

Single batch placement, post-only:

```
vulcan_trade_multi_limit → {
  symbol: "SOL",
  bids: [
    { price: 149.80, size: 50 },
    { price: 148.15, size: 50 },
    { price: 146.50, size: 50 },
    { price: 144.85, size: 50 },
    { price: 143.20, size: 50 }
  ],
  asks: [],
  slide: false,
  acknowledged: true
}
```

- For a **sell** ladder, put the orders in `asks` and leave `bids` empty.
- `slide: false` enforces strict post-only at the requested price — preferred for scale entries. Use `slide: true` only when the user explicitly accepts repricing to the top of book on collision.
- `vulcan_trade_multi_limit` accepts optional per-leg `tp` and `sl` on each bid/ask entry. When any leg carries tp/sl the submission expands to N individual limit-order instructions bundled into a single transaction (each with its own bracket); `slide` is ignored on that path. See [Attaching TP/SL](#attaching-tpsl).

Verify placement immediately:

```
vulcan_trade_orders → { symbol: "SOL" }
```

Report every accepted order plus the full transaction signature.

## Attaching TP/SL

`vulcan_trade_multi_limit` supports **native per-leg TP/SL** via optional `tp` and `sl` on each bid/ask entry. When any leg carries tp/sl the call still submits in **one transaction** (Phoenix's multi_limit instruction cannot bundle conditionals itself, so the SDK expands the batch to N individual `place_limit_order_with_conditionals` ixs in the same tx). `slide` is ignored on that path; entries are post-only.

Three TP/SL models exist for a scale ladder:

- **Per-leg brackets** (default, recommended) — each ladder leg ships with its own `tp` / `sl` in the same `vulcan_trade_multi_limit` tx. One signature, one tx, each leg's bracket sized to that leg's base lots and armed when that leg fills.
- **Position staircase** — place the ladder with no per-leg tp/sl, then attach `tp_levels` / `sl_levels` on the resulting position via `vulcan_trade_set_tpsl`. The exit fires off mark vs. the **aggregated** position size, so this is the right choice when you want one unified staircase across the new ladder and any pre-existing position in the same symbol.
- **Deferred** — no TP/SL during entry. Only acceptable when the far end of the ladder is itself the hard invalidation. Risky and discouraged.

### Choosing the model

Before placing, if TP/SL is in scope, pick using this rule and confirm with the user:

- No existing position in this symbol → **per-leg brackets**.
- Adding to an existing position and the user wants one unified exit ladder → **position staircase**.
- User explicitly accepts running without an SL during entry and the bottom of the ladder is their invalidation → **deferred**.

If you ask the user, present the three options in that order with one-line descriptions; do not re-introduce the A/B/C labels.

### Per-leg brackets (default)

Pass `tp` and `sl` on each bid/ask entry. One transaction, one signature, each leg's bracket sized to that leg's base lots:

```
vulcan_trade_multi_limit → {
  symbol: "SOL",
  bids: [
    { price: 149.80, size: 50, tp: 157.29, sl: 145.31 },
    { price: 148.15, size: 50, tp: 155.56, sl: 143.71 },
    { price: 146.50, size: 50, tp: 153.83, sl: 142.11 },
    { price: 144.85, size: 50, tp: 152.09, sl: 140.50 },
    { price: 143.20, size: 50, tp: 150.36, sl: 138.90 }
  ],
  asks: [],
  acknowledged: true
}
```

Per-leg prices typically use a fixed offset from each leg's entry, e.g.:

```
tp_for_this_leg = price × (1 + tp_pct)         # long ladder
sl_for_this_leg = price × (1 − sl_pct)
```

Constraints and gotchas:

- **Cross margin only.** Multi_limit (with or without brackets) rejects isolated-only markets. For isolated-margin scale entries, fall back to per-order `vulcan_trade` placement and accept the N-transaction cost.
- **`slide: true` is ignored when any leg has tp/sl.** The expanded per-leg limit ixs use Phoenix's standard post-only placement; explicit slide isn't exposed by the limit-order builder.
- **No explicit-size brackets on limits.** The SDK rejects `BracketLegSize::BaseLots` on limit-leg brackets (`UnsupportedLimitBracketLegSizing`); the on-chain `max_size` defaults to the leg's base lots, which is what you want here.
- **TP/SL roll-back rule still applies.** If a bracket would reduce (rather than open or extend) the position when it fires, that trigger rolls back. Per-leg brackets are safe for the entry phase but never for scale-out — use the **position staircase** `tp_levels` instead.
- **No partial-fill brackets.** If a leg only partially fills, the bracket still arms for the full leg size, which can exceed what actually filled. Re-fetch `vulcan_position_show` after the tx lands and reconcile.
- **Brackets only cover the new ladder.** If a prior position in the same symbol already exists, per-leg brackets do not cover it. Use the **position staircase** instead when you want one unified exit.

Confirm direction rules from [`vulcan-tpsl-management`](../vulcan-tpsl-management/SKILL.md) for every computed pair before placing.

Report the tx signature and the final per-leg table as soon as the multi-limit lands; surface each fill (and any bracket trigger) immediately — never batch the updates.

### Position staircase (when adding to an existing position)

Place the ladder with multi_limit (no per-leg tp/sl), wait until **at least one level fills** (position exists), then attach laddered TP/SL on the position:

```
vulcan_trade_set_tpsl → {
  symbol: "SOL",
  tp_levels: [
    { price: 158.0, size: 1.0 },   # close 1.0 SOL at $158
    { price: 165.0, size: 1.0 },   # close 1.0 SOL at $165
    { price: 175.0, size: 0.5 }    # runner — last 0.5 SOL at $175
  ],
  sl_levels: [{ price: 138.0 }],   # full position at $138
  acknowledged: true
}
```

Constraints (from [`vulcan-tpsl-management`](../vulcan-tpsl-management/SKILL.md)):
- Sum of `tp_levels` sizes must not exceed current filled position size. As more entry levels fill, you may need to **append** additional TP slices with another `vulcan_trade_set_tpsl` call (`set_tpsl` appends; it does not replace).
- Long ladder → TP above entry, SL below. Short ladder → TP below entry, SL above.

#### Direction sanity check

Before calling `vulcan_trade_set_tpsl`, re-fetch position side and entry price:

```
vulcan_position_show → { symbol: "SOL" }
```

This avoids attaching wrong-direction TP/SL if the user changed plans between ladder placement and fill.

### Deferred TP/SL (discouraged)

Simpler but riskier — the position is unprotected while levels fill. Only acceptable if:
- The user explicitly accepts running without SL during the entry, **and**
- The full `lower_price` (or `upper_price`) is itself a hard invalidation level the user is willing to ride down to.

If the user picks deferred, set a wakeup or schedule a follow-up check; do not just walk away.

## Scale-Out (reduce-only)

To scale **out** of an existing long, use a sell ladder with `reduce_only: true`. `vulcan_trade_multi_limit` does **not** accept per-order `reduce_only`. Two options:

1. **Prefer**: use multi-level TP via `tp_levels` on the existing position (see [Position staircase](#position-staircase-when-adding-to-an-existing-position)) — these are reduce-only trigger orders by construction and survive position-side changes safely.
2. **Fallback**: if the user wants resting **limit** sells (not triggers) above market for a long, place them individually:
   ```
   for price in ladder:
     vulcan_trade → {
       symbol, side: "sell", order_type: "limit",
       price, size: <lots>, reduce_only: true, acknowledged: true
     }
   ```
   Slower than `multi_limit` but the only way to get reduce-only resting limits today.

Default to option 1 unless the user explicitly asks for resting reduce-only limits.

## Isolated Margin

`vulcan_trade_multi_limit` submits a single cross-margin transaction. For isolated-margin scale entries, fall back to per-order placement:

```
for (price, lots) in ladder:
  vulcan_trade → {
    symbol, side, order_type: "limit",
    price, size: lots,
    isolated: true,
    collateral: <only on the first order>,
    acknowledged: true
  }
```

Report each placed order and signature as it lands — do not batch the updates.

## Monitoring & Cleanup

Scale orders have no replacement loop. After placement:

1. Watch fills with `vulcan_position_show → { symbol }` and `vulcan_trade_orders → { symbol }`.
2. If you chose the position staircase, top up `tp_levels` as fills come in so unprotected size never grows. (Per-leg brackets handle this automatically — each bracket arms with its own leg.)
3. If the user changes their mind mid-fill, cancel remaining levels with:
   ```
   vulcan_trade_cancel → { symbol, scope: "all", acknowledged: true }
   ```
   then either close the partial position (`vulcan_position_close`) or attach the agreed TP/SL.
4. If the range fully fills, confirm the final laddered TP/SL covers the **entire** position size — sum mismatches between filled size and `tp_levels` total are a common bug.

Report every fill, cancellation, and signature immediately — never batch into an end-of-run summary.

## Hard Rules

1. Never place a scale ladder without explicit user approval of the full level table (unless in auto-execute mode, which still requires immediate reporting).
2. Always validate against `tick_size` and minimum lot size from `vulcan_market_info` — never silently round to zero on small far-end slices.
3. Always compute worst-case (all levels fill) margin before placing.
4. When TA-suggesting a range, surface trend / overbought-oversold warnings (`adx > 30` against trend, `rsi` past extreme) and ask the user to confirm.
5. Buy ladders sit at or below the best ask; sell ladders at or above the best bid — otherwise use `slide: true` only with explicit approval.
6. **Default to per-leg brackets.** `vulcan_trade_multi_limit` supports native per-leg TP/SL via optional `tp` / `sl` on each entry — one tx, one signature, each leg's bracket sized to that leg's base lots. Use the **position staircase** (`vulcan_trade_set_tpsl` with `tp_levels` / `sl_levels`) only when adding to an existing position you want managed under one unified exit ladder. Re-fetch position side with `vulcan_position_show` before any `set_tpsl` call. Never claim TP/SL is active until it is recorded on the position.
7. Sum of `tp_levels` sizes must never exceed currently filled position size; top up incrementally as more entry levels fill.
8. Route errors by `.error.category` via [`vulcan-error-recovery`](../vulcan-error-recovery/SKILL.md). On `tx_failed`, re-fetch `vulcan_trade_orders` before retrying — partial placement is possible.

---
name: vulcan-portfolio-intel
version: 1.1.0
description: "Full portfolio snapshot: cross + isolated margin, positions with uPnL %, resting orders, funding exposure, and optional daily/weekly performance."
metadata:
  openclaw:
    category: "finance"
  requires:
    bins: ["vulcan"]
    skills: ["vulcan"]
---

# vulcan-portfolio-intel

Use this skill when the user asks any of:

- "what's my portfolio / account / position state"
- "check my portfolio / margin / pnl / positions"
- "how am I doing on Phoenix / Vulcan"
- daily / weekly recap requests

Always load this skill first for portfolio questions — do not improvise the layout.

## Active Wallet

The snapshot reflects exactly one wallet at a time, resolved in this priority:

1. `ctx.session_wallet` — the MCP session wallet unlocked at server start (`VULCAN_WALLET_NAME`, falling back to the configured default).
2. `--wallet <name>` override (CLI only).
3. Default wallet from `vulcan wallet set-default`.

Within that wallet, the snapshot covers cross-margin (subaccount 0) **and** every isolated subaccount. If the user has multiple wallets, the active one is whatever the MCP server was started with; call `vulcan_status` if you need to confirm `wallet.source` and `wallet.public_key`.

## Primary Call

Prefer the combined snapshot. It returns margin (cross), positions (all subaccounts), resting orders (cross), isolated subaccount summaries, and account totals in one call:

```text
vulcan_portfolio → {}
```

Use granular calls only for a single section or symbol-level filtering:

```text
vulcan_margin_status  → {}          # cross-margin only
vulcan_position_list  → {}          # all subaccounts; includes uPnL % and initial margin per position
vulcan_trade_orders   → { symbol? }
```

## Optional Performance Context

When the user asks for daily, weekly, or historical performance — not for every portfolio check — pull realized PnL history:

```text
vulcan_history → { type: "pnl", limit: 50 }
```

Sum fills since the cutoff (24h or 7d) to report realized PnL. If history is empty or the call fails, say so explicitly; do **not** invent a number. Open-position uPnL is **not** daily/weekly performance — keep them separate.

For agent-session activity, use:

```text
vulcan://agent/position-report
vulcan://agent/session-summary
```

State the basis for any "win rate" figure — currently open-position win rate from unrealized PnL, not realized closed-trade history.

## Field Map

`vulcan_portfolio` returns:

- `margin` (cross subaccount 0):
  - `collateral_balance` — USDC parked in cross.
  - `effective_collateral` — collateral + discounted uPnL + unsettled funding.
  - `portfolio_value` — collateral + uPnL (cross only).
  - `unrealized_pnl`, `initial_margin`, `maintenance_margin`.
  - `risk_state` (Safe / AtRisk / Cancellable / Liquidatable / BackstopLiquidatable / HighRisk).
  - `risk_tier`.
  - `available_to_withdraw`.
  - `num_positions`, `num_open_orders`.
- `totals` (cross + every isolated):
  - `total_collateral`, `total_account_value`, `total_unrealized_pnl`, `num_subaccounts`.
  - **Always prefer `total_account_value` over cross-only `portfolio_value`** when reporting "account value" — otherwise collateral parked in isolated subaccounts is invisible.
- `isolated_subaccounts[]`:
  - `subaccount_index`, `symbol`, `collateral_balance`, `effective_collateral`, `portfolio_value`, `unrealized_pnl`, `initial_margin`, `maintenance_margin`.
- `positions[]`:
  - `symbol` (suffix ` [iso]` for isolated), `side`, `size`, `entry_price`, `mark_price`.
  - `unrealized_pnl`, `unrealized_pnl_pct` — already computed using the phoenix-v2-frontend formula (see below).
  - `initial_margin`, `maintenance_margin`, `liquidation_price`.
  - `subaccount_index`, `subaccount_collateral` — present only for isolated.
- `orders[]` — resting limit orders plus conditional TP/SL trigger legs.

## Unrealized PnL % (Phoenix-v2-frontend formula)

The field is precomputed; trust `unrealized_pnl_pct`. For reference, the formula is:

- **Isolated:** `uPnL / subaccount_collateral`.
- **Cross:** `uPnL / entry_initial_margin`, where `entry_initial_margin = initial_margin * entry_price / mark_price` (rebases current initial margin back to entry).

Returned as a signed percent string, e.g. `+1.45%`, `-3.20%`, or `—` when the denominator is zero / missing.

## Presentation Template

Use this exact layout. Adapt only when sections are empty (skip them). Use the listed emojis; do not substitute.

```text
📊 Portfolio Snapshot — <wallet name or short pubkey>

🏦 Account
- Risk: <risk_state> / <risk_tier>      [🟢 Safe • 🟡 AtRisk/Cancellable • 🔴 Liquidatable/HighRisk]
- Total account value: $<total_account_value>   (cross + isolated)
- Total collateral:    $<total_collateral>
- Total uPnL:          <signed $total_unrealized_pnl>
- Free to withdraw:    $<available_to_withdraw>   (cross)
- Cross initial / maintenance margin: $<initial_margin> / $<maintenance_margin>

📈 Positions (<n>)
| Market | Side | Size | Entry | Mark | uPnL | uPnL % | Liq |
| ...    | ...  | ...  | ...   | ...  | ...  | ...    | ... |

For each isolated row also note: `iso collateral $X · init margin $Y` underneath the table or in a follow-up bullet.

🧾 Open Orders (<n>) — group by symbol
- <symbol>: <side> <size> @ <price> (<type>)            ← regular limit
- <symbol>: TP / SL <side> <size> @ <price> (reduce-only) ← conditional trigger

💸 Funding (only when a position has non-trivial funding exposure)
- <symbol>: <rate>%/8h — <"longs pay shorts" or "shorts pay longs"> · costs/credits you ~$X/day at current size

📅 Performance (only when user asked for daily/weekly, or when realized PnL > 0 was observed this session)
- 24h realized PnL: <$value>  (from vulcan_history type=pnl)
- 7d realized PnL:  <$value>

🛡️ Notes
- Flag missing TP/SL on positions where the user has not opted out.
- Flag tight liquidation distance (< 5% from mark).
- Do **not** flag laddered TP/SL whose sizes sum to ≤ position size — that is intentional.
```

Rules:

- The "Account" block uses `totals.total_account_value`, not `margin.portfolio_value`. The cross `portfolio_value` is misleading when isolated subaccounts hold collateral.
- Always show `uPnL %` next to `uPnL` in the positions table.
- For each isolated position, surface its `subaccount_collateral` and `initial_margin` — those values explain the liquidation price and the cross/total gap.
- Funding section is conditional. Skip it for positions where `|funding_rate| < 0.001` AND there's no large exposure. Use `vulcan_market_ticker` per held symbol when including funding.
- Performance section is conditional. Skip it unless the user asked or there was a meaningful realized event.

## Risk State Decoration

Map `risk_state` to one indicator emoji:

- `Safe` → 🟢
- `AtRisk` / `Cancellable` → 🟡
- `Liquidatable` / `BackstopLiquidatable` / `HighRisk` → 🔴

Use the same emoji once next to the risk line; do not pepper it through the rest of the snapshot.

## Reduce-Only TP/SL — Do The Math Before Warning

Multiple reduce-only TP or SL orders on the same position are almost always a **laddered exit**, not a bug:

1. Sum sizes of reduce-only TP orders. Same for SL.
2. Compare to position size.
3. Only flag if a side's sum **exceeds** position size (excess no-ops due to reduce-only) or direction is wrong (long SL above entry, long TP below entry, etc.).

A long 2.00 SOL with two reduce-only SL sells of 1.00 each → sums to 2.00 → exact match → valid laddered SL, **do not warn**.

## When To Hand Off

- User asks to close a position → `vulcan-position-management` (confirm first, orderbook check for large closes).
- User asks to add/withdraw collateral, transfer between subaccounts → `vulcan-margin-operations`.
- User wants to open a new position → `vulcan-trade-execution` + `vulcan-risk-management`.

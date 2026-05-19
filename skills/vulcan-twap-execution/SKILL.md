---

name: vulcan-twap-execution
version: 1.0.0
description: "Execute large orders as time-weighted slices to reduce market impact on Phoenix DEX."
metadata:
  openclaw:
    category: "finance"
  requires:
    bins: ["vulcan"]
    skills: ["vulcan", "vulcan-execution-modes", "vulcan-trade-execution", "vulcan-lot-size-calculator", "vulcan-risk-management", "vulcan-error-recovery"]

---

# vulcan-twap-execution

Use this skill for:

- Breaking a large order into smaller time-spaced slices
- Reducing market impact and slippage on size
- Executing over minutes or hours
- Tracking average fill price across slices

## Core Concept

Time-Weighted Average Price (TWAP) splits a large order into N equal slices executed at regular intervals. The goal is an average fill price close to the time-weighted market average, reducing the impact a single large order would have on the book.

Vulcan has a first-class TWAP strategy runner. Prefer it when the user wants a standard TWAP:

```bash
vulcan strategy twap start --symbol SOL --side buy --notional-usdc 1000 --slices 5 --interval-seconds 30 --mode paper -o json
```

The strategy runner owns the loop, supports sub-minute foreground cadence, checks live margin feasibility before launch, prints each tick during execution, writes structured tick logs and slice ledgers under `~/.vulcan/strategy-runs`, and can produce `strategy monitor`, `strategy status`, and `strategy report` outputs.

**Launch and monitoring contract for multi-tick MCP runs: see [CONTEXT.md § Strategy Monitoring (Detached Runs)](../../CONTEXT.md#strategy-monitoring-detached-runs).** Do not restate or interpret that contract here — apply it directly.

TWAP-specific output behavior:

- CLI table mode prints each tick to stdout while the run is active.
- CLI JSON mode keeps stdout as the final JSON report and prints tick summaries to stderr while the run is active.
- Every tick is a fill (or a `dry_run` placeholder). Report each as a compact Markdown table row with full transaction signature, fill, cumulative filled size/notional, position/exposure, and next tick. Lead with execution progress, not equity.
- Do not launch a multi-minute live TWAP as one blocking MCP call unless the user explicitly asks for final-only output.

Use the Vulcan runner for standard market-order TWAPs. Use the agent client's looping/background execution only for observe-only workflows, unsupported strategy shapes, or explicitly requested manual workflows. If a requested live mode or strategy shape is unsupported, do **not** treat that as permission to self-drive live slices with raw `vulcan_trade` calls. Offer supported runner modes, a reduced/manual plan, or ask for an explicit typed instruction requesting a manual live fallback. Do not write a custom scheduler script if the Vulcan runner, agent terminal, MCP tools, or app loop can execute the slices directly.

Before starting, pick one of the execution modes defined in `vulcan-execution-modes` — load that skill for the canonical labels, the question format, and the follow-up checklist per mode. The five modes are Observe (read-only), Paper (`--mode paper`), Dry-Run (`--mode dry_run`), Confirm-Each (`--mode confirm_each`), and Auto-Execute (`--mode auto_execute`). Use a structured question UI when available. If the user selects Auto-Execute, treat that answer as permission to run within the stated parameters. In Paper mode, use the runner or MCP paper tools; do not use live trade tools.

Always collect strategy controls before launch:

- **Additional triggers**: ask whether the TWAP should pause or stop on price/mark levels, drift bps, funding-rate changes, TA signals, time windows, PnL/liquidation-distance conditions, or no additional triggers. The built-in TWAP runner enforces configured drift/exposure/notional guardrails and execution failures; unsupported trigger shapes must be monitored by the agent while it waits for ticks, with the action explicitly recorded as pause or stop.
- **Safety parameters**: ask for TP/SL intent, max leverage or exposure ratio, max total notional, max per-slice notional, max price drift, and isolated/collateral preference when relevant. Map max leverage to `max_exposure_ratio` for runner launch. If the user wants TP/SL and the TWAP runner cannot attach it inline for the selected shape, plan to set TP/SL with `vulcan_trade_set_tpsl` or `vulcan_position_tp_sl` after the live position exists.

For live `confirm_each` or `auto_execute`, check live-agent readiness before market/risk prechecks or final launch confirmation:

```bash
vulcan agent live-ready --target claude --scope user -o json
vulcan agent live-ready --target cursor --scope project -o json
```

`live-ready` is a read-only readiness check; it does not install or modify MCP config. Use the target for the current agent client. If readiness returns `not_ready`, stop and show its `resolution_summary`, `install_command`, and `restart_instructions`. Do not continue to final live confirmation until the readiness route is `mcp` or the user explicitly chooses CLI env fallback.

For live modes, show the selected wallet name/address and the exact TWAP parameters. If the user already selected live `auto_execute` and accepted any risk recommendation, do not ask for another final typed approval just to satisfy the agent client. Agent-driven live execution should prefer MCP installed via `vulcan agent mcp install --target <…> --scope user --dangerous` with a pre-unlocked session wallet; `vulcan agent mcp doctor` returns the exact `manual_install_command` for the current target. If Phoenix API auth is missing or expired, Vulcan may refresh it automatically when the wallet is already unlocked; this shares the API session but does not replace wallet unlock for transaction signing.

Do **not** tell the user to run a live TWAP manually with a `! vulcan strategy ...` command just because the agent shell cannot answer a wallet password prompt. That turns an agent execution flow into a user-operated CLI flow and loses MCP/session-wallet benefits. On `WALLET_PASSWORD_REQUIRED` or non-interactive live auth failure: if `vulcan agent mcp doctor` shows MCP is already configured with `password_env_present: true`, the fix is to restart the agent client. If MCP is not configured (or dangerous/password env is missing), run `vulcan agent mcp install --target <current-agent> --scope user --dangerous` outside the agent session and restart the client. `live-ready` is for checking, not installing. If dangerous MCP install is blocked by the agent client safety classifier, do not retry or bypass it; ask the user to run the install command from a regular shell and restart the client. CLI fallback is acceptable only if the user explicitly sets `VULCAN_WALLET_PASSWORD` before the command starts.

Safety checks and risk checks are recommendations for the user and inputs to the visible report. They should not block a user-approved live `auto_execute` run at a smooth tier-derived size. The runner may still fail or pause for actual execution failures, missing wallet signing, exchange/RPC errors, or transaction rejection.

If an external agent-client safety classifier blocks a live strategy call despite approval, do not repeat the full risk case, do not mention a "collateral mismatch" after the amount was adjusted, and do not re-offer paper as the default escape hatch. Ask for the shortest possible confirmation tied to the already-approved smooth amount, e.g. "Reply `yes` and I will retry the approved $90 SOL TWAP on wallet onboard3."

## Parameters

Agree on these with the user before starting:

- **Symbol**: e.g., SOL
- **Side**: buy or sell
- **Total size**: in tokens or notional USDC. Vulcan converts this to integer base lots before execution; display the planned base lots, executable base lots, per-tick executable notional/tokens, and any rounding/catch-up note.
- **Slices**: number of child orders (e.g., 5-10)
- **Interval**: time between slices (e.g., 60s, 300s). Do not silently round this.
- **Run label**: short user-visible name for the loop, used in logs/status messages.
- **Advisory thresholds for `auto_execute`**: max total notional, max per-slice notional, max price drift bps, max exposure ratio, and reconciliation attempts.

## Cadence Constraints

The Vulcan TWAP runner supports sub-minute foreground intervals. If using `vulcan strategy twap start`, do not apply an agent scheduler's cron minimum to the requested interval.

Only compare the requested interval with the active agent client's minimum cadence when using an agent scheduler fallback instead of the Vulcan runner. If the requested interval is below that scheduler minimum, stop and ask for an explicit user choice before creating any job or executing the first slice.

For example, if the user asks for 30s but the available cron loop only supports 60s, say that 30s is unavailable in this client and ask whether to:

- Proceed at 60s.
- Change the slice count/notional for a 60s cadence.
- Use a manual foreground loop if the client can safely wait between ticks.
- Cancel.

Do not say "confirm with you" and then proceed. Do not create the cron job until the user explicitly accepts the changed cadence. Treat a cadence change as a strategy parameter change that invalidates the prior approval.

## Pre-TWAP Checks

```
MCP:
1. vulcan_market_info      → { "symbol": "SOL" }    # base_lots_decimals, fees
2. vulcan_market_ticker    → { "symbol": "SOL" }    # current price, volume
3. vulcan_market_orderbook → { "symbol": "SOL" }    # depth — is there enough liquidity per slice?
4. vulcan_margin_status    → {}                      # enough collateral for total position?
5. vulcan_position_list    → {}                      # existing exposure

CLI:
1. vulcan market info SOL -o json
2. vulcan market ticker SOL -o json
3. vulcan market orderbook SOL -o json
4. vulcan margin status -o json
5. vulcan position list -o json
```

## Collateral-Based Alternatives

If live collateral is thin for the requested notional, compute the largest smooth live notional from deposited collateral and the market leverage tier before asking for approval. Do not only offer paper, dry-run, or cancel.

Use this pattern:

- `market_max_leverage = first_applicable_leverage_tier.max_leverage`
- `requested_leverage = requested_notional / deposited_collateral`
- `max_live_notional = deposited_collateral * market_max_leverage`
- `smooth_live_notional = max_live_notional * 0.97`
- Optional lower-risk examples: `deposited_collateral * 5`, `deposited_collateral * 7.5`, and `deposited_collateral * min(10, market_max_leverage) * 0.97`

Example with `$9.23` collateral, a `10x` first-tier cap, and a `$100` request:

- Requested: `$100` notional, about `10.8x`.
- Max tier notional: about `$92.30`.
- Smooth live TWAP: about `$89-$90`, leaving fee/spread/headroom.

When presenting choices, include a live reduced option before paper/dry-run:

1. Run the smooth live amount, e.g. `$89-$90` based on collateral and market leverage.
2. Deposit more collateral, then run the original amount.
3. Paper mode with original amount.
4. Dry-run original amount.
5. Cancel.

Do not attempt a live notional that is above the tier-derived smooth amount unless the user explicitly insists after seeing that the exchange may reject later slices. Prefer a smooth adjusted launch over a risky launch narrative that triggers external agent classifiers.

Do not add generic risk warnings solely because `notional / collateral` is greater than `1x`, or because the TWAP uses a large share of collateral, when margin state is healthy and the requested notional is comfortably within the market leverage tier. For example, a `4x` SOL TWAP on a `15x` tier is normal leverage context, not a warning. Warn only for concrete liquidation or margin risk: non-healthy risk state, tier/smooth-notional exceedance, close liquidation distance, elevated funding, wide spread, or already-large existing exposure.

If the user accepts a reduced live notional that the agent suggested, treat that as the current intent. Do not later say the reduced amount conflicts with the original larger request, and do not require the user to restate the full strategy. A short confirmation is enough if the agent client requires user-authored text.

Launch wording should be concise and deterministic: "Launching approved live auto_execute TWAP: SOL buy, $90 notional, 10 slices x 60s, wallet onboard3." Avoid adding fresh high-risk commentary immediately before the tool call; it can trigger external classifiers after the user already approved the adjusted live amount.

## Calculate Slice Size

```
total_base_lots = total_tokens * 10^base_lots_decimals
slice_lots = total_base_lots / slices    # round to integer
```

Example: 5 SOL over 5 slices, decimals=2:

```
total_base_lots = 5 * 100 = 500
slice_lots = 500 / 5 = 100 base lots per slice
```

Verify: `slice_lots * slices` should equal `total_base_lots`. Vulcan's internal TWAP runner does this lot-aware planning itself: it converts token or USDC-notional inputs to market-accepted integer base lots, spreads those lots across ticks, records executable base lots/notional/tokens in the ledger, and uses later ticks to catch up when earlier ticks rounded down, subject to `max_step_notional_usdc`.

## Confirm with User

Present before starting:

- Total: 5 SOL (500 base lots) or $2,000 notional across 5 slices
- Per slice: 1 SOL (100 base lots) or $400 notional
- Lot-aware executable plan: show requested size, planned base lots, executable base lots, estimated executable total notional/tokens, per-tick executable notional/tokens, and any total rounding delta.
- Interval: 60 seconds
- Estimated total fees: `total_base_lots * price * taker_fee * 2` (if round-trip)
- Starting position and current notional exposure before the TWAP.
- Default wallet name and address for live modes.
- Execution mode using the user-facing label and raw mode value.
- Additional triggers, including the exact pause/stop action or "none".
- Safety parameters: TP/SL plan, max leverage/exposure ratio, max total notional, max per-slice notional, and max drift.
- For `auto_execute`, show the configured safety policy as hard guardrails. Vulcan can block launch or pause execution on margin infeasibility, notional limits, drift/exposure violations, wallet signing failures, transaction failures, exchange/RPC rejection, or unrecoverable execution errors.
- Get explicit approval to begin the TWAP.
- If the cadence was adjusted because of scheduler limits, get explicit approval for the adjusted cadence before scheduling or running slice 1.

## Market Order TWAP Loop

Use `vulcan strategy twap start` or `vulcan_strategy_twap_start` for standard market-order TWAPs. The runner owns the loop, tick display, ledger, and report.

### Per-Tick Render Checklist (Required)

This checklist is the canonical per-tick render contract for TWAP and is binding on every executed slice. It is enumerated in Hard Rule 12 below; future edits to this skill must not drop or condense this section.

Every tick must surface, in this order:

1. **Header**: strategy, symbol, slice number (e.g. `3/10`), mode, timestamp.
2. **Market snapshot**: mark, mid, funding rate, and bps drift from the start mark (`max_price_drift_bps` guardrail context).
3. **Planned vs executable slice**: even target notional/tokens, executable base lots, executable notional/tokens, and any rounding or catch-up note.
4. **Execution result**: fill price, fill tokens, fill notional, fee, full tx signature (or paper fill ID). Never abbreviate the signature.
5. **Cumulative TWAP progress**: slices filled, cumulative tokens, cumulative notional, VWAP, cumulative fees.
6. **Current position**: side, total tokens/lots, mark, position notional, uPnL (USDC and %), exposure ratio / leverage-equivalent (`position_notional / equity_or_collateral`).
7. **Account health (secondary line)**: equity/collateral and available funds.
8. **Next tick**: remaining slices, next fire time, and pause/cancel handle.

Lead with execution progress over account equity — equity blends fees and mark-to-market movement, so it is misleading as a headline number.

### Progress Surface (Task-List Render)

In addition to the inline 8-field block above, render strategy-run progress as a **client-native task checklist** when the host agent exposes one. This is the visible "running TWAP… checklist crossed out per tick" surface, and it must coexist with the 8-field message body — the task list is the *progress indicator*, the message body is the *tick report*.

Capability detection — apply the first option your client supports:

1. **Claude Code (and other clients with `TaskCreate`/`TaskUpdate`/`TaskList`)**: create one task per planned slice immediately after a successful strategy start, before the first `wait_next_tick` call. Use `subject: "Tick K/N — <symbol> <side> ~$X"` and `activeForm: "Filling tick K/N"`. Transition each task `pending → in_progress` right before its `wait_next_tick` poll, and `in_progress → completed` once the tick lands. On pause or stop, mark the still-`pending` tasks as `deleted` (not `completed`) so the strikethrough doesn't lie about what filled. The harness loading animation surrounds the in-flight `wait_next_tick` call automatically — no separate work needed.
2. **Clients with a TodoWrite-style tool**: same pattern, one todo per slice, mark each as completed as it lands.
3. **Clients with structured plan/step UIs (e.g. some IDE agents)**: emit one step per slice and transition step state on each tick.
4. **Plain MCP/CLI clients with no task surface**: the canonical 8-field message body above is the entire surface. Do not synthesize a fake task list inside the message body when a native one would have rendered.

The task-list render is independent of strategy mode — apply it for paper, dry-run, confirm-each, and auto-execute alike. For grid and TA strategies that don't have a fixed slice count, render one task per *planned firing window* or *expected order layer* instead, and document any open-ended runs as a single "Monitoring <strategy>" task that completes on stop.

### Drift Reporting

Report price drift from the start mark as guardrail context. The runner enforces `max_price_drift_bps`; stricter or differently shaped drift triggers can still be monitored by the agent.

Example trigger semantics:

```json
{ "type": "drift_bps_above", "bps": 100, "action": "pause" }
```

Without an extra trigger, say "mark moved 42 bps from start" and continue if execution succeeds and runner guardrails pass. If the user configured an agent-monitored trigger that the runner does not enforce natively, evaluate it after each wait/status result and issue `vulcan_strategy_pause` or `vulcan_strategy_stop` when it fires.

### Execution

For live runner modes, execute with `vulcan strategy twap start` or MCP `vulcan_strategy_twap_start`. In MCP, set `detached: true` by default. Do not manually call `vulcan_trade` for each slice unless the user explicitly requested a manual fallback or the built-in runner cannot express the strategy.

For each executed slice, report: run label, slice number, timestamp, fill price, fill tokens, fill notional, fee, and tx signature or paper fill ID.

The runner waits the agreed interval and repeats until all slices complete, the user stops it, or an actual execution error occurs.

### Agent Tick Logging

The MCP launch and monitoring contract lives in [CONTEXT.md § Strategy Monitoring (Detached Runs)](../../CONTEXT.md#strategy-monitoring-detached-runs). For CLI execution paths:

1. **CLI table mode**: relay each printed tick.
2. **CLI JSON mode**: relay each stderr tick summary while preserving stdout JSON for the final report.
3. **Long background run**: if the client supports background terminals, start the runner in the background and periodically read/relay the output. Keep the run ID visible.

Use `vulcan_strategy_pause` for a resumable pause and `vulcan_strategy_stop` for permanent stop.

## Manual Fallback Loop

Only use this section if the Vulcan TWAP runner cannot express the request or the user explicitly asks for a manual live fallback. This is an agent-managed loop, not the first-class runner path.

For each slice:

1. Re-read ticker/orderbook as needed.
2. Place the slice with `vulcan_trade` or `vulcan paper buy/sell`.
3. Record fill details and signatures.
4. Wait the agreed interval.
5. Repeat until target fill is reached.

Example live market slice:

```json
vulcan_trade → { "symbol": "SOL", "side": "buy", "order_type": "market", "notional_usdc": 25, "acknowledged": true }
```

Manual fallback loops must keep their own run label, start watermark, fill counter, and stop/cancel handle because they do not get the runner's ledger for free.

## Limit-Order TWAP Variant

The first-class TWAP runner is a market-order runner today. A limit-order TWAP is an agent-managed fallback: use it only when the user explicitly asks for limit-order slicing or accepts this non-runner path.

Use limit orders at the current best bid/ask for potentially better fills (maker fees):

### 1. Read current price

```
vulcan_market_ticker → { symbol: "SOL" }
```

### 2. Place limit order at or near the ask (for buys)

```
vulcan_trade → { symbol: "SOL", side: "buy", order_type: "limit", size: 100, price: <current_ask>, acknowledged: true }
```

### 3. Wait, then check fill status

```
vulcan_trade_orders → { symbol: "SOL" }
```

### 4. If unfilled after interval, cancel and place next slice

```
vulcan_trade_cancel → { symbol: "SOL", scope: "ids", order_ids: ["<id>"], acknowledged: true }
```

Then adjust price and place next slice. Track unfilled volume to add to remaining slices.

## Tracking Average Fill

After all slices, compute the volume-weighted average price:

```
vulcan_position_show → { symbol: "SOL" }
```

The `entry_price` field reflects the average across all fills for the position.

For first-class runner flows, use the Vulcan strategy run ledger/report as the source of truth for per-slice tracking. The agent may keep short working notes, but should not duplicate the full ledger.

When using a manual fallback and paper fills as the loop counter, do not count all historical fills unless the paper state was reset for this run. Capture a start watermark before the first tick, such as the current fill count, last fill ID, and start timestamp, then count only fills after that watermark for the target symbol/side/run.

Final reports should include these columns when available: slice, time, fill ID or tx signature, price, tokens, notional, fee, cumulative tokens, cumulative notional, VWAP, mark, position notional, uPnL, and exposure ratio. If the output would be too wide, split it into an execution table and a final position/account summary.

## Handling Errors Mid-Loop

- **On `network` error**: Pause, retry the current slice after backoff.
- **On `tx_failed`**: Check position state before retrying. See `vulcan-error-recovery` skill.
- **On `rate_limit`**: Wait and retry. A 60s interval between slices should avoid rate limits.
- **Never skip a slice on error** — pause the loop, diagnose, then resume with `vulcan strategy resume <RUN_ID>` or the TWAP alias `vulcan strategy twap resume <RUN_ID>`.

## Hard Rules

1. Each TWAP session requires human approval before the first slice.
2. In live `confirm_each`, get explicit approval for the run and surface stricter reconciliation/status reporting. In `auto_execute`, log every slice and continue within the approved parameters unless execution fails.
3. Track cumulative fill volume — stop if total exceeds target (handle partial fills from limit orders).
4. On any error, pause the loop rather than skipping the slice.
5. Price drift is a runner guardrail through `max_price_drift_bps`; stricter or differently shaped drift triggers should pause only when the user configured that trigger.
6. Report every slice, fill, dry-run action, paper fill ID, and transaction signature immediately.
7. Present final summary: total filled, average price, total fees, time elapsed.
8. Do not generate one-off scripts for the loop unless the user explicitly asks for durable/reusable automation or the agent loop cannot safely execute the plan.
9. If using a recurring scheduler, store and report the job ID/run label on every tick and delete the job immediately after completion or pause.
10. The Vulcan runner supports sub-minute foreground cadence. Apply a 60-second practical minimum only to agent-managed scheduler fallbacks that cannot wake faster.
11. Never round a user-requested cadence up or down and continue automatically. Ask first, wait for the answer, then schedule.
12. Every executed slice must surface the full eight-field [Per-Tick Render Checklist](#per-tick-render-checklist-required) above — header, market snapshot with drift, planned vs executable, execution result with full tx signature, cumulative progress, position, account health, and next tick. Do not condense or skip fields when the data is available from the tick log.
13. When the host client exposes a task/checklist tool (e.g. `TaskCreate`/`TaskUpdate`, TodoWrite, plan steps), render strategy-run progress as a [task-list checklist](#progress-surface-task-list-render) alongside the 8-field message body — one task per planned slice, transitioning `pending → in_progress → completed` per tick. Clients without that surface stay on the canonical 8-field block alone; do not fabricate a fake checklist inline.


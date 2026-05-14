# Strategy Runner Template

Use this template when adding a Vulcan-owned strategy such as grid, TA, or volatility-based execution.

## Required Pieces

1. **Config**: define a typed strategy config with `run_id`, `run_label`, wallet context, symbol(s), mode, cadence, and `StrategySafetyPolicy`.
2. **Plan builder**: create a `StrategyRunLedger` before any execution. Every intended step should have a stable index, scheduled time, planned size/notional, and initial `planned` status.
3. **Preflight**: fetch market/account state and run shared checks from `strategy::safety` before every live step.
4. **Execution**: use `strategy::execution` helpers for paper, dry-run, and live market execution. Add strategy-specific order helpers only when required.
5. **Reconciliation**: use `strategy::reconcile` for live history matching. If a live transaction signature is returned and the position/order state is consistent with submission, continue the runner and reconcile history in the background; do not re-fire the same step because the history indexer is late.
6. **Tick emission**: after every executed or terminal step, call `strategy::runner::emit_strategy_tick`. Do not write ticks directly. The shared emitter appends JSONL ticks, records agent action log events, prints table output through the strategy renderer, and prints JSON-mode diagnostics to stderr.
7. **Persistence**: write ledger updates after every status change and write a summary report at pause/completion.
8. **Resume**: load the ledger, verify wallet/config match, skip completed steps, and continue from the next incomplete step.
9. **Agent contract**: expose an MCP tool schema, update `agents/tool-catalog.json`, and document mode support, safety policy, tick/report fields, terminal statuses, and pause/stop controls.

## Agent Monitoring Contract

Detached strategy agents should use this low-context loop:

1. Store `run_id` and `last_tick_seen = 0` immediately after launch.
2. Call status with `since_tick = 0` once after launch to backfill any tick emitted during startup.
3. Report every execution tick as a compact table. Include the full transaction signature; do not abbreviate it.
4. Update `last_tick_seen` after each reported tick, then call `wait_next_tick(after_tick = last_tick_seen)` only when actively waiting for the next expected tick.
5. Use `monitor` for non-blocking checkpoints and heartbeat/stale checks. Do not request the full ledger unless stale, failed, final, or debugging.
6. On user shutdown, use `finalize` with explicit cleanup flags instead of hand-rolled cancel/close sequences.

Preferred execution tick table:

| Tick | Fill | Tx | Cumulative | Position | Next |
| --- | --- | --- | --- | --- | --- |
| `3/10` | `buy 0.11 SOL @ $97.20 = $10.69` | full transaction signature | `$32.09 / 0.33 SOL` | `1.29x exposure, healthy` | `19:01:55Z` |

## Safety Defaults

- `paper` and `dry_run` must never require wallet signing.
- `confirm_each` and `auto_execute` are dangerous and must require `--yes` or MCP `acknowledged=true`.
- `auto_execute` should persist the approved safety policy as advisory context. Do not block solely on risk recommendations once the user approved auto-execute; wallet signing failures, transaction failures, exchange/RPC rejection, or unrecoverable execution errors can still stop the run.
- Strategy launch prompts should use user-facing mode labels: Paper mode, Live mode with confirmation required, Live mode with automatic execution, Plan mode with dry run, and Observe only when supported.
- Strategy launch prompts must ask for additional triggers and safety parameters before launch. Include price/mark, drift, funding, TA, time, PnL/liquidation-distance triggers, TP/SL intent, max leverage or exposure, notional caps, per-step caps, and whether triggers pause or stop.
- Prefer pause over retry unless retry behavior is explicit and bounded in the policy.
- Multi-tick MCP runs must support detached start, mandatory status polling until `completed`, `paused`, `stopped`, or `failed`, plus pause and stop control requests.

## Shared Modules

- `vulcan-lib/src/strategy/types.rs`: shared JSON contracts.
- `vulcan-lib/src/strategy/runner.rs`: ledger lifecycle helpers and mandatory tick emission.
- `vulcan-lib/src/strategy/execution.rs`: paper, dry-run, and live execution helpers.
- `vulcan-lib/src/strategy/reconcile.rs`: history matching helpers.
- `vulcan-lib/src/strategy/safety.rs`: reusable live guardrails.

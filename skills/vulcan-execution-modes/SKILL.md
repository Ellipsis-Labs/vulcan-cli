---
name: vulcan-execution-modes
version: 1.0.0
description: "Canonical taxonomy of Vulcan's execution modes (Observe / Paper / Dry-Run / Confirm-Each / Auto-Execute), with explicit instructions for when to ask the user which mode, how to format the question, and what follow-up to collect per mode. Load whenever the user asks 'should this be paper or live?' or before any strategy/trade launch."
metadata:
  openclaw:
    category: "finance"
  requires:
    bins: ["vulcan"]
    skills: ["vulcan", "vulcan-risk-management"]
---

# vulcan-execution-modes

Every Vulcan action that creates or modifies state runs in exactly one **execution mode**. The mode picks: whether real funds are touched, whether the wallet is unlocked for signing, whether the user approves each fill, and which safety guardrails apply. Always pick the most conservative mode that solves the problem — never default silently to live.

The mode is the combination of: (a) MCP server args (whether `--allow-dangerous` is on the registered server invocation), (b) the `--mode` flag on strategy/trade commands, and (c) the safety overrides for that launch. Each mode below is a coherent bundle of all three.

## The Five Execution Modes

| Mode | `--mode` value | Real funds? | Wallet unlocked? | Per-action approval? | Use when |
| --- | --- | --- | --- | --- | --- |
| **Observe** | (no flag) | No | No | n/a | Reading market data, indicators, history, portfolio snapshots; no execution intent |
| **Paper** | `paper` | No (simulated balance) | No | No | Validating a strategy or trade against live market prices before risking funds |
| **Dry-Run** | `dry_run` | No | No | No | Inspecting what a live run *would* do — no fills, no submission |
| **Confirm-Each** | `confirm_each` | Yes | Yes | Yes — per slice / per firing | First live runs of a validated strategy, small size, tight guardrails |
| **Auto-Execute** | `auto_execute` | Yes | Yes | No — pre-authorized within stated bounds | Strategy you've run successfully in confirm-each, sized to acceptable loss, running unattended |

### Mode details

**Observe.** Default state. `vulcan mcp` started without `--allow-dangerous` filters dangerous tools out of `tools/list`; what remains is read-only. Strategy starts are visible but you should not launch any strategy from this mode — wait for an explicit user mode choice first.

**Paper.** Live Phoenix market prices, local simulated balance, no real funds touched. Every order, fill, position, and PnL is simulated. Initialize once per session with `vulcan paper init --balance <N>`. Strategy runners passed `--mode paper` route execution through the paper engine.

**Dry-Run.** Like paper but without simulation either. The strategy runner walks its tick loop, computes `planned_action` for each tick, writes it to the ledger, but never calls the paper or live executor — `tx_signature` and `fill_*` fields stay `None`. Useful for inspecting how a config sizes lots against the current orderbook, or for reviewing the tick cadence + rule firings without committing to anything. Not a graduation step on the path to live; after dry-run, go back to paper.

**Confirm-Each.** Real funds, wallet unlocked. Every dangerous tool call requires `acknowledged: true`; strategy runners pause for explicit per-slice acknowledgement and Vulcan reconciles each fill against authoritative trade history. Pair with tight `max_step_notional_usdc` and `max_total_notional_usdc` guardrails. **Always run `vulcan strategy preflight` first and confirm READY before launching.**

**Auto-Execute.** Real funds, no per-fill approval. Strategy runs autonomously within its configured guardrails until completion, max-ticks, max-firings, or an explicit `vulcan strategy stop`. The agent's role shifts to monitor + alert + stop on anomalies. **Use a dedicated sub-wallet** whose collateral equals the maximum acceptable loss for the run.

## When To Ask The User Which Mode

**The execution-mode question is mandatory and non-skippable.** Silently picking a mode — including Paper — is a bug, not a safe default. If the session is operating under any "don't ask clarifying questions" / "make the reasonable call" / "just do it" directive (Claude Code Fast Mode, custom instructions, or user shorthand earlier in the conversation), that directive **does not apply** to the execution-mode question. Stop and ask. The user can always answer in one word, but they must answer — Paper vs live is a financial decision that belongs to them, not you.

**Always ask before**:

- Any strategy start (`vulcan_strategy_twap_start`, `vulcan_strategy_grid_start`, `vulcan_strategy_ta_start`).
- Any individual trade that would touch real funds (`vulcan_trade`, `vulcan_position_close`, `vulcan_position_reduce`, margin moves).
- Resuming a paused strategy run (`vulcan_strategy_resume`) when the run's prior mode was live — re-confirm.
- Finalizing a run with cleanup flags that submit real transactions (`vulcan_strategy_finalize --cancel-orders` or `--close-position`).

**Never ask before**:

- Read-only market, portfolio, indicator, history, status queries — they're already Observe mode by construction.
- Paper-mode tools (`vulcan_paper_*`) — non-destructive by definition.
- `vulcan_strategy_preflight` — that command is the *answer* to "are we ready for live?", not a launch.
- Inspecting an existing run (`status`, `monitor`, `report`, `wait_next_tick`).
- Stopping or pausing a running strategy (`stop`, `pause`) — those are safety actions, never less safe than the current state.

**Re-ask when**: the user changes wallet, the strategy config materially changes (sizing, max firings, guardrails), or a session resumes after >15 minutes of inactivity. Do not infer a prior mode answer carries forward across these events.

## How To Format The Question

Prefer a structured question UI (e.g. AskUserQuestion in Claude Code) over open prose — agents pick answers more reliably from a numbered list than they do parsing freeform replies. Use these exact labels in this exact order so the choice looks identical across sessions and across strategies:

```
Q: Which execution mode for this <action>?

1. Paper          — simulated, real prices, no funds touched
2. Dry-Run        — plan inspection, no fills, no submission
3. Confirm-Each   — live; you approve every fill
4. Auto-Execute   — live; runs autonomously within stated guardrails

Default: Paper
```

For brand-new users or strategies the user hasn't run before, the default suggestion is **Paper**. For strategies the user has run successfully in confirm-each multiple times, the suggested default may be Confirm-Each — do not auto-suggest Auto-Execute as default; require explicit selection.

If the user types an unambiguous shorthand (`paper`, `live`, `confirm`, `auto`), accept it. Treat ambiguous live-related phrases ("just run it", "go for it", "make the reasonable call") as **not enough** — re-ask with the structured options. A "Default: Paper" label in the prompt is a *suggestion*, not consent; never proceed on the default without an affirmative answer.

## What To Ask After The Mode Is Picked

Each mode has its own follow-up checklist. Collect everything in a single structured prompt where possible; agents lose context across multiple round-trips.

### After Paper

- **Paper balance** if not initialized: `vulcan paper init --balance <N>` (suggest $10,000 default).
- **Tick interval / strategy-specific knobs** as the strategy demands.

### After Dry-Run

- No additional questions; the existing strategy config is enough.

### After Confirm-Each

- **Wallet** to unlock — by name, ideally a dedicated sub-wallet, not the user's main wallet.
- **Max step notional (USDC)** — per-slice/per-firing cap; suggest $5–10 for first runs.
- **Max total notional (USDC)** — total exposure cap.
- **Max price drift (bps)** — pause if mark drifts this far from launch price.
- **Max exposure ratio** — `notional / equity`; suggest 2–3x for first runs.
- **Strategy-specific knobs**: TP/SL preference, isolated vs cross margin, run duration.
- Confirm `vulcan strategy preflight` returns READY before launching. If not, surface the blockers verbatim and pause.

### After Auto-Execute

Everything from confirm-each, plus:

- **Time bound** — `--max-ticks <N>` or `--run-until-stopped` with an explicit out-of-band stop condition the user states (e.g. "stop at 6pm Eastern", "stop after 20 firings").
- **Monitoring cadence** — how often the agent will poll `vulcan_strategy_monitor` and what triggers an alert back to the user (e.g. "ping me on any failed fill", "ping me if drawdown exceeds $X").

## Example Structured Prompt For A Strategy Launch

Here's the recommended single-prompt shape when launching a TA strategy. Adapt by stripping fields that don't apply to other runner types.

```
Q: Launch parameters for the SOL RSI mean-reversion strategy.

Execution mode (pick one):
  1. Paper          — recommended for first run
  2. Dry-Run        — plan inspection only
  3. Confirm-Each   — live, you approve every fill
  4. Auto-Execute   — live, autonomous

If live (mode 3 or 4):
  - Wallet name (default: ask)
  - Max step notional USDC (default: $10)
  - Max total notional USDC (default: $50)
  - Max exposure ratio (default: 2.0)

If auto-execute:
  - Sub-wallet confirmed? (Y/N)
  - Time bound (max-ticks N, or run-until-stopped with stop time)
  - Alert cadence (every N firings, or on failure)

Paper balance to use (mode 1 only, default: $10,000):
```

The agent should generate the matching shape for grid (price bounds, levels per side, tokens per level) and TWAP (total notional, slices, interval seconds) automatically — never copy this exact prompt without strategy-specific adaptation.

## Graduating Between Modes

Before stepping any strategy up one mode, confirm explicitly with the user — never infer authorization from prior turns:

1. The strategy ran successfully at the lower mode for the agreed duration / number of firings.
2. No unexpected fills, paused/errored slices, or guardrail trips.
3. Sizing for the next mode is explicitly capped to an acceptable loss.
4. `vulcan strategy preflight` reports READY for the wallet being used.
5. The user has stated, in this session, "go live" / "step up to confirm-each" / "make it autonomous." Don't infer from "looks good" or "yeah."

If any of those are missing, stay at the current mode and explain what's still needed.

**Common graduation paths**:

- Observe → Paper: when the user explicitly wants to test a strategy and you've identified the right one.
- Paper → Confirm-Each: after ≥ 1 full market cycle in paper without anomalies, user explicitly authorizes live with small size.
- Confirm-Each → Auto-Execute: after 3–5 successful confirm-each runs with the same config, user explicitly authorizes autonomous execution with a stated time/loss bound.
- Confirm-Each → Paper (downward): always allowed; if anything feels off, step back down.

## Stopping And Rolling Back

A running strategy at any live mode can be stopped at any time:

```bash
vulcan strategy stop <run_id> --reason "manual stop"
vulcan strategy finalize <run_id> --cancel-orders --close-position --yes   # if you need to flatten
```

If you suspect a strategy has misbehaved, **stop first, investigate the ledger second**. Do not let it keep running while you debug.

## Cross-Reference

- Strategy-specific runbooks: `vulcan-twap-execution`, `vulcan-grid-trading`, `vulcan-ta-strategy`.
- Risk thresholds and pre-trade checks: `vulcan-risk-management`.
- Error category routing on `tx_failed` / network / validation errors: `vulcan-error-recovery`.
- Preflight semantics and the eight Non-Negotiable Rules: `vulcan` (entry-point skill; required before any live launch).
- First-session setup: `vulcan-quickstart`.

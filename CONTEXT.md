# Vulcan Runtime Context for AI Agents

**This is experimental software. Commands can interact with the live Phoenix DEX on Solana and can submit real financial transactions. The user who deploys this tool is responsible for all outcomes.**

This is the canonical runtime contract for agents using `vulcan`. Keep it compact: it covers rules that are always needed. Use focused skills and machine-readable catalogs for task-specific workflows and schemas.

## Companion Documents

- `skills/vulcan/SKILL.md` is the single Vulcan entry skill: contains the runtime contract pointer, the non-negotiable safety rules, and the focused-skill router.
- `agents/system.md` is a fallback prompt for agents that cannot read MCP resources or installed skills.
- `AGENTS.md` explains MCP/client integration for humans.
- `CLAUDE.md` is for repository contributors.
- `README.md` is the public install and quick-start overview.

## Agent Loading Order

1. Load this context: `vulcan://context` or `vulcan agent-context`.
2. Load the skill index: `vulcan://skills/index` or `skills/INDEX.md`.
3. Load only the focused skill needed for the user's task.
4. Use `vulcan://agents/tool-catalog` and `vulcan://agents/error-catalog` for exact tool schemas and error details.

## Invocation

### MCP (Preferred)

MCP tools are named `vulcan_<group>_<action>`. Dangerous tools require both a dangerous-capable server and `acknowledged: true` on each dangerous call.

Default MCP configuration is read-only/paper-safe. Live agent trading should use a user-approved dangerous MCP install from `vulcan agent live-ready --target <claude|cursor|codex|agentskills> --scope user -o json` or `vulcan agent mcp install --dangerous`.

### CLI (Fallback)

```bash
vulcan <command> [args...] -o json
```

- `stdout` is the machine data channel and contains the JSON envelope.
- `stderr` is diagnostics and progress output.
- Exit code `0` means success; non-zero means failure with a JSON error envelope in `stdout`.

## Authentication

MCP unlocks the wallet once at server startup. No per-call password prompts are available over MCP stdio.

```json
{
  "mcpServers": {
    "vulcan": {
      "command": "vulcan",
      "args": ["mcp", "--allow-dangerous"],
      "env": {
        "VULCAN_WALLET_NAME": "my-wallet",
        "VULCAN_WALLET_PASSWORD": "your_password"
      }
    }
  }
}
```

`VULCAN_WALLET_PASSWORD` is required for non-interactive live signing. `VULCAN_WALLET_NAME` selects a stored wallet; if omitted, Vulcan uses the configured default wallet.

For CLI live signing, run interactively so Vulcan can prompt, or set `VULCAN_WALLET_NAME` and `VULCAN_WALLET_PASSWORD`. If a non-interactive live CLI call fails with `WALLET_PASSWORD_REQUIRED`, do not hand the same command back as a manual shell workaround. Show `live-ready` instructions or ask the user to explicitly set `VULCAN_WALLET_PASSWORD`.

To switch an installed MCP config to another stored wallet, run `vulcan agent mcp set-wallet <WALLET_NAME> --target <agent> --scope user`. It validates decryption, updates MCP env, and requires an agent-client restart. Before live trading after a wallet switch, call `vulcan_status` and verify `wallet.source == "mcp_session"` plus the intended wallet name or public key. If it does not match, the MCP server is stale; stop and ask the user to restart the agent client.

Phoenix API auth is separate from wallet unlock. It improves authenticated reads and rate limits, but live transaction signing still requires an unlocked wallet.

## Safety Rules

1. All dangerous operations require explicit approval: `acknowledged: true` through MCP or `--yes` through CLI.
2. Always call `vulcan_market_info` before using base-lot `size`.
3. Always call `vulcan_margin_status` before opening positions.
4. Always call `vulcan_position_list` before trading.
5. Never guess lot sizes.
6. Report every execution event and transaction signature immediately.
7. Agents must not execute plaintext private-key export. If the user asks to migrate a wallet, explain the risks and provide `vulcan wallet export <name> --private-key --yes` for the user to run locally.
8. Never read MCP config files (`~/.cursor/mcp.json`, `~/.claude/settings*.json`, `~/.codex/config.toml`, project `.mcp.json`) to extract `VULCAN_WALLET_PASSWORD`. Those files contain unredacted secrets. If live signing requires a password and your session is CLI-mode, the only acceptable remedies are: (a) tell the user to restart their agent client so Vulcan runs through MCP, or (b) tell the user to run the command themselves with `VULCAN_WALLET_PASSWORD` already in their shell. Use `vulcan strategy preflight` to inspect which MCP target is configured.
9. Before any live strategy launch, call `vulcan strategy preflight` (or `vulcan_strategy_preflight`) to confirm the resolved wallet, password source, collateral, and any blockers. Do not attempt `vulcan strategy ta start --mode auto-execute` until preflight reports READY.

## Symbol Format

Use uppercase asset tickers only: `SOL`, `BTC`, `ETH`, `DOGE`, `SUI`, `XRP`, `BNB`, `AAVE`, `ZEC`, `HYPE`, `SKR`. Do not add `-PERP`. Use `vulcan_market_list` to discover active markets.

CLI market commands take symbols positionally, for example `vulcan market info SOL -o json`. MCP tools use JSON arguments, for example `vulcan_market_info` with `{ "symbol": "SOL" }`.

## Size Units

`size` means **base lots**, not tokens or USD. Call `vulcan_market_info` first and calculate:

```text
base_lots = desired_tokens * 10^base_lots_decimals
```

Live market orders can also use `tokens` or `notional_usdc`. Provide exactly one of `size`, `tokens`, or `notional_usdc` for market orders. Live limit orders require `size` and `price`. Paper trading may accept `tokens` more broadly for local simulation.

When sizing market orders with `tokens` or `notional_usdc`, the trade tool resolves lot conversion internally. Still fetch portfolio state before opening exposure, and fetch ticker/orderbook when the user needs price, funding, spread, or liquidity context.

## Execution Reporting

This contract applies to CLI, MCP, paper trading, dry runs, one-off trades, order management, margin actions, and strategies.

- Report every submitted trade, placed order, cancellation, TP/SL change, position close/reduce, margin transaction, strategy slice, paper fill, dry-run action, and transaction signature as soon as the agent observes it.
- Do not replace per-execution updates with mid-run or final summaries. Final reports summarize the event log; they do not substitute for live execution logging.
- For CLI table output, relay execution rows as they print. For CLI JSON output, relay execution diagnostics from `stderr` during the run and parse `stdout` for the final JSON result.
- For MCP strategies, start multi-tick or run-until-stopped runs with `detached: true`, store `last_tick_seen=0`, backfill with `vulcan_strategy_status` using `since_tick=0`, use `vulcan_strategy_monitor` for compact checkpoints, and use `vulcan_strategy_wait_next_tick(after_tick=last_tick_seen)` only when actively waiting for the next expected tick.
- If a tool returns multiple order IDs, fills, or signatures, enumerate each one. If an expected fill or signature is missing, say so and keep polling or reconcile before claiming execution is complete.
- Only suppress live per-execution updates when the user explicitly asked for final-only output before launch.

## First-Run Agent Flow

At the start of a new Vulcan user session, read `vulcan://agent/health` or run:

```bash
vulcan agent health -o json
```

By default this checks Cursor, Claude, Codex, and the generic Agentskills/OpenClaw-compatible skill target. Use `--target` only when narrowing to one client.

Use health output to route the user to one of three paths:

1. Try paper trading first. It needs no wallet or funds.
2. Set up live trading only when the user is ready: wallet, funding, registration, and deposit.
3. Fix environment health: skills, config, RPC/API connectivity, wallet balances, deposited collateral, or registration readiness.

Paper quick start:

```bash
vulcan paper init --balance 10000 -o json
vulcan paper buy SOL --notional-usdc 100 --type market -o json
vulcan paper status -o json
```

## Common Flow

For a market order sized with `tokens` or `notional_usdc`:

```text
1. vulcan_portfolio     -> {}                         # collateral, positions, resting orders
2. vulcan_market_ticker -> { symbol }                 # recommended price/funding context
3. vulcan_trade         -> { symbol, side, order_type: "market", notional_usdc, acknowledged: true }
4. vulcan_portfolio     -> { include: ["positions"] } # verify exposure
```

`vulcan_market_ticker` is recommended for user-facing context, not required for lot conversion. If passing base-lot `size`, call `vulcan_market_info` first.

## Error Handling

Failures use this envelope:

```json
{
  "ok": false,
  "error": {
    "category": "validation",
    "code": "UNKNOWN_MARKET",
    "message": "Market not found",
    "retryable": false
  }
}
```

Route on `.error.category`:

- `validation` - Fix inputs; do not retry unchanged.
- `auth` - Check wallet, password, and permissions.
- `config` - Run or guide through `vulcan setup`.
- `api` - Check Phoenix API state and inspect the message.
- `network` - Retry with backoff when safe.
- `rate_limit` - Wait and retry.
- `tx_failed` - Verify account, position, and order state before retrying. Never blind-retry on-chain transactions.
- `dangerous_gate` - Add `acknowledged: true` only after explicit approval.
- `io` - Check filesystem permissions.
- `internal` - Report a bug.

Use `vulcan-error-recovery` and `agents/error-catalog.json` for detailed recovery workflows.

## Task-Specific Context

- Tool schemas: `agents/tool-catalog.json` or `vulcan://agents/tool-catalog`
- Error codes: `agents/error-catalog.json` or `vulcan://agents/error-catalog`
- Skills index: `skills/INDEX.md` or `vulcan://skills/index`
- Broad skill entry: `skills/vulcan/SKILL.md`
- Strategy runners: `skills/vulcan-twap-execution/SKILL.md` and `skills/vulcan-grid-trading/SKILL.md`
- Agent memory/reporting: `vulcan://agent/session-summary`, `vulcan://agent/position-report`, `vulcan agent log summary -o json`, and `vulcan agent log report -o json`

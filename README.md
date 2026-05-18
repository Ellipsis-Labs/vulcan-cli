# Vulcan

Agent and human-friendly CLI for trading perpetual futures on [Phoenix](https://phoenix.trade).

> ⚠️ **Experimental software:** live commands can execute irreversible financial transactions on Solana Mainnet. You are responsible for wallet security, agent permissions, and all trading outcomes.

## What Vulcan Provides

- Account, margin, position, and order management for Phoenix perps.
- Local paper trading with live market prices and no wallet required.
- A local MCP server so agents can use Vulcan tools directly.
- First-class strategy runners for TWAP, grid trading, and TA.
- Bundled Agent Skills for Cursor, Claude Code, Codex, and Agentskills/OpenClaw-style clients.

### Example Agent Prompts

> "Give me a full portfolio snapshot: cross + isolated margin health, open positions with unrealized PnL %, resting orders, and any funding exposure I should know about."
>
> "Scan the top 5 markets on Phoenix by 24h volume — give me funding rate, mark price, and a one-line take on each."
>
> "Open a $200 long on SOL then attach a TP at +5% and SL at -3%."
>
> "I need to long $5,000 of SOL. Run a TWAP over 20 minutes in 10 slices.
>
> "Watch BTC on 15m and alert me when EMA(9) crosses EMA(21), paper only"
>
> "Build a TA strategy that goes long when RSI(14) crosses above 30 on the 5m and exits when it crosses 70. Paper mode first, $500 per entry"

## Install

Install the latest release on macOS/Linux:

```bash
curl -fsSL https://github.com/Ellipsis-Labs/vulcan-cli/releases/latest/download/install.sh | sh
```

The installer verifies the release archive against `vulcan-checksums-sha256.txt` and installs to `~/.local/bin/vulcan` by default. Make sure `~/.local/bin` is on your `PATH`.

Install a specific version:

```bash
curl -fsSL https://github.com/Ellipsis-Labs/vulcan-cli/releases/download/v0.5.3/install.sh | sh
```

Build from source:

```bash
cargo install --path vulcan
```

Verify:

```bash
vulcan version
```

Check for newer releases:

```bash
vulcan update check
```

This is read-only — vulcan never modifies its own binary. When an update is available it prints a method-appropriate hint: re-run the install one-liner above for default or custom `~/.local/bin`-style installs, or use your package manager for Homebrew/Nix/distro installs. Agents that load `vulcan://agent/health` see the same information under `cli_update`.

## Quick Start

```bash
# Guided setup for config, wallet, registration, deposit, and agent options
vulcan setup

# Check installation, config, wallet, RPC/API, registration, and paper readiness
vulcan status -o json

# Read market data
vulcan market list -o json
vulcan market ticker SOL -o json

# Begin paper trading
vulcan paper init --balance 10000 -o json
vulcan paper buy SOL --notional-usdc 100 --type market -o json
vulcan paper status -o json
```

## Agent Setup

Vulcan installs a local MCP server and a bundled set of Agent Skills into your agent host (Claude Code, Cursor, Codex, or any agentskills-compatible client). Two commands per host: one to install the skill files, one to register the MCP server. Live trading is opt-in.

Supported `--target` values: `claude`, `cursor`, `codex`, `agentskills`. `--scope user` writes to your user-level config (default); `--scope project` writes to the current project.

### Step 1: Install skills + MCP server (read-only / paper-safe)

```bash
# Bundled Agent Skills (workflow guides, recipes, runtime contract)
vulcan agent install --target claude --scope user

# Register the Vulcan MCP server with the host's standard config file
vulcan agent mcp install --target claude --scope user

# End-to-end probe: spawn the server, handshake, assert vulcan_* tools appear
vulcan agent mcp diagnose --target claude --scope user
```

**Fully restart your host** afterward so it picks up the new MCP server.

### Step 2 (optional): Enable live trading

Live trading requires a stored wallet to unlock for signing and live signing permission. You can setup at install time with `--dangerous`

```bash
# Wire up the wallet for live signing.
# Prompts for the wallet password interactively, validates decryption
vulcan agent mcp install --target claude --scope user --dangerous
```

After restart, dangerous tools (live trades, deposits, withdrawals, cancellations) appear in the agent's tool list and can be invoked with `acknowledged: true`.

> ⚠️ Both flows write your wallet password into the host's MCP config file on disk. The file is created `chmod 0600` — protect it like any other credential file.

### Switching wallets later

```bash
vulcan agent mcp set-wallet <wallet-name> --target claude --scope user
```

### Inspecting and repairing

```bash
# Show what's installed and what's missing per host
vulcan agent mcp doctor --target claude --scope user

# Reinstall command path / args while preserving the wallet password env
vulcan agent mcp install --target claude --scope user --repair

# Combined readiness check across all configured hosts
vulcan strategy preflight
```

### Running MCP standalone

For local testing or custom orchestration without a host:

```bash
# Read-only / paper-safe
vulcan mcp

# Live-capable
export VULCAN_WALLET_NAME=my-wallet
export VULCAN_WALLET_PASSWORD=your-password
vulcan mcp --allow-dangerous
```

`VULCAN_WALLET_NAME` selects a stored wallet; if omitted, Vulcan uses the default wallet. `VULCAN_WALLET_PASSWORD` is required when `--allow-dangerous` is set; empty strings are treated as unset.

Dangerous tools require both:

1. MCP server started with `--allow-dangerous`.
2. `acknowledged: true` on each dangerous tool call.

### Agent context, skills, and catalogs

- `vulcan://context` or `vulcan agent-context` — canonical runtime contract.
- `vulcan://skills/index` or `skills/INDEX.md` — workflow skill index.
- `vulcan://agents/tool-catalog` or `agents/tool-catalog.json` — exact tool schemas.
- `vulcan://agents/error-catalog` or `agents/error-catalog.json` — error codes and recovery hints.

Plaintext private-key export is user-only. Agents should explain the risk and provide the command for the user to run locally, but should not execute it.

See `AGENTS.md` for full integration details.

## Command Groups


| Group           | Purpose                                                                                   |
| --------------- | ----------------------------------------------------------------------------------------- |
| `setup`         | Interactive setup wizard for config, wallet, registration, deposits, and agent setup.     |
| `status`        | Health check for config, wallet, RPC, API, registration, and balances.                    |
| `market`        | Market list, ticker, market info, orderbook, and candles.                                 |
| `paper`         | Local paper trading with live prices and no real funds.                                   |
| `trade`         | Place and manage live orders, cancellations, multi-limit orders, and TP/SL.               |
| `position`      | List, show, close, reduce, and attach TP/SL to positions.                                 |
| `margin`        | Deposit, withdraw, transfer collateral, add isolated collateral, and view leverage tiers. |
| `portfolio`     | Combined margin, positions, and orders snapshot.                                          |
| `strategy`      | TWAP, grid, and TA runners, status, monitor, pause, stop, resume, finalize, and reports.  |
| `ta`            | Technical analysis — compute indicators, evaluate triggers, and multi-indicator reports.  |
| `history`       | Trader history for trades, orders, collateral, funding, and PnL.                          |
| `wallet`        | Create, import, export encrypted backups, list, select, and inspect wallets.              |
| `account`       | Account info and registration.                                                            |
| `auth`          | Phoenix API wallet-session login, status, and logout.                                     |
| `agent`         | Install, inspect, and repair Agent Skills and MCP config.                                 |
| `mcp`           | Start the local MCP server.                                                               |
| `agent-context` | Print the runtime contract for agents.                                                    |
| `version`       | Print version and build information.                                                      |


All commands support JSON output with `-o json`:

```json
{ "ok": true, "data": { }, "meta": { } }
{ "ok": false, "error": { "category": "...", "code": "...", "message": "...", "retryable": false } }
```

### Global Flags

Available on every command:


| Flag            | Purpose                                               |
| --------------- | ----------------------------------------------------- |
| `-o, --output`  | `json` or `table` (default `table`).                  |
| `-w, --wallet`  | Use a specific stored wallet instead of the default.  |
| `--dry-run`     | Simulate the action without submitting a transaction. |
| `-y, --yes`     | Skip interactive confirmation prompts.                |
| `--watch`       | Stream live updates via WebSocket where supported.    |
| `-v, --verbose` | Verbose/debug logging to stderr.                      |
| `--rpc-url`     | Override the Solana RPC endpoint.                     |
| `--api-url`     | Override the Phoenix API endpoint.                    |


## Strategies

Use first-class runners when Vulcan provides the strategy. Runners own the loop, perform launch-time checks for live modes, persist structured tick logs and ledgers under `~/.vulcan/strategy-runs`, and produce status/report output.

Supported runners:

- TWAP: split a larger order into timed slices.
- Grid: maintain layered limit orders across a price range.
- TA: rule-based runner over technical indicators for entry/exit.

Modes:

- `paper` - local simulation against live prices.
- `dry_run` - plan intended live actions without submitting transactions.
- `confirm_each` - live mode with confirmation before each execution step.
- `auto_execute` - live mode after explicit approval for the configured parameters.

Examples:

```bash
vulcan strategy twap start --symbol SOL --side buy --notional-usdc 1000 --slices 5 --interval-seconds 30 --mode paper -o json
vulcan strategy runs -o json
vulcan strategy monitor <RUN_ID> -o json
vulcan strategy finalize <RUN_ID> --cancel-orders --wait --yes -o json
```

For agent-specific strategy behavior, see `CONTEXT.md`, `skills/vulcan-twap-execution/SKILL.md`, and `skills/vulcan-grid-trading/SKILL.md`.

## Development

Prerequisites:

- Rust 1.84+
- Phoenix Rise SDK dependency from crates.io

```bash
cargo build
cargo test
cargo run -- --help
cargo run -- market ticker SOL -o json
```

Project layout:

```text
vulcan/           # Binary crate and CLI entry point
vulcan-lib/       # Library crate with commands, MCP, wallet, config, output, and errors
agents/           # Tool/error catalogs and fallback agent prompt
skills/           # Bundled Agent Skills (markdown)
CONTEXT.md        # Canonical runtime contract for agents
AGENTS.md         # MCP/client integration guide
CLAUDE.md         # Contributor guide
```


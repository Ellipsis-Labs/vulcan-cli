# Vulcan

Agent and human-friendly CLI for trading perpetual futures on [Phoenix](https://phoenix.trade).

> ⚠️ **Experimental software:** live commands can execute irreversible financial transactions on Solana Mainnet. You are responsible for wallet security, agent permissions, and all trading outcomes.

## What Vulcan Provides

- A human-friendly CLI with JSON output for automation.
- Local paper trading with live market prices and no wallet required.
- Wallet, account, margin, position, and order management for Phoenix perps.
- First-class strategy runners for TWAP and grid trading.
- A local MCP server so agents can use Vulcan tools directly.
- Bundled Agent Skills for Cursor, Claude Code, Codex, and Agentskills/OpenClaw-style clients.

## Install

Install the latest release on macOS/Linux:

```bash
curl -fsSL https://github.com/Ellipsis-Labs/vulcan/releases/latest/download/install.sh | sh
```

The installer verifies the release archive against `vulcan-checksums-sha256.txt` and installs to `~/.local/bin/vulcan` by default. Make sure `~/.local/bin` is on your `PATH`.

Install a specific version:

```bash
curl -fsSL https://github.com/Ellipsis-Labs/vulcan/releases/download/v0.1.0/install.sh | sh
```

Build from source:

```bash
cargo install --path vulcan
```

Verify:

```bash
vulcan version
```

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

Live trading requires both (a) a stored wallet to unlock for signing and (b) the `--allow-dangerous` flag on the MCP server args (without it, dangerous tools are filtered out of `tools/list`). The recommended flow:

```bash
# Wire up the wallet for live signing.
# Prompts for the wallet password interactively, validates decryption,
# writes VULCAN_WALLET_NAME + VULCAN_WALLET_PASSWORD into the MCP env block,
# AND ensures --allow-dangerous is in the MCP server args (idempotent).
vulcan agent mcp set-wallet <wallet-name> --target claude --scope user
```

Equivalently, you can do everything at install time:

```bash
vulcan agent mcp install --target claude --scope user --dangerous
```

After restart, dangerous tools (live trades, deposits, withdrawals, cancellations) appear in the agent's tool list and can be invoked with `acknowledged: true`.

> ⚠️ Both flows write your wallet password into the host's MCP config file on disk. The file is created `chmod 0600` — protect it like any other credential file.

### Switching wallets later

```bash
vulcan agent mcp set-wallet <other-wallet> --target claude --scope user
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
| `strategy`      | TWAP and grid runners, status, monitor, pause, stop, resume, finalize, and reports.       |
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

## Strategies

Use first-class runners when Vulcan provides the strategy. Runners own the loop, perform launch-time checks for live modes, persist structured tick logs and ledgers under `~/.vulcan/strategy-runs`, and produce status/report output.

Supported runners:

- TWAP: split a larger order into timed slices.
- Grid: maintain layered limit orders across a price range.

Modes:

- `paper` - local simulation against live prices.
- `dry_run` - plan intended live actions without submitting transactions.
- `confirm_each` - live mode with confirmation before each execution step.
- `auto_execute` - live mode after explicit approval for the configured parameters.

Examples:

```bash
vulcan strategy twap start --symbol SOL --side buy --notional-usdc 1000 --slices 5 --interval-seconds 30 --mode paper -o json
vulcan strategy grid start --symbol SOL --lower-price 140 --upper-price 160 --levels-per-side 5 --tokens-per-level 0.5 --mode paper -o json
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


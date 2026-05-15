# Agent Integration Guide: vulcan

> **This is experimental software. Commands can execute real financial transactions on Solana mainnet. The user who deploys this tool is responsible for all outcomes.**

This guide is for integrating `vulcan` into AI agents, MCP clients, and automated pipelines. It intentionally links to canonical runtime and machine-readable contracts instead of repeating every tool schema.

## Document Ownership

Keep all agent-facing documents, but give each one a narrow role:

- `CONTEXT.md` is the canonical runtime contract for all agent types. MCP exposes the same content as `vulcan://context`, and CLI exposes it through `vulcan agent-context`.
- `skills/vulcan/SKILL.md` is the single entry skill for skill-capable agents. It points to `CONTEXT.md`, lists the eight non-negotiable rules, gates live launches on `vulcan strategy preflight`, and routes to focused skills.
- `skills/*/SKILL.md` files are focused task workflows.
- `agents/system.md` is a fallback prompt only for clients that cannot read MCP resources or installed skills.
- `AGENTS.md` is this integration guide for humans wiring Vulcan into agents and MCP clients.
- `CLAUDE.md` is the contributor guide for this repository.
- `README.md` is the public install, quick-start, and product overview.

Do not copy long runtime rules or full tool tables across these files. Update `CONTEXT.md` for universal runtime behavior, focused skills for workflow behavior, and the machine-readable catalogs for schemas and error details.

## Read Order

- Runtime rules every agent needs: `CONTEXT.md` or `vulcan://context`
- Workflow selection: `skills/INDEX.md` or `vulcan://skills/index`
- Full tool schemas: `agents/tool-catalog.json` or `vulcan://agents/tool-catalog`
- Error codes and recovery hints: `agents/error-catalog.json` or `vulcan://agents/error-catalog`

## First-Run Agent Flow

1. Read `vulcan://agent/health` or run `vulcan agent health -o json`.
2. Offer paper trading first: `vulcan paper init --balance 10000 -o json`.
3. Offer wallet, registration, and deposit setup only when the user is ready for live funds.
4. Use health output to guide missing skills, config, wallets, balances, registration, RPC/API connectivity, and paper readiness.

`agent health` checks Cursor, Claude, Codex, and the generic Agentskills/OpenClaw-compatible target by default. Use `--target` only to narrow the check.

## Installation

From the repo:

```bash
cargo install --path vulcan
vulcan --version
```

Install bundled Agent Skills for compatible agents:

```bash
vulcan agent install --target cursor --scope user
vulcan agent install --target claude --scope project --dry-run
vulcan agent doctor --target cursor --scope user
```

The release installer can also install skills:

```bash
curl -fsSL https://github.com/Ellipsis-Labs/vulcan-cli/releases/latest/download/install.sh \
  | sh -s -- --with-agent-skills --agent-target cursor --agent-scope user
```

`vulcan setup` offers skill installation as an interactive step.

## MCP Integration

MCP is the preferred path for agents. Tools are named `vulcan_<group>_<action>`. Use `--groups` to expose only selected tool groups, for example `vulcan mcp --groups market,position`.

Default MCP configuration is read-only/paper-safe. Dangerous operations require both `--allow-dangerous` on the server and `acknowledged: true` in the tool call.

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

Three commands cover target-specific client config; they are not interchangeable:

- `vulcan agent mcp install --target <claude|cursor|codex|agentskills> --scope user --dangerous` — installs/updates the MCP entry. This is the one that actually wires live signing.
- `vulcan agent mcp doctor --target <…> --scope user` — inspects the installed entry (path, JSON storage pointer, dangerous flag, password env presence) and returns a `manual_install_command` when something is missing. Read-only.
- `vulcan agent live-ready --target <…> --scope user -o json` — readiness summary used by preflight and onboarding. Read-only.

Dangerous MCP setup stores wallet unlock material in local agent config, so only use `mcp install --dangerous` after explicit user opt-in.

To switch an installed MCP config to another stored wallet, run:

```bash
vulcan agent mcp set-wallet <WALLET_NAME> --target <agent> --scope user
```

After restart, call `vulcan_status` and verify `wallet.source == "mcp_session"` and the intended wallet name or public key before live actions.

## CLI Fallback

```bash
vulcan <command> [args...] -o json
```

- `stdout` contains the JSON envelope.
- `stderr` contains diagnostics and progress output.
- Exit code `0` means success; non-zero means failure with a JSON error envelope in `stdout`.

For CLI live signing, run interactively or set `VULCAN_WALLET_NAME` and `VULCAN_WALLET_PASSWORD`. `VULCAN_WALLET_NAME` is optional when a default wallet is configured; `VULCAN_WALLET_PASSWORD` is required for non-interactive live signing.

## Secrets And Wallets

Agents never need private keys. Agents must not ask for, display, log, export, or inspect private keys, decrypted keypair bytes, wallet bytes, auth tokens, signed challenge material, or unredacted wallet passwords.

Use encrypted backups for normal wallet portability:

```bash
vulcan wallet export my-wallet --file backup.json
```

Plaintext private-key export is user-only. If a user asks for migration help, explain that plaintext output is secret material and can be captured by terminal scrollback, logs, and shell history, then provide this command for the user to run locally:

```bash
vulcan wallet export my-wallet --private-key --yes
```

## Configuration

`vulcan setup` creates `~/.vulcan/config.toml`:

```toml
[network]
rpc_url = "https://api.mainnet-beta.solana.com"
api_url = "https://perp-api.phoenix.trade"

[wallet]
default = "my-wallet"
```

Credential resolution order:

1. CLI flags: `--rpc-url`, `--api-url`, `--api-key`
2. Config file: `~/.vulcan/config.toml`

## Runtime Invariants

Do not copy these into downstream prompts by hand; load `CONTEXT.md` or `vulcan://context`.

- Symbols are uppercase tickers such as `SOL`; do not use `-PERP`.
- `size` is base lots. Call `vulcan_market_info` before using it.
- Market orders may use exactly one of `size`, `tokens`, or `notional_usdc`.
- Call portfolio/margin/position preflight tools before opening exposure.
- Report every execution event and transaction signature immediately.
- For multi-tick MCP strategies, start detached, monitor incrementally, and finalize explicitly when cleaning up.

## Machine-Readable Resources

| Resource | Path | MCP URI |
| --- | --- | --- |
| Runtime context | `CONTEXT.md` | `vulcan://context` |
| Tool catalog | `agents/tool-catalog.json` | `vulcan://agents/tool-catalog` |
| Error catalog | `agents/error-catalog.json` | `vulcan://agents/error-catalog` |
| Skills index | `skills/INDEX.md` | `vulcan://skills/index` |
| Session summary | n/a | `vulcan://agent/session-summary` |
| Position report | n/a | `vulcan://agent/position-report` |

# CLAUDE.md

> **This is experimental software. Commands can execute real financial transactions on Solana mainnet. The user who deploys this tool is responsible for all outcomes.**

Vulcan is an AI-native CLI and MCP server for trading perpetual futures on Phoenix DEX. This file is for repository contributors. Agent runtime behavior lives in `CONTEXT.md`; workflow guidance lives in `skills/INDEX.md`.

## Fast Entry Points

- Runtime agent contract: `CONTEXT.md`
- Workflow skills: `skills/INDEX.md`
- Full command contract: `agents/tool-catalog.json`
- Error routing contract: `agents/error-catalog.json`
- MCP/integration guide: `AGENTS.md`

Agents should load `vulcan://context` or run `vulcan agent-context` before using Vulcan tools. Do not duplicate the full runtime contract or tool catalog in this file.

## Build And Run

```bash
cargo build
cargo run -- --help
cargo run -- market ticker SOL
cargo test
```

## Architecture

- `vulcan/` - Binary crate. CLI entry point, clap parse, and command dispatch.
- `vulcan-lib/` - Library crate. All business logic lives here.
  - `cli/` - Clap derive structs only. No business logic.
  - `commands/` - Command execution. Receives parsed args, calls SDK/client helpers, returns typed results.
  - `output/` - JSON envelope and table formatting.
  - `mcp/` - MCP server, tool registry, resources, and session wallet.
  - `wallet/` - Wallet struct and encrypted storage.
  - `config/` - `~/.vulcan/config.toml` parsing.
  - `context.rs` - `AppContext` shared across commands.
  - `error.rs` - `VulcanError` with categories and exit codes.

## Contributor Conventions

- Keep command return types as `Result<(), VulcanError>`; do not introduce `anyhow` in command boundaries.
- JSON output uses the envelope `{ "ok": true, "data": ..., "meta": ... }` or `{ "ok": false, "error": { "category", "code", "message", "retryable" } }`.
- Machine-readable tool schemas belong in `agents/tool-catalog.json`.
- Error categories, codes, and recovery hints belong in `agents/error-catalog.json`.
- Always update `CONTEXT.md` when changing universal agent runtime behavior.
- Update focused `skills/*/SKILL.md` files when changing task-specific workflows.
- Keep `AGENTS.md`, `README.md`, and this file as pointers to canonical sources instead of copying large tool tables.

## Safety And Secrets

- Wallet private keys must never be logged or printed accidentally.
- Plaintext private-key export exists only for user-run wallet migration via `wallet export --private-key --yes`; agents must not execute plaintext export commands.
- Prefer encrypted wallet backups for normal portability.
- The wallet implementation should continue to use zeroization for secret material.

## Agent Docs Maintenance

Use this ownership model when editing docs:

- `CONTEXT.md` - Always-needed runtime contract for agents.
- `skills/vulcan/SKILL.md` - Single Vulcan entry skill: runtime contract pointer, non-negotiable safety rules, focused-skill router, live-launch preflight gate.
- `skills/*/SKILL.md` - Task-specific flows.
- `AGENTS.md` - MCP/client integration guide.
- `README.md` - Human-facing install and product overview.
- `agents/system.md` - Fallback prompt for clients that cannot load MCP resources or skills.

Compatibility rule: keep all of these files, but do not make them peers. `CONTEXT.md` and the machine-readable catalogs are authoritative; `skills/vulcan/SKILL.md` adapts that contract to skill-capable agents; `agents/system.md` adapts it to non-resource agents.

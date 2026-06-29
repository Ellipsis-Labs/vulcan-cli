---
name: vulcan-quickstart
version: 1.0.0
description: "Five-minute install + first paper trade. Use for any 'set up Vulcan / install kraken-cli-like tool / get started' request before falling through to vulcan-onboarding's wallet-creation flow."
metadata:
  openclaw:
    category: "finance"
  requires:
    bins: ["vulcan"]
    skills: ["vulcan"]
---

# vulcan-quickstart

Single-page setup. Get a new user from zero to "I just paper-traded SOL on Phoenix" in under five minutes. For wallet creation, registration, and the first live deposit, hand off to `vulcan-onboarding`.

## 1. Install the `vulcan` binary

Pick one. Both put `vulcan` on `PATH`.

```bash
# Prebuilt release (macOS, Linux)
curl -fsSL https://github.com/Ellipsis-Labs/vulcan-cli/releases/latest/download/install.sh | sh

# From a local clone
cargo install --path vulcan --force
```

Verify:

```bash
vulcan version
```

## 2. Paper-trade on day one

Paper uses real Phoenix market prices and a simulated balance. **No wallet, no API key, no setup.**

```bash
vulcan paper init --balance 10000 -o json
vulcan paper buy SOL --tokens 0.1 -o json
vulcan paper status -o json
vulcan paper positions -o json
```

Public market data also needs no credentials:

```bash
vulcan market ticker SOL -o json
vulcan market orderbook SOL --depth 10 -o json
vulcan ta report SOL --timeframe 1h -o json
```

Most first sessions can do everything they need on paper + public data. Stop here if that's enough.

## 3. (Optional) Wire Vulcan into your agent client

Once you want the agent to use Vulcan tools directly, install the bundled Skills and register the local MCP server with your host. Two commands per host (substitute `--target` as needed: `claude`, `cursor`, `codex`, `agentskills`):

```bash
vulcan agent install --target claude --scope user           # bundled Agent Skills
vulcan agent mcp install --target claude --scope user       # register MCP server
vulcan agent mcp diagnose --target claude --scope user      # end-to-end probe
```

Restart the host fully. The agent now sees `vulcan_*` tools — read-only and paper-trading only until you opt in to live (next step).

## 4. (Optional) Add a wallet for live trading

For real funds, hand off to `vulcan-onboarding` — it covers wallet creation, account registration, deposit, and verification. The short version:

```bash
vulcan wallet create my-wallet                       # creates an encrypted wallet, prompts for password
vulcan account register                              # or add --referral-code <CODE>; registers the trader on Phoenix
vulcan margin deposit 50                             # deposit 50 USDC of collateral
```

Then wire the wallet into the MCP server:

```bash
vulcan agent mcp set-wallet my-wallet --target claude --scope user
```

`set-wallet` prompts for the wallet password interactively, validates decryption, writes `VULCAN_WALLET_NAME` + `VULCAN_WALLET_PASSWORD` into the host's MCP env block, and ensures `--allow-dangerous` is in the MCP server args (idempotent). Restart the host; Vulcan can now sign live transactions.

The wallet password is written into your host's MCP config file on disk (created `chmod 0600`). Treat that file like any other credential file. For local CLI / standalone use without a host, you can instead `export VULCAN_WALLET_PASSWORD=…` in your shell and run `vulcan mcp --allow-dangerous` directly.

## 5. Always run preflight before going live

Before any agent-driven live trade or strategy launch:

```bash
vulcan strategy preflight -o json
```

Confirms the active wallet, password source, collateral, and lists any blockers with concrete remedy commands. If it reports anything other than READY, fix that first — do not attempt a live launch.

## Scoping recommendation: use a dedicated sub-wallet

An AI agent should not have access to your main wallet's balance. Create a separate encrypted wallet with a small collateral allocation specifically for agent-driven trading. If the agent goes wrong, the blast radius is bounded to that wallet's balance.

```bash
vulcan wallet create agent-sandbox              # separate keypair, separate password
# fund agent-sandbox externally with a little SOL + USDC, then:
vulcan account register -w agent-sandbox
vulcan margin deposit 50 -w agent-sandbox       # deposit 50 USDC to the sandbox trader
```

Then point the MCP server at that wallet (`vulcan agent mcp set-wallet agent-sandbox --target <host> --scope user`), not your main one.

For finer-grained risk isolation inside a single wallet, Phoenix also supports numbered subaccounts via `vulcan account create-subaccount --subaccount-index <N>` and `vulcan margin transfer --from 0 --to <N> <AMOUNT>`. That is an advanced workflow beyond quickstart scope.

## Next skills

- **For execution modes** (Observe / Paper / Dry-Run / Confirm-Each / Auto-Execute, plus how to ask the user which mode and what follow-up to collect): `vulcan-execution-modes`. This is the canonical taxonomy — load it before any strategy/trade launch.
- For wallet creation / registration / first deposit details: `vulcan-onboarding`.
- For ad-hoc indicator reads: `vulcan-technical-analysis`.
- For declarative TA strategies: `vulcan-ta-strategy`.
- For TWAP execution of large orders: `vulcan-twap-execution`.
- For grid trading: `vulcan-grid-trading`.

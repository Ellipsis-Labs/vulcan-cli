---

name: vulcan-onboarding
version: 1.0.0
description: "First interaction and new user setup: health check, paper-first path, wallet creation, registration, first deposit, and verification."
metadata:
  openclaw:
    category: "finance"
  requires:
    bins: ["vulcan"]
    skills: ["vulcan"]

---

# vulcan-onboarding

Use this skill for:

- First user interaction when the user wants help getting started with Vulcan
- First-time setup of vulcan
- Paper-first onboarding before live funds
- Creating and configuring a wallet
- Registering a trader account
- Making the first deposit

Do not use this skill as the general Vulcan router. The `vulcan` entry skill routes broad requests here when the user is new or needs setup.

## First Interaction

Start with a health check:

```bash
vulcan agent health -o json
```

or read:

```text
vulcan://agent/health
```

Then offer:

1. Try paper trading first. This is recommended and needs no wallet or funds.
2. Set up live trading only when the user is ready: wallet, registration, and deposit.
3. Fix environment health: agent skills, Phoenix API, Solana RPC, wallets, balances, deposited collateral, registration, or paper readiness.

By default, `agent health` checks Cursor, Claude, Codex, and the generic Agentskills/OpenClaw-compatible skill target. Use `--target` only when the user asks about a specific agent client.

Paper quick start:

```bash
vulcan paper init --balance 10000 -o json
vulcan paper buy SOL --notional-usdc 100 --type market -o json
vulcan paper status -o json
```

## Prerequisites

- Solana wallet with SOL (for transaction fees) and USDC (for collateral).
- An access code or referral code for Phoenix DEX registration.

## Step 1: Install and Configure

```bash
cargo install --path vulcan    # from repo
vulcan setup                   # interactive setup wizard
vulcan agent health -o json    # readiness and next steps
vulcan agent mcp doctor --target cursor --scope user -o json
```

Setup creates `~/.vulcan/config.toml`, checks trader registration status, can complete registration with an access code or referral code, and can install read-only/paper MCP config. The Phoenix API session is signed in automatically using the configured wallet — no separate login step is required.

MCP is optional for paper and dry-run usage. For agent-driven live trading, prefer dangerous MCP with an unlocked session wallet: `vulcan agent mcp install --target cursor --dangerous`. Use this only after the user accepts that `VULCAN_WALLET_PASSWORD` may live in plaintext agent config.

## Step 2: Create a Wallet

Wallet operations are CLI-only (not available via MCP):

```bash
vulcan wallet create --name <NAME>             # interactive password prompt
vulcan wallet import --name <NAME> <SOURCE>    # import existing Solana keypair
vulcan wallet list             # verify wallet created
vulcan wallet set-default <NAME>
```

## Step 3: Fund the Wallet

The wallet needs:

- **SOL** — for Solana transaction fees (~0.01 SOL per transaction).
- **USDC** — for trading collateral.

After wallet creation, give the user the wallet public key so they can fund it externally. Funding happens outside Vulcan through a wallet transfer, exchange withdrawal, or similar flow. In MCP, prefer the explicit address helper:

```
vulcan_wallet_address → {}
```

Check balances:

```
vulcan_wallet_balance → {}
```

`vulcan_wallet_balance` is wallet funds only. Trader collateral is separate USDC already deposited into Phoenix; use `vulcan_portfolio` or `vulcan agent health -o json` to view deposited collateral.

## Step 4: Register Trader Account

Registration is invite/access-code gated. Health surfaces this before the user attempts registration. The setup wizard can do this interactively. CLI supports either code type:

```bash
vulcan account register --access-code <CODE>
vulcan account register --referral-code <CODE>
```

For MCP:

```
vulcan_account_register → { access_code: "YOUR_CODE", acknowledged: true }
vulcan_account_register → { referral_code: "YOUR_CODE", acknowledged: true }
vulcan_account_register → { invite_code: "YOUR_CODE", acknowledged: true } # backwards-compatible alias
```

Registration activates the access/referral code through the Phoenix API and creates the on-chain trader account for the default cross-margin subaccount. If the trader is already registered, verify with `vulcan_account_info`.

## Step 5: Deposit Collateral

```
vulcan_margin_deposit → { amount: 100.0, acknowledged: true }
```

`amount` is in USDC. Verify with `vulcan_margin_status`; collateral should reflect the deposit and risk state should be `Healthy`.

## Step 6: Verify Everything

```
vulcan_status → {}    # checks config, wallet, RPC, API, registration
vulcan_auth_status → {} # checks shared Phoenix API session status
```

All checks should pass. If any fail, the status or health output includes recovery hints. API auth sessions are separate from wallet unlock; they improve authenticated API access but do not sign live transactions.

## Step 7: First Trade (Optional)

Follow the safe order flow from the `vulcan-trade-execution` skill:

```
vulcan_market_info   → { symbol: "SOL" }
vulcan_market_ticker → { symbol: "SOL" }
vulcan_margin_status → {}
```

Then place a small test trade.

## Troubleshooting


| Issue                 | Fix                                                          |
| --------------------- | ------------------------------------------------------------ |
| `NO_DEFAULT_WALLET`   | `vulcan wallet set-default <name>`                           |
| `DECRYPT_FAILED`      | Wrong password. Set `VULCAN_WALLET_PASSWORD`                 |
| `NO_TRADER_ACCOUNT`   | Register with an access code or referral code                |
| `CONFIG_ERROR`        | Run `vulcan setup`                                           |
| `REGISTER_API_FAILED` | Check code validity and `api_url` in `~/.vulcan/config.toml` |
| Insufficient SOL      | Fund wallet with SOL for tx fees                             |
| Insufficient USDC     | Transfer USDC to wallet address                              |

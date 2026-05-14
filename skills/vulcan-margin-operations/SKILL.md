---
name: vulcan-margin-operations
version: 1.0.0
description: "Deposit, withdraw, transfer collateral, isolated margin, and leverage tier management."
metadata:
  openclaw:
    category: "finance"
  requires:
    bins: ["vulcan"]
    skills: ["vulcan"]
---

# vulcan-margin-operations

Use this skill for:
- Depositing and withdrawing USDC collateral
- Transferring between cross-margin and isolated subaccounts
- Adding collateral to isolated positions
- Checking leverage tiers

## Check Margin Status

```
vulcan_margin_status → {}
```

Key fields: collateral, total_unrealized_pnl, risk_state, available_to_withdraw.

## Deposit USDC

```
vulcan_margin_deposit → { amount: 100.0, acknowledged: true }
```

Prerequisite: wallet must have USDC. Check with `vulcan_wallet_balance`.

## Withdraw USDC

```
vulcan_margin_withdraw → { amount: 50.0, acknowledged: true }
```

Check `available_to_withdraw` from `vulcan_margin_status` first. Cannot withdraw if it would put account into HighRisk state.

## Transfer Between Subaccounts

Transfer from cross-margin (subaccount 0) to isolated (subaccount 1+):

```
vulcan_margin_transfer → {
  from_subaccount: 0,
  to_subaccount: 1,
  amount: 50.0,
  acknowledged: true
}
```

## Add Collateral to Isolated Position

Shorthand for transferring from cross-margin to the isolated subaccount holding a position:

```
vulcan_margin_add_collateral → { symbol: "SOL", amount: 25.0, acknowledged: true }
```

## Sweep Child to Cross-Margin

Move all collateral from an isolated subaccount back to cross-margin:

```
vulcan_margin_transfer_child_to_parent → { child_subaccount: 1, acknowledged: true }
```

## Sync Parent to Child

Sync parent (cross-margin) state to a child subaccount:

```
vulcan_margin_sync_parent_to_child → { child_subaccount: 1, acknowledged: true }
```

## Leverage Tiers

Check max leverage for different position sizes:

```
vulcan_margin_leverage_tiers → { symbol: "SOL" }
```

Returns a tiered schedule — larger positions get lower max leverage.

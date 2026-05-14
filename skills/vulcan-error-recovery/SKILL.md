---
name: vulcan-error-recovery
version: 1.0.0
description: "Error category routing, tx_failed recovery, and network error handling for Vulcan."
metadata:
  openclaw:
    category: "finance"
  requires:
    bins: ["vulcan"]
---

# vulcan-error-recovery

Use this skill for:
- Routing errors by category
- Recovering from failed on-chain transactions
- Handling network and rate limit errors

## Error Envelope Format

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

## Category Routing Table

| Category | Exit | Retryable | Action |
|----------|------|-----------|--------|
| `validation` | 1 | No | Fix the input. Common: UNKNOWN_MARKET, MISSING_ARG, INVALID_INTERVAL |
| `auth` | 2 | No | Check wallet exists and password is correct. Run `vulcan wallet list` |
| `config` | 3 | No | Run `vulcan setup` to recreate config |
| `api` | 4 | No | Phoenix API issue. Check `vulcan status` for connectivity |
| `network` | 5 | Yes | Transient. Retry with exponential backoff (1s, 2s, 4s) |
| `rate_limit` | 6 | Yes | Wait 5s and retry. Reduce request frequency |
| `tx_failed` | 7 | No | **Critical: verify state before retrying.** See below |
| `io` | 8 | Yes | File permission issue. Check `~/.vulcan/` permissions |
| `dangerous_gate` | 9 | No | Add `acknowledged: true` to the tool call |
| `internal` | 10 | No | Bug in vulcan. Report it |

## tx_failed Recovery (Critical)

On-chain transactions can fail in complex ways. **Never blind-retry.**

1. **Check position state first:**
   ```
   vulcan_position_list → {}
   vulcan_margin_status → {}
   ```

2. **Common causes:**
   - Blockhash expired — transaction took too long. Safe to retry with fresh state.
   - Insufficient SOL for fees — check `vulcan_wallet_balance`.
   - Account state changed — another transaction modified the account between build and send.
   - Slippage exceeded — market moved. Re-check price and retry.

3. **Recovery pattern:**
   ```
   1. vulcan_position_list → {}           # did the original tx partially succeed?
   2. vulcan_margin_status → {}           # is collateral state as expected?
   3. vulcan_wallet_balance → {}          # enough SOL for fees?
   4. vulcan_market_ticker → { symbol }   # has price moved significantly?
   5. Re-attempt the operation if state is clean
   ```

## Network Error Recovery

```
1. Wait 1 second
2. Retry the same call
3. If still failing, wait 2 seconds and retry
4. After 3 failures, check connectivity: vulcan_status → {}
5. Report to user if API is down
```

## Common Error Codes and Fixes

| Code | Fix |
|------|-----|
| `UNKNOWN_MARKET` | Run `vulcan_market_list` to see available symbols |
| `MISSING_ARG` | Check tool schema for required fields |
| `NO_POSITION` | No open position. Check `vulcan_position_list` |
| `ISOLATED_ONLY_MARKET` | Re-run with `isolated: true, collateral: <amount>` |
| `NO_DEFAULT_WALLET` | Run `vulcan wallet set-default <name>` |
| `DECRYPT_FAILED` | Wrong password. Check `VULCAN_WALLET_PASSWORD` |
| `NO_TRADER_ACCOUNT` | Register with `vulcan_account_register` |
| `BUILD_TPSL_FAILED` | TP/SL only works when opening/extending a position. Same-call TP/SL is supported on market orders and cross-margin limit orders; use post-hoc TP/SL tools for isolated limit orders. |

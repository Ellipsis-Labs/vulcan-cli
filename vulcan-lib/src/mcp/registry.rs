//! Tool registry — static tool definitions with JSON schemas for MCP.

use serde_json::{json, Value};

pub struct ToolDef {
    pub name: &'static str,
    pub description: &'static str,
    pub group: &'static str,
    pub dangerous: bool,
    pub schema: fn() -> Value,
}

/// All tools exposed by the Vulcan MCP server.
pub static TOOLS: &[ToolDef] = &[
    // ── Market (read-only) ──────────────────────────────────────────────
    ToolDef {
        name: "vulcan_market_list",
        description: "List all available perpetual markets on Phoenix DEX with fees and leverage info.",
        group: "market",
        dangerous: false,
        schema: || json!({
            "type": "object",
            "properties": {},
            "additionalProperties": false
        }),
    },
    ToolDef {
        name: "vulcan_market_ticker",
        description: "Get real-time ticker data for a market: mark price, funding rate, 24h volume and change.",
        group: "market",
        dangerous: false,
        schema: || json!({
            "type": "object",
            "properties": {
                "symbol": { "type": "string", "description": "Market symbol, e.g. SOL" }
            },
            "required": ["symbol"],
            "additionalProperties": false
        }),
    },
    ToolDef {
        name: "vulcan_market_info",
        description: "Get detailed market configuration: tick size, fees, funding params, leverage tiers.",
        group: "market",
        dangerous: false,
        schema: || json!({
            "type": "object",
            "properties": {
                "symbol": { "type": "string", "description": "Market symbol, e.g. SOL" }
            },
            "required": ["symbol"],
            "additionalProperties": false
        }),
    },
    ToolDef {
        name: "vulcan_market_orderbook",
        description: "Get L2 orderbook snapshot with bids, asks, mid price, and spread.",
        group: "market",
        dangerous: false,
        schema: || json!({
            "type": "object",
            "properties": {
                "symbol": { "type": "string", "description": "Market symbol, e.g. SOL" },
                "depth": { "type": "integer", "description": "Number of price levels per side", "default": 10 }
            },
            "required": ["symbol"],
            "additionalProperties": false
        }),
    },
    ToolDef {
        name: "vulcan_market_candles",
        description: "Get historical candlestick (OHLCV) data for a market.",
        group: "market",
        dangerous: false,
        schema: || json!({
            "type": "object",
            "properties": {
                "symbol": { "type": "string", "description": "Market symbol, e.g. SOL" },
                "interval": { "type": "string", "description": "Candle interval: 1m, 5m, 15m, 1h, 4h, 1d", "default": "1h" },
                "limit": { "type": "integer", "description": "Max candles to return", "default": 50 }
            },
            "required": ["symbol"],
            "additionalProperties": false
        }),
    },

    // ── Trade (dangerous) ───────────────────────────────────────────────
    ToolDef {
        name: "vulcan_trade",
        description: "Place a market or limit order. `side` is buy|sell. `order_type` is market|limit. Market orders take exactly one of `size` (base lots), `tokens` (base asset), or `notional_usdc` (USDC). Limit orders require `size` (base lots) and `price`.",
        group: "trade",
        dangerous: true,
        schema: || json!({
            "type": "object",
            "properties": {
                "symbol": { "type": "string", "description": "Market symbol, e.g. SOL" },
                "side": { "type": "string", "enum": ["buy", "sell"], "description": "Order side" },
                "order_type": { "type": "string", "enum": ["market", "limit"], "description": "Order type. Market executes immediately, limit rests on the book." },
                "size": { "type": "number", "description": "Size in base lots. Required for limit; one of size/tokens/notional_usdc for market." },
                "tokens": { "type": "number", "description": "Size in tokens of the base asset (market only)" },
                "notional_usdc": { "type": "number", "description": "Size as USDC notional, quoted at mid (market only). Actual fill may differ by spread + impact." },
                "price": { "type": "number", "description": "Limit price in USD (required for order_type=limit)" },
                "tp": { "type": "number", "description": "Optional take-profit price" },
                "sl": { "type": "number", "description": "Optional stop-loss price" },
                "isolated": { "type": "boolean", "description": "Use isolated margin (dedicated collateral per position)" },
                "collateral": { "type": "number", "description": "USDC collateral for isolated subaccount (requires isolated=true)" },
                "reduce_only": { "type": "boolean", "description": "Order can only reduce existing position" },
                "acknowledged": { "type": "boolean", "description": "Must be true to confirm this dangerous operation" }
            },
            "required": ["symbol", "side", "order_type", "acknowledged"],
            "additionalProperties": false
        }),
    },
    ToolDef {
        name: "vulcan_trade_multi_limit",
        description: "Place multiple limit orders (bids and asks) in a single transaction. Much faster than placing orders individually. Orders are post-only.",
        group: "trade",
        dangerous: true,
        schema: || json!({
            "type": "object",
            "properties": {
                "symbol": { "type": "string", "description": "Market symbol, e.g. SOL" },
                "bids": {
                    "type": "array",
                    "description": "Array of bid orders, each with price (USD) and size (base lots)",
                    "items": {
                        "type": "object",
                        "properties": {
                            "price": { "type": "number", "description": "Limit price in USD" },
                            "size": { "type": "integer", "description": "Order size in base lots" }
                        },
                        "required": ["price", "size"]
                    }
                },
                "asks": {
                    "type": "array",
                    "description": "Array of ask orders, each with price (USD) and size (base lots)",
                    "items": {
                        "type": "object",
                        "properties": {
                            "price": { "type": "number", "description": "Limit price in USD" },
                            "size": { "type": "integer", "description": "Order size in base lots" }
                        },
                        "required": ["price", "size"]
                    }
                },
                "slide": { "type": "boolean", "description": "Whether orders should slide to top of book if they would cross. Default false.", "default": false },
                "acknowledged": { "type": "boolean", "description": "Must be true to confirm this dangerous operation" }
            },
            "required": ["symbol", "bids", "asks", "acknowledged"],
            "additionalProperties": false
        }),
    },
    ToolDef {
        name: "vulcan_trade_orders",
        description: "List open orders. Omit symbol to list across all markets.",
        group: "trade",
        dangerous: false,
        schema: || json!({
            "type": "object",
            "properties": {
                "symbol": { "type": "string", "description": "Market symbol, e.g. SOL. Omit to list all markets." }
            },
            "required": [],
            "additionalProperties": false
        }),
    },
    ToolDef {
        name: "vulcan_trade_cancel",
        description: "Cancel orders. `scope=ids` cancels specific orders (requires order_ids); `scope=all` cancels every open order for the market (requires symbol); `scope=all-markets` cancels every open order across every market (symbol ignored); `scope=tpsl` cancels take-profit/stop-loss bracket orders on the position (use tp/sl booleans).",
        group: "trade",
        dangerous: true,
        schema: || json!({
            "type": "object",
            "properties": {
                "symbol": { "type": "string", "description": "Market symbol, e.g. SOL. Required for scope=ids, scope=all, and scope=tpsl. Ignored for scope=all-markets." },
                "scope": { "type": "string", "enum": ["ids", "all", "all-markets", "tpsl"], "description": "What to cancel" },
                "order_ids": { "type": "array", "items": { "type": "string" }, "description": "Order IDs to cancel (required when scope=ids)" },
                "tp": { "type": "boolean", "description": "When scope=tpsl, cancel the take-profit (default false)" },
                "sl": { "type": "boolean", "description": "When scope=tpsl, cancel the stop-loss (default false)" },
                "acknowledged": { "type": "boolean", "description": "Must be true to confirm this dangerous operation" }
            },
            "required": ["scope", "acknowledged"],
            "additionalProperties": false
        }),
    },

    ToolDef {
        name: "vulcan_trade_set_tpsl",
        description: "Set take-profit and/or stop-loss on an existing position. Auto-detects position side. Supports multiple levels per side and partial sizing via 'tp_levels' / 'sl_levels'. Use the legacy 'tp' / 'sl' fields for a single full-position exit. Sums of level sizes must not exceed the current position.",
        group: "trade",
        dangerous: true,
        schema: || {
            let level_schema = json!({
                "type": "object",
                "properties": {
                    "price": { "type": "number", "description": "Trigger and execution price" },
                    "size": { "type": "number", "description": "Size in base-asset tokens (e.g., 0.5 SOL). Mutually exclusive with size_lots; omit both to use full position (single level only)." },
                    "size_lots": { "type": "integer", "description": "Size in base lots. Mutually exclusive with size." }
                },
                "required": ["price"],
                "additionalProperties": false
            });
            json!({
                "type": "object",
                "properties": {
                    "symbol": { "type": "string", "description": "Market symbol, e.g. SOL" },
                    "tp": { "type": "number", "description": "Single take-profit price covering the full position. Mutually exclusive with tp_levels." },
                    "sl": { "type": "number", "description": "Single stop-loss price covering the full position. Mutually exclusive with sl_levels." },
                    "tp_levels": { "type": "array", "items": level_schema.clone(), "description": "Take-profit levels, e.g. [{price: 90, size: 0.5}, {price: 95, size: 0.5}]. Mutually exclusive with tp." },
                    "sl_levels": { "type": "array", "items": level_schema, "description": "Stop-loss levels. Mutually exclusive with sl." },
                    "acknowledged": { "type": "boolean", "description": "Must be true to confirm this dangerous operation" }
                },
                "required": ["symbol", "acknowledged"],
                "additionalProperties": false
            })
        },
    },
    // ── Position ────────────────────────────────────────────────────────
    ToolDef {
        name: "vulcan_position_list",
        description: "List all open positions across cross and isolated subaccounts. Each row carries side, size, entry, mark, unrealized_pnl, unrealized_pnl_pct (precomputed), initial_margin, maintenance_margin, liquidation_price, and subaccount_collateral/subaccount_index for isolated positions.",
        group: "position",
        dangerous: false,
        schema: || json!({
            "type": "object",
            "properties": {},
            "additionalProperties": false
        }),
    },
    ToolDef {
        name: "vulcan_position_show",
        description: "Show detailed info for a specific position: PnL, margin, liquidation price, TP/SL.",
        group: "position",
        dangerous: false,
        schema: || json!({
            "type": "object",
            "properties": {
                "symbol": { "type": "string", "description": "Market symbol, e.g. SOL" }
            },
            "required": ["symbol"],
            "additionalProperties": false
        }),
    },
    ToolDef {
        name: "vulcan_position_close",
        description: "Close an entire position via market order on the opposite side.",
        group: "position",
        dangerous: true,
        schema: || json!({
            "type": "object",
            "properties": {
                "symbol": { "type": "string", "description": "Market symbol, e.g. SOL" },
                "acknowledged": { "type": "boolean", "description": "Must be true to confirm this dangerous operation" }
            },
            "required": ["symbol", "acknowledged"],
            "additionalProperties": false
        }),
    },
    ToolDef {
        name: "vulcan_position_close_all",
        description: "Close every open position across all markets and subaccounts via market orders on the opposite side. Used for emergency flatten flows.",
        group: "position",
        dangerous: true,
        schema: || json!({
            "type": "object",
            "properties": {
                "acknowledged": { "type": "boolean", "description": "Must be true to confirm this dangerous operation" }
            },
            "required": ["acknowledged"],
            "additionalProperties": false
        }),
    },

    // ── Margin ──────────────────────────────────────────────────────────
    ToolDef {
        name: "vulcan_margin_status",
        description: "Show cross-margin (subaccount 0) status only: collateral, PnL, risk state, available to withdraw. For full account view including isolated subaccounts and total_account_value, call vulcan_portfolio.",
        group: "margin",
        dangerous: false,
        schema: || json!({
            "type": "object",
            "properties": {},
            "additionalProperties": false
        }),
    },
    ToolDef {
        name: "vulcan_margin_deposit",
        description: "Deposit USDC collateral into the trading account.",
        group: "margin",
        dangerous: true,
        schema: || json!({
            "type": "object",
            "properties": {
                "amount": { "type": "number", "description": "USDC amount to deposit" },
                "acknowledged": { "type": "boolean", "description": "Must be true to confirm this dangerous operation" }
            },
            "required": ["amount", "acknowledged"],
            "additionalProperties": false
        }),
    },
    ToolDef {
        name: "vulcan_margin_withdraw",
        description: "Withdraw USDC collateral from the trading account.",
        group: "margin",
        dangerous: true,
        schema: || json!({
            "type": "object",
            "properties": {
                "amount": { "type": "number", "description": "USDC amount to withdraw" },
                "acknowledged": { "type": "boolean", "description": "Must be true to confirm this dangerous operation" }
            },
            "required": ["amount", "acknowledged"],
            "additionalProperties": false
        }),
    },
    ToolDef {
        name: "vulcan_margin_transfer",
        description: "Transfer collateral between subaccounts (e.g., cross-margin to isolated).",
        group: "margin",
        dangerous: true,
        schema: || json!({
            "type": "object",
            "properties": {
                "from_subaccount": { "type": "integer", "description": "Source subaccount index (0 = cross-margin)" },
                "to_subaccount": { "type": "integer", "description": "Destination subaccount index" },
                "amount": { "type": "number", "description": "USDC amount to transfer" },
                "acknowledged": { "type": "boolean", "description": "Must be true to confirm this dangerous operation" }
            },
            "required": ["from_subaccount", "to_subaccount", "amount", "acknowledged"],
            "additionalProperties": false
        }),
    },
    ToolDef {
        name: "vulcan_margin_transfer_child_to_parent",
        description: "Sweep all collateral from a child (isolated) subaccount back to cross-margin.",
        group: "margin",
        dangerous: true,
        schema: || json!({
            "type": "object",
            "properties": {
                "child_subaccount": { "type": "integer", "description": "Child subaccount index to sweep" },
                "acknowledged": { "type": "boolean", "description": "Must be true to confirm this dangerous operation" }
            },
            "required": ["child_subaccount", "acknowledged"],
            "additionalProperties": false
        }),
    },
    ToolDef {
        name: "vulcan_margin_sync_parent_to_child",
        description: "Sync parent (cross-margin) state to a child (isolated) subaccount.",
        group: "margin",
        dangerous: true,
        schema: || json!({
            "type": "object",
            "properties": {
                "child_subaccount": { "type": "integer", "description": "Child subaccount index to sync to" },
                "acknowledged": { "type": "boolean", "description": "Must be true to confirm this dangerous operation" }
            },
            "required": ["child_subaccount", "acknowledged"],
            "additionalProperties": false
        }),
    },
    ToolDef {
        name: "vulcan_margin_leverage_tiers",
        description: "Show leverage tier schedule for a market: max leverage and max size per tier.",
        group: "margin",
        dangerous: false,
        schema: || json!({
            "type": "object",
            "properties": {
                "symbol": { "type": "string", "description": "Market symbol, e.g. SOL" }
            },
            "required": ["symbol"],
            "additionalProperties": false
        }),
    },

    ToolDef {
        name: "vulcan_margin_add_collateral",
        description: "Add USDC collateral to an isolated position by symbol. Transfers from cross-margin to the isolated subaccount.",
        group: "margin",
        dangerous: true,
        schema: || json!({
            "type": "object",
            "properties": {
                "symbol": { "type": "string", "description": "Market symbol of the isolated position, e.g. SOL" },
                "amount": { "type": "number", "description": "USDC amount to add" },
                "acknowledged": { "type": "boolean", "description": "Must be true to confirm this dangerous operation" }
            },
            "required": ["symbol", "amount", "acknowledged"],
            "additionalProperties": false
        }),
    },

    // ── Position (new) ─────────────────────────────────────────────────
    ToolDef {
        name: "vulcan_position_reduce",
        description: "Reduce a position by a specified size via market order on the opposite side.",
        group: "position",
        dangerous: true,
        schema: || json!({
            "type": "object",
            "properties": {
                "symbol": { "type": "string", "description": "Market symbol, e.g. SOL" },
                "size": { "type": "number", "description": "Size to reduce by in base lots" },
                "acknowledged": { "type": "boolean", "description": "Must be true to confirm this dangerous operation" }
            },
            "required": ["symbol", "size", "acknowledged"],
            "additionalProperties": false
        }),
    },
    ToolDef {
        name: "vulcan_position_tp_sl",
        description: "Attach take-profit and/or stop-loss bracket orders to an existing position.",
        group: "position",
        dangerous: true,
        schema: || json!({
            "type": "object",
            "properties": {
                "symbol": { "type": "string", "description": "Market symbol, e.g. SOL" },
                "tp": { "type": "number", "description": "Take-profit price" },
                "sl": { "type": "number", "description": "Stop-loss price" },
                "acknowledged": { "type": "boolean", "description": "Must be true to confirm this dangerous operation" }
            },
            "required": ["symbol", "acknowledged"],
            "additionalProperties": false
        }),
    },

    // ── History (read-only) ────────────────────────────────────────────
    ToolDef {
        name: "vulcan_history",
        description: "Get Phoenix/Rise trader history. `type` selects: trades (fills), orders, collateral, funding, or pnl.",
        group: "history",
        dangerous: false,
        schema: || json!({
            "type": "object",
            "properties": {
                "type": { "type": "string", "enum": ["trades", "orders", "collateral", "funding", "pnl"], "description": "Which history feed to query" },
                "symbol": { "type": "string", "description": "Filter by market symbol (ignored for collateral and pnl)" },
                "resolution": { "type": "string", "description": "PnL only: 1m, 5m, 15m, 1h, 4h, 1d, 1w, 1M", "default": "1h" },
                "limit": { "type": "integer", "description": "Max results to return", "default": 20 },
                "cursor": { "type": "string", "description": "Pagination cursor for history feeds that support it" }
            },
            "required": ["type"],
            "additionalProperties": false
        }),
    },

    // ── Status ────────────────────────────────────────────────────────
    ToolDef {
        name: "vulcan_status",
        description: "Health check: verify config, wallet, RPC, API, and trader registration status.",
        group: "status",
        dangerous: false,
        schema: || json!({
            "type": "object",
            "properties": {},
            "additionalProperties": false
        }),
    },

    // ── Update (read-only) ────────────────────────────────────────────
    ToolDef {
        name: "vulcan_update_check",
        description: "Check whether a newer Vulcan release is available on GitHub. Read-only: never modifies the binary. Returns current vs. latest version, a `update_available` boolean, the release URL, and an install-path-aware `update_command` hint (install.sh re-run for default/custom dirs, or 'use your package manager' for Homebrew/Nix/distro installs). Results are cached for 6 hours; pass `force_refresh: true` to bypass the cache. Surface the result to the user; do not run the update on their behalf.",
        group: "status",
        dangerous: false,
        schema: || json!({
            "type": "object",
            "properties": {
                "force_refresh": { "type": "boolean", "description": "Bypass the on-disk cache and hit the GitHub API directly.", "default": false }
            },
            "additionalProperties": false
        }),
    },

    // ── Wallet ────────────────────────────────────────────────────────
    ToolDef {
        name: "vulcan_wallet_create",
        description: "Create a new encrypted local wallet and return its public deposit address. Requires a password used only for local encryption.",
        group: "wallet",
        dangerous: false,
        schema: || json!({
            "type": "object",
            "properties": {
                "name": { "type": "string", "description": "Wallet name to create" },
                "password": { "type": "string", "description": "Password for encrypting the local wallet file" }
            },
            "required": ["name", "password"],
            "additionalProperties": false
        }),
    },
    ToolDef {
        name: "vulcan_wallet_list",
        description: "List all stored wallets with names, public keys, and default status.",
        group: "wallet",
        dangerous: false,
        schema: || json!({
            "type": "object",
            "properties": {},
            "additionalProperties": false
        }),
    },
    ToolDef {
        name: "vulcan_wallet_address",
        description: "Return a wallet public deposit address without querying balances.",
        group: "wallet",
        dangerous: false,
        schema: || json!({
            "type": "object",
            "properties": {
                "name": { "type": "string", "description": "Wallet name (omit for default wallet)" }
            },
            "additionalProperties": false
        }),
    },
    ToolDef {
        name: "vulcan_wallet_balance",
        description: "Check SOL and USDC balance for a wallet.",
        group: "wallet",
        dangerous: false,
        schema: || json!({
            "type": "object",
            "properties": {
                "name": { "type": "string", "description": "Wallet name (omit for default wallet)" }
            },
            "additionalProperties": false
        }),
    },

    // ── Portfolio (read-only) ─────────────────────────────────────────
    ToolDef {
        name: "vulcan_portfolio",
        description: "Full account snapshot in one call: cross margin, positions across cross and isolated subaccounts (with unrealized_pnl_pct and initial_margin), resting orders + conditional TP/SL triggers, isolated_subaccounts[] per-iso summary, and a totals block (total_collateral / total_account_value / total_unrealized_pnl) across all subaccounts. Prefer totals.total_account_value over margin.portfolio_value when reporting account value.",
        group: "portfolio",
        dangerous: false,
        schema: || json!({
            "type": "object",
            "properties": {
                "include": {
                    "type": "array",
                    "items": { "type": "string", "enum": ["margin", "positions", "orders"] },
                    "description": "Optional subset of sections to return. Default: all three."
                }
            },
            "additionalProperties": false
        }),
    },

    // ── Paper trading (local simulation) ───────────────────────────────
    ToolDef {
        name: "vulcan_paper_init",
        description: "Initialize or reset the local futures paper account. Uses live prices but no real funds.",
        group: "paper",
        dangerous: false,
        schema: || json!({
            "type": "object",
            "properties": {
                "balance": { "type": "number", "description": "Starting paper collateral balance", "default": 10000 },
                "currency": { "type": "string", "description": "Currency label", "default": "USDC" },
                "fee_bps": { "type": "number", "description": "Fee in basis points charged on fills", "default": 5 }
            },
            "additionalProperties": false
        }),
    },
    ToolDef {
        name: "vulcan_paper_status",
        description: "Show paper account equity, PnL, fees, aggregate position notional, exposure ratio, positions, orders, and fill counts.",
        group: "paper",
        dangerous: false,
        schema: || json!({
            "type": "object",
            "properties": {},
            "additionalProperties": false
        }),
    },
    ToolDef {
        name: "vulcan_paper_positions",
        description: "List open paper positions with entry, mark, and unrealized PnL.",
        group: "paper",
        dangerous: false,
        schema: || json!({
            "type": "object",
            "properties": {},
            "additionalProperties": false
        }),
    },
    ToolDef {
        name: "vulcan_paper_orders",
        description: "List open paper limit orders.",
        group: "paper",
        dangerous: false,
        schema: || json!({
            "type": "object",
            "properties": {},
            "additionalProperties": false
        }),
    },
    ToolDef {
        name: "vulcan_paper_fills",
        description: "List recent paper fills.",
        group: "paper",
        dangerous: false,
        schema: || json!({
            "type": "object",
            "properties": {
                "limit": { "type": "integer", "description": "Maximum fills to return", "default": 50 }
            },
            "additionalProperties": false
        }),
    },
    ToolDef {
        name: "vulcan_paper_trade",
        description: "Place a paper market or limit order. No real funds or Solana transactions are used.",
        group: "paper",
        dangerous: false,
        schema: || json!({
            "type": "object",
            "properties": {
                "symbol": { "type": "string", "description": "Market symbol, e.g. SOL" },
                "side": { "type": "string", "enum": ["buy", "sell"] },
                "order_type": { "type": "string", "enum": ["market", "limit"], "default": "market" },
                "size": { "type": "number", "description": "Size in base lots" },
                "tokens": { "type": "number", "description": "Size in base-asset tokens" },
                "notional_usdc": { "type": "number", "description": "Size as USDC notional" },
                "price": { "type": "number", "description": "Limit price" }
            },
            "required": ["symbol", "side", "order_type"],
            "additionalProperties": false
        }),
    },
    ToolDef {
        name: "vulcan_paper_cancel",
        description: "Cancel a local paper order by ID.",
        group: "paper",
        dangerous: false,
        schema: || json!({
            "type": "object",
            "properties": {
                "order_id": { "type": "string" }
            },
            "required": ["order_id"],
            "additionalProperties": false
        }),
    },
    ToolDef {
        name: "vulcan_paper_cancel_all",
        description: "Cancel all local paper orders, optionally filtered by symbol.",
        group: "paper",
        dangerous: false,
        schema: || json!({
            "type": "object",
            "properties": {
                "symbol": { "type": "string" }
            },
            "additionalProperties": false
        }),
    },
    ToolDef {
        name: "vulcan_paper_reconcile",
        description: "Evaluate resting paper limit orders against live market prices and fill crossed orders.",
        group: "paper",
        dangerous: false,
        schema: || json!({
            "type": "object",
            "properties": {
                "symbol": { "type": "string" }
            },
            "additionalProperties": false
        }),
    },

    // ── Account ───────────────────────────────────────────────────────
    ToolDef {
        name: "vulcan_account_info",
        description: "Get trader account info: collateral, positions, risk state.",
        group: "account",
        dangerous: false,
        schema: || json!({
            "type": "object",
            "properties": {},
            "additionalProperties": false
        }),
    },
    ToolDef {
        name: "vulcan_account_register",
        description: "Register a new trader account with an access code or referral code. At least one of access_code, referral_code, or invite_code must be provided.",
        group: "account",
        dangerous: true,
        schema: || json!({
            "type": "object",
            "properties": {
                "access_code": { "type": "string", "description": "Access code for registration" },
                "referral_code": { "type": "string", "description": "Referral code for registration" },
                "invite_code": { "type": "string", "description": "Backwards-compatible alias for access_code" },
                "acknowledged": { "type": "boolean", "description": "Must be true to execute" }
            },
            "required": ["acknowledged"],
            "additionalProperties": false
        }),
    },

    // ── Auth ────────────────────────────────────────────────────────────
    ToolDef {
        name: "vulcan_auth_status",
        description: "Show redacted Phoenix API wallet-session auth status. Does not expose tokens.",
        group: "auth",
        dangerous: false,
        schema: || json!({
            "type": "object",
            "properties": {},
            "additionalProperties": false
        }),
    },
    ToolDef {
        name: "vulcan_auth_login",
        description: "Log in to the Phoenix API by signing a wallet nonce with the unlocked MCP session wallet. Requires --allow-dangerous and acknowledged=true.",
        group: "auth",
        dangerous: true,
        schema: || json!({
            "type": "object",
            "properties": {
                "acknowledged": { "type": "boolean", "description": "Must be true to sign the auth challenge" }
            },
            "required": ["acknowledged"],
            "additionalProperties": false
        }),
    },
    ToolDef {
        name: "vulcan_auth_logout",
        description: "Clear the stored Phoenix API auth session. Does not affect wallet files.",
        group: "auth",
        dangerous: true,
        schema: || json!({
            "type": "object",
            "properties": {
                "acknowledged": { "type": "boolean", "description": "Must be true to clear the auth session" }
            },
            "required": ["acknowledged"],
            "additionalProperties": false
        }),
    },

    // ── Strategy runners ───────────────────────────────────────────────
    ToolDef {
        name: "vulcan_strategy_twap_start",
        description: "Run a TWAP strategy: market-order slices with lot-aware base-lot conversion, persisted slice ledger, and launch-time margin feasibility checks. Launch/monitoring contract: see CONTEXT.md § Strategy Monitoring (Detached Runs). TWAP narration: every tick is a fill — report planned vs executable slice, fill, cumulative VWAP, equity, and the full transaction signature per slice.",
        group: "strategy",
        dangerous: false,
        schema: || json!({
            "type": "object",
            "properties": {
                "symbol": { "type": "string", "description": "Market symbol, e.g. SOL" },
                "side": { "type": "string", "enum": ["buy", "sell"] },
                "notional_usdc": { "type": "number", "description": "Total strategy notional in USDC" },
                "tokens": { "type": "number", "description": "Total strategy size in base tokens" },
                "slices": { "type": "integer", "description": "Number of TWAP slices" },
                "interval_seconds": { "type": "integer", "description": "Seconds between slices", "default": 60 },
                "mode": { "type": "string", "enum": ["paper", "dry_run", "confirm_each", "auto_execute"], "description": "paper (Paper mode), confirm_each (Live mode with confirmation required), auto_execute (Live mode with automatic execution), or dry_run (Plan mode with dry run)", "default": "paper" },
                "margin_mode": { "type": "string", "enum": ["cross", "isolated"], "description": "Live margin mode. Ask the user whether they want cross-margin or isolated-margin execution before launch.", "default": "cross" },
                "isolated_collateral": { "type": "number", "description": "Optional USDC collateral to transfer from cross-margin on the first isolated live order" },
                "run_label": { "type": "string", "description": "Optional user-visible run label" },
                "max_total_notional_usdc": { "type": "number", "description": "Hard guardrail for total planned strategy notional" },
                "max_step_notional_usdc": { "type": "number", "description": "Hard guardrail for per-step planned notional" },
                "max_price_drift_bps": { "type": "number", "description": "Hard guardrail for mark-price drift from run start" },
                "max_exposure_ratio": { "type": "number", "description": "Hard guardrail for position notional divided by equity; map user max-leverage safety input to this value" },
                "reconcile_attempts": { "type": "integer", "description": "Number of history reconciliation attempts per live step" },
                "reconcile_delay_ms": { "type": "integer", "description": "Milliseconds between reconciliation attempts" },
                "acknowledged": { "type": "boolean", "description": "Required for live confirm_each or auto_execute; paper and dry_run do not require it" },
                "detached": { "type": "boolean", "description": "Return immediately with run_id; the agent enters the monitoring loop automatically (see CONTEXT.md § Strategy Monitoring (Detached Runs))", "default": false },
                "no_sleep": { "type": "boolean", "description": "Skip sleeping between slices for smoke tests", "default": false }
            },
            "required": ["symbol", "side", "slices"],
            "additionalProperties": false
        }),
    },
    ToolDef {
        name: "vulcan_strategy_grid_start",
        description: "Run a grid trading strategy: persisted level ledger, optional per-level TP/SL intent, replacement maintenance ticks, optional run-until-stopped lifecycle, stale-run detection, and local runner locking. Cross-margin uses multi-limit transactions; isolated submits individual limit orders and can transfer optional collateral on the first order. Live maintenance treats missing resting levels as filled and submits flipped replacements. Launch/monitoring contract: see CONTEXT.md § Strategy Monitoring (Detached Runs). Grid narration: most ticks are no-fill maintenance — send a concise heartbeat per tick (do not go silent); report fills/replacements in compact tables with full tx signatures. Per-level TP/SL is intent only until a bracket action is recorded. The runner is a local process, not a daemon — if lifecycle.stale=true, stop claiming the grid is maintained and tell the user to resume/reconcile. Use vulcan_strategy_finalize for explicit cleanup.",
        group: "strategy",
        dangerous: false,
        schema: || json!({
            "type": "object",
            "properties": {
                "symbol": { "type": "string", "description": "Market symbol, e.g. SOL" },
                "lower_price": { "type": "number", "description": "Lower grid boundary price. Required unless center_on_mark is true." },
                "upper_price": { "type": "number", "description": "Upper grid boundary price. Required unless center_on_mark is true." },
                "center_on_mark": { "type": "boolean", "description": "Compute lower/upper from the current mark at launch. Requires width_pct. Prefer this over absolute bounds when the approval round-trip might let the mark drift outside a fixed range.", "default": false },
                "width_pct": { "type": "number", "description": "Half-width of the grid as a percentage of mark (e.g. 1.0 = +-1%). Required when center_on_mark is true." },
                "levels_per_side": { "type": "integer", "description": "Number of bid levels below mark and ask levels above mark" },
                "tokens_per_level": { "type": "number", "description": "Generated level size in base tokens; mutually exclusive with size_lots_per_level" },
                "size_lots_per_level": { "type": "integer", "description": "Generated level size in base lots; mutually exclusive with tokens_per_level" },
                "bid_levels": { "type": "array", "items": { "type": "string" }, "description": "Custom bid levels as PRICE:SIZE_LOTS[:TP][:SL]" },
                "ask_levels": { "type": "array", "items": { "type": "string" }, "description": "Custom ask levels as PRICE:SIZE_LOTS[:TP][:SL]" },
                "take_profit_spacing": { "type": "number", "description": "Distance from entry to TP for generated levels" },
                "stop_loss_spacing": { "type": "number", "description": "Distance from entry to SL for generated levels" },
                "interval_seconds": { "type": "integer", "description": "Seconds between maintenance ticks", "default": 60 },
                "ticks": { "type": "integer", "description": "Maximum ticks to run, including initial placement", "default": 60 },
                "run_until_stopped": { "type": "boolean", "description": "Keep running maintenance ticks until paused or stopped", "default": false },
                "stale_after_seconds": { "type": "integer", "description": "Seconds without a tick before status reports this run as stale; defaults to max(2 * interval_seconds, 180)" },
                "mode": { "type": "string", "enum": ["paper", "dry_run", "confirm_each", "auto_execute"], "description": "paper, dry_run, confirm_each, or auto_execute", "default": "paper" },
                "margin_mode": { "type": "string", "enum": ["cross", "isolated"], "description": "Live margin mode. Ask the user whether they want cross-margin or isolated-margin execution before launch.", "default": "cross" },
                "isolated_collateral": { "type": "number", "description": "Optional USDC collateral to transfer from cross-margin on the first isolated live order" },
                "run_label": { "type": "string", "description": "Optional user-visible run label" },
                "slide": { "type": "boolean", "description": "Whether live multi-limit orders may slide to top of book if crossing", "default": false },
                "max_total_notional_usdc": { "type": "number", "description": "Max total planned grid notional" },
                "max_step_notional_usdc": { "type": "number", "description": "Max per-level notional" },
                "max_price_drift_bps": { "type": "number", "description": "Optional center-relative mark-price drift guard; grid bounds are the default price guard when omitted" },
                "max_exposure_ratio": { "type": "number", "description": "Max position notional divided by equity" },
                "reconcile_attempts": { "type": "integer", "description": "Number of reconciliation attempts" },
                "reconcile_delay_ms": { "type": "integer", "description": "Milliseconds between reconciliation attempts" },
                "acknowledged": { "type": "boolean", "description": "Required for live confirm_each or auto_execute; paper and dry_run do not require it" },
                "detached": { "type": "boolean", "description": "Return immediately with run_id; the agent enters the monitoring loop automatically (see CONTEXT.md § Strategy Monitoring (Detached Runs))", "default": false },
                "no_sleep": { "type": "boolean", "description": "Skip sleeping between ticks for smoke tests", "default": false }
            },
            "required": ["symbol", "levels_per_side"],
            "additionalProperties": false
        }),
    },
    ToolDef {
        name: "vulcan_strategy_runs",
        description: "List persisted strategy run summaries.",
        group: "strategy",
        dangerous: false,
        schema: || json!({
            "type": "object",
            "properties": {
                "limit": { "type": "integer", "default": 20 }
            },
            "additionalProperties": false
        }),
    },
    ToolDef {
        name: "vulcan_strategy_status",
        description: "Show compact latest status and ticks for a persisted strategy run. By default this omits the full ledger to keep agent context small; set include_ledger=true only when needed.",
        group: "strategy",
        dangerous: false,
        schema: || json!({
            "type": "object",
            "properties": {
                "run_id": { "type": "string" },
                "since_tick": { "type": "integer", "description": "Return ticks newer than this tick index in new_ticks" },
                "include_ledger": { "type": "boolean", "description": "Include the full persisted ledger; defaults to false to reduce context", "default": false }
            },
            "required": ["run_id"],
            "additionalProperties": false
        }),
    },
    ToolDef {
        name: "vulcan_strategy_monitor",
        description: "Return compact, non-blocking monitor state for a strategy run from persisted logs and ledger state: last observed tick, heartbeat age, expected next tick, runner lock, stale status, and event counts.",
        group: "strategy",
        dangerous: false,
        schema: || json!({
            "type": "object",
            "properties": {
                "run_id": { "type": "string" },
                "include_ledger": { "type": "boolean", "description": "Include the full persisted ledger; defaults to false to reduce context", "default": false }
            },
            "required": ["run_id"],
            "additionalProperties": false
        }),
    },
    ToolDef {
        name: "vulcan_strategy_reconcile_grid",
        description: "Read-only inspection of live grid orders against the persisted grid ledger. Reports expected resting levels and missing levels without mutating ledger state.",
        group: "strategy",
        dangerous: false,
        schema: || json!({
            "type": "object",
            "properties": {
                "run_id": { "type": "string" }
            },
            "required": ["run_id"],
            "additionalProperties": false
        }),
    },
    ToolDef {
        name: "vulcan_strategy_wait_next_tick",
        description: "Wait server-side until a strategy emits a tick newer than after_tick, reaches a terminal status, or times out. Use this between detached MCP strategy ticks instead of shell sleep or repeated full status polls.",
        group: "strategy",
        dangerous: false,
        schema: || json!({
            "type": "object",
            "properties": {
                "run_id": { "type": "string" },
                "after_tick": { "type": "integer", "description": "Return once a tick newer than this index exists. Omit on the first poll after a detached start to receive every tick already emitted (including ticks that landed before the call). For subsequent polls, pass the last tick index you have already narrated." },
                "timeout_seconds": { "type": "integer", "description": "Maximum seconds to wait before returning current status with timed_out=true. Server caps this at 300; longer values are silently clamped so the heartbeat contract holds on slow-cadence runs.", "default": 90, "minimum": 1, "maximum": 300 },
                "include_ledger": { "type": "boolean", "description": "Include the full persisted ledger; defaults to false to reduce context", "default": false }
            },
            "required": ["run_id"],
            "additionalProperties": false
        }),
    },
    ToolDef {
        name: "vulcan_strategy_report",
        description: "Read the final or latest persisted report for a strategy run.",
        group: "strategy",
        dangerous: false,
        schema: || json!({
            "type": "object",
            "properties": {
                "run_id": { "type": "string" }
            },
            "required": ["run_id"],
            "additionalProperties": false
        }),
    },
    ToolDef {
        name: "vulcan_strategy_pause",
        description: "Request a running strategy to pause at the next safe point. Paused runs can be resumed.",
        group: "strategy",
        dangerous: false,
        schema: || json!({
            "type": "object",
            "properties": {
                "run_id": { "type": "string" },
                "reason": { "type": "string", "description": "Optional reason recorded in the ledger" }
            },
            "required": ["run_id"],
            "additionalProperties": false
        }),
    },
    ToolDef {
        name: "vulcan_strategy_stop",
        description: "Request a running strategy to stop permanently at the next safe point.",
        group: "strategy",
        dangerous: false,
        schema: || json!({
            "type": "object",
            "properties": {
                "run_id": { "type": "string" },
                "reason": { "type": "string", "description": "Optional reason recorded in the ledger" }
            },
            "required": ["run_id"],
            "additionalProperties": false
        }),
    },
    ToolDef {
        name: "vulcan_strategy_finalize",
        description: "Request strategy stop and optionally perform explicit cleanup: cancel open orders on the strategy symbol and/or close any open position. Cleanup flags are dangerous and require acknowledged=true plus --allow-dangerous.",
        group: "strategy",
        dangerous: false,
        schema: || json!({
            "type": "object",
            "properties": {
                "run_id": { "type": "string" },
                "reason": { "type": "string", "description": "Optional reason recorded in the ledger" },
                "cancel_orders": { "type": "boolean", "description": "Cancel open orders on the strategy symbol", "default": false },
                "close_position": { "type": "boolean", "description": "Close any open position on the strategy symbol", "default": false },
                "wait": { "type": "boolean", "description": "Wait for the runner to observe the stop request before cleanup", "default": false },
                "timeout_seconds": { "type": "integer", "description": "Maximum seconds to wait for runner progress", "default": 90 },
                "acknowledged": { "type": "boolean", "description": "Required when cancel_orders or close_position is true" }
            },
            "required": ["run_id"],
            "additionalProperties": false
        }),
    },
    ToolDef {
        name: "vulcan_strategy_preflight",
        description: "Inspect live-readiness for the active wallet without launching anything. Reports wallet identity, password availability (env vs MCP session vs missing), trader registration, collateral, and every blocker with a concrete remedy command. Call this BEFORE attempting any live strategy start to avoid allocating an aborted run.",
        group: "strategy",
        dangerous: false,
        schema: || json!({
            "type": "object",
            "properties": {},
            "additionalProperties": false
        }),
    },
    ToolDef {
        name: "vulcan_strategy_ta_start",
        description: "Start a TA-driven strategy run. `config` describes rules of (condition, action): each tick the runner evaluates rules top-down and the first whose composable boolean condition fires (and is not cooldown-locked) dispatches its action (open/close/reduce). Conditions compose `compare`, `crosses`, `all`/`any`/`not` over indicators, price, and constants. Reuses TWAP/grid scaffolding (run id, ledger, control IPC, pause/stop, monitor/status/wait_next_tick/report). Launch/monitoring contract: see CONTEXT.md § Strategy Monitoring (Detached Runs). TA narration: report which rule fired (or 'no rule matched') per tick, plus the indicator values that drove the decision.",
        group: "strategy",
        dangerous: false,
        schema: || json!({
            "type": "object",
            "properties": {
                "config": {
                    "type": "object",
                    "description": "TA strategy config with symbol, interval_seconds, rules[].",
                    "additionalProperties": true
                },
                "mode": { "type": "string", "enum": ["paper", "dry_run", "confirm_each", "auto_execute"], "default": "paper" },
                "max_ticks": { "type": "integer", "default": 60 },
                "run_until_stopped": { "type": "boolean", "default": false },
                "run_label": { "type": "string" },
                "detached": { "type": "boolean", "description": "Return immediately with run_id; the agent enters the monitoring loop automatically (see CONTEXT.md § Strategy Monitoring (Detached Runs))", "default": false },
                "max_total_notional_usdc": { "type": "number" },
                "max_step_notional_usdc": { "type": "number" },
                "max_price_drift_bps": { "type": "number" },
                "max_exposure_ratio": { "type": "number" },
                "reconcile_attempts": { "type": "integer" },
                "reconcile_delay_ms": { "type": "integer" },
                "no_sleep": { "type": "boolean", "default": false },
                "acknowledged": { "type": "boolean", "description": "Required for live modes." }
            },
            "required": ["config"],
            "additionalProperties": false
        }),
    },

    // ── Technical analysis (read-only) ──────────────────────────────────
    ToolDef {
        name: "vulcan_ta_compute",
        description: "Compute a single technical indicator (sma, ema, rsi, macd, bbands, atr, vwap, adx, stoch) over the latest candles for a market. Returns the series plus a summary verdict.",
        group: "ta",
        dangerous: false,
        schema: || json!({
            "type": "object",
            "properties": {
                "symbol": { "type": "string", "description": "Market symbol, e.g. SOL" },
                "indicator": { "type": "string", "description": "Indicator: sma | ema | rsi | macd | bbands | atr | vwap | adx | stoch" },
                "timeframe": { "type": "string", "description": "Candle interval: 1m, 5m, 15m, 1h, 4h, 1d", "default": "1h" },
                "period": { "type": "integer", "description": "Lookback period; falls back to the indicator's default when omitted" },
                "limit": { "type": "integer", "description": "Number of candles to fetch", "default": 200 },
                "params": { "type": "object", "description": "Indicator-specific parameters (e.g. MACD fast/slow/signal, BBands dev_up/dev_down)", "additionalProperties": { "type": "number" } }
            },
            "required": ["symbol", "indicator"],
            "additionalProperties": false
        }),
    },
    ToolDef {
        name: "vulcan_ta_signal",
        description: "Evaluate a technical-analysis trigger spec against the latest indicator value. Returns { fired, value, threshold, op } so agents can gate decisions on conditions like 'RSI < 30' or 'MACD histogram crosses_above 0'.",
        group: "ta",
        dangerous: false,
        schema: || json!({
            "type": "object",
            "properties": {
                "symbol": { "type": "string", "description": "Market symbol, e.g. SOL" },
                "spec": {
                    "type": "object",
                    "description": "Trigger spec",
                    "properties": {
                        "indicator": { "type": "string" },
                        "timeframe": { "type": "string" },
                        "period": { "type": "integer" },
                        "op": { "type": "string", "enum": ["lt", "lte", "gt", "gte", "crosses_above", "crosses_below"] },
                        "threshold": { "type": "number" },
                        "key": { "type": "string", "description": "Optional series key override (e.g. 'hist' for MACD histogram, 'k' or 'd' for stoch)" },
                        "params": { "type": "object", "additionalProperties": { "type": "number" } }
                    },
                    "required": ["indicator", "timeframe", "op", "threshold"]
                }
            },
            "required": ["symbol", "spec"],
            "additionalProperties": false
        }),
    },
    ToolDef {
        name: "vulcan_ta_report",
        description: "Bundled multi-indicator snapshot (RSI, MACD, BBands, ATR, ADX) for market-intel reports. One call summarizes momentum, volatility, and trend strength.",
        group: "ta",
        dangerous: false,
        schema: || json!({
            "type": "object",
            "properties": {
                "symbol": { "type": "string", "description": "Market symbol, e.g. SOL" },
                "timeframe": { "type": "string", "description": "Candle interval", "default": "1h" }
            },
            "required": ["symbol"],
            "additionalProperties": false
        }),
    },

    ToolDef {
        name: "vulcan_strategy_resume",
        description: "Resume a paused or incomplete persisted strategy run. Live runs are dynamically treated as dangerous and require acknowledged=true plus --allow-dangerous.",
        group: "strategy",
        dangerous: false,
        schema: || json!({
            "type": "object",
            "properties": {
                "run_id": { "type": "string" },
                "from_step": { "type": "integer", "description": "Optional step index to resume from" },
                "acknowledged": { "type": "boolean", "description": "Required when resuming a live strategy" },
                "no_sleep": { "type": "boolean", "description": "Skip sleeping between steps for recovery smoke tests", "default": false }
            },
            "required": ["run_id"],
            "additionalProperties": false
        }),
    },
];

/// Filter tools by group. If groups is None, return all tools.
pub fn tools_for_groups(groups: &Option<Vec<String>>) -> Vec<&'static ToolDef> {
    match groups {
        None => TOOLS.iter().collect(),
        Some(gs) => TOOLS
            .iter()
            .filter(|t| gs.iter().any(|g| g == t.group))
            .collect(),
    }
}

#!/usr/bin/env python3
# /// script
# requires-python = ">=3.11"
# ///
"""End-to-end smoke test for paper mode.

Drives the paper-mode CLI against live Phoenix ticker data (read-only network)
to verify the full paper surface: account lifecycle, order placement, order
cancellation, reconcile, position arithmetic, fees, history, and the full
TP/SL surface (order-time, position-time, cascade cancel, firing, re-parent).

No funds, no wallet, no Solana transactions. Uses an isolated HOME so it never
touches the user's real paper-state.json.

Usage:
    ./scripts/paper_smoke.py SOL
    ./scripts/paper_smoke.py SOL --vulcan-bin ./target/debug/vulcan
"""

from __future__ import annotations

import argparse
import json
import os
import shlex
import subprocess
import sys
import tempfile
import time
from dataclasses import dataclass
from pathlib import Path
from typing import Any


@dataclass
class CommandResult:
    cmd: list[str]
    payload: dict[str, Any]
    returncode: int
    elapsed_s: float
    raw_stdout: str
    raw_stderr: str

    @property
    def ok(self) -> bool:
        return self.returncode == 0 and self.payload.get("ok") is True

    @property
    def data(self) -> dict[str, Any]:
        return self.payload.get("data") or {}

    @property
    def error_code(self) -> str | None:
        err = self.payload.get("error")
        return err.get("code") if isinstance(err, dict) else None


class Smoke:
    def __init__(self, args: argparse.Namespace) -> None:
        self.args = args
        self.use_color = sys.stdout.isatty() and not args.no_color
        self.temp_home = tempfile.TemporaryDirectory(prefix="paper-smoke-")
        self.env = os.environ.copy()
        self.env["HOME"] = self.temp_home.name
        self.passes: list[str] = []
        self.failures: list[str] = []

    def close(self) -> None:
        self.temp_home.cleanup()

    # ── output ────────────────────────────────────────────────────────────
    def _c(self, text: str, code: str) -> str:
        return f"\033[{code}m{text}\033[0m" if self.use_color else text

    def section_header(self, msg: str) -> None:
        print(self._c(f"\n━━ {msg} ━━", "1;35"))

    def header(self, msg: str) -> None:
        print(self._c(f"\n· {msg}", "1;36"))

    def ok(self, msg: str) -> None:
        print(self._c(f"  ✓ {msg}", "32"))
        self.passes.append(msg)

    def fail(self, msg: str) -> None:
        print(self._c(f"  ✗ {msg}", "31"))
        self.failures.append(msg)

    def info(self, msg: str) -> None:
        print(self._c(f"    {msg}", "90"))

    # ── command runner ────────────────────────────────────────────────────
    def _base_cmd(self) -> list[str]:
        cmd = [self.args.vulcan_bin]
        if self.args.api_url:
            cmd.extend(["--api-url", self.args.api_url])
        cmd.extend(["-o", "json"])
        return cmd

    def run(
        self, subcmd: list[str], *, allow_error: bool = False
    ) -> CommandResult:
        cmd = self._base_cmd() + subcmd
        start = time.perf_counter()
        try:
            proc = subprocess.run(
                cmd,
                env=self.env,
                text=True,
                capture_output=True,
                stdin=subprocess.DEVNULL,
                timeout=self.args.timeout,
            )
        except subprocess.TimeoutExpired as e:
            raise SystemExit(
                f"timed out after {self.args.timeout}s: {shlex.join(cmd)}"
            ) from e
        elapsed = time.perf_counter() - start
        try:
            payload = json.loads(proc.stdout) if proc.stdout.strip() else {}
        except json.JSONDecodeError:
            payload = {}
        result = CommandResult(
            cmd, payload, proc.returncode, elapsed, proc.stdout, proc.stderr
        )
        if not allow_error and not result.ok:
            err = result.payload.get("error") or {
                "stdout": proc.stdout,
                "stderr": proc.stderr,
            }
            raise SystemExit(
                f"command failed: {shlex.join(cmd)}\n{json.dumps(err, indent=2)}"
            )
        return result

    # ── state inspection ─────────────────────────────────────────────────
    def state_path(self) -> Path:
        return Path(self.temp_home.name) / ".vulcan" / "paper-state.json"

    def read_state(self) -> dict[str, Any]:
        return json.loads(self.state_path().read_text())

    def write_state(self, state: dict[str, Any]) -> None:
        self.state_path().write_text(json.dumps(state, indent=2))

    def get_mark(self, symbol: str) -> float:
        result = self.run(["market", "ticker", symbol])
        return float(result.data["mark_price"])

    def reset(self, fee_bps: float = 0.0, balance: float = 10000.0) -> None:
        """Start each scenario from a clean slate."""
        self.run(
            [
                "paper", "reset",
                "--balance", str(balance),
                "--fee-bps", str(fee_bps),
            ]
        )

    def init(self, fee_bps: float = 0.0, balance: float = 10000.0) -> CommandResult:
        return self.run(
            [
                "paper", "init",
                "--balance", str(balance),
                "--fee-bps", str(fee_bps),
            ]
        )

    # ── assertion helpers ────────────────────────────────────────────────
    def expect_err(self, label: str, result: CommandResult, code: str) -> None:
        if result.ok:
            self.fail(f"{label}: expected {code}, command succeeded")
            return
        if result.error_code != code:
            self.fail(
                f"{label}: expected {code}, got {result.error_code} "
                f"({result.payload.get('error', {}).get('message')})"
            )
            return
        self.ok(f"{label}: rejected with {code}")

    def expect_eq(self, label: str, actual: Any, expected: Any) -> bool:
        if actual != expected:
            self.fail(f"{label}: expected {expected!r}, got {actual!r}")
            return False
        self.ok(f"{label}: {actual!r}")
        return True

    def expect_close(
        self, label: str, actual: float | None, expected: float, tol: float = 1e-4
    ) -> bool:
        if actual is None or abs(float(actual) - expected) > tol:
            self.fail(f"{label}: expected ~{expected}, got {actual}")
            return False
        self.ok(f"{label}: {float(actual):.6f} ≈ {expected}")
        return True

    # ═══════════════════════════════════════════════════════════════════
    # ACCOUNT LIFECYCLE
    # ═══════════════════════════════════════════════════════════════════
    def scenario_init_and_status(self, symbol: str) -> None:
        self.header("Initialize paper account, status reflects it")
        r = self.init(fee_bps=5.0, balance=10000.0)
        data = r.data.get("state", {})
        self.expect_close("starting balance", data.get("starting_balance"), 10000.0)
        self.expect_close("balance", data.get("balance"), 10000.0)
        self.expect_close("equity", data.get("equity"), 10000.0)
        self.expect_eq("currency", data.get("currency"), "USDC")
        self.expect_eq("open positions", data.get("open_positions"), 0)
        self.expect_eq("open orders", data.get("open_orders"), 0)
        self.expect_eq("triggers", data.get("triggers"), 0)
        self.expect_eq("fills", data.get("fills"), 0)
        s = self.run(["paper", "status"]).data
        self.expect_close("status.equity", s.get("equity"), 10000.0)

    def scenario_reset_clears_state(self, symbol: str) -> None:
        self.header("Reset wipes positions, orders, triggers, fills")
        self.reset()
        self.run(["paper", "buy", symbol, "--tokens", "1", "--type", "market"])
        status_before = self.run(["paper", "status"]).data
        if status_before.get("open_positions", 0) < 1:
            self.fail(f"expected position before reset, got {status_before}")
            return
        self.reset()
        s = self.run(["paper", "status"]).data
        self.expect_eq("positions after reset", s.get("open_positions"), 0)
        self.expect_eq("fills after reset", s.get("fills"), 0)
        self.expect_close("balance after reset", s.get("balance"), 10000.0)

    # ═══════════════════════════════════════════════════════════════════
    # ORDER PLACEMENT
    # ═══════════════════════════════════════════════════════════════════
    def scenario_market_buy_opens_long(self, symbol: str) -> None:
        self.header("Market buy creates a long position with immediate fill")
        self.reset()
        r = self.run(
            ["paper", "buy", symbol, "--tokens", "1", "--type", "market"]
        )
        self.expect_eq("action", r.data.get("action"), "filled")
        self.expect_eq("order_id (market: None)", r.data.get("order_id"), None)
        fill = r.data.get("fill") or {}
        self.expect_eq("fill side", fill.get("side"), "buy")
        self.expect_eq("fill symbol", fill.get("symbol"), symbol.upper())
        self.expect_eq("fill order_type", fill.get("order_type"), "market")
        self.expect_close("fill size_tokens", fill.get("size_tokens"), 1.0)
        positions = self.run(["paper", "positions"]).data.get("positions", [])
        self.expect_eq("position count", len(positions), 1)
        if positions:
            self.expect_eq("position side", positions[0].get("side"), "long")
            self.expect_close(
                "position size_tokens", positions[0].get("size_tokens"), 1.0
            )

    def scenario_market_sell_opens_short(self, symbol: str) -> None:
        self.header("Market sell from flat creates a short position")
        self.reset()
        self.run(
            ["paper", "sell", symbol, "--tokens", "1", "--type", "market"]
        )
        positions = self.run(["paper", "positions"]).data.get("positions", [])
        self.expect_eq("position count", len(positions), 1)
        if positions:
            self.expect_eq("position side", positions[0].get("side"), "short")

    def scenario_limit_crosses_immediately(self, symbol: str) -> None:
        self.header("Crossing limit fills immediately, no resting order")
        self.reset()
        mark = self.get_mark(symbol)
        price = round(mark * 1.50, 4)
        r = self.run(
            [
                "paper", "buy", symbol,
                "--tokens", "1",
                "--type", "limit",
                "--price", str(price),
            ]
        )
        self.expect_eq("action", r.data.get("action"), "filled")
        self.expect_eq("no resting order_id", r.data.get("order_id"), None)
        if r.data.get("fill"):
            self.expect_eq(
                "fill order_type", r.data["fill"].get("order_type"), "limit"
            )
        orders = self.run(["paper", "orders"]).data.get("orders", [])
        self.expect_eq("no resting orders", len(orders), 0)

    def scenario_limit_rests(self, symbol: str) -> None:
        self.header("Non-crossing limit goes to the open orders list")
        self.reset()
        mark = self.get_mark(symbol)
        price = round(mark * 0.80, 4)
        r = self.run(
            [
                "paper", "buy", symbol,
                "--tokens", "1",
                "--type", "limit",
                "--price", str(price),
            ]
        )
        self.expect_eq("action", r.data.get("action"), "resting")
        order_id = r.data.get("order_id")
        if not order_id:
            self.fail("expected order_id on resting limit")
            return
        self.expect_eq("no fill on resting", r.data.get("fill"), None)
        orders = self.run(["paper", "orders"]).data.get("orders", [])
        self.expect_eq("orders list shows it", len(orders), 1)
        self.expect_eq("order id matches", orders[0].get("order_id"), order_id)
        positions = self.run(["paper", "positions"]).data.get("positions", [])
        self.expect_eq("no position while resting", len(positions), 0)

    def scenario_sizing_paths(self, symbol: str) -> None:
        self.header("Sizing inputs: tokens, notional_usdc, missing → error")
        self.reset()
        mark = self.get_mark(symbol)
        self.run(["paper", "buy", symbol, "--tokens", "0.5", "--type", "market"])
        pos = self.run(["paper", "positions"]).data.get("positions", [])
        if pos:
            self.expect_close("tokens sizing", pos[0].get("size_tokens"), 0.5)
        self.reset()
        notional = round(mark * 0.5, 4)
        self.run(
            [
                "paper", "buy", symbol,
                "--notional-usdc", str(notional),
                "--type", "market",
            ]
        )
        pos = self.run(["paper", "positions"]).data.get("positions", [])
        if pos:
            actual = float(pos[0].get("size_tokens", 0.0))
            self.expect_close(
                "notional sizing ≈ notional/mark", actual, 0.5, tol=0.05
            )
        r = self.run(
            ["paper", "buy", symbol, "--type", "market"], allow_error=True
        )
        self.expect_err("no size args", r, "AMBIGUOUS_SIZE")
        self.reset()
        r = self.run(
            ["paper", "buy", symbol, "--tokens", "1", "--type", "limit"],
            allow_error=True,
        )
        self.expect_err("limit without --price", r, "MISSING_PRICE")

    # ═══════════════════════════════════════════════════════════════════
    # ORDER LIFECYCLE
    # ═══════════════════════════════════════════════════════════════════
    def scenario_cancel_single_order(self, symbol: str) -> None:
        self.header("Cancel removes one specific resting order")
        self.reset()
        mark = self.get_mark(symbol)
        r = self.run(
            [
                "paper", "buy", symbol,
                "--tokens", "1",
                "--type", "limit",
                "--price", str(round(mark * 0.80, 4)),
            ]
        )
        order_id = r.data.get("order_id")
        r = self.run(["paper", "cancel", order_id])
        self.expect_eq("cancelled count", r.data.get("cancelled"), 1)
        self.expect_eq("returned id", r.data.get("order_ids"), [order_id])
        orders = self.run(["paper", "orders"]).data.get("orders", [])
        self.expect_eq("orders list empty", len(orders), 0)
        r = self.run(["paper", "cancel", order_id], allow_error=True)
        self.expect_err("cancel-of-gone", r, "PAPER_ORDER_NOT_FOUND")

    def scenario_cancel_all_by_symbol(self, symbol: str) -> None:
        self.header("cancel-all filters by symbol")
        self.reset()
        mark = self.get_mark(symbol)
        self.run(
            [
                "paper", "buy", symbol,
                "--tokens", "1",
                "--type", "limit",
                "--price", str(round(mark * 0.70, 4)),
            ]
        )
        self.run(
            [
                "paper", "buy", symbol,
                "--tokens", "1",
                "--type", "limit",
                "--price", str(round(mark * 0.60, 4)),
            ]
        )
        orders_before = self.run(["paper", "orders"]).data.get("orders", [])
        self.expect_eq("two resting orders", len(orders_before), 2)
        r = self.run(["paper", "cancel-all", symbol])
        self.expect_eq("cancelled count", r.data.get("cancelled"), 2)
        orders = self.run(["paper", "orders"]).data.get("orders", [])
        self.expect_eq("orders cleared", len(orders), 0)

    def scenario_reconcile_fills_crossing(self, symbol: str) -> None:
        self.header("Reconcile fills a limit once its price crosses")
        self.reset()
        mark = self.get_mark(symbol)
        place_price = round(mark * 0.80, 4)
        r = self.run(
            [
                "paper", "buy", symbol,
                "--tokens", "1",
                "--type", "limit",
                "--price", str(place_price),
            ]
        )
        order_id = r.data.get("order_id")
        state = self.read_state()
        crossing_price = round(mark * 1.50, 4)
        for o in state.get("orders", []):
            if o.get("order_id") == order_id:
                o["price"] = crossing_price
                break
        self.write_state(state)
        r = self.run(["paper", "reconcile", symbol])
        fills = r.data.get("fills", [])
        self.expect_eq("reconcile fills", len(fills), 1)
        if fills:
            self.expect_eq("fill order_id", fills[0].get("order_id"), order_id)
        positions = self.run(["paper", "positions"]).data.get("positions", [])
        self.expect_eq("position created from fill", len(positions), 1)
        orders = self.run(["paper", "orders"]).data.get("orders", [])
        self.expect_eq("order removed after fill", len(orders), 0)

    # ═══════════════════════════════════════════════════════════════════
    # POSITION ARITHMETIC
    # ═══════════════════════════════════════════════════════════════════
    def scenario_extend_and_partial_close(self, symbol: str) -> None:
        self.header("Extend (same side), then partial close realizes PnL")
        self.reset()
        self.run(["paper", "buy", symbol, "--tokens", "1", "--type", "market"])
        s1 = self.read_state()
        p1 = s1["positions"][0]
        entry1 = float(p1["entry_price"])
        self.info(f"first buy entry: {entry1}")

        self.run(["paper", "buy", symbol, "--tokens", "1", "--type", "market"])
        s2 = self.read_state()
        p2 = s2["positions"][0]
        self.expect_close("extended size", p2.get("size_tokens"), 2.0)
        entry2 = float(p2["entry_price"])
        mark = self.get_mark(symbol)
        lo = min(entry1, mark) - 0.01
        hi = max(entry1, mark) + 0.01
        if lo <= entry2 <= hi:
            self.ok(f"weighted entry {entry2:.4f} in [{lo:.4f}, {hi:.4f}]")
        else:
            self.fail(
                f"weighted entry {entry2:.4f} outside [{lo:.4f}, {hi:.4f}]"
            )

        r = self.run(
            ["paper", "sell", symbol, "--tokens", "0.5", "--type", "market"]
        )
        fill = r.data.get("fill") or {}
        sell_price = float(fill.get("price", 0.0))
        expected_pnl = (sell_price - entry2) * 0.5
        actual_pnl = float(fill.get("realized_pnl", 0.0))
        self.expect_close(
            "realized PnL = (sell - entry) * size",
            actual_pnl, expected_pnl, tol=1e-3,
        )
        positions = self.run(["paper", "positions"]).data.get("positions", [])
        if positions:
            self.expect_close(
                "remaining size", positions[0].get("size_tokens"), 1.5
            )

    def scenario_flip_side(self, symbol: str) -> None:
        self.header("Opposite-side order larger than position flips side")
        self.reset()
        self.run(["paper", "buy", symbol, "--tokens", "1", "--type", "market"])
        self.run(["paper", "sell", symbol, "--tokens", "2", "--type", "market"])
        positions = self.run(["paper", "positions"]).data.get("positions", [])
        self.expect_eq("position count", len(positions), 1)
        if positions:
            self.expect_eq("flipped to short", positions[0].get("side"), "short")
            self.expect_close(
                "new size = sell - long", positions[0].get("size_tokens"), 1.0
            )

    def scenario_full_close_removes_position(self, symbol: str) -> None:
        self.header("Closing fully removes the position from state")
        self.reset()
        self.run(["paper", "buy", symbol, "--tokens", "1", "--type", "market"])
        self.run(["paper", "sell", symbol, "--tokens", "1", "--type", "market"])
        positions = self.run(["paper", "positions"]).data.get("positions", [])
        self.expect_eq("position removed", len(positions), 0)
        fills = self.run(["paper", "fills"]).data.get("fills", [])
        self.expect_eq("two fills recorded", len(fills), 2)

    # ═══════════════════════════════════════════════════════════════════
    # FEES & STATUS
    # ═══════════════════════════════════════════════════════════════════
    def scenario_fees_charged(self, symbol: str) -> None:
        self.header("Non-zero fee_bps debits balance per fill")
        self.reset(fee_bps=10.0)
        r = self.run(
            ["paper", "buy", symbol, "--tokens", "1", "--type", "market"]
        )
        fill = r.data.get("fill") or {}
        price = float(fill.get("price", 0.0))
        size = float(fill.get("size_tokens", 0.0))
        actual_fee = float(fill.get("fee", 0.0))
        expected_fee = price * size * 10.0 / 10000.0
        self.expect_close(
            "fee = price * size * bps / 10000",
            actual_fee, expected_fee, tol=1e-4,
        )
        status = self.run(["paper", "status"]).data
        self.expect_close(
            "balance reduced by fee", status.get("balance"), 10000.0 - actual_fee,
        )
        self.expect_close(
            "fees_paid", status.get("fees_paid"), actual_fee
        )

    def scenario_fills_limit(self, symbol: str) -> None:
        self.header("paper fills --limit caps the returned slice")
        self.reset()
        for _ in range(5):
            self.run(["paper", "buy", symbol, "--tokens", "0.1", "--type", "market"])
        all_fills = self.run(["paper", "fills"]).data.get("fills", [])
        self.expect_eq("default returns all 5", len(all_fills), 5)
        capped = self.run(["paper", "fills", "--limit", "2"]).data.get("fills", [])
        self.expect_eq("limit=2 returns 2", len(capped), 2)
        if capped and all_fills:
            self.expect_eq(
                "limit returns most recent",
                capped[-1].get("fill_id"),
                all_fills[-1].get("fill_id"),
            )

    def scenario_status_fields_reflect_state(self, symbol: str) -> None:
        self.header("Status equity / unrealized_pnl / exposure track a position")
        self.reset()
        self.run(
            ["paper", "buy", symbol, "--tokens", "1", "--type", "market"]
        )
        # Use a SINGLE status snapshot — each `paper status` re-fetches the
        # ticker, so cross-referencing between calls would race against mark
        # drift. All fields below come from one consistent snapshot.
        status = self.run(["paper", "status"]).data
        balance = float(status.get("balance", 0.0))
        upnl = float(status.get("unrealized_pnl", 0.0))
        equity = float(status.get("equity", 0.0))
        notional = float(status.get("position_notional_usdc", 0.0))
        exposure = float(status.get("exposure_ratio", 0.0))
        self.expect_close("equity = balance + uPnL", equity, balance + upnl)
        self.expect_eq("open_positions", status.get("open_positions"), 1)
        self.expect_eq("open_orders", status.get("open_orders"), 0)
        if abs(equity) > 1e-9:
            self.expect_close(
                "exposure_ratio = notional / equity",
                exposure, notional / equity, tol=1e-6,
            )
        if notional <= 0.0:
            self.fail(f"expected positive position_notional_usdc, got {notional}")
        else:
            self.ok(f"position_notional_usdc positive: {notional:.4f}")

    # ═══════════════════════════════════════════════════════════════════
    # TP / SL
    # ═══════════════════════════════════════════════════════════════════
    def scenario_market_with_tpsl(self, symbol: str) -> None:
        self.header("Market buy with order-time TP/SL → active triggers")
        self.reset()
        mark = self.get_mark(symbol)
        tp = round(mark * 1.10, 4)
        sl = round(mark * 0.90, 4)
        self.info(f"mark={mark} tp={tp} sl={sl}")

        r = self.run(
            [
                "paper", "buy", symbol,
                "--tokens", "1",
                "--type", "market",
                "--tp", str(tp),
                "--sl", str(sl),
            ]
        )
        attached = r.data.get("attached_triggers", [])
        self.expect_eq("attached triggers count", len(attached), 2)
        if attached:
            kinds = sorted(t["kind"] for t in attached)
            self.expect_eq("attached kinds", kinds, ["stop_loss", "take_profit"])
            if all(t.get("parent_order_id") is None for t in attached):
                self.ok("all triggers active (parent_order_id=None)")
            else:
                self.fail("expected all triggers active")
        pos = self.run(["paper", "positions"]).data.get("positions", [])
        if pos:
            self.expect_eq(
                "position tp_levels", len(pos[0].get("tp_levels", [])), 1
            )
            self.expect_eq(
                "position sl_levels", len(pos[0].get("sl_levels", [])), 1
            )
        self.expect_eq(
            "status.triggers after order",
            r.data.get("state", {}).get("triggers"), 2,
        )

    def scenario_resting_limit_pending_tpsl(self, symbol: str) -> None:
        self.header("Resting limit with TP/SL → pending triggers")
        self.reset()
        mark = self.get_mark(symbol)
        price = round(mark * 0.80, 4)
        tp = round(price * 1.10, 4)
        sl = round(price * 0.90, 4)
        r = self.run(
            [
                "paper", "buy", symbol,
                "--tokens", "1",
                "--type", "limit",
                "--price", str(price),
                "--tp", str(tp),
                "--sl", str(sl),
            ]
        )
        order_id = r.data.get("order_id")
        if not order_id:
            self.fail("expected resting limit")
            return
        attached = r.data.get("attached_triggers", [])
        parents = [t.get("parent_order_id") for t in attached]
        if all(p == order_id for p in parents) and len(parents) == 2:
            self.ok(f"both triggers parented to {order_id}")
        else:
            self.fail(f"parents={parents}")
        pos = self.run(["paper", "positions"]).data.get("positions", [])
        self.expect_eq("no position while limit rests", len(pos), 0)
        triggers = self.run(["paper", "triggers"]).data.get("triggers", [])
        self.expect_eq("triggers list size", len(triggers), 2)

    def scenario_reduce_only_tpsl_rejection(self, symbol: str) -> None:
        self.header("Order-time TP/SL on reduce-only order → rejected")
        self.reset()
        mark = self.get_mark(symbol)
        self.run(["paper", "buy", symbol, "--tokens", "1", "--type", "market"])
        sl = round(mark * 0.90, 4)
        r = self.run(
            [
                "paper", "sell", symbol,
                "--tokens", "0.5",
                "--type", "market",
                "--sl", str(sl),
            ],
            allow_error=True,
        )
        self.expect_err("reduce-only with TP/SL", r, "TPSL_ORDER_REDUCES")
        pos = self.run(["paper", "positions"]).data.get("positions", [])
        if pos and abs(float(pos[0].get("size_tokens", 0.0)) - 1.0) < 1e-6:
            self.ok("position untouched after rejection")
        else:
            self.fail(f"position changed: {pos}")

    def scenario_tpsl_direction_validation(self, symbol: str) -> None:
        self.header("set-tpsl direction validation")
        self.reset()
        mark = self.get_mark(symbol)
        self.run(["paper", "buy", symbol, "--tokens", "1", "--type", "market"])
        r = self.run(
            ["paper", "set-tpsl", symbol, "--tp", str(round(mark * 0.90, 4))],
            allow_error=True,
        )
        self.expect_err("long TP below entry", r, "TPSL_WRONG_DIRECTION")
        r = self.run(
            ["paper", "set-tpsl", symbol, "--sl", str(round(mark * 1.10, 4))],
            allow_error=True,
        )
        self.expect_err("long SL above entry", r, "TPSL_WRONG_DIRECTION")

    def scenario_multi_level_set_tpsl(self, symbol: str) -> None:
        self.header("Multi-level set-tpsl + validation")
        self.reset()
        mark = self.get_mark(symbol)
        self.run(["paper", "buy", symbol, "--tokens", "1", "--type", "market"])
        tp1, tp2 = round(mark * 1.10, 4), round(mark * 1.20, 4)
        sl = round(mark * 0.90, 4)
        r = self.run(
            [
                "paper", "set-tpsl", symbol,
                "--tp-level", f"{tp1}:0.5",
                "--tp-level", f"{tp2}:0.5",
                "--sl-level", str(sl),
            ]
        )
        self.expect_eq("multi-level TP count", len(r.data.get("tp_levels", [])), 2)
        self.expect_eq("multi-level SL count", len(r.data.get("sl_levels", [])), 1)
        pos = self.run(["paper", "positions"]).data.get("positions", [])
        if pos:
            self.expect_eq(
                "position projects 2 tp_levels",
                len(pos[0].get("tp_levels", [])), 2,
            )
        self.reset()
        self.run(["paper", "buy", symbol, "--tokens", "1", "--type", "market"])
        r = self.run(
            [
                "paper", "set-tpsl", symbol,
                "--tp-level", f"{tp1}:0.7",
                "--tp-level", f"{tp2}:0.5",
            ],
            allow_error=True,
        )
        self.expect_err("sum > position", r, "TP_SIZE_EXCEEDS_POSITION")
        r = self.run(
            [
                "paper", "set-tpsl", symbol,
                "--tp", str(tp1),
                "--tp-level", f"{tp2}:0.5",
            ],
            allow_error=True,
        )
        self.expect_err("--tp + --tp-level", r, "TPSL_FLAG_CONFLICT")
        r = self.run(
            [
                "paper", "set-tpsl", symbol,
                "--tp-level", str(tp1),
                "--tp-level", f"{tp2}:0.5",
            ],
            allow_error=True,
        )
        self.expect_err(
            "Full + multi-level", r, "TPSL_FULL_WITH_MULTI_LEVEL"
        )

    def scenario_cascade_cancel(self, symbol: str) -> None:
        self.header("cancel <order_id> cascades to pending TP/SL")
        self.reset()
        mark = self.get_mark(symbol)
        price = round(mark * 0.80, 4)
        r = self.run(
            [
                "paper", "buy", symbol,
                "--tokens", "1",
                "--type", "limit",
                "--price", str(price),
                "--tp", str(round(price * 1.10, 4)),
                "--sl", str(round(price * 0.90, 4)),
            ]
        )
        order_id = r.data.get("order_id")
        r = self.run(["paper", "cancel", order_id])
        self.expect_eq(
            "cancelled (1 order + 2 triggers)", r.data.get("cancelled"), 3
        )
        triggers = self.run(["paper", "triggers"]).data.get("triggers", [])
        self.expect_eq("triggers cleared", len(triggers), 0)
        orders = self.run(["paper", "orders"]).data.get("orders", [])
        self.expect_eq("orders cleared", len(orders), 0)

    def scenario_trigger_fires_on_reconcile(self, symbol: str) -> None:
        self.header("Trigger fires on reconcile when mark crosses (forced)")
        self.reset()
        self.run(["paper", "buy", symbol, "--tokens", "1", "--type", "market"])
        mark = self.get_mark(symbol)
        sl = round(mark * 0.90, 4)
        self.run(["paper", "set-tpsl", symbol, "--sl", str(sl)])
        state = self.read_state()
        forced_sl = round(mark * 1.05, 4)
        for t in state.get("triggers", []):
            if t.get("kind") == "stop_loss" and t.get("symbol") == symbol.upper():
                t["trigger_price"] = forced_sl
                break
        self.write_state(state)
        r = self.run(["paper", "reconcile", symbol])
        fills = r.data.get("fills", [])
        sl_fills = [
            f for f in fills if f.get("order_id", "").startswith("paper-trigger-")
        ]
        self.expect_eq("SL fills on reconcile", len(sl_fills), 1)
        if sl_fills:
            self.expect_close(
                "fill price == trigger price",
                float(sl_fills[0]["price"]), forced_sl,
            )
        pos = self.run(["paper", "positions"]).data.get("positions", [])
        self.expect_eq("position closed after SL", len(pos), 0)
        triggers = self.run(["paper", "triggers"]).data.get("triggers", [])
        self.expect_eq("trigger purged after fire", len(triggers), 0)

    def scenario_candle_replay_on_stale_state(self, symbol: str) -> None:
        self.header("Reconcile replays historical candles when state is stale")
        self.reset()
        # Open a position so triggers have a position to act on.
        self.run(["paper", "buy", symbol, "--tokens", "1", "--type", "market"])
        mark = self.get_mark(symbol)
        # Fetch recent 1h candles so we can place a trigger at a price the
        # market has actually visited in the gap window.
        candles_r = self.run(
            ["market", "candles", symbol, "--interval", "1h", "--limit", "24"]
        )
        candles = candles_r.data.get("candles", [])
        if not candles:
            self.fail("could not fetch recent candles to set up replay scenario")
            return
        recent_high = max(float(c["high"]) for c in candles)
        recent_low = min(float(c["low"]) for c in candles)
        self.info(
            f"24h range: low={recent_low:.4f} high={recent_high:.4f} mark={mark:.4f}"
        )

        # Pick a TP between current mark and the 24h high — guaranteed to have
        # been visited at some point if the market moved at all.
        if recent_high > mark * 1.001:
            tp_price = round((mark + recent_high) / 2, 4)
            self.run(["paper", "set-tpsl", symbol, "--tp", str(tp_price)])
            self.info(f"set TP at {tp_price} (in [mark, 24h high])")
        else:
            self.info(
                "market hasn't risen meaningfully in 24h; skipping TP-fires assertion"
            )
            tp_price = None

        # Backdate last_evaluated_at and the trigger's created_at so the replay
        # window covers the 24h of candles we just inspected.
        state = self.read_state()
        backdated = "2020-01-01T00:00:00Z"
        state["last_evaluated_at"] = backdated
        for t in state.get("triggers", []):
            t["created_at"] = backdated
        self.write_state(state)

        r = self.run(["paper", "reconcile", symbol])
        # last_evaluated_at must be bumped past the backdate.
        state_after = self.read_state()
        if state_after.get("last_evaluated_at", "").startswith("2020"):
            self.fail("last_evaluated_at not updated by reconcile")
        else:
            self.ok(
                f"last_evaluated_at advanced: {state_after.get('last_evaluated_at')}"
            )

        # If we set a TP that was crossable, expect at least one historical fill.
        if tp_price is not None:
            trigger_fills = [
                f
                for f in r.data.get("fills", [])
                if f.get("order_id", "").startswith("paper-trigger-")
            ]
            if not trigger_fills:
                self.fail(
                    f"expected TP@{tp_price} to fire (recent high was {recent_high})"
                )
            else:
                f = trigger_fills[0]
                self.ok(
                    f"replay fired {len(trigger_fills)} trigger(s); first @ {f['price']}"
                )
                # Fill price must equal trigger price.
                if abs(float(f["price"]) - tp_price) < 1e-6:
                    self.ok("fill at trigger_price, not candle close")
                else:
                    self.fail(
                        f"fill price {f['price']} != trigger price {tp_price}"
                    )
                # Fill timestamp must be in the past — at least 30 minutes
                # behind "now" to confirm it came from a candle close and not
                # the post-replay current-mark eval.
                import datetime as _dt
                try:
                    fill_ts = _dt.datetime.fromisoformat(
                        f["timestamp"].replace("Z", "+00:00")
                    )
                    age_s = (
                        _dt.datetime.now(_dt.timezone.utc) - fill_ts
                    ).total_seconds()
                    if age_s > 1800:
                        self.ok(
                            f"fill timestamp is historical ({age_s/3600:.1f}h ago): {f['timestamp']}"
                        )
                    else:
                        self.fail(
                            f"fill timestamp too recent ({age_s:.0f}s ago): {f['timestamp']}"
                        )
                except ValueError:
                    self.fail(f"could not parse fill timestamp: {f.get('timestamp')}")

    def scenario_grid_attaches_tpsl_per_level(self, symbol: str) -> None:
        self.header("vulcan strategy grid in paper attaches TP/SL per level")
        self.reset()
        mark = self.get_mark(symbol)
        levels_per_side = 2
        tokens_per_level = 0.1
        tp_spacing = round(mark * 0.02, 4)
        sl_spacing = round(mark * 0.02, 4)
        r = self.run(
            [
                "strategy", "grid", "start",
                "--symbol", symbol,
                "--mode", "paper",
                "--center-on-mark",
                "--width-pct", "2",
                "--levels-per-side", str(levels_per_side),
                "--tokens-per-level", str(tokens_per_level),
                "--take-profit-spacing", str(tp_spacing),
                "--stop-loss-spacing", str(sl_spacing),
                "--ticks", "1",
                "--interval-seconds", "1",
            ]
        )
        self.info(f"grid run: {r.data.get('status', '?')}")
        # 2 bid levels + 2 ask levels = 4 entry orders.
        orders = self.run(["paper", "orders"]).data.get("orders", [])
        expected_orders = levels_per_side * 2
        self.expect_eq("grid entry orders resting", len(orders), expected_orders)

        # Each order should have one pending TP and one pending SL attached.
        triggers = self.run(["paper", "triggers"]).data.get("triggers", [])
        expected_triggers = expected_orders * 2
        self.expect_eq("grid TP+SL triggers", len(triggers), expected_triggers)

        order_ids = {o["order_id"] for o in orders}
        order_by_id = {o["order_id"]: o for o in orders}
        per_order: dict[str, dict[str, list]] = {oid: {"tp": [], "sl": []} for oid in order_ids}
        for t in triggers:
            parent = t.get("parent_order_id")
            if parent not in order_ids:
                self.fail(f"trigger has no/unknown parent: {parent}")
                continue
            per_order[parent][
                "tp" if t["kind"] == "take_profit" else "sl"
            ].append(t)

        per_order_correct = True
        for oid, buckets in per_order.items():
            if len(buckets["tp"]) != 1 or len(buckets["sl"]) != 1:
                per_order_correct = False
                self.fail(
                    f"order {oid}: tp={len(buckets['tp'])} sl={len(buckets['sl'])}"
                )
                continue
            order = order_by_id[oid]
            entry = float(order["price"])
            side = order["side"]
            tp_price = float(buckets["tp"][0]["trigger_price"])
            sl_price = float(buckets["sl"][0]["trigger_price"])
            # For a buy level (long): TP > entry, SL < entry.
            # For a sell level (short): TP < entry, SL > entry.
            if side == "buy":
                ok = tp_price > entry and sl_price < entry
            else:
                ok = tp_price < entry and sl_price > entry
            if not ok:
                per_order_correct = False
                self.fail(
                    f"order {oid} ({side}@{entry}): tp={tp_price} sl={sl_price} wrong direction"
                )
        if per_order_correct:
            self.ok(f"all {expected_orders} grid levels have correctly-directed TP+SL")

    def scenario_reparent_pending_on_fill(self, symbol: str) -> None:
        self.header("Pending triggers re-parent when their limit fills")
        self.reset()
        mark = self.get_mark(symbol)
        place_price = round(mark * 0.80, 4)
        r = self.run(
            [
                "paper", "buy", symbol,
                "--tokens", "1",
                "--type", "limit",
                "--price", str(place_price),
                "--tp", str(round(place_price * 1.20, 4)),
                "--sl", str(round(place_price * 0.90, 4)),
            ]
        )
        order_id = r.data.get("order_id")
        state = self.read_state()
        crossing = round(mark * 1.50, 4)
        safe_tp = round(mark * 1.20, 4)
        safe_sl = round(mark * 0.50, 4)
        for o in state.get("orders", []):
            if o.get("order_id") == order_id:
                o["price"] = crossing
        for t in state.get("triggers", []):
            if t.get("parent_order_id") == order_id:
                t["trigger_price"] = safe_tp if t["kind"] == "take_profit" else safe_sl
        self.write_state(state)
        r = self.run(["paper", "reconcile", symbol])
        order_fills = [
            f for f in r.data.get("fills", []) if f.get("order_id") == order_id
        ]
        self.expect_eq("limit fills on reconcile", len(order_fills), 1)
        triggers = self.run(["paper", "triggers"]).data.get("triggers", [])
        self.expect_eq("total triggers preserved", len(triggers), 2)
        active = [t for t in triggers if t.get("parent_order_id") is None]
        self.expect_eq("re-parented to position", len(active), 2)


def main() -> int:
    parser = argparse.ArgumentParser(description=__doc__.splitlines()[0])
    parser.add_argument("symbol", help="market symbol (e.g. SOL)")
    parser.add_argument(
        "--vulcan-bin",
        default="./target/debug/vulcan",
        help="path to the vulcan binary (default: ./target/debug/vulcan)",
    )
    parser.add_argument("--api-url", help="Phoenix API URL override")
    parser.add_argument(
        "--timeout", type=int, default=60, help="per-command timeout in seconds"
    )
    parser.add_argument("--no-color", action="store_true")
    args = parser.parse_args()

    if not Path(args.vulcan_bin).exists():
        print(
            f"vulcan binary not found at {args.vulcan_bin}; build it first with "
            "`cargo build -p vulcan` or pass --vulcan-bin",
            file=sys.stderr,
        )
        return 2

    smoke = Smoke(args)
    try:
        smoke.section_header("Account lifecycle")
        smoke.scenario_init_and_status(args.symbol)
        smoke.scenario_reset_clears_state(args.symbol)

        smoke.section_header("Order placement")
        smoke.scenario_market_buy_opens_long(args.symbol)
        smoke.scenario_market_sell_opens_short(args.symbol)
        smoke.scenario_limit_crosses_immediately(args.symbol)
        smoke.scenario_limit_rests(args.symbol)
        smoke.scenario_sizing_paths(args.symbol)

        smoke.section_header("Order lifecycle")
        smoke.scenario_cancel_single_order(args.symbol)
        smoke.scenario_cancel_all_by_symbol(args.symbol)
        smoke.scenario_reconcile_fills_crossing(args.symbol)

        smoke.section_header("Position arithmetic")
        smoke.scenario_extend_and_partial_close(args.symbol)
        smoke.scenario_flip_side(args.symbol)
        smoke.scenario_full_close_removes_position(args.symbol)

        smoke.section_header("Fees & status")
        smoke.scenario_fees_charged(args.symbol)
        smoke.scenario_fills_limit(args.symbol)
        smoke.scenario_status_fields_reflect_state(args.symbol)

        smoke.section_header("TP / SL")
        smoke.scenario_market_with_tpsl(args.symbol)
        smoke.scenario_resting_limit_pending_tpsl(args.symbol)
        smoke.scenario_reduce_only_tpsl_rejection(args.symbol)
        smoke.scenario_tpsl_direction_validation(args.symbol)
        smoke.scenario_multi_level_set_tpsl(args.symbol)
        smoke.scenario_cascade_cancel(args.symbol)
        smoke.scenario_trigger_fires_on_reconcile(args.symbol)
        smoke.scenario_reparent_pending_on_fill(args.symbol)

        smoke.section_header("Cross-session replay")
        smoke.scenario_candle_replay_on_stale_state(args.symbol)

        smoke.section_header("Strategy integration")
        smoke.scenario_grid_attaches_tpsl_per_level(args.symbol)
    finally:
        smoke.close()

    print(f"\n{len(smoke.passes)} passed, {len(smoke.failures)} failed")
    if smoke.failures:
        print("\nfailures:")
        for f in smoke.failures:
            print(f"  - {f}")
        return 1
    return 0


if __name__ == "__main__":
    sys.exit(main())

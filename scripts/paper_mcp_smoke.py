#!/usr/bin/env python3
# /// script
# requires-python = ">=3.11"
# ///
"""Paper-mode TP/SL smoke test over the MCP transport.

The CLI smoke test (paper_smoke.py) drives the CLI dispatch path. This script
drives the *MCP* dispatch path by speaking JSON-RPC over stdio to a running
`vulcan mcp` server. Verifies that the MCP arg parsers (arg_tpsl_levels, the
paper-trade dispatcher, etc.) haven't drifted from the CLI parsers.

No funds, no wallet, no Solana transactions.

Usage:
    ./scripts/paper_mcp_smoke.py SOL
"""

from __future__ import annotations

import argparse
import json
import os
import subprocess
import sys
import tempfile
import threading
import time
from dataclasses import dataclass
from pathlib import Path
from queue import Empty, Queue
from typing import Any


@dataclass
class McpResponse:
    payload: dict[str, Any]

    @property
    def result(self) -> dict[str, Any] | None:
        return self.payload.get("result")

    @property
    def error(self) -> dict[str, Any] | None:
        return self.payload.get("error")

    def tool_data(self) -> dict[str, Any]:
        """Decode the inner payload returned by a vulcan tool call.

        Returns a uniform `{"ok": bool, "data": {...}}` or
        `{"ok": false, "error": {code, message, ...}}` shape regardless of
        whether MCP signalled success or a tool error.
        """
        if self.error:
            return {"ok": False, "error": self.error}
        result = self.result or {}
        content = result.get("content") or []
        if not (content and content[0].get("type") == "text"):
            return {"ok": True, "data": result}
        try:
            payload = json.loads(content[0]["text"])
        except json.JSONDecodeError:
            return {"ok": True, "data": {"_raw": content[0]["text"]}}
        if result.get("isError"):
            return {"ok": False, "error": payload}
        return {"ok": True, "data": payload}


class McpClient:
    def __init__(self, bin_path: str, home: str, api_url: str | None) -> None:
        env = os.environ.copy()
        env["HOME"] = home
        cmd = [bin_path]
        if api_url:
            cmd.extend(["--api-url", api_url])
        cmd.append("mcp")
        self.proc = subprocess.Popen(
            cmd,
            env=env,
            stdin=subprocess.PIPE,
            stdout=subprocess.PIPE,
            stderr=subprocess.PIPE,
            text=True,
            bufsize=1,
        )
        self.responses: Queue[dict[str, Any]] = Queue()
        self.next_id = 1
        self._reader = threading.Thread(target=self._read_loop, daemon=True)
        self._reader.start()
        self._stderr_buf: list[str] = []
        threading.Thread(target=self._stderr_loop, daemon=True).start()

    def _read_loop(self) -> None:
        assert self.proc.stdout is not None
        for line in self.proc.stdout:
            line = line.strip()
            if not line:
                continue
            try:
                self.responses.put(json.loads(line))
            except json.JSONDecodeError:
                # Non-JSON line (e.g., tracing output); ignore.
                pass

    def _stderr_loop(self) -> None:
        assert self.proc.stderr is not None
        for line in self.proc.stderr:
            self._stderr_buf.append(line)

    def close(self) -> None:
        try:
            if self.proc.stdin:
                self.proc.stdin.close()
            self.proc.wait(timeout=5)
        except subprocess.TimeoutExpired:
            self.proc.kill()
        if self.proc.returncode not in (0, None):
            stderr = "".join(self._stderr_buf[-50:])
            sys.stderr.write(
                f"\nMCP server exited with code {self.proc.returncode}; "
                f"last stderr:\n{stderr}\n"
            )

    def _send(self, payload: dict[str, Any]) -> None:
        assert self.proc.stdin is not None
        self.proc.stdin.write(json.dumps(payload) + "\n")
        self.proc.stdin.flush()

    def _await(self, request_id: int, timeout: float = 30.0) -> McpResponse:
        deadline = time.monotonic() + timeout
        while time.monotonic() < deadline:
            try:
                msg = self.responses.get(timeout=0.5)
            except Empty:
                continue
            if msg.get("id") == request_id:
                return McpResponse(msg)
            # Notification or unrelated message — ignore.
        raise SystemExit(f"MCP response for id={request_id} timed out")

    def request(self, method: str, params: dict[str, Any] | None = None) -> McpResponse:
        req_id = self.next_id
        self.next_id += 1
        self._send(
            {
                "jsonrpc": "2.0",
                "id": req_id,
                "method": method,
                "params": params or {},
            }
        )
        return self._await(req_id)

    def notify(self, method: str, params: dict[str, Any] | None = None) -> None:
        self._send(
            {"jsonrpc": "2.0", "method": method, "params": params or {}}
        )

    def initialize(self) -> McpResponse:
        r = self.request(
            "initialize",
            {
                "protocolVersion": "2024-11-05",
                "capabilities": {},
                "clientInfo": {"name": "paper-mcp-smoke", "version": "0.1"},
            },
        )
        # Standard MCP handshake — client sends 'notifications/initialized'.
        self.notify("notifications/initialized")
        return r

    def list_tools(self) -> McpResponse:
        return self.request("tools/list")

    def call(self, name: str, arguments: dict[str, Any] | None = None) -> McpResponse:
        return self.request(
            "tools/call", {"name": name, "arguments": arguments or {}}
        )


# ── smoke harness ────────────────────────────────────────────────────────


def color(text: str, code: str, enabled: bool) -> str:
    return f"\033[{code}m{text}\033[0m" if enabled else text


class Smoke:
    def __init__(self, client: McpClient, args: argparse.Namespace) -> None:
        self.client = client
        self.args = args
        self.use_color = sys.stdout.isatty() and not args.no_color
        self.passes: list[str] = []
        self.failures: list[str] = []

    def header(self, msg: str) -> None:
        print(color(f"\n· {msg}", "1;36", self.use_color))

    def ok(self, msg: str) -> None:
        print(color(f"  ✓ {msg}", "32", self.use_color))
        self.passes.append(msg)

    def fail(self, msg: str) -> None:
        print(color(f"  ✗ {msg}", "31", self.use_color))
        self.failures.append(msg)

    def info(self, msg: str) -> None:
        print(color(f"    {msg}", "90", self.use_color))

    def call_ok(self, name: str, args_in: dict[str, Any]) -> dict[str, Any]:
        r = self.client.call(name, args_in)
        data = r.tool_data()
        if not data.get("ok"):
            raise SystemExit(
                f"MCP call {name} failed: {json.dumps(data, indent=2)}"
            )
        return data.get("data") or {}

    def call_err(
        self, name: str, args_in: dict[str, Any], expected_code: str, label: str
    ) -> None:
        r = self.client.call(name, args_in)
        data = r.tool_data()
        if data.get("ok"):
            self.fail(f"{label}: expected {expected_code}, call succeeded")
            return
        code = (data.get("error") or {}).get("code")
        if code != expected_code:
            self.fail(f"{label}: expected {expected_code}, got {code}")
            return
        self.ok(f"{label}: rejected with {expected_code}")

    def get_mark(self, symbol: str) -> float:
        data = self.call_ok("vulcan_market_ticker", {"symbol": symbol})
        return float(data["mark_price"])

    # ── scenarios ─────────────────────────────────────────────────────────
    def scenario_initialize_and_list_tools(self) -> None:
        self.header("MCP initialize + tools/list includes new paper tools")
        init = self.client.initialize()
        if not init.result:
            self.fail(f"initialize returned no result: {init.payload}")
            return
        proto = init.result.get("protocolVersion")
        if proto:
            self.ok(f"initialized with protocol {proto}")
        else:
            self.fail("no protocolVersion in initialize result")
            return
        tools_resp = self.client.list_tools()
        if not tools_resp.result:
            self.fail(f"tools/list returned no result: {tools_resp.payload}")
            return
        tools = tools_resp.result.get("tools") or []
        names = {t.get("name") for t in tools}
        for required in (
            "vulcan_paper_init",
            "vulcan_paper_trade",
            "vulcan_paper_set_tpsl",
            "vulcan_paper_cancel_tpsl",
            "vulcan_paper_triggers",
            "vulcan_paper_cancel",
        ):
            if required in names:
                self.ok(f"tool registered: {required}")
            else:
                self.fail(f"tool MISSING from MCP list: {required}")

    def scenario_paper_init(self) -> None:
        self.header("vulcan_paper_init creates account via MCP")
        data = self.call_ok(
            "vulcan_paper_init", {"balance": 10000, "fee_bps": 0}
        )
        state = data.get("state") or {}
        if state.get("starting_balance") == 10000.0:
            self.ok("starting_balance = 10000")
        else:
            self.fail(f"unexpected starting_balance: {state.get('starting_balance')}")

    def scenario_paper_trade_with_tpsl(self, symbol: str) -> None:
        self.header("vulcan_paper_trade with tp/sl attaches active triggers")
        mark = self.get_mark(symbol)
        tp = round(mark * 1.10, 4)
        sl = round(mark * 0.90, 4)
        self.info(f"mark={mark} tp={tp} sl={sl}")
        data = self.call_ok(
            "vulcan_paper_trade",
            {
                "symbol": symbol,
                "side": "buy",
                "order_type": "market",
                "tokens": 1.0,
                "tp": tp,
                "sl": sl,
            },
        )
        attached = data.get("attached_triggers") or []
        if len(attached) == 2 and all(t.get("parent_order_id") is None for t in attached):
            self.ok("2 active triggers attached")
        else:
            self.fail(f"unexpected attached_triggers: {attached}")

    def scenario_paper_set_tpsl_multi_level(self, symbol: str) -> None:
        self.header("vulcan_paper_set_tpsl multi-level via tp_levels array")
        # Reset for a clean position.
        self.call_ok("vulcan_paper_init", {"balance": 10000, "fee_bps": 0})
        self.call_ok(
            "vulcan_paper_trade",
            {
                "symbol": symbol,
                "side": "buy",
                "order_type": "market",
                "tokens": 1.0,
            },
        )
        mark = self.get_mark(symbol)
        tp1, tp2 = round(mark * 1.10, 4), round(mark * 1.20, 4)
        sl = round(mark * 0.90, 4)
        data = self.call_ok(
            "vulcan_paper_set_tpsl",
            {
                "symbol": symbol,
                "tp_levels": [
                    {"price": tp1, "size": 0.5},
                    {"price": tp2, "size": 0.5},
                ],
                "sl_levels": [{"price": sl}],
            },
        )
        tp_levels = data.get("tp_levels") or []
        sl_levels = data.get("sl_levels") or []
        if len(tp_levels) == 2:
            self.ok(f"2 TP levels accepted: {[lv.get('price') for lv in tp_levels]}")
        else:
            self.fail(f"expected 2 TP levels, got {len(tp_levels)}")
        if len(sl_levels) == 1:
            self.ok(f"1 SL level accepted: {sl_levels[0].get('price')}")
        else:
            self.fail(f"expected 1 SL level, got {len(sl_levels)}")

    def scenario_paper_triggers_lists_active(self, symbol: str) -> None:
        self.header("vulcan_paper_triggers lists active triggers")
        data = self.call_ok("vulcan_paper_triggers", {"symbol": symbol})
        triggers = data.get("triggers") or []
        if len(triggers) >= 3:
            self.ok(f"{len(triggers)} triggers listed for {symbol}")
        else:
            self.fail(f"expected ≥3 triggers, got {len(triggers)}")
        if all(t.get("parent_order_id") is None for t in triggers):
            self.ok("all triggers active (no pending parent)")
        else:
            self.fail("expected all active triggers")

    def scenario_paper_cancel_tpsl(self, symbol: str) -> None:
        self.header("vulcan_paper_cancel_tpsl by side")
        data = self.call_ok(
            "vulcan_paper_cancel_tpsl",
            {"symbol": symbol, "tp": True, "sl": False},
        )
        if data.get("cancelled", 0) >= 2:
            self.ok(f"cancelled {data['cancelled']} TP triggers")
        else:
            self.fail(f"expected ≥2 cancelled, got {data.get('cancelled')}")
        # After cancelling TP, only SL should remain.
        triggers = self.call_ok("vulcan_paper_triggers", {"symbol": symbol}).get(
            "triggers"
        ) or []
        kinds = {t.get("kind") for t in triggers}
        if kinds == {"stop_loss"}:
            self.ok("only stop_loss remains")
        else:
            self.fail(f"expected only stop_loss, got kinds={kinds}")

    def scenario_paper_cancel_tpsl_neither_side(self, symbol: str) -> None:
        self.header("cancel_tpsl with neither side → error")
        self.call_err(
            "vulcan_paper_cancel_tpsl",
            {"symbol": symbol, "tp": False, "sl": False},
            "TPSL_CANCEL_NOTHING",
            "neither tp nor sl",
        )

    def scenario_paper_trade_reduce_only_rejection(self, symbol: str) -> None:
        self.header("paper_trade with tp/sl on reduce-only order → error")
        # Position should be 1 long from earlier scenarios; sell with tp/sl
        # should be rejected.
        mark = self.get_mark(symbol)
        self.call_err(
            "vulcan_paper_trade",
            {
                "symbol": symbol,
                "side": "sell",
                "order_type": "market",
                "tokens": 0.5,
                "sl": round(mark * 0.90, 4),
            },
            "TPSL_ORDER_REDUCES",
            "reduce-only with tp/sl",
        )


def main() -> int:
    parser = argparse.ArgumentParser(description=__doc__.splitlines()[0])
    parser.add_argument("symbol", help="market symbol (e.g. SOL)")
    parser.add_argument(
        "--vulcan-bin",
        default="./target/debug/vulcan",
        help="path to the vulcan binary (default: ./target/debug/vulcan)",
    )
    parser.add_argument("--api-url", help="Phoenix API URL override")
    parser.add_argument("--no-color", action="store_true")
    args = parser.parse_args()

    if not Path(args.vulcan_bin).exists():
        print(
            f"vulcan binary not found at {args.vulcan_bin}; build with "
            "`cargo build -p vulcan` or pass --vulcan-bin",
            file=sys.stderr,
        )
        return 2

    with tempfile.TemporaryDirectory(prefix="paper-mcp-smoke-") as home:
        client = McpClient(args.vulcan_bin, home, args.api_url)
        smoke = Smoke(client, args)
        try:
            smoke.scenario_initialize_and_list_tools()
            smoke.scenario_paper_init()
            smoke.scenario_paper_trade_with_tpsl(args.symbol)
            smoke.scenario_paper_set_tpsl_multi_level(args.symbol)
            smoke.scenario_paper_triggers_lists_active(args.symbol)
            smoke.scenario_paper_cancel_tpsl(args.symbol)
            smoke.scenario_paper_cancel_tpsl_neither_side(args.symbol)
            smoke.scenario_paper_trade_reduce_only_rejection(args.symbol)
        finally:
            client.close()

        print(f"\n{len(smoke.passes)} passed, {len(smoke.failures)} failed")
        if smoke.failures:
            print("\nfailures:")
            for f in smoke.failures:
                print(f"  - {f}")
            return 1
    return 0


if __name__ == "__main__":
    sys.exit(main())

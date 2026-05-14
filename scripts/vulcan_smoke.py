#!/usr/bin/env python3
# /// script
# requires-python = ">=3.11"
# ///
"""End-to-end Vulcan CLI smoke test.

This script intentionally exercises real trading flows when run with
`--execute`. By default it runs in dry-run mode and only verifies read paths
plus command construction.
"""

from __future__ import annotations

import argparse
import json
import os
import shutil
import shlex
import subprocess
import sys
import tempfile
import time
from dataclasses import dataclass
from decimal import Decimal, InvalidOperation
from pathlib import Path
from typing import Any


DEFAULT_API_URL = "https://perp-api.phoenix.trade"
SMOKE_PASSWORD = "vulcan-smoke-password"


class SmokeError(RuntimeError):
    pass


@dataclass
class CommandResult:
    command: list[str]
    json: dict[str, Any]
    elapsed_s: float


def color(text: str, code: str, enabled: bool) -> str:
    if not enabled:
        return text
    return f"\033[{code}m{text}\033[0m"


class Smoke:
    def __init__(self, args: argparse.Namespace) -> None:
        self.args = args
        self.color = not args.no_color
        self.temp_home: tempfile.TemporaryDirectory[str] | None = None
        self.env = os.environ.copy()
        self.wallet_name = "vulcan-smoke"
        self.executed_market_position = False
        self.mutation_started = False
        self.timings: list[tuple[str, float, bool]] = []

    @property
    def using_current_wallet(self) -> bool:
        return not self.args.keypair

    def log(self, message: str) -> None:
        print(message, flush=True)

    def step(self, message: str) -> None:
        self.log(color(f"\n==> {message}", "1;36", self.color))

    def ok(self, message: str) -> None:
        self.log(color(f"  ok  {message}", "32", self.color))

    def warn(self, message: str) -> None:
        self.log(color(f"  warn {message}", "33", self.color))

    def fail(self, message: str) -> None:
        self.log(color(f"  fail {message}", "31", self.color))

    def setup_home_and_wallet(self) -> None:
        if not self.args.keypair:
            self.warn("using current Vulcan config and default wallet")
            if "VULCAN_WALLET_PASSWORD" not in self.env:
                self.warn(
                    "VULCAN_WALLET_PASSWORD is not set; dangerous commands may prompt "
                    "for the wallet password"
                )
            return

        keypair = Path(os.path.expanduser(self.args.keypair)).resolve()
        if not keypair.exists():
            raise SmokeError(f"keypair file does not exist: {keypair}")
        self.temp_home = tempfile.TemporaryDirectory(prefix="vulcan-smoke-home-")
        self.env["HOME"] = self.temp_home.name
        self.env["VULCAN_WALLET_PASSWORD"] = SMOKE_PASSWORD

        self.step("Import Solana keypair into isolated Vulcan wallet store")
        self.run(
            [
                "wallet",
                "import",
                "--name",
                self.wallet_name,
                "--format",
                "file",
                str(keypair),
            ]
        )
        self.run(["wallet", "set-default", self.wallet_name])
        self.ok(f"using temporary HOME={self.temp_home.name}")

    def base_cmd(self) -> list[str]:
        cmd = [self.args.vulcan_bin]
        if self.args.url:
            cmd.extend(["--rpc-url", self.args.url])
        if self.args.api_url:
            cmd.extend(["--api-url", self.args.api_url])
        cmd.extend(["-o", "json", "--yes"])
        if not self.args.execute:
            cmd.append("--dry-run")
        return cmd

    def is_mutating(self, subcmd: list[str]) -> bool:
        if not subcmd:
            return False
        if subcmd[0] in {"trade", "position", "margin", "account"}:
            if subcmd[:2] in (
                ["trade", "orders"],
                ["position", "show"],
                ["position", "list"],
                ["margin", "status"],
                ["margin", "leverage-tiers"],
                ["account", "info"],
            ):
                return False
            return True
        return subcmd[0] == "wallet" and len(subcmd) > 1 and subcmd[1] not in {
            "list",
            "show",
            "export",
            "balance",
        }

    def run(self, subcmd: list[str], *, allow_error: bool = False) -> CommandResult:
        cmd = self.base_cmd() + subcmd
        display = shlex.join(cmd)
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
        except subprocess.TimeoutExpired as exc:
            elapsed = time.perf_counter() - start
            stderr = (exc.stderr or "").strip() if isinstance(exc.stderr, str) else exc.stderr
            stdout = (exc.stdout or "").strip() if isinstance(exc.stdout, str) else exc.stdout
            hint = ""
            if self.args.execute and self.using_current_wallet:
                hint = (
                    "\nHint: when running --execute with the current Vulcan wallet, set "
                    "VULCAN_WALLET_PASSWORD so dangerous commands can unlock the wallet "
                    "non-interactively."
                )
            raise SmokeError(
                f"command timed out after {elapsed:.2f}s:\n{display}\n"
                f"stdout:\n{stdout or ''}\nstderr:\n{stderr or ''}{hint}"
            ) from exc
        elapsed = time.perf_counter() - start
        mutating = self.is_mutating(subcmd)
        self.timings.append((" ".join(subcmd), elapsed, mutating))

        stdout = proc.stdout.strip()
        stderr = proc.stderr.strip()
        try:
            payload = json.loads(stdout) if stdout else {}
        except json.JSONDecodeError as exc:
            raise SmokeError(
                f"command did not return JSON after {elapsed:.2f}s:\n{display}\n"
                f"stdout:\n{stdout}\nstderr:\n{stderr}"
            ) from exc

        if proc.returncode != 0 or payload.get("ok") is False:
            if allow_error:
                return CommandResult(cmd, payload, elapsed)
            error = payload.get("error") or {"stderr": stderr, "stdout": stdout}
            raise SmokeError(
                f"command failed after {elapsed:.2f}s:\n{display}\n"
                f"{json.dumps(error, indent=2)}"
            )

        label = "tx" if mutating and self.args.execute else "cmd"
        self.ok(f"{' '.join(subcmd)} ({elapsed:.2f}s {label})")
        return CommandResult(cmd, payload, elapsed)

    def data(self, subcmd: list[str]) -> dict[str, Any]:
        return self.run(subcmd).json.get("data", {})

    def decimal(self, value: Any, field: str) -> Decimal:
        try:
            return Decimal(str(value))
        except (InvalidOperation, ValueError) as exc:
            raise SmokeError(f"could not parse {field} as decimal: {value!r}") from exc

    def portfolio(self) -> dict[str, Any]:
        return self.data(["portfolio"])

    def positions(self, portfolio: dict[str, Any] | None = None) -> list[dict[str, Any]]:
        p = portfolio or self.portfolio()
        return p.get("positions", {}).get("positions", [])

    def orders(self, portfolio: dict[str, Any] | None = None) -> list[dict[str, Any]]:
        p = portfolio or self.portfolio()
        return p.get("orders", {}).get("orders", [])

    def position_for(self, symbol: str) -> dict[str, Any] | None:
        prefix = symbol.upper()
        for pos in self.positions():
            if str(pos.get("symbol", "")).split()[0] == prefix:
                return pos
        return None

    def orders_for(self, symbol: str) -> list[dict[str, Any]]:
        prefix = symbol.upper()
        return [o for o in self.orders() if str(o.get("symbol")) == prefix]

    def assert_clean_symbol(self) -> None:
        symbol = self.args.symbol.upper()
        p = self.portfolio()
        pos = [x for x in self.positions(p) if str(x.get("symbol", "")).split()[0] == symbol]
        orders = [x for x in self.orders(p) if str(x.get("symbol")) == symbol]
        if pos or orders:
            raise SmokeError(
                f"{symbol} is not clean before smoke test: "
                f"{len(pos)} position(s), {len(orders)} order(s). "
                "Choose another --symbol or clean it up first."
            )
        self.ok(f"{symbol} has no pre-existing positions or orders")

    def require_position(self, symbol: str) -> dict[str, Any]:
        pos = self.position_for(symbol)
        if not pos:
            raise SmokeError(f"expected open {symbol} position, found none")
        self.ok(
            f"verified {symbol} position size={pos.get('size')} "
            f"entry={pos.get('entry_price')}"
        )
        return pos

    def require_tpsl_levels(self, symbol: str) -> dict[str, Any]:
        position = self.data(["position", "show", symbol])
        tp_levels = position.get("tp_levels") or []
        sl_levels = position.get("sl_levels") or []
        if not tp_levels:
            raise SmokeError(f"expected {symbol} take-profit level in position show")
        if not sl_levels:
            raise SmokeError(f"expected {symbol} stop-loss level in position show")
        self.ok(
            f"verified {symbol} TP/SL levels "
            f"tp={tp_levels[0].get('price')} sl={sl_levels[0].get('price')}"
        )
        return position

    def require_no_tpsl_levels(self, symbol: str) -> None:
        position = self.data(["position", "show", symbol])
        tp_levels = position.get("tp_levels") or []
        sl_levels = position.get("sl_levels") or []
        if tp_levels or sl_levels:
            raise SmokeError(
                f"expected no {symbol} TP/SL levels, found "
                f"tp={len(tp_levels)} sl={len(sl_levels)}"
            )
        self.ok(f"verified no {symbol} TP/SL levels")

    def require_no_position(self, symbol: str) -> None:
        if self.position_for(symbol):
            raise SmokeError(f"expected no open {symbol} position")
        self.ok(f"verified no open {symbol} position")

    def require_orders(self, symbol: str, minimum: int = 1) -> list[dict[str, Any]]:
        orders = self.orders_for(symbol)
        if len(orders) < minimum:
            raise SmokeError(f"expected at least {minimum} {symbol} order(s), found {len(orders)}")
        self.ok(f"verified {len(orders)} {symbol} order(s)")
        return orders

    def require_no_orders(self, symbol: str) -> None:
        orders = self.orders_for(symbol)
        if orders:
            raise SmokeError(f"expected no {symbol} orders, found {len(orders)}")
        self.ok(f"verified no {symbol} orders")

    def market_context(self) -> tuple[dict[str, Any], dict[str, Any], int]:
        symbol = self.args.symbol.upper()
        info = self.data(["market", "info", symbol])
        ticker = self.data(["market", "ticker", symbol])
        self.data(["market", "orderbook", symbol, "--depth", "10"])
        decimals = int(info["base_lots_decimals"])
        self.ok(
            f"{symbol} mark={ticker.get('mark_price')} mid={ticker.get('mid_price')} "
            f"base_lots_decimals={decimals}"
        )
        return info, ticker, decimals

    def lots_to_tokens(self, lots: int, decimals: int) -> float:
        return lots / (10**decimals)

    def price(self, raw: float) -> str:
        return f"{raw:.8f}".rstrip("0").rstrip(".")

    def print_health(self) -> None:
        self.step("Fetch trader health and portfolio")
        self.data(["status"])
        self.data(["account", "info"])
        margin = self.data(["margin", "status"])
        portfolio = self.portfolio()
        self.ok(
            "health="
            f"{portfolio.get('margin', {}).get('risk_state')} "
            f"portfolio_value={portfolio.get('margin', {}).get('portfolio_value')} "
            f"available={portfolio.get('margin', {}).get('available_to_withdraw')}"
        )
        if margin:
            self.ok("margin status returned")

    def margin_roundtrip(self) -> None:
        amount = self.decimal(self.args.margin_roundtrip_amount, "margin roundtrip amount")
        if amount <= 0:
            self.warn("skipping margin round trip because amount is <= 0")
            return

        self.step(f"Margin withdraw/deposit round trip ({amount} USDC)")
        margin = self.data(["margin", "status"])
        available = self.decimal(
            margin.get("available_to_withdraw", "0"),
            "available_to_withdraw",
        )
        if available < amount:
            self.warn(
                f"skipping withdraw: available_to_withdraw={available} "
                f"is less than {amount}"
            )
            return

        if not self.args.execute:
            self.warn(
                f"dry-run mode: would withdraw {amount} USDC, verify wallet USDC, "
                "then deposit it back"
            )
            return

        amount_s = str(amount)
        self.mutation_started = True
        self.run(["margin", "withdraw", amount_s])

        balance = self.data(["wallet", "balance"])
        wallet_usdc = self.decimal(balance.get("usdc", "0"), "wallet usdc")
        if wallet_usdc < amount:
            raise SmokeError(
                f"withdraw submitted but wallet USDC={wallet_usdc} is less than "
                f"expected deposit amount {amount}"
            )
        self.ok(f"verified wallet USDC={wallet_usdc} after withdraw")

        self.run(["margin", "deposit", amount_s])
        final_margin = self.data(["margin", "status"])
        self.ok(
            "completed margin round trip; "
            f"available_to_withdraw={final_margin.get('available_to_withdraw')}"
        )

    def cancel_all_regular_orders(self) -> None:
        symbol = self.args.symbol.upper()
        result = self.run(["trade", "cancel-all", symbol], allow_error=True)
        if result.json.get("ok") is False:
            error = result.json.get("error", {})
            code = error.get("code")
            if code in {"NO_OPEN_ORDERS", "ORDER_NOT_FOUND"}:
                self.ok(f"no regular {symbol} orders to cancel")
                return
            raise SmokeError(f"cancel-all failed: {json.dumps(error, indent=2)}")
        self.ok(f"cancel-all {symbol} submitted")

    def cancel_tpsl(self) -> None:
        symbol = self.args.symbol.upper()
        result = self.run(["trade", "cancel-tpsl", symbol, "--tp", "--sl"], allow_error=True)
        if result.json.get("ok") is False:
            error = result.json.get("error", {})
            code = error.get("code")
            if code in {
                "NO_CONDITIONAL_ORDERS",
                "NO_OPEN_ORDERS",
                "POSITION_NOT_FOUND",
                "NO_POSITION",
            }:
                self.ok(f"no {symbol} TP/SL to cancel")
                return
            raise SmokeError(f"cancel-tpsl failed: {json.dumps(error, indent=2)}")
        self.ok(f"cancel-tpsl {symbol} submitted")

    def close_position_if_any(self) -> None:
        symbol = self.args.symbol.upper()
        if not self.position_for(symbol):
            self.ok(f"no {symbol} position to close")
            return
        self.run(["position", "close", symbol])
        self.require_no_position(symbol)

    def cleanup(self) -> None:
        if not self.args.execute or not self.mutation_started:
            return
        symbol = self.args.symbol.upper()
        self.step(f"Cleanup {symbol}")
        try:
            self.cancel_all_regular_orders()
        except Exception as exc:  # noqa: BLE001 - cleanup should keep going
            self.warn(f"regular order cleanup failed: {exc}")
        try:
            self.cancel_tpsl()
        except Exception as exc:  # noqa: BLE001
            self.warn(f"TP/SL cleanup failed: {exc}")
        try:
            self.close_position_if_any()
        except Exception as exc:  # noqa: BLE001
            self.warn(f"position cleanup failed: {exc}")

    def dry_run_notice(self) -> None:
        if self.args.execute:
            self.warn("EXECUTE mode: this will submit real Solana mainnet transactions.")
        else:
            self.warn(
                "dry-run mode: dangerous commands are simulated; state-changing "
                "postconditions are skipped. Pass --execute for full regression smoke."
            )

    def run_smoke(self) -> None:
        self.dry_run_notice()
        if not shutil.which(self.args.vulcan_bin):
            raise SmokeError(f"vulcan binary not found: {self.args.vulcan_bin}")
        if self.args.execute and self.using_current_wallet and "VULCAN_WALLET_PASSWORD" not in self.env:
            raise SmokeError(
                "VULCAN_WALLET_PASSWORD must be set when running --execute with the "
                "current Vulcan config/default wallet. Alternatively pass -k <keypair> "
                "to import an isolated smoke-test wallet for the run."
            )

        self.setup_home_and_wallet()
        self.print_health()
        self.margin_roundtrip()
        self.assert_clean_symbol()

        symbol = self.args.symbol.upper()
        _, ticker, decimals = self.market_context()
        lots = int(self.args.size_lots)
        tokens = self.lots_to_tokens(lots, decimals)
        mark = float(ticker.get("mark_price") or ticker.get("mid_price"))
        mid = float(ticker.get("mid_price") or mark)
        limit_price = self.price(mid * (1 - self.args.limit_distance_bps / 10_000))
        limit_tp = self.price(mid * 1.02)
        limit_sl = self.price(mid * 0.98)
        pos_tp = self.price(mark * 1.02)
        pos_sl = self.price(mark * 0.98)

        self.step(f"Open and cancel regular {symbol} limit order")
        if self.args.execute:
            self.mutation_started = True
        self.run(["trade", "limit-buy", symbol, str(lots), limit_price])
        if self.args.execute:
            self.require_orders(symbol, 1)
            self.cancel_all_regular_orders()
            self.require_no_orders(symbol)

        self.step(f"Open and cancel {symbol} limit order with conditional trigger args")
        trigger_limit = self.price(mid * (1 - (self.args.limit_distance_bps + 50) / 10_000))
        self.run(
            [
                "trade",
                "limit-buy",
                symbol,
                str(lots),
                trigger_limit,
                "--tp",
                limit_tp,
                "--sl",
                limit_sl,
            ]
        )
        if self.args.execute:
            self.require_orders(symbol, 1)
            self.cancel_all_regular_orders()
            self.cancel_tpsl()
            self.require_no_orders(symbol)

        self.step(f"Open small {symbol} market position")
        self.run(["trade", "market-buy", symbol, str(lots)])
        if self.args.execute:
            self.executed_market_position = True
            self.require_position(symbol)

        self.step(f"Attach and cancel {symbol} TP/SL")
        if self.args.execute:
            self.run(["trade", "set-tpsl", symbol, "--tp", pos_tp, "--sl", pos_sl])
            self.require_tpsl_levels(symbol)
            self.cancel_tpsl()
            self.require_no_tpsl_levels(symbol)
        else:
            self.warn("skipping TP/SL attach in dry-run mode because no position is created")

        self.step(f"Close {symbol} position")
        if self.args.execute:
            self.run(["position", "close", symbol])
            self.executed_market_position = False
            self.require_no_position(symbol)
        else:
            self.warn("skipping close in dry-run mode because no position is created")

        self.step("Final health and portfolio")
        self.print_health()
        self.ok(
            f"smoke passed for {symbol}: lots={lots}, tokens={tokens:g}, "
            f"rpc={self.args.url or 'current-config'}"
        )
        self.print_timing_summary()

    def print_timing_summary(self) -> None:
        if not self.timings:
            return
        reads = [elapsed for _, elapsed, mutating in self.timings if not mutating]
        writes = [elapsed for _, elapsed, mutating in self.timings if mutating]
        slowest = sorted(self.timings, key=lambda item: item[1], reverse=True)[:5]
        self.step("Timing summary")
        if reads:
            self.ok(f"read commands avg={sum(reads) / len(reads):.2f}s max={max(reads):.2f}s")
        if writes:
            label = "submitted tx commands" if self.args.execute else "dry-run mutating commands"
            self.ok(f"{label} avg={sum(writes) / len(writes):.2f}s max={max(writes):.2f}s")
        for name, elapsed, mutating in slowest:
            kind = "tx" if mutating and self.args.execute else "cmd"
            self.log(f"  {elapsed:6.2f}s {kind}  {name}")

    def close(self) -> None:
        if self.temp_home is not None:
            self.temp_home.cleanup()


def parse_args(argv: list[str]) -> argparse.Namespace:
    parser = argparse.ArgumentParser(
        description=(
            "Run a Vulcan CLI smoke test against a wallet/keypair. "
            "Defaults to dry-run; pass --execute to submit real transactions."
        )
    )
    parser.add_argument("-u", "--url", help="Solana RPC URL override")
    parser.add_argument(
        "-k",
        "--keypair",
        help=(
            "Solana CLI keypair JSON path. When omitted, use the current Vulcan "
            "config and default wallet."
        ),
    )
    parser.add_argument("--api-url", help=f"Phoenix API URL override (default from config)")
    parser.add_argument("--symbol", default="SOL", help="Market symbol to smoke test")
    parser.add_argument(
        "--size-lots",
        type=int,
        default=1,
        help="Small test size in base lots (default: 1)",
    )
    parser.add_argument(
        "--limit-distance-bps",
        type=int,
        default=500,
        help="Distance below mid for non-crossing limit-buy smoke orders (default: 500)",
    )
    parser.add_argument(
        "--execute",
        action="store_true",
        help="Submit real transactions and verify state changes",
    )
    parser.add_argument("--vulcan-bin", default="vulcan", help="vulcan binary path/name")
    parser.add_argument("--timeout", type=int, default=60, help="Per-command timeout seconds")
    parser.add_argument(
        "--margin-roundtrip-amount",
        default="1.0",
        help="USDC amount for execute-mode withdraw/deposit smoke test (default: 1.0)",
    )
    parser.add_argument("--no-color", action="store_true", help="Disable ANSI colors")
    return parser.parse_args(argv)


def main(argv: list[str]) -> int:
    args = parse_args(argv)
    smoke = Smoke(args)
    try:
        smoke.run_smoke()
        return 0
    except KeyboardInterrupt:
        smoke.warn("interrupted")
        smoke.cleanup()
        return 130
    except Exception as exc:  # noqa: BLE001 - top-level reporter
        smoke.fail(str(exc))
        smoke.cleanup()
        return 1
    finally:
        smoke.close()


if __name__ == "__main__":
    raise SystemExit(main(sys.argv[1:]))

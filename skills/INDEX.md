# Skills Index

17 agent skills for `vulcan`, organized by category.

## Context Loading Model

Use the smallest context set that safely matches the task:

1. Always load the runtime contract first: `vulcan://context` or `CONTEXT.md`.
2. Load `vulcan` as the entry-point — it contains the non-negotiable safety rules, the focused-skill router, and the live-launch preflight gate.
3. For tasks that open, close, or change exposure, also load `vulcan-execution-modes`, `vulcan-risk-management`, and `vulcan-error-recovery` before executing dangerous tools.
4. Load focused task skills only when the user intent needs them.

## Entry Point

Single Vulcan entry skill. Contains the runtime contract pointer, the eight Non-Negotiable Rules, the focused-skill router, and the live-launch preflight gate.


| Skill                       | Description                                         |
| --------------------------- | --------------------------------------------------- |
| [vulcan](./vulcan/SKILL.md) | Entry-point: safety rules, routing, preflight gate. Load before any answer about Vulcan / Phoenix DEX / Solana perps. |


## Core

Execution modes, risk management, and error recovery.


| Skill                                                       | Description                                                             |
| ----------------------------------------------------------- | ----------------------------------------------------------------------- |
| [vulcan-quickstart](./vulcan-quickstart/SKILL.md)           | Five-minute install + first paper trade. Hands off to onboarding for wallet creation. |
| [vulcan-execution-modes](./vulcan-execution-modes/SKILL.md) | **Canonical taxonomy**: Observe / Paper / Dry-Run / Confirm-Each / Auto-Execute. Lists what each mode does, when to ask the user which mode, how to format the question, and what follow-up to collect per mode. Strategy skills (TWAP, grid, TA) all defer to this. |
| [vulcan-risk-management](./vulcan-risk-management/SKILL.md) | Pre-trade risk checks, leverage tiers, margin health, and when to warn. |
| [vulcan-error-recovery](./vulcan-error-recovery/SKILL.md)   | Error category routing, tx_failed recovery, and network error handling. |


## Trading

Order execution, lot size calculation, TP/SL management, and execution strategies.


| Skill                                                               | Description                                                                |
| ------------------------------------------------------------------- | -------------------------------------------------------------------------- |
| [vulcan-trade-execution](./vulcan-trade-execution/SKILL.md)         | Safe order execution with pre-trade checks and post-trade verification.    |
| [vulcan-lot-size-calculator](./vulcan-lot-size-calculator/SKILL.md) | Convert desired token amounts to base lots with worked examples.           |
| [vulcan-tpsl-management](./vulcan-tpsl-management/SKILL.md)         | Take-profit and stop-loss: direction rules, constraints, set/cancel flows. |
| [vulcan-twap-execution](./vulcan-twap-execution/SKILL.md)           | Execute large orders as time-weighted slices to reduce market impact.      |
| [vulcan-grid-trading](./vulcan-grid-trading/SKILL.md)               | Grid trading with layered limit orders across a price range.               |
| [vulcan-ta-strategy](./vulcan-ta-strategy/SKILL.md)                 | TA-driven strategy runner: declarative rules (Condition → Action) for EMA cross, RSI mean-reversion, MACD trend-follow, multi-confirmation entries. |


## Market Data

Price reads, orderbook analysis, derived indicators, and pre-trade research.


| Skill                                                                 | Description                                                                                  |
| --------------------------------------------------------------------- | -------------------------------------------------------------------------------------------- |
| [vulcan-market-intel](./vulcan-market-intel/SKILL.md)                 | Ticker, orderbook, candles, market info, and pre-trade analysis patterns.                    |
| [vulcan-technical-analysis](./vulcan-technical-analysis/SKILL.md)     | Technical indicators (RSI, MACD, BBands, ATR, ADX, …) and trigger eval over candle history.  |


## Portfolio & Account

Margin operations, portfolio monitoring, and onboarding.


| Skill                                                           | Description                                                              |
| --------------------------------------------------------------- | ------------------------------------------------------------------------ |
| [vulcan-portfolio-intel](./vulcan-portfolio-intel/SKILL.md)     | Portfolio snapshot: cross + isolated margin totals, positions with uPnL %, resting orders, funding exposure, and optional daily/weekly performance. |
| [vulcan-margin-operations](./vulcan-margin-operations/SKILL.md) | Deposit, withdraw, transfer, isolated margin, and collateral management. |
| [vulcan-onboarding](./vulcan-onboarding/SKILL.md)               | First interaction and new user setup: health, paper-first path, wallet, registration, first deposit. |


## Position

Position monitoring and management.


| Skill                                                               | Description                                                    |
| ------------------------------------------------------------------- | -------------------------------------------------------------- |
| [vulcan-position-management](./vulcan-position-management/SKILL.md) | List, show, close, reduce positions and attach TP/SL post-hoc. |

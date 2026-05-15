# Changelog

All notable changes to the Vertexa project will be documented in this file.

The format is based on [Keep a Changelog](https://keepachangelog.com/en/1.0.0/),
and this project adheres to [Semantic Versioning](https://semver.org/spec/v2.0.0.html).

## [0.1.0] - 2026-05-15

### Added
- **Core Trading Engine**: Initial implementation of the autonomous DEX trading bot for Arbitrum.
- **Signals System**: Technical analysis signals including RSI, EMA Crossover, Order Book Imbalance, and On-chain Flow (CVD).
- **Consensus Engine**: Weighted voting mechanism with adaptive pre-gate regime filtering.
- **Volatility Regime**: ATR-based adaptive regime classification (Ranging, Trending, Volatile).
- **MEV Guard**: Mempool monitoring with Uniswap V3 and 1inch calldata decoding, Flashbots integration, and split execution.
- **Execution System**: Actor-based executor with aggressive speculative gas escalation and nonce management.
- **Profitability Gate**: Institutional-grade profitability check using predictive gas estimates (EIP-1559) and MEV loss assessment.
- **Risk Management**: Position limits, daily loss limits, and circuit breakers.
- **Ingestion**: Real-time pool event subscription (Swap, Mint, Burn) for tick-by-tick liquidity reconstruction.
- **Notifications**: Discord webhook integration for trade alerts and system status.
- **Persistence**: Graceful shutdown support with state persistence to `vertexa_state.json`.
- **Backtesting**: `backtester` and `backtest_historical` binaries for strategy validation against CSV event logs.
- **Documentation**: Comprehensive `AGENTS.md`, `ARCHITECTURE.md`, `BUILD.md`, and `ENHANCEMENTS.md`.

### Fixed
- Resolved workspace-wide compilation errors and type mismatches in core crates.
- Fixed gas estimation accuracy by implementing forward-looking `eth_feeHistory` modeling.
- Improved staleness protection with RPC/WS divergence checks and automatic re-sync.

### Changed
- Refactored `FixedUsd` wrapper to ensure precision in financial arithmetic across the workspace.
- Enhanced [AGENTS.md](AGENTS.md) with institutional-grade architectural details and agent-specific instructions.


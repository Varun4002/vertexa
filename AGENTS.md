# Vertexa — AI Agent Instructions

## Project Overview

Vertexa is an autonomous DEX trading bot for Arbitrum. It ingests on-chain market data, computes technical signals (RSI, EMA crossover, orderbook imbalance, onchain flow), reaches consensus via weighted voting, assesses MEV threats, checks risk limits, and executes swaps through the public mempool, Flashbots, or split execution.

## Workspace Structure

```text
crates/core/       Shared types, traits, errors, constants
crates/ingestion/  Data ingestion — price feeds, pool reader, orderbook, context builder
crates/signals/    Trading signals — RSI, EMA, orderbook, onchain flow
crates/consensus/  Consensus engine — aggregates signal votes into decisions
crates/mev_guard/  MEV protection — mempool monitor, Flashbots relay, split executor
crates/risk/       Risk checking — position limits, daily loss limits
crates/executor/   Execution — swap builder, tx signer, broadcaster
bin/               Binary entrypoint (vertexa.rs)
```

## Key Conventions

- **Async runtime:** Tokio
- **Error handling:** `eyre` for application-level, `thiserror` for library errors (`VertexaError`)
- **Configuration:** `config/default.toml` + env var overrides (`VERTEXA_RPC_WS`, `VERTEXA_PAPER`)
- **Logging:** `tracing` with json output and env-filter
- **Web3:** Alloy framework
- **Target:** Arbitrum (chain ID 42161)

## Build & Run

```bash
cargo build --release          # production build
cargo run --release            # run (reads config/default.toml)
VERTEXA_PAPER=true cargo run   # paper trade mode (no real txns)
```

## Agent Rules

- Read `docs/ARCHITECTURE.md` first for full system understanding
- Read `docs/BUILD.md` for setup and config details
- Preserve existing code patterns (async traits, alloy types, tracing)
- Do not add dependencies unless justified
- Match existing naming conventions (camelCase for types, snake_case for functions)

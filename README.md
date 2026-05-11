# Vertexa

Autonomous DEX trading bot for Arbitrum.

## Overview

Vertexa ingests on-chain market data, computes technical signals (RSI, EMA crossover, orderbook imbalance, onchain flow), reaches consensus via weighted voting, assesses MEV threats, checks risk limits, and executes swaps through the public mempool, Flashbots, or split execution.

## Architecture

```
crates/core/       Shared types, traits, errors, constants
crates/ingestion/  Data ingestion — price feeds, pool reader, orderbook, context builder
crates/signals/    Trading signals — RSI, EMA, orderbook, onchain flow
crates/consensus/  Consensus engine — aggregates signal votes into decisions
crates/mev_guard/  MEV protection — mempool monitor, Flashbots relay, split executor
crates/risk/       Risk checking — position limits, daily loss limits
crates/executor/   Execution — swap builder, tx signer, broadcaster
bin/               Binary entrypoint
```

## Quick Start

```bash
# Build
cargo build --release

# Paper trade (no real transactions)
VERTEXA_PAPER=true cargo run --release

# Production
cargo run --release
```

## Documentation

| File | Description |
|---|---|
| `docs/ARCHITECTURE.md` | System architecture and crate design |
| `docs/BUILD.md` | Build, setup, config, and deployment guide |
| `AGENTS.md` | AI agent context and conventions |
# vertexa

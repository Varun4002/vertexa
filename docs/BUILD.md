# Vertexa Build Guide

## Prerequisites

- **Rust** (edition 2021) — install via [rustup](https://rustup.rs/)
- Access to Arbitrum RPC endpoints (HTTP + WebSocket)
- A funded wallet private key for real trading

## Build

```bash
# Debug build
cargo build

# Release build (optimized)
cargo build --release

# Check compilation without building
cargo check
```

The workspace compiles 8 crates + 2 binaries. First build will download dependencies.

## Configuration

Copy and edit the default config:

```bash
cp config/default.toml config/local.toml
# or use default.toml directly
```

See `config/default.toml` for all settings:

- **`[network]`** — RPC endpoints, chain ID
- **`[trading]`** — Trading pair, pool address, fee tier, max trade size
- **`[risk]`** — Slippage tolerances, daily loss limit, min chunk size
- **`[mev]`** — Flashbots relay URL, known bot addresses, mempool buffer
- **`[consensus]`** — Required signal votes, minimum confidence
- **`[notify]`** — Discord webhook URL for trade notifications (optional)

## Environment Variables

| Variable | Required | Description |
|---|---|---|
| `VERTEXA_RPC_WS` | No | Override WebSocket RPC URL |
| `VERTEXA_PAPER` | No | Set to `true` to enable paper trading |
| `VERTEXA_PRIVATE_KEY` | Yes* | Ethereum private key for signing transactions |

\* Required unless running in paper trade mode.

## Discord Notifications

Add this to your config to receive trade notifications on Discord:

```toml
[notify]
discord_webhook_url = "https://discord.com/api/webhooks/YOUR_WEBHOOK_ID/YOUR_WEBHOOK_TOKEN"
```

The notifier is optional — if the `[notify]` section is omitted, no webhook calls are made. Notifications fire asynchronously and do not block the main loop.

Bot sends embeds for:
- **Successful trades** (green) — action, amount, route, risk score, tx hash
- **Failed trades** (orange) — action, amount, reason (MEV abort, profitability gate, risk gate, simulation failure)
- **Execution errors** (orange) — error details

## State Persistence

On graceful shutdown (SIGINT/SIGTERM), the bot saves:
- Daily loss counter
- Circuit breaker status
- Position tracking

to `vertexa_state.json` in the working directory. State is automatically restored on next startup.

## Event Logging

Every loop iteration is logged to a CSV file in `data/`:

```bash
ls data/
# events_2026-05-13.csv
```

One file per day, auto-rotated at UTC midnight. Fields: timestamp, block, price, regime, action, confidence, size multiplier, execution result, route, tx hash, error, gas cost, MEV cost, edge, reward-to-cost ratio.

Use the backtester binary to replay these logs for strategy validation:

```bash
cargo run --bin backtester -- data/events_2026-05-13.csv
```

## Running

```bash
# Paper trade mode (safe for testing)
VERTEXA_PAPER=true cargo run

# Production mode (ensure PRIVATE_KEY is set)
cargo run --release

# With custom RPC
VERTEXA_RPC_WS=wss://my-custom-arbitrum-node.com/ws cargo run --release

# Backtester
cargo run --bin backtester -- data/events_2026-05-13.csv

# Stop gracefully: Ctrl+C (state will be saved)
```

## Validation

```bash
cargo check --workspace
cargo clippy --workspace -- -D warnings
cargo test --workspace
```

## Project Layout

```text
Vertexa/
├── Cargo.toml              # Workspace manifest
├── config/
│   └── default.toml        # Default configuration
├── data/                   # Event CSV logs (auto-generated)
│   └── events_*.csv
├── crates/
│   ├── core/               # Shared types, traits, errors
│   ├── ingestion/          # Data ingestion
│   │   ├── pool_events.rs  # Swap event subscription (real-time price)
│   │   ├── gas_estimator.rs# Real gas estimation
│   │   ├── event_logger.rs # CSV event logging for backtesting
│   │   ├── price_feed.rs   # Binance WS price feed
│   │   ├── pool_reader.rs  # On-chain pool state (fallback poll)
│   │   ├── orderbook.rs    # Simulated orderbook
│   │   └── context_builder.rs # MarketContext assembly
│   ├── signals/            # Trading signals
│   ├── consensus/          # Decision engine
│   ├── mev_guard/          # MEV protection (swap calldata decoder)
│   ├── risk/               # Risk management
│   ├── executor/           # Trade execution
│   │   ├── swap_builder.rs # Uniswap V3 swap tx builder
│   │   ├── simulator.rs    # Pre-trade eth_call simulation
│   │   ├── actor.rs        # mpsc-based executor with nonce/retry
│   │   └── signer.rs       # Tx signing
│   └── notify/             # Discord webhook notifications
├── bin/
│   ├── Cargo.toml
│   ├── vertexa.rs          # Main binary entrypoint
│   └── backtester.rs       # CSV replay / backtesting binary
├── docs/                   # Documentation
├── AGENTS.md               # AI agent instructions
└── README.md
```

## Dependencies

Key external dependencies:
- **alloy** — Ethereum/Arbitrum RPC, providers, signers, types
- **tokio** — Async runtime, mpsc channels, oneshot
- **tracing** — Structured logging
- **config** — File-based configuration
- **eyre** / **thiserror** — Error handling
- **serde** / **serde_json** — Serialization
- **reqwest** — HTTP client
- **tokio-tungstenite** — WebSocket client
- **chrono** — Time utilities (file rotation, midnight reset)

All dependencies are already specified in the workspace `Cargo.toml` — no new crates were needed for Phase 1 improvements.

## Architecture Overview

### Real-Time Price (Swap Events)
The primary price source is a WebSocket subscription to the Uniswap V3 pool's `Swap` event. Each event contains `sqrtPriceX96` which is converted to a USD price. This replaces 12-second polling with sub-block updates and ~95% fewer RPC calls. A poll-based fallback runs in case events are missed.

### Executor Actor
Trade execution is handled by a dedicated mpsc-based actor that owns the signer and nonce. This eliminates nonce collisions and enables reliable gas bump retry logic:
- Send tx → wait 5s → if pending, bump gas +20% → re-sign with same nonce → retry (3x max)
- Pre-trade simulation via `eth_call` verifies the swap output before signing

### Mempool Whale Decoder
Pending swap transactions are decoded by parsing `exactInputSingle` calldata from the Uniswap V3 router. This determines whether a whale is buying or selling ETH, replacing the old heuristic that guessed direction from the `to` address.

### Backtesting
Every loop iteration is logged to CSV. The `backtester` binary replays these logs to compute PnL, win rate, Sharpe ratio, and max drawdown. Use it to validate strategy changes against historical data before going live.

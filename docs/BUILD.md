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

The workspace compiles 8 crates + 1 binary. First build will download dependencies.

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
- **Failed trades** (orange) — action, amount, reason (MEV abort, profitability gate, risk gate)
- **Execution errors** (orange) — error details

## State Persistence

On graceful shutdown (SIGINT/SIGTERM), the bot saves:
- Daily loss counter
- Circuit breaker status
- Position tracking

to `vertexa_state.json` in the working directory. State is automatically restored on next startup.

## Running

```bash
# Paper trade mode (safe for testing)
VERTEXA_PAPER=true cargo run

# Production mode (ensure PRIVATE_KEY is set)
cargo run --release

# With custom RPC
VERTEXA_RPC_WS=wss://my-custom-arbitrum-node.com/ws cargo run --release

# Stop gracefully: Ctrl+C (state will be saved)
```

## Validation

```bash
cargo check --workspace
cargo clippy --workspace -- -D warnings
cargo test --workspace   # 36 tests
```

## Project Layout

```text
Vertexa/
├── Cargo.toml              # Workspace manifest
├── config/
│   └── default.toml        # Default configuration
├── crates/
│   ├── core/               # Shared types, traits, errors
│   ├── ingestion/          # Data ingestion
│   ├── signals/            # Trading signals
│   ├── consensus/          # Decision engine
│   ├── mev_guard/          # MEV protection
│   ├── risk/               # Risk management
│   ├── executor/           # Trade execution
│   └── notify/             # Discord webhook notifications
├── bin/
│   ├── Cargo.toml
│   └── vertexa.rs          # Binary entrypoint
├── docs/                   # Documentation
├── AGENTS.md               # AI agent instructions
└── README.md
```

## Dependencies

Key external dependencies:
- **alloy** — Ethereum/Arbitrum RPC, providers, signers, types
- **tokio** — Async runtime
- **tracing** — Structured logging
- **config** — File-based configuration
- **eyre** / **thiserror** — Error handling
- **serde** / **serde_json** — Serialization
- **reqwest** — HTTP client
- **tokio-tungstenite** — WebSocket client
- **chrono** — Time utilities

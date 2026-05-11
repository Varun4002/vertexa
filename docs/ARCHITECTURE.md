# Vertexa Architecture

## Overview

Vertexa is an autonomous DEX trading bot that operates on Arbitrum. It implements a pipeline architecture where each stage is a separate crate in the workspace.

### Pipeline

```text
┌─────────────┐    ┌──────────┐    ┌───────────┐    ┌──────────┐    ┌──────────┐    ┌───────────┐
│  Ingestion  │───▶│  Signals  │───▶│ Consensus │───▶│ MEV Guard│───▶│   Risk   │───▶│ Executor  │
│             │    │          │    │           │    │          │    │  Check   │    │           │
│ price_feed  │    │ RSI      │    │  Engine   │    │ Monitor  │    │          │    │ SwapBuilder│
│ pool_reader │    │ EMA      │    │  (votes)  │    │ Flashbots│    │ Limits   │    │ TxSigner  │
│ orderbook   │    │ OB imb.  │    │           │    │ SplitExec│    │          │    │ Broadcast │
│ whale_tx    │    │ onchain   │    │           │    │ Guard    │    │          │    │           │
└──────┬──────┘    └──────────┘    └───────────┘    └──────────┘    └──────────┘    └───────────┘
       │                                                                                      
       └────────────── MarketContext ──────────────────────────────────────────────────────────▶
```

## Crate Design

### `core` — Shared Foundation

Defines all shared types and traits used across crates:

| Type/Trait | Purpose |
|---|---|
| `MarketContext` | Aggregate market snapshot fed to signals |
| `Vote` | `Buy` / `Sell` / `Neutral` |
| `Signal` trait | Async trait: `evaluate(&self, ctx: &MarketContext) -> SignalResult` |
| `SignalResult` | Signal vote + confidence (0–1) |
| `PriceSeries` | Ring buffer of close prices + volumes |
| `PlannedTrade` | Trade parameters after decision |
| `ExecutionRoute` | `PublicMempool` / `FlashbotsBundle` / `SplitExecution` / `Abort` |
| `MevThreatAssessment` | Risk score, sandwich probability, recommended route |
| `Decision` | Consensus output — action, agreeing/dissenting signals, avg confidence |
| `VertexaError` | Enum error type with `thiserror` |

Key constants: `WETH_ADDRESS`, `USDC_ADDRESS`, `UNISWAP_V3_ROUTER`, `UNISWAP_V3_QUOTER`, `ARBITRUM_CHAIN_ID`.

### `ingestion` — Data Ingestion

Collects raw market data:

- **`price_feed`** — Fetches historical price data, pushes into `PriceSeries`
- **`pool_reader`** — Reads on-chain pool state (reserves, current price, liquidity)
- **`orderbook`** — Fetches CEX/DEX orderbook data (bids/asks)
- **`context_builder`** — Assembles `MarketContext` from all data sources

### `signals` — Trading Signals

Each signal implements the `Signal` trait and produces a `SignalResult`:

- **`RsiSignal`** — Relative Strength Index overbought/oversold
- **`EmaCrossoverSignal`** — EMA fast/slow crossover detection
- **`OrderBookSignal`** — Orderbook imbalance signal (bid/ask pressure)
- **`OnchainFlowSignal`** — Whale transaction and large flow detection

### `consensus` — Decision Engine

- **`ConsensusEngine`** — Collects votes from all signals, requires minimum votes and confidence threshold to produce a `Decision`
- Configurable via `required_votes` and `min_confidence`

### `mev_guard` — MEV Protection

Protects against sandwich attacks, frontrunning, and other MEV threats:

- **`MempoolMonitor`** — Watches pending transactions for known bot addresses and suspicious patterns
- **`FlashbotsRelay`** — Submits bundles via Flashbots to avoid public mempool
- **`SplitExecutor`** — Splits large trades into smaller chunks across blocks
- **`MevGuard`** — Orchestrator: assesses threat level and recommends `ExecutionRoute`

### `risk` — Risk Management

- **`RiskChecker`** — Validates trades against configured limits (max trade size, max position, daily loss limit)
- Resets daily loss counter at midnight

### `executor` — Trade Execution

- **`SwapBuilder`** — Builds Uniswap V3 swap transactions via the quoter and router
- **`TxSigner`** — Signs transactions using a private key from environment
- **`Broadcaster`** — Sends transactions via selected route (public, Flashbots, or split)

## `bin` — Entrypoint

- Reads `config/default.toml`
- Initializes all components
- Runs the main loop:
  1. Build `MarketContext`
  2. Evaluate signals → consensus decision
  3. If decision is directional → plan trade
  4. Assess MEV threat → select execution route
  5. Check risk limits
  6. Build + sign + broadcast transaction
  7. Record trade

## Configuration

See `config/default.toml` for full reference. Sections: `[network]`, `[trading]`, `[risk]`, `[mev]`, `[consensus]`.

Environment variable overrides:
- `VERTEXA_RPC_WS` — Override WebSocket RPC URL
- `VERTEXA_PAPER=true` — Enable paper trade mode (no real transactions)

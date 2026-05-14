# Vertexa Architecture

## Overview

Vertexa is an autonomous DEX trading bot that operates on Arbitrum, targeting the Uniswap V3 WETH/USDC (0.05%) pool. It implements a pipeline architecture where each stage is a separate crate, with an mpsc-based executor actor for nonce-managed execution and real-time data ingestion via WebSocket event subscriptions.

### Pipeline

```text
┌─────────────┐    ┌──────────┐    ┌───────────┐    ┌──────────┐    ┌──────────┐    ┌───────────┐    ┌───────────┐
│  Ingestion  │───▶│  Signals  │───▶│ Consensus │───▶│ MEV Guard│───▶│   Risk   │───▶│  Notify   │───▶│ Executor  │
│             │    │          │    │           │    │          │    │  Check   │    │           │    │  Actor    │
│ pool_events │    │ RSI      │    │  Engine   │    │ Monitor  │    │          │    │ Discord   │    │  (mpsc)   │
│ gas_estim.  │    │ EMA      │    │  (votes)  │    │ Flashbots│    │ Limits   │    │ Webhook   │    │  nonce    │
│ pool_reader │    │ OB imb.  │    │           │    │ SplitExec│    │          │    │           │    │  gas bump │
│ price_feed  │    │ onchain   │    │           │    │ Guard    │    │          │    │           │    │  simulate │
│ event_log   │    │          │    │           │    │          │    │          │    │           │    │           │
└──────┬──────┘    └──────────┘    └───────────┘    └──────────┘    └──────────┘    └───────────┘    └───────────┘
       │
       └────────────── MarketContext ─────────────────────────────────────────────────────────────────────────────▶
```

## Data Flow

```
┌──────────────────┐     Swap events      ┌────────────────┐      ┌──────────────┐
│  Pool Events     │─────────────────────▶│  SharedState    │      │  Mempool     │
│  (eth_subscribe) │     sqrtPriceX96,    │  (Arc<RwLock>)  │      │  Monitor     │
│  spawned task    │     liquidity        │                 │      │  (spawned)   │
└──────────────────┘                      │ price_series    │      └──────┬───────┘
                                          │ pool_price      │             │
┌──────────────────┐                      │ liquidity       │    Decoded calldata
│  Pool Reader     │─────────────────────▶│ block_number    │      (swap direction)
│  (slot0 poll)    │     fallback poll    │ pending_txs     │◄────────────┘
│  12s interval    │                      └────────────────┘
└──────────────────┘                             │
                                                 │ read
                                                 ▼
                                          ┌──────────────┐
                                          │ ContextBuilder│
                                          │ .build()      │
                                          └──────┬───────┘
                                                 │ MarketContext
                                                 ▼
                                          ┌──────────────┐
                                          │   Consensus   │
                                          │   Engine      │
                                          │   evaluate()  │
                                          └──────┬───────┘
                                                 │ Decision
                                                 ▼
                                          ┌──────────────┐
                                          │  Main Loop    │
                                          │  checks:      │
                                          │  neutral?     │
                                          │  size_mult    │
                                          │  MEV assess   │
                                          │  gas estimate │
                                          │  profit gate  │
                                          │  risk check   │
                                          └──────┬───────┘
                                                 │ TradeIntent (mpsc)
                                                 ▼
                                          ┌──────────────────┐
                                          │  Executor Actor   │
                                          │  (spawned task)   │
                                          │                   │
                                          │ 1. eth_call sim   │
                                          │ 2. sign with nonce│
                                          │ 3. broadcast      │
                                          │ 4. select! loop   │
                                          │    ├ receipt      │
                                          │    ├ timeout→bump │
                                          │    └ revert       │
                                          └──────┬───────────┘
                                                 │ ExecutionResult (oneshot)
                                                 ▼
                                          ┌──────────────┐
                                          │  CSV Logger   │
                                          │  + Notify     │
                                          │  (Discord)    │
                                          └──────────────┘
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
| `TradeIntent` | Sent to ExecutorActor via mpsc (tokens, amount, min output, etc.) |
| `ExecutionResult` | Returned from ExecutorActor via oneshot |
| `ExecutionRoute` | `PublicMempool` / `FlashbotsBundle` / `SplitExecution` / `Abort` |
| `MevThreatAssessment` | Risk score, sandwich probability, recommended route |
| `Decision` | Consensus output — action, agreeing/dissenting signals, avg confidence |
| `VertexaError` | Enum error type with `thiserror` |

Key constants: `WETH_ADDRESS`, `USDC_ADDRESS`, `UNISWAP_V3_ROUTER`, `UNISWAP_V3_QUOTER`, `ARBITRUM_CHAIN_ID`.

### `ingestion` — Data Ingestion

Collects raw market data from multiple sources:

- **`pool_events`** — Primary price source. Subscribes to the pool's `Swap` event via `eth_subscribe`, parses `sqrtPriceX96` → ETH/USDC price and `liquidity`. Updates shared state in real-time. ~95% fewer RPC calls than polling.
- **`gas_estimator`** — Real-time gas estimation. Uses `eth_gasPrice` + `eth_estimateGas` to compute exact USD cost of a transaction. Replaces the original hardcoded $0.10.
- **`event_logger`** — CSV event logger. Writes every loop iteration to `data/events_YYYY-MM-DD.csv` with rotation at UTC midnight. Fields: timestamp, block, price, regime, action, confidence, size multiplier, execution result, route, tx hash, error, gas cost, MEV cost, edge, reward-to-cost ratio.
- **`price_feed`** — Secondary price source. Binance WS 1m klines (CEX price reference).
- **`pool_reader`** — Fallback on-chain reader. Polls `slot0()` + `liquidity()` every 12s as a safety net if Swap events are missed.
- **`orderbook`** — Builds a simulated orderbook from pool liquidity (simplified; not real DEX orderbook data).
- **`context_builder`** — Assembles `MarketContext` from all shared state sources. Handles whale tx estimation using decoded calldata direction (from mempool monitor) rather than the old `tx.to == USDC_ADDRESS` heuristic.

### `signals` — Trading Signals

Each signal implements the `Signal` trait and produces a `SignalResult`:

- **`RsiSignal`** — Relative Strength Index overbought/oversold
- **`EmaCrossoverSignal`** — EMA fast/slow crossover detection
- **`OrderBookSignal`** — Orderbook imbalance signal (bid/ask pressure)
- **`OnchainFlowSignal`** — Whale transaction and large flow detection (uses decoded direction from mempool)

### `consensus` — Decision Engine

- **`ConsensusEngine`** — Collects votes from all signals, requires minimum votes and confidence threshold to produce a `Decision`
- Runs regime pre-gate first (ATR-based: Ranging=block, Trending=1.0x, Volatile=0.5x)
- Configurable via `required_votes` and `min_confidence`

### `mev_guard` — MEV Protection

Protects against sandwich attacks, frontrunning, and other MEV threats:

- **`MempoolMonitor`** — Watches pending transactions. Now includes a **swap calldata decoder** that parses `exactInputSingle` to determine true buy/sell direction for whale detection.
- **`FlashbotsRelay`** — Submits bundles via Flashbots to avoid public mempool
- **`SplitExecutor`** — Splits large trades into smaller chunks across blocks
- **`MevGuard`** — Orchestrator: assesses threat level and recommends `ExecutionRoute`

### `risk` — Risk Management

- **`RiskChecker`** — Validates trades against configured limits (max trade size, max position, daily loss limit)
- **`ProfitabilityCheck`** — Edge vs cost calculation with reward-to-cost ratio minimum of 1.5x
- Uses **real gas estimates** from `GasEstimator` (not hardcoded $0.10)
- Resets daily loss counter at midnight
- Supports state injection via `new_with_state()` for persistence across restarts

### `notify` — Notifications

- **`Notifier`** — Sends Discord webhook embeds for trade events
- Fires asynchronously (non-blocking via `tokio::spawn`)
- `TradeNotification` struct captures action, amount, route, risk metrics, tx hash, error details

### `executor` — Trade Execution

- **`SwapBuilder`** — Builds Uniswap V3 swap transactions via the quoter and router
- **`Simulator`** — Pre-trade simulation. Calls `eth_call` with the pending transaction to verify output against current pool state before signing. Retries with higher slippage tolerance if simulation fails.
- **`ExecutorActor`** — mpsc-based actor that **owns the signer and nonce**. Lifecycle:
  1. Receive `TradeIntent` from main loop
  2. Simulate via `eth_call`
  3. Sign with next nonce (EIP-1559)
  4. Broadcast via public mempool (Phase 1)
  5. `tokio::select!` loop monitoring for confirmation, timeout, or revert
  6. On timeout: gas bump +20%, re-sign, retry (3x max)
  7. Send `ExecutionResult` back via `oneshot`
- **`TxSigner`** — Signs transactions using a private key from environment. Owned exclusively by `ExecutorActor`.
- **`Broadcaster`** — Sends transactions via selected route (public, Flashbots, or split)

## `bin` — Entrypoints

### `vertexa` — Main binary

- Reads `config/default.toml`
- Initializes all components
- Spawns background tasks: pool events listener, pool reader fallback, mempool monitor, executor actor, midnight reset
- Loads persisted state from `vertexa_state.json` (if exists)
- Registers signal handler for SIGINT/SIGTERM
- Runs the main loop:
  1. Build `MarketContext` from `SharedState`
  2. Evaluate signals → consensus decision (with regime pre-gate)
  3. If decision is directional → plan trade
  4. Assess MEV threat → select execution route
  5. Estimate gas cost (real-time)
  6. Check profitability (edge vs real gas + slippage + MEV)
  7. Check risk limits
  8. Send `TradeIntent` to `ExecutorActor` via mpsc
  9. Await `ExecutionResult` via oneshot
  10. Log to CSV
  11. Send notification (Discord webhook)
  12. Record trade
- On shutdown: persists state (daily loss, circuit breaker, position) to JSON file

### `backtester` — Backtesting binary

- Reads CSV event logs from `data/events_*.csv`
- Replays through consensus engine or computes PnL from historical decisions
- Outputs: PnL, win rate, Sharpe ratio, max drawdown
- Used to validate strategy changes against historical data before going live

## Configuration

See `config/default.toml` for full reference. Sections: `[network]`, `[trading]`, `[risk]`, `[mev]`, `[consensus]`, `[notify]`.

Environment variable overrides:
- `VERTEXA_RPC_WS` — Override WebSocket RPC URL
- `VERTEXA_PAPER=true` — Enable paper trade mode (no real transactions)

## Key Design Decisions

### Swap Events Over Polling
The original 12s `slot0()` poll was replaced with a WebSocket subscription to the pool's `Swap` event. This provides sub-block price updates and reduces RPC usage by ~95%. The poll remains as a fallback.

### Executor Actor Over Direct Broadcast
The original synchronous `broadcaster.broadcast()` was replaced with an mpsc-based actor pattern. The actor owns the signer and nonce, enabling atomic nonce management and reliable gas bump retries without race conditions.

### Mempool Calldata Decoding Over Heuristics
The original whale detection used `tx.to == USDC_ADDRESS` to guess buy/sell direction. This was incorrect for router-based swaps. The new approach ABI-decodes `exactInputSingle` calldata from the Uniswap V3 router to determine the actual direction.

### Real Gas Estimates Over Hardcoded Default
The original `$0.10` gas estimate was replaced with real-time estimation using `eth_gasPrice` + `eth_estimateGas`. When estimation fails, it falls back to `$0.10` with a warning.

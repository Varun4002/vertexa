# Vertexa

Autonomous DEX trading bot for Arbitrum with real-time on-chain data ingestion, volatility regime detection, multi-signal consensus, MEV protection, gas-adjusted profitability gating, nonce-managed executor actor, event logging for backtesting, Discord notifications, and graceful shutdown with state persistence.

---

## Overview

Vertexa is a production-grade, modular trading bot that:

1. **Ingests** real-time on-chain data via Swap event subscription (replaces polling)
2. **Decodes** mempool swap calldata to determine true whale buy/sell direction
3. **Regime pre-filter** — ATR-based volatility classification (blocks ranging markets)
4. **Computes** 4 technical signals + majority vote consensus
5. **Assesses MEV threats** from mempool scanning and sandwich detection
6. **Simulates** trades via `eth_call` before signing (pre-flight check)
7. **Checks profitability** using real gas estimates (not hardcoded $0.10)
8. **Enforces risk limits** (position limits, daily loss circuit breaker)
9. **Executes** via a dedicated executor actor with nonce management and gas bump retry
10. **Logs** every loop iteration to CSV for backtesting and strategy iteration
11. **Notifies** via Discord webhook on trade attempts (success/failure/abort)
12. **Persists state** to disk and restores on restart (daily loss, circuit breaker, position)
13. **Shuts down gracefully** on SIGINT/SIGTERM — saves state before exit

---

## Features

### Voting Signals

| Signal | Description | Parameters |
|--------|-------------|------------|
| **RSI** | Relative Strength Index | Period 14, Buy < 30, Sell > 70 |
| **EMA Crossover** | 9/21 EMA crossover detection | Fast=9, Slow=21 |
| **OrderBook Imbalance** | Bid/ask depth imbalance | Weighted by price level |
| **Onchain Flow** | Whale transaction tracking | Filter by USD value threshold |

### Volatility Regime Pre-Gate

Prevents false signals in ranging markets and reduces size in volatile markets.

```
ATR Calculation (close-to-close approximation):
  true_range[i] = abs(prices[i] - prices[i-1])
  atr = mean(last 14 true_ranges)
  atr_pct = atr / current_price

Regime Classification:
  ┌─────────────────────────────────────────────────────────┐
  │ atr_pct < 0.008      →  Ranging   →  BLOCK ALL TRADES  │
  │ 0.008 ≤ atr_pct ≤ 0.025  →  Trending  →  size_multiplier = 1.0 │
  │ atr_pct > 0.025      →  Volatile  →  size_multiplier = 0.5 │
  └─────────────────────────────────────────────────────────┘
```

### Profitability Gate

Prevents execution when expected profit doesn't exceed total costs. Uses **real gas estimates** from the network rather than a hardcoded default.

```
Inputs:
  - trade.amount_usd
  - decision.avg_confidence (0.0 to 1.0)
  - mev_assessment.estimated_mev_loss_usd
  - gas_estimate_usd = real-time from GasEstimator (was $0.10 hardcoded)

Calculation:
  edge_pct = 0.005 + (avg_confidence * 0.01)
  expected_edge_usd = trade_amount * edge_pct
  slippage_cost = trade_amount * max_slippage
  total_cost = gas + slippage + mev_loss
  reward_to_cost = expected_edge / total_cost

Decision:
  if reward_to_cost >= 1.5  →  PROFITABLE (execute)
  else                       →  NOT PROFITABLE (abort)
```

### MEV Protection

Three execution routes based on threat assessment, with **pre-trade simulation** to verify output:

| Route | When to Use |
|-------|-------------|
| **Public Mempool** | Low sandwich probability, low urgency |
| **Flashbots Bundle** | High MEV risk, needs frontrunning protection |
| **Split Execution** | Large orders that can't be atomic |
| **Abort** | MEV threat exceeds acceptable threshold |

### Executor Actor

Dedicated `tokio::sync::mpsc`-based actor that **owns the signer and nonce**:

1. Receives `TradeIntent` (token, amount, direction, min output)
2. Simulates via `eth_call` to verify output against current pool state
3. Signs transaction with the next nonce
4. Broadcasts and enters a `tokio::select!` loop:
   - **Confirmation** → returns success
   - **Timeout (5s)** → gas bump (+20%), re-signs with same nonce, retries up to 3x
   - **Revert** → returns failure
5. Sends `ExecutionResult` back via `oneshot` channel for logging and notification

This eliminates nonce collisions and enables reliable gas bump retries.

### Real-Time Price via Swap Events

Replaces 12-second polling of `slot0()` with a WebSocket subscription to the pool's `Swap` event:

- Parses `sqrtPriceX96` and `liquidity` from each event
- Updates shared state atomically via `Arc<RwLock<f64>>`
- Falls back to `slot0()` poll if no event received in 60s (missed event guard)
- ~95% reduction in RPC usage vs polling

### Mempool Whale Decoder

Decodes `exactInputSingle` calldata from the Uniswap V3 router to determine true trade direction:

- Parses `tokenIn`/`tokenOut` from pending transaction input
- Classifies: buying ETH (USDC→WETH) or selling ETH (WETH→USDC)
- Replaces heuristic `tx.to == USDC_ADDRESS` guess with actual decoded direction
- Multi-hop swaps (`exactInput`) are skipped — too complex for reliable direction inference

### Notifications

Sends Discord webhook embeds for every trade attempt:

- **Success**: Green embed with action, amount, route, risk metrics, tx hash
- **Failure**: Orange embed with error reason
- **Abort**: Orange embed with reason (MEV, profitability, risk gate)

Configure by adding a `[notify]` section to your config:

```toml
[notify]
discord_webhook_url = "https://discord.com/api/webhooks/..."
```

### Event Logging & Backtesting

Every loop iteration is logged to `data/events_YYYY-MM-DD.csv`:

```
timestamp,block,price,regime,action,confidence,size_mult,executed,route,tx_hash,error,gas_usd,mev_usd,edge_usd,reward_to_cost
```

The `backtester` binary replays this CSV to:
- Compute PnL, win rate, Sharpe ratio, max drawdown
- Re-run consensus with modified signal parameters ("what if" scenarios)
- Validate strategy changes against historical data before going live

### Graceful Shutdown

- Traps SIGINT (Ctrl+C) and SIGTERM
- Persists daily loss counter, circuit breaker state, and position tracking to `vertexa_state.json`
- Restores state on next startup
- Exits cleanly without mid-trade interruption

---

## Architecture

```
┌────────────────────────────────────────────────────────────────────┐
│                        MAIN LOOP (vertexa.rs)                        │
│  ┌────────────────────────────────────────────────────────────┐   │
│  │  1. Build MarketContext (from SharedState)                 │   │
│  │  2. ConsensusEngine::evaluate()                            │   │
│  │     ├── Regime pre-gate (BLOCK if ranging)               │   │
│  │     ├── Size multiplier (1.0x trending, 0.5x volatile)   │   │
│  │     ├── Signal evaluation & majority vote                 │   │
│  │     └── Confidence threshold gate                         │   │
│  │  3. If Neutral → continue                                 │   │
│  │  4. Apply size_multiplier → adjusted_usd                  │   │
│  │  5. Build TradeIntent                                     │   │
│  │  6. MEV assessment (mev_guard)                           │   │
│  │  7. Gas estimate (real-time from GasEstimator)           │   │
│  │  8. PROFITABILITY CHECK (uses real gas + MEV estimate)   │   │
│  │  9. Risk checks (limits, circuit breaker)                │   │
│  │  10. Send TradeIntent ──mpsc──► ExecutorActor            │   │
│  │  11. Log to CSV (EventLogger)                            │   │
│  │  12. Notify (Discord webhook)                            │   │
│  └────────────────────────────────────────────────────────────┘   │
└────────────────────────────────────────────────────────────────────┘

                          EXECUTOR ACTOR (spawned task)
┌────────────────────────────────────────────────────────────────────┐
│  mpsc::Receiver<TradeIntent>                                       │
│       │                                                             │
│   1. Simulate via eth_call (pre-flight check)                      │
│   2. Sign tx with next nonce                                       │
│   3. Broadcast                                                     │
│   4. tokio::select! loop:                                          │
│        ├ Confirmed   ──→ oneshot::Sender──► log + notify           │
│        ├ Timeout(5s) ──→ gas bump +20%, re-sign, retry (3x max)   │
│        └ Reverted    ──→ oneshot::Sender──► log + abort            │
└────────────────────────────────────────────────────────────────────┘

                         DATA FLOW:
┌─────────────┐  Swap events   ┌──────────────┐        ┌──────────────┐
│ Pool Events ├───────────────►│ SharedState   │        │  Mempool     │
│ (spawned)   │                │ (Arc<RwLock>) │        │  Monitor     │
└─────────────┘                │              │        │  (spawned)   │
                               │ price_series │        └──────┬───────┘
┌─────────────┐                │ pool_price   │               │
│ Pool Reader ├───────────────►│ liquidity    │       Decoded calldata
│ (fallback)  │                │ block_number │               │
└─────────────┘                │ pending_txs  │◄──────────────┘
                               └──────────────┘
                                      │
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
                               └──────────────┘
```

---

## Configuration

`config/default.toml`:

```toml
[network]
rpc_http = "https://arb1.arbitrum.io/rpc"
rpc_ws = "wss://arb1.arbitrum.io/ws"
chain_id = 42161

[trading]
pair = "ETH/USDC"
pool_address = "0xC31E54c7a869B9FcBEcc14363CF510d1c41fa443"
pool_fee_tier = 500
max_trade_usd = 10000.0
max_position_usd = 50000.0
loop_interval_s = 15

[risk]
daily_loss_limit_usd = 500.0
max_slippage_public = 0.005
max_slippage_flashbots = 0.002
max_slippage_split = 0.001
min_chunk_usd = 500.0

[mev]
flashbots_relay_url = "https://relay.arbitrum.io"
known_bot_addresses = [
    "0x00000000003b3cc22aF3aE1EAc0440BcEe416B40",
]
mempool_buffer_size = 500

[consensus]
required_votes = 3
min_confidence = 0.35

[notify]
discord_webhook_url = "https://discord.com/api/webhooks/..."
```

**Environment Variables:**

| Variable | Purpose |
|----------|---------|
| `VERTEXA_RPC_WS` | Override WebSocket RPC endpoint |
| `VERTEXA_PAPER` | Set to `true` for paper trading |

**Private Key:**

| Variable | Purpose |
|----------|---------|
| `VERTEXA_PRIVATE_KEY` | Ethereum private key for signing transactions |

The bot expects `VERTEXA_PRIVATE_KEY` in the environment or a `.env` file. Required unless running in paper mode.

---

## Quick Start

### Prerequisites

- Rust 1.75+
- Arbitrum RPC endpoint (HTTP + WebSocket)
- Ethereum private key (for signing transactions)

### Build

```bash
cargo build --release
```

### Paper Trade (Recommended for Testing)

No real transactions will be submitted:

```bash
VERTEXA_PAPER=true cargo run --release
```

### Production

```bash
cargo run --release
```

### Validation

```bash
# Check compilation
cargo check --workspace

# Run lints
cargo clippy --workspace -- -D warnings

# Run tests
cargo test --workspace
```

---

## Execution Flow (Step-by-Step)

```
┌──────────────────────────────────────────────────────────────┐
│                    1. MARKET CONTEXT                          │
│  ├── Price feed (Binance WS, 1m klines)                     │
│  ├── Pool events (Swap event subscription, real-time)        │
│  ├── Pool reader (slot0() fallback poll every 12s)           │
│  ├── Orderbook (simulated from on-chain liquidity)           │
│  ├── Recent whale transactions (calldata-decoded direction)  │
│  └── Block number + timestamp                                 │
└──────────────────────────────────────────────────────────────┘
                              ↓
┌──────────────────────────────────────────────────────────────┐
│                    2. REGIME PRE-GATE                         │
│  ├── Calculate ATR from last 14 price changes               │
│  ├── Classify: Ranging / Trending / Volatile                │
│  ├── IF RANGING: RETURN NEUTRAL (block trades)              │
│  └── Set size_multiplier: Trending=1.0, Volatile=0.5       │
└──────────────────────────────────────────────────────────────┘
                              ↓
┌──────────────────────────────────────────────────────────────┐
│                    3. SIGNALS & CONSENSUS                    │
│  ├── RSI evaluation                                           │
│  ├── EMA crossover detection                                  │
│  ├── Orderbook imbalance calculation                         │
│  ├── Onchain whale flow analysis                             │
│  ├── Majority vote aggregation                               │
│  └── Confidence threshold gate (min 0.35)                   │
└──────────────────────────────────────────────────────────────┘
                              ↓
┌──────────────────────────────────────────────────────────────┐
│                    4. TRADE PLANNING                          │
│  ├── Apply size_multiplier to max_trade_usd                  │
│  ├── If adjusted < $10.00 → skip                             │
│  ├── Select token_in / token_out based on Buy/Sell          │
│  └── Calculate amount_in and set slippage                    │
└──────────────────────────────────────────────────────────────┘
                              ↓
┌──────────────────────────────────────────────────────────────┐
│                    5. MEV ASSESSMENT                          │
│  ├── Mempool scan for known bot addresses                    │
│  ├── Decode swap calldata for whale direction                │
│  ├── Sandwich probability calculation                        │
│  ├── Estimated MEV loss (USD)                                │
│  └── Recommended route: Public / Flashbots / Split / Abort  │
└──────────────────────────────────────────────────────────────┘
                              ↓
┌──────────────────────────────────────────────────────────────┐
│                    6. PROFITABILITY GATE                      │
│  ├── Real gas estimate (from GasEstimator)                   │
│  ├── Expected edge: confidence → % move                      │
│  ├── Costs: gas (real) + slippage + MEV loss                │
│  ├── reward_to_cost = expected_edge / total_cost             │
│  └── IF ratio < 1.5 → ABORT (not profitable)                │
└──────────────────────────────────────────────────────────────┘
                              ↓
┌──────────────────────────────────────────────────────────────┐
│                    7. RISK CHECKS                             │
│  ├── Circuit breaker active? (daily loss exceeded)           │
│  ├── Trade size <= max_trade_usd?                            │
│  ├── New position <= max_position_usd?                        │
│  └── Slippage within safe limit for route?                   │
└──────────────────────────────────────────────────────────────┘
                              ↓
┌──────────────────────────────────────────────────────────────┐
│                    8. EXECUTE via ACTOR                       │
│  ├── Send TradeIntent to ExecutorActor via mpsc              │
│  ├── Actor simulates via eth_call (pre-flight check)        │
│  ├── Actor signs with next nonce                             │
│  ├── Actor broadcasts (public mempool)                       │
│  ├── Actor monitors for confirmation / timeout / revert     │
│  ├── On timeout: gas bump +20%, re-sign, retry (up to 3x)   │
│  └── ExecutionResult sent back via oneshot                   │
└──────────────────────────────────────────────────────────────┘
                              ↓
┌──────────────────────────────────────────────────────────────┐
│                    9. LOG & NOTIFY                            │
│  ├── CSV event logger (every iteration, data/events_*.csv)   │
│  ├── Discord webhook embed — success, failure, or abort      │
│  └── Record trade for position tracking                      │
└──────────────────────────────────────────────────────────────┘
```

State persistence and graceful shutdown wrap the entire loop — state is saved to `vertexa_state.json` on SIGINT/SIGTERM and restored on next startup.

---

## Tech Stack

| Component | Technology |
|-----------|------------|
| Runtime | Tokio (async) |
| Web3 | Alloy |
| Actor Communication | `tokio::sync::mpsc`, `oneshot` |
| Error Handling | `eyre` (app), `thiserror` (libraries) |
| Config | `config-rs` + `dotenvy` |
| Logging | `tracing` (JSON output), CSV event files |
| Notifications | Discord webhooks via `reqwest` |
| Backtesting | Replay binary (`backtester`) |
| Target Chain | Arbitrum (Chain ID 42161) |
| Target DEX | Uniswap V3 (WETH/USDC 0.05%) |

---

## Project Structure

```
Vertexa/
├── AGENTS.md           # AI agent context / dev conventions
├── Cargo.toml          # Workspace manifest
├── README.md           # This file
├── vertexa_state.json  # Persisted state (auto-generated)
├── data/               # Event CSV logs (auto-generated)
├── bin/
│   ├── vertexa.rs      # Main entrypoint & loop
│   └── backtester.rs   # CSV replay / backtesting binary
├── config/
│   └── default.toml    # Configuration
└── crates/
    ├── core/           # Shared types, traits, errors
    ├── consensus/      # Voting & regime pre-gate
    ├── signals/        # RSI, EMA, OrderBook, OnchainFlow, VolatilityRegime
    ├── ingestion/      # Data collection (pool_events, gas_estimator, event_logger)
    ├── mev_guard/      # MEV detection & routing (swap calldata decoder)
    ├── risk/           # Risk checks + profitability gate
    ├── executor/       # Swap building, simulator, executor actor, signer
    └── notify/         # Discord webhook notifications
```

---

## Safety & Operational Guidelines

1. **Always test in paper mode first** (`VERTEXA_PAPER=true`)
2. **Start with small position sizes**
3. **Set conservative daily loss limits**
4. **Monitor logs and Discord notifications continuously**
5. **Run the backtester before changing any signal parameters**
6. **Understand that past performance ≠ future results**

---

## License

Internal project. Not for external distribution.

---

## Contributing

Read `AGENTS.md` for code conventions before making changes.

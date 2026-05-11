# Vertexa

Autonomous DEX trading bot for Arbitrum with volatility regime detection, MEV protection, and gas-adjusted profitability gating.

---

## Overview

Vertexa is a production-grade, modular trading bot that:

1. **Ingests** on-chain market data (prices, volumes, orderbook, whale transactions)
2. **Computes** 5 technical signals + volatility regime classification
3. **Reaches consensus** via weighted voting with pre-gate regime filtering
4. **Assesses MEV threats** from mempool scanning and sandwich detection
5. **Checks profitability** (expected edge vs gas + slippage + MEV loss)
6. **Enforces risk limits** (position limits, daily loss circuit breaker)
7. **Executes** via optimal route: public mempool, Flashbots bundle, or split execution

---

## Features

### Signals

| Signal | Description | Parameters |
|--------|-------------|------------|
| **RSI** | Relative Strength Index | Period 14, Buy < 30, Sell > 70 |
| **EMA Crossover** | 9/21 EMA crossover detection | Fast=9, Slow=21 |
| **OrderBook Imbalance** | Bid/ask depth imbalance | Weighted by price level |
| **Onchain Flow** | Whale transaction tracking | Filter by USD value threshold |
| **Volatility Regime** | ATR-based market classification | Period 14, thresholds 0.8% and 2.5% |

### Volatility Regime Filter

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

Prevents execution when expected profit doesn't exceed total costs.

```
Inputs:
  - trade.amount_usd
  - decision.avg_confidence (0.0 to 1.0)
  - mev_assessment.estimated_mev_loss_usd
  - gas_estimate_usd = $0.10 (Arbitrum default)

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

Three execution routes based on threat assessment:

| Route | When to Use |
|-------|-------------|
| **Public Mempool** | Low sandwich probability, low urgency |
| **Flashbots Bundle** | High MEV risk, needs frontrunning protection |
| **Split Execution** | Large orders that can't be atomic |
| **Abort** | MEV threat exceeds acceptable threshold |

---

## Architecture

```
┌─────────────────────────────────────────────────────────────────┐
│                        bin/vertexa.rs                             │
│  ┌─────────────────────────────────────────────────────────┐   │
│  │                    MAIN LOOP                              │   │
│  ├─────────────────────────────────────────────────────────┤   │
│  │  1. Build MarketContext                                  │   │
│  │  2. ConsensusEngine::evaluate()                          │   │
│  │     ├── Regime pre-gate (BLOCK if ranging)             │   │
│  │     ├── Size multiplier (1.0x trending, 0.5x volatile)│   │
│  │     ├── Signal evaluation & majority vote               │   │
│  │     └── Confidence threshold gate                       │   │
│  │  3. If Neutral → continue                               │   │
│  │  4. Apply size_multiplier                               │   │
│  │  5. Build PlannedTrade                                  │   │
│  │  6. MEV assessment (mev_guard)                         │   │
│  │  7. PROFITABILITY CHECK (uses MEV estimate)            │   │
│  │  8. Existing risk checks                                │   │
│  │  9. Execute via recommended route                       │   │
│  └─────────────────────────────────────────────────────────┘   │
└─────────────────────────────────────────────────────────────────┘

                        CRATE LAYOUT:

┌──────────┐  ┌──────────┐  ┌───────────┐  ┌───────────┐
│  core    │  │ signals  │  │ consensus │  │  ingestion│
├──────────┤  ├──────────┤  ├───────────┤  ├───────────┤
│ types    │  │ rsi      │  │ engine    │  │price_feed │
│ signal   │  │ ema      │  │           │  │pool_reader│
│ errors   │  │ orderbook│  │           │  │context_bld│
│          │  │onchain_flow│ │           │  │           │
│          │  │volatility_  │ │           │  │           │
│          │  │  regime    │ │           │  │           │
└──────────┘  └──────────┘  └───────────┘  └───────────┘

┌──────────┐  ┌──────────┐  ┌───────────┐
│mev_guard │  │  risk    │  │ executor  │
├──────────┤  ├──────────┤  ├───────────┤
│ detector │  │ checker  │  │swap_builder│
│ flashbots│  │profitability││  signer   │
│split_exec│  │          │  │broadcaster│
│  guard   │  │          │  │           │
└──────────┘  └──────────┘  └───────────┘
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
```

**Environment Variables:**

| Variable | Purpose |
|----------|---------|
| `VERTEXA_RPC_WS` | Override WebSocket RPC endpoint |
| `VERTEXA_PAPER` | Set to `true` for paper trading |

**Private Key:**

The bot expects a standard Ethereum private key in the environment. Configure via your wallet or key management solution.

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
│  ├── Price feed (latest prices & volumes)                    │
│  ├── Pool reader (current price, liquidity)                  │
│  ├── Orderbook (simulated from on-chain data)               │
│  ├── Recent whale transactions                               │
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
│  ├── Sandwich probability calculation                        │
│  ├── Estimated MEV loss (USD)                                │
│  └── Recommended route: Public / Flashbots / Split / Abort  │
└──────────────────────────────────────────────────────────────┘
                              ↓
┌──────────────────────────────────────────────────────────────┐
│                    6. PROFITABILITY GATE                      │
│  ├── Expected edge: confidence → % move                      │
│  ├── Costs: gas ($0.10) + slippage + MEV loss              │
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
│                    8. EXECUTION                                │
│  ├── Build Uniswap V3 swap calldata                          │
│  ├── Sign transaction with private key                        │
│  ├── Broadcast via recommended route                          │
│  └── Record trade for position tracking                       │
└──────────────────────────────────────────────────────────────┘
```

---

## Tech Stack

| Component | Technology |
|-----------|------------|
| Runtime | Tokio (async) |
| Web3 | Alloy |
| Error Handling | `eyre` (app), `thiserror` (libraries) |
| Config | `config-rs` + `dotenvy` |
| Logging | `tracing` (JSON output) |
| Target Chain | Arbitrum (Chain ID 42161) |

---

## Project Structure

```
Vertexa/
├── AGENTS.md           # AI agent context / dev conventions
├── Cargo.toml          # Workspace manifest
├── README.md           # This file
├── bin/
│   └── vertexa.rs      # Main entrypoint & loop
├── config/
│   └── default.toml    # Configuration
└── crates/
    ├── core/           # Shared types, traits, errors
    ├── consensus/      # Voting & regime pre-gate
    ├── signals/        # RSI, EMA, OrderBook, OnchainFlow, VolatilityRegime
    ├── ingestion/      # Data collection
    ├── mev_guard/      # MEV detection & routing
    ├── risk/           # Risk checks + profitability gate
    └── executor/       # Swap building & broadcasting
```

---

## Safety & Operational Guidelines

1. **Always test in paper mode first** (`VERTEXA_PAPER=true`)
2. **Start with small position sizes**
3. **Set conservative daily loss limits**
4. **Monitor logs continuously**
5. **Understand that past performance ≠ future results**

---

## License

Internal project. Not for external distribution.

---

## Contributing

Read `AGENTS.md` for code conventions before making changes.

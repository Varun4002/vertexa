# Vertexa — AI Agent Instructions

## Project Overview

Vertexa is an autonomous DEX trading bot for Arbitrum. It ingests on-chain market data, computes technical signals (RSI, EMA crossover, orderbook imbalance, onchain flow, volatility regime), reaches consensus via weighted voting with pre-gate regime filtering, assesses MEV threats, checks profitability (gas + slippage + MEV vs expected edge), checks risk limits, and executes swaps through the public mempool, Flashbots, or split execution.

## Workspace Structure

```text
crates/core/       Shared types, traits, errors, constants
crates/ingestion/  Data ingestion — price feeds, pool reader, orderbook, context builder
crates/signals/    Trading signals — RSI, EMA, orderbook, onchain flow, volatility_regime
crates/consensus/  Consensus engine — aggregates signal votes, regime pre-gate, size multiplier
crates/mev_guard/  MEV protection — mempool monitor, Flashbots relay, split executor
crates/risk/       Risk checking — position limits, daily loss limits, profitability gate
crates/executor/   Execution — swap builder, tx signer, broadcaster
bin/               Binary entrypoint (vertexa.rs)
```

## Key Types Added

- `FixedUsd` — Explicit money wrapper around `f64` with `from_dollars()`, `to_dollars()`, `Add`, `Sub`, `Display`
- `Regime` — `Ranging`, `Trending`, `Volatile` (volatility classification)
- `Decision` now has:
  - `size_multiplier: f64` — `1.0` = full, `0.5` = half, `0.0` = blocked
  - `blocked_by: Option<&'static str>` — reason if blocked
- `CostBreakdown` — gas + slippage + MEV cost analysis
- `VertexaError::TradeNotProfitable { reward_to_cost, minimum }`

## Key Conventions

- **Async runtime:** Tokio
- **Error handling:** `eyre` for application-level, `thiserror` for library errors (`VertexaError`)
- **Configuration:** `config/default.toml` + env var overrides (`VERTEXA_RPC_WS`, `VERTEXA_PAPER`)
- **Logging:** `tracing` with json output and env-filter, always `target: "vertexa"`
- **Web3:** Alloy framework
- **Target:** Arbitrum (chain ID 42161)
- **Money arithmetic:** Use `FixedUsd`, not raw `f64` for financial values

## Signals

| Signal | Purpose |
|--------|---------|
| `RsiSignal` | RSI 14-period, buy <30, sell >70 |
| `EmaCrossoverSignal` | EMA 9/21 crossover |
| `OrderBookSignal` | Bid/ask imbalance |
| `OnchainFlowSignal` | Whale transaction flow |
| `VolatilityRegimeSignal` | ATR-based regime classification (Ranging = block, Trending = 1.0x, Volatile = 0.5x) |

## Consensus Engine Flow

```
1. REGIME PRE-GATE (FIRST)
   ├── ATR calculation: true_range[i] = abs(prices[i] - prices[i-1])
   ├── atr_pct = atr / current_price
   ├── atr_pct < 0.008 → Ranging → BLOCK TRADE
   ├── 0.008 ≤ atr_pct ≤ 0.025 → Trending → size_multiplier = 1.0
   └── atr_pct > 0.025 → Volatile → size_multiplier = 0.5

2. Run all signals, collect votes

3. Majority vote logic

4. Confidence gate (min_confidence threshold)
```

## Profitability Gate Algorithm

```
Inputs:
  - trade.amount_usd
  - decision.avg_confidence
  - mev_assessment.estimated_mev_loss_usd
  - gas_estimate_usd = $0.10 (Arbitrum default)

Calculation:
  edge_pct = 0.005 + (avg_confidence * 0.01)
  expected_edge_usd = amount * edge_pct
  slippage_cost = amount * max_slippage
  total_cost = gas + slippage + mev_loss
  reward_to_cost = expected_edge / total_cost

Decision:
  reward_to_cost >= 1.5 → PROFITABLE (execute)
  else                   → NOT PROFITABLE (abort)
```

## Main Loop Order

```
1. Build MarketContext
2. Run consensus → decision (includes regime pre-gate)
3. If Neutral → continue
4. Apply size_multiplier → adjusted_usd
5. If adjusted_usd < 10.0 → continue
6. Build PlannedTrade with adjusted_usd
7. MEV assessment → mev_assessment (MOVED BEFORE risk checks)
8. PROFITABILITY CHECK (uses MEV estimate)
9. Existing risk.check()
10. Execute via recommended route
```

## Build & Run

```bash
cargo build --release          # production build
cargo run --release            # run (reads config/default.toml)
VERTEXA_PAPER=true cargo run   # paper trade mode (no real txns)
```

## Validation Commands

```bash
cargo check --workspace
cargo clippy --workspace -- -D warnings
cargo test --workspace
```

## Agent Rules

- Read existing files before modifying them
- Type mismatches will break compilation. Match existing types exactly.
- `FixedUsd` for all money. Never `f64` for financial arithmetic.
- No `unwrap()`, no `expect()`, no `todo!()`, no `unimplemented!()`
- No new dependencies unless justified
- Do not reformat, rename, or restructure existing code not explicitly listed
- Every new tracing log must include `target: "vertexa"`

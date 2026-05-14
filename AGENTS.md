# Vertexa — AI Agent Instructions

## Project Overview

Vertexa is an autonomous DEX trading bot for Arbitrum. It ingests on-chain market data via real-time Swap, Mint, and Burn event subscription (not polling), maintains a local tick-by-tick liquidity chart, decodes mempool swap calldata for whale direction (Uniswap V3 + 1inch), computes technical signals (RSI, EMA crossover, orderbook imbalance, onchain flow with CVD, adaptive volatility regime), reaches consensus via weighted voting with adaptive pre-gate regime filtering, simulates trades via `eth_call` before signing, assesses MEV threats, checks profitability using predictive gas estimates (EIP-1559), checks risk limits, sends Discord notifications, and executes swaps through an mpsc-based executor actor with nonce management and aggressive speculative gas escalation. It logs every iteration to CSV for backtesting, supports a replay binary for strategy iteration, and provides graceful shutdown (SIGINT/SIGTERM) with state persistence.

## Workspace Structure

```text
crates/core/       Shared types, traits, errors, constants
crates/ingestion/  Data ingestion — pool_events (Swap/Mint/Burn subscription),
                   gas_estimator (predictive), event_logger (CSV), pool_reader,
                   price_feed, orderbook, context_builder
crates/signals/    Trading signals — RSI, EMA, orderbook, onchain flow (CVD), volatility_regime (Adaptive)
crates/consensus/  Consensus engine — aggregates signal votes, regime pre-gate, size multiplier
crates/mev_guard/  MEV protection — mempool monitor (with Uniswap/1inch calldata decoder),
                   Flashbots relay, split executor
crates/risk/       Risk checking — position limits, daily loss limits, profitability gate
crates/executor/   Execution — swap builder, simulator (eth_call), executor actor (Aggressive),
                   tx signer, broadcaster
crates/notify/     Notifications — Discord webhook notifier
bin/               Binary entrypoints (vertexa.rs, backtester.rs)
```

## Key Types

- `FixedUsd` — Explicit money wrapper around `f64` with `from_dollars()`, `to_dollars()`, `Add`, `Sub`, `Display`
- `Regime` — `Ranging`, `Trending`, `Volatile` (adaptive volatility classification)
- `TickLiquidity` — Local reconstruction of the Uniswap V3 tick chart from Mint/Burn events
- `Decision` — includes `size_multiplier: f64` (1.0/0.5/0.0) and `blocked_by: Option<&'static str>`
- `CostBreakdown` — gas + slippage + MEV cost analysis
- `TradeIntent` — Sent to ExecutorActor via mpsc (token, amount, direction, min output)
- `ExecutionResult` — Returned from ExecutorActor via oneshot (success/failure, tx hash, gas used, actual_amount_out)
- `VertexaError::TradeNotProfitable { reward_to_cost, minimum }`
- `TradeNotification` — Discord-ready notification struct with action, amount, route, risk, tx hash
- `actual_amount_out` — Used for post-execution slippage kill-switch (detects deviation from expected_min)

## Key Conventions

- **Async runtime:** Tokio
- **Actor communication:** `tokio::sync::mpsc` (TradeIntent → ExecutorActor), `oneshot` (result back)
- **Shared state:** `Arc<RwLock<PriceSeries>>`, `Arc<RwLock<TickLiquidity>>` for local liquidity chart
- **Error handling:** `eyre` for application-level, `thiserror` for library errors (`VertexaError`)
- **Configuration:** `config/default.toml` + env var overrides (`VERTEXA_RPC_WS`, `VERTEXA_PAPER`)
- **Address Reputation:** `[reputation]` config section for weighting "smart money" whale flows
- **Logging:** `tracing` with json output and env-filter, always `target: "vertexa"`
- **CSV logging:** Every loop iteration written to `data/events_YYYY-MM-DD.csv`
- **Web3:** Alloy framework
- **Target:** Arbitrum (chain ID 42161)
- **Target DEX:** Uniswap V3 (WETH/USDC 0.05% fee pool)
- **Money arithmetic:** Use `FixedUsd`, not raw `f64` for financial values
- **Notifications:** Discord webhook via `vertexa-notify` crate (optional — configured in `[notify]` config section)
- **Persistence:** State saved to `vertexa_state.json` on graceful shutdown, restored on startup
- **Safety Gates:** Includes Minimum Trade Size Gate ($20 floor) and Staleness Protection re-sync

## Signals

| Signal | Purpose |
|--------|---------|
| `RsiSignal` | RSI 14-period, buy <30, sell >70 |
| `EmaCrossoverSignal` | EMA 9/21 crossover |
| `OrderBookSignal` | Bid/ask imbalance (moving toward tick-depth weighted) |
| `OnchainFlowSignal` | Whale transaction flow with **Cumulative Volume Delta (CVD)** and reputation weights |
| `VolatilityRegimeSignal` | **Adaptive** ATR-based regime classification (uses rolling 100-period percentiles) |

## Consensus Engine Flow

```
1. REGIME PRE-GATE (FIRST)
   ├── ATR calculation: true_range[i] = abs(prices[i] - prices[i-1])
   ├── Adaptive Percentile: Compare current ATR_pct to last 100 periods
   ├── atr_pct < p25 → Ranging → BLOCK TRADE
   ├── p25 ≤ atr_pct ≤ p75 → Trending → size_multiplier = 1.0
   └── atr_pct > p75 → Volatile → size_multiplier = 0.5

2. MINIMUM TRADE SIZE GATE
   ├── adjusted_usd = max_trade_usd * size_multiplier
   └── if adjusted_usd < config.min_trade_usd → BLOCK TRADE

3. Run all signals, collect votes

4. Majority vote logic

5. Confidence gate (min_confidence threshold)
```

## Profitability Gate Algorithm

```
Inputs:
  - trade.amount_usd
  - decision.avg_confidence
  - mev_assessment.estimated_mev_loss_usd
  - gas_estimate_usd = Predictive Gas estimate from eth_feeHistory

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

## Gas Estimation (Institutional-Grade)

Uses EIP-1559 `eth_feeHistory` for forward-looking modeling:

```
GasEstimator::estimate(tx):
  1. eth_feeHistory(10, latest, [50.0])
  2. Predicted Base Fee = last_base_fee * 1.125 (conservative bump)
  3. Median Priority Fee = 50th percentile of last 10 blocks
  4. gas_price = predicted_base + median_priority
  5. cost_usd = gas_units * gas_price * eth_price / 1e18
```

## Price & Liquidity from Events (Not Polling)

Real-time state via `eth_subscribe` on pool events:

```
Events: Swap(..., sqrtPriceX96, liquidity, tick), Mint(...), Burn(...)

1. Subscribe to pool address, filter by Swap/Mint/Burn topics
2. Swap: update price, current liquidity, and current tick
3. Mint/Burn: update local TickLiquidity BTreeMap (liquidity_net per tick)
4. Staleness Protection: Divergence > 0.1% between WS and RPC triggers re-sync from latest - 10 blocks
```

## Mempool Whale Decoder

Decodes Uniswap V3 and 1inch calldata to capture more whale flow:

```
Input: raw tx calldata bytes
1. Routing:
   ├── 0x414bf389 (exactInputSingle)
   ├── 0xdb3e2198 (exactOutputSingle)
   ├── 0xc04b8d59 (exactInput - multi-hop path parsing)
   ├── 0xf28c0440 (exactOutput - reversed path parsing)
   └── 0x12aa3caf (1inch v5 swap)
2. Direction: Buy (USDC→WETH) or Sell (WETH→USDC)
3. Weighting: Apply reputation multiplier from config per sender address
```

## Cumulative Volume Delta (CVD) & Absorption

Used in `OnchainFlowSignal`:

```
1. sum(buy_vol) - sum(sell_vol) in rolling window
2. Detect Absorption: High CVD (e.g. Ratio > 0.5) with price moving OPPOSITE
3. Absorption = Strong Reversal Signal (0.9 confidence)
```

## Executor Actor Flow (Aggressive Escalation)

```
Input (mpsc): TradeIntent { action, token_in, token_out, amount_in, min_out, max_gas_price }

Actor lifecycle:
  1. Simulate via eth_call (pre-flight check)
  2. Get initial p75 priority fee from feeHistory
  3. Attempt Loop (max 4 attempts):
     ├── max_priority = p75_priority * escalation_multiplier
     ├── max_fee = (base_fee * 1.1) + max_priority
     ├── Sign with same nonce, Send Raw Tx
     ├── tokio::select! loop:
     │    ├── Receipt → Success/Status
     │    └── Timeout (2s aggressive) → Bump escalation_multiplier (1.5x)
     └── Exhausted → send ExecutionResult::Failed
```

## Main Loop Order

```
1. Build MarketContext (from SharedState + TickLiquidity)
2. Run consensus → decision (includes adaptive regime pre-gate)
3. If Neutral → continue
4. Apply size_multiplier → adjusted_usd
5. Build TradeIntent with adjusted_usd
6. MEV assessment → mev_assessment
7. Predictive Gas estimate
8. PROFITABILITY CHECK (uses real gas + MEV estimate)
9. Risk.check()
10. Send TradeIntent to ExecutorActor via mpsc
11. Await ExecutionResult via oneshot
12. Log iteration to CSV (EventLogger)
13. Notify (Discord webhook on success/failure/abort)
14. Record trade for position tracking
```

Shutdown (SIGINT/SIGTERM): Persists state (daily_loss, circuit_breaker, position) to `vertexa_state.json` before exit. State is restored on next startup.

## Event Logging

Every loop iteration produces a CSV row in `data/events_YYYY-MM-DD.csv`:

```
timestamp, block, price, regime, action, confidence, size_mult,
executed, route, tx_hash, error, gas_usd, mev_usd, edge_usd, reward_to_cost
```

One file per day, auto-rotated at UTC midnight. Uses `BufWriter<tokio::fs::File>`.

## Backtesting

The `backtester` binary replays CSV event logs:

```
vertexa-backtester data/events_2026-05-13.csv

Reads CSV → replays through consensus engine or computes PnL from historical decisions
Outputs: PnL, win rate, Sharpe ratio, max drawdown
```

Use after every signal parameter change to validate against historical data.

## Build & Run

```bash
cargo build --release          # production build
cargo run --release            # run (reads config/default.toml)
VERTEXA_PAPER=true cargo run   # paper trade mode (no real txns)
cargo run --bin backtester -- data/events.csv  # run backtester
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
- No `unwrap()`, no `expect()`, no `todo!()`, no `unimplemented!()`, no `unreachable!()`
- No new dependencies unless justified
- Do not reformat, rename, or restructure existing code not explicitly listed
- Every new tracing log must include `target: "vertexa"`
- ExecutorActor owns the signer + nonce — main loop never signs transactions
- SharedState is read by main loop, written by ingestion tasks (Swap/Mint/Burn events, pool reader, mempool monitor)
- All mpsc channels must have bounded buffers with clear backpressure strategy (drop oldest, or block sender)

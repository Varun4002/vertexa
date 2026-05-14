use alloy::primitives::{Address, B256, U256, address};
use alloy::rpc::types::TransactionRequest;
use tokio::sync::oneshot;
use crate::Vote;
use std::time::Instant;
use std::fmt;

#[derive(Debug, Clone, Copy, PartialEq, PartialOrd)]
pub struct FixedUsd(pub f64);

impl FixedUsd {
    pub fn from_dollars(amount: f64) -> Self {
        Self(amount)
    }

    pub fn to_dollars(&self) -> f64 {
        self.0
    }
}

impl fmt::Display for FixedUsd {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "${:.2}", self.0)
    }
}

impl std::ops::Add for FixedUsd {
    type Output = Self;
    fn add(self, rhs: Self) -> Self::Output {
        Self(self.0 + rhs.0)
    }
}

impl std::ops::Sub for FixedUsd {
    type Output = Self;
    fn sub(self, rhs: Self) -> Self::Output {
        Self(self.0 - rhs.0)
    }
}

#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct MacroRegime {
    pub name: String,
    pub description: String,
    pub match_score: f64,
    pub size_multiplier_override: Option<f64>,
    pub confidence_modifier: f64,
    pub historical_period: Option<String>,
}

#[derive(Debug, Clone, Default)]
pub struct TickData {
    pub liquidity_net: i128,
    pub liquidity_gross: u128,
}

#[derive(Debug, Clone, Default)]
pub struct TickLiquidity {
    pub ticks: std::collections::BTreeMap<i32, TickData>,
    pub current_tick: i32,
    pub current_liquidity: u128,
}

#[derive(Debug, Clone)]
pub struct MarketContext {
    pub pair: String,
    pub pool_address: Address,
    pub prices: Vec<f64>,
    pub volumes: Vec<f64>,
    pub orderbook: OrderBook,
    pub tick_liquidity: Option<TickLiquidity>,
    pub recent_whale_txs: Vec<WhaleTx>,
    pub pool_liquidity: f64,
    pub current_price: f64,
    pub block_number: u64,
    pub timestamp: Instant,
    pub macro_regime: Option<MacroRegime>,
}

#[derive(Debug, Clone)]
pub struct OrderBook {
    pub bids: Vec<PriceLevel>,
    pub asks: Vec<PriceLevel>,
}

#[derive(Debug, Clone)]
pub struct PriceLevel {
    pub price: f64,
    pub size: f64,
}

#[derive(Debug, Clone)]
pub struct WhaleTx {
    pub hash: B256,
    pub from: Address,
    pub usd_value: f64,
    pub is_buy: bool,
    pub block: u64,
}

#[derive(Debug, Clone)]
pub struct PlannedTrade {
    pub action: Vote,
    pub token_in: Address,
    pub token_out: Address,
    pub amount_in: U256,
    pub expected_min_amount_out: U256,
    pub amount_usd: f64,
    pub max_slippage: f64,
    pub pool_fee: u32,
}

#[derive(Debug, Clone)]
pub struct PendingTx {
    pub hash: B256,
    pub from: Address,
    pub to: Option<Address>,
    pub value: U256,
    pub input: Vec<u8>,
    pub block_number: Option<u64>,
    pub direction: Option<Vote>,
}

pub type TradeIntent = (PlannedTrade, TransactionRequest, oneshot::Sender<ExecutionResult>, Option<u128>); // (trade, tx, resp, max_gas_price)

#[derive(Debug, Clone, Copy, PartialEq, serde::Serialize, serde::Deserialize)]
pub enum SimulationConfidence {
    High,
    Low,
}

#[derive(Debug, Clone)]
pub struct SimulationResult {
    pub amount_out: U256,
    pub gas_used: u64,
    pub confidence: SimulationConfidence,
}

#[derive(Debug, Clone)]
pub struct ExecutionResult {
    pub success: bool,
    pub tx_hash: Option<B256>,
    pub gas_used: Option<u64>,
    pub actual_amount_out: Option<U256>,
    pub error: Option<String>,
}

#[derive(Debug, Clone)]
pub struct TradeChunk {
    pub amount_in: U256,
    pub delay_blocks: u64,
}

#[derive(Debug, Clone, PartialEq)]
pub enum ExecutionRoute {
    PublicMempool,
    FlashbotsBundle,
    SplitExecution,
    Abort,
}

#[derive(Debug, Clone)]
pub struct MevThreatAssessment {
    pub risk_score: f64,
    pub sandwich_probability: f64,
    pub recommended_route: ExecutionRoute,
    pub estimated_mev_loss_usd: f64,
}

#[derive(Debug, Clone)]
pub struct Decision {
    pub action: Vote,
    pub agreeing_signals: Vec<String>,
    pub avg_confidence: f64,
    pub dissenting_signals: Vec<String>,
    pub size_multiplier: f64,
    pub blocked_by: Option<&'static str>,
}

#[derive(Debug, Clone)]
pub struct PriceSeries {
    pub closes: Vec<f64>,
    pub volumes: Vec<f64>,
    pub max_len: usize,
}

impl PriceSeries {
    pub fn new(max_len: usize) -> Self {
        Self {
            closes: Vec::with_capacity(max_len),
            volumes: Vec::with_capacity(max_len),
            max_len,
        }
    }

    pub fn push(&mut self, close: f64, volume: f64) {
        if self.closes.len() >= self.max_len {
            self.closes.remove(0);
        }
        if self.volumes.len() >= self.max_len {
            self.volumes.remove(0);
        }
        self.closes.push(close);
        self.volumes.push(volume);
    }
}

pub const WETH_ADDRESS: Address = address!("82aF49447D8a07e3bd95BD0d56f35241523fBab1");
pub const USDC_ADDRESS: Address = address!("af88d065e77c8cC2239327C5EDb3A432268e5831");
pub const UNISWAP_V3_ROUTER: Address = address!("68b3465833fb72A70ecDF485E0e4C7bD8665Fc45");
pub const UNISWAP_V3_QUOTER: Address = address!("61fFE014bA17989E743c5F6cB21bF9697530B21e");
pub const ARBITRUM_CHAIN_ID: u64 = 42161;

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_fixed_usd_from_dollars() {
        let amount = FixedUsd::from_dollars(100.50);
        assert!((amount.to_dollars() - 100.50).abs() < 1e-10);
    }

    #[test]
    fn test_fixed_usd_display() {
        let amount = FixedUsd::from_dollars(1234.56);
        assert_eq!(format!("{amount}"), "$1234.56");
    }

    #[test]
    fn test_fixed_usd_add() {
        let a = FixedUsd::from_dollars(100.0);
        let b = FixedUsd::from_dollars(50.0);
        let c = a + b;
        assert!((c.to_dollars() - 150.0).abs() < 1e-10);
    }

    #[test]
    fn test_fixed_usd_sub() {
        let a = FixedUsd::from_dollars(100.0);
        let b = FixedUsd::from_dollars(30.0);
        let c = a - b;
        assert!((c.to_dollars() - 70.0).abs() < 1e-10);
    }

    #[test]
    fn test_fixed_usd_negative_not_allowed() {
        let amount = FixedUsd::from_dollars(-50.0);
        assert!(amount.to_dollars() < 0.0);
    }

    #[test]
    fn test_price_series_push_and_capacity() {
        let mut series = PriceSeries::new(5);
        for i in 0..10 {
            series.push(i as f64, (i * 2) as f64);
        }
        assert_eq!(series.closes.len(), 5);
        assert_eq!(series.closes[0], 5.0);
        assert_eq!(series.closes[4], 9.0);
        assert_eq!(series.volumes.len(), 5);
    }

    #[test]
    fn test_price_series_empty() {
        let series = PriceSeries::new(100);
        assert!(series.closes.is_empty());
        assert!(series.volumes.is_empty());
    }
}

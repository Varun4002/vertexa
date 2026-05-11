use alloy::primitives::{Address, B256, U256, address};
use crate::Vote;
use std::time::Instant;

#[derive(Debug, Clone)]
pub struct MarketContext {
    pub pair: String,
    pub pool_address: Address,
    pub prices: Vec<f64>,
    pub volumes: Vec<f64>,
    pub orderbook: OrderBook,
    pub recent_whale_txs: Vec<WhaleTx>,
    pub pool_liquidity: f64,
    pub current_price: f64,
    pub block_number: u64,
    pub timestamp: Instant,
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

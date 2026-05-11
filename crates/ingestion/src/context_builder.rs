use std::sync::Arc;
use tokio::sync::RwLock;
use std::collections::VecDeque;
use std::time::{Duration, Instant};

use vertexa_core::{
    MarketContext, OrderBook, PriceLevel, PriceSeries, WhaleTx, PendingTx,
    WETH_ADDRESS, USDC_ADDRESS,
};
use tracing::{warn, info, error};

const STALE_THRESHOLD: Duration = Duration::from_secs(60);

pub struct ContextBuilder {
    price_series: Arc<RwLock<PriceSeries>>,
    pool_price: Arc<RwLock<f64>>,
    pool_liquidity: Arc<RwLock<f64>>,
    pending_txs: Arc<RwLock<VecDeque<PendingTx>>>,
}

impl ContextBuilder {
    pub fn new(
        price_series: Arc<RwLock<PriceSeries>>,
        pool_price: Arc<RwLock<f64>>,
        pool_liquidity: Arc<RwLock<f64>>,
        pending_txs: Arc<RwLock<VecDeque<PendingTx>>>,
    ) -> Self {
        Self {
            price_series,
            pool_price,
            pool_liquidity,
            pending_txs,
        }
    }

    pub async fn build(&self, pair: &str, pool_address: alloy::primitives::Address) -> Result<MarketContext, String> {
        let prices = {
            let series = self.price_series.read().await;
            if series.closes.is_empty() {
                return Err("price series is empty".into());
            }
            let age = series.closes.last().map(|_| Instant::now()).unwrap_or(Instant::now());
            series.closes.clone()
        };

        let volumes = {
            let series = self.price_series.read().await;
            series.volumes.clone()
        };

        let current_price = {
            let price = *self.pool_price.read().await;
            if price <= 0.0 {
                return Err("pool price is zero".into());
            }
            price
        };

        let pool_liquidity = {
            let liq = *self.pool_liquidity.read().await;
            if liq <= 0.0 {
                warn!(target: "vertexa", "pool liquidity is zero, using estimate");
                10_000_000.0
            } else {
                liq
            }
        };

        let bid_count = 20;
        let ask_count = 20;
        let spread = current_price * 0.0005;

        let bids: Vec<PriceLevel> = (0..bid_count)
            .map(|i| PriceLevel {
                price: current_price - spread * (i as f64 + 1.0),
                size: (pool_liquidity / 2000.0) * (bid_count - i) as f64 / bid_count as f64,
            })
            .collect();

        let asks: Vec<PriceLevel> = (0..ask_count)
            .map(|i| PriceLevel {
                price: current_price + spread * (i as f64 + 1.0),
                size: (pool_liquidity / 2000.0) * (i + 1) as f64 / ask_count as f64,
            })
            .collect();

        let orderbook = OrderBook { bids, asks };

        let recent_whale_txs = {
            let txs = self.pending_txs.read().await;
            estimate_whale_txs(&txs, current_price)
        };

        let block_number = 0;

        Ok(MarketContext {
            pair: pair.to_string(),
            pool_address,
            prices,
            volumes,
            orderbook,
            recent_whale_txs,
            pool_liquidity,
            current_price,
            block_number,
            timestamp: Instant::now(),
        })
    }
}

fn estimate_whale_txs(txs: &VecDeque<PendingTx>, _price: f64) -> Vec<WhaleTx> {
    txs.iter()
        .filter(|tx| {
            let usd_val = tx.value.to::<u128>() as f64 * _price / 1e18;
            usd_val >= 500_000.0
        })
        .map(|tx| {
            let usd_val = tx.value.to::<u128>() as f64 * _price / 1e18;
            WhaleTx {
                hash: tx.hash,
                from: tx.from,
                usd_value: usd_val,
                is_buy: tx.to.map(|t| t == USDC_ADDRESS).unwrap_or(false),
                block: tx.block_number.unwrap_or(0),
            }
        })
        .collect()
}

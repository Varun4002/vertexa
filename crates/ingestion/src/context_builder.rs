use std::sync::Arc;
use tokio::sync::RwLock;
use std::collections::VecDeque;
use std::time::Instant;

use vertexa_core::{
    MarketContext, PriceSeries, WhaleTx, PendingTx,
    Vote, MacroRegime, TickLiquidity,
};
use tracing::warn;

use crate::orderbook;

pub struct ContextBuilder {
    price_series: Arc<RwLock<PriceSeries>>,
    pool_price: Arc<RwLock<f64>>,
    pool_liquidity: Arc<RwLock<f64>>,
    tick_liquidity: Arc<RwLock<TickLiquidity>>,
    pending_txs: Arc<RwLock<VecDeque<PendingTx>>>,
    block_number: Arc<RwLock<u64>>,
    macro_regime: Arc<RwLock<Option<MacroRegime>>>,
}

impl ContextBuilder {
    pub fn new(
        price_series: Arc<RwLock<PriceSeries>>,
        pool_price: Arc<RwLock<f64>>,
        pool_liquidity: Arc<RwLock<f64>>,
        tick_liquidity: Arc<RwLock<TickLiquidity>>,
        pending_txs: Arc<RwLock<VecDeque<PendingTx>>>,
        block_number: Arc<RwLock<u64>>,
        macro_regime: Arc<RwLock<Option<MacroRegime>>>,
    ) -> Self {
        Self {
            price_series,
            pool_price,
            pool_liquidity,
            tick_liquidity,
            pending_txs,
            block_number,
            macro_regime,
        }
    }

    pub async fn build(&self, pair: &str, pool_address: alloy::primitives::Address) -> Result<MarketContext, String> {
        let prices = {
            let series = self.price_series.read().await;
            if series.closes.is_empty() {
                return Err("price series is empty".into());
            }
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

        let tick_liquidity = self.tick_liquidity.read().await.clone();
        let orderbook = orderbook::build_orderbook(tick_liquidity.current_tick, pool_liquidity, current_price);

        let recent_whale_txs = {
            let txs = self.pending_txs.read().await;
            estimate_whale_txs(&txs, current_price)
        };

        let block_number = *self.block_number.read().await;

        let macro_regime = self.macro_regime.read().await.clone();

        Ok(MarketContext {
            pair: pair.to_string(),
            pool_address,
            prices,
            volumes,
            orderbook,
            tick_liquidity: Some(tick_liquidity),
            recent_whale_txs,
            pool_liquidity,
            current_price,
            block_number,
            timestamp: Instant::now(),
            macro_regime,
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
                is_buy: matches!(tx.direction, Some(Vote::Buy)),
                block: tx.block_number.unwrap_or(0),
            }
        })
        .collect()
}

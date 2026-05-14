use async_trait::async_trait;
use vertexa_core::{Signal, SignalResult, Vote, MarketContext};

pub struct OrderBookSignal;

impl OrderBookSignal {
    pub fn new() -> Self {
        Self
    }
}

impl Default for OrderBookSignal {
    fn default() -> Self {
        Self::new()
    }
}

#[async_trait]
impl Signal for OrderBookSignal {
    fn name(&self) -> &'static str {
        "OrderBook"
    }

    async fn evaluate(&self, ctx: &MarketContext) -> SignalResult {
        let total_bids: f64 = ctx.orderbook.bids.iter().map(|l| l.size).sum();
        let total_asks: f64 = ctx.orderbook.asks.iter().map(|l| l.size).sum();
        let total = total_bids + total_asks;

        if total == 0.0 {
            return SignalResult::neutral(self.name());
        }

        let imbalance = (total_bids - total_asks) / total;

        if imbalance > 0.15 {
            SignalResult::new(self.name(), Vote::Buy, imbalance.min(1.0))
        } else if imbalance < -0.15 {
            SignalResult::new(self.name(), Vote::Sell, (-imbalance).min(1.0))
        } else {
            SignalResult::neutral(self.name())
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::Instant;
    use vertexa_core::{MarketContext, OrderBook, PriceLevel};

    fn make_context(bid_size: f64, ask_size: f64) -> MarketContext {
        MarketContext {
            pair: "ETH/USDC".into(),
            pool_address: Default::default(),
            prices: vec![3000.0; 30],
            volumes: vec![],
            orderbook: OrderBook {
                bids: vec![PriceLevel { price: 2990.0, size: bid_size }; 10],
                asks: vec![PriceLevel { price: 3010.0, size: ask_size }; 10],
            },
            tick_liquidity: None,
            recent_whale_txs: vec![],
            pool_liquidity: 10_000_000.0,
            current_price: 3000.0,
            block_number: 0,
            timestamp: Instant::now(),
            macro_regime: None,
        }
    }

    #[tokio::test]
    async fn test_buy_imbalance() {
        let ctx = make_context(1000.0, 100.0);
        let signal = OrderBookSignal::new();
        let result = signal.evaluate(&ctx).await;
        assert_eq!(result.vote, Vote::Buy);
    }

    #[tokio::test]
    async fn test_sell_imbalance() {
        let ctx = make_context(100.0, 1000.0);
        let signal = OrderBookSignal::new();
        let result = signal.evaluate(&ctx).await;
        assert_eq!(result.vote, Vote::Sell);
    }

    #[tokio::test]
    async fn test_neutral_balanced() {
        let ctx = make_context(500.0, 500.0);
        let signal = OrderBookSignal::new();
        let result = signal.evaluate(&ctx).await;
        assert_eq!(result.vote, Vote::Neutral);
    }
}

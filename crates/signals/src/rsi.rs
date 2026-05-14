use async_trait::async_trait;
use vertexa_core::{Signal, SignalResult, Vote, MarketContext};

pub struct RsiSignal;

impl RsiSignal {
    pub fn new() -> Self {
        Self
    }
}

impl Default for RsiSignal {
    fn default() -> Self {
        Self::new()
    }
}

impl RsiSignal {

    fn calculate_rsi(prices: &[f64], period: usize) -> Option<f64> {
        if prices.len() < period + 1 {
            return None;
        }

        let mut gains = 0.0;
        let mut losses = 0.0;

        for i in prices.len() - period..prices.len() {
            let diff = prices[i] - prices[i - 1];
            if diff > 0.0 {
                gains += diff;
            } else {
                losses -= diff;
            }
        }

        let mut avg_gain = gains / period as f64;
        let mut avg_loss = losses / period as f64;

        for i in prices.len() - period..prices.len() - 1 {
            let diff = prices[i + 1] - prices[i];
            avg_gain = (avg_gain * (period as f64 - 1.0) + diff.max(0.0)) / period as f64;
            avg_loss = (avg_loss * (period as f64 - 1.0) + (-diff).max(0.0)) / period as f64;
        }

        if avg_loss == 0.0 {
            return Some(100.0);
        }

        let rs = avg_gain / avg_loss;
        Some(100.0 - (100.0 / (1.0 + rs)))
    }
}

#[async_trait]
impl Signal for RsiSignal {
    fn name(&self) -> &'static str {
        "RSI"
    }

    async fn evaluate(&self, ctx: &MarketContext) -> SignalResult {
        const PERIOD: usize = 14;

        if ctx.prices.len() < PERIOD + 1 {
            return SignalResult::neutral(self.name());
        }

        match Self::calculate_rsi(&ctx.prices, PERIOD) {
            Some(rsi) => {
                if rsi < 30.0 {
                    let confidence = ((30.0 - rsi) / 30.0).clamp(0.0, 1.0);
                    SignalResult::new(self.name(), Vote::Buy, confidence)
                } else if rsi > 70.0 {
                    let confidence = ((rsi - 70.0) / 30.0).clamp(0.0, 1.0);
                    SignalResult::new(self.name(), Vote::Sell, confidence)
                } else {
                    SignalResult::neutral(self.name())
                }
            }
            None => SignalResult::neutral(self.name()),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::Instant;
    use vertexa_core::{MarketContext, OrderBook};

    fn make_context(prices: Vec<f64>, current_price: f64) -> MarketContext {
        MarketContext {
            pair: "ETH/USDC".into(),
            pool_address: Default::default(),
            prices,
            volumes: vec![],
            orderbook: OrderBook { bids: vec![], asks: vec![] },
            tick_liquidity: None,
            recent_whale_txs: vec![],
            pool_liquidity: 10_000_000.0,
            current_price,
            block_number: 0,
            timestamp: Instant::now(),
            macro_regime: None,
        }
    }

    #[tokio::test]
    async fn test_rsi_buy_signal() {
        let prices: Vec<f64> = (0..30).map(|i| 100.0 - (i as f64 * 3.0)).collect();
        let ctx = make_context(prices, 70.0);
        let signal = RsiSignal::new();
        let result = signal.evaluate(&ctx).await;
        assert_eq!(result.vote, Vote::Buy);
        assert!(result.confidence > 0.0);
    }

    #[tokio::test]
    async fn test_rsi_sell_signal() {
        let prices: Vec<f64> = (0..30).map(|i| 100.0 + (i as f64 * 3.0)).collect();
        let ctx = make_context(prices, 180.0);
        let signal = RsiSignal::new();
        let result = signal.evaluate(&ctx).await;
        assert_eq!(result.vote, Vote::Sell);
        assert!(result.confidence > 0.0);
    }

    #[tokio::test]
    async fn test_rsi_neutral_insufficient_data() {
        let prices = vec![100.0, 101.0];
        let ctx = make_context(prices, 101.0);
        let signal = RsiSignal::new();
        let result = signal.evaluate(&ctx).await;
        assert_eq!(result.vote, Vote::Neutral);
    }

    #[test]
    fn test_rsi_math_wilder_smoothing() {
        let prices = vec![
            100.0, 102.0, 101.0, 103.0, 105.0, 104.0, 106.0, 108.0,
            107.0, 109.0, 110.0, 108.0, 107.0, 109.0, 111.0, 112.0,
        ];
        let rsi = RsiSignal::calculate_rsi(&prices, 14);
        assert!(rsi.is_some());
        let rsi_val = rsi.unwrap();
        assert!(rsi_val >= 0.0 && rsi_val <= 100.0);
    }
}

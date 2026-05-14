use async_trait::async_trait;
use vertexa_core::{Signal, SignalResult, Vote, MarketContext};

pub struct EmaCrossoverSignal;

impl EmaCrossoverSignal {
    pub fn new() -> Self {
        Self
    }
}

impl Default for EmaCrossoverSignal {
    fn default() -> Self {
        Self::new()
    }
}

impl EmaCrossoverSignal {

    fn calculate_ema(prices: &[f64], period: usize) -> Vec<f64> {
        if prices.len() < period {
            return vec![];
        }

        let k = 2.0 / (period as f64 + 1.0);
        let mut ema = vec![0.0_f64; prices.len()];
        let mut sum = 0.0;

        for p in prices.iter().take(period) {
            sum += p;
        }
        ema[period - 1] = sum / period as f64;

        for i in period..prices.len() {
            ema[i] = prices[i] * k + ema[i - 1] * (1.0 - k);
        }

        ema
    }
}

#[async_trait]
impl Signal for EmaCrossoverSignal {
    fn name(&self) -> &'static str {
        "EMA"
    }

    async fn evaluate(&self, ctx: &MarketContext) -> SignalResult {
        const FAST_PERIOD: usize = 9;
        const SLOW_PERIOD: usize = 21;

        if ctx.prices.len() < SLOW_PERIOD {
            return SignalResult::neutral(self.name());
        }

        let ema_fast = Self::calculate_ema(&ctx.prices, FAST_PERIOD);
        let ema_slow = Self::calculate_ema(&ctx.prices, SLOW_PERIOD);

        let fast_val = *ema_fast.last().unwrap_or(&0.0);
        let slow_val = *ema_slow.last().unwrap_or(&0.0);

        if slow_val == 0.0 {
            return SignalResult::neutral(self.name());
        }

        let spread = (fast_val - slow_val) / slow_val;

        if spread > 0.005 {
            let confidence = (spread / 0.05).min(1.0);
            SignalResult::new(self.name(), Vote::Buy, confidence)
        } else if spread < -0.005 {
            let confidence = (-spread / 0.05).min(1.0);
            SignalResult::new(self.name(), Vote::Sell, confidence)
        } else {
            SignalResult::neutral(self.name())
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::Instant;
    use vertexa_core::{MarketContext, OrderBook};

    fn make_context(prices: Vec<f64>) -> MarketContext {
        let current_price = *prices.last().unwrap_or(&0.0);
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
    async fn test_ema_buy_crossover() {
        let prices: Vec<f64> = (0..30)
            .map(|i| 100.0 + (i as f64 * 2.0))
            .collect();
        let ctx = make_context(prices);
        let signal = EmaCrossoverSignal::new();
        let result = signal.evaluate(&ctx).await;
        assert_eq!(result.vote, Vote::Buy);
    }

    #[tokio::test]
    async fn test_ema_neutral_insufficient_data() {
        let prices = vec![100.0; 10];
        let ctx = make_context(prices);
        let signal = EmaCrossoverSignal::new();
        let result = signal.evaluate(&ctx).await;
        assert_eq!(result.vote, Vote::Neutral);
    }

    #[test]
    fn test_ema_calculation() {
        let prices = vec![100.0; 30];
        let ema = EmaCrossoverSignal::calculate_ema(&prices, 9);
        assert_eq!(ema.len(), 30);
        assert!((ema[29] - 100.0).abs() < 1.0);
    }
}

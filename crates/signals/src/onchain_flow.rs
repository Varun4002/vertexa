use std::collections::HashMap;
use alloy::primitives::Address;
use async_trait::async_trait;
use vertexa_core::{Signal, SignalResult, Vote, MarketContext};

pub struct OnchainFlowSignal {
    reputation_weights: HashMap<Address, f64>,
}

impl OnchainFlowSignal {
    pub fn new(weights: HashMap<Address, f64>) -> Self {
        Self {
            reputation_weights: weights,
        }
    }
}

impl Default for OnchainFlowSignal {
    fn default() -> Self {
        Self::new(HashMap::new())
    }
}

#[async_trait]
impl Signal for OnchainFlowSignal {
    fn name(&self) -> &'static str {
        "OnchainFlow"
    }

    async fn evaluate(&self, ctx: &MarketContext) -> SignalResult {
        let threshold = 500_000.0;

        let buy_vol: f64 = ctx.recent_whale_txs
            .iter()
            .filter(|tx| tx.is_buy && tx.usd_value >= threshold)
            .map(|tx| {
                let weight = self.reputation_weights.get(&tx.from).copied().unwrap_or(1.0);
                tx.usd_value * weight
            })
            .sum();

        let sell_vol: f64 = ctx.recent_whale_txs
            .iter()
            .filter(|tx| !tx.is_buy && tx.usd_value >= threshold)
            .map(|tx| {
                let weight = self.reputation_weights.get(&tx.from).copied().unwrap_or(1.0);
                tx.usd_value * weight
            })
            .sum();

        let cvd = buy_vol - sell_vol;
        let total_vol = buy_vol + sell_vol;

        if total_vol == 0.0 {
            return SignalResult::neutral(self.name());
        }

        let ratio = cvd / total_vol;

        // Absorption detection: CVD moving strongly but price stays flat or moves opposite
        // Simplified: if prices.len() >= 2, check last move
        let price_move = if ctx.prices.len() >= 2 {
            ctx.current_price - ctx.prices[ctx.prices.len() - 2]
        } else {
            0.0
        };

        // If CVD is very high (Buy) but price moved Down -> Strong reversal (Buy signal)
        if ratio > 0.5 && price_move < 0.0 {
            return SignalResult::new(self.name(), Vote::Buy, 0.9);
        }
        // If CVD is very low (Sell) but price moved Up -> Strong reversal (Sell signal)
        if ratio < -0.5 && price_move > 0.0 {
            return SignalResult::new(self.name(), Vote::Sell, 0.9);
        }

        if ratio > 0.20 {
            SignalResult::new(self.name(), Vote::Buy, ratio.abs().min(1.0))
        } else if ratio < -0.20 {
            SignalResult::new(self.name(), Vote::Sell, ratio.abs().min(1.0))
        } else {
            SignalResult::neutral(self.name())
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::Instant;
    use vertexa_core::{MarketContext, OrderBook, WhaleTx};
    use alloy::primitives::{address, B256};

    fn make_context(txs: Vec<WhaleTx>) -> MarketContext {
        MarketContext {
            pair: "ETH/USDC".into(),
            pool_address: Default::default(),
            prices: vec![3000.0; 30],
            volumes: vec![],
            orderbook: OrderBook { bids: vec![], asks: vec![] },
            tick_liquidity: None,
            recent_whale_txs: txs,
            pool_liquidity: 10_000_000.0,
            current_price: 3000.0,
            block_number: 0,
            timestamp: Instant::now(),
            macro_regime: None,
        }
    }

    fn make_whale(value: f64, is_buy: bool) -> WhaleTx {
        WhaleTx {
            hash: B256::default(),
            from: address!("0000000000000000000000000000000000000001"),
            usd_value: value,
            is_buy,
            block: 0,
        }
    }

    #[tokio::test]
    async fn test_buy_pressure() {
        let txs = vec![
            make_whale(1_000_000.0, true),
            make_whale(100_000.0, true),
        ];
        let ctx = make_context(txs);
        let signal = OnchainFlowSignal::new(HashMap::new());
        let result = signal.evaluate(&ctx).await;
        assert_eq!(result.vote, Vote::Buy);
    }

    #[tokio::test]
    async fn test_sell_pressure() {
        let txs = vec![
            make_whale(1_000_000.0, false),
            make_whale(500_000.0, false),
        ];
        let ctx = make_context(txs);
        let signal = OnchainFlowSignal::new(HashMap::new());
        let result = signal.evaluate(&ctx).await;
        assert_eq!(result.vote, Vote::Sell);
    }

    #[tokio::test]
    async fn test_no_whale_txs() {
        let ctx = make_context(vec![]);
        let signal = OnchainFlowSignal::new(HashMap::new());
        let result = signal.evaluate(&ctx).await;
        assert_eq!(result.vote, Vote::Neutral);
    }

    #[tokio::test]
    async fn test_reputation_weighting() {
        let addr = address!("0000000000000000000000000000000000000001");
        let txs = vec![
            make_whale(600_000.0, false), // 600k sell
        ];
        let ctx = make_context(txs);
        
        // With weight 2.0, 600k becomes 1.2M
        let mut weights = HashMap::new();
        weights.insert(addr, 2.0);
        
        let signal = OnchainFlowSignal::new(weights);
        let result = signal.evaluate(&ctx).await;
        assert_eq!(result.vote, Vote::Sell);
        assert!(result.confidence > 0.9);
    }
}

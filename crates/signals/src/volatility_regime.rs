use async_trait::async_trait;
use vertexa_core::{Signal, SignalResult, Vote, MarketContext};

#[derive(Debug, Clone, PartialEq)]
pub enum Regime {
    Ranging,
    Trending,
    Volatile,
}

pub struct VolatilityRegimeSignal {
    pub atr_period: usize,
}

impl VolatilityRegimeSignal {
    pub fn new() -> Self {
        Self { atr_period: 14 }
    }

    pub fn classify(prices: &[f64], atr_period: usize) -> Option<Regime> {
        if prices.len() < atr_period + 1 {
            return None;
        }

        let start_idx = prices.len() - atr_period;
        let mut total_tr = 0.0;

        for i in start_idx..prices.len() {
            let tr = (prices[i] - prices[i - 1]).abs();
            total_tr += tr;
        }

        let atr = total_tr / atr_period as f64;
        let current_price = prices[prices.len() - 1];
        let atr_pct = atr / current_price;

        let regime = if atr_pct < 0.008 {
            Regime::Ranging
        } else if atr_pct <= 0.025 {
            Regime::Trending
        } else {
            Regime::Volatile
        };

        Some(regime)
    }
}

impl Default for VolatilityRegimeSignal {
    fn default() -> Self {
        Self::new()
    }
}

#[async_trait]
impl Signal for VolatilityRegimeSignal {
    fn name(&self) -> &'static str {
        "VolatilityRegime"
    }

    async fn evaluate(&self, ctx: &MarketContext) -> SignalResult {
        match Self::classify(&ctx.prices, self.atr_period) {
            Some(regime) => match regime {
                Regime::Ranging => SignalResult::new(self.name(), Vote::Neutral, 0.0),
                Regime::Trending => SignalResult::new(self.name(), Vote::Neutral, 1.0),
                Regime::Volatile => SignalResult::new(self.name(), Vote::Neutral, 0.5),
            },
            None => SignalResult::neutral(self.name()),
        }
    }
}

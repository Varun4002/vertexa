use std::collections::VecDeque;
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
    pub history: std::sync::Mutex<VecDeque<f64>>,
}

impl VolatilityRegimeSignal {
    pub fn new() -> Self {
        Self {
            atr_period: 14,
            history: std::sync::Mutex::new(VecDeque::with_capacity(200)),
        }
    }

    pub fn classify(&self, prices: &[f64]) -> Option<Regime> {
        if prices.len() < self.atr_period + 1 {
            return None;
        }

        let start_idx = prices.len() - self.atr_period;
        let mut total_tr = 0.0;

        for i in start_idx..prices.len() {
            let tr = (prices[i] - prices[i - 1]).abs();
            total_tr += tr;
        }

        let atr = total_tr / self.atr_period as f64;
        let current_price = prices[prices.len() - 1];
        let atr_pct = atr / current_price;

        let mut history = self.history.lock().unwrap();
        if history.len() >= 100 {
            history.pop_front();
        }
        history.push_back(atr_pct);

        if history.len() < 50 {
            // Not enough history for adaptive thresholds, use defaults
            return Some(if atr_pct < 0.008 {
                Regime::Ranging
            } else if atr_pct <= 0.025 {
                Regime::Trending
            } else {
                Regime::Volatile
            });
        }

        let mut sorted = history.iter().copied().collect::<Vec<f64>>();
        sorted.sort_unstable_by(|a, b| a.partial_cmp(b).unwrap());

        let p25 = sorted[sorted.len() / 4];
        let p75 = sorted[3 * sorted.len() / 4];

        let regime = if atr_pct < p25 {
            Regime::Ranging
        } else if atr_pct <= p75 {
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
        match self.classify(&ctx.prices) {
            Some(regime) => match regime {
                Regime::Ranging => SignalResult::new(self.name(), Vote::Neutral, 0.0),
                Regime::Trending => SignalResult::new(self.name(), Vote::Neutral, 1.0),
                Regime::Volatile => SignalResult::new(self.name(), Vote::Neutral, 0.5),
            },
            None => SignalResult::neutral(self.name()),
        }
    }
}

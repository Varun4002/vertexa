use crate::MarketContext;
use async_trait::async_trait;
use std::time::Instant;

#[derive(Debug, Clone, PartialEq)]
pub enum Vote {
    Buy,
    Sell,
    Neutral,
}

impl Vote {
    pub fn is_directional(&self) -> bool {
        matches!(self, Vote::Buy | Vote::Sell)
    }
}

#[derive(Debug, Clone)]
pub struct SignalResult {
    pub name: &'static str,
    pub vote: Vote,
    pub confidence: f64,
    pub timestamp: Instant,
}

impl SignalResult {
    pub fn new(name: &'static str, vote: Vote, confidence: f64) -> Self {
        Self {
            name,
            vote,
            confidence: confidence.clamp(0.0, 1.0),
            timestamp: Instant::now(),
        }
    }

    pub fn neutral(name: &'static str) -> Self {
        Self::new(name, Vote::Neutral, 0.0)
    }
}

#[async_trait]
pub trait Signal: Send + Sync {
    fn name(&self) -> &'static str;
    async fn evaluate(&self, ctx: &MarketContext) -> SignalResult;
}

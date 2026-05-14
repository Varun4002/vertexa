use futures::future::join_all;
use tracing::info;

use vertexa_core::{Decision, MarketContext, Signal, Vote, SignalResult};
use vertexa_signals::{VolatilityRegimeSignal, Regime};

pub struct ConsensusEngine {
    signals: Vec<Box<dyn Signal>>,
    required_votes: usize,
    min_confidence: f64,
    pub regime_signal: VolatilityRegimeSignal,
}

impl ConsensusEngine {
    pub fn new(
        signals: Vec<Box<dyn Signal>>,
        required_votes: usize,
        min_confidence: f64,
    ) -> Self {
        Self {
            signals,
            required_votes,
            min_confidence,
            regime_signal: VolatilityRegimeSignal::new(),
        }
    }

    pub async fn evaluate(&self, ctx: &MarketContext) -> Decision {
        let regime = match self.regime_signal.classify(&ctx.prices) {
            Some(r) => r,
            None => Regime::Ranging,
        };

        let mut size_multiplier = match regime {
            Regime::Trending => 1.0,
            Regime::Volatile => 0.5,
            Regime::Ranging => 0.0,
        };

        let mut confidence_modifier = 1.0;

        if let Some(macro_regime) = &ctx.macro_regime {
            info!(
                target: "vertexa",
                macro_regime = macro_regime.name,
                match_score = macro_regime.match_score,
                "Applying macro regime modifiers"
            );
            if let Some(m_size) = macro_regime.size_multiplier_override {
                size_multiplier = m_size;
            }
            confidence_modifier = macro_regime.confidence_modifier;
        }

        if size_multiplier <= 0.0 {
            info!(
                target: "vertexa",
                regime = ?regime,
                macro = ?ctx.macro_regime.as_ref().map(|m| &m.name),
                "Pre-gate blocked — market ranging or macro-blocked"
            );
            return Decision {
                action: Vote::Neutral,
                agreeing_signals: vec![],
                avg_confidence: 0.0,
                dissenting_signals: vec![],
                blocked_by: Some("RegimeGate"),
                size_multiplier: 0.0,
            };
        }

        let futures = self.signals.iter().map(|s| s.evaluate(ctx));
        let results = join_all(futures).await;

        for result in &results {
            info!(
                target: "vertexa",
                signal = result.name,
                vote   = ?result.vote,
                conf   = result.confidence,
                "signal evaluated"
            );
        }

        let buys: Vec<&SignalResult> = results.iter().filter(|r| r.vote == Vote::Buy).collect();
        let sells: Vec<&SignalResult> = results.iter().filter(|r| r.vote == Vote::Sell).collect();
        let neutrals: Vec<&SignalResult> = results.iter().filter(|r| r.vote == Vote::Neutral).collect();

        let (action, agreeing): (Vote, Vec<&SignalResult>) = if buys.len() >= self.required_votes {
            (Vote::Buy, buys)
        } else if sells.len() >= self.required_votes {
            (Vote::Sell, sells)
        } else if neutrals.len() >= self.required_votes {
            (Vote::Neutral, neutrals)
        } else if buys.len() == sells.len() && !buys.is_empty() {
            let buy_avg_conf: f64 = buys.iter().map(|r| r.confidence).sum::<f64>() / buys.len() as f64;
            let sell_avg_conf: f64 = sells.iter().map(|r| r.confidence).sum::<f64>() / sells.len() as f64;

            if (buy_avg_conf - sell_avg_conf).abs() < 0.001 {
                (Vote::Neutral, vec![])
            } else if buy_avg_conf > sell_avg_conf {
                (Vote::Buy, buys)
            } else {
                (Vote::Sell, sells)
            }
        } else {
            if buys.len() > sells.len() {
                (Vote::Buy, buys)
            } else if sells.len() > buys.len() {
                (Vote::Sell, sells)
            } else {
                (Vote::Neutral, vec![])
            }
        };

        let agreeing_signals: Vec<String> = agreeing.iter().map(|r| r.name.to_string()).collect();
        let avg_confidence = if agreeing.is_empty() {
            0.0
        } else {
            (agreeing.iter().map(|r| r.confidence).sum::<f64>() / agreeing.len() as f64) * confidence_modifier
        };

        let dissenting_signals: Vec<String> = results
            .iter()
            .filter(|r| r.vote != action)
            .map(|r| r.name.to_string())
            .collect();

        let decision = Decision {
            action: action.clone(),
            avg_confidence,
            agreeing_signals: agreeing_signals.clone(),
            dissenting_signals: dissenting_signals.clone(),
            size_multiplier,
            blocked_by: None,
        };

        if action.is_directional() && avg_confidence < self.min_confidence {
            info!(
                target: "vertexa",
                action = ?action,
                avg_conf = avg_confidence,
                min_conf = self.min_confidence,
                "confidence gate blocked — below minimum threshold"
            );
            return Decision {
                action: Vote::Neutral,
                agreeing_signals: vec![],
                avg_confidence: 0.0,
                dissenting_signals: agreeing_signals,
                size_multiplier: 0.0,
                blocked_by: Some("ConfidenceGate"),
            };
        }

        info!(
            target: "vertexa",
            action    = ?decision.action,
            signals   = ?decision.agreeing_signals,
            avg_conf  = decision.avg_confidence,
            dissenting = ?decision.dissenting_signals,
            "consensus reached"
        );

        decision
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::Instant;
    use vertexa_core::{MarketContext, OrderBook, Vote};

    struct TestSignal {
        name: &'static str,
        vote: Vote,
        confidence: f64,
    }

    #[async_trait::async_trait]
    impl Signal for TestSignal {
        fn name(&self) -> &'static str { self.name }
        async fn evaluate(&self, _ctx: &MarketContext) -> SignalResult {
            SignalResult::new(self.name, self.vote.clone(), self.confidence)
        }
    }

    fn make_ctx() -> MarketContext {
        let prices: Vec<f64> = (0..30).map(|i| 3000.0 + i as f64 * 35.0).collect();
        let current_price = prices.last().copied().unwrap_or(3000.0);
        MarketContext {
            pair: "ETH/USDC".into(),
            pool_address: Default::default(),
            prices,
            volumes: vec![],
            orderbook: OrderBook { bids: vec![], asks: vec![] },
            recent_whale_txs: vec![],
            pool_liquidity: 10_000_000.0,
            current_price,
            block_number: 0,
            timestamp: Instant::now(),
        }
    }

    #[tokio::test]
    async fn test_majority_buy() {
        let signals: Vec<Box<dyn Signal>> = vec![
            Box::new(TestSignal { name: "A", vote: Vote::Buy, confidence: 0.8 }),
            Box::new(TestSignal { name: "B", vote: Vote::Buy, confidence: 0.7 }),
            Box::new(TestSignal { name: "C", vote: Vote::Sell, confidence: 0.6 }),
            Box::new(TestSignal { name: "D", vote: Vote::Buy, confidence: 0.9 }),
        ];
        let engine = ConsensusEngine::new(signals, 3, 0.35);
        let decision = engine.evaluate(&make_ctx()).await;
        assert_eq!(decision.action, Vote::Buy);
    }

    #[tokio::test]
    async fn test_tie_with_confidence_break() {
        let signals: Vec<Box<dyn Signal>> = vec![
            Box::new(TestSignal { name: "A", vote: Vote::Buy, confidence: 0.9 }),
            Box::new(TestSignal { name: "B", vote: Vote::Buy, confidence: 0.8 }),
            Box::new(TestSignal { name: "C", vote: Vote::Sell, confidence: 0.5 }),
            Box::new(TestSignal { name: "D", vote: Vote::Sell, confidence: 0.5 }),
        ];
        let engine = ConsensusEngine::new(signals, 3, 0.35);
        let decision = engine.evaluate(&make_ctx()).await;
        assert_eq!(decision.action, Vote::Buy);
    }

    #[tokio::test]
    async fn test_confidence_gate() {
        let signals: Vec<Box<dyn Signal>> = vec![
            Box::new(TestSignal { name: "A", vote: Vote::Buy, confidence: 0.2 }),
            Box::new(TestSignal { name: "B", vote: Vote::Buy, confidence: 0.1 }),
            Box::new(TestSignal { name: "C", vote: Vote::Buy, confidence: 0.3 }),
            Box::new(TestSignal { name: "D", vote: Vote::Neutral, confidence: 0.0 }),
        ];
        let engine = ConsensusEngine::new(signals, 3, 0.35);
        let decision = engine.evaluate(&make_ctx()).await;
        assert_eq!(decision.action, Vote::Neutral);
    }

    #[tokio::test]
    async fn test_regime_pregate_blocks_ranging() {
        let prices: Vec<f64> = (0..30).map(|_| 3000.0).collect();
        let current_price = 3000.0;
        let ctx = MarketContext {
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
        };

        let signals: Vec<Box<dyn Signal>> = vec![
            Box::new(TestSignal { name: "A", vote: Vote::Buy, confidence: 0.8 }),
            Box::new(TestSignal { name: "B", vote: Vote::Buy, confidence: 0.7 }),
            Box::new(TestSignal { name: "C", vote: Vote::Buy, confidence: 0.9 }),
            Box::new(TestSignal { name: "D", vote: Vote::Buy, confidence: 0.85 }),
        ];
        let engine = ConsensusEngine::new(signals, 3, 0.35);
        let decision = engine.evaluate(&ctx).await;
        assert_eq!(decision.action, Vote::Neutral);
        assert_eq!(decision.blocked_by, Some("RegimeGate"));
        assert_eq!(decision.size_multiplier, 0.0);
    }

}

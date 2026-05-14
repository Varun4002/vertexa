use std::collections::{HashSet, VecDeque};
use std::sync::Arc;
use tokio::sync::RwLock;
use tracing::info;

use vertexa_core::{
    ExecutionRoute, MevThreatAssessment, PlannedTrade, PendingTx,
};

pub struct MevGuard {
    pending_txs: Arc<RwLock<VecDeque<PendingTx>>>,
    known_bots: HashSet<alloy::primitives::Address>,
    flashbots_relay_url: String,
    min_chunk_usd: f64,
}

impl MevGuard {
    pub fn new(
        pending_txs: Arc<RwLock<VecDeque<PendingTx>>>,
        known_bots: HashSet<alloy::primitives::Address>,
        flashbots_relay_url: &str,
        min_chunk_usd: f64,
    ) -> Self {
        Self {
            pending_txs,
            known_bots,
            flashbots_relay_url: flashbots_relay_url.to_string(),
            min_chunk_usd,
        }
    }

    pub async fn assess(
        &self,
        trade: &PlannedTrade,
        pool_liquidity: f64,
    ) -> MevThreatAssessment {
        let pending_txs = self.pending_txs.read().await;

        let bot_count = self.count_bots_in_mempool(&pending_txs);
        let trade_impact = if pool_liquidity > 0.0 {
            trade.amount_usd / pool_liquidity
        } else {
            0.0
        };
        let active_bots = bot_count as f64;
        let slippage = trade.max_slippage;

        let mut score = 0.0;

        score += (bot_count as f64 * 0.25).min(0.50);
        score += (trade_impact * 2.0).min(0.30);
        score += (active_bots * 0.05).min(0.20);
        score += (slippage * 5.0).min(0.15);
        score = score.clamp(0.0, 1.0);

        let risk_score = score * trade_impact.min(1.0);

        let recommended_route = if risk_score < 0.15 {
            ExecutionRoute::PublicMempool
        } else if risk_score < 0.45 {
            ExecutionRoute::FlashbotsBundle
        } else if risk_score < 0.75 {
            ExecutionRoute::SplitExecution
        } else {
            ExecutionRoute::Abort
        };

        let estimated_mev_loss_usd = trade.amount_usd * score * 0.01;

        let assessment = MevThreatAssessment {
            risk_score,
            sandwich_probability: score,
            recommended_route: recommended_route.clone(),
            estimated_mev_loss_usd,
        };

        info!(
            target: "vertexa",
            risk_score = assessment.risk_score,
            sandwich_prob = assessment.sandwich_probability,
            route = ?assessment.recommended_route,
            mev_loss_est = assessment.estimated_mev_loss_usd,
            "mev assessment"
        );

        assessment
    }

    fn count_bots_in_mempool(&self, pending_txs: &VecDeque<PendingTx>) -> usize {
        pending_txs
            .iter()
            .filter(|tx| self.known_bots.contains(&tx.from))
            .count()
    }

    pub fn flashbots_relay_url(&self) -> &str {
        &self.flashbots_relay_url
    }

    pub fn min_chunk_usd(&self) -> f64 {
        self.min_chunk_usd
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use alloy::primitives::{address, B256, U256};
    use vertexa_core::{Vote, PendingTx};

    #[tokio::test]
    async fn test_low_risk_goes_public() {
        let guard = MevGuard::new(
            Arc::new(RwLock::new(VecDeque::new())),
            HashSet::new(),
            "https://relay.arbitrum.io",
            500.0,
        );
        let trade = PlannedTrade {
            action: Vote::Buy,
            token_in: address!("0000000000000000000000000000000000000001"),
            token_out: address!("0000000000000000000000000000000000000002"),
            amount_in: U256::from(100_000_000_000_000_000u128),
            amount_usd: 100.0,
            max_slippage: 0.005,
            pool_fee: 3000,
        };
        let assessment = guard.assess(&trade, 10_000_000.0).await;
        assert_eq!(assessment.recommended_route, ExecutionRoute::PublicMempool);
    }

    #[tokio::test]
    async fn test_high_risk_aborts() {
        let mut pending = VecDeque::new();
        let bot = address!("0000000000000000000000000000000000000001");
        pending.push_back(PendingTx {
            hash: B256::default(),
            from: bot,
            to: None,
            value: U256::ZERO,
            input: vec![],
            block_number: None,
            direction: None,
        });
        let mut bots = HashSet::new();
        bots.insert(bot);

        let guard = MevGuard::new(
            Arc::new(RwLock::new(pending)),
            bots,
            "https://relay.arbitrum.io",
            500.0,
        );
        let trade = PlannedTrade {
            action: Vote::Buy,
            token_in: address!("0000000000000000000000000000000000000001"),
            token_out: address!("0000000000000000000000000000000000000002"),
            amount_in: U256::from(10_000_000_000_000_000_000_000u128),
            amount_usd: 100_000.0,
            max_slippage: 0.05,
            pool_fee: 3000,
        };
        let assessment = guard.assess(&trade, 100_000.0).await;
        assert_eq!(assessment.recommended_route, ExecutionRoute::Abort);
        assert!(assessment.risk_score >= 0.75);
    }
}

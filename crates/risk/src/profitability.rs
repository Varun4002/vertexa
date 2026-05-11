use vertexa_core::{PlannedTrade, FixedUsd};

pub struct ProfitabilityCheck {
    pub min_reward_to_cost: f64,
}

pub struct CostBreakdown {
    pub gas_cost_usd: FixedUsd,
    pub slippage_cost_usd: FixedUsd,
    pub mev_cost_usd: FixedUsd,
    pub total_cost_usd: FixedUsd,
    pub expected_edge_usd: FixedUsd,
    pub net_expected_usd: FixedUsd,
    pub is_profitable: bool,
    pub reward_to_cost: f64,
}

impl ProfitabilityCheck {
    pub fn new() -> Self {
        Self { min_reward_to_cost: 1.5 }
    }

    pub fn evaluate(
        &self,
        trade: &PlannedTrade,
        avg_confidence: f64,
        estimated_mev_loss: FixedUsd,
        gas_estimate_usd: FixedUsd,
    ) -> CostBreakdown {
        let edge_pct = 0.005 + (avg_confidence * 0.01);
        let expected_edge_usd = FixedUsd::from_dollars(
            trade.amount_usd * edge_pct
        );

        let slippage_cost_usd = FixedUsd::from_dollars(
            trade.amount_usd * trade.max_slippage
        );

        let total_cost_usd = gas_estimate_usd + slippage_cost_usd + estimated_mev_loss;

        let net_expected_usd = FixedUsd(
            expected_edge_usd.0 - total_cost_usd.0
        );

        let reward_to_cost = if total_cost_usd.0 == 0.0 {
            f64::INFINITY
        } else {
            expected_edge_usd.to_dollars() / total_cost_usd.to_dollars()
        };

        let is_profitable = reward_to_cost >= self.min_reward_to_cost;

        CostBreakdown {
            gas_cost_usd: gas_estimate_usd,
            slippage_cost_usd,
            mev_cost_usd: estimated_mev_loss,
            total_cost_usd,
            expected_edge_usd,
            net_expected_usd,
            is_profitable,
            reward_to_cost,
        }
    }
}

impl Default for ProfitabilityCheck {
    fn default() -> Self {
        Self::new()
    }
}

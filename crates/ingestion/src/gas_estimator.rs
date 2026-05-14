use std::sync::Arc;
use tokio::sync::RwLock;
use alloy::providers::{Provider, ProviderBuilder};
use alloy::rpc::types::{TransactionRequest, BlockNumberOrTag};
use tracing::{info, warn};

use vertexa_core::FixedUsd;

pub struct GasEstimator {
    rpc_url: String,
    price_feed: Arc<RwLock<f64>>,
}

impl GasEstimator {
    pub fn new(rpc_url: &str, price_feed: Arc<RwLock<f64>>) -> Self {
        Self {
            rpc_url: rpc_url.to_string(),
            price_feed,
        }
    }

    pub async fn estimate(&self, tx: &TransactionRequest) -> Result<FixedUsd, String> {
        let provider = ProviderBuilder::new()
            .connect(&self.rpc_url)
            .await
            .map_err(|e| format!("failed to connect for gas estimate: {e}"))?;

        // 1. Get fee history for base fee prediction and priority fee median
        let fee_history = provider
            .get_fee_history(10, BlockNumberOrTag::Latest, &[50.0])
            .await
            .map_err(|e| format!("failed to get fee history: {e}"))?;

        // Predict next base fee (conservative 12.5% increase from last known)
        let last_base_fee = fee_history.base_fee_per_gas.last().copied().unwrap_or_default();
        let predicted_base_fee = (last_base_fee as f64 * 1.125) as u128;

        // Get median priority fee from last 10 blocks (50th percentile)
        let rewards: Vec<u128> = fee_history.reward
            .iter()
            .flatten()
            .filter_map(|r| r.first().copied())
            .collect();
        
        let median_priority_fee = if rewards.is_empty() {
            1_000_000_000u128 // 1 gwei fallback
        } else {
            let mut sorted = rewards;
            sorted.sort_unstable();
            sorted[sorted.len() / 2]
        };

        let effective_gas_price = predicted_base_fee + median_priority_fee;

        let gas_units = provider
            .estimate_gas(tx.clone())
            .await
            .map_err(|e| format!("failed to estimate gas units: {e}"))?;

        let cost_eth = gas_units as f64 * effective_gas_price as f64 / 1e18;

        let eth_price = *self.price_feed.read().await;

        if eth_price <= 0.0 {
            warn!(target: "vertexa", "eth price unavailable for gas estimate, using fallback $0.10");
            return Ok(FixedUsd::from_dollars(0.10));
        }

        let cost_usd = cost_eth * eth_price;

        info!(
            target: "vertexa",
            gas_units,
            base_fee = predicted_base_fee,
            priority_fee = median_priority_fee,
            cost_eth,
            cost_usd,
            "institutional gas estimate computed"
        );

        Ok(FixedUsd::from_dollars(cost_usd))
    }
}

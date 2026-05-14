use std::time::Duration;
use alloy::providers::{Provider, ProviderBuilder};
use alloy::primitives::U256;
use alloy::rpc::types::TransactionRequest;
use tracing::{info, warn};
use vertexa_core::{SimulationResult, SimulationConfidence};
use vertexa_mev_guard::FlashbotsRelay;

const MAX_RETRIES: u8 = 2;

pub struct Simulator {
    rpc_url: String,
    flashbots: Option<FlashbotsRelay>,
}

impl Simulator {
    pub fn new(rpc_url: &str, flashbots_url: Option<String>) -> Self {
        Self { 
            rpc_url: rpc_url.to_string(),
            flashbots: flashbots_url.map(|url| FlashbotsRelay::new(&url))
        }
    }

    pub async fn simulate_bundle(
        &self, 
        signed_txs: &[Vec<u8>],
        target_block: u64,
        state_block: u64,
    ) -> Result<SimulationResult, String> {
        let flashbots = self.flashbots.as_ref()
            .ok_or_else(|| "flashbots relay not configured for bundle simulation".to_string())?;

        let result = flashbots.call_bundle(signed_txs, target_block, state_block).await?;
        
        // Extract amount_out and gas_used from the last tx in the bundle (ours)
        let tx_results = result.as_array()
            .ok_or_else(|| "invalid bundle simulation result format".to_string())?;
        
        let our_result = tx_results.last()
            .ok_or_else(|| "empty bundle simulation results".to_string())?;

        let amount_out_hex = our_result.get("value")
            .and_then(|v| v.as_str())
            .ok_or_else(|| "missing value in simulation result".to_string())?;
        
        let amount_out = U256::from_str_radix(amount_out_hex.trim_start_matches("0x"), 16)
            .map_err(|e| format!("failed to parse amount_out: {e}"))?;

        let gas_used = our_result.get("gasUsed")
            .and_then(|v| v.as_u64())
            .unwrap_or(0);

        info!(
            target: "vertexa",
            amount_out = %amount_out,
            gas_used,
            confidence = "High",
            "bundle simulation successful"
        );

        Ok(SimulationResult {
            amount_out,
            gas_used,
            confidence: SimulationConfidence::High,
        })
    }

    pub async fn simulate(&self, tx: &TransactionRequest) -> Result<SimulationResult, String> {
        let provider = ProviderBuilder::new()
            .connect(&self.rpc_url)
            .await
            .map_err(|e| format!("failed to connect for simulation: {e}"))?;

        for attempt in 0..=MAX_RETRIES {
            if attempt > 0 {
                tokio::time::sleep(Duration::from_millis(500)).await;
            }

            info!(
                target: "vertexa",
                attempt = attempt + 1,
                max_retries = MAX_RETRIES + 1,
                "simulating swap (standard eth_call)"
            );

            match provider.call(tx.clone()).await {
                Ok(result) => {
                    let amount_out = U256::from_be_slice(&result);
                    if amount_out.is_zero() {
                        warn!(target: "vertexa", "simulation returned zero output");
                        return Err("simulation returned zero output".into());
                    }
                    
                    let gas_used = provider.estimate_gas(tx.clone()).await.unwrap_or(200_000);

                    info!(
                        target: "vertexa",
                        amount_out = %amount_out,
                        gas_used,
                        confidence = "Low",
                        "simulation successful"
                    );
                    
                    return Ok(SimulationResult {
                        amount_out,
                        gas_used,
                        confidence: SimulationConfidence::Low,
                    });
                }
                Err(e) => {
                    warn!(
                        target: "vertexa",
                        attempt = attempt + 1,
                        error = %e,
                        "simulation failed"
                    );
                }
            }
        }

        Err(format!("simulation failed after {} retries", MAX_RETRIES + 1))
    }
}

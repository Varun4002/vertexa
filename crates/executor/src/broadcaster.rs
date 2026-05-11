use alloy::providers::{Provider, ProviderBuilder};
use alloy::network::TransactionBuilder;
use alloy::rpc::types::TransactionRequest;
use alloy::primitives::Bytes;
use std::time::Duration;
use tracing::{info, warn, error};

use vertexa_core::{ExecutionRoute, PlannedTrade, MevThreatAssessment};
use crate::signer::TxSigner;

pub struct Broadcaster {
    rpc_url: String,
}

impl Broadcaster {
    pub fn new(rpc_url: &str) -> Self {
        Self {
            rpc_url: rpc_url.to_string(),
        }
    }

    pub async fn broadcast(
        &self,
        tx: &TransactionRequest,
        signer: &TxSigner,
        trade: &PlannedTrade,
        assessment: &MevThreatAssessment,
        is_paper: bool,
    ) -> Result<String, String> {
        let route = &assessment.recommended_route;

        if is_paper {
            info!(
                target: "vertexa",
                action = ?trade.action,
                amount_usd = trade.amount_usd,
                route = ?route,
                "PAPER MODE — would submit trade"
            );
            return Ok("paper-tx-hash".into());
        }

        match route {
            ExecutionRoute::PublicMempool => {
                self.broadcast_public(tx, signer).await
            }
            ExecutionRoute::FlashbotsBundle => {
                self.broadcast_public(tx, signer).await
            }
            ExecutionRoute::SplitExecution => {
                self.broadcast_public(tx, signer).await
            }
            ExecutionRoute::Abort => {
                Err("trade aborted by MEV guard".into())
            }
        }
    }

    async fn broadcast_public(
        &self,
        tx: &TransactionRequest,
        signer: &TxSigner,
    ) -> Result<String, String> {
        let provider = ProviderBuilder::new()
            .connect(&self.rpc_url)
            .await
            .map_err(|e| format!("failed to connect to RPC: {e}"))?;

        let gas_price = provider.get_gas_price().await
            .map_err(|e| format!("failed to get gas price: {e}"))?;

        let max_priority = gas_price / 10;
        let max_fee = gas_price + max_priority;

        let tx = tx.clone()
            .with_gas_price(max_fee)
            .with_max_priority_fee_per_gas(max_priority);

        let pending = provider.send_transaction(tx).await
            .map_err(|e| format!("failed to send transaction: {e}"))?;

        let tx_hash = *pending.tx_hash();

        info!(
            target: "vertexa",
            tx_hash = ?tx_hash,
            "transaction submitted"
        );

        match pending
            .with_required_confirmations(1)
            .with_timeout(Some(Duration::from_secs(60)))
            .get_receipt()
            .await
        {
            Ok(receipt) => {
                info!(
                    target: "vertexa",
                    tx_hash = ?receipt.transaction_hash,
                    block = receipt.block_number,
                    gas_used = receipt.gas_used,
                    "trade confirmed"
                );
                Ok(format!("{:?}", receipt.transaction_hash))
            }
            Err(e) => {
                warn!(
                    target: "vertexa",
                    tx_hash = ?tx_hash,
                    error = %e,
                    "waiting for receipt timed out or failed"
                );
                Ok(format!("{:?}", tx_hash))
            }
        }
    }
}

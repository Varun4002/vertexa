use alloy::providers::{Provider, ProviderBuilder};
use alloy::network::TransactionBuilder;
use alloy::rpc::types::TransactionRequest;
use std::time::Duration;
use tracing::{info, warn};

use vertexa_core::{ExecutionRoute, PlannedTrade, MevThreatAssessment};
use vertexa_mev_guard::{FlashbotsRelay, SplitExecutor};
use crate::signer::TxSigner;

pub struct Broadcaster {
    rpc_url: String,
    flashbots_relay: FlashbotsRelay,
    split_executor: SplitExecutor,
}

impl Broadcaster {
    pub fn new(
        rpc_url: &str,
        flashbots_relay: FlashbotsRelay,
        split_executor: SplitExecutor,
    ) -> Self {
        Self {
            rpc_url: rpc_url.to_string(),
            flashbots_relay,
            split_executor,
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
                self.broadcast_public(tx).await
            }
            ExecutionRoute::FlashbotsBundle => {
                self.broadcast_flashbots(tx, signer).await
            }
            ExecutionRoute::SplitExecution => {
                self.broadcast_split(tx, signer, trade, assessment).await
            }
            ExecutionRoute::Abort => {
                Err("trade aborted by MEV guard".into())
            }
        }
    }

    async fn broadcast_public(
        &self,
        tx: &TransactionRequest,
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

    async fn broadcast_flashbots(
        &self,
        tx: &TransactionRequest,
        signer: &TxSigner,
    ) -> Result<String, String> {
        let provider = ProviderBuilder::new()
            .connect(&self.rpc_url)
            .await
            .map_err(|e| format!("failed to connect for signing: {e}"))?;

        let block_number = provider.get_block_number().await
            .map_err(|e| format!("failed to get block number: {e}"))?;

        let signed_tx = signer.sign_tx(tx, &provider).await?;

        info!(
            target: "vertexa",
            target_block = block_number + 1,
            "submitting flashbots bundle"
        );

        self.flashbots_relay.submit_bundle_raw(
            &[signed_tx],
            block_number + 1,
        ).await
    }

    async fn broadcast_split(
        &self,
        tx: &TransactionRequest,
        signer: &TxSigner,
        trade: &PlannedTrade,
        assessment: &MevThreatAssessment,
    ) -> Result<String, String> {
        let provider = ProviderBuilder::new()
            .connect(&self.rpc_url)
            .await
            .map_err(|e| format!("failed to connect for split: {e}"))?;

        let block_number = provider.get_block_number().await
            .map_err(|e| format!("failed to get block number: {e}"))?;

        let signed_tx = signer.sign_tx(tx, &provider).await?;
        let chunks = self.split_executor.compute_chunks(trade, assessment.risk_score);

        for chunk in &chunks {
            let target_block = block_number + chunk.delay_blocks + 1;
            info!(
                target: "vertexa",
                chunk_target = target_block,
                chunk_amount = %chunk.amount_in,
                "submitting split chunk via flashbots"
            );
            self.flashbots_relay.submit_bundle_raw(
                std::slice::from_ref(&signed_tx),
                target_block,
            ).await?;
        }

        Ok(format!("split-{}chunks", chunks.len()))
    }
}

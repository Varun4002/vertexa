use std::time::Duration;
use tokio::sync::{mpsc, oneshot};
use alloy::consensus::{SignableTransaction, TxEip1559, TxEnvelope};
use alloy::eips::Encodable2718;
use alloy::primitives::{TxKind, U256, B256};
use alloy::providers::{Provider, ProviderBuilder};
use alloy::rpc::types::{TransactionRequest, BlockNumberOrTag};
use alloy::signers::local::PrivateKeySigner;
use alloy::signers::Signer;
use tracing::{info, warn, error};

use vertexa_core::{ExecutionResult, PlannedTrade, ARBITRUM_CHAIN_ID, TradeIntent};

pub type ExecCommand = TradeIntent;

const MAX_RETRIES: u8 = 4;
const ATTEMPT_TIMEOUT_SECS: u64 = 2; // Aggressive escalation
const INITIAL_FEE_BUMP_PCT: f64 = 0.10; // Start at 110% of base fee
const ESCALATION_MULTIPLIER: f64 = 1.5; // 1.5x fee every timeout
const FIRST_BUMP_DELAY_MS: u64 = 500; // 0.5s delay (1 block) before first bump

pub struct ExecutorActor {
    rx: mpsc::Receiver<ExecCommand>,
    rpc_url: String,
    signer: PrivateKeySigner,
    nonce: u64,
    is_paper: bool,
}

impl ExecutorActor {
    pub fn spawn(
        rx: mpsc::Receiver<ExecCommand>,
        rpc_url: &str,
        signer: PrivateKeySigner,
        is_paper: bool,
    ) {
        let url = rpc_url.to_string();
        tokio::spawn(async move {
            let mut actor = ExecutorActor {
                rx,
                rpc_url: url,
                signer,
                nonce: 0,
                is_paper,
            };
            actor.run().await;
        });
    }

    async fn run(&mut self) {
        if !self.is_paper {
            match self.init_nonce().await {
                Ok(nonce) => {
                    self.nonce = nonce;
                    info!(target: "vertexa", nonce, "executor actor initialized nonce");
                }
                Err(e) => {
                    error!(target: "vertexa", error = %e, "failed to get initial nonce");
                }
            }
        }

        while let Some((trade, tx, resp, max_gas_price)) = self.rx.recv().await {
            let result = if self.is_paper {
                self.execute_paper(&trade).await
            } else {
                self.execute_real(&trade, &tx, max_gas_price).await
            };
            let _ = resp.send(result);
        }
    }

    async fn init_nonce(&self) -> Result<u64, String> {
        let provider = ProviderBuilder::new()
            .connect(&self.rpc_url)
            .await
            .map_err(|e| format!("failed to connect: {e}"))?;
        provider
            .get_transaction_count(self.signer.address())
            .await
            .map_err(|e| format!("failed to get nonce: {e}"))
    }

    async fn execute_paper(&self, trade: &PlannedTrade) -> ExecutionResult {
        info!(
            target: "vertexa",
            action = ?trade.action,
            amount_usd = trade.amount_usd,
            "PAPER MODE — speculative execution"
        );
        ExecutionResult {
            success: true,
            tx_hash: Some(B256::default()),
            gas_used: Some(100_000),
            actual_amount_out: Some(trade.amount_in), // Mock output
            error: None,
        }
    }

    async fn execute_real(
        &mut self,
        trade: &PlannedTrade,
        tx: &TransactionRequest,
        max_gas_price: Option<u128>,
    ) -> ExecutionResult {
        let provider = match ProviderBuilder::new().connect(&self.rpc_url).await {
            Ok(p) => p,
            Err(e) => return ExecutionResult { success: false, tx_hash: None, gas_used: None, actual_amount_out: None, error: Some(format!("connect failed: {e}")) },
        };

        let gas_limit = match provider.estimate_gas(tx.clone()).await {
            Ok(g) => g,
            Err(e) => return ExecutionResult { success: false, tx_hash: None, gas_used: None, actual_amount_out: None, error: Some(format!("gas limit fail: {e}")) },
        };

        // Get initial fee history for 75th percentile priority fee
        let fee_history = match provider.get_fee_history(5, BlockNumberOrTag::Latest, &[75.0]).await {
            Ok(h) => h,
            Err(e) => return ExecutionResult { success: false, tx_hash: None, gas_used: None, actual_amount_out: None, error: Some(format!("fee history fail: {e}")) },
        };

        let base_fee = fee_history.base_fee_per_gas.last().copied().unwrap_or_default() as u128;
        let p75_priority = fee_history.reward.iter().flatten().filter_map(|r| r.first().copied()).max().unwrap_or(1_000_000_000);

        let to = tx.to.unwrap_or(TxKind::Create);
        let value = tx.value.unwrap_or(U256::ZERO);
        let input = tx.input.input().cloned().unwrap_or_default();

        let mut current_base_multiplier = 1.0 + INITIAL_FEE_BUMP_PCT;
        let mut current_priority_multiplier = 1.0;
        let mut last_error = String::new();

        for attempt in 0..MAX_RETRIES {
            if attempt == 1 {
                // Delay before the first bump (transition from attempt 1 to 2)
                tokio::time::sleep(Duration::from_millis(FIRST_BUMP_DELAY_MS)).await;
            }

            let mut max_priority = (p75_priority as f64 * current_priority_multiplier) as u128;
            let mut max_fee = (base_fee as f64 * current_base_multiplier + max_priority as f64) as u128;

            // Apply gas price cap
            if let Some(cap) = max_gas_price {
                if max_fee > cap {
                    warn!(target: "vertexa", max_fee, cap, "clamping gas price to profitability cap");
                    max_fee = cap;
                    // Adjust priority if fee was capped
                    if max_priority > cap.saturating_sub(base_fee) {
                        max_priority = cap.saturating_sub(base_fee);
                    }
                }
            }

            let tx_1559 = TxEip1559 {
                chain_id: ARBITRUM_CHAIN_ID,
                nonce: self.nonce,
                gas_limit,
                max_fee_per_gas: max_fee,
                max_priority_fee_per_gas: max_priority,
                to,
                value,
                input: input.clone(),
                access_list: Default::default(),
            };

            let hash = tx_1559.signature_hash();
            let signature = match self.signer.sign_hash(&hash).await {
                Ok(s) => s,
                Err(e) => {
                    last_error = format!("signing fail: {e}");
                    continue;
                }
            };

            let signed_tx = tx_1559.into_signed(signature);
            let envelope = TxEnvelope::from(signed_tx);
            let raw_tx = envelope.encoded_2718();

            info!(
                target: "vertexa",
                attempt = attempt + 1,
                max_fee,
                max_priority,
                "broadcasting with aggressive fee escalation"
            );

            let pending = match provider.send_raw_transaction(&raw_tx).await {
                Ok(p) => p,
                Err(e) => {
                    last_error = format!("broadcast fail: {e}");
                    continue;
                }
            };

            let tx_hash = *pending.tx_hash();

            match tokio::time::timeout(
                Duration::from_secs(ATTEMPT_TIMEOUT_SECS),
                pending.get_receipt(),
            ).await {
                Ok(Ok(receipt)) => {
                    let actual_amount_out = if receipt.status() {
                        // Extract actual amount out from Swap events in logs
                        // This is a simplified extraction; real implementation would use proper decoding
                        receipt.logs().iter().find_map(|log| {
                            // Match Swap event signature hash
                            if !log.topics().is_empty() && log.topics()[0] == alloy::primitives::keccak256(b"Swap(address,address,int256,int256,uint160,uint128,int24)") {
                                let data = &log.data().data;
                                if data.len() >= 64 {
                                    // Uniswap V3 Swap event: amount0 and amount1 are first two 32-byte slots
                                    let amount0 = alloy::primitives::I256::try_from_be_slice(&data[0..32]).unwrap_or_default();
                                    let amount1 = alloy::primitives::I256::try_from_be_slice(&data[32..64]).unwrap_or_default();
                                    
                                    // If we are selling ETH (amount0 < 0), we want amount1 (positive)
                                    // If we are buying ETH (amount0 > 0), we want amount0 (positive, but it's amount_in)
                                    // Actually, amount_out is the positive value that was NOT amount_in.
                                    let out = if amount0.is_negative() { amount1 } else { amount0 };
                                    Some(out.abs().into_raw())
                                } else { None }
                            } else { None }
                        })
                    } else { None };

                    if receipt.status() {
                        self.nonce += 1;
                        return ExecutionResult {
                            success: true,
                            tx_hash: Some(tx_hash),
                            gas_used: Some(receipt.gas_used),
                            actual_amount_out,
                            error: None,
                        };
                    } else {
                        self.nonce += 1;
                        return ExecutionResult {
                            success: false,
                            tx_hash: Some(tx_hash),
                            gas_used: Some(receipt.gas_used),
                            actual_amount_out: None,
                            error: Some("reverted".into()),
                        };
                    }
                }
                Ok(Err(e)) => {
                    last_error = format!("receipt fail: {e}");
                }
                Err(_) => {
                    warn!(target: "vertexa", attempt = attempt + 1, "escalation timeout — bumping fees");
                    current_base_multiplier *= ESCALATION_MULTIPLIER;
                    current_priority_multiplier *= ESCALATION_MULTIPLIER;
                    last_error = format!("timeout after {ATTEMPT_TIMEOUT_SECS}s");
                }
            }
        }

        error!(target: "vertexa", nonce = self.nonce, "failed after max escalations");
        self.nonce += 1;
        ExecutionResult {
            success: false,
            tx_hash: None,
            gas_used: None,
            actual_amount_out: None,
            error: Some(format!("escalation exhausted: {last_error}")),
        }
    }
}

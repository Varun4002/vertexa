use std::collections::{HashSet, VecDeque};
use std::sync::Arc;
use std::time::Duration;
use tokio::sync::RwLock;
use futures::StreamExt;
use alloy::providers::{Provider, ProviderBuilder};
use alloy::primitives::{Address, B256};
use alloy::consensus::Transaction;
use alloy::network::TransactionResponse;
use tracing::{info, warn, error};

use vertexa_core::PendingTx;

const MAX_MEMPOOL_SIZE: usize = 500;
const SWAP_EXACT_INPUT_SINGLE_SELECTOR: [u8; 4] = [0x41, 0x4b, 0xf3, 0x89];
const SWAP_EXACT_INPUT_SELECTOR: [u8; 4] = [0xc0, 0x4b, 0x8d, 0x59];

pub struct MempoolMonitor {
    pending_txs: Arc<RwLock<VecDeque<PendingTx>>>,
    known_bots: HashSet<Address>,
}

impl MempoolMonitor {
    pub fn new(known_bots: &[String]) -> Self {
        let bot_addresses = known_bots.iter()
            .filter_map(|s| s.parse::<Address>().ok())
            .collect();

        Self {
            pending_txs: Arc::new(RwLock::new(VecDeque::with_capacity(MAX_MEMPOOL_SIZE))),
            known_bots: bot_addresses,
        }
    }

    pub fn pending_txs(&self) -> Arc<RwLock<VecDeque<PendingTx>>> {
        self.pending_txs.clone()
    }

    pub fn known_bots(&self) -> &HashSet<Address> {
        &self.known_bots
    }

    pub async fn start_monitor(&self, ws_url: &str) {
        let pending_txs = self.pending_txs.clone();
        let url = ws_url.to_string();

        tokio::spawn(async move {
            let mut retry_delay = Duration::from_secs(1);

            loop {
                match run_mempool_connection(&url, pending_txs.clone()).await {
                    Ok(()) => {
                        info!(target: "vertexa", "mempool monitor connection closed normally");
                    }
                    Err(e) => {
                        error!(target: "vertexa", error = %e, "mempool monitor error");
                    }
                }

                tokio::time::sleep(retry_delay).await;
                retry_delay = (retry_delay * 2).min(Duration::from_secs(30));
            }
        });
    }
}

async fn run_mempool_connection(
    url: &str,
    pending_txs: Arc<RwLock<VecDeque<PendingTx>>>,
) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
    let provider = ProviderBuilder::new()
        .connect(url)
        .await
        .map_err(|e| format!("failed to connect WS: {e}"))?;

    let sub = provider
        .subscribe_pending_transactions()
        .await
        .map_err(|e| format!("failed to subscribe pending txs: {e}"))?;

    let mut stream = sub.into_stream();

    info!(target: "vertexa", "connected to mempool, watching pending txs");

    while let Some(tx_hash) = stream.next().await {
        match provider.get_transaction_by_hash(tx_hash).await {
            Ok(Some(tx)) => {
                let input = tx.input().0.to_vec();
                let is_swap = input.len() >= 4 && (
                    input[..4] == SWAP_EXACT_INPUT_SINGLE_SELECTOR ||
                    input[..4] == SWAP_EXACT_INPUT_SELECTOR
                );

                if !is_swap {
                    continue;
                }

                let pending = PendingTx {
                    hash: tx_hash,
                    from: tx.from(),
                    to: tx.to(),
                    value: tx.value(),
                    input,
                    block_number: tx.block_number(),
                };

                let mut queue = pending_txs.write().await;
                if queue.len() >= MAX_MEMPOOL_SIZE {
                    queue.pop_front();
                }
                queue.push_back(pending);
            }
            Ok(None) => {}
            Err(e) => {
                warn!(target: "vertexa", hash = ?tx_hash, error = %e, "failed to get tx details");
            }
        }
    }

    Ok(())
}

use std::collections::{HashSet, VecDeque};
use std::sync::Arc;
use std::time::Duration;
use tokio::sync::RwLock;
use futures::StreamExt;
use alloy::providers::{Provider, ProviderBuilder};
use alloy::primitives::Address;
use alloy::consensus::Transaction;
use alloy::network::TransactionResponse;
use tracing::{info, warn, error};

use alloy::sol_types::SolCall;
use vertexa_core::{PendingTx, Vote, WETH_ADDRESS, USDC_ADDRESS};

alloy::sol! {
    struct ExactInputSingleDecode {
        address tokenIn;
        address tokenOut;
        uint24 fee;
        address recipient;
        uint256 amountIn;
        uint256 amountOutMinimum;
        uint160 sqrtPriceLimitX96;
    }

    struct ExactOutputSingleDecode {
        address tokenIn;
        address tokenOut;
        uint24 fee;
        address recipient;
        uint256 amountOut;
        uint256 amountInMaximum;
        uint160 sqrtPriceLimitX96;
    }

    struct ExactInputDecode {
        bytes path;
        address recipient;
        uint256 amountIn;
        uint256 amountOutMinimum;
    }

    struct ExactOutputDecode {
        bytes path;
        address recipient;
        uint256 amountOut;
        uint256 amountInMaximum;
    }

    struct SwapDescription {
        address srcToken;
        address dstToken;
        address srcReceiver;
        address dstReceiver;
        uint256 amount;
        uint256 minReturnAmount;
        uint256 flags;
    }

    function exactInputSingle(ExactInputSingleDecode params) external payable returns (uint256 amountOut);
    function exactOutputSingle(ExactOutputSingleDecode params) external payable returns (uint256 amountIn);
    function exactInput(ExactInputDecode params) external payable returns (uint256 amountOut);
    function exactOutput(ExactOutputDecode params) external payable returns (uint256 amountIn);
    function swap(address executor, SwapDescription params, bytes permit, bytes data) external payable returns (uint256 returnAmount, uint256 spentAmount);
}

const MAX_MEMPOOL_SIZE: usize = 500;
const SWAP_EXACT_INPUT_SINGLE_SELECTOR: [u8; 4] = [0x41, 0x4b, 0xf3, 0x89];
const SWAP_EXACT_INPUT_SELECTOR: [u8; 4] = [0xc0, 0x4b, 0x8d, 0x59];
const SWAP_EXACT_OUTPUT_SINGLE_SELECTOR: [u8; 4] = [0xdb, 0x3e, 0x21, 0x98];
const SWAP_EXACT_OUTPUT_SELECTOR: [u8; 4] = [0xf2, 0x8c, 0x04, 0x40];
const ONE_INCH_SWAP_SELECTOR: [u8; 4] = [0x12, 0xaa, 0x3c, 0xaf];

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
                    input[..4] == SWAP_EXACT_INPUT_SELECTOR ||
                    input[..4] == SWAP_EXACT_OUTPUT_SINGLE_SELECTOR ||
                    input[..4] == SWAP_EXACT_OUTPUT_SELECTOR ||
                    input[..4] == ONE_INCH_SWAP_SELECTOR
                );

                if !is_swap {
                    continue;
                }

                let direction = decode_swap_direction(&input);

                let pending = PendingTx {
                    hash: tx_hash,
                    from: tx.from(),
                    to: tx.to(),
                    value: tx.value(),
                    input,
                    block_number: tx.block_number(),
                    direction,
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

pub fn decode_swap_direction(input: &[u8]) -> Option<Vote> {
    if input.len() < 4 {
        return None;
    }

    let selector = &input[..4];
    let data = &input[4..];

    let (token_in, token_out) = if selector == SWAP_EXACT_INPUT_SINGLE_SELECTOR {
        let call = exactInputSingleCall::abi_decode(data).ok()?;
        (call.params.tokenIn, call.params.tokenOut)
    } else if selector == SWAP_EXACT_OUTPUT_SINGLE_SELECTOR {
        let call = exactOutputSingleCall::abi_decode(data).ok()?;
        (call.params.tokenIn, call.params.tokenOut)
    } else if selector == SWAP_EXACT_INPUT_SELECTOR {
        let call = exactInputCall::abi_decode(data).ok()?;
        let path = &call.params.path;
        if path.len() < 43 {
            return None;
        }
        let token_in = Address::from_slice(&path[..20]);
        let token_out = Address::from_slice(&path[path.len() - 20..]);
        (token_in, token_out)
    } else if selector == SWAP_EXACT_OUTPUT_SELECTOR {
        let call = exactOutputCall::abi_decode(data).ok()?;
        let path = &call.params.path;
        if path.len() < 43 {
            return None;
        }
        // For ExactOutput, path is (tokenOut, fee, tokenIn)
        let token_out = Address::from_slice(&path[..20]);
        let token_in = Address::from_slice(&path[path.len() - 20..]);
        (token_in, token_out)
    } else if selector == ONE_INCH_SWAP_SELECTOR {
        let call = swapCall::abi_decode(data).ok()?;
        (call.params.srcToken, call.params.dstToken)
    } else {
        return None;
    };

    if token_in == USDC_ADDRESS && token_out == WETH_ADDRESS {
        Some(Vote::Buy)
    } else if token_in == WETH_ADDRESS && token_out == USDC_ADDRESS {
        Some(Vote::Sell)
    } else {
        None
    }
}

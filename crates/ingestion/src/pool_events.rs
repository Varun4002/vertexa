use std::sync::Arc;
use std::time::Duration;
use tokio::sync::RwLock;
use futures::StreamExt;
use alloy::providers::{Provider, ProviderBuilder};
use alloy::primitives::{Address, U256, keccak256};
use alloy::rpc::types::Filter;
use tracing::{info, error};
use vertexa_core::{TickLiquidity, TickData};

fn sqrt_price_to_usd(sqrt_price_x96: U256) -> f64 {
    let q96 = U256::from(1u128 << 96);
    let price_x96 = sqrt_price_x96 * sqrt_price_x96 * U256::from(10u128.pow(12));
    let numerator = price_x96;
    let denominator = q96 * q96;
    if denominator.is_zero() {
        return 0.0;
    }
    let div = numerator / denominator;
    let rem = numerator % denominator;
    let int_part = div.to::<u128>();
    let frac_part = if !denominator.is_zero() {
        let frac_num = rem * U256::from(1_000_000_000u128);
        (frac_num / denominator).to::<u128>() as f64 / 1_000_000_000.0
    } else {
        0.0
    };
    int_part as f64 + frac_part
}

fn swap_signature_hash() -> [u8; 32] {
    *keccak256(b"Swap(address,address,int256,int256,uint160,uint128,int24)")
}

fn mint_signature_hash() -> [u8; 32] {
    *keccak256(b"Mint(address,address,int24,int24,uint128,uint256,uint256)")
}

fn burn_signature_hash() -> [u8; 32] {
    *keccak256(b"Burn(address,int24,int24,uint128,uint256,uint256)")
}

fn parse_swap_event(log_topics: &[[u8; 32]], log_data: &[u8]) -> Option<(U256, u128, i32)> {
    if log_topics.is_empty() || log_topics[0] != swap_signature_hash() {
        return None;
    }
    if log_data.len() < 160 {
        return None;
    }
    let sqrt_price = U256::from_be_slice(&log_data[64..96]);
    let liquidity_bytes: [u8; 16] = log_data[112..128].try_into().ok()?;
    let liquidity = u128::from_be_bytes(liquidity_bytes);
    let tick_bytes: [u8; 4] = log_data[156..160].try_into().ok()?;
    let tick = i32::from_be_bytes(tick_bytes);
    Some((sqrt_price, liquidity, tick))
}

fn parse_liquidity_event(log_topics: &[[u8; 32]], log_data: &[u8]) -> Option<(i32, i32, i128)> {
    if log_topics.is_empty() {
        return None;
    }

    let is_mint = log_topics[0] == mint_signature_hash();
    let is_burn = log_topics[0] == burn_signature_hash();

    if !is_mint && !is_burn {
        return None;
    }

    if log_data.len() < 96 {
        return None;
    }

    let tick_lower_bytes: [u8; 4] = log_data[28..32].try_into().ok()?;
    let tick_lower = i32::from_be_bytes(tick_lower_bytes);
    
    let tick_upper_bytes: [u8; 4] = log_data[60..64].try_into().ok()?;
    let tick_upper = i32::from_be_bytes(tick_upper_bytes);
    
    let amount_bytes: [u8; 16] = log_data[80..96].try_into().ok()?;
    let amount = u128::from_be_bytes(amount_bytes) as i128;
    
    let amount_net = if is_mint { amount } else { -amount };
    
    Some((tick_lower, tick_upper, amount_net))
}

pub async fn start(
    pool_price: Arc<RwLock<f64>>,
    pool_liquidity: Arc<RwLock<f64>>,
    tick_liquidity: Arc<RwLock<TickLiquidity>>,
    pool_address: Address,
    rpc_ws: &str,
    reset_signal: Arc<tokio::sync::Notify>,
) {
    let url = rpc_ws.to_string();
    tokio::spawn(async move {
        let mut retry_delay = Duration::from_secs(1);
        loop {
            tokio::select! {
                res = run_stream(
                    pool_price.clone(),
                    pool_liquidity.clone(),
                    tick_liquidity.clone(),
                    pool_address,
                    &url,
                ) => {
                    match res {
                        Ok(()) => {
                            info!(target: "vertexa", "pool events stream closed normally");
                        }
                        Err(e) => {
                            error!(target: "vertexa", error = %e, "pool events stream error");
                        }
                    }
                }
                _ = reset_signal.notified() => {
                    info!(target: "vertexa", "re-sync signal received, restarting pool event stream");
                    // Reset local state if needed (optional, or caller can handle it)
                }
            }
            info!(target: "vertexa", delay_ms = retry_delay.as_millis(), "reconnecting pool events");
            tokio::time::sleep(retry_delay).await;
            retry_delay = (retry_delay * 2).min(Duration::from_secs(60));
        }
    });
}

async fn run_stream(
    pool_price: Arc<RwLock<f64>>,
    pool_liquidity: Arc<RwLock<f64>>,
    tick_liquidity: Arc<RwLock<TickLiquidity>>,
    pool_address: Address,
    url: &str,
) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
    let provider = ProviderBuilder::new()
        .connect(url)
        .await?;

    let latest_block = provider.get_block_number().await.unwrap_or(0);
    let from_block = if latest_block > 10 { latest_block - 10 } else { 0 };

    let filter = Filter::new()
        .address(pool_address)
        .from_block(from_block);

    let sub = provider.subscribe_logs(&filter).await?;
    let mut stream = sub.into_stream();

    info!(target: "vertexa", pool = ?pool_address, "subscribed to pool events");

    while let Some(log) = stream.next().await {
        let topics: Vec<[u8; 32]> = log.topics().iter().map(|t| *t.as_ref()).collect();
        let data = &log.data().data;

        if let Some((sqrt_price, liquidity, tick)) = parse_swap_event(&topics, data) {
            let price = sqrt_price_to_usd(sqrt_price);
            {
                let mut p = pool_price.write().await;
                *p = price;
            }
            {
                let mut l = pool_liquidity.write().await;
                *l = liquidity as f64 * 1e-12;
            }
            {
                let mut tl = tick_liquidity.write().await;
                tl.current_tick = tick;
                tl.current_liquidity = liquidity;
            }
            info!(target: "vertexa", price, liquidity, tick, "swap event processed");
        } else if let Some((tick_lower, tick_upper, amount_net)) = parse_liquidity_event(&topics, data) {
            let mut tl = tick_liquidity.write().await;
            
            let lower = tl.ticks.entry(tick_lower).or_insert(TickData::default());
            lower.liquidity_net += amount_net;
            lower.liquidity_gross = lower.liquidity_gross.wrapping_add_signed(amount_net);
            
            let upper = tl.ticks.entry(tick_upper).or_insert(TickData::default());
            upper.liquidity_net -= amount_net;
            upper.liquidity_gross = upper.liquidity_gross.wrapping_add_signed(amount_net);
            
            info!(target: "vertexa", tick_lower, tick_upper, amount_net, "liquidity event processed");
        }
    }

    Ok(())
}

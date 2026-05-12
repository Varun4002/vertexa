use std::sync::Arc;
use tokio::sync::RwLock;
use alloy::providers::{Provider, ProviderBuilder};
use alloy::primitives::U256;
use alloy::sol;
use tracing::{info, warn, error};
use std::time::Duration;

sol! {
    #[sol(rpc)]
    contract UniswapV3Pool {
        function slot0() external view returns (uint160 sqrtPriceX96, int24 tick, uint16 observationIndex, uint16 observationCardinality, uint16 observationCardinalityNext, uint8 feeProtocol, bool unlocked);
        function liquidity() external view returns (uint128);
    }
}

const POOL_ADDR: &str = "0xC31E54c7a869B9FcBEcc14363CF510d1c41fa443";
const POLL_INTERVAL: Duration = Duration::from_secs(12);

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

pub async fn start(
    current_price: Arc<RwLock<f64>>,
    pool_liquidity: Arc<RwLock<f64>>,
    block_number: Arc<RwLock<u64>>,
    rpc_url: &str,
) {
    let pool_addr = match POOL_ADDR.parse::<alloy::primitives::Address>() {
        Ok(a) => a,
        Err(e) => {
            error!(target: "vertexa", error = %e, "invalid pool address");
            return;
        }
    };

    let rpc = rpc_url.to_string();
    tokio::spawn(async move {
        let provider = match ProviderBuilder::new()
            .connect(&rpc)
            .await
        {
            Ok(p) => p,
            Err(e) => {
                error!(target: "vertexa", error = %e, "failed to connect to RPC for pool reader");
                return;
            }
        };

        let pool = UniswapV3Pool::new(pool_addr, &provider);

        loop {
            match pool.slot0().call().await {
                Ok(result) => {
                    let price = sqrt_price_to_usd(U256::from(result.sqrtPriceX96));
                    let mut price_val = current_price.write().await;
                    *price_val = price;

                    info!(
                        target: "vertexa",
                        sqrt_price = %result.sqrtPriceX96,
                        tick = ?result.tick,
                        price_usd = price,
                        "pool slot0 updated"
                    );
                }
                Err(e) => {
                    warn!(target: "vertexa", error = %e, "failed to read slot0");
                }
            }

            match pool.liquidity().call().await {
                Ok(result) => {
                    let liquidity_f64 = result as f64;
                    let price = *current_price.read().await;
                    let tvl = estimate_tvl(liquidity_f64, price);
                    let mut liq = pool_liquidity.write().await;
                    *liq = tvl;
                }
                Err(e) => {
                    warn!(target: "vertexa", error = %e, "failed to read liquidity");
                }
            }

            if let Ok(bn) = provider.get_block_number().await {
                let mut bn_val = block_number.write().await;
                *bn_val = bn;
            }

            tokio::time::sleep(POLL_INTERVAL).await;
        }
    });
}

fn estimate_tvl(liquidity: f64, _price: f64) -> f64 {
    liquidity * 1e-12
}

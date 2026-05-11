use alloy::sol;
use alloy::sol_types::SolCall;
use alloy::primitives::{Address, U256, Uint, Bytes};
use alloy::providers::{Provider, ProviderBuilder};
use alloy::network::TransactionBuilder;
use alloy::rpc::types::TransactionRequest;
use tracing::{info, warn};

use vertexa_core::{PlannedTrade, UNISWAP_V3_ROUTER, UNISWAP_V3_QUOTER};

sol! {
    #[derive(Debug)]
    struct ExactInputSingleParams {
        address tokenIn;
        address tokenOut;
        uint24 fee;
        address recipient;
        uint256 amountIn;
        uint256 amountOutMinimum;
        uint160 sqrtPriceLimitX96;
    }

    function exactInputSingle(ExactInputSingleParams calldata params) external payable returns (uint256 amountOut);

    function quoteExactInputSingle(
        address tokenIn,
        address tokenOut,
        uint24 fee,
        uint256 amountIn,
        uint160 sqrtPriceLimitX96
    ) external view returns (uint256 amountOut);
}

pub struct SwapBuilder {
    router_address: Address,
    quoter_address: Address,
    recipient: Address,
}

impl SwapBuilder {
    pub fn new(recipient: Address) -> Self {
        Self {
            router_address: UNISWAP_V3_ROUTER,
            quoter_address: UNISWAP_V3_QUOTER,
            recipient,
        }
    }

    pub async fn build_tx(
        &self,
        trade: &PlannedTrade,
        rpc_url: &str,
    ) -> Result<TransactionRequest, String> {
        let quote_amount = self.fetch_quote(
            trade.token_in,
            trade.token_out,
            trade.pool_fee as u32,
            trade.amount_in,
            rpc_url,
        ).await?;

        let amount_out_min = U256::try_from(
            (quote_amount.to::<u128>() as f64 * (1.0 - trade.max_slippage)) as u128
        ).unwrap_or(U256::ZERO);

        if amount_out_min == U256::ZERO {
            return Err("amount out minimum is zero".into());
        }

        let params = ExactInputSingleParams {
            tokenIn: trade.token_in,
            tokenOut: trade.token_out,
            fee: Uint::<24, 1>::from(trade.pool_fee),
            recipient: self.recipient,
            amountIn: trade.amount_in,
            amountOutMinimum: amount_out_min,
            sqrtPriceLimitX96: Uint::<160, 3>::ZERO,
        };

        let encoded = exactInputSingleCall { params }.abi_encode();

        let tx = TransactionRequest::default()
            .to(self.router_address)
            .input(Bytes::from(encoded).into())
            .value(U256::ZERO);

        info!(
            target: "vertexa",
            method = "exactInputSingle",
            token_in = ?trade.token_in,
            token_out = ?trade.token_out,
            amount_in = %trade.amount_in,
            amount_out_min = %amount_out_min,
            "built swap transaction"
        );

        Ok(tx)
    }

    async fn fetch_quote(
        &self,
        token_in: Address,
        token_out: Address,
        fee: u32,
        amount_in: U256,
        rpc_url: &str,
    ) -> Result<U256, String> {
        let provider = ProviderBuilder::new()
            .connect(rpc_url)
            .await
            .map_err(|e| format!("failed to connect for quote: {e}"))?;

        let encoded = quoteExactInputSingleCall {
            tokenIn: token_in,
            tokenOut: token_out,
            fee: Uint::<24, 1>::from(fee),
            amountIn: amount_in,
            sqrtPriceLimitX96: Uint::<160, 3>::ZERO,
        }.abi_encode();

        let call = TransactionRequest::default()
            .to(self.quoter_address)
            .input(Bytes::from(encoded).into());

        let result = provider.call(call).await
            .map_err(|e| format!("quote call failed: {e}"))?;

        let decoded = quoteExactInputSingleCall::abi_decode_returns(&result)
            .map_err(|e| format!("quote decode failed: {e}"))?;

        Ok(decoded)
    }
}

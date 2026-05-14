use alloy::consensus::{SignableTransaction, TxEip1559, TxEnvelope};
use alloy::primitives::TxKind;
use alloy::eips::Encodable2718;
use alloy::network::TransactionBuilder;
use alloy::primitives::{Address, U256};
use alloy::providers::Provider;
use alloy::rpc::types::TransactionRequest;
use alloy::signers::local::PrivateKeySigner;
use alloy::signers::Signer;
use tracing::info;

use vertexa_core::ARBITRUM_CHAIN_ID;

pub struct TxSigner {
    signer: PrivateKeySigner,
}

impl TxSigner {
    pub fn from_env() -> Result<Self, String> {
        let key = std::env::var("VERTEXA_PRIVATE_KEY")
            .map_err(|_| "VERTEXA_PRIVATE_KEY not set".to_string())?;

        let signer: PrivateKeySigner = key.parse()
            .map_err(|e| format!("invalid private key: {e}"))?;

        let signer = signer.with_chain_id(Some(ARBITRUM_CHAIN_ID));

        info!(
            target: "vertexa",
            address = ?signer.address(),
            chain_id = ARBITRUM_CHAIN_ID,
            "signer initialized"
        );

        Ok(Self { signer })
    }

    pub fn address(&self) -> Address {
        self.signer.address()
    }

    pub fn signer(&self) -> &PrivateKeySigner {
        &self.signer
    }

    pub fn clone_signer(&self) -> PrivateKeySigner {
        self.signer.clone()
    }

    pub async fn sign_tx(
        &self,
        tx: &TransactionRequest,
        provider: &impl Provider,
    ) -> Result<Vec<u8>, String> {
        let from = self.address();
        let tx = tx.clone().with_from(from);

        let nonce = provider
            .get_transaction_count(from)
            .await
            .map_err(|e| format!("failed to get nonce: {e}"))?;

        let chain_id = provider
            .get_chain_id()
            .await
            .map_err(|e| format!("failed to get chain id: {e}"))?;

        let gas_limit = provider
            .estimate_gas(tx.clone())
            .await
            .map_err(|e| format!("failed to estimate gas: {e}"))?;

        let fees = provider
            .estimate_eip1559_fees()
            .await
            .map_err(|e| format!("failed to estimate fees: {e}"))?;

        let input = tx.input.input().cloned().unwrap_or_default();
        let to = tx.to.unwrap_or(TxKind::Create);
        let value = tx.value.unwrap_or(U256::ZERO);

        let tx_1559 = TxEip1559 {
            chain_id,
            nonce,
            gas_limit,
            max_fee_per_gas: fees.max_fee_per_gas,
            max_priority_fee_per_gas: fees.max_priority_fee_per_gas,
            to,
            value,
            input,
            access_list: Default::default(),
        };

        let hash = tx_1559.signature_hash();
        let signature = self
            .signer
            .sign_hash(&hash)
            .await
            .map_err(|e| format!("signing failed: {e}"))?;

        let signed_tx = tx_1559.into_signed(signature);
        let envelope = TxEnvelope::from(signed_tx);
        Ok(envelope.encoded_2718())
    }
}

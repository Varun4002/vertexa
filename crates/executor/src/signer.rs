use alloy::signers::local::PrivateKeySigner;
use alloy::signers::Signer;
use alloy::primitives::Address;
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
}

use serde_json::json;
use tracing::{info, warn};
use vertexa_core::PlannedTrade;

pub struct FlashbotsRelay {
    relay_url: String,
}

impl FlashbotsRelay {
    pub fn new(relay_url: &str) -> Self {
        Self {
            relay_url: relay_url.to_string(),
        }
    }

    pub async fn submit_bundle(
        &self,
        _trade: &PlannedTrade,
        _signed_tx: &[u8],
        target_block: u64,
    ) -> Result<String, String> {
        let client = reqwest::Client::new();
        let tx_hex = format!("0x{}", hex::encode(_signed_tx));

        let params = json!({
            "txs": [tx_hex],
            "blockNumber": format!("0x{:x}", target_block),
            "minTimestamp": 0,
            "maxTimestamp": 0,
            "revertingTxHashes": []
        });

        let body = json!({
            "jsonrpc": "2.0",
            "id": 1,
            "method": "eth_sendBundle",
            "params": [params]
        });

        info!(
            target: "vertexa",
            relay = %self.relay_url,
            target_block = target_block,
            method = "eth_sendBundle",
            "submitting flashbots bundle"
        );

        let response = client
            .post(&self.relay_url)
            .header("Content-Type", "application/json")
            .header("User-Agent", "VERTEXA/0.1.0")
            .json(&body)
            .send()
            .await
            .map_err(|e| format!("relay request failed: {e}"))?;

        let status = response.status();
        let text = response.text().await.unwrap_or_default();

        if status.is_success() {
            let bundle_hash = text.trim().to_string();
            info!(
                target: "vertexa",
                bundle_hash = %bundle_hash,
                target_block = target_block,
                "bundle submitted"
            );
            Ok(bundle_hash)
        } else {
            let err = format!("relay returned {status}: {text}");
            warn!(target: "vertexa", error = %err, "flashbots relay error");
            Err(err)
        }
    }

    pub async fn submit_bundle_raw(
        &self,
        signed_txs: &[Vec<u8>],
        target_block: u64,
    ) -> Result<String, String> {
        let client = reqwest::Client::new();
        let txs: Vec<String> = signed_txs.iter()
            .map(|tx| format!("0x{}", hex::encode(tx)))
            .collect();

        let params = json!({
            "txs": txs,
            "blockNumber": format!("0x{:x}", target_block),
            "minTimestamp": 0,
            "maxTimestamp": 0,
            "revertingTxHashes": []
        });

        let body = json!({
            "jsonrpc": "2.0",
            "id": 1,
            "method": "eth_sendBundle",
            "params": [params]
        });

        let response = client
            .post(&self.relay_url)
            .header("Content-Type", "application/json")
            .header("User-Agent", "VERTEXA/0.1.0")
            .json(&body)
            .send()
            .await
            .map_err(|e| format!("relay request failed: {e}"))?;

        let status = response.status();
        let text = response.text().await.unwrap_or_default();

        if status.is_success() {
            Ok(text.trim().to_string())
        } else {
            Err(format!("relay returned {status}: {text}"))
        }
    }

    pub async fn call_bundle(
        &self,
        signed_txs: &[Vec<u8>],
        target_block: u64,
        state_block: u64,
    ) -> Result<serde_json::Value, String> {
        let client = reqwest::Client::new();
        let txs: Vec<String> = signed_txs.iter()
            .map(|tx| format!("0x{}", hex::encode(tx)))
            .collect();

        let body = json!({
            "jsonrpc": "2.0",
            "id": 1,
            "method": "eth_callBundle",
            "params": [
                txs,
                format!("0x{:x}", target_block),
                format!("0x{:x}", state_block)
            ]
        });

        let response = client
            .post(&self.relay_url)
            .header("Content-Type", "application/json")
            .header("User-Agent", "VERTEXA/0.1.0")
            .json(&body)
            .send()
            .await
            .map_err(|e| format!("relay call failed: {e}"))?;

        let text = response.text().await.unwrap_or_default();
        let json: serde_json::Value = serde_json::from_str(&text)
            .map_err(|e| format!("failed to parse simulation result: {e}"))?;

        if let Some(error) = json.get("error") {
            return Err(format!("relay simulation error: {error}"));
        }

        Ok(json.get("result").cloned().unwrap_or_default())
    }

    pub fn relay_url(&self) -> &str {
        &self.relay_url
    }
}

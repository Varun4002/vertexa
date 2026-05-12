use std::sync::Arc;
use tokio::sync::RwLock;
use tokio_tungstenite::{connect_async, tungstenite::Message};
use futures::StreamExt;
use serde_json::Value;
use tracing::{info, error};
use std::time::Duration;

use vertexa_core::PriceSeries;

const BINANCE_WS: &str = "wss://stream.binance.com:9443/ws/ethusdt@kline_1m";

pub async fn start(shared: Arc<RwLock<PriceSeries>>) {
    tokio::spawn(async move {
        let mut retry_delay = Duration::from_secs(1);

        loop {
            match run_connection(shared.clone()).await {
                Ok(()) => {
                    info!(target: "vertexa", "price feed connection closed normally");
                }
                Err(e) => {
                    error!(target: "vertexa", error = %e, "price feed connection error");
                }
            }

            info!(target: "vertexa", delay_ms = retry_delay.as_millis(), "reconnecting price feed");
            tokio::time::sleep(retry_delay).await;
            retry_delay = (retry_delay * 2).min(Duration::from_secs(60));
        }
    });
}

async fn run_connection(shared: Arc<RwLock<PriceSeries>>) -> eyre::Result<()> {
    let ws = connect_async(BINANCE_WS).await.map_err(|e| eyre::eyre!("WS connect failed: {e}"))?;
    let (_write, mut read) = ws.0.split();

    info!(target: "vertexa", "connected to Binance WS price feed");

    while let Some(msg) = read.next().await {
        let msg = msg.map_err(|e| eyre::eyre!("WS read error: {e}"))?;

        if let Message::Text(text) = msg {
            let parsed: Value = serde_json::from_str(&text)?;

            if let Some(kline) = parsed.get("k") {
                let close: f64 = kline.get("c")
                    .and_then(|v| v.as_str())
                    .and_then(|s| s.parse().ok())
                    .unwrap_or(0.0);

                let volume: f64 = kline.get("v")
                    .and_then(|v| v.as_str())
                    .and_then(|s| s.parse().ok())
                    .unwrap_or(0.0);

                let is_final = kline.get("x")
                    .and_then(|v| v.as_bool())
                    .unwrap_or(false);

                if close > 0.0 {
                    let mut series = shared.write().await;
                    series.push(close, volume);

                    if is_final {
                        info!(
                            target: "vertexa",
                            price = close,
                            volume = volume,
                            candles = series.closes.len(),
                            "kline closed"
                        );
                    }
                }
            }
        }
    }

    Ok(())
}

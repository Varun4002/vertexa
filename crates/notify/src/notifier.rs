use tracing::info;
use vertexa_core::{FixedUsd, Vote};

#[derive(Debug, Clone)]
pub struct TradeNotification {
    pub action: Vote,
    pub amount: FixedUsd,
    pub route: String,
    pub risk_score: f64,
    pub sandwich_probability: f64,
    pub tx_hash: Option<String>,
    pub success: bool,
    pub error: Option<String>,
    pub regime: Option<String>,
    pub block_number: Option<u64>,
}

#[derive(Debug, Clone)]
pub struct Notifier {
    discord_webhook_url: Option<String>,
}

impl Notifier {
    pub fn new(discord_webhook_url: Option<String>) -> Self {
        Self { discord_webhook_url }
    }

    pub fn notify(&self, notification: &TradeNotification) {
        info!(
            target: "vertexa",
            action = ?notification.action,
            amount = %notification.amount,
            route = %notification.route,
            risk = notification.risk_score,
            sandwich_prob = notification.sandwich_probability,
            tx_hash = ?notification.tx_hash,
            success = notification.success,
            regime = ?notification.regime,
            "trade notification"
        );

        if let Some(url) = &self.discord_webhook_url {
            let payload = build_discord_payload(notification);
            let url = url.clone();

            tokio::spawn(async move {
                if let Err(e) = send_discord_webhook(&url, &payload).await {
                    info!(
                        target: "vertexa",
                        error = %e,
                        "failed to send discord notification"
                    );
                }
            });
        }
    }
}

fn build_discord_payload(n: &TradeNotification) -> serde_json::Value {
    let color = match (&n.action, n.success) {
        (Vote::Buy, true) => 0x00ff00,
        (Vote::Sell, true) => 0xff0000,
        (_, true) => 0x3498db,
        (_, false) => 0xe67e22,
    };

    let title = if n.success {
        format!("{:?} Executed — {}", n.action, n.amount)
    } else {
        format!("{:?} Failed — {}", n.action, n.amount)
    };

    let risk_str = format!("{:.2}", n.risk_score);
    let sandwich_str = format!("{:.1}%", n.sandwich_probability * 100.0);
    let block_str = n.block_number.map(|b| b.to_string());

    let mut fields: Vec<serde_json::Value> = vec![
        serde_json::json!({"name": "Route", "value": n.route, "inline": true}),
        serde_json::json!({"name": "Risk Score", "value": risk_str, "inline": true}),
        serde_json::json!({"name": "Sandwich Prob", "value": sandwich_str, "inline": true}),
    ];

    if let Some(ref hash) = n.tx_hash {
        fields.push(serde_json::json!({"name": "Tx Hash", "value": hash, "inline": false}));
    }
    if let Some(ref regime) = n.regime {
        fields.push(serde_json::json!({"name": "Regime", "value": regime, "inline": true}));
    }
    if let Some(ref err) = n.error {
        fields.push(serde_json::json!({"name": "Error", "value": err, "inline": false}));
    }
    if let Some(ref bn_str) = block_str {
        fields.push(serde_json::json!({"name": "Block", "value": bn_str, "inline": true}));
    }

    serde_json::json!({
        "embeds": [{
            "title": title,
            "color": color,
            "timestamp": chrono::Utc::now().to_rfc3339(),
            "fields": fields,
        }]
    })
}

async fn send_discord_webhook(url: &str, payload: &serde_json::Value) -> Result<(), String> {
    let client = reqwest::Client::builder()
        .timeout(std::time::Duration::from_secs(10))
        .build()
        .map_err(|e| format!("failed to build client: {e}"))?;

    let resp = client
        .post(url)
        .json(payload)
        .send()
        .await
        .map_err(|e| format!("webhook request failed: {e}"))?;

    if !resp.status().is_success() {
        let status = resp.status();
        let body = resp.text().await.unwrap_or_default();
        return Err(format!("discord returned {status}: {body}"));
    }

    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use vertexa_core::FixedUsd;

    #[test]
    fn test_build_discord_payload() {
        let n = TradeNotification {
            action: Vote::Buy,
            amount: FixedUsd::from_dollars(5000.0),
            route: "FlashbotsBundle".into(),
            risk_score: 0.25,
            sandwich_probability: 0.15,
            tx_hash: Some("0xabc".into()),
            success: true,
            error: None,
            regime: Some("Trending".into()),
            block_number: Some(123456),
        };

        let payload = build_discord_payload(&n);
        assert_eq!(payload["embeds"][0]["title"].as_str().unwrap(), "Buy Executed — $5000.00");
        assert_eq!(payload["embeds"][0]["color"], 0x00ff00);
        assert_eq!(payload["embeds"][0]["fields"][0]["name"], "Route");
    }

    #[test]
    fn test_failed_notification_embed() {
        let n = TradeNotification {
            action: Vote::Sell,
            amount: FixedUsd::from_dollars(1000.0),
            route: "PublicMempool".into(),
            risk_score: 0.6,
            sandwich_probability: 0.45,
            tx_hash: None,
            success: false,
            error: Some("insufficient liquidity".into()),
            regime: None,
            block_number: None,
        };

        let payload = build_discord_payload(&n);
        assert_eq!(payload["embeds"][0]["title"].as_str().unwrap(), "Sell Failed — $1000.00");
        assert_eq!(payload["embeds"][0]["color"], 0xe67e22);
    }
}

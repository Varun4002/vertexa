use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;
use chrono::{Utc, DateTime};
use tokio::time::{interval, Duration};
use tracing::{info, warn};

use vertexa_core::{PlannedTrade, MevThreatAssessment, ExecutionRoute};

pub struct RiskChecker {
    max_trade_usd: f64,
    max_position_usd: f64,
    daily_loss_limit_usd: f64,
    current_position: AtomicU64,
    daily_loss: AtomicU64,
    circuit_breaker_active: AtomicU64,
    last_reset: Arc<std::sync::Mutex<DateTime<Utc>>>,
}

impl RiskChecker {
    pub fn new(config: &RiskConfig) -> Self {
        Self {
            max_trade_usd: config.max_trade_usd,
            max_position_usd: config.max_position_usd,
            daily_loss_limit_usd: config.daily_loss_limit_usd,
            current_position: AtomicU64::new(0),
            daily_loss: AtomicU64::new(0),
            circuit_breaker_active: AtomicU64::new(0),
            last_reset: Arc::new(std::sync::Mutex::new(Utc::now())),
        }
    }

    pub fn check(
        &self,
        trade: &PlannedTrade,
        assessment: &MevThreatAssessment,
    ) -> Result<(), String> {
        if self.circuit_breaker_active.load(Ordering::SeqCst) == 1 {
            return Err("circuit breaker active — trading halted".into());
        }

        let max_safe_slippage = match assessment.recommended_route {
            ExecutionRoute::PublicMempool => 0.005,
            ExecutionRoute::FlashbotsBundle => 0.002,
            ExecutionRoute::SplitExecution => 0.001,
            ExecutionRoute::Abort => return Err("MEV guard aborted this trade".into()),
        };

        if trade.max_slippage > max_safe_slippage {
            return Err(format!(
                "slippage {:.3} exceeds max safe {:.3}",
                trade.max_slippage, max_safe_slippage
            ));
        }

        if trade.amount_usd > self.max_trade_usd {
            return Err(format!(
                "trade ${:.2} exceeds max trade ${:.2}",
                trade.amount_usd, self.max_trade_usd
            ));
        }

        let current_pos = self.current_position.load(Ordering::SeqCst) as f64 / 100.0;
        let new_pos = current_pos + trade.amount_usd;
        if new_pos > self.max_position_usd {
            return Err(format!(
                "position ${:.2} would exceed max position ${:.2}",
                new_pos, self.max_position_usd
            ));
        }

        Ok(())
    }

    pub fn record_trade(&self, amount_usd: f64, realized_pnl: f64) {
        self.current_position.fetch_add((amount_usd * 100.0) as u64, Ordering::SeqCst);

        if realized_pnl < 0.0 {
            let loss_cents = (-realized_pnl * 100.0) as u64;
            let new_loss = self.daily_loss.fetch_add(loss_cents, Ordering::SeqCst);

            if (new_loss + loss_cents) as f64 / 100.0 >= self.daily_loss_limit_usd {
                self.circuit_breaker_active.store(1, Ordering::SeqCst);
                let current_loss = (new_loss + loss_cents) as f64 / 100.0;
                warn!(
                    target: "vertexa",
                    daily_loss = current_loss,
                    limit = self.daily_loss_limit_usd,
                    "CIRCUIT BREAKER TRIGGERED — halting all trades"
                );
            }
        }
    }

    pub fn start_midnight_reset(self: &Arc<Self>) {
        let checker = self.clone();
        tokio::spawn(async move {
            let mut ticker = interval(Duration::from_secs(60));
            loop {
                ticker.tick().await;
                let now = Utc::now();
                let should_reset = {
                    let last = checker.last_reset.lock().unwrap();
                    now.date_naive() != last.date_naive()
                };

                if should_reset {
                    checker.daily_loss.store(0, Ordering::SeqCst);
                    checker.circuit_breaker_active.store(0, Ordering::SeqCst);
                    *checker.last_reset.lock().unwrap() = now;
                    info!(target: "vertexa", "daily loss counter reset at midnight");
                }
            }
        });
    }
}

pub struct RiskConfig {
    pub max_trade_usd: f64,
    pub max_position_usd: f64,
    pub daily_loss_limit_usd: f64,
}

#[cfg(test)]
mod tests {
    use super::*;
    use vertexa_core::{Vote, MevThreatAssessment, ExecutionRoute};
    use alloy::primitives::address;

    fn make_trade(amount_usd: f64, slippage: f64) -> PlannedTrade {
        PlannedTrade {
            action: Vote::Buy,
            token_in: address!("0000000000000000000000000000000000000001"),
            token_out: address!("0000000000000000000000000000000000000002"),
            amount_in: Default::default(),
            amount_usd,
            max_slippage: slippage,
            pool_fee: 3000,
        }
    }

    fn make_assessment(route: ExecutionRoute) -> MevThreatAssessment {
        MevThreatAssessment {
            risk_score: 0.3,
            sandwich_probability: 0.3,
            recommended_route: route,
            estimated_mev_loss_usd: 0.0,
        }
    }

    #[test]
    fn test_risk_check_passes() {
        let config = RiskConfig {
            max_trade_usd: 10_000.0,
            max_position_usd: 50_000.0,
            daily_loss_limit_usd: 500.0,
        };
        let checker = RiskChecker::new(&config);
        let trade = make_trade(5000.0, 0.005);
        let assessment = make_assessment(ExecutionRoute::PublicMempool);
        assert!(checker.check(&trade, &assessment).is_ok());
    }

    #[test]
    fn test_risk_check_blocks_over_max_trade() {
        let config = RiskConfig {
            max_trade_usd: 10_000.0,
            max_position_usd: 50_000.0,
            daily_loss_limit_usd: 500.0,
        };
        let checker = RiskChecker::new(&config);
        let trade = make_trade(20_000.0, 0.005);
        let assessment = make_assessment(ExecutionRoute::PublicMempool);
        assert!(checker.check(&trade, &assessment).is_err());
    }

    #[test]
    fn test_circuit_breaker_halts() {
        let config = RiskConfig {
            max_trade_usd: 10_000.0,
            max_position_usd: 50_000.0,
            daily_loss_limit_usd: 500.0,
        };
        let checker = RiskChecker::new(&config);
        checker.circuit_breaker_active.store(1, std::sync::atomic::Ordering::SeqCst);
        let trade = make_trade(1000.0, 0.005);
        let assessment = make_assessment(ExecutionRoute::PublicMempool);
        assert!(checker.check(&trade, &assessment).is_err());
    }
}

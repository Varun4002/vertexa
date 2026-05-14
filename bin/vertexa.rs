use std::sync::Arc;
use std::time::Duration;
use tokio::sync::{RwLock, mpsc, oneshot};
use tokio::signal;
use tracing::{info, warn, error};
use tracing_subscriber::EnvFilter;

use alloy::primitives::{Address, B256};

use vertexa_core::{
    Decision, ExecutionResult, MarketContext, PlannedTrade, PriceSeries,
    Signal, Vote, ExecutionRoute, FixedUsd,
};
use vertexa_ingestion::{price_feed, pool_reader, pool_events, gas_estimator, event_logger,
    context_builder::ContextBuilder};
use vertexa_signals::{RsiSignal, EmaCrossoverSignal, OrderBookSignal, OnchainFlowSignal};
use vertexa_consensus::ConsensusEngine;
use vertexa_mev_guard::{MempoolMonitor, FlashbotsRelay, SplitExecutor, MevGuard};
use vertexa_risk::{RiskChecker, RiskConfig};
use vertexa_executor::{SwapBuilder, TxSigner, Broadcaster, Simulator, ExecutorActor, ExecCommand};
use vertexa_notify::{Notifier, TradeNotification};
use vertexa_rag::RagClient;

const STATE_FILE: &str = "vertexa_state.json";

#[derive(serde::Serialize, serde::Deserialize, Default)]
struct PersistentState {
    daily_loss_cents: u64,
    circuit_breaker_active: bool,
    current_position_cents: u64,
    last_updated: String,
}

#[derive(serde::Deserialize, Debug)]
struct AppConfig {
    network: NetworkConfig,
    trading: TradingConfig,
    risk: RiskTomlConfig,
    mev: MevConfig,
    consensus: ConsensusConfig,
    notify: Option<NotifyConfig>,
    reputation: Option<std::collections::HashMap<String, f64>>,
}

#[derive(serde::Deserialize, Debug)]
struct NetworkConfig {
    rpc_http: String,
    rpc_ws: String,
    #[allow(dead_code)]
    chain_id: u64,
}

#[derive(serde::Deserialize, Debug)]
struct TradingConfig {
    pair: String,
    pool_address: String,
    pool_fee_tier: u32,
    max_trade_usd: f64,
    min_trade_usd: f64,
    max_position_usd: f64,
    loop_interval_s: u64,
}

#[derive(serde::Deserialize, Debug)]
struct RiskTomlConfig {
    daily_loss_limit_usd: f64,
    max_slippage_public: f64,
    max_slippage_flashbots: f64,
    #[allow(dead_code)]
    max_slippage_split: f64,
    min_chunk_usd: f64,
}

#[derive(serde::Deserialize, Debug)]
#[allow(dead_code)]
struct MevConfig {
    flashbots_relay_url: String,
    known_bot_addresses: Vec<String>,
    mempool_buffer_size: usize,
}

#[derive(serde::Deserialize, Debug)]
struct ConsensusConfig {
    required_votes: usize,
    min_confidence: f64,
}

#[derive(serde::Deserialize, Debug)]
struct NotifyConfig {
    discord_webhook_url: String,
}

#[tokio::main]
async fn main() -> eyre::Result<()> {
    let env_filter = EnvFilter::try_from_default_env()
        .unwrap_or_else(|_| EnvFilter::new("info"));

    tracing_subscriber::fmt()
        .with_env_filter(env_filter)
        .json()
        .with_target(true)
        .init();

    info!(
        target: "vertexa",
        version = env!("CARGO_PKG_VERSION"),
        "VERTEXA — Autonomous DEX Trading Bot"
    );

    println!(
"█████╗ ███████╗██████╗ ████████╗███████╗██╗  ██╗ █████╗
██╔══██╗██╔════╝██╔══██╗╚══██╔══╝██╔════╝╚██╗██╔╝██╔══██╗
███████║█████╗  ██████╔╝   ██║   █████╗   ╚███╔╝ ███████║
██╔══██║██╔══╝  ██╔══██╗   ██║   ██╔══╝   ██╔██╗ ██║  ██║
██║  ██║███████╗██║  ██║   ██║   ███████╗██╔╝ ██╗██║  ██║
╚═╝  ╚═╝╚══════╝╚═╝  ╚═╝   ╚═╝   ╚══════╝╚═╝  ╚═╝╚═╝  ╚═╝");

    dotenvy::dotenv().ok();

    let config = config::Config::builder()
        .add_source(config::File::with_name("config/default"))
        .build()
        .map_err(|e| eyre::eyre!("config load failed: {e}"))?;

    let app_cfg: AppConfig = config.try_deserialize()
        .map_err(|e| eyre::eyre!("config parse failed: {e}"))?;

    let rpc_ws = std::env::var("VERTEXA_RPC_WS")
        .unwrap_or_else(|_| app_cfg.network.rpc_ws.clone());

    let is_paper = std::env::var("VERTEXA_PAPER").unwrap_or_default() == "true";

    if is_paper {
        info!(target: "vertexa", "PAPER TRADE MODE — no real transactions will be submitted");
    }

    let pool_address: Address = app_cfg.trading.pool_address.parse()
        .map_err(|e| eyre::eyre!("invalid pool address: {e}"))?;

    let signer = TxSigner::from_env()
        .map_err(|e| eyre::eyre!("signer init failed: {e}"))?;

    let price_series = Arc::new(RwLock::new(PriceSeries::new(100)));
    let pool_price = Arc::new(RwLock::new(0.0_f64));
    let pool_liquidity = Arc::new(RwLock::new(0.0_f64));
    let tick_liquidity = Arc::new(RwLock::new(vertexa_core::TickLiquidity::default()));
    let block_number = Arc::new(RwLock::new(0u64));
    let macro_regime = Arc::new(RwLock::new(None));
    let pool_reset_signal = Arc::new(tokio::sync::Notify::new());

    price_feed::start(price_series.clone()).await;

    pool_reader::start(
        pool_price.clone(),
        pool_liquidity.clone(),
        block_number.clone(),
        &app_cfg.network.rpc_http,
        pool_reset_signal.clone(),
    ).await;

    pool_events::start(
        pool_price.clone(),
        pool_liquidity.clone(),
        tick_liquidity.clone(),
        pool_address,
        &rpc_ws,
        pool_reset_signal.clone(),
    ).await;

    let mempool_monitor = MempoolMonitor::new(&app_cfg.mev.known_bot_addresses);
    let pending_txs = mempool_monitor.pending_txs();

    mempool_monitor.start_monitor(&rpc_ws).await;

    let context_builder = ContextBuilder::new(
        price_series,
        pool_price.clone(),
        pool_liquidity.clone(),
        tick_liquidity.clone(),
        pending_txs.clone(),
        block_number.clone(),
        macro_regime.clone(),
    );

    // RAG Pipeline Task
    let rag_client = RagClient::new(
        "http://localhost:6333", // Default Qdrant URL
        "market_regimes",
        "https://api.news.com",
    ).expect("failed to init RAG client");

    let macro_regime_clone = macro_regime.clone();
    tokio::spawn(async move {
        let mut interval = tokio::time::interval(Duration::from_secs(300)); // Every 5 mins
        loop {
            interval.tick().await;
            if let Err(e) = rag_client.update_macro_regime(macro_regime_clone.clone()).await {
                error!(target: "vertexa", error = %e, "RAG update failed");
            }
        }
    });

    let mut reputation_weights = std::collections::HashMap::new();
    if let Some(rep) = &app_cfg.reputation {
        for (addr_str, weight) in rep {
            if let Ok(addr) = addr_str.parse::<Address>() {
                reputation_weights.insert(addr, *weight);
            }
        }
    }

    let signals: Vec<Box<dyn Signal>> = vec![
        Box::new(RsiSignal::new()),
        Box::new(EmaCrossoverSignal::new()),
        Box::new(OrderBookSignal::new()),
        Box::new(OnchainFlowSignal::new(reputation_weights)),
    ];

    let engine = ConsensusEngine::new(
        signals,
        app_cfg.consensus.required_votes,
        app_cfg.consensus.min_confidence,
    );

    let mev_guard = MevGuard::new(
        pending_txs.clone(),
        mempool_monitor.known_bots().clone(),
        &app_cfg.mev.flashbots_relay_url,
        app_cfg.risk.min_chunk_usd,
    );

    let risk_config = RiskConfig {
        max_trade_usd: app_cfg.trading.max_trade_usd,
        max_position_usd: app_cfg.trading.max_position_usd,
        daily_loss_limit_usd: app_cfg.risk.daily_loss_limit_usd,
    };

    let persisted = load_state();
    let (daily_loss_cents, circuit_breaker, position_cents) = match persisted {
        Some(s) => (s.daily_loss_cents, s.circuit_breaker_active, s.current_position_cents),
        None => (0, false, 0),
    };
    let risk_checker = Arc::new(RiskChecker::new_with_state(
        &risk_config,
        daily_loss_cents,
        circuit_breaker,
        position_cents,
    ));
    risk_checker.start_midnight_reset();

    let notifier = Notifier::new(
        app_cfg.notify.as_ref().map(|n| n.discord_webhook_url.clone()),
    );

    let swap_builder = SwapBuilder::new(signer.address());

    let flashbots_relay = FlashbotsRelay::new(&app_cfg.mev.flashbots_relay_url);
    let split_executor = SplitExecutor::new(app_cfg.risk.min_chunk_usd);
    let broadcaster = Broadcaster::new(
        &app_cfg.network.rpc_http,
        flashbots_relay,
        split_executor,
    );

    let gas_estimator = gas_estimator::GasEstimator::new(
        &app_cfg.network.rpc_http,
        pool_price.clone(),
    );

    let event_log = event_logger::EventLogger::new(
        std::path::PathBuf::from("data"),
    ).await;

    let simulator = Simulator::new(
        &app_cfg.network.rpc_http,
        Some(app_cfg.mev.flashbots_relay_url.clone()),
    );

    let (actor_tx, actor_rx) = mpsc::channel::<ExecCommand>(16);
    ExecutorActor::spawn(
        actor_rx,
        &app_cfg.network.rpc_http,
        signer.clone_signer(),
        is_paper,
    );

    let loop_interval = Duration::from_secs(app_cfg.trading.loop_interval_s);

    let use_actor: bool = true;

    info!(
        target: "vertexa",
        pair = %app_cfg.trading.pair,
        pool = %app_cfg.trading.pool_address,
        interval_s = app_cfg.trading.loop_interval_s,
        signals = 4,
        "main loop starting"
    );

    let shutdown = Arc::new(tokio::sync::Notify::new());
    let shutdown_clone = shutdown.clone();

    tokio::spawn(async move {
        let mut sigterm = tokio::signal::unix::signal(
            tokio::signal::unix::SignalKind::terminate(),
        ).expect("failed to register SIGTERM handler");

        tokio::select! {
            _ = signal::ctrl_c() => {
                info!(target: "vertexa", "SIGINT received — initiating graceful shutdown");
            }
            _ = sigterm.recv() => {
                info!(target: "vertexa", "SIGTERM received — initiating graceful shutdown");
            }
        }

        shutdown_clone.notify_one();
    });

    loop {
        tokio::select! {
            _ = shutdown.notified() => {
                info!(target: "vertexa", "shutting down gracefully...");
                let state = PersistentState {
                    daily_loss_cents: risk_checker.daily_loss(),
                    circuit_breaker_active: risk_checker.is_circuit_breaker_active(),
                    current_position_cents: risk_checker.current_position_cents(),
                    last_updated: chrono::Utc::now().to_rfc3339(),
                };
                save_state(&state);
                info!(target: "vertexa", "state persisted — goodbye");
                break;
            }
            _ = tokio::time::sleep(loop_interval) => {
                run_loop_iteration(
                    &context_builder,
                    &engine,
                    &mev_guard,
                    &risk_checker,
                    &swap_builder,
                    &broadcaster,
                    &notifier,
                    &app_cfg,
                    pool_address,
                    is_paper,
                    &gas_estimator,
                    &event_log,
                    &simulator,
                    &actor_tx,
                    use_actor,
                ).await;
            }
        }
    }

    Ok(())
}

#[allow(clippy::too_many_arguments)]
async fn run_loop_iteration(
    context_builder: &ContextBuilder,
    engine: &ConsensusEngine,
    mev_guard: &MevGuard,
    risk_checker: &RiskChecker,
    swap_builder: &SwapBuilder,
    broadcaster: &Broadcaster,
    notifier: &Notifier,
    cfg: &AppConfig,
    pool_address: Address,
    is_paper: bool,
    gas_estimator: &gas_estimator::GasEstimator,
    event_log: &event_logger::EventLogger,
    simulator: &Simulator,
    actor_tx: &mpsc::Sender<ExecCommand>,
    use_actor: bool,
) {
    let ctx = match context_builder.build(&cfg.trading.pair, pool_address).await {
        Ok(ctx) => ctx,
        Err(e) => {
            warn!(target: "vertexa", error = %e, "failed to build market context");
            return;
        }
    };

    let decision = engine.evaluate(&ctx).await;

    let log_regime = if let Some(blocked_by) = decision.blocked_by {
        if blocked_by.contains("Ranging") {
            "Ranging"
        } else {
            "Blocked"
        }
    } else if decision.size_multiplier >= 1.0 {
        "Trending"
    } else {
        "Volatile"
    };

    if decision.action == Vote::Neutral {
        event_log.log(
            ctx.block_number, ctx.current_price, log_regime,
            &decision.action, decision.avg_confidence, decision.size_multiplier,
            false, "", &None, &None,
            FixedUsd::from_dollars(0.0), FixedUsd::from_dollars(0.0),
            FixedUsd::from_dollars(0.0), 0.0,
        ).await;
        return;
    }

    let adjusted_usd = cfg.trading.max_trade_usd * decision.size_multiplier;

    if adjusted_usd < cfg.trading.min_trade_usd {
        warn!(
            target: "vertexa",
            adjusted_usd,
            min_trade_usd = cfg.trading.min_trade_usd,
            "adjusted trade size too small, skipping"
        );
        event_log.log(
            ctx.block_number, ctx.current_price, log_regime,
            &decision.action, decision.avg_confidence, decision.size_multiplier,
            false, "TooSmall", &None, &Some(format!("adjusted size ${:.2} under minimum ${:.2}", adjusted_usd, cfg.trading.min_trade_usd)),
            FixedUsd::from_dollars(0.0), FixedUsd::from_dollars(0.0),
            FixedUsd::from_dollars(0.0), 0.0,
        ).await;
        return;
    }

    let trade = build_planned_trade(&decision, &ctx, cfg, adjusted_usd);
    let assessment = mev_guard.assess(&trade, ctx.pool_liquidity).await;

    if assessment.recommended_route == ExecutionRoute::Abort {
        warn!(
            target: "vertexa",
            risk_score = assessment.risk_score,
            reason = "MEV guard aborted",
            "TRADE ABORTED"
        );
        notifier.notify(&TradeNotification {
            action: decision.action.clone(),
            amount: FixedUsd::from_dollars(adjusted_usd),
            route: "Abort".into(),
            risk_score: assessment.risk_score,
            sandwich_probability: assessment.sandwich_probability,
            tx_hash: None,
            success: false,
            error: Some(format!("MEV guard aborted (risk {:.2})", assessment.risk_score)),
            regime: Some(log_regime.into()),
            block_number: Some(ctx.block_number),
        });
        event_log.log(
            ctx.block_number, ctx.current_price, log_regime,
            &decision.action, decision.avg_confidence, decision.size_multiplier,
            false, "MEVAbort", &None, &Some("MEV guard aborted".into()),
            FixedUsd::from_dollars(0.0), FixedUsd::from_dollars(assessment.estimated_mev_loss_usd),
            FixedUsd::from_dollars(0.0), 0.0,
        ).await;
        return;
    }

    let tx = match swap_builder.build_tx(&trade, &cfg.network.rpc_http).await {
        Ok(tx) => tx,
        Err(e) => {
            warn!(target: "vertexa", error = %e, "failed to build swap tx");
            event_log.log(
                ctx.block_number, ctx.current_price, log_regime,
                &decision.action, decision.avg_confidence, decision.size_multiplier,
                false, "BuildError", &None, &Some(e.clone()),
                FixedUsd::from_dollars(0.0), FixedUsd::from_dollars(0.0),
                FixedUsd::from_dollars(0.0), 0.0,
            ).await;
            return;
        }
    };

    let sim_result = match simulator.simulate(&tx).await {
        Ok(res) => res,
        Err(e) => {
            warn!(target: "vertexa", error = %e, "simulation failed — aborting trade");
            notifier.notify(&TradeNotification {
                action: decision.action.clone(),
                amount: FixedUsd::from_dollars(adjusted_usd),
                route: "SimulationGate".into(),
                risk_score: assessment.risk_score,
                sandwich_probability: assessment.sandwich_probability,
                tx_hash: None,
                success: false,
                error: Some(e.clone()),
                regime: Some(log_regime.into()),
                block_number: Some(ctx.block_number),
            });
            event_log.log(
                ctx.block_number, ctx.current_price, log_regime,
                &decision.action, decision.avg_confidence, decision.size_multiplier,
                false, "SimulationGate", &None, &Some(e),
                FixedUsd::from_dollars(0.0), FixedUsd::from_dollars(0.0),
                FixedUsd::from_dollars(0.0), 0.0,
            ).await;
            return;
        }
    };

    let gas_cost = FixedUsd::from_dollars(sim_result.gas_used as f64 * ctx.current_price / 1e18);
    let mev_cost = FixedUsd::from_dollars(assessment.estimated_mev_loss_usd);

    let cost_breakdown = match risk_checker.check_profitability(
        &trade,
        decision.avg_confidence,
        mev_cost,
        gas_cost,
        Some(sim_result.confidence),
    ) {
        Ok(b) => b,
        Err(e) => {
            warn!(
                target: "vertexa",
                error = %e,
                "profitability check failed — skipping trade"
            );
            notifier.notify(&TradeNotification {
                action: decision.action.clone(),
                amount: FixedUsd::from_dollars(adjusted_usd),
                route: "ProfitabilityGate".into(),
                risk_score: assessment.risk_score,
                sandwich_probability: assessment.sandwich_probability,
                tx_hash: None,
                success: false,
                error: Some(e.to_string()),
                regime: Some(log_regime.into()),
                block_number: Some(ctx.block_number),
            });
            let edge_pct = 0.005 + (decision.avg_confidence * 0.01);
            let expected_edge = FixedUsd::from_dollars(trade.amount_usd * edge_pct);
            let slippage_cost = FixedUsd::from_dollars(trade.amount_usd * trade.max_slippage);
            let total = gas_cost + slippage_cost + mev_cost;
            let ratio = if total.to_dollars() <= 0.0 { 0.0 } else { expected_edge.to_dollars() / total.to_dollars() };
            event_log.log(
                ctx.block_number, ctx.current_price, log_regime,
                &decision.action, decision.avg_confidence, decision.size_multiplier,
                false, "ProfitabilityGate", &None, &Some(e.to_string()),
                gas_cost, mev_cost,
                expected_edge, ratio,
            ).await;
            return;
        }
    };

    if let Err(err) = risk_checker.check(&trade, &assessment, ctx.macro_regime.as_ref()) {
        let err_str = err;
        warn!(
            target: "vertexa",
            error = %err_str,
            "risk check failed — skipping trade"
        );
        notifier.notify(&TradeNotification {
            action: decision.action.clone(),
            amount: FixedUsd::from_dollars(adjusted_usd),
            route: "RiskGate".into(),
            risk_score: assessment.risk_score,
            sandwich_probability: assessment.sandwich_probability,
            tx_hash: None,
            success: false,
            error: Some(err_str.clone()),
            regime: Some(log_regime.into()),
            block_number: Some(ctx.block_number),
        });
        event_log.log(
            ctx.block_number, ctx.current_price, log_regime,
            &decision.action, decision.avg_confidence, decision.size_multiplier,
            false, "RiskGate", &None, &Some(err_str),
            gas_cost, mev_cost,
            cost_breakdown.expected_edge_usd,
            cost_breakdown.reward_to_cost,
        ).await;
        return;
    }

    info!(target: "vertexa", amount_out = %sim_result.amount_out, confidence = ?sim_result.confidence, "simulation passed");

    let route_str = format!("{:?}", assessment.recommended_route);

    // Item 8: ExecutorActor Fee Cap calculation
    // max_gas_price = (expected_edge_usd - total_other_costs) / gas_units * eth_price
    let slippage_cost_usd = trade.amount_usd * trade.max_slippage;
    let other_costs_usd = slippage_cost_usd + mev_cost.to_dollars();
    let edge_usd = cost_breakdown.expected_edge_usd.to_dollars();
    
    let max_gas_budget_usd = edge_usd - other_costs_usd;
    let max_gas_price_wei = if max_gas_budget_usd > 0.0 {
        Some((max_gas_budget_usd * 1e18 / (sim_result.gas_used as f64 * ctx.current_price)) as u128)
    } else {
        None
    };

    let (executed, result) = if assessment.recommended_route == ExecutionRoute::PublicMempool && use_actor {
        let (resp_tx, resp_rx) = oneshot::channel();
        match actor_tx.send((trade.clone(), tx.clone(), resp_tx, max_gas_price_wei)).await {
            Ok(()) => {
                match resp_rx.await {
                    Ok(res) => (res.success, res),
                    Err(e) => {
                        let res = ExecutionResult {
                            success: false,
                            tx_hash: None,
                            gas_used: None,
                            actual_amount_out: None,
                            error: Some(format!("oneshot cancelled: {e}")),
                        };
                        (false, res)
                    }
                }
            }
            Err(e) => {
                let res = ExecutionResult {
                    success: false,
                    tx_hash: None,
                    gas_used: None,
                    actual_amount_out: None,
                    error: Some(format!("actor send failed: {e}")),
                };
                (false, res)
            }
        }
    } else {
        match broadcaster.broadcast(&tx, &TxSigner::from_env().unwrap(), &trade, &assessment, is_paper).await {
            Ok(hash) => {
                let res = ExecutionResult {
                    success: true,
                    tx_hash: hash.parse::<B256>().ok(),
                    gas_used: None,
                    actual_amount_out: None, // Broadcaster doesn't fetch receipts yet
                    error: None,
                };
                (true, res)
            },
            Err(e) => {
                let res = ExecutionResult {
                    success: false,
                    tx_hash: None,
                    gas_used: None,
                    actual_amount_out: None,
                    error: Some(e),
                };
                (false, res)
            },
        }
    };

    if executed {
        info!(
            target: "vertexa",
            tx_hash = ?result.tx_hash,
            action = ?decision.action,
            amount_usd = trade.amount_usd,
            route = ?assessment.recommended_route,
            "trade executed"
        );

        // Item 7: Post-Execution Slippage Kill-Switch
        if let Err(err) = risk_checker.check_slippage(&trade, &result) {
            error!(target: "vertexa", error = %err, "slippage check failed after execution");
            notifier.notify(&TradeNotification {
                action: decision.action.clone(),
                amount: FixedUsd::from_dollars(adjusted_usd),
                route: "SlippageGate".into(),
                risk_score: assessment.risk_score,
                sandwich_probability: assessment.sandwich_probability,
                tx_hash: result.tx_hash.as_ref().map(|h| format!("{:#x}", h)),
                success: false,
                error: Some(err),
                regime: Some(log_regime.into()),
                block_number: Some(ctx.block_number),
            });
            // We still record the trade size for position tracking, but PnL might be hit
            risk_checker.record_trade(trade.amount_usd, 0.0);
            return;
        }

        risk_checker.record_trade(trade.amount_usd, 0.0);

        let notif = TradeNotification {
            action: decision.action.clone(),
            amount: FixedUsd::from_dollars(adjusted_usd),
            route: route_str.clone(),
            risk_score: assessment.risk_score,
            sandwich_probability: assessment.sandwich_probability,
            tx_hash: result.tx_hash.as_ref().map(|h| format!("{:#x}", h)),
            success: true,
            error: None,
            regime: Some(log_regime.into()),
            block_number: Some(ctx.block_number),
        };
        notifier.notify(&notif);
    } else {
        let err_msg = result.error.clone().unwrap_or_default();
        error!(target: "vertexa", error = %err_msg, "trade execution failed");
        notifier.notify(&TradeNotification {
            action: decision.action.clone(),
            amount: FixedUsd::from_dollars(adjusted_usd),
            route: route_str.clone(),
            risk_score: assessment.risk_score,
            sandwich_probability: assessment.sandwich_probability,
            tx_hash: None,
            success: false,
            error: result.error.clone(),
            regime: Some(log_regime.into()),
            block_number: Some(ctx.block_number),
        });
    }

    event_log.log(
        ctx.block_number, ctx.current_price, log_regime,
        &decision.action, decision.avg_confidence, decision.size_multiplier,
        executed, &route_str, &result.tx_hash.as_ref().map(|h| format!("{:#x}", h)), &result.error,
        gas_cost, mev_cost,
        cost_breakdown.expected_edge_usd,
        cost_breakdown.reward_to_cost,
    ).await;
}

fn build_planned_trade(
    decision: &Decision,
    ctx: &MarketContext,
    cfg: &AppConfig,
    adjusted_usd: f64,
) -> PlannedTrade {
    let amount_usd = adjusted_usd;
    let amount_in_wei = (amount_usd * 1e18 / ctx.current_price) as u128;

    let max_slippage = if decision.avg_confidence > 0.7 {
        cfg.risk.max_slippage_flashbots
    } else {
        cfg.risk.max_slippage_public
    };

    let (token_in, token_out) = match decision.action {
        Vote::Buy => (vertexa_core::USDC_ADDRESS, vertexa_core::WETH_ADDRESS),
        Vote::Sell => (vertexa_core::WETH_ADDRESS, vertexa_core::USDC_ADDRESS),
        Vote::Neutral => (vertexa_core::USDC_ADDRESS, vertexa_core::WETH_ADDRESS),
    };

    let expected_out_usd = amount_usd * (1.0 - max_slippage);
    let expected_min_amount_out = if decision.action == Vote::Buy {
        // We get WETH (18 decimals)
        alloy::primitives::U256::from((expected_out_usd * 1e18 / ctx.current_price) as u128)
    } else {
        // We get USDC (6 decimals)
        alloy::primitives::U256::from((expected_out_usd * 1e6) as u128)
    };

    PlannedTrade {
        action: decision.action.clone(),
        token_in,
        token_out,
        amount_in: alloy::primitives::U256::from(amount_in_wei),
        expected_min_amount_out,
        amount_usd,
        max_slippage,
        pool_fee: cfg.trading.pool_fee_tier,
    }
}

fn load_state() -> Option<PersistentState> {
    let content = std::fs::read_to_string(STATE_FILE).ok()?;
    let state: PersistentState = serde_json::from_str(&content).ok()?;
    info!(
        target: "vertexa",
        daily_loss_cents = state.daily_loss_cents,
        circuit_breaker = state.circuit_breaker_active,
        position_cents = state.current_position_cents,
        last_updated = %state.last_updated,
        "loaded persistent state"
    );
    Some(state)
}

fn save_state(state: &PersistentState) {
    match serde_json::to_string_pretty(state) {
        Ok(json) => {
            if let Err(e) = std::fs::write(STATE_FILE, &json) {
                warn!(target: "vertexa", error = %e, "failed to save state");
            } else {
                info!(target: "vertexa", "state saved to {STATE_FILE}");
            }
        }
        Err(e) => {
            warn!(target: "vertexa", error = %e, "failed to serialize state");
        }
    }
}

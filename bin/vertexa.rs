use std::collections::HashSet;
use std::sync::Arc;
use std::time::Duration;
use tokio::sync::RwLock;
use std::collections::VecDeque;
use tracing::{info, warn, error};
use tracing_subscriber::EnvFilter;

use alloy::primitives::Address;
use alloy::providers::ProviderBuilder;

use vertexa_core::{
    Decision, MarketContext, MevThreatAssessment, PlannedTrade, PriceSeries,
    Signal, Vote, ExecutionRoute, PendingTx, UNISWAP_V3_ROUTER, FixedUsd, VertexaError,
};
use vertexa_ingestion::{price_feed, pool_reader, context_builder::ContextBuilder};
use vertexa_signals::{RsiSignal, EmaCrossoverSignal, OrderBookSignal, OnchainFlowSignal};
use vertexa_consensus::ConsensusEngine;
use vertexa_mev_guard::{MempoolMonitor, FlashbotsRelay, SplitExecutor, MevGuard};
use vertexa_risk::{RiskChecker, RiskConfig};
use vertexa_executor::{SwapBuilder, TxSigner, Broadcaster};

#[derive(serde::Deserialize, Debug)]
struct AppConfig {
    network: NetworkConfig,
    trading: TradingConfig,
    risk: RiskTomlConfig,
    mev: MevConfig,
    consensus: ConsensusConfig,
}

#[derive(serde::Deserialize, Debug)]
struct NetworkConfig {
    rpc_http: String,
    rpc_ws: String,
    chain_id: u64,
}

#[derive(serde::Deserialize, Debug)]
struct TradingConfig {
    pair: String,
    pool_address: String,
    pool_fee_tier: u32,
    max_trade_usd: f64,
    max_position_usd: f64,
    loop_interval_s: u64,
}

#[derive(serde::Deserialize, Debug)]
struct RiskTomlConfig {
    daily_loss_limit_usd: f64,
    max_slippage_public: f64,
    max_slippage_flashbots: f64,
    max_slippage_split: f64,
    min_chunk_usd: f64,
}

#[derive(serde::Deserialize, Debug)]
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

    println!("{}", r#"
█████╗ ███████╗██████╗ ████████╗███████╗██╗  ██╗ █████╗
██╔══██╗██╔════╝██╔══██╗╚══██╔══╝██╔════╝╚██╗██╔╝██╔══██╗
███████║█████╗  ██████╔╝   ██║   █████╗   ╚███╔╝ ███████║
██╔══██║██╔══╝  ██╔══██╗   ██║   ██╔══╝   ██╔██╗ ██╔══██║
██║  ██║███████╗██║  ██║   ██║   ███████╗██╔╝ ██╗██║  ██║
╚═╝  ╚═╝╚══════╝╚═╝  ╚═╝   ╚═╝   ╚══════╝╚═╝  ╚═╝╚═╝  ╚═╝
    "#);

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

    price_feed::start(price_series.clone()).await;

    pool_reader::start(
        pool_price.clone(),
        pool_liquidity.clone(),
        &app_cfg.network.rpc_http,
    ).await;

    let mempool_monitor = MempoolMonitor::new(&app_cfg.mev.known_bot_addresses);
    let pending_txs = mempool_monitor.pending_txs();

    mempool_monitor.start_monitor(&rpc_ws).await;

    let context_builder = ContextBuilder::new(
        price_series,
        pool_price,
        pool_liquidity,
        pending_txs.clone(),
    );

    let signals: Vec<Box<dyn Signal>> = vec![
        Box::new(RsiSignal::new()),
        Box::new(EmaCrossoverSignal::new()),
        Box::new(OrderBookSignal::new()),
        Box::new(OnchainFlowSignal::new()),
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
    let risk_checker = Arc::new(RiskChecker::new(&risk_config));
    risk_checker.start_midnight_reset();

    let swap_builder = SwapBuilder::new(signer.address());
    let broadcaster = Broadcaster::new(&app_cfg.network.rpc_http);

    let loop_interval = Duration::from_secs(app_cfg.trading.loop_interval_s);

    info!(
        target: "vertexa",
        pair = %app_cfg.trading.pair,
        pool = %app_cfg.trading.pool_address,
        interval_s = app_cfg.trading.loop_interval_s,
        signals = 4,
        "main loop starting"
    );

    loop {
        tokio::time::sleep(loop_interval).await;

        let ctx = match context_builder.build(
            &app_cfg.trading.pair,
            pool_address,
        ).await {
            Ok(ctx) => ctx,
            Err(e) => {
                warn!(target: "vertexa", error = %e, "failed to build market context");
                continue;
            }
        };

        let decision = engine.evaluate(&ctx).await;

        if decision.action == Vote::Neutral {
            continue;
        }

        let adjusted_usd = app_cfg.trading.max_trade_usd * decision.size_multiplier;

        if adjusted_usd < 10.0 {
            warn!(target: "vertexa", "Adjusted trade size too small, skipping");
            continue;
        }

        let trade = build_planned_trade(&decision, &ctx, &app_cfg, adjusted_usd);

        let assessment = mev_guard.assess(&trade, ctx.pool_liquidity).await;

        if assessment.recommended_route == ExecutionRoute::Abort {
            warn!(
                target: "vertexa",
                risk_score = assessment.risk_score,
                reason = "MEV guard aborted",
                "TRADE ABORTED"
            );
            continue;
        }

        if let Err(e) = risk_checker.check_profitability(
            &trade,
            decision.avg_confidence,
            FixedUsd::from_dollars(assessment.estimated_mev_loss_usd),
        ) {
            warn!(
                target: "vertexa",
                error = %e,
                "profitability check failed — skipping trade"
            );
            continue;
        }

        if let Err(e) = risk_checker.check(&trade, &assessment) {
            warn!(
                target: "vertexa",
                error = %e,
                "risk check failed — skipping trade"
            );
            continue;
        }

        let tx = match swap_builder.build_tx(&trade, &app_cfg.network.rpc_http).await {
            Ok(tx) => tx,
            Err(e) => {
                warn!(target: "vertexa", error = %e, "failed to build swap tx");
                continue;
            }
        };

        match broadcaster.broadcast(
            &tx,
            &signer,
            &trade,
            &assessment,
            is_paper,
        ).await {
            Ok(tx_hash) => {
                info!(
                    target: "vertexa",
                    tx_hash = %tx_hash,
                    action = ?decision.action,
                    amount_usd = trade.amount_usd,
                    route = ?assessment.recommended_route,
                    "trade executed"
                );
                risk_checker.record_trade(trade.amount_usd, 0.0);
            }
            Err(e) => {
                error!(
                    target: "vertexa",
                    error = %e,
                    "trade execution failed"
                );
            }
        }
    }
}

fn build_planned_trade(
    decision: &Decision,
    ctx: &MarketContext,
    cfg: &AppConfig,
    adjusted_usd: f64,
) -> PlannedTrade {
    let amount_usd = adjusted_usd.min(1000.0);
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

    PlannedTrade {
        action: decision.action.clone(),
        token_in,
        token_out,
        amount_in: alloy::primitives::U256::from(amount_in_wei),
        amount_usd,
        max_slippage,
        pool_fee: cfg.trading.pool_fee_tier,
    }
}

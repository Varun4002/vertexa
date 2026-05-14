use std::path::PathBuf;
use clap::Parser;
use vertexa_core::{MarketContext, OrderBook, Signal, Vote};
use vertexa_signals::{RsiSignal, EmaCrossoverSignal, OrderBookSignal, OnchainFlowSignal};
use vertexa_consensus::ConsensusEngine;

#[derive(Parser, Debug)]
#[command(name = "backtest_historical", about = "Backtest Vertexa strategy on Binance OHLCV data")]
struct Args {
    #[arg(short, long)]
    csv: PathBuf,

    #[arg(long, default_value = "2")]
    required_votes: usize,

    #[arg(long, default_value = "0.35")]
    min_confidence: f64,

    #[arg(long, default_value_t = 0.001)]
    fee: f64,

    #[arg(long, default_value_t = 10_000.0)]
    capital: f64,
}

#[derive(Debug, serde::Deserialize)]
#[allow(dead_code)]
struct Candle {
    open_time: u64,
    open: f64,
    high: f64,
    low: f64,
    close: f64,
    volume: f64,
    close_time: u64,
    quote_asset_volume: f64,
    num_trades: u64,
    taker_buy_base_vol: f64,
    taker_buy_quote_vol: f64,
    ignore: f64,
}

const WARMUP: usize = 21;
const PRICE_CAP: usize = 100;

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    let args = Args::parse();

    let mut reader = csv::ReaderBuilder::new()
        .has_headers(true)
        .from_path(&args.csv)?;

    let mut candles: Vec<Candle> = Vec::new();
    for result in reader.deserialize() {
        candles.push(result?);
    }

    if candles.len() <= WARMUP {
        eprintln!("Not enough data: need > {} candles, got {}", WARMUP, candles.len());
        std::process::exit(1);
    }

    println!("Loaded {} candles from {}", candles.len(), args.csv.display());
    println!("Warmup:       {} candles", WARMUP);
    println!("Trading:      {} candles", candles.len() - WARMUP);
    println!("Fee:          {:.3}%", args.fee * 100.0);
    println!("Req votes:    {}", args.required_votes);
    println!("Min conf:     {:.2}", args.min_confidence);
    println!("Initial cap:  ${:.2}", args.capital);
    println!();

    let signals: Vec<Box<dyn Signal>> = vec![
        Box::new(RsiSignal::new()),
        Box::new(EmaCrossoverSignal::new()),
        Box::new(OrderBookSignal::new()),
        Box::new(OnchainFlowSignal::new(std::collections::HashMap::new())),
    ];

    let engine = ConsensusEngine::new(signals, args.required_votes, args.min_confidence);

    let fee = args.fee;
    let mut capital = args.capital;
    let mut in_position = false;
    let mut entry_price = 0.0;
    let mut position_qty = 0.0;
    let mut total_trades: usize = 0;
    let mut wins: usize = 0;

    let mut prices: Vec<f64> = Vec::with_capacity(PRICE_CAP);
    let mut equity_curve: Vec<f64> = Vec::with_capacity(candles.len() - WARMUP);
    let mut peak = args.capital;

    for i in 0..WARMUP {
        prices.push(candles[i].close);
    }

    for i in WARMUP..candles.len() {
        let c = &candles[i];
        prices.push(c.close);
        if prices.len() > PRICE_CAP {
            prices.remove(0);
        }

        let ctx = MarketContext {
            pair: "ETH/USDC".into(),
            pool_address: Default::default(),
            prices: prices.clone(),
            volumes: vec![],
            orderbook: OrderBook { bids: vec![], asks: vec![] },
            tick_liquidity: None,
            recent_whale_txs: vec![],
            pool_liquidity: 10_000_000.0,
            current_price: c.close,
            block_number: 0,
            timestamp: std::time::Instant::now(),
            macro_regime: None,
        };

        let decision = engine.evaluate(&ctx).await;

        match decision.action {
            Vote::Buy if !in_position => {
                entry_price = c.close * (1.0 + fee);
                position_qty = capital / entry_price;
                in_position = true;
                total_trades += 1;
            }
            Vote::Sell if in_position => {
                let exit_price = c.close * (1.0 - fee);
                let new_capital = position_qty * exit_price;
                if new_capital > capital { wins += 1; }
                capital = new_capital;
                in_position = false;
            }
            _ => {}
        }

        let equity = if in_position {
            (position_qty * c.close * (1.0 - fee)).max(capital)
        } else {
            capital
        };
        equity_curve.push(equity);
        if equity > peak { peak = equity; }
    }

    if in_position {
        let last = &candles[candles.len() - 1];
        capital = position_qty * last.close * (1.0 - fee);
        in_position = false;
    }

    let total_pnl_pct = (capital - args.capital) / args.capital;
    let losses = total_trades.saturating_sub(wins);
    let win_rate = if total_trades > 0 { wins as f64 / total_trades as f64 * 100.0 } else { 0.0 };

    let mut max_dd = 0.0;
    let mut running_peak = args.capital;
    for &eq in &equity_curve {
        if eq > running_peak { running_peak = eq; }
        let dd = (running_peak - eq) / running_peak;
        if dd > max_dd { max_dd = dd; }
    }

    let sharpe = compute_sharpe(&equity_curve);

    println!("=== Backtest Results ===");
    println!("Total trades:   {}", total_trades);
    println!("Wins:           {}", wins);
    println!("Losses:         {}", losses);
    println!("Win rate:       {:.2}%", win_rate);
    println!("Final capital:  ${:.2}", capital);
    println!("Total PnL:      {:+.2}%", total_pnl_pct * 100.0);
    println!("Max drawdown:   {:.2}%", max_dd * 100.0);
    println!("Sharpe ratio:   {:.3}", sharpe);

    Ok(())
}

fn compute_sharpe(equity: &[f64]) -> f64 {
    if equity.len() < 2 {
        return 0.0;
    }
    let n = (equity.len() - 1) as f64;
    let mean = equity.windows(2)
        .map(|w| (w[1] - w[0]) / w[0])
        .sum::<f64>() / n;
    let variance = equity.windows(2)
        .map(|w| {
            let r = (w[1] - w[0]) / w[0];
            (r - mean).powi(2)
        })
        .sum::<f64>() / (n - 1.0);
    if variance <= 0.0 { return 0.0; }
    (mean / variance.sqrt()) * 525_600_f64.sqrt()
}

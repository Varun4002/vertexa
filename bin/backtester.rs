use std::path::PathBuf;

#[derive(serde::Deserialize, Debug, Clone)]
#[allow(dead_code)]
struct EventRow {
    timestamp: String,
    block: u64,
    price: f64,
    regime: String,
    action: String,
    confidence: f64,
    size_mult: f64,
    executed: bool,
    route: String,
    tx_hash: String,
    error: String,
    gas_usd: f64,
    mev_usd: f64,
    edge_usd: f64,
    reward_to_cost: f64,
}

#[derive(Debug, Default)]
#[allow(dead_code)]
struct TradeRecord {
    buy_timestamp: String,
    buy_price: f64,
    buy_block: u64,
}

#[derive(Debug, Default)]
struct BacktestResult {
    total_trades: usize,
    wins: usize,
    losses: usize,
    total_pnl_pct: f64,
    max_drawdown: f64,
    peak_value: f64,
    cumulative_pnl: Vec<f64>,
}

fn main() {
    let args: Vec<String> = std::env::args().collect();
    if args.len() < 2 {
        eprintln!("usage: vertexa-backtester <events.csv>");
        std::process::exit(1);
    }

    let path = PathBuf::from(&args[1]);
    let content = match std::fs::read_to_string(&path) {
        Ok(c) => c,
        Err(e) => {
            eprintln!("failed to read {}: {e}", path.display());
            std::process::exit(1);
        }
    };

    let mut reader = csv::Reader::from_reader(content.as_bytes());
    let mut rows: Vec<EventRow> = Vec::new();

    for result in reader.deserialize() {
        match result {
            Ok(row) => rows.push(row),
            Err(e) => {
                eprintln!("warning: failed to parse row: {e}");
            }
        }
    }

    if rows.is_empty() {
        eprintln!("no events found in {}", path.display());
        std::process::exit(1);
    }

    println!("Loaded {} events from {}", rows.len(), path.display());
    println!();

    let result = compute_backtest(&rows);

    println!("=== Backtest Results ===");
    println!("Total trades:     {}", result.total_trades);
    println!("Wins:             {}", result.wins);
    println!("Losses:           {}", result.losses);
    if result.total_trades > 0 {
        let win_rate = result.wins as f64 / result.total_trades as f64 * 100.0;
        println!("Win rate:         {:.1}%", win_rate);
        println!("Total PnL:        {:.2}%", result.total_pnl_pct * 100.0);
        println!("Max drawdown:     {:.2}%", result.max_drawdown * 100.0);
    }
}

fn compute_backtest(rows: &[EventRow]) -> BacktestResult {
    let mut result = BacktestResult::default();
    let mut open_trade: Option<TradeRecord> = None;
    let mut cumulative: f64 = 1.0;

    for row in rows {
        if !row.executed || !matches!(row.action.as_str(), "Buy" | "Sell") {
            continue;
        }

        if row.action == "Buy" {
            if open_trade.is_some() {
                result.losses += 1;
            }
            open_trade = Some(TradeRecord {
                buy_timestamp: row.timestamp.clone(),
                buy_price: row.price,
                buy_block: row.block,
            });
        } else if row.action == "Sell" {
            if let Some(entry) = &open_trade {
                let pnl_pct = (row.price - entry.buy_price) / entry.buy_price;
                cumulative *= 1.0 + pnl_pct;
                result.total_trades += 1;
                if pnl_pct > 0.0 {
                    result.wins += 1;
                } else {
                    result.losses += 1;
                }
                result.total_pnl_pct = cumulative - 1.0;
                result.cumulative_pnl.push(cumulative);
                result.peak_value = result.peak_value.max(cumulative);
                let dd = (result.peak_value - cumulative) / result.peak_value;
                result.max_drawdown = result.max_drawdown.max(dd);
                open_trade = None;
            }
        }
    }

    result
}

/// Simple CSV reader without external dependency for backtester
mod csv {
    use std::io::Read;

    pub struct Reader<R> {
        #[allow(dead_code)]
        inner: R,
        headers: Vec<String>,
        buffer: String,
    }

    impl<R: Read> Reader<R> {
        pub fn from_reader(reader: R) -> Self {
            let mut buffer = String::new();
            let mut r = reader;
            r.read_to_string(&mut buffer).ok();
            let headers = if let Some(first_line) = buffer.lines().next() {
                first_line.split(',').map(|s| s.to_string()).collect()
            } else {
                Vec::new()
            };
            Reader {
                inner: r,
                headers,
                buffer,
            }
        }

        pub fn deserialize<T: serde::de::DeserializeOwned>(&mut self) -> Vec<Result<T, String>> {
            let mut results = Vec::new();
            let lines: Vec<&str> = self.buffer.lines().skip(1).collect();
            for line in lines {
                if line.trim().is_empty() {
                    continue;
                }
                let values: Vec<&str> = line.split(',').collect();
                let mut map = serde_json::Map::new();
                for (i, header) in self.headers.iter().enumerate() {
                    let val = values.get(i).unwrap_or(&"");
                    map.insert(header.clone(), serde_json::Value::String(val.to_string()));
                }
                match serde_json::from_value::<T>(serde_json::Value::Object(map)) {
                    Ok(item) => results.push(Ok(item)),
                    Err(e) => results.push(Err(e.to_string())),
                }
            }
            results
        }
    }
}

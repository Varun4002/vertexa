use std::path::{Path, PathBuf};
use std::sync::Arc;
use tokio::io::{AsyncWriteExt, BufWriter};
use tokio::fs::{self, File};
use tokio::sync::Mutex;
use chrono::{Utc, NaiveDate};
use tracing::{info, error};

use vertexa_core::{Vote, FixedUsd};

pub struct EventLogger {
    file: Arc<Mutex<BufWriter<File>>>,
    current_date: NaiveDate,
    path: PathBuf,
}

impl EventLogger {
    pub async fn new(path: PathBuf) -> Self {
        let date = Utc::now().date_naive();
        let file_path = Self::file_path_for_date(&path, date);
        let file = Self::open_file(&file_path).await;

        let mut writer = BufWriter::new(file);
        let header = "timestamp,block,price,regime,action,confidence,size_mult,executed,route,tx_hash,error,gas_usd,mev_usd,edge_usd,reward_to_cost\n";
        if let Err(e) = writer.write_all(header.as_bytes()).await {
            error!(target: "vertexa", error = %e, "failed to write csv header");
        }

        Self {
            file: Arc::new(Mutex::new(writer)),
            current_date: date,
            path,
        }
    }

    fn file_path_for_date(base: &Path, date: NaiveDate) -> PathBuf {
        let filename = format!("events_{}.csv", date.format("%Y-%m-%d"));
        base.join(filename)
    }

    async fn open_file(path: &Path) -> File {
        if let Some(parent) = path.parent() {
            let _ = fs::create_dir_all(parent).await;
        }
        File::create(path).await.unwrap_or_else(|e| {
            panic!("failed to create event log file {path:?}: {e}")
        })
    }

    #[allow(clippy::too_many_arguments)]
    pub async fn log(
        &self,
        block: u64,
        price: f64,
        regime: &str,
        action: &Vote,
        confidence: f64,
        size_mult: f64,
        executed: bool,
        route: &str,
        tx_hash: &Option<String>,
        error: &Option<String>,
        gas_cost: FixedUsd,
        mev_cost: FixedUsd,
        edge: FixedUsd,
        reward_to_cost: f64,
    ) {
        let now = Utc::now();
        let today = now.date_naive();

        let mut writer = self.file.lock().await;

        if today != self.current_date {
            let new_path = Self::file_path_for_date(&self.path, today);
            let new_file = Self::open_file(&new_path).await;
            let mut new_writer = BufWriter::new(new_file);
            let header = "timestamp,block,price,regime,action,confidence,size_mult,executed,route,tx_hash,error,gas_usd,mev_usd,edge_usd,reward_to_cost\n";
            let _ = new_writer.write_all(header.as_bytes()).await;
            *writer = new_writer;
            info!(target: "vertexa", date = %today.format("%Y-%m-%d"), "rotated event log file");
        }

        let timestamp = now.format("%Y-%m-%dT%H:%M:%S%.3fZ");
        let action_str = match action {
            Vote::Buy => "Buy",
            Vote::Sell => "Sell",
            Vote::Neutral => "Neutral",
        };
        let tx_hash_str = tx_hash.as_deref().unwrap_or("");
        let error_str = error.as_deref().unwrap_or("");

        let line = format!(
            "{},{},{:.2},{},{},{:.4},{},{},{},{},{},{:.4},{:.4},{:.4},{:.4}\n",
            timestamp,
            block,
            price,
            regime,
            action_str,
            confidence,
            size_mult,
            executed,
            route,
            tx_hash_str,
            error_str,
            gas_cost.to_dollars(),
            mev_cost.to_dollars(),
            edge.to_dollars(),
            reward_to_cost,
        );

        if let Err(e) = writer.write_all(line.as_bytes()).await {
            error!(target: "vertexa", error = %e, "failed to write event log line");
        }
        let _ = writer.flush().await;
    }
}

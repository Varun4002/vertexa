use alloy::primitives::U256;
use tracing::info;
use vertexa_core::{PlannedTrade, TradeChunk};

pub struct SplitExecutor {
    min_chunk_usd: f64,
}

impl SplitExecutor {
    pub fn new(min_chunk_usd: f64) -> Self {
        Self { min_chunk_usd }
    }

    pub fn compute_chunks(&self, trade: &PlannedTrade, risk_score: f64) -> Vec<TradeChunk> {
        let (num_chunks, blocks_apart) = if risk_score >= 0.60 {
            (3, 2u64)
        } else {
            (2, 1u64)
        };

        let chunk_amount = trade.amount_in / U256::from(num_chunks);
        let chunk_usd = trade.amount_usd / num_chunks as f64;

        if chunk_usd < self.min_chunk_usd {
            info!(
                target: "vertexa",
                chunk_usd = chunk_usd,
                min_chunk = self.min_chunk_usd,
                "chunk too small, not splitting"
            );
            return vec![TradeChunk {
                amount_in: trade.amount_in,
                delay_blocks: 0,
            }];
        }

        let chunks: Vec<TradeChunk> = (0..num_chunks)
            .map(|i| TradeChunk {
                amount_in: chunk_amount,
                delay_blocks: i as u64 * blocks_apart,
            })
            .collect();

        info!(
            target: "vertexa",
            num_chunks = num_chunks,
            blocks_apart = blocks_apart,
            chunk_usd = chunk_usd,
            total_chunks = chunks.len(),
            "computed split execution chunks"
        );

        chunks
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use vertexa_core::Vote;
    use alloy::primitives::address;

    #[test]
    fn test_compute_two_chunks() {
        let executor = SplitExecutor::new(500.0);
        let trade = PlannedTrade {
            action: Vote::Buy,
            token_in: address!("0000000000000000000000000000000000000001"),
            token_out: address!("0000000000000000000000000000000000000002"),
            amount_in: U256::from(1_000_000_000_000_000_000u128),
            amount_usd: 10_000.0,
            max_slippage: 0.01,
            pool_fee: 3000,
        };
        let chunks = executor.compute_chunks(&trade, 0.50);
        assert_eq!(chunks.len(), 2);
        assert_eq!(chunks[0].delay_blocks, 0);
        assert_eq!(chunks[1].delay_blocks, 1);
    }

    #[test]
    fn test_compute_three_chunks() {
        let executor = SplitExecutor::new(500.0);
        let trade = PlannedTrade {
            action: Vote::Buy,
            token_in: address!("0000000000000000000000000000000000000001"),
            token_out: address!("0000000000000000000000000000000000000002"),
            amount_in: U256::from(1_000_000_000_000_000_000u128),
            amount_usd: 10_000.0,
            max_slippage: 0.01,
            pool_fee: 3000,
        };
        let chunks = executor.compute_chunks(&trade, 0.65);
        assert_eq!(chunks.len(), 3);
        assert_eq!(chunks[0].delay_blocks, 0);
        assert_eq!(chunks[1].delay_blocks, 2);
        assert_eq!(chunks[2].delay_blocks, 4);
    }

    #[test]
    fn test_no_split_when_below_minimum() {
        let executor = SplitExecutor::new(500.0);
        let trade = PlannedTrade {
            action: Vote::Buy,
            token_in: address!("0000000000000000000000000000000000000001"),
            token_out: address!("0000000000000000000000000000000000000002"),
            amount_in: U256::from(1_000_000_000_000_000u128),
            amount_usd: 600.0,
            max_slippage: 0.01,
            pool_fee: 3000,
        };
        let chunks = executor.compute_chunks(&trade, 0.50);
        assert_eq!(chunks.len(), 1);
    }
}

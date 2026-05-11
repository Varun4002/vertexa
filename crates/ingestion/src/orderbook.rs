use vertexa_core::{OrderBook, PriceLevel};

pub fn build_orderbook(_current_tick: i32, _liquidity: f64, _price: f64) -> OrderBook {
    let spread = _price * 0.0005;
    let mut bids = Vec::with_capacity(20);
    let mut asks = Vec::with_capacity(20);

    for i in 0..20 {
        let bid_price = _price - spread * (i as f64 + 1.0);
        let ask_price = _price + spread * (i as f64 + 1.0);
        let size = _liquidity / 2000.0;

        bids.push(PriceLevel {
            price: bid_price,
            size: size * (20 - i) as f64 / 20.0,
        });
        asks.push(PriceLevel {
            price: ask_price,
            size: size * (i + 1) as f64 / 20.0,
        });
    }

    bids.sort_by(|a, b| b.price.partial_cmp(&a.price).unwrap_or(std::cmp::Ordering::Equal));
    asks.sort_by(|a, b| a.price.partial_cmp(&b.price).unwrap_or(std::cmp::Ordering::Equal));

    OrderBook { bids, asks }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_orderbook_sorted() {
        let ob = build_orderbook(0, 1_000_000.0, 3000.0);
        assert!(!ob.bids.is_empty());
        assert!(!ob.asks.is_empty());

        for w in ob.bids.windows(2) {
            assert!(w[0].price >= w[1].price);
        }
        for w in ob.asks.windows(2) {
            assert!(w[0].price <= w[1].price);
        }
    }

    #[test]
    fn test_orderbook_returns_20_levels() {
        let ob = build_orderbook(0, 1_000_000.0, 3000.0);
        assert_eq!(ob.bids.len(), 20);
        assert_eq!(ob.asks.len(), 20);
    }
}

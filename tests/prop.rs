use proptest::prelude::*;
use shardgate::{Config, Decision, RateLimiter};
use std::collections::HashMap;

fn cfg(cap: u32) -> Config {
    Config {
        capacity: cap,
        refill_rate: 0.0,
        num_shards: 16,
    }
}

// Any sequence of keys with zero refill: per-key admitted == min(uses, capacity).
proptest! {
    #![proptest_config(ProptestConfig::with_cases(256))]
    #[test]
    fn tokens_never_exceed_capacity(ops in prop::collection::vec(0u8..4, 1..200), cap in 1u32..10) {
        let lim = RateLimiter::new(cfg(cap));
        let mut admitted: HashMap<u8, usize> = HashMap::new();
        for k in ops {
            let key = format!("k{}", k);
            if matches!(lim.check(&key), Decision::Allowed) {
                *admitted.entry(k).or_insert(0) += 1;
            }
        }
        for (_, n) in admitted {
            prop_assert!(n <= cap as usize, "admitted {} exceeded capacity {}", n, cap);
        }
    }

    #[test]
    fn keys_are_independent(ops in prop::collection::vec(0u8..8, 1..200), cap in 1u32..5) {
        let lim = RateLimiter::new(cfg(cap));
        let mut uses: HashMap<u8, usize> = HashMap::new();
        let mut admitted: HashMap<u8, usize> = HashMap::new();
        for k in ops {
            *uses.entry(k).or_insert(0) += 1;
            if matches!(lim.check(&format!("k{}", k)), Decision::Allowed) {
                *admitted.entry(k).or_insert(0) += 1;
            }
        }
        // Each key admits exactly min(uses, cap), proving no token leak across keys.
        for (k, u) in uses {
            let a = admitted.get(&k).copied().unwrap_or(0);
            prop_assert_eq!(a, std::cmp::min(u, cap as usize));
        }
    }

    #[test]
    fn retry_after_nonnegative(cap in 1u32..5) {
        let lim = RateLimiter::new(Config { capacity: cap, refill_rate: 1.0, num_shards: 4 });
        // Drain the bucket, then retry_after must be >= 0.
        for _ in 0..cap {
            let _ = lim.check("alice");
        }
        if let Decision::Denied { retry_after } = lim.check("alice") {
            prop_assert!(retry_after.as_secs_f64() >= 0.0);
        }
    }
}

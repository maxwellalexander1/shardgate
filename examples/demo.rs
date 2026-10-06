//! Demo: prove ShardGate fills up -> denies, frees up -> allows.
//! Run: cargo run --example demo

use shardgate::{Config, Decision, RateLimiter};
use std::sync::Arc;
use std::time::Instant;

#[tokio::main]
async fn main() {
    let config = Config {
        capacity: 10,
        refill_rate: 1.0, // 1 token/sec
        num_shards: 16,
    };
    let limiter = Arc::new(RateLimiter::new(config));

    // --- Scenario A: HOT KEY — everyone fights over 1 bucket ---
    // Think: 32 people trying to use the same arcade card with 10 tokens.
    println!("--- A: HOT single key (32 tasks x check(\"hot_key\")) ---");
    let start = Instant::now();
    let mut handles = Vec::new();
    for _ in 0..32 {
        let l = limiter.clone();
        handles.push(tokio::spawn(async move { l.check("hot_key") }));
    }
    let (mut allowed, mut denied) = (0, 0);
    for h in handles {
        match h.await.unwrap() {
            Decision::Allowed => allowed += 1,
            Decision::Denied { .. } => denied += 1,
        }
    }
    println!(
        "Allowed: {}, Denied: {}, time: {:?} (expect ~10 allowed, rest denied)",
        allowed,
        denied,
        start.elapsed()
    );

    // --- Scenario B: SPREAD — everyone has their own card ---
    // Think: 32 people, each with 100 own keys = no fighting, mostly Allowed.
    println!("\n--- B: SPREAD many keys (32 tasks x 100 unique keys) ---");
    let limiter2 = Arc::new(RateLimiter::new(Config {
        capacity: 10,
        refill_rate: 1.0,
        num_shards: 16,
    }));
    let start = Instant::now();
    let mut handles = Vec::new();
    for t in 0..32 {
        let l = limiter2.clone();
        handles.push(tokio::spawn(async move {
            let (mut a, mut d) = (0, 0);
            for i in 0..100 {
                let key = format!("user_{}_{}", t, i);
                match l.check(&key) {
                    Decision::Allowed => a += 1,
                    Decision::Denied { .. } => d += 1,
                }
            }
            (a, d)
        }));
    }
    let (mut allowed, mut denied) = (0, 0);
    for h in handles {
        let (a, d) = h.await.unwrap();
        allowed += a;
        denied += d;
    }
    println!(
        "Allowed: {}, Denied: {}, time: {:?} (expect ~3200 allowed, 0 denied)",
        allowed,
        denied,
        start.elapsed()
    );

    println!("\nDone. A proves deny-when-full, B proves parallel-when-spread.");
}

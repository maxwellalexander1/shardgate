use shardgate::{Config, Decision, NaiveLimiter, RateLimiter};
use std::sync::{
    atomic::{AtomicUsize, Ordering},
    Arc, Barrier,
};
use std::time::Instant;

fn cfg(capacity: u32, refill_rate: f64) -> Config {
    Config {
        capacity,
        refill_rate,
        num_shards: 16,
    }
}

#[test]
fn zero_refill_single_key_exact() {
    let lim = Arc::new(RateLimiter::new(cfg(10, 0.0)));
    let barrier = Arc::new(Barrier::new(8));
    let allowed = Arc::new(AtomicUsize::new(0));

    std::thread::scope(|s| {
        for _ in 0..8 {
            let lim = lim.clone();
            let barrier = barrier.clone();
            let allowed = allowed.clone();
            s.spawn(move || {
                barrier.wait();
                for _ in 0..100 {
                    if matches!(lim.check("alice"), Decision::Allowed) {
                        allowed.fetch_add(1, Ordering::Relaxed);
                    }
                }
            });
        }
    });

    assert_eq!(
        allowed.load(Ordering::Relaxed),
        10,
        "zero refill: 8 threads x 100 checks must admit exactly capacity"
    );
}

#[test]
fn refill_bounded_window() {
    let lim = Arc::new(RateLimiter::new(cfg(5, 10.0)));
    let barrier = Arc::new(Barrier::new(8));
    let allowed = Arc::new(AtomicUsize::new(0));
    let start = Instant::now();

    std::thread::scope(|s| {
        for _ in 0..8 {
            let lim = lim.clone();
            let barrier = barrier.clone();
            let allowed = allowed.clone();
            s.spawn(move || {
                barrier.wait();
                let end = start + std::time::Duration::from_millis(200);
                while Instant::now() < end {
                    if matches!(lim.check("bob"), Decision::Allowed) {
                        allowed.fetch_add(1, Ordering::Relaxed);
                    }
                }
            });
        }
    });

    let elapsed = start.elapsed().as_secs_f64();
    let admitted = allowed.load(Ordering::Relaxed) as f64;
    let bound = 5.0 + (10.0 * elapsed).ceil() + 1.0;
    assert!(
        admitted <= bound,
        "admitted {} exceeded bound {} (elapsed {:.3}s)",
        admitted,
        bound,
        elapsed
    );
}

#[test]
fn spread_keys_isolated() {
    let lim = Arc::new(RateLimiter::new(cfg(10, 0.0)));
    let counts: Arc<Vec<AtomicUsize>> = Arc::new((0..8).map(|_| AtomicUsize::new(0)).collect());

    std::thread::scope(|s| {
        for i in 0..8 {
            let lim = lim.clone();
            let counts = counts.clone();
            s.spawn(move || {
                let key = format!("key_{}", i);
                for _ in 0..50 {
                    if matches!(lim.check(&key), Decision::Allowed) {
                        counts[i].fetch_add(1, Ordering::Relaxed);
                    }
                }
            });
        }
    });

    for i in 0..8 {
        assert_eq!(
            counts[i].load(Ordering::Relaxed),
            10,
            "key_{} must admit exactly capacity",
            i
        );
    }
}

#[test]
fn naive_agrees_with_sharded() {
    for (cap, refill) in [(10u32, 0.0f64), (5u32, 2.0f64)] {
        let sharded = Arc::new(RateLimiter::new(cfg(cap, refill)));
        let naive = Arc::new(NaiveLimiter::new(cfg(cap, refill)));

        let run = |lim: Arc<RateLimiter>| {
            let allowed = Arc::new(AtomicUsize::new(0));
            let barrier = Arc::new(Barrier::new(8));
            std::thread::scope(|s| {
                for _ in 0..8 {
                    let lim = lim.clone();
                    let allowed = allowed.clone();
                    let barrier = barrier.clone();
                    s.spawn(move || {
                        barrier.wait();
                        for _ in 0..50 {
                            if matches!(lim.check("shared"), Decision::Allowed) {
                                allowed.fetch_add(1, Ordering::Relaxed);
                            }
                        }
                    });
                }
            });
            allowed.load(Ordering::Relaxed)
        };

        let run_naive = |lim: Arc<NaiveLimiter>| {
            let allowed = Arc::new(AtomicUsize::new(0));
            let barrier = Arc::new(Barrier::new(8));
            std::thread::scope(|s| {
                for _ in 0..8 {
                    let lim = lim.clone();
                    let allowed = allowed.clone();
                    let barrier = barrier.clone();
                    s.spawn(move || {
                        barrier.wait();
                        for _ in 0..50 {
                            if matches!(lim.check("shared"), Decision::Allowed) {
                                allowed.fetch_add(1, Ordering::Relaxed);
                            }
                        }
                    });
                }
            });
            allowed.load(Ordering::Relaxed)
        };

        let s = run(sharded);
        let n = run_naive(naive);
        assert_eq!(
            s, n,
            "sharded ({}) and naive ({}) must agree for cap={} refill={}",
            s, n, cap, refill
        );
        if refill == 0.0 {
            assert_eq!(s, cap as usize, "zero refill must equal capacity");
        }
    }
}

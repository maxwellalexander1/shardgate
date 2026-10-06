//! ShardGate HTTP front door.
//! Run: PORT=8080 cargo run --bin shardgate-server
//! Try: curl localhost:8080/healthz
//!      curl localhost:8080/check?key=alice
//!
//! Think: same bouncer (Arc<RateLimiter>), just a door people knock on.

use axum::{
    extract::{Query, State},
    http::{header, HeaderMap, StatusCode},
    response::{IntoResponse, Json},
    routing::get,
    Router,
};
use serde::{Deserialize, Serialize};
use shardgate::{Config, Decision, RateLimiter};
use std::{
    net::SocketAddr,
    sync::{
        atomic::{AtomicU64, Ordering},
        Arc,
    },
    time::Duration,
};

#[derive(Clone)]
struct AppState {
    limiter: Arc<RateLimiter>,
    allowed_total: Arc<AtomicU64>,
    denied_total: Arc<AtomicU64>,
}

#[derive(Debug, Deserialize)]
struct CheckParams {
    key: Option<String>,
}

#[derive(Debug, Serialize)]
struct CheckBody {
    key: String,
    allowed: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    retry_after_secs: Option<u64>,
}

fn config_from_env() -> (Config, u16) {
    let capacity: u32 = std::env::var("SHARDGATE_CAPACITY")
        .ok()
        .and_then(|v| v.parse().ok())
        .unwrap_or(10);
    let refill_rate: f64 = std::env::var("SHARDGATE_REFILL_RATE")
        .ok()
        .and_then(|v| v.parse().ok())
        .unwrap_or(1.0);
    let num_shards: usize = std::env::var("SHARDGATE_SHARDS")
        .ok()
        .and_then(|v| v.parse().ok())
        .unwrap_or(16);
    let port: u16 = std::env::var("PORT")
        .ok()
        .and_then(|v| v.parse().ok())
        .unwrap_or(8080);
    (
        Config {
            capacity,
            refill_rate,
            num_shards,
        },
        port,
    )
}

async fn healthz() -> &'static str {
    "ok"
}

async fn check(
    State(state): State<AppState>,
    Query(params): Query<CheckParams>,
) -> impl IntoResponse {
    let key = match params.key {
        Some(k) if !k.is_empty() => k,
        _ => {
            let body = Json(serde_json::json!({"error": "missing ?key="}));
            return (StatusCode::BAD_REQUEST, HeaderMap::new(), body).into_response();
        }
    };

    match state.limiter.check(&key) {
        Decision::Allowed => {
            state.allowed_total.fetch_add(1, Ordering::Relaxed);
            let body = Json(CheckBody {
                key,
                allowed: true,
                retry_after_secs: None,
            });
            (StatusCode::OK, HeaderMap::new(), body).into_response()
        }
        Decision::Denied { retry_after } => {
            // HTTP wants whole seconds, rounded UP. Min 1 so client waits a bit.
            let secs = (retry_after.as_secs_f64().ceil() as u64).max(1);
            state.denied_total.fetch_add(1, Ordering::Relaxed);
            let mut headers = HeaderMap::new();
            headers.insert(header::RETRY_AFTER, secs.to_string().parse().unwrap());
            let body = Json(CheckBody {
                key,
                allowed: false,
                retry_after_secs: Some(secs),
            });
            (StatusCode::TOO_MANY_REQUESTS, headers, body).into_response()
        }
    }
}

async fn metrics(State(state): State<AppState>) -> String {
    format!(
        "allowed_total {}\ndenied_total {}\nactive_buckets {}\n",
        state.allowed_total.load(Ordering::Relaxed),
        state.denied_total.load(Ordering::Relaxed),
        state.limiter.bucket_count(),
    )
}

async fn shutdown_signal() {
    let ctrl_c = async {
        tokio::signal::ctrl_c()
            .await
            .expect("install Ctrl-C handler");
    };

    #[cfg(unix)]
    let terminate = async {
        let mut sig = tokio::signal::unix::signal(tokio::signal::unix::SignalKind::terminate())
            .expect("install SIGTERM handler");
        sig.recv().await;
    };

    #[cfg(not(unix))]
    let terminate = std::future::pending::<()>();

    tokio::select! {
        _ = ctrl_c => {},
        _ = terminate => {},
    }
    println!("shutting down");
}

#[tokio::main]
async fn main() {
    let (config, port) = config_from_env();
    println!(
        "shardgate: capacity={} refill_rate={} shards={} port={}",
        config.capacity, config.refill_rate, config.num_shards, port
    );

    let limiter = Arc::new(RateLimiter::new(config));
    // Sweeper at boot so memory stays bounded (same 60s/300s as README).
    let _sweeper = limiter.spawn_cleanup(Duration::from_secs(60), Duration::from_secs(300));

    let state = AppState {
        limiter,
        allowed_total: Arc::new(AtomicU64::new(0)),
        denied_total: Arc::new(AtomicU64::new(0)),
    };
    let app = Router::new()
        .route("/healthz", get(healthz))
        .route("/check", get(check))
        .route("/metrics", get(metrics))
        .with_state(state);

    let addr = SocketAddr::from(([0, 0, 0, 0], port));
    println!("listening on {}", addr);
    let listener = tokio::net::TcpListener::bind(addr).await.unwrap();
    axum::serve(listener, app)
        .with_graceful_shutdown(shutdown_signal())
        .await
        .unwrap();
}

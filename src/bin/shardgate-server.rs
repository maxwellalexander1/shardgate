//! ShardGate HTTP front door with TLS + bearer auth (T10).
//! Buckets are keyed by authenticated identity, never by client ?key=.
//! /healthz stays open for load balancers. Everything else needs auth.

use axum::{
    extract::State,
    http::{header, HeaderMap, Request, StatusCode},
    middleware::{from_fn_with_state, Next},
    response::{IntoResponse, Json, Response},
    routing::get,
    Router,
};
use serde::Serialize;
use shardgate::{Config, Decision, RateLimiter};
use std::{
    net::SocketAddr,
    sync::{
        atomic::{AtomicU64, Ordering},
        Arc,
    },
    time::Duration,
};
use subtle::ConstantTimeEq;
use tower_http::cors::CorsLayer;

#[derive(Clone)]
struct Tokens {
    // (identity, secret). Secrets never logged.
    pairs: Arc<Vec<(String, String)>>,
}

#[derive(Clone)]
struct AuthIdentity(String);

#[derive(Clone)]
struct AppState {
    limiter: Arc<RateLimiter>,
    allowed_total: Arc<AtomicU64>,
    denied_total: Arc<AtomicU64>,
}

#[derive(Debug, Serialize)]
struct CheckBody {
    key: String,
    allowed: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    retry_after_secs: Option<u64>,
}

fn tokens_from_env() -> Tokens {
    let mut pairs = Vec::new();
    if let Ok(raw) = std::env::var("SHARDGATE_TOKENS") {
        for part in raw.split(',') {
            let part = part.trim();
            if part.is_empty() {
                continue;
            }
            if let Some((id, secret)) = part.split_once(':') {
                pairs.push((id.trim().to_string(), secret.trim().to_string()));
            }
        }
    }
    if pairs.is_empty() {
        if let Ok(secret) = std::env::var("SHARDGATE_API_TOKEN") {
            pairs.push(("default".to_string(), secret));
        }
    }
    Tokens {
        pairs: Arc::new(pairs),
    }
}

fn auth_required() -> bool {
    // No tokens configured means dev mode: auth off, ?key= fallback.
    !tokens_from_env().pairs.is_empty()
}

async fn auth_middleware(
    State(tokens): State<Tokens>,
    mut req: Request<axum::body::Body>,
    next: Next,
) -> Response {
    if req.uri().path() == "/healthz" {
        return next.run(req).await;
    }
    if tokens.pairs.is_empty() {
        // Dev mode, no auth configured.
        req.extensions_mut().insert(AuthIdentity("dev".to_string()));
        return next.run(req).await;
    }
    let header_val = req
        .headers()
        .get(header::AUTHORIZATION)
        .and_then(|v| v.to_str().ok())
        .unwrap_or("");
    let presented = header_val.strip_prefix("Bearer ").unwrap_or("");
    for (id, secret) in tokens.pairs.iter() {
        // Constant-time compare, same length pad to avoid early length leak.
        if presented.as_bytes().ct_eq(secret.as_bytes()).into() {
            req.extensions_mut().insert(AuthIdentity(id.clone()));
            return next.run(req).await;
        }
    }
    (StatusCode::UNAUTHORIZED, "unauthorized\n").into_response()
}

fn do_check(state: &AppState, identity: &str) -> Response {
    // Bucket = identity. Client ?key= is ignored by design (prevents
    // unbounded bucket creation).
    match state.limiter.check(identity) {
        Decision::Allowed => {
            state.allowed_total.fetch_add(1, Ordering::Relaxed);
            let body = Json(CheckBody {
                key: identity.to_string(),
                allowed: true,
                retry_after_secs: None,
            });
            (StatusCode::OK, HeaderMap::new(), body).into_response()
        }
        Decision::Denied { retry_after } => {
            let secs = (retry_after.as_secs_f64().ceil() as u64).max(1);
            state.denied_total.fetch_add(1, Ordering::Relaxed);
            let mut headers = HeaderMap::new();
            headers.insert(header::RETRY_AFTER, secs.to_string().parse().unwrap());
            let body = Json(CheckBody {
                key: identity.to_string(),
                allowed: false,
                retry_after_secs: Some(secs),
            });
            (StatusCode::TOO_MANY_REQUESTS, headers, body).into_response()
        }
    }
}

async fn healthz() -> &'static str {
    "ok"
}

async fn check(State(state): State<AppState>, req: Request<axum::body::Body>) -> Response {
    let identity = req
        .extensions()
        .get::<AuthIdentity>()
        .map(|a| a.0.clone())
        .unwrap_or_else(|| "dev".to_string());
    // If dev mode without auth, honor ?key= for backwards compat.
    if identity == "dev" && tokens_from_env().pairs.is_empty() {
        let key = req
            .uri()
            .query()
            .and_then(|q| {
                for pair in q.split('&') {
                    if let Some(v) = pair.strip_prefix("key=") {
                        return Some(v.to_string());
                    }
                }
                None
            })
            .unwrap_or_else(|| "dev".to_string());
        return do_check(&state, &key);
    }
    do_check(&state, &identity)
}

async fn metrics(State(state): State<AppState>) -> String {
    format!(
        "allowed_total {}\ndenied_total {}\nactive_buckets {}\n",
        state.allowed_total.load(Ordering::Relaxed),
        state.denied_total.load(Ordering::Relaxed),
        state.limiter.bucket_count(),
    )
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

async fn shutdown_signal() {
    let ctrl_c = async {
        tokio::signal::ctrl_c()
            .await
            .expect("install Ctrl-C handler");
    };
    #[cfg(unix)]
    let terminate = async {
        let mut sig = tokio::signal::unix::signal(tokio::signal::unix::SignalKind::terminate())
            .expect("install SIGTERM");
        sig.recv().await;
    };
    #[cfg(not(unix))]
    let terminate = std::future::pending::<()>();
    tokio::select! {
        _ = ctrl_c => {},
        _ = terminate => {},
    }
}

#[tokio::main]
async fn main() {
    let (config, port) = config_from_env();
    let tokens = tokens_from_env();
    eprintln!(
        "shardgate: cap={} rate={} shards={} port={} auth={} tls={}",
        config.capacity,
        config.refill_rate,
        config.num_shards,
        port,
        if tokens.pairs.is_empty() {
            "off(dev)"
        } else {
            "on"
        },
        std::env::var("SHARDGATE_TLS_CERT").is_ok()
    );

    let limiter = Arc::new(RateLimiter::new(config));
    let _sweeper = limiter.spawn_cleanup(Duration::from_secs(60), Duration::from_secs(300));
    let state = AppState {
        limiter,
        allowed_total: Arc::new(AtomicU64::new(0)),
        denied_total: Arc::new(AtomicU64::new(0)),
    };

    let app = Router::new()
        .route("/healthz", get(healthz))
        .route("/check", get(check).post(check))
        .route("/metrics", get(metrics))
        .layer(from_fn_with_state(tokens.clone(), auth_middleware))
        .layer(
            CorsLayer::new()
                .allow_origin(["http://localhost:5173".parse().unwrap()])
                .allow_methods([axum::http::Method::GET, axum::http::Method::POST])
                .allow_headers([header::CONTENT_TYPE]),
        )
        .with_state(state);

    let addr = SocketAddr::from(([0, 0, 0, 0], port));
    let cert = std::env::var("SHARDGATE_TLS_CERT").ok();
    let key = std::env::var("SHARDGATE_TLS_KEY").ok();
    match (cert, key) {
        (Some(cert), Some(key)) => {
            eprintln!("TLS on: {}", addr);
            let tls = axum_server::tls_rustls::RustlsConfig::from_pem_file(cert, key)
                .await
                .unwrap();
            axum_server::bind_rustls(addr, tls)
                .serve(app.into_make_service())
                .await
                .unwrap();
        }
        _ => {
            eprintln!(
                "TLS off (set SHARDGATE_TLS_CERT/KEY for TLS), listening on {}",
                addr
            );
            let listener = tokio::net::TcpListener::bind(addr).await.unwrap();
            axum::serve(listener, app)
                .with_graceful_shutdown(shutdown_signal())
                .await
                .unwrap();
        }
    }
    let _ = auth_required();
}

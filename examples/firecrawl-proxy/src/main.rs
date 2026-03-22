//! # Firecrawl MPP Proxy
//!
//! A proxy that gates Firecrawl's web scraping API behind MPP payments on Movement.
//! Users pay in MOVE per request — the proxy handles the 402 challenge flow and
//! forwards authenticated requests to Firecrawl.
//!
//! ## Endpoints
//!
//! - `GET  /api/health`     — free health check
//! - `POST /api/scrape`     — scrape a URL (0.01 MOVE per request)
//! - `POST /api/crawl`      — crawl a site (0.05 MOVE per request)
//! - `GET  /api/services`   — service discovery (JSON)
//! - `GET  /api/llms.txt`   — LLM-friendly service discovery
//!
//! ## Running
//!
//! ```bash
//! cp .env.example .env
//! # Add your Firecrawl API key to .env
//! cargo run --bin firecrawl-proxy
//! ```

use axum::{
    extract::{ConnectInfo, FromRef, State},
    http::{header, HeaderMap, StatusCode},
    response::IntoResponse,
    routing::{get, post},
    Json, Router,
};
use mpp::proxy::service::{Endpoint, PaidEndpoint, ProxyConfig, Service};
use mpp::server::axum::{ChargeChallenger, ChargeConfig, MppCharge, WithReceipt};
use mpp::server::{movement, MovementConfig, Mpp};
use std::collections::HashMap;
use std::net::SocketAddr;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};
use tower_http::cors::{AllowOrigin, CorsLayer};
use tower_http::limit::RequestBodyLimitLayer;

const FIRECRAWL_BASE: &str = "https://api.firecrawl.dev";

// --- Rate limiter ---

#[derive(Clone)]
struct RateLimiter {
    /// IP -> (request count, window start)
    windows: Arc<Mutex<HashMap<String, (u32, Instant)>>>,
    max_requests: u32,
    window_secs: u64,
}

impl RateLimiter {
    fn new(max_requests: u32, window_secs: u64) -> Self {
        Self {
            windows: Arc::new(Mutex::new(HashMap::new())),
            max_requests,
            window_secs,
        }
    }

    fn check(&self, ip: &str) -> bool {
        let mut windows = self.windows.lock().unwrap();
        let now = Instant::now();
        let window_duration = Duration::from_secs(self.window_secs);

        let entry = windows.entry(ip.to_string()).or_insert((0, now));

        // Reset window if expired
        if now.duration_since(entry.1) > window_duration {
            *entry = (1, now);
            return true;
        }

        if entry.0 >= self.max_requests {
            return false;
        }

        entry.0 += 1;
        true
    }

    /// Periodically clean up expired entries to prevent memory growth
    fn cleanup(&self) {
        let mut windows = self.windows.lock().unwrap();
        let now = Instant::now();
        let window_duration = Duration::from_secs(self.window_secs);
        windows.retain(|_, (_, start)| now.duration_since(*start) <= window_duration);
    }
}

// --- URL validation ---

fn validate_scrape_url(input: &str) -> Result<String, String> {
    let parsed = url::Url::parse(input).map_err(|e| format!("Invalid URL: {e}"))?;

    // Only allow http/https
    match parsed.scheme() {
        "http" | "https" => {}
        s => return Err(format!("Scheme '{s}' not allowed. Use http or https.")),
    }

    // Block private/internal IPs to prevent SSRF
    if let Some(host) = parsed.host_str() {
        let lower = host.to_lowercase();
        if lower == "localhost"
            || lower == "127.0.0.1"
            || lower == "0.0.0.0"
            || lower == "::1"
            || lower.starts_with("10.")
            || lower.starts_with("172.16.")
            || lower.starts_with("172.17.")
            || lower.starts_with("172.18.")
            || lower.starts_with("172.19.")
            || lower.starts_with("172.2")
            || lower.starts_with("172.30.")
            || lower.starts_with("172.31.")
            || lower.starts_with("192.168.")
            || lower.ends_with(".internal")
            || lower.ends_with(".local")
            || lower == "metadata.google.internal"
            || lower == "169.254.169.254"
        {
            return Err("Internal/private URLs are not allowed".to_string());
        }
    } else {
        return Err("URL must have a host".to_string());
    }

    Ok(parsed.to_string())
}

// --- App state ---

#[derive(Clone)]
struct AppState {
    mpp: Arc<dyn ChargeChallenger>,
    firecrawl_api_key: String,
    http: reqwest::Client,
    proxy_config: ProxyConfig,
    rate_limiter: RateLimiter,
}

impl FromRef<AppState> for Arc<dyn ChargeChallenger> {
    fn from_ref(state: &AppState) -> Self {
        Arc::clone(&state.mpp)
    }
}

// --- Charge configs ---

/// 0.01 MOVE per scrape (~$0.002 at $0.20/MOVE — Firecrawl costs ~$0.0008/page)
struct ScrapeFee;
impl ChargeConfig for ScrapeFee {
    fn amount() -> &'static str {
        "0.01"
    }
    fn description() -> Option<&'static str> {
        Some("Scrape a URL via Firecrawl")
    }
}

/// 0.05 MOVE per crawl (~$0.01 — crawls hit multiple pages)
struct CrawlFee;
impl ChargeConfig for CrawlFee {
    fn amount() -> &'static str {
        "0.05"
    }
    fn description() -> Option<&'static str> {
        Some("Crawl a site via Firecrawl")
    }
}

// --- Helpers ---

fn get_client_ip(headers: &HeaderMap, addr: &SocketAddr) -> String {
    // Check X-Forwarded-For first (for reverse proxy setups)
    if let Some(xff) = headers.get("x-forwarded-for") {
        if let Ok(s) = xff.to_str() {
            if let Some(first) = s.split(',').next() {
                return first.trim().to_string();
            }
        }
    }
    addr.ip().to_string()
}

#[tokio::main]
async fn main() {
    dotenvy::dotenv().ok();

    let firecrawl_api_key =
        std::env::var("FIRECRAWL_API_KEY").expect("FIRECRAWL_API_KEY must be set in .env");

    let recipient = std::env::var("RECIPIENT").unwrap_or_else(|_| {
        eprintln!("WARNING: Using default RECIPIENT address. Set RECIPIENT in .env for production.");
        "0x74f1060add0c641a0c10bb5bab2bf5fd05f94d7c25055f2419fa82d7bbf2b1e8".to_string()
    });

    let secret_key = std::env::var("MPP_SECRET_KEY").unwrap_or_else(|_| {
        let generated: String = (0..32)
            .map(|_| {
                let idx = rand::random::<u8>() % 62;
                match idx {
                    0..=9 => (b'0' + idx) as char,
                    10..=35 => (b'a' + idx - 10) as char,
                    _ => (b'A' + idx - 36) as char,
                }
            })
            .collect();
        eprintln!("WARNING: MPP_SECRET_KEY not set. Generated random key for this session.");
        eprintln!("         Set MPP_SECRET_KEY in .env for consistent challenge IDs across restarts.");
        generated
    });

    let rest_url = std::env::var("MOVEMENT_REST_URL")
        .unwrap_or_else(|_| "https://testnet.movementnetwork.xyz/v1".to_string());

    let bind_addr = std::env::var("BIND_ADDR").unwrap_or_else(|_| "0.0.0.0:3010".to_string());

    let cors_origin = std::env::var("CORS_ORIGIN").ok();

    let rate_limit: u32 = std::env::var("RATE_LIMIT_PER_MINUTE")
        .ok()
        .and_then(|v| v.parse().ok())
        .unwrap_or(30);

    let mpp = Mpp::create_movement(
        movement(MovementConfig {
            recipient: &recipient,
        })
        .rest_url(&rest_url)
        .secret_key(&secret_key)
        .decimals(8),
    )
    .expect("failed to create Movement MPP handler");

    // Build proxy config for service discovery
    let proxy_config = ProxyConfig {
        base_path: Some("/api".to_string()),
        services: vec![
            Service::new("firecrawl", FIRECRAWL_BASE)
                .route(
                    "POST /v1/scrape",
                    Endpoint::Paid(PaidEndpoint {
                        intent: "charge".into(),
                        amount: "0.01".into(),
                        unit_type: None,
                        description: Some("Scrape a single URL and extract clean content".into()),
                    }),
                )
                .route(
                    "POST /v1/crawl",
                    Endpoint::Paid(PaidEndpoint {
                        intent: "charge".into(),
                        amount: "0.05".into(),
                        unit_type: None,
                        description: Some("Crawl an entire site".into()),
                    }),
                )
                .route("GET /v1/crawl/:id", Endpoint::Free)
                .build(),
        ],
    };

    let is_testnet = rest_url.contains("testnet");

    println!("Firecrawl MPP Proxy (Movement)");
    println!("  Network:    {}", if is_testnet { "testnet" } else { "mainnet" });
    println!("  REST URL:   {rest_url}");
    println!("  Recipient:  {recipient}");
    println!("  Rate limit: {rate_limit} req/min per IP");
    println!("  CORS:       {}", cors_origin.as_deref().unwrap_or("permissive (dev mode)"));
    println!();

    // HTTP client with timeouts for upstream Firecrawl calls
    let http = reqwest::Client::builder()
        .timeout(Duration::from_secs(30))
        .connect_timeout(Duration::from_secs(5))
        .build()
        .expect("failed to create HTTP client");

    let rate_limiter = RateLimiter::new(rate_limit, 60);

    let state = AppState {
        mpp: Arc::new(mpp),
        firecrawl_api_key,
        http,
        proxy_config,
        rate_limiter: rate_limiter.clone(),
    };

    // CORS: restrictive in production, permissive in dev
    let cors = match cors_origin {
        Some(ref origin) => CorsLayer::new()
            .allow_origin(AllowOrigin::exact(origin.parse().expect("invalid CORS_ORIGIN")))
            .allow_methods([axum::http::Method::GET, axum::http::Method::POST])
            .allow_headers([header::CONTENT_TYPE, header::AUTHORIZATION]),
        None => CorsLayer::permissive(),
    };

    let app = Router::new()
        // Service discovery
        .route("/api/services", get(services))
        .route("/api/llms.txt", get(llms_txt))
        // Free
        .route("/api/health", get(health))
        // Paid endpoints
        .route("/api/scrape", post(scrape))
        .route("/api/crawl", post(crawl))
        .layer(cors)
        .layer(RequestBodyLimitLayer::new(1024 * 64)) // 64KB max request body
        .with_state(state);

    let listener = tokio::net::TcpListener::bind(&bind_addr)
        .await
        .unwrap_or_else(|e| panic!("failed to bind to {bind_addr}: {e}"));

    println!("Listening on http://{bind_addr}");
    println!("  GET  /api/health    — free");
    println!("  POST /api/scrape    — 0.01 MOVE");
    println!("  POST /api/crawl     — 0.05 MOVE");
    println!("  GET  /api/services  — discovery");

    // Spawn cleanup task for rate limiter
    let cleanup_limiter = rate_limiter;
    tokio::spawn(async move {
        loop {
            tokio::time::sleep(Duration::from_secs(300)).await;
            cleanup_limiter.cleanup();
        }
    });

    // Graceful shutdown on Ctrl+C
    axum::serve(
        listener,
        app.into_make_service_with_connect_info::<SocketAddr>(),
    )
    .with_graceful_shutdown(shutdown_signal())
    .await
    .expect("server error");
}

async fn shutdown_signal() {
    tokio::signal::ctrl_c()
        .await
        .expect("failed to install signal handler");
    println!("\nShutting down...");
}

// --- Handlers ---

async fn health() -> Json<serde_json::Value> {
    Json(serde_json::json!({ "status": "ok" }))
}

async fn services(State(state): State<AppState>) -> Json<serde_json::Value> {
    Json(mpp::proxy::service::serialize_services(
        &state.proxy_config.services,
    ))
}

async fn llms_txt(State(state): State<AppState>) -> impl IntoResponse {
    let txt = mpp::proxy::service::to_llms_txt(&state.proxy_config.services);
    ([(header::CONTENT_TYPE, "text/plain")], txt)
}

async fn scrape(
    State(state): State<AppState>,
    ConnectInfo(addr): ConnectInfo<SocketAddr>,
    headers: HeaderMap,
    charge: MppCharge<ScrapeFee>,
    Json(body): Json<serde_json::Value>,
) -> impl IntoResponse {
    // Rate limit check
    let ip = get_client_ip(&headers, &addr);
    if !state.rate_limiter.check(&ip) {
        return (
            StatusCode::TOO_MANY_REQUESTS,
            Json(serde_json::json!({ "error": "Rate limit exceeded. Try again in a minute." })),
        )
            .into_response();
    }

    // Validate URL
    let raw_url = body
        .get("url")
        .and_then(|v| v.as_str())
        .unwrap_or("https://example.com");

    let url = match validate_scrape_url(raw_url) {
        Ok(u) => u,
        Err(e) => {
            return (
                StatusCode::BAD_REQUEST,
                Json(serde_json::json!({ "error": e })),
            )
                .into_response();
        }
    };

    // Forward to Firecrawl
    let result = state
        .http
        .post(format!("{FIRECRAWL_BASE}/v1/scrape"))
        .bearer_auth(&state.firecrawl_api_key)
        .json(&serde_json::json!({
            "url": url,
            "formats": ["markdown"],
        }))
        .send()
        .await;

    match result {
        Ok(resp) => {
            let status = resp.status();
            let body: serde_json::Value = resp.json().await.unwrap_or_else(|_| {
                serde_json::json!({ "error": "failed to parse Firecrawl response" })
            });

            if status.is_success() {
                WithReceipt {
                    receipt: charge.receipt,
                    body: Json(body),
                }
                .into_response()
            } else {
                (StatusCode::BAD_GATEWAY, Json(body)).into_response()
            }
        }
        Err(e) => (
            StatusCode::BAD_GATEWAY,
            Json(serde_json::json!({ "error": format!("Firecrawl request failed: {e}") })),
        )
            .into_response(),
    }
}

async fn crawl(
    State(state): State<AppState>,
    ConnectInfo(addr): ConnectInfo<SocketAddr>,
    headers: HeaderMap,
    charge: MppCharge<CrawlFee>,
    Json(body): Json<serde_json::Value>,
) -> impl IntoResponse {
    // Rate limit check
    let ip = get_client_ip(&headers, &addr);
    if !state.rate_limiter.check(&ip) {
        return (
            StatusCode::TOO_MANY_REQUESTS,
            Json(serde_json::json!({ "error": "Rate limit exceeded. Try again in a minute." })),
        )
            .into_response();
    }

    // Validate URL
    let raw_url = body
        .get("url")
        .and_then(|v| v.as_str())
        .unwrap_or("https://example.com");

    let url = match validate_scrape_url(raw_url) {
        Ok(u) => u,
        Err(e) => {
            return (
                StatusCode::BAD_REQUEST,
                Json(serde_json::json!({ "error": e })),
            )
                .into_response();
        }
    };

    // Clamp crawl limit to prevent abuse
    let limit = body
        .get("limit")
        .and_then(|v| v.as_u64())
        .unwrap_or(5)
        .min(20);

    let result = state
        .http
        .post(format!("{FIRECRAWL_BASE}/v1/crawl"))
        .bearer_auth(&state.firecrawl_api_key)
        .json(&serde_json::json!({
            "url": url,
            "limit": limit,
        }))
        .send()
        .await;

    match result {
        Ok(resp) => {
            let status = resp.status();
            let body: serde_json::Value = resp.json().await.unwrap_or_else(|_| {
                serde_json::json!({ "error": "failed to parse Firecrawl response" })
            });

            if status.is_success() {
                WithReceipt {
                    receipt: charge.receipt,
                    body: Json(body),
                }
                .into_response()
            } else {
                (StatusCode::BAD_GATEWAY, Json(body)).into_response()
            }
        }
        Err(e) => (
            StatusCode::BAD_GATEWAY,
            Json(serde_json::json!({ "error": format!("Firecrawl request failed: {e}") })),
        )
            .into_response(),
    }
}

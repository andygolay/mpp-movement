//! # Search → Image → Email
//!
//! A multi-service MPP demo that chains three paid APIs in one request:
//!
//! 1. **Exa** — web search for a topic
//! 2. **fal.ai** — generate an AI image summarizing the results
//! 3. **Resend** — email the search results + image to a recipient
//!
//! User pays once (0.05 MOVE), the server orchestrates all three calls.
//!
//! ## Running
//!
//! ```bash
//! cp .env.example .env
//! # Fill in EXA_API_KEY, FAL_API_KEY, RESEND_API_KEY, RECIPIENT
//! cargo run --bin search-image-email
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

// --- Rate limiter (same as firecrawl-proxy) ---

#[derive(Clone)]
struct RateLimiter {
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

    fn cleanup(&self) {
        let mut windows = self.windows.lock().unwrap();
        let now = Instant::now();
        let window_duration = Duration::from_secs(self.window_secs);
        windows.retain(|_, (_, start)| now.duration_since(*start) <= window_duration);
    }
}

fn get_client_ip(headers: &HeaderMap, addr: &SocketAddr) -> String {
    if let Some(xff) = headers.get("x-forwarded-for") {
        if let Ok(s) = xff.to_str() {
            if let Some(first) = s.split(',').next() {
                return first.trim().to_string();
            }
        }
    }
    addr.ip().to_string()
}

// --- App state ---

#[derive(Clone)]
struct AppState {
    mpp: Arc<dyn ChargeChallenger>,
    exa_api_key: String,
    fal_api_key: String,
    resend_api_key: String,
    from_email: String,
    http: reqwest::Client,
    proxy_config: ProxyConfig,
    rate_limiter: RateLimiter,
}

impl FromRef<AppState> for Arc<dyn ChargeChallenger> {
    fn from_ref(state: &AppState) -> Self {
        Arc::clone(&state.mpp)
    }
}

// --- Charge config ---

/// 0.05 MOVE for the full pipeline (search + image gen + email)
struct PipelineFee;
impl ChargeConfig for PipelineFee {
    fn amount() -> &'static str {
        "0.05"
    }
    fn description() -> Option<&'static str> {
        Some("Search + AI image + email delivery")
    }
}

// --- Request/response types ---

#[derive(serde::Deserialize)]
struct PipelineRequest {
    /// What to search for (e.g. "20 scientific studies supporting carnosine")
    query: String,
    /// Email address to send results to
    email: String,
    /// Override number of results (if not set, extracted from query text)
    num_results: Option<u32>,
}

/// Try to extract a number from the query like "20 things to do" or "find 15 studies".
fn extract_num_from_query(query: &str) -> Option<u32> {
    // Match patterns like "20 things", "find 15 studies", "top 10 restaurants"
    let words: Vec<&str> = query.split_whitespace().collect();
    for (i, word) in words.iter().enumerate() {
        if let Ok(n) = word.parse::<u32>() {
            if n >= 1 && n <= 50 {
                // Check it's likely a count, not a year or other number
                // Skip if it looks like a year (4 digits, starts with 19 or 20)
                if n >= 1900 && n <= 2100 {
                    continue;
                }
                // Check if preceded by "top", "find", "list", or followed by a noun-ish word
                let prev = if i > 0 { words[i - 1].to_lowercase() } else { String::new() };
                let next = if i + 1 < words.len() { words[i + 1].to_lowercase() } else { String::new() };
                if ["top", "find", "list", "get", "show", "give"].contains(&prev.as_str())
                    || !next.is_empty()
                {
                    return Some(n);
                }
            }
        }
    }
    None
}

#[derive(serde::Serialize)]
struct PipelineResponse {
    search_results: Vec<SearchResult>,
    image_url: Option<String>,
    email_sent_to: Option<String>,
    email_id: Option<String>,
    /// Which steps completed successfully
    steps_completed: Vec<String>,
    /// If a step failed, what happened (user still gets partial results)
    partial_failure: Option<String>,
}

#[derive(serde::Serialize, Clone)]
struct SearchResult {
    title: String,
    url: String,
    summary: String,
}

#[tokio::main]
async fn main() {
    dotenvy::dotenv().ok();

    let exa_api_key = std::env::var("EXA_API_KEY").expect("EXA_API_KEY must be set in .env");
    let fal_api_key = std::env::var("FAL_API_KEY").expect("FAL_API_KEY must be set in .env");
    let resend_api_key =
        std::env::var("RESEND_API_KEY").expect("RESEND_API_KEY must be set in .env");

    let from_email = std::env::var("FROM_EMAIL")
        .unwrap_or_else(|_| "MPP Search <onboarding@resend.dev>".to_string());

    let recipient = std::env::var("RECIPIENT").unwrap_or_else(|_| {
        eprintln!("WARNING: Using default RECIPIENT. Set RECIPIENT in .env for production.");
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
        generated
    });

    let rest_url = std::env::var("MOVEMENT_REST_URL")
        .unwrap_or_else(|_| "https://testnet.movementnetwork.xyz/v1".to_string());
    let bind_addr = std::env::var("BIND_ADDR").unwrap_or_else(|_| "0.0.0.0:3020".to_string());
    let cors_origin = std::env::var("CORS_ORIGIN").ok();
    let rate_limit: u32 = std::env::var("RATE_LIMIT_PER_MINUTE")
        .ok()
        .and_then(|v| v.parse().ok())
        .unwrap_or(20);

    let mpp = Mpp::create_movement(
        movement(MovementConfig {
            recipient: &recipient,
        })
        .rest_url(&rest_url)
        .secret_key(&secret_key)
        .decimals(8),
    )
    .expect("failed to create Movement MPP handler");

    let proxy_config = ProxyConfig {
        base_path: Some("/api".to_string()),
        services: vec![Service::new("search-image-email", "http://localhost:3020")
            .route(
                "POST /v1/run",
                Endpoint::Paid(PaidEndpoint {
                    intent: "charge".into(),
                    amount: "0.05".into(),
                    unit_type: None,
                    description: Some(
                        "Search the web, generate an AI image, and email the results".into(),
                    ),
                }),
            )
            .build()],
    };

    let is_testnet = rest_url.contains("testnet");
    println!("Search → Image → Email (Movement MPP)");
    println!("  Network:    {}", if is_testnet { "testnet" } else { "mainnet" });
    println!("  Recipient:  {recipient}");
    println!("  Services:   Exa + fal.ai + Resend");
    println!("  Rate limit: {rate_limit} req/min per IP");
    println!();

    let http = reqwest::Client::builder()
        .timeout(Duration::from_secs(60)) // image gen can be slow
        .connect_timeout(Duration::from_secs(5))
        .build()
        .expect("failed to create HTTP client");

    let rate_limiter = RateLimiter::new(rate_limit, 60);

    let state = AppState {
        mpp: Arc::new(mpp),
        exa_api_key,
        fal_api_key,
        resend_api_key,
        from_email,
        http,
        proxy_config,
        rate_limiter: rate_limiter.clone(),
    };

    let cors = match cors_origin {
        Some(ref origin) => CorsLayer::new()
            .allow_origin(AllowOrigin::exact(origin.parse().expect("invalid CORS_ORIGIN")))
            .allow_methods([axum::http::Method::GET, axum::http::Method::POST])
            .allow_headers([header::CONTENT_TYPE, header::AUTHORIZATION]),
        None => CorsLayer::permissive(),
    };

    let app = Router::new()
        .route("/api/services", get(services))
        .route("/api/llms.txt", get(llms_txt))
        .route("/api/health", get(health))
        .route("/api/run", post(run_pipeline))
        .layer(cors)
        .layer(RequestBodyLimitLayer::new(1024 * 16))
        .with_state(state);

    let listener = tokio::net::TcpListener::bind(&bind_addr)
        .await
        .unwrap_or_else(|e| panic!("failed to bind to {bind_addr}: {e}"));

    println!("Listening on http://{bind_addr}");
    println!("  GET  /api/health   — free");
    println!("  POST /api/run      — 0.05 MOVE (search + image + email)");
    println!("  GET  /api/services — discovery");

    let cleanup = rate_limiter;
    tokio::spawn(async move {
        loop {
            tokio::time::sleep(Duration::from_secs(300)).await;
            cleanup.cleanup();
        }
    });

    axum::serve(
        listener,
        app.into_make_service_with_connect_info::<SocketAddr>(),
    )
    .with_graceful_shutdown(async {
        tokio::signal::ctrl_c().await.ok();
        println!("\nShutting down...");
    })
    .await
    .expect("server error");
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

async fn run_pipeline(
    State(state): State<AppState>,
    ConnectInfo(addr): ConnectInfo<SocketAddr>,
    headers: HeaderMap,
    charge: MppCharge<PipelineFee>,
    Json(body): Json<PipelineRequest>,
) -> impl IntoResponse {
    let ip = get_client_ip(&headers, &addr);
    if !state.rate_limiter.check(&ip) {
        return (
            StatusCode::TOO_MANY_REQUESTS,
            Json(serde_json::json!({ "error": "Rate limit exceeded" })),
        )
            .into_response();
    }

    // Validate email
    if !body.email.contains('@') || body.email.len() > 254 {
        return (
            StatusCode::BAD_REQUEST,
            Json(serde_json::json!({ "error": "Invalid email address" })),
        )
            .into_response();
    }

    // Validate query
    let query = body.query.trim();
    if query.is_empty() || query.len() > 500 {
        return (
            StatusCode::BAD_REQUEST,
            Json(serde_json::json!({ "error": "Query must be 1-500 characters" })),
        )
            .into_response();
    }

    let num_results = body
        .num_results
        .or_else(|| extract_num_from_query(query))
        .unwrap_or(10)
        .min(20);

    println!("  [pipeline] query={query:?} email={} results={num_results}", body.email);

    let mut steps_completed: Vec<String> = Vec::new();
    let mut partial_failure: Option<String> = None;

    // --- Step 1: Search with Exa (with retry) ---
    // Search is required — without results there's nothing to image or email.
    println!("  [1/3] Searching with Exa...");
    let search_results = match retry(|| exa_search(&state, query, num_results), 3).await {
        Ok(results) => {
            println!("  [1/3] Got {} results", results.len());
            steps_completed.push("search".into());
            results
        }
        Err(e) => {
            // Search is the foundation — if it fails, return error.
            // User paid but got nothing. Log it for manual resolution.
            eprintln!("  [1/3] FAILED after retries: {e}");
            return (
                StatusCode::BAD_GATEWAY,
                Json(serde_json::json!({
                    "error": format!("Search failed after retries: {e}"),
                    "steps_completed": steps_completed,
                    "note": "Payment was received. Contact support if you need a refund.",
                })),
            )
                .into_response();
        }
    };

    // --- Step 2: Generate image with fal.ai (with retry, optional) ---
    println!("  [2/3] Generating image with fal.ai...");
    let image_prompt = build_image_prompt(query, &search_results);
    let image_url = match retry(|| fal_generate_image(&state, &image_prompt), 2).await {
        Ok(url) => {
            println!("  [2/3] Image: {url}");
            steps_completed.push("image".into());
            Some(url)
        }
        Err(e) => {
            // Image failed but we still have search results — continue without image.
            eprintln!("  [2/3] FAILED after retries: {e}");
            partial_failure = Some(format!("Image generation failed: {e}"));
            None
        }
    };

    // --- Step 3: Email results with Resend (with retry, optional) ---
    println!("  [3/3] Sending email via Resend...");
    let html = build_email_html(query, &search_results, image_url.as_deref());
    let (email_sent_to, email_id) =
        match retry(|| resend_send(&state, &body.email, query, &html), 3).await {
            Ok(id) => {
                println!("  [3/3] Email sent: {id}");
                steps_completed.push("email".into());
                (Some(body.email.clone()), Some(id))
            }
            Err(e) => {
                // Email failed but we still have search results + maybe image.
                eprintln!("  [3/3] FAILED after retries: {e}");
                let msg = format!("Email delivery failed: {e}");
                partial_failure = Some(match partial_failure {
                    Some(prev) => format!("{prev}; {msg}"),
                    None => msg,
                });
                (None, None)
            }
        };

    let response = PipelineResponse {
        search_results,
        image_url,
        email_sent_to,
        email_id,
        steps_completed,
        partial_failure,
    };

    WithReceipt {
        receipt: charge.receipt,
        body: Json(serde_json::json!(response)),
    }
    .into_response()
}

// --- Retry helper ---

async fn retry<F, Fut, T>(f: F, max_attempts: u32) -> Result<T, String>
where
    F: Fn() -> Fut,
    Fut: std::future::Future<Output = Result<T, String>>,
{
    let mut last_err = String::new();
    for attempt in 1..=max_attempts {
        match f().await {
            Ok(val) => return Ok(val),
            Err(e) => {
                last_err = e;
                if attempt < max_attempts {
                    let delay = Duration::from_millis(500 * attempt as u64);
                    eprintln!("    retry {attempt}/{max_attempts} failed, waiting {delay:?}...");
                    tokio::time::sleep(delay).await;
                }
            }
        }
    }
    Err(last_err)
}

// --- Query intelligence ---

// --- Service calls ---

async fn exa_search(
    state: &AppState,
    query: &str,
    num_results: u32,
) -> Result<Vec<SearchResult>, String> {
    let resp = state
        .http
        .post("https://api.exa.ai/search")
        .header("x-api-key", &state.exa_api_key)
        .json(&serde_json::json!({
            "query": query,
            "numResults": num_results,
            "type": "deep-reasoning",
            "systemPrompt": "Find the most relevant, authoritative sources for this query. \
                If the query asks for scientific studies, research, or evidence, prefer \
                peer-reviewed journals, PubMed, and academic sources. \
                If the query asks for things to do, places, or recommendations, prefer \
                well-known travel and review sites. \
                Always prefer primary sources over aggregator or SEO content. \
                Avoid duplicate results.",
            "contents": {
                "text": {
                    "maxCharacters": 300,
                },
                "highlights": {
                    "numSentences": 3,
                    "maxCharacters": 300,
                }
            }
        }))
        .send()
        .await
        .map_err(|e| e.to_string())?;

    if !resp.status().is_success() {
        let status = resp.status();
        let body = resp.text().await.unwrap_or_default();
        return Err(format!("Exa returned {status}: {body}"));
    }

    let body: serde_json::Value = resp.json().await.map_err(|e| e.to_string())?;

    let results = body["results"]
        .as_array()
        .map(|arr| {
            arr.iter()
                .map(|r| {
                    let highlights = r["highlights"]
                        .as_array()
                        .map(|h| {
                            h.iter()
                                .filter_map(|s| s.as_str())
                                .collect::<Vec<_>>()
                                .join(" ")
                        })
                        .unwrap_or_default();

                    SearchResult {
                        title: r["title"].as_str().unwrap_or("Untitled").to_string(),
                        url: r["url"].as_str().unwrap_or("").to_string(),
                        summary: if highlights.is_empty() {
                            r["text"]
                                .as_str()
                                .unwrap_or("")
                                .chars()
                                .take(200)
                                .collect()
                        } else {
                            highlights
                        },
                    }
                })
                .collect()
        })
        .unwrap_or_default();

    Ok(results)
}

fn build_image_prompt(query: &str, results: &[SearchResult]) -> String {
    let titles: Vec<&str> = results.iter().map(|r| r.title.as_str()).take(5).collect();
    format!(
        "A vibrant, colorful illustration summarizing: {}. Key topics: {}. \
         Style: modern digital art, clean composition, no text.",
        query,
        titles.join(", ")
    )
}

async fn fal_generate_image(state: &AppState, prompt: &str) -> Result<String, String> {
    let resp = state
        .http
        .post("https://queue.fal.run/fal-ai/flux/schnell")
        .header("Authorization", format!("Key {}", state.fal_api_key))
        .json(&serde_json::json!({
            "prompt": prompt,
            "image_size": "landscape_16_9",
            "num_images": 1,
        }))
        .send()
        .await
        .map_err(|e| e.to_string())?;

    if !resp.status().is_success() {
        let status = resp.status();
        let body = resp.text().await.unwrap_or_default();
        return Err(format!("fal.ai returned {status}: {body}"));
    }

    let body: serde_json::Value = resp.json().await.map_err(|e| e.to_string())?;

    body["images"][0]["url"]
        .as_str()
        .map(|s| s.to_string())
        .ok_or_else(|| "No image URL in fal.ai response".to_string())
}

fn build_email_html(query: &str, results: &[SearchResult], image_url: Option<&str>) -> String {
    let mut items = String::new();
    for (i, r) in results.iter().enumerate() {
        items.push_str(&format!(
            r#"<tr>
                <td style="padding:12px 16px;border-bottom:1px solid #eee;">
                    <strong style="color:#111;">{num}. {title}</strong><br/>
                    <span style="color:#666;font-size:14px;">{summary}</span><br/>
                    <a href="{url}" style="color:#6366f1;font-size:13px;">{url}</a>
                </td>
            </tr>"#,
            num = i + 1,
            title = html_escape(&r.title),
            summary = html_escape(&r.summary),
            url = html_escape(&r.url),
        ));
    }

    format!(
        r#"<!DOCTYPE html>
<html>
<head><meta charset="utf-8"/></head>
<body style="font-family:-apple-system,sans-serif;max-width:600px;margin:0 auto;padding:20px;background:#f9f9fb;">
    <div style="background:white;border-radius:12px;overflow:hidden;box-shadow:0 1px 3px rgba(0,0,0,0.1);">
        {image_tag}
        <div style="padding:24px;">
            <h1 style="font-size:20px;color:#111;margin:0 0 4px;">{query}</h1>
            <p style="color:#888;font-size:13px;margin:0 0 20px;">Search results powered by MPP on Movement Network</p>
            <table style="width:100%;border-collapse:collapse;">
                {items}
            </table>
        </div>
    </div>
    <p style="text-align:center;color:#aaa;font-size:12px;margin-top:16px;">
        Sent via Machine Payments Protocol — paid with MOVE
    </p>
</body>
</html>"#,
        image_tag = image_url
            .map(|u| format!(r#"<img src="{}" alt="AI generated image" style="width:100%;height:auto;display:block;"/>"#, u))
            .unwrap_or_default(),
        query = html_escape(query),
        items = items,
    )
}

fn html_escape(s: &str) -> String {
    s.replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
        .replace('"', "&quot;")
}

async fn resend_send(
    state: &AppState,
    to: &str,
    subject: &str,
    html: &str,
) -> Result<String, String> {
    let resp = state
        .http
        .post("https://api.resend.com/emails")
        .header("Authorization", format!("Bearer {}", state.resend_api_key))
        .json(&serde_json::json!({
            "from": state.from_email,
            "to": [to],
            "subject": format!("Search results: {}", subject),
            "html": html,
        }))
        .send()
        .await
        .map_err(|e| e.to_string())?;

    if !resp.status().is_success() {
        let status = resp.status();
        let body = resp.text().await.unwrap_or_default();
        return Err(format!("Resend returned {status}: {body}"));
    }

    let body: serde_json::Value = resp.json().await.map_err(|e| e.to_string())?;
    Ok(body["id"].as_str().unwrap_or("unknown").to_string())
}

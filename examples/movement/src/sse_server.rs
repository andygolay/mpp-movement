//! # Pay-Per-Token LLM Streaming Server
//!
//! Demonstrates pay-per-token AI streaming using Server-Sent Events (SSE)
//! with Movement payment channels on testnet.
//!
//! The server has its own funded wallet and settles vouchers on-chain
//! every 5 vouchers, then closes the channel when the client disconnects.
//!
//! ## Running
//!
//! ```bash
//! cargo run --bin movement-sse-server
//! ```

use axum::{
    body::Body,
    extract::{Query, State},
    http::{header, Method, StatusCode},
    response::IntoResponse,
    routing::get,
    Json, Router,
};
use tower_http::cors::{Any, CorsLayer};
use ed25519_dalek::SigningKey;
use mpp::protocol::methods::movement::rest_client::{EntryFunctionPayload, MovementRestClient};
use mpp::protocol::methods::movement::{self, voucher};
use mpp::format_www_authenticate;
use sha3::{Digest, Sha3_256};
use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::time::Duration;
use tokio_stream::StreamExt;

const REST_URL: &str = "https://testnet.movementnetwork.xyz/v1";
const FAUCET_URL: &str = "https://faucet.testnet.movementnetwork.xyz";
const MODULE_ADDRESS: &str =
    "0x3e9edf3be513781a6db0706b652da425ad67f58b5cb366847126bf0fb716fc58";

/// Defaults for MOVE (8 decimals). Override with env vars for other tokens.
const DEFAULT_PRICE_PER_TOKEN: u64 = 1_000;
const DEFAULT_SUGGESTED_DEPOSIT: u64 = 100_000;
const DEFAULT_TOKEN_METADATA: &str = "0xa";

/// Settle on-chain every N vouchers.
const SETTLE_EVERY: u32 = 5;

struct AppState {
    secret_key: String,
    realm: String,
    /// Server's (payee) address.
    server_address: String,
    /// Token FA metadata address.
    token_metadata: String,
    /// Price per token in base units.
    price_per_token: u64,
    /// Suggested deposit for a session.
    suggested_deposit: u64,
    /// Server's signing key for on-chain settlement.
    server_key: SigningKey,
    rest_client: MovementRestClient,
    /// Tracks channel state.
    channels: std::sync::Mutex<std::collections::HashMap<String, ChannelSession>>,
}

#[derive(Clone)]
struct ChannelSession {
    authorized_pubkey: Vec<u8>,
    highest_cumulative: u64,
    highest_signature: Vec<u8>,
    tokens_delivered: usize,
    voucher_count: u32,
    settled_cumulative: u64,
    settle_tx_hashes: Vec<String>,
}

#[derive(serde::Deserialize)]
struct ChatQuery {
    prompt: Option<String>,
    channel_id: Option<String>,
    cumulative_amount: Option<String>,
    signature: Option<String>,
    pubkey: Option<String>,
}

fn derive_address(pubkey: &[u8; 32]) -> String {
    let mut hasher = Sha3_256::new();
    hasher.update(pubkey);
    hasher.update([0x00]);
    format!("0x{}", hex::encode(hasher.finalize()))
}

#[tokio::main]
async fn main() {
    let _ = dotenvy::dotenv();

    // Generate a server wallet (the payee).
    let server_key = SigningKey::from_bytes(&rand::random());
    let server_pubkey = server_key.verifying_key().to_bytes();
    let server_address = derive_address(&server_pubkey);
    let rest_client = MovementRestClient::new(REST_URL);

    // Token configuration via env vars (defaults to MOVE).
    // For USDCx (6 decimals): TOKEN_METADATA=0x63f1...  PRICE_PER_TOKEN=10  SUGGESTED_DEPOSIT=10000
    let token_metadata = std::env::var("TOKEN_METADATA")
        .unwrap_or_else(|_| DEFAULT_TOKEN_METADATA.to_string());
    let price_per_token: u64 = std::env::var("PRICE_PER_TOKEN")
        .ok()
        .and_then(|v| v.parse().ok())
        .unwrap_or(DEFAULT_PRICE_PER_TOKEN);
    let suggested_deposit: u64 = std::env::var("SUGGESTED_DEPOSIT")
        .ok()
        .and_then(|v| v.parse().ok())
        .unwrap_or(DEFAULT_SUGGESTED_DEPOSIT);

    println!("Movement Pay-Per-Token LLM API");
    println!("  Server (payee): {server_address}");
    println!("  Token metadata: {token_metadata}");
    println!("  Price per token: {price_per_token} base units");
    println!("  Suggested deposit: {suggested_deposit} base units");
    println!("  Settles every {SETTLE_EVERY} vouchers\n");

    // Fund server from faucet.
    println!("Funding server from faucet...");
    let http = reqwest::Client::new();
    let resp = http
        .post(&format!("{}/mint?amount=100000000&address={}", FAUCET_URL, server_address))
        .send()
        .await
        .expect("faucet failed");
    assert!(resp.status().is_success());
    tokio::time::sleep(Duration::from_secs(2)).await;

    // Migrate to FA.
    let migrate = EntryFunctionPayload::new("0x1::coin::migrate_to_fungible_store", vec![])
        .with_type_arguments(vec!["0x1::aptos_coin::AptosCoin".to_string()]);
    rest_client
        .build_sign_submit(&server_key, &server_address, migrate)
        .await
        .expect("migration failed");
    println!("Server funded.\n");

    let state = Arc::new(AppState {
        secret_key: std::env::var("MPP_SECRET_KEY")
            .unwrap_or_else(|_| "sse-example-secret".to_string()),
        realm: "movement-llm-api".to_string(),
        server_address,
        server_key,
        rest_client,
        token_metadata,
        price_per_token,
        suggested_deposit,
        channels: std::sync::Mutex::new(std::collections::HashMap::new()),
    });

    let cors = CorsLayer::new()
        .allow_origin(Any)
        .allow_methods([Method::GET, Method::POST, Method::OPTIONS])
        .allow_headers(Any)
        .expose_headers([header::WWW_AUTHENTICATE]);

    let app = Router::new()
        .route("/api/health", get(health))
        .route("/api/chat", get(chat))
        .route("/api/close", get(close_channel))
        .layer(cors)
        .with_state(state);

    let listener = tokio::net::TcpListener::bind("0.0.0.0:3001")
        .await
        .expect("failed to bind");

    println!("Listening on http://localhost:3001");
    axum::serve(listener, app).await.expect("server error");
}

async fn health() -> impl IntoResponse {
    Json(serde_json::json!({ "status": "ok" }))
}

async fn chat(
    State(state): State<Arc<AppState>>,
    Query(query): Query<ChatQuery>,
) -> impl IntoResponse {
    let prompt = query.prompt.unwrap_or_else(|| "Hello!".to_string());

    // No channel_id → return 402.
    let channel_id = match &query.channel_id {
        Some(id) => id.clone(),
        None => {
            let challenge = movement::charge_challenge(
                &state.secret_key,
                &state.realm,
                &state.suggested_deposit.to_string(),
                &state.token_metadata,
                &state.server_address,
            )
            .expect("failed to create challenge");

            let www_auth = format_www_authenticate(&challenge).expect("failed to format");
            return (
                StatusCode::PAYMENT_REQUIRED,
                [(header::WWW_AUTHENTICATE, www_auth)],
                Json(serde_json::json!({
                    "error": "Payment Required",
                    "price_per_token": state.price_per_token.to_string(),
                    "suggested_deposit": state.suggested_deposit.to_string(),
                    "recipient": state.server_address,
                    "token": state.token_metadata,
                })),
            )
                .into_response();
        }
    };

    // Parse voucher params.
    let cumulative: u64 = match query.cumulative_amount.as_deref().and_then(|s| s.parse().ok()) {
        Some(v) => v,
        None => {
            return (StatusCode::BAD_REQUEST, Json(serde_json::json!({
                "error": "missing or invalid cumulative_amount"
            })))
                .into_response();
        }
    };

    let sig_hex = match &query.signature {
        Some(s) => s.clone(),
        None => {
            return (StatusCode::BAD_REQUEST, Json(serde_json::json!({
                "error": "missing signature"
            })))
                .into_response();
        }
    };

    let sig_clean = sig_hex.strip_prefix("0x").unwrap_or(&sig_hex);
    let sig_bytes: [u8; 64] = match hex::decode(sig_clean) {
        Ok(b) if b.len() == 64 => b.try_into().unwrap(),
        _ => {
            return (StatusCode::BAD_REQUEST, Json(serde_json::json!({
                "error": "signature must be 64 bytes hex"
            })))
                .into_response();
        }
    };

    let channel_id_bytes = {
        let clean = channel_id.strip_prefix("0x").unwrap_or(&channel_id);
        match hex::decode(clean) {
            Ok(b) => b,
            Err(_) => {
                return (StatusCode::BAD_REQUEST, Json(serde_json::json!({
                    "error": "invalid channel_id hex"
                })))
                    .into_response();
            }
        }
    };

    // Get or create session.
    let mut channels = state.channels.lock().unwrap();
    let session = channels.entry(channel_id.clone()).or_insert_with(|| {
        let pubkey_hex = query.pubkey.as_deref().unwrap_or("");
        let clean = pubkey_hex.strip_prefix("0x").unwrap_or(pubkey_hex);
        ChannelSession {
            authorized_pubkey: hex::decode(clean).unwrap_or_default(),
            highest_cumulative: 0,
            highest_signature: vec![],
            tokens_delivered: 0,
            voucher_count: 0,
            settled_cumulative: 0,
            settle_tx_hashes: vec![],
        }
    });

    if cumulative <= session.highest_cumulative {
        return (StatusCode::BAD_REQUEST, Json(serde_json::json!({
            "error": "cumulative_amount must increase",
            "highest": session.highest_cumulative.to_string(),
        })))
            .into_response();
    }

    let pubkey: [u8; 32] = match session.authorized_pubkey.clone().try_into() {
        Ok(pk) => pk,
        Err(_) => {
            return (StatusCode::BAD_REQUEST, Json(serde_json::json!({
                "error": "pubkey must be 32 bytes; send pubkey= on first request"
            })))
                .into_response();
        }
    };

    let valid = voucher::verify_voucher(
        &channel_id_bytes,
        cumulative,
        &sig_bytes,
        &pubkey,
        &session.authorized_pubkey,
    );

    if !valid {
        return (StatusCode::PAYMENT_REQUIRED, Json(serde_json::json!({
            "error": "invalid voucher signature"
        })))
            .into_response();
    }

    // Accept voucher.
    let delta = cumulative - session.highest_cumulative;
    let tokens_bought = delta / state.price_per_token;
    session.highest_cumulative = cumulative;
    session.highest_signature = sig_bytes.to_vec();
    session.voucher_count += 1;
    let token_offset = session.tokens_delivered;
    session.tokens_delivered += tokens_bought as usize;
    let voucher_count = session.voucher_count;
    let should_settle = voucher_count % SETTLE_EVERY == 0;
    let pubkey_for_settle = session.authorized_pubkey.clone();
    let settled_so_far = session.settled_cumulative;
    drop(channels);

    if tokens_bought == 0 {
        return (StatusCode::BAD_REQUEST, Json(serde_json::json!({
            "error": "voucher delta too small",
            "price_per_token": state.price_per_token.to_string(),
        })))
            .into_response();
    }

    // Settle on-chain periodically.
    if should_settle && cumulative > settled_so_far {
        let state_clone = state.clone();
        let channel_id_clone = channel_id.clone();
        let channel_id_bytes_clone = channel_id_bytes.clone();
        let sig_clone = sig_bytes;
        let pubkey_clone = pubkey_for_settle;
        tokio::spawn(async move {
            println!(
                "  [settle] channel {}... voucher #{}, cumulative={}",
                &channel_id_clone[..16],
                voucher_count,
                cumulative,
            );
            let payload = EntryFunctionPayload::new(
                &format!("{}::channel::settle", MODULE_ADDRESS),
                vec![
                    serde_json::json!(MODULE_ADDRESS),
                    serde_json::json!(format!("0x{}", hex::encode(&channel_id_bytes_clone))),
                    serde_json::json!(cumulative.to_string()),
                    serde_json::json!(format!("0x{}", hex::encode(sig_clone))),
                    serde_json::json!(format!("0x{}", hex::encode(&pubkey_clone))),
                ],
            );

            match state_clone
                .rest_client
                .build_sign_submit(&state_clone.server_key, &state_clone.server_address, payload)
                .await
            {
                Ok(tx) => {
                    println!("  [settle] tx: {tx}");
                    let mut channels = state_clone.channels.lock().unwrap();
                    if let Some(session) = channels.get_mut(&channel_id_clone) {
                        session.settled_cumulative = cumulative;
                        session.settle_tx_hashes.push(tx);
                    }
                }
                Err(e) => eprintln!("  [settle] failed: {e}"),
            }
        });
    }

    // Stream tokens as SSE, tracking actual delivery.
    let tokens = generate_tokens(&prompt, token_offset, tokens_bought as usize);
    let actual_count = Arc::new(AtomicUsize::new(0));
    let actual_count_stream = actual_count.clone();
    let actual_count_done = actual_count.clone();
    let state_done = state.clone();
    let channel_id_done = channel_id.clone();

    let event_stream = async_stream::stream! {
        let mut stream = std::pin::pin!(tokens);
        let mut count = 0u64;
        while let Some(token) = StreamExt::next(&mut stream).await {
            count += 1;
            actual_count_stream.store(count as usize, Ordering::Relaxed);
            yield Ok::<_, std::convert::Infallible>(
                format!("data: {}\n\n", serde_json::json!({"token": token, "index": count}))
            );
        }
        // Correct tokens_delivered to reflect actual count.
        let actually_streamed = count as usize;
        actual_count_done.store(actually_streamed, Ordering::Relaxed);
        {
            let mut channels = state_done.channels.lock().unwrap();
            if let Some(session) = channels.get_mut(&channel_id_done) {
                let overshoot = tokens_bought as usize - actually_streamed;
                if overshoot > 0 {
                    session.tokens_delivered = session.tokens_delivered.saturating_sub(overshoot);
                }
            }
        }
        yield Ok(format!(
            "event: receipt\ndata: {}\n\n",
            serde_json::json!({
                "tokens_streamed": count,
                "cumulative_paid": cumulative.to_string(),
                "channel_id": channel_id,
                "voucher_number": voucher_count,
            })
        ));
    };

    let body = Body::from_stream(event_stream);
    let mut response = body.into_response();
    response.headers_mut().insert(header::CONTENT_TYPE, "text/event-stream".parse().unwrap());
    response.headers_mut().insert(header::CACHE_CONTROL, "no-cache".parse().unwrap());
    response
}

#[derive(serde::Deserialize)]
struct CloseQuery {
    channel_id: String,
}

async fn close_channel(
    State(state): State<Arc<AppState>>,
    Query(query): Query<CloseQuery>,
) -> Json<serde_json::Value> {
    let session = {
        let channels = state.channels.lock().unwrap();
        match channels.get(&query.channel_id) {
            Some(s) => s.clone(),
            None => {
                return Json(serde_json::json!({ "error": "channel not found" }));
            }
        }
    }; // lock dropped here

    if session.highest_cumulative == 0 {
        return Json(serde_json::json!({ "error": "no vouchers to settle" }));
    }

    let channel_id_bytes = hex::decode(
        query.channel_id.strip_prefix("0x").unwrap_or(&query.channel_id),
    ).unwrap_or_default();

    // Settle only for tokens actually delivered, not the full voucher amount.
    let fair_amount = (session.tokens_delivered as u64) * state.price_per_token;
    let settle_amount = fair_amount.min(session.highest_cumulative);

    println!(
        "  [close] channel {}... tokens_delivered={}, fair_amount={}, voucher_cumulative={}",
        &query.channel_id[..16], session.tokens_delivered, fair_amount, session.highest_cumulative
    );

    let payload = EntryFunctionPayload::new(
        &format!("{}::channel::close", MODULE_ADDRESS),
        vec![
            serde_json::json!(MODULE_ADDRESS),
            serde_json::json!(format!("0x{}", hex::encode(&channel_id_bytes))),
            serde_json::json!(settle_amount.to_string()),
            serde_json::json!(format!("0x{}", hex::encode(&session.highest_signature))),
            serde_json::json!(format!("0x{}", hex::encode(&session.authorized_pubkey))),
        ],
    );

    let deposit = session.highest_cumulative;
    match state.rest_client.build_sign_submit(&state.server_key, &state.server_address, payload).await {
        Ok(tx) => {
            println!("  [close] tx: {tx}");
            Json(serde_json::json!({
                "status": "closed",
                "close_tx": tx,
                "settled": settle_amount.to_string(),
                "deposit": deposit.to_string(),
                "tokens_delivered": session.tokens_delivered,
                "refund": (deposit - settle_amount).to_string(),
                "settle_txns": session.settle_tx_hashes,
                "vouchers_received": session.voucher_count,
            }))
        }
        Err(e) => {
            eprintln!("  [close] failed: {e}");
            Json(serde_json::json!({ "error": format!("close failed: {e}") }))
        }
    }
}

/// Mock token generator. Skips `offset` tokens and returns the next `count`.
fn generate_tokens(
    prompt: &str,
    offset: usize,
    count: usize,
) -> futures::stream::BoxStream<'static, String> {
    use futures::stream;

    let all_words: Vec<String> = vec![
        format!("Regarding \"{prompt}\": "),
        "the".into(), " answer".into(), " depends".into(), " on".into(), " context.".into(),
        " Let".into(), " me".into(), " explain".into(), " with".into(),
        " a".into(), " few".into(), " key".into(), " points:".into(),
        "\n\n".into(),
        "First,".into(), " consider".into(), " the".into(), " underlying".into(),
        " assumptions.".into(),
        " Then,".into(), " evaluate".into(), " the".into(), " available".into(),
        " evidence.".into(),
        "\n\n".into(),
        "Second,".into(), " think".into(), " about".into(), " the".into(),
        " broader".into(), " implications.".into(),
        " Every".into(), " decision".into(), " has".into(), " ripple".into(),
        " effects".into(), " that".into(), " extend".into(), " far".into(),
        " beyond".into(), " the".into(), " immediate".into(), " situation.".into(),
        "\n\n".into(),
        "Third,".into(), " examine".into(), " the".into(), " problem".into(),
        " from".into(), " multiple".into(), " angles.".into(),
        " What".into(), " looks".into(), " like".into(), " a".into(),
        " dead".into(), " end".into(), " from".into(), " one".into(),
        " perspective".into(), " might".into(), " be".into(), " an".into(),
        " opportunity".into(), " from".into(), " another.".into(),
        "\n\n".into(),
        "Finally,".into(), " draw".into(), " your".into(), " own".into(),
        " conclusions".into(), " based".into(), " on".into(), " what".into(),
        " you".into(), " find.".into(),
        "\n\n".into(),
        "The".into(), " key".into(), " insight".into(), " is".into(), " that".into(),
        " every".into(), " question".into(), " contains".into(), " the".into(),
        " seed".into(), " of".into(), " its".into(), " own".into(), " answer.".into(),
        " The".into(), " act".into(), " of".into(), " asking".into(),
        " is".into(), " itself".into(), " a".into(), " step".into(),
        " toward".into(), " understanding.".into(),
        "\n\n".into(),
        "In".into(), " summary:".into(), " stay".into(), " curious,".into(),
        " keep".into(), " exploring,".into(), " and".into(), " never".into(),
        " stop".into(), " learning.".into(),
        " The".into(), " universe".into(), " rewards".into(), " those".into(),
        " who".into(), " ask".into(), " good".into(), " questions.".into(),
        "\n\n".into(),
        "Remember".into(), " that".into(), " knowledge".into(), " builds".into(),
        " on".into(), " itself.".into(),
        " Each".into(), " thing".into(), " you".into(), " learn".into(),
        " opens".into(), " doors".into(), " to".into(), " new".into(),
        " possibilities".into(), " you".into(), " never".into(),
        " knew".into(), " existed.".into(),
        "\n\n".into(),
        "Hope".into(), " that".into(), " helps!".into(),
        " Feel".into(), " free".into(), " to".into(), " ask".into(),
        " follow-up".into(), " questions.".into(),
        " [end]".into(),
    ];

    let tokens: Vec<String> = all_words.into_iter().skip(offset).take(count).collect();

    Box::pin(stream::iter(tokens).then(|token| async move {
        let delay = 30 + rand::random::<u64>() % 70;
        tokio::time::sleep(Duration::from_millis(delay)).await;
        token
    }))
}

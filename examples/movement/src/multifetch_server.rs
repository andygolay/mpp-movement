//! # Multi-Fetch Session Server (Movement)
//!
//! A payment-channel-gated web scraping API. Each request costs 0.001 MOVE
//! and is paid via off-chain ed25519 vouchers over a payment channel.
//!
//! - First request: client opens a channel on-chain and sends initial voucher
//! - Subsequent requests: client sends incrementing vouchers (no gas!)
//! - Channel close: server settles and closes the channel on-chain
//!
//! The server has its own funded wallet for on-chain settlement.
//!
//! ## Running
//!
//! ```bash
//! cargo run --bin movement-multifetch-server
//! ```

use axum::{
    extract::{Query, State},
    http::{header, HeaderMap, StatusCode},
    response::IntoResponse,
    routing::get,
    Json, Router,
};
use ed25519_dalek::SigningKey;
use mpp::client::MovementProvider;
use mpp::protocol::methods::movement::{
    self, rest_client::EntryFunctionPayload, InMemoryChannelStore, SessionMethod,
    SessionMethodConfig,
};
use mpp::server::{movement as movement_builder, MovementConfig, MovementSessionOptions, Mpp};
use mpp::{format_www_authenticate, parse_authorization};
use std::sync::Arc;
use std::time::Duration;

const REST_URL: &str = "https://testnet.movementnetwork.xyz/v1";
const FAUCET_URL: &str = "https://faucet.testnet.movementnetwork.xyz";
const MODULE_ADDRESS: &str =
    "0x74f1060add0c641a0c10bb5bab2bf5fd05f94d7c25055f2419fa82d7bbf2b1e8";

/// 0.001 MOVE per request (8 decimals).
const AMOUNT_PER_REQUEST: &str = "100000";
/// Suggested deposit: 0.01 MOVE.
const SUGGESTED_DEPOSIT: &str = "1000000";

type PaymentHandler = Mpp<mpp::server::MovementChargeMethod, SessionMethod>;

struct AppState {
    payment: PaymentHandler,
    /// Server's provider for on-chain settlement.
    server_provider: MovementProvider,
    /// Channel store for reading latest voucher state at close time.
    channel_store: Arc<InMemoryChannelStore>,
}

#[derive(serde::Deserialize)]
struct ScrapeQuery {
    url: Option<String>,
}

#[derive(serde::Deserialize)]
struct CloseQuery {
    channel_id: String,
}

#[tokio::main]
async fn main() {
    // Generate a server wallet (the payee) and fund it.
    let server_key = SigningKey::from_bytes(&rand::random());
    let server_provider =
        MovementProvider::new(server_key.clone(), REST_URL).expect("failed to create provider");
    let server_address = server_provider.address().to_string();

    let secret_key = std::env::var("MPP_SECRET_KEY")
        .unwrap_or_else(|_| "multifetch-example-secret".to_string());

    println!("Movement Multi-Fetch Scraping API");
    println!("  Server (payee): {server_address}");
    println!("  Cost:           0.001 MOVE per request");
    println!("  Deposit:        0.01 MOVE suggested");
    println!();

    // Fund server from faucet (needed for gas to settle/close channels).
    println!("Funding server from faucet...");
    let http = reqwest::Client::new();
    let resp = http
        .post(&format!(
            "{}/mint?amount=100000000&address={}",
            FAUCET_URL, server_address
        ))
        .send()
        .await
        .expect("faucet failed");
    assert!(resp.status().is_success());
    tokio::time::sleep(Duration::from_secs(2)).await;

    // Migrate to FA.
    let migrate = EntryFunctionPayload::new("0x1::coin::migrate_to_fungible_store", vec![])
        .with_type_arguments(vec!["0x1::aptos_coin::AptosCoin".to_string()]);
    server_provider
        .rest_client()
        .build_sign_submit(&server_key, &server_address, migrate)
        .await
        .expect("migration failed");
    println!("Server funded.\n");

    // Create the base MPP handler.
    let base = Mpp::create_movement(
        movement_builder(MovementConfig {
            recipient: &server_address,
        })
        .rest_url(REST_URL)
        .secret_key(&secret_key)
        .decimals(8),
    )
    .expect("failed to create MPP handler");

    // Create the session method with an in-memory channel store.
    let store = Arc::new(InMemoryChannelStore::new());
    let store_dyn: Arc<dyn movement::ChannelStore> = Arc::clone(&store) as _;
    let session_method = SessionMethod::new(
        store_dyn,
        SessionMethodConfig {
            module_address: MODULE_ADDRESS.to_string(),
            registry_address: MODULE_ADDRESS.to_string(),
            rest_url: REST_URL.to_string(),
            token_metadata: movement::MOVE_TOKEN_METADATA.to_string(),
            min_voucher_delta: 0,
        },
    );

    let payment = base.with_session_method(session_method);

    let state = Arc::new(AppState {
        payment,
        server_provider,
        channel_store: store,
    });

    let app = Router::new()
        .route("/api/health", get(health))
        .route("/api/scrape", get(scrape).post(scrape))
        .route("/api/close", get(close_channel))
        .with_state(state);

    let listener = tokio::net::TcpListener::bind("0.0.0.0:3002")
        .await
        .expect("failed to bind");

    println!("Listening on http://localhost:3002");
    println!("  GET /api/health          — free");
    println!("  GET /api/scrape?url=...  — 0.001 MOVE per request (HTTP 402 session)");
    println!("  GET /api/close?channel_id=... — settle and close channel on-chain");
    axum::serve(listener, app).await.expect("server error");
}

async fn health() -> impl IntoResponse {
    Json(serde_json::json!({ "status": "ok" }))
}

async fn scrape(
    State(state): State<Arc<AppState>>,
    headers: HeaderMap,
    Query(query): Query<ScrapeQuery>,
) -> impl IntoResponse {
    let page_url = query.url.as_deref().unwrap_or("https://example.com");

    // Check for a payment credential in the Authorization header.
    if let Some(credential) = parse_credential(&headers) {
        match state.payment.verify_session(&credential).await {
            Ok(result) => {
                // Management responses (open/topUp/close) are returned directly.
                if let Some(mgmt) = result.management_response {
                    let receipt_header = result.receipt.to_header().unwrap_or_default();
                    return (
                        StatusCode::OK,
                        [("payment-receipt", receipt_header)],
                        Json(mgmt),
                    )
                        .into_response();
                }

                // Voucher verified — return scraped content with receipt.
                let content = scrape_page(page_url);
                let receipt_header = result.receipt.to_header().unwrap_or_default();
                return (
                    StatusCode::OK,
                    [("payment-receipt", receipt_header)],
                    Json(serde_json::json!({
                        "content": content,
                        "url": page_url,
                    })),
                )
                    .into_response();
            }
            Err(e) => {
                eprintln!("Session verification failed: {e}");
                // Fall through to return a fresh 402 challenge
            }
        }
    }

    // No valid credential — return 402 with a session challenge.
    let challenge = state
        .payment
        .movement_session_challenge(
            AMOUNT_PER_REQUEST,
            MovementSessionOptions {
                unit_type: Some("request"),
                suggested_deposit: Some(SUGGESTED_DEPOSIT),
                module_address: Some(MODULE_ADDRESS),
                ..Default::default()
            },
        )
        .expect("failed to create session challenge");

    let www_auth = format_www_authenticate(&challenge).expect("failed to format challenge");
    (
        StatusCode::PAYMENT_REQUIRED,
        [(header::WWW_AUTHENTICATE, www_auth)],
        Json(serde_json::json!({
            "error": "Payment Required",
            "amount_per_request": AMOUNT_PER_REQUEST,
            "suggested_deposit": SUGGESTED_DEPOSIT,
            "currency": "0xa",
        })),
    )
        .into_response()
}

/// Settle and close a channel on-chain, claiming earned funds.
///
/// Uses the highest accepted voucher from the in-memory store to do a
/// final settlement + close in one on-chain transaction.
async fn close_channel(
    State(state): State<Arc<AppState>>,
    Query(query): Query<CloseQuery>,
) -> Json<serde_json::Value> {
    let channel_id_hex = &query.channel_id;
    let channel_id_bytes = match hex::decode(
        channel_id_hex
            .strip_prefix("0x")
            .unwrap_or(channel_id_hex),
    ) {
        Ok(b) => b,
        Err(_) => return Json(serde_json::json!({ "error": "invalid channel_id hex" })),
    };

    // Read the channel from our in-memory store to get the highest voucher.
    let channel = match state.channel_store.get_channel_sync(channel_id_hex) {
        Some(ch) => ch,
        None => {
            return Json(serde_json::json!({ "error": "channel not found in store" }));
        }
    };

    if channel.finalized {
        return Json(serde_json::json!({ "error": "channel already finalized" }));
    }

    let cumulative_amount = channel.highest_voucher_amount;
    let signature = channel.highest_voucher_signature.unwrap_or_default();
    let pubkey = channel.authorized_signer_pubkey;

    if cumulative_amount == 0 {
        return Json(serde_json::json!({ "error": "no vouchers to settle" }));
    }

    println!(
        "  [close] channel {}... highest_voucher={}, deposit={}",
        &channel_id_hex[..18.min(channel_id_hex.len())],
        cumulative_amount,
        channel.deposit,
    );

    // Close the channel on-chain with the highest voucher.
    // The contract settles the delta (cumulative - already_settled) to the payee
    // and refunds the remainder to the payer.
    match state
        .server_provider
        .close_channel(
            MODULE_ADDRESS,
            MODULE_ADDRESS,
            &channel_id_bytes,
            cumulative_amount,
            &signature,
            &pubkey,
        )
        .await
    {
        Ok(tx) => {
            println!("  [close] tx: {tx}");
            let refund = channel.deposit.saturating_sub(cumulative_amount);
            Json(serde_json::json!({
                "status": "closed",
                "close_tx": tx,
                "settled": cumulative_amount.to_string(),
                "deposit": channel.deposit.to_string(),
                "refund": refund.to_string(),
            }))
        }
        Err(e) => {
            eprintln!("  [close] failed: {e}");
            Json(serde_json::json!({ "error": format!("close failed: {e}") }))
        }
    }
}

fn parse_credential(headers: &HeaderMap) -> Option<mpp::PaymentCredential> {
    headers
        .get(header::AUTHORIZATION)
        .and_then(|h| h.to_str().ok())
        .and_then(|s| parse_authorization(s).ok())
}

fn scrape_page(url: &str) -> String {
    format!("<h1>{url}</h1><p>Scraped content from {url}. This is mock data returned by the Movement MPP multi-fetch example.</p>")
}

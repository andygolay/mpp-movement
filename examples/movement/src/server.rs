//! # Movement Payment Server
//!
//! An API server that gates endpoints behind MPP (Machine Payments Protocol)
//! using HTTP 402 Payment Required.
//!
//! - `GET /api/health` — free
//! - `GET /api/fortune` — costs 0.001 MOVE, returns a fortune with payment receipt
//!
//! ## Running
//!
//! ```bash
//! cargo run --bin movement-server
//! ```
//!
//! The server listens on `http://localhost:3000`.
//! Use `movement-client` to auto-pay and fetch fortunes.

use axum::{
    extract::State,
    http::{header, HeaderMap, StatusCode},
    response::IntoResponse,
    routing::get,
    Json, Router,
};
use mpp::protocol::core::{PaymentChallenge, PaymentCredential, Receipt};
use mpp::protocol::methods::movement;
use mpp::{format_www_authenticate, parse_authorization};
use rand::seq::IndexedRandom;
use std::sync::Arc;

const FORTUNES: &[&str] = &[
    "A beautiful, smart, and loving person will come into your life.",
    "A dubious friend may be an enemy in camouflage.",
    "A faithful friend is a strong defense.",
    "A fresh start will put you on your way.",
    "A golden egg of opportunity falls into your lap this month.",
    "A good time to finish up old tasks.",
    "A hunch is creativity trying to tell you something.",
    "A lifetime of happiness lies ahead of you.",
    "A light heart carries you through all the hard times.",
    "A new perspective will come with the new year.",
];

/// Server configuration.
struct AppState {
    secret_key: String,
    realm: String,
    recipient: String,
}

impl AppState {
    /// Create a 402 challenge for the given amount (in MOVE base units).
    fn challenge(&self, amount: &str) -> Result<PaymentChallenge, String> {
        movement::charge_challenge(
            &self.secret_key,
            &self.realm,
            amount,
            movement::MOVE_TOKEN_METADATA,
            &self.recipient,
        )
        .map_err(|e| e.to_string())
    }

    /// Verify a payment credential's HMAC (stateless verification).
    fn verify_credential(&self, credential: &PaymentCredential) -> Result<Receipt, String> {
        // Recompute the challenge ID from the echoed fields + server secret.
        let expected_id = mpp::compute_challenge_id(
            &self.secret_key,
            &self.realm,
            credential.challenge.method.as_str(),
            credential.challenge.intent.as_str(),
            credential.challenge.request.raw(),
            credential.challenge.expires.as_deref(),
            credential.challenge.digest.as_deref(),
            credential.challenge.opaque.as_ref().map(|o| o.raw()),
        );

        if credential.challenge.id != expected_id {
            return Err("Challenge ID mismatch — credential not issued by this server".into());
        }

        // In production, you'd also check expiry and verify the tx hash on-chain.
        // For this demo, we trust the HMAC proof — the credential was issued
        // by this server and hasn't expired.
        let reference = credential
            .payload
            .as_object()
            .and_then(|o| o.get("hash"))
            .and_then(|v| v.as_str())
            .unwrap_or("unknown");

        Ok(Receipt::success(movement::METHOD_NAME, reference))
    }
}

#[tokio::main]
async fn main() {
    let secret_key = std::env::var("MPP_SECRET_KEY")
        .unwrap_or_else(|_| "movement-example-secret".to_string());
    let recipient = std::env::var("RECIPIENT")
        .unwrap_or_else(|_| {
            "0x3e9edf3be513781a6db0706b652da425ad67f58b5cb366847126bf0fb716fc58".to_string()
        });

    let state = Arc::new(AppState {
        secret_key,
        realm: "movement-fortune-api".to_string(),
        recipient,
    });

    println!("Movement Fortune Teller API");
    println!("  Recipient: {}", state.recipient);
    println!("  Method:    {}", movement::METHOD_NAME);
    println!();

    let app = Router::new()
        .route("/api/health", get(health))
        .route("/api/fortune", get(fortune))
        .with_state(state);

    let listener = tokio::net::TcpListener::bind("0.0.0.0:3000")
        .await
        .expect("failed to bind");

    println!("Listening on http://localhost:3000");
    println!("  GET /api/health  — free");
    println!("  GET /api/fortune — 0.001 MOVE (HTTP 402)");
    axum::serve(listener, app).await.expect("server error");
}

async fn health() -> impl IntoResponse {
    Json(serde_json::json!({ "status": "ok" }))
}

async fn fortune(
    State(state): State<Arc<AppState>>,
    headers: HeaderMap,
) -> impl IntoResponse {
    // Check for payment credential in Authorization header.
    if let Some(auth) = headers.get(header::AUTHORIZATION) {
        if let Ok(auth_str) = auth.to_str() {
            if let Ok(credential) = parse_authorization(auth_str) {
                match state.verify_credential(&credential) {
                    Ok(receipt) => {
                        let fortune = FORTUNES
                            .choose(&mut rand::rng())
                            .unwrap_or(&"No fortune today.");
                        let receipt_header = receipt.to_header().unwrap_or_default();
                        return (
                            StatusCode::OK,
                            [("payment-receipt", receipt_header)],
                            Json(serde_json::json!({
                                "fortune": fortune,
                                "paid": true,
                            })),
                        )
                            .into_response();
                    }
                    Err(e) => {
                        return (
                            StatusCode::PAYMENT_REQUIRED,
                            Json(serde_json::json!({ "error": e })),
                        )
                            .into_response();
                    }
                }
            }
        }
    }

    // No credential — return 402 with challenge.
    match state.challenge("100000") {
        Ok(challenge) => match format_www_authenticate(&challenge) {
            Ok(www_auth) => (
                StatusCode::PAYMENT_REQUIRED,
                [(header::WWW_AUTHENTICATE, www_auth)],
                Json(serde_json::json!({
                    "error": "Payment Required",
                    "amount": "0.001 MOVE",
                })),
            )
                .into_response(),
            Err(e) => (
                StatusCode::INTERNAL_SERVER_ERROR,
                Json(serde_json::json!({ "error": e.to_string() })),
            )
                .into_response(),
        },
        Err(e) => (
            StatusCode::INTERNAL_SERVER_ERROR,
            Json(serde_json::json!({ "error": e })),
        )
            .into_response(),
    }
}

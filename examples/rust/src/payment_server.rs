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
//! cargo run --bin movement-payment-server
//! ```
//!
//! The server listens on `http://localhost:3000`.
//! Use `movement-payment-client` to auto-pay and fetch fortunes.
//!
//! ## Using the Simple API
//!
//! This example uses `Mpp::create_movement()` with the builder pattern:
//! - Automatic challenge generation with dollar-to-base-unit conversion
//! - Server-side charge verification (checks tx hash, recipient, amount, token)
//! - Axum `ChargeChallenger` integration for per-route pricing

use axum::{routing::get, Json, Router};
use mpp::server::axum::{ChargeChallenger, ChargeConfig, MppCharge, WithReceipt};
use mpp::server::{movement, MovementConfig, Mpp};
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

/// Charge $0.001 MOVE (100_000 base units at 8 decimals).
struct FortuneFee;
impl ChargeConfig for FortuneFee {
    fn amount() -> &'static str {
        "0.001"
    }
    fn description() -> Option<&'static str> {
        Some("Fortune telling")
    }
}

#[tokio::main]
async fn main() {
    let recipient = std::env::var("RECIPIENT").unwrap_or_else(|_| {
        "0x74f1060add0c641a0c10bb5bab2bf5fd05f94d7c25055f2419fa82d7bbf2b1e8".to_string()
    });

    // Create the MPP handler with the Movement builder.
    // This sets up:
    // - Challenge generation (movement method, MOVE token, 8 decimals)
    // - Server-side charge verification via REST API
    // - Axum ChargeChallenger integration
    let mpp = Mpp::create_movement(
        movement(MovementConfig {
            recipient: &recipient,
        })
        .rest_url("https://testnet.movementnetwork.xyz/v1")
        .secret_key(
            &std::env::var("MPP_SECRET_KEY")
                .unwrap_or_else(|_| "movement-example-secret".to_string()),
        )
        .decimals(8), // MOVE has 8 decimals
    )
    .expect("failed to create Movement MPP handler");

    println!("Movement Fortune Teller API");
    println!("  Recipient: {}", recipient);
    println!("  Method:    movement");
    println!("  Currency:  MOVE (0xa)");
    println!();

    let app = Router::new()
        .route("/api/health", get(health))
        .route("/api/fortune", get(fortune))
        .with_state(Arc::new(mpp) as Arc<dyn ChargeChallenger>);

    let listener = tokio::net::TcpListener::bind("0.0.0.0:3000")
        .await
        .expect("failed to bind");

    println!("Listening on http://localhost:3000");
    println!("  GET /api/health  — free");
    println!("  GET /api/fortune — 0.001 MOVE (HTTP 402)");
    axum::serve(listener, app).await.expect("server error");
}

async fn health() -> Json<serde_json::Value> {
    Json(serde_json::json!({ "status": "ok" }))
}

async fn fortune(charge: MppCharge<FortuneFee>) -> WithReceipt<Json<serde_json::Value>> {
    use rand::seq::IndexedRandom;

    let fortune = FORTUNES
        .choose(&mut rand::rng())
        .unwrap_or(&"No fortune today.");

    WithReceipt {
        receipt: charge.receipt,
        body: Json(serde_json::json!({
            "fortune": fortune,
            "paid": true,
        })),
    }
}

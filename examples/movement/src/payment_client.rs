//! # Movement Payment Client
//!
//! A CLI client that fetches a fortune from the payment-gated API
//! with automatic HTTP 402 handling and real on-chain payments.
//!
//! ## Running
//!
//! ```bash
//! # Start the server first:
//! cargo run --bin movement-payment-server
//!
//! # Then in another terminal:
//! cargo run --bin movement-payment-client
//! ```
//!
//! ## How it works
//!
//! 1. Client sends `GET /api/fortune`
//! 2. Server returns `402 Payment Required` with `WWW-Authenticate: Payment ...`
//! 3. Client parses the challenge, builds the right transfer for the token type
//! 4. Client submits a real transfer on Movement testnet (native MOVE or any FA token)
//! 5. Client retries the request with the tx hash as a payment credential
//! 6. Server verifies the credential and returns the fortune + receipt

use ed25519_dalek::SigningKey;
use mpp::client::{Fetch, MovementProvider};
use mpp::parse_receipt;
use reqwest::Client;

const SERVER_URL: &str = "http://localhost:3000";
const REST_URL: &str = "https://testnet.movementnetwork.xyz/v1";
const FAUCET_URL: &str = "https://faucet.testnet.movementnetwork.xyz";

#[tokio::main]
async fn main() {
    let url = std::env::args()
        .nth(1)
        .unwrap_or_else(|| format!("{}/api/fortune", SERVER_URL));

    println!("=== Movement MPP Client ===\n");

    // --- Setup: generate a wallet and fund it ---
    let signing_key = SigningKey::from_bytes(&rand::random());
    let provider =
        MovementProvider::new(signing_key.clone(), REST_URL).expect("failed to create provider");
    let address = provider.address().to_string();

    println!("Wallet: {address}");
    println!("Funding from faucet...");

    let http = reqwest::Client::new();
    let faucet_resp = http
        .post(&format!(
            "{}/mint?amount=100000000&address={}",
            FAUCET_URL, address
        ))
        .send()
        .await
        .expect("faucet request failed");
    assert!(faucet_resp.status().is_success(), "faucet failed");
    tokio::time::sleep(std::time::Duration::from_secs(2)).await;

    // Migrate CoinStore to FA (required for native MOVE transfers on Movement).
    use mpp::protocol::methods::movement::rest_client::EntryFunctionPayload;
    let migrate_payload =
        EntryFunctionPayload::new("0x1::coin::migrate_to_fungible_store", vec![])
            .with_type_arguments(vec!["0x1::aptos_coin::AptosCoin".to_string()]);
    provider
        .rest_client()
        .build_sign_submit(&signing_key, &address, migrate_payload)
        .await
        .expect("migration failed");

    println!("Funded with 1 MOVE on testnet.\n");

    let client = Client::new();

    // --- Automatic 402 flow using MovementProvider ---
    // The SDK handles: GET → 402 → parse challenge → build correct transfer
    // (native MOVE or FA token) → submit tx → retry with credential → 200
    println!("Fetching {url} with automatic payment...\n");
    let resp = client
        .get(&url)
        .send_with_payment(&provider)
        .await
        .expect("request with payment failed");

    println!(
        "Response: {} {}",
        resp.status().as_u16(),
        resp.status().canonical_reason().unwrap_or("")
    );

    // --- Read the receipt ---
    if let Some(receipt_hdr) = resp.headers().get("payment-receipt") {
        if let Ok(receipt_str) = receipt_hdr.to_str() {
            if let Ok(receipt) = parse_receipt(receipt_str) {
                println!("\nPayment receipt:");
                println!(
                    "  status:    {}",
                    if receipt.is_success() {
                        "success"
                    } else {
                        "failed"
                    }
                );
                println!("  method:    {}", receipt.method);
                println!("  reference: {}", receipt.reference);
            }
        }
    }

    let body = resp.text().await.unwrap_or_default();
    if let Ok(json) = serde_json::from_str::<serde_json::Value>(&body) {
        if let Some(fortune) = json.get("fortune").and_then(|v| v.as_str()) {
            println!("\nFortune: {fortune}");
        } else {
            println!("\nResponse: {json}");
        }
    } else {
        println!("\nResponse: {body}");
    }
}

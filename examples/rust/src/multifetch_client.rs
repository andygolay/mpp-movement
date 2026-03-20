//! # Multi-Fetch Session Client (Movement)
//!
//! Demonstrates making multiple paid requests over a single payment channel
//! using `MovementSessionProvider` for automatic channel management.
//!
//! - First request: opens channel on-chain automatically
//! - Subsequent requests: sends off-chain vouchers (no gas!)
//! - Close: asks server to settle on-chain
//!
//! ## Running
//!
//! ```bash
//! # Start the server first:
//! cargo run --bin movement-multifetch-server
//!
//! # Then in another terminal:
//! cargo run --bin movement-multifetch-client
//! ```

use ed25519_dalek::SigningKey;
use mpp::client::{Fetch, MovementSessionProvider};
use mpp::protocol::methods::movement::rest_client::EntryFunctionPayload;
use reqwest::Client;

const SERVER_URL: &str = "http://localhost:3002";
const REST_URL: &str = "https://testnet.movementnetwork.xyz/v1";
const FAUCET_URL: &str = "https://faucet.testnet.movementnetwork.xyz";

const PAGE_COUNT: usize = 9;

#[tokio::main]
async fn main() {
    let base_url = std::env::var("BASE_URL").unwrap_or_else(|_| SERVER_URL.to_string());

    println!("=== Movement Multi-Fetch Client ===\n");

    // --- Setup: create session provider and fund wallet ---
    let signing_key = SigningKey::from_bytes(&rand::random());
    let session = MovementSessionProvider::new(signing_key.clone(), REST_URL)
        .expect("failed to create session provider")
        .with_max_deposit(1_000_000); // Cap at 0.01 MOVE

    let address = session.address().to_string();
    let http = Client::new();

    println!("Wallet: {address}");
    println!("Funding from faucet...");

    let faucet_resp = http
        .post(&format!(
            "{}/mint?amount=100000000&address={}",
            FAUCET_URL, address
        ))
        .send()
        .await
        .expect("faucet failed");
    assert!(faucet_resp.status().is_success());
    tokio::time::sleep(std::time::Duration::from_secs(2)).await;

    // Migrate to FA.
    let migrate = EntryFunctionPayload::new("0x1::coin::migrate_to_fungible_store", vec![])
        .with_type_arguments(vec!["0x1::aptos_coin::AptosCoin".to_string()]);
    session
        .rest_client()
        .build_sign_submit(session.signing_key(), &address, migrate)
        .await
        .expect("migration failed");
    println!("Funded with 1 MOVE.\n");

    // --- Scrape pages using send_with_payment ---
    // The session provider handles everything automatically:
    // - First request: 402 → open channel on-chain → retry with credential
    // - Subsequent requests: 402 → sign voucher (no gas) → retry
    println!("Scraping {PAGE_COUNT} pages...\n");

    for i in 1..=PAGE_COUNT {
        let page_url = format!("https://example.com/page/{i}");
        let request_url = format!(
            "{base_url}/api/scrape?url={}",
            urlencoding::encode(&page_url)
        );

        let resp = http
            .get(&request_url)
            .send_with_payment(&session)
            .await
            .expect("request failed");

        if resp.status().is_success() {
            let _body: serde_json::Value = resp.json().await.unwrap_or_default();
            println!(
                "  {page_url} -> OK (cumulative: {} base units)",
                session.cumulative()
            );
        } else {
            let body = resp.text().await.unwrap_or_default();
            eprintln!("  {page_url} -> FAILED: {body}");
            return;
        }
    }

    // --- Ask server to settle and close on-chain ---
    println!("\nRequesting server to close channel...");
    tokio::time::sleep(std::time::Duration::from_secs(2)).await;

    // Get the channel ID from the session provider's internal state.
    let channels = session.channels();
    if let Some(entry) = channels.values().next() {
        let channel_id_hex = format!("0x{}", hex::encode(&entry.channel_id));
        let close_url = format!("{base_url}/api/close?channel_id={channel_id_hex}");
        let close_resp = http.get(&close_url).send().await.expect("close failed");
        let close_body: serde_json::Value = close_resp.json().await.unwrap_or_default();

        if let Some(tx) = close_body.get("close_tx").and_then(|v| v.as_str()) {
            println!("  Close tx: {tx}");
        } else if let Some(err) = close_body.get("error").and_then(|v| v.as_str()) {
            eprintln!("  Close failed: {err}");
        }
    }

    // --- Summary ---
    let cumulative = session.cumulative();
    println!("\n=== Summary ===");
    println!("  Pages scraped:      {PAGE_COUNT}");
    println!("  Total paid:         {cumulative} base units ({:.4} MOVE)", cumulative as f64 / 1e8);
    println!("  Client on-chain tx: 1 (open, automatic)");
    println!("  Off-chain vouchers: {} (automatic)", PAGE_COUNT - 1);
}

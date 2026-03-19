//! # Movement Payment Client
//!
//! A CLI client that fetches a fortune from the payment-gated API
//! with automatic HTTP 402 handling and real on-chain payments.
//!
//! ## Running
//!
//! ```bash
//! # Start the server first:
//! cargo run --bin movement-server
//!
//! # Then in another terminal:
//! cargo run --bin movement-client
//! ```
//!
//! ## How it works
//!
//! 1. Client sends `GET /api/fortune`
//! 2. Server returns `402 Payment Required` with `WWW-Authenticate: Payment ...`
//! 3. Client parses the challenge, sees it's a `movement` charge for 0.001 MOVE
//! 4. Client builds, signs, and submits a real transfer on Movement testnet
//! 5. Client retries the request with the tx hash as a payment credential
//! 6. Server verifies the credential and returns the fortune + receipt

use ed25519_dalek::SigningKey;
use mpp::protocol::core::{PaymentCredential, PaymentPayload};
use mpp::protocol::methods::movement::rest_client::{EntryFunctionPayload, MovementRestClient};
use mpp::{format_authorization, parse_receipt, parse_www_authenticate};
use reqwest::Client;
use sha3::{Digest, Sha3_256};

const SERVER_URL: &str = "http://localhost:3000";
const REST_URL: &str = "https://testnet.movementnetwork.xyz/v1";
const FAUCET_URL: &str = "https://faucet.testnet.movementnetwork.xyz";

fn derive_address(pubkey: &[u8; 32]) -> String {
    let mut hasher = Sha3_256::new();
    hasher.update(pubkey);
    hasher.update([0x00]);
    format!("0x{}", hex::encode(hasher.finalize()))
}

#[tokio::main]
async fn main() {
    let url = std::env::args()
        .nth(1)
        .unwrap_or_else(|| format!("{}/api/fortune", SERVER_URL));

    println!("=== Movement MPP Client ===\n");

    // --- Setup: generate a wallet and fund it ---
    let signing_key = SigningKey::from_bytes(&rand::random());
    let pubkey = signing_key.verifying_key().to_bytes();
    let address = derive_address(&pubkey);
    let rest_client = MovementRestClient::new(REST_URL);

    println!("Wallet: {address}");
    println!("Funding from faucet...");

    let http = reqwest::Client::new();
    let faucet_resp = http
        .post(&format!("{}/mint?amount=100000000&address={}", FAUCET_URL, address))
        .send()
        .await
        .expect("faucet request failed");
    assert!(faucet_resp.status().is_success(), "faucet failed");
    tokio::time::sleep(std::time::Duration::from_secs(2)).await;

    // Migrate CoinStore to FA.
    let migrate_payload = EntryFunctionPayload::new(
        "0x1::coin::migrate_to_fungible_store",
        vec![],
    ).with_type_arguments(vec!["0x1::aptos_coin::AptosCoin".to_string()]);
    rest_client
        .build_sign_submit(&signing_key, &address, migrate_payload)
        .await
        .expect("migration failed");

    println!("Funded with 1 MOVE on testnet.\n");

    let client = Client::new();

    // --- Step 1: Initial request → expect 402 ---
    println!("Fetching {url} ...\n");
    let resp = client.get(&url).send().await.expect("request failed");
    println!(
        "1. Response: {} {}",
        resp.status().as_u16(),
        resp.status().canonical_reason().unwrap_or("")
    );

    if resp.status() != reqwest::StatusCode::PAYMENT_REQUIRED {
        let body = resp.text().await.unwrap_or_default();
        println!("   Not a 402 — server response: {body}");
        return;
    }

    // --- Step 2: Parse the 402 challenge ---
    let www_auth = resp
        .headers()
        .get("www-authenticate")
        .and_then(|v| v.to_str().ok())
        .expect("missing WWW-Authenticate header");

    let challenge = parse_www_authenticate(www_auth).expect("failed to parse challenge");

    println!("2. Payment challenge received:");
    println!("   method:  {}", challenge.method);
    println!("   intent:  {}", challenge.intent);

    let request: mpp::ChargeRequest = challenge
        .request
        .decode()
        .expect("failed to decode charge request");

    let amount: u64 = request.amount.parse().unwrap_or(0);
    let recipient = request.recipient.as_deref().unwrap_or("?");
    println!("   amount:  {} ({:.4} MOVE)", amount, amount as f64 / 1e8);
    println!("   token:   {}", request.currency);
    println!("   to:      {}\n", recipient);

    // --- Step 3: Pay — real on-chain transfer on Movement testnet ---
    println!("3. Submitting payment on Movement testnet...");

    let transfer_payload = EntryFunctionPayload::new(
        "0x1::aptos_account::transfer",
        vec![
            serde_json::json!(recipient),
            serde_json::json!(amount.to_string()),
        ],
    );

    let tx_hash = rest_client
        .build_sign_submit(&signing_key, &address, transfer_payload)
        .await
        .expect("payment transaction failed");

    println!("   Tx: {tx_hash}");
    println!("   https://explorer.movementnetwork.xyz/txn/{}?network=testnet", tx_hash);

    // Build the payment credential with the real tx hash.
    let echo = challenge.to_echo();
    let credential = PaymentCredential::new(echo, PaymentPayload::hash(&tx_hash));
    let auth_header = format_authorization(&credential).expect("failed to format credential");

    // --- Step 4: Retry with payment credential ---
    println!("\n4. Retrying with payment credential...");
    let resp = client
        .get(&url)
        .header("Authorization", &auth_header)
        .send()
        .await
        .expect("retry request failed");

    println!(
        "   Response: {} {}",
        resp.status().as_u16(),
        resp.status().canonical_reason().unwrap_or("")
    );

    // --- Step 5: Read the receipt ---
    if let Some(receipt_hdr) = resp.headers().get("payment-receipt") {
        if let Ok(receipt_str) = receipt_hdr.to_str() {
            if let Ok(receipt) = parse_receipt(receipt_str) {
                println!("\n5. Payment receipt:");
                println!("   status:    {}", if receipt.is_success() { "success" } else { "failed" });
                println!("   method:    {}", receipt.method);
                println!("   reference: {}", receipt.reference);
            }
        }
    }

    let body = resp.text().await.unwrap_or_default();
    if let Ok(json) = serde_json::from_str::<serde_json::Value>(&body) {
        if let Some(fortune) = json.get("fortune").and_then(|v| v.as_str()) {
            println!("\n   Fortune: {fortune}");
        } else {
            println!("\n   Response: {json}");
        }
    } else {
        println!("\n   Response: {body}");
    }
}

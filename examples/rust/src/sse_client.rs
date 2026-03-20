//! # Pay-Per-Token LLM Streaming Client
//!
//! Opens a payment channel on Movement testnet, then streams tokens
//! from the SSE server, paying per token with ed25519 vouchers.
//!
//! ## Running
//!
//! ```bash
//! # Start the server first:
//! cargo run --bin movement-sse-server
//!
//! # Then in another terminal:
//! cargo run --bin movement-sse-client
//!
//! # Or with a custom prompt:
//! cargo run --bin movement-sse-client -- "What is the meaning of life?"
//! ```

use ed25519_dalek::SigningKey;
use futures::StreamExt;
use mpp::client::MovementProvider;
use mpp::protocol::core::{PaymentCredential, PaymentPayload};
use mpp::protocol::methods::movement::rest_client::EntryFunctionPayload;
use mpp::{format_authorization, parse_www_authenticate};
use reqwest::Client;
use std::time::Duration;

const SERVER_URL: &str = "http://localhost:3001";
const REST_URL: &str = "https://testnet.movementnetwork.xyz/v1";
const FAUCET_URL: &str = "https://faucet.testnet.movementnetwork.xyz";
const MODULE_ADDRESS: &str =
    "0x74f1060add0c641a0c10bb5bab2bf5fd05f94d7c25055f2419fa82d7bbf2b1e8";

/// Price per token (must match server).
const PRICE_PER_TOKEN: u64 = 1_000;
/// How many tokens to buy per voucher.
const TOKENS_PER_VOUCHER: u64 = 10;

#[tokio::main]
async fn main() {
    let prompt = std::env::args()
        .nth(1)
        .unwrap_or_else(|| "Tell me something interesting".to_string());

    println!("=== Movement Pay-Per-Token LLM Client ===\n");

    // --- Setup: create provider and fund wallet ---
    let signing_key = SigningKey::from_bytes(&rand::random());
    let provider = MovementProvider::new(signing_key.clone(), REST_URL)
        .expect("failed to create provider");
    let address = provider.address().to_string();
    let pubkey = provider.public_key_bytes();
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
    tokio::time::sleep(Duration::from_secs(2)).await;

    // Migrate to FA.
    let migrate = EntryFunctionPayload::new("0x1::coin::migrate_to_fungible_store", vec![])
        .with_type_arguments(vec!["0x1::aptos_coin::AptosCoin".to_string()]);
    provider
        .rest_client()
        .build_sign_submit(provider.signing_key(), &address, migrate)
        .await
        .expect("migration failed");
    println!("Funded with 1 MOVE.\n");

    // --- Step 1: Hit the API, get 402 ---
    let chat_url = format!(
        "{}/api/chat?prompt={}",
        SERVER_URL,
        urlencoding::encode(&prompt)
    );
    println!("1. Requesting: GET /api/chat?prompt=\"{prompt}\"");

    let resp = http.get(&chat_url).send().await.expect("request failed");
    println!("   Response: {}", resp.status());

    if resp.status() != reqwest::StatusCode::PAYMENT_REQUIRED {
        let body = resp.text().await.unwrap_or_default();
        println!("   Unexpected response: {body}");
        return;
    }

    // Parse the 402 challenge.
    let www_auth = resp
        .headers()
        .get("www-authenticate")
        .and_then(|v| v.to_str().ok())
        .expect("missing WWW-Authenticate");
    let challenge = parse_www_authenticate(www_auth).expect("failed to parse challenge");

    let req: mpp::ChargeRequest = challenge.request.decode().unwrap();
    let deposit: u64 = req.amount.parse().unwrap();
    let recipient = req.recipient.as_deref().unwrap();
    let token_metadata = &req.currency;
    println!("   Deposit required: {} base units", deposit);

    // --- Step 2: Open payment channel using the SDK ---
    println!("\n2. Opening payment channel on Movement testnet...");
    let salt: [u8; 32] = rand::random();

    let (open_tx, channel_id) = provider
        .open_channel(
            MODULE_ADDRESS,
            MODULE_ADDRESS,
            recipient,
            token_metadata,
            deposit,
            &salt,
        )
        .await
        .expect("open channel failed");

    let channel_id_hex = format!("0x{}", hex::encode(&channel_id));
    println!("   Open tx: {open_tx}");
    println!("   Channel: {channel_id_hex}");

    // Send the open credential to the server.
    let echo = challenge.to_echo();
    let credential = PaymentCredential::new(echo, PaymentPayload::hash(&open_tx));
    let auth_header = format_authorization(&credential).unwrap();

    let resp = http
        .get(&chat_url)
        .header("Authorization", &auth_header)
        .send()
        .await
        .expect("credential request failed");
    let _ = resp.text().await;

    // --- Step 3: Stream tokens, paying with vouchers ---
    println!(
        "\n3. Streaming tokens (paying {} base units per {} tokens)...\n",
        PRICE_PER_TOKEN * TOKENS_PER_VOUCHER,
        TOKENS_PER_VOUCHER,
    );

    let mut cumulative: u64 = 0;
    let mut total_tokens: u64 = 0;
    let mut voucher_count: u32 = 0;

    loop {
        let delta = PRICE_PER_TOKEN * TOKENS_PER_VOUCHER;
        if cumulative + delta > deposit {
            println!("\n   (deposit exhausted)");
            break;
        }
        cumulative += delta;
        voucher_count += 1;

        // Sign voucher using the SDK.
        let sig = provider.sign_voucher(&channel_id, cumulative);

        let stream_url = format!(
            "{}/api/chat?prompt={}&channel_id={}&cumulative_amount={}&signature=0x{}&pubkey=0x{}",
            SERVER_URL,
            urlencoding::encode(&prompt),
            channel_id_hex,
            cumulative,
            hex::encode(sig),
            hex::encode(pubkey),
        );

        let resp = http
            .get(&stream_url)
            .send()
            .await
            .expect("stream request failed");

        if !resp.status().is_success() {
            let body = resp.text().await.unwrap_or_default();
            println!("\n   Server error: {body}");
            break;
        }

        // Read SSE events.
        let mut stream = resp.bytes_stream();
        let mut buffer = String::new();

        while let Some(chunk) = stream.next().await {
            let chunk = match chunk {
                Ok(c) => c,
                Err(_) => break,
            };
            buffer.push_str(&String::from_utf8_lossy(&chunk));

            while let Some(pos) = buffer.find("\n\n") {
                let event_str = buffer[..pos].to_string();
                buffer = buffer[pos + 2..].to_string();

                for line in event_str.lines() {
                    if let Some(data) = line.strip_prefix("data: ") {
                        if let Ok(json) = serde_json::from_str::<serde_json::Value>(data) {
                            if let Some(token) = json.get("token").and_then(|v| v.as_str()) {
                                print!("{token}");
                                total_tokens += 1;
                            }
                        }
                    }
                }
            }
        }
    }

    // --- Step 4: Ask server to close the channel ---
    println!("\n\n4. Requesting server to close channel...");
    tokio::time::sleep(Duration::from_secs(3)).await;

    let close_url = format!("{}/api/close?channel_id={}", SERVER_URL, channel_id_hex);
    let close_resp = http
        .get(&close_url)
        .send()
        .await
        .expect("close request failed");
    let close_body: serde_json::Value = close_resp.json().await.unwrap_or_default();

    let settle_txns = close_body
        .get("settle_txns")
        .and_then(|v| v.as_array())
        .cloned()
        .unwrap_or_default();

    if !settle_txns.is_empty() {
        println!("\n   Server settlements:");
        for (i, tx) in settle_txns.iter().enumerate() {
            if let Some(hash) = tx.as_str() {
                println!("     settle {}: {hash}", i + 1);
            }
        }
    }

    if let Some(tx) = close_body.get("close_tx").and_then(|v| v.as_str()) {
        println!("     close:    {tx}");
    } else if let Some(err) = close_body.get("error").and_then(|v| v.as_str()) {
        println!("   Close error: {err}");
    }

    let server_on_chain =
        settle_txns.len() as u32 + if close_body.get("close_tx").is_some() { 1 } else { 0 };

    // --- Summary ---
    println!("\n=== Summary ===");
    println!("   Tokens streamed:    {total_tokens}");
    println!("   Vouchers sent:      {voucher_count}");
    println!("   Total paid:         {cumulative} base units");
    println!("   Deposit:            {deposit} base units");
    println!("   Refund to payer:    {} base units", deposit - cumulative);
    println!("   Client on-chain:    1 tx (open)");
    println!(
        "   Server on-chain:    {server_on_chain} txns ({} settles + {} close)",
        settle_txns.len(),
        if close_body.get("close_tx").is_some() {
            1
        } else {
            0
        },
    );
    println!("   Off-chain vouchers: {voucher_count}");
    println!("   Channel: {channel_id_hex}");
}

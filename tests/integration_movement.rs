//! Integration tests for Movement payment flows.
//!
//! These tests verify the Movement session lifecycle using real ed25519
//! cryptography and the InMemoryChannelStore. They do not require a running
//! Movement node — all on-chain state is simulated via the store.
//!
//! # Running
//!
//! ```bash
//! cargo test --features movement,server,client --test integration_movement
//! ```

#![cfg(all(feature = "movement", feature = "server"))]

use std::sync::Arc;

use ed25519_dalek::SigningKey;
use mpp::protocol::methods::movement::{
    self, compute_channel_id, sign_voucher, verify_voucher, InMemoryChannelStore, SessionMethod,
    SessionMethodConfig,
};
use mpp::protocol::methods::movement::session_method::{ChannelState, deduct_from_channel};

// ==================== Test Helpers ====================

fn test_signing_key() -> SigningKey {
    SigningKey::from_bytes(&[0x42; 32])
}

fn test_channel_id_bytes() -> Vec<u8> {
    let key = test_signing_key();
    let pubkey = key.verifying_key().to_bytes();
    let payer = [0x0A_u8; 32];
    let payee = [0x0B_u8; 32];
    let token = [0x00; 31].iter().chain(&[0x0a]).copied().collect::<Vec<u8>>();
    let mut token_arr = [0u8; 32];
    token_arr.copy_from_slice(&token);
    let salt = b"test_salt";

    compute_channel_id(&payer, &payee, &token_arr, salt, &pubkey).to_vec()
}

fn test_channel_id_hex() -> String {
    format!("0x{}", hex::encode(test_channel_id_bytes()))
}

fn populated_store(channel_id: &str, deposit: u64) -> InMemoryChannelStore {
    let key = test_signing_key();
    let pubkey = key.verifying_key().to_bytes();
    let store = InMemoryChannelStore::new();
    store.insert(
        channel_id,
        ChannelState {
            channel_id: channel_id.to_string(),
            module_address: movement::DEFAULT_MODULE_ADDRESS.to_string(),
            registry_address: movement::DEFAULT_MODULE_ADDRESS.to_string(),
            payer: format!("0x{}", "0a".repeat(32)),
            payee: format!("0x{}", "0b".repeat(32)),
            token: "0xa".to_string(),
            authorized_signer_pubkey: pubkey.to_vec(),
            deposit,
            settled_on_chain: 0,
            highest_voucher_amount: 0,
            highest_voucher_signature: None,
            spent: 0,
            units: 0,
            finalized: false,
            created_at: "2026-01-01T00:00:00Z".to_string(),
        },
    );
    store
}

fn make_session_method(store: Arc<InMemoryChannelStore>) -> SessionMethod {
    SessionMethod::new(
        store,
        SessionMethodConfig {
            module_address: movement::DEFAULT_MODULE_ADDRESS.to_string(),
            registry_address: movement::DEFAULT_MODULE_ADDRESS.to_string(),
            rest_url: "http://localhost:0".to_string(), // not used in voucher tests
            token_metadata: "0xa".to_string(),
            min_voucher_delta: 0,
        },
    )
}

fn make_voucher_credential(
    channel_id: &str,
    cumulative_amount: u64,
    signing_key: &SigningKey,
) -> mpp::PaymentCredential {
    let channel_id_bytes = hex::decode(channel_id.strip_prefix("0x").unwrap_or(channel_id)).unwrap();
    let sig = sign_voucher(signing_key, &channel_id_bytes, cumulative_amount);

    let payload = serde_json::json!({
        "action": "voucher",
        "channelId": channel_id,
        "cumulativeAmount": cumulative_amount.to_string(),
        "signature": format!("0x{}", hex::encode(sig)),
    });

    let request = mpp::protocol::intents::SessionRequest {
        amount: "1000".to_string(),
        currency: "0xa".to_string(),
        recipient: Some(format!("0x{}", "0b".repeat(32))),
        method_details: Some(serde_json::json!({
            "moduleAddress": movement::DEFAULT_MODULE_ADDRESS,
        })),
        ..Default::default()
    };
    let encoded = mpp::protocol::core::Base64UrlJson::from_typed(&request).unwrap();

    let id = mpp::compute_challenge_id(
        "test-secret",
        "test-realm",
        "movement",
        "session",
        encoded.raw(),
        None,
        None,
        None,
    );

    let echo = mpp::ChallengeEcho {
        id,
        realm: "test-realm".into(),
        method: "movement".into(),
        intent: "session".into(),
        request: encoded,
        expires: None,
        digest: None,
        opaque: None,
    };

    mpp::PaymentCredential::new(echo, payload)
}

fn make_close_credential(
    channel_id: &str,
    cumulative_amount: u64,
    signing_key: &SigningKey,
) -> mpp::PaymentCredential {
    let channel_id_bytes = hex::decode(channel_id.strip_prefix("0x").unwrap_or(channel_id)).unwrap();
    let sig = sign_voucher(signing_key, &channel_id_bytes, cumulative_amount);

    let payload = serde_json::json!({
        "action": "close",
        "channelId": channel_id,
        "cumulativeAmount": cumulative_amount.to_string(),
        "signature": format!("0x{}", hex::encode(sig)),
    });

    let request = mpp::protocol::intents::SessionRequest {
        amount: "1000".to_string(),
        currency: "0xa".to_string(),
        recipient: Some(format!("0x{}", "0b".repeat(32))),
        method_details: Some(serde_json::json!({
            "moduleAddress": movement::DEFAULT_MODULE_ADDRESS,
        })),
        ..Default::default()
    };
    let encoded = mpp::protocol::core::Base64UrlJson::from_typed(&request).unwrap();

    let id = mpp::compute_challenge_id(
        "test-secret",
        "test-realm",
        "movement",
        "session",
        encoded.raw(),
        None,
        None,
        None,
    );

    let echo = mpp::ChallengeEcho {
        id,
        realm: "test-realm".into(),
        method: "movement".into(),
        intent: "session".into(),
        request: encoded,
        expires: None,
        digest: None,
        opaque: None,
    };

    mpp::PaymentCredential::new(echo, payload)
}

// ==================== Charge Challenge Tests ====================

#[test]
fn test_movement_charge_challenge_roundtrip() {
    let challenge = movement::charge_challenge(
        "test-secret",
        "api.example.com",
        "1000000",
        "0xa",
        movement::DEFAULT_MODULE_ADDRESS,
    )
    .unwrap();

    assert_eq!(challenge.method.as_str(), "movement");
    assert_eq!(challenge.intent.as_str(), "charge");
    assert!(challenge.expires.is_some());
    assert_eq!(challenge.id.len(), 43); // base64url HMAC-SHA256

    // Decode the request back
    let request: mpp::ChargeRequest = challenge.request.decode().unwrap();
    assert_eq!(request.amount, "1000000");
    assert_eq!(request.currency, "0xa");
}

#[test]
fn test_movement_charge_challenge_deterministic() {
    let c1 = movement::charge_challenge_with_options(
        "test-secret",
        "api.example.com",
        &mpp::ChargeRequest {
            amount: "1000000".into(),
            currency: "0xa".into(),
            recipient: Some(movement::DEFAULT_MODULE_ADDRESS.into()),
            ..Default::default()
        },
        Some("2026-01-01T00:00:00Z"),
        None,
    )
    .unwrap();

    let c2 = movement::charge_challenge_with_options(
        "test-secret",
        "api.example.com",
        &mpp::ChargeRequest {
            amount: "1000000".into(),
            currency: "0xa".into(),
            recipient: Some(movement::DEFAULT_MODULE_ADDRESS.into()),
            ..Default::default()
        },
        Some("2026-01-01T00:00:00Z"),
        None,
    )
    .unwrap();

    assert_eq!(c1.id, c2.id);
}

#[test]
fn test_movement_charge_challenge_different_amounts() {
    let c1 = movement::charge_challenge("s", "r", "100", "0xa", "0xabc").unwrap();
    let c2 = movement::charge_challenge("s", "r", "200", "0xa", "0xabc").unwrap();
    assert_ne!(c1.id, c2.id);
}

// ==================== Voucher Crypto Tests ====================

#[test]
fn test_sign_verify_voucher_roundtrip() {
    let key = test_signing_key();
    let pubkey = key.verifying_key().to_bytes();
    let channel_id = test_channel_id_bytes();

    let sig = sign_voucher(&key, &channel_id, 5000);
    assert!(verify_voucher(&channel_id, 5000, &sig, &pubkey, &pubkey));
}

#[test]
fn test_verify_voucher_wrong_amount_fails() {
    let key = test_signing_key();
    let pubkey = key.verifying_key().to_bytes();
    let channel_id = test_channel_id_bytes();

    let sig = sign_voucher(&key, &channel_id, 5000);
    assert!(!verify_voucher(&channel_id, 9999, &sig, &pubkey, &pubkey));
}

#[test]
fn test_verify_voucher_wrong_key_fails() {
    let key = test_signing_key();
    let wrong_key = SigningKey::from_bytes(&[0x99; 32]);
    let wrong_pubkey = wrong_key.verifying_key().to_bytes();
    let channel_id = test_channel_id_bytes();

    let sig = sign_voucher(&key, &channel_id, 5000);
    assert!(!verify_voucher(&channel_id, 5000, &sig, &wrong_pubkey, &[]));
}

#[test]
fn test_channel_id_deterministic() {
    let id1 = test_channel_id_bytes();
    let id2 = test_channel_id_bytes();
    assert_eq!(id1, id2);
    assert_ne!(id1, vec![0u8; 32]);
}

// ==================== Session Lifecycle Tests ====================

#[tokio::test]
async fn test_voucher_accept_and_monotonicity() {
    let key = test_signing_key();
    let channel_id = test_channel_id_hex();
    let store = Arc::new(populated_store(&channel_id, 100_000));
    let method = make_session_method(store.clone());

    // First voucher: amount = 1000
    let cred1 = make_voucher_credential(&channel_id, 1000, &key);
    let request: mpp::protocol::intents::SessionRequest = cred1.challenge.request.decode().unwrap();

    use mpp::protocol::traits::SessionMethod as _;
    let receipt1 = method.verify_session(&cred1, &request).await.unwrap();
    assert!(receipt1.is_success());

    // Check store updated
    let state = store.get_channel_sync(&channel_id).unwrap();
    assert_eq!(state.highest_voucher_amount, 1000);
    assert!(state.highest_voucher_signature.is_some());

    // Second voucher: amount = 3000 (delta = 2000)
    let cred2 = make_voucher_credential(&channel_id, 3000, &key);
    let receipt2 = method.verify_session(&cred2, &request).await.unwrap();
    assert!(receipt2.is_success());

    let state2 = store.get_channel_sync(&channel_id).unwrap();
    assert_eq!(state2.highest_voucher_amount, 3000);
}

#[tokio::test]
async fn test_voucher_amount_exceeds_deposit_rejected() {
    let key = test_signing_key();
    let channel_id = test_channel_id_hex();
    let store = Arc::new(populated_store(&channel_id, 1000)); // low deposit
    let method = make_session_method(store);

    let cred = make_voucher_credential(&channel_id, 5000, &key); // exceeds deposit
    let request: mpp::protocol::intents::SessionRequest = cred.challenge.request.decode().unwrap();

    use mpp::protocol::traits::SessionMethod as _;
    let result = method.verify_session(&cred, &request).await;
    assert!(result.is_err());
    let err = result.unwrap_err();
    assert_eq!(
        err.code,
        Some(mpp::protocol::traits::ErrorCode::AmountExceedsDeposit)
    );
}

#[tokio::test]
async fn test_voucher_replay_idempotent() {
    let key = test_signing_key();
    let channel_id = test_channel_id_hex();
    let store = Arc::new(populated_store(&channel_id, 100_000));
    let method = make_session_method(store.clone());

    // Accept first voucher
    let cred = make_voucher_credential(&channel_id, 1000, &key);
    let request: mpp::protocol::intents::SessionRequest = cred.challenge.request.decode().unwrap();

    use mpp::protocol::traits::SessionMethod as _;
    let receipt1 = method.verify_session(&cred, &request).await.unwrap();
    assert!(receipt1.is_success());

    // Replay exact same credential — should succeed idempotently
    let receipt2 = method.verify_session(&cred, &request).await.unwrap();
    assert!(receipt2.is_success());

    // Amount should still be 1000 (not double-counted)
    let state = store.get_channel_sync(&channel_id).unwrap();
    assert_eq!(state.highest_voucher_amount, 1000);
}

#[tokio::test]
async fn test_voucher_delta_too_small() {
    let key = test_signing_key();
    let channel_id = test_channel_id_hex();
    let store = Arc::new(populated_store(&channel_id, 100_000));

    // Create method with min_voucher_delta = 500
    let method = SessionMethod::new(
        store.clone(),
        SessionMethodConfig {
            min_voucher_delta: 500,
            ..SessionMethodConfig::default()
        },
    );

    // First voucher at 1000
    let cred1 = make_voucher_credential(&channel_id, 1000, &key);
    let request: mpp::protocol::intents::SessionRequest = cred1.challenge.request.decode().unwrap();

    use mpp::protocol::traits::SessionMethod as _;
    method.verify_session(&cred1, &request).await.unwrap();

    // Second voucher at 1100 (delta = 100, below min 500)
    let cred2 = make_voucher_credential(&channel_id, 1100, &key);
    let result = method.verify_session(&cred2, &request).await;
    assert!(result.is_err());
    let err = result.unwrap_err();
    assert_eq!(
        err.code,
        Some(mpp::protocol::traits::ErrorCode::DeltaTooSmall)
    );
}

#[tokio::test]
async fn test_voucher_on_finalized_channel_rejected() {
    let key = test_signing_key();
    let channel_id = test_channel_id_hex();
    let store = Arc::new(InMemoryChannelStore::new());

    // Insert a finalized channel
    let pubkey = key.verifying_key().to_bytes();
    store.insert(
        &channel_id,
        ChannelState {
            channel_id: channel_id.clone(),
            module_address: movement::DEFAULT_MODULE_ADDRESS.to_string(),
            registry_address: movement::DEFAULT_MODULE_ADDRESS.to_string(),
            payer: format!("0x{}", "0a".repeat(32)),
            payee: format!("0x{}", "0b".repeat(32)),
            token: "0xa".to_string(),
            authorized_signer_pubkey: pubkey.to_vec(),
            deposit: 100_000,
            settled_on_chain: 0,
            highest_voucher_amount: 50_000,
            highest_voucher_signature: None,
            spent: 0,
            units: 0,
            finalized: true,
            created_at: "2026-01-01T00:00:00Z".to_string(),
        },
    );

    let method = make_session_method(store);
    let cred = make_voucher_credential(&channel_id, 60_000, &key);
    let request: mpp::protocol::intents::SessionRequest = cred.challenge.request.decode().unwrap();

    use mpp::protocol::traits::SessionMethod as _;
    let result = method.verify_session(&cred, &request).await;
    assert!(result.is_err());
    let err = result.unwrap_err();
    assert_eq!(
        err.code,
        Some(mpp::protocol::traits::ErrorCode::ChannelClosed)
    );
}

#[tokio::test]
async fn test_voucher_wrong_signature_rejected() {
    let key = test_signing_key();
    let wrong_key = SigningKey::from_bytes(&[0x99; 32]);
    let channel_id = test_channel_id_hex();
    let store = Arc::new(populated_store(&channel_id, 100_000));
    let method = make_session_method(store);

    // Sign with the wrong key
    let cred = make_voucher_credential(&channel_id, 1000, &wrong_key);
    let request: mpp::protocol::intents::SessionRequest = cred.challenge.request.decode().unwrap();

    use mpp::protocol::traits::SessionMethod as _;
    let result = method.verify_session(&cred, &request).await;
    assert!(result.is_err());
    let err = result.unwrap_err();
    assert_eq!(
        err.code,
        Some(mpp::protocol::traits::ErrorCode::InvalidSignature)
    );
}

#[tokio::test]
async fn test_close_channel() {
    let key = test_signing_key();
    let channel_id = test_channel_id_hex();
    let store = Arc::new(populated_store(&channel_id, 100_000));
    let method = make_session_method(store.clone());

    // Accept some vouchers first
    let cred1 = make_voucher_credential(&channel_id, 5000, &key);
    let request: mpp::protocol::intents::SessionRequest = cred1.challenge.request.decode().unwrap();

    use mpp::protocol::traits::SessionMethod as _;
    method.verify_session(&cred1, &request).await.unwrap();

    // Close with final amount >= highest voucher
    let close_cred = make_close_credential(&channel_id, 5000, &key);
    let receipt = method.verify_session(&close_cred, &request).await.unwrap();
    assert!(receipt.is_success());

    // Channel should be finalized
    let state = store.get_channel_sync(&channel_id).unwrap();
    assert!(state.finalized);
    assert_eq!(state.highest_voucher_amount, 5000);
}

#[tokio::test]
async fn test_close_below_highest_voucher_rejected() {
    let key = test_signing_key();
    let channel_id = test_channel_id_hex();
    let store = Arc::new(populated_store(&channel_id, 100_000));
    let method = make_session_method(store.clone());

    // Accept a voucher at 5000
    let cred1 = make_voucher_credential(&channel_id, 5000, &key);
    let request: mpp::protocol::intents::SessionRequest = cred1.challenge.request.decode().unwrap();

    use mpp::protocol::traits::SessionMethod as _;
    method.verify_session(&cred1, &request).await.unwrap();

    // Try to close at 3000 (below 5000)
    let close_cred = make_close_credential(&channel_id, 3000, &key);
    let result = method.verify_session(&close_cred, &request).await;
    assert!(result.is_err());
}

#[tokio::test]
async fn test_voucher_after_close_rejected() {
    let key = test_signing_key();
    let channel_id = test_channel_id_hex();
    let store = Arc::new(populated_store(&channel_id, 100_000));
    let method = make_session_method(store.clone());

    // Close the channel
    let close_cred = make_close_credential(&channel_id, 0, &key);
    let request: mpp::protocol::intents::SessionRequest = close_cred.challenge.request.decode().unwrap();

    use mpp::protocol::traits::SessionMethod as _;
    method.verify_session(&close_cred, &request).await.unwrap();

    // Try to send a voucher after close
    let cred = make_voucher_credential(&channel_id, 1000, &key);
    let result = method.verify_session(&cred, &request).await;
    assert!(result.is_err());
    let err = result.unwrap_err();
    assert_eq!(
        err.code,
        Some(mpp::protocol::traits::ErrorCode::ChannelClosed)
    );
}

// ==================== Deduction Tests ====================

#[tokio::test]
async fn test_deduct_from_channel_success() {
    let channel_id = test_channel_id_hex();
    let store = populated_store(&channel_id, 100_000);

    // Set voucher amount so there's something to deduct from
    store.insert(
        &channel_id,
        ChannelState {
            highest_voucher_amount: 10_000,
            ..store.get_channel_sync(&channel_id).unwrap()
        },
    );

    let result = deduct_from_channel(&store, &channel_id, 3_000).await;
    assert!(result.is_ok());
    let state = result.unwrap();
    assert_eq!(state.spent, 3_000);
    assert_eq!(state.units, 1);

    // Second deduction
    let result2 = deduct_from_channel(&store, &channel_id, 5_000).await;
    assert!(result2.is_ok());
    let state2 = result2.unwrap();
    assert_eq!(state2.spent, 8_000);
    assert_eq!(state2.units, 2);
}

#[tokio::test]
async fn test_deduct_insufficient_balance() {
    let channel_id = test_channel_id_hex();
    let store = populated_store(&channel_id, 100_000);
    store.insert(
        &channel_id,
        ChannelState {
            highest_voucher_amount: 1_000,
            ..store.get_channel_sync(&channel_id).unwrap()
        },
    );

    let result = deduct_from_channel(&store, &channel_id, 5_000).await;
    assert!(result.is_err());
}

// ==================== Config Tests ====================

#[test]
fn test_session_method_config_default() {
    let config = SessionMethodConfig::default();
    assert_eq!(config.module_address, movement::DEFAULT_MODULE_ADDRESS);
    assert_eq!(config.min_voucher_delta, 0);
}

#[test]
fn test_session_method_config_for_network() {
    let testnet = SessionMethodConfig::for_network(movement::MovementNetwork::Testnet);
    assert!(testnet.rest_url.contains("testnet"));
    assert_eq!(
        testnet.module_address,
        movement::DEFAULT_MODULE_ADDRESS_TESTNET
    );

    let mainnet = SessionMethodConfig::for_network(movement::MovementNetwork::Mainnet);
    assert!(!mainnet.rest_url.contains("testnet"));
}

#[test]
fn test_network_module_addresses() {
    let testnet_addr = movement::MovementNetwork::Testnet.default_module_address();
    let mainnet_addr = movement::MovementNetwork::Mainnet.default_module_address();

    assert!(testnet_addr.starts_with("0x"));
    assert!(mainnet_addr.starts_with("0x"));
}

// ==================== Full Session Flow ====================

#[tokio::test]
async fn test_full_session_lifecycle() {
    let key = test_signing_key();
    let channel_id = test_channel_id_hex();
    let store = Arc::new(populated_store(&channel_id, 100_000));
    let method = make_session_method(store.clone());

    use mpp::protocol::traits::SessionMethod as _;

    // Step 1: Accept voucher for 1000
    let cred1 = make_voucher_credential(&channel_id, 1000, &key);
    let request: mpp::protocol::intents::SessionRequest = cred1.challenge.request.decode().unwrap();
    let receipt = method.verify_session(&cred1, &request).await.unwrap();
    assert!(receipt.is_success());

    // Step 2: Deduct 500 for a service unit
    let state = deduct_from_channel(store.as_ref(), &channel_id, 500)
        .await
        .unwrap();
    assert_eq!(state.spent, 500);
    assert_eq!(state.units, 1);

    // Step 3: Accept voucher for 3000 (delta = 2000)
    let cred2 = make_voucher_credential(&channel_id, 3000, &key);
    method.verify_session(&cred2, &request).await.unwrap();

    // Step 4: Deduct more
    let state = deduct_from_channel(store.as_ref(), &channel_id, 1000)
        .await
        .unwrap();
    assert_eq!(state.spent, 1500);
    assert_eq!(state.units, 2);

    // Step 5: Close the channel
    let close_cred = make_close_credential(&channel_id, 3000, &key);
    let receipt = method
        .verify_session(&close_cred, &request)
        .await
        .unwrap();
    assert!(receipt.is_success());

    // Verify final state
    let final_state = store.get_channel_sync(&channel_id).unwrap();
    assert!(final_state.finalized);
    assert_eq!(final_state.highest_voucher_amount, 3000);
    assert_eq!(final_state.spent, 1500);
    assert_eq!(final_state.units, 2);
}

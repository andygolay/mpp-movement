//! Movement session provider for automatic payment channel management.
//!
//! `MovementSessionProvider` auto-manages the full session lifecycle:
//! - First request: opens a payment channel on-chain
//! - Subsequent requests: sends off-chain ed25519 vouchers (no gas!)
//! - Close: settles on-chain with the highest voucher
//!
//! # Example
//!
//! ```ignore
//! use mpp::client::{Fetch, MovementSessionProvider};
//!
//! let session = MovementSessionProvider::new(signing_key, "https://testnet.movementnetwork.xyz/v1")?;
//!
//! // First request: opens channel on-chain automatically
//! let resp = client.get(url).send_with_payment(&session).await?;
//!
//! // Subsequent requests: off-chain vouchers, no gas
//! let resp = client.get(url).send_with_payment(&session).await?;
//!
//! // When done: close and settle on-chain
//! session.close(&client, url).await?;
//! ```

use std::collections::HashMap;
use std::sync::{Arc, Mutex};

use ed25519_dalek::SigningKey;

use crate::client::PaymentProvider;
use crate::error::MppError;
use crate::protocol::core::{PaymentChallenge, PaymentCredential};
use crate::protocol::intents::SessionRequest;
use crate::protocol::methods::movement::rest_client::{EntryFunctionPayload, MovementRestClient};
use crate::protocol::methods::movement::session::MovementSessionExt;
use crate::protocol::methods::movement::{self, voucher};

/// State for a single payment channel.
#[derive(Debug, Clone)]
pub struct ChannelEntry {
    /// Channel ID (32 bytes).
    pub channel_id: Vec<u8>,
    /// Random salt used at channel creation.
    pub salt: Vec<u8>,
    /// Running total of all voucher amounts (monotonically increasing).
    pub cumulative_amount: u64,
    /// Module address where the contract is deployed.
    pub module_address: String,
    /// Whether the channel has been opened on-chain.
    pub opened: bool,
    /// Authorized signer public key.
    pub authorized_pubkey: Vec<u8>,
    /// Highest voucher signature (for close).
    pub highest_signature: Vec<u8>,
}

/// Movement session provider for automatic payment channel management.
///
/// Implements `PaymentProvider` so it works with `send_with_payment()`.
/// Tracks open channels internally and auto-manages the lifecycle.
#[derive(Clone)]
pub struct MovementSessionProvider {
    signing_key: SigningKey,
    sender_address: String,
    rest_client: MovementRestClient,
    /// Max deposit the client will lock (caps server suggestions).
    max_deposit: Option<u64>,
    /// Default deposit if server doesn't suggest one.
    default_deposit: Option<u64>,
    /// Open channels keyed by "payee:currency:module".
    channels: Arc<Mutex<HashMap<String, ChannelEntry>>>,
    /// Last challenge received (for close).
    last_challenge: Arc<Mutex<Option<PaymentChallenge>>>,
}

impl MovementSessionProvider {
    /// Create a new session provider.
    pub fn new(signing_key: SigningKey, rest_url: &str) -> Result<Self, MppError> {
        let pubkey = signing_key.verifying_key();
        let sender_address = super::derive_address(&pubkey.to_bytes());

        Ok(Self {
            signing_key,
            sender_address,
            rest_client: MovementRestClient::new(rest_url),
            max_deposit: None,
            default_deposit: None,
            channels: Arc::new(Mutex::new(HashMap::new())),
            last_challenge: Arc::new(Mutex::new(None)),
        })
    }

    /// Set the maximum deposit (caps server suggestions).
    pub fn with_max_deposit(mut self, amount: u64) -> Self {
        self.max_deposit = Some(amount);
        self
    }

    /// Set the default deposit (used if server doesn't suggest one).
    pub fn with_default_deposit(mut self, amount: u64) -> Self {
        self.default_deposit = Some(amount);
        self
    }

    /// Get the sender address.
    pub fn address(&self) -> &str {
        &self.sender_address
    }

    /// Get the current cumulative amount of the first open channel.
    pub fn cumulative(&self) -> u64 {
        let channels = self.channels.lock().unwrap();
        channels
            .values()
            .find(|ch| ch.opened)
            .map(|ch| ch.cumulative_amount)
            .unwrap_or(0)
    }

    /// Get a snapshot of all channels.
    pub fn channels(&self) -> HashMap<String, ChannelEntry> {
        self.channels.lock().unwrap().clone()
    }

    /// Get the public key bytes.
    pub fn public_key_bytes(&self) -> [u8; 32] {
        self.signing_key.verifying_key().to_bytes()
    }

    /// Get a reference to the REST client.
    pub fn rest_client(&self) -> &MovementRestClient {
        &self.rest_client
    }

    /// Get a reference to the signing key.
    pub fn signing_key(&self) -> &SigningKey {
        &self.signing_key
    }

    /// Close the active channel by requesting the server to settle on-chain.
    ///
    /// Sends a `close` credential to the server's endpoint. The server
    /// should then call `channel::close` on-chain with the highest voucher.
    pub async fn close(
        &self,
        client: &reqwest::Client,
        close_url: &str,
    ) -> Result<Option<crate::protocol::core::Receipt>, MppError> {
        let challenge = {
            let guard = self.last_challenge.lock().unwrap();
            match guard.as_ref() {
                Some(c) => c.clone(),
                None => return Ok(None),
            }
        };

        let entry = {
            let channels = self.channels.lock().unwrap();
            match channels.values().find(|ch| ch.opened) {
                Some(e) => e.clone(),
                None => return Ok(None),
            }
        };

        let channel_id_hex = format!("0x{}", hex::encode(&entry.channel_id));
        let sig = voucher::sign_voucher(
            &self.signing_key,
            &entry.channel_id,
            entry.cumulative_amount,
        );

        let close_payload = serde_json::json!({
            "action": "close",
            "channelId": channel_id_hex,
            "cumulativeAmount": entry.cumulative_amount.to_string(),
            "signature": format!("0x{}", hex::encode(sig)),
        });

        let echo = challenge.to_echo();
        let credential = PaymentCredential::new(echo, close_payload);
        let auth_header = crate::protocol::core::format_authorization(&credential)
            .map_err(|e| MppError::Http(format!("failed to format close credential: {}", e)))?;

        let resp = client
            .post(close_url)
            .header("Authorization", auth_header)
            .send()
            .await
            .map_err(|e| MppError::Http(format!("close request failed: {}", e)))?;

        if resp.status().is_success() {
            if let Some(receipt_hdr) = resp.headers().get("payment-receipt") {
                if let Ok(s) = receipt_hdr.to_str() {
                    if let Ok(receipt) = crate::protocol::core::parse_receipt(s) {
                        return Ok(Some(receipt));
                    }
                }
            }
            Ok(None)
        } else {
            let text = resp.text().await.unwrap_or_default();
            Err(MppError::Http(format!("close failed: {}", text)))
        }
    }

    /// Resolve the deposit amount from server suggestion and client caps.
    fn resolve_deposit(&self, suggested: Option<&str>) -> Result<u64, MppError> {
        let suggested = suggested.and_then(|s| s.parse::<u64>().ok());

        match (suggested, self.max_deposit, self.default_deposit) {
            (Some(s), Some(max), _) => Ok(s.min(max)),
            (Some(s), None, _) => Ok(s),
            (None, Some(max), _) => Ok(max),
            (None, None, Some(def)) => Ok(def),
            (None, None, None) => Err(MppError::InvalidConfig(
                "no deposit amount: server didn't suggest one and no max_deposit/default_deposit set"
                    .into(),
            )),
        }
    }

    /// Build the channel registry key from session params.
    fn channel_key(payee: &str, currency: &str, module: &str) -> String {
        format!(
            "{}:{}:{}",
            payee.to_lowercase(),
            currency.to_lowercase(),
            module.to_lowercase()
        )
    }
}

impl PaymentProvider for MovementSessionProvider {
    fn supports(&self, method: &str, intent: &str) -> bool {
        method == movement::METHOD_NAME && intent == movement::INTENT_SESSION
    }

    async fn pay(&self, challenge: &PaymentChallenge) -> Result<PaymentCredential, MppError> {
        // Cache the challenge for close().
        {
            let mut guard = self.last_challenge.lock().unwrap();
            *guard = Some(challenge.clone());
        }

        let request: SessionRequest = challenge.request.decode()?;
        let amount: u64 = request
            .amount
            .parse()
            .map_err(|e| MppError::InvalidAmount(format!("invalid session amount: {}", e)))?;
        let payee = request
            .recipient
            .as_deref()
            .ok_or_else(|| MppError::InvalidConfig("no recipient in session challenge".into()))?;
        let currency = &request.currency;

        let module_address = request
            .module_address()
            .unwrap_or_else(|_| movement::DEFAULT_MODULE_ADDRESS.to_string());

        let key = Self::channel_key(payee, currency, &module_address);
        let echo = challenge.to_echo();

        // Check if we already have an open channel for this payee/currency/module.
        // Scope the lock tightly to avoid holding it across await points.
        {
            let mut channels = self.channels.lock().unwrap();
            if let Some(entry) = channels.get_mut(&key) {
                if entry.opened {
                    // Existing channel — send an incremental voucher.
                    entry.cumulative_amount += amount;
                    let cumulative = entry.cumulative_amount;
                    let channel_id = entry.channel_id.clone();
                    let sig = voucher::sign_voucher(&self.signing_key, &channel_id, cumulative);
                    entry.highest_signature = sig.to_vec();

                    let payload = serde_json::json!({
                        "action": "voucher",
                        "channelId": format!("0x{}", hex::encode(&channel_id)),
                        "cumulativeAmount": cumulative.to_string(),
                        "signature": format!("0x{}", hex::encode(sig)),
                    });

                    return Ok(PaymentCredential::new(echo, payload));
                }
            }
        } // lock released here

        // No existing channel — open a new one on-chain.
        let deposit = self.resolve_deposit(request.suggested_deposit.as_deref())?;
        let registry_address = request
            .registry_address()
            .unwrap_or_else(|| module_address.clone());
        let token_metadata = request
            .token_metadata()
            .unwrap_or_else(|| currency.to_string());

        let pubkey_bytes = self.signing_key.verifying_key().to_bytes();
        let salt: [u8; 32] = rand::random();

        // Build and submit the open transaction.
        let open_payload = EntryFunctionPayload::new(
            &format!("{}::channel::open", module_address),
            vec![
                serde_json::json!(registry_address),
                serde_json::json!(payee),
                serde_json::json!(token_metadata),
                serde_json::json!(deposit.to_string()),
                serde_json::json!(format!("0x{}", hex::encode(salt))),
                serde_json::json!(format!("0x{}", hex::encode(pubkey_bytes))),
            ],
        );

        let tx_hash = self
            .rest_client
            .build_sign_submit(&self.signing_key, &self.sender_address, open_payload)
            .await?;

        // Compute channel ID.
        let payer_bytes = super::parse_address_bytes(&self.sender_address)?;
        let payee_bytes = super::parse_address_bytes(payee)?;
        let token_bytes = super::parse_address_bytes(&token_metadata)?;
        let channel_id = voucher::compute_channel_id(
            &payer_bytes,
            &payee_bytes,
            &token_bytes,
            &salt,
            &pubkey_bytes,
        );

        // Sign the initial voucher.
        let initial_amount = amount;
        let sig = voucher::sign_voucher(&self.signing_key, &channel_id, initial_amount);

        // Store the channel.
        let entry = ChannelEntry {
            channel_id: channel_id.to_vec(),
            salt: salt.to_vec(),
            cumulative_amount: initial_amount,
            module_address: module_address.clone(),
            opened: true,
            authorized_pubkey: pubkey_bytes.to_vec(),
            highest_signature: sig.to_vec(),
        };

        let mut channels = self.channels.lock().unwrap();
        channels.insert(key, entry);
        drop(channels);

        // Build the open credential.
        let channel_id_hex = format!("0x{}", hex::encode(channel_id));
        let payload = serde_json::json!({
            "action": "open",
            "txHash": tx_hash,
            "channelId": channel_id_hex,
            "authorizedSigner": format!("0x{}", hex::encode(pubkey_bytes)),
            "cumulativeAmount": initial_amount.to_string(),
            "signature": format!("0x{}", hex::encode(sig)),
        });

        Ok(PaymentCredential::new(echo, payload))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_session_provider_supports() {
        let key = SigningKey::from_bytes(&[0x42; 32]);
        let provider =
            MovementSessionProvider::new(key, "https://testnet.movementnetwork.xyz/v1").unwrap();

        assert!(provider.supports("movement", "session"));
        assert!(!provider.supports("movement", "charge"));
        assert!(!provider.supports("other", "session"));
    }

    #[test]
    fn test_resolve_deposit() {
        let key = SigningKey::from_bytes(&[0x42; 32]);
        let provider = MovementSessionProvider::new(key, "https://test")
            .unwrap()
            .with_max_deposit(1_000_000);

        // Server suggests more than max → capped
        assert_eq!(
            provider.resolve_deposit(Some("5000000")).unwrap(),
            1_000_000
        );
        // Server suggests less than max → use suggestion
        assert_eq!(provider.resolve_deposit(Some("500000")).unwrap(), 500_000);
        // No suggestion → use max
        assert_eq!(provider.resolve_deposit(None).unwrap(), 1_000_000);
    }

    #[test]
    fn test_resolve_deposit_default() {
        let key = SigningKey::from_bytes(&[0x42; 32]);
        let provider = MovementSessionProvider::new(key, "https://test")
            .unwrap()
            .with_default_deposit(100_000);

        // No suggestion, no max → use default
        assert_eq!(provider.resolve_deposit(None).unwrap(), 100_000);
        // With suggestion → use suggestion
        assert_eq!(provider.resolve_deposit(Some("500000")).unwrap(), 500_000);
    }

    #[test]
    fn test_resolve_deposit_no_config() {
        let key = SigningKey::from_bytes(&[0x42; 32]);
        let provider = MovementSessionProvider::new(key, "https://test").unwrap();

        // No suggestion, no max, no default → error
        assert!(provider.resolve_deposit(None).is_err());
        // With suggestion → works
        assert_eq!(provider.resolve_deposit(Some("500000")).unwrap(), 500_000);
    }

    #[test]
    fn test_channel_key() {
        let key = MovementSessionProvider::channel_key("0xABCD", "0xA", "0x1234");
        assert_eq!(key, "0xabcd:0xa:0x1234");
    }

    #[test]
    fn test_cumulative_no_channels() {
        let key = SigningKey::from_bytes(&[0x42; 32]);
        let provider = MovementSessionProvider::new(key, "https://test").unwrap();
        assert_eq!(provider.cumulative(), 0);
    }

    // ==================== Pay Flow Tests ====================
    //
    // These test the voucher path by pre-populating the channel registry,
    // simulating that an open already happened. The voucher path is pure
    // computation (no network calls), so it can be tested in isolation.

    fn test_provider() -> MovementSessionProvider {
        let key = SigningKey::from_bytes(&[0x42; 32]);
        MovementSessionProvider::new(key, "https://test").unwrap()
    }

    fn test_challenge(amount: &str) -> PaymentChallenge {
        let request = crate::protocol::intents::SessionRequest {
            amount: amount.to_string(),
            currency: "0xa".to_string(),
            recipient: Some(format!("0x{}", "bb".repeat(32))),
            suggested_deposit: Some("1000000".to_string()),
            method_details: Some(serde_json::json!({
                "moduleAddress": movement::DEFAULT_MODULE_ADDRESS,
            })),
            ..Default::default()
        };
        let encoded = crate::protocol::core::Base64UrlJson::from_typed(&request).unwrap();
        crate::protocol::core::PaymentChallenge::new(
            "test-id",
            "test-realm",
            "movement",
            "session",
            encoded,
        )
    }

    fn insert_test_channel(provider: &MovementSessionProvider) -> String {
        let channel_id = vec![0xAB; 32];
        let pubkey = provider.public_key_bytes();
        let payee = format!("0x{}", "bb".repeat(32));
        let currency = "0xa";
        let module = movement::DEFAULT_MODULE_ADDRESS;
        let key = MovementSessionProvider::channel_key(&payee, currency, module);

        let entry = ChannelEntry {
            channel_id: channel_id.clone(),
            salt: vec![0; 32],
            cumulative_amount: 0,
            module_address: module.to_string(),
            opened: true,
            authorized_pubkey: pubkey.to_vec(),
            highest_signature: vec![],
        };

        provider.channels.lock().unwrap().insert(key.clone(), entry);
        key
    }

    #[tokio::test]
    async fn test_pay_voucher_increments_cumulative() {
        let provider = test_provider();
        let key = insert_test_channel(&provider);

        // First voucher: amount = 1000
        let challenge = test_challenge("1000");
        let credential = provider.pay(&challenge).await.unwrap();

        let payload = credential.payload;
        assert_eq!(payload["action"], "voucher");
        assert_eq!(payload["cumulativeAmount"], "1000");

        let channels = provider.channels.lock().unwrap();
        assert_eq!(channels[&key].cumulative_amount, 1000);
        drop(channels);

        // Second voucher: amount = 2000 → cumulative = 3000
        let challenge = test_challenge("2000");
        let credential = provider.pay(&challenge).await.unwrap();

        assert_eq!(credential.payload["action"], "voucher");
        assert_eq!(credential.payload["cumulativeAmount"], "3000");

        let channels = provider.channels.lock().unwrap();
        assert_eq!(channels[&key].cumulative_amount, 3000);
    }

    #[tokio::test]
    async fn test_pay_voucher_signature_verifies() {
        let provider = test_provider();
        insert_test_channel(&provider);

        let challenge = test_challenge("5000");
        let credential = provider.pay(&challenge).await.unwrap();

        let channel_id = [0xAB_u8; 32];
        let sig_hex = credential.payload["signature"].as_str().unwrap();
        let sig_bytes: [u8; 64] = hex::decode(sig_hex.strip_prefix("0x").unwrap())
            .unwrap()
            .try_into()
            .unwrap();
        let pubkey = provider.public_key_bytes();

        assert!(voucher::verify_voucher(
            &channel_id,
            5000,
            &sig_bytes,
            &pubkey,
            &[],
        ));
    }

    #[tokio::test]
    async fn test_pay_voucher_updates_highest_signature() {
        let provider = test_provider();
        let key = insert_test_channel(&provider);

        let challenge = test_challenge("1000");
        provider.pay(&challenge).await.unwrap();

        let channels = provider.channels.lock().unwrap();
        assert!(!channels[&key].highest_signature.is_empty());
        assert_eq!(channels[&key].highest_signature.len(), 64);
    }

    #[tokio::test]
    async fn test_pay_caches_challenge() {
        let provider = test_provider();
        insert_test_channel(&provider);

        assert!(provider.last_challenge.lock().unwrap().is_none());

        let challenge = test_challenge("1000");
        provider.pay(&challenge).await.unwrap();

        let cached = provider.last_challenge.lock().unwrap();
        assert!(cached.is_some());
        assert_eq!(cached.as_ref().unwrap().id, "test-id");
    }

    #[tokio::test]
    async fn test_cumulative_reflects_vouchers() {
        let provider = test_provider();
        insert_test_channel(&provider);

        assert_eq!(provider.cumulative(), 0);

        let challenge = test_challenge("1000");
        provider.pay(&challenge).await.unwrap();
        assert_eq!(provider.cumulative(), 1000);

        provider.pay(&test_challenge("500")).await.unwrap();
        assert_eq!(provider.cumulative(), 1500);
    }

    #[tokio::test]
    async fn test_channels_snapshot() {
        let provider = test_provider();
        insert_test_channel(&provider);

        let snapshot = provider.channels();
        assert_eq!(snapshot.len(), 1);
        let entry = snapshot.values().next().unwrap();
        assert!(entry.opened);
        assert_eq!(entry.channel_id, vec![0xAB; 32]);
    }
}

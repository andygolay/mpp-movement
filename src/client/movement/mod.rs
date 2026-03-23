//! Movement Network payment provider.
//!
//! Implements the `PaymentProvider` trait for Movement, handling both
//! charge (one-time) and session (streaming) payments.
//!
//! # Example
//!
//! ```ignore
//! use mpp::client::{Fetch, MovementProvider};
//!
//! let provider = MovementProvider::new(signing_key, "https://testnet.movementnetwork.xyz/v1")?;
//! let resp = client.get(url).send_with_payment(&provider).await?;
//! ```

pub mod session;

use ed25519_dalek::SigningKey;

use crate::client::PaymentProvider;
pub use session::MovementSessionProvider;

macro_rules! mpp_info {
    ($($arg:tt)*) => {
        #[cfg(feature = "observability")]
        tracing::info!($($arg)*);
    };
}
use crate::error::MppError;
use crate::protocol::core::{PaymentChallenge, PaymentCredential, PaymentPayload};
use crate::protocol::intents::ChargeRequest;
use crate::protocol::methods::movement::rest_client::{EntryFunctionPayload, MovementRestClient};
use crate::protocol::methods::movement::{self, MovementChargeExt};

/// Movement payment provider for automatic 402 handling.
///
/// Handles charge payments by building and submitting `aptos_account::transfer`
/// transactions on Movement Network.
#[derive(Clone)]
pub struct MovementProvider {
    signing_key: SigningKey,
    sender_address: String,
    rest_client: MovementRestClient,
}

impl MovementProvider {
    /// Create a new Movement payment provider.
    ///
    /// The sender address is derived from the signing key's public key
    /// using the standard Aptos authentication key derivation.
    pub fn new(signing_key: SigningKey, rest_url: &str) -> Result<Self, MppError> {
        let pubkey = signing_key.verifying_key();
        let sender_address = derive_address(&pubkey.to_bytes());

        Ok(Self {
            signing_key,
            sender_address,
            rest_client: MovementRestClient::new(rest_url),
        })
    }

    /// Create a provider with an explicit sender address.
    pub fn with_address(signing_key: SigningKey, rest_url: &str, address: &str) -> Self {
        Self {
            signing_key,
            sender_address: address.to_string(),
            rest_client: MovementRestClient::new(rest_url),
        }
    }

    /// Get the sender address.
    pub fn address(&self) -> &str {
        &self.sender_address
    }

    /// Get a reference to the REST client.
    pub fn rest_client(&self) -> &MovementRestClient {
        &self.rest_client
    }

    /// Execute a charge payment: transfer tokens to the recipient.
    #[cfg_attr(
        feature = "observability",
        tracing::instrument(skip(self, challenge), fields(intent = "charge"))
    )]
    async fn pay_charge(
        &self,
        challenge: &PaymentChallenge,
    ) -> Result<PaymentCredential, MppError> {
        let request: ChargeRequest = challenge.request.decode()?;
        let amount = request.amount_u64()?;
        let recipient = request.recipient_str()?.to_string();
        let currency = request.currency_str();

        // Build the transfer payload based on the token type.
        let payload = build_transfer_payload(currency, &recipient, amount);

        mpp_info!(recipient = %recipient, amount = amount, "submitting charge payment");
        let tx_hash = self
            .rest_client
            .build_sign_submit(&self.signing_key, &self.sender_address, payload)
            .await?;
        mpp_info!(tx_hash = %tx_hash, "charge payment confirmed");

        let echo = challenge.to_echo();
        Ok(PaymentCredential::new(echo, PaymentPayload::hash(&tx_hash)))
    }

    // ==================== Session / Channel Operations ====================

    /// Open a payment channel on-chain.
    ///
    /// Returns `(tx_hash, channel_id)` where `channel_id` is the 32-byte
    /// deterministic ID derived from the channel parameters.
    ///
    /// # Arguments
    ///
    /// * `module_address` - Address where MovementStreamChannel is deployed
    /// * `registry_address` - Registry address (usually same as module)
    /// * `payee` - Recipient/server address
    /// * `token_metadata` - FA metadata address (e.g., `"0xa"` for MOVE)
    /// * `deposit` - Initial deposit in base units
    /// * `salt` - Random salt for channel ID uniqueness
    #[allow(clippy::too_many_arguments)]
    pub async fn open_channel(
        &self,
        module_address: &str,
        registry_address: &str,
        payee: &str,
        token_metadata: &str,
        deposit: u64,
        salt: &[u8],
    ) -> Result<(String, Vec<u8>), MppError> {
        let pubkey_bytes = self.signing_key.verifying_key().to_bytes();

        let payload = EntryFunctionPayload::new(
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

        mpp_info!(payee = %payee, deposit = deposit, "opening payment channel");
        let tx_hash = self
            .rest_client
            .build_sign_submit(&self.signing_key, &self.sender_address, payload)
            .await?;

        // Compute channel ID.
        let payer_bytes = parse_address_bytes(&self.sender_address)?;
        let payee_bytes = parse_address_bytes(payee)?;
        let token_bytes = parse_address_bytes(token_metadata)?;
        let channel_id = movement::voucher::compute_channel_id(
            &payer_bytes,
            &payee_bytes,
            &token_bytes,
            salt,
            &pubkey_bytes,
        );

        mpp_info!(tx_hash = %tx_hash, channel_id = %hex::encode(channel_id), "channel opened");
        Ok((tx_hash, channel_id.to_vec()))
    }

    /// Settle a payment channel on-chain with the latest voucher.
    ///
    /// Called by the payee (server) to claim accumulated payments.
    /// Returns the settlement transaction hash.
    pub async fn settle_channel(
        &self,
        module_address: &str,
        registry_address: &str,
        channel_id: &[u8],
        cumulative_amount: u64,
        signature: &[u8],
        authorized_pubkey: &[u8],
    ) -> Result<String, MppError> {
        let payload = EntryFunctionPayload::new(
            &format!("{}::channel::settle", module_address),
            vec![
                serde_json::json!(registry_address),
                serde_json::json!(format!("0x{}", hex::encode(channel_id))),
                serde_json::json!(cumulative_amount.to_string()),
                serde_json::json!(format!("0x{}", hex::encode(signature))),
                serde_json::json!(format!("0x{}", hex::encode(authorized_pubkey))),
            ],
        );

        mpp_info!(cumulative_amount = cumulative_amount, "settling channel");
        self.rest_client
            .build_sign_submit(&self.signing_key, &self.sender_address, payload)
            .await
    }

    /// Close a payment channel on-chain.
    ///
    /// Called by the payee (server) to finalize and close the channel.
    /// Returns the close transaction hash.
    pub async fn close_channel(
        &self,
        module_address: &str,
        registry_address: &str,
        channel_id: &[u8],
        cumulative_amount: u64,
        signature: &[u8],
        authorized_pubkey: &[u8],
    ) -> Result<String, MppError> {
        let payload = EntryFunctionPayload::new(
            &format!("{}::channel::close", module_address),
            vec![
                serde_json::json!(registry_address),
                serde_json::json!(format!("0x{}", hex::encode(channel_id))),
                serde_json::json!(cumulative_amount.to_string()),
                serde_json::json!(format!("0x{}", hex::encode(signature))),
                serde_json::json!(format!("0x{}", hex::encode(authorized_pubkey))),
            ],
        );

        mpp_info!(cumulative_amount = cumulative_amount, "closing channel");
        self.rest_client
            .build_sign_submit(&self.signing_key, &self.sender_address, payload)
            .await
    }

    /// Sign a voucher for a session payment.
    pub fn sign_voucher(&self, channel_id: &[u8], cumulative_amount: u64) -> [u8; 64] {
        movement::voucher::sign_voucher(&self.signing_key, channel_id, cumulative_amount)
    }

    /// Get the ed25519 public key bytes.
    pub fn public_key_bytes(&self) -> [u8; 32] {
        self.signing_key.verifying_key().to_bytes()
    }

    /// Get a reference to the signing key.
    pub fn signing_key(&self) -> &SigningKey {
        &self.signing_key
    }
}

impl PaymentProvider for MovementProvider {
    fn supports(&self, method: &str, intent: &str) -> bool {
        method == movement::METHOD_NAME
            && (intent == movement::INTENT_CHARGE || intent == movement::INTENT_SESSION)
    }

    async fn pay(&self, challenge: &PaymentChallenge) -> Result<PaymentCredential, MppError> {
        match challenge.intent.as_str() {
            "charge" => self.pay_charge(challenge).await,
            "session" => {
                // For session, the client only handles the initial charge/open.
                // Voucher signing is done separately via sign_voucher().
                self.pay_charge(challenge).await
            }
            _ => Err(MppError::UnsupportedPaymentMethod(format!(
                "unsupported intent: {}",
                challenge.intent
            ))),
        }
    }
}

/// Build the appropriate transfer payload for the given token.
///
/// - Native MOVE (`0xa`): uses `aptos_account::transfer` (2 args: recipient, amount)
/// - Any other FA token: uses `primary_fungible_store::transfer` (3 args: metadata, recipient, amount)
fn build_transfer_payload(currency: &str, recipient: &str, amount: u64) -> EntryFunctionPayload {
    if is_native_move(currency) {
        // Native MOVE token — use the simple transfer
        EntryFunctionPayload::new(
            "0x1::aptos_account::transfer",
            vec![
                serde_json::json!(recipient),
                serde_json::json!(amount.to_string()),
            ],
        )
    } else {
        // Fungible Asset token — use primary_fungible_store::transfer
        // with the FA metadata address as the first argument
        EntryFunctionPayload::new(
            "0x1::primary_fungible_store::transfer",
            vec![
                serde_json::json!(currency),
                serde_json::json!(recipient),
                serde_json::json!(amount.to_string()),
            ],
        )
        .with_type_arguments(vec!["0x1::fungible_asset::Metadata".to_string()])
    }
}

/// Check if a currency address is the native MOVE token.
///
/// The native MOVE token has the special FA metadata address `0xa`
/// (which is `0x000...00a` when fully expanded).
fn is_native_move(currency: &str) -> bool {
    let hex = currency.strip_prefix("0x").unwrap_or(currency);
    // Normalize: strip leading zeros and compare
    let trimmed = hex.trim_start_matches('0');
    trimmed.eq_ignore_ascii_case("a")
}

/// Derive an Aptos account address from an ed25519 public key.
///
/// address = sha3_256(pubkey || 0x00)  (scheme byte for ed25519)
fn derive_address(pubkey: &[u8; 32]) -> String {
    use sha3::{Digest, Sha3_256};
    let mut hasher = Sha3_256::new();
    hasher.update(pubkey);
    hasher.update([0x00]); // Ed25519 scheme byte
    let hash = hasher.finalize();
    format!("0x{}", hex::encode(hash))
}

/// Parse a hex address string to a 32-byte array.
fn parse_address_bytes(addr: &str) -> Result<[u8; 32], MppError> {
    let hex_str = addr.strip_prefix("0x").unwrap_or(addr);
    // Pad to 64 hex chars (32 bytes) if short (e.g., "0xa" → 32 bytes).
    let padded = format!("{:0>64}", hex_str);
    let bytes = hex::decode(&padded)
        .map_err(|e| MppError::Http(format!("invalid address hex '{}': {}", addr, e)))?;
    bytes
        .try_into()
        .map_err(|_| MppError::Http(format!("address must be 32 bytes: {}", addr)))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_derive_address() {
        let pubkey = [0xAB_u8; 32];
        let addr = derive_address(&pubkey);
        assert!(addr.starts_with("0x"));
        assert_eq!(addr.len(), 66); // 0x + 64 hex chars
    }

    #[test]
    fn test_parse_address_bytes_full() {
        let addr = format!("0x{}", "ab".repeat(32));
        let bytes = parse_address_bytes(&addr).unwrap();
        assert_eq!(bytes, [0xAB; 32]);
    }

    #[test]
    fn test_parse_address_bytes_short() {
        let bytes = parse_address_bytes("0xa").unwrap();
        assert_eq!(bytes[31], 0x0a);
        assert_eq!(bytes[0], 0x00);
    }

    #[test]
    fn test_parse_address_bytes_no_prefix() {
        let addr = "ab".repeat(32);
        let bytes = parse_address_bytes(&addr).unwrap();
        assert_eq!(bytes, [0xAB; 32]);
    }

    #[test]
    fn test_provider_supports() {
        let key = SigningKey::from_bytes(&[0x42; 32]);
        let provider =
            MovementProvider::with_address(key, "https://testnet.movementnetwork.xyz/v1", "0xabc");
        assert!(provider.supports("movement", "charge"));
        assert!(provider.supports("movement", "session"));
        assert!(!provider.supports("other", "charge"));
        assert!(!provider.supports("movement", "authorize"));
    }

    #[test]
    fn test_sign_voucher() {
        let key = SigningKey::from_bytes(&[0x42; 32]);
        let provider =
            MovementProvider::with_address(key, "https://testnet.movementnetwork.xyz/v1", "0xabc");

        let channel_id = [0xAB_u8; 32];
        let sig = provider.sign_voucher(&channel_id, 1000);
        assert_eq!(sig.len(), 64);

        // Verify the signature.
        let pubkey = provider.public_key_bytes();
        let valid = movement::voucher::verify_voucher(&channel_id, 1000, &sig, &pubkey, &[]);
        assert!(valid);
    }

    #[test]
    fn test_is_native_move() {
        assert!(is_native_move("0xa"));
        assert!(is_native_move("0x0a"));
        assert!(is_native_move(
            "0x000000000000000000000000000000000000000000000000000000000000000a"
        ));
        assert!(is_native_move("0xA"));
        assert!(is_native_move("a"));
        assert!(!is_native_move(
            "0x63f169ba69623ba6ccf34620857644feb46d0f87e1d7bbcf8c071d30c3d94bd6"
        ));
        assert!(!is_native_move(
            "0xc6f5b46ab5307dfe3e565668edcc1461b31cac5a6c2739fba17d9fdde16813a2"
        ));
        assert!(!is_native_move("0x1"));
        assert!(!is_native_move("0xab"));
    }

    #[test]
    fn test_build_transfer_payload_native() {
        let payload = build_transfer_payload("0xa", "0xrecipient", 1000);
        assert_eq!(payload.function, "0x1::aptos_account::transfer");
        assert_eq!(payload.arguments.len(), 2);
        assert!(payload.type_arguments.is_empty());
    }

    #[test]
    fn test_build_transfer_payload_fa_token() {
        let usdc_metadata = "0x63f169ba69623ba6ccf34620857644feb46d0f87e1d7bbcf8c071d30c3d94bd6";
        let payload = build_transfer_payload(usdc_metadata, "0xrecipient", 500);
        assert_eq!(payload.function, "0x1::primary_fungible_store::transfer");
        assert_eq!(payload.arguments.len(), 3);
        // First arg is the metadata address
        assert_eq!(payload.arguments[0].as_str().unwrap(), usdc_metadata);
        // Second arg is the recipient
        assert_eq!(payload.arguments[1].as_str().unwrap(), "0xrecipient");
        // Type argument for FA
        assert_eq!(
            payload.type_arguments,
            vec!["0x1::fungible_asset::Metadata"]
        );
    }
}

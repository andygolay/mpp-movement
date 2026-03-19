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

use ed25519_dalek::SigningKey;

use crate::client::PaymentProvider;
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
    pub fn with_address(
        signing_key: SigningKey,
        rest_url: &str,
        address: &str,
    ) -> Self {
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
    async fn pay_charge(
        &self,
        challenge: &PaymentChallenge,
    ) -> Result<PaymentCredential, MppError> {
        let request: ChargeRequest = challenge.request.decode()?;
        let amount = request.amount_u64()?;
        let recipient = request.recipient_str()?.to_string();
        let _currency = request.currency_str();

        // Build a transfer transaction.
        // Use aptos_account::transfer which handles both coin and FA.
        let payload = EntryFunctionPayload::new(
            "0x1::aptos_account::transfer",
            vec![
                serde_json::json!(recipient),
                serde_json::json!(amount.to_string()),
            ],
        );

        let tx_hash = self
            .rest_client
            .build_sign_submit(&self.signing_key, &self.sender_address, payload)
            .await?;

        let echo = challenge.to_echo();
        Ok(PaymentCredential::new(
            echo,
            PaymentPayload::hash(&tx_hash),
        ))
    }

    /// Execute a session open: call channel::open on the Move contract.
    #[allow(dead_code)]
    async fn pay_session_open(
        &self,
        _challenge: &PaymentChallenge,
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

        Ok((tx_hash, channel_id.to_vec()))
    }

    /// Sign a voucher for a session payment.
    pub fn sign_voucher(&self, channel_id: &[u8], cumulative_amount: u64) -> [u8; 64] {
        movement::voucher::sign_voucher(&self.signing_key, channel_id, cumulative_amount)
    }

    /// Get the ed25519 public key bytes.
    pub fn public_key_bytes(&self) -> [u8; 32] {
        self.signing_key.verifying_key().to_bytes()
    }
}

impl PaymentProvider for MovementProvider {
    fn supports(&self, method: &str, intent: &str) -> bool {
        method == movement::METHOD_NAME
            && (intent == movement::INTENT_CHARGE || intent == movement::INTENT_SESSION)
    }

    async fn pay(
        &self,
        challenge: &PaymentChallenge,
    ) -> Result<PaymentCredential, MppError> {
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
#[allow(dead_code)]
fn parse_address_bytes(addr: &str) -> Result<[u8; 32], MppError> {
    let hex_str = addr.strip_prefix("0x").unwrap_or(addr);
    // Pad to 64 hex chars (32 bytes) if short (e.g., "0xa" → 32 bytes).
    let padded = format!("{:0>64}", hex_str);
    let bytes = hex::decode(&padded).map_err(|e| {
        MppError::Http(format!("invalid address hex '{}': {}", addr, e))
    })?;
    bytes.try_into().map_err(|_| {
        MppError::Http(format!("address must be 32 bytes: {}", addr))
    })
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
        let provider = MovementProvider::with_address(
            key,
            "https://testnet.movementnetwork.xyz/v1",
            "0xabc",
        );
        assert!(provider.supports("movement", "charge"));
        assert!(provider.supports("movement", "session"));
        assert!(!provider.supports("tempo", "charge"));
        assert!(!provider.supports("movement", "authorize"));
    }

    #[test]
    fn test_sign_voucher() {
        let key = SigningKey::from_bytes(&[0x42; 32]);
        let provider = MovementProvider::with_address(
            key,
            "https://testnet.movementnetwork.xyz/v1",
            "0xabc",
        );

        let channel_id = [0xAB_u8; 32];
        let sig = provider.sign_voucher(&channel_id, 1000);
        assert_eq!(sig.len(), 64);

        // Verify the signature.
        let pubkey = provider.public_key_bytes();
        let valid = movement::voucher::verify_voucher(
            &channel_id,
            1000,
            &sig,
            &pubkey,
            &[],
        );
        assert!(valid);
    }
}

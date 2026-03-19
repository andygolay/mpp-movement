//! Movement Network payment method for Web Payment Auth.
//!
//! This module provides Movement-specific implementations for the Machine
//! Payments Protocol, using the TempoStreamChannel Move contract deployed
//! on Movement Network.
//!
//! # Differences from the Tempo (EVM) method
//!
//! | Aspect | Tempo (EVM) | Movement |
//! |--------|-------------|----------|
//! | Signature | EIP-712 + ECDSA (secp256k1) | ed25519 |
//! | Serialization | ABI encoding | BCS |
//! | Hash | keccak256 | sha3-256 |
//! | Token standard | TIP-20 (ERC-20) | Fungible Asset (FA) |
//! | Address format | 20-byte EVM | 32-byte Move |
//!
//! # Types
//!
//! - [`MovementMethodDetails`]: Movement-specific method details
//! - [`MovementNetwork`]: Network configuration (mainnet/testnet)
//! - [`MovementChargeExt`]: Extension trait for ChargeRequest
//! - [`MovementSessionExt`]: Extension trait for SessionRequest
//! - [`SessionCredentialPayload`]: Session lifecycle credential payloads
//!
//! # Voucher Helpers
//!
//! - [`voucher::sign_voucher`]: Sign a voucher with ed25519
//! - [`voucher::verify_voucher`]: Verify a voucher signature
//! - [`voucher::compute_channel_id`]: Compute channel ID (sha3-256)

pub mod charge;
pub mod network;
pub mod session;
pub mod types;
pub mod voucher;

#[cfg(feature = "client")]
pub mod rest_client;

#[cfg(feature = "server")]
pub mod session_method;

pub use charge::MovementChargeExt;
pub use network::MovementNetwork;
pub use session::{MovementSessionExt, MovementSessionMethodDetails, SessionCredentialPayload};
pub use types::MovementMethodDetails;
pub use voucher::{compute_channel_id, sign_voucher, verify_voucher};

#[cfg(feature = "client")]
pub use rest_client::MovementRestClient;

#[cfg(feature = "server")]
pub use session_method::{
    ChannelState, ChannelStore, InMemoryChannelStore, SessionMethod, SessionMethodConfig,
    deduct_from_channel,
};

/// Payment method name for Movement.
pub const METHOD_NAME: &str = "movement";

/// Charge intent name.
pub const INTENT_CHARGE: &str = "charge";

/// Session intent name.
pub const INTENT_SESSION: &str = "session";

/// Default REST API URL for Movement mainnet.
pub const DEFAULT_REST_URL_MAINNET: &str = "https://mainnet.movementnetwork.xyz/v1";

/// Default REST API URL for Movement testnet.
pub const DEFAULT_REST_URL_TESTNET: &str = "https://testnet.movementnetwork.xyz/v1";

/// Default faucet URL for Movement testnet.
pub const DEFAULT_FAUCET_URL_TESTNET: &str = "https://faucet.testnet.movementnetwork.xyz";

/// MOVE token FA metadata address.
pub const MOVE_TOKEN_METADATA: &str = "0xa";

/// Default deployed TempoStreamChannel module address (testnet).
pub const DEFAULT_MODULE_ADDRESS: &str =
    "0x3e9edf3be513781a6db0706b652da425ad67f58b5cb366847126bf0fb716fc58";

/// Default challenge expiration in minutes.
pub const DEFAULT_EXPIRES_MINUTES: u64 = 5;

/// Create a Movement charge challenge with minimal parameters.
///
/// # Arguments
///
/// * `secret_key` - Server secret key for HMAC-bound challenge ID
/// * `realm` - Protection space / realm (e.g., "api.example.com")
/// * `amount` - Amount in base units (e.g., "1000000" for 0.01 MOVE)
/// * `currency` - Token metadata address (e.g., "0xa" for MOVE)
/// * `recipient` - Recipient address
pub fn charge_challenge(
    secret_key: &str,
    realm: &str,
    amount: &str,
    currency: &str,
    recipient: &str,
) -> crate::error::Result<crate::protocol::core::PaymentChallenge> {
    let request = crate::protocol::intents::ChargeRequest {
        amount: amount.to_string(),
        currency: currency.to_string(),
        recipient: Some(recipient.to_string()),
        ..Default::default()
    };

    charge_challenge_with_options(secret_key, realm, &request, None, None)
}

/// Create a Movement charge challenge with full options.
pub fn charge_challenge_with_options(
    secret_key: &str,
    realm: &str,
    request: &crate::protocol::intents::ChargeRequest,
    expires: Option<&str>,
    description: Option<&str>,
) -> crate::error::Result<crate::protocol::core::PaymentChallenge> {
    use crate::protocol::core::{Base64UrlJson, PaymentChallenge};
    use time::{Duration, OffsetDateTime};

    let request = request.clone().with_base_units()?;
    let encoded_request = Base64UrlJson::from_typed(&request)?;

    let default_expires;
    let expires = match expires {
        Some(e) => Some(e),
        None => {
            let expiry_time =
                OffsetDateTime::now_utc() + Duration::minutes(DEFAULT_EXPIRES_MINUTES as i64);
            default_expires = expiry_time
                .format(&time::format_description::well_known::Rfc3339)
                .map_err(|e| {
                    crate::error::MppError::InvalidConfig(format!("failed to format expires: {e}"))
                })?;
            Some(default_expires.as_str())
        }
    };

    let id = crate::protocol::core::compute_challenge_id(
        secret_key,
        realm,
        METHOD_NAME,
        INTENT_CHARGE,
        encoded_request.raw(),
        expires,
        None,
        None,
    );

    Ok(PaymentChallenge {
        id,
        realm: realm.to_string(),
        method: METHOD_NAME.into(),
        intent: INTENT_CHARGE.into(),
        request: encoded_request,
        expires: expires.map(|s| s.to_string()),
        description: description.map(|s| s.to_string()),
        digest: None,
        opaque: None,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    const TEST_SECRET: &str = "test-secret-key";

    #[test]
    fn test_charge_challenge_basic() {
        let challenge = charge_challenge(
            TEST_SECRET,
            "api.example.com",
            "1000000",
            "0xa",
            "0x3e9edf3be513781a6db0706b652da425ad67f58b5cb366847126bf0fb716fc58",
        )
        .unwrap();

        assert_eq!(challenge.method.as_str(), "movement");
        assert_eq!(challenge.intent.as_str(), "charge");
        assert!(challenge.expires.is_some());
        assert_eq!(challenge.id.len(), 43); // base64url HMAC-SHA256
    }

    #[test]
    fn test_challenge_id_is_deterministic() {
        use crate::protocol::intents::ChargeRequest;

        let request = ChargeRequest {
            amount: "1000000".into(),
            currency: "0xa".into(),
            recipient: Some(
                "0x3e9edf3be513781a6db0706b652da425ad67f58b5cb366847126bf0fb716fc58".into(),
            ),
            ..Default::default()
        };

        let c1 = charge_challenge_with_options(
            TEST_SECRET,
            "api.example.com",
            &request,
            Some("2026-01-01T00:00:00Z"),
            None,
        )
        .unwrap();

        let c2 = charge_challenge_with_options(
            TEST_SECRET,
            "api.example.com",
            &request,
            Some("2026-01-01T00:00:00Z"),
            None,
        )
        .unwrap();

        assert_eq!(c1.id, c2.id);
    }

    #[test]
    fn test_challenge_id_differs_for_different_params() {
        let c1 = charge_challenge(TEST_SECRET, "api.example.com", "1000000", "0xa", "0xabc")
            .unwrap();
        let c2 = charge_challenge(TEST_SECRET, "api.example.com", "2000000", "0xa", "0xabc")
            .unwrap();

        assert_ne!(c1.id, c2.id);
    }

    #[test]
    fn test_constants() {
        assert_eq!(METHOD_NAME, "movement");
        assert_eq!(MOVE_TOKEN_METADATA, "0xa");
        assert!(DEFAULT_MODULE_ADDRESS.starts_with("0x"));
    }
}

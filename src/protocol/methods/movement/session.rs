//! Movement extensions for SessionRequest.
//!
//! Provides Movement-specific accessors and credential payload types for SessionRequest.

use crate::error::{MppError, Result};
use crate::protocol::intents::SessionRequest;
use serde::{Deserialize, Serialize};

/// Movement session-specific method details.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct MovementSessionMethodDetails {
    /// Module address where MovementStreamChannel is deployed.
    pub module_address: String,

    /// Registry address (usually same as module address).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub registry_address: Option<String>,

    /// Token FA metadata address (e.g., "0xa" for MOVE).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub token_metadata: Option<String>,

    /// Pre-existing channel ID (hex), if reconnecting to an open channel.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub channel_id: Option<String>,

    /// Minimum voucher delta to accept (in base units).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub min_voucher_delta: Option<String>,

    /// Network identifier ("testnet" or "mainnet").
    #[serde(skip_serializing_if = "Option::is_none")]
    pub network: Option<String>,
}

/// Session credential payload for Movement, discriminated on `action`.
///
/// Each variant corresponds to a channel lifecycle action:
/// - `Open`: open a new payment channel (with on-chain transaction hash)
/// - `TopUp`: add funds to an existing channel
/// - `Voucher`: off-chain payment voucher (ed25519 signed)
/// - `Close`: close the channel with final voucher
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "action", rename_all = "camelCase")]
pub enum SessionCredentialPayload {
    #[serde(rename = "open")]
    Open {
        /// Transaction hash of the on-chain open call.
        #[serde(rename = "txHash")]
        tx_hash: String,
        /// Channel ID (hex).
        #[serde(rename = "channelId")]
        channel_id: String,
        /// Ed25519 public key of the authorized signer (hex).
        #[serde(rename = "authorizedSigner", skip_serializing_if = "Option::is_none")]
        authorized_signer: Option<String>,
        /// Initial voucher cumulative amount.
        #[serde(rename = "cumulativeAmount")]
        cumulative_amount: String,
        /// Ed25519 signature (hex).
        signature: String,
    },
    #[serde(rename = "topUp")]
    TopUp {
        /// Transaction hash of the on-chain top_up call.
        #[serde(rename = "txHash")]
        tx_hash: String,
        /// Channel ID (hex).
        #[serde(rename = "channelId")]
        channel_id: String,
        /// Additional deposit amount.
        #[serde(rename = "additionalDeposit")]
        additional_deposit: String,
    },
    #[serde(rename = "voucher")]
    Voucher {
        /// Channel ID (hex).
        #[serde(rename = "channelId")]
        channel_id: String,
        /// Cumulative amount authorized.
        #[serde(rename = "cumulativeAmount")]
        cumulative_amount: String,
        /// Ed25519 signature (hex).
        signature: String,
    },
    #[serde(rename = "close")]
    Close {
        /// Channel ID (hex).
        #[serde(rename = "channelId")]
        channel_id: String,
        /// Final cumulative amount.
        #[serde(rename = "cumulativeAmount")]
        cumulative_amount: String,
        /// Ed25519 signature (hex).
        signature: String,
    },
}

/// Extension trait for SessionRequest with Movement-specific accessors.
pub trait MovementSessionExt {
    /// Get the module address from methodDetails.
    fn module_address(&self) -> Result<String>;

    /// Get the registry address from methodDetails.
    fn registry_address(&self) -> Option<String>;

    /// Get the token metadata address from methodDetails.
    fn token_metadata(&self) -> Option<String>;

    /// Get the channel ID from methodDetails, if present.
    fn channel_id(&self) -> Option<String>;

    /// Get the minimum voucher delta from methodDetails, if present.
    fn min_voucher_delta(&self) -> Option<String>;

    /// Get the network name from methodDetails.
    fn network_name(&self) -> Option<String>;

    /// Parse the method_details as Movement session-specific details.
    fn movement_session_details(&self) -> Result<MovementSessionMethodDetails>;
}

impl MovementSessionExt for SessionRequest {
    fn module_address(&self) -> Result<String> {
        self.method_details
            .as_ref()
            .and_then(|v| v.get("moduleAddress"))
            .and_then(|v| v.as_str())
            .map(|s| s.to_string())
            .ok_or_else(|| {
                MppError::invalid_challenge_reason(
                    "Missing moduleAddress in methodDetails".to_string(),
                )
            })
    }

    fn registry_address(&self) -> Option<String> {
        self.method_details
            .as_ref()
            .and_then(|v| v.get("registryAddress"))
            .and_then(|v| v.as_str())
            .map(|s| s.to_string())
    }

    fn token_metadata(&self) -> Option<String> {
        self.method_details
            .as_ref()
            .and_then(|v| v.get("tokenMetadata"))
            .and_then(|v| v.as_str())
            .map(|s| s.to_string())
    }

    fn channel_id(&self) -> Option<String> {
        self.method_details
            .as_ref()
            .and_then(|v| v.get("channelId"))
            .and_then(|v| v.as_str())
            .map(|s| s.to_string())
    }

    fn min_voucher_delta(&self) -> Option<String> {
        self.method_details
            .as_ref()
            .and_then(|v| v.get("minVoucherDelta"))
            .and_then(|v| v.as_str())
            .map(|s| s.to_string())
    }

    fn network_name(&self) -> Option<String> {
        self.method_details
            .as_ref()
            .and_then(|v| v.get("network"))
            .and_then(|v| v.as_str())
            .map(|s| s.to_string())
    }

    fn movement_session_details(&self) -> Result<MovementSessionMethodDetails> {
        match &self.method_details {
            Some(value) => serde_json::from_value(value.clone()).map_err(|e| {
                MppError::invalid_challenge_reason(format!(
                    "Invalid Movement session method details: {}",
                    e
                ))
            }),
            None => Err(MppError::invalid_challenge_reason(
                "Missing methodDetails for session intent".to_string(),
            )),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn test_session_request() -> SessionRequest {
        SessionRequest {
            amount: "100000".to_string(),
            unit_type: Some("request".to_string()),
            currency: "0xa".to_string(),
            recipient: Some(
                "0x74f1060add0c641a0c10bb5bab2bf5fd05f94d7c25055f2419fa82d7bbf2b1e8".to_string(),
            ),
            suggested_deposit: Some("1000000".to_string()),
            method_details: Some(serde_json::json!({
                "moduleAddress": "0x74f1060add0c641a0c10bb5bab2bf5fd05f94d7c25055f2419fa82d7bbf2b1e8",
                "registryAddress": "0x74f1060add0c641a0c10bb5bab2bf5fd05f94d7c25055f2419fa82d7bbf2b1e8",
                "tokenMetadata": "0xa",
                "channelId": "0xabc123",
                "minVoucherDelta": "10000",
                "network": "testnet"
            })),
            ..Default::default()
        }
    }

    #[test]
    fn test_module_address() {
        let req = test_session_request();
        assert!(req.module_address().unwrap().starts_with("0x3e9e"));
    }

    #[test]
    fn test_channel_id() {
        let req = test_session_request();
        assert_eq!(req.channel_id(), Some("0xabc123".to_string()));
    }

    #[test]
    fn test_min_voucher_delta() {
        let req = test_session_request();
        assert_eq!(req.min_voucher_delta(), Some("10000".to_string()));
    }

    #[test]
    fn test_network_name() {
        let req = test_session_request();
        assert_eq!(req.network_name(), Some("testnet".to_string()));
    }

    #[test]
    fn test_missing_module_address() {
        let req = SessionRequest {
            method_details: None,
            ..test_session_request()
        };
        assert!(req.module_address().is_err());
    }

    #[test]
    fn test_credential_payload_voucher() {
        let json = r#"{"action":"voucher","channelId":"0xabc","cumulativeAmount":"5000","signature":"0xdef"}"#;
        let payload: SessionCredentialPayload = serde_json::from_str(json).unwrap();
        assert!(matches!(payload, SessionCredentialPayload::Voucher { .. }));
    }

    #[test]
    fn test_credential_payload_open() {
        let json = r#"{"action":"open","txHash":"0xtx123","channelId":"0xabc","cumulativeAmount":"1000","signature":"0xsig"}"#;
        let payload: SessionCredentialPayload = serde_json::from_str(json).unwrap();
        assert!(matches!(payload, SessionCredentialPayload::Open { .. }));
    }

    #[test]
    fn test_credential_payload_close() {
        let json = r#"{"action":"close","channelId":"0xabc","cumulativeAmount":"9000","signature":"0xsig"}"#;
        let payload: SessionCredentialPayload = serde_json::from_str(json).unwrap();
        assert!(matches!(payload, SessionCredentialPayload::Close { .. }));
    }

    #[test]
    fn test_credential_payload_topup() {
        let json = r#"{"action":"topUp","txHash":"0xtx456","channelId":"0xabc","additionalDeposit":"5000"}"#;
        let payload: SessionCredentialPayload = serde_json::from_str(json).unwrap();
        assert!(matches!(payload, SessionCredentialPayload::TopUp { .. }));
    }

    #[test]
    fn test_session_method_details_serialization() {
        let details = MovementSessionMethodDetails {
            module_address: "0xabcd".to_string(),
            registry_address: Some("0xabcd".to_string()),
            token_metadata: Some("0xa".to_string()),
            channel_id: None,
            min_voucher_delta: Some("1000".to_string()),
            network: Some("testnet".to_string()),
        };

        let json = serde_json::to_string(&details).unwrap();
        assert!(json.contains("\"moduleAddress\":\"0xabcd\""));
        assert!(json.contains("\"minVoucherDelta\":\"1000\""));
        assert!(!json.contains("channelId"));

        let parsed: MovementSessionMethodDetails = serde_json::from_str(&json).unwrap();
        assert_eq!(parsed.module_address, "0xabcd");
    }
}

//! Movement extensions for ChargeRequest.
//!
//! Provides Movement-specific accessors for ChargeRequest.

use super::types::MovementMethodDetails;
use crate::error::{MppError, Result};
use crate::protocol::intents::ChargeRequest;

/// Extension trait for ChargeRequest with Movement-specific accessors.
pub trait MovementChargeExt {
    /// Get the amount as u64 (Movement uses u64 amounts).
    fn amount_u64(&self) -> Result<u64>;

    /// Get the recipient address as a string.
    fn recipient_str(&self) -> Result<&str>;

    /// Get the currency/token metadata address.
    fn currency_str(&self) -> &str;

    /// Get the network identifier from methodDetails.
    fn network_name(&self) -> Option<&str>;

    /// Parse the method_details as Movement-specific details.
    fn movement_method_details(&self) -> Result<MovementMethodDetails>;

    /// Check if this request targets testnet.
    fn is_testnet(&self) -> bool;

    /// Get the Movement network, if recognized.
    fn network(&self) -> Option<super::network::MovementNetwork>;
}

impl MovementChargeExt for ChargeRequest {
    fn amount_u64(&self) -> Result<u64> {
        self.amount.parse::<u64>().map_err(|e| {
            MppError::InvalidAmount(format!("Failed to parse amount '{}': {}", self.amount, e))
        })
    }

    fn recipient_str(&self) -> Result<&str> {
        self.recipient
            .as_deref()
            .ok_or_else(|| MppError::invalid_challenge_reason("No recipient specified".to_string()))
    }

    fn currency_str(&self) -> &str {
        &self.currency
    }

    fn network_name(&self) -> Option<&str> {
        self.method_details
            .as_ref()
            .and_then(|v| v.get("network"))
            .and_then(|v| v.as_str())
    }

    fn movement_method_details(&self) -> Result<MovementMethodDetails> {
        match &self.method_details {
            Some(value) => serde_json::from_value(value.clone()).map_err(|e| {
                MppError::invalid_challenge_reason(format!(
                    "Invalid Movement method details: {}",
                    e
                ))
            }),
            None => Ok(MovementMethodDetails::default()),
        }
    }

    fn is_testnet(&self) -> bool {
        self.network_name() == Some("testnet")
    }

    fn network(&self) -> Option<super::network::MovementNetwork> {
        match self.network_name() {
            Some("testnet") => Some(super::network::MovementNetwork::Testnet),
            Some("mainnet") | Some("movement") => Some(super::network::MovementNetwork::Mainnet),
            _ => None,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn test_charge_request() -> ChargeRequest {
        ChargeRequest {
            amount: "1000000".to_string(),
            currency: "0xa".to_string(),
            recipient: Some(
                "0x74f1060add0c641a0c10bb5bab2bf5fd05f94d7c25055f2419fa82d7bbf2b1e8".to_string(),
            ),
            description: None,
            external_id: None,
            method_details: Some(serde_json::json!({
                "network": "testnet",
                "moduleAddress": "0x74f1060add0c641a0c10bb5bab2bf5fd05f94d7c25055f2419fa82d7bbf2b1e8"
            })),
            ..Default::default()
        }
    }

    #[test]
    fn test_amount_u64() {
        let req = test_charge_request();
        assert_eq!(req.amount_u64().unwrap(), 1_000_000);
    }

    #[test]
    fn test_is_testnet() {
        let req = test_charge_request();
        assert!(req.is_testnet());

        let req_mainnet = ChargeRequest {
            method_details: Some(serde_json::json!({"network": "mainnet"})),
            ..test_charge_request()
        };
        assert!(!req_mainnet.is_testnet());
    }

    #[test]
    fn test_network() {
        let req = test_charge_request();
        assert_eq!(
            req.network(),
            Some(super::super::network::MovementNetwork::Testnet)
        );
    }

    #[test]
    fn test_movement_method_details() {
        let req = test_charge_request();
        let details = req.movement_method_details().unwrap();
        assert!(details.is_testnet());
        assert!(details.module_address.is_some());
    }

    #[test]
    fn test_no_method_details() {
        let req = ChargeRequest {
            method_details: None,
            ..test_charge_request()
        };
        assert!(!req.is_testnet());
        assert!(req.network().is_none());
    }
}

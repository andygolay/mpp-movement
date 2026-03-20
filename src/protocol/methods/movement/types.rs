//! Movement-specific types for Web Payment Auth.

use serde::{Deserialize, Serialize};

/// Movement method-specific details in payment requests.
///
/// # Examples
///
/// ```
/// use mpp::protocol::methods::movement::MovementMethodDetails;
///
/// let details = MovementMethodDetails {
///     network: Some("testnet".into()),
///     module_address: None,
///     registry_address: None,
/// };
/// assert!(details.is_testnet());
/// ```
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct MovementMethodDetails {
    /// Network identifier ("mainnet" or "testnet").
    #[serde(skip_serializing_if = "Option::is_none")]
    pub network: Option<String>,

    /// The address where the MovementStreamChannel module is deployed.
    #[serde(rename = "moduleAddress", skip_serializing_if = "Option::is_none")]
    pub module_address: Option<String>,

    /// The address of the channel registry (usually same as module address).
    #[serde(rename = "registryAddress", skip_serializing_if = "Option::is_none")]
    pub registry_address: Option<String>,
}

impl MovementMethodDetails {
    /// Check if this is for the Movement testnet.
    pub fn is_testnet(&self) -> bool {
        self.network.as_deref() == Some("testnet")
    }

    /// Get the effective module address, falling back to the default.
    pub fn module_address_or_default(&self) -> &str {
        self.module_address
            .as_deref()
            .unwrap_or(super::DEFAULT_MODULE_ADDRESS)
    }

    /// Get the effective registry address, falling back to module address.
    pub fn registry_address_or_default(&self) -> &str {
        self.registry_address
            .as_deref()
            .or(self.module_address.as_deref())
            .unwrap_or(super::DEFAULT_MODULE_ADDRESS)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_serialization() {
        let details = MovementMethodDetails {
            network: Some("testnet".into()),
            module_address: Some("0xabcd".into()),
            registry_address: None,
        };

        let json = serde_json::to_string(&details).unwrap();
        assert!(json.contains("\"network\":\"testnet\""));
        assert!(json.contains("\"moduleAddress\":\"0xabcd\""));
        assert!(!json.contains("registryAddress"));

        let parsed: MovementMethodDetails = serde_json::from_str(&json).unwrap();
        assert!(parsed.is_testnet());
    }

    #[test]
    fn test_defaults() {
        let details = MovementMethodDetails::default();
        assert!(!details.is_testnet());
        assert_eq!(
            details.module_address_or_default(),
            super::super::DEFAULT_MODULE_ADDRESS
        );
    }

    #[test]
    fn test_registry_falls_back_to_module() {
        let details = MovementMethodDetails {
            module_address: Some("0x1234".into()),
            ..Default::default()
        };
        assert_eq!(details.registry_address_or_default(), "0x1234");
    }
}

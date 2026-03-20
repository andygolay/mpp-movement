//! Movement network configuration.

use core::fmt;

/// Known Movement blockchain networks.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum MovementNetwork {
    /// Movement mainnet
    Mainnet,
    /// Movement testnet
    Testnet,
}

impl MovementNetwork {
    /// Returns the default REST API URL for this network.
    pub const fn default_rest_url(self) -> &'static str {
        match self {
            Self::Mainnet => super::DEFAULT_REST_URL_MAINNET,
            Self::Testnet => super::DEFAULT_REST_URL_TESTNET,
        }
    }

    /// Returns the default faucet URL for this network (testnet only).
    pub const fn default_faucet_url(self) -> Option<&'static str> {
        match self {
            Self::Mainnet => None,
            Self::Testnet => Some(super::DEFAULT_FAUCET_URL_TESTNET),
        }
    }

    /// Returns the default currency (FA metadata address) for this network.
    pub const fn default_currency(self) -> &'static str {
        // MOVE token on both networks
        super::MOVE_TOKEN_METADATA
    }

    /// Returns the default deployed MovementStreamChannel module address for this network.
    pub const fn default_module_address(self) -> &'static str {
        match self {
            Self::Mainnet => super::DEFAULT_MODULE_ADDRESS_MAINNET,
            Self::Testnet => super::DEFAULT_MODULE_ADDRESS_TESTNET,
        }
    }

    /// Returns a string identifier for this network.
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Mainnet => "movement",
            Self::Testnet => "movement-testnet",
        }
    }
}

impl fmt::Display for MovementNetwork {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.as_str())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn as_str_matches_expected() {
        assert_eq!(MovementNetwork::Mainnet.as_str(), "movement");
        assert_eq!(MovementNetwork::Testnet.as_str(), "movement-testnet");
    }

    #[test]
    fn display_output() {
        assert_eq!(format!("{}", MovementNetwork::Mainnet), "movement");
        assert_eq!(format!("{}", MovementNetwork::Testnet), "movement-testnet");
    }

    #[test]
    fn testnet_has_faucet() {
        assert!(MovementNetwork::Testnet.default_faucet_url().is_some());
        assert!(MovementNetwork::Mainnet.default_faucet_url().is_none());
    }
}

//! Server-side payment verification.
//!
//! # Simple API
//!
//! ```ignore
//! use mpp::server::{Mpp, movement, MovementConfig};
//!
//! let mpp = Mpp::create_movement(movement(MovementConfig {
//!     recipient: "0x3e9e...",
//! }))?;
//!
//! let challenge = mpp.movement_charge("0.10")?;
//! ```
//!
//! # Advanced API
//!
//! ```ignore
//! use mpp::server::{Mpp, MovementChargeMethod};
//!
//! let method = MovementChargeMethod::new("https://testnet.movementnetwork.xyz/v1");
//! let payment = Mpp::new(method, "api.example.com", "my-server-secret");
//!
//! let challenge = payment.movement_charge_challenge("100000", "0xa", "0x...")?;
//! let receipt = payment.verify(&credential, &request).await?;
//! ```

mod amount;
mod mpp;
pub mod sse;

#[cfg(feature = "tower")]
pub mod middleware;

#[cfg(feature = "axum")]
pub mod axum;

pub use crate::protocol::traits::{ChargeMethod, ErrorCode, SessionMethod, VerificationError};
pub use amount::{parse_dollar_amount, AmountError};
pub use mpp::{Mpp, SessionVerifyResult};

#[cfg(all(feature = "movement", feature = "server", feature = "client"))]
pub use crate::protocol::methods::movement::ChargeMethod as MovementChargeMethod;

/// Options for [`Mpp::charge_with_options()`].
#[derive(Debug, Default)]
pub struct ChargeOptions<'a> {
    /// Human-readable description.
    pub description: Option<&'a str>,
    /// Merchant reference ID.
    pub external_id: Option<&'a str>,
    /// Custom expiration (ISO 8601). Default: now + 5 minutes.
    pub expires: Option<&'a str>,
    /// Enable fee sponsorship.
    pub fee_payer: bool,
}

// ==================== Movement Simple API ====================

/// Configuration for the Movement payment method.
///
/// Only `recipient` is required. Everything else has smart defaults.
///
/// # Example
///
/// ```ignore
/// use mpp::server::{Mpp, movement, MovementConfig};
///
/// let mpp = Mpp::create_movement(movement(MovementConfig {
///     recipient: "0x3e9e...",
/// }))?;
///
/// let challenge = mpp.movement_charge("0.10")?;
/// ```
#[cfg(all(feature = "movement", feature = "server", feature = "client"))]
pub struct MovementConfig<'a> {
    /// Recipient address for payments (32-byte hex Move address).
    pub recipient: &'a str,
}

/// Builder returned by [`movement()`] for configuring a Movement payment method.
#[cfg(all(feature = "movement", feature = "server", feature = "client"))]
pub struct MovementBuilder {
    pub(crate) currency: String,
    pub(crate) recipient: String,
    pub(crate) rest_url: String,
    pub(crate) realm: String,
    pub(crate) secret_key: Option<String>,
    pub(crate) decimals: u32,
    pub(crate) network: Option<crate::protocol::methods::movement::MovementNetwork>,
}

#[cfg(all(feature = "movement", feature = "server", feature = "client"))]
impl MovementBuilder {
    /// Override the REST API URL.
    ///
    /// Also auto-detects the network from the URL if not explicitly set:
    /// - URLs containing "testnet" -> `MovementNetwork::Testnet`
    /// - Otherwise -> `MovementNetwork::Mainnet`
    pub fn rest_url(mut self, url: &str) -> Self {
        self.rest_url = url.to_string();
        if self.network.is_none() {
            self.network = Some(network_from_rest_url(url));
        }
        self
    }

    /// Explicitly set the network.
    pub fn network(mut self, network: crate::protocol::methods::movement::MovementNetwork) -> Self {
        self.network = Some(network);
        self
    }

    /// Override the token currency (default: MOVE token `0xa`).
    pub fn currency(mut self, addr: &str) -> Self {
        self.currency = addr.to_string();
        self
    }

    /// Override the realm (default: auto-detected from environment variables).
    pub fn realm(mut self, realm: &str) -> Self {
        self.realm = realm.to_string();
        self
    }

    /// Override the secret key (default: reads `MPP_SECRET_KEY` env var).
    pub fn secret_key(mut self, key: &str) -> Self {
        self.secret_key = Some(key.to_string());
        self
    }

    /// Override the token decimals (default: `8` for MOVE).
    pub fn decimals(mut self, d: u32) -> Self {
        self.decimals = d;
        self
    }
}

/// Create a Movement payment method configuration with smart defaults.
///
/// Returns a [`MovementBuilder`] that can be passed to [`Mpp::create_movement()`].
///
/// # Defaults
///
/// - **rest_url**: `https://mainnet.movementnetwork.xyz/v1`
/// - **currency**: MOVE token (`0xa`)
/// - **decimals**: `8` (for MOVE token)
/// - **realm**: auto-detected from environment variables
/// - **secret_key**: reads `MPP_SECRET_KEY` env var
///
/// # Example
///
/// ```ignore
/// use mpp::server::{Mpp, movement, MovementConfig};
///
/// let mpp = Mpp::create_movement(movement(MovementConfig {
///     recipient: "0x3e9e...",
/// }))?;
/// ```
#[cfg(all(feature = "movement", feature = "server", feature = "client"))]
pub fn movement(config: MovementConfig<'_>) -> MovementBuilder {
    MovementBuilder {
        currency: crate::protocol::methods::movement::MOVE_TOKEN_METADATA.to_string(),
        recipient: config.recipient.to_string(),
        rest_url: crate::protocol::methods::movement::DEFAULT_REST_URL_MAINNET.to_string(),
        realm: detect_realm(),
        secret_key: None,
        decimals: 8,
        network: None,
    }
}

/// Options for [`Mpp::movement_session_challenge()`].
#[cfg(all(feature = "movement", feature = "server", feature = "client"))]
#[derive(Debug, Default)]
pub struct MovementSessionOptions<'a> {
    /// Unit type label (e.g., "token", "byte", "request").
    pub unit_type: Option<&'a str>,
    /// Suggested deposit amount in base units.
    pub suggested_deposit: Option<&'a str>,
    /// Module address (defaults to `DEFAULT_MODULE_ADDRESS`).
    pub module_address: Option<&'a str>,
    /// Registry address (defaults to module address).
    pub registry_address: Option<&'a str>,
    /// Minimum voucher delta in base units.
    pub min_voucher_delta: Option<&'a str>,
    /// Human-readable description.
    pub description: Option<&'a str>,
    /// Custom expiration (ISO 8601). Default: now + 5 minutes.
    pub expires: Option<&'a str>,
}

/// Detect the server realm from environment variables.
///
/// Checks platform-specific env vars in order, falling back to `"MPP Payment"`.
#[cfg(all(feature = "movement", feature = "server", feature = "client"))]
fn detect_realm() -> String {
    const REALM_ENV_VARS: &[&str] = &[
        "MPP_REALM",
        "FLY_APP_NAME",
        "HEROKU_APP_NAME",
        "HOST",
        "HOSTNAME",
        "RAILWAY_PUBLIC_DOMAIN",
        "RENDER_EXTERNAL_HOSTNAME",
        "VERCEL_URL",
        "WEBSITE_HOSTNAME",
    ];

    for name in REALM_ENV_VARS {
        if let Ok(value) = std::env::var(name) {
            if !value.is_empty() {
                return value;
            }
        }
    }
    "MPP Payment".to_string()
}

/// Derive a Movement network from a REST API URL.
#[cfg(all(feature = "movement", feature = "server", feature = "client"))]
fn network_from_rest_url(url: &str) -> crate::protocol::methods::movement::MovementNetwork {
    if url.contains("testnet") {
        crate::protocol::methods::movement::MovementNetwork::Testnet
    } else {
        crate::protocol::methods::movement::MovementNetwork::Mainnet
    }
}

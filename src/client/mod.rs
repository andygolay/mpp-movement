//! Client-side payment providers.
//!
//! This module provides the client-side API for creating payment credentials.
//!
//! # Exports
//!
//! - [`PaymentProvider`]: Trait for payment providers
//! - [`Fetch`]: Extension trait for reqwest with `.send_with_payment()` method
//! - [`MovementProvider`]: Movement charge provider
//! - [`MovementSessionProvider`]: Movement session provider (auto-manages channels)
//!
//! # Example
//!
//! ```ignore
//! use mpp::client::{Fetch, MovementProvider};
//!
//! let provider = MovementProvider::new(signing_key, "https://testnet.movementnetwork.xyz/v1")?;
//! let resp = client.get(url).send_with_payment(&provider).await?;
//! ```

mod error;
mod provider;

#[cfg(feature = "movement")]
pub mod movement;

#[cfg(feature = "client")]
mod fetch;

#[cfg(feature = "middleware")]
mod middleware;

pub use error::HttpError;
pub use provider::{MultiProvider, PaymentProvider};

#[cfg(feature = "client")]
pub use fetch::PaymentExt as Fetch;

#[cfg(feature = "middleware")]
pub use middleware::PaymentMiddleware;

// Re-export Movement types at client level
#[cfg(feature = "movement")]
pub use movement::{MovementProvider, MovementSessionProvider};

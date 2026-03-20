//! Payment method implementations for Web Payment Auth.
//!
//! This module provides method-specific types and helpers.
//!
//! # Available Methods
//!
//! - [`movement`]: Movement Network (requires `movement` feature)
//!
//! # Architecture
//!
//! ```text
//! methods/
//! └── movement/   # Movement-specific (Move, ed25519, BCS, FA)
//!     ├── types.rs       # MovementMethodDetails
//!     ├── network.rs     # MovementNetwork
//!     ├── voucher.rs     # ed25519 voucher signing/verification
//!     ├── method.rs      # ChargeMethod (server-side verification)
//!     ├── session_method.rs  # SessionMethod (channel lifecycle)
//!     └── rest_client.rs # Movement REST API client
//! ```

#[cfg(feature = "movement")]
pub mod movement;

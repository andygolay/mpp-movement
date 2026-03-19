//! Payment method implementations for Web Payment Auth.
//!
//! This module provides method-specific types and helpers.
//!
//! # Available Methods
//!
//! - [`tempo`]: Tempo blockchain (requires `tempo` feature)
//! - [`movement`]: Movement Network (requires `movement` feature)
//!
//! # Architecture
//!
//! ```text
//! methods/
//! ├── tempo/      # Tempo-specific (EVM, EIP-712, ECDSA, TIP-20)
//! │   ├── types.rs    # TempoMethodDetails
//! │   └── charge.rs   # TempoChargeExt trait
//! └── movement/   # Movement-specific (Move, ed25519, BCS, FA)
//!     ├── types.rs    # MovementMethodDetails
//!     ├── network.rs  # MovementNetwork
//!     └── voucher.rs  # ed25519 voucher signing/verification
//! ```

#[cfg(feature = "tempo")]
pub mod tempo;

#[cfg(feature = "movement")]
pub mod movement;


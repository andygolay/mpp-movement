# mpp-movement

Rust SDK for the [**Machine Payments Protocol (MPP)**](https://mpp.dev) on [Movement Network](https://movementnetwork.xyz).

Fork of [tempoxyz/mpp-rs](https://github.com/tempoxyz/mpp-rs) with a `movement` payment method that uses ed25519 signatures, BCS serialization, and the [TempoStreamChannel](https://github.com/andygolay/tempo-move) Move contract for streaming payment channels.

## What is MPP?

[MPP](https://mpp.dev) lets any client — agents, apps, or humans — pay for any service in the same HTTP request. It standardizes [HTTP 402 (Payment Required)](https://mpp.dev/protocol/http-402) with an open [IETF specification](https://paymentauth.org), so servers can charge and clients can pay without API keys, billing accounts, or checkout flows.

This SDK adds Movement Network as a payment method alongside the original Tempo (EVM) support.

## Install

```bash
cargo add mpp --git https://github.com/andygolay/mpp-movement --features movement
```

## Quick Start

### Server (Axum)

```rust
use mpp::protocol::methods::movement;

// Create a 402 challenge for 0.001 MOVE
let challenge = movement::charge_challenge(
    "my-server-secret",
    "api.example.com",
    "100000",       // 0.001 MOVE (8 decimals)
    "0xa",          // MOVE token
    "0xrecipient",
)?;

// Verify a payment credential (stateless HMAC check)
let expected_id = mpp::compute_challenge_id(
    "my-server-secret", realm, method, intent, request, expires, digest, opaque,
);
assert_eq!(credential.challenge.id, expected_id);
```

### Client

```rust
use mpp::client::MovementProvider;

let provider = MovementProvider::new(signing_key, "https://testnet.movementnetwork.xyz/v1")?;

// Provider handles 402 challenges automatically:
// 1. Parse WWW-Authenticate header
// 2. Build + sign Movement transaction
// 3. Retry with payment credential
```

### Voucher Signing (Session Payments)

```rust
use mpp::protocol::methods::movement::voucher;

// Sign a voucher (off-chain, matches on-chain TempoStreamChannel contract)
let sig = voucher::sign_voucher(&signing_key, &channel_id, cumulative_amount);

// Verify (server-side)
let valid = voucher::verify_voucher(&channel_id, amount, &sig, &pubkey, &authorized_pubkey);
```

## Feature Flags

| Feature | Description |
|---------|-------------|
| `movement` | Movement Network support (ed25519, BCS, sha3-256) |
| `client` | Client-side payment providers + Movement REST client |
| `server` | Server-side session method, channel store, verification |
| `tempo` | Original Tempo (EVM) blockchain support |
| `evm` | Shared EVM utilities |
| `middleware` | reqwest-middleware with `PaymentMiddleware` |
| `tower` | Tower middleware for server-side integration |
| `axum` | Axum extractor support |

## Movement vs Tempo

| Aspect | Tempo (EVM) | Movement |
|--------|-------------|----------|
| Signature | EIP-712 + ECDSA | ed25519 |
| Serialization | ABI encoding | BCS |
| Hash | keccak256 | sha3-256 |
| Token standard | TIP-20 (ERC-20) | Fungible Asset (FA) |
| Address format | 20-byte | 32-byte |
| Contract | Solidity TempoStreamChannel | [Move TempoStreamChannel](https://github.com/andygolay/tempo-move) |

## Examples

See [`examples/`](./examples/) for runnable demos. All examples run against Movement testnet with real on-chain transactions.

### Fortune Teller (one-time payment via HTTP 402)

```bash
cd examples/movement
cargo run --bin movement-server    # Terminal 1
cargo run --bin movement-client    # Terminal 2
```

### Pay-Per-Token LLM Streaming (session payments with vouchers)

```bash
cd examples/movement
cargo run --bin movement-sse-server    # Terminal 1
cargo run --bin movement-sse-client    # Terminal 2
```

## On-Chain Contract

The Movement payment method settles on the [TempoStreamChannel Move contract](https://github.com/andygolay/tempo-move) deployed on Movement testnet:

```
Module: 0x3e9edf3be513781a6db0706b652da425ad67f58b5cb366847126bf0fb716fc58
Token:  0xa (MOVE)
```

Entry functions: `open`, `settle`, `top_up`, `close`, `request_close`, `withdraw`

## Protocol

Built on the ["Payment" HTTP Authentication Scheme](https://paymentauth.org), an open specification proposed to the IETF. See [mpp.dev/protocol](https://mpp.dev/protocol/) for the protocol overview.

## Contributing

```bash
git clone https://github.com/andygolay/mpp-movement
cd mpp-movement
cargo test --features movement,server,client
```

321 tests, all passing.

## License

Licensed under either of [Apache License, Version 2.0](./LICENSE-APACHE) or [MIT License](./LICENSE-MIT) at your option.

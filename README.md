# mpp-movement

Rust SDK for the [**Machine Payments Protocol (MPP)**](https://mpp.dev) on [Movement Network](https://movementnetwork.xyz).

Lets any client — agents, apps, or humans — pay for any service in the same HTTP request using [HTTP 402 (Payment Required)](https://mpp.dev/protocol/http-402). Supports one-time charges, streaming payments via payment channels, and any Fungible Asset token (MOVE, USDC.e, USDCx, etc.).

Built on the [MovementStreamChannel](https://github.com/andygolay/tempo-move) Move contract for on-chain payment channel settlement.

## Install

```bash
cargo add mpp --git https://github.com/andygolay/mpp-movement --features movement,client,server
```

## Quick Start

### Server

```rust
use mpp::server::{Mpp, movement, MovementConfig};

// Create a payment handler — currency, realm, secret all have smart defaults
let mpp = Mpp::create_movement(movement(MovementConfig {
    recipient: "0x3e9e...",
})
.rest_url("https://testnet.movementnetwork.xyz/v1")
.secret_key("my-secret"))?;

// Generate a charge challenge for $0.10
let challenge = mpp.movement_charge("0.10")?;
```

With the Axum extractor, you can gate any route behind a payment in one line:

```rust
use mpp::server::axum::{MppCharge, ChargeConfig};

struct TenCents;
impl ChargeConfig for TenCents {
    fn amount() -> &'static str { "0.10" }
}

async fn paid_endpoint(charge: MppCharge<TenCents>) -> &'static str {
    "You paid! Here's your content."
}
```

### Client (one-time charge)

```rust
use mpp::client::{Fetch, MovementProvider};

let provider = MovementProvider::new(signing_key, "https://testnet.movementnetwork.xyz/v1")?;

// Automatic 402 handling: GET → 402 → pay on-chain → retry with credential → 200
let resp = client.get(url).send_with_payment(&provider).await?;
```

Works with any FA token — the SDK picks the right transfer function (`aptos_account::transfer` for native MOVE, `primary_fungible_store::transfer` for USDC.e, USDCx, etc.).

### Client (session / streaming)

```rust
use mpp::client::{Fetch, MovementSessionProvider};

let session = MovementSessionProvider::new(signing_key, rest_url)?
    .with_max_deposit(1_000_000);

// First request: opens payment channel on-chain automatically
let resp = client.get(url).send_with_payment(&session).await?;

// Subsequent requests: off-chain vouchers, no gas!
let resp = client.get(url).send_with_payment(&session).await?;
let resp = client.get(url).send_with_payment(&session).await?;

println!("Total spent: {} base units", session.cumulative());
```

### Voucher Signing

```rust
use mpp::protocol::methods::movement::voucher;

// Sign a voucher (off-chain, matches on-chain MovementStreamChannel contract)
let sig = voucher::sign_voucher(&signing_key, &channel_id, cumulative_amount);

// Verify (server-side)
let valid = voucher::verify_voucher(&channel_id, amount, &sig, &pubkey, &authorized_pubkey);
```

## Feature Flags

| Feature | Description |
|---------|-------------|
| `movement` | Movement Network support (ed25519, BCS, sha3-256) |
| `client` | Client-side payment providers + REST client |
| `server` | Server-side session method, channel store, SSE metering |
| `axum` | Axum extractor for per-route payment gating |
| `tower` | Tower middleware for server-side integration |
| `middleware` | reqwest-middleware with `PaymentMiddleware` |
| `observability` | Structured logging via `tracing` (zero-cost when disabled) |
| `utils` | Hex/rand utilities |

## Examples

All examples run against Movement testnet with real on-chain transactions. See [`examples/`](./examples/).

### Fortune Teller (one-time charge)

Server uses `Mpp::create_movement()` + Axum `MppCharge` extractor. Client uses `MovementProvider` + `send_with_payment()`.

```bash
cd examples/rust
cargo run --bin movement-payment-server    # Terminal 1
cargo run --bin movement-payment-client    # Terminal 2
```

### Multi-Fetch Scraper (session payments)

Server verifies vouchers via `SessionMethod`. Client uses `MovementSessionProvider` — opens a channel on the first request, then sends off-chain vouchers for subsequent requests.

```bash
cd examples/rust
cargo run --bin movement-multifetch-server    # Terminal 1
cargo run --bin movement-multifetch-client    # Terminal 2
```

### Pay-Per-Token LLM Streaming (SSE + payment channels)

Server streams tokens via Server-Sent Events, settling vouchers on-chain periodically. Client opens a channel and pays per token batch.

```bash
cd examples/rust
cargo run --bin movement-sse-server    # Terminal 1
cargo run --bin movement-sse-client    # Terminal 2
```

### Streaming Demo (React)

A browser-based demo that connects to the SSE server. Users connect a Movement wallet, open a payment channel, and stream AI text paid with off-chain vouchers.

```bash
cargo run --manifest-path examples/rust/Cargo.toml --bin movement-sse-server  # Terminal 1
cd examples/token-stream && pnpm install && pnpm dev                            # Terminal 2
```

Open http://localhost:5173. Supports MOVE, USDCx, and USDC.e — configure via `.env` files (see [`examples/rust/.env.example`](./examples/rust/.env.example)).

### Voice Call (React + Rust)

Pay-per-second voice calls using WebRTC + MPP payment channels. Trustless design — vouchers flow peer-to-peer over a WebRTC data channel, the server never touches funds. See [`examples/voice-call/`](./examples/voice-call/).

```bash
cd examples/voice-call/server && cargo run                    # Terminal 1
cd examples/voice-call/client && pnpm install && pnpm dev     # Terminal 2
```

## On-Chain Contract

Settles on the [MovementStreamChannel Move contract](https://github.com/andygolay/tempo-move) deployed on Movement testnet:

```
Module: 0x74f1060add0c641a0c10bb5bab2bf5fd05f94d7c25055f2419fa82d7bbf2b1e8
```

Entry functions: `open`, `settle`, `top_up`, `close`, `request_close`, `withdraw`

Supports any Fungible Asset token. Default: MOVE (`0xa`).

## Architecture

```
src/
├── protocol/          # Core HTTP 402 protocol (headers, types, traits)
│   ├── core/          # Challenge, credential, receipt parsing/formatting
│   ├── intents/       # ChargeRequest, SessionRequest schemas
│   ├── traits/        # ChargeMethod, SessionMethod traits
│   └── methods/
│       └── movement/  # Movement-specific implementation
│           ├── charge.rs         # ChargeRequest extension trait
│           ├── voucher.rs        # ed25519 voucher signing/verification
│           ├── method.rs         # Server-side charge verification
│           ├── session_method.rs # Server-side session/channel lifecycle
│           ├── rest_client.rs    # Movement REST API client
│           ├── network.rs        # Network configuration
│           └── types.rs          # MovementMethodDetails
├── client/
│   └── movement/
│       ├── mod.rs     # MovementProvider (charge payments)
│       └── session.rs # MovementSessionProvider (auto-managed channels)
├── server/
│   ├── mpp.rs         # Mpp handler (challenge generation + verification)
│   ├── axum.rs        # Axum MppCharge extractor
│   ├── sse.rs         # Server-Sent Events metering
│   └── middleware.rs  # Tower middleware
└── store.rs           # Channel state persistence
```

## Protocol

Built on the ["Payment" HTTP Authentication Scheme](https://paymentauth.org), an open specification proposed to the IETF. See [mpp.dev/protocol](https://mpp.dev/protocol/) for the protocol overview.

## Contributing

```bash
git clone https://github.com/andygolay/mpp-movement
cd mpp-movement
cargo test --features movement,server,client,axum
```

## License

Licensed under either of [Apache License, Version 2.0](./LICENSE-APACHE) or [MIT License](./LICENSE-MIT) at your option.

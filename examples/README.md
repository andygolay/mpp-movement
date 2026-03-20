# Examples

All examples run against Movement testnet with real on-chain transactions.

## Fortune Teller API (one-time charge)

A payment-gated API that charges 0.001 MOVE per fortune using HTTP 402. Server uses `Mpp::create_movement()` with the Axum `MppCharge` extractor. Client uses `MovementProvider` with `send_with_payment()` for automatic 402 handling.

```bash
cd examples/movement

# Terminal 1:
cargo run --bin movement-payment-server

# Terminal 2:
cargo run --bin movement-payment-client
```

## Multi-Fetch Scraper (session payments)

A payment-channel-gated scraping API. Each page costs 0.001 MOVE. Client uses `MovementSessionProvider` which auto-manages the channel — first request opens on-chain, subsequent requests send off-chain vouchers.

```bash
cd examples/movement

# Terminal 1:
cargo run --bin movement-multifetch-server

# Terminal 2:
cargo run --bin movement-multifetch-client
```

```
=== Summary ===
  Pages scraped:      9
  Total paid:         900000 base units (0.0090 MOVE)
  Client on-chain tx: 1 (open, automatic)
  Off-chain vouchers: 8 (automatic)
```

## Pay-Per-Token LLM Streaming (SSE + payment channels)

A simulated LLM API that streams tokens via Server-Sent Events, charged per token using payment channel vouchers. The server settles vouchers on-chain every 5 requests, then closes the channel when the client disconnects.

```bash
cd examples/movement

# Terminal 1:
cargo run --bin movement-sse-server

# Terminal 2:
cargo run --bin movement-sse-client

# Or with a custom prompt:
cargo run --bin movement-sse-client -- "What is the meaning of life?"
```

```
=== Summary ===
   Tokens streamed:    81
   Vouchers sent:      10
   Total paid:         0.001 MOVE
   Client on-chain:    1 tx (open)
   Server on-chain:    3 txns (2 settles + 1 close)
   Off-chain vouchers: 10
```

## React Demo

A browser-based demo that connects to the SSE server. Users connect a Movement wallet, open a payment channel, and stream AI text paid with off-chain vouchers. Supports MOVE, USDCx, and USDC.e.

```bash
# Terminal 1 — start the SSE server
cargo run --manifest-path examples/movement/Cargo.toml --bin movement-sse-server

# Terminal 2 — start the React frontend
cd examples/movement-demo
pnpm install
pnpm dev
```

Open http://localhost:5173. See [`movement-demo/README.md`](./movement-demo/README.md) for token configuration.

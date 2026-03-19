# Examples

## Fortune Teller API (one-time payment)

A payment-gated API that charges 0.001 MOVE per fortune using HTTP 402.

```bash
cd examples/movement

# Terminal 1:
cargo run --bin movement-server

# Terminal 2:
cargo run --bin movement-client
```

The client generates a wallet, funds it from the Movement testnet faucet, submits a real on-chain transfer, and retries with the tx hash as a payment credential.

```
Fetching http://localhost:3000/api/fortune ...

1. Response: 402 Payment Required
2. Payment challenge received:
   method:  movement
   intent:  charge
   amount:  100000 (0.0010 MOVE)
   token:   0xa
   to:      0x3e9edf3be513781a6db0706b652da425ad67f58b5cb366847126bf0fb716fc58

3. Submitting payment on Movement testnet...
   Tx: 0xdb6f8174b09e75af8640ede1c7427ba1bf8140cb9fda28f918f2dc59aca3d192
   https://explorer.movementnetwork.xyz/txn/0xdb6f8174b09e75af8640ede1c7427ba1bf8140cb9fda28f918f2dc59aca3d192?network=testnet

4. Retrying with payment credential...
   Response: 200 OK

5. Payment receipt:
   status:    success
   method:    movement
   reference: 0xdb6f8174b09e75af8640ede1c7427ba1bf8140cb9fda28f918f2dc59aca3d192

   Fortune: A new perspective will come with the new year.
```   

## Pay-Per-Token LLM Streaming (session payments)

A simulated LLM API that streams tokens via SSE, charged per token using payment channel vouchers. The server settles vouchers on-chain every 5 requests.

```bash
cd examples/movement

# Terminal 1:
cargo run --bin movement-sse-server

# Terminal 2:
cargo run --bin movement-sse-client

# Or with a custom prompt:
cargo run --bin movement-sse-client -- "What is the meaning of life?"
```

The client opens a real payment channel on Movement testnet, then sends ed25519-signed vouchers to pay for each batch of tokens as they stream. The server settles on-chain periodically.

```
=== Summary ===
   Tokens streamed:    81
   Vouchers sent:      10
   Total paid:         0.001 MOVE
   Deposit:            0.001 MOVE
   Refund to payer:    0 MOVE
   Client on-chain:    1 tx (open)
   Server on-chain:    3 txns (2 settles + 1 close)
   Off-chain vouchers: 10
   Channel: 0xeb8e7e4e054e02e37c1b5288c99c979fb2329efed502f0a74ad66fafeb79af21
```

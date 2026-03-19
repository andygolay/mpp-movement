# Movement Stream Demo

React frontend for the [MPP pay-per-token SSE server](../movement/src/sse_server.rs). Connect a wallet, open a payment channel, and stream AI text paid with USDCx (or MOVE) vouchers on Movement testnet.

## How it works

1. User connects a Movement-compatible wallet (Nightly, Razor, etc.)
2. Clicks **Start Streaming** — frontend requests `/api/chat`, gets HTTP 402
3. Frontend opens a TempoStreamChannel on-chain via wallet (user approves one tx)
4. An ephemeral ed25519 session key signs vouchers — no wallet popup per token
5. Tokens stream in via SSE; the server settles vouchers on-chain every 5 batches
6. User clicks **Stop** — server closes the channel, refunds unused deposit

## Setup

```bash
# 1. Start the SSE server (from repo root)
cargo run --bin movement-sse-server

# 2. In another terminal, start the frontend
cd examples/movement-demo
pnpm install
pnpm dev
```

Open http://localhost:5173 and connect your wallet.

## Configuration

Copy `.env.example` to `.env` and adjust as needed. Defaults to USDCx on Movement testnet.

To use native MOVE instead (works with the testnet faucet):
```
VITE_TOKEN_METADATA_ADDR=0xa
VITE_TOKEN_SYMBOL=MOVE
VITE_TOKEN_DECIMALS=8
```

## Notes

- The server generates its own wallet and funds it from the testnet faucet (for gas to settle/close)
- The server's `PRICE_PER_TOKEN` and `SUGGESTED_DEPOSIT` are in MOVE base units (8 decimals) by default — adjust for USDCx (6 decimals) if needed
- Your wallet needs USDCx (or MOVE) on Movement testnet to open a channel

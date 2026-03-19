# Movement Stream Demo

React frontend for the [MPP pay-per-token SSE server](../movement/src/sse_server.rs). Connect a wallet, open a payment channel, and stream AI text paid with USDCx, USDC.e, or MOVE vouchers on Movement testnet.

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
cargo run --manifest-path examples/movement/Cargo.toml --bin movement-sse-server

# 2. In another terminal, start the frontend
cd examples/movement-demo
pnpm install
pnpm dev
```

Open http://localhost:5173 and connect your wallet.

## Server environment variables

Set these in `examples/movement/.env` (loaded automatically via dotenvy):

| Variable | Default | Description |
|----------|---------|-------------|
| `TOKEN_METADATA` | `0xa` (MOVE) | FA metadata address for the payment token |
| `PRICE_PER_TOKEN` | `1000` | Price per streamed token in base units |
| `SUGGESTED_DEPOSIT` | `100000` | Deposit amount suggested to the client |
| `MPP_SECRET_KEY` | `sse-example-secret` | HMAC secret for 402 challenge IDs |

### USDCx config (6 decimals)

```
TOKEN_METADATA=0x63f169ba69623ba6ccf34620857644feb46d0f87e1d7bbcf8c071d30c3d94bd6
PRICE_PER_TOKEN=10
SUGGESTED_DEPOSIT=10000
```

This gives 0.0001 USDCx per voucher (10 tokens × 10 base units each), with a 0.01 USDCx deposit.

### USDC.e config (6 decimals)

```
TOKEN_METADATA=0xc6f5b46ab5307dfe3e565668edcc1461b31cac5a6c2739fba17d9fdde16813a2
PRICE_PER_TOKEN=10
SUGGESTED_DEPOSIT=10000
```

Same pricing as USDCx. Ready-made env files: `cp ../.env.usdc-e ../movement/.env && cp .env.usdc-e .env`

## Frontend environment variables

Copy `.env.example` to `.env` and adjust as needed. The frontend defaults to USDCx.

| Variable | Default | Description |
|----------|---------|-------------|
| `VITE_SERVER_URL` | `http://localhost:3001` | SSE server URL |
| `VITE_MODULE_ADDRESS` | `0x3e9edf...` | TempoStreamChannel module address |
| `VITE_REGISTRY_ADDR` | same as module | Channel registry address |
| `VITE_TOKEN_METADATA_ADDR` | `0x63f169...` (USDCx) | FA metadata address |
| `VITE_TOKEN_SYMBOL` | `USDCx` | Display symbol |
| `VITE_TOKEN_DECIMALS` | `6` | Token decimal places |

### To use native MOVE instead

```
VITE_TOKEN_METADATA_ADDR=0xa
VITE_TOKEN_SYMBOL=MOVE
VITE_TOKEN_DECIMALS=8
```

And in the server `.env`:

```
TOKEN_METADATA=0xa
PRICE_PER_TOKEN=1000
SUGGESTED_DEPOSIT=100000
```

## Notes

- The server generates its own ephemeral wallet and funds it from the testnet faucet (for gas to settle/close)
- Your wallet needs USDCx, USDC.e, or MOVE on Movement testnet to open a channel
- Server and frontend token configs must match — both must use the same token

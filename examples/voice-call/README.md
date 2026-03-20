# Voice Call Example

Pay-per-second voice calls using WebRTC + MPP payment channels on Movement.

A host goes live with a per-second rate. Callers open a payment channel via HTTP 402, connect via WebRTC audio, and send vouchers directly to the host over a WebRTC data channel. The host verifies vouchers locally and settles on-chain when the call ends.

## Architecture

```
┌──────────────┐  WebRTC audio + data  ┌──────────────┐
│  Caller      │◄─────────────────────▶│  Host         │
│  (browser)   │  vouchers (p2p)       │  (browser)    │
└──────┬───────┘                       └───────┬───────┘
       │ 402 + signaling                       │ signaling
       ▼                                       ▼
┌──────────────────────────────────────────────────────┐
│  Rust axum server (port 3002)                        │
│  - HTTP 402 challenges (channel opening)             │
│  - WebSocket signaling relay (WebRTC setup)          │
│  - Host/call state management                        │
│  - Never touches funds or vouchers                   │
└──────────────────────────────────────────────────────┘
```

**Trustless design:** The server only handles signaling and the initial 402 challenge. Vouchers flow peer-to-peer from caller to host over a WebRTC data channel. The host verifies each voucher signature locally using `@mpp/client`. Settlement happens on-chain — only the host (payee) can call `channel::close`.

## Prerequisites

- Rust toolchain
- Node.js 18+ and pnpm
- A Movement wallet (e.g., Razor, Nightly) with testnet MOVE

## Running

### 1. Start the server

```bash
cd server
cargo run
```

The server starts on `http://localhost:3002`. Optional environment variables:

| Variable | Default | Description |
|---|---|---|
| `SECRET_KEY` | `voice-call-demo-secret` | MPP server secret for challenge HMAC |
| `MODULE_ADDRESS` | `0x74f1060...b1e8` | MovementStreamChannel module address |
| `REST_URL` | `https://testnet.movementnetwork.xyz/v1` | Movement REST API |

### 2. Start the client

```bash
cd client
pnpm install
pnpm dev
```

Opens at `http://localhost:5173`.

### 3. Use it

**As the host:**
1. Open the app and connect your wallet
2. Switch to **Host** mode
3. Enter your display name and rate (in MOVE per second)
4. Click **Go Live**

**As a caller (open a second browser/tab with a different wallet):**
1. Open the app and connect a different wallet
2. Stay in **Caller** mode
3. You'll see the host listed with their rate
4. Click **Call** — your wallet will prompt to open a payment channel
5. Once the channel is open and WebRTC connects, you're in a live voice call
6. Vouchers are sent every 5 seconds directly to the host (peer-to-peer)
7. Click **Hang Up** when done — the host's wallet will prompt to close the channel on-chain

## Configuration

The client reads environment variables via Vite:

| Variable | Default | Description |
|---|---|---|
| `VITE_SERVER_URL` | `http://localhost:3002` | Server URL |
| `VITE_MODULE_ADDRESS` | `0x74f1060...b1e8` | MovementStreamChannel module |
| `VITE_TOKEN_METADATA_ADDR` | `0xa` | Token (native MOVE) |
| `VITE_TOKEN_SYMBOL` | `MOVE` | Display symbol |
| `VITE_TOKEN_DECIMALS` | `8` | Token decimals |

## How Payment Works

1. Caller hits `POST /api/call/start` → server returns **HTTP 402** with an MPP session challenge (host's address is the payee)
2. Caller's browser parses the challenge using `@mpp/client`
3. `MovementSessionProvider` opens a payment channel on-chain with the host as payee (user approves 1 wallet tx)
4. Caller retries with `Authorization: Payment <credential>` → server returns `callId`
5. Both parties connect via WebSocket for WebRTC signaling (SDP offer/answer + ICE candidates)
6. WebRTC establishes peer-to-peer audio + a `"vouchers"` data channel
7. Every 5 seconds, the caller signs an ed25519 voucher and sends it to the host over the data channel (off-chain, no gas, no server involvement)
8. The host verifies each voucher signature locally using `verifyVoucher` from `@mpp/client` and tracks the highest
9. On hangup, the host calls `channel::close` on-chain with the highest voucher — settling the earned MOVE to the host and refunding unused deposit to the caller

## Security

- **Caller can't avoid paying** — deposit is locked in the on-chain contract
- **Host can't overcharge** — only gets what the caller signed in vouchers
- **Server can't steal or withhold** — never sees vouchers, only does signaling
- **Host is incentivized to close** — that's how they get paid
- **Caller can stop anytime** — just stops sending vouchers and hangs up
- **Fallback** — if the host never closes, the caller can `request_close` on-chain after a 15-minute grace period

## Stack

- **Server:** Rust + axum + `mpp` crate (402 challenges) + WebSocket (signaling)
- **Client:** React + `@mpp/client` (payment flow + voucher verification) + `@moveindustries/wallet-adapter-react` (wallet) + WebRTC (audio + data channel)

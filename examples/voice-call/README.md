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
│  - Rate limiting + WebSocket auth                    │
│  - Never touches funds or vouchers                   │
└──────────────────────────────────────────────────────┘
```

**Trustless design:** The server only handles signaling and the initial 402 challenge. Vouchers flow peer-to-peer from caller to host over a WebRTC data channel. The host verifies each voucher signature locally using `@mpp/client`. Settlement happens on-chain — only the host (payee) can call `channel::close`.

### Call Flow

```
  Caller                          Server (:3002)                        Host
    │                                  │                                  │
    │  POST /api/host/go-live          │◄─────────────────────────────────┤
    │                                  │  { address, ratePerSecond, ... } │
    │                                  │─────────────────────────────────▶│
    │                                  │  200 OK                          │
    │                                  │                                  │
    │  GET /api/hosts                  │                                  │
    │─────────────────────────────────▶│                                  │
    │◄─────────────────────────────────│                                  │
    │  [ { address, rate, ... } ]      │                                  │
    │                                  │                                  │
    │  POST /api/call/start?host=0x..  │                                  │
    │  (no channel_id)                 │                                  │
    │─────────────────────────────────▶│                                  │
    │◄─────────────────────────────────│                                  │
    │  402 Payment Required            │                                  │
    │  WWW-Authenticate: MPP session.. │                                  │
    │                                  │                                  │
    │  ┌──────────────────────┐        │                                  │
    │  │ Parse 402 challenge  │        │                                  │
    │  │ Open payment channel │        │                                  │
    │  │ on-chain (1 wallet   │        │                                  │
    │  │ tx, host = payee)    │        │                                  │
    │  └──────────────────────┘        │                                  │
    │                                  │                                  │
    │  POST /api/call/start?host=0x..  │                                  │
    │  { channelId: "0x..." }          │                                  │
    │─────────────────────────────────▶│  (marks host busy)               │
    │◄─────────────────────────────────│                                  │
    │  200 { callId, wsUrl,            │                                  │
    │        callerToken, hostToken }  │                                  │
    │                                  │                                  │
    │                                  │  GET /api/host/poll?address=0x.. │
    │                                  │◄─────────────────────────────────┤
    │                                  │─────────────────────────────────▶│
    │                                  │  { callId, callerAddress,        │
    │                                  │    wsToken }                     │
    │                                  │                                  │
    │  GET /ws/signal/{callId}         │       GET /ws/signal/{callId}    │
    │  ?address=...&token=...          │       ?address=...&token=...     │
    │  ══════════ WebSocket ══════════▶│◄══════════ WebSocket ════════════┤
    │                                  │                                  │
    │  ┌───────────────────────────────┼──────────────────────────────┐   │
    │  │  Signaling relay (SDP + ICE)  │  Server relays messages      │   │
    │  │  offer ──────────────────────▶│──────────────────────────▶   │   │
    │  │       ◀──────────────────────│◀────────────────────────── answer│
    │  │  ice  ──────────────────────▶│──────────────────────────▶   │   │
    │  │       ◀──────────────────────│◀────────────────────────── ice │  │
    │  └───────────────────────────────┼──────────────────────────────┘   │
    │                                  │                                  │
    │  ════════════ WebRTC P2P (direct, server not involved) ════════════ │
    │  │  audio ◄──────────────────────────────────────────────▶ audio │  │
    │  │  voucher (every 5s) ──────────────────────────────────▶       │  │
    │  │                                          (verify signature)   │  │
    │                                  │                                  │
    │  ┌──────────────────────┐        │                                  │
    │  │ Running low on time? │        │                                  │
    │  │ Click "+5 min" to    │        │                                  │
    │  │ top up the channel   │        │                                  │
    │  │ on-chain             │        │                                  │
    │  └──────────────────────┘        │                                  │
    │                                  │                                  │
    │  POST /api/call/hangup           │                                  │
    │─────────────────────────────────▶│  (marks host available)          │
    │◄─────────────────────────────────│                                  │
    │  200 { duration }                │                                  │
    │                                  │                                  │
    │                                  │  ┌───────────────────────────┐   │
    │                                  │  │ Host calls channel::close │   │
    │                                  │  │ on-chain with highest     │   │
    │                                  │  │ voucher → settles MOVE    │   │
    │                                  │  └───────────────────────────┘   │
```

## Prerequisites

- Rust toolchain
- Node.js 18+ and pnpm
- Two Movement wallets with testnet MOVE (e.g., Razor, Nightly — you need two different wallets to play both roles)

## Running Locally

### 1. Start the server

```bash
cd server
cp .env.example .env
# Edit .env — set SECRET_KEY (required). For local dev, generate one with:
#   openssl rand -base64 32
cargo run
```

The server starts on `http://localhost:3002`.

### 2. Start the client

```bash
cd client
pnpm install
pnpm dev
```

Opens at `http://localhost:5173`.

### 3. Test with two browser tabs

You can test the full flow on one computer using two tabs. You'll need two different wallet browser extensions (e.g., Razor in Chrome and Nightly in a Chrome profile, or use Chrome + Firefox).

**Tab 1 — Host:**
1. Open `http://localhost:5173` and connect your first wallet
2. Switch to **Host** mode
3. Enter a display name and rate (e.g., `0.001` MOVE per second)
4. Click **Go Live** — your wallet will ask you to sign a message (this proves you own the address)

**Tab 2 — Caller:**
1. Open `http://localhost:5173` in a second tab and connect a different wallet
2. Stay in **Caller** mode
3. You'll see the host listed with their rate
4. Click **Call** — your wallet will prompt to open a payment channel (one on-chain transaction)
5. Once the channel is open, WebRTC connects and vouchers start flowing
6. The caller sees **remaining time** — click **+5 min** to add more time if needed
7. Click **Hang Up** when done — the host's tab will prompt to close the channel on-chain, settling payment

### 4. What to check during testing

- **Payment flow**: The caller's wallet prompts to open a channel, the host's wallet prompts to close it on hangup
- **Vouchers**: The host panel shows increasing earnings every 5 seconds
- **Remaining time**: The caller sees a countdown; it turns yellow at 60s and red at 30s
- **Add time**: The "+5 min" button tops up the payment channel on-chain mid-call
- **Signaling**: Console logs show `[caller] sent offer`, `[host] received offer`, `[host] sending answer`

## Configuration

### Server environment variables

See `server/.env.example` for the full list.

| Variable | Required | Default | Description |
|---|---|---|---|
| `SECRET_KEY` | **Yes** | _(none)_ | HMAC secret for payment challenges and WebSocket auth. Generate with `openssl rand -base64 32` |
| `PORT` | No | `3002` | HTTP listen port |
| `MODULE_ADDRESS` | No | `0x74f1060...b1e8` | MovementStreamChannel module address |
| `REST_URL` | No | `https://testnet.movementnetwork.xyz/v1` | Movement REST API |
| `ALLOWED_ORIGINS` | No | _(all origins)_ | Comma-separated CORS origins (e.g. `https://your-app.vercel.app`) |
| `RUST_LOG` | No | `info` | Log level filter (e.g. `debug`, `warn`, `voice_call_server=debug`) |
| `LOG_FORMAT` | No | _(human-readable)_ | Set to `json` for structured JSON logs |

### Client environment variables

See `client/.env.example` for the full list.

| Variable | Default | Description |
|---|---|---|
| `VITE_SERVER_URL` | `http://localhost:3002` | Server URL |
| `VITE_MODULE_ADDRESS` | `0x74f1060...b1e8` | MovementStreamChannel module |
| `VITE_TOKEN_METADATA_ADDR` | `0xa` | Token (native MOVE) |
| `VITE_TOKEN_SYMBOL` | `MOVE` | Display symbol |
| `VITE_TOKEN_DECIMALS` | `8` | Token decimals |
| `VITE_TURN_URL` | _(none)_ | TURN server URL (e.g. `turn:your-server.com:3478`) |
| `VITE_TURN_USERNAME` | _(none)_ | TURN server username |
| `VITE_TURN_CREDENTIAL` | _(none)_ | TURN server password |

## Production Deployment

See [DEPLOY.md](./DEPLOY.md) for a step-by-step guide.

### Production checklist

- [ ] Set a strong `SECRET_KEY` (generate with `openssl rand -base64 32`)
- [ ] Set `ALLOWED_ORIGINS` to your client domain(s)
- [ ] Set `LOG_FORMAT=json` and `RUST_LOG=info` for production logging
- [ ] Point `MODULE_ADDRESS` and `REST_URL` at mainnet (if deploying to mainnet)
- [ ] Deploy the Rust server with TLS (Caddy reverse proxy recommended)
- [ ] Set `VITE_SERVER_URL` to your public server URL
- [ ] Configure a TURN server for cross-network calls
- [ ] Serve the client over HTTPS (required for `getUserMedia` mic access)
- [ ] Set up monitoring on `GET /health` (returns `{"status":"ok"}`)

## Operations

### Health check

```
GET /health → {"status":"ok"}
```

Use this for load balancer health checks, uptime monitoring, and alerting.

### Logging

The server uses structured logging via the `tracing` crate.

- **Human-readable** (default): good for development and manual debugging
- **JSON** (`LOG_FORMAT=json`): good for production log aggregation (ELK, CloudWatch, Datadog, etc.)

Control verbosity with `RUST_LOG`:
```bash
RUST_LOG=info          # default — call lifecycle events
RUST_LOG=debug         # verbose — includes all request details
RUST_LOG=warn          # quiet — only warnings and errors
```

### Graceful shutdown

The server handles `SIGTERM` and `SIGINT` (Ctrl+C) gracefully:
- In-flight HTTP requests complete before the process exits
- All active calls are cleaned up
- All hosts are marked offline

This means `systemctl restart voice-call-server` won't leave ghost state.

### Background cleanup

A background task runs every 60 seconds to:
- Remove calls older than 5 minutes with no active WebSocket connections
- Mark hosts as offline if they haven't polled in 2 minutes
- Clean up expired rate limit entries

### Rate limiting

All API endpoints are rate-limited to **60 requests per IP per minute**. Exceeding the limit returns `429 Too Many Requests`.

### WebSocket authentication

WebSocket connections require an HMAC token. Tokens are generated by the server and returned in the `start_call` response (for the caller) and `host_poll` response (for the host). This prevents third parties from connecting to a call's signaling channel.

## How Payment Works

1. Caller hits `POST /api/call/start` → server returns **HTTP 402** with an MPP session challenge (host's address is the payee)
2. Caller's browser parses the challenge using `@mpp/client`
3. `MovementSessionProvider` opens a payment channel on-chain with the host as payee (user approves 1 wallet tx)
4. Caller retries with `Authorization: Payment <credential>` → server returns `callId` + WebSocket auth tokens
5. Both parties connect via authenticated WebSocket for WebRTC signaling (SDP offer/answer + ICE candidates)
6. WebRTC establishes peer-to-peer audio + a `"vouchers"` data channel
7. Every 5 seconds, the caller signs an ed25519 voucher and sends it to the host over the data channel (off-chain, no gas, no server involvement)
8. The host verifies each voucher signature locally using `verifyVoucher` from `@mpp/client` and tracks the highest
9. If the caller is running low on time, they can top up the payment channel on-chain ("+5 min" button)
10. On hangup, the host calls `channel::close` on-chain with the highest voucher — settling the earned MOVE to the host and refunding unused deposit to the caller

## Security

- **Caller can't avoid paying** — deposit is locked in the on-chain contract
- **Host can't overcharge** — only gets what the caller signed in vouchers
- **Server can't steal or withhold** — never sees vouchers, only does signaling
- **Host is incentivized to close** — that's how they get paid
- **Caller can stop anytime** — just stops sending vouchers and hangs up
- **Fallback** — if the host never closes, the caller can `request_close` on-chain after a 15-minute grace period
- **WebSocket auth** — HMAC tokens prevent third parties from intercepting or injecting signaling messages
- **Rate limiting** — per-IP rate limits prevent spam and DoS
- **Input validation** — all API inputs are validated (address format, rate, name length)
- **No sensitive data in logs** — addresses, keys, and signatures are never logged

## Stack

- **Server:** Rust + axum + `mpp` crate (402 challenges) + WebSocket (signaling) + tracing (structured logging)
- **Client:** React + `@mpp/client` (payment flow + voucher verification + channel top-up) + `@moveindustries/wallet-adapter-react` (wallet) + WebRTC (audio + data channel)

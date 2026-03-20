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
    │  200 { callId, wsUrl }           │                                  │
    │                                  │                                  │
    │                                  │  GET /api/host/poll?address=0x.. │
    │                                  │◄─────────────────────────────────┤
    │                                  │─────────────────────────────────▶│
    │                                  │  { callId, callerAddress }       │
    │                                  │                                  │
    │  GET /ws/signal/{callId}         │       GET /ws/signal/{callId}    │
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
6. Click **Hang Up** when done — the host's tab will prompt to close the channel on-chain, settling payment

### 4. Verify audio is working

On the same computer, you won't actually hear audio between tabs — the browser's echo cancellation suppresses the loopback since both tabs share the same mic and speakers. This is normal.

To confirm audio is actually flowing, open the browser console (F12) on either tab during a call and run:

```js
checkAudio()
```

You should see output like:

```
[audio] bytes sent: 48320, packets: 302
[audio] bytes received: 47800, packets: 299, lost: 0
```

If `bytesReceived` is increasing each time you run it, audio is working — echo cancellation is just muting the playback. To hear actual audio, test with two separate devices on the same network.

### 5. What to check during testing

- **Payment flow**: The caller's wallet prompts to open a channel, the host's wallet prompts to close it on hangup
- **Vouchers**: The host panel shows increasing earnings every 5 seconds
- **Audio**: Run `checkAudio()` in the console to verify bytes are flowing
- **Signaling**: Console logs show `[caller] sent offer`, `[host] received offer`, `[host] sending answer`

## Configuration

The client reads environment variables via Vite:

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

## Deploying for Real Users

For calls to work between users on different networks, you need a **TURN relay server**. STUN alone only handles simple NAT traversal — when peers are behind firewalls or symmetric NATs (most real-world networks), WebRTC needs a TURN server to relay the media.

### TURN server setup

You can use any TURN provider or self-host one. Set the credentials in `client/.env`:

```env
VITE_TURN_URL=turn:your-turn-server.com:3478
VITE_TURN_USERNAME=your-username
VITE_TURN_CREDENTIAL=your-password
```

To self-host, [coturn](https://github.com/coturn/coturn) is the standard open-source TURN server:

```bash
sudo apt install coturn
```

Minimal `/etc/turnserver.conf`:

```
listening-port=3478
tls-listening-port=5349
realm=your-domain.com
user=myuser:mypassword
lt-cred-mech
fingerprint
```

### Production checklist

- [ ] Deploy the Rust server with a public URL and TLS (WebRTC requires HTTPS in production)
- [ ] Set `VITE_SERVER_URL` to your public server URL
- [ ] Configure a TURN server (see above)
- [ ] Set a strong `SECRET_KEY` on the server (not the default)
- [ ] Serve the client over HTTPS (required for `getUserMedia` mic access)

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

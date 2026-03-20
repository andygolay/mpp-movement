# @mpp/client

TypeScript client SDK for the [Machine Payments Protocol (MPP)](https://mpp.dev) on [Movement Network](https://movementnetwork.xyz).

Handles the client side of the HTTP 402 payment flow: parsing challenges, opening payment channels, signing vouchers, and building credentials.

## Install

```bash
pnpm add @mpp/client @noble/curves @noble/hashes
```

For Movement blockchain support (payment channels, on-chain transactions):

```bash
pnpm add @moveindustries/ts-sdk@5.1.6
```

## Quick Start

### One-time charge (programmatic)

```ts
import { parseWwwAuthenticate, chargeCredential, formatAuthorization } from "@mpp/client";

// 1. Make a request, get a 402 back
const resp = await fetch("https://api.example.com/fortune");
// resp.status === 402

// 2. Parse the challenge
const challenge = parseWwwAuthenticate(resp.headers.get("www-authenticate")!);

// 3. Pay on-chain (using your preferred method)
const txHash = await submitPaymentTransaction(challenge);

// 4. Build and send the credential
const credential = chargeCredential(challenge, txHash);
const retry = await fetch("https://api.example.com/fortune", {
  headers: { Authorization: formatAuthorization(credential) },
});
// retry.status === 200
```

### Auto-handling with fetchWithPayment

```ts
import { fetchWithPayment, MovementProvider } from "@mpp/client";

const provider = new MovementProvider(privateKey, {
  restUrl: "https://testnet.movementnetwork.xyz/v1",
});

// Automatically handles 402 -> pay -> retry
const resp = await fetchWithPayment("https://api.example.com/fortune", undefined, provider);
```

### Payment channels (browser wallet)

For streaming payments (pay-per-second, pay-per-token), use `MovementSessionProvider` with a wallet adapter:

```ts
import { MovementSessionProvider, parseWwwAuthenticate, formatAuthorization } from "@mpp/client";

const provider = new MovementSessionProvider(wallet, {
  moduleAddress: "0x74f1060add0c641a0c10bb5bab2bf5fd05f94d7c25055f2419fa82d7bbf2b1e8",
  tokenMetadata: "0xa", // native MOVE
});

// First call: opens a channel on-chain (user approves 1 tx)
const credential = await provider.pay(challenge);

// Subsequent calls: signs off-chain vouchers (instant, no gas)
const credential2 = await provider.pay(challenge2);

// Manual voucher signing for custom flows (e.g., periodic vouchers)
const { channelId, cumulativeAmount, signature } = provider.signVoucherFor(
  recipientAddress,
  currency,
  deltaAmount,
);
```

## API Reference

### Protocol Layer

| Function | Description |
|---|---|
| `parseWwwAuthenticate(header)` | Parse a `WWW-Authenticate: Payment ...` header |
| `parseWwwAuthenticateAll(headers)` | Parse multiple headers, skip non-Payment ones |
| `formatWwwAuthenticate(challenge)` | Format a challenge as a header value |
| `extractPaymentScheme(header)` | Extract `Payment ...` from mixed schemes |
| `decodeRequest(challenge)` | Decode the base64url request field to JSON |
| `parseAuthorization(header)` | Parse an `Authorization: Payment ...` header |
| `formatAuthorization(credential)` | Format a credential as a header value |
| `chargeCredential(challenge, txHash)` | Build a credential for a charge payment |
| `sessionCredential(challenge, payload)` | Build a credential for a session payment |
| `parseReceipt(header)` | Parse a `Payment-Receipt` header |
| `formatReceipt(receipt)` | Format a receipt as a header value |
| `fetchWithPayment(url, init, provider)` | Auto-handling fetch wrapper |

### Movement Layer

| Export | Description |
|---|---|
| `MovementProvider` | One-time charge provider (programmatic) |
| `MovementSessionProvider` | Payment channel provider (browser wallet) |
| `signVoucher(voucher, privateKey)` | Sign a voucher with ed25519 |
| `verifyVoucher(voucher, signature, pubkey)` | Verify a voucher signature |
| `computeChannelId(payer, payee, token, salt, pubkey)` | Compute deterministic channel ID |
| `deriveAddress(publicKey)` | Derive Movement address from ed25519 pubkey |
| `toHex(bytes)` / `hexToBytes(hex)` | Hex encoding utilities |

## Architecture

This SDK implements the **client side** of the MPP protocol. It pairs with a Rust server using the `mpp` crate:

```
┌──────────────────────┐         ┌──────────────────────┐
│  Your App (TS)       │  HTTP   │  Server (Rust)       │
│                      │  402    │                      │
│  @mpp/client handles │◄───────▶│  mpp crate handles   │
│  challenges, pays,   │         │  challenges, verifies │
│  signs vouchers      │         │  payments, receipts   │
└──────────────────────┘         └──────────────────────┘
```

## License

MIT OR Apache-2.0

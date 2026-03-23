# Search → Image → Email

A multi-service MPP demo that chains three paid APIs in one payment:

1. **Exa** — searches the web for your query
2. **fal.ai** — generates an AI image summarizing the results
3. **Resend** — emails the search results + image to an address

User pays 0.05 MOVE once. The server orchestrates all three API calls.

## Local testing

### 1. Get API keys

- **Exa** — https://dashboard.exa.ai/api-keys (free tier available)
- **fal.ai** — https://fal.ai/dashboard/keys (free credits on signup)
- **Resend** — https://resend.com/api-keys (free tier: 100 emails/day)

### 2. Server

```bash
cp .env.example .env
# Fill in EXA_API_KEY, FAL_API_KEY, RESEND_API_KEY, and RECIPIENT

cargo run --bin search-image-email
```

Server runs on http://localhost:3020.

### 3. Frontend

```bash
cd client
pnpm install
pnpm dev
```

Frontend runs on http://localhost:3021.

Open http://localhost:3021, connect a Movement testnet wallet, enter a search query and email, and run it.

**Note:** With Resend's test domain (`onboarding@resend.dev`), emails can only be sent to the email on your Resend account. To send to any address, verify your own domain in Resend.

## Production deployment

```bash
# Required
EXA_API_KEY=exa-...
FAL_API_KEY=fal-...
RESEND_API_KEY=re_...
RECIPIENT=0xYourMainnetWallet
FROM_EMAIL=Your App <noreply@yourdomain.com>

# Required for production
MPP_SECRET_KEY=$(openssl rand -hex 32)
MOVEMENT_REST_URL=https://mainnet.movementnetwork.xyz/v1
CORS_ORIGIN=https://your-app.example.com
BIND_ADDR=0.0.0.0:8080
```

## How it works

```
User pays 0.05 MOVE
        │
        ▼
┌─── Server Pipeline ──────────────────────────┐
│                                               │
│  1. Exa Search ──► list of results            │
│                        │                      │
│  2. fal.ai ───────► AI image from results     │
│                        │                      │
│  3. Resend ───────► email with image + text   │
│                                               │
└───────────────────────────────────────────────┘
        │
        ▼
User gets: search results + image in browser
Recipient gets: formatted email with image + results
```

## Endpoint

| Endpoint | Cost | Description |
|----------|------|-------------|
| `POST /api/run` | 0.05 MOVE | Full pipeline: search + image gen + email |
| `GET /api/health` | Free | Health check |
| `GET /api/services` | Free | Service discovery (JSON) |

### Request body

```json
{
  "query": "things to do in San Francisco in April",
  "email": "you@example.com",
  "num_results": 5
}
```

### Response

```json
{
  "search_results": [
    { "title": "...", "url": "...", "summary": "..." }
  ],
  "image_url": "https://fal.media/...",
  "email_sent_to": "you@example.com",
  "email_id": "re_..."
}
```

## Pricing

| Service | Our cost | User pays |
|---------|----------|-----------|
| Exa search (5 results) | ~$0.001 | |
| fal.ai image (Flux Schnell) | ~$0.003 | |
| Resend email | ~$0.001 | |
| **Total** | **~$0.005** | **0.05 MOVE (~$0.01)** |

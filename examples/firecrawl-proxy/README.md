# Firecrawl MPP Proxy

A proxy that gates [Firecrawl's](https://firecrawl.dev) web scraping API behind MPP payments on Movement Network. Users pay in MOVE per request — no Firecrawl account needed.

## How it works

1. User connects their Movement wallet in the browser
2. Enters a URL and clicks Scrape
3. Server responds with HTTP 402 + payment challenge
4. User's wallet signs a 0.01 MOVE transfer on Movement
5. Frontend retries with the tx hash as proof of payment
6. Server verifies on-chain, forwards to Firecrawl, returns scraped content

## Local testing

### 1. Proxy server

```bash
cp .env.example .env
```

Edit `.env` — you need at minimum:
- `FIRECRAWL_API_KEY` — get a free key at https://firecrawl.dev (500 free credits)
- `RECIPIENT` — your Movement testnet wallet address

Then:

```bash
cargo run --bin firecrawl-proxy
```

Server runs on http://localhost:3010. It defaults to Movement testnet with permissive CORS and auto-generated MPP secret key.

### 2. Frontend

```bash
cd client
pnpm install
pnpm dev
```

Frontend runs on http://localhost:3011 and proxies `/api` requests to `:3010`.

Open http://localhost:3011, connect a Movement testnet wallet (you need testnet MOVE from the [faucet](https://faucet.testnet.movementnetwork.xyz)), and scrape a URL.

## Production deployment

Set these environment variables for production:

```bash
# Required
FIRECRAWL_API_KEY=fc-your-production-key
RECIPIENT=0xYourMainnetWalletAddress

# Required for production
MPP_SECRET_KEY=$(openssl rand -hex 32)     # Persistent across restarts
MOVEMENT_REST_URL=https://mainnet.movementnetwork.xyz/v1
CORS_ORIGIN=https://your-app.example.com   # Restrict CORS to your frontend
BIND_ADDR=0.0.0.0:8080                     # Or wherever your reverse proxy points

# Optional
RATE_LIMIT_PER_MINUTE=30                   # Per IP, default 30
```

Build the frontend for production:

```bash
cd client
pnpm build
```

Then serve the `client/dist/` output behind nginx/caddy alongside the Rust proxy, or point your frontend hosting at it.

### What's hardened

- **Rate limiting** — per-IP, configurable requests per minute
- **URL validation** — blocks private/internal IPs (SSRF protection)
- **Request body limit** — 64KB max to prevent abuse
- **Crawl limit cap** — max 20 pages per crawl request
- **CORS** — permissive in dev, restrictive when `CORS_ORIGIN` is set
- **Upstream timeouts** — 30s request timeout, 5s connect timeout on Firecrawl calls
- **MPP secret key** — auto-generated if not set (with warning), set it for production
- **Graceful shutdown** — clean Ctrl+C handling
- **Configurable network** — testnet by default, set `MOVEMENT_REST_URL` for mainnet

## Endpoints

| Endpoint | Cost | Description |
|----------|------|-------------|
| `POST /api/scrape` | 0.01 MOVE | Scrape a single URL, returns clean markdown |
| `POST /api/crawl` | 0.05 MOVE | Crawl an entire site (max 20 pages) |
| `GET /api/health` | Free | Health check |
| `GET /api/services` | Free | Service discovery (JSON) |
| `GET /api/llms.txt` | Free | LLM-friendly service discovery |

## Pricing

Firecrawl costs ~$0.0008 per scrape at their Standard tier. At MOVE = $0.20:

- Scrape: 0.01 MOVE = $0.002 (~2.5x markup)
- Crawl: 0.05 MOVE = $0.01

## Adding more services

This example proxies one service. The same pattern works for any API — see `src/proxy/services/` in the main MPP crate for pre-built configs (OpenAI, Anthropic, Stripe, Firecrawl).

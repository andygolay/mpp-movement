# Production Deployment Guide

Deploy the voice-call example for real users on different networks.

## Architecture

```
Vercel (free) ─── React client (static)
   │
   └──▶ DigitalOcean VPS ($6/mo) ─── Caddy (reverse proxy + TLS)
                                  ─── Rust signaling server (:3002)
                                  ─── coturn TURN server (:3478)
```

## What You Need

- A domain name (any registrar, ~$10/year)
- A DigitalOcean account (or any VPS provider)
- A Vercel account (free)
- This repo pushed to GitHub

## Step 1: Create the VPS

1. Go to [digitalocean.com](https://www.digitalocean.com/), create an account
2. Click **Create → Droplets**
3. Choose:
   - **Region**: closest to your users
   - **Image**: Ubuntu 24.04
   - **Size**: Basic, $6/mo (1 vCPU, 1 GB RAM, 25 GB disk)
   - **Authentication**: SSH key (add your public key — run `cat ~/.ssh/id_rsa.pub` locally to get it. If you don't have one, run `ssh-keygen` first)
4. Click **Create Droplet**
5. Copy the IP address it gives you (e.g. `164.90.xxx.xxx`)

## Step 2: Point Your Domain at the VPS

In your domain registrar's DNS settings, add two A records:

| Type | Name | Value |
|------|------|-------|
| A | `api` | `164.90.xxx.xxx` (your VPS IP) |
| A | `turn` | `164.90.xxx.xxx` (same IP) |

This gives you `api.yourdomain.com` and `turn.yourdomain.com`. Replace `yourdomain.com` with your actual domain throughout this guide.

DNS can take a few minutes to propagate. You can check with:

```bash
dig api.yourdomain.com
```

## Step 3: Set Up the VPS

SSH into your new server:

```bash
ssh root@164.90.xxx.xxx
```

### 3a. Create a non-root user

```bash
adduser deploy
usermod -aG sudo deploy

# Copy your SSH key to the new user
mkdir -p /home/deploy/.ssh
cp /root/.ssh/authorized_keys /home/deploy/.ssh/
chown -R deploy:deploy /home/deploy/.ssh
```

Log out and SSH back in as `deploy`:

```bash
ssh deploy@164.90.xxx.xxx
```

### 3b. Open the firewall

```bash
sudo ufw allow 22/tcp           # SSH
sudo ufw allow 80/tcp           # HTTP (for Caddy TLS challenge)
sudo ufw allow 443/tcp          # HTTPS
sudo ufw allow 3478/udp         # TURN
sudo ufw allow 3478/tcp         # TURN TCP fallback
sudo ufw allow 5349/tcp         # TURN over TLS
sudo ufw allow 49152:65535/udp  # TURN media relay range
sudo ufw enable
```

Type `y` when it asks to confirm.

### 3c. Install Rust

```bash
curl --proto '=https' --tlsv1.2 -sSf https://sh.rustup.rs | sh
```

Press `1` for the default install. Then:

```bash
source $HOME/.cargo/env
```

### 3d. Add swap (prevents out-of-memory during Rust builds on 1 GB VPS)

```bash
sudo fallocate -l 2G /swapfile
sudo chmod 600 /swapfile
sudo mkswap /swapfile
sudo swapon /swapfile
echo '/swapfile none swap sw 0 0' | sudo tee -a /etc/fstab
```

## Step 4: Install and Configure coturn

```bash
sudo apt update
sudo apt install coturn -y
```

Enable coturn as a service:

```bash
sudo sed -i 's/#TURNSERVER_ENABLED=1/TURNSERVER_ENABLED=1/' /etc/default/coturn
```

Find your VPS IP info (you'll need this for the config):

```bash
# Public IP (should match your DigitalOcean droplet IP)
curl -4 ifconfig.me

# Private IP (look for the eth0 or ens3 address)
ip addr show | grep 'inet ' | grep -v 127.0.0.1
```

On DigitalOcean, your public IP is usually directly assigned (not behind NAT), so both IPs may be the same. That's fine.

Create the coturn config:

```bash
sudo tee /etc/turnserver.conf << 'EOF'
listening-port=3478
tls-listening-port=5349
realm=yourdomain.com
server-name=turn.yourdomain.com

# Authentication
lt-cred-mech
user=voicecall:CHANGE_THIS_TO_A_STRONG_PASSWORD

# IP config — replace with your VPS IPs
# If your VPS has a single public IP, use it for both
external-ip=YOUR_PUBLIC_IP
relay-ip=YOUR_PUBLIC_IP

# Relay port range (must match firewall rules)
min-port=49152
max-port=65535

fingerprint
no-cli
no-tlsv1
no-tlsv1_1

log-file=/var/log/turnserver.log
simple-log
EOF
```

**Edit the file** to fill in your real values:

```bash
sudo nano /etc/turnserver.conf
```

Replace:
- `yourdomain.com` → your actual domain
- `CHANGE_THIS_TO_A_STRONG_PASSWORD` → a random password (run `openssl rand -base64 24` to generate one)
- `YOUR_PUBLIC_IP` → your VPS's public IP

Start coturn:

```bash
sudo systemctl enable coturn
sudo systemctl start coturn
sudo systemctl status coturn
```

You should see `active (running)`. If not, check `sudo journalctl -u coturn -n 50`.

## Step 5: Build and Run the Rust Server

Clone the repo:

```bash
cd /home/deploy
git clone https://github.com/YOUR_USERNAME/mpp-movement.git
cd mpp-movement
```

Build (this takes a few minutes on a small VPS):

```bash
cargo build --release --bin voice-call-server
```

Create a production env file:

```bash
cat > /home/deploy/mpp-movement/.env.production << 'EOF'
SECRET_KEY=CHANGE_THIS_TO_A_RANDOM_SECRET
MODULE_ADDRESS=0x74f1060add0c641a0c10bb5bab2bf5fd05f94d7c25055f2419fa82d7bbf2b1e8
REST_URL=https://testnet.movementnetwork.xyz/v1
EOF
```

Generate and set the secret:

```bash
SECRET=$(openssl rand -base64 32)
sed -i "s|CHANGE_THIS_TO_A_RANDOM_SECRET|$SECRET|" /home/deploy/mpp-movement/.env.production
```

Create a systemd service so it runs automatically:

```bash
sudo tee /etc/systemd/system/voice-call-server.service << 'EOF'
[Unit]
Description=Voice Call Signaling Server
After=network.target

[Service]
Type=simple
User=deploy
WorkingDirectory=/home/deploy/mpp-movement
EnvironmentFile=/home/deploy/mpp-movement/.env.production
ExecStart=/home/deploy/mpp-movement/target/release/voice-call-server
Restart=always
RestartSec=5

[Install]
WantedBy=multi-user.target
EOF
```

Start it:

```bash
sudo systemctl daemon-reload
sudo systemctl enable voice-call-server
sudo systemctl start voice-call-server
```

Verify it's running:

```bash
sudo systemctl status voice-call-server
# Should say "active (running)"

curl http://localhost:3002/api/hosts
# Should return: []
```

## Step 6: Set Up Caddy (HTTPS + Reverse Proxy)

Caddy automatically gets TLS certificates from Let's Encrypt — no manual cert setup.

```bash
sudo apt install -y debian-keyring debian-archive-keyring apt-transport-https curl
curl -1sLf 'https://dl.cloudsmith.io/public/caddy/stable/gpg.key' | sudo gpg --dearmor -o /usr/share/keyrings/caddy-stable-archive-keyring.gpg
curl -1sLf 'https://dl.cloudsmith.io/public/caddy/stable/debian.deb.txt' | sudo tee /etc/apt/sources.list.d/caddy-stable.list
sudo apt update
sudo apt install caddy -y
```

Configure it to proxy to the Rust server:

```bash
sudo tee /etc/caddy/Caddyfile << 'EOF'
api.yourdomain.com {
    reverse_proxy localhost:3002
}
EOF
```

Replace `yourdomain.com` with your actual domain, then:

```bash
sudo systemctl restart caddy
```

Test it (may take 30 seconds for the TLS cert):

```bash
curl https://api.yourdomain.com/api/hosts
# Should return: []
```

If that works, your server is live with HTTPS. Caddy handles WebSocket proxying automatically.

## Step 7: Deploy the Client on Vercel

1. Go to [vercel.com](https://vercel.com/), sign in with GitHub
2. Click **Add New → Project**, import your `mpp-movement` repo
3. Configure the build:

| Setting | Value |
|---------|-------|
| **Framework Preset** | Vite |
| **Root Directory** | `.` (leave as repo root) |
| **Build Command** | `cd ts/client && npm install && npm run build && cd ../../examples/voice-call/client && npm install && npm run build` |
| **Output Directory** | `examples/voice-call/client/dist` |

4. Add environment variables:

| Variable | Value |
|----------|-------|
| `VITE_SERVER_URL` | `https://api.yourdomain.com` |
| `VITE_MODULE_ADDRESS` | `0x74f1060add0c641a0c10bb5bab2bf5fd05f94d7c25055f2419fa82d7bbf2b1e8` |
| `VITE_TURN_URL` | `turn:turn.yourdomain.com:3478` |
| `VITE_TURN_USERNAME` | `voicecall` |
| `VITE_TURN_CREDENTIAL` | (the coturn password you set in Step 4) |

5. Click **Deploy**

Once deployed, Vercel gives you a URL like `your-project.vercel.app`. That's your live app.

## Updating the Server

When you push code changes:

```bash
ssh deploy@164.90.xxx.xxx
cd /home/deploy/mpp-movement
git pull
cargo build --release --bin voice-call-server
sudo systemctl restart voice-call-server
```

The Vercel client redeploys automatically on git push.

## Troubleshooting

**"Audio blocked" error**: The browser requires user interaction before playing audio. Make sure users click something on the page before the call starts (clicking the "Call" button counts).

**WebRTC connects but no audio between different networks**: coturn isn't working. Check:
```bash
sudo systemctl status coturn
sudo cat /var/log/turnserver.log | tail -20
```
Make sure the firewall allows UDP 49152-65535.

**Server returns 502 from Caddy**: The Rust server crashed. Check:
```bash
sudo journalctl -u voice-call-server -n 50
```

**Caddy can't get a TLS certificate**: DNS isn't pointing to your VPS yet. Check:
```bash
dig api.yourdomain.com
```
The IP should match your VPS. Also make sure ports 80 and 443 are open.

**Vercel build fails**: The most likely issue is the `@mpp/client` build. Check the Vercel build logs — if `ts/client` fails to build, make sure its `tsconfig.json` and dependencies are correct.

**Rust build runs out of memory on 1 GB VPS**: Make sure you set up swap (Step 3d). Alternatively, build locally and copy the binary:
```bash
# On your Mac (if targeting Linux):
cargo install cross
cross build --release --target x86_64-unknown-linux-gnu --bin voice-call-server
scp target/x86_64-unknown-linux-gnu/release/voice-call-server deploy@YOUR_VPS_IP:/home/deploy/mpp-movement/target/release/
```

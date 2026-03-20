//! # Voice Call Server
//!
//! Pay-per-second voice call signaling server using WebRTC + MPP payment channels.
//!
//! ## Routes
//!
//! - `POST /api/host/go-live` — Host registers as available for calls
//! - `DELETE /api/host/go-live` — Host goes offline
//! - `GET /api/hosts` — List all live hosts
//! - `POST /api/call/start?host=0x...` — Caller initiates a call (402 flow)
//! - `GET /ws/signal/:call_id` — WebSocket for WebRTC signaling
//! - `POST /api/call/hangup` — Either party hangs up
//!
//! ## Running
//!
//! ```bash
//! cargo run --bin voice-call-server
//! ```

use axum::{
    extract::{
        ws::{Message, WebSocket, WebSocketUpgrade},
        Path, Query, State,
    },
    http::{header, Method, StatusCode},
    response::IntoResponse,
    routing::{get, post},
    Json, Router,
};
use dashmap::DashMap;
use ed25519_dalek::{Signature, VerifyingKey};
use futures::{SinkExt, StreamExt};
use mpp::format_www_authenticate;
use mpp::server::{movement as movement_builder, MovementConfig, MovementSessionOptions, Mpp};
use serde::{Deserialize, Serialize};
use sha3::{Digest, Sha3_256};
use std::sync::Arc;
use std::time::Instant;
use tokio::sync::broadcast;
use tower_http::cors::{Any, CorsLayer};
use uuid::Uuid;

const DEFAULT_REST_URL: &str = "https://testnet.movementnetwork.xyz/v1";
const DEFAULT_MODULE_ADDRESS: &str =
    "0x74f1060add0c641a0c10bb5bab2bf5fd05f94d7c25055f2419fa82d7bbf2b1e8";
const DEFAULT_SECRET_KEY: &str = "voice-call-demo-secret";

// ---------------------------------------------------------------------------
// State types
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
struct HostInfo {
    address: String,
    rate_per_second: String,
    currency: String,
    #[serde(default)]
    online: bool,
    #[serde(default)]
    busy: bool,
    #[serde(default)]
    name: Option<String>,
    /// Last time the host polled (used for staleness detection). Not serialized to clients.
    #[serde(skip)]
    last_seen: Option<Instant>,
}

/// Hosts that haven't polled in this long are considered stale and removed from listings.
const HOST_STALE_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(30);

struct ActiveCall {
    host_addr: String,
    caller_addr: String,
    started_at: Instant,
    /// Broadcast channel for WebRTC signaling relay.
    signal_tx: broadcast::Sender<SignalMessage>,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
struct SignalMessage {
    from: String,
    #[serde(rename = "type")]
    msg_type: String,
    data: serde_json::Value,
}

struct AppState {
    mpp: Mpp<mpp::server::MovementChargeMethod>,
    hosts: DashMap<String, HostInfo>,
    calls: DashMap<String, ActiveCall>,
    module_address: String,
}

// ---------------------------------------------------------------------------
// Request / response types
// ---------------------------------------------------------------------------

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct GoLiveRequest {
    address: String,
    rate_per_second: String,
    currency: String,
    name: Option<String>,
    /// Ed25519 signature proving wallet ownership (hex-encoded).
    signature: String,
    /// The full message that was signed (wallet-prefixed).
    full_message: String,
    /// Nonce (timestamp) to prevent replay.
    nonce: String,
    /// Ed25519 public key (hex-encoded, 0x-prefixed).
    pubkey: String,
}

#[derive(Deserialize)]
struct GoOfflineRequest {
    address: String,
}

#[derive(Deserialize)]
struct StartCallQuery {
    host: String,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct StartCallBody {
    /// Caller's address.
    address: String,
    /// Payment channel ID (hex, 0x-prefixed). Present after channel is opened.
    channel_id: Option<String>,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct HangupRequest {
    call_id: String,
}

// ---------------------------------------------------------------------------
// Main
// ---------------------------------------------------------------------------

#[tokio::main]
async fn main() {
    let _ = dotenvy::dotenv();

    let secret_key =
        std::env::var("SECRET_KEY").unwrap_or_else(|_| DEFAULT_SECRET_KEY.to_string());
    let module_address =
        std::env::var("MODULE_ADDRESS").unwrap_or_else(|_| DEFAULT_MODULE_ADDRESS.to_string());
    let rest_url =
        std::env::var("REST_URL").unwrap_or_else(|_| DEFAULT_REST_URL.to_string());

    // The recipient in the MPP challenge will be set per-call to the host's address.
    // We use a placeholder here; the actual recipient is set in start_call.
    let mpp = Mpp::create_movement(
        movement_builder(MovementConfig {
            recipient: "0x0", // placeholder — overridden per call
        })
        .rest_url(&rest_url)
        .secret_key(&secret_key)
        .decimals(8),
    )
    .expect("failed to create Movement MPP handler");

    let state = Arc::new(AppState {
        mpp,
        hosts: DashMap::new(),
        calls: DashMap::new(),
        module_address,
    });

    let cors = CorsLayer::new()
        .allow_origin(Any)
        .allow_methods([Method::GET, Method::POST, Method::DELETE, Method::OPTIONS])
        .allow_headers(Any)
        .expose_headers([header::WWW_AUTHENTICATE]);

    let app = Router::new()
        .route("/api/host/go-live", post(go_live).delete(go_offline))
        .route("/api/host/poll", get(host_poll))
        .route("/api/hosts", get(list_hosts))
        .route("/api/call/start", post(start_call))
        .route("/ws/signal/{call_id}", get(ws_signal))
        .route("/api/call/hangup", post(hangup))
        .layer(cors)
        .with_state(state);

    let listener = tokio::net::TcpListener::bind("0.0.0.0:3002")
        .await
        .expect("failed to bind");

    println!("Voice Call Server listening on http://localhost:3002");
    println!("  POST   /api/host/go-live     — register as host");
    println!("  DELETE /api/host/go-live     — go offline");
    println!("  GET    /api/hosts            — list live hosts");
    println!("  POST   /api/call/start       — initiate call (402 flow)");
    println!("  GET    /ws/signal/:call_id   — WebRTC signaling WebSocket");
    println!("  POST   /api/call/hangup      — end call");
    axum::serve(listener, app).await.expect("server error");
}

// ---------------------------------------------------------------------------
// Signature verification
// ---------------------------------------------------------------------------

/// Verify that:
///  1. The ed25519 signature is valid over the full message.
///  2. The public key derives to the claimed address.
///  3. The full message contains the expected address and a recent nonce.
///
/// The wallet returns BCS-serialized `AnyPublicKey` and `AnySignature` from `toString()`.
/// For ed25519, these are prefixed with a ULEB128 variant byte (0x00).
/// Address derivation depends on account type:
///  - Legacy: sha3_256(raw_pubkey || 0x00)
///  - SingleKey: sha3_256(0x00 || raw_pubkey || 0x02)
fn verify_go_live_signature(
    address: &str,
    full_message: &str,
    nonce: &str,
    signature_hex: &str,
    pubkey_hex: &str,
) -> Result<(), String> {
    // Check the nonce is a recent timestamp (within 5 minutes).
    let nonce_ts: u64 = nonce
        .parse()
        .map_err(|_| "Invalid nonce (expected timestamp)".to_string())?;
    let now_ms = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap()
        .as_millis() as u64;
    if now_ms.abs_diff(nonce_ts) > 5 * 60 * 1000 {
        return Err("Nonce expired (must be within 5 minutes)".to_string());
    }

    // Check the full message contains the address and nonce.
    let addr_lower = address.to_lowercase();
    let msg_lower = full_message.to_lowercase();
    if !msg_lower.contains(&addr_lower) {
        return Err("Full message does not contain the claimed address".to_string());
    }
    if !full_message.contains(nonce) {
        return Err("Full message does not contain the nonce".to_string());
    }

    // Decode the public key bytes (strip 0x prefix).
    let pubkey_clean = pubkey_hex.strip_prefix("0x").unwrap_or(pubkey_hex);
    println!(
        "  pubkey hex: 0x{} ({} hex chars = {} bytes)",
        pubkey_clean,
        pubkey_clean.len(),
        pubkey_clean.len() / 2
    );
    let pubkey_bytes =
        hex::decode(pubkey_clean).map_err(|e| format!("Invalid pubkey hex: {e}"))?;

    // Extract the raw 32-byte ed25519 key.
    // Wallet may send raw 32 bytes, or BCS-wrapped AnyPublicKey (0x00 prefix + 32 bytes).
    let raw_pubkey: [u8; 32] = if pubkey_bytes.len() == 32 {
        pubkey_bytes
            .try_into()
            .map_err(|_| "Public key length mismatch".to_string())?
    } else if pubkey_bytes.len() == 33 && pubkey_bytes[0] == 0x00 {
        // AnyPublicKey BCS: variant 0x00 (ed25519) + 32 bytes
        pubkey_bytes[1..]
            .try_into()
            .map_err(|_| "Public key length mismatch".to_string())?
    } else {
        return Err(format!(
            "Unexpected public key length: {} bytes (expected 32 or 33)",
            pubkey_bytes.len()
        ));
    };

    let verifying_key = VerifyingKey::from_bytes(&raw_pubkey)
        .map_err(|e| format!("Invalid public key: {e}"))?;

    // Derive the address and check it matches.
    // Try both SingleKey and Legacy derivation schemes.
    let normalize_addr = |a: &str| -> String {
        let clean = a.strip_prefix("0x").unwrap_or(a).to_lowercase();
        format!("0x{:0>64}", clean)
    };
    let claimed = normalize_addr(address);

    // SingleKey scheme: sha3_256(0x00 || raw_pubkey || 0x02)
    let mut hasher = Sha3_256::new();
    hasher.update([0x00]); // ed25519 variant
    hasher.update(&raw_pubkey);
    hasher.update([0x02]); // SingleKey scheme suffix
    let single_key_addr = format!("0x{}", hex::encode(hasher.finalize()));

    // Legacy scheme: sha3_256(raw_pubkey || 0x00)
    let mut hasher = Sha3_256::new();
    hasher.update(&raw_pubkey);
    hasher.update([0x00]); // Legacy ed25519 scheme suffix
    let legacy_addr = format!("0x{}", hex::encode(hasher.finalize()));

    println!("  claimed addr:     {}", claimed);
    println!("  single_key addr:  {}", normalize_addr(&single_key_addr));
    println!("  legacy addr:      {}", normalize_addr(&legacy_addr));

    if normalize_addr(&single_key_addr) != claimed && normalize_addr(&legacy_addr) != claimed {
        return Err("Public key does not match the claimed address".to_string());
    }

    // Decode the signature (strip 0x prefix).
    // May be raw 64 bytes or BCS-wrapped AnySignature (0x00 prefix + 64 bytes).
    let sig_clean = signature_hex.strip_prefix("0x").unwrap_or(signature_hex);
    println!(
        "  sig hex: 0x{}... ({} bytes)",
        &sig_clean[..std::cmp::min(16, sig_clean.len())],
        sig_clean.len() / 2
    );
    let sig_bytes = hex::decode(sig_clean).map_err(|e| format!("Invalid signature hex: {e}"))?;

    let raw_sig = if sig_bytes.len() == 64 {
        &sig_bytes[..]
    } else if sig_bytes.len() == 65 && sig_bytes[0] == 0x00 {
        &sig_bytes[1..]
    } else {
        return Err(format!(
            "Unexpected signature length: {} bytes (expected 64 or 65)",
            sig_bytes.len()
        ));
    };

    let signature =
        Signature::from_slice(raw_sig).map_err(|e| format!("Invalid signature: {e}"))?;

    verifying_key
        .verify_strict(full_message.as_bytes(), &signature)
        .map_err(|_| "Signature verification failed".to_string())
}

// ---------------------------------------------------------------------------
// Handlers
// ---------------------------------------------------------------------------

/// POST /api/host/go-live
async fn go_live(
    State(state): State<Arc<AppState>>,
    Json(body): Json<GoLiveRequest>,
) -> impl IntoResponse {
    // Verify the wallet signature before accepting registration.
    if let Err(e) = verify_go_live_signature(
        &body.address,
        &body.full_message,
        &body.nonce,
        &body.signature,
        &body.pubkey,
    ) {
        println!("Host go-live rejected for {}: {}", body.address, e);
        return (
            StatusCode::UNAUTHORIZED,
            Json(serde_json::json!({ "error": e })),
        )
            .into_response();
    }

    let info = HostInfo {
        address: body.address.clone(),
        rate_per_second: body.rate_per_second,
        currency: body.currency,
        online: true,
        busy: false,
        name: body.name,
        last_seen: Some(Instant::now()),
    };
    state.hosts.insert(body.address.clone(), info.clone());
    println!("Host go-live (verified): {}", body.address);
    (StatusCode::OK, Json(serde_json::json!(info))).into_response()
}

/// DELETE /api/host/go-live
async fn go_offline(
    State(state): State<Arc<AppState>>,
    Json(body): Json<GoOfflineRequest>,
) -> impl IntoResponse {
    if let Some(mut host) = state.hosts.get_mut(&body.address) {
        host.online = false;
        println!("Host offline: {}", body.address);
    }
    StatusCode::OK
}

/// GET /api/hosts
async fn list_hosts(State(state): State<Arc<AppState>>) -> Json<serde_json::Value> {
    let now = Instant::now();
    let hosts: Vec<HostInfo> = state
        .hosts
        .iter()
        .filter(|entry| {
            let h = entry.value();
            h.online
                && h.last_seen
                    .map(|t| now.duration_since(t) < HOST_STALE_TIMEOUT)
                    .unwrap_or(false)
        })
        .map(|entry| entry.value().clone())
        .collect();
    Json(serde_json::json!(hosts))
}

/// POST /api/call/start?host=0x...
///
/// Two-phase flow:
///  1. First call (no channel_id): returns 402 with MPP session challenge.
///  2. Second call (with channel_id + pubkey): creates the call and returns callId + wsUrl.
async fn start_call(
    State(state): State<Arc<AppState>>,
    Query(query): Query<StartCallQuery>,
    Json(body): Json<StartCallBody>,
) -> impl IntoResponse {
    let host_addr = &query.host;

    // Check host exists and is online.
    let host = match state.hosts.get(host_addr) {
        Some(h) if h.online => h.clone(),
        Some(_) => {
            return (
                StatusCode::NOT_FOUND,
                Json(serde_json::json!({ "error": "Host is offline" })),
            )
                .into_response();
        }
        None => {
            return (
                StatusCode::NOT_FOUND,
                Json(serde_json::json!({ "error": "Host not found" })),
            )
                .into_response();
        }
    };

    if host.busy {
        return (
            StatusCode::CONFLICT,
            Json(serde_json::json!({ "error": "Host is busy" })),
        )
            .into_response();
    }

    // Phase 1: No channel_id yet -- return 402 with session challenge.
    let _channel_id = match &body.channel_id {
        Some(id) if !id.is_empty() => id.clone(),
        _ => {
            let suggested_deposit =
                (host.rate_per_second.parse::<u64>().unwrap_or(1000) * 300).to_string();

            // The host is the payee — they receive the MOVE directly on-chain.
            let challenge = state
                .mpp
                .movement_session_challenge_with_recipient(
                    &host.rate_per_second,
                    host_addr,
                    MovementSessionOptions {
                        unit_type: Some("second"),
                        suggested_deposit: Some(&suggested_deposit),
                        module_address: Some(&state.module_address),
                        description: Some("Pay-per-second voice call"),
                        ..Default::default()
                    },
                )
                .expect("failed to create session challenge");

            let www_auth = format_www_authenticate(&challenge).expect("failed to format");
            return (
                StatusCode::PAYMENT_REQUIRED,
                [(header::WWW_AUTHENTICATE, www_auth)],
                Json(serde_json::json!({
                    "error": "Payment Required",
                    "ratePerSecond": host.rate_per_second,
                    "currency": host.currency,
                    "recipient": host_addr,
                    "suggestedDeposit": suggested_deposit,
                })),
            )
                .into_response();
        }
    };

    // Phase 2: Channel opened -- create the call.

    // Mark host as busy.
    if let Some(mut h) = state.hosts.get_mut(host_addr) {
        h.busy = true;
    }

    let call_id = Uuid::new_v4().to_string();
    let (signal_tx, _) = broadcast::channel::<SignalMessage>(64);

    state.calls.insert(
        call_id.clone(),
        ActiveCall {
            host_addr: host_addr.clone(),
            caller_addr: body.address.clone(),
            started_at: Instant::now(),
            signal_tx,
        },
    );

    let ws_url = format!("/ws/signal/{call_id}");
    println!(
        "Call started: {} -> {} (call_id: {})",
        body.address, host_addr, call_id
    );

    (
        StatusCode::OK,
        Json(serde_json::json!({
            "callId": call_id,
            "wsUrl": ws_url,
        })),
    )
        .into_response()
}

/// GET /api/host/poll?address=0x... -- Check if there's an incoming call for this host.
async fn host_poll(
    State(state): State<Arc<AppState>>,
    Query(params): Query<std::collections::HashMap<String, String>>,
) -> impl IntoResponse {
    let address = match params.get("address") {
        Some(a) => a,
        None => {
            return (
                StatusCode::BAD_REQUEST,
                Json(serde_json::json!({ "error": "Missing address param" })),
            )
                .into_response()
        }
    };

    // Update last_seen so the host doesn't go stale
    if let Some(mut host) = state.hosts.get_mut(address) {
        host.last_seen = Some(Instant::now());
    }

    // Find a call where this address is the host (ignore stale calls > 60s old
    // with no WebSocket subscribers, meaning neither party connected).
    let now = Instant::now();
    let mut stale_calls = vec![];
    for entry in state.calls.iter() {
        if entry.value().host_addr == *address {
            let age = now.duration_since(entry.value().started_at);
            // If the call has active WebSocket subscribers, it's a real call.
            // If it's old and nobody connected (receiver_count == 0), it's stale.
            if age.as_secs() > 30 && entry.value().signal_tx.receiver_count() == 0 {
                stale_calls.push(entry.key().clone());
                continue;
            }
            return Json(serde_json::json!({
                "callId": entry.key(),
                "callerAddress": entry.value().caller_addr,
            }))
            .into_response();
        }
    }
    // Clean up stale calls
    for call_id in stale_calls {
        if let Some((_, call)) = state.calls.remove(&call_id) {
            if let Some(mut host) = state.hosts.get_mut(&call.host_addr) {
                host.busy = false;
            }
            println!("Cleaned up stale call: {}", call_id);
        }
    }

    Json(serde_json::json!({})).into_response()
}

/// GET /ws/signal/:call_id -- WebSocket upgrade for WebRTC signaling.
async fn ws_signal(
    State(state): State<Arc<AppState>>,
    Path(call_id): Path<String>,
    Query(params): Query<std::collections::HashMap<String, String>>,
    ws: WebSocketUpgrade,
) -> impl IntoResponse {
    let call = match state.calls.get(&call_id) {
        Some(c) => c,
        None => {
            return (
                StatusCode::NOT_FOUND,
                Json(serde_json::json!({ "error": "Call not found" })),
            )
                .into_response();
        }
    };

    let signal_tx = call.signal_tx.clone();
    let host_addr = call.host_addr.clone();
    let caller_addr = call.caller_addr.clone();
    drop(call);

    // Determine identity from query param `address`.
    let addr = params.get("address").cloned().unwrap_or_default();
    let role = if addr == host_addr {
        "host".to_string()
    } else if addr == caller_addr {
        "caller".to_string()
    } else {
        "unknown".to_string()
    };

    ws.on_upgrade(move |socket| handle_ws(socket, signal_tx, addr, role))
        .into_response()
}

async fn handle_ws(
    socket: WebSocket,
    signal_tx: broadcast::Sender<SignalMessage>,
    addr: String,
    role: String,
) {
    let (mut ws_sink, mut ws_stream) = socket.split();
    let mut signal_rx = signal_tx.subscribe();

    println!("WebSocket connected: {role} ({addr})");

    // Relay: broadcast -> this client (skip own messages).
    let addr_clone = addr.clone();
    let send_task = tokio::spawn(async move {
        while let Ok(msg) = signal_rx.recv().await {
            // Only relay messages from the other party.
            if msg.from == addr_clone {
                continue;
            }
            // Send the original message as-is (data contains the full client message)
            let json = serde_json::to_string(&msg.data).unwrap_or_default();
            if ws_sink.send(Message::Text(json.into())).await.is_err() {
                break;
            }
        }
    });

    // Receive: this client -> broadcast.
    let signal_tx_clone = signal_tx.clone();
    let recv_task = tokio::spawn(async move {
        while let Some(Ok(msg)) = ws_stream.next().await {
            match msg {
                Message::Text(text) => {
                    if let Ok(parsed) =
                        serde_json::from_str::<serde_json::Value>(text.as_ref())
                    {
                        let msg_type = parsed
                            .get("type")
                            .and_then(|v| v.as_str())
                            .unwrap_or("unknown")
                            .to_string();

                        let _ = signal_tx_clone.send(SignalMessage {
                            from: addr.clone(),
                            msg_type,
                            data: parsed,
                        });
                    }
                }
                Message::Close(_) => break,
                _ => {}
            }
        }
    });

    tokio::select! {
        _ = send_task => {},
        _ = recv_task => {},
    }
    println!("WebSocket disconnected: {role}");
}

/// POST /api/call/hangup
///
/// Removes the call and marks the host as not busy. The host already has
/// settlement data (vouchers) from the WebRTC data channel.
async fn hangup(
    State(state): State<Arc<AppState>>,
    Json(body): Json<HangupRequest>,
) -> impl IntoResponse {
    let call = match state.calls.remove(&body.call_id) {
        Some((_, c)) => c,
        None => {
            return (
                StatusCode::NOT_FOUND,
                Json(serde_json::json!({ "error": "Call not found" })),
            )
                .into_response();
        }
    };

    let duration = call.started_at.elapsed().as_secs();

    // Mark host as no longer busy.
    if let Some(mut host) = state.hosts.get_mut(&call.host_addr) {
        host.busy = false;
    }

    println!(
        "Call ended: {} -> {} (duration={}s)",
        call.caller_addr, call.host_addr, duration
    );

    (
        StatusCode::OK,
        Json(serde_json::json!({
            "duration": duration,
        })),
    )
        .into_response()
}

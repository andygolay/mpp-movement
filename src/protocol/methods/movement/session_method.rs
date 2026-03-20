//! Server-side session payment verification for Movement.
//!
//! Implements the `SessionMethod` trait for Movement session payments (pay-as-you-go).
//! Handles four channel lifecycle actions: open, topUp, voucher, close.

use std::future::Future;
use std::sync::Arc;

// Observability macros — compile to no-ops when the `observability` feature is disabled.
macro_rules! mpp_info {
    ($($arg:tt)*) => {
        #[cfg(feature = "observability")]
        tracing::info!($($arg)*);
    };
}

#[allow(unused_macros)]
macro_rules! mpp_warn {
    ($($arg:tt)*) => {
        #[cfg(feature = "observability")]
        tracing::warn!($($arg)*);
    };
}

macro_rules! mpp_error {
    ($($arg:tt)*) => {
        #[cfg(feature = "observability")]
        tracing::error!($($arg)*);
    };
}

use super::session::{MovementSessionMethodDetails, SessionCredentialPayload};
use super::voucher::verify_voucher;
use super::{INTENT_SESSION, METHOD_NAME};
use crate::protocol::core::{PaymentCredential, Receipt};
use crate::protocol::intents::SessionRequest;
use crate::protocol::traits::{SessionMethod as SessionMethodTrait, VerificationError};

// ==================== ChannelState ====================

/// State for a payment channel, including per-session accounting.
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct ChannelState {
    pub channel_id: String,
    pub module_address: String,
    pub registry_address: String,
    /// Payer address (32-byte hex).
    pub payer: String,
    /// Payee address (32-byte hex).
    pub payee: String,
    /// Token metadata address.
    pub token: String,
    /// Ed25519 public key of the authorized signer (hex).
    pub authorized_signer_pubkey: Vec<u8>,
    pub deposit: u64,
    pub settled_on_chain: u64,
    pub highest_voucher_amount: u64,
    pub highest_voucher_signature: Option<Vec<u8>>,
    pub spent: u64,
    pub units: u64,
    pub finalized: bool,
    pub created_at: String,
}

// ==================== ChannelStore ====================

/// Trait for channel state persistence.
pub trait ChannelStore: Send + Sync {
    fn get_channel(
        &self,
        channel_id: &str,
    ) -> std::pin::Pin<
        Box<dyn Future<Output = Result<Option<ChannelState>, VerificationError>> + Send + '_>,
    >;

    #[allow(clippy::type_complexity)]
    fn update_channel(
        &self,
        channel_id: &str,
        updater: Box<
            dyn FnOnce(Option<ChannelState>) -> Result<Option<ChannelState>, VerificationError>
                + Send,
        >,
    ) -> std::pin::Pin<
        Box<dyn Future<Output = Result<Option<ChannelState>, VerificationError>> + Send + '_>,
    >;

    fn wait_for_update(
        &self,
        _channel_id: &str,
    ) -> std::pin::Pin<Box<dyn Future<Output = ()> + Send + '_>> {
        Box::pin(async {})
    }
}

/// Atomically deduct `amount` from a channel's available balance.
pub async fn deduct_from_channel(
    store: &dyn ChannelStore,
    channel_id: &str,
    amount: u64,
) -> Result<ChannelState, VerificationError> {
    let result = store
        .update_channel(
            channel_id,
            Box::new(move |current| {
                let state = current
                    .ok_or_else(|| VerificationError::channel_not_found("channel not found"))?;
                let available = state.highest_voucher_amount.saturating_sub(state.spent);
                if available >= amount {
                    Ok(Some(ChannelState {
                        spent: state.spent + amount,
                        units: state.units + 1,
                        ..state
                    }))
                } else {
                    Err(VerificationError::insufficient_balance(format!(
                        "requested {}, available {}",
                        amount, available
                    )))
                }
            }),
        )
        .await?;

    result.ok_or_else(|| VerificationError::channel_not_found("channel not found"))
}

// ==================== In-memory store ====================

/// In-memory channel store for testing.
pub struct InMemoryChannelStore {
    channels: std::sync::Mutex<std::collections::HashMap<String, ChannelState>>,
}

impl Default for InMemoryChannelStore {
    fn default() -> Self {
        Self {
            channels: std::sync::Mutex::new(std::collections::HashMap::new()),
        }
    }
}

impl InMemoryChannelStore {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn get_channel_sync(&self, channel_id: &str) -> Option<ChannelState> {
        self.channels.lock().unwrap().get(channel_id).cloned()
    }

    pub fn insert(&self, channel_id: &str, state: ChannelState) {
        self.channels
            .lock()
            .unwrap()
            .insert(channel_id.to_string(), state);
    }
}

impl ChannelStore for InMemoryChannelStore {
    fn get_channel(
        &self,
        channel_id: &str,
    ) -> std::pin::Pin<
        Box<dyn Future<Output = Result<Option<ChannelState>, VerificationError>> + Send + '_>,
    > {
        let result = self.channels.lock().unwrap().get(channel_id).cloned();
        Box::pin(async move { Ok(result) })
    }

    fn update_channel(
        &self,
        channel_id: &str,
        updater: Box<
            dyn FnOnce(Option<ChannelState>) -> Result<Option<ChannelState>, VerificationError>
                + Send,
        >,
    ) -> std::pin::Pin<
        Box<dyn Future<Output = Result<Option<ChannelState>, VerificationError>> + Send + '_>,
    > {
        let mut map = self.channels.lock().unwrap();
        let current = map.get(channel_id).cloned();
        let result = updater(current);
        let channel_id = channel_id.to_string();
        match result {
            Ok(Some(state)) => {
                map.insert(channel_id, state.clone());
                Box::pin(async move { Ok(Some(state)) })
            }
            Ok(None) => {
                map.remove(&channel_id);
                Box::pin(async { Ok(None) })
            }
            Err(e) => Box::pin(async { Err(e) }),
        }
    }
}

// ==================== On-chain reading via REST ====================

/// On-chain channel state from the Move contract.
#[derive(Debug, Clone)]
pub struct OnChainChannel {
    pub payer: String,
    pub payee: String,
    pub token: String,
    pub deposit: u64,
    pub settled: u64,
    pub close_requested_at: u64,
    pub finalized: bool,
}

/// On-chain transaction verification result.
#[derive(Debug, Clone)]
pub struct OnChainTransaction {
    pub hash: String,
    pub success: bool,
    pub vm_status: String,
}

/// Verify that a transaction succeeded on-chain via Movement REST API.
///
/// Fetches the transaction by hash and checks `success == true`.
/// Returns an error if the transaction failed, is pending, or not found.
pub async fn verify_transaction_on_chain(
    rest_url: &str,
    tx_hash: &str,
) -> Result<OnChainTransaction, VerificationError> {
    #[cfg(not(feature = "client"))]
    {
        let _ = (rest_url, tx_hash);
        Err(VerificationError::network_error(
            "on-chain transaction verification requires the 'client' feature (reqwest)",
        ))
    }

    #[cfg(feature = "client")]
    {
        let hash = tx_hash.strip_prefix("0x").unwrap_or(tx_hash);
        let url = format!(
            "{}/transactions/by_hash/0x{}",
            rest_url.trim_end_matches('/'),
            hash
        );

        let client = reqwest::Client::new();
        let resp = client.get(&url).send().await.map_err(|e| {
            VerificationError::network_error(format!("transaction lookup failed: {}", e))
        })?;

        if resp.status() == reqwest::StatusCode::NOT_FOUND {
            return Err(VerificationError::pending(
                "transaction not yet confirmed on-chain",
            ));
        }

        if !resp.status().is_success() {
            let status = resp.status();
            let text = resp.text().await.unwrap_or_default();
            return Err(VerificationError::network_error(format!(
                "transaction lookup failed ({}): {}",
                status, text
            )));
        }

        let body: serde_json::Value = resp.json().await.map_err(|e| {
            VerificationError::network_error(format!("failed to parse transaction info: {}", e))
        })?;

        let success = body.get("success").and_then(|v| v.as_bool()).unwrap_or(false);
        let vm_status = body
            .get("vm_status")
            .and_then(|v| v.as_str())
            .unwrap_or("")
            .to_string();
        let hash_str = body
            .get("hash")
            .and_then(|v| v.as_str())
            .unwrap_or(tx_hash)
            .to_string();

        // Check if it's still pending (type == "pending_transaction")
        let tx_type = body
            .get("type")
            .and_then(|v| v.as_str())
            .unwrap_or("");
        if tx_type == "pending_transaction" {
            return Err(VerificationError::pending(
                "transaction is pending confirmation",
            ));
        }

        if !success {
            mpp_error!(tx_hash = %tx_hash, vm_status = %vm_status, "transaction failed on-chain");
            return Err(VerificationError::transaction_failed(format!(
                "transaction failed on-chain: {}",
                vm_status
            )));
        }

        mpp_info!(tx_hash = %tx_hash, "transaction verified on-chain");
        Ok(OnChainTransaction {
            hash: hash_str,
            success,
            vm_status,
        })
    }
}

/// Read channel state from the Move contract via Movement REST API.
///
/// Calls the `get_channel` view function.
/// Requires the `client` feature (for reqwest HTTP client).
pub async fn get_on_chain_channel(
    rest_url: &str,
    module_address: &str,
    registry_address: &str,
    channel_id_hex: &str,
) -> Result<OnChainChannel, VerificationError> {
    #[cfg(not(feature = "client"))]
    {
        let _ = (rest_url, module_address, registry_address, channel_id_hex);
        Err(VerificationError::network_error(
            "on-chain channel reading requires the 'client' feature (reqwest)",
        ))
    }

    #[cfg(feature = "client")]
    {
        let url = format!("{}/view", rest_url.trim_end_matches('/'));

        let channel_id_bytes: Vec<u8> = hex::decode(
            channel_id_hex.strip_prefix("0x").unwrap_or(channel_id_hex),
        )
        .map_err(|e| {
            VerificationError::invalid_payload(format!("invalid channel ID hex: {}", e))
        })?;

        let body = serde_json::json!({
            "function": format!("{}::channel::get_channel", module_address),
            "type_arguments": [],
            "arguments": [
                registry_address,
                format!("0x{}", hex::encode(&channel_id_bytes))
            ]
        });

        let client = reqwest::Client::new();
        let resp = client
            .post(&url)
            .json(&body)
            .send()
            .await
            .map_err(|e| {
                VerificationError::network_error(format!("REST request failed: {}", e))
            })?;

        if !resp.status().is_success() {
            let status = resp.status();
            let text = resp.text().await.unwrap_or_default();
            return Err(VerificationError::network_error(format!(
                "get_channel failed ({}): {}",
                status, text
            )));
        }

        // Response: [payer, payee, token, deposit, settled, close_requested_at, finalized]
        let result: Vec<serde_json::Value> = resp.json().await.map_err(|e| {
            VerificationError::network_error(format!("failed to parse response: {}", e))
        })?;

        if result.len() < 7 {
            return Err(VerificationError::network_error(
                "unexpected get_channel response format",
            ));
        }

        Ok(OnChainChannel {
            payer: result[0].as_str().unwrap_or("").to_string(),
            payee: result[1].as_str().unwrap_or("").to_string(),
            token: result[2].as_str().unwrap_or("").to_string(),
            deposit: result[3]
                .as_str()
                .and_then(|s| s.parse().ok())
                .unwrap_or(0),
            settled: result[4]
                .as_str()
                .and_then(|s| s.parse().ok())
                .unwrap_or(0),
            close_requested_at: result[5]
                .as_str()
                .and_then(|s| s.parse().ok())
                .unwrap_or(0),
            finalized: result[6].as_bool().unwrap_or(false),
        })
    }
}

// ==================== SessionMethod ====================

/// Configuration for the Movement session method.
#[derive(Debug, Clone)]
pub struct SessionMethodConfig {
    /// Module address where MovementStreamChannel is deployed.
    pub module_address: String,
    /// Registry address (usually same as module address).
    pub registry_address: String,
    /// Movement REST API URL.
    pub rest_url: String,
    /// Token metadata address (e.g., "0xa" for MOVE).
    pub token_metadata: String,
    /// Minimum voucher delta to accept (in base units).
    pub min_voucher_delta: u64,
}

impl Default for SessionMethodConfig {
    fn default() -> Self {
        Self {
            module_address: super::DEFAULT_MODULE_ADDRESS.to_string(),
            registry_address: super::DEFAULT_MODULE_ADDRESS.to_string(),
            rest_url: super::DEFAULT_REST_URL_TESTNET.to_string(),
            token_metadata: super::MOVE_TOKEN_METADATA.to_string(),
            min_voucher_delta: 0,
        }
    }
}

impl SessionMethodConfig {
    /// Create a config from environment variables, falling back to defaults.
    ///
    /// Checks `MOVEMENT_MODULE_ADDRESS` for the module/registry address and
    /// `MOVEMENT_REST_URL` for the REST API URL.
    pub fn from_env() -> Self {
        let module_address = std::env::var(super::MODULE_ADDRESS_ENV_VAR)
            .unwrap_or_else(|_| super::DEFAULT_MODULE_ADDRESS.to_string());
        let registry_address = module_address.clone();
        let rest_url = std::env::var("MOVEMENT_REST_URL")
            .unwrap_or_else(|_| super::DEFAULT_REST_URL_TESTNET.to_string());

        Self {
            module_address,
            registry_address,
            rest_url,
            ..Default::default()
        }
    }

    /// Create a config for a specific Movement network.
    pub fn for_network(network: super::MovementNetwork) -> Self {
        Self {
            module_address: network.default_module_address().to_string(),
            registry_address: network.default_module_address().to_string(),
            rest_url: network.default_rest_url().to_string(),
            token_metadata: network.default_currency().to_string(),
            min_voucher_delta: 0,
        }
    }
}

/// Movement session method for server-side session payment verification.
///
/// Handles four channel lifecycle actions:
/// - `open`: verify open tx on-chain, verify initial voucher, create channel in store
/// - `topUp`: verify topUp tx on-chain, update deposit in store
/// - `voucher`: verify voucher signature, check monotonicity/bounds/delta, update store
/// - `close`: verify final voucher, finalize in store
#[derive(Clone)]
pub struct SessionMethod {
    store: Arc<dyn ChannelStore>,
    config: SessionMethodConfig,
}

impl SessionMethod {
    /// Create a new Movement session method.
    pub fn new(store: Arc<dyn ChannelStore>, config: SessionMethodConfig) -> Self {
        Self { store, config }
    }

    /// Get the session method configuration.
    pub fn config(&self) -> &SessionMethodConfig {
        &self.config
    }

    fn parse_hex_bytes(hex_str: &str) -> Result<Vec<u8>, VerificationError> {
        let s = hex_str.strip_prefix("0x").unwrap_or(hex_str);
        hex::decode(s).map_err(|e| {
            VerificationError::invalid_payload(format!("invalid hex: {}", e))
        })
    }

    fn resolve_details(
        &self,
        request: &SessionRequest,
    ) -> MovementSessionMethodDetails {
        use super::session::MovementSessionExt;
        request.movement_session_details().unwrap_or(MovementSessionMethodDetails {
            module_address: self.config.module_address.clone(),
            registry_address: Some(self.config.registry_address.clone()),
            token_metadata: Some(self.config.token_metadata.clone()),
            channel_id: None,
            min_voucher_delta: None,
            network: None,
        })
    }

    fn resolve_rest_url(&self) -> &str {
        &self.config.rest_url
    }

    fn resolve_min_delta(&self, details: &MovementSessionMethodDetails) -> u64 {
        details
            .min_voucher_delta
            .as_ref()
            .and_then(|s| s.parse::<u64>().ok())
            .unwrap_or(self.config.min_voucher_delta)
    }

    /// Handle 'open' action: verify the open tx landed on-chain, read channel state,
    /// verify initial voucher, create channel in store.
    #[cfg_attr(feature = "observability", tracing::instrument(skip(self, payload, details), fields(action = "open")))]
    async fn handle_open(
        &self,
        payload: &SessionCredentialPayload,
        details: &MovementSessionMethodDetails,
    ) -> Result<Receipt, VerificationError> {
        let (tx_hash, channel_id_str, cumulative_amount_str, signature_str, authorized_signer_str) =
            match payload {
                SessionCredentialPayload::Open {
                    tx_hash,
                    channel_id,
                    cumulative_amount,
                    signature,
                    authorized_signer,
                } => (tx_hash, channel_id, cumulative_amount, signature, authorized_signer),
                _ => unreachable!(),
            };

        let module_address = &details.module_address;
        let registry_address = details
            .registry_address
            .as_deref()
            .unwrap_or(module_address);
        let rest_url = self.resolve_rest_url();

        // Verify the open transaction actually succeeded on-chain.
        mpp_info!(tx_hash = %tx_hash, channel_id = %channel_id_str, "verifying open transaction on-chain");
        verify_transaction_on_chain(rest_url, tx_hash).await?;

        let on_chain =
            get_on_chain_channel(rest_url, module_address, registry_address, channel_id_str)
                .await?;

        if on_chain.deposit == 0 {
            return Err(VerificationError::channel_not_found(
                "channel not funded on-chain",
            ));
        }
        if on_chain.finalized {
            return Err(VerificationError::channel_closed("channel is finalized"));
        }

        let cumulative_amount: u64 = cumulative_amount_str
            .parse()
            .map_err(|_| VerificationError::invalid_payload("invalid cumulativeAmount"))?;

        if cumulative_amount > on_chain.deposit {
            return Err(VerificationError::amount_exceeds_deposit(format!(
                "voucher amount {} exceeds deposit {}",
                cumulative_amount, on_chain.deposit
            )));
        }

        // Verify voucher signature.
        let sig_bytes = Self::parse_hex_bytes(signature_str)?;
        let sig_array: [u8; 64] = sig_bytes.try_into().map_err(|_| {
            VerificationError::invalid_payload("signature must be 64 bytes")
        })?;

        let authorized_pubkey = match authorized_signer_str {
            Some(s) => Self::parse_hex_bytes(s)?,
            None => vec![],
        };
        let pubkey_for_verify: [u8; 32] = if authorized_pubkey.len() == 32 {
            authorized_pubkey.clone().try_into().unwrap()
        } else {
            return Err(VerificationError::invalid_payload(
                "authorized signer public key must be 32 bytes",
            ));
        };

        let channel_id_bytes = Self::parse_hex_bytes(channel_id_str)?;
        let is_valid = verify_voucher(
            &channel_id_bytes,
            cumulative_amount,
            &sig_array,
            &pubkey_for_verify,
            &authorized_pubkey,
        );
        if !is_valid {
            return Err(VerificationError::invalid_signature("invalid voucher signature"));
        }

        // Create channel in store.
        let channel_id_key = channel_id_str.clone();
        let channel_id_val = channel_id_str.clone();
        let module_addr = module_address.to_string();
        let registry_addr = registry_address.to_string();
        let on_chain_deposit = on_chain.deposit;
        let on_chain_payer = on_chain.payer.clone();
        let on_chain_payee = on_chain.payee.clone();
        let on_chain_token = on_chain.token.clone();
        let authorized_pubkey_clone = authorized_pubkey.clone();
        let sig_bytes_clone = sig_array.to_vec();

        self.store
            .update_channel(
                &channel_id_key,
                Box::new(move |_existing| {
                    Ok(Some(ChannelState {
                        channel_id: channel_id_val,
                        module_address: module_addr,
                        registry_address: registry_addr,
                        payer: on_chain_payer,
                        payee: on_chain_payee,
                        token: on_chain_token,
                        authorized_signer_pubkey: authorized_pubkey_clone,
                        deposit: on_chain_deposit,
                        settled_on_chain: 0,
                        highest_voucher_amount: cumulative_amount,
                        highest_voucher_signature: Some(sig_bytes_clone),
                        spent: 0,
                        units: 0,
                        finalized: false,
                        created_at: now_iso8601(),
                    }))
                }),
            )
            .await?;

        mpp_info!(channel_id = %channel_id_str, deposit = on_chain_deposit, "channel opened successfully");
        Ok(Receipt::success(METHOD_NAME, tx_hash))
    }

    /// Handle 'topUp' action.
    #[cfg_attr(feature = "observability", tracing::instrument(skip(self, payload, details), fields(action = "topUp")))]
    async fn handle_top_up(
        &self,
        payload: &SessionCredentialPayload,
        details: &MovementSessionMethodDetails,
    ) -> Result<Receipt, VerificationError> {
        let (tx_hash, channel_id_str, _additional_deposit_str) = match payload {
            SessionCredentialPayload::TopUp {
                tx_hash,
                channel_id,
                additional_deposit,
            } => (tx_hash, channel_id, additional_deposit),
            _ => unreachable!(),
        };

        let channel = self
            .store
            .get_channel(channel_id_str)
            .await?
            .ok_or_else(|| VerificationError::channel_not_found("channel not found"))?;

        let module_address = &details.module_address;
        let registry_address = details
            .registry_address
            .as_deref()
            .unwrap_or(module_address);
        let rest_url = self.resolve_rest_url();

        // Verify the top-up transaction actually succeeded on-chain.
        mpp_info!(tx_hash = %tx_hash, channel_id = %channel_id_str, "verifying top-up transaction on-chain");
        verify_transaction_on_chain(rest_url, tx_hash).await?;

        let on_chain =
            get_on_chain_channel(rest_url, module_address, registry_address, channel_id_str)
                .await?;

        if on_chain.deposit <= channel.deposit {
            return Err(VerificationError::new(format!(
                "channel deposit did not increase after topUp (on-chain: {}, stored: {})",
                on_chain.deposit, channel.deposit
            )));
        }

        let new_deposit = on_chain.deposit;
        let channel_id_owned = channel_id_str.clone();
        self.store
            .update_channel(
                &channel_id_owned,
                Box::new(move |current| {
                    let state = current
                        .ok_or_else(|| VerificationError::channel_not_found("channel not found"))?;
                    Ok(Some(ChannelState {
                        deposit: new_deposit,
                        ..state
                    }))
                }),
            )
            .await?;

        Ok(Receipt::success(METHOD_NAME, tx_hash))
    }

    /// Handle 'voucher' action — pure off-chain verification, no RPC call.
    #[cfg_attr(feature = "observability", tracing::instrument(skip(self, payload, details), fields(action = "voucher")))]
    async fn handle_voucher(
        &self,
        payload: &SessionCredentialPayload,
        details: &MovementSessionMethodDetails,
    ) -> Result<Receipt, VerificationError> {
        let (channel_id_str, cumulative_amount_str, signature_str) = match payload {
            SessionCredentialPayload::Voucher {
                channel_id,
                cumulative_amount,
                signature,
            } => (channel_id, cumulative_amount, signature),
            _ => unreachable!(),
        };

        let channel = self
            .store
            .get_channel(channel_id_str)
            .await?
            .ok_or_else(|| VerificationError::channel_not_found("channel not found"))?;

        if channel.finalized {
            return Err(VerificationError::channel_closed("channel is finalized"));
        }

        let cumulative_amount: u64 = cumulative_amount_str
            .parse()
            .map_err(|_| VerificationError::invalid_payload("invalid cumulativeAmount"))?;

        let min_delta = self.resolve_min_delta(details);

        self.verify_and_accept_voucher(
            channel_id_str,
            &channel,
            cumulative_amount,
            signature_str,
            min_delta,
        )
        .await
    }

    /// Handle 'close' action.
    #[cfg_attr(feature = "observability", tracing::instrument(skip(self, payload, _details), fields(action = "close")))]
    async fn handle_close(
        &self,
        payload: &SessionCredentialPayload,
        _details: &MovementSessionMethodDetails,
    ) -> Result<Receipt, VerificationError> {
        let (channel_id_str, cumulative_amount_str, signature_str) = match payload {
            SessionCredentialPayload::Close {
                channel_id,
                cumulative_amount,
                signature,
            } => (channel_id, cumulative_amount, signature),
            _ => unreachable!(),
        };

        let channel = self
            .store
            .get_channel(channel_id_str)
            .await?
            .ok_or_else(|| VerificationError::channel_not_found("channel not found"))?;

        if channel.finalized {
            return Err(VerificationError::channel_closed("channel is already finalized"));
        }

        let cumulative_amount: u64 = cumulative_amount_str
            .parse()
            .map_err(|_| VerificationError::invalid_payload("invalid cumulativeAmount"))?;

        if cumulative_amount < channel.highest_voucher_amount {
            return Err(VerificationError::new(format!(
                "close voucher amount {} must be >= highest accepted voucher {}",
                cumulative_amount, channel.highest_voucher_amount
            )));
        }

        // Verify signature.
        let channel_id_bytes = Self::parse_hex_bytes(channel_id_str)?;
        let sig_bytes = Self::parse_hex_bytes(signature_str)?;
        let sig_array: [u8; 64] = sig_bytes.try_into().map_err(|_| {
            VerificationError::invalid_payload("signature must be 64 bytes")
        })?;
        let pubkey: [u8; 32] = channel
            .authorized_signer_pubkey
            .clone()
            .try_into()
            .map_err(|_| VerificationError::invalid_payload("stored pubkey not 32 bytes"))?;

        let is_valid = verify_voucher(
            &channel_id_bytes,
            cumulative_amount,
            &sig_array,
            &pubkey,
            &channel.authorized_signer_pubkey,
        );
        if !is_valid {
            return Err(VerificationError::invalid_signature("invalid voucher signature"));
        }

        // Finalize in store.
        let channel_id_owned = channel_id_str.clone();
        let sig_vec = sig_array.to_vec();
        self.store
            .update_channel(
                &channel_id_owned,
                Box::new(move |current| {
                    let state = match current {
                        Some(s) => s,
                        None => return Ok(None),
                    };
                    Ok(Some(ChannelState {
                        highest_voucher_amount: cumulative_amount,
                        highest_voucher_signature: Some(sig_vec),
                        finalized: true,
                        ..state
                    }))
                }),
            )
            .await?;

        mpp_info!(channel_id = %channel_id_str, final_amount = cumulative_amount, "channel closed");
        Ok(Receipt::success(METHOD_NAME, channel_id_str))
    }

    /// Verify an incremental voucher and update channel state.
    async fn verify_and_accept_voucher(
        &self,
        channel_id_str: &str,
        channel: &ChannelState,
        cumulative_amount: u64,
        signature_str: &str,
        min_delta: u64,
    ) -> Result<Receipt, VerificationError> {
        if cumulative_amount > channel.deposit {
            return Err(VerificationError::amount_exceeds_deposit(format!(
                "voucher amount {} exceeds deposit {}",
                cumulative_amount, channel.deposit
            )));
        }

        // Idempotent accept for replays of the highest voucher.
        if cumulative_amount <= channel.highest_voucher_amount {
            let sig_bytes = Self::parse_hex_bytes(signature_str)?;
            let is_exact_replay = channel
                .highest_voucher_signature
                .as_ref()
                .is_some_and(|stored| {
                    stored == &sig_bytes && cumulative_amount == channel.highest_voucher_amount
                });
            if is_exact_replay {
                return Ok(Receipt::success(METHOD_NAME, &channel.channel_id));
            }

            // Not exact replay — verify signature to prevent forgery.
            let channel_id_bytes = Self::parse_hex_bytes(channel_id_str)?;
            let sig_array: [u8; 64] = sig_bytes.try_into().map_err(|_| {
                VerificationError::invalid_payload("signature must be 64 bytes")
            })?;
            let pubkey: [u8; 32] = channel
                .authorized_signer_pubkey
                .clone()
                .try_into()
                .map_err(|_| VerificationError::invalid_payload("stored pubkey not 32 bytes"))?;

            let is_valid = verify_voucher(
                &channel_id_bytes,
                cumulative_amount,
                &sig_array,
                &pubkey,
                &channel.authorized_signer_pubkey,
            );
            if !is_valid {
                return Err(VerificationError::invalid_signature("invalid voucher signature"));
            }
            return Ok(Receipt::success(METHOD_NAME, &channel.channel_id));
        }

        let delta = cumulative_amount - channel.highest_voucher_amount;
        if delta < min_delta {
            return Err(VerificationError::delta_too_small(format!(
                "voucher delta {} below minimum {}",
                delta, min_delta
            )));
        }

        // Verify new voucher signature.
        let channel_id_bytes = Self::parse_hex_bytes(channel_id_str)?;
        let sig_bytes = Self::parse_hex_bytes(signature_str)?;
        let sig_array: [u8; 64] = sig_bytes.clone().try_into().map_err(|_| {
            VerificationError::invalid_payload("signature must be 64 bytes")
        })?;
        let pubkey: [u8; 32] = channel
            .authorized_signer_pubkey
            .clone()
            .try_into()
            .map_err(|_| VerificationError::invalid_payload("stored pubkey not 32 bytes"))?;

        let is_valid = verify_voucher(
            &channel_id_bytes,
            cumulative_amount,
            &sig_array,
            &pubkey,
            &channel.authorized_signer_pubkey,
        );
        if !is_valid {
            return Err(VerificationError::invalid_signature("invalid voucher signature"));
        }

        // Update store.
        let channel_id_owned = channel_id_str.to_string();
        let sig_vec = sig_bytes;
        let updated = self
            .store
            .update_channel(
                &channel_id_owned,
                Box::new(move |current| {
                    let state = current
                        .ok_or_else(|| VerificationError::channel_not_found("channel not found"))?;
                    if cumulative_amount > state.highest_voucher_amount {
                        Ok(Some(ChannelState {
                            highest_voucher_amount: cumulative_amount,
                            highest_voucher_signature: Some(sig_vec),
                            ..state
                        }))
                    } else {
                        Ok(Some(state))
                    }
                }),
            )
            .await?;

        let state =
            updated.ok_or_else(|| VerificationError::channel_not_found("channel not found"))?;
        mpp_info!(
            channel_id = %state.channel_id,
            cumulative_amount = cumulative_amount,
            delta = delta,
            "voucher accepted"
        );
        Ok(Receipt::success(METHOD_NAME, &state.channel_id))
    }
}

impl SessionMethodTrait for SessionMethod {
    fn method(&self) -> &str {
        METHOD_NAME
    }

    fn challenge_method_details(&self) -> Option<serde_json::Value> {
        let details = MovementSessionMethodDetails {
            module_address: self.config.module_address.clone(),
            registry_address: Some(self.config.registry_address.clone()),
            token_metadata: Some(self.config.token_metadata.clone()),
            channel_id: None,
            min_voucher_delta: if self.config.min_voucher_delta > 0 {
                Some(self.config.min_voucher_delta.to_string())
            } else {
                None
            },
            network: None,
        };
        serde_json::to_value(details).ok()
    }

    fn respond(
        &self,
        credential: &PaymentCredential,
        _receipt: &Receipt,
    ) -> Option<serde_json::Value> {
        let payload: SessionCredentialPayload = credential.payload_as().ok()?;
        match payload {
            SessionCredentialPayload::Voucher { .. } => None,
            _ => Some(serde_json::json!({ "status": "ok" })),
        }
    }

    fn verify_session(
        &self,
        credential: &PaymentCredential,
        request: &SessionRequest,
    ) -> impl Future<Output = Result<Receipt, VerificationError>> + Send {
        let credential = credential.clone();
        let request = request.clone();
        let store = Arc::clone(&self.store);
        let config = self.config.clone();

        async move {
            let this = SessionMethod { store, config };

            if credential.challenge.method.as_str() != METHOD_NAME {
                return Err(VerificationError::credential_mismatch(format!(
                    "Method mismatch: expected {}, got {}",
                    METHOD_NAME, credential.challenge.method
                )));
            }
            if credential.challenge.intent.as_str() != INTENT_SESSION {
                return Err(VerificationError::credential_mismatch(format!(
                    "Intent mismatch: expected {}, got {}",
                    INTENT_SESSION, credential.challenge.intent
                )));
            }

            let details = this.resolve_details(&request);

            let payload: SessionCredentialPayload = credential.payload_as().map_err(|e| {
                VerificationError::invalid_payload(format!("Expected session payload: {}", e))
            })?;

            match &payload {
                SessionCredentialPayload::Open { .. } => {
                    this.handle_open(&payload, &details).await
                }
                SessionCredentialPayload::TopUp { .. } => {
                    this.handle_top_up(&payload, &details).await
                }
                SessionCredentialPayload::Voucher { .. } => {
                    this.handle_voucher(&payload, &details).await
                }
                SessionCredentialPayload::Close { .. } => {
                    this.handle_close(&payload, &details).await
                }
            }
        }
    }
}

fn now_iso8601() -> String {
    use time::format_description::well_known::Iso8601;
    use time::OffsetDateTime;

    OffsetDateTime::now_utc()
        .format(&Iso8601::DEFAULT)
        .unwrap_or_else(|_| "1970-01-01T00:00:00Z".to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn test_channel_state(channel_id: &str) -> ChannelState {
        ChannelState {
            channel_id: channel_id.to_string(),
            module_address: super::super::DEFAULT_MODULE_ADDRESS.to_string(),
            registry_address: super::super::DEFAULT_MODULE_ADDRESS.to_string(),
            payer: "0x".to_string() + &"0a".repeat(32),
            payee: "0x".to_string() + &"0b".repeat(32),
            token: "0xa".to_string(),
            authorized_signer_pubkey: vec![0x0d; 32],
            deposit: 100_000,
            settled_on_chain: 0,
            highest_voucher_amount: 0,
            highest_voucher_signature: None,
            spent: 0,
            units: 0,
            finalized: false,
            created_at: "2026-01-01T00:00:00Z".to_string(),
        }
    }

    #[test]
    fn test_in_memory_store_insert_and_get() {
        let store = InMemoryChannelStore::new();
        let state = test_channel_state("0xabc");
        store.insert("0xabc", state);

        let retrieved = store.get_channel_sync("0xabc");
        assert!(retrieved.is_some());
        assert_eq!(retrieved.unwrap().deposit, 100_000);
        assert!(store.get_channel_sync("0xmissing").is_none());
    }

    #[tokio::test]
    async fn test_deduct_success() {
        let store = InMemoryChannelStore::new();
        let mut state = test_channel_state("0xabc");
        state.highest_voucher_amount = 10_000;
        store.insert("0xabc", state);

        let result = deduct_from_channel(&store, "0xabc", 3_000).await;
        assert!(result.is_ok());
        let updated = result.unwrap();
        assert_eq!(updated.spent, 3_000);
        assert_eq!(updated.units, 1);
    }

    #[tokio::test]
    async fn test_deduct_insufficient() {
        let store = InMemoryChannelStore::new();
        let mut state = test_channel_state("0xabc");
        state.highest_voucher_amount = 10_000;
        state.spent = 9_000;
        store.insert("0xabc", state);

        let result = deduct_from_channel(&store, "0xabc", 5_000).await;
        assert!(result.is_err());
    }

    #[tokio::test]
    async fn test_deduct_not_found() {
        let store = InMemoryChannelStore::new();
        let result = deduct_from_channel(&store, "0xmissing", 1_000).await;
        assert!(result.is_err());
    }

    #[test]
    fn test_channel_state_serialization() {
        let state = test_channel_state("0xabc");
        let json = serde_json::to_string(&state).unwrap();
        let deserialized: ChannelState = serde_json::from_str(&json).unwrap();
        assert_eq!(deserialized.channel_id, "0xabc");
        assert_eq!(deserialized.deposit, 100_000);
    }

    #[test]
    fn test_session_method_config_default() {
        let config = SessionMethodConfig::default();
        assert!(config.module_address.starts_with("0x"));
        assert_eq!(config.min_voucher_delta, 0);
    }
}

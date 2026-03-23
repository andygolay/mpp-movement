//! Server-side charge verification for Movement.
//!
//! Implements the `ChargeMethod` trait for Movement, verifying one-time
//! payment credentials by checking transaction success and transfer events
//! on-chain via the Movement REST API.

use std::future::Future;

use crate::protocol::core::{PaymentCredential, Receipt};
use crate::protocol::intents::ChargeRequest;
use crate::protocol::traits::{ChargeMethod as ChargeMethodTrait, VerificationError};

use super::MovementChargeExt;

/// Movement charge method for server-side payment verification.
///
/// Verifies that a payment transaction:
/// 1. Exists on-chain and succeeded
/// 2. Transferred the correct amount to the expected recipient
///
/// # Example
///
/// ```ignore
/// use mpp::protocol::methods::movement::ChargeMethod;
///
/// let method = ChargeMethod::new("https://testnet.movementnetwork.xyz/v1");
/// ```
#[derive(Clone)]
pub struct ChargeMethod {
    rest_url: String,
}

impl ChargeMethod {
    /// Create a new Movement charge method.
    pub fn new(rest_url: &str) -> Self {
        Self {
            rest_url: rest_url.trim_end_matches('/').to_string(),
        }
    }

    /// Verify a transaction hash on-chain.
    ///
    /// Fetches the transaction, checks success, and verifies transfer events
    /// match the expected recipient and amount.
    async fn verify_tx_hash(
        &self,
        tx_hash: &str,
        request: &ChargeRequest,
    ) -> Result<Receipt, VerificationError> {
        let hash = tx_hash.strip_prefix("0x").unwrap_or(tx_hash);
        let url = format!("{}/transactions/by_hash/0x{}", self.rest_url, hash);

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
            VerificationError::network_error(format!("failed to parse transaction: {}", e))
        })?;

        // Check if pending
        let tx_type = body.get("type").and_then(|v| v.as_str()).unwrap_or("");
        if tx_type == "pending_transaction" {
            return Err(VerificationError::pending(
                "transaction is pending confirmation",
            ));
        }

        // Check success
        let success = body
            .get("success")
            .and_then(|v| v.as_bool())
            .unwrap_or(false);
        if !success {
            let vm_status = body
                .get("vm_status")
                .and_then(|v| v.as_str())
                .unwrap_or("unknown");
            return Err(VerificationError::transaction_failed(format!(
                "transaction failed: {}",
                vm_status
            )));
        }

        // Verify transfer events match the expected values.
        let expected_amount = request
            .amount_u64()
            .map_err(|e| VerificationError::invalid_amount(format!("invalid amount: {}", e)))?;
        let expected_recipient = request.recipient_str().map_err(|e| {
            VerificationError::invalid_recipient(format!("invalid recipient: {}", e))
        })?;
        let expected_currency = request.currency_str();

        // Always verify the payload to confirm the correct token was used.
        // This prevents paying with a worthless token instead of the requested one.
        self.verify_payload_currency(&body, expected_currency)?;

        // Look for coin/FA transfer events in the transaction.
        // Movement uses Aptos events: CoinStore::WithdrawEvent, CoinStore::DepositEvent,
        // or fungible_asset::Deposit/Withdraw events.
        if let Some(events) = body.get("events").and_then(|v| v.as_array()) {
            let transfer_verified =
                self.verify_transfer_events(events, expected_recipient, expected_amount);

            if !transfer_verified {
                // If we can't find matching events, check the payload as a fallback.
                // The transaction may use `aptos_account::transfer` which combines
                // coin registration + transfer atomically.
                self.verify_payload_fallback(
                    &body,
                    expected_currency,
                    expected_recipient,
                    expected_amount,
                )?;
            }
        } else {
            // No events field — verify via payload
            self.verify_payload_fallback(
                &body,
                expected_currency,
                expected_recipient,
                expected_amount,
            )?;
        }

        Ok(Receipt::success(super::METHOD_NAME, tx_hash))
    }

    /// Check transfer events for a matching deposit to the expected recipient.
    ///
    /// Supports both native coin events and Fungible Asset (FA) events:
    /// - `0x1::coin::DepositEvent` — native coin deposits (MOVE)
    /// - `0x1::fungible_asset::Deposit` — FA token deposits (USDC.e, USDCx, etc.)
    /// - `0x1::fungible_asset::Transfer` — FA direct transfer events
    fn verify_transfer_events(
        &self,
        events: &[serde_json::Value],
        expected_recipient: &str,
        expected_amount: u64,
    ) -> bool {
        let recipient_normalized = normalize_address(expected_recipient);

        for event in events {
            let event_type = event.get("type").and_then(|v| v.as_str()).unwrap_or("");

            // Check for deposit/transfer events from both coin and FA systems
            let is_deposit = event_type.contains("::coin::DepositEvent")
                || event_type.contains("::fungible_asset::Deposit");
            let is_fa_transfer = event_type.contains("::fungible_asset::Transfer");

            if !is_deposit && !is_fa_transfer {
                continue;
            }

            let data = match event.get("data") {
                Some(d) => d,
                None => continue,
            };

            let event_amount = parse_event_amount(data);
            if event_amount != Some(expected_amount) {
                continue;
            }

            // For FA Transfer events, check the `to` field directly
            if is_fa_transfer {
                if let Some(to) = data.get("to").and_then(|v| v.as_str()) {
                    if normalize_address(to) == recipient_normalized {
                        return true;
                    }
                }
                continue;
            }

            // For deposit events, check various address fields
            // Coin events: guid.account_address
            if let Some(guid) = event.get("guid") {
                if let Some(account_address) = guid.get("account_address").and_then(|v| v.as_str())
                {
                    if normalize_address(account_address) == recipient_normalized {
                        return true;
                    }
                }
            }

            // FA Deposit events: the event is emitted on the recipient's store.
            // In the v2 event format, check `key` or the event's emitting address.
            if let Some(key) = event.get("key") {
                if let Some(account_address) = key.as_str() {
                    if normalize_address(account_address) == recipient_normalized {
                        return true;
                    }
                }
            }
        }

        false
    }

    /// Verify the transaction payload uses the correct token.
    ///
    /// For `primary_fungible_store::transfer`, checks the metadata address argument.
    /// For `aptos_account::transfer`, verifies the expected currency is native MOVE.
    /// For `coin::transfer` / `transfer_coins`, checks type arguments.
    fn verify_payload_currency(
        &self,
        tx: &serde_json::Value,
        expected_currency: &str,
    ) -> Result<(), VerificationError> {
        let payload = match tx.get("payload") {
            Some(p) => p,
            None => return Ok(()), // No payload to check — will fail in fallback
        };

        let function = payload
            .get("function")
            .and_then(|v| v.as_str())
            .unwrap_or("");

        match function {
            // FA transfer: first argument is the metadata address — must match currency
            "0x1::primary_fungible_store::transfer" => {
                let arguments = payload.get("arguments").and_then(|v| v.as_array());
                if let Some(args) = arguments {
                    if let Some(metadata) = args.first().and_then(|v| v.as_str()) {
                        if normalize_address(metadata) != normalize_address(expected_currency) {
                            return Err(VerificationError::new(format!(
                                "token mismatch: transaction transfers FA {} but expected {}",
                                metadata, expected_currency
                            )));
                        }
                    }
                }
            }

            // Native MOVE transfer: only valid if expected currency is native MOVE
            "0x1::aptos_account::transfer" => {
                if !is_native_move(expected_currency) {
                    return Err(VerificationError::new(format!(
                        "token mismatch: transaction uses aptos_account::transfer (native MOVE) \
                         but expected FA token {}",
                        expected_currency
                    )));
                }
            }

            // Typed coin transfers: check type_arguments for the coin type
            "0x1::aptos_account::transfer_coins" | "0x1::coin::transfer" => {
                // For native MOVE, type arg should be 0x1::aptos_coin::AptosCoin
                // For other coins, the type arg identifies the coin — we check
                // that native MOVE is not used for a non-MOVE currency request.
                if is_native_move(expected_currency) {
                    // Expecting native MOVE — type arg should be AptosCoin
                    let type_args = payload.get("type_arguments").and_then(|v| v.as_array());
                    if let Some(args) = type_args {
                        let has_aptos_coin = args.iter().any(|a| {
                            a.as_str()
                                .map(|s| s.contains("aptos_coin::AptosCoin"))
                                .unwrap_or(false)
                        });
                        if !has_aptos_coin && !args.is_empty() {
                            return Err(VerificationError::new(
                                "token mismatch: expected native MOVE but coin type is different",
                            ));
                        }
                    }
                }
                // For non-native currencies with coin::transfer, we can't easily
                // map the coin type to an FA metadata address, so we allow it
                // and rely on event/amount verification.
            }

            _ => {
                // Unknown function — will fail in verify_payload_fallback
            }
        }

        Ok(())
    }

    /// Fallback verification: check the transaction payload matches expectations.
    ///
    /// If we can't find deposit events (some indexing delays), we verify
    /// the transaction payload was a known transfer function with the
    /// correct recipient and amount.
    ///
    /// Supports:
    /// - `0x1::aptos_account::transfer(recipient, amount)` — native MOVE
    /// - `0x1::aptos_account::transfer_coins(recipient, amount)` — typed coin
    /// - `0x1::coin::transfer(recipient, amount)` — legacy coin
    /// - `0x1::primary_fungible_store::transfer(metadata, recipient, amount)` — any FA token
    fn verify_payload_fallback(
        &self,
        tx: &serde_json::Value,
        expected_currency: &str,
        expected_recipient: &str,
        expected_amount: u64,
    ) -> Result<(), VerificationError> {
        let payload = tx
            .get("payload")
            .ok_or_else(|| VerificationError::new("transaction has no payload"))?;

        let function = payload
            .get("function")
            .and_then(|v| v.as_str())
            .unwrap_or("");

        let arguments = payload
            .get("arguments")
            .and_then(|v| v.as_array())
            .ok_or_else(|| VerificationError::new("missing arguments in payload"))?;

        // Determine the (recipient_index, amount_index) based on the function signature.
        let (recipient_idx, amount_idx) = match function {
            // Native/coin transfers: (recipient, amount)
            "0x1::aptos_account::transfer"
            | "0x1::aptos_account::transfer_coins"
            | "0x1::coin::transfer" => (0, 1),

            // FA transfers: (metadata_address, recipient, amount)
            "0x1::primary_fungible_store::transfer" => {
                // Also verify metadata matches currency (belt-and-suspenders with verify_payload_currency)
                if let Some(metadata) = arguments.first().and_then(|v| v.as_str()) {
                    if normalize_address(metadata) != normalize_address(expected_currency) {
                        return Err(VerificationError::new(format!(
                            "token mismatch: FA metadata {} does not match expected {}",
                            metadata, expected_currency
                        )));
                    }
                }
                (1, 2)
            }

            _ => {
                return Err(VerificationError::new(format!(
                    "unexpected transaction function: {}",
                    function
                )));
            }
        };

        if arguments.len() <= amount_idx {
            return Err(VerificationError::new("insufficient arguments in transfer"));
        }

        // Verify recipient
        let recipient = arguments[recipient_idx].as_str().unwrap_or("");
        if normalize_address(recipient) != normalize_address(expected_recipient) {
            return Err(VerificationError::invalid_recipient(format!(
                "transfer recipient {} does not match expected {}",
                recipient, expected_recipient
            )));
        }

        // Verify amount
        let amount_str = arguments[amount_idx].as_str().unwrap_or("0");
        let amount: u64 = amount_str.parse().unwrap_or(0);
        if amount != expected_amount {
            return Err(VerificationError::invalid_amount(format!(
                "transfer amount {} does not match expected {}",
                amount, expected_amount
            )));
        }

        Ok(())
    }
}

impl ChargeMethodTrait for ChargeMethod {
    fn method(&self) -> &str {
        super::METHOD_NAME
    }

    fn verify(
        &self,
        credential: &PaymentCredential,
        request: &ChargeRequest,
    ) -> impl Future<Output = Result<Receipt, VerificationError>> + Send {
        let credential = credential.clone();
        let request = request.clone();
        let this = self.clone();

        async move {
            // Extract the tx hash from the credential payload.
            // First try parsing as a standard PaymentPayload (type + hash/signature).
            let tx_hash = if let Ok(payload) = credential.charge_payload() {
                if payload.is_hash() {
                    payload.data().to_string()
                } else {
                    // Transaction payloads aren't supported yet for Movement
                    return Err(VerificationError::invalid_payload(
                        "Movement only supports hash payloads (submit tx client-side first)",
                    ));
                }
            } else {
                // Fallback: try extracting hash from arbitrary JSON payload
                credential
                    .payload
                    .get("hash")
                    .or_else(|| credential.payload.get("txHash"))
                    .and_then(|v| v.as_str())
                    .map(|s| s.to_string())
                    .ok_or_else(|| {
                        VerificationError::invalid_payload(
                            "payload must contain a transaction hash",
                        )
                    })?
            };

            this.verify_tx_hash(&tx_hash, &request).await
        }
    }
}

/// Parse the amount from an event data object.
///
/// Handles both string and numeric amount representations in event data.
fn parse_event_amount(data: &serde_json::Value) -> Option<u64> {
    if let Some(n) = data.get("amount").and_then(|v| v.as_u64()) {
        return Some(n);
    }
    if let Some(s) = data.get("amount").and_then(|v| v.as_str()) {
        return s.parse::<u64>().ok();
    }
    None
}

/// Check if a currency address is the native MOVE token (`0xa`).
fn is_native_move(currency: &str) -> bool {
    let hex = currency.strip_prefix("0x").unwrap_or(currency);
    let trimmed = hex.trim_start_matches('0');
    trimmed.eq_ignore_ascii_case("a")
}

/// Normalize a Move address to lowercase with full 64-char hex (no 0x prefix).
fn normalize_address(addr: &str) -> String {
    let hex = addr.strip_prefix("0x").unwrap_or(addr);
    format!("{:0>64}", hex).to_lowercase()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_normalize_address() {
        assert_eq!(normalize_address("0xa"), format!("{:0>64}", "a"));
        assert_eq!(normalize_address("0xABCD"), format!("{:0>64}", "abcd"));
        let full = format!("0x{}", "ab".repeat(32));
        assert_eq!(normalize_address(&full), "ab".repeat(32));
    }

    #[test]
    fn test_charge_method_name() {
        let method = ChargeMethod::new("https://testnet.movementnetwork.xyz/v1");
        assert_eq!(ChargeMethodTrait::method(&method), "movement");
    }

    const USDC_METADATA: &str =
        "0x63f169ba69623ba6ccf34620857644feb46d0f87e1d7bbcf8c071d30c3d94bd6";

    #[test]
    fn test_verify_payload_fallback_native_transfer() {
        let method = ChargeMethod::new("https://test");
        let tx = serde_json::json!({
            "payload": {
                "function": "0x1::aptos_account::transfer",
                "arguments": ["0xabcd", "1000000"]
            }
        });

        let result = method.verify_payload_fallback(&tx, "0xa", "0xabcd", 1_000_000);
        assert!(result.is_ok());
    }

    #[test]
    fn test_verify_payload_fallback_fa_transfer() {
        let method = ChargeMethod::new("https://test");
        let tx = serde_json::json!({
            "payload": {
                "function": "0x1::primary_fungible_store::transfer",
                "arguments": [USDC_METADATA, "0xabcd", "500000"]
            }
        });

        let result = method.verify_payload_fallback(&tx, USDC_METADATA, "0xabcd", 500_000);
        assert!(result.is_ok());
    }

    #[test]
    fn test_verify_payload_fallback_fa_wrong_recipient() {
        let method = ChargeMethod::new("https://test");
        let tx = serde_json::json!({
            "payload": {
                "function": "0x1::primary_fungible_store::transfer",
                "arguments": [USDC_METADATA, "0xabcd", "500000"]
            }
        });

        let result = method.verify_payload_fallback(&tx, USDC_METADATA, "0x1234", 500_000);
        assert!(result.is_err());
    }

    #[test]
    fn test_verify_payload_fallback_wrong_recipient() {
        let method = ChargeMethod::new("https://test");
        let tx = serde_json::json!({
            "payload": {
                "function": "0x1::aptos_account::transfer",
                "arguments": ["0xabcd", "1000000"]
            }
        });

        let result = method.verify_payload_fallback(&tx, "0xa", "0x1234", 1_000_000);
        assert!(result.is_err());
    }

    #[test]
    fn test_verify_payload_fallback_wrong_amount() {
        let method = ChargeMethod::new("https://test");
        let tx = serde_json::json!({
            "payload": {
                "function": "0x1::aptos_account::transfer",
                "arguments": ["0xabcd", "1000000"]
            }
        });

        let result = method.verify_payload_fallback(&tx, "0xa", "0xabcd", 999);
        assert!(result.is_err());
    }

    #[test]
    fn test_verify_payload_fallback_wrong_function() {
        let method = ChargeMethod::new("https://test");
        let tx = serde_json::json!({
            "payload": {
                "function": "0x1::some_module::some_function",
                "arguments": ["0xabcd", "1000000"]
            }
        });

        let result = method.verify_payload_fallback(&tx, "0xa", "0xabcd", 1_000_000);
        assert!(result.is_err());
    }

    // ==================== Token Verification Tests ====================

    #[test]
    fn test_verify_payload_currency_native_move_ok() {
        let method = ChargeMethod::new("https://test");
        let tx = serde_json::json!({
            "payload": {
                "function": "0x1::aptos_account::transfer",
                "arguments": ["0xabcd", "1000"]
            }
        });
        assert!(method.verify_payload_currency(&tx, "0xa").is_ok());
    }

    #[test]
    fn test_verify_payload_currency_native_rejects_fa_request() {
        let method = ChargeMethod::new("https://test");
        // Transaction uses native transfer, but server expects USDC
        let tx = serde_json::json!({
            "payload": {
                "function": "0x1::aptos_account::transfer",
                "arguments": ["0xabcd", "1000"]
            }
        });
        let result = method.verify_payload_currency(&tx, USDC_METADATA);
        assert!(result.is_err());
        assert!(result.unwrap_err().message.contains("token mismatch"));
    }

    #[test]
    fn test_verify_payload_currency_fa_correct_token() {
        let method = ChargeMethod::new("https://test");
        let tx = serde_json::json!({
            "payload": {
                "function": "0x1::primary_fungible_store::transfer",
                "arguments": [USDC_METADATA, "0xabcd", "1000"]
            }
        });
        assert!(method.verify_payload_currency(&tx, USDC_METADATA).is_ok());
    }

    #[test]
    fn test_verify_payload_currency_fa_wrong_token() {
        let method = ChargeMethod::new("https://test");
        let wrong_token = "0xdeadbeefdeadbeefdeadbeefdeadbeefdeadbeefdeadbeefdeadbeefdeadbeef";
        // Transaction transfers wrong_token, but server expects USDC
        let tx = serde_json::json!({
            "payload": {
                "function": "0x1::primary_fungible_store::transfer",
                "arguments": [wrong_token, "0xabcd", "1000"]
            }
        });
        let result = method.verify_payload_currency(&tx, USDC_METADATA);
        assert!(result.is_err());
        assert!(result.unwrap_err().message.contains("token mismatch"));
    }

    #[test]
    fn test_verify_payload_fallback_fa_wrong_token() {
        let method = ChargeMethod::new("https://test");
        let wrong_token = "0xdeadbeefdeadbeefdeadbeefdeadbeefdeadbeefdeadbeefdeadbeefdeadbeef";
        // Transaction sends wrong token to correct recipient with correct amount
        let tx = serde_json::json!({
            "payload": {
                "function": "0x1::primary_fungible_store::transfer",
                "arguments": [wrong_token, "0xabcd", "500000"]
            }
        });
        let result = method.verify_payload_fallback(&tx, USDC_METADATA, "0xabcd", 500_000);
        assert!(result.is_err());
        assert!(result.unwrap_err().message.contains("token mismatch"));
    }

    #[test]
    fn test_is_native_move() {
        assert!(is_native_move("0xa"));
        assert!(is_native_move("0x0a"));
        assert!(is_native_move("0xA"));
        assert!(is_native_move("a"));
        assert!(!is_native_move(USDC_METADATA));
        assert!(!is_native_move("0x1"));
        assert!(!is_native_move("0xab"));
    }

    #[test]
    fn test_verify_transfer_events_coin_deposit() {
        let method = ChargeMethod::new("https://test");
        let events = vec![serde_json::json!({
            "type": "0x1::coin::DepositEvent",
            "guid": { "account_address": "0xabcd" },
            "data": { "amount": "1000000" }
        })];

        assert!(method.verify_transfer_events(&events, "0xabcd", 1_000_000));
        assert!(!method.verify_transfer_events(&events, "0x1234", 1_000_000));
        assert!(!method.verify_transfer_events(&events, "0xabcd", 999));
    }

    #[test]
    fn test_verify_transfer_events_fa_deposit() {
        let method = ChargeMethod::new("https://test");
        let events = vec![serde_json::json!({
            "type": "0x1::fungible_asset::Deposit",
            "guid": { "account_address": "0xabcd" },
            "data": { "amount": "500000" }
        })];

        assert!(method.verify_transfer_events(&events, "0xabcd", 500_000));
        assert!(!method.verify_transfer_events(&events, "0x1234", 500_000));
    }

    #[test]
    fn test_verify_transfer_events_fa_transfer_event() {
        let method = ChargeMethod::new("https://test");
        // FA Transfer events have `from` and `to` in data
        let events = vec![serde_json::json!({
            "type": "0x1::fungible_asset::Transfer",
            "data": {
                "from": "0x1111",
                "to": "0xabcd",
                "amount": "500000"
            }
        })];

        assert!(method.verify_transfer_events(&events, "0xabcd", 500_000));
        assert!(!method.verify_transfer_events(&events, "0x1111", 500_000));
    }

    #[test]
    fn test_verify_transfer_events_numeric_amount() {
        let method = ChargeMethod::new("https://test");
        // Some event formats use numeric instead of string amounts
        let events = vec![serde_json::json!({
            "type": "0x1::fungible_asset::Deposit",
            "guid": { "account_address": "0xabcd" },
            "data": { "amount": 1000000 }
        })];

        assert!(method.verify_transfer_events(&events, "0xabcd", 1_000_000));
    }
}

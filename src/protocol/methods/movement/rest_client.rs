//! Movement REST API client for transaction building, signing, and submission.
//!
//! Uses the Aptos-compatible REST API endpoints to avoid implementing
//! full BCS transaction serialization. The flow:
//!
//! 1. Build transaction as JSON
//! 2. POST `/v1/transactions/encode_submission` → get signing message bytes
//! 3. Sign with ed25519
//! 4. POST `/v1/transactions` with JSON + signature
//!
//! Requires the `client` feature (for reqwest).

use ed25519_dalek::{Signer, SigningKey};
use serde::{Deserialize, Serialize};

use crate::error::{MppError, Result};

/// A Movement REST API client.
#[derive(Debug, Clone)]
pub struct MovementRestClient {
    rest_url: String,
    http: reqwest::Client,
}

/// Account info from the REST API.
#[derive(Debug, Deserialize)]
pub struct AccountInfo {
    pub sequence_number: String,
    pub authentication_key: String,
}

/// Transaction submission response.
#[derive(Debug, Deserialize)]
pub struct PendingTransaction {
    pub hash: String,
}

/// Transaction status from the REST API.
#[derive(Debug, Deserialize)]
pub struct TransactionInfo {
    pub hash: String,
    pub success: bool,
    #[serde(default)]
    pub vm_status: String,
    #[serde(rename = "type")]
    pub tx_type: String,
}

/// An entry function payload for transaction building.
#[derive(Debug, Clone, Serialize)]
pub struct EntryFunctionPayload {
    #[serde(rename = "type")]
    pub payload_type: String,
    pub function: String,
    pub type_arguments: Vec<String>,
    pub arguments: Vec<serde_json::Value>,
}

impl EntryFunctionPayload {
    /// Create a new entry function payload.
    pub fn new(function: &str, args: Vec<serde_json::Value>) -> Self {
        Self {
            payload_type: "entry_function_payload".to_string(),
            function: function.to_string(),
            type_arguments: vec![],
            arguments: args,
        }
    }

    /// Add type arguments.
    pub fn with_type_arguments(mut self, type_args: Vec<String>) -> Self {
        self.type_arguments = type_args;
        self
    }
}

/// A JSON-encoded transaction request for the REST API.
#[derive(Debug, Serialize)]
pub struct TransactionRequest {
    pub sender: String,
    pub sequence_number: String,
    pub max_gas_amount: String,
    pub gas_unit_price: String,
    pub expiration_timestamp_secs: String,
    pub payload: EntryFunctionPayload,
}

/// A signed transaction ready for submission.
#[derive(Debug, Serialize)]
pub struct SignedTransactionRequest {
    #[serde(flatten)]
    pub request: TransactionRequest,
    pub signature: TransactionSignature,
}

#[derive(Debug, Serialize)]
pub struct TransactionSignature {
    #[serde(rename = "type")]
    pub sig_type: String,
    pub public_key: String,
    pub signature: String,
}

impl MovementRestClient {
    /// Create a new REST client.
    pub fn new(rest_url: &str) -> Self {
        Self {
            rest_url: rest_url.trim_end_matches('/').to_string(),
            http: reqwest::Client::new(),
        }
    }

    /// Get account info (sequence number).
    pub async fn get_account(&self, address: &str) -> Result<AccountInfo> {
        let url = format!("{}/accounts/{}", self.rest_url, address);
        let resp = self.http.get(&url).send().await.map_err(|e| {
            MppError::Http(format!("failed to get account: {}", e))
        })?;

        if !resp.status().is_success() {
            let text = resp.text().await.unwrap_or_default();
            return Err(MppError::Http(format!("get account failed: {}", text)));
        }

        resp.json().await.map_err(|e| {
            MppError::Http(format!("failed to parse account info: {}", e))
        })
    }

    /// Encode a transaction for signing.
    ///
    /// Returns the signing message bytes (already includes the prefix hash).
    pub async fn encode_submission(&self, request: &TransactionRequest) -> Result<Vec<u8>> {
        let url = format!("{}/transactions/encode_submission", self.rest_url);
        let resp = self
            .http
            .post(&url)
            .json(request)
            .send()
            .await
            .map_err(|e| {
                MppError::Http(format!("encode_submission failed: {}", e))
            })?;

        if !resp.status().is_success() {
            let text = resp.text().await.unwrap_or_default();
            return Err(MppError::Http(format!(
                "encode_submission error: {}",
                text
            )));
        }

        // Response is a hex-encoded string (with quotes and 0x prefix).
        let hex_str: String = resp.json().await.map_err(|e| {
            MppError::Http(format!("failed to parse encode_submission response: {}", e))
        })?;

        let hex_clean = hex_str.strip_prefix("0x").unwrap_or(&hex_str);
        hex::decode(hex_clean).map_err(|e| {
            MppError::Http(format!("invalid hex in encode_submission response: {}", e))
        })
    }

    /// Submit a signed transaction.
    pub async fn submit_transaction(
        &self,
        signed: &SignedTransactionRequest,
    ) -> Result<PendingTransaction> {
        let url = format!("{}/transactions", self.rest_url);
        let resp = self
            .http
            .post(&url)
            .json(signed)
            .send()
            .await
            .map_err(|e| {
                MppError::Http(format!("submit transaction failed: {}", e))
            })?;

        if !resp.status().is_success() {
            let text = resp.text().await.unwrap_or_default();
            return Err(MppError::Http(format!(
                "submit transaction error: {}",
                text
            )));
        }

        resp.json().await.map_err(|e| {
            MppError::Http(format!("failed to parse submission response: {}", e))
        })
    }

    /// Wait for a transaction to be confirmed.
    pub async fn wait_for_transaction(&self, hash: &str) -> Result<TransactionInfo> {
        let url = format!(
            "{}/transactions/wait_by_hash/{}",
            self.rest_url, hash
        );
        let resp = self.http.get(&url).send().await.map_err(|e| {
            MppError::Http(format!("wait_for_transaction failed: {}", e))
        })?;

        if !resp.status().is_success() {
            let text = resp.text().await.unwrap_or_default();
            return Err(MppError::Http(format!(
                "wait_for_transaction error: {}",
                text
            )));
        }

        resp.json().await.map_err(|e| {
            MppError::Http(format!("failed to parse transaction info: {}", e))
        })
    }

    /// Build, sign, and submit an entry function transaction.
    ///
    /// Returns the transaction hash.
    pub async fn build_sign_submit(
        &self,
        signing_key: &SigningKey,
        sender: &str,
        payload: EntryFunctionPayload,
    ) -> Result<String> {
        // Get sequence number.
        let account = self.get_account(sender).await?;

        // Build expiration (5 minutes from now).
        let expiration = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_secs()
            + 300;

        let request = TransactionRequest {
            sender: sender.to_string(),
            sequence_number: account.sequence_number,
            max_gas_amount: "200000".to_string(),
            gas_unit_price: "100".to_string(),
            expiration_timestamp_secs: expiration.to_string(),
            payload,
        };

        // Encode for signing.
        let signing_message = self.encode_submission(&request).await?;

        // Sign with ed25519.
        let signature = signing_key.sign(&signing_message);
        let pubkey = signing_key.verifying_key();

        let signed = SignedTransactionRequest {
            request,
            signature: TransactionSignature {
                sig_type: "ed25519_signature".to_string(),
                public_key: format!("0x{}", hex::encode(pubkey.to_bytes())),
                signature: format!("0x{}", hex::encode(signature.to_bytes())),
            },
        };

        // Submit.
        let pending = self.submit_transaction(&signed).await?;

        // Wait for confirmation.
        let tx_info = self.wait_for_transaction(&pending.hash).await?;
        if !tx_info.success {
            return Err(MppError::Http(format!(
                "transaction failed: {}",
                tx_info.vm_status
            )));
        }

        Ok(pending.hash)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_entry_function_payload_serialization() {
        let payload = EntryFunctionPayload::new(
            "0xabcd::channel::open",
            vec![
                serde_json::json!("0x1234"),
                serde_json::json!("0x5678"),
                serde_json::json!("0xa"),
                serde_json::json!("1000000"),
            ],
        );

        let json = serde_json::to_string(&payload).unwrap();
        assert!(json.contains("entry_function_payload"));
        assert!(json.contains("0xabcd::channel::open"));
    }

    #[test]
    fn test_transaction_request_serialization() {
        let request = TransactionRequest {
            sender: "0xabc".to_string(),
            sequence_number: "42".to_string(),
            max_gas_amount: "200000".to_string(),
            gas_unit_price: "100".to_string(),
            expiration_timestamp_secs: "1700000000".to_string(),
            payload: EntryFunctionPayload::new("0x1::coin::transfer", vec![]),
        };

        let json = serde_json::to_string(&request).unwrap();
        assert!(json.contains("\"sender\":\"0xabc\""));
        assert!(json.contains("\"sequence_number\":\"42\""));
    }

    #[test]
    fn test_signed_transaction_serialization() {
        let request = TransactionRequest {
            sender: "0xabc".to_string(),
            sequence_number: "0".to_string(),
            max_gas_amount: "200000".to_string(),
            gas_unit_price: "100".to_string(),
            expiration_timestamp_secs: "1700000000".to_string(),
            payload: EntryFunctionPayload::new("0x1::coin::transfer", vec![]),
        };

        let signed = SignedTransactionRequest {
            request,
            signature: TransactionSignature {
                sig_type: "ed25519_signature".to_string(),
                public_key: "0xdef".to_string(),
                signature: "0x123".to_string(),
            },
        };

        let json = serde_json::to_string(&signed).unwrap();
        assert!(json.contains("\"type\":\"ed25519_signature\""));
        assert!(json.contains("\"public_key\":\"0xdef\""));
        // Flattened fields from request should also be present.
        assert!(json.contains("\"sender\":\"0xabc\""));
    }
}

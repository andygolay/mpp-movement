//! Payment handler that binds method, realm, and secret_key.
//!
//! This module provides the [`Mpp`] struct which wraps a payment method
//! with server configuration for stateless challenge verification.
//!
//! # Example (simple API)
//!
//! ```ignore
//! use mpp::server::{Mpp, movement, MovementConfig};
//!
//! let mpp = Mpp::create_movement(movement(MovementConfig {
//!     recipient: "0x3e9e...",
//! }))?;
//!
//! let challenge = mpp.movement_charge("0.10")?;
//! ```

#[cfg(all(feature = "movement", feature = "server", feature = "client"))]
use crate::error::Result;
use crate::protocol::core::{PaymentCredential, Receipt};
use crate::protocol::intents::ChargeRequest;
use crate::protocol::traits::{ChargeMethod, VerificationError};

#[cfg(all(feature = "movement", feature = "server", feature = "client"))]
const SECRET_KEY_ENV_VAR: &str = "MPP_SECRET_KEY";
const DEFAULT_DECIMALS: u32 = 6;

/// Result of session verification, including optional management response.
#[derive(Debug)]
pub struct SessionVerifyResult {
    /// The payment receipt.
    pub receipt: Receipt,
    /// Optional management response body (for channel open/close/topUp).
    /// When `Some`, the caller should return this as the response body
    /// instead of proceeding with normal request handling.
    pub management_response: Option<serde_json::Value>,
}

/// Server-side payment handler.
///
/// Binds a payment method with realm, secret_key, and optionally
/// a default currency and recipient for simplified `charge()` calls.
///
/// # Simple API
///
/// ```ignore
/// use mpp::server::{Mpp, movement, MovementConfig};
///
/// let mpp = Mpp::create_movement(movement(MovementConfig {
///     recipient: "0x3e9e...",
/// }))?;
///
/// // Charge $0.10 — currency, recipient, realm, secret, expires all handled
/// let challenge = mpp.movement_charge("0.10")?;
/// ```
///
/// # Advanced API
///
/// ```ignore
/// use mpp::server::{Mpp, MovementChargeMethod};
///
/// let method = MovementChargeMethod::new("https://fullnode.testnet.movementnetwork.xyz/v1");
/// let payment = Mpp::new(method, "api.example.com", "my-server-secret");
///
/// let challenge = payment.movement_charge_challenge("1000000", "0x...", "0x...")?;
/// ```
#[derive(Clone)]
pub struct Mpp<M, S = ()> {
    method: M,
    session_method: Option<S>,
    realm: String,
    secret_key: String,
    currency: Option<String>,
    recipient: Option<String>,
    decimals: u32,
    fee_payer: bool,
    chain_id: Option<u64>,
}

impl<M> Mpp<M, ()>
where
    M: ChargeMethod,
{
    /// Create a new payment handler (advanced API).
    ///
    /// For a simpler API, use [`Mpp::create_movement()`] with [`movement()`](super::movement).
    pub fn new(method: M, realm: impl Into<String>, secret_key: impl Into<String>) -> Mpp<M, ()> {
        Mpp {
            method,
            session_method: None,
            realm: realm.into(),
            secret_key: secret_key.into(),
            currency: None,
            recipient: None,
            decimals: DEFAULT_DECIMALS,
            fee_payer: false,
            chain_id: None,
        }
    }
}

impl<M> Mpp<M, ()>
where
    M: ChargeMethod,
{
    /// Create a payment handler with bound currency/recipient for testing.
    #[cfg(test)]
    pub(crate) fn new_with_config(
        method: M,
        realm: impl Into<String>,
        secret_key: impl Into<String>,
        currency: impl Into<String>,
        recipient: impl Into<String>,
    ) -> Self {
        Mpp {
            method,
            session_method: None,
            realm: realm.into(),
            secret_key: secret_key.into(),
            currency: Some(currency.into()),
            recipient: Some(recipient.into()),
            decimals: DEFAULT_DECIMALS,
            fee_payer: false,
            chain_id: None,
        }
    }
}

impl<M, S> Mpp<M, S>
where
    M: ChargeMethod,
{
    /// Add a session method to this payment handler.
    pub fn with_session_method<S2>(self, session_method: S2) -> Mpp<M, S2> {
        Mpp {
            method: self.method,
            session_method: Some(session_method),
            realm: self.realm,
            secret_key: self.secret_key,
            currency: self.currency,
            recipient: self.recipient,
            decimals: self.decimals,
            fee_payer: self.fee_payer,
            chain_id: self.chain_id,
        }
    }

    /// Get the realm.
    pub fn realm(&self) -> &str {
        &self.realm
    }

    /// Get the method name.
    pub fn method_name(&self) -> &str {
        self.method.method()
    }

    /// Get the bound currency, if configured.
    pub fn currency(&self) -> Option<&str> {
        self.currency.as_deref()
    }

    /// Get the bound recipient, if configured.
    pub fn recipient(&self) -> Option<&str> {
        self.recipient.as_deref()
    }

    /// Get the configured decimals.
    pub fn decimals(&self) -> u32 {
        self.decimals
    }

    /// Verify the challenge HMAC and reject expired challenges.
    ///
    /// Shared validation used by both charge and session verification paths.
    fn verify_hmac_and_expiry(
        &self,
        credential: &PaymentCredential,
    ) -> std::result::Result<(), VerificationError> {
        let expected_id = crate::protocol::core::compute_challenge_id(
            &self.secret_key,
            &self.realm,
            credential.challenge.method.as_str(),
            credential.challenge.intent.as_str(),
            credential.challenge.request.raw(),
            credential.challenge.expires.as_deref(),
            credential.challenge.digest.as_deref(),
            credential.challenge.opaque.as_ref().map(|o| o.raw()),
        );

        if credential.challenge.id != expected_id {
            return Err(VerificationError::with_code(
                "Challenge ID mismatch - not issued by this server",
                crate::protocol::traits::ErrorCode::CredentialMismatch,
            ));
        }

        if let Some(ref expires) = credential.challenge.expires {
            if let Ok(expires_at) =
                time::OffsetDateTime::parse(expires, &time::format_description::well_known::Rfc3339)
            {
                if expires_at <= time::OffsetDateTime::now_utc() {
                    return Err(VerificationError::expired(format!(
                        "Challenge expired at {}",
                        expires
                    )));
                }
            } else {
                return Err(VerificationError::new(
                    "Invalid expires timestamp in challenge",
                ));
            }
        }

        Ok(())
    }

    /// Verify a payment credential (simple API).
    ///
    /// Decodes the charge request from the echoed challenge automatically.
    /// No need to reconstruct the request manually.
    pub async fn verify_credential(
        &self,
        credential: &PaymentCredential,
    ) -> std::result::Result<Receipt, VerificationError> {
        let request: ChargeRequest = credential
            .challenge
            .request
            .decode()
            .map_err(|e| VerificationError::new(format!("Failed to decode request: {}", e)))?;
        self.verify(credential, &request).await
    }

    /// Verify a payment credential, ensuring the charge request matches the server's expected values.
    ///
    /// This prevents cross-route credential replay attacks where a credential
    /// obtained from a cheaper endpoint (or different recipient/currency) is
    /// replayed on another.
    pub async fn verify_credential_with_expected_request(
        &self,
        credential: &PaymentCredential,
        expected: &ChargeRequest,
    ) -> std::result::Result<Receipt, VerificationError> {
        let request: ChargeRequest = credential
            .challenge
            .request
            .decode()
            .map_err(|e| VerificationError::new(format!("Failed to decode request: {}", e)))?;

        if request.amount != expected.amount {
            return Err(VerificationError::with_code(
                format!(
                    "Amount mismatch: credential has {} but endpoint expects {}",
                    request.amount, expected.amount
                ),
                crate::protocol::traits::ErrorCode::CredentialMismatch,
            ));
        }

        if request.currency != expected.currency {
            return Err(VerificationError::with_code(
                format!(
                    "Currency mismatch: credential has {} but endpoint expects {}",
                    request.currency, expected.currency
                ),
                crate::protocol::traits::ErrorCode::CredentialMismatch,
            ));
        }

        if request.recipient != expected.recipient {
            return Err(VerificationError::with_code(
                "Recipient mismatch: credential was issued for a different recipient",
                crate::protocol::traits::ErrorCode::CredentialMismatch,
            ));
        }

        self.verify(credential, &request).await
    }

    /// Verify a charge credential with an explicit request.
    pub async fn verify(
        &self,
        credential: &PaymentCredential,
        request: &ChargeRequest,
    ) -> std::result::Result<Receipt, VerificationError> {
        self.verify_hmac_and_expiry(credential)?;
        let receipt = self.method.verify(credential, request).await?;
        Ok(receipt)
    }
}

impl<M, S> Mpp<M, S>
where
    M: ChargeMethod,
    S: crate::protocol::traits::SessionMethod,
{
    /// Verify a session credential.
    pub async fn verify_session(
        &self,
        credential: &PaymentCredential,
    ) -> std::result::Result<SessionVerifyResult, crate::protocol::traits::VerificationError> {
        let session = self.session_method.as_ref().ok_or_else(|| {
            crate::protocol::traits::VerificationError::new("No session method configured")
        })?;

        self.verify_hmac_and_expiry(credential)?;

        let request: crate::protocol::intents::SessionRequest =
            credential.challenge.request.decode().map_err(|e| {
                crate::protocol::traits::VerificationError::new(format!(
                    "Failed to decode session request: {}",
                    e
                ))
            })?;

        let receipt = session.verify_session(credential, &request).await?;

        // Call respond hook — management actions (open, topUp, close) may
        // return a response body that short-circuits normal request handling.
        let management_response = session.respond(credential, &receipt);

        Ok(SessionVerifyResult {
            receipt,
            management_response,
        })
    }
}

/// Movement-specific `create_movement` constructor for [`Mpp`].
#[cfg(all(feature = "movement", feature = "server", feature = "client"))]
impl Mpp<super::MovementChargeMethod> {
    /// Create a payment handler from a [`MovementBuilder`](super::MovementBuilder).
    ///
    /// This is the simplest way to set up server-side Movement payments.
    /// Currency and recipient are bound at creation time, so
    /// [`movement_charge()`](Mpp::movement_charge) only needs the dollar amount.
    ///
    /// # Example
    ///
    /// ```ignore
    /// use mpp::server::{Mpp, movement, MovementConfig};
    ///
    /// let mpp = Mpp::create_movement(movement(MovementConfig {
    ///     recipient: "0x3e9e...",
    /// }))?;
    ///
    /// let challenge = mpp.movement_charge("0.10")?;
    /// ```
    pub fn create_movement(builder: super::MovementBuilder) -> Result<Self> {
        let secret_key = builder
            .secret_key
            .or_else(|| std::env::var(SECRET_KEY_ENV_VAR).ok())
            .and_then(|value| {
                if value.trim().is_empty() {
                    None
                } else {
                    Some(value)
                }
            })
            .ok_or_else(|| {
                crate::error::MppError::InvalidConfig(format!(
                    "Missing secret key. Set {} environment variable or pass .secret_key(...).",
                    SECRET_KEY_ENV_VAR
                ))
            })?;

        let method = crate::protocol::methods::movement::ChargeMethod::new(&builder.rest_url);

        Ok(Self {
            method,
            session_method: None,
            realm: builder.realm,
            secret_key,
            currency: Some(builder.currency),
            recipient: Some(builder.recipient),
            decimals: builder.decimals,
            fee_payer: false,
            chain_id: None,
        })
    }
}

/// Movement charge and session challenge methods.
///
/// These are available on any `Mpp<MovementChargeMethod, S>`, regardless of
/// whether a session method is attached.
#[cfg(all(feature = "movement", feature = "server", feature = "client"))]
impl<S> Mpp<super::MovementChargeMethod, S> {
    /// Generate a charge challenge for a dollar amount (Movement).
    ///
    /// Requires currency and recipient to be bound (via [`Mpp::create_movement()`]).
    /// The amount is automatically converted from dollars to base units.
    pub fn movement_charge(&self, amount: &str) -> Result<crate::protocol::core::PaymentChallenge> {
        self.movement_charge_with_options(amount, super::ChargeOptions::default())
    }

    /// Generate a charge challenge with options (Movement).
    pub fn movement_charge_with_options(
        &self,
        amount: &str,
        options: super::ChargeOptions<'_>,
    ) -> Result<crate::protocol::core::PaymentChallenge> {
        let (currency, recipient) = self.require_movement_config()?;
        let base_units = super::parse_dollar_amount(amount, self.decimals)?;
        let request = ChargeRequest {
            amount: base_units,
            currency: currency.to_string(),
            recipient: Some(recipient.to_string()),
            description: options.description.map(|s| s.to_string()),
            external_id: options.external_id.map(|s| s.to_string()),
            ..Default::default()
        };
        crate::protocol::methods::movement::charge_challenge_with_options(
            &self.secret_key,
            &self.realm,
            &request,
            options.expires,
            options.description,
        )
    }

    /// Generate a Movement charge challenge with explicit parameters (base units).
    pub fn movement_charge_challenge(
        &self,
        amount: &str,
        currency: &str,
        recipient: &str,
    ) -> Result<crate::protocol::core::PaymentChallenge> {
        crate::protocol::methods::movement::charge_challenge(
            &self.secret_key,
            &self.realm,
            amount,
            currency,
            recipient,
        )
    }

    /// Generate a session challenge for Movement (base units).
    ///
    /// The challenge includes session-specific method details like
    /// `moduleAddress`, `tokenMetadata`, and `suggestedDeposit`.
    pub fn movement_session_challenge(
        &self,
        amount_per_unit: &str,
        options: super::MovementSessionOptions<'_>,
    ) -> Result<crate::protocol::core::PaymentChallenge> {
        let (currency, recipient) = self.require_movement_config()?;

        let module_address = options
            .module_address
            .unwrap_or(crate::protocol::methods::movement::DEFAULT_MODULE_ADDRESS);

        let mut method_details = serde_json::json!({
            "moduleAddress": module_address,
            "registryAddress": options.registry_address.unwrap_or(module_address),
            "tokenMetadata": currency,
        });
        if let Some(delta) = options.min_voucher_delta {
            method_details["minVoucherDelta"] = serde_json::json!(delta);
        }

        let request = crate::protocol::intents::SessionRequest {
            amount: amount_per_unit.to_string(),
            unit_type: options.unit_type.map(|s| s.to_string()),
            currency: currency.to_string(),
            recipient: Some(recipient.to_string()),
            suggested_deposit: options.suggested_deposit.map(|s| s.to_string()),
            method_details: Some(method_details),
            ..Default::default()
        };
        let encoded = crate::protocol::core::Base64UrlJson::from_typed(&request)?;

        let expires_str;
        let expires = match options.expires {
            Some(e) => Some(e),
            None => {
                let expiry = time::OffsetDateTime::now_utc()
                    + time::Duration::minutes(
                        crate::protocol::methods::movement::DEFAULT_EXPIRES_MINUTES as i64,
                    );
                expires_str = expiry
                    .format(&time::format_description::well_known::Rfc3339)
                    .map_err(|e| {
                        crate::error::MppError::InvalidConfig(format!(
                            "failed to format expires: {e}"
                        ))
                    })?;
                Some(expires_str.as_str())
            }
        };

        let id = crate::protocol::core::compute_challenge_id(
            &self.secret_key,
            &self.realm,
            crate::protocol::methods::movement::METHOD_NAME,
            crate::protocol::methods::movement::INTENT_SESSION,
            encoded.raw(),
            expires,
            None,
            None,
        );

        Ok(crate::protocol::core::PaymentChallenge {
            id,
            realm: self.realm.clone(),
            method: crate::protocol::methods::movement::METHOD_NAME.into(),
            intent: crate::protocol::methods::movement::INTENT_SESSION.into(),
            request: encoded,
            expires: expires.map(|s| s.to_string()),
            description: options.description.map(|s| s.to_string()),
            digest: None,
            opaque: None,
        })
    }

    /// Generate a session challenge with a specific recipient (payee) address.
    ///
    /// Like [`movement_session_challenge`], but allows overriding the recipient
    /// per-call. Useful when the payee is a third party (e.g., a host in a
    /// voice call app) rather than the server itself.
    pub fn movement_session_challenge_with_recipient(
        &self,
        amount_per_unit: &str,
        recipient: &str,
        options: super::MovementSessionOptions<'_>,
    ) -> Result<crate::protocol::core::PaymentChallenge> {
        let (currency, _default_recipient) = self.require_movement_config()?;

        let module_address = options
            .module_address
            .unwrap_or(crate::protocol::methods::movement::DEFAULT_MODULE_ADDRESS);

        let mut method_details = serde_json::json!({
            "moduleAddress": module_address,
            "registryAddress": options.registry_address.unwrap_or(module_address),
            "tokenMetadata": currency,
        });
        if let Some(delta) = options.min_voucher_delta {
            method_details["minVoucherDelta"] = serde_json::json!(delta);
        }

        let request = crate::protocol::intents::SessionRequest {
            amount: amount_per_unit.to_string(),
            unit_type: options.unit_type.map(|s| s.to_string()),
            currency: currency.to_string(),
            recipient: Some(recipient.to_string()),
            suggested_deposit: options.suggested_deposit.map(|s| s.to_string()),
            method_details: Some(method_details),
            ..Default::default()
        };
        let encoded = crate::protocol::core::Base64UrlJson::from_typed(&request)?;

        let expires_str;
        let expires = match options.expires {
            Some(e) => Some(e),
            None => {
                let expiry = time::OffsetDateTime::now_utc()
                    + time::Duration::minutes(
                        crate::protocol::methods::movement::DEFAULT_EXPIRES_MINUTES as i64,
                    );
                expires_str = expiry
                    .format(&time::format_description::well_known::Rfc3339)
                    .map_err(|e| {
                        crate::error::MppError::InvalidConfig(format!(
                            "failed to format expires: {e}"
                        ))
                    })?;
                Some(expires_str.as_str())
            }
        };

        let id = crate::protocol::core::compute_challenge_id(
            &self.secret_key,
            &self.realm,
            crate::protocol::methods::movement::METHOD_NAME,
            crate::protocol::methods::movement::INTENT_SESSION,
            encoded.raw(),
            expires,
            None,
            None,
        );

        Ok(crate::protocol::core::PaymentChallenge {
            id,
            realm: self.realm.clone(),
            method: crate::protocol::methods::movement::METHOD_NAME.into(),
            intent: crate::protocol::methods::movement::INTENT_SESSION.into(),
            request: encoded,
            expires: expires.map(|s| s.to_string()),
            description: options.description.map(|s| s.to_string()),
            digest: None,
            opaque: None,
        })
    }

    fn require_movement_config(&self) -> Result<(&str, &str)> {
        let currency = self.currency.as_deref().ok_or_else(|| {
            crate::error::MppError::InvalidConfig(
                "currency not configured — use Mpp::create_movement() or set currency".into(),
            )
        })?;
        let recipient = self.recipient.as_deref().ok_or_else(|| {
            crate::error::MppError::InvalidConfig(
                "recipient not configured — use Mpp::create_movement() or set recipient".into(),
            )
        })?;
        Ok((currency, recipient))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::protocol::core::{ChallengeEcho, PaymentPayload};
    use crate::protocol::traits::ErrorCode;
    use std::future::Future;

    #[derive(Clone)]
    struct MockMethod;

    #[allow(clippy::manual_async_fn)]
    impl ChargeMethod for MockMethod {
        fn method(&self) -> &str {
            "mock"
        }

        fn verify(
            &self,
            _credential: &PaymentCredential,
            _request: &ChargeRequest,
        ) -> impl Future<Output = std::result::Result<Receipt, VerificationError>> + Send {
            async { Ok(Receipt::success("mock", "mock_ref")) }
        }
    }

    #[derive(Clone)]
    struct SuccessReceiptMethod;

    #[allow(clippy::manual_async_fn)]
    impl ChargeMethod for SuccessReceiptMethod {
        fn method(&self) -> &str {
            "mock"
        }

        fn verify(
            &self,
            _credential: &PaymentCredential,
            _request: &ChargeRequest,
        ) -> impl Future<Output = std::result::Result<Receipt, VerificationError>> + Send {
            async { Ok(Receipt::success("mock", "0xabc123")) }
        }
    }

    #[derive(Clone)]
    struct FailedTransactionMethod;

    #[allow(clippy::manual_async_fn)]
    impl ChargeMethod for FailedTransactionMethod {
        fn method(&self) -> &str {
            "mock"
        }

        fn verify(
            &self,
            _credential: &PaymentCredential,
            _request: &ChargeRequest,
        ) -> impl Future<Output = std::result::Result<Receipt, VerificationError>> + Send {
            async {
                Err(VerificationError::transaction_failed(
                    "Transaction reverted on-chain",
                ))
            }
        }
    }

    fn test_credential(secret_key: &str) -> PaymentCredential {
        let request = "eyJ0ZXN0IjoidmFsdWUifQ";
        let id = crate::protocol::core::compute_challenge_id(
            secret_key,
            "api.example.com",
            "mock",
            "charge",
            request,
            None,
            None,
            None,
        );

        let echo = ChallengeEcho {
            id,
            realm: "api.example.com".into(),
            method: "mock".into(),
            intent: "charge".into(),
            request: crate::protocol::core::Base64UrlJson::from_raw(request),
            expires: None,
            digest: None,
            opaque: None,
        };
        PaymentCredential::new(echo, PaymentPayload::hash("0x123"))
    }

    fn test_request() -> ChargeRequest {
        ChargeRequest {
            amount: "1000".into(),
            currency: "0x123".into(),
            recipient: Some("0x456".into()),
            ..Default::default()
        }
    }

    #[test]
    fn test_mpp_creation() {
        let payment = Mpp::new(MockMethod, "api.example.com", "secret");
        assert_eq!(payment.realm(), "api.example.com");
        assert_eq!(payment.method_name(), "mock");
        assert!(payment.currency().is_none());
        assert!(payment.recipient().is_none());
    }

    #[tokio::test]
    async fn test_verify_returns_receipt_for_success() {
        let payment = Mpp::new(SuccessReceiptMethod, "api.example.com", "secret");
        let credential = test_credential("secret");
        let request = test_request();

        let result = payment.verify(&credential, &request).await;

        assert!(result.is_ok());
        let receipt = result.unwrap();
        assert!(receipt.is_success());
        assert_eq!(receipt.reference, "0xabc123");
    }

    #[tokio::test]
    async fn test_verify_returns_error_for_failed_transaction() {
        use crate::error::{MppError, PaymentError};
        use crate::protocol::traits::ErrorCode;

        let payment = Mpp::new(FailedTransactionMethod, "api.example.com", "secret");
        let credential = test_credential("secret");
        let request = test_request();

        let result = payment.verify(&credential, &request).await;

        assert!(result.is_err());
        let err = result.unwrap_err();
        assert_eq!(err.code, Some(ErrorCode::TransactionFailed));
        assert!(err.message.contains("reverted"));

        let mpp_err: MppError = err.into();
        let problem = mpp_err.to_problem_details(None);
        assert_eq!(problem.status, 402);
    }

    #[tokio::test]
    async fn test_verify_credential_decodes_request() {
        let request = ChargeRequest {
            amount: "500000".into(),
            currency: "0x20c0000000000000000000000000000000000000".into(),
            recipient: Some("0x742d35Cc6634C0532925a3b844Bc9e7595f1B0F2".into()),
            ..Default::default()
        };
        let encoded = crate::protocol::core::Base64UrlJson::from_typed(&request).unwrap();
        let raw = encoded.raw().to_string();

        let secret = "test-secret";
        let id = crate::protocol::core::compute_challenge_id(
            secret,
            "api.example.com",
            "mock",
            "charge",
            &raw,
            None,
            None,
            None,
        );

        let echo = ChallengeEcho {
            id,
            realm: "api.example.com".into(),
            method: "mock".into(),
            intent: "charge".into(),
            request: crate::protocol::core::Base64UrlJson::from_raw(raw),
            expires: None,
            digest: None,
            opaque: None,
        };
        let credential = PaymentCredential::new(echo, PaymentPayload::hash("0x123"));

        let payment = Mpp::new(SuccessReceiptMethod, "api.example.com", secret);
        let receipt = payment.verify_credential(&credential).await.unwrap();
        assert!(receipt.is_success());
        assert_eq!(receipt.reference, "0xabc123");
    }

    // ── Mock SessionMethod for session verification tests ─────────────

    #[derive(Clone)]
    struct MockSessionMethod {
        receipt: Receipt,
        management_response: Option<serde_json::Value>,
    }

    impl MockSessionMethod {
        fn success() -> Self {
            Self {
                receipt: Receipt::success("movement", "0xsession_ref"),
                management_response: None,
            }
        }

        fn with_management_response(mut self, resp: serde_json::Value) -> Self {
            self.management_response = Some(resp);
            self
        }
    }

    impl crate::protocol::traits::SessionMethod for MockSessionMethod {
        fn method(&self) -> &str {
            "movement"
        }

        fn verify_session(
            &self,
            _credential: &PaymentCredential,
            _request: &crate::protocol::intents::SessionRequest,
        ) -> impl Future<Output = std::result::Result<Receipt, VerificationError>> + Send {
            let receipt = self.receipt.clone();
            async move { Ok(receipt) }
        }

        fn respond(
            &self,
            _credential: &PaymentCredential,
            _receipt: &Receipt,
        ) -> Option<serde_json::Value> {
            self.management_response.clone()
        }
    }

    // ── Mock SessionMethod that always returns an error ─────────────────

    #[derive(Clone)]
    struct MockFailingSessionMethod {
        error: VerificationError,
    }

    impl MockFailingSessionMethod {
        fn with_error(code: ErrorCode, message: &str) -> Self {
            Self {
                error: VerificationError::with_code(message, code),
            }
        }
    }

    impl crate::protocol::traits::SessionMethod for MockFailingSessionMethod {
        fn method(&self) -> &str {
            "movement"
        }

        fn verify_session(
            &self,
            _credential: &PaymentCredential,
            _request: &crate::protocol::intents::SessionRequest,
        ) -> impl Future<Output = std::result::Result<Receipt, VerificationError>> + Send {
            let error = self.error.clone();
            async move { Err(error) }
        }

        fn respond(
            &self,
            _credential: &PaymentCredential,
            _receipt: &Receipt,
        ) -> Option<serde_json::Value> {
            None
        }
    }

    #[test]
    fn test_session_verify_result_debug() {
        let result = SessionVerifyResult {
            receipt: Receipt::success("movement", "0xref"),
            management_response: Some(serde_json::json!({"status": "ok"})),
        };
        let debug = format!("{:?}", result);
        assert!(debug.contains("0xref"));
        assert!(debug.contains("status"));
    }
}

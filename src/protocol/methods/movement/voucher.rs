//! Voucher signing and channel ID computation for Movement session payments.
//!
//! Movement uses ed25519 signatures over BCS-serialized vouchers, matching
//! the on-chain MovementStreamChannel Move contract.

use ed25519_dalek::{Signature, Signer, SigningKey, Verifier, VerifyingKey};
use sha3::{Digest, Sha3_256};

/// BCS-serialize a voucher to match the on-chain Voucher struct.
///
/// ```move
/// struct Voucher has copy, drop {
///     channel_id: vector<u8>,
///     cumulative_amount: u64,
/// }
/// ```
///
/// BCS encoding:
/// - channel_id: ULEB128 length prefix + raw bytes
/// - cumulative_amount: 8 bytes little-endian
pub fn serialize_voucher(channel_id: &[u8], cumulative_amount: u64) -> Vec<u8> {
    let mut buf = Vec::new();

    // BCS vector<u8>: ULEB128 length + bytes
    bcs::serialize_uleb128(&mut buf, channel_id.len() as u64);
    buf.extend_from_slice(channel_id);

    // BCS u64: 8 bytes little-endian
    buf.extend_from_slice(&cumulative_amount.to_le_bytes());

    buf
}

/// Compute a channel ID from its parameters.
///
/// Mirrors the on-chain `compute_channel_id` function:
/// `sha3_256( bcs(payer) || bcs(payee) || bcs(token) || salt || authorized_signer_pubkey )`
///
/// where `bcs(address)` is the raw 32-byte address (fixed-size, no length prefix)
/// and `salt` / `authorized_signer_pubkey` are raw bytes (not BCS-wrapped).
pub fn compute_channel_id(
    payer: &[u8; 32],
    payee: &[u8; 32],
    token: &[u8; 32],
    salt: &[u8],
    authorized_signer_pubkey: &[u8],
) -> [u8; 32] {
    let mut hasher = Sha3_256::new();
    hasher.update(payer);
    hasher.update(payee);
    hasher.update(token);
    hasher.update(salt);
    hasher.update(authorized_signer_pubkey);
    hasher.finalize().into()
}

/// Sign a voucher with an ed25519 signing key.
///
/// Returns the 64-byte ed25519 signature.
pub fn sign_voucher(
    signing_key: &SigningKey,
    channel_id: &[u8],
    cumulative_amount: u64,
) -> [u8; 64] {
    let message = serialize_voucher(channel_id, cumulative_amount);
    let signature = signing_key.sign(&message);
    signature.to_bytes()
}

/// Verify a voucher signature against an ed25519 public key.
///
/// If `authorized_pubkey` is non-empty, `public_key_bytes` must match it
/// (mirroring the on-chain check).
pub fn verify_voucher(
    channel_id: &[u8],
    cumulative_amount: u64,
    signature_bytes: &[u8; 64],
    public_key_bytes: &[u8; 32],
    authorized_pubkey: &[u8],
) -> bool {
    // If channel has an authorized signer, the provided key must match.
    if !authorized_pubkey.is_empty() && public_key_bytes != authorized_pubkey {
        return false;
    }

    let verifying_key = match VerifyingKey::from_bytes(public_key_bytes) {
        Ok(vk) => vk,
        Err(_) => return false,
    };

    let signature = Signature::from_bytes(signature_bytes);
    let message = serialize_voucher(channel_id, cumulative_amount);

    verifying_key.verify(&message, &signature).is_ok()
}

/// Helper module for BCS ULEB128 encoding.
mod bcs {
    /// Encode a u64 as ULEB128 into a buffer.
    pub fn serialize_uleb128(buf: &mut Vec<u8>, mut value: u64) {
        loop {
            let mut byte = (value & 0x7F) as u8;
            value >>= 7;
            if value != 0 {
                byte |= 0x80;
            }
            buf.push(byte);
            if value == 0 {
                break;
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_serialize_voucher_format() {
        // channel_id = [0xAB; 32], cumulative_amount = 1000
        let channel_id = [0xAB_u8; 32];
        let amount = 1000u64;
        let serialized = serialize_voucher(&channel_id, amount);

        // ULEB128(32) = 0x20 (one byte), then 32 bytes, then 8 bytes LE
        assert_eq!(serialized.len(), 1 + 32 + 8);
        assert_eq!(serialized[0], 32); // length prefix
        assert_eq!(&serialized[1..33], &channel_id);
        assert_eq!(&serialized[33..41], &1000u64.to_le_bytes());
    }

    #[test]
    fn test_compute_channel_id_deterministic() {
        let payer = [0x0A_u8; 32];
        let payee = [0x0B_u8; 32];
        let token = [0x0C_u8; 32];
        let salt = b"test_salt";
        let pubkey = [0x0D_u8; 32];

        let id1 = compute_channel_id(&payer, &payee, &token, salt, &pubkey);
        let id2 = compute_channel_id(&payer, &payee, &token, salt, &pubkey);

        assert_eq!(id1, id2);
        assert_ne!(id1, [0u8; 32]);
    }

    #[test]
    fn test_compute_channel_id_differs_for_different_params() {
        let payer = [0x0A_u8; 32];
        let payee = [0x0B_u8; 32];
        let token = [0x0C_u8; 32];
        let pubkey = [0x0D_u8; 32];

        let id1 = compute_channel_id(&payer, &payee, &token, b"salt1", &pubkey);
        let id2 = compute_channel_id(&payer, &payee, &token, b"salt2", &pubkey);

        assert_ne!(id1, id2);
    }

    #[test]
    fn test_sign_verify_roundtrip() {
        let signing_key = SigningKey::from_bytes(&rand::random());
        let verifying_key = signing_key.verifying_key();
        let pubkey_bytes = verifying_key.to_bytes();

        let channel_id = [0xAB_u8; 32];
        let amount = 5000u64;

        let sig = sign_voucher(&signing_key, &channel_id, amount);

        assert!(verify_voucher(
            &channel_id,
            amount,
            &sig,
            &pubkey_bytes,
            &[], // no authorized pubkey check
        ));
    }

    #[test]
    fn test_verify_wrong_amount_fails() {
        let signing_key = SigningKey::from_bytes(&rand::random());
        let pubkey_bytes = signing_key.verifying_key().to_bytes();

        let channel_id = [0xAB_u8; 32];
        let sig = sign_voucher(&signing_key, &channel_id, 5000);

        assert!(!verify_voucher(
            &channel_id,
            9999, // wrong amount
            &sig,
            &pubkey_bytes,
            &[],
        ));
    }

    #[test]
    fn test_verify_wrong_key_fails() {
        let signing_key = SigningKey::from_bytes(&rand::random());
        let wrong_key = SigningKey::from_bytes(&rand::random());
        let wrong_pubkey = wrong_key.verifying_key().to_bytes();

        let channel_id = [0xAB_u8; 32];
        let sig = sign_voucher(&signing_key, &channel_id, 5000);

        assert!(!verify_voucher(&channel_id, 5000, &sig, &wrong_pubkey, &[],));
    }

    #[test]
    fn test_verify_authorized_pubkey_mismatch() {
        let signing_key = SigningKey::from_bytes(&rand::random());
        let pubkey_bytes = signing_key.verifying_key().to_bytes();
        let wrong_authorized = [0xFF_u8; 32];

        let channel_id = [0xAB_u8; 32];
        let sig = sign_voucher(&signing_key, &channel_id, 5000);

        // Signature is valid but pubkey doesn't match authorized key
        assert!(!verify_voucher(
            &channel_id,
            5000,
            &sig,
            &pubkey_bytes,
            &wrong_authorized,
        ));
    }

    #[test]
    fn test_verify_authorized_pubkey_match() {
        let signing_key = SigningKey::from_bytes(&rand::random());
        let pubkey_bytes = signing_key.verifying_key().to_bytes();

        let channel_id = [0xAB_u8; 32];
        let sig = sign_voucher(&signing_key, &channel_id, 5000);

        // Pubkey matches authorized key — should pass
        assert!(verify_voucher(
            &channel_id,
            5000,
            &sig,
            &pubkey_bytes,
            &pubkey_bytes, // authorized = actual
        ));
    }
}

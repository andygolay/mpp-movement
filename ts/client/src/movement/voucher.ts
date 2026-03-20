/**
 * Voucher signing and channel ID computation for Movement payment channels.
 *
 * Lifted from examples/token-stream/src/lib/voucher.ts and extended
 * with verification support. Matches the Rust implementation in
 * src/protocol/methods/movement/voucher.rs.
 */

import { ed25519 } from "@noble/curves/ed25519";
import { sha3_256 } from "@noble/hashes/sha3";

export interface Voucher {
  channelId: Uint8Array;
  cumulativeAmount: bigint;
}

/**
 * BCS-serialize a voucher to match the on-chain Voucher struct.
 * struct Voucher { channel_id: vector<u8>, cumulative_amount: u64 }
 */
export function serializeVoucher(voucher: Voucher): Uint8Array {
  const channelId = voucher.channelId;
  // BCS vector<u8>: ULEB128 length prefix + raw bytes
  const lenBytes = uleb128(channelId.length);
  // BCS u64: 8 bytes little-endian
  const amountBytes = new Uint8Array(8);
  const view = new DataView(amountBytes.buffer);
  view.setBigUint64(0, voucher.cumulativeAmount, true);

  const result = new Uint8Array(lenBytes.length + channelId.length + 8);
  let offset = 0;
  result.set(lenBytes, offset);
  offset += lenBytes.length;
  result.set(channelId, offset);
  offset += channelId.length;
  result.set(amountBytes, offset);
  return result;
}

function uleb128(value: number): Uint8Array {
  const bytes: number[] = [];
  let v = value;
  do {
    let byte = v & 0x7f;
    v >>= 7;
    if (v !== 0) byte |= 0x80;
    bytes.push(byte);
  } while (v !== 0);
  return new Uint8Array(bytes);
}

/** Sign a voucher with an ed25519 private key. Returns 64-byte signature. */
export function signVoucher(voucher: Voucher, privateKey: Uint8Array): Uint8Array {
  const message = serializeVoucher(voucher);
  return ed25519.sign(message, privateKey);
}

/** Verify a voucher signature against a public key. */
export function verifyVoucher(
  voucher: Voucher,
  signature: Uint8Array,
  publicKey: Uint8Array,
): boolean {
  const message = serializeVoucher(voucher);
  try {
    return ed25519.verify(signature, message, publicKey);
  } catch {
    return false;
  }
}

/** Get the ed25519 public key from a private key. */
export function getPublicKey(privateKey: Uint8Array): Uint8Array {
  return ed25519.getPublicKey(privateKey);
}

/**
 * Compute channel ID matching on-chain:
 * sha3_256(payer || payee || token || salt || authorized_signer_pubkey)
 *
 * All address inputs should be 32-byte Uint8Arrays (left-padded if needed).
 */
export function computeChannelId(
  payer: Uint8Array,
  payee: Uint8Array,
  token: Uint8Array,
  salt: Uint8Array,
  authorizedSignerPubkey: Uint8Array,
): Uint8Array {
  const totalLen =
    payer.length + payee.length + token.length + salt.length + authorizedSignerPubkey.length;
  const data = new Uint8Array(totalLen);
  let offset = 0;
  data.set(payer, offset);
  offset += payer.length;
  data.set(payee, offset);
  offset += payee.length;
  data.set(token, offset);
  offset += token.length;
  data.set(salt, offset);
  offset += salt.length;
  data.set(authorizedSignerPubkey, offset);
  return sha3_256(data);
}

/** Generate a random 32-byte salt. */
export function randomSalt(): Uint8Array {
  return crypto.getRandomValues(new Uint8Array(32));
}

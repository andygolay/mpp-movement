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
function serializeVoucher(voucher: Voucher): Uint8Array {
  const channelId = voucher.channelId;
  // BCS vector<u8>: ULEB128 length prefix + raw bytes
  const lenBytes = uleb128(channelId.length);
  // BCS u64: 8 bytes little-endian
  const amountBytes = new Uint8Array(8);
  const view = new DataView(amountBytes.buffer);
  view.setBigUint64(0, voucher.cumulativeAmount, true);

  const result = new Uint8Array(
    lenBytes.length + channelId.length + 8,
  );
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

export function signVoucher(
  voucher: Voucher,
  privateKey: Uint8Array,
): Uint8Array {
  const message = serializeVoucher(voucher);
  return ed25519.sign(message, privateKey);
}

export function getPublicKey(privateKey: Uint8Array): Uint8Array {
  return ed25519.getPublicKey(privateKey);
}

/**
 * Compute channel ID matching on-chain:
 * sha3_256( bcs(payer) || bcs(payee) || bcs(token) || salt || authorized_signer_pubkey )
 */
export function computeChannelId(
  payer: Uint8Array,
  payee: Uint8Array,
  token: Uint8Array,
  salt: Uint8Array,
  authorizedSignerPubkey: Uint8Array,
): Uint8Array {
  const data = new Uint8Array(
    payer.length +
      payee.length +
      token.length +
      salt.length +
      authorizedSignerPubkey.length,
  );
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

export function randomSalt(): Uint8Array {
  return crypto.getRandomValues(new Uint8Array(32));
}

export function toHex(bytes: Uint8Array): string {
  return (
    "0x" +
    Array.from(bytes)
      .map((b) => b.toString(16).padStart(2, "0"))
      .join("")
  );
}

export function hexToBytes(hex: string): Uint8Array {
  const clean = hex.startsWith("0x") ? hex.slice(2) : hex;
  const padded = clean.padStart(64, "0");
  const bytes = new Uint8Array(padded.length / 2);
  for (let i = 0; i < bytes.length; i++) {
    bytes[i] = parseInt(padded.slice(i * 2, i * 2 + 2), 16);
  }
  return bytes;
}

/**
 * Movement/Aptos address utilities.
 */

import { sha3_256 } from "@noble/hashes/sha3";

/**
 * Derive a Movement account address from an ed25519 public key.
 *
 * Matches the Rust implementation: sha3_256(pubkey_bytes || 0x00)
 * where 0x00 is the Ed25519 single-key authentication scheme byte.
 */
export function deriveAddress(publicKey: Uint8Array): Uint8Array {
  const data = new Uint8Array(publicKey.length + 1);
  data.set(publicKey);
  data[publicKey.length] = 0x00; // Ed25519 scheme byte
  return sha3_256(data);
}

/** Convert bytes to 0x-prefixed hex string. */
export function toHex(bytes: Uint8Array): string {
  return (
    "0x" +
    Array.from(bytes)
      .map((b) => b.toString(16).padStart(2, "0"))
      .join("")
  );
}

/** Convert a hex string (with or without 0x prefix) to 32-byte Uint8Array (left-padded). */
export function hexToBytes(hex: string): Uint8Array {
  const clean = hex.startsWith("0x") ? hex.slice(2) : hex;
  const padded = clean.padStart(64, "0");
  const bytes = new Uint8Array(padded.length / 2);
  for (let i = 0; i < bytes.length; i++) {
    bytes[i] = parseInt(padded.slice(i * 2, i * 2 + 2), 16);
  }
  return bytes;
}

/** Check if an address is the native MOVE token (0xa). */
export function isNativeMove(address: string): boolean {
  const clean = address.startsWith("0x") ? address.slice(2) : address;
  return /^0*a$/i.test(clean);
}

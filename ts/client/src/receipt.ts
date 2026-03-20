/**
 * Payment-Receipt header parsing and formatting.
 */

import type { Receipt } from "./types.js";
import * as base64url from "./base64url.js";

const MAX_TOKEN_LEN = 16 * 1024;

/**
 * Parse a Payment-Receipt header into a Receipt.
 *
 * Format: `<base64url-json>`
 */
export function parseReceipt(header: string): Receipt {
  const token = header.trim();
  if (token.length > MAX_TOKEN_LEN) {
    throw new Error(`Receipt exceeds maximum length of ${MAX_TOKEN_LEN} bytes`);
  }
  const decoded = base64url.decodeString(token);
  return JSON.parse(decoded) as Receipt;
}

/**
 * Format a Receipt as a Payment-Receipt header value.
 *
 * Format: `<base64url-json>`
 */
export function formatReceipt(receipt: Receipt): string {
  const json = JSON.stringify(receipt);
  return base64url.encodeString(json);
}

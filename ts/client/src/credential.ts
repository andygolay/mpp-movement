/**
 * Authorization header parsing and formatting for Payment credentials.
 *
 * Ports the Rust logic from src/protocol/core/headers.rs.
 */

import type { PaymentCredential, PaymentChallenge, ChallengeEcho } from "./types.js";
import * as base64url from "./base64url.js";
import { extractPaymentScheme } from "./challenge.js";

const MAX_TOKEN_LEN = 16 * 1024;

/**
 * Parse an Authorization header into a PaymentCredential.
 *
 * Format: `Payment <base64url-json>`
 */
export function parseAuthorization(header: string): PaymentCredential {
  const payment = extractPaymentScheme(header);
  if (!payment) throw new Error("Expected 'Payment' scheme");

  const token = payment.substring(8).trim();
  if (token.length > MAX_TOKEN_LEN) {
    throw new Error(`Token exceeds maximum length of ${MAX_TOKEN_LEN} bytes`);
  }

  const decoded = base64url.decodeString(token);
  return JSON.parse(decoded) as PaymentCredential;
}

/**
 * Format a PaymentCredential as an Authorization header value.
 *
 * Format: `Payment <base64url-json>`
 */
export function formatAuthorization(credential: PaymentCredential): string {
  const json = JSON.stringify(credential);
  const encoded = base64url.encodeString(json);
  return `Payment ${encoded}`;
}

/**
 * Build a ChallengeEcho from a PaymentChallenge.
 */
export function challengeToEcho(challenge: PaymentChallenge): ChallengeEcho {
  const echo: ChallengeEcho = {
    id: challenge.id,
    realm: challenge.realm,
    method: challenge.method,
    intent: challenge.intent,
    request: challenge.request,
  };
  if (challenge.expires) echo.expires = challenge.expires;
  if (challenge.digest) echo.digest = challenge.digest;
  if (challenge.opaque) echo.opaque = challenge.opaque;
  return echo;
}

/**
 * Build a PaymentCredential for a charge intent with a transaction hash.
 */
export function chargeCredential(
  challenge: PaymentChallenge,
  txHash: string,
  source?: string,
): PaymentCredential {
  const credential: PaymentCredential = {
    challenge: challengeToEcho(challenge),
    payload: { type: "hash", hash: txHash },
  };
  if (source) credential.source = source;
  return credential;
}

/**
 * Build a PaymentCredential for a session intent.
 */
export function sessionCredential(
  challenge: PaymentChallenge,
  payload: Record<string, unknown>,
  source?: string,
): PaymentCredential {
  const credential: PaymentCredential = {
    challenge: challengeToEcho(challenge),
    payload,
  };
  if (source) credential.source = source;
  return credential;
}

/**
 * Core protocol types for the Machine Payments Protocol (MPP).
 *
 * These mirror the Rust types in src/protocol/core/challenge.rs and types.rs.
 */

/** Payment challenge from server (parsed from WWW-Authenticate header). */
export interface PaymentChallenge {
  id: string;
  realm: string;
  method: string;
  intent: "charge" | "session" | string;
  /** Base64url-encoded JSON request data. */
  request: string;
  expires?: string;
  description?: string;
  digest?: string;
  /** Base64url-encoded JSON opaque correlation data. */
  opaque?: string;
}

/** Challenge echo in credential (echoes server challenge parameters). */
export interface ChallengeEcho {
  id: string;
  realm: string;
  method: string;
  intent: string;
  request: string;
  expires?: string;
  digest?: string;
  opaque?: string;
}

/** Payment credential from client (sent in Authorization header). */
export interface PaymentCredential {
  challenge: ChallengeEcho;
  source?: string;
  payload: unknown;
}

/** Payment payload for charge intents. */
export interface PaymentPayload {
  type: "transaction" | "hash";
  signature?: string;
  hash?: string;
}

/** Payment receipt from server (parsed from Payment-Receipt header). */
export interface Receipt {
  status: "success";
  method: string;
  timestamp: string;
  reference: string;
}

/** Decoded charge request from challenge.request. */
export interface ChargeRequest {
  amount: string;
  currency: string;
  decimals?: number;
  recipient?: string;
  description?: string;
  externalId?: string;
  methodDetails?: Record<string, unknown>;
}

/** Decoded session request from challenge.request. */
export interface SessionRequest {
  amount: string;
  unitType?: string;
  currency: string;
  decimals?: number;
  recipient?: string;
  suggestedDeposit?: string;
  methodDetails?: Record<string, unknown>;
}

/** Provider interface for handling 402 payment challenges. */
export interface PaymentProvider {
  /** Check if this provider supports the given method and intent. */
  supports(method: string, intent: string): boolean;

  /** Handle a 402 challenge and return a credential. */
  pay(challenge: PaymentChallenge): Promise<PaymentCredential>;
}

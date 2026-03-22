/**
 * Minimal MPP client helpers for the browser.
 * Handles parsing 402 challenges and building credentials.
 */

// --- Base64url ---

function base64urlEncode(str: string): string {
  const bytes = new TextEncoder().encode(str);
  const binString = Array.from(bytes, (b) => String.fromCharCode(b)).join("");
  return btoa(binString).replace(/\+/g, "-").replace(/\//g, "_").replace(/=+$/, "");
}

function base64urlDecode(encoded: string): string {
  let base64 = encoded.replace(/-/g, "+").replace(/_/g, "/");
  while (base64.length % 4) base64 += "=";
  const binString = atob(base64);
  const bytes = Uint8Array.from(binString, (c) => c.charCodeAt(0));
  return new TextDecoder().decode(bytes);
}

// --- Types ---

export interface PaymentChallenge {
  id: string;
  realm: string;
  method: string;
  intent: string;
  request: string;
  expires?: string;
  description?: string;
  opaque?: string;
}

export interface ChargeRequest {
  amount: string;
  currency: string;
  decimals?: number;
  recipient?: string;
  description?: string;
}

// --- Parse WWW-Authenticate ---

export function parseWwwAuthenticate(header: string): PaymentChallenge {
  if (!header.toLowerCase().startsWith("payment ")) {
    throw new Error("Expected 'Payment' scheme");
  }
  const rest = header.substring(8);
  const params = new Map<string, string>();

  // Simple key="value" parser
  const regex = /(\w+)="([^"\\]*(?:\\.[^"\\]*)*)"/g;
  let match;
  while ((match = regex.exec(rest)) !== null) {
    params.set(match[1], match[2].replace(/\\(.)/g, "$1"));
  }

  const id = params.get("id");
  const realm = params.get("realm");
  const method = params.get("method");
  const intent = params.get("intent");
  const request = params.get("request");

  if (!id || !realm || !method || !intent || !request) {
    throw new Error("Missing required challenge fields");
  }

  const challenge: PaymentChallenge = { id, realm, method, intent, request };
  if (params.has("expires")) challenge.expires = params.get("expires");
  if (params.has("description")) challenge.description = params.get("description");
  if (params.has("opaque")) challenge.opaque = params.get("opaque");
  return challenge;
}

export function decodeChargeRequest(challenge: PaymentChallenge): ChargeRequest {
  const json = base64urlDecode(challenge.request);
  return JSON.parse(json);
}

// --- Build Authorization header ---

export function buildAuthorizationHeader(
  challenge: PaymentChallenge,
  txHash: string,
  senderAddress: string,
): string {
  const credential = {
    challenge: {
      id: challenge.id,
      realm: challenge.realm,
      method: challenge.method,
      intent: challenge.intent,
      request: challenge.request,
      ...(challenge.expires ? { expires: challenge.expires } : {}),
      ...(challenge.opaque ? { opaque: challenge.opaque } : {}),
    },
    source: `did:movement:${senderAddress}`,
    payload: { type: "hash", hash: txHash },
  };

  const encoded = base64urlEncode(JSON.stringify(credential));
  return `Payment ${encoded}`;
}

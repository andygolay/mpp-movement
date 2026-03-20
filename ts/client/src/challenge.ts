/**
 * WWW-Authenticate header parsing and formatting.
 *
 * Ports the Rust parser from src/protocol/core/headers.rs.
 */

import type { PaymentChallenge } from "./types.js";
import * as base64url from "./base64url.js";

/**
 * Parse key="value" pairs from an auth-param string.
 * Handles quoted strings with escaped quotes, comma/space separated.
 */
function parseAuthParams(input: string): Map<string, string> {
  const params = new Map<string, string>();
  const chars = [...input];
  let i = 0;

  while (i < chars.length) {
    // Skip whitespace and commas
    while (i < chars.length && (chars[i] === " " || chars[i] === "\t" || chars[i] === ",")) i++;
    if (i >= chars.length) break;

    // Read key
    const keyStart = i;
    while (i < chars.length && chars[i] !== "=" && chars[i] !== " " && chars[i] !== "\t") i++;
    if (i >= chars.length || chars[i] !== "=") {
      // Skip non-key=value tokens
      while (i < chars.length && chars[i] !== " " && chars[i] !== "\t" && chars[i] !== ",") i++;
      continue;
    }

    const key = chars.slice(keyStart, i).join("");
    i++; // skip '='
    if (i >= chars.length) break;

    let value: string;
    if (chars[i] === '"') {
      // Quoted string
      i++;
      let v = "";
      while (i < chars.length && chars[i] !== '"') {
        if (chars[i] === "\\" && i + 1 < chars.length) {
          i++;
          v += chars[i];
        } else {
          v += chars[i];
        }
        i++;
      }
      if (i < chars.length) i++; // skip closing '"'
      value = v;
    } else {
      // Unquoted value
      const valueStart = i;
      while (i < chars.length && chars[i] !== " " && chars[i] !== "\t" && chars[i] !== ",") i++;
      value = chars.slice(valueStart, i).join("");
    }

    if (params.has(key)) {
      throw new Error(`Duplicate parameter: ${key}`);
    }
    params.set(key, value);
  }

  return params;
}

/**
 * Parse a single WWW-Authenticate header into a PaymentChallenge.
 *
 * Format: `Payment id="<id>", realm="<realm>", method="<method>", intent="<intent>", request="<base64url-json>"`
 */
export function parseWwwAuthenticate(header: string): PaymentChallenge {
  const trimmed = header.trimStart();
  if (!trimmed.substring(0, 8).toLowerCase().startsWith("payment ")) {
    throw new Error("Expected 'Payment' scheme");
  }
  const rest = trimmed.substring(8).trimStart();
  const params = parseAuthParams(rest);

  const id = params.get("id");
  if (!id) throw new Error("Missing 'id' field");
  if (id === "") throw new Error("Empty 'id' parameter");

  const realm = params.get("realm");
  if (!realm) throw new Error("Missing 'realm' field");

  const method = params.get("method");
  if (!method) throw new Error("Missing 'method' field");
  if (method === "" || !/^[a-z]+$/.test(method)) {
    throw new Error(`Invalid method: "${method}". Must match method-name ABNF.`);
  }

  const intent = params.get("intent");
  if (!intent) throw new Error("Missing 'intent' field");

  const request = params.get("request");
  if (!request) throw new Error("Missing 'request' field");

  // Validate request is valid base64url JSON
  try {
    const decoded = base64url.decodeString(request);
    JSON.parse(decoded);
  } catch {
    throw new Error("Invalid JSON in request field");
  }

  const challenge: PaymentChallenge = { id, realm, method, intent, request };

  const expires = params.get("expires");
  if (expires) challenge.expires = expires;

  const description = params.get("description");
  if (description) challenge.description = description;

  const digest = params.get("digest");
  if (digest) {
    if (!digest.startsWith("sha-256=")) throw new Error("Invalid digest format");
    challenge.digest = digest;
  }

  const opaque = params.get("opaque");
  if (opaque) challenge.opaque = opaque;

  return challenge;
}

/**
 * Parse all WWW-Authenticate headers that use the Payment scheme.
 * Non-Payment headers are skipped.
 */
export function parseWwwAuthenticateAll(headers: string[]): PaymentChallenge[] {
  return headers
    .filter((h) => h.trimStart().substring(0, 8).toLowerCase().startsWith("payment "))
    .map(parseWwwAuthenticate);
}

/** Escape a string for use in a quoted-string header value. */
function escapeQuotedValue(s: string): string {
  if (s.includes("\r") || s.includes("\n")) {
    throw new Error("Header value contains invalid CRLF characters");
  }
  return s.replace(/\\/g, "\\\\").replace(/"/g, '\\"');
}

/** Format a PaymentChallenge as a WWW-Authenticate header value. */
export function formatWwwAuthenticate(challenge: PaymentChallenge): string {
  const parts = [
    `id="${escapeQuotedValue(challenge.id)}"`,
    `realm="${escapeQuotedValue(challenge.realm)}"`,
    `method="${escapeQuotedValue(challenge.method)}"`,
    `intent="${escapeQuotedValue(challenge.intent)}"`,
    `request="${escapeQuotedValue(challenge.request)}"`,
  ];

  if (challenge.expires) parts.push(`expires="${escapeQuotedValue(challenge.expires)}"`);
  if (challenge.description) parts.push(`description="${escapeQuotedValue(challenge.description)}"`);
  if (challenge.digest) parts.push(`digest="${escapeQuotedValue(challenge.digest)}"`);
  if (challenge.opaque) parts.push(`opaque="${escapeQuotedValue(challenge.opaque)}"`);

  return `Payment ${parts.join(", ")}`;
}

/**
 * Extract the Payment scheme from an Authorization header that may contain
 * multiple comma-separated schemes (per RFC 9110).
 */
export function extractPaymentScheme(header: string): string | null {
  const parts = header.split(",").map((s) => s.trim());
  return parts.find((s) => s.substring(0, 8).toLowerCase().startsWith("payment ")) ?? null;
}

/**
 * Decode the request field of a challenge to a typed object.
 */
export function decodeRequest<T = Record<string, unknown>>(challenge: PaymentChallenge): T {
  const json = base64url.decodeString(challenge.request);
  return JSON.parse(json) as T;
}

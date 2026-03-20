/**
 * Fetch wrapper that auto-handles HTTP 402 Payment Required responses.
 *
 * Ports the Rust fetch wrapper from src/client/fetch.rs.
 */

import type { PaymentProvider, PaymentChallenge } from "./types.js";
import { parseWwwAuthenticate } from "./challenge.js";
import { formatAuthorization } from "./credential.js";

/**
 * Fetch with automatic 402 handling.
 *
 * If the server returns 402 with a WWW-Authenticate: Payment header,
 * the provider is called to pay, and the request is retried with
 * the Authorization header.
 *
 * @param input - URL or Request
 * @param init - fetch options
 * @param provider - payment provider to handle 402 challenges
 * @returns the final Response (either the original non-402, or the retry after payment)
 */
export async function fetchWithPayment(
  input: RequestInfo | URL,
  init: RequestInit | undefined,
  provider: PaymentProvider,
): Promise<Response> {
  const resp = await fetch(input, init);

  if (resp.status !== 402) return resp;

  const wwwAuth = resp.headers.get("www-authenticate");
  if (!wwwAuth) return resp;

  let challenge: PaymentChallenge;
  try {
    challenge = parseWwwAuthenticate(wwwAuth);
  } catch {
    return resp;
  }

  if (!provider.supports(challenge.method, challenge.intent)) {
    return resp;
  }

  const credential = await provider.pay(challenge);
  const authHeader = formatAuthorization(credential);

  const headers = new Headers(init?.headers);
  headers.set("Authorization", authHeader);

  return fetch(input, { ...init, headers });
}

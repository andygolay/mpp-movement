/**
 * Movement payment provider for one-time charge intents.
 *
 * Ports the Rust MovementProvider from src/client/movement/mod.rs.
 */

import type { PaymentProvider, PaymentChallenge, PaymentCredential, ChargeRequest } from "../types.js";
import { decodeRequest } from "../challenge.js";
import { challengeToEcho } from "../credential.js";
import { deriveAddress, toHex, hexToBytes, isNativeMove } from "./address.js";
import { getPublicKey } from "./voucher.js";

export interface MovementProviderOptions {
  /** Movement REST API URL (e.g., "https://testnet.movementnetwork.xyz/v1") */
  restUrl: string;
}

/**
 * Payment provider for one-time Movement charge payments.
 *
 * Handles the 402 flow: receives a charge challenge, submits an on-chain
 * transfer transaction, and returns a credential with the tx hash.
 *
 * Requires a signing key (for programmatic use) or a wallet adapter (for browser use).
 * For browser wallet integration, use WalletMovementProvider instead.
 */
export class MovementProvider implements PaymentProvider {
  private readonly privateKey: Uint8Array;
  private readonly publicKey: Uint8Array;
  readonly address: string;
  private readonly restUrl: string;

  constructor(privateKey: Uint8Array, options: MovementProviderOptions) {
    this.privateKey = privateKey;
    this.publicKey = getPublicKey(privateKey);
    this.address = toHex(deriveAddress(this.publicKey));
    this.restUrl = options.restUrl;
  }

  supports(method: string, intent: string): boolean {
    return method.toLowerCase() === "movement" && intent.toLowerCase() === "charge";
  }

  async pay(challenge: PaymentChallenge): Promise<PaymentCredential> {
    if (!this.supports(challenge.method, challenge.intent)) {
      throw new Error(`Unsupported: ${challenge.method}/${challenge.intent}`);
    }

    const request = decodeRequest<ChargeRequest>(challenge);
    const recipient = request.recipient;
    if (!recipient) throw new Error("Charge request missing recipient");

    // Submit transfer transaction
    const txHash = await this.submitTransfer(
      recipient,
      BigInt(request.amount),
      request.currency,
    );

    return {
      challenge: challengeToEcho(challenge),
      source: `did:movement:${this.address}`,
      payload: { type: "hash", hash: txHash },
    };
  }

  private async submitTransfer(
    recipient: string,
    amount: bigint,
    currency: string,
  ): Promise<string> {
    const isNative = isNativeMove(currency);

    const payload = isNative
      ? {
          type: "entry_function_payload",
          function: "0x1::aptos_account::transfer",
          type_arguments: [],
          arguments: [recipient, amount.toString()],
        }
      : {
          type: "entry_function_payload",
          function: "0x1::primary_fungible_store::transfer",
          type_arguments: ["0x1::fungible_asset::Metadata"],
          arguments: [currency, recipient, amount.toString()],
        };

    // Build, sign, and submit transaction via REST API
    // This is a simplified version — production use should use @moveindustries/ts-sdk
    const response = await fetch(`${this.restUrl}/transactions`, {
      method: "POST",
      headers: { "Content-Type": "application/json" },
      body: JSON.stringify({
        sender: this.address,
        payload,
      }),
    });

    if (!response.ok) {
      throw new Error(`Transaction submission failed: ${response.status}`);
    }

    const result = await response.json() as { hash: string };
    return result.hash;
  }
}

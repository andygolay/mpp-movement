/**
 * Movement session payment provider for payment channel / streaming intents.
 *
 * Ports the Rust MovementSessionProvider from src/client/movement/session.rs.
 *
 * This provider manages the lifecycle of payment channels:
 * 1. First request: opens a channel on-chain (1 tx, gas cost)
 * 2. Subsequent requests: signs off-chain vouchers (0 gas, instant)
 * 3. Close: signals the server to settle and close the channel
 */

import type {
  PaymentProvider,
  PaymentChallenge,
  PaymentCredential,
  SessionRequest,
} from "../types.js";
import { decodeRequest } from "../challenge.js";
import { challengeToEcho } from "../credential.js";
import {
  signVoucher,
  getPublicKey,
  computeChannelId,
  randomSalt,
} from "./voucher.js";
import { deriveAddress, toHex, hexToBytes } from "./address.js";

/** Wallet adapter interface — matches @moveindustries/wallet-adapter-react. */
export interface WalletAdapter {
  signAndSubmitTransaction(payload: {
    data: {
      function: `${string}::${string}::${string}`;
      functionArguments: unknown[];
    };
  }): Promise<{ hash: string }>;
  account: { address: string } | null;
}

export interface SessionProviderOptions {
  /** MovementStreamChannel module address. */
  moduleAddress: string;
  /** Registry address (defaults to moduleAddress). */
  registryAddress?: string;
  /** Token metadata address (e.g., "0xa" for native MOVE). */
  tokenMetadata: string;
  /** Maximum deposit to allow (caps server suggestions). */
  maxDeposit?: bigint;
  /** Default deposit if server doesn't suggest one. */
  defaultDeposit?: bigint;
}

interface ChannelEntry {
  channelId: Uint8Array;
  salt: Uint8Array;
  cumulativeAmount: bigint;
  moduleAddress: string;
  opened: boolean;
}

/**
 * Session payment provider using a browser wallet for channel opening
 * and an ephemeral ed25519 key for voucher signing.
 *
 * Usage:
 * ```ts
 * const provider = new MovementSessionProvider(wallet, {
 *   moduleAddress: "0x74f106...",
 *   tokenMetadata: "0xa",
 * });
 *
 * // Use with fetchWithPayment or manually:
 * const credential = await provider.pay(challenge);
 * ```
 */
export class MovementSessionProvider implements PaymentProvider {
  private readonly wallet: WalletAdapter;
  private readonly options: SessionProviderOptions;

  /** Ephemeral session private key (generated fresh per provider instance). */
  private readonly sessionKey: Uint8Array;
  /** Ephemeral session public key. */
  readonly sessionPublicKey: Uint8Array;

  /** Open channels keyed by "payee:currency:module". */
  private readonly channels = new Map<string, ChannelEntry>();

  constructor(wallet: WalletAdapter, options: SessionProviderOptions) {
    this.wallet = wallet;
    this.options = {
      ...options,
      registryAddress: options.registryAddress ?? options.moduleAddress,
    };

    // Generate ephemeral session keypair for voucher signing
    this.sessionKey = crypto.getRandomValues(new Uint8Array(32));
    this.sessionPublicKey = getPublicKey(this.sessionKey);
  }

  supports(method: string, intent: string): boolean {
    return method.toLowerCase() === "movement" && intent.toLowerCase() === "session";
  }

  /** Get the total cumulative amount across all channels. */
  get cumulative(): bigint {
    let total = 0n;
    for (const ch of this.channels.values()) {
      total += ch.cumulativeAmount;
    }
    return total;
  }

  async pay(challenge: PaymentChallenge): Promise<PaymentCredential> {
    if (!this.supports(challenge.method, challenge.intent)) {
      throw new Error(`Unsupported: ${challenge.method}/${challenge.intent}`);
    }

    const request = decodeRequest<SessionRequest>(challenge);
    const recipient = request.recipient;
    if (!recipient) throw new Error("Session request missing recipient");

    const currency = request.currency;
    const moduleAddress = request.methodDetails?.moduleAddress as string ?? this.options.moduleAddress;
    const channelKey = `${recipient}:${currency}:${moduleAddress}`;

    const existing = this.channels.get(channelKey);
    if (existing?.opened) {
      return this.sendVoucher(challenge, request, existing);
    }

    return this.openChannel(challenge, request, channelKey);
  }

  private async openChannel(
    challenge: PaymentChallenge,
    request: SessionRequest,
    channelKey: string,
  ): Promise<PaymentCredential> {
    const account = this.wallet.account;
    if (!account) throw new Error("Wallet not connected");

    const recipient = request.recipient!;
    const currency = request.currency;
    const moduleAddress = request.methodDetails?.moduleAddress as string ?? this.options.moduleAddress;
    const registryAddress = this.options.registryAddress!;

    // Determine deposit amount
    let deposit: bigint;
    if (request.suggestedDeposit) {
      deposit = BigInt(request.suggestedDeposit);
      if (this.options.maxDeposit && deposit > this.options.maxDeposit) {
        deposit = this.options.maxDeposit;
      }
    } else if (this.options.defaultDeposit) {
      deposit = this.options.defaultDeposit;
    } else {
      throw new Error("No deposit amount: server didn't suggest and no default set");
    }

    // Compute channel ID
    const salt = randomSalt();
    const payerBytes = hexToBytes(account.address);
    const payeeBytes = hexToBytes(recipient);
    const tokenBytes = hexToBytes(currency);
    const channelId = computeChannelId(
      payerBytes,
      payeeBytes,
      tokenBytes,
      salt,
      this.sessionPublicKey,
    );

    // Open channel on-chain via wallet
    const txResponse = await this.wallet.signAndSubmitTransaction({
      data: {
        function: `${moduleAddress}::channel::open` as `${string}::${string}::${string}`,
        functionArguments: [
          registryAddress,
          recipient,
          this.options.tokenMetadata,
          Number(deposit),
          Array.from(salt),
          Array.from(this.sessionPublicKey),
        ],
      },
    });

    // Register channel
    const entry: ChannelEntry = {
      channelId,
      salt,
      cumulativeAmount: 0n,
      moduleAddress,
      opened: true,
    };
    this.channels.set(channelKey, entry);

    // Sign initial voucher
    const amount = BigInt(request.amount);
    entry.cumulativeAmount = amount;
    const signature = signVoucher(
      { channelId, cumulativeAmount: amount },
      this.sessionKey,
    );

    return {
      challenge: challengeToEcho(challenge),
      source: `did:movement:${account.address}`,
      payload: {
        action: "open",
        txHash: txResponse.hash,
        channelId: toHex(channelId),
        authorizedSigner: toHex(this.sessionPublicKey),
        cumulativeAmount: amount.toString(),
        signature: toHex(signature),
      },
    };
  }

  private sendVoucher(
    challenge: PaymentChallenge,
    request: SessionRequest,
    entry: ChannelEntry,
  ): PaymentCredential {
    const amount = BigInt(request.amount);
    entry.cumulativeAmount += amount;

    const signature = signVoucher(
      { channelId: entry.channelId, cumulativeAmount: entry.cumulativeAmount },
      this.sessionKey,
    );

    const account = this.wallet.account;

    return {
      challenge: challengeToEcho(challenge),
      source: account ? `did:movement:${account.address}` : undefined,
      payload: {
        action: "voucher",
        channelId: toHex(entry.channelId),
        cumulativeAmount: entry.cumulativeAmount.toString(),
        signature: toHex(signature),
      },
    };
  }

  /**
   * Get the channel ID for a given recipient/currency/module combination.
   * Returns null if no channel is open.
   */
  getChannelId(recipient: string, currency: string, moduleAddress?: string): Uint8Array | null {
    const mod = moduleAddress ?? this.options.moduleAddress;
    const key = `${recipient}:${currency}:${mod}`;
    return this.channels.get(key)?.channelId ?? null;
  }

  /**
   * Get the current cumulative amount for a channel.
   */
  getChannelCumulative(recipient: string, currency: string, moduleAddress?: string): bigint {
    const mod = moduleAddress ?? this.options.moduleAddress;
    const key = `${recipient}:${currency}:${mod}`;
    return this.channels.get(key)?.cumulativeAmount ?? 0n;
  }

  /**
   * Manually sign a voucher for a specific channel (for custom flows).
   * Increments the cumulative amount by the given delta.
   */
  signVoucherFor(
    recipient: string,
    currency: string,
    delta: bigint,
    moduleAddress?: string,
  ): { channelId: Uint8Array; cumulativeAmount: bigint; signature: Uint8Array } {
    const mod = moduleAddress ?? this.options.moduleAddress;
    const key = `${recipient}:${currency}:${mod}`;
    const entry = this.channels.get(key);
    if (!entry) throw new Error("No open channel for this recipient/currency");

    entry.cumulativeAmount += delta;
    const signature = signVoucher(
      { channelId: entry.channelId, cumulativeAmount: entry.cumulativeAmount },
      this.sessionKey,
    );

    return {
      channelId: entry.channelId,
      cumulativeAmount: entry.cumulativeAmount,
      signature,
    };
  }
}

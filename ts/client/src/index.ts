// Protocol types
export type {
  PaymentChallenge,
  ChallengeEcho,
  PaymentCredential,
  PaymentPayload,
  Receipt,
  ChargeRequest,
  SessionRequest,
  PaymentProvider,
} from "./types.js";

// Challenge (WWW-Authenticate) parsing/formatting
export {
  parseWwwAuthenticate,
  parseWwwAuthenticateAll,
  formatWwwAuthenticate,
  extractPaymentScheme,
  decodeRequest,
} from "./challenge.js";

// Credential (Authorization) parsing/formatting
export {
  parseAuthorization,
  formatAuthorization,
  challengeToEcho,
  chargeCredential,
  sessionCredential,
} from "./credential.js";

// Receipt (Payment-Receipt) parsing/formatting
export { parseReceipt, formatReceipt } from "./receipt.js";

// Base64url utilities
export * as base64url from "./base64url.js";

// Fetch wrapper
export { fetchWithPayment } from "./fetch.js";

// Movement-specific (re-exported for convenience)
export {
  signVoucher,
  verifyVoucher,
  serializeVoucher,
  getPublicKey,
  computeChannelId,
  randomSalt,
  type Voucher,
  deriveAddress,
  toHex,
  hexToBytes,
  isNativeMove,
  MovementProvider,
  type MovementProviderOptions,
  MovementSessionProvider,
  type SessionProviderOptions,
  type WalletAdapter,
} from "./movement/index.js";

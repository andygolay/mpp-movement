export const SERVER_URL =
  import.meta.env.VITE_SERVER_URL ?? "http://localhost:3002";

export const MODULE_ADDRESS =
  import.meta.env.VITE_MODULE_ADDRESS ??
  "0x74f1060add0c641a0c10bb5bab2bf5fd05f94d7c25055f2419fa82d7bbf2b1e8";

export const REGISTRY_ADDR =
  import.meta.env.VITE_REGISTRY_ADDR ?? MODULE_ADDRESS;

// Native MOVE token
export const TOKEN_METADATA_ADDR =
  import.meta.env.VITE_TOKEN_METADATA_ADDR ?? "0xa";

export const TOKEN_SYMBOL =
  import.meta.env.VITE_TOKEN_SYMBOL ?? "MOVE";

export const TOKEN_DECIMALS = Number(
  import.meta.env.VITE_TOKEN_DECIMALS ?? "8",
);

// TURN server config for WebRTC across different networks.
// Set these env vars to enable relay (required when peers aren't on the same LAN).
export const TURN_URL = import.meta.env.VITE_TURN_URL ?? "";
export const TURN_USERNAME = import.meta.env.VITE_TURN_USERNAME ?? "";
export const TURN_CREDENTIAL = import.meta.env.VITE_TURN_CREDENTIAL ?? "";

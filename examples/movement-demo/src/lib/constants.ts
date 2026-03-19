export const SERVER_URL =
  import.meta.env.VITE_SERVER_URL ?? "http://localhost:3001";

export const MODULE_ADDRESS =
  import.meta.env.VITE_MODULE_ADDRESS ??
  "0x3e9edf3be513781a6db0706b652da425ad67f58b5cb366847126bf0fb716fc58";

export const REGISTRY_ADDR =
  import.meta.env.VITE_REGISTRY_ADDR ?? MODULE_ADDRESS;

// USDCx FA metadata on Movement testnet.
// Set to "0xa" for native MOVE instead.
export const TOKEN_METADATA_ADDR =
  import.meta.env.VITE_TOKEN_METADATA_ADDR ??
  "0x63f169ba69623ba6ccf34620857644feb46d0f87e1d7bbcf8c071d30c3d94bd6";

export const TOKEN_SYMBOL =
  import.meta.env.VITE_TOKEN_SYMBOL ?? "USDCx";

export const TOKEN_DECIMALS = Number(
  import.meta.env.VITE_TOKEN_DECIMALS ?? "6",
);

// How many tokens to buy per voucher sent to the server.
export const TOKENS_PER_VOUCHER = 10;

import { describe, it, expect } from "vitest";
import {
  signVoucher,
  verifyVoucher,
  serializeVoucher,
  getPublicKey,
  computeChannelId,
  randomSalt,
} from "../src/movement/voucher.js";
import { deriveAddress, toHex, hexToBytes } from "../src/movement/address.js";

describe("voucher signing", () => {
  const privateKey = new Uint8Array(32);
  privateKey[0] = 1; // deterministic test key
  const publicKey = getPublicKey(privateKey);

  it("signs and verifies a voucher", () => {
    const channelId = new Uint8Array(32).fill(0xab);
    const voucher = { channelId, cumulativeAmount: 1000n };

    const signature = signVoucher(voucher, privateKey);
    expect(signature).toHaveLength(64);

    expect(verifyVoucher(voucher, signature, publicKey)).toBe(true);
  });

  it("rejects wrong amount", () => {
    const channelId = new Uint8Array(32).fill(0xab);
    const voucher = { channelId, cumulativeAmount: 1000n };
    const signature = signVoucher(voucher, privateKey);

    const wrongVoucher = { channelId, cumulativeAmount: 2000n };
    expect(verifyVoucher(wrongVoucher, signature, publicKey)).toBe(false);
  });

  it("rejects wrong key", () => {
    const channelId = new Uint8Array(32).fill(0xab);
    const voucher = { channelId, cumulativeAmount: 1000n };
    const signature = signVoucher(voucher, privateKey);

    const wrongKey = new Uint8Array(32);
    wrongKey[0] = 2;
    const wrongPubKey = getPublicKey(wrongKey);
    expect(verifyVoucher(voucher, signature, wrongPubKey)).toBe(false);
  });
});

describe("serializeVoucher", () => {
  it("serializes correctly", () => {
    const channelId = new Uint8Array([1, 2, 3]);
    const voucher = { channelId, cumulativeAmount: 256n };
    const bytes = serializeVoucher(voucher);

    // ULEB128(3) = [3], then [1,2,3], then u64 LE 256 = [0,1,0,0,0,0,0,0]
    expect(bytes).toEqual(
      new Uint8Array([3, 1, 2, 3, 0, 1, 0, 0, 0, 0, 0, 0]),
    );
  });

  it("handles zero amount", () => {
    const channelId = new Uint8Array(32);
    const voucher = { channelId, cumulativeAmount: 0n };
    const bytes = serializeVoucher(voucher);
    expect(bytes).toHaveLength(32 + 1 + 8); // ULEB128(32)=1 byte + 32 bytes + 8 bytes
  });

  it("handles large amounts", () => {
    const channelId = new Uint8Array(32);
    const voucher = { channelId, cumulativeAmount: 2n ** 63n };
    const bytes = serializeVoucher(voucher);
    // Last 8 bytes should be the u64 LE representation
    const view = new DataView(bytes.buffer, bytes.byteOffset + 33);
    expect(view.getBigUint64(0, true)).toBe(2n ** 63n);
  });
});

describe("computeChannelId", () => {
  it("produces 32 bytes", () => {
    const payer = new Uint8Array(32).fill(1);
    const payee = new Uint8Array(32).fill(2);
    const token = new Uint8Array(32).fill(3);
    const salt = randomSalt();
    const pubkey = getPublicKey(new Uint8Array(32).fill(4));

    const id = computeChannelId(payer, payee, token, salt, pubkey);
    expect(id).toHaveLength(32);
  });

  it("is deterministic", () => {
    const payer = new Uint8Array(32).fill(1);
    const payee = new Uint8Array(32).fill(2);
    const token = new Uint8Array(32).fill(3);
    const salt = new Uint8Array(32).fill(4);
    const pubkey = getPublicKey(new Uint8Array(32).fill(5));

    const id1 = computeChannelId(payer, payee, token, salt, pubkey);
    const id2 = computeChannelId(payer, payee, token, salt, pubkey);
    expect(toHex(id1)).toBe(toHex(id2));
  });

  it("changes with different inputs", () => {
    const payer = new Uint8Array(32).fill(1);
    const payee = new Uint8Array(32).fill(2);
    const token = new Uint8Array(32).fill(3);
    const salt = new Uint8Array(32).fill(4);
    const pubkey = getPublicKey(new Uint8Array(32).fill(5));

    const id1 = computeChannelId(payer, payee, token, salt, pubkey);

    const differentPayee = new Uint8Array(32).fill(9);
    const id2 = computeChannelId(payer, differentPayee, token, salt, pubkey);

    expect(toHex(id1)).not.toBe(toHex(id2));
  });
});

describe("address", () => {
  it("deriveAddress produces 32 bytes", () => {
    const privateKey = new Uint8Array(32);
    privateKey[0] = 1;
    const pubkey = getPublicKey(privateKey);
    const addr = deriveAddress(pubkey);
    expect(addr).toHaveLength(32);
  });

  it("toHex / hexToBytes roundtrip", () => {
    const bytes = new Uint8Array(32).fill(0xab);
    const hex = toHex(bytes);
    expect(hex).toMatch(/^0x/);
    expect(hexToBytes(hex)).toEqual(bytes);
  });

  it("hexToBytes pads short addresses", () => {
    const bytes = hexToBytes("0xa");
    expect(bytes).toHaveLength(32);
    expect(bytes[31]).toBe(0x0a);
    expect(bytes[0]).toBe(0);
  });
});

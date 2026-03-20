import { describe, it, expect } from "vitest";
import { encode, decode, encodeString, decodeString } from "../src/base64url.js";

describe("base64url", () => {
  it("roundtrips bytes", () => {
    const data = new Uint8Array([104, 101, 108, 108, 111, 32, 119, 111, 114, 108, 100]);
    const encoded = encode(data);
    expect(encoded).not.toContain("=");
    expect(encoded).not.toContain("+");
    expect(encoded).not.toContain("/");
    const decoded = decode(encoded);
    expect(decoded).toEqual(data);
  });

  it("roundtrips strings", () => {
    const str = '{"amount":"1000","currency":"USD"}';
    const encoded = encodeString(str);
    const decoded = decodeString(encoded);
    expect(decoded).toBe(str);
  });

  it("handles empty input", () => {
    expect(encode(new Uint8Array([]))).toBe("");
    expect(decode("")).toEqual(new Uint8Array([]));
  });

  it("strips padding if present", () => {
    const encoded = encode(new Uint8Array([1, 2, 3]));
    const withPadding = encoded + "==";
    expect(decode(withPadding)).toEqual(new Uint8Array([1, 2, 3]));
  });

  it("matches known vector: e30 = {}", () => {
    const decoded = decodeString("e30");
    expect(decoded).toBe("{}");
    const encoded = encodeString("{}");
    expect(encoded).toBe("e30");
  });
});

/**
 * Base64url encoding/decoding (RFC 4648 §5, no padding).
 */

const CHARS = "ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789-_";

export function encode(data: Uint8Array): string {
  let result = "";
  const len = data.length;
  for (let i = 0; i < len; i += 3) {
    const b0 = data[i];
    const b1 = i + 1 < len ? data[i + 1] : 0;
    const b2 = i + 2 < len ? data[i + 2] : 0;
    result += CHARS[(b0 >> 2) & 0x3f];
    result += CHARS[((b0 << 4) | (b1 >> 4)) & 0x3f];
    if (i + 1 < len) result += CHARS[((b1 << 2) | (b2 >> 6)) & 0x3f];
    if (i + 2 < len) result += CHARS[b2 & 0x3f];
  }
  return result;
}

export function decode(input: string): Uint8Array {
  const lookup = new Uint8Array(128);
  for (let i = 0; i < CHARS.length; i++) lookup[CHARS.charCodeAt(i)] = i;

  // Strip padding if present
  const str = input.replace(/=+$/, "");

  const out = new Uint8Array(Math.floor((str.length * 3) / 4));
  let j = 0;
  for (let i = 0; i < str.length; i += 4) {
    const c0 = lookup[str.charCodeAt(i)];
    const c1 = lookup[str.charCodeAt(i + 1)];
    const c2 = i + 2 < str.length ? lookup[str.charCodeAt(i + 2)] : 0;
    const c3 = i + 3 < str.length ? lookup[str.charCodeAt(i + 3)] : 0;
    out[j++] = (c0 << 2) | (c1 >> 4);
    if (i + 2 < str.length) out[j++] = ((c1 << 4) | (c2 >> 2)) & 0xff;
    if (i + 3 < str.length) out[j++] = ((c2 << 6) | c3) & 0xff;
  }
  return out.slice(0, j);
}

export function encodeString(s: string): string {
  return encode(new TextEncoder().encode(s));
}

export function decodeString(s: string): string {
  return new TextDecoder().decode(decode(s));
}

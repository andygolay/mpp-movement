import { describe, it, expect } from "vitest";
import {
  parseWwwAuthenticate,
  parseWwwAuthenticateAll,
  formatWwwAuthenticate,
  extractPaymentScheme,
  decodeRequest,
} from "../src/challenge.js";
import { encodeString } from "../src/base64url.js";
import type { PaymentChallenge } from "../src/types.js";

const REQUEST_JSON = '{"amount":"10000","currency":"0x123"}';
const REQUEST_B64 = encodeString(REQUEST_JSON);

function makeHeader(overrides: Partial<Record<string, string>> = {}): string {
  const id = overrides.id ?? "abc123";
  const realm = overrides.realm ?? "api";
  const method = overrides.method ?? "movement";
  const intent = overrides.intent ?? "charge";
  const request = overrides.request ?? REQUEST_B64;
  return `Payment id="${id}", realm="${realm}", method="${method}", intent="${intent}", request="${request}"`;
}

describe("parseWwwAuthenticate", () => {
  it("parses a valid challenge", () => {
    const challenge = parseWwwAuthenticate(makeHeader());
    expect(challenge.id).toBe("abc123");
    expect(challenge.realm).toBe("api");
    expect(challenge.method).toBe("movement");
    expect(challenge.intent).toBe("charge");
    expect(challenge.request).toBe(REQUEST_B64);
  });

  it("is case-insensitive on scheme", () => {
    const header = makeHeader().replace("Payment", "payment");
    const challenge = parseWwwAuthenticate(header);
    expect(challenge.id).toBe("abc123");
  });

  it("parses optional expires", () => {
    const header = makeHeader() + ', expires="2024-01-01T00:00:00Z"';
    const challenge = parseWwwAuthenticate(header);
    expect(challenge.expires).toBe("2024-01-01T00:00:00Z");
  });

  it("parses optional description", () => {
    const header = makeHeader() + ', description="Pay for fortune"';
    const challenge = parseWwwAuthenticate(header);
    expect(challenge.description).toBe("Pay for fortune");
  });

  it("parses optional digest", () => {
    const header = makeHeader() + ', digest="sha-256=abc123"';
    const challenge = parseWwwAuthenticate(header);
    expect(challenge.digest).toBe("sha-256=abc123");
  });

  it("rejects non-Payment scheme", () => {
    expect(() => parseWwwAuthenticate("Bearer token")).toThrow("Expected 'Payment' scheme");
  });

  it("rejects empty id", () => {
    expect(() => parseWwwAuthenticate(makeHeader({ id: "" }))).toThrow();
  });

  it("rejects missing method", () => {
    const header = `Payment id="abc", realm="api", intent="charge", request="${REQUEST_B64}"`;
    expect(() => parseWwwAuthenticate(header)).toThrow("Missing 'method'");
  });

  it("rejects invalid method (mixed case)", () => {
    expect(() => parseWwwAuthenticate(makeHeader({ method: "Movement" }))).toThrow("Invalid method");
  });

  it("rejects invalid method (dash)", () => {
    expect(() => parseWwwAuthenticate(makeHeader({ method: "movement-v2" }))).toThrow("Invalid method");
  });

  it("rejects invalid method (digit prefix)", () => {
    expect(() => parseWwwAuthenticate(makeHeader({ method: "1movement" }))).toThrow("Invalid method");
  });

  it("rejects invalid digest format", () => {
    const header = makeHeader() + ', digest="md5=abc"';
    expect(() => parseWwwAuthenticate(header)).toThrow("Invalid digest format");
  });

  it("rejects invalid JSON in request", () => {
    const badB64 = encodeString("not json");
    expect(() => parseWwwAuthenticate(makeHeader({ request: badB64 }))).toThrow("Invalid JSON");
  });

  it("handles escaped quotes in values", () => {
    const header = `Payment id="abc\\"def", realm="api", method="movement", intent="charge", request="${REQUEST_B64}"`;
    const challenge = parseWwwAuthenticate(header);
    expect(challenge.id).toBe('abc"def');
  });
});

describe("parseWwwAuthenticateAll", () => {
  it("filters non-Payment headers", () => {
    const headers = [
      "Bearer token",
      makeHeader(),
      "Basic xyz",
      makeHeader().replace("abc123", "def456"),
    ];
    const challenges = parseWwwAuthenticateAll(headers);
    expect(challenges).toHaveLength(2);
    expect(challenges[0].id).toBe("abc123");
    expect(challenges[1].id).toBe("def456");
  });

  it("returns empty for no Payment headers", () => {
    expect(parseWwwAuthenticateAll(["Bearer token"])).toHaveLength(0);
  });
});

describe("formatWwwAuthenticate", () => {
  it("roundtrips a challenge", () => {
    const original = parseWwwAuthenticate(makeHeader());
    const formatted = formatWwwAuthenticate(original);
    const reparsed = parseWwwAuthenticate(formatted);
    expect(reparsed.id).toBe(original.id);
    expect(reparsed.realm).toBe(original.realm);
    expect(reparsed.method).toBe(original.method);
    expect(reparsed.intent).toBe(original.intent);
    expect(reparsed.request).toBe(original.request);
  });

  it("includes optional fields", () => {
    const challenge: PaymentChallenge = {
      id: "test",
      realm: "api",
      method: "movement",
      intent: "charge",
      request: REQUEST_B64,
      expires: "2024-01-01T00:00:00Z",
      description: "Test payment",
    };
    const header = formatWwwAuthenticate(challenge);
    expect(header).toContain('expires="2024-01-01T00:00:00Z"');
    expect(header).toContain('description="Test payment"');
  });

  it("escapes special characters", () => {
    const challenge: PaymentChallenge = {
      id: 'has"quotes',
      realm: "api",
      method: "movement",
      intent: "charge",
      request: REQUEST_B64,
    };
    const header = formatWwwAuthenticate(challenge);
    expect(header).toContain('id="has\\"quotes"');
    const reparsed = parseWwwAuthenticate(header);
    expect(reparsed.id).toBe('has"quotes');
  });

  it("rejects CRLF in values", () => {
    const challenge: PaymentChallenge = {
      id: "test\r\ninjection",
      realm: "api",
      method: "movement",
      intent: "charge",
      request: REQUEST_B64,
    };
    expect(() => formatWwwAuthenticate(challenge)).toThrow("CRLF");
  });
});

describe("extractPaymentScheme", () => {
  it("extracts from single scheme", () => {
    expect(extractPaymentScheme("Payment eyJhYmMi")).toBe("Payment eyJhYmMi");
  });

  it("extracts from mixed schemes", () => {
    const result = extractPaymentScheme("Bearer token123, Payment eyJhYmMi");
    expect(result).toBe("Payment eyJhYmMi");
  });

  it("returns null for no Payment scheme", () => {
    expect(extractPaymentScheme("Bearer token123")).toBeNull();
  });
});

describe("decodeRequest", () => {
  it("decodes charge request", () => {
    const challenge = parseWwwAuthenticate(makeHeader());
    const request = decodeRequest<{ amount: string; currency: string }>(challenge);
    expect(request.amount).toBe("10000");
    expect(request.currency).toBe("0x123");
  });
});

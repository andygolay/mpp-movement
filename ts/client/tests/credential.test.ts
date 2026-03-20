import { describe, it, expect } from "vitest";
import {
  parseAuthorization,
  formatAuthorization,
  challengeToEcho,
  chargeCredential,
  sessionCredential,
} from "../src/credential.js";
import { encodeString } from "../src/base64url.js";
import type { PaymentChallenge, PaymentCredential } from "../src/types.js";

const REQUEST_B64 = encodeString('{"amount":"1000","currency":"0xa"}');

const challenge: PaymentChallenge = {
  id: "test-id",
  realm: "api.example.com",
  method: "movement",
  intent: "charge",
  request: REQUEST_B64,
  expires: "2024-12-31T00:00:00Z",
};

describe("formatAuthorization / parseAuthorization", () => {
  it("roundtrips a credential", () => {
    const credential: PaymentCredential = {
      challenge: challengeToEcho(challenge),
      source: "did:movement:0xabc",
      payload: { type: "hash", hash: "0xdeadbeef" },
    };
    const header = formatAuthorization(credential);
    expect(header).toMatch(/^Payment /);

    const parsed = parseAuthorization(header);
    expect(parsed.challenge.id).toBe("test-id");
    expect(parsed.challenge.realm).toBe("api.example.com");
    expect(parsed.source).toBe("did:movement:0xabc");
    expect((parsed.payload as { hash: string }).hash).toBe("0xdeadbeef");
  });

  it("rejects non-Payment scheme", () => {
    expect(() => parseAuthorization("Bearer token")).toThrow("Expected 'Payment'");
  });

  it("rejects oversized tokens", () => {
    const huge = "Payment " + "A".repeat(20000);
    expect(() => parseAuthorization(huge)).toThrow("maximum length");
  });
});

describe("challengeToEcho", () => {
  it("copies required fields", () => {
    const echo = challengeToEcho(challenge);
    expect(echo.id).toBe(challenge.id);
    expect(echo.realm).toBe(challenge.realm);
    expect(echo.method).toBe(challenge.method);
    expect(echo.intent).toBe(challenge.intent);
    expect(echo.request).toBe(challenge.request);
    expect(echo.expires).toBe(challenge.expires);
  });

  it("omits undefined optional fields", () => {
    const minimal: PaymentChallenge = {
      id: "x",
      realm: "y",
      method: "movement",
      intent: "charge",
      request: "e30",
    };
    const echo = challengeToEcho(minimal);
    expect(echo.expires).toBeUndefined();
    expect(echo.digest).toBeUndefined();
    expect(echo.opaque).toBeUndefined();
  });
});

describe("chargeCredential", () => {
  it("builds a hash credential", () => {
    const cred = chargeCredential(challenge, "0xdeadbeef", "did:movement:0xabc");
    expect(cred.challenge.id).toBe("test-id");
    expect(cred.source).toBe("did:movement:0xabc");
    expect((cred.payload as { type: string }).type).toBe("hash");
    expect((cred.payload as { hash: string }).hash).toBe("0xdeadbeef");
  });

  it("works without source", () => {
    const cred = chargeCredential(challenge, "0xdeadbeef");
    expect(cred.source).toBeUndefined();
  });
});

describe("sessionCredential", () => {
  it("builds a session credential", () => {
    const cred = sessionCredential(challenge, {
      action: "open",
      txHash: "0xabc",
      channelId: "0x123",
    });
    expect(cred.challenge.intent).toBe("charge"); // echoes original
    expect((cred.payload as { action: string }).action).toBe("open");
  });
});

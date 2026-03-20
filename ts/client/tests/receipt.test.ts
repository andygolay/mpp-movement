import { describe, it, expect } from "vitest";
import { parseReceipt, formatReceipt } from "../src/receipt.js";
import type { Receipt } from "../src/types.js";

describe("receipt", () => {
  const receipt: Receipt = {
    status: "success",
    method: "movement",
    timestamp: "2024-01-01T00:00:00Z",
    reference: "0xdeadbeef",
  };

  it("roundtrips a receipt", () => {
    const header = formatReceipt(receipt);
    const parsed = parseReceipt(header);
    expect(parsed.status).toBe("success");
    expect(parsed.method).toBe("movement");
    expect(parsed.timestamp).toBe("2024-01-01T00:00:00Z");
    expect(parsed.reference).toBe("0xdeadbeef");
  });

  it("rejects oversized tokens", () => {
    const huge = "A".repeat(20000);
    expect(() => parseReceipt(huge)).toThrow("maximum length");
  });
});

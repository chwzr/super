import { describe, expect, it } from "vitest";
import { createVerifier, deriveChallenge } from "@/lib/pkce";

describe("pkce", () => {
  it("generates a high-entropy URL-safe verifier", () => {
    const a = createVerifier();
    const b = createVerifier();
    expect(a).not.toEqual(b);
    expect(a).toMatch(/^[A-Za-z0-9_-]{20,}$/);
    expect(a.length).toBeGreaterThanOrEqual(32);
  });

  it("derives a deterministic SHA-256 challenge", async () => {
    const verifier = "dBjftJeZ4CVP-mB92K27uhbUJU1p1r_wW1gFWFOEjXk";
    const challenge = await deriveChallenge(verifier);
    expect(challenge).toBe("E9Melhoa2OwvFrEMTJguCHaoeK1t8URWbuGJSstw-cM");
  });

  it("produces challenges that are URL-safe and unpadded", async () => {
    const verifier = createVerifier();
    const challenge = await deriveChallenge(verifier);
    expect(challenge).toMatch(/^[A-Za-z0-9_-]+$/);
    expect(challenge).not.toMatch(/=$/);
  });
});

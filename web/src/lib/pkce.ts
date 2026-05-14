/**
 * Helpers for the PKCE leg of the Super auth flow.
 *
 * Mirrors the verifier/challenge derivation used by the Rust CLI so the
 * platform server can authorize a token regardless of which client kicked
 * off the flow. See `cli/src/auth/pkce.rs` (server side) and the
 * `/auth/login` + `/auth/authorize` routes in `server/src/routes/auth.rs`.
 */

const VERIFIER_BYTES = 32;

function base64UrlEncode(bytes: Uint8Array): string {
  let binary = "";
  for (const byte of bytes) {
    binary += String.fromCharCode(byte);
  }
  return btoa(binary).replace(/\+/g, "-").replace(/\//g, "_").replace(/=+$/, "");
}

export function createVerifier(): string {
  const bytes = new Uint8Array(VERIFIER_BYTES);
  crypto.getRandomValues(bytes);
  return base64UrlEncode(bytes);
}

export async function deriveChallenge(verifier: string): Promise<string> {
  const data = new TextEncoder().encode(verifier);
  const digest = await crypto.subtle.digest("SHA-256", data);
  return base64UrlEncode(new Uint8Array(digest));
}

export interface PkcePair {
  verifier: string;
  challenge: string;
}

export async function createPkcePair(): Promise<PkcePair> {
  const verifier = createVerifier();
  const challenge = await deriveChallenge(verifier);
  return { verifier, challenge };
}

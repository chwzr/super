/**
 * Thin client for the Super platform server's `/auth/*` API.
 *
 * The server exposes (see `server/src/routes/auth.rs`):
 *   POST /auth/register          -> UserProfile
 *   POST /auth/authorize         -> TokenResponse        (PKCE second leg)
 *   POST /auth/refresh           -> TokenResponse
 *   GET  /auth/me                -> UserProfile          (Bearer)
 *   POST /auth/key               -> UserProfile          (Bearer, rotate)
 *   GET  /auth/usage             -> UsageResponse        (Bearer)
 *   POST /auth/login (form-url)  -> 302 Location: ...?code=<code>
 *
 * In dev, requests go through the Vite proxy at `/auth`. In production
 * the SPA is served by the Rust server itself, so same-origin requests
 * to `/auth` work without CORS config.
 */

export interface UserProfile {
  id: string;
  email: string;
  openrouter_api_key: string;
  created_at: string;
}

export interface TokenResponse {
  access_token: string;
  refresh_token: string;
  expires_in: number;
}

export interface UsageResponse {
  used_usd: number;
  limit_usd: number;
  remaining_usd: number;
}

export class ApiError extends Error {
  status: number;
  body: string;

  constructor(status: number, body: string) {
    super(`API ${status}: ${body || "(empty body)"}`);
    this.status = status;
    this.body = body;
  }
}

const DEFAULT_BASE = (import.meta.env.VITE_API_BASE_URL as string | undefined) ?? "";

interface RequestOptions {
  method?: "GET" | "POST";
  body?: unknown;
  headers?: Record<string, string>;
  signal?: AbortSignal;
}

async function request<T>(path: string, opts: RequestOptions = {}): Promise<T> {
  const url = `${DEFAULT_BASE}${path}`;
  const headers: Record<string, string> = {
    Accept: "application/json",
    ...opts.headers,
  };
  let body: string | undefined;
  if (opts.body !== undefined) {
    headers["Content-Type"] = "application/json";
    body = JSON.stringify(opts.body);
  }
  const res = await fetch(url, {
    method: opts.method ?? "GET",
    headers,
    body,
    signal: opts.signal,
  });
  const text = await res.text();
  if (!res.ok) {
    throw new ApiError(res.status, text);
  }
  if (!text) {
    return undefined as T;
  }
  return JSON.parse(text) as T;
}

export const authApi = {
  register(payload: { email: string; password: string }): Promise<UserProfile> {
    return request<UserProfile>("/auth/register", { method: "POST", body: payload });
  },

  /**
   * Performs the form-style /auth/login request and returns the
   * single-use authorization code from the redirect Location header.
   */
  async login(args: {
    email: string;
    password: string;
    codeChallenge: string;
    redirectUri?: string;
  }): Promise<string> {
    const url = `${DEFAULT_BASE}/auth/login`;
    const form = new URLSearchParams();
    form.set("email", args.email);
    form.set("password", args.password);
    form.set("code_challenge", args.codeChallenge);
    if (args.redirectUri) {
      form.set("redirect_uri", args.redirectUri);
    }
    const res = await fetch(url, {
      method: "POST",
      headers: { "Content-Type": "application/x-www-form-urlencoded" },
      body: form.toString(),
      redirect: "manual",
    });
    if (res.status === 0 || res.type === "opaqueredirect") {
      // Browsers hide the Location header on opaque redirects; we cannot
      // recover the authorization code from a redirected response.
      throw new ApiError(
        500,
        "login redirect was opaque; ensure the SPA shares an origin with the server",
      );
    }
    if (res.status >= 400) {
      throw new ApiError(res.status, await res.text());
    }
    const location = res.headers.get("Location") ?? "";
    const code = new URL(location, window.location.origin).searchParams.get("code");
    if (!code) {
      throw new ApiError(500, `login response missing 'code' (Location=${location})`);
    }
    return code;
  },

  authorize(payload: { code: string; codeVerifier: string }): Promise<TokenResponse> {
    return request<TokenResponse>("/auth/authorize", {
      method: "POST",
      body: { code: payload.code, code_verifier: payload.codeVerifier },
    });
  },

  refresh(refreshToken: string): Promise<TokenResponse> {
    return request<TokenResponse>("/auth/refresh", {
      method: "POST",
      body: { refresh_token: refreshToken },
    });
  },

  me(accessToken: string): Promise<UserProfile> {
    return request<UserProfile>("/auth/me", {
      headers: { Authorization: `Bearer ${accessToken}` },
    });
  },

  rotateKey(accessToken: string): Promise<UserProfile> {
    return request<UserProfile>("/auth/key", {
      method: "POST",
      headers: { Authorization: `Bearer ${accessToken}` },
    });
  },

  usage(accessToken: string): Promise<UsageResponse> {
    return request<UsageResponse>("/auth/usage", {
      headers: { Authorization: `Bearer ${accessToken}` },
    });
  },
};

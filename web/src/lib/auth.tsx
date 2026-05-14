import { useCallback, useEffect, useMemo, useState, type ReactNode } from "react";

import { ApiError, authApi, type TokenResponse, type UserProfile } from "./api";
import { AuthContext, type AuthContextValue, type AuthStatus } from "./auth-context";

interface PersistedSession {
  access_token: string;
  refresh_token: string;
  expires_at: number;
}

const STORAGE_KEY = "super.auth.v1";

function loadSession(): PersistedSession | null {
  try {
    const raw = window.localStorage.getItem(STORAGE_KEY);
    if (!raw) return null;
    const parsed = JSON.parse(raw) as PersistedSession;
    if (
      typeof parsed.access_token !== "string" ||
      typeof parsed.refresh_token !== "string" ||
      typeof parsed.expires_at !== "number"
    ) {
      return null;
    }
    return parsed;
  } catch {
    return null;
  }
}

function saveSession(session: PersistedSession | null) {
  if (session === null) {
    window.localStorage.removeItem(STORAGE_KEY);
    return;
  }
  window.localStorage.setItem(STORAGE_KEY, JSON.stringify(session));
}

function fromTokenResponse(res: TokenResponse): PersistedSession {
  return {
    access_token: res.access_token,
    refresh_token: res.refresh_token,
    expires_at: Date.now() + res.expires_in * 1000,
  };
}

export function AuthProvider({ children }: { children: ReactNode }) {
  const [session, setSessionState] = useState<PersistedSession | null>(() => loadSession());
  const [user, setUser] = useState<UserProfile | null>(null);
  const [status, setStatus] = useState<AuthStatus>(session ? "loading" : "anonymous");

  const logout = useCallback(() => {
    saveSession(null);
    setSessionState(null);
    setUser(null);
    setStatus("anonymous");
  }, []);

  const refreshProfile = useCallback(async () => {
    if (!session) return;
    try {
      const profile = await authApi.me(session.access_token);
      setUser(profile);
      setStatus("authenticated");
    } catch (err) {
      if (err instanceof ApiError && err.status === 401) {
        logout();
      } else {
        setStatus("anonymous");
      }
    }
  }, [session, logout]);

  const setSession = useCallback(async (res: TokenResponse) => {
    const persisted = fromTokenResponse(res);
    saveSession(persisted);
    setSessionState(persisted);
    try {
      const profile = await authApi.me(persisted.access_token);
      setUser(profile);
      setStatus("authenticated");
    } catch {
      setUser(null);
      setStatus("anonymous");
    }
  }, []);

  useEffect(() => {
    if (!session) {
      setStatus("anonymous");
      return;
    }
    void refreshProfile();
  }, [session, refreshProfile]);

  const value = useMemo<AuthContextValue>(
    () => ({
      status,
      user,
      accessToken: session?.access_token ?? null,
      setSession,
      logout,
      refreshProfile,
    }),
    [status, user, session, setSession, logout, refreshProfile],
  );

  return <AuthContext value={value}>{children}</AuthContext>;
}

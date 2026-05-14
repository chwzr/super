import { createContext, useContext } from "react";
import type { TokenResponse, UserProfile } from "./api";

export type AuthStatus = "loading" | "authenticated" | "anonymous";

export interface AuthContextValue {
  status: AuthStatus;
  user: UserProfile | null;
  accessToken: string | null;
  setSession(res: TokenResponse): Promise<void>;
  logout(): void;
  refreshProfile(): Promise<void>;
}

export const AuthContext = createContext<AuthContextValue | null>(null);

export function useAuth(): AuthContextValue {
  const ctx = useContext(AuthContext);
  if (!ctx) {
    throw new Error("useAuth must be used inside <AuthProvider>");
  }
  return ctx;
}

import { type ReactNode } from "react";
import { Navigate } from "react-router";

import { useAuth } from "@/lib/auth-context";
import { Spinner } from "@/components/ui/spinner";

interface RequireAuthProps {
  children: ReactNode;
}

export function RequireAuth({ children }: RequireAuthProps) {
  const { status } = useAuth();
  if (status === "loading") {
    return (
      <div className="flex min-h-svh items-center justify-center text-muted-foreground">
        <Spinner data-icon="inline-start" />
        <span className="ml-2">Loading session…</span>
      </div>
    );
  }
  if (status === "anonymous") {
    return <Navigate to="/login" replace />;
  }
  return <>{children}</>;
}

export function RedirectIfAuthed({ children }: RequireAuthProps) {
  const { status } = useAuth();
  if (status === "authenticated") {
    return <Navigate to="/app" replace />;
  }
  return <>{children}</>;
}

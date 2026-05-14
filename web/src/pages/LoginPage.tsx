import { useState, type FormEvent } from "react";
import { Link, useNavigate } from "react-router";

import { ApiError, authApi } from "@/lib/api";
import { useAuth } from "@/lib/auth-context";
import { createPkcePair } from "@/lib/pkce";

import { Alert, AlertDescription, AlertTitle } from "@/components/ui/alert";
import { Button } from "@/components/ui/button";
import {
  Card,
  CardContent,
  CardDescription,
  CardFooter,
  CardHeader,
  CardTitle,
} from "@/components/ui/card";
import { Field, FieldDescription, FieldGroup, FieldLabel } from "@/components/ui/field";
import { Input } from "@/components/ui/input";
import { Spinner } from "@/components/ui/spinner";

export function LoginPage() {
  const navigate = useNavigate();
  const auth = useAuth();

  const [email, setEmail] = useState("");
  const [password, setPassword] = useState("");
  const [error, setError] = useState<string | null>(null);
  const [submitting, setSubmitting] = useState(false);

  async function onSubmit(event: FormEvent<HTMLFormElement>) {
    event.preventDefault();
    setError(null);
    setSubmitting(true);
    try {
      const pkce = await createPkcePair();
      const code = await authApi.login({
        email,
        password,
        codeChallenge: pkce.challenge,
        redirectUri: `${window.location.origin}/auth/callback`,
      });
      const tokens = await authApi.authorize({
        code,
        codeVerifier: pkce.verifier,
      });
      await auth.setSession(tokens);
      void navigate("/", { replace: true });
    } catch (err) {
      if (err instanceof ApiError) {
        setError(err.body || err.message);
      } else if (err instanceof Error) {
        setError(err.message);
      } else {
        setError("Unexpected error during login.");
      }
    } finally {
      setSubmitting(false);
    }
  }

  return (
    <div className="flex min-h-svh items-center justify-center px-4 py-10">
      <Card className="w-full max-w-md">
        <CardHeader>
          <CardTitle>Sign in to Super</CardTitle>
          <CardDescription>
            Use the credentials you registered with the Super platform server.
          </CardDescription>
        </CardHeader>
        <form onSubmit={onSubmit} noValidate>
          <CardContent>
            <FieldGroup>
              {error && (
                <Alert variant="destructive">
                  <AlertTitle>Sign in failed</AlertTitle>
                  <AlertDescription>{error}</AlertDescription>
                </Alert>
              )}
              <Field data-invalid={error ? true : undefined}>
                <FieldLabel htmlFor="email">Email</FieldLabel>
                <Input
                  id="email"
                  type="email"
                  autoComplete="email"
                  required
                  value={email}
                  onChange={(e) => setEmail(e.target.value)}
                  aria-invalid={error ? true : undefined}
                />
              </Field>
              <Field data-invalid={error ? true : undefined}>
                <FieldLabel htmlFor="password">Password</FieldLabel>
                <Input
                  id="password"
                  type="password"
                  autoComplete="current-password"
                  required
                  value={password}
                  onChange={(e) => setPassword(e.target.value)}
                  aria-invalid={error ? true : undefined}
                />
                <FieldDescription>
                  Forgot your password? Reset it via the CLI's{" "}
                  <code className="mono">super login</code> flow.
                </FieldDescription>
              </Field>
            </FieldGroup>
          </CardContent>
          <CardFooter className="flex flex-col gap-3">
            <Button type="submit" className="w-full" disabled={submitting}>
              {submitting && <Spinner data-icon="inline-start" />}
              Sign in
            </Button>
            <FieldDescription className="text-center">
              No account yet? <Link to="/register">Create one</Link>
            </FieldDescription>
          </CardFooter>
        </form>
      </Card>
    </div>
  );
}

import { useMemo, useState, type FormEvent } from "react";
import { Link, useNavigate } from "react-router";
import { toast } from "sonner";

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
import { Field, FieldDescription, FieldError, FieldGroup, FieldLabel } from "@/components/ui/field";
import { Input } from "@/components/ui/input";
import { Spinner } from "@/components/ui/spinner";

const PASSWORD_MIN = 8;

interface FieldErrors {
  email?: string;
  password?: string;
  confirm?: string;
}

function validate(email: string, password: string, confirm: string): FieldErrors {
  const errors: FieldErrors = {};
  if (!email) {
    errors.email = "Email is required.";
  } else if (!/^[^@\s]+@[^@\s]+\.[^@\s]+$/.test(email)) {
    errors.email = "Please enter a valid email address.";
  }
  if (!password) {
    errors.password = "Password is required.";
  } else if (password.length < PASSWORD_MIN) {
    errors.password = `Password must be at least ${PASSWORD_MIN} characters.`;
  }
  if (confirm !== password) {
    errors.confirm = "Passwords do not match.";
  }
  return errors;
}

export function RegisterPage() {
  const navigate = useNavigate();
  const auth = useAuth();

  const [email, setEmail] = useState("");
  const [password, setPassword] = useState("");
  const [confirm, setConfirm] = useState("");
  const [touched, setTouched] = useState<Record<keyof FieldErrors, boolean>>({
    email: false,
    password: false,
    confirm: false,
  });
  const [serverError, setServerError] = useState<string | null>(null);
  const [submitting, setSubmitting] = useState(false);

  const errors = useMemo(() => validate(email, password, confirm), [email, password, confirm]);

  async function onSubmit(event: FormEvent<HTMLFormElement>) {
    event.preventDefault();
    setTouched({ email: true, password: true, confirm: true });
    setServerError(null);
    if (Object.keys(errors).length > 0) {
      return;
    }
    setSubmitting(true);
    try {
      await authApi.register({ email, password });
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
      toast.success("Welcome to Super.");
      void navigate("/", { replace: true });
    } catch (err) {
      if (err instanceof ApiError) {
        setServerError(err.body || err.message);
      } else if (err instanceof Error) {
        setServerError(err.message);
      } else {
        setServerError("Unexpected error during registration.");
      }
    } finally {
      setSubmitting(false);
    }
  }

  function maybeError(key: keyof FieldErrors): string | undefined {
    return touched[key] ? errors[key] : undefined;
  }

  return (
    <div className="flex min-h-svh items-center justify-center px-4 py-10">
      <Card className="w-full max-w-md">
        <CardHeader>
          <CardTitle>Create your Super account</CardTitle>
          <CardDescription>
            One account works for the CLI, the web app, and any future client connecting to the
            platform server.
          </CardDescription>
        </CardHeader>
        <form onSubmit={onSubmit} noValidate>
          <CardContent>
            <FieldGroup>
              {serverError && (
                <Alert variant="destructive">
                  <AlertTitle>Registration failed</AlertTitle>
                  <AlertDescription>{serverError}</AlertDescription>
                </Alert>
              )}
              <Field data-invalid={maybeError("email") ? true : undefined}>
                <FieldLabel htmlFor="email">Email</FieldLabel>
                <Input
                  id="email"
                  type="email"
                  autoComplete="email"
                  required
                  value={email}
                  onChange={(e) => setEmail(e.target.value)}
                  onBlur={() => setTouched((t) => ({ ...t, email: true }))}
                  aria-invalid={maybeError("email") ? true : undefined}
                />
                <FieldError>{maybeError("email")}</FieldError>
              </Field>
              <Field data-invalid={maybeError("password") ? true : undefined}>
                <FieldLabel htmlFor="password">Password</FieldLabel>
                <Input
                  id="password"
                  type="password"
                  autoComplete="new-password"
                  required
                  value={password}
                  onChange={(e) => setPassword(e.target.value)}
                  onBlur={() => setTouched((t) => ({ ...t, password: true }))}
                  aria-invalid={maybeError("password") ? true : undefined}
                />
                <FieldDescription>
                  Use at least {PASSWORD_MIN} characters. Stored hashed with bcrypt by the platform
                  server.
                </FieldDescription>
                <FieldError>{maybeError("password")}</FieldError>
              </Field>
              <Field data-invalid={maybeError("confirm") ? true : undefined}>
                <FieldLabel htmlFor="confirm">Confirm password</FieldLabel>
                <Input
                  id="confirm"
                  type="password"
                  autoComplete="new-password"
                  required
                  value={confirm}
                  onChange={(e) => setConfirm(e.target.value)}
                  onBlur={() => setTouched((t) => ({ ...t, confirm: true }))}
                  aria-invalid={maybeError("confirm") ? true : undefined}
                />
                <FieldError>{maybeError("confirm")}</FieldError>
              </Field>
            </FieldGroup>
          </CardContent>
          <CardFooter className="flex flex-col gap-3">
            <Button type="submit" className="w-full" disabled={submitting}>
              {submitting && <Spinner data-icon="inline-start" />}
              Create account
            </Button>
            <FieldDescription className="text-center">
              Already registered? <Link to="/login">Sign in</Link>
            </FieldDescription>
          </CardFooter>
        </form>
      </Card>
    </div>
  );
}

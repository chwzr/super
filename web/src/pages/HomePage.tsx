import { Link } from "react-router";

import { useAuth } from "@/lib/auth-context";
import { Button } from "@/components/ui/button";
import {
  Card,
  CardContent,
  CardDescription,
  CardFooter,
  CardHeader,
  CardTitle,
} from "@/components/ui/card";
import { Spinner } from "@/components/ui/spinner";

export function HomePage() {
  const auth = useAuth();
  const { status, user } = auth;

  if (status === "loading") {
    return (
      <div className="flex min-h-svh items-center justify-center text-muted-foreground">
        <Spinner data-icon="inline-start" />
        <span className="ml-2">Loading session…</span>
      </div>
    );
  }

  if (status === "anonymous") {
    return (
      <div className="flex min-h-svh items-center justify-center px-4 py-10">
        <Card className="w-full max-w-md">
          <CardHeader>
            <CardTitle>Super</CardTitle>
            <CardDescription>The coding agent for engineers who think.</CardDescription>
          </CardHeader>
          <CardContent>
            <p className="text-sm text-muted-foreground">
              You are not signed in. Sign in to access the dashboard, or create a new account if
              this is your first time here.
            </p>
          </CardContent>
          <CardFooter className="flex flex-col gap-3">
            <Button asChild className="w-full">
              <Link to="/login">Sign in</Link>
            </Button>
            <Button asChild variant="outline" className="w-full">
              <Link to="/register">Create account</Link>
            </Button>
          </CardFooter>
        </Card>
      </div>
    );
  }

  return (
    <div className="flex min-h-svh items-center justify-center px-4 py-10">
      <Card className="w-full max-w-lg">
        <CardHeader>
          <CardTitle>Welcome back</CardTitle>
          <CardDescription>
            Signed in as <span className="mono">{user?.email}</span>
          </CardDescription>
        </CardHeader>
        <CardContent>
          <p className="text-sm text-muted-foreground">
            The full dashboard is on the way. For now the CLI is the primary surface — point it at
            this server and run <code className="mono">super login</code>.
          </p>
        </CardContent>
        <CardFooter>
          <Button variant="outline" onClick={() => auth.logout()}>
            Sign out
          </Button>
        </CardFooter>
      </Card>
    </div>
  );
}

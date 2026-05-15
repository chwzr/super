import { useAuth } from "@/lib/auth-context";

import { Logo } from "./Logo";

export function Nav() {
  const { status } = useAuth();
  const authed = status === "authenticated";
  return (
    <nav className="landing-nav">
      <div className="nav-inner">
        <Logo href="#top" />
        <div className="nav-links">
          <a href="#workflows">Workflows</a>
          <a href="#explain">Explain</a>
          <a href="#depth">Depth</a>
          <a href={authed ? "/app" : "/register"} className="btn btn-primary">
            {authed ? "Open app" : "Request an invite"}
          </a>
        </div>
      </div>
    </nav>
  );
}

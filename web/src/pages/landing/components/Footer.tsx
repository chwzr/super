import { Logo } from "./Logo";

export function Footer() {
  return (
    <footer className="landing-footer">
      <div className="footer-inner">
        <Logo fontSize={13} href="#top" />
        <div className="footer-links">
          <a href="#workflows">Workflows</a>
          <a href="#explain">Explain</a>
          <a href="#depth">Depth</a>
          <a href="https://github.com/chwzr/super">GitHub</a>
        </div>
        <div className="copyright mono">© 2026 · BUILT IN PUBLIC</div>
      </div>
    </footer>
  );
}

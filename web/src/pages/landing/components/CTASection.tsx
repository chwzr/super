export function CTASection() {
  return (
    <section className="cta-section bordered">
      <div className="container" style={{ maxWidth: 720 }}>
        <span className="h2-eyebrow">Early access</span>
        <h2 className="center">Bring deliberate engineering back to AI code.</h2>
        <p className="section-lede">
          Super is in private preview. Request an invite to get the v1 binary, the spec, and a seat
          in the early-access channel.
        </p>
        <div className="hero-actions">
          <a href="/register" className="btn btn-primary btn-lg">
            Request an invite
          </a>
          <a href="https://github.com/chwzr/super" className="btn btn-ghost btn-lg">
            Read PLAN.md →
          </a>
        </div>
      </div>
    </section>
  );
}

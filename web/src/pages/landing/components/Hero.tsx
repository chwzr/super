export function Hero() {
  return (
    <section className="hero" id="top">
      <div className="hero-content">
        <div className="eyebrow">
          <span className="dot" />
          Super CLI · Private preview
        </div>
        <h1>
          The Agent CLI that respects your{" "}
          <span className="accent">Brain&apos;s Context Window.</span>
        </h1>
        <p className="hero-tagline mono">Software engineering in the age of AI.</p>
        <p className="lede">
          Super is a coding agent harness that brainstorms with you, then turns your vision into a
          sequence of small presentations — design specs, implementation plans, steps — asks for
          each one for your sign-off, then builds exactly what you approved.
        </p>
        <div className="hero-actions">
          <a href="/register" className="btn btn-primary btn-lg">
            Request an invite
          </a>
          <a href="#explain" className="btn btn-ghost btn-lg">
            See /explain in action →
          </a>
        </div>
      </div>
    </section>
  );
}

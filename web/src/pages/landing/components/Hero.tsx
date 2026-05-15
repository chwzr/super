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
        <p className="hero-tagline mono">Software Engineering, baked in.</p>
        <p className="lede">
          A coding agent built for engineers who want to think about what their tools produce. Every
          action recorded as a discrete step. Every decision explainable in plain English. Every
          phase ending in a checkpoint you can actually read.
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

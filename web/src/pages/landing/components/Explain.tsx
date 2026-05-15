export function Explain() {
  return (
    <section className="section bordered" id="explain">
      <div className="container">
        <div className="explain-grid">
          <div>
            <span className="h2-eyebrow">/explain</span>
            <h2>Learn while you build.</h2>
            <p className="section-lede">
              Type{" "}
              <span className="mono" style={{ color: "var(--apple-blue-hi)" }}>
                /explain
              </span>{" "}
              on any change, any tool call, any line of generated code. Super walks you through the
              reasoning, the alternatives it considered, and the tradeoffs it made — at whatever
              depth you ask for.
            </p>
            <p className="section-lede">
              It&apos;s not autocomplete. It&apos;s a senior engineer narrating their work over your
              shoulder, in your terminal, on your code.
            </p>
            <div className="hero-actions" style={{ marginTop: 28, justifyContent: "flex-start" }}>
              <a href="#" className="btn btn-ghost">
                Read the /explain spec →
              </a>
            </div>
          </div>

          <div className="terminal">
            <div className="terminal-bar">
              <span className="dot active" />
              <span className="dot" />
              <span className="dot" />
              <span className="label">super — ~/projects/api</span>
            </div>
            <div className="terminal-body">
              <div>
                <span className="term-prompt">▸</span>
                <span className="term-cmd">/explain why a channel instead of a mutex?</span>
              </div>
              <div className="term-spacer" />
              <div className="term-out">Two reasons it lands here:</div>
              <div className="term-spacer" />
              <div className="term-out">
                1. <span className="term-accent">Backpressure</span> — the HTTP handler can
              </div>
              <div className="term-out">
                &nbsp;&nbsp;&nbsp;outpace the writer under load. A bounded
              </div>
              <div className="term-out">
                &nbsp;&nbsp;&nbsp;channel makes the limit explicit; a mutex
              </div>
              <div className="term-out">
                &nbsp;&nbsp;&nbsp;would silently queue and surprise you.
              </div>
              <div className="term-spacer" />
              <div className="term-out">
                2. <span className="term-accent">Cancellation</span> — closing the channel
              </div>
              <div className="term-out">
                &nbsp;&nbsp;&nbsp;cleanly drains in-flight writes during
              </div>
              <div className="term-out">
                &nbsp;&nbsp;&nbsp;shutdown. Covered in{" "}
                <span className="term-dim">writer_test.rs:84</span>.
              </div>
              <div className="term-spacer" />
              <div className="term-comment"># considered, rejected:</div>
              <div className="term-muted">
                &nbsp;&nbsp;· <span className="term-dim">Mutex&lt;VecDeque&gt;</span> — no
                backpressure signal
              </div>
              <div className="term-muted">
                &nbsp;&nbsp;· <span className="term-dim">tokio::sync::Semaphore</span> — fine, but
                two
              </div>
              <div className="term-muted">
                &nbsp;&nbsp;&nbsp;&nbsp;primitives where one would do
              </div>
              <div className="term-spacer" />
              <div>
                <span className="term-prompt">▸</span>
                <span className="term-cursor" />
              </div>
            </div>
          </div>
        </div>
      </div>
    </section>
  );
}

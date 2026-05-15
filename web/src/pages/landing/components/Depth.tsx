interface Card {
  icon: string;
  title: string;
  body: string;
}

const CARDS: Card[] = [
  {
    icon: "≡",
    title: "Thinking blocks, not black boxes",
    body: "Every plan, every revision, every rejected approach is captured inline. Collapsed by default; one keystroke away when you want to look.",
  },
  {
    icon: "↺",
    title: "Stop, edit, resume",
    body: "Disagree with a step? Edit it. Redirect it. Super merges your intent into the running plan without resetting the session or losing what you've built.",
  },
  {
    icon: "∗",
    title: "Skills, baked in",
    body: "Superpowers ships in the binary. Test-driven development, systematic debugging, plan-execute-review — first-class discipline, not bolt-on prompts.",
  },
  {
    icon: "⌘",
    title: "Remote control",
    body: "Drive your CLI session from the web or — soon — your phone. Same agent, same trail, same review checkpoints. No state lives in the client.",
  },
  {
    icon: "⎈",
    title: "Sandbox-ready",
    body: "Run Super headless inside an E2B sandbox, mounted against any Git repo. The trail comes back to you. The blast radius doesn't.",
  },
  {
    icon: "⊕",
    title: "One provider, scoped credentials",
    body: "Routed through OpenRouter via a managed key with quota. No raw provider tokens on your laptop, no surprise bills, no provider lock-in to manage.",
  },
];

export function Depth() {
  return (
    <section className="section bordered" id="depth">
      <div className="container">
        <div className="section-header">
          <span className="h2-eyebrow">Built for depth</span>
          <h2>Generated code you actually understand.</h2>
          <p className="section-lede">
            Speed-to-merge is a low bar. Super is built for the engineers who want to think deeply
            about what their tools produce — and what they&apos;re putting their name on.
          </p>
        </div>

        <div className="grid">
          {CARDS.map((card) => (
            <div key={card.title} className="card">
              <div className="card-icon">{card.icon}</div>
              <h3>{card.title}</h3>
              <p>{card.body}</p>
            </div>
          ))}
        </div>
      </div>
    </section>
  );
}

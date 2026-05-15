interface Card {
  icon: string;
  title: string;
  body: React.ReactNode;
}

const CARDS: Card[] = [
  {
    icon: "◆",
    title: "A trail, not a diff",
    body: "Every action lives as a discrete step with its inputs, its outputs, and the reasoning that produced it. Review the path. Don't just rubber-stamp the patch.",
  },
  {
    icon: "⟡",
    title: "Checkpoint by default",
    body: "Pause the agent at any tool boundary. Edit the plan. Redirect intent. Replay from any checkpoint without losing context — or accidentally agreeing to something you didn't read.",
  },
  {
    icon: "⟢",
    title: "Behavioral parity",
    body: (
      <>
        The exact toolset, context strategy, and subagent semantics you already know from Claude
        Code — with the{" "}
        <span className="mono" style={{ color: "var(--apple-blue-hi)" }}>
          Superpowers
        </span>{" "}
        plugin built in, not bolted on.
      </>
    ),
  },
];

interface Step {
  ix: string;
  tool: string;
  what: React.ReactNode;
  meta: React.ReactNode;
  kind?: "thinking" | "checkpoint";
}

const STEPS: Step[] = [
  { ix: "01", tool: "read_file", what: "server/src/writer.rs", meta: "142 ms" },
  {
    ix: "02",
    tool: "thinking…",
    what: "three approaches to backpressure — channel vs. mutex vs. semaphore",
    meta: "expand ▾",
    kind: "thinking",
  },
  {
    ix: "03",
    tool: "checkpoint",
    what: "plan ready · 4 steps · awaiting your review",
    meta: "paused",
    kind: "checkpoint",
  },
  {
    ix: "04",
    tool: "edit_file",
    what: "server/src/writer.rs · introduce bounded channel",
    meta: "+18 / −6",
  },
  {
    ix: "05",
    tool: "run_tests",
    what: "cargo test -p server writer::",
    meta: <span className="term-success">7 passed</span>,
  },
];

export function Workflow() {
  return (
    <section className="section bordered" id="workflows">
      <div className="container">
        <div className="section-header">
          <span className="h2-eyebrow">Engineered for review</span>
          <h2>Workflows that show their work.</h2>
          <p className="section-lede">
            Most agents hide their reasoning behind one chaotic PR. Super exposes every step — tool
            calls, file reads, intent — as a structured trail you can audit, edit, and replay.
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

        <div className="workflow" aria-label="Example session trace">
          {STEPS.map((step) => {
            const cls =
              step.kind === "thinking"
                ? "step is-thinking"
                : step.kind === "checkpoint"
                  ? "step is-checkpoint"
                  : "step";
            return (
              <div key={step.ix} className={cls}>
                <span className="ix">{step.ix}</span>
                <span className="tool">{step.tool}</span>
                <span className="what">{step.what}</span>
                <span className="meta">{step.meta}</span>
              </div>
            );
          })}
        </div>
      </div>
    </section>
  );
}

interface Phase {
  num: string;
  title: string;
  pin?: string;
  body: string;
}

const PHASES: Phase[] = [
  {
    num: "PHASE 01",
    title: "Brainstorm",
    pin: "before code",
    body: "Before a keystroke of implementation, Super separates what you said from what you meant. Surfaces the requirements you haven't named, the constraints you're assuming, and the edge cases worth deciding now. You sign off on the brief — not the code.",
  },
  {
    num: "PHASE 02",
    title: "Plan in writing",
    body: "The work gets written down before it gets done. Files to touch, order of operations, decisions to defer. You read it like a checklist — small enough to keep in your head, specific enough to argue with. Approve, edit, or send it back.",
  },
  {
    num: "PHASE 03",
    title: "A failing test first",
    body: "A failing test before every change. The test names what should be true; the change makes it true. \"I'll add tests after\" is where bugs live — Super won't ship without them.",
  },
  {
    num: "PHASE 04",
    title: "Execute one step at a time",
    body: "Each step has explicit inputs and outputs. Each step ends before the next begins. You can pause, inspect, edit, or redirect at any boundary — and resume without rewinding context.",
  },
  {
    num: "PHASE 05",
    title: "Debug by hypothesis, not by vibe",
    body: "When something breaks, Super doesn't guess. It forms a hypothesis, names the evidence that would confirm or kill it, gathers that evidence, then fixes the root cause — not the symptom. The reasoning is in your history, not lost to a swap-out.",
  },
  {
    num: "PHASE 06",
    title: "Verify before claiming done",
    body: '"It should work" doesn\'t ship. Before marking a task complete, Super runs the relevant checks and shows you the output — type-check, tests, lint, manual verification. Green or red, but never assumed.',
  },
  {
    num: "PHASE 07",
    title: "Receive review as evidence",
    body: "When you push back, Super treats your feedback as evidence — not friction. Re-checks the assumptions behind the disputed line, surfaces what your objection implies elsewhere in the diff, and changes its mind when the argument is right.",
  },
];

export function Phases() {
  return (
    <section className="section bordered" id="discipline">
      <div className="container">
        <div className="section-header">
          <span className="h2-eyebrow">Workflow discipline</span>
          <h2>Workflows built to respect the token budget of your brain.</h2>
          <p className="section-lede">
            Engineering is a sequence of discrete decisions. Super treats it that way — work broken
            into named phases, each one short enough to hold in your head, each one ending with a
            choice you actually have context to make.
          </p>
        </div>

        <ol className="phases">
          {PHASES.map((phase) => (
            <li key={phase.num} className="phase">
              <span className="phase-num">{phase.num}</span>
              <div>
                <h3 className="phase-title">
                  {phase.title}
                  {phase.pin && <span className="pin">{phase.pin}</span>}
                </h3>
                <p>{phase.body}</p>
              </div>
            </li>
          ))}
        </ol>
      </div>
    </section>
  );
}

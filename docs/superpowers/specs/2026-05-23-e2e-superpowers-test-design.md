# E2E Superpowers Skill Chain Test

A tmux-driven E2E test that verifies super can run the full Superpowers
skill chain (brainstorming → writing-plans → executing-plans with subagents)
without Rust errors, persist/restore sessions, and handle the `/rename` command.

## Architecture

A single bash script at `cli/tests/e2e_superpowers.sh` that drives super via
tmux on an isolated socket. Each phase sends input, waits for stable output,
captures the pane, and validates assertions. The test script exits non-zero
on first failure.

### Test directory

`/tmp/super-e2e-test/` — created fresh per run, initialized as a git repo.

### Tmux setup

- Isolated socket: `super-e2e`
- Session name: `e2e`
- Pane size: 200x50
- `-f /dev/null` to avoid user tmux config interference

## Test Phases

### Phase 0 — Setup

1. `rm -rf /tmp/super-e2e-test && mkdir /tmp/super-e2e-test`
2. `cd /tmp/super-e2e-test && git init`
3. Build super if not already built (`cargo build --package super-cli`)
4. Start super in tmux: `tmux -L super-e2e -f /dev/null new-session -d -s e2e -x 200 -y 50 -e 'TERM=xterm-256color' /path/to/super`
5. Wait for splash screen (diamond ASCII art)

**Assertion:** Screen contains the diamond splash or input prompt.

### Phase 1 — Brainstorming Skill

1. Type the initial prompt: `create a cargo rust package which is a simple binary that adds numbers`
2. Send Enter
3. Wait for super to respond

Since super's `using-superpowers` auto-loads on startup, it should recognize
this as creative work and invoke the `brainstorming` skill. The skill will:

- Create Task items for its checklist
- Ask clarifying questions one at a time (e.g. about scope, error handling, etc.)
- The test answers each question concisely to keep the flow moving
- Eventually present a design for approval

**Test responses strategy:** The script maintains a small queue of pre-written
responses. After each "wait for stable", it sends the next response and
Enter. The queue:

1. `just a simple binary, no library crate, bare minimum`
2. `yes, exactly`
3. `no further requirements, keep it minimal`
4. `approved, proceed to write the spec`

If the screen appears to be asking for design approval (contains "look right"
or "approve"), the script sends an approval variant. The response queue is
small because brainstorming typically asks 2-3 clarifying questions before
presenting the design.

After the spec is written and committed, super will transition to writing-plans.
The script detects the transition by watching for plan-related content.

**Assertions:**
- No Rust panic output on screen
- Screen contains "brainstorming" or design-related content
- Screen contains Task items

### Phase 2 — Writing Plans

After the user approves the brainstorming spec, super transitions to
`writing-plans`. The test:

1. Watches for plan-related content (implementation steps, file lists)
2. When the plan is presented and super asks for approval, sends `approved, implement it`
3. Waits for super to begin implementation

**Assertions:**
- No Rust panic output
- Screen contains plan-related content (implementation steps, file list)
- Super transitions toward implementation (agent/subagent activity)

### Phase 3 — Subagent-Driven Development

After plan approval, super invokes `executing-plans` which uses
`subagent-driven-development` to dispatch implementation tasks.

The test lets subagents run (they create files in the test directory).
The test monitors for completion by:
- Waiting until the activity indicator goes idle
- Checking that files exist: `Cargo.toml`, `src/main.rs`

Then validates the generated code compiles:
```bash
cd /tmp/super-e2e-test && cargo build
```

**Assertions:**
- No Rust panic output
- `Cargo.toml` and `src/main.rs` exist
- `cargo build` succeeds (exit code 0)

### Phase 4 — Resume

1. Send `/exit` to quit super (or `C-c` if needed)
2. Wait for tmux session to end / process to exit
3. Kill any remaining tmux session
4. Start a new tmux session with `--resume`:
   `tmux -L super-e2e -f /dev/null new-session -d -s e2e -x 200 -y 50 -e 'TERM=xterm-256color' /path/to/super --resume`
5. Wait for resume picker modal to render

The resume picker should show the previous session (identified by the first
prompt text).

6. Send Enter to select the most recent session
7. Wait for conversation history to load

**Assertions:**
- Resume picker shows at least one session
- After selection, previous conversation is visible on screen
- No Rust panic output

### Phase 5 — Rename, Exit, Reload

1. Send `/rename e2e-test-session`
2. Send Enter
3. Wait for confirmation

**Assertion:** Screen contains "Session renamed to: e2e-test-session"

4. Send `/exit`
5. Wait for super to quit
6. Start a new tmux session with `--resume`
7. Wait for resume picker

**Assertion:** Resume picker shows "e2e-test-session" as the custom title

8. Select the session, verify history loads intact

**Assertion:** Full conversation history is visible, no errors

### Phase 6 — Cleanup

- Kill tmux session and server
- Optionally keep `/tmp/super-e2e-test/` for manual inspection

## Test Script Structure

```
cli/tests/e2e_superpowers.sh
```

Functions:
- `fail(msg)` — print FAIL and exit 1
- `pass(msg)` — print PASS
- `assert_contains(pattern)` — grep current capture for pattern
- `assert_no_pattern(pattern)` — ensure pattern NOT in capture
- `wait_for(pattern, timeout_secs)` — poll until pattern appears or timeout
- `send(text)` — tmux send-keys
- `send_enter()` — tmux send-keys Enter
- `capture()` — tmux capture-pane -p, store in variable
- `wait_stable()` — loop until screen stops changing

Each phase is a function returning 0 or 1. Main loops through phases
in order, exiting on first failure.

## Error Handling

- `set -euo pipefail` at script top
- `trap cleanup EXIT` to always tear down tmux
- Each phase has a timeout (default 120s, longer for subagent work: 300s)
- If a phase times out, capture current screen and fail with it visible

### Phase 1 Adaptive Strategy

The brainstorming skill's behavior depends on the model, so the script uses
pattern-based detection rather than hardcoded step counts:

- After sending the initial prompt, the script enters a response loop
- Each iteration: wait for stable output, capture, check patterns
- If screen contains a question mark and looks like a clarifying question →
  pop next response from queue and send it
- If screen contains "look right" / "approve" / "proceed" → send approval
- If screen transitions to writing-plans content → break out of loop
- Loop has a max of 8 iterations (safety valve against infinite loops)
- If response queue is exhausted before transition, send `proceed` as default

## Non-Goals

- Not testing specific UI rendering (colors, glyphs)
- Not testing every superpowers skill — only the core chain
- Not validating the quality of generated code beyond `cargo build` success
- Not testing across different terminal emulators or OSes

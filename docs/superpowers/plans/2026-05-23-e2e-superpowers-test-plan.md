# E2E Superpowers Skill Chain Test — Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Create a single bash script that drives super via tmux through the full Superpowers skill chain, verifying no Rust errors occur and session persistence works.

**Architecture:** A single self-contained bash script (`cli/tests/e2e_superpowers.sh`) using tmux on an isolated socket to drive super. Each phase is a function with assertions. The script uses `set -euo pipefail` and traps cleanup on exit.

**Tech Stack:** bash, tmux 3.x, cargo

---

### Task 1: Create the test script skeleton

**Files:**
- Create: `cli/tests/e2e_superpowers.sh`

- [ ] **Step 1: Write the script skeleton with all helper functions**

```bash
#!/usr/bin/env bash
set -euo pipefail

# --- Config ---
TEST_DIR="/tmp/super-e2e-test"
SOCK="super-e2e"
SESSION="e2e"
SUPER_BIN="$(cd "$(dirname "$0")/../.." && pwd)/target/debug/super"
PANELOG="/tmp/super-e2e-panelog.txt"

# --- Colors ---
RED='\033[0;31m'
GREEN='\033[0;32m'
YELLOW='\033[1;33m'
NC='\033[0m'

# --- State ---
CAPTURE=""
PASS_COUNT=0
FAIL_COUNT=0

# --- Helper: tmux wrapper ---
T() { tmux -L "$SOCK" -f /dev/null "$@"; }

# --- Capture current pane ---
capture() {
    CAPTURE="$(T capture-pane -t "$SESSION" -p 2>/dev/null || true)"
}

# --- Wait for screen to stop changing (max 30 polls, 0.3s each) ---
wait_stable() {
    local prev="" cur="" max="${1:-30}"
    for _ in $(seq 1 "$max"); do
        cur="$(T capture-pane -t "$SESSION" -p 2>/dev/null || true)"
        if [ "$cur" = "$prev" ] && [ -n "$cur" ]; then
            CAPTURE="$cur"
            return 0
        fi
        prev="$cur"
        sleep 0.3
    done
    CAPTURE="$cur"
    return 1  # timeout, but still set CAPTURE
}

# --- Wait until pattern appears in screen (timeout in seconds) ---
wait_for() {
    local pattern="$1" timeout="${2:-120}" elapsed=0 interval=1
    while [ "$elapsed" -lt "$timeout" ]; do
        capture
        if echo "$CAPTURE" | grep -qF "$pattern"; then
            return 0
        fi
        sleep "$interval"
        elapsed=$((elapsed + interval))
    done
    echo -e "${RED}Timeout waiting for: $pattern${NC}"
    echo "$CAPTURE"
    return 1
}

# --- Assertions ---
assert_contains() {
    local msg="$1" pattern="$2"
    capture
    if echo "$CAPTURE" | grep -qF "$pattern"; then
        echo -e "  ${GREEN}PASS${NC}: $msg"
        PASS_COUNT=$((PASS_COUNT + 1))
        return 0
    else
        echo -e "  ${RED}FAIL${NC}: $msg (pattern not found: '$pattern')"
        echo "--- screen ---"
        echo "$CAPTURE" | tail -80
        echo "--- end screen ---"
        FAIL_COUNT=$((FAIL_COUNT + 1))
        return 1
    fi
}

assert_no_pattern() {
    local msg="$1" pattern="$2"
    capture
    if echo "$CAPTURE" | grep -q "$pattern"; then
        echo -e "  ${RED}FAIL${NC}: $msg (unexpected pattern found: '$pattern')"
        echo "--- screen ---"
        echo "$CAPTURE" | tail -80
        echo "--- end screen ---"
        FAIL_COUNT=$((FAIL_COUNT + 1))
        return 1
    else
        echo -e "  ${GREEN}PASS${NC}: $msg"
        PASS_COUNT=$((PASS_COUNT + 1))
        return 0
    fi
}

panic_patterns='thread .* panicked|PANIC|panicked at|error\[|unwrapped on|called `Result::unwrap`'

assert_no_panic() {
    assert_no_pattern "No Rust panics" "$panic_patterns" || return 1
}

# --- Send input ---
send() {
    T send-keys -t "$SESSION" -l "$*" 2>/dev/null
}

send_enter() {
    T send-keys -t "$SESSION" Enter 2>/dev/null
}

# --- Send text and press Enter ---
type_line() {
    T send-keys -t "$SESSION" -l "$*" 2>/dev/null
    T send-keys -t "$SESSION" Enter 2>/dev/null
}

# --- Cleanup ---
cleanup() {
    T kill-session -t "$SESSION" 2>/dev/null || true
    # Don't kill server — subsequent phases may need it
    echo ""
    echo "=============================="
    echo -e "Results: ${GREEN}${PASS_COUNT} passed${NC}, ${RED}${FAIL_COUNT} failed${NC}"
    echo "=============================="
    if [ "$FAIL_COUNT" -gt 0 ]; then
        exit 1
    fi
}

trap cleanup EXIT

# --- Kill any leftover tmux from previous runs ---
nuke() {
    T kill-server 2>/dev/null || true
    sleep 0.2
}

echo "=== Super E2E Superpowers Test ==="
echo ""
```

- [ ] **Step 2: Make it executable and verify syntax**

Run: `bash -n cli/tests/e2e_superpowers.sh`
Expected: exit 0, no output.

- [ ] **Step 3: Commit**

```bash
git add cli/tests/e2e_superpowers.sh
git commit -m "feat: add e2e test script skeleton with helpers"
```

---

### Task 2: Implement Phase 0 — Setup

**Files:**
- Modify: `cli/tests/e2e_superpowers.sh` (append after echo line)

- [ ] **Step 1: Add Phase 0 function**

```bash
phase_0_setup() {
    echo "--- Phase 0: Setup ---"

    nuke

    echo "  Preparing test directory..."
    rm -rf "$TEST_DIR"
    mkdir -p "$TEST_DIR"
    cd "$TEST_DIR"
    git init
    cd - > /dev/null

    echo "  Building super..."
    local super_root
    super_root="$(cd "$(dirname "$0")/../.." && pwd)"
    cd "$super_root"
    cargo build --package super-cli 2>&1 | tail -5
    cd - > /dev/null

    if [ ! -x "$SUPER_BIN" ]; then
        echo -e "${RED}FAIL: super binary not found at $SUPER_BIN${NC}"
        return 1
    fi
    echo "  Super binary: $SUPER_BIN"

    echo "  Starting super in tmux..."
    T new-session -d -s "$SESSION" -x 200 -y 50 \
        -e "TERM=xterm-256color" \
        -c "$TEST_DIR" \
        "$SUPER_BIN"
    sleep 1.0

    capture
    echo "  Initial screen captured (${#CAPTURE} chars)"

    # Super splash should be visible, or at minimum the session is running
    if [ -z "$CAPTURE" ]; then
        echo -e "${RED}FAIL: empty capture after start${NC}"
        return 1
    fi

    echo -e "${GREEN}Phase 0 complete${NC}"
}
```

- [ ] **Step 2: Add phase invocation at end of script**

```bash
# --- Main ---
phase_0_setup || exit 1
```

- [ ] **Step 3: Verify syntax**

Run: `bash -n cli/tests/e2e_superpowers.sh`
Expected: exit 0.

- [ ] **Step 4: Run Phase 0 to verify it works**

Run: `bash cli/tests/e2e_superpowers.sh`
Expected: Phase 0 passes, super starts, screen captured.

- [ ] **Step 5: Commit**

```bash
git add cli/tests/e2e_superpowers.sh
git commit -m "feat: implement Phase 0 setup in e2e test"
```

---

### Task 3: Implement Phase 1 — Brainstorming interaction loop

**Files:**
- Modify: `cli/tests/e2e_superpowers.sh` (add function before main)

- [ ] **Step 1: Add Phase 1 function with adaptive response loop**

```bash
phase_1_brainstorming() {
    echo "--- Phase 1: Brainstorming Skill ---"

    # Build response queue
    local responses=(
        "just a simple binary, no library crate, bare minimum"
        "yes, exactly"
        "no further requirements, keep it minimal"
        "approved, proceed to write the spec"
    )
    local resp_idx=0

    echo "  Sending initial prompt..."
    type_line "create a cargo rust package which is a simple binary that adds numbers"
    sleep 2.0

    local max_rounds=12
    local round=0

    while [ "$round" -lt "$max_rounds" ]; do
        round=$((round + 1))
        echo "  Round $round: waiting for stable output..."
        wait_stable 40 || echo "  (wait_stable timed out, continuing)"

        assert_no_panic || return 1

        # Check if we've transitioned to writing plans
        if echo "$CAPTURE" | grep -qi "writing.plan\|implementation plan\|plan document"; then
            echo "  Detected transition to writing-plans phase"
            break
        fi

        # Check if brainstorming appears to be asking a question (contains '?')
        if echo "$CAPTURE" | grep -q '\?'; then
            if [ "$resp_idx" -lt "${#responses[@]}" ]; then
                local resp="${responses[$resp_idx]}"
                resp_idx=$((resp_idx + 1))
                echo "  Responding (q $resp_idx): $resp"
                type_line "$resp"
                sleep 1.5
            else
                echo "  Response queue exhausted, sending 'proceed'"
                type_line "proceed"
                sleep 1.5
            fi
            continue
        fi

        # Check for approval prompt (design presentation)
        if echo "$CAPTURE" | grep -qi "look right\|looks good\|approve\|proceed\|shall i"; then
            echo "  Design approval prompt detected, approving..."
            type_line "approved, proceed to write the spec"
            sleep 2.0
            continue
        fi

        # Check for task list rendering (brainstorming created tasks)
        if echo "$CAPTURE" | grep -q "Task\|task"; then
            echo "  Task items visible, waiting for question..."
            sleep 2.0
            continue
        fi

        # If we see "spec" and "design" content, brainstorming may have produced a design
        if echo "$CAPTURE" | grep -qi "spec\|design doc\|design document"; then
            echo "  Spec/design content visible..."
            if [ "$resp_idx" -lt "${#responses[@]}" ]; then
                local resp="${responses[$resp_idx]}"
                resp_idx=$((resp_idx + 1))
                type_line "$resp"
                sleep 1.5
            fi
            continue
        fi

        # Default: send next response or proceed
        if [ "$resp_idx" -lt "${#responses[@]}" ]; then
            local resp="${responses[$resp_idx]}"
            resp_idx=$((resp_idx + 1))
            echo "  Sending next response: $resp"
            type_line "$resp"
            sleep 1.5
        else
            echo "  No pattern matched, waiting more..."
            sleep 3.0
        fi
    done

    # Post-loop assertions
    assert_contains "Brainstorming or design content visible" "design\|brainstorming\|spec\|task\|Task" || true
    assert_no_panic || return 1

    echo -e "${GREEN}Phase 1 complete${NC} (rounds: $round)"
}
```

- [ ] **Step 2: Update main to call Phase 1**

```bash
# --- Main ---
phase_0_setup || exit 1
phase_1_brainstorming || exit 1
```

- [ ] **Step 3: Verify syntax**

Run: `bash -n cli/tests/e2e_superpowers.sh`
Expected: exit 0.

- [ ] **Step 4: Commit**

```bash
git add cli/tests/e2e_superpowers.sh
git commit -m "feat: implement Phase 1 brainstorming loop in e2e test"
```

---

### Task 4: Implement Phase 2 — Writing Plans

**Files:**
- Modify: `cli/tests/e2e_superpowers.sh` (add function)

- [ ] **Step 1: Add Phase 2 function**

```bash
phase_2_writing_plans() {
    echo "--- Phase 2: Writing Plans ---"

    local max_rounds=8
    local round=0

    while [ "$round" -lt "$max_rounds" ]; do
        round=$((round + 1))
        wait_stable 40

        assert_no_panic || return 1

        # Check for plan approval prompt
        if echo "$CAPTURE" | grep -qi "approve\|proceed\|shall i implement\|look right\|looks good"; then
            echo "  Plan approval detected, sending approval..."
            type_line "approved, implement it"
            sleep 2.0
        fi

        # Check for transition to implementation (agent/tool activity)
        if echo "$CAPTURE" | grep -qi "Agent\|agent\|subagent\|implement\|execution\|spawn\|dispatching"; then
            echo "  Implementation starting..."
            break
        fi

        # Check if plan is visible
        if echo "$CAPTURE" | grep -qi "implementation plan\|step\|task\|file.*create\|file.*modify"; then
            echo "  Plan content visible..."
            # Plan might still be writing, wait a bit
            sleep 3.0
            continue
        fi

        sleep 3.0
    done

    echo -e "${GREEN}Phase 2 complete${NC}"
}
```

- [ ] **Step 2: Update main to call Phase 2**

```bash
# --- Main ---
phase_0_setup || exit 1
phase_1_brainstorming || exit 1
phase_2_writing_plans || exit 1
```

- [ ] **Step 3: Verify syntax and commit**

Run: `bash -n cli/tests/e2e_superpowers.sh` then commit.

```bash
git add cli/tests/e2e_superpowers.sh
git commit -m "feat: implement Phase 2 writing-plans in e2e test"
```

---

### Task 5: Implement Phase 3 — Subagent Development + Code Validation

**Files:**
- Modify: `cli/tests/e2e_superpowers.sh` (add function)

- [ ] **Step 1: Add Phase 3 function**

```bash
phase_3_subagent_dev() {
    echo "--- Phase 3: Subagent-Driven Development ---"

    # Wait for subagents to complete — this is the longest phase
    # We poll for file existence and idle screen
    local max_wait=600 elapsed=0 interval=5

    while [ "$elapsed" -lt "$max_wait" ]; do
        capture

        assert_no_panic || return 1

        # Check if files exist
        if [ -f "$TEST_DIR/Cargo.toml" ] && [ -f "$TEST_DIR/src/main.rs" ]; then
            echo "  Files created after ${elapsed}s, waiting for screen to settle..."
            sleep 5.0
            wait_stable 20
            break
        fi

        sleep "$interval"
        elapsed=$((elapsed + interval))
        echo "  Waiting for file creation... (${elapsed}s / ${max_wait}s)"
    done

    if [ ! -f "$TEST_DIR/Cargo.toml" ]; then
        echo -e "${RED}FAIL: Cargo.toml was not created${NC}"
        capture
        echo "$CAPTURE" | tail -40
        return 1
    fi

    if [ ! -f "$TEST_DIR/src/main.rs" ]; then
        echo -e "${RED}FAIL: src/main.rs was not created${NC}"
        return 1
    fi

    echo "  Verifying generated code compiles..."
    cd "$TEST_DIR"
    if cargo build 2>&1 | tail -10; then
        echo -e "  ${GREEN}Build succeeded${NC}"
    else
        echo -e "${RED}FAIL: cargo build failed${NC}"
        cd - > /dev/null
        return 1
    fi
    cd - > /dev/null

    assert_no_panic || return 1
    echo -e "${GREEN}Phase 3 complete${NC}"
}
```

- [ ] **Step 2: Update main to call Phase 3**

```bash
# --- Main ---
phase_0_setup || exit 1
phase_1_brainstorming || exit 1
phase_2_writing_plans || exit 1
phase_3_subagent_dev || exit 1
```

- [ ] **Step 3: Verify syntax and commit**

Run: `bash -n cli/tests/e2e_superpowers.sh` then commit.

```bash
git add cli/tests/e2e_superpowers.sh
git commit -m "feat: implement Phase 3 subagent dev + build validation"
```

---

### Task 6: Implement Phase 4 — Resume

**Files:**
- Modify: `cli/tests/e2e_superpowers.sh` (add function)

- [ ] **Step 1: Add Phase 4 function**

```bash
phase_4_resume() {
    echo "--- Phase 4: Resume ---"

    echo "  Exiting super..."
    type_line "/exit"
    sleep 2.0

    # Wait for process to exit
    local max_wait=10 elapsed=0
    while T capture-pane -t "$SESSION" -p > /dev/null 2>&1; do
        sleep 0.5
        elapsed=$((elapsed + 1))
        [ "$elapsed" -ge "$max_wait" ] && break
    done
    T kill-session -t "$SESSION" 2>/dev/null || true
    sleep 0.5

    echo "  Restarting super with --resume..."
    T new-session -d -s "$SESSION" -x 200 -y 50 \
        -e "TERM=xterm-256color" \
        -c "$TEST_DIR" \
        "$SUPER_BIN" --resume
    sleep 1.5
    wait_stable 20

    capture
    echo "  Resume picker screen:"
    echo "$CAPTURE" | head -30

    # Should show session list or resume picker
    if ! echo "$CAPTURE" | grep -qi "session\|resume\|select"; then
        echo -e "  ${YELLOW}WARNING: Resume picker may not be visible${NC}"
    fi

    assert_no_panic || return 1

    echo "  Selecting session (Enter)..."
    send_enter
    sleep 2.0
    wait_stable 30

    capture
    echo "  After resume, screen size: ${#CAPTURE} chars"

    assert_no_panic || return 1

    # Should have loaded conversation history (screen has substantial content)
    if [ "${#CAPTURE}" -lt 200 ]; then
        echo -e "  ${RED}FAIL: Screen too small after resume — history may not have loaded${NC}"
        echo "--- screen ---"
        echo "$CAPTURE"
        echo "--- end screen ---"
        return 1
    fi
    echo -e "  ${GREEN}PASS: Conversation history loaded (${#CAPTURE} chars)${NC}"

    echo -e "${GREEN}Phase 4 complete${NC}"
}
```

- [ ] **Step 2: Update main to call Phase 4**

```bash
# --- Main ---
phase_0_setup || exit 1
phase_1_brainstorming || exit 1
phase_2_writing_plans || exit 1
phase_3_subagent_dev || exit 1
phase_4_resume || exit 1
```

- [ ] **Step 3: Verify syntax and commit**

Run: `bash -n cli/tests/e2e_superpowers.sh` then commit.

```bash
git add cli/tests/e2e_superpowers.sh
git commit -m "feat: implement Phase 4 resume in e2e test"
```

---

### Task 7: Implement Phase 5 — Rename, Exit, Reload

**Files:**
- Modify: `cli/tests/e2e_superpowers.sh` (add function)

- [ ] **Step 1: Add Phase 5 function**

```bash
phase_5_rename() {
    echo "--- Phase 5: Rename, Exit, Reload ---"

    echo "  Sending /rename command..."
    type_line "/rename e2e-test-session"
    sleep 1.0
    wait_stable 10

    assert_contains "Rename confirmation" "e2e-test-session" || return 1
    assert_no_panic || return 1

    echo "  Exiting super..."
    type_line "/exit"
    sleep 2.0

    local max_wait=10 elapsed=0
    while T capture-pane -t "$SESSION" -p > /dev/null 2>&1; do
        sleep 0.5
        elapsed=$((elapsed + 1))
        [ "$elapsed" -ge "$max_wait" ] && break
    done
    T kill-session -t "$SESSION" 2>/dev/null || true
    sleep 0.5

    echo "  Restarting super with --resume to verify rename..."
    T new-session -d -s "$SESSION" -x 200 -y 50 \
        -e "TERM=xterm-256color" \
        -c "$TEST_DIR" \
        "$SUPER_BIN" --resume
    sleep 1.5
    wait_stable 20

    capture
    echo "  Resume picker after rename:"
    echo "$CAPTURE" | head -30

    assert_contains "Custom title in picker" "e2e-test-session" || return 1
    assert_no_panic || return 1

    echo "  Selecting renamed session..."
    send_enter
    sleep 2.0
    wait_stable 30

    assert_no_panic || return 1

    # Verify history loaded intact for renamed session
    if [ "${#CAPTURE}" -lt 200 ]; then
        echo -e "  ${RED}FAIL: Screen too small after rename+resume — history may not have loaded${NC}"
        echo "--- screen ---"
        echo "$CAPTURE"
        echo "--- end screen ---"
        return 1
    fi
    echo -e "  ${GREEN}PASS: Renamed session history loaded (${#CAPTURE} chars)${NC}"

    echo -e "${GREEN}Phase 5 complete${NC}"
}
```

- [ ] **Step 2: Update main to call Phase 5**

```bash
# --- Main ---
phase_0_setup || exit 1
phase_1_brainstorming || exit 1
phase_2_writing_plans || exit 1
phase_3_subagent_dev || exit 1
phase_4_resume || exit 1
phase_5_rename || exit 1

echo ""
echo -e "${GREEN}All phases passed!${NC}"
```

- [ ] **Step 3: Verify syntax and commit**

Run: `bash -n cli/tests/e2e_superpowers.sh` then commit.

```bash
git add cli/tests/e2e_superpowers.sh
git commit -m "feat: implement Phase 5 rename/exit/reload in e2e test"
```

---

### Task 8: Full end-to-end verification

- [ ] **Step 1: Run complete test script**

Run: `bash cli/tests/e2e_superpowers.sh`
Expected: All phases pass, exit code 0, no Rust panics detected.

- [ ] **Step 2: Fix any issues found during the run**

Capture screen output on failures, adjust patterns or timeouts as needed.

- [ ] **Step 3: Commit any fixes**

```bash
git add cli/tests/e2e_superpowers.sh
git commit -m "fix: tune e2e test timeouts and patterns after full run"
```

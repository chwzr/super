#!/usr/bin/env bash
set -euo pipefail

# --- Config ---
TEST_DIR="/tmp/super-e2e-test"
SOCK="super-e2e"
SESSION="e2e"
SCRIPT_DIR="$(cd "$(dirname "$0")" && pwd)"
SUPER_ROOT="$(cd "$SCRIPT_DIR/../.." && pwd)"
SUPER_BIN="$SUPER_ROOT/target/debug/super"
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
    sleep 0.5
}

echo "=== Super E2E Superpowers Test ==="
echo ""

phase_0_setup() {
    echo "--- Phase 0: Setup ---"

    nuke

    echo "  Preparing test directory..."
    rm -rf "$TEST_DIR"
    mkdir -p "$TEST_DIR"
    pushd "$TEST_DIR" > /dev/null
    git init
    popd > /dev/null

    echo "  Building super..."
    cd "$SUPER_ROOT"
    cargo build --package super-cli 2>&1 | tail -5
    cd "$SCRIPT_DIR" > /dev/null  # keep cwd near script

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
    sleep 3.0

    # Retry capture if empty (super may still be loading)
    local retries=0
    while [ "$retries" -lt 5 ]; do
        capture
        if [ -n "$CAPTURE" ]; then break; fi
        retries=$((retries + 1))
        sleep 1.0
    done

    echo "  Initial screen captured (${#CAPTURE} chars) after ${retries} retries"

    if [ -z "$CAPTURE" ]; then
        echo -e "${RED}FAIL: empty capture after start${NC}"
        return 1
    fi

    echo -e "${GREEN}Phase 0 complete${NC}"
}

phase_1_brainstorming() {
    echo "--- Phase 1: Brainstorming Skill ---"

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

        if echo "$CAPTURE" | grep -qi "writing.plan\|implementation plan\|plan document"; then
            echo "  Detected transition to writing-plans phase"
            break
        fi

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

        if echo "$CAPTURE" | grep -qi "look right\|looks good\|approve\|proceed\|shall i"; then
            echo "  Design approval prompt detected, approving..."
            type_line "approved, proceed to write the spec"
            sleep 2.0
            continue
        fi

        if echo "$CAPTURE" | grep -q "Task\|task"; then
            echo "  Task items visible, waiting for question..."
            sleep 2.0
            continue
        fi

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

    assert_no_panic || return 1

    echo -e "${GREEN}Phase 1 complete${NC} (rounds: $round)"
}

phase_2_writing_plans() {
    echo "--- Phase 2: Writing Plans ---"

    local max_rounds=8
    local round=0

    while [ "$round" -lt "$max_rounds" ]; do
        round=$((round + 1))
        wait_stable 40

        assert_no_panic || return 1

        if echo "$CAPTURE" | grep -qi "approve\|proceed\|shall i implement\|look right\|looks good"; then
            echo "  Plan approval detected, sending approval..."
            type_line "approved, implement it"
            sleep 2.0
        fi

        if echo "$CAPTURE" | grep -qi "Agent\|agent\|subagent\|implement\|execution\|spawn\|dispatching"; then
            echo "  Implementation starting..."
            break
        fi

        if echo "$CAPTURE" | grep -qi "implementation plan\|step\|task\|file.*create\|file.*modify"; then
            echo "  Plan content visible..."
            sleep 3.0
            continue
        fi

        sleep 3.0
    done

    echo -e "${GREEN}Phase 2 complete${NC}"
}

phase_3_subagent_dev() {
    echo "--- Phase 3: Subagent-Driven Development ---"

    local max_wait=600 elapsed=0 interval=5

    while [ "$elapsed" -lt "$max_wait" ]; do
        capture

        assert_no_panic || return 1

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
    pushd "$TEST_DIR" > /dev/null
    if cargo build 2>&1 | tail -10; then
        echo -e "  ${GREEN}Build succeeded${NC}"
    else
        echo -e "${RED}FAIL: cargo build failed${NC}"
        popd > /dev/null
        return 1
    fi
    popd > /dev/null

    assert_no_panic || return 1
    echo -e "${GREEN}Phase 3 complete${NC}"
}

phase_4_resume() {
    echo "--- Phase 4: Resume ---"

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

# --- Main ---
phase_0_setup || exit 1
phase_1_brainstorming || exit 1
phase_2_writing_plans || exit 1
phase_3_subagent_dev || exit 1
phase_4_resume || exit 1
phase_5_rename || exit 1

echo ""
echo -e "${GREEN}All phases passed!${NC}"

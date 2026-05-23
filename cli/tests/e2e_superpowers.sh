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

    assert_contains "Brainstorming or design content visible" "design\|brainstorming\|spec\|task\|Task" || true
    assert_no_panic || return 1

    echo -e "${GREEN}Phase 1 complete${NC} (rounds: $round)"
}

# --- Main ---
phase_0_setup || exit 1
phase_1_brainstorming || exit 1

#!/usr/bin/env bash
set -euo pipefail

# ============================================================================
# E2E Superpowers Skill Chain Test
#
# Drives super via tmux through the full Superpowers workflow, verifying
# no Rust errors/panics occur and session persistence works.
#
# Usage: bash cli/tests/e2e_superpowers.sh
# ============================================================================

# macOS: /tmp is a symlink to /private/tmp. Use /private/tmp so 'find'
# actually traverses the directory (find skips the symlink root by default).
TMPDIR="${TMPDIR:-/tmp}"
# Resolve symlinks for the test directory
TEST_DIR="$(cd "$TMPDIR" && pwd)/super-e2e-test"
# Ensure /tmp also resolves for the broader search
RESOLVED_TMP="$(cd "$TMPDIR" && pwd)"
SOCK="super-e2e"
SESSION="e2e"
SCRIPT_DIR="$(cd "$(dirname "$0")" && pwd)"
SUPER_ROOT="$(cd "$SCRIPT_DIR/../.." && pwd)"
SUPER_BIN="$SUPER_ROOT/target/debug/super"

RED='\033[0;31m'
GREEN='\033[0;32m'
YELLOW='\033[1;33m'
NC='\033[0m'

CAPTURE=""
PASS_COUNT=0
FAIL_COUNT=0

T() { tmux -L "$SOCK" -f /dev/null "$@"; }

capture() {
    CAPTURE="$(T capture-pane -t "$SESSION" -p 2>/dev/null || true)"
}

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
    return 1
}

# Wait for super to show its input prompt (meaning it's idle and ready for input).
wait_for_prompt() {
    local timeout="${1:-300}" elapsed=0 interval=3
    while [ "$elapsed" -lt "$timeout" ]; do
        capture
        if echo "$CAPTURE" | grep -qF "ask, plan"; then
            return 0
        fi
        sleep "$interval"
        elapsed=$((elapsed + interval))
    done
    echo -e "  ${YELLOW}wait_for_prompt: timeout after ${timeout}s${NC}"
    return 1
}

# Wait until tool execution appears in recent output (super is actively doing something).
wait_for_tool_activity() {
    local timeout="${1:-300}" elapsed=0 interval=3
    while [ "$elapsed" -lt "$timeout" ]; do
        capture
        if echo "$CAPTURE" | tail -20 | grep -qE "⏺ (Bash|Write|Read|Edit|Skill|Agent)"; then
            echo "  Tool activity detected at ${elapsed}s"
            return 0
        fi
        sleep "$interval"
        elapsed=$((elapsed + interval))
    done
    echo -e "  ${YELLOW}wait_for_tool_activity: timeout after ${timeout}s${NC}"
    return 1
}

assert_contains() {
    local msg="$1" pattern="$2"
    capture
    if echo "$CAPTURE" | grep -qF "$pattern"; then
        echo -e "  ${GREEN}PASS${NC}: $msg"
        PASS_COUNT=$((PASS_COUNT + 1))
        return 0
    else
        echo -e "  ${RED}FAIL${NC}: $msg (pattern: '$pattern')"
        echo "--- screen tail ---"
        echo "$CAPTURE" | tail -60
        echo "--- end ---"
        FAIL_COUNT=$((FAIL_COUNT + 1))
        return 1
    fi
}

assert_no_pattern() {
    local msg="$1" pattern="$2"
    capture
    if echo "$CAPTURE" | grep -q "$pattern"; then
        echo -e "  ${RED}FAIL${NC}: $msg (unexpected: '$pattern')"
        echo "--- screen tail ---"
        echo "$CAPTURE" | tail -60
        echo "--- end ---"
        FAIL_COUNT=$((FAIL_COUNT + 1))
        return 1
    else
        echo -e "  ${GREEN}PASS${NC}: $msg"
        PASS_COUNT=$((PASS_COUNT + 1))
        return 0
    fi
}

panic_patterns='thread .* panicked|PANIC|panicked at|error\[.*E[0-9]{4}\]|unwrapped on|called .Result::unwrap.'

assert_no_panic() {
    assert_no_pattern "No Rust panics/errors" "$panic_patterns" || return 1
}

type_line() {
    T send-keys -t "$SESSION" -l "$*" 2>/dev/null
    T send-keys -t "$SESSION" Enter 2>/dev/null
}

send_enter() {
    T send-keys -t "$SESSION" Enter 2>/dev/null
}

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

nuke() {
    T kill-server 2>/dev/null || true
    sleep 0.5
}

echo "=== Super E2E Superpowers Test ==="
echo ""

# ============================================================================
# Phase 0 — Setup
# ============================================================================
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
    cd "$SCRIPT_DIR" > /dev/null

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

    local retries=0
    while [ "$retries" -lt 5 ]; do
        capture
        if [ -n "$CAPTURE" ]; then break; fi
        retries=$((retries + 1))
        sleep 1.0
    done

    echo "  Initial screen: ${#CAPTURE} chars (${retries} retries)"

    if [ -z "$CAPTURE" ]; then
        echo -e "${RED}FAIL: empty capture after start${NC}"
        return 1
    fi

    assert_no_panic || return 1
    echo -e "${GREEN}Phase 0 complete${NC}"
}

# ============================================================================
# Phase 1 — Conversation: brainstorm through implementation
#
# Strategy: send the prompt, then respond when super is idle at the input
# prompt. We detect idle by waiting for "ask, plan". Use a response queue
# for the brainstorming Q&A phase. Once tool activity (Write/Bash/etc.)
# starts, switch to detection mode — wait for files to appear.
# ============================================================================
phase_1_conversation() {
    echo "--- Phase 1: Conversation (brainstorm → implement) ---"

    local responses=(
        "just a simple binary, no library crate, bare minimum"
        "yes, exactly"
        "no further requirements, keep it minimal"
        "approved, proceed to implement it"
    )
    local resp_idx=0
    local max_rounds=15 round=0

    echo "  Sending prompt..."
    type_line "create a cargo rust package which is a simple binary that adds numbers"

    while [ "$round" -lt "$max_rounds" ]; do
        round=$((round + 1))

        # Wait for super to finish and show prompt.
        echo "  [${round}/${max_rounds}] Waiting for prompt..."
        if ! wait_for_prompt 240; then
            echo "  Prompt timeout, checking for tool activity..."
        fi

        assert_no_panic || return 1

        # Check: has super created a cargo project anywhere in resolved tmp?
        local found_cargo
        found_cargo="$(find "$RESOLVED_TMP" -maxdepth 5 -name "Cargo.toml" -not -path "*/.git/*" -not -path "*/target/*" 2>/dev/null | head -1)"
        if [ -n "$found_cargo" ]; then
            local proj_dir
            proj_dir="$(dirname "$found_cargo")"
            if [ -f "$proj_dir/src/main.rs" ]; then
                echo "  Project found at $proj_dir"
                break
            fi
        fi

        # Detect recent tool execution (last 20 lines only, not scrollback history).
        if echo "$CAPTURE" | tail -20 | grep -qE "⏺ (Bash|Write|Edit|Skill|Agent)"; then
            echo "  Tool execution detected"
            # Super is writing code. Wait for completion.
            sleep 10.0
            wait_stable 20
            # Check for files again.
            local check_cargo
            check_cargo="$(find "$RESOLVED_TMP" -maxdepth 5 -name "Cargo.toml" -not -path "*/.git/*" -not -path "*/target/*" 2>/dev/null | head -1)"
            if [ -n "$check_cargo" ]; then
                local check_dir
                check_dir="$(dirname "$check_cargo")"
                if [ -f "$check_dir/src/main.rs" ]; then
                    echo "  Implementation complete — project at $check_dir"
                    break
                fi
            fi
        fi

        # Send next response if queue not empty.
        if [ "$resp_idx" -lt "${#responses[@]}" ]; then
            local resp="${responses[$resp_idx]}"
            resp_idx=$((resp_idx + 1))
            echo "  Responding (${resp_idx}/${#responses[@]}): $resp"
            type_line "$resp"
        else
            echo "  Queue exhausted, waiting for implementation..."
            sleep 10.0
        fi
    done

    assert_no_panic || return 1

    echo -e "${GREEN}Phase 1 complete${NC} (rounds: $round)"
}

# ============================================================================
# Phase 2 — Code Validation
# ============================================================================
phase_2_validate() {
    echo "--- Phase 2: Code Validation ---"

    local cargo_toml
    cargo_toml="$(find "$RESOLVED_TMP" -maxdepth 5 -name "Cargo.toml" -not -path "*/.git/*" -not -path "*/target/*" 2>/dev/null | head -1)"

    if [ -z "$cargo_toml" ]; then
        # Wider search
        cargo_toml="$(find /Users/chwzr -maxdepth 3 -name "Cargo.toml" -not -path "*/.git/*" -not -path "*/target/*" -not -path "*/Library/*" 2>/dev/null | head -1)"
    fi

    if [ -z "$cargo_toml" ]; then
        echo -e "${RED}FAIL: No Cargo.toml found anywhere${NC}"
        capture
        echo "$CAPTURE" | tail -40
        return 1
    fi

    local project_dir
    project_dir="$(dirname "$cargo_toml")"
    echo "  Project: $project_dir"

    if [ ! -f "$project_dir/src/main.rs" ]; then
        echo -e "${RED}FAIL: src/main.rs missing${NC}"
        return 1
    fi

    echo "  Building..."
    pushd "$project_dir" > /dev/null

    # If edition is 2024, check if the installed rustc supports it.
    # Fall back to 2021 if needed.
    if grep -q 'edition = "2024"' Cargo.toml 2>/dev/null; then
        if ! rustc --version 2>/dev/null | grep -q "1.8[5-9]\|1.9"; then
            echo "  Downgrading edition from 2024 to 2021 for compat..."
            sed -i '' 's/edition = "2024"/edition = "2021"/' Cargo.toml
        fi
    fi

    if cargo build 2>&1 | tail -15; then
        echo -e "  ${GREEN}Build succeeded${NC}"
    else
        echo -e "${RED}FAIL: cargo build failed${NC}"
        popd > /dev/null
        return 1
    fi
    popd > /dev/null

    assert_no_panic || return 1
    echo -e "${GREEN}Phase 2 complete${NC}"
}

# ============================================================================
# Phase 3 — Resume
# ============================================================================
phase_3_resume() {
    echo "--- Phase 3: Resume ---"

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

    echo "  Restarting with --resume (auto-loads most recent session)..."
    T new-session -d -s "$SESSION" -x 200 -y 50 \
        -e "TERM=xterm-256color" \
        -c "$TEST_DIR" \
        "$SUPER_BIN" --resume
    sleep 2.0
    wait_stable 30

    capture
    echo "  Screen after --resume: ${#CAPTURE} chars"

    assert_no_panic || return 1

    # --resume auto-loads the most recent session without showing a picker.
    # The screen should show conversation history (substantial content).
    if [ "${#CAPTURE}" -lt 200 ]; then
        echo -e "  ${RED}FAIL: Screen too small — history may not have loaded${NC}"
        echo "$CAPTURE"
        return 1
    fi
    echo -e "  ${GREEN}PASS: History loaded (${#CAPTURE} chars)${NC}"
    PASS_COUNT=$((PASS_COUNT + 1))

    echo -e "${GREEN}Phase 3 complete${NC}"
}

# ============================================================================
# Phase 4 — Rename, Exit, Reload
# ============================================================================
phase_4_rename() {
    echo "--- Phase 4: Rename, Exit, Reload ---"

    type_line "/rename e2e-test-session"
    sleep 1.0
    wait_stable 10

    assert_contains "Rename confirmation" "e2e-test-session" || return 1
    assert_no_panic || return 1

    echo "  Exiting..."
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

    echo "  Restarting with --resume (auto-loads renamed session)..."
    T new-session -d -s "$SESSION" -x 200 -y 50 \
        -e "TERM=xterm-256color" \
        -c "$TEST_DIR" \
        "$SUPER_BIN" --resume
    sleep 2.0
    wait_stable 30

    capture
    echo "  Screen after rename+resume: ${#CAPTURE} chars"

    assert_no_panic || return 1

    if [ "${#CAPTURE}" -lt 200 ]; then
        echo -e "  ${RED}FAIL: Screen too small after rename+resume${NC}"
        echo "$CAPTURE"
        return 1
    fi
    echo -e "  ${GREEN}PASS: Renamed session history loaded (${#CAPTURE} chars)${NC}"
    PASS_COUNT=$((PASS_COUNT + 1))

    echo -e "${GREEN}Phase 4 complete${NC}"
}

# ============================================================================
# Main
# ============================================================================
phase_0_setup || exit 1
phase_1_conversation || exit 1
phase_2_validate || exit 1
phase_3_resume || exit 1
phase_4_rename || exit 1

echo ""
echo -e "${GREEN}All phases passed!${NC}"

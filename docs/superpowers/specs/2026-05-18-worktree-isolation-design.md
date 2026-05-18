# Worktree Isolation — EnterWorktree & ExitWorktree

## Summary

Replace the stub implementations of `EnterWorktree` and `ExitWorktree` with real git worktree isolation, matching Claude Code's behavior.

## Scope

**In v1**: core git worktree create/remove, CWD switching, session state management, and entering existing worktrees via `path` parameter.

**Out of scope for v1**: hook-based worktree support, tmux session management, sparse checkout, `.worktreeinclude` file copying, `settings.local.json` propagation.

## Session State

A module-level `OnceLock<RwLock<Option<WorktreeSession>>>` in `cli/src/utils/worktree.rs`:

```rust
pub struct WorktreeSession {
    pub original_cwd: PathBuf,
    pub worktree_path: PathBuf,
    pub worktree_name: String,
    pub worktree_branch: String,
    pub original_head_commit: String,
    pub session_id: String,
}
```

ExitWorktree is a no-op (informational message, no error) when no session is active.

## Git Operations

Direct subprocess calls via `tokio::process::Command`. A shared `git()` helper lives in `cli/src/utils/git.rs`:

- `GIT_TERMINAL_PROMPT=0`, `GIT_ASKPASS=""`, stdin closed — prevents credential prompts from hanging
- Returns `Result<String, GitError>` with stdout, exit code, and stderr

Key commands: `git worktree add -B`, `git worktree remove --force`, `git worktree list --porcelain`, `git branch -D`, `git rev-parse`, `git status --porcelain`, `git rev-list --count`, `git fetch origin`.

Base ref resolution: try reading `origin/<default-branch>` from `.git/refs` directly first; if missing, `git fetch origin <branch>`; fallback to `HEAD`.

## Worktree Utilities (`cli/src/utils/worktree.rs`)

- **`validate_worktree_slug(name)`** — per-segment allowlist `[a-zA-Z0-9._-]`, max 64 chars, no `.` or `..` segments, `/`-separated nesting flattened with `+`
- **`get_or_create_worktree(repo_root, slug)`** — fast resume: read `.git` pointer file for HEAD SHA; if missing, create: `mkdir .claude/worktrees/`, resolve base, `git worktree add -B worktree-<slug> <path> <base>`
- **`enter_existing_worktree(repo_root, path)`** — validate path is in `git worktree list` output, read HEAD, return session
- **`keep_worktree(session)`** — chdir to original_cwd, clear session global
- **`cleanup_worktree(session)`** — chdir to original_cwd, `git worktree remove --force`, `git branch -D`, clear session global
- **`count_worktree_changes(path, head_commit)`** — `git status --porcelain` + `git rev-list --count <base>..HEAD`

Worktree path: `.claude/worktrees/<flattened-slug>` (not `.super/worktrees/` — behavioral parity).

## EnterWorktree Tool (`cli/src/tools/enter_worktree.rs`)

**call() flow:**
1. Guard: error if already in a worktree session
2. Resolve main repo root via `find_canonical_git_root(cwd)` — if inside a worktree, chdir to main repo first
3. If `input.name` → validate slug, call `get_or_create_worktree`
4. If `input.path` → validate path in `git worktree list`, construct session from existing path
5. `process::chdir` into worktree, update atomic CWD, store session
6. Return message with path and branch

**Input schema** (extends existing):
- `name` (optional string) — mutually exclusive with `path`
- `path` (optional string) — mutually exclusive with `name`

**Flags**: `should_defer: true`, `is_destructive: false`, `is_concurrency_safe: false`

## ExitWorktree Tool (`cli/src/tools/exit_worktree.rs`)

**validate_input():**
- If no active session → `Ok` (no-op path in `call()` handles messaging)
- If `action == "remove"` and no `discard_changes` → run `count_worktree_changes`, if changes exist return `Err` listing them

**call() flow:**
1. Guard: if no session → return no-op message
2. If `action == "keep"` → `keep_worktree()`, return message with path
3. If `action == "remove"` → `cleanup_worktree()`, return message with discarded counts

**Input schema** (already correct):
- `action` (required): `"keep"` | `"remove"`
- `discard_changes` (optional bool, default false)

**Flags**: `should_defer: true`, `is_destructive: true` when action is `remove`

## CWD Mechanism

Tools currently read CWD from `std::env::current_dir()` at `ToolCallContext` construction time. For worktree CWD switches to be visible to subsequent tool calls, introduce an atomic CWD:

```rust
// cli/src/utils/cwd.rs
static CURRENT_CWD: OnceLock<RwLock<PathBuf>> = OnceLock::new();

pub fn get_cwd() -> PathBuf { /* read from static, default to env::current_dir */ }
pub fn set_cwd(path: PathBuf) { /* write static + process::chdir */ }
```

`ToolCallContext` reads from `get_cwd()` instead of `std::env::current_dir()`. EnterWorktree and ExitWorktree call `set_cwd()`.

## Files Changed

| File | Action |
|------|--------|
| `cli/src/utils/git.rs` | **New** — shared git subprocess helper |
| `cli/src/utils/worktree.rs` | **New** — session state, worktree operations |
| `cli/src/utils/cwd.rs` | **New** — atomic CWD management |
| `cli/src/utils/mod.rs` | **Edit** — add `pub mod git; pub mod worktree; pub mod cwd;` |
| `cli/src/tools/enter_worktree.rs` | **Edit** — implement `call()`, add `path` to schema |
| `cli/src/tools/exit_worktree.rs` | **Edit** — implement `call()` and `validate_input()` |
| `cli/src/tools/contract.rs` | **Edit** — read CWD from `get_cwd()` in `ToolCallContext` |

## Error Handling

| Scenario | Behavior |
|----------|----------|
| Not in a git repo | EnterWorktree returns error |
| Already in worktree | EnterWorktree returns error |
| Invalid slug | EnterWorktree returns error with specific message |
| `path` not registered | EnterWorktree returns error |
| ExitWorktree with no session | No-op informational result, not an error |
| `remove` with changes, no `discard_changes` | `validate_input` returns `Err`, model re-invokes |
| Git subprocess failure | Return error with stderr |
| Concurrent EnterWorktree | `RwLock` write lock ensures mutual exclusion |

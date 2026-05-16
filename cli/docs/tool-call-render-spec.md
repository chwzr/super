# Tool-call rendering — visual spec

Derived from observing Claude Code v2.1.143 in tmux on 2026-05-16. Each
scenario was triggered with an explicit prompt asking for parallel calls; the
captured renderings are reproduced verbatim below.

## TL;DR

Claude Code splits tool calls into **two render modes**:

1. **Collapsed batch** — used for *read-only* tools (`Read`, `Grep`, `Glob`,
   and any combination of them) regardless of count, including `N=1`. One
   single line with a class-level summary; no `⏺` glyph.
2. **Per-call block** — used for *mutating / side-effecting* tools (`Bash`,
   `Write`, `Edit`/`Update`, and presumably `MultiEdit`, `NotebookEdit`, …).
   One block per call with the familiar `⏺ Name(args)` + `⎿ result` pattern.

Mixed turns combine both: read-only calls collapse into one summary line,
mutating calls each get their own block, regardless of ordering inside the
assistant turn.

## Render mode 1 — collapsed batch (read-only family)

```
  Read 5 files (ctrl+o to expand)
  Read 2 files (ctrl+o to expand)
  Read 1 file  (ctrl+o to expand)
  Searched for 1 pattern, listed 1 directory (ctrl+o to expand)
```

Styling (ANSI palette, captured from screen):

- 2-space left indent. **No `⏺` prefix glyph.**
- Entire line in `fg 246` (dim gray).
- The numeric count is rendered **bold** while staying `fg 246`.
- Trailing hint `(ctrl+o to expand)` is part of the same dim-gray line.
- Singular/plural ("file" vs "files", "pattern" vs "patterns", …) follows
  English grammar.

Summary template per tool kind:

| Tool  | Singular              | Plural                |
| ----- | --------------------- | --------------------- |
| Read  | `Read 1 file`         | `Read N files`        |
| Grep  | `Searched for 1 pattern` | `Searched for N patterns` |
| Glob  | `listed 1 directory`  | `listed N directories` |

When the batch contains more than one *kind* of read-only tool, the fragments
are joined with a comma in the order they appeared in the assistant turn:

> `Searched for 1 pattern, listed 1 directory (ctrl+o to expand)`

(Captured from a single turn that emitted one `Grep` and one `Glob` in
parallel.)

## Render mode 2 — per-call block (mutating family)

```
⏺ Bash(echo a)
  ⎿  a

⏺ Write(c.txt)
  ⎿  Wrote 1 lines to c.txt
      1 hello

⏺ Update(a.txt)
  ⎿  Added 1 line, removed 1 line
      1 -hello
      1   No newline at end of file
      2 +hi
      3   No newline at end of file
```

### Common styling for all per-call blocks

- One blank line above each block.
- `⏺` in `fg 114` (green). (Super currently uses cyan — separate parity drift
  to fix in the same pass.)
- Tool name (display alias — see below) **bold** in default foreground.
- Args inside `(…)` — no extra styling. The path inside the parens is
  rendered as an **OSC8 terminal hyperlink** to the absolute file path so
  terminals that support it show a clickable underline.
- Result row: `  ⎿  ` corner glyph in `fg 246` dim gray, then the summary.
- Multi-line bodies continue indented under the corner glyph with 5-space
  alignment (matches the column after `⎿  `).

### Per-tool display names (`userFacingName`)

Claude renames the tool for the rendered label; the underlying tool ID stays
the same:

| Tool input                                       | Displayed as    |
| ------------------------------------------------ | --------------- |
| `Edit` with `old_string == ""` (insert/create)   | `Create`        |
| `Edit` of a file under the plans directory       | `Updated plan`  |
| `Edit` (any other case)                          | `Update`        |
| `Write` of a file under the plans directory      | `Updated plan`  |
| `Write` (any other case)                         | `Write`         |
| `Bash`                                           | `Bash`          |
| `Task`                                           | `Task` (uses grouping path B — different render) |

### Edit/Create result format (`Update(...)` / `Create(...)`)

Summary line (text in default fg, numerals bold):

- `Added N line[s], removed M line[s]` — when both > 0 (lowercase `r`).
- `Added N line[s]` — when only additions.
- `Removed M line[s]` — when only removals (capital `R`).
- Singular/plural follows English: `1 line`, `N lines`.

Below the summary, the structured diff hunks render in this format:

```
 <line#> -<removed content>     ← bg 52, fg 167 (red)
 <line#>  <context content>     ← dim, default fg
 <line#> +<added content>       ← bg 22, fg 77 (green)
```

- Line-number column is right-padded to align across the hunk.
- Hunk separators (`...`) appear when there are skipped regions.
- The entire diff body is indented 5 spaces from the message start (i.e.
  under `⎿  `).

### Write result format (`Write(...)`)

Summary line:

- `Wrote N line[s] to <relative path>` (number bold).

Below: the newly written content with line numbers, indented 5 spaces:

```
      1 hello
      2 world
```

(For very long files, Claude truncates and gates expansion behind
fullscreen; v1 of super can simply truncate to N lines with an ellipsis.)

### Bash result format

Verified against Claude Code v2.1.143 on 2026-05-16.

**Success, single line of output:**

```
⏺ Bash(echo hello)
  ⎿  hello
```

- `⏺` in `fg 114` (green).
- `  ⎿  ` corner in `fg 246` dim gray, then output in default fg.

**Success, multi-line output (≤3 lines shown):**

```
⏺ Bash(seq 1 8)
  ⎿  1
     2
     3
     … +5 lines (ctrl+o to expand)
```

- First 3 lines of stdout rendered in default fg, indented 5 spaces under
  `⎿` (line 1 follows the corner; lines 2–3 align at column 6).
- If total output exceeds 3 lines, append a faint truncation row:
  `     … +<K> lines (ctrl+o to expand)` where K = total_lines − 3.
- The truncation row uses ANSI faint (`ESC[2m`), no special background.
- The 3-line threshold matches Claude exactly; do not tune it.
- Pressing `ctrl+o` expands the full output inline (same affordance as the
  read/search collapse).

**Non-zero exit code (and/or stderr content):**

```
⏺ Bash(bash -c "echo ohno >&2; exit 2")
  ⎿  Error: Exit code 2
     ohno
```

- `⏺` switches to `fg 211` (orange/coral) — distinct from the success green.
- First result row is `Error: Exit code <N>` in `fg 211`.
- Subsequent rows contain the captured output (stderr first, then stdout if
  any) in `fg 211` to match the error color.
- Truncation rule (`… +K lines (ctrl+o to expand)`) applies the same way for
  long error output; the truncation row stays faint dim, not orange.
- `⎿` corner stays `fg 246` (dim gray) regardless of success/error.

### Quick summary table

| Scenario              | `⏺` color | Result body color | Truncation rule         |
| --------------------- | --------- | ----------------- | ----------------------- |
| Bash success, 1 line  | green 114 | default           | n/a                     |
| Bash success, ≤3 lines | green 114 | default           | n/a                     |
| Bash success, >3 lines | green 114 | default + faint `… +K lines` | first 3 + ellipsis |
| Bash non-zero exit    | orange 211 | orange 211 (`Error: Exit code N` first) | same `… +K lines` if >3 |

## What this means for super

Super currently renders **every** tool call (including `Read`) as a
per-call block. So:

- `Read` always shows `⏺ Read (/path/to/file)` instead of `Read N files`.
- Parallel reads expand into N blocks instead of a single summary.
- Mixed `Grep` + `Glob` parallel calls expand into multiple blocks instead of
  one comma-joined summary.

Required changes to reach parity (locked-in decisions from 2026-05-16):

1. **Group consecutive read-only tool calls within a single assistant turn**
   into one synthetic transcript item that renders as the dim-gray
   summary line. Tools to include in the group: `Read`, `Grep`, `Glob`
   (and future read-only family members — `WebFetch`/`WebSearch`/MCP
   read-style tools require a follow-up capture before being added).
2. **Keep per-call blocks** for `Bash`, `Write`, `Edit`, `MultiEdit`,
   `NotebookEdit`, `Task`, MCP tools (until evidence says otherwise).
3. **Collapse at any N including N=1.** A single `Read` renders as
   `Read 1 file (ctrl+o to expand)`. No path shown in the collapsed line.
4. **Include the `(ctrl+o to expand)` hint** and wire `ctrl+o` to actually
   toggle expansion of the currently-focused batch into individual
   per-call blocks (same per-call format as mutating tools, but still in
   dim gray).
5. Style: 2-space indent, no glyph, `fg 246` dim gray, count in bold.

## Numeric-count basis

"Read N files" counts tool *invocations*, not unique paths. (Reading the same
path twice in one turn shows as `Read 2 files`.) Confirmed by the 2-Read
capture above.

## Boundary of the "read-only" group (verified against claude-code-src)

Two separate mechanisms exist in Claude Code:

### A. `collapseReadSearch` — the `Read N files` line

Each tool opts in via an `isSearchOrReadCommand` hook. From the source
(`utils/collapseReadSearch.ts`):

- **Opted in (read/search):** `Read`, `Grep`, `Glob`, `LSP`.
- **Special case:** `Write`/`Edit` targeting a *memory file* (CLAUDE.md and
  friends) is also collapsed into this same dim-gray line.
- **Not opted in (normal mutating):** `Bash`, `Write`, `Edit`, `MultiEdit`,
  `NotebookEdit`, `Task`, MCP tools — render per-call.

Triggers at N≥1.

### B. `groupToolUses` — same-tool same-message grouping (≥2 calls)

A second mechanism for tools that implement `renderGroupedToolUse`. In the
current source, **only `Task` (AgentTool)** opts in. So:

- 5 parallel `Task` calls in one assistant message → one grouped render
  (subagent fan-out summary).
- 5 parallel `Write` calls → 5 separate blocks (Write does not opt in).
- 5 parallel `Bash` calls → 5 separate blocks.

### Still to verify against live Claude (not yet captured)

- `WebFetch`, `WebSearch` — likely collapse, but confirm.
- MCP read-style tools — likely **not** since they don't implement the
  `isSearchOrReadCommand` hook by default.
- `Task` grouped render styling (different from the read/search line).

## Expansion behavior (ctrl+o)

- Toggle on the *currently-focused* (selected) batch item in the scrollback.
- Expanded form: each call rendered as its own dim-gray block with `⎿`
  result summary, in the order it appeared in the assistant turn.
- Hint text on the collapsed line: `(ctrl+o to expand)`; on the expanded
  line: `(ctrl+o to collapse)`.
- Default state: collapsed.

## Render summary table

| Tools in turn                          | Render                                                       |
| -------------------------------------- | ------------------------------------------------------------ |
| 1× Read                                | `  Read 1 file (ctrl+o to expand)`                           |
| 5× Read                                | `  Read 5 files (ctrl+o to expand)`                          |
| 1× Grep + 1× Glob                      | `  Searched for 1 pattern, listed 1 directory (ctrl+o to expand)` |
| 2× Grep                                | `  Searched for 2 patterns (ctrl+o to expand)`               |
| 1× Bash                                | `⏺ Bash(...)` per-call block                                 |
| 5× Write                               | 5 separate `⏺ Write(...)` per-call blocks                    |
| 2× Read + 2× Bash (parallel, one turn) | one `Read 2 files` line **plus** 2 per-call `⏺ Bash` blocks  |

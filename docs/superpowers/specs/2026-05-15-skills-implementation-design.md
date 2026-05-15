# Skills Implementation Design

**Date:** 2026-05-15  
**Branch:** feat/skills-implementation  
**Status:** Approved

---

## Problem

Super has a partial skills skeleton (loader + SkillTool) that fails in six ways compared to Claude Code's implementation:

1. Skill directory format (`skill-name/SKILL.md`) is not supported — only flat `.md` files are loaded
2. No plugin path — Superpowers skills at `~/.claude/plugins/cache/` are never loaded
3. SkillTool returns skill content as a tool result string instead of injecting it as a user message
4. `Base directory for this skill:` header is not prepended — companion files are inaccessible
5. Per-turn `<system-reminder>` skill listing is absent — model may not know skills exist
6. Frontmatter parses only `name` + `description` — `user-invocable`, `when_to_use`, `allowed-tools`, `model`, `context`, `argument-hint` are ignored

---

## Design

### Section 1: Build-Time Skill Embedding

Superpowers skills are embedded in the binary at compile time and extracted to disk on first run.

**Source layout:**
```
cli/assets/skills/
  brainstorming/
    SKILL.md
    visual-companion.md
    scripts/
      ...
  systematic-debugging/
    SKILL.md
  using-superpowers/
    SKILL.md
  ... (all current Superpowers skills)
```

Files are copied from `~/.claude/plugins/cache/claude-plugins-official/superpowers/5.1.0/skills/` at the time of this commit and live in the repo as first-class assets.

**`build.rs`** walks `cli/assets/skills/` at compile time and generates `cli/src/skills/bundled_gen.rs`:

```rust
pub struct BundledSkillFile {
    pub rel_path: &'static str,
    pub content: &'static str,
}

pub struct BundledSkillDef {
    pub name: &'static str,
    pub files: &'static [BundledSkillFile],
}

pub static BUNDLED_SKILLS: &[BundledSkillDef] = &[
    BundledSkillDef {
        name: "brainstorming",
        files: &[
            BundledSkillFile { rel_path: "SKILL.md", content: include_str!("../../assets/skills/brainstorming/SKILL.md") },
            BundledSkillFile { rel_path: "visual-companion.md", content: include_str!("../../assets/skills/brainstorming/visual-companion.md") },
        ],
    },
    // ...
];
```

**Extraction** — `skills/bundled.rs` exposes `extract_bundled_skills() -> Vec<Skill>`:
- Target root: `~/.super/plugins/superpowers/`
- For each `BundledSkillDef`, create `~/.super/plugins/superpowers/<name>/`
- Write each file using `O_CREAT | O_EXCL` (skip if already exists — idempotent)
- Parse the extracted `SKILL.md` with the full frontmatter parser
- Set `base_directory` = `~/.super/plugins/superpowers/<name>/`
- Set `loaded_from` = `LoadedFrom::Bundled`

Called from `bootstrap.rs` before `load_all_skills()`. Errors are logged and skipped (skills still work from in-memory content if extraction fails, but without companion file access).

---

### Section 2: Loader Expansion

`skills/loader.rs` is rewritten with a unified `load_all_skills()` that collects from three sources. Later sources win on name collision.

**Sources (priority order, lowest to highest):**
1. Bundled — result of `extract_bundled_skills()` (see Section 1)
2. User — `~/.super/skills/`
3. Project — `.claude/skills/` relative to cwd

**Both formats supported per source:**
- **Directory**: `<dir>/<skill-name>/SKILL.md` → `base_directory = <dir>/<skill-name>/`
- **Flat**: `<dir>/<skill-name>.md` → `base_directory = None`

**Expanded `Skill` struct:**

```rust
#[derive(Clone, Debug)]
pub struct Skill {
    pub name: String,
    pub description: String,
    pub content: String,
    pub base_directory: Option<PathBuf>,
    pub user_invocable: bool,       // default: true
    pub when_to_use: Option<String>,
    pub allowed_tools: Vec<String>,
    pub model: Option<String>,
    pub context: SkillContext,      // Inline (default) | Fork
    pub argument_hint: Option<String>,
    pub loaded_from: LoadedFrom,
}

#[derive(Clone, Debug, Default)]
pub enum SkillContext { #[default] Inline, Fork }

#[derive(Clone, Debug)]
pub enum LoadedFrom { Bundled, User, Project }
```

**Frontmatter fields parsed:**

| Frontmatter key | Struct field | Default |
|---|---|---|
| `name` | `name` | filename stem |
| `description` | `description` | `""` |
| `user-invocable` | `user_invocable` | `true` |
| `when_to_use` | `when_to_use` | `None` |
| `allowed-tools` | `allowed_tools` | `[]` |
| `model` | `model` | `None` |
| `context` | `context` | `Inline` |
| `argument-hint` | `argument_hint` | `None` |

Skills with `user_invocable: false` (e.g. `brainstorming`, `using-superpowers`) are loaded and callable via `SkillTool` but excluded from the `/` command menu and the user-facing section of the skill listing.

---

### Section 3: SkillTool Message Injection

**`ToolResult` struct** gains an `inject_messages` field:

```rust
pub struct ToolResult {
    pub content: String,
    pub is_error: bool,
    pub metadata: Option<serde_json::Value>,
    pub inject_messages: Vec<String>,  // additional text blocks to inject
}
```

**`SkillTool.call()` changes:**
- `content` → `"Launching skill: <name>"` (short acknowledgment — matches CC's wire format exactly)
- `inject_messages[0]` → the skill body, prepended with the base-dir header if present:
  ```
  Base directory for this skill: ~/.super/plugins/superpowers/brainstorming
  
  <full SKILL.md content>
  ```

**`run_tool_uses()` in `tool_loop.rs` changes:**

After collecting all `ToolResult`s for a turn, build the user message. For any result with non-empty `inject_messages`, append those strings as additional `{"type": "text", "text": "..."}` blocks in the same user message alongside the `tool_result` block:

```json
{
  "role": "user",
  "content": [
    {
      "type": "tool_result",
      "tool_use_id": "toolu_abc",
      "content": "Launching skill: brainstorming"
    },
    {
      "type": "text",
      "text": "Base directory for this skill: /Users/chwzr/.super/plugins/superpowers/brainstorming\n\n# Brainstorming Ideas Into Designs\n\n..."
    }
  ]
}
```

The Anthropic API accepts mixed `tool_result` + `text` blocks in a user message. This is exactly the pattern CC uses and ensures the model processes the skill content as conversation context rather than a tool return value.

---

### Section 4: Per-Turn Skill Listing

**`ConversationEngine`** in `conversation/engine.rs` tracks `skill_listing_sent: bool`, defaulting to `false`. Reset to `false` on `/clear`.

Before the first API call each session, if `skill_listing_sent` is false, the engine prepends a `<system-reminder>` block to the first user message's content:

```
<system-reminder>
The following skills are available for use with the Skill tool:

- brainstorming: You MUST use this before any creative work...
- systematic-debugging: Use when encountering any bug...
- using-superpowers: Use when starting any conversation...

User-invocable skills (user can type /<name>):
- init: Initialize a new CLAUDE.md file with codebase documentation
- review: Review a pull request
- security-review: Complete a security review of the pending changes
</system-reminder>
```

Format rules (matching CC):
- All skills appear in the listing (model needs to know about non-user-invocable ones too)
- Each entry: `- <name>: <description>[ — <when_to_use>]`  
- `when_to_use` is appended with ` — ` separator if present, truncated at 250 chars total
- Non-user-invocable skills (brainstorming, etc.) listed first as they drive model behavior
- User-invocable skills listed second under a sub-header

After injection, `skill_listing_sent` is set to `true`.

**The static skill list in `bootstrap.rs`** (`system_prompt.add_section(format!("Available skills:\n{}", ...))`) is removed — replaced entirely by this per-turn injection.

---

## Files Changed

| File | Change |
|---|---|
| `build.rs` (new) | Walk `assets/skills/`, generate `bundled_gen.rs` |
| `cli/assets/skills/**` (new) | Superpowers skill files committed as assets |
| `cli/src/skills/bundled_gen.rs` (generated) | `BUNDLED_SKILLS` constant |
| `cli/src/skills/bundled.rs` (new) | `extract_bundled_skills()` |
| `cli/src/skills/loader.rs` | Rewrite: directory format, multi-source, expanded frontmatter |
| `cli/src/skills/mod.rs` | Export new modules |
| `cli/src/tools/contract.rs` | Add `inject_messages: Vec<String>` to `ToolResult` |
| `cli/src/tools/skill.rs` | Injection pattern: `"Launching skill: <name>"` + inject content |
| `cli/src/conversation/tool_loop.rs` | Append inject_messages as text blocks in user message |
| `cli/src/conversation/engine.rs` | `skill_listing_sent` flag + per-turn injection |
| `cli/src/bootstrap.rs` | Call `extract_bundled_skills()`, remove static skill listing |

---

## Out of Scope

- `context: fork` — skill execution in a sub-agent (complex, requires subagent machinery; implement after this lands)
- Dynamic skill discovery (skills loaded as files are edited during a session)
- Conditional skills (`paths:` frontmatter)
- `/plugin install` command
- `$ARGUMENTS` / `${CLAUDE_SKILL_DIR}` substitution in skill content

# Skills Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Implement a fully working skills system: bundled Superpowers skills embedded in the binary, directory-format loading, per-turn skill listing, and skill content injected as a user message (not a tool result string).

**Architecture:** Superpowers skills are embedded at compile time via `build.rs` + `include_str!`, extracted to `~/.super/plugins/superpowers/` on first run, and loaded alongside user/project skills. `SkillTool.call()` returns a short acknowledgement as the tool result and injects the full skill content as an additional text block in the same user message turn. On the first prompt of each session the engine prepends a `<system-reminder>` skill listing.

**Tech Stack:** Rust, `walkdir` (already in Cargo.toml), `dirs` (already in Cargo.toml), Anthropic messages API multi-block user turns.

**Spec:** `docs/superpowers/specs/2026-05-15-skills-implementation-design.md`

---

## File Map

| File | Action | Purpose |
|---|---|---|
| `cli/assets/skills/**` | Create | Superpowers skill source files committed as assets |
| `cli/build.rs` | Create | Walks assets/skills/, generates bundled_gen.rs |
| `cli/src/skills/bundled_gen.rs` | Generated | `BUNDLED_SKILLS` constant (do not edit by hand) |
| `cli/src/skills/bundled.rs` | Create | `extract_bundled_skills()` — extracts+parses bundled skills |
| `cli/src/skills/loader.rs` | Rewrite | Expanded `Skill` struct, directory format, multi-source, full frontmatter |
| `cli/src/skills/mod.rs` | Modify | Export `bundled` module |
| `cli/src/tools/contract.rs` | Modify | Add `inject_messages: Vec<String>` to `ToolResult`; impl `Default` |
| `cli/src/tools/skill.rs` | Rewrite | `"Launching skill: <name>"` + inject body with base-dir header |
| `cli/src/conversation/tool_loop.rs` | Modify | Append `Text` blocks for inject_messages after each `ToolResult` block |
| `cli/src/conversation/engine.rs` | Modify | Add `skills`, `skill_listing_sent`; inject listing on first turn |
| `cli/src/bootstrap.rs` | Modify | Call extract+load, pass skills to engine, remove static listing |

---

## Task 1: Copy Superpowers skill assets

**Files:**
- Create: `cli/assets/skills/` (directory tree)

- [ ] **Step 1: Copy the skill assets**

```bash
mkdir -p cli/assets/skills
cp -r ~/.claude/plugins/cache/claude-plugins-official/superpowers/5.1.0/skills/. cli/assets/skills/
```

- [ ] **Step 2: Verify the copy**

```bash
find cli/assets/skills -name "SKILL.md" | sort
```

Expected output (14 lines, one per skill):
```
cli/assets/skills/brainstorming/SKILL.md
cli/assets/skills/dispatching-parallel-agents/SKILL.md
cli/assets/skills/executing-plans/SKILL.md
cli/assets/skills/finishing-a-development-branch/SKILL.md
cli/assets/skills/receiving-code-review/SKILL.md
cli/assets/skills/requesting-code-review/SKILL.md
cli/assets/skills/subagent-driven-development/SKILL.md
cli/assets/skills/systematic-debugging/SKILL.md
cli/assets/skills/test-driven-development/SKILL.md
cli/assets/skills/using-git-worktrees/SKILL.md
cli/assets/skills/using-superpowers/SKILL.md
cli/assets/skills/verification-before-completion/SKILL.md
cli/assets/skills/writing-plans/SKILL.md
cli/assets/skills/writing-skills/SKILL.md
```

- [ ] **Step 3: Commit**

```bash
git add cli/assets/skills
git commit -m "feat(skills): add Superpowers skill assets to binary"
```

---

## Task 2: Write build.rs

**Files:**
- Create: `cli/build.rs`

`build.rs` walks `cli/assets/skills/`, collects every file under each skill subdirectory, and writes `$OUT_DIR/bundled_gen.rs` containing a `BUNDLED_SKILLS` static with one entry per skill. Each entry holds the skill name and a slice of `(rel_path, include_str!(abs_path))` pairs. Using `concat!(env!("CARGO_MANIFEST_DIR"), ...)` makes paths portable across machines.

- [ ] **Step 1: Write cli/build.rs**

```rust
use std::fs;
use std::path::{Path, PathBuf};

fn main() {
    let manifest = std::env::var("CARGO_MANIFEST_DIR").unwrap();
    let assets = PathBuf::from(&manifest).join("assets/skills");
    let out_dir = std::env::var("OUT_DIR").unwrap();
    let out_file = PathBuf::from(&out_dir).join("bundled_gen.rs");

    // Re-run if any skill file changes.
    println!("cargo:rerun-if-changed=assets/skills");

    let mut skills: Vec<(String, Vec<String>)> = Vec::new(); // (skill_name, [rel_paths])

    if assets.exists() {
        let mut dirs: Vec<_> = fs::read_dir(&assets)
            .unwrap()
            .flatten()
            .filter(|e| e.path().is_dir())
            .collect();
        dirs.sort_by_key(|e| e.file_name());

        for dir_entry in dirs {
            let skill_name = dir_entry.file_name().to_string_lossy().to_string();
            let mut rel_paths: Vec<String> = Vec::new();
            collect_rel_paths(&dir_entry.path(), &dir_entry.path(), &mut rel_paths);
            rel_paths.sort();
            skills.push((skill_name, rel_paths));
        }
    }

    let mut out = String::new();
    out.push_str("// @generated by build.rs — do not edit\n\n");
    out.push_str("pub struct BundledSkillFile {\n    pub rel_path: &'static str,\n    pub content: &'static str,\n}\n\n");
    out.push_str("pub struct BundledSkillDef {\n    pub name: &'static str,\n    pub files: &'static [BundledSkillFile],\n}\n\n");
    out.push_str("pub static BUNDLED_SKILLS: &[BundledSkillDef] = &[\n");

    for (skill_name, rel_paths) in &skills {
        out.push_str(&format!("    BundledSkillDef {{ name: {:?}, files: &[\n", skill_name));
        for rel in rel_paths {
            let abs = format!("{}/assets/skills/{}/{}", manifest, skill_name, rel);
            out.push_str(&format!(
                "        BundledSkillFile {{ rel_path: {:?}, content: include_str!({:?}) }},\n",
                rel, abs
            ));
        }
        out.push_str("    ] },\n");
    }
    out.push_str("];\n");

    fs::write(&out_file, out).unwrap();
}

fn collect_rel_paths(base: &Path, current: &Path, out: &mut Vec<String>) {
    for entry in fs::read_dir(current).unwrap().flatten() {
        let path = entry.path();
        if path.is_dir() {
            collect_rel_paths(base, &path, out);
        } else {
            let rel = path.strip_prefix(base).unwrap();
            out.push(rel.to_string_lossy().replace('\\', "/"));
        }
    }
}
```

- [ ] **Step 2: Verify build.rs compiles and generates output**

```bash
cd cli && cargo build 2>&1 | head -20
```

Expected: no errors. Check the generated file exists:

```bash
find target -name "bundled_gen.rs" 2>/dev/null | head -3
```

Expected: one path printed under `target/`.

- [ ] **Step 3: Inspect generated file to confirm correctness**

```bash
find target -name "bundled_gen.rs" | head -1 | xargs head -40
```

Expected: sees `BUNDLED_SKILLS` static with `BundledSkillDef { name: "brainstorming", files: &[...] }`.

- [ ] **Step 4: Commit**

```bash
git add cli/build.rs
git commit -m "feat(skills): add build.rs to embed Superpowers skills at compile time"
```

---

## Task 3: Rewrite loader.rs — expanded Skill struct + full frontmatter

**Files:**
- Modify: `cli/src/skills/loader.rs`

- [ ] **Step 1: Write the failing tests**

Replace the contents of `cli/src/skills/loader.rs` with this file. Tests come first — the impl follows.

```rust
use std::collections::HashMap;
use std::path::{Path, PathBuf};

// ── Public types ─────────────────────────────────────────────────────────────

#[derive(Clone, Debug, Default, PartialEq)]
pub enum SkillContext {
    #[default]
    Inline,
    Fork,
}

#[derive(Clone, Debug, PartialEq)]
pub enum LoadedFrom {
    Bundled,
    User,
    Project,
}

#[derive(Clone, Debug)]
pub struct Skill {
    pub name: String,
    pub description: String,
    pub content: String,
    pub base_directory: Option<PathBuf>,
    pub user_invocable: bool,
    pub when_to_use: Option<String>,
    pub allowed_tools: Vec<String>,
    pub model: Option<String>,
    pub context: SkillContext,
    pub argument_hint: Option<String>,
    pub loaded_from: LoadedFrom,
}

// ── Public API ────────────────────────────────────────────────────────────────

/// Load user skills (~/.super/skills/) and project skills (.claude/skills/).
/// Bundled skills come from `bundled::extract_bundled_skills()` and are
/// merged in bootstrap.rs. Later sources win on name collision.
pub fn load_all_skills() -> Vec<Skill> {
    let mut by_name: HashMap<String, Skill> = HashMap::new();

    // User skills — ~/.super/skills/
    if let Some(home) = dirs::home_dir() {
        for skill in load_from_dir(&home.join(".super").join("skills"), LoadedFrom::User) {
            by_name.insert(skill.name.clone(), skill);
        }
    }

    // Project skills — .claude/skills/ relative to cwd
    if let Ok(cwd) = std::env::current_dir() {
        for skill in load_from_dir(&cwd.join(".claude").join("skills"), LoadedFrom::Project) {
            by_name.insert(skill.name.clone(), skill);
        }
    }

    by_name.into_values().collect()
}

/// Parse a skill from raw SKILL.md content. Used by bundled.rs to avoid
/// reading from disk a second time after extraction.
pub(crate) fn parse_skill_str(
    content: &str,
    fallback_name: &str,
    base_directory: Option<PathBuf>,
    loaded_from: LoadedFrom,
) -> Option<Skill> {
    build_skill(content, fallback_name, base_directory, loaded_from)
}

// ── Internal helpers ──────────────────────────────────────────────────────────

fn load_from_dir(dir: &Path, source: LoadedFrom) -> Vec<Skill> {
    if !dir.exists() {
        return Vec::new();
    }
    let Ok(entries) = std::fs::read_dir(dir) else {
        return Vec::new();
    };
    let mut skills = Vec::new();
    for entry in entries.flatten() {
        let path = entry.path();
        if path.is_dir() {
            // Directory format: <skill-name>/SKILL.md
            let skill_md = path.join("SKILL.md");
            if skill_md.exists() {
                if let Ok(raw) = std::fs::read_to_string(&skill_md) {
                    let fallback = path.file_name().and_then(|s| s.to_str()).unwrap_or("");
                    if let Some(s) = build_skill(&raw, fallback, Some(path), source.clone()) {
                        skills.push(s);
                    }
                }
            }
        } else if path.extension().map_or(false, |e| e == "md") {
            // Flat format: <skill-name>.md
            if let Ok(raw) = std::fs::read_to_string(&path) {
                let fallback = path.file_stem().and_then(|s| s.to_str()).unwrap_or("");
                if let Some(s) = build_skill(&raw, fallback, None, source.clone()) {
                    skills.push(s);
                }
            }
        }
    }
    skills
}

fn build_skill(
    raw: &str,
    fallback_name: &str,
    base_directory: Option<PathBuf>,
    loaded_from: LoadedFrom,
) -> Option<Skill> {
    let (fm, body) = parse_frontmatter(raw);
    if fallback_name.is_empty() {
        return None;
    }
    let name = fm.get("name").cloned().unwrap_or_else(|| fallback_name.to_string());
    let description = fm.get("description").cloned().unwrap_or_default();
    let user_invocable = fm.get("user-invocable").map(|v| v != "false").unwrap_or(true);
    let when_to_use = fm.get("when_to_use").or_else(|| fm.get("when-to-use")).cloned();
    let allowed_tools = fm
        .get("allowed-tools")
        .map(|v| v.split(',').map(|s| s.trim().to_string()).filter(|s| !s.is_empty()).collect())
        .unwrap_or_default();
    let model = fm.get("model").cloned();
    let context = match fm.get("context").map(String::as_str) {
        Some("fork") => SkillContext::Fork,
        _ => SkillContext::Inline,
    };
    let argument_hint = fm.get("argument-hint").cloned();

    Some(Skill {
        name,
        description,
        content: body,
        base_directory,
        user_invocable,
        when_to_use,
        allowed_tools,
        model,
        context,
        argument_hint,
        loaded_from,
    })
}

/// Returns (frontmatter key-value map, body after the closing ---).
fn parse_frontmatter(content: &str) -> (HashMap<String, String>, String) {
    let trimmed = content.trim();
    if !trimmed.starts_with("---") {
        return (HashMap::new(), trimmed.to_string());
    }
    let rest = &trimmed[3..];
    // Find closing --- on its own line
    let Some(end) = rest.find("\n---") else {
        return (HashMap::new(), trimmed.to_string());
    };
    let fm_str = &rest[..end];
    let body = rest[end + 4..].trim_start_matches('\n').to_string();
    let mut map = HashMap::new();
    for line in fm_str.lines() {
        if let Some((k, v)) = line.split_once(':') {
            let key = k.trim().to_string();
            let val = v.trim().to_string();
            if !key.is_empty() {
                map.insert(key, val);
            }
        }
    }
    (map, body)
}

// ── Tests ─────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;

    const SAMPLE_SKILL_MD: &str = r#"---
name: my-skill
description: Does something useful
user-invocable: false
when_to_use: Use when foo is needed
allowed-tools: Bash,Read
model: claude-opus-4-7
context: inline
argument-hint: <target>
---

# My Skill

This is the body.
"#;

    #[test]
    fn parse_frontmatter_extracts_all_fields() {
        let (fm, body) = parse_frontmatter(SAMPLE_SKILL_MD);
        assert_eq!(fm["name"], "my-skill");
        assert_eq!(fm["description"], "Does something useful");
        assert_eq!(fm["user-invocable"], "false");
        assert_eq!(fm["when_to_use"], "Use when foo is needed");
        assert_eq!(fm["allowed-tools"], "Bash,Read");
        assert_eq!(fm["model"], "claude-opus-4-7");
        assert_eq!(fm["context"], "inline");
        assert_eq!(fm["argument-hint"], "<target>");
        assert!(body.contains("# My Skill"), "body = {body:?}");
    }

    #[test]
    fn parse_frontmatter_no_frontmatter_returns_whole_content() {
        let raw = "# Just a body\nno frontmatter here";
        let (fm, body) = parse_frontmatter(raw);
        assert!(fm.is_empty());
        assert!(body.contains("# Just a body"));
    }

    #[test]
    fn build_skill_populates_all_fields() {
        let skill = build_skill(SAMPLE_SKILL_MD, "fallback", None, LoadedFrom::User).unwrap();
        assert_eq!(skill.name, "my-skill");
        assert_eq!(skill.description, "Does something useful");
        assert!(!skill.user_invocable);
        assert_eq!(skill.when_to_use.as_deref(), Some("Use when foo is needed"));
        assert_eq!(skill.allowed_tools, vec!["Bash", "Read"]);
        assert_eq!(skill.model.as_deref(), Some("claude-opus-4-7"));
        assert_eq!(skill.context, SkillContext::Inline);
        assert_eq!(skill.argument_hint.as_deref(), Some("<target>"));
        assert!(skill.content.contains("# My Skill"));
    }

    #[test]
    fn build_skill_defaults_user_invocable_to_true() {
        let raw = "---\nname: simple\ndescription: hi\n---\nbody";
        let skill = build_skill(raw, "simple", None, LoadedFrom::User).unwrap();
        assert!(skill.user_invocable);
    }

    #[test]
    fn build_skill_fork_context() {
        let raw = "---\nname: forked\ndescription: x\ncontext: fork\n---\nbody";
        let skill = build_skill(raw, "forked", None, LoadedFrom::User).unwrap();
        assert_eq!(skill.context, SkillContext::Fork);
    }

    #[test]
    fn load_from_dir_flat_format() {
        let dir = std::env::temp_dir().join(format!("super_test_flat_{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let skill_md = dir.join("my-skill.md");
        std::fs::write(&skill_md, "---\nname: my-skill\ndescription: flat\n---\nbody").unwrap();

        let skills = load_from_dir(&dir, LoadedFrom::User);
        assert_eq!(skills.len(), 1);
        assert_eq!(skills[0].name, "my-skill");
        assert!(skills[0].base_directory.is_none());

        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn load_from_dir_directory_format() {
        let dir = std::env::temp_dir().join(format!("super_test_dir_{}", std::process::id()));
        let skill_dir = dir.join("my-skill");
        std::fs::create_dir_all(&skill_dir).unwrap();
        std::fs::write(
            skill_dir.join("SKILL.md"),
            "---\nname: my-skill\ndescription: dir\n---\nbody",
        )
        .unwrap();

        let skills = load_from_dir(&dir, LoadedFrom::Project);
        assert_eq!(skills.len(), 1);
        assert_eq!(skills[0].name, "my-skill");
        assert!(skills[0].base_directory.is_some());
        assert_eq!(skills[0].loaded_from, LoadedFrom::Project);

        std::fs::remove_dir_all(&dir).unwrap();
    }
}
```

- [ ] **Step 2: Run failing tests (parse_frontmatter tests should pass immediately; load_from_dir tests exercise real FS)**

```bash
cd cli && cargo test skills::loader 2>&1 | tail -20
```

Expected: all 7 tests pass.

- [ ] **Step 3: Commit**

```bash
git add cli/src/skills/loader.rs
git commit -m "feat(skills): rewrite loader with expanded Skill struct and directory format"
```

---

## Task 4: Write bundled.rs and update mod.rs

**Files:**
- Create: `cli/src/skills/bundled.rs`
- Modify: `cli/src/skills/mod.rs`

- [ ] **Step 1: Write the failing test first (in bundled.rs)**

Create `cli/src/skills/bundled.rs`:

```rust
//! Extracts Superpowers skills embedded at compile time into
//! ~/.super/plugins/superpowers/ and parses them as Skill objects.

include!(concat!(env!("OUT_DIR"), "/bundled_gen.rs"));

use super::loader::{LoadedFrom, Skill};
use std::io::Write;
use std::path::PathBuf;

/// Extract all bundled skills to `~/.super/plugins/superpowers/` (idempotent)
/// and return them as `Skill` objects. Extraction failures are logged and
/// skipped — the skill is still usable from in-memory content but
/// companion files (visual-companion.md etc.) won't be accessible via Read.
pub fn extract_bundled_skills() -> Vec<Skill> {
    let root = match bundled_root() {
        Some(r) => r,
        None => return parse_in_memory_only(),
    };

    let mut skills = Vec::new();
    for def in BUNDLED_SKILLS {
        let skill_dir = root.join(def.name);
        if let Err(e) = extract_skill(def, &skill_dir) {
            eprintln!("[super] warning: could not extract skill '{}': {e}", def.name);
        }
        // Always parse from in-memory content (extraction may have already
        // existed; re-reading disk is not necessary and adds I/O).
        if let Some(skill) = skill_from_def(def, Some(skill_dir)) {
            skills.push(skill);
        }
    }
    skills
}

fn bundled_root() -> Option<PathBuf> {
    dirs::home_dir().map(|h| h.join(".super").join("plugins").join("superpowers"))
}

fn extract_skill(def: &BundledSkillDef, skill_dir: &PathBuf) -> std::io::Result<()> {
    for file in def.files {
        let target = skill_dir.join(file.rel_path);
        if target.exists() {
            continue; // idempotent
        }
        if let Some(parent) = target.parent() {
            std::fs::create_dir_all(parent)?;
        }
        let mut fh = std::fs::OpenOptions::new()
            .write(true)
            .create_new(true) // O_EXCL: fail if already exists (race-safe)
            .open(&target)?;
        fh.write_all(file.content.as_bytes())?;
    }
    Ok(())
}

fn skill_from_def(def: &BundledSkillDef, base_directory: Option<PathBuf>) -> Option<Skill> {
    let skill_content = def.files.iter().find(|f| f.rel_path == "SKILL.md")?.content;
    super::loader::parse_skill_str(skill_content, def.name, base_directory, LoadedFrom::Bundled)
}

fn parse_in_memory_only() -> Vec<Skill> {
    BUNDLED_SKILLS
        .iter()
        .filter_map(|def| skill_from_def(def, None))
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn bundled_skills_constant_is_non_empty() {
        assert!(
            !BUNDLED_SKILLS.is_empty(),
            "BUNDLED_SKILLS should contain at least one skill"
        );
    }

    #[test]
    fn every_bundled_skill_has_skill_md() {
        for def in BUNDLED_SKILLS {
            let has_skill_md = def.files.iter().any(|f| f.rel_path == "SKILL.md");
            assert!(has_skill_md, "skill '{}' is missing SKILL.md", def.name);
        }
    }

    #[test]
    fn every_bundled_skill_parses_to_non_empty_name() {
        for def in BUNDLED_SKILLS {
            let skill = skill_from_def(def, None);
            assert!(skill.is_some(), "skill '{}' failed to parse", def.name);
            assert!(!skill.unwrap().name.is_empty(), "skill '{}' parsed with empty name", def.name);
        }
    }

    #[test]
    fn using_superpowers_is_not_user_invocable() {
        let skill = BUNDLED_SKILLS
            .iter()
            .find(|d| d.name == "using-superpowers")
            .and_then(|d| skill_from_def(d, None));
        let skill = skill.expect("using-superpowers not found in BUNDLED_SKILLS");
        assert!(
            !skill.user_invocable,
            "using-superpowers should have user-invocable: false"
        );
    }
}
```

- [ ] **Step 2: Update cli/src/skills/mod.rs**

```rust
pub mod bundled;
pub mod loader;
pub mod discovery;
```

- [ ] **Step 3: Run tests**

```bash
cd cli && cargo test skills::bundled 2>&1 | tail -20
```

Expected: all 4 tests pass.

- [ ] **Step 4: Commit**

```bash
git add cli/src/skills/bundled.rs cli/src/skills/mod.rs
git commit -m "feat(skills): add bundled.rs to extract and parse embedded Superpowers skills"
```

---

## Task 5: Add inject_messages to ToolResult

**Files:**
- Modify: `cli/src/tools/contract.rs`
- Modify: `cli/src/conversation/tool_loop.rs` (all `ToolResult { ... }` construction sites)

`inject_messages` carries additional text to be injected into the conversation alongside the `tool_result` block. It is purely in-process — never serialized to disk or sent to a remote.

- [ ] **Step 1: Update contract.rs**

Replace the `ToolResult` struct and add `Default`:

```rust
use std::collections::HashMap;
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ToolResult {
    pub content: String,
    pub is_error: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub metadata: Option<HashMap<String, String>>,
    /// Additional text blocks to inject into the conversation alongside
    /// the tool_result. Never serialized — in-process only.
    #[serde(skip)]
    pub inject_messages: Vec<String>,
}

impl Default for ToolResult {
    fn default() -> Self {
        ToolResult {
            content: String::new(),
            is_error: false,
            metadata: None,
            inject_messages: Vec::new(),
        }
    }
}

pub struct ToolCallContext {
    pub cwd: std::path::PathBuf,
    pub permission_mode: crate::state::store::PermissionMode,
    pub abort_signal: Option<tokio::sync::watch::Receiver<bool>>,
    pub parent_tool_use_id: Option<String>,
    pub bus: Option<std::sync::Arc<crate::conversation::session_bus::SessionBus>>,
    pub auto_deny_prompts: bool,
    pub tool_use_id: String,
}

#[async_trait::async_trait]
pub trait Tool: Send + Sync {
    async fn call(&self, input: serde_json::Value, context: &ToolCallContext) -> ToolResult;
    fn name(&self) -> &str;
    fn description(&self) -> &str;
    fn input_schema(&self) -> serde_json::Value;
    fn is_concurrency_safe(&self) -> bool { false }
    fn is_read_only(&self) -> bool { false }
    fn is_destructive(&self) -> bool { false }
    fn check_permission(&self, _input: &serde_json::Value) -> crate::tools::permission::Decision {
        crate::tools::permission::Decision::Ask
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn tool_result_default_has_empty_inject_messages() {
        let r = ToolResult::default();
        assert!(r.inject_messages.is_empty());
        assert!(!r.is_error);
    }

    #[test]
    fn tool_result_inject_messages_not_in_json() {
        let r = ToolResult {
            content: "hello".into(),
            inject_messages: vec!["injected".into()],
            ..Default::default()
        };
        let json = serde_json::to_string(&r).unwrap();
        assert!(!json.contains("inject_messages"), "inject_messages must not appear in JSON: {json}");
        assert!(!json.contains("injected"));
    }

    #[test]
    fn tool_call_context_carries_auto_deny_flag() {
        let ctx = ToolCallContext {
            cwd: std::path::PathBuf::from("/tmp"),
            permission_mode: crate::state::store::PermissionMode::Default,
            abort_signal: None,
            parent_tool_use_id: None,
            bus: None,
            auto_deny_prompts: true,
            tool_use_id: "tu_test".into(),
        };
        assert!(ctx.auto_deny_prompts);
        assert_eq!(ctx.tool_use_id, "tu_test");
    }
}
```

- [ ] **Step 2: Fix all ToolResult construction sites in tool_loop.rs**

In `cli/src/conversation/tool_loop.rs`, update every `ToolResult { ... }` literal (panic handler, join error handler, `MissingTool::call`) to use `..Default::default()`:

```rust
// panic handler (line ~102)
Err(e) if e.is_panic() => ToolResult {
    content: format!("Tool panicked: {}", downcast_panic(&e.into_panic())),
    is_error: true,
    ..Default::default()
},
// join error handler (line ~110)
Err(e) => ToolResult {
    content: format!("Tool task error: {e}"),
    is_error: true,
    ..Default::default()
},
// MissingTool::call (line ~227)
async fn call(&self, _input: serde_json::Value, _ctx: &ToolCallContext) -> ToolResult {
    ToolResult {
        content: format!("Unknown tool: {}", self.name),
        is_error: true,
        ..Default::default()
    }
}
// join_error synthetic placeholder (line ~133)
ToolResult {
    content: format!("Tool task join error: {e}"),
    is_error: true,
    ..Default::default()
},
```

- [ ] **Step 3: Check that every other tool file that constructs ToolResult also compiles**

```bash
cd cli && cargo build 2>&1 | grep "error\[" | head -20
```

Expected: no errors. If any tool file has a `ToolResult { content: ..., is_error: ..., metadata: None }` literal without `inject_messages`, add `..Default::default()` to it. Common files to check: `src/tools/bash.rs`, `src/tools/read.rs`, etc.

- [ ] **Step 4: Run tests**

```bash
cd cli && cargo test tools::contract 2>&1 | tail -10
```

Expected: all 3 tests pass.

- [ ] **Step 5: Commit**

```bash
git add cli/src/tools/contract.rs cli/src/conversation/tool_loop.rs
git commit -m "feat(skills): add inject_messages to ToolResult for skill content injection"
```

---

## Task 6: Rewrite SkillTool

**Files:**
- Modify: `cli/src/tools/skill.rs`

The new SkillTool returns `"Launching skill: <name>"` as the tool result content and puts the full skill body (with base-dir header) in `inject_messages[0]`.

- [ ] **Step 1: Rewrite cli/src/tools/skill.rs**

```rust
use async_trait::async_trait;
use serde_json::json;
use super::contract::{Tool, ToolCallContext, ToolResult};

pub struct SkillTool {
    pub skills: Vec<crate::skills::loader::Skill>,
}

#[async_trait]
impl Tool for SkillTool {
    fn name(&self) -> &str { "Skill" }

    fn description(&self) -> &str {
        "Execute a skill within the main conversation. \
         Skills provide specialized capabilities and domain knowledge. \
         When users reference a slash command (/<name>), use this tool."
    }

    fn input_schema(&self) -> serde_json::Value {
        json!({
            "type": "object",
            "properties": {
                "skill": {
                    "type": "string",
                    "description": "The skill name (e.g. \"commit\", \"brainstorming\", \"superpowers:writing-plans\")"
                },
                "args": {
                    "type": "string",
                    "description": "Optional arguments passed to the skill"
                }
            },
            "required": ["skill"]
        })
    }

    async fn call(&self, input: serde_json::Value, _context: &ToolCallContext) -> ToolResult {
        let skill_name = input["skill"].as_str().unwrap_or("").trim().to_string();
        if skill_name.is_empty() {
            return ToolResult {
                content: "No skill name provided.".into(),
                is_error: true,
                ..Default::default()
            };
        }
        let args = input["args"].as_str().unwrap_or("");

        // Normalize: strip leading slash if present.
        let normalized = skill_name.trim_start_matches('/');

        let skill = self.find_skill(normalized);
        match skill {
            None => {
                let available: Vec<&str> = self.skills.iter().map(|s| s.name.as_str()).collect();
                ToolResult {
                    content: format!(
                        "Skill '{}' not found. Available skills: {}",
                        normalized,
                        if available.is_empty() { "none loaded".into() } else { available.join(", ") }
                    ),
                    is_error: true,
                    ..Default::default()
                }
            }
            Some(s) => {
                // Prepend "Base directory for this skill:" header when available.
                let body = match &s.base_directory {
                    Some(dir) => format!(
                        "Base directory for this skill: {}\n\n{}",
                        dir.display(),
                        s.content
                    ),
                    None => s.content.clone(),
                };
                // Append args if provided.
                let inject_content = if args.is_empty() {
                    body
                } else {
                    format!("{}\n\n---\nArguments: {}", body, args)
                };
                ToolResult {
                    content: format!("Launching skill: {}", s.name),
                    is_error: false,
                    inject_messages: vec![inject_content],
                    ..Default::default()
                }
            }
        }
    }
}

impl SkillTool {
    fn find_skill<'a>(&'a self, name: &str) -> Option<&'a crate::skills::loader::Skill> {
        let lower = name.to_lowercase();
        // Exact match first.
        if let Some(s) = self.skills.iter().find(|s| s.name.to_lowercase() == lower) {
            return Some(s);
        }
        // Suffix match for namespaced names (e.g. "superpowers:brainstorming" → "brainstorming").
        self.skills.iter().find(|s| {
            let sn = s.name.to_lowercase();
            sn.ends_with(&format!(":{}", lower)) || sn == lower
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::skills::loader::{LoadedFrom, Skill, SkillContext};
    use std::path::PathBuf;

    fn make_skill(name: &str, user_invocable: bool, base_dir: Option<PathBuf>) -> Skill {
        Skill {
            name: name.to_string(),
            description: format!("{name} description"),
            content: format!("# {name}\n\nContent of {name}."),
            base_directory: base_dir,
            user_invocable,
            when_to_use: None,
            allowed_tools: vec![],
            model: None,
            context: SkillContext::Inline,
            argument_hint: None,
            loaded_from: LoadedFrom::Bundled,
        }
    }

    fn make_tool(skills: Vec<Skill>) -> SkillTool {
        SkillTool { skills }
    }

    fn ctx() -> ToolCallContext {
        ToolCallContext {
            cwd: PathBuf::from("/tmp"),
            permission_mode: crate::state::store::PermissionMode::Default,
            abort_signal: None,
            parent_tool_use_id: None,
            bus: None,
            auto_deny_prompts: false,
            tool_use_id: "tu_test".into(),
        }
    }

    #[tokio::test]
    async fn returns_launching_skill_content() {
        let tool = make_tool(vec![make_skill("brainstorming", false, None)]);
        let result = tool.call(json!({"skill": "brainstorming"}), &ctx()).await;
        assert_eq!(result.content, "Launching skill: brainstorming");
        assert!(!result.is_error);
    }

    #[tokio::test]
    async fn injects_skill_body_without_base_dir() {
        let tool = make_tool(vec![make_skill("brainstorming", false, None)]);
        let result = tool.call(json!({"skill": "brainstorming"}), &ctx()).await;
        assert_eq!(result.inject_messages.len(), 1);
        assert!(result.inject_messages[0].contains("Content of brainstorming"));
        assert!(!result.inject_messages[0].contains("Base directory"));
    }

    #[tokio::test]
    async fn injects_base_dir_header_when_present() {
        let dir = PathBuf::from("/home/user/.super/plugins/superpowers/brainstorming");
        let tool = make_tool(vec![make_skill("brainstorming", false, Some(dir.clone()))]);
        let result = tool.call(json!({"skill": "brainstorming"}), &ctx()).await;
        assert!(result.inject_messages[0].starts_with("Base directory for this skill:"));
        assert!(result.inject_messages[0].contains(dir.to_str().unwrap()));
    }

    #[tokio::test]
    async fn strips_leading_slash_from_skill_name() {
        let tool = make_tool(vec![make_skill("commit", true, None)]);
        let result = tool.call(json!({"skill": "/commit"}), &ctx()).await;
        assert_eq!(result.content, "Launching skill: commit");
        assert!(!result.is_error);
    }

    #[tokio::test]
    async fn unknown_skill_returns_error() {
        let tool = make_tool(vec![make_skill("commit", true, None)]);
        let result = tool.call(json!({"skill": "nonexistent"}), &ctx()).await;
        assert!(result.is_error);
        assert!(result.inject_messages.is_empty());
    }

    #[tokio::test]
    async fn args_appended_to_inject_content() {
        let tool = make_tool(vec![make_skill("compact", true, None)]);
        let result = tool
            .call(json!({"skill": "compact", "args": "focus on recent changes"}), &ctx())
            .await;
        assert!(result.inject_messages[0].contains("focus on recent changes"));
    }
}
```

- [ ] **Step 2: Run tests**

```bash
cd cli && cargo test tools::skill 2>&1 | tail -20
```

Expected: all 6 tests pass.

- [ ] **Step 3: Commit**

```bash
git add cli/src/tools/skill.rs
git commit -m "feat(skills): rewrite SkillTool to inject skill body as conversation message"
```

---

## Task 7: Update tool_loop.rs to handle inject_messages

**Files:**
- Modify: `cli/src/conversation/tool_loop.rs`

`run_tool_uses` currently maps `(id, ToolResult)` → `ContentBlockFinal::ToolResult`. After this task it also appends `ContentBlockFinal::Text` blocks for each entry in `inject_messages`, in the same returned Vec. The engine already pushes the whole Vec as a single user-turn history entry, so inject_messages become text blocks in the same user turn as the tool_result blocks — exactly matching the Anthropic API's multi-block user message pattern.

- [ ] **Step 1: Update the final map at the bottom of run_tool_uses**

Find this block at the end of `run_tool_uses` in `cli/src/conversation/tool_loop.rs`:

```rust
combined
    .into_iter()
    .map(|(_i, id, res)| ContentBlockFinal::ToolResult {
        tool_use_id: id,
        content: res.content,
        is_error: res.is_error,
    })
    .collect()
```

Replace it with:

```rust
let mut blocks: Vec<ContentBlockFinal> = Vec::with_capacity(combined.len());
for (_i, id, res) in combined {
    blocks.push(ContentBlockFinal::ToolResult {
        tool_use_id: id,
        content: res.content,
        is_error: res.is_error,
    });
    for msg in res.inject_messages {
        blocks.push(ContentBlockFinal::Text { text: msg });
    }
}
blocks
```

- [ ] **Step 2: Run existing tool_loop tests to confirm no regressions**

```bash
cd cli && cargo test conversation::tool_loop 2>&1 | tail -20
```

Expected: all existing tests pass.

- [ ] **Step 3: Add a test for inject_messages propagation**

Append this to the `#[cfg(test)]` block in `cli/src/conversation/tool_loop.rs`:

```rust
#[test]
fn inject_messages_become_text_blocks() {
    use crate::sdk::protocol::ContentBlockFinal;
    use crate::tools::contract::ToolResult;

    // Simulate what run_tool_uses produces for one ToolResult with inject_messages.
    let results: Vec<(usize, String, ToolResult)> = vec![(
        0,
        "tu_1".to_string(),
        ToolResult {
            content: "Launching skill: foo".into(),
            is_error: false,
            inject_messages: vec!["# Foo Skill\n\nDo the thing.".into()],
            ..Default::default()
        },
    )];

    let mut blocks: Vec<ContentBlockFinal> = Vec::new();
    for (_i, id, res) in results {
        blocks.push(ContentBlockFinal::ToolResult {
            tool_use_id: id,
            content: res.content,
            is_error: res.is_error,
        });
        for msg in res.inject_messages {
            blocks.push(ContentBlockFinal::Text { text: msg });
        }
    }

    assert_eq!(blocks.len(), 2);
    assert!(matches!(&blocks[0], ContentBlockFinal::ToolResult { content, .. } if content == "Launching skill: foo"));
    assert!(matches!(&blocks[1], ContentBlockFinal::Text { text } if text.contains("# Foo Skill")));
}
```

- [ ] **Step 4: Run the new test**

```bash
cd cli && cargo test conversation::tool_loop::tests::inject_messages_become_text_blocks 2>&1 | tail -10
```

Expected: PASS.

- [ ] **Step 5: Commit**

```bash
git add cli/src/conversation/tool_loop.rs
git commit -m "feat(skills): propagate inject_messages as Text blocks in tool turn"
```

---

## Task 8: Per-turn skill listing in engine.rs

**Files:**
- Modify: `cli/src/conversation/engine.rs`

Add `skills: Arc<Vec<Skill>>` and `skill_listing_sent: Arc<AtomicBool>` to `ConversationEngine`. On the first prompt of each session inject a `<system-reminder>` text block listing all skills before the user's message.

- [ ] **Step 1: Add imports and new fields to ConversationEngine**

At the top of `cli/src/conversation/engine.rs`, add to the existing imports:

```rust
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
```

Add two fields to the `ConversationEngine` struct:

```rust
pub struct ConversationEngine {
    // ... existing fields ...
    /// All loaded skills (bundled + user + project). Used to build the
    /// per-session skill listing injected on the first user turn.
    pub skills: Arc<Vec<crate::skills::loader::Skill>>,
    /// Whether the skill listing has been injected this session.
    /// AtomicBool because process_prompt takes &self.
    pub skill_listing_sent: Arc<AtomicBool>,
}
```

- [ ] **Step 2: Update ConversationEngine::new() and new_child()**

In `ConversationEngine::new()`, add:

```rust
skills: Arc::new(Vec::new()),
skill_listing_sent: Arc::new(AtomicBool::new(false)),
```

In `ConversationEngine::new_child()`, add:

```rust
skills: Arc::new(Vec::new()), // child engines get listing via parent's first turn
skill_listing_sent: Arc::new(AtomicBool::new(false)),
```

- [ ] **Step 3: Add skill listing builder functions (bottom of engine.rs)**

Append these private functions at the end of `engine.rs`:

```rust
fn build_skill_listing(skills: &[crate::skills::loader::Skill]) -> String {
    use crate::skills::loader::Skill;

    let mut model_only: Vec<&Skill> = skills.iter().filter(|s| !s.user_invocable).collect();
    let mut user_facing: Vec<&Skill> = skills.iter().filter(|s| s.user_invocable).collect();
    model_only.sort_by(|a, b| a.name.cmp(&b.name));
    user_facing.sort_by(|a, b| a.name.cmp(&b.name));

    let mut out = String::from("<system-reminder>\nThe following skills are available for use with the Skill tool:\n\n");

    for s in &model_only {
        out.push_str(&format!("- {}\n", format_skill_entry(s)));
    }

    if !user_facing.is_empty() {
        if !model_only.is_empty() {
            out.push('\n');
        }
        out.push_str("User-invocable skills (user can type /<name>):\n\n");
        for s in &user_facing {
            out.push_str(&format!("- {}\n", format_skill_entry(s)));
        }
    }

    out.push_str("</system-reminder>");
    out
}

fn format_skill_entry(skill: &crate::skills::loader::Skill) -> String {
    const MAX: usize = 250;
    let full = match &skill.when_to_use {
        Some(w) => format!("{}: {} — {}", skill.name, skill.description, w),
        None    => format!("{}: {}", skill.name, skill.description),
    };
    if full.len() > MAX { format!("{}…", &full[..MAX - 1]) } else { full }
}
```

- [ ] **Step 4: Inject the listing in process_prompt**

In `process_prompt`, find the block that builds the initial user history entry. It currently looks like:

```rust
let user_block = ContentBlockFinal::Text { text: user_input.clone() };
history.push(HistoryEntry {
    role: Role::User,
    content: vec![user_block.clone()],
});
```

Replace it with:

```rust
// Reset listing flag when starting a fresh conversation.
if history.is_empty() {
    self.skill_listing_sent.store(false, Ordering::Relaxed);
}

let mut user_content: Vec<ContentBlockFinal> = Vec::new();

// Inject skill listing on first turn of each session.
if !self.skill_listing_sent.swap(true, Ordering::Relaxed) && !self.skills.is_empty() {
    let listing = build_skill_listing(&self.skills);
    user_content.push(ContentBlockFinal::Text { text: listing });
}

let user_block = ContentBlockFinal::Text { text: user_input.clone() };
user_content.push(user_block.clone());

history.push(HistoryEntry {
    role: Role::User,
    content: user_content.clone(),
});
```

Also update the `self.bus.emit(BusMessage::User { ... })` call that immediately follows to emit `user_content` instead of `vec![user_block]`:

```rust
self.bus.emit(BusMessage::User {
    message: UserPayload {
        role: "user".to_string(),
        content: user_content,
    },
    parent_tool_use_id: parent_tool_use_id.clone(),
    uuid: Uuid::new_v4(),
    session_id: session_id.clone(),
});
```

- [ ] **Step 5: Build to verify compilation**

```bash
cd cli && cargo build 2>&1 | grep "error\[" | head -10
```

Expected: no errors.

- [ ] **Step 6: Commit**

```bash
git add cli/src/conversation/engine.rs
git commit -m "feat(skills): inject per-turn skill listing system-reminder on first prompt"
```

---

## Task 9: Wire bootstrap.rs — extract, merge, pass to engine

**Files:**
- Modify: `cli/src/bootstrap.rs`

- [ ] **Step 1: Replace the skills loading and system-prompt listing block**

Open `cli/src/bootstrap.rs`. Find this block:

```rust
// Load skills and (re-)register the SkillTool with loaded skills.
let skills = crate::skills::loader::load_all_skills();
if !skills.is_empty() {
    registry.register(Arc::new(crate::tools::skill::SkillTool {
        skills: skills.clone(),
    }));
}
```

And this block further down:

```rust
// Add skill descriptions to system prompt.
if !skills.is_empty() {
    let skill_desc: Vec<String> = skills
        .iter()
        .map(|s| format!("- {}: {}", s.name, s.description))
        .collect();
    system_prompt.add_section(format!("Available skills:\n{}", skill_desc.join("\n")));
}
```

Replace both blocks with a single merged section:

```rust
// Load skills: bundled (embedded in binary) + user/project.
// Bundled skills are extracted to ~/.super/plugins/superpowers/ on first run.
let bundled_skills = crate::skills::bundled::extract_bundled_skills();
let local_skills  = crate::skills::loader::load_all_skills();

// Merge: local (user/project) overrides bundled on name collision.
let mut by_name: std::collections::HashMap<String, crate::skills::loader::Skill> =
    bundled_skills.into_iter().map(|s| (s.name.clone(), s)).collect();
for s in local_skills {
    by_name.insert(s.name.clone(), s);
}
let all_skills: Vec<crate::skills::loader::Skill> = by_name.into_values().collect();

// Register SkillTool (always — even when no skills are loaded the tool must
// exist so the model can receive a clear error on invocation).
registry.register(Arc::new(crate::tools::skill::SkillTool {
    skills: all_skills.clone(),
}));
```

- [ ] **Step 2: Pass all_skills to the engine**

Find the `ConversationEngine::new(...)` call and add `.skills` assignment after construction:

```rust
let mut engine = crate::conversation::engine::ConversationEngine::new(
    store.clone(),
    config.clone(),
    registry.clone(),
    bus.clone(),
);
engine.skills = std::sync::Arc::new(all_skills);
```

(If the engine is constructed directly with a struct literal rather than `::new()`, add the field there instead.)

- [ ] **Step 3: Build and confirm no compile errors**

```bash
cd cli && cargo build 2>&1 | grep "error\[" | head -20
```

Expected: no errors.

- [ ] **Step 4: Smoke test — run super and verify skill listing appears**

```bash
cd cli && cargo run -- <<'EOF'
hello
EOF
```

Expected: the first API request (visible if you set `RUST_LOG=debug`) includes a `<system-reminder>` block listing at least the bundled skills before the user's "hello" message. Alternatively, check the sidechain JSONL transcript:

```bash
ls ~/.super/sidechains/ | tail -1 | xargs -I{} cat ~/.super/sidechains/{} | python3 -m json.tool | grep -A5 "system-reminder" | head -20
```

Expected: output shows `"The following skills are available for use with the Skill tool:"` followed by skill names.

- [ ] **Step 5: Commit**

```bash
git add cli/src/bootstrap.rs
git commit -m "feat(skills): wire extract+merge+engine skills in bootstrap; remove static listing"
```

---

## Task 10: Final build + integration check

**Files:**
- No file changes. Verification only.

- [ ] **Step 1: Run the full test suite**

```bash
cd cli && cargo test 2>&1 | tail -30
```

Expected: all tests pass, zero failures.

- [ ] **Step 2: Verify bundled skill count**

```bash
cd cli && cargo test skills::bundled::tests::bundled_skills_constant_is_non_empty -- --nocapture 2>&1 | tail -5
```

Expected: PASS.

- [ ] **Step 3: Verify using-superpowers is not user-invocable**

```bash
cd cli && cargo test skills::bundled::tests::using_superpowers_is_not_user_invocable -- --nocapture 2>&1 | tail -5
```

Expected: PASS.

- [ ] **Step 4: Verify extraction idempotency — run bootstrap twice, check no duplicate files**

```bash
# First run creates ~/.super/plugins/superpowers/
cargo run -- <<< "exit" 2>/dev/null || true
# Second run should not error (create_new(true) on existing files is skipped)
cargo run -- <<< "exit" 2>/dev/null || true
echo "exit code: $?"
ls ~/.super/plugins/superpowers/ | wc -l
```

Expected: second run exits cleanly; directory has 14 skill subdirectories.

- [ ] **Step 5: Final commit**

```bash
git add -A
git commit -m "feat(skills): complete skills implementation — bundled, injection, per-turn listing"
```

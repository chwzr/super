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
                    let fallback_owned = path
                        .file_name()
                        .and_then(|s| s.to_str())
                        .unwrap_or("")
                        .to_string();
                    if let Some(s) = build_skill(&raw, &fallback_owned, Some(path), source.clone())
                    {
                        skills.push(s);
                    }
                }
            }
        } else if path.extension().is_some_and(|e| e == "md") {
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
    let name = fm
        .get("name")
        .cloned()
        .unwrap_or_else(|| fallback_name.to_string());
    let description = fm.get("description").cloned().unwrap_or_default();
    let user_invocable = fm
        .get("user-invocable")
        .map(|v| v != "false")
        .unwrap_or(true);
    let when_to_use = fm
        .get("when_to_use")
        .or_else(|| fm.get("when-to-use"))
        .cloned();
    // Only inline comma-separated format is supported (e.g. `allowed-tools: Bash,Read`).
    // YAML block sequences are not parsed.
    let allowed_tools = fm
        .get("allowed-tools")
        .map(|v| {
            v.split(',')
                .map(|s| s.trim().to_string())
                .filter(|s| !s.is_empty())
                .collect()
        })
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
    let body = rest[end + 4..].trim_start_matches(['\n', '\r']).to_string();
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
        std::fs::write(
            &skill_md,
            "---\nname: my-skill\ndescription: flat\n---\nbody",
        )
        .unwrap();

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

use std::path::{Path, PathBuf};

use serde::Deserialize;

use super::definition::{AgentDefinition, AgentSource};
use crate::state::store::PermissionMode;

#[derive(Debug, Deserialize)]
struct Frontmatter {
    description: String,
    #[serde(default)]
    tools: Option<Vec<String>>,
    #[serde(default, rename = "disallowedTools")]
    disallowed_tools: Option<Vec<String>>,
    #[serde(default)]
    model: Option<String>,
    #[serde(default, rename = "permissionMode")]
    permission_mode: Option<String>,
    #[serde(default, rename = "maxTurns")]
    max_turns: Option<u32>,
}

/// Parse a single `.claude/agents/<name>.md` file's contents into an
/// `AgentDefinition`. `filename` is used only to derive `agent_type` (strip the
/// `.md` suffix). Source identifies where the file came from.
pub fn parse_agent_md(
    filename: &str,
    contents: &str,
    source: AgentSource,
) -> Result<AgentDefinition, String> {
    let (fm_src, body) = split_frontmatter(contents).ok_or_else(|| {
        format!("{filename}: missing or malformed YAML frontmatter (expected leading ---)")
    })?;
    let fm: Frontmatter = serde_yaml::from_str(fm_src)
        .map_err(|e| format!("{filename}: invalid frontmatter: {e}"))?;
    if fm.description.trim().is_empty() {
        return Err(format!("{filename}: description must not be empty"));
    }
    let body_trimmed = body.trim().to_string();
    if body_trimmed.is_empty() {
        return Err(format!(
            "{filename}: body (system prompt) must not be empty"
        ));
    }

    let permission_mode = match fm.permission_mode.as_deref() {
        None => None,
        Some("default") => Some(PermissionMode::Default),
        Some("acceptEdits") => Some(PermissionMode::AcceptEdits),
        Some("bypassPermissions") => Some(PermissionMode::BypassPermissions),
        Some("plan") => Some(PermissionMode::Plan),
        Some(other) => return Err(format!("{filename}: unknown permissionMode '{other}'")),
    };

    let agent_type = Path::new(filename)
        .file_stem()
        .and_then(|s| s.to_str())
        .unwrap_or(filename)
        .to_string();

    Ok(AgentDefinition {
        agent_type,
        description: fm.description.trim().to_string(),
        system_prompt: body_trimmed,
        tools: fm.tools,
        disallowed_tools: fm.disallowed_tools.unwrap_or_default(),
        model: fm.model,
        permission_mode,
        max_turns: fm.max_turns,
        source,
    })
}

/// Returns (frontmatter_yaml, body) on success, or None when the file does not
/// begin with a `---` fence.
fn split_frontmatter(contents: &str) -> Option<(&str, &str)> {
    let rest = contents.strip_prefix("---\n")?;
    let end = rest.find("\n---")?;
    let fm = &rest[..end];
    let body_start = end + "\n---".len();
    let body = &rest[body_start..];
    let body = body.strip_prefix('\n').unwrap_or(body);
    Some((fm, body))
}

/// Scan a directory for `*.md` agent files. Skips files that fail to parse,
/// logging a warning. Returns whatever did parse.
pub fn load_agents_from_dir(dir: &Path, source: AgentSource) -> Vec<AgentDefinition> {
    let mut out = Vec::new();
    let entries = match std::fs::read_dir(dir) {
        Ok(e) => e,
        Err(_) => return out, // dir doesn't exist — that's fine
    };
    for entry in entries.flatten() {
        let path: PathBuf = entry.path();
        if path.extension().and_then(|s| s.to_str()) != Some("md") {
            continue;
        }
        let filename = path
            .file_name()
            .and_then(|s| s.to_str())
            .unwrap_or("?")
            .to_string();
        match std::fs::read_to_string(&path) {
            Ok(contents) => match parse_agent_md(&filename, &contents, source) {
                Ok(def) => out.push(def),
                Err(e) => tracing::warn!("agents loader: {e}"),
            },
            Err(e) => tracing::warn!("agents loader: cannot read {}: {e}", path.display()),
        }
    }
    out
}

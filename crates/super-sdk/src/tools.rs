//! The one table that maps a built-in tool name to its implementation.
//!
//! Both the command-line program (`crates/super/src/setup.rs`) and the SDK build
//! their tool list from here, so "which tools does `super` have" has exactly one
//! answer no matter how the agent was started.

use std::path::Path;
use std::sync::Arc;
use super_agent::{DynTool, ToolSession};
use super_coding::settings::Settings;
use super_mcp::McpManager;

/// Tools enabled when the caller does not choose.
pub const DEFAULT_TOOLS: &[&str] = &["Read", "Write", "Edit", "Bash"];

/// Every built-in tool name the agent recognizes.
pub const ALL_TOOLS: &[&str] = &["Read", "Write", "Edit", "Bash", "grep", "find", "ls", "mcp"];

/// Resolve a requested tool list into concrete tools.
///
/// Names that are not in [`ALL_TOOLS`] are ignored rather than rejected, which
/// keeps a configuration written for a newer build usable on an older one.
pub fn build_tools(
    names: &[String],
    cwd: &Path,
    settings: &Settings,
    mcp: Option<&McpManager>,
) -> Vec<DynTool> {
    let mut tools: Vec<DynTool> = Vec::new();
    // Read, Edit and Write share one record of what was read, so they work
    // together even outside a session; a session binds them to its own.
    let session = ToolSession::new();
    for name in names {
        let tool: Option<DynTool> = match name.as_str() {
            "Read" => Some(Arc::new(super_agent::tools::read::ReadTool {
                cwd: cwd.to_path_buf(),
                session: session.clone(),
            })),
            "Bash" => {
                let mut bash = super_agent::tools::bash::BashTool::new(cwd.to_path_buf());
                if let Some(path) = settings
                    .shell_path
                    .as_ref()
                    .filter(|path| !path.trim().is_empty())
                {
                    bash.shell_path = Some(path.clone());
                }
                bash.command_prefix = settings.shell_command_prefix.clone();
                Some(Arc::new(bash))
            }
            "Edit" => Some(Arc::new(super_agent::tools::edit::EditTool {
                cwd: cwd.to_path_buf(),
                session: session.clone(),
            })),
            "Write" => Some(Arc::new(super_agent::tools::write::WriteTool {
                cwd: cwd.to_path_buf(),
                session: session.clone(),
            })),
            "grep" => Some(Arc::new(super_coding::tools::grep::GrepTool {
                cwd: cwd.to_path_buf(),
            })),
            "find" => Some(Arc::new(super_coding::tools::find::FindTool {
                cwd: cwd.to_path_buf(),
            })),
            "ls" => Some(Arc::new(super_coding::tools::ls::LsTool {
                cwd: cwd.to_path_buf(),
            })),
            "mcp" => mcp
                .cloned()
                .map(super_mcp::McpTool::new)
                .map(|tool| Arc::new(tool) as DynTool),
            _ => None,
        };
        if let Some(tool) = tool {
            tools.push(tool);
        }
    }
    tools
}

/// Apply an allowlist and an exclusion list to the built-in tool names.
pub fn select_tool_names(
    allow: Option<&[String]>,
    exclude: &[String],
    no_tools: bool,
    defaults: Option<&[String]>,
) -> Vec<String> {
    if no_tools {
        return Vec::new();
    }
    let mut names: Vec<String> = match allow {
        Some(allow) => allow.to_vec(),
        None => {
            let entries = defaults.unwrap_or_default();
            let mut names: Vec<String> = entries
                .iter()
                .filter(|name| !name.starts_with(['+', '-']))
                .cloned()
                .collect();
            if defaults.is_none() || (!entries.is_empty() && names.is_empty()) {
                names = DEFAULT_TOOLS.iter().map(|name| name.to_string()).collect();
            }
            for entry in entries {
                if let Some(name) = entry.strip_prefix('+') {
                    if !name.is_empty() && !names.iter().any(|existing| existing == name) {
                        names.push(name.to_string());
                    }
                } else if let Some(name) = entry.strip_prefix('-') {
                    names.retain(|existing| existing != name);
                }
            }
            names
        }
    };
    names.retain(|name| ALL_TOOLS.contains(&name.as_str()));
    names.retain(|name| !exclude.iter().any(|excluded| excluded == name));
    let mut seen = std::collections::HashSet::new();
    names.retain(|name| seen.insert(name.clone()));
    names
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn defaults_match_the_command_line_program() {
        assert_eq!(select_tool_names(None, &[], false, None), DEFAULT_TOOLS);
    }

    #[test]
    fn exclusions_apply_after_the_allowlist() {
        let allow = ["Read".to_string(), "grep".to_string(), "Bash".to_string()];
        let exclude = ["Bash".to_string()];
        assert_eq!(
            select_tool_names(Some(&allow), &exclude, false, None),
            ["Read", "grep"]
        );
    }

    #[test]
    fn unknown_names_are_dropped() {
        let allow = ["Read".to_string(), "teleport".to_string()];
        assert_eq!(select_tool_names(Some(&allow), &[], false, None), ["Read"]);
    }

    #[test]
    fn the_four_core_tools_have_no_lower_case_names() {
        let allow = ["read", "write", "edit", "bash"].map(str::to_string);
        assert!(select_tool_names(Some(&allow), &[], false, None).is_empty());
        let defaults = ["-bash", "+read"].map(str::to_string);
        assert_eq!(
            select_tool_names(None, &[], false, Some(&defaults)),
            DEFAULT_TOOLS
        );
        let exclude = ["bash".to_string()];
        assert_eq!(
            select_tool_names(None, &exclude, false, None),
            DEFAULT_TOOLS
        );
        let built = build_tools(&allow, Path::new("/work"), &Settings::default(), None);
        assert!(built.is_empty());
    }

    #[test]
    fn no_tools_wins() {
        let allow = ["Read".to_string()];
        assert!(select_tool_names(Some(&allow), &[], true, None).is_empty());
    }

    #[test]
    fn configured_defaults_and_cli_selection_have_ordered_precedence() {
        let defaults = ["-Bash", "+grep", "+grep", "+Bash", "-Write"].map(str::to_string);
        assert_eq!(
            select_tool_names(None, &[], false, Some(&defaults)),
            ["Read", "Edit", "grep", "Bash"]
        );
        let defaults = ["Read", "+grep"].map(str::to_string);
        let exclude = ["grep".to_string()];
        assert_eq!(
            select_tool_names(None, &exclude, false, Some(&defaults)),
            ["Read"]
        );
        let allow = ["ls".to_string()];
        assert_eq!(
            select_tool_names(Some(&allow), &[], false, Some(&defaults)),
            ["ls"]
        );
        assert!(select_tool_names(None, &[], false, Some(&[])).is_empty());
        assert!(select_tool_names(Some(&allow), &[], true, Some(&defaults)).is_empty());
    }
}

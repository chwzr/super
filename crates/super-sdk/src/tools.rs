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

/// Which of the tools a session adds itself (the agent tools, `Monitor`,
/// `TaskStop`, `Workflow`, `Skill`, `ScheduleWakeup`, `EnterWorktree`,
/// `ExitWorktree`) the same lists let through: the allowlist as it is, and
/// the exclusions together with the `-Name` entries of `defaults`
/// (`defaultTools`). Its other entries choose among the built-in tools only.
pub fn tool_selection(
    allow: Option<&[String]>,
    exclude: &[String],
    defaults: Option<&[String]>,
) -> super_coding::ToolSelection {
    let mut excluded = exclude.to_vec();
    if allow.is_none() {
        excluded.extend(
            defaults
                .unwrap_or_default()
                .iter()
                .filter_map(|entry| entry.strip_prefix('-'))
                .filter(|name| !name.is_empty())
                .map(str::to_string),
        );
    }
    super_coding::ToolSelection {
        allow: allow.map(<[String]>::to_vec),
        exclude: excluded,
    }
}

/// The tools a session adds itself, which the lists name too: they are no
/// unknown names.
pub const SESSION_TOOLS: &[&str] = &[
    "Monitor",
    "TaskStop",
    "Agent",
    "SendMessage",
    "ListAgents",
    "Workflow",
    "Skill",
    "ScheduleWakeup",
    "EnterWorktree",
    "ExitWorktree",
    "webmcp",
];

/// The names in the allowlist, the exclusions and `defaults`
/// (`defaultTools`, without its `+` and `-`) that are no tool's, each with
/// the tool name it differs from only in case, such as `read` and `Read`
/// from an earlier version, when there is one. They are left out without a
/// word otherwise, so a host can say so.
pub fn unknown_tool_names(
    allow: Option<&[String]>,
    exclude: &[String],
    defaults: Option<&[String]>,
) -> Vec<(String, Option<&'static str>)> {
    let known = || ALL_TOOLS.iter().chain(SESSION_TOOLS);
    let mut out: Vec<(String, Option<&'static str>)> = Vec::new();
    let entries = allow
        .unwrap_or_default()
        .iter()
        .chain(exclude)
        .chain(defaults.unwrap_or_default());
    for entry in entries {
        let name = entry.trim_start_matches(['+', '-']);
        if name.is_empty()
            || known().any(|known| *known == name)
            || out.iter().any(|(seen, _)| seen == name)
        {
            continue;
        }
        let like = known()
            .copied()
            .find(|known| known.eq_ignore_ascii_case(name));
        out.push((name.to_string(), like));
    }
    out
}

/// One line for `names` of no tool ([`unknown_tool_names`]), each with the
/// name it meant: `read (use Read), nothing`.
fn named(names: Vec<(String, Option<&'static str>)>) -> String {
    names
        .into_iter()
        .map(|(name, like)| match like {
            Some(like) => format!("{name} (use {like})"),
            None => name,
        })
        .collect::<Vec<_>>()
        .join(", ")
}

/// The warnings a host shows when a session starts, one line each, about
/// names that are no tool's: in the allowlist, the exclusions and
/// `defaults` ([`unknown_tool_names`]), and in the `tools` of the agent
/// types in `types`. Such a name is left out without a word otherwise, so a
/// selection or a type written for an earlier version would give a session
/// or an agent fewer tools than it names.
pub fn tool_name_warnings(
    allow: Option<&[String]>,
    exclude: &[String],
    defaults: Option<&[String]>,
    types: &[super_coding::agents::AgentType],
) -> Vec<String> {
    let mut warnings = Vec::new();
    let unknown = unknown_tool_names(allow, exclude, defaults);
    if !unknown.is_empty() {
        warnings.push(format!("unknown tool names ignored: {}", named(unknown)));
    }
    for agent_type in types {
        let names = match &agent_type.tools {
            super_coding::agents::ToolChoice::Only(names)
            | super_coding::agents::ToolChoice::AllExcept(names) => names,
            super_coding::agents::ToolChoice::All => continue,
        };
        let unknown = unknown_tool_names(Some(names), &[], None);
        if !unknown.is_empty() {
            warnings.push(format!(
                "unknown tool names in agent type {} ignored: {}",
                agent_type.name,
                named(unknown)
            ));
        }
    }
    warnings
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn names_of_no_tool_are_named_in_a_warning_for_the_session_and_for_agent_types() {
        let names = |list: &[&str]| list.iter().map(|name| name.to_string()).collect::<Vec<_>>();
        let of_type = |name: &str, tools| super_coding::agents::AgentType {
            name: name.into(),
            description: String::new(),
            instructions: String::new(),
            model: None,
            effort: None,
            tools,
            source: super_coding::agents::AgentTypeSource::BuiltIn,
        };
        let types = [
            of_type(
                "checker",
                super_coding::agents::ToolChoice::Only(names(&["Read", "bash", "teleport"])),
            ),
            of_type(
                "fine",
                super_coding::agents::ToolChoice::Only(names(&["Read", "Monitor"])),
            ),
            of_type("all", super_coding::agents::ToolChoice::All),
        ];
        assert_eq!(
            tool_name_warnings(None, &[], Some(&names(&["read", "Edit"])), &types),
            [
                "unknown tool names ignored: read (use Read)",
                "unknown tool names in agent type checker ignored: bash (use Bash), teleport",
            ]
        );
        assert!(tool_name_warnings(None, &[], None, &types[1..]).is_empty());
    }

    #[test]
    fn names_of_no_tool_are_found_with_the_name_they_meant() {
        let names = |list: &[&str]| list.iter().map(|name| name.to_string()).collect::<Vec<_>>();
        let allow = names(&["read", "Bash", "Monitor"]);
        let defaults = names(&["+grep", "-bash", "write", "nothing", "read"]);
        assert_eq!(
            unknown_tool_names(Some(&allow), &names(&["Agent"]), Some(&defaults)),
            [
                ("read".to_string(), Some("Read")),
                ("bash".to_string(), Some("Bash")),
                ("write".to_string(), Some("Write")),
                ("nothing".to_string(), None),
            ]
        );
        assert!(unknown_tool_names(None, &[], Some(&names(&["Read", "-Monitor"]))).is_empty());
    }

    #[test]
    fn the_same_lists_choose_among_the_tools_a_session_adds() {
        let names = |list: &[&str]| list.iter().map(|name| name.to_string()).collect::<Vec<_>>();
        let everything = tool_selection(None, &[], None);
        assert!(everything.admits("Monitor") && everything.admits("Agent"));
        let allow = names(&["Read", "Bash", "TaskStop"]);
        let allowed = tool_selection(Some(&allow), &names(&["TaskStop"]), None);
        assert!(!allowed.admits("Monitor"));
        assert!(!allowed.admits("TaskStop"));
        let defaults = names(&["+grep", "-Monitor", "-EnterWorktree", "Read"]);
        let configured = tool_selection(None, &names(&["Skill"]), Some(&defaults));
        assert!(!configured.admits("Monitor"));
        assert!(!configured.admits("EnterWorktree"));
        assert!(!configured.admits("Skill"));
        assert!(configured.admits("Read") && configured.admits("ExitWorktree"));
        // An allowlist on the command line replaces defaultTools, as it does
        // for the built-in tools.
        let replaced = tool_selection(Some(&allow), &[], Some(&defaults));
        assert!(replaced.admits("Bash") && !replaced.admits("Monitor"));
    }

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

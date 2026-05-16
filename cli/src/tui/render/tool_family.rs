//! Maps a tool name (and optionally its input) to a rendering family.
//!
//! - `ReadSearch` — collapses into a dim-gray `Read N files` / `Searched for N`
//!   summary line. Per spec, this covers tools that don't mutate state.
//! - `Mutating` — renders as a per-call `⏺ Name(args)` block with results.
//! - `Subagent` — `Task`/`Agent` calls; rendered per-call for now (Claude's
//!   grouped subagent render is a separate follow-up).

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ToolFamily {
    ReadSearch,
    Mutating,
    Subagent,
}

/// Classify a tool by name. Unknown names default to `Mutating` — safer to
/// render them in full than to silently hide them in a collapsed summary.
pub fn classify(name: &str) -> ToolFamily {
    match name {
        "Read" | "Grep" | "Glob" | "LSP" => ToolFamily::ReadSearch,
        "Task" | "Agent" => ToolFamily::Subagent,
        _ => ToolFamily::Mutating,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn read_grep_glob_lsp_classify_as_read_search() {
        for n in ["Read", "Grep", "Glob", "LSP"] {
            assert_eq!(classify(n), ToolFamily::ReadSearch, "tool={n}");
        }
    }

    #[test]
    fn task_and_agent_classify_as_subagent() {
        assert_eq!(classify("Task"), ToolFamily::Subagent);
        assert_eq!(classify("Agent"), ToolFamily::Subagent);
    }

    #[test]
    fn mutating_tools_classify_as_mutating() {
        for n in ["Bash", "Write", "Edit", "MultiEdit", "NotebookEdit", "WebFetch", "WebSearch"] {
            assert_eq!(classify(n), ToolFamily::Mutating, "tool={n}");
        }
    }

    #[test]
    fn unknown_tool_defaults_to_mutating() {
        assert_eq!(classify("SomeMcpTool"), ToolFamily::Mutating);
    }
}

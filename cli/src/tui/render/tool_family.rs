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

/// Per-spec phrasing for the collapsed-batch summary line. Returns a fragment
/// (e.g. "Read 3 files", "Searched for 2 patterns") suitable for joining with
/// commas. Returns None for unsupported names — caller falls back to the tool
/// name.
pub fn batch_fragment(name: &str, count: usize) -> Option<String> {
    let (sing, plur) = match name {
        "Read" => ("Read 1 file", "Read N files"),
        "Grep" => ("Searched for 1 pattern", "Searched for N patterns"),
        "Glob" => ("listed 1 directory", "listed N directories"),
        "LSP"  => ("queried 1 symbol", "queried N symbols"),
        _ => return None,
    };
    let s = if count == 1 { sing.to_string() } else { plur.replace("N", &count.to_string()) };
    Some(s)
}

#[cfg(test)]
mod fragment_tests {
    use super::*;

    #[test]
    fn read_fragment_singular_and_plural() {
        assert_eq!(batch_fragment("Read", 1).unwrap(), "Read 1 file");
        assert_eq!(batch_fragment("Read", 5).unwrap(), "Read 5 files");
    }

    #[test]
    fn grep_glob_fragments() {
        assert_eq!(batch_fragment("Grep", 1).unwrap(), "Searched for 1 pattern");
        assert_eq!(batch_fragment("Grep", 2).unwrap(), "Searched for 2 patterns");
        assert_eq!(batch_fragment("Glob", 1).unwrap(), "listed 1 directory");
        assert_eq!(batch_fragment("Glob", 4).unwrap(), "listed 4 directories");
    }

    #[test]
    fn unknown_tool_returns_none() {
        assert!(batch_fragment("Mystery", 3).is_none());
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

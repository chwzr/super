//! Maps a tool name (and optionally its input) to a rendering family.
//!
//! - `ReadSearch` — collapses into a dim-gray `Read N files` / `Searched for N`
//!   summary line. Per spec, this covers tools that don't mutate state.
//! - `Mutating` — renders as a per-call `⏺ Name(args)` block with results.
//! - `Subagent` — `Task`/`Agent` calls; rendered per-call for now (Claude's
//!   grouped subagent render is a separate follow-up).

use ratatui::{
    style::{Modifier, Style},
    text::Span,
};

use crate::tui::colors::CC_DIM;

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

/// Spans for the collapsed-batch summary fragment (e.g. `Read 3 files`).
/// The count is rendered bold while keeping the dim foreground (matches
/// Claude Code's `Read **N** files`). Unknown tool names fall back to
/// `<Name> x<count>` so the line is still informative.
pub fn batch_fragment(name: &str, count: usize) -> Vec<Span<'static>> {
    let dim = Style::default().fg(CC_DIM);
    let bold = dim.add_modifier(Modifier::BOLD);
    let (prefix, sing_suffix, plur_suffix) = match name {
        "Read" => ("Read ", " file", " files"),
        "Grep" => ("Searched for ", " pattern", " patterns"),
        "Glob" => ("listed ", " directory", " directories"),
        "LSP"  => ("queried ", " symbol", " symbols"),
        _ => {
            return vec![
                Span::styled(format!("{name} x"), dim),
                Span::styled(count.to_string(), bold),
            ];
        }
    };
    let suffix = if count == 1 { sing_suffix } else { plur_suffix };
    vec![
        Span::styled(prefix.to_string(), dim),
        Span::styled(count.to_string(), bold),
        Span::styled(suffix.to_string(), dim),
    ]
}

#[cfg(test)]
mod fragment_tests {
    use super::*;

    fn flat(spans: &[Span<'_>]) -> String {
        spans.iter().map(|s| s.content.as_ref()).collect()
    }

    fn bold_count(spans: &[Span<'_>]) -> usize {
        spans.iter().filter(|s| s.style.add_modifier.contains(Modifier::BOLD)).count()
    }

    #[test]
    fn read_fragment_singular_and_plural() {
        assert_eq!(flat(&batch_fragment("Read", 1)), "Read 1 file");
        assert_eq!(flat(&batch_fragment("Read", 5)), "Read 5 files");
    }

    #[test]
    fn grep_glob_fragments() {
        assert_eq!(flat(&batch_fragment("Grep", 1)), "Searched for 1 pattern");
        assert_eq!(flat(&batch_fragment("Grep", 2)), "Searched for 2 patterns");
        assert_eq!(flat(&batch_fragment("Glob", 1)), "listed 1 directory");
        assert_eq!(flat(&batch_fragment("Glob", 4)), "listed 4 directories");
    }

    #[test]
    fn unknown_tool_falls_back_to_name_x_count() {
        assert_eq!(flat(&batch_fragment("Mystery", 3)), "Mystery x3");
    }

    #[test]
    fn count_span_is_bold() {
        assert_eq!(bold_count(&batch_fragment("Read", 5)), 1);
        assert_eq!(bold_count(&batch_fragment("Grep", 2)), 1);
        assert_eq!(bold_count(&batch_fragment("Mystery", 3)), 1);
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

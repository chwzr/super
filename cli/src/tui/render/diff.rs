//! Diff helpers: compute additions/removals, render hunks as colored lines.
//!
//! Uses the `similar` crate for line-based diffing. The output mirrors Claude
//! Code's `StructuredDiffList` layout (see cli/docs/tool-call-render-spec.md).

use ratatui::{
    style::{Modifier, Style},
    text::{Line, Span},
};
use similar::{ChangeTag, TextDiff};

use crate::tui::colors::{CC_DIFF_ADD_BG, CC_DIFF_ADD_FG, CC_DIFF_DEL_BG, CC_DIFF_DEL_FG, CC_DIM};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct DiffCounts {
    pub additions: usize,
    pub removals: usize,
}

/// Count added (`+`) and removed (`-`) lines between `old` and `new`.
pub fn count_changes(old: &str, new: &str) -> DiffCounts {
    let diff = TextDiff::from_lines(old, new);
    let mut additions = 0;
    let mut removals = 0;
    for ch in diff.iter_all_changes() {
        match ch.tag() {
            ChangeTag::Insert => additions += 1,
            ChangeTag::Delete => removals += 1,
            ChangeTag::Equal => {}
        }
    }
    DiffCounts {
        additions,
        removals,
    }
}

/// Format the "Added X line[s], removed Y line[s]" summary string, matching
/// Claude's grammar (lowercase 'r' when there are also additions, uppercase R
/// when only removals). Returns spans so the numerals can be bold.
pub fn summary_spans(counts: DiffCounts) -> Vec<Span<'static>> {
    let mut spans: Vec<Span<'static>> = Vec::new();
    if counts.additions > 0 {
        spans.push(Span::raw("Added "));
        spans.push(Span::styled(
            counts.additions.to_string(),
            Style::default().add_modifier(Modifier::BOLD),
        ));
        spans.push(Span::raw(if counts.additions == 1 {
            " line"
        } else {
            " lines"
        }));
    }
    if counts.removals > 0 {
        let lead = if counts.additions > 0 {
            ", removed "
        } else {
            "Removed "
        };
        spans.push(Span::raw(lead));
        spans.push(Span::styled(
            counts.removals.to_string(),
            Style::default().add_modifier(Modifier::BOLD),
        ));
        spans.push(Span::raw(if counts.removals == 1 {
            " line"
        } else {
            " lines"
        }));
    }
    spans
}

/// Render colored diff hunks. Each output `Line` is indented 4 spaces (so the
/// body's leading space aligns at column 5, under `⎿  `). Format per line:
///     " <N> -content"  (removed) — fg 167 bg 52
///     " <N> +content"  (added)   — fg 77  bg 22
///     " <N>  content"  (context) — dim
pub fn render_hunks(old: &str, new: &str) -> Vec<Line<'static>> {
    let diff = TextDiff::from_lines(old, new);
    let mut out: Vec<Line<'static>> = Vec::new();
    let mut old_lineno = 1usize;
    let mut new_lineno = 1usize;
    let lineno_width = diff.iter_all_changes().count().to_string().len().max(2);
    for change in diff.iter_all_changes() {
        let (lineno_for_display, tag_char, style) = match change.tag() {
            ChangeTag::Delete => {
                let n = old_lineno;
                old_lineno += 1;
                (
                    n,
                    '-',
                    Style::default().fg(CC_DIFF_DEL_FG).bg(CC_DIFF_DEL_BG),
                )
            }
            ChangeTag::Insert => {
                let n = new_lineno;
                new_lineno += 1;
                (
                    n,
                    '+',
                    Style::default().fg(CC_DIFF_ADD_FG).bg(CC_DIFF_ADD_BG),
                )
            }
            ChangeTag::Equal => {
                let n = new_lineno;
                old_lineno += 1;
                new_lineno += 1;
                (n, ' ', Style::default().fg(CC_DIM))
            }
        };
        let content = change.value().trim_end_matches('\n').to_string();
        let body = format!(
            " {:>width$} {}{}",
            lineno_for_display,
            tag_char,
            content,
            width = lineno_width
        );
        out.push(Line::from(vec![
            Span::raw("    "),
            Span::styled(body, style),
        ]));
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn count_changes_single_line_swap() {
        let c = count_changes("hello\n", "hi\n");
        assert_eq!(
            c,
            DiffCounts {
                additions: 1,
                removals: 1
            }
        );
    }

    #[test]
    fn count_changes_pure_insert() {
        let c = count_changes("", "a\nb\n");
        assert_eq!(
            c,
            DiffCounts {
                additions: 2,
                removals: 0
            }
        );
    }

    #[test]
    fn summary_spans_adds_and_removes_uses_lowercase_r() {
        let spans = summary_spans(DiffCounts {
            additions: 1,
            removals: 1,
        });
        let text: String = spans.iter().map(|s| s.content.as_ref()).collect();
        assert_eq!(text, "Added 1 line, removed 1 line");
    }

    #[test]
    fn summary_spans_only_removed_uses_capital_r() {
        let spans = summary_spans(DiffCounts {
            additions: 0,
            removals: 3,
        });
        let text: String = spans.iter().map(|s| s.content.as_ref()).collect();
        assert_eq!(text, "Removed 3 lines");
    }

    #[test]
    fn summary_spans_only_added_no_removed_suffix() {
        let spans = summary_spans(DiffCounts {
            additions: 5,
            removals: 0,
        });
        let text: String = spans.iter().map(|s| s.content.as_ref()).collect();
        assert_eq!(text, "Added 5 lines");
    }

    #[test]
    fn render_hunks_marks_added_and_removed_with_color() {
        let lines = render_hunks("hello\n", "hi\n");
        let texts: Vec<String> = lines
            .iter()
            .map(|l| {
                l.spans
                    .iter()
                    .map(|s| s.content.as_ref())
                    .collect::<String>()
            })
            .collect();
        assert!(
            texts.iter().any(|t| t.contains(" 1 -hello")),
            "removed: {texts:?}"
        );
        assert!(
            texts.iter().any(|t| t.contains(" 1 +hi")),
            "added: {texts:?}"
        );
    }
}

use std::path::PathBuf;

use serde::{Deserialize, Serialize};

/// Framework-agnostic render description. Tools emit this; TUI / server /
/// web frontend interpret it. The bridge between Claude's React-typed
/// render hooks and Super's multi-frontend rendering surface.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum RenderSpec {
    /// Inline header line with a verb + target (e.g. "Reading src/foo.rs").
    Header {
        verb: String,
        target: Option<String>,
        tag: Option<Tag>,
    },

    /// Multiline plain text with a portable style hint. Renderer maps the
    /// hint to its own palette (CLI: `cli/src/tui/colors.rs`; web: design tokens).
    Text { body: String, style: TextStyle },

    /// A code block with optional language; rendered monospace.
    Code {
        language: Option<String>,
        body: String,
        truncated: bool,
    },

    /// A unified diff. Renderer applies +/- colors and gutter.
    Diff {
        file_path: String,
        hunks: Vec<DiffHunk>,
    },

    /// File-path-prefixed list (Grep/Glob results).
    PathList {
        entries: Vec<PathEntry>,
        total: usize,
        truncated: bool,
    },

    /// Key/value pairs (e.g. tool-use tag, web search result metadata).
    KeyValues { rows: Vec<(String, String)> },

    /// Collapsible block. Children rendered only when expanded.
    Collapsible {
        summary: String,
        expanded_by_default: bool,
        children: Vec<RenderSpec>,
    },

    /// Vertical group of specs (rendered as a column).
    Group { children: Vec<RenderSpec> },

    /// Horizontal group of specs (rendered as a row).
    Row { children: Vec<RenderSpec> },

    /// Status badge: in-progress / success / error / rejected / queued.
    Status {
        state: StatusState,
        message: Option<String>,
    },

    /// Interactive widget. The renderer maps `widget` to its own input
    /// component; the agent loop suspends the turn until the user completes
    /// it.
    Interactive {
        widget: InteractiveWidget,
        /// Renderer-agnostic, serializable response shape. Tools that
        /// produce an Interactive spec also document what response payload
        /// they expect to receive back.
        response_schema: serde_json::Value,
    },

    /// Empty — explicitly render nothing.
    Nothing,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DiffHunk {
    pub old_start: u32,
    pub new_start: u32,
    pub lines: Vec<DiffLine>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum DiffLine {
    Context { line: String },
    Add { line: String },
    Remove { line: String },
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PathEntry {
    pub path: PathBuf,
    pub line: Option<u32>,
    pub preview: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum Tag {
    Timeout { ms: u64 },
    Model { id: String },
    Truncated,
    ResumeId { value: String },
    Custom { value: String },
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum StatusState {
    Queued,
    InProgress,
    Success,
    Error,
    Rejected,
}

/// Closed set of style hints for `RenderSpec::Text`. Renderers map each hint
/// to a palette entry; tools cannot express arbitrary colors by design.
#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum TextStyle {
    Plain,
    Dim,
    Error,
    Success,
    Warn,
    Strong,
}

// InteractiveWidget is the only variant Batch 1 does not actually emit;
// included so the enum is forward-compatible with Batch 2.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum InteractiveWidget {
    /// AskUserQuestion. 1–4 questions, each with options (single- or
    /// multi-select), optional preview, and an "Other" text fallback.
    MultiQuestion { questions: Vec<Question> },

    /// ExitPlanMode. Renderer shows the plan markdown and asks accept/reject.
    PlanApproval { plan_markdown: String },

    /// Permission prompt for a tool call.
    PermissionPrompt {
        tool_name: String,
        input_summary: String,
        rule_suggestions: Vec<RuleSuggestion>,
    },

    /// SendMessage to a peer agent / channel.
    MessageCompose {
        recipients: Vec<String>,
        draft: String,
    },
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Question {
    pub question: String,
    pub header: String,
    pub multi_select: bool,
    pub options: Vec<QuestionOption>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct QuestionOption {
    pub label: String,
    pub description: String,
    /// Markdown rendered in a monospace box (renderer-dependent).
    pub preview: Option<String>,
}

/// Permission rule suggestion shown on a permission prompt. Mirrors
/// Claude's `ruleSuggestions`. The actual `PermissionRule` type lives in
/// `cli/src/tools/permission.rs`; we duplicate the *transport* shape here
/// so `shared` doesn't depend on the cli crate.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RuleSuggestion {
    pub label: String,
    pub rule_json: serde_json::Value,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn nothing_round_trips_through_json() {
        let spec = RenderSpec::Nothing;
        let json = serde_json::to_string(&spec).unwrap();
        let back: RenderSpec = serde_json::from_str(&json).unwrap();
        assert!(matches!(back, RenderSpec::Nothing));
        assert_eq!(json, r#"{"kind":"nothing"}"#);
    }

    #[test]
    fn header_serializes_with_kind_tag() {
        let spec = RenderSpec::Header {
            verb: "Reading".into(),
            target: Some("src/foo.rs".into()),
            tag: None,
        };
        let json = serde_json::to_string(&spec).unwrap();
        assert!(json.contains(r#""kind":"header""#));
        assert!(json.contains(r#""verb":"Reading""#));
    }

    #[test]
    fn text_serializes_with_style_tag() {
        let spec = RenderSpec::Text {
            body: "boom".into(),
            style: TextStyle::Error,
        };
        let json = serde_json::to_string(&spec).unwrap();
        assert!(json.contains(r#""kind":"text""#), "got: {json}");
        assert!(json.contains(r#""style":"error""#), "got: {json}");
        assert!(json.contains(r#""body":"boom""#), "got: {json}");
    }

    #[test]
    fn text_style_round_trips() {
        for s in [
            TextStyle::Plain,
            TextStyle::Dim,
            TextStyle::Error,
            TextStyle::Success,
            TextStyle::Warn,
            TextStyle::Strong,
        ] {
            let spec = RenderSpec::Text {
                body: "x".into(),
                style: s,
            };
            let json = serde_json::to_string(&spec).unwrap();
            let back: RenderSpec = serde_json::from_str(&json).unwrap();
            match back {
                RenderSpec::Text { style, .. } => assert_eq!(style, s),
                _ => panic!("wrong variant"),
            }
        }
    }

    #[test]
    fn diff_round_trips() {
        let spec = RenderSpec::Diff {
            file_path: "/tmp/a".into(),
            hunks: vec![DiffHunk {
                old_start: 1,
                new_start: 1,
                lines: vec![
                    DiffLine::Add { line: "+x".into() },
                    DiffLine::Remove { line: "-y".into() },
                ],
            }],
        };
        let json = serde_json::to_string(&spec).unwrap();
        let back: RenderSpec = serde_json::from_str(&json).unwrap();
        match back {
            RenderSpec::Diff { hunks, .. } => assert_eq!(hunks.len(), 1),
            _ => panic!("wrong variant"),
        }
    }
}

#![cfg_attr(
    test,
    allow(
        clippy::unwrap_used,
        clippy::expect_used,
        clippy::panic,
        clippy::todo,
        clippy::unimplemented,
        clippy::unreachable,
    )
)]

pub mod auth;
pub mod render_spec;

pub use auth::*;
pub use render_spec::{
    DiffHunk, DiffLine, InteractiveWidget, PathEntry, Question, QuestionOption, RenderSlot,
    RenderSpec, RuleSuggestion, StatusState, Tag, TextStyle,
};

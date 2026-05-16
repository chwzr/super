//! Default helpers for `Tool` impls so per-tool files stay short.
//!
//! Usage pattern: a tool struct implements only the methods it needs to
//! override; the rest come from the trait defaults defined in
//! `crate::tools::contract::Tool`.
//!
//! For tools that want to be even more concise, the `tool_defaults!`
//! macro can be expanded inside an impl block to spell out the most
//! commonly-overridden defaults. Use only when it saves real
//! boilerplate — most tools are fine relying on the trait defaults.

/// Expand inside an `impl Tool for X` block to add the four most-overridden
/// flag methods with read-only / safe / non-destructive defaults.
#[macro_export]
macro_rules! tool_read_only_defaults {
    () => {
        fn is_read_only(&self, _input: &serde_json::Value) -> bool { true }
        fn is_concurrency_safe(&self, _input: &serde_json::Value) -> bool { true }
        fn is_destructive(&self, _input: &serde_json::Value) -> bool { false }
        fn is_open_world(&self, _input: &serde_json::Value) -> bool { false }
    };
}

/// Expand for tools that perform writes (file edits, shell side-effects).
#[macro_export]
macro_rules! tool_write_defaults {
    () => {
        fn is_read_only(&self, _input: &serde_json::Value) -> bool { false }
        fn is_concurrency_safe(&self, _input: &serde_json::Value) -> bool { false }
        fn is_destructive(&self, _input: &serde_json::Value) -> bool { true }
        fn is_open_world(&self, _input: &serde_json::Value) -> bool { false }
    };
}

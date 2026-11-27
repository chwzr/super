//! Placeholders for prompt text.
//!
//! Some text the model reads as guidance is not written yet: a tool
//! description, a parameter description, the fixed sentences of a notice, the
//! instructions of an agent type, a section of the system prompt. Each such
//! slot holds a placeholder instead, so that the wording can be written later
//! and put in its place.
//!
//! A placeholder has the form `SUPER_PROMPT_TODO(<key>)`, and one search for
//! [`PREFIX`] finds every one of them. The key names the slot:
//!
//! - a tool description: `Read.description`, from [`tool_description`]
//! - a parameter description: `Read.param.file_path`, from [`tool_parameter`];
//!   a nested parameter joins its path with dots, as in `Monitor.param.ws.url`
//! - anything else: `<Area>.<name>`, from [`prompt_todo`], for example
//!   `Notification.header`, `AgentType.Explore.instructions` or
//!   `SystemPrompt.advanced`
//!
//! A placeholder is the whole text of its slot, with nothing before or after
//! it. Write one with these functions, never by hand, so the form stays the
//! same everywhere.
//!
//! An error message a tool returns is not prompt text and gets no placeholder.

/// The text every placeholder starts with.
pub const PREFIX: &str = "SUPER_PROMPT_TODO(";

/// The placeholder for the slot `key`, such as `SystemPrompt.advanced`.
///
/// The key has an area and a name joined by a dot, and holds no spaces or
/// parentheses.
pub fn prompt_todo(key: &str) -> String {
    debug_assert!(
        key.contains('.')
            && !key.starts_with('.')
            && !key.ends_with('.')
            && !key.contains("..")
            && !key
                .chars()
                .any(|character| character.is_whitespace() || matches!(character, '(' | ')')),
        "a placeholder key is `<Area>.<name>` without spaces or parentheses: {key:?}"
    );
    format!("{PREFIX}{key})")
}

/// The placeholder for the description of the tool `tool`.
pub fn tool_description(tool: &str) -> String {
    prompt_todo(&format!("{tool}.description"))
}

/// The placeholder for the description of the parameter `parameter` of the
/// tool `tool`. A nested parameter is its path joined with dots, such as
/// `ws.url`.
pub fn tool_parameter(tool: &str, parameter: &str) -> String {
    prompt_todo(&format!("{tool}.param.{parameter}"))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn placeholders_have_the_agreed_form() {
        assert_eq!(
            tool_description("Read"),
            "SUPER_PROMPT_TODO(Read.description)"
        );
        assert_eq!(
            tool_parameter("Read", "file_path"),
            "SUPER_PROMPT_TODO(Read.param.file_path)"
        );
        assert_eq!(
            tool_parameter("Monitor", "ws.url"),
            "SUPER_PROMPT_TODO(Monitor.param.ws.url)"
        );
        assert_eq!(
            prompt_todo("Notification.header"),
            "SUPER_PROMPT_TODO(Notification.header)"
        );
        assert_eq!(
            prompt_todo("AgentType.Explore.instructions"),
            "SUPER_PROMPT_TODO(AgentType.Explore.instructions)"
        );
        assert_eq!(
            prompt_todo("SystemPrompt.advanced"),
            "SUPER_PROMPT_TODO(SystemPrompt.advanced)"
        );
        assert!(prompt_todo("Read.description").starts_with(PREFIX));
    }

    #[test]
    #[cfg(debug_assertions)]
    #[should_panic(expected = "a placeholder key")]
    fn a_key_without_an_area_is_refused() {
        prompt_todo("description");
    }

    #[test]
    #[cfg(debug_assertions)]
    #[should_panic(expected = "a placeholder key")]
    fn a_key_with_a_space_is_refused() {
        prompt_todo("Read.file path");
    }
}

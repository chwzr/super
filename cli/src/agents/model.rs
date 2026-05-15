/// Resolve an agent's `model` field (alias or full id) to a full OpenRouter
/// model id, falling back to the parent model.
pub fn resolve_model(agent_model: Option<&str>, parent_model: &str) -> String {
    match agent_model {
        None | Some("inherit") => parent_model.to_string(),
        Some("sonnet") => "anthropic/claude-sonnet-4-6".to_string(),
        Some("opus") => "anthropic/claude-opus-4-7".to_string(),
        Some("haiku") => "anthropic/claude-haiku-4-5-20251001".to_string(),
        Some(other) => other.to_string(),
    }
}

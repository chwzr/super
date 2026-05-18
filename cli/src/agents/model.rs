use crate::providers;

/// Resolve an agent's `model` field (alias or full id) to a full OpenRouter
/// model id, using the parent's provider for class aliases.
pub fn resolve_model(
    agent_model: Option<&str>,
    parent_model: &str,
    parent_provider: &str,
) -> String {
    match agent_model {
        None | Some("inherit") => parent_model.to_string(),
        Some("sonnet") => providers::resolve_slug(parent_provider, "sonnet").to_string(),
        Some("opus") => providers::resolve_slug(parent_provider, "opus").to_string(),
        Some("haiku") => providers::resolve_slug(parent_provider, "haiku").to_string(),
        Some(other) => other.to_string(),
    }
}

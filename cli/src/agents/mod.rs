pub mod definition;
pub mod model;

#[cfg(test)]
mod tests {
    use super::definition::*;

    #[test]
    fn agent_definition_constructible() {
        let def = AgentDefinition {
            agent_type: "general-purpose".into(),
            description: "general agent".into(),
            system_prompt: "you are a general agent".into(),
            tools: Some(vec!["*".into()]),
            disallowed_tools: vec![],
            model: None,
            permission_mode: None,
            max_turns: None,
            source: AgentSource::BuiltIn,
        };
        assert_eq!(def.agent_type, "general-purpose");
        assert!(matches!(def.source, AgentSource::BuiltIn));
    }

    #[test]
    fn resolve_model_handles_aliases_and_inherit() {
        use super::model::resolve_model;
        let parent = "anthropic/claude-sonnet-4-6";
        assert_eq!(resolve_model(Some("sonnet"), parent), "anthropic/claude-sonnet-4-6");
        assert_eq!(resolve_model(Some("opus"), parent), "anthropic/claude-opus-4-7");
        assert_eq!(resolve_model(Some("haiku"), parent), "anthropic/claude-haiku-4-5-20251001");
        assert_eq!(resolve_model(Some("inherit"), parent), parent);
        assert_eq!(resolve_model(None, parent), parent);
        // Unknown -> pass through
        assert_eq!(resolve_model(Some("anthropic/claude-something-else"), parent), "anthropic/claude-something-else");
    }
}

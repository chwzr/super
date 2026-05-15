pub mod built_in;
pub mod definition;
pub mod model;
pub mod permission;

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

    #[test]
    fn permission_overlay_parent_bypass_wins() {
        use super::permission::resolve_permission_mode;
        use crate::state::store::PermissionMode;
        let out = resolve_permission_mode(&PermissionMode::Bypass, Some(&PermissionMode::Plan), false);
        assert!(matches!(out, PermissionMode::Bypass));
    }

    #[test]
    fn permission_overlay_parent_accept_edits_wins() {
        use super::permission::resolve_permission_mode;
        use crate::state::store::PermissionMode;
        let out = resolve_permission_mode(&PermissionMode::AcceptEdits, Some(&PermissionMode::Plan), false);
        assert!(matches!(out, PermissionMode::AcceptEdits));
    }

    #[test]
    fn permission_overlay_agent_override_applies_when_parent_default() {
        use super::permission::resolve_permission_mode;
        use crate::state::store::PermissionMode;
        let out = resolve_permission_mode(&PermissionMode::Default, Some(&PermissionMode::Plan), false);
        assert!(matches!(out, PermissionMode::Plan));
    }

    #[test]
    fn permission_overlay_no_agent_override_inherits_parent() {
        use super::permission::resolve_permission_mode;
        use crate::state::store::PermissionMode;
        let out = resolve_permission_mode(&PermissionMode::Default, None, false);
        assert!(matches!(out, PermissionMode::Default));
    }

    #[test]
    fn built_in_agents_list_contains_expected_types() {
        use super::built_in::built_in_agents;
        let agents = built_in_agents();
        let names: Vec<&str> = agents.iter().map(|a| a.agent_type.as_str()).collect();
        assert!(names.contains(&"general-purpose"));
        assert!(names.contains(&"Explore"));
        assert!(names.contains(&"Plan"));
        assert!(names.contains(&"statusline-setup"));
        assert!(names.contains(&"claude-code-guide"));
    }

    #[test]
    fn explore_agent_is_read_only_blocking_edit_write() {
        use super::built_in::built_in_agents;
        let explore = built_in_agents().into_iter()
            .find(|a| a.agent_type == "Explore")
            .expect("Explore in built-ins");
        assert!(explore.disallowed_tools.iter().any(|t| t == "Edit"));
        assert!(explore.disallowed_tools.iter().any(|t| t == "Write"));
        assert!(explore.disallowed_tools.iter().any(|t| t == "NotebookEdit"));
        assert!(explore.disallowed_tools.iter().any(|t| t == "Task"));
    }
}

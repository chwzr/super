pub mod built_in;
pub mod definition;
pub mod loader;
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
    fn loader_parses_full_frontmatter() {
        use super::loader::parse_agent_md;
        use super::definition::AgentSource;
        let src = "---\ndescription: An explorer\ntools: [Read, Grep]\ndisallowedTools: [Edit]\nmodel: haiku\npermissionMode: plan\nmaxTurns: 8\n---\nYou are an explorer.\nUse the tools.\n";
        let def = parse_agent_md("Explore.md", src, AgentSource::Project)
            .expect("parses");
        assert_eq!(def.agent_type, "Explore");
        assert_eq!(def.description, "An explorer");
        assert_eq!(def.tools.as_ref().unwrap(), &vec!["Read".to_string(), "Grep".to_string()]);
        assert_eq!(def.disallowed_tools, vec!["Edit".to_string()]);
        assert_eq!(def.model.as_deref(), Some("haiku"));
        assert_eq!(def.max_turns, Some(8));
        assert!(def.system_prompt.contains("You are an explorer"));
        assert!(matches!(def.source, AgentSource::Project));
    }

    #[test]
    fn loader_rejects_missing_frontmatter() {
        use super::loader::parse_agent_md;
        use super::definition::AgentSource;
        let src = "no frontmatter here\n";
        assert!(parse_agent_md("foo.md", src, AgentSource::User).is_err());
    }

    #[test]
    fn loader_rejects_empty_description() {
        use super::loader::parse_agent_md;
        use super::definition::AgentSource;
        let src = "---\ndescription: \"\"\n---\nbody\n";
        assert!(parse_agent_md("foo.md", src, AgentSource::User).is_err());
    }

    #[test]
    fn loader_strips_md_suffix_for_agent_type() {
        use super::loader::parse_agent_md;
        use super::definition::AgentSource;
        let src = "---\ndescription: x\n---\nbody";
        let def = parse_agent_md("my-agent.md", src, AgentSource::Project).unwrap();
        assert_eq!(def.agent_type, "my-agent");
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

use async_trait::async_trait;
use serde_json::json;
use super::contract::{DescriptionCtx, PromptCtx, ProgressSink, Tool, ToolCallContext, ToolResult, ToolResultBlock, ToolResultContent};

pub struct SkillTool {
    pub skills: Vec<crate::skills::loader::Skill>,
}

#[async_trait]
impl Tool for SkillTool {
    fn name(&self) -> &str { "Skill" }

    fn description(&self, _input: Option<&serde_json::Value>, _ctx: &DescriptionCtx) -> String {
        "Execute a skill within the main conversation. \
         Skills provide specialized capabilities and domain knowledge. \
         When users reference a slash command (/<name>), use this tool."
            .into()
    }

    fn prompt(&self, _ctx: &PromptCtx) -> String {
        include_str!("prompts/skill.txt").into()
    }

    fn input_schema(&self) -> serde_json::Value {
        json!({
            "type": "object",
            "properties": {
                "skill": {
                    "type": "string",
                    "description": "The skill name (e.g. \"commit\", \"brainstorming\", \"superpowers:writing-plans\")"
                },
                "args": {
                    "type": "string",
                    "description": "Optional arguments passed to the skill"
                }
            },
            "required": ["skill"]
        })
    }

    fn output_schema(&self) -> Option<serde_json::Value> {
        Some(json!({
            "oneOf": [
                {
                    "type": "object",
                    "properties": {
                        "inline": {"type": "boolean", "const": true},
                        "message": {"type": "string"}
                    },
                    "required": ["inline", "message"]
                },
                {
                    "type": "object",
                    "properties": {
                        "forked": {"type": "boolean", "const": true},
                        "message": {"type": "string"}
                    },
                    "required": ["forked", "message"]
                }
            ]
        }))
    }

    async fn call(&self, input: serde_json::Value, _context: &ToolCallContext, _on_progress: Option<ProgressSink>) -> ToolResult {
        let skill_name = input["skill"].as_str().unwrap_or("").trim().to_string();
        if skill_name.is_empty() {
            return ToolResult {
                content: "No skill name provided.".into(),
                is_error: true,
                inject_messages: Vec::new(),
                metadata: None,
                mcp_meta: None,
                new_messages: Vec::new(),
            };
        }
        let args = input["args"].as_str().unwrap_or("");

        // Normalize: strip leading slash if present.
        let normalized = skill_name.trim_start_matches('/');

        let skill = self.find_skill(normalized);
        match skill {
            None => {
                let available: Vec<&str> = self.skills.iter().map(|s| s.name.as_str()).collect();
                ToolResult {
                    content: format!(
                        "Skill '{}' not found. Available skills: {}",
                        normalized,
                        if available.is_empty() { "none loaded".into() } else { available.join(", ") }
                    ),
                    is_error: true,
                    inject_messages: Vec::new(),
                    metadata: None,
                    mcp_meta: None,
                    new_messages: Vec::new(),
                }
            }
            Some(s) => {
                // Prepend "Base directory for this skill:" header when available.
                let body = match &s.base_directory {
                    Some(dir) => format!(
                        "Base directory for this skill: {}\n\n{}",
                        dir.display(),
                        s.content
                    ),
                    None => s.content.clone(),
                };
                // Append args if provided.
                let inject_content = if args.is_empty() {
                    body
                } else {
                    format!("{}\n\n---\nArguments: {}", body, args)
                };
                ToolResult {
                    content: format!("Launching skill: {}", s.name),
                    is_error: false,
                    inject_messages: vec![inject_content],
                    metadata: None,
                    mcp_meta: None,
                    new_messages: Vec::new(),
                }
            }
        }
    }

    fn map_tool_result_to_block(
        &self,
        output: &serde_json::Value,
        tool_use_id: &str,
    ) -> ToolResultBlock {
        ToolResultBlock {
            tool_use_id: tool_use_id.into(),
            content: ToolResultContent::Text(
                output.as_str().map(String::from).unwrap_or_else(|| output.to_string()),
            ),
            is_error: false,
        }
    }
}

impl SkillTool {
    fn find_skill<'a>(&'a self, name: &str) -> Option<&'a crate::skills::loader::Skill> {
        let lower = name.to_lowercase();
        // Exact match first.
        if let Some(s) = self.skills.iter().find(|s| s.name.to_lowercase() == lower) {
            return Some(s);
        }
        // Suffix match for namespaced names (e.g. "superpowers:brainstorming" → "brainstorming").
        self.skills.iter().find(|s| {
            let sn = s.name.to_lowercase();
            sn.ends_with(&format!(":{}", lower)) || sn == lower
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::skills::loader::{LoadedFrom, Skill, SkillContext};
    use std::path::PathBuf;

    fn make_skill(name: &str, user_invocable: bool, base_dir: Option<PathBuf>) -> Skill {
        Skill {
            name: name.to_string(),
            description: format!("{name} description"),
            content: format!("# {name}\n\nContent of {name}."),
            base_directory: base_dir,
            user_invocable,
            when_to_use: None,
            allowed_tools: vec![],
            model: None,
            context: SkillContext::Inline,
            argument_hint: None,
            loaded_from: LoadedFrom::Bundled,
        }
    }

    fn make_tool(skills: Vec<Skill>) -> SkillTool {
        SkillTool { skills }
    }

    fn ctx() -> ToolCallContext {
        ToolCallContext {
            cwd: PathBuf::from("/tmp"),
            permission_mode: crate::state::store::PermissionMode::Default,
            abort_signal: None,
            parent_tool_use_id: None,
            bus: None,
            auto_deny_prompts: false,
            tool_use_id: "tu_test".into(),
            progress_sink: None,
        }
    }

    #[tokio::test]
    async fn returns_launching_skill_content() {
        let tool = make_tool(vec![make_skill("brainstorming", false, None)]);
        let result = tool.call(json!({"skill": "brainstorming"}), &ctx(), None).await;
        assert_eq!(result.content, "Launching skill: brainstorming");
        assert!(!result.is_error);
    }

    #[tokio::test]
    async fn injects_skill_body_without_base_dir() {
        let tool = make_tool(vec![make_skill("brainstorming", false, None)]);
        let result = tool.call(json!({"skill": "brainstorming"}), &ctx(), None).await;
        assert_eq!(result.inject_messages.len(), 1);
        assert!(result.inject_messages[0].contains("Content of brainstorming"));
        assert!(!result.inject_messages[0].contains("Base directory"));
    }

    #[tokio::test]
    async fn injects_base_dir_header_when_present() {
        let dir = PathBuf::from("/home/user/.super/plugins/superpowers/brainstorming");
        let tool = make_tool(vec![make_skill("brainstorming", false, Some(dir.clone()))]);
        let result = tool.call(json!({"skill": "brainstorming"}), &ctx(), None).await;
        assert!(result.inject_messages[0].starts_with("Base directory for this skill:"));
        assert!(result.inject_messages[0].contains(dir.to_str().unwrap()));
    }

    #[tokio::test]
    async fn strips_leading_slash_from_skill_name() {
        let tool = make_tool(vec![make_skill("commit", true, None)]);
        let result = tool.call(json!({"skill": "/commit"}), &ctx(), None).await;
        assert_eq!(result.content, "Launching skill: commit");
        assert!(!result.is_error);
    }

    #[tokio::test]
    async fn unknown_skill_returns_error() {
        let tool = make_tool(vec![make_skill("commit", true, None)]);
        let result = tool.call(json!({"skill": "nonexistent"}), &ctx(), None).await;
        assert!(result.is_error);
        assert!(result.inject_messages.is_empty());
    }

    #[tokio::test]
    async fn args_appended_to_inject_content() {
        let tool = make_tool(vec![make_skill("compact", true, None)]);
        let result = tool
            .call(json!({"skill": "compact", "args": "focus on recent changes"}), &ctx(), None)
            .await;
        assert!(result.inject_messages[0].contains("focus on recent changes"));
    }
}

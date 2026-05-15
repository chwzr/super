use async_trait::async_trait;
use serde_json::json;
use super::contract::{Tool, ToolCallContext, ToolResult};

pub struct SkillTool {
    pub skills: Vec<crate::skills::loader::Skill>,
}

#[async_trait]
impl Tool for SkillTool {
    fn name(&self) -> &str { "Skill" }
    fn description(&self) -> &str {
        "Execute a skill by name. Skills provide specialized capabilities and domain knowledge \
         for performing specific tasks."
    }
    fn input_schema(&self) -> serde_json::Value {
        json!({
            "type": "object",
            "properties": {
                "skill": {"type": "string", "description": "The name of the skill to execute"},
                "args": {"type": "string", "description": "Optional arguments for the skill"}
            },
            "required": ["skill"]
        })
    }

    async fn call(&self, input: serde_json::Value, _context: &ToolCallContext) -> ToolResult {
        let skill_name = input["skill"].as_str().unwrap_or("");
        let args = input["args"].as_str().unwrap_or("");

        if skill_name.is_empty() {
            return ToolResult { content: "No skill name provided.".into(), is_error: true, ..Default::default() };
        }

        // Find the skill by name
        let trimmed = skill_name.trim().to_lowercase();
        let skill = self.skills.iter().find(|s| {
            s.name.to_lowercase() == trimmed
                || format!("/{}", s.name.to_lowercase()) == trimmed
                || s.name.to_lowercase().ends_with(&format!(":{}", trimmed))
        });

        match skill {
            Some(s) => {
                let content = if args.is_empty() {
                    s.content.clone()
                } else {
                    format!("{}\n\n---\nArguments: {}", s.content, args)
                };
                ToolResult { content, is_error: false, ..Default::default() }
            }
            None => {
                let available: Vec<&str> = self.skills.iter().map(|s| s.name.as_str()).collect();
                ToolResult {
                    content: format!(
                        "Skill '{skill_name}' not found. Available skills: {}",
                        if available.is_empty() { "none loaded".into() } else { available.join(", ") }
                    ),
                    is_error: true,
                    ..Default::default()
                }
            }
        }
    }
}
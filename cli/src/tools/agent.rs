use std::sync::Arc;
use async_trait::async_trait;
use serde_json::json;
use uuid::Uuid;
use super::contract::{Tool, ToolCallContext, ToolResult};

pub struct AgentTool {
    pub store: Arc<crate::state::store::Store>,
    pub config: shared::CliConfig,
}

#[async_trait]
impl Tool for AgentTool {
    fn name(&self) -> &str { "Agent" }
    fn description(&self) -> &str {
        "Launches a sub-agent to handle complex multi-step tasks. \
         subagent_type: explore (read-only), plan (design), general-purpose (default). \
         Set run_in_background for async execution."
    }
    fn input_schema(&self) -> serde_json::Value {
        json!({
            "type": "object",
            "properties": {
                "description": {"type": "string"},
                "prompt": {"type": "string"},
                "subagent_type": {"type": "string", "enum": ["explore", "plan", "general-purpose"]},
                "run_in_background": {"type": "boolean"}
            },
            "required": ["description", "prompt"]
        })
    }

    async fn call(&self, input: serde_json::Value, _context: &ToolCallContext) -> ToolResult {
        let prompt = input["prompt"].as_str().unwrap_or("");
        let description = input["description"].as_str().unwrap_or("unnamed");
        let run_in_bg = input["run_in_background"].as_bool().unwrap_or(false);
        let task_id = Uuid::new_v4().to_string();

        if run_in_bg {
            let store = self.store.clone();
            let config = self.config.clone();
            let prompt = prompt.to_string();
            let desc = description.to_string();
            let task_id_clone = task_id.clone();

            // Clone for the set_state closure so `store` and `prompt` remain for the spawn
            let store_state = store.clone();
            let prompt_for_state = prompt.clone();
            let tid_for_state = task_id_clone.clone();
            let id_for_record = tid_for_state.clone();
            store_state.set_state(move |s| {
                s.tasks.insert(tid_for_state, crate::state::store::TaskRecord {
                    id: id_for_record,
                    subject: desc,
                    description: prompt_for_state,
                    status: crate::state::store::TaskStatus::InProgress,
                    blocks: vec![],
                    blocked_by: vec![],
                });
            });

            tokio::spawn(async move {
                let engine = crate::conversation::engine::ConversationEngine::new(store.clone(), config);
                let sp = crate::conversation::system_prompt::SystemPrompt::build(
                    &std::env::current_dir().unwrap_or_default()
                );
                let tid2 = task_id_clone.clone();
                match engine.process_prompt(prompt, &sp).await {
                    Ok(_) => {
                        store.set_state(move |s| {
                            if let Some(t) = s.tasks.get_mut(&tid2) {
                                t.status = crate::state::store::TaskStatus::Completed;
                            }
                        });
                    }
                    Err(_) => {
                        store.set_state(move |s| {
                            if let Some(t) = s.tasks.get_mut(&tid2) {
                                t.status = crate::state::store::TaskStatus::Failed;
                            }
                        });
                    }
                }
            });

            ToolResult {
                content: format!("Agent launched in background: {description}"),
                is_error: false,
                metadata: Some([
                    ("task_id".into(), task_id.clone()),
                    ("status".into(), "async_launched".into()),
                ].into()),
            }
        } else {
            let engine = crate::conversation::engine::ConversationEngine::new(self.store.clone(), self.config.clone());
            let sp = crate::conversation::system_prompt::SystemPrompt::build(
                &std::env::current_dir().unwrap_or_default()
            );
            match engine.process_prompt(prompt.to_string(), &sp).await {
                Ok(response) => ToolResult {
                    content: format!("Agent completed: {description}\n\n{response}"),
                    is_error: false,
                    metadata: Some([
                        ("status".into(), "completed".into()),
                    ].into()),
                },
                Err(e) => ToolResult {
                    content: format!("Agent failed: {e}"),
                    is_error: true,
                    metadata: None,
                },
            }
        }
    }
}
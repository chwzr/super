use std::sync::Arc;

use async_trait::async_trait;
use serde_json::json;
use tokio::sync::watch;
use uuid::Uuid;

use super::contract::{Tool, ToolCallContext, ToolResult};
use crate::agents::definition::AgentDefinition;
use crate::agents::model::resolve_model;
use crate::agents::permission::resolve_permission_mode;
use crate::agents::AgentRegistry;
use crate::conversation::engine::ConversationEngine;
use crate::conversation::session_bus::SessionBus;
use crate::conversation::system_prompt::SystemPrompt;
use crate::sdk::protocol::{BusMessage, SystemSubtype};
use crate::state::store::{AsyncAgentHandle, Store};
use crate::tools::ToolRegistry;

pub struct AgentTool {
    pub store: Arc<Store>,
    pub config: shared::CliConfig,
    pub registry: Arc<AgentRegistry>,
    pub tool_registry: Arc<ToolRegistry>,
}

#[async_trait]
impl Tool for AgentTool {
    fn name(&self) -> &str {
        "Task"
    }

    fn description(&self) -> &str {
        "Launches a sub-agent to handle a focused task. \
         subagent_type selects the agent definition. \
         Set run_in_background to spawn an async agent."
    }

    fn input_schema(&self) -> serde_json::Value {
        json!({
            "type": "object",
            "properties": {
                "description":   { "type": "string", "description": "A short (3-5 word) description of the task" },
                "prompt":        { "type": "string", "description": "The task for the agent to perform" },
                "subagent_type": { "type": "string", "description": "The agent type to use (e.g. Explore, Plan, general-purpose)" },
                "model":         { "type": "string", "enum": ["sonnet", "opus", "haiku", "inherit"] },
                "run_in_background": { "type": "boolean" }
            },
            "required": ["description", "prompt", "subagent_type"]
        })
    }

    async fn call(&self, input: serde_json::Value, ctx: &ToolCallContext) -> ToolResult {
        // 1. Validate input
        let description = input.get("description").and_then(|v| v.as_str()).unwrap_or("");
        let prompt = match input.get("prompt").and_then(|v| v.as_str()) {
            Some(p) if !p.trim().is_empty() => p.to_string(),
            _ => return err("missing required field: prompt"),
        };
        let subagent_type = match input.get("subagent_type").and_then(|v| v.as_str()) {
            Some(s) if !s.trim().is_empty() => s.to_string(),
            _ => return err("missing required field: subagent_type"),
        };
        let model_override = input.get("model").and_then(|v| v.as_str()).map(|s| s.to_string());
        let is_async = input.get("run_in_background").and_then(|v| v.as_bool()).unwrap_or(false);

        // 2. Resolve agent
        let Some(agent_def) = self.registry.resolve(&subagent_type) else {
            let available: Vec<String> = self.registry.list().into_iter()
                .map(|d| d.agent_type.clone())
                .collect();
            return err(&format!(
                "unknown subagent: {subagent_type} (available: {})",
                available.join(", ")
            ));
        };
        let agent_def: AgentDefinition = agent_def.clone();

        // 3. Bus must be present
        let Some(bus) = ctx.bus.clone() else {
            return err("Task tool requires bus in ToolCallContext");
        };

        // The child's events must be stamped with the tool_use_id of THIS
        // Task invocation — not the chain id (`ctx.parent_tool_use_id`).
        // The tool_loop populates ctx.tool_use_id with the invoking block's id.
        let parent_tool_use_id = ctx.tool_use_id.clone();

        // 4. Build child execution config
        let agent_id = Uuid::new_v4().to_string();
        let child_perm = resolve_permission_mode(
            &ctx.permission_mode,
            agent_def.permission_mode.as_ref(),
            is_async,
        );
        let child_model = resolve_model(
            model_override.as_deref().or(agent_def.model.as_deref()),
            &self.config.model,
        );
        let mut child_config = self.config.clone();
        child_config.model = child_model;

        let child_registry = Arc::new(self.tool_registry.filter_for_agent(&agent_def));

        // 5a. Sync path
        if !is_async {
            let child = ConversationEngine::new_child(
                self.store.clone(),
                child_config,
                child_registry,
                bus.clone(),
                agent_id.clone(),
                ctx.abort_signal.clone(),
                Some(child_perm.clone()),
            );
            let sys = build_child_system_prompt(&agent_def);

            return match child.process_prompt(prompt, &sys, Some(parent_tool_use_id)).await {
                Ok(final_text) => ToolResult {
                    content: final_text,
                    is_error: false,
                    metadata: Some({
                        let mut m = std::collections::HashMap::new();
                        m.insert("agent_id".to_string(), agent_id);
                        m.insert("agent_type".to_string(), agent_def.agent_type.clone());
                        m
                    }),
                },
                Err(e) => err(&format!("Agent failed: {e}")),
            };
        }

        // 5b. Async path
        let (abort_tx, abort_rx) = watch::channel(false);
        let handle = AsyncAgentHandle {
            agent_id: agent_id.clone(),
            parent_tool_use_id: parent_tool_use_id.clone(),
            abort: abort_tx,
            description: description.to_string(),
            started_at: std::time::Instant::now(),
        };
        self.store.register_async_agent(handle);

        let child = ConversationEngine::new_child(
            self.store.clone(),
            child_config,
            child_registry,
            bus.clone(),
            agent_id.clone(),
            Some(abort_rx),
            Some(child_perm),
        );
        let sys = build_child_system_prompt(&agent_def);
        let store_for_task = self.store.clone();
        let bus_for_task = bus.clone();
        let agent_id_for_task = agent_id.clone();
        let parent_tu_for_task = parent_tool_use_id.clone();

        tokio::spawn(async move {
            let result = child.process_prompt(prompt, &sys, Some(parent_tu_for_task.clone())).await;
            let text = match result {
                Ok(t) => t,
                Err(e) => format!("error: {e}"),
            };
            bus_for_task.emit(BusMessage::SystemEvent {
                subtype: SystemSubtype::AsyncAgentDone,
                message: format!("{agent_id_for_task} finished: {text}"),
                parent_tool_use_id: Some(parent_tu_for_task),
                uuid: Uuid::new_v4(),
                session_id: agent_id_for_task.clone(),
            });
            store_for_task.complete_async_agent(&agent_id_for_task);
        });

        ToolResult {
            content: format!(
                "Agent {agent_id} running in background. Completion will be reported via AsyncAgentDone."
            ),
            is_error: false,
            metadata: Some({
                let mut m = std::collections::HashMap::new();
                m.insert("agent_id".into(), agent_id);
                m.insert("agent_type".into(), agent_def.agent_type.clone());
                m.insert("async".into(), "true".into());
                m
            }),
        }
    }
}

fn err(msg: &str) -> ToolResult {
    ToolResult { content: msg.to_string(), is_error: true, metadata: None }
}

fn build_child_system_prompt(agent: &AgentDefinition) -> SystemPrompt {
    SystemPrompt { sections: vec![agent.system_prompt.clone()] }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::state::store::PermissionMode;

    #[tokio::test]
    async fn agent_tool_errors_on_unknown_subagent_type() {
        let store = Arc::new(Store::new());
        let cfg = shared::CliConfig::default();
        let agent_reg = Arc::new(AgentRegistry::built_in_only());
        let tool_reg = ToolRegistry::new(store.clone(), cfg.clone(), agent_reg.clone());

        let tool = AgentTool {
            store: store.clone(),
            config: cfg.clone(),
            registry: agent_reg,
            tool_registry: tool_reg.clone(),
        };

        let bus = Arc::new(SessionBus::new("s-root".into()));
        let ctx = ToolCallContext {
            cwd: std::env::current_dir().unwrap(),
            permission_mode: PermissionMode::Default,
            abort_signal: None,
            parent_tool_use_id: None,
            bus: Some(bus),
            auto_deny_prompts: false,
            tool_use_id: String::new(),
        };
        let input = serde_json::json!({
            "description": "do thing",
            "prompt": "thing prompt",
            "subagent_type": "does-not-exist",
        });
        let result = tool.call(input, &ctx).await;
        assert!(result.is_error);
        assert!(result.content.contains("unknown subagent"), "got: {}", result.content);
    }

    #[tokio::test]
    async fn agent_tool_name_is_task() {
        let store = Arc::new(Store::new());
        let cfg = shared::CliConfig::default();
        let agent_reg = Arc::new(AgentRegistry::built_in_only());
        let tool_reg = ToolRegistry::new(store.clone(), cfg.clone(), agent_reg.clone());
        let tool = AgentTool {
            store, config: cfg, registry: agent_reg, tool_registry: tool_reg,
        };
        assert_eq!(tool.name(), "Task");
    }
}

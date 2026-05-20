use std::sync::Arc;

use async_trait::async_trait;
use serde_json::json;
use tokio::sync::watch;
use uuid::Uuid;

use super::contract::{
    DescriptionCtx, ProgressSink, PromptCtx, Tool, ToolCallContext, ToolResult, ToolResultBlock,
    ToolResultContent,
};
use crate::agents::definition::AgentDefinition;
use crate::agents::model::resolve_model;
use crate::agents::permission::resolve_permission_mode;
use crate::agents::AgentRegistry;
use crate::conversation::engine::ConversationEngine;
use crate::conversation::message_queue::{MessageQueue, PromptInputMode, QueuePriority};
use crate::conversation::system_prompt::SystemPrompt;
use crate::sdk::protocol::{BusMessage, SystemSubtype};
use crate::state::store::{AsyncAgentHandle, Store};
use crate::tools::ToolRegistry;

pub struct AgentTool {
    pub store: Arc<Store>,
    pub config: shared::CliConfig,
    pub registry: Arc<AgentRegistry>,
    pub tool_registry: Arc<ToolRegistry>,
    pub queue: Arc<MessageQueue>,
}

#[async_trait]
impl Tool for AgentTool {
    fn name(&self) -> &str {
        "Task"
    }

    fn description(&self, _input: Option<&serde_json::Value>, _ctx: &DescriptionCtx) -> String {
        "Launches a sub-agent to handle a focused task. \
         subagent_type selects the agent definition. \
         Set run_in_background to spawn an async agent."
            .into()
    }

    fn prompt(&self, _ctx: &PromptCtx) -> String {
        include_str!("prompts/agent.txt").into()
    }

    fn search_hint(&self) -> Option<&'static str> {
        Some("launch a new agent to handle complex, multi-step tasks autonomously")
    }

    fn output_schema(&self) -> Option<serde_json::Value> {
        Some(json!({
            "oneOf": [
                {
                    "type": "object",
                    "properties": {
                        "status": {"const": "completed"},
                        "prompt": {"type": "string"}
                    },
                    "required": ["status", "prompt"]
                },
                {
                    "type": "object",
                    "properties": {
                        "status": {"const": "async_launched"},
                        "agentId": {"type": "string"},
                        "prompt": {"type": "string"}
                    },
                    "required": ["status", "agentId", "prompt"]
                }
            ]
        }))
    }

    fn input_schema(&self) -> serde_json::Value {
        let agents = self.registry.list();
        let agent_types: Vec<String> = agents.iter().map(|d| d.agent_type.clone()).collect();
        let agent_descriptions: String = agents
            .iter()
            .map(|d| format!("- {}: {}", d.agent_type, d.description))
            .collect::<Vec<_>>()
            .join("\n");
        let subagent_desc =
            format!("The agent type to use. Available agents:\n{agent_descriptions}");

        json!({
            "type": "object",
            "properties": {
                "description":   { "type": "string", "description": "A short (3-5 word) description of the task" },
                "prompt":        { "type": "string", "description": "The task for the agent to perform" },
                "subagent_type": {
                    "type": "string",
                    "enum": agent_types,
                    "description": subagent_desc,
                },
                "model":         { "type": "string", "enum": ["sonnet", "opus", "haiku", "inherit"] },
                "run_in_background": { "type": "boolean" }
            },
            "required": ["description", "prompt", "subagent_type"]
        })
    }

    async fn call(
        &self,
        input: serde_json::Value,
        ctx: &ToolCallContext,
        _on_progress: Option<ProgressSink>,
    ) -> ToolResult {
        // 1. Validate input
        let description = input
            .get("description")
            .and_then(|v| v.as_str())
            .unwrap_or("");
        let prompt = match input.get("prompt").and_then(|v| v.as_str()) {
            Some(p) if !p.trim().is_empty() => p.to_string(),
            _ => return err("missing required field: prompt"),
        };
        let subagent_type = match input.get("subagent_type").and_then(|v| v.as_str()) {
            Some(s) if !s.trim().is_empty() => s.to_string(),
            _ => return err("missing required field: subagent_type"),
        };
        let model_override = input
            .get("model")
            .and_then(|v| v.as_str())
            .map(|s| s.to_string());
        let is_async = input
            .get("run_in_background")
            .and_then(|v| v.as_bool())
            .unwrap_or(false);

        // 2. Resolve agent
        let Some(agent_def) = self.registry.resolve(&subagent_type) else {
            let available: Vec<String> = self
                .registry
                .list()
                .into_iter()
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
            &self.config.provider,
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
                Some(child_perm),
                false,
                self.queue.clone(),
            );
            let sys = build_child_system_prompt(&agent_def);

            return match child
                .process_prompt(prompt, &sys, Some(parent_tool_use_id))
                .await
            {
                Ok(final_text) => ToolResult {
                    content: final_text,
                    is_error: false,
                    metadata: Some({
                        let mut m = std::collections::HashMap::new();
                        m.insert("agent_id".to_string(), agent_id);
                        m.insert("agent_type".to_string(), agent_def.agent_type.clone());
                        m
                    }),
                    inject_messages: Vec::new(),
                    mcp_meta: None,
                    new_messages: Vec::new(),
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
            true,
            self.queue.clone(),
        );
        let sys = build_child_system_prompt(&agent_def);
        let store_for_task = self.store.clone();
        let bus_for_task = bus.clone();
        let agent_id_for_task = agent_id.clone();
        let parent_tu_for_task = parent_tool_use_id.clone();
        let queue_for_agent = self.queue.clone();
        let description_for_task = description.to_string();

        tokio::spawn(async move {
            let result = child
                .process_prompt(prompt, &sys, Some(parent_tu_for_task.clone()))
                .await;
            let is_ok = result.is_ok();
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
            // Enqueue notification for the model
            let status = if is_ok { "completed" } else { "failed" };
            let summary = format!(
                "Agent \"{description_for_task}\" {}",
                if is_ok { "completed" } else { "failed" }
            );
            let notification = format!(
                "<task-notification>\n  <task-id>{}</task-id>\n  <status>{}</status>\n  <summary>{}</summary>\n</task-notification>",
                agent_id_for_task, status, summary
            );
            queue_for_agent.enqueue_pending_notification(
                notification,
                PromptInputMode::TaskNotification,
                QueuePriority::Later,
                None, // agent notification goes to main thread
            );
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
            inject_messages: Vec::new(),
            mcp_meta: None,
            new_messages: Vec::new(),
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
                output
                    .as_str()
                    .map(String::from)
                    .unwrap_or_else(|| output.to_string()),
            ),
            is_error: false,
        }
    }
}

fn err(msg: &str) -> ToolResult {
    ToolResult {
        content: msg.to_string(),
        is_error: true,
        inject_messages: Vec::new(),
        metadata: None,
        mcp_meta: None,
        new_messages: Vec::new(),
    }
}

fn build_child_system_prompt(agent: &AgentDefinition) -> SystemPrompt {
    SystemPrompt {
        sections: vec![agent.system_prompt.clone()],
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::conversation::session_bus::SessionBus;
    use crate::state::store::PermissionMode;

    fn make_test_deps() -> (
        Arc<MessageQueue>,
        Arc<
            std::sync::Mutex<
                std::collections::HashMap<String, crate::conversation::cron_runtime::CronJob>,
            >,
        >,
        watch::Sender<bool>,
    ) {
        let queue = Arc::new(MessageQueue::new());
        let jobs = Arc::new(std::sync::Mutex::new(std::collections::HashMap::new()));
        let (tx, _rx) = watch::channel(false);
        (queue, jobs, tx)
    }

    fn make_test_registry(store: Arc<Store>, agent_reg: Arc<AgentRegistry>) -> Arc<ToolRegistry> {
        let (queue, jobs, wake_tx) = make_test_deps();
        ToolRegistry::new(
            store,
            shared::CliConfig::default(),
            agent_reg,
            queue,
            jobs,
            wake_tx,
        )
    }

    #[tokio::test]
    async fn agent_tool_errors_on_unknown_subagent_type() {
        let store = Arc::new(Store::new());
        let cfg = shared::CliConfig::default();
        let agent_reg = Arc::new(AgentRegistry::built_in_only());
        let tool_reg = make_test_registry(store.clone(), agent_reg.clone());

        let tool = AgentTool {
            store: store.clone(),
            config: cfg.clone(),
            registry: agent_reg,
            tool_registry: tool_reg.clone(),
            queue: Arc::new(MessageQueue::new()),
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
            progress_sink: None,
            queue: None,
        };
        let input = serde_json::json!({
            "description": "do thing",
            "prompt": "thing prompt",
            "subagent_type": "does-not-exist",
        });
        let result = tool.call(input, &ctx, None).await;
        assert!(result.is_error);
        assert!(
            result.content.contains("unknown subagent"),
            "got: {}",
            result.content
        );
    }

    #[tokio::test]
    async fn agent_tool_name_is_task() {
        let store = Arc::new(Store::new());
        let cfg = shared::CliConfig::default();
        let agent_reg = Arc::new(AgentRegistry::built_in_only());
        let tool_reg = make_test_registry(store.clone(), agent_reg.clone());
        let tool = AgentTool {
            store,
            config: cfg,
            registry: agent_reg,
            tool_registry: tool_reg,
            queue: Arc::new(MessageQueue::new()),
        };
        assert_eq!(tool.name(), "Task");
    }

    #[test]
    fn input_schema_lists_registered_agents() {
        let store = Arc::new(Store::new());
        let cfg = shared::CliConfig::default();
        let agent_reg = Arc::new(AgentRegistry::built_in_only());
        let tool_reg = make_test_registry(store.clone(), agent_reg.clone());
        let tool = AgentTool {
            store,
            config: cfg,
            registry: agent_reg,
            tool_registry: tool_reg,
            queue: Arc::new(MessageQueue::new()),
        };
        let schema = tool.input_schema();
        let st = &schema["properties"]["subagent_type"];
        let enum_vals: Vec<&str> = st["enum"]
            .as_array()
            .unwrap()
            .iter()
            .map(|v| v.as_str().unwrap())
            .collect();
        assert!(
            enum_vals.contains(&"general-purpose"),
            "missing general-purpose: {enum_vals:?}"
        );
        assert!(
            enum_vals.contains(&"Explore"),
            "missing Explore: {enum_vals:?}"
        );
        assert!(enum_vals.contains(&"Plan"), "missing Plan: {enum_vals:?}");
        let desc = st["description"].as_str().unwrap();
        assert!(
            desc.contains("Explore"),
            "description missing Explore: {desc}"
        );
    }
}

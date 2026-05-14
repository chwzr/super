pub mod agent;
pub mod ask_user_question;
pub mod bash;
pub mod config_tool;
pub mod contract;
pub mod cron_create;
pub mod cron_delete;
pub mod cron_list;
pub mod edit;
pub mod enter_plan_mode;
pub mod enter_worktree;
pub mod exit_plan_mode;
pub mod exit_worktree;
pub mod glob_tool;
pub mod grep;
pub mod lsp;
pub mod monitor;
pub mod notebook_edit;
pub mod permission;
pub mod read;
pub mod send_message;
pub mod skill;
pub mod sleep;
pub mod structured_output;
pub mod task_create;
pub mod task_get;
pub mod task_list;
pub mod task_output;
pub mod task_stop;
pub mod task_update;
pub mod todo_write;
pub mod tool_search;
pub mod web_fetch;
pub mod web_search;
pub mod write;

use std::sync::{Arc, RwLock};
use contract::{Tool, ToolCallContext, ToolResult};
use crate::state::store::{PermissionMode, Store};
use agent::AgentTool;
use ask_user_question::AskUserQuestionTool;
use bash::BashTool;
use config_tool::ConfigTool;
use cron_create::CronCreateTool;
use cron_delete::CronDeleteTool;
use cron_list::CronListTool;
use edit::EditTool;
use enter_plan_mode::EnterPlanModeTool;
use enter_worktree::EnterWorktreeTool;
use exit_plan_mode::ExitPlanModeTool;
use exit_worktree::ExitWorktreeTool;
use glob_tool::GlobTool;
use grep::GrepTool;
use lsp::LspTool;
use monitor::MonitorTool;
use notebook_edit::NotebookEditTool;
use read::ReadTool;
use send_message::SendMessageTool;
use skill::SkillTool;
use sleep::SleepTool;
use structured_output::StructuredOutputTool;
use task_create::TaskCreateTool;
use task_get::TaskGetTool;
use task_list::TaskListTool;
use task_output::TaskOutputTool;
use task_stop::TaskStopTool;
use task_update::TaskUpdateTool;
use todo_write::TodoWriteTool;
use tool_search::ToolSearchTool;
use web_fetch::WebFetchTool;
use web_search::WebSearchTool;
use write::WriteTool;

pub struct ToolRegistry {
    tools: Arc<RwLock<Vec<Arc<dyn Tool>>>>,
}

impl ToolRegistry {
    pub fn new(store: Arc<Store>, config: shared::CliConfig) -> Self {
        let mut tools: Vec<Arc<dyn Tool>> = Vec::new();

        // Standard tools
        tools.push(Arc::new(ReadTool::default()));
        tools.push(Arc::new(EditTool::default()));
        tools.push(Arc::new(WriteTool::default()));
        tools.push(Arc::new(GlobTool::default()));
        tools.push(Arc::new(GrepTool::default()));
        tools.push(Arc::new(NotebookEditTool::default()));
        tools.push(Arc::new(BashTool));
        tools.push(Arc::new(ConfigTool::default()));
        tools.push(Arc::new(WebFetchTool));
        tools.push(Arc::new(WebSearchTool));

        // LSP
        tools.push(Arc::new(LspTool));

        // Cron tools (share the same job registry)
        let cron_jobs = Arc::new(std::sync::Mutex::new(std::collections::HashMap::new()));
        tools.push(Arc::new(CronCreateTool { jobs: cron_jobs.clone() }));
        tools.push(Arc::new(CronDeleteTool { jobs: cron_jobs.clone() }));
        tools.push(Arc::new(CronListTool { jobs: cron_jobs.clone() }));

        // Sleep
        tools.push(Arc::new(SleepTool));

        // Monitor (stub)
        tools.push(Arc::new(MonitorTool));

        // Skill tool (empty skills vec for now)
        tools.push(Arc::new(SkillTool { skills: Vec::new() }));

        // ToolSearch
        tools.push(Arc::new(ToolSearchTool));

        // StructuredOutput
        tools.push(Arc::new(StructuredOutputTool));

        // Worktree tools (stubs)
        tools.push(Arc::new(EnterWorktreeTool));
        tools.push(Arc::new(ExitWorktreeTool));

        // Agent and task management tools
        tools.push(Arc::new(AgentTool {
            store: store.clone(),
            config: config.clone(),
        }));
        tools.push(Arc::new(TaskCreateTool {
            store: store.clone(),
        }));
        tools.push(Arc::new(TaskGetTool {
            store: store.clone(),
        }));
        tools.push(Arc::new(TaskListTool {
            store: store.clone(),
        }));
        tools.push(Arc::new(TaskOutputTool {
            store: store.clone(),
        }));
        tools.push(Arc::new(TaskStopTool {
            store: store.clone(),
        }));
        tools.push(Arc::new(TaskUpdateTool {
            store: store.clone(),
        }));
        tools.push(Arc::new(TodoWriteTool));
        tools.push(Arc::new(EnterPlanModeTool {
            store: store.clone(),
        }));
        tools.push(Arc::new(ExitPlanModeTool {
            store: store.clone(),
        }));
        tools.push(Arc::new(SendMessageTool));
        tools.push(Arc::new(AskUserQuestionTool));

        Self { tools: Arc::new(RwLock::new(tools)) }
    }

    pub fn register(&self, tool: Arc<dyn Tool>) {
        self.tools.write().unwrap().push(tool);
    }

    pub fn get(&self, name: &str) -> Option<Arc<dyn Tool>> {
        self.tools.read().unwrap().iter().find(|t| t.name() == name).cloned()
    }

    pub fn list(&self) -> Vec<String> {
        self.tools.read().unwrap().iter().map(|t| t.name().to_string()).collect()
    }

    pub fn assemble_for_mode(&self, mode: &PermissionMode) -> Vec<Arc<dyn Tool>> {
        let tools = self.tools.read().unwrap();
        let mut pool: Vec<Arc<dyn Tool>> = tools
            .iter()
            .filter(|t| match mode {
                PermissionMode::Plan => t.is_read_only(),
                _ => true,
            })
            .cloned()
            .collect();
        pool.sort_by(|a, b| a.name().cmp(b.name()));
        pool
    }

    pub fn tool_descriptions(&self, mode: &PermissionMode) -> String {
        let pool = self.assemble_for_mode(mode);
        pool.iter()
            .map(|t| format!("- **{}**: {}", t.name(), t.description()))
            .collect::<Vec<_>>()
            .join("\n")
    }

    pub async fn execute(
        &self,
        name: &str,
        input: serde_json::Value,
        context: &ToolCallContext,
    ) -> Result<ToolResult, String> {
        let tool = self
            .get(name)
            .ok_or_else(|| format!("unknown tool: {name}"))?;
        Ok(tool.call(input, context).await)
    }
}

impl Clone for ToolRegistry {
    fn clone(&self) -> Self {
        Self {
            tools: self.tools.clone(),
        }
    }
}

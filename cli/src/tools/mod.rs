pub mod config_tool;
pub mod contract;
pub mod edit;
pub mod glob_tool;
pub mod grep;
pub mod notebook_edit;
pub mod permission;
pub mod read;
pub mod write;

use std::sync::Arc;
use contract::{Tool, ToolCallContext, ToolResult};
use crate::state::store::PermissionMode;
use read::ReadTool;
use edit::EditTool;
use write::WriteTool;
use glob_tool::GlobTool;
use grep::GrepTool;
use notebook_edit::NotebookEditTool;
use config_tool::ConfigTool;

pub struct ToolRegistry {
    tools: Vec<Arc<dyn Tool>>,
}

impl ToolRegistry {
    pub fn new() -> Self {
        let mut tools: Vec<Arc<dyn Tool>> = Vec::new();
        tools.push(Arc::new(ReadTool::default()));
        tools.push(Arc::new(EditTool::default()));
        tools.push(Arc::new(WriteTool::default()));
        tools.push(Arc::new(GlobTool::default()));
        tools.push(Arc::new(GrepTool::default()));
        tools.push(Arc::new(NotebookEditTool::default()));
        tools.push(Arc::new(ConfigTool::default()));
        Self { tools }
    }

    pub fn register(&mut self, tool: Arc<dyn Tool>) {
        self.tools.push(tool);
    }

    pub fn get(&self, name: &str) -> Option<Arc<dyn Tool>> {
        self.tools.iter().find(|t| t.name() == name).cloned()
    }

    pub fn list(&self) -> Vec<String> {
        self.tools.iter().map(|t| t.name().to_string()).collect()
    }

    pub fn assemble_for_mode(&self, mode: &PermissionMode) -> Vec<Arc<dyn Tool>> {
        let mut pool: Vec<Arc<dyn Tool>> = self
            .tools
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

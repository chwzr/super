use serde_json::Value;
use std::collections::HashMap;
use tokio::sync::RwLock;

/// Manages connections to MCP servers
pub struct McpManager {
    servers: RwLock<HashMap<String, McpServerState>>,
}

#[derive(Clone)]
struct McpServerState {
    config: McpServerConfig,
    connected: bool,
    tools: Vec<McpToolDef>,
}

#[derive(Clone, Debug)]
pub struct McpServerConfig {
    pub command: String,
    pub args: Vec<String>,
    pub env: HashMap<String, String>,
}

#[derive(Clone, Debug)]
pub struct McpToolDef {
    pub name: String,
    pub description: String,
    pub input_schema: Value,
}

impl Default for McpManager {
    fn default() -> Self {
        Self::new()
    }
}

impl McpManager {
    pub fn new() -> Self {
        Self {
            servers: RwLock::new(HashMap::new()),
        }
    }

    /// Register an MCP server from its config
    pub async fn register_server(&self, name: &str, config: McpServerConfig) {
        self.servers.write().await.insert(
            name.to_string(),
            McpServerState {
                config,
                connected: false,
                tools: Vec::new(),
            },
        );
    }

    /// Connect to a registered server and discover its tools
    pub async fn connect(&self, name: &str) -> Result<(), String> {
        let state = {
            let servers = self.servers.read().await;
            servers
                .get(name)
                .cloned()
                .ok_or_else(|| format!("server not found: {name}"))?
        };

        // Start the server process via stdio transport
        let tools = crate::mcp::transport::connect_and_list_tools(
            &state.config.command,
            &state.config.args,
        )
        .await?;

        // Store discovered tools
        if let Some(s) = self.servers.write().await.get_mut(name) {
            s.connected = true;
            s.tools = tools;
        }

        Ok(())
    }

    /// Get all tools from all connected MCP servers
    pub async fn get_all_tools(&self) -> Vec<McpToolDef> {
        let servers = self.servers.read().await;
        servers
            .values()
            .filter(|s| s.connected)
            .flat_map(|s| s.tools.clone())
            .collect()
    }

    /// Call an MCP tool by its full name `mcp__<server>__<tool>`
    pub async fn call_tool(&self, full_name: &str, input: Value) -> Result<String, String> {
        let parts: Vec<&str> = full_name.splitn(3, "__").collect();
        if parts.len() != 3 {
            return Err(format!("invalid MCP tool name: {full_name}"));
        }
        let server_name = parts[1];
        let tool_name = parts[2];

        let config = {
            let servers = self.servers.read().await;
            servers
                .get(server_name)
                .map(|s| s.config.clone())
                .ok_or_else(|| format!("MCP server not found: {server_name}"))?
        };

        crate::mcp::transport::call_tool(&config.command, &config.args, tool_name, input).await
    }
}

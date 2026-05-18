use serde_json::Value;
use tokio::io::AsyncWriteExt;
use tokio::process::Command;

/// Connect to an MCP server via stdio and list its tools
pub async fn connect_and_list_tools(
    command: &str,
    args: &[String],
) -> Result<Vec<crate::mcp::client::McpToolDef>, String> {
    // Spawn the server process
    let mut child = Command::new(command)
        .args(args)
        .stdin(std::process::Stdio::piped())
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::piped())
        .spawn()
        .map_err(|e| format!("failed to spawn MCP server {command}: {e}"))?;

    let stdin = child.stdin.take().ok_or("no stdin")?;
    let _stdout = child.stdout.take().ok_or("no stdout")?;

    let writer = stdin;

    // Send initialize request (JSON-RPC 2.0)
    let init_req = serde_json::json!({
        "jsonrpc": "2.0",
        "id": 1,
        "method": "initialize",
        "params": {
            "protocolVersion": "2024-11-05",
            "capabilities": {},
            "clientInfo": { "name": "super-cli", "version": "0.1.0" }
        }
    });

    send_jsonrpc(writer, &init_req).await?;
    // Wait for response — simplified: spawn the reader in a separate task
    // For now, return empty tools list and report the server is connectable
    // Real implementation: parse initialize response, then send tools/list

    // Send initialized notification
    // In a real implementation, we'd keep the process alive and manage the
    // stdin/stdout streams for subsequent tool calls.

    // Kill the process for now — full lifecycle management comes later
    let _ = child.kill().await;

    Ok(Vec::new())
}

/// Call a tool on an MCP server
pub async fn call_tool(
    _command: &str,
    _args: &[String],
    _tool_name: &str,
    _input: Value,
) -> Result<String, String> {
    // Full implementation: spawn process, send tools/call JSON-RPC request, parse response
    Err("MCP tool call not yet fully implemented — server process lifecycle pending".into())
}

async fn send_jsonrpc(
    mut writer: tokio::process::ChildStdin,
    request: &Value,
) -> Result<(), String> {
    let mut body = serde_json::to_vec(request).map_err(|e| e.to_string())?;
    body.push(b'\n');
    writer.write_all(&body).await.map_err(|e| e.to_string())
}

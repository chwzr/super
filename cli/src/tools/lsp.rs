use std::collections::HashMap;

use crate::tools::contract::{
    DescriptionCtx, ProgressSink, PromptCtx, Tool, ToolCallContext, ToolResult, ToolResultBlock,
    ToolResultContent, ValidationResult,
};
use crate::tools::permission::{DecisionReason, PermissionResult};
use async_trait::async_trait;
use serde_json::json;

pub struct LspTool;

#[async_trait]
impl Tool for LspTool {
    // ── Identity ────────────────────────────────────────────────────────
    fn name(&self) -> &str {
        "LSP"
    }

    // ── Discovery / loading ─────────────────────────────────────────────
    fn description(&self, _input: Option<&serde_json::Value>, _ctx: &DescriptionCtx) -> String {
        "Interact with Language Server Protocol (LSP) servers to get code intelligence features."
            .into()
    }

    fn prompt(&self, _ctx: &PromptCtx) -> String {
        include_str!("prompts/lsp.txt").into()
    }

    fn search_hint(&self) -> Option<&'static str> {
        Some("code intelligence (definitions, references, symbols, hover)")
    }

    fn should_defer(&self) -> bool {
        true
    }

    fn is_enabled(&self) -> bool {
        crate::lsp::is_lsp_connected()
    }

    // ── Schemas ─────────────────────────────────────────────────────────
    fn input_schema(&self) -> serde_json::Value {
        json!({
            "type": "object",
            "properties": {
                "operation": {"type": "string", "enum": [
                    "goToDefinition", "findReferences", "hover",
                    "documentSymbol", "workspaceSymbol", "goToImplementation",
                    "prepareCallHierarchy", "incomingCalls", "outgoingCalls"
                ]},
                "filePath": {"type": "string"},
                "line": {"type": "integer"},
                "character": {"type": "integer"}
            },
            "required": ["operation", "filePath", "line", "character"]
        })
    }

    fn output_schema(&self) -> Option<serde_json::Value> {
        Some(json!({
            "type": "object",
            "properties": {
                "operation": {"type": "string", "description": "The LSP operation that was performed"},
                "filePath": {"type": "string", "description": "The file the operation was performed on"},
                "result": {"type": "string", "description": "Formatted human-readable result of the LSP operation"},
                "resultCount": {"type": "integer", "description": "Number of result items found, or 0 if not applicable"},
                "fileCount": {"type": "integer", "description": "Number of distinct files involved in the result, or 0 if not applicable"}
            }
        }))
    }

    fn is_read_only(&self, _input: &serde_json::Value) -> bool {
        true
    }

    fn is_lsp(&self) -> bool {
        true
    }

    fn get_path(&self, input: &serde_json::Value) -> Option<std::path::PathBuf> {
        input
            .get("filePath")
            .and_then(|v| v.as_str())
            .map(|p| expand_path(p))
    }

    // ── Validation + permissions ────────────────────────────────────────
    async fn validate_input(
        &self,
        input: &serde_json::Value,
        _ctx: &ToolCallContext,
    ) -> ValidationResult {
        let file_path = match input.get("filePath").and_then(|v| v.as_str()) {
            Some(p) => expand_path(p),
            None => {
                return ValidationResult::Err {
                    message: "Missing required parameter: filePath".into(),
                    error_code: 1,
                };
            }
        };

        match std::fs::metadata(&file_path) {
            Ok(meta) => {
                if !meta.is_file() {
                    return ValidationResult::Err {
                        message: format!(
                            "Not a regular file: {}",
                            file_path.display()
                        ),
                        error_code: 2,
                    };
                }
                ValidationResult::Ok
            }
            Err(_) => ValidationResult::Err {
                message: format!("File not found: {}", file_path.display()),
                error_code: 1,
            },
        }
    }

    async fn check_permissions(
        &self,
        _input: &serde_json::Value,
        _ctx: &ToolCallContext,
    ) -> PermissionResult {
        // LSP is read-only. Deny in plan mode if the tool would be denied.
        // Otherwise, allow by default.
        PermissionResult::Allow {
            updated_input: None,
            decision_reason: Some(DecisionReason::ToolDefault),
        }
    }

    // ── Execution ───────────────────────────────────────────────────────
    async fn call(
        &self,
        input: serde_json::Value,
        _context: &ToolCallContext,
        _on_progress: Option<ProgressSink>,
    ) -> ToolResult {
        let operation = input["operation"].as_str().unwrap_or("");
        let file_path = input["filePath"].as_str().unwrap_or("");
        let line = input["line"].as_u64().unwrap_or(0) as usize;
        let character = input["character"].as_u64().unwrap_or(0) as usize;

        // Wait for LSP initialization if still pending
        crate::lsp::wait_for_initialization().await;

        // Get the LSP manager
        let manager_guard = match crate::lsp::get_lsp_manager() {
            Some(g) => g,
            None => {
                return LspTool::error_result(operation, file_path, "LSP server manager not initialized.");
            }
        };

        // Handle the double Option: get_lsp_manager returns MutexGuard<Option<LspServerManager>>
        if manager_guard.is_none() {
            return LspTool::error_result(operation, file_path, "LSP server manager not initialized.");
        }

        let absolute_path = expand_path(file_path);
        let abs_path_str = match absolute_path.to_str() {
            Some(s) => s.to_string(),
            None => {
                return LspTool::error_result(operation, file_path, "Invalid file path (non-UTF-8).");
            }
        };

        // Map operation to LSP method + params
        let (method, params) = match map_operation(operation, &abs_path_str, line, character) {
            Some(mp) => mp,
            None => {
                let result = json!({
                    "operation": operation,
                    "filePath": file_path,
                    "result": format!("Unknown LSP operation: {}", operation),
                    "resultCount": 0,
                    "fileCount": 0,
                });
                return ToolResult {
                    content: result.to_string(),
                    is_error: true,
                    ..Default::default()
                };
            }
        };

        // Drop the manager guard before entering block_in_place so we don't
        // hold a std MutexGuard across an async boundary. Then re-acquire
        // inside block_in_place.
        drop(manager_guard);

        // We use block_in_place to bridge the std Mutex with async LSP calls.
        // The LSP calls use spawn_blocking internally, which requires
        // block_in_place so the runtime knows the current thread may block.
        let result = tokio::task::block_in_place(|| {
            let guard = match crate::lsp::get_lsp_manager() {
                Some(g) => g,
                None => return Err("LSP server manager not initialized.".to_string()),
            };

            let mut guard = guard;
            let manager = match guard.as_mut() {
                Some(m) => m,
                None => return Err("LSP server manager not initialized.".to_string()),
            };

            let rt = tokio::runtime::Handle::current();

            match operation {
                "incomingCalls" | "outgoingCalls" => {
                    // Two-step: prepareCallHierarchy → calls/incoming or calls/outgoing
                    let hierarchy_method = "textDocument/prepareCallHierarchy";
                    let hierarchy_params = json!({
                        "textDocument": {"uri": format!("file://{}", abs_path_str)},
                        "position": {
                            "line": line.saturating_sub(1),
                            "character": character.saturating_sub(1),
                        },
                    });

                    let items: Option<serde_json::Value> = rt
                        .block_on(manager.send_request(
                            &abs_path_str,
                            hierarchy_method,
                            hierarchy_params,
                        ))
                        .unwrap_or(None);

                    let items = match items {
                        Some(v) => v,
                        None => {
                            return Ok(LspTool::build_result(
                                operation,
                                file_path,
                                "No call hierarchy items found at this position.",
                                0,
                                0,
                            ));
                        }
                    };

                    let first_item = match items.as_array().and_then(|a| a.first()) {
                        Some(item) => item.clone(),
                        None => {
                            return Ok(LspTool::build_result(
                                operation,
                                file_path,
                                "No call hierarchy items found at this position.",
                                0,
                                0,
                            ));
                        }
                    };

                    let step2_method = if operation == "incomingCalls" {
                        "callHierarchy/incomingCalls"
                    } else {
                        "callHierarchy/outgoingCalls"
                    };

                    let step2_params = json!({"item": first_item});

                    let calls: Option<serde_json::Value> = rt
                        .block_on(manager.send_request(&abs_path_str, step2_method, step2_params))
                        .unwrap_or(None);

                    let formatted = format_call_hierarchy(operation, &calls);
                    let count = calls
                        .as_ref()
                        .and_then(|v| v.as_array())
                        .map(|a| a.len())
                        .unwrap_or(0);

                    Ok(LspTool::build_result(operation, file_path, &formatted, count, 0))
                }
                _ => {
                    let response: Option<serde_json::Value> = rt
                        .block_on(manager.send_request(&abs_path_str, &method, params))
                        .unwrap_or(None);

                    let (formatted, result_count, file_count) =
                        format_result(operation, &response);

                    Ok(LspTool::build_result(
                        operation,
                        file_path,
                        &formatted,
                        result_count,
                        file_count,
                    ))
                }
            }
        });

        match result {
            Ok(tool_result) => tool_result,
            Err(err_msg) => LspTool::error_result(
                operation,
                file_path,
                &err_msg,
            ),
        }
    }

    // ── Result mapping ──────────────────────────────────────────────────
    fn map_tool_result_to_block(
        &self,
        output: &serde_json::Value,
        tool_use_id: &str,
    ) -> ToolResultBlock {
        // Extract the "result" field string, fall back to full JSON
        let text = output
            .get("result")
            .and_then(|v| v.as_str())
            .map(String::from)
            .unwrap_or_else(|| output.to_string());

        ToolResultBlock {
            tool_use_id: tool_use_id.into(),
            content: ToolResultContent::Text(text),
            is_error: false,
        }
    }
}

impl LspTool {
    fn build_result(
        operation: &str,
        file_path: &str,
        result: &str,
        result_count: usize,
        file_count: usize,
    ) -> ToolResult {
        let content = json!({
            "operation": operation,
            "filePath": file_path,
            "result": result,
            "resultCount": result_count,
            "fileCount": file_count,
        });

        ToolResult {
            content: content.to_string(),
            is_error: false,
            ..Default::default()
        }
    }

    fn error_result(operation: &str, file_path: &str, error: &str) -> ToolResult {
        let content = json!({
            "operation": operation,
            "filePath": file_path,
            "result": error,
            "resultCount": 0,
            "fileCount": 0,
        });

        ToolResult {
            content: content.to_string(),
            is_error: true,
            ..Default::default()
        }
    }
}

// ── Helpers ─────────────────────────────────────────────────────────────

/// Resolve a (potentially relative) path to an absolute one.
fn expand_path(file_path: &str) -> std::path::PathBuf {
    let p = std::path::Path::new(file_path);
    if p.is_absolute() {
        p.to_path_buf()
    } else {
        std::env::current_dir()
            .unwrap_or_else(|_| std::path::PathBuf::from("."))
            .join(p)
    }
}

/// Map a user-facing operation name to an LSP method string and params.
fn map_operation(
    operation: &str,
    absolute_path: &str,
    line: usize,    // 1-based from user
    character: usize, // 1-based from user
) -> Option<(String, serde_json::Value)> {
    let uri = format!("file://{}", absolute_path);
    // Convert 1-based (user) to 0-based (LSP protocol)
    let position = json!({
        "line": line.saturating_sub(1),
        "character": character.saturating_sub(1),
    });

    match operation {
        "goToDefinition" => Some((
            "textDocument/definition".into(),
            json!({"textDocument": {"uri": uri}, "position": position}),
        )),
        "findReferences" => Some((
            "textDocument/references".into(),
            json!({
                "textDocument": {"uri": uri},
                "position": position,
                "context": {"includeDeclaration": true},
            }),
        )),
        "hover" => Some((
            "textDocument/hover".into(),
            json!({"textDocument": {"uri": uri}, "position": position}),
        )),
        "documentSymbol" => Some((
            "textDocument/documentSymbol".into(),
            json!({"textDocument": {"uri": uri}}),
        )),
        "workspaceSymbol" => Some((
            "workspace/symbol".into(),
            json!({"query": ""}),
        )),
        "goToImplementation" => Some((
            "textDocument/implementation".into(),
            json!({"textDocument": {"uri": uri}, "position": position}),
        )),
        "prepareCallHierarchy" => Some((
            "textDocument/prepareCallHierarchy".into(),
            json!({"textDocument": {"uri": uri}, "position": position}),
        )),
        // incomingCalls / outgoingCalls are handled via two-step in call()
        _ => None,
    }
}

/// Format an LSP response into a human-readable string along with counts.
///
/// TODO: Add richer per-operation formatters (extracting line/char from
/// Location objects, grouping references by file, rendering Hover as
/// Markdown, etc.). These are deferred pending the full typed deserialization
/// layer.
fn format_result(
    operation: &str,
    response: &Option<serde_json::Value>,
) -> (String, usize, usize) {
    let value = match response {
        Some(v) => v,
        None => return ("No response from LSP server.".into(), 0, 0),
    };

    match operation {
        "goToDefinition" | "goToImplementation" => {
            match value.as_array() {
                Some(arr) if arr.is_empty() => {
                    (format!("No {} found.", operation_label(operation)), 0, 0)
                }
                Some(arr) => {
                    let mut lines = Vec::new();
                    for loc in arr {
                        if let Some(formatted) = format_location(loc) {
                            lines.push(formatted);
                        }
                    }
                    let count = lines.len();
                    (
                        format!(
                            "Found {} {}:\n{}",
                            count,
                            if count == 1 { "result" } else { "results" },
                            lines.join("\n"),
                        ),
                        count,
                        0,
                    )
                }
                None if value.get("uri").is_some() => {
                    // Single Location object, not an array
                    match format_location(value) {
                        Some(formatted) => {
                            (format!("Defined at {}", formatted), 1, 1)
                        }
                        None => ("Result returned but could not be parsed.".into(), 0, 0),
                    }
                }
                _ => {
                    let count = value.as_array().map(|a| a.len()).unwrap_or(0);
                    let raw = serde_json::to_string_pretty(value).unwrap_or_else(|_| format!("{:?}", value));
                    (format!("Result:\n{}", raw), count, 0)
                }
            }
        }
        "findReferences" => {
            match value.as_array() {
                Some(arr) if arr.is_empty() => {
                    ("No references found.".into(), 0, 0)
                }
                Some(arr) => {
                    // Group by file
                    let mut file_groups: HashMap<String, Vec<String>> = HashMap::new();
                    for loc in arr {
                        let uri = loc
                            .get("uri")
                            .and_then(|v| v.as_str())
                            .unwrap_or("unknown");
                        let path = uri
                            .strip_prefix("file://")
                            .unwrap_or(uri);
                        let entry = format!("  line {}", loc.get("range")
                            .and_then(|r| r.get("start"))
                            .and_then(|s| s.get("line"))
                            .and_then(|l| l.as_u64())
                            .map(|n| (n as usize + 1).to_string())
                            .unwrap_or_else(|| "?".into()));
                        file_groups
                            .entry(path.to_string())
                            .or_default()
                            .push(entry);
                    }

                    let total: usize = file_groups.values().map(|v| v.len()).sum();
                    let file_count = file_groups.len();

                    let mut output = format!(
                        "Found {} reference{} across {} file{}:",
                        total,
                        if total == 1 { "" } else { "s" },
                        file_count,
                        if file_count == 1 { "" } else { "s" },
                    );
                    for (path, lines) in &file_groups {
                        output.push_str(&format!("\n{}:\n{}", path, lines.join("\n")));
                    }

                    (output, total, file_count)
                }
                _ => {
                    ("Unexpected response format for references.".into(), 0, 0)
                }
            }
        }
        "hover" => {
            let contents = value.get("contents");
            let text = match contents {
                Some(c) => {
                    // Could be a string, MarkupContent, or MarkedString[]
                    if let Some(s) = c.as_str() {
                        s.to_string()
                    } else if let Some(obj) = c.as_object() {
                        obj.get("value")
                            .and_then(|v| v.as_str())
                            .unwrap_or("")
                            .to_string()
                    } else if let Some(arr) = c.as_array() {
                        arr.iter()
                            .filter_map(|m| {
                                m.as_str()
                                    .or_else(|| m.get("value").and_then(|v| v.as_str()))
                            })
                            .collect::<Vec<_>>()
                            .join("\n")
                    } else {
                        "Hover information available.".to_string()
                    }
                }
                None => "No hover information at this position.".to_string(),
            };
            (text, 0, 0)
        }
        "documentSymbol" => {
            let symbols = value.as_array().map(|a| a.len()).unwrap_or(0);
            if symbols == 0 {
                ("No document symbols found.".into(), 0, 0)
            } else {
                let mut out = format!("Found {} document symbol{}:\n", symbols, if symbols == 1 { "" } else { "s" });
                format_symbol_list(&mut out, value.as_array(), 0);
                (out, symbols, 0)
            }
        }
        "workspaceSymbol" => {
            let symbols = value.as_array().map(|a| a.len()).unwrap_or(0);
            if symbols == 0 {
                ("No workspace symbols found.".into(), 0, 0)
            } else {
                let mut out = format!("Found {} workspace symbol{}:\n", symbols, if symbols == 1 { "" } else { "s" });
                if let Some(arr) = value.as_array() {
                    for sym in arr {
                        let name = sym.get("name").and_then(|v| v.as_str()).unwrap_or("?");
                        let kind = sym
                            .get("kind")
                            .and_then(|v| v.as_u64())
                            .map(|k| symbol_kind_name(k))
                            .unwrap_or_else(|| "".into());
                        let loc = sym
                            .get("location")
                            .and_then(|l| {
                                let uri = l.get("uri").and_then(|v| v.as_str())?;
                                let path = uri.strip_prefix("file://").unwrap_or(uri);
                                let line = l
                                    .get("range")
                                    .and_then(|r| r.get("start"))
                                    .and_then(|s| s.get("line"))
                                    .and_then(|l| l.as_u64())
                                    .map(|n| n as usize + 1);
                                match line {
                                    Some(l) => Some(format!("{}:{}", path, l)),
                                    None => Some(path.to_string()),
                                }
                            })
                            .unwrap_or_else(|| "?".into());
                        out.push_str(&format!("  {} {} ({})\n", kind, name, loc));
                    }
                }
                (out, symbols, 0)
            }
        }
        "prepareCallHierarchy" => {
            match value.as_array() {
                Some(arr) if arr.is_empty() => {
                    ("No call hierarchy items found at this position.".into(), 0, 0)
                }
                Some(arr) => {
                    let mut lines = Vec::new();
                    for item in arr {
                        let name = item.get("name").and_then(|v| v.as_str()).unwrap_or("?");
                        let kind = item
                            .get("kind")
                            .and_then(|v| v.as_u64())
                            .map(|k| symbol_kind_name(k))
                            .unwrap_or_default();
                        lines.push(format!("  {} ({})", name, kind));
                    }
                    let count = lines.len();
                    (
                        format!(
                            "Found {} call hierarchy item{}:\n{}",
                            count,
                            if count == 1 { "" } else { "s" },
                            lines.join("\n"),
                        ),
                        count,
                        0,
                    )
                }
                _ => ("Unexpected response format.".into(), 0, 0),
            }
        }
        _ => {
            let raw = serde_json::to_string_pretty(value).unwrap_or_else(|_| format!("{:?}", value));
            (format!("Result:\n{}", raw), 0, 0)
        }
    }
}

fn format_call_hierarchy(
    operation: &str,
    calls: &Option<serde_json::Value>,
) -> String {
    let direction = if operation == "incomingCalls" {
        "incoming"
    } else {
        "outgoing"
    };

    let arr = match calls.as_ref().and_then(|v| v.as_array()) {
        Some(a) => a,
        None => return format!("No {} calls found.", direction),
    };

    if arr.is_empty() {
        return format!("No {} calls found.", direction);
    }

    let mut out = format!(
        "Found {} {} call{}:\n",
        arr.len(),
        direction,
        if arr.len() == 1 { "" } else { "s" },
    );

    for call in arr {
        let from = call
            .get("from")
            .or_else(|| call.get("to"))
            .and_then(|item| item.get("name"))
            .and_then(|v| v.as_str())
            .unwrap_or("?");

        let from_kind = call
            .get("from")
            .or_else(|| call.get("to"))
            .and_then(|item| item.get("kind"))
            .and_then(|v| v.as_u64())
            .map(|k| symbol_kind_name(k))
            .unwrap_or_default();

        let ranges = call.get("fromRanges").or_else(|| call.get("toRanges"));
        let location = ranges
            .and_then(|r| r.as_array())
            .and_then(|arr| arr.first())
            .and_then(|r| {
                let line = r
                    .get("start")
                    .and_then(|s| s.get("line"))
                    .and_then(|l| l.as_u64())
                    .map(|n| n as usize + 1);
                let char = r
                    .get("start")
                    .and_then(|s| s.get("character"))
                    .and_then(|c| c.as_u64())
                    .map(|n| n as usize + 1);
                match (line, char) {
                    (Some(l), Some(c)) => Some(format!("line {}, col {}", l, c)),
                    (Some(l), None) => Some(format!("line {}", l)),
                    _ => None,
                }
            })
            .unwrap_or_else(|| "?".into());

        out.push_str(&format!("  {} ({}) — {}\n", from, from_kind, location));
    }

    out
}

fn format_symbol_list(out: &mut String, symbols: Option<&Vec<serde_json::Value>>, indent: usize) {
    let symbols = match symbols {
        Some(s) => s,
        None => return,
    };
    let prefix = "  ".repeat(indent + 1);
    for sym in symbols {
        let name = sym.get("name").and_then(|v| v.as_str()).unwrap_or("?");
        let kind = sym
            .get("kind")
            .and_then(|v| v.as_u64())
            .map(|k| symbol_kind_name(k))
            .unwrap_or_default();
        out.push_str(&format!("{}{} ({})\n", prefix, name, kind));
        if let Some(children) = sym.get("children").and_then(|v| v.as_array()) {
            format_symbol_list(out, Some(&children.iter().cloned().collect()), indent + 1);
        }
    }
}

fn format_location(loc: &serde_json::Value) -> Option<String> {
    let uri = loc.get("uri").and_then(|v| v.as_str())?;
    let path = uri.strip_prefix("file://").unwrap_or(uri);
    let range = loc.get("range");
    let line = range
        .and_then(|r| r.get("start"))
        .and_then(|s| s.get("line"))
        .and_then(|l| l.as_u64())
        .map(|n| n as usize + 1);
    let character = range
        .and_then(|r| r.get("start"))
        .and_then(|s| s.get("character"))
        .and_then(|c| c.as_u64())
        .map(|n| n as usize + 1);

    match (line, character) {
        (Some(l), Some(c)) => Some(format!("{}:{}:{}", path, l, c)),
        (Some(l), None) => Some(format!("{}:{}", path, l)),
        _ => Some(path.to_string()),
    }
}

fn operation_label(op: &str) -> &str {
    match op {
        "goToDefinition" => "definitions",
        "goToImplementation" => "implementations",
        "findReferences" => "references",
        _ => "results",
    }
}

fn symbol_kind_name(kind: u64) -> String {
    // SymbolKind as defined in the LSP spec:
    // https://microsoft.github.io/language-server-protocol/specifications/lsp/3.17/specification/#symbolKind
    match kind {
        1 => "File",
        2 => "Module",
        3 => "Namespace",
        4 => "Package",
        5 => "Class",
        6 => "Method",
        7 => "Property",
        8 => "Field",
        9 => "Constructor",
        10 => "Enum",
        11 => "Interface",
        12 => "Function",
        13 => "Variable",
        14 => "Constant",
        15 => "String",
        16 => "Number",
        17 => "Boolean",
        18 => "Array",
        19 => "Object",
        20 => "Key",
        21 => "Null",
        22 => "EnumMember",
        23 => "Struct",
        24 => "Event",
        25 => "Operator",
        26 => "TypeParameter",
        _ => "Symbol",
    }
    .to_string()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn expand_path_absolute() {
        let result = expand_path("/tmp/test.rs");
        assert_eq!(result, std::path::PathBuf::from("/tmp/test.rs"));
    }

    #[test]
    fn expand_path_relative() {
        let result = expand_path("test.rs");
        assert!(result.is_absolute());
        assert!(result.ends_with("test.rs"));
    }

    #[test]
    fn is_lsp_returns_true() {
        let tool = LspTool;
        assert!(tool.is_lsp());
    }

    #[test]
    fn should_defer_returns_true() {
        let tool = LspTool;
        assert!(tool.should_defer());
    }

    #[test]
    fn search_hint_is_some() {
        let tool = LspTool;
        assert!(tool.search_hint().is_some());
    }

    #[test]
    fn get_path_extracts_file_path() {
        let tool = LspTool;
        let input = json!({"filePath": "/tmp/test.rs"});
        let path = tool.get_path(&input);
        assert!(path.is_some());
        assert!(path.unwrap().to_string_lossy().contains("test.rs"));
    }

    #[test]
    fn map_operation_go_to_definition() {
        let (method, params) =
            map_operation("goToDefinition", "/tmp/test.rs", 10, 5).unwrap();
        assert_eq!(method, "textDocument/definition");
        assert_eq!(params["textDocument"]["uri"], "file:///tmp/test.rs");
        assert_eq!(params["position"]["line"], 9);  // 0-based
        assert_eq!(params["position"]["character"], 4); // 0-based
    }

    #[test]
    fn map_operation_unknown() {
        assert!(map_operation("unknownOp", "/tmp/test.rs", 1, 1).is_none());
    }

    #[test]
    fn output_schema_has_expected_fields() {
        let tool = LspTool;
        let schema = tool.output_schema().unwrap();
        let props = schema.get("properties").unwrap();
        assert!(props.get("operation").is_some());
        assert!(props.get("filePath").is_some());
        assert!(props.get("result").is_some());
        assert!(props.get("resultCount").is_some());
        assert!(props.get("fileCount").is_some());
    }
}

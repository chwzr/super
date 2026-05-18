use crate::tools::contract::{DescriptionCtx, PromptCtx, ProgressSink, Tool, ToolCallContext, ToolResult, ToolResultBlock, ToolResultContent};
use async_trait::async_trait;
use serde_json::json;

#[derive(Default)]
pub struct ConfigTool;

#[async_trait]
impl Tool for ConfigTool {
    fn name(&self) -> &str {
        "Config"
    }

    fn description(&self, _input: Option<&serde_json::Value>, _ctx: &DescriptionCtx) -> String {
        "Reads and writes configuration settings.".into()
    }

    fn prompt(&self, _ctx: &PromptCtx) -> String {
        include_str!("prompts/config_tool.txt").into()
    }

    fn input_schema(&self) -> serde_json::Value {
        serde_json::json!({
            "type": "object",
            "properties": {
                "key": {
                    "type": "string",
                    "description": "The configuration key to get or set. Use dot notation for nested keys (e.g., \"settings.model\", \"permissions.read\")"
                },
                "value": {
                    "description": "The value to set. If provided, sets the configuration. If omitted, reads the current value."
                }
            },
            "required": ["key"]
        })
    }

    fn output_schema(&self) -> Option<serde_json::Value> {
        Some(json!({
            "type": "object",
            "properties": {
                "key": {"type": "string"},
                "value": {"description": "The current value, if reading"},
                "set": {"type": "boolean", "description": "Whether the key was set"}
            }
        }))
    }

    fn is_concurrency_safe(&self, _input: &serde_json::Value) -> bool {
        true
    }

    fn is_read_only(&self, input: &serde_json::Value) -> bool {
        // Read-only when no value is provided (get mode);
        // write when value is present (set mode).
        input.get("value").is_none()
    }

    async fn call(&self, input: serde_json::Value, _context: &ToolCallContext, _on_progress: Option<ProgressSink>) -> ToolResult {
        let key = match input.get("key").and_then(|v| v.as_str()) {
            Some(k) => k,
            None => {
                return ToolResult {
                    content: "Missing required parameter: key".to_string(),
                    is_error: true,
                    inject_messages: Vec::new(),
                    metadata: None,
                    mcp_meta: None,
                    new_messages: Vec::new(),
                };
            }
        };

        let config = crate::config::load_config();

        if let Some(value) = input.get("value") {
            // Write mode: set the value
            let settings = &mut config.settings.clone();
            let settings_map = settings.as_object_mut().map(|m| {
                set_nested(m, key, value.clone());
            });

            if settings_map.is_none() {
                return ToolResult {
                    content: "Cannot set configuration: settings is not an object".to_string(),
                    is_error: true,
                    inject_messages: Vec::new(),
                    metadata: None,
                    mcp_meta: None,
                    new_messages: Vec::new(),
                };
            }

            let mut new_config = config.clone();
            new_config.settings = settings.clone();
            crate::config::save_config(&new_config);

            ToolResult {
                content: format!("Set configuration key '{}'", key),
                is_error: false,
                inject_messages: Vec::new(),
                metadata: None,
                mcp_meta: None,
                new_messages: Vec::new(),
            }
        } else {
            // Read mode: get the value
            let settings = &config.settings;
            let settings_obj = match settings.as_object() {
                Some(obj) => obj,
                None => {
                    return ToolResult {
                        content: format!("Key '{}': settings is not an object", key),
                        is_error: true,
                        inject_messages: Vec::new(),
                        metadata: None,
                        mcp_meta: None,
                        new_messages: Vec::new(),
                    };
                }
            };

            match get_nested(settings_obj, key) {
                Some(val) => {
                    ToolResult {
                        content: format!("{}: {}", key, serde_json::to_string_pretty(val).unwrap_or_default()),
                        is_error: false,
                        inject_messages: Vec::new(),
                        metadata: None,
                        mcp_meta: None,
                        new_messages: Vec::new(),
                    }
                }
                None => {
                    let config_val = get_config_field(&config, key);
                    match config_val {
                        Some(val) => ToolResult {
                            content: format!("{}: {}", key, serde_json::to_string_pretty(&val).unwrap_or_default()),
                            is_error: false,
                            inject_messages: Vec::new(),
                            metadata: None,
                            mcp_meta: None,
                            new_messages: Vec::new(),
                        },
                        None => ToolResult {
                            content: format!("Key '{}' not found in configuration", key),
                            is_error: true,
                            inject_messages: Vec::new(),
                            metadata: None,
                            mcp_meta: None,
                            new_messages: Vec::new(),
                        },
                    }
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

fn get_nested<'a>(obj: &'a serde_json::Map<String, serde_json::Value>, key: &str) -> Option<&'a serde_json::Value> {
    let parts: Vec<&str> = key.split('.').collect();

    let mut current_obj = obj;
    for (i, part) in parts.iter().enumerate() {
        if i == parts.len() - 1 {
            return current_obj.get(*part);
        }
        match current_obj.get(*part) {
            Some(serde_json::Value::Object(next)) => {
                current_obj = next;
            }
            _ => return None,
        }
    }
    None
}

fn set_nested(
    obj: &mut serde_json::Map<String, serde_json::Value>,
    key: &str,
    value: serde_json::Value,
) {
    let parts: Vec<&str> = key.split('.').collect();

    if parts.len() == 1 {
        obj.insert(parts[0].to_string(), value);
        return;
    }

    let mut current: *mut serde_json::Map<String, serde_json::Value> = obj;
    for (i, part) in parts.iter().enumerate() {
        if i == parts.len() - 1 {
            unsafe {
                (*current).insert(part.to_string(), value.clone());
            }
        } else {
            let entry = unsafe { (*current).entry(part.to_string()) };
            match entry {
                serde_json::map::Entry::Occupied(o) => {
                    if !o.get().is_object() {
                        o.into_mut();
                        unsafe {
                            (*current).insert(part.to_string(), serde_json::Value::Object(serde_json::Map::new()));
                        }
                    }
                }
                serde_json::map::Entry::Vacant(v) => {
                    v.insert(serde_json::Value::Object(serde_json::Map::new()));
                }
            }
            if let Some(serde_json::Value::Object(ref mut next)) = unsafe { (*current).get_mut(*part) } {
                current = next;
            }
        }
    }
}

fn get_config_field(config: &shared::CliConfig, key: &str) -> Option<serde_json::Value> {
    match key {
        "access_token" => config.access_token.as_ref().map(|v| serde_json::Value::String(v.clone())),
        "refresh_token" => config.refresh_token.as_ref().map(|v| serde_json::Value::String(v.clone())),
        "openrouter_api_key" => config.openrouter_api_key.as_ref().map(|v| serde_json::Value::String(v.clone())),
        "api_base_url" => Some(serde_json::Value::String(config.api_base_url.clone())),
        "model" => Some(serde_json::Value::String(config.model.clone())),
        "settings" => Some(config.settings.clone()),
        "permissions" => Some(config.permissions.clone()),
        _ => None,
    }
}

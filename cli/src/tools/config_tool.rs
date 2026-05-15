use crate::tools::contract::{Tool, ToolCallContext, ToolResult};
use async_trait::async_trait;

#[derive(Default)]
pub struct ConfigTool;

#[async_trait]
impl Tool for ConfigTool {
    fn name(&self) -> &str {
        "Config"
    }

    fn description(&self) -> &str {
        "Reads and writes configuration settings."
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

    fn is_concurrency_safe(&self) -> bool {
        true
    }

    fn is_read_only(&self) -> bool {
        true
    }

    async fn call(&self, input: serde_json::Value, _context: &ToolCallContext) -> ToolResult {
        let key = match input.get("key").and_then(|v| v.as_str()) {
            Some(k) => k,
            None => {
                return ToolResult {
                    content: "Missing required parameter: key".to_string(),
                    is_error: true,
                    ..Default::default()
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
                // settings is not an object, can't set nested values
                return ToolResult {
                    content: "Cannot set configuration: settings is not an object".to_string(),
                    is_error: true,
                    ..Default::default()
                };
            }

            let mut new_config = config.clone();
            new_config.settings = settings.clone();
            crate::config::save_config(&new_config);

            ToolResult {
                content: format!("Set configuration key '{}'", key),
                is_error: false,
                ..Default::default()
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
                        ..Default::default()
                    };
                }
            };

            match get_nested(settings_obj, key) {
                Some(val) => {
                    ToolResult {
                        content: format!("{}: {}", key, serde_json::to_string_pretty(val).unwrap_or_default()),
                        is_error: false,
                        ..Default::default()
                    }
                }
                None => {
                    // Check top-level config fields too
                    let config_val = get_config_field(&config, key);
                    match config_val {
                        Some(val) => ToolResult {
                            content: format!("{}: {}", key, serde_json::to_string_pretty(&val).unwrap_or_default()),
                            is_error: false,
                            ..Default::default()
                        },
                        None => ToolResult {
                            content: format!("Key '{}' not found in configuration", key),
                            is_error: true,
                            ..Default::default()
                        },
                    }
                }
            }
        }
    }
}

/// Navigate a nested object using dot notation and return the value
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

/// Set a value in a nested object using dot notation
fn set_nested(
    obj: &mut serde_json::Map<String, serde_json::Value>,
    key: &str,
    value: serde_json::Value,
) {
    let parts: Vec<&str> = key.split('.').collect();

    // For the simple case (single key), avoid the complex borrow dance
    if parts.len() == 1 {
        obj.insert(parts[0].to_string(), value);
        return;
    }

    // Walk the path, creating intermediate objects as needed
    let mut current: *mut serde_json::Map<String, serde_json::Value> = obj;
    for (i, part) in parts.iter().enumerate() {
        if i == parts.len() - 1 {
            // Safety: we know `current` is a valid pointer to the map we own
            unsafe {
                (*current).insert(part.to_string(), value.clone());
            }
        } else {
            let entry = unsafe { (*current).entry(part.to_string()) };
            match entry {
                serde_json::map::Entry::Occupied(o) => {
                    if !o.get().is_object() {
                        // Replace with an empty object
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
            // Get mutable ref to the nested object
            if let Some(serde_json::Value::Object(ref mut next)) = unsafe { (*current).get_mut(*part) } {
                current = next;
            }
        }
    }
}

/// Get a top-level config field value by key name
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

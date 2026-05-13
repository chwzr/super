use crate::tools::contract::{Tool, ToolCallContext, ToolResult};
use async_trait::async_trait;

#[derive(Default)]
pub struct NotebookEditTool;

#[async_trait]
impl Tool for NotebookEditTool {
    fn name(&self) -> &str {
        "NotebookEdit"
    }

    fn description(&self) -> &str {
        "Edits Jupyter notebook cells."
    }

    fn input_schema(&self) -> serde_json::Value {
        serde_json::json!({
            "type": "object",
            "properties": {
                "notebook_path": {
                    "type": "string",
                    "description": "The absolute path to the Jupyter notebook file to edit"
                },
                "cell_id": {
                    "type": "string",
                    "description": "The ID of the cell to edit. When inserting a new cell, the new cell will be inserted after the cell with this ID, or at the beginning if not specified."
                },
                "new_source": {
                    "type": "string",
                    "description": "The new source for the cell"
                },
                "cell_type": {
                    "type": "string",
                    "description": "The type of the cell (code or markdown). Required when edit_mode is 'insert'.",
                    "enum": ["code", "markdown"]
                },
                "edit_mode": {
                    "type": "string",
                    "description": "The type of edit to make (replace, insert, delete). Defaults to replace.",
                    "enum": ["replace", "insert", "delete"]
                }
            },
            "required": ["notebook_path", "new_source"]
        })
    }

    fn is_destructive(&self) -> bool {
        true
    }

    async fn call(&self, input: serde_json::Value, _context: &ToolCallContext) -> ToolResult {
        let notebook_path = match input.get("notebook_path").and_then(|v| v.as_str()) {
            Some(p) => p,
            None => {
                return ToolResult {
                    content: "Missing required parameter: notebook_path".to_string(),
                    is_error: true,
                    metadata: None,
                };
            }
        };

        let new_source: String = match input.get("new_source").and_then(|v| v.as_str()) {
            Some(s) => s.to_string(),
            None => {
                return ToolResult {
                    content: "Missing required parameter: new_source".to_string(),
                    is_error: true,
                    metadata: None,
                };
            }
        };

        let cell_id = input.get("cell_id").and_then(|v| v.as_str());
        let cell_type = input.get("cell_type").and_then(|v| v.as_str());
        let edit_mode = input
            .get("edit_mode")
            .and_then(|v| v.as_str())
            .unwrap_or("replace");

        let path = std::path::Path::new(notebook_path);
        if !path.exists() {
            return ToolResult {
                content: format!("Notebook not found: {}", notebook_path),
                is_error: true,
                metadata: None,
            };
        }

        let content = match std::fs::read_to_string(path) {
            Ok(c) => c,
            Err(e) => {
                return ToolResult {
                    content: format!("Error reading notebook {}: {}", notebook_path, e),
                    is_error: true,
                    metadata: None,
                };
            }
        };

        let mut notebook: serde_json::Value = match serde_json::from_str(&content) {
            Ok(v) => v,
            Err(e) => {
                return ToolResult {
                    content: format!(
                        "Error parsing notebook {}: {}. Ensure the file is a valid .ipynb JSON file.",
                        notebook_path, e
                    ),
                    is_error: true,
                    metadata: None,
                };
            }
        };

        let cells = notebook
            .get_mut("cells")
            .and_then(|c| c.as_array_mut());

        let cells = match cells {
            Some(c) => c,
            None => {
                return ToolResult {
                    content: format!(
                        "Invalid notebook format: {} does not contain a 'cells' array.",
                        notebook_path
                    ),
                    is_error: true,
                    metadata: None,
                };
            }
        };

        match edit_mode {
            "replace" => {
                let cell_idx = find_cell_index(cells, cell_id, notebook_path);

                let idx = match cell_idx {
                    Ok(i) => i,
                    Err(e) => return e,
                };

                // Preserve existing cell_type if not specified (clone to avoid borrow conflict)
                let existing_type = cells[idx]
                    .get("cell_type")
                    .and_then(|v| v.as_str())
                    .unwrap_or("code")
                    .to_string();
                let ct: String = cell_type.map(|s| s.to_string()).unwrap_or(existing_type);

                cells[idx]["source"] = cell_source_value(&new_source, &ct);
                cells[idx]["cell_type"] = serde_json::Value::String(ct);
            }
            "insert" => {
                let ct = match cell_type {
                    Some(t) => t,
                    None => {
                        return ToolResult {
                            content: "cell_type is required when edit_mode is 'insert'".to_string(),
                            is_error: true,
                            metadata: None,
                        };
                    }
                };

                let new_cell = create_cell(ct, new_source.as_str());

                if let Some(cid) = cell_id {
                    let insert_after = cells.iter().position(|c| {
                        c.get("id").and_then(|v| v.as_str()) == Some(cid)
                    });
                    match insert_after {
                        Some(pos) => cells.insert(pos + 1, new_cell),
                        None => {
                            return ToolResult {
                                content: format!(
                                    "Cell with id '{}' not found in notebook {}",
                                    cid, notebook_path
                                ),
                                is_error: true,
                                metadata: None,
                            };
                        }
                    }
                } else {
                    cells.insert(0, new_cell);
                }
            }
            "delete" => {
                let cell_idx = find_cell_index(cells, cell_id, notebook_path);

                let idx = match cell_idx {
                    Ok(i) => i,
                    Err(e) => return e,
                };

                cells.remove(idx);
            }
            _ => {
                return ToolResult {
                    content: format!("Invalid edit_mode '{}'. Must be 'replace', 'insert', or 'delete'.", edit_mode),
                    is_error: true,
                    metadata: None,
                };
            }
        }

        // Write the notebook back
        // Release the mutable borrow on cells so notebook can be serialized
        let cell_count = cells.len();
        let _ = cells;
        let json_str = match serde_json::to_string_pretty(&notebook) {
            Ok(s) => s,
            Err(e) => {
                return ToolResult {
                    content: format!("Error serializing notebook: {}", e),
                    is_error: true,
                    metadata: None,
                };
            }
        };

        match std::fs::write(path, &json_str) {
            Ok(()) => {
                let mut meta = std::collections::HashMap::new();
                meta.insert("edit_mode".to_string(), edit_mode.to_string());
                meta.insert("cell_count".to_string(), cell_count.to_string());

                ToolResult {
                    content: format!(
                        "Successfully performed '{}' on notebook {} ({} cells)",
                        edit_mode, notebook_path, cell_count
                    ),
                    is_error: false,
                    metadata: Some(meta),
                }
            }
            Err(e) => ToolResult {
                content: format!("Error writing notebook {}: {}", notebook_path, e),
                is_error: true,
                metadata: None,
            },
        }
    }
}

fn find_cell_index(
    cells: &[serde_json::Value],
    cell_id: Option<&str>,
    notebook_path: &str,
) -> Result<usize, ToolResult> {
    match cell_id {
        Some(cid) => {
            let pos = cells.iter().position(|c| {
                c.get("id").and_then(|v| v.as_str()) == Some(cid)
            });
            match pos {
                Some(i) => Ok(i),
                None => Err(ToolResult {
                    content: format!(
                        "Cell with id '{}' not found in notebook {}",
                        cid, notebook_path
                    ),
                    is_error: true,
                    metadata: None,
                }),
            }
        }
        None => {
            if cells.is_empty() {
                Err(ToolResult {
                    content: format!("Notebook {} has no cells to edit", notebook_path),
                    is_error: true,
                    metadata: None,
                })
            } else {
                Ok(0)
            }
        }
    }
}

fn cell_source_value(source: &str, cell_type: &str) -> serde_json::Value {
    if cell_type == "markdown" || source.contains('\n') {
        // Multi-line source: split into array of strings
        let lines: Vec<String> = source.lines().map(|l| format!("{}\n", l)).collect();
        serde_json::Value::Array(lines.into_iter().map(serde_json::Value::String).collect())
    } else {
        // Single line: single string in array
        serde_json::json!([source])
    }
}

fn create_cell(cell_type: &str, source: &str) -> serde_json::Value {
    let source_val = cell_source_value(source, cell_type);

    let mut cell = serde_json::json!({
        "cell_type": cell_type,
        "metadata": {},
        "source": source_val,
    });

    // Add default outputs for code cells
    if cell_type == "code" {
        cell["outputs"] = serde_json::Value::Array(Vec::new());
        // Generate a simple execution_count
        cell["execution_count"] = serde_json::Value::Null;
    }

    // Add a deterministic id
    cell["id"] = serde_json::Value::String(format!("cell-{}", uuid::Uuid::new_v4()));

    cell
}

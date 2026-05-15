use std::collections::HashMap;
use std::path::Path;

use crate::tools::contract::{Tool, ToolCallContext, ToolResult};
use async_trait::async_trait;

const DANGEROUS_PATHS: &[&str] = &[
    "/dev/zero",
    "/dev/random",
    "/dev/urandom",
    "/dev/null",
    "/proc",
    "/sys",
];

#[derive(Default)]
pub struct ReadTool;

#[async_trait]
impl Tool for ReadTool {
    fn name(&self) -> &str {
        "Read"
    }

    fn description(&self) -> &str {
        "Reads a file from the local filesystem. Supports text, images, PDFs, and Jupyter notebooks."
    }

    fn input_schema(&self) -> serde_json::Value {
        serde_json::json!({
            "type": "object",
            "properties": {
                "file_path": {
                    "type": "string",
                    "description": "The absolute path to the file to read"
                },
                "offset": {
                    "type": "integer",
                    "description": "The line number to start reading from (0-based for internal processing, used as 1-based in output)",
                    "minimum": 0
                },
                "limit": {
                    "type": "integer",
                    "description": "The number of lines to read",
                    "minimum": 1
                }
            },
            "required": ["file_path"]
        })
    }

    fn is_concurrency_safe(&self) -> bool {
        true
    }

    fn is_read_only(&self) -> bool {
        true
    }

    async fn call(&self, input: serde_json::Value, _context: &ToolCallContext) -> ToolResult {
        let file_path = match input.get("file_path").and_then(|v| v.as_str()) {
            Some(p) => p,
            None => {
                return ToolResult {
                    content: "Missing required parameter: file_path".to_string(),
                    is_error: true,
                    ..Default::default()
                };
            }
        };

        // Block dangerous paths
        let canonical = Path::new(file_path).canonicalize().ok();
        for dangerous in DANGEROUS_PATHS {
            if file_path.starts_with(dangerous) {
                return ToolResult {
                    content: format!("Access denied: reading '{}' is not allowed", dangerous),
                    is_error: true,
                    ..Default::default()
                };
            }
            if let Some(ref canon) = canonical {
                if canon.starts_with(dangerous) {
                    return ToolResult {
                        content: format!("Access denied: reading '{}' is not allowed", dangerous),
                        is_error: true,
                        ..Default::default()
                    };
                }
            }
        }

        let path = Path::new(file_path);

        if !path.exists() {
            return ToolResult {
                content: format!("File not found: {}", file_path),
                is_error: true,
                ..Default::default()
            };
        }

        // Detect file type by extension
        let ext = path
            .extension()
            .and_then(|e| e.to_str())
            .map(|e| e.to_lowercase());

        // Check for Jupyter notebooks
        if ext.as_deref() == Some("ipynb") {
            return self.read_notebook(path, file_path);
        }

        // Check for image files
        if let Some(ext) = &ext {
            let image_exts = [
                "png", "jpg", "jpeg", "gif", "bmp", "webp", "svg", "ico", "tiff", "tif",
            ];
            if image_exts.contains(&ext.as_str()) {
                return self.read_image(path, file_path);
            }
        }

        // Check for PDF
        if ext.as_deref() == Some("pdf") {
            // For PDF, return the file path info — actual PDF reading would need a PDF crate
            let metadata = std::fs::metadata(path).ok();
            let size = metadata.map(|m| m.len()).unwrap_or(0);
            let mut meta = HashMap::new();
        meta.insert("file_type".to_string(), "pdf".to_string());
        return ToolResult {
                content: format!(
                    "PDF file: {}\nSize: {} bytes\nTo read the PDF content, provide the 'pages' parameter (e.g., pages=\"1-5\").",
                    file_path, size
                ),
                is_error: false,
                metadata: Some(meta),
                inject_messages: Vec::new(),
            };
        }

        // Default: read as text with line numbers
        let offset = input
            .get("offset")
            .and_then(|v| v.as_i64())
            .map(|v| v as usize)
            .unwrap_or(0);
        let limit = input
            .get("limit")
            .and_then(|v| v.as_i64())
            .map(|v| v as usize);

        self.read_text(path, file_path, offset, limit)
    }
}

impl ReadTool {
    fn read_text(
        &self,
        path: &Path,
        file_path: &str,
        offset: usize,
        limit: Option<usize>,
    ) -> ToolResult {
        let content = match std::fs::read_to_string(path) {
            Ok(c) => c,
            Err(e) => {
                return ToolResult {
                    content: format!("Error reading file {}: {}", file_path, e),
                    is_error: true,
                    ..Default::default()
                };
            }
        };

        let lines: Vec<&str> = content.lines().collect();
        let total_lines = lines.len();

        // If offset >= total_lines, return empty
        if offset > 0 && offset >= total_lines {
            return ToolResult {
                content: format!(
                    "Offset {} is beyond the end of the file ({} lines total)",
                    offset, total_lines
                ),
                is_error: true,
                ..Default::default()
            };
        }

        let start = offset;
        let end = match limit {
            Some(l) => std::cmp::min(start + l, total_lines),
            None => total_lines,
        };

        let display_lines: Vec<String> = lines[start..end]
            .iter()
            .enumerate()
            .map(|(i, line)| format!("{}\t{}", start + i + 1, line))
            .collect();

        let result = display_lines.join("\n");

        let mut meta = HashMap::new();
        meta.insert("total_lines".to_string(), total_lines.to_string());
        if limit.is_some() {
            meta.insert(
                "returned_lines".to_string(),
                (end - start).to_string(),
            );
        }

        ToolResult {
            content: result,
            is_error: false,
            metadata: Some(meta),
            inject_messages: Vec::new(),
        }
    }

    fn read_notebook(&self, path: &Path, file_path: &str) -> ToolResult {
        let content = match std::fs::read_to_string(path) {
            Ok(c) => c,
            Err(e) => {
                return ToolResult {
                    content: format!("Error reading notebook {}: {}", file_path, e),
                    is_error: true,
                    ..Default::default()
                };
            }
        };

        let notebook: Result<serde_json::Value, _> = serde_json::from_str(&content);
        match notebook {
            Ok(json) => {
                let cells = json
                    .get("cells")
                    .and_then(|c| c.as_array())
                    .map(|c| c.len())
                    .unwrap_or(0);

                let mut result = format!(
                    "Jupyter Notebook: {}\nTotal cells: {}\n\n",
                    file_path, cells
                );

                if let Some(cells_arr) = json.get("cells").and_then(|c| c.as_array()) {
                    for (i, cell) in cells_arr.iter().enumerate() {
                        let cell_type = cell
                            .get("cell_type")
                            .and_then(|v| v.as_str())
                            .unwrap_or("unknown");
                        let source = cell
                            .get("source")
                            .and_then(|v| v.as_array())
                            .map(|arr| {
                                arr.iter()
                                    .filter_map(|v| v.as_str())
                                    .collect::<Vec<_>>()
                                    .join("")
                            })
                            .unwrap_or_default();

                        let preview = if source.len() > 200 {
                            format!("{}...", &source[..200])
                        } else {
                            source.clone()
                        };

                        result.push_str(&format!(
                            "Cell {} [{}]:\n{}\n\n",
                            i, cell_type, preview
                        ));
                    }
                }

                let mut meta = HashMap::new();
                meta.insert("cell_count".to_string(), cells.to_string());
                meta.insert("file_type".to_string(), "ipynb".to_string());

                ToolResult {
                    content: result,
                    is_error: false,
                    metadata: Some(meta),
                    inject_messages: Vec::new(),
                }
            }
            Err(e) => ToolResult {
                content: format!("Error parsing notebook {}: {}", file_path, e),
                is_error: true,
                ..Default::default()
            },
        }
    }

    fn read_image(&self, path: &Path, file_path: &str) -> ToolResult {
        let metadata = match std::fs::metadata(path) {
            Ok(m) => m,
            Err(e) => {
                return ToolResult {
                    content: format!("Error reading image {}: {}", file_path, e),
                    is_error: true,
                    ..Default::default()
                };
            }
        };

        let size = metadata.len();
        let size_str = if size < 1024 {
            format!("{} B", size)
        } else if size < 1024 * 1024 {
            format!("{:.1} KB", size as f64 / 1024.0)
        } else {
            format!("{:.1} MB", size as f64 / (1024.0 * 1024.0))
        };

        // Try to read image bytes for visual display
        let _image_data = std::fs::read(path).unwrap_or_default();

        let mut meta = HashMap::new();
        meta.insert("file_size".to_string(), size_str.clone());
        meta.insert("file_type".to_string(), "image".to_string());

        ToolResult {
            content: format!("Image file: {}\nSize: {}", file_path, size_str),
            is_error: false,
            metadata: Some(meta),
            inject_messages: Vec::new(),
        }
    }
}

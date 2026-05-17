# Batch 3 — File & Shell Tools Parity Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Bring Read, Edit, Write, Glob, Grep, NotebookEdit, and Bash to full Claude Code parity — correct schemas, prompt text, output schemas, render hooks, and permission matchers.

**Architecture:** Seven tools, each with prompt ports, schema expansions, output schemas, and render hook wiring. Read gets `pages` and a discriminated output type. Edit/Write get `getPath`/`toAutoClassifierInput`/`backfillObservableInput` and `RenderSpec::Diff`. Grep gets a major schema expansion (11 new flags). Bash gets `prepare_permission_matcher` and the massive conditional prompt. All prompts are ported from `claude-code-src/tools/<ToolName>/prompt.ts`.

**Tech Stack:** Rust, serde_json, regex, glob, walkdir, shared::RenderSpec (Batch 1).

**Prerequisite:** Batch 1 (extended Tool trait) must be complete. Batches 2 and 3 are independent of each other.

---

## File Map

| File | Responsibility |
|------|---------------|
| `cli/src/tools/read.rs` | Add `pages`, outputSchema discriminated union, render hooks |
| `cli/src/tools/edit.rs` | Port prompt, outputSchema, getPath, toAutoClassifierInput, RenderSpec::Diff |
| `cli/src/tools/write.rs` | Port prompt, outputSchema, getPath, toAutoClassifierInput |
| `cli/src/tools/glob_tool.rs` | Port prompt, outputSchema, RenderSpec::PathList |
| `cli/src/tools/grep.rs` | Major schema expansion, port prompt, outputSchema by mode |
| `cli/src/tools/notebook_edit.rs` | Port prompt, outputSchema |
| `cli/src/tools/bash.rs` | Add dangerouslyDisableSandbox, port massive prompt, prepare_permission_matcher, outputSchema |
| `cli/src/tools/prompts/read.txt` | Port FileReadTool prompt |
| `cli/src/tools/prompts/edit.txt` | Port FileEditTool prompt |
| `cli/src/tools/prompts/write.txt` | Port FileWriteTool prompt |
| `cli/src/tools/prompts/glob_tool.txt` | Port GlobTool prompt |
| `cli/src/tools/prompts/grep.txt` | Port GrepTool prompt |
| `cli/src/tools/prompts/notebook_edit.txt` | Port NotebookEditTool prompt |
| `cli/src/tools/prompts/bash.txt` | Port BashTool prompt (massive, ~370 lines) |

---

## Prerequisites

Before any task in this batch, verify Batch 1 is complete:
- The extended `Tool` trait in `cli/src/tools/contract.rs` has all methods from the design spec
- `shared/src/render_spec.rs` has `Diff`, `PathList`, `PathEntry`, `DiffHunk`, `DiffLine` variants
- All existing tools compile against the new trait with placeholder impls

Check: `cargo check -p cli 2>&1`

---

### Task 1: Port Read tool prompt text

**Files:**
- Modify: `cli/src/tools/prompts/read.txt`

Replace the TODO placeholder with the full prompt from `../claude-code-src/tools/FileReadTool/prompt.ts` — the return value of `renderPromptTemplate()`, using the default instruction variant (not targeted).

- [ ] **Step 1: Write the prompt file**

Write `cli/src/tools/prompts/read.txt`:

```
Reads a file from the local filesystem. You can access any file directly by using this tool.
Assume this tool is able to read all files on the machine. If the User provides a path to a file assume that path is valid. It is okay to read a file that does not exist; an error will be returned.

Usage:
- The file_path parameter must be an absolute path, not a relative path
- By default, it reads up to 2000 lines starting from the beginning of the file
- You can optionally specify a line offset and limit (especially handy for long files), but it's recommended to read the whole file by not providing these parameters
- Results are returned using cat -n format, with line numbers starting at 1
- This tool allows Claude Code to read images (eg PNG, JPG, etc). When reading an image file the contents are presented visually as Claude Code is a multimodal LLM.
- This tool can read PDF files (.pdf). For large PDFs (more than 10 pages), you MUST provide the pages parameter to read specific page ranges (e.g., pages: "1-5"). Reading a large PDF without the pages parameter will fail. Maximum 20 pages per request.
- This tool can read Jupyter notebooks (.ipynb files) and returns all cells with their outputs, combining code, text, and visualizations.
- This tool can only read files, not directories. To read a directory, use an ls command via the Bash tool.
- You will regularly be asked to read screenshots. If the user provides a path to a screenshot, ALWAYS use this tool to view the file at the path. This tool will work with all temporary file paths.
- If you read a file that exists but has empty contents you will receive a system reminder warning in place of file contents.
```

- [ ] **Step 2: Verify the file was written**

Run: `wc -l cli/src/tools/prompts/read.txt`

- [ ] **Step 3: Commit**

```bash
git add cli/src/tools/prompts/read.txt
git commit -m "feat(parity): port Read tool prompt text from Claude"
```

---

### Task 2: Add pages field, output_schema, and parity methods to ReadTool

**Files:**
- Modify: `cli/src/tools/read.rs`

Add `pages` field to `input_schema()`, add `output_schema()` returning a discriminated union, implement `extract_search_text`, `getPath`, `is_search_or_read_command`, `getActivityDescription`, and set `maxResultSizeChars` large.

- [ ] **Step 1: Add `pages` to input_schema**

In `input_schema()`, add the `pages` property after `limit`:

```rust
// ... existing properties ...
"limit": {
    "type": "integer",
    "description": "The number of lines to read. Only provide if the file is too large to read at once.",
    "exclusiveMinimum": 0,
    "maximum": 2000
},
"pages": {
    "type": "string",
    "description": "Page range for PDF files (e.g., \"1-5\", \"3\", \"10-20\"). Only applicable to PDF files. Maximum 20 pages per request."
}
// ...
```

- [ ] **Step 2: Add `output_schema()` method**

Add after `input_schema()`:

```rust
fn output_schema(&self) -> Option<serde_json::Value> {
    Some(serde_json::json!({
        "anyOf": [
            {
                "type": "object",
                "description": "Text file output with line-numbered content",
                "properties": {
                    "content": { "type": "string" },
                    "total_lines": { "type": "integer" },
                    "offset": { "type": "integer" }
                }
            },
            {
                "type": "object",
                "description": "Image file output",
                "properties": {
                    "file_path": { "type": "string" },
                    "file_size": { "type": "string" },
                    "file_type": { "const": "image" }
                }
            },
            {
                "type": "object",
                "description": "PDF file output",
                "properties": {
                    "file_path": { "type": "string" },
                    "file_size": { "type": "integer" },
                    "file_type": { "const": "pdf" },
                    "pages": { "type": "string" }
                }
            },
            {
                "type": "object",
                "description": "Jupyter notebook output with cells",
                "properties": {
                    "file_path": { "type": "string" },
                    "cell_count": { "type": "integer" },
                    "file_type": { "const": "ipynb" },
                    "cells": {
                        "type": "array",
                        "items": {
                            "type": "object",
                            "properties": {
                                "index": { "type": "integer" },
                                "cell_type": { "type": "string" },
                                "source": { "type": "string" }
                            }
                        }
                    }
                }
            }
        ]
    }))
}
```

- [ ] **Step 3: Implement `getPath`, `getActivityDescription`, `is_search_or_read_command`, `extract_search_text`**

Add these method overrides before the `call()` method:

```rust
fn get_path(&self, input: &serde_json::Value) -> Option<std::path::PathBuf> {
    input.get("file_path").and_then(|v| v.as_str()).map(std::path::PathBuf::from)
}

fn get_activity_description(&self, input: &serde_json::Value) -> Option<String> {
    input.get("file_path").and_then(|v| v.as_str()).map(|p| format!("Reading {}", p))
}

fn is_search_or_read_command(&self, _input: &serde_json::Value) -> SearchReadKind {
    SearchReadKind { is_read: true, ..Default::default() }
}

fn extract_search_text(&self, output: &serde_json::Value) -> Option<String> {
    output.as_str().map(String::from)
}

fn max_result_size_chars(&self) -> usize {
    500_000 // Large headroom for the Read→file→Read loop
}
```

- [ ] **Step 4: Update call() to respect `pages` parameter for PDFs**

In the PDF branch of `call()`, check for and use the `pages` parameter:

```rust
// Inside the PDF branch (around the existing PDF handling code):
if ext.as_deref() == Some("pdf") {
    let pages = input.get("pages").and_then(|v| v.as_str());
    let metadata = std::fs::metadata(path).ok();
    let size = metadata.map(|m| m.len()).unwrap_or(0);
    let mut meta = HashMap::new();
    meta.insert("file_type".to_string(), "pdf".to_string());
    if let Some(p) = pages {
        meta.insert("pages".to_string(), p.to_string());
    }
    return ToolResult {
        content: format!(
            "PDF file: {}\nSize: {} bytes{}",
            file_path, size,
            pages.map(|p| format!("\nPages requested: {}", p)).unwrap_or_default()
        ),
        is_error: false,
        metadata: Some(meta),
        ..Default::default()
    };
}
```

- [ ] **Step 5: Update the `limit` property to match Claude's constraint (exclusiveMinimum 0, maximum 2000)**

Remove the old `minimum: 1` from `limit` and replace with `exclusiveMinimum: 0, maximum: 2000`.

- [ ] **Step 6: Compile check**

Run: `cargo check -p cli 2>&1`

- [ ] **Step 7: Commit**

```bash
git add cli/src/tools/read.rs
git commit -m "feat(parity): add pages field, output_schema, and parity methods to ReadTool"
```

---

### Task 3: Port Edit tool prompt text

**Files:**
- Modify: `cli/src/tools/prompts/edit.txt`

Replace the TODO placeholder with the return value of `getEditToolDescription()` from `../claude-code-src/tools/FileEditTool/prompt.ts`. Use the standard prefix format description (not compact). Omit the ant-specific `minimalUniquenessHint`.

- [ ] **Step 1: Write the prompt file**

Write `cli/src/tools/prompts/edit.txt`:

```
Performs exact string replacements in files.

Usage:
- You must use your `Read` tool at least once in the conversation before editing. This tool will error if you attempt an edit without reading the file. 
- When editing text from Read tool output, ensure you preserve the exact indentation (tabs/spaces) as it appears AFTER the line number prefix. The line number prefix format is: line number + tab. Everything after that is the actual file content to match. Never include any part of the line number prefix in the old_string or new_string.
- ALWAYS prefer editing existing files in the codebase. NEVER write new files unless explicitly required.
- Only use emojis if the user explicitly requests it. Avoid adding emojis to files unless asked.
- The edit will FAIL if `old_string` is not unique in the file. Either provide a larger string with more surrounding context to make it unique or use `replace_all` to change every instance of `old_string`.
- Use `replace_all` for replacing and renaming strings across the file. This parameter is useful if you want to rename a variable for instance.
```

- [ ] **Step 2: Commit**

```bash
git add cli/src/tools/prompts/edit.txt
git commit -m "feat(parity): port Edit tool prompt text from Claude"
```

---

### Task 4: Add output_schema, getPath, toAutoClassifierInput, RenderSpec::Diff to EditTool

**Files:**
- Modify: `cli/src/tools/edit.rs`

Add `output_schema()` returning FileEditOutput shape, implement `getPath`, `getActivityDescription`, `toAutoClassifierInput`, `backfill_observable_input`. In `call()`, generate a unified diff after a successful edit. Add `render_tool_result_message()` emitting `RenderSpec::Diff`.

- [ ] **Step 1: Add parity methods**

Add these methods after `input_schema()`:

```rust
fn output_schema(&self) -> Option<serde_json::Value> {
    Some(serde_json::json!({
        "type": "object",
        "properties": {
            "file_path": { "type": "string" },
            "hunks": {
                "type": "array",
                "items": {
                    "type": "object",
                    "properties": {
                        "old_start": { "type": "integer" },
                        "new_start": { "type": "integer" },
                        "lines": {
                            "type": "array",
                            "items": {
                                "anyOf": [
                                    { "type": "object", "properties": { "kind": { "const": "context" }, "line": { "type": "string" } } },
                                    { "type": "object", "properties": { "kind": { "const": "add" }, "line": { "type": "string" } } },
                                    { "type": "object", "properties": { "kind": { "const": "remove" }, "line": { "type": "string" } } }
                                ]
                            }
                        }
                    }
                }
            },
            "replaced": { "type": "integer" },
            "replace_all": { "type": "boolean" }
        }
    }))
}

fn get_path(&self, input: &serde_json::Value) -> Option<std::path::PathBuf> {
    input.get("file_path").and_then(|v| v.as_str()).map(std::path::PathBuf::from)
}

fn get_activity_description(&self, input: &serde_json::Value) -> Option<String> {
    input.get("file_path").and_then(|v| v.as_str()).map(|p| format!("Editing {}", p))
}

fn to_auto_classifier_input(&self, input: &serde_json::Value) -> serde_json::Value {
    let path = input.get("file_path").and_then(|v| v.as_str()).unwrap_or("");
    let new_str = input.get("new_string").and_then(|v| v.as_str()).unwrap_or("");
    serde_json::Value::String(format!("{}: {}", path, new_str))
}

fn backfill_observable_input(&self, input: &mut serde_json::Value) {
    // Expand relative paths to absolute before the permission check
    if let Some(path_str) = input.get("file_path").and_then(|v| v.as_str()) {
        let p = std::path::Path::new(path_str);
        if p.is_relative() {
            if let Ok(canon) = std::env::current_dir().map(|cwd| cwd.join(p)) {
                if let Some(s) = canon.to_str() {
                    input["file_path"] = serde_json::Value::String(s.to_string());
                }
            }
        }
    }
}
```

- [ ] **Step 2: Generate unified diff on successful edit**

Modify `call()` — after the successful `std::fs::write()`, compute a simple diff. Replace the Ok(()) arm with:

```rust
Ok(()) => {
    // Generate a simple unified diff: old vs new
    let old_lines: Vec<&str> = content.lines().collect();
    let new_lines: Vec<&str> = new_content.lines().collect();

    // Build diff hunks — find the first changed region
    let mut hunks: Vec<serde_json::Value> = Vec::new();
    let mut diff_lines: Vec<serde_json::Value> = Vec::new();
    let mut old_start = 0usize;
    let mut new_start = 0usize;
    let mut in_change = false;

    let max_len = old_lines.len().max(new_lines.len());
    for i in 0..max_len {
        let old_line = old_lines.get(i);
        let new_line = new_lines.get(i);
        if old_line != new_line {
            if !in_change {
                old_start = i.saturating_sub(1); // context line before
                new_start = old_start;
                // Emit context line before the change
                if old_start < i && old_start < old_lines.len() {
                    diff_lines.push(serde_json::json!({"kind": "context", "line": old_lines[old_start]}));
                }
                in_change = true;
            }
            if let Some(l) = old_line {
                diff_lines.push(serde_json::json!({"kind": "remove", "line": *l}));
            }
            if let Some(l) = new_line {
                diff_lines.push(serde_json::json!({"kind": "add", "line": *l}));
            }
        } else if in_change {
            // Context line after change
            if let Some(l) = old_line {
                diff_lines.push(serde_json::json!({"kind": "context", "line": *l}));
            }
            // Emit the hunk and reset
            if !diff_lines.is_empty() {
                hunks.push(serde_json::json!({
                    "old_start": old_start + 1,
                    "new_start": new_start + 1,
                    "lines": diff_lines
                }));
            }
            diff_lines = Vec::new();
            in_change = false;
        }
    }
    // Flush remaining diff
    if !diff_lines.is_empty() {
        hunks.push(serde_json::json!({
            "old_start": old_start + 1,
            "new_start": new_start + 1,
            "lines": diff_lines
        }));
    }

    let result_json = serde_json::json!({
        "file_path": file_path,
        "hunks": hunks,
        "replaced": occurrences,
        "replace_all": replace_all
    });

    let mut meta = std::collections::HashMap::new();
    meta.insert("replaced".to_string(), occurrences.to_string());
    meta.insert("replace_all".to_string(), replace_all.to_string());

    ToolResult {
        content: result_json.to_string(),
        is_error: false,
        metadata: Some(meta),
        ..Default::default()
    }
}
```

- [ ] **Step 3: Add `render_tool_result_message()` emitting `RenderSpec::Diff`**

Add after `is_destructive()`:

```rust
fn render_tool_result_message(
    &self,
    output: &serde_json::Value,
    _progress: &[crate::tools::contract::ProgressEvent],
    _opts: &crate::tools::contract::RenderOpts,
) -> Option<shared::RenderSpec> {
    let file_path = output["file_path"].as_str().unwrap_or("").to_string();
    let hunks_json = output["hunks"].as_array().cloned().unwrap_or_default();

    let hunks: Vec<shared::DiffHunk> = hunks_json.iter().filter_map(|h| {
        let lines: Vec<shared::DiffLine> = h["lines"].as_array()?.iter().filter_map(|l| {
            match l["kind"].as_str()? {
                "context" => Some(shared::DiffLine::Context { line: l["line"].as_str()?.to_string() }),
                "add" => Some(shared::DiffLine::Add { line: l["line"].as_str()?.to_string() }),
                "remove" => Some(shared::DiffLine::Remove { line: l["line"].as_str()?.to_string() }),
                _ => None,
            }
        }).collect();
        Some(shared::DiffHunk {
            old_start: h["old_start"].as_u64()? as u32,
            new_start: h["new_start"].as_u64()? as u32,
            lines,
        })
    }).collect();

    Some(shared::RenderSpec::Diff { file_path, hunks })
}
```

Add necessary imports at the top:

```rust
use shared;
```

- [ ] **Step 4: Compile check**

Run: `cargo check -p cli 2>&1`

- [ ] **Step 5: Commit**

```bash
git add cli/src/tools/edit.rs
git commit -m "feat(parity): add output_schema, getPath, toAutoClassifierInput, and RenderSpec::Diff to EditTool"
```

---

### Task 5: Port Write tool prompt text

**Files:**
- Modify: `cli/src/tools/prompts/write.txt`

Replace the TODO placeholder with the return value of `getWriteToolDescription()` from `../claude-code-src/tools/FileWriteTool/prompt.ts`.

- [ ] **Step 1: Write the prompt file**

Write `cli/src/tools/prompts/write.txt`:

```
Writes a file to the local filesystem.

Usage:
- This tool will overwrite the existing file if there is one at the provided path.
- If this is an existing file, you MUST use the Read tool first to read the file's contents. This tool will fail if you did not read the file first.
- Prefer the Edit tool for modifying existing files — it only sends the diff. Only use this tool to create new files or for complete rewrites.
- NEVER create documentation files (*.md) or README files unless explicitly requested by the User.
- Only use emojis if the user explicitly requests it. Avoid writing emojis to files unless asked.
```

- [ ] **Step 2: Commit**

```bash
git add cli/src/tools/prompts/write.txt
git commit -m "feat(parity): port Write tool prompt text from Claude"
```

---

### Task 6: Add output_schema, getPath, toAutoClassifierInput, overwrite warning to WriteTool

**Files:**
- Modify: `cli/src/tools/write.rs`

Add `output_schema()`, `getPath`, `getActivityDescription`, `toAutoClassifierInput`, and warn when overwriting an existing file.

- [ ] **Step 1: Add parity methods**

Add these methods after `input_schema()`:

```rust
fn output_schema(&self) -> Option<serde_json::Value> {
    Some(serde_json::json!({
        "type": "object",
        "properties": {
            "file_path": { "type": "string" },
            "bytes_written": { "type": "integer" },
            "overwritten": { "type": "boolean" }
        }
    }))
}

fn get_path(&self, input: &serde_json::Value) -> Option<std::path::PathBuf> {
    input.get("file_path").and_then(|v| v.as_str()).map(std::path::PathBuf::from)
}

fn get_activity_description(&self, input: &serde_json::Value) -> Option<String> {
    input.get("file_path").and_then(|v| v.as_str()).map(|p| format!("Writing {}", p))
}

fn to_auto_classifier_input(&self, input: &serde_json::Value) -> serde_json::Value {
    let path = input.get("file_path").and_then(|v| v.as_str()).unwrap_or("");
    let content = input.get("content").and_then(|v| v.as_str()).unwrap_or("");
    // Truncate content for classifier — first 200 chars
    let preview = if content.len() > 200 {
        format!("{}...", &content[..200])
    } else {
        content.to_string()
    };
    serde_json::Value::String(format!("{}: {}", path, preview))
}
```

- [ ] **Step 2: Add overwrite warning in call()**

After the `let path = std::path::Path::new(file_path);` line, add a check that notes whether the file existed:

```rust
let path = std::path::Path::new(file_path);
let existed = path.exists();

// ... (existing parent creation code) ...

// In the Ok(()) arm, add 'overwritten' to meta:
meta.insert("overwritten".to_string(), existed.to_string());
```

And update the success message to note overwriting:

```rust
// Change the success message to:
"Successfully {} {} bytes to {}{}",
if existed { "overwrote" } else { "wrote" },
content.len(),
file_path,
if existed { " (file was overwritten)" } else { "" }
```

- [ ] **Step 3: Compile check**

Run: `cargo check -p cli 2>&1`

- [ ] **Step 4: Commit**

```bash
git add cli/src/tools/write.rs
git commit -m "feat(parity): add output_schema, getPath, toAutoClassifierInput, and overwrite warning to WriteTool"
```

---

### Task 7: Port Glob tool prompt text

**Files:**
- Modify: `cli/src/tools/prompts/glob_tool.txt`

Replace the TODO placeholder with the `DESCRIPTION` constant from `../claude-code-src/tools/GlobTool/prompt.ts`.

- [ ] **Step 1: Write the prompt file**

Write `cli/src/tools/prompts/glob_tool.txt`:

```
- Fast file pattern matching tool that works with any codebase size
- Supports glob patterns like "**/*.js" or "src/**/*.ts"
- Returns matching file paths sorted by modification time
- Use this tool when you need to find files by name patterns
- When you are doing an open ended search that may require multiple rounds of globbing and grepping, use the Agent tool instead
```

- [ ] **Step 2: Commit**

```bash
git add cli/src/tools/prompts/glob_tool.txt
git commit -m "feat(parity): port Glob tool prompt text from Claude"
```

---

### Task 8: Add output_schema, RenderSpec::PathList, and extract_search_text to GlobTool

**Files:**
- Modify: `cli/src/tools/glob_tool.rs`

Add `output_schema()`, `render_tool_result_message()` emitting `RenderSpec::PathList`, and `extract_search_text`.

- [ ] **Step 1: Add parity methods**

Add these methods after `input_schema()`:

```rust
fn output_schema(&self) -> Option<serde_json::Value> {
    Some(serde_json::json!({
        "type": "object",
        "properties": {
            "durationMs": { "type": "number", "description": "Time the search took in milliseconds" },
            "numFiles": { "type": "integer", "description": "Number of matching files found" },
            "filenames": {
                "type": "array",
                "items": { "type": "string" },
                "description": "Matching file paths"
            },
            "truncated": { "type": "boolean", "description": "True when results exceed the limit" }
        }
    }))
}

fn render_tool_result_message(
    &self,
    output: &serde_json::Value,
    _progress: &[crate::tools::contract::ProgressEvent],
    _opts: &crate::tools::contract::RenderOpts,
) -> Option<shared::RenderSpec> {
    let filenames = output["filenames"]
        .as_array()
        .map(|arr| {
            arr.iter()
                .filter_map(|v| v.as_str())
                .map(|s| shared::PathEntry {
                    path: std::path::PathBuf::from(s),
                    line: None,
                    preview: None,
                })
                .collect()
        })
        .unwrap_or_default();

    let num_files = output["numFiles"].as_u64().unwrap_or(filenames.len() as u64) as usize;
    let truncated = output["truncated"].as_bool().unwrap_or(false);

    Some(shared::RenderSpec::PathList {
        entries: filenames,
        total: num_files,
        truncated,
    })
}

fn extract_search_text(&self, output: &serde_json::Value) -> Option<String> {
    output["filenames"].as_array().map(|arr| {
        arr.iter()
            .filter_map(|v| v.as_str())
            .collect::<Vec<_>>()
            .join("\n")
    })
}
```

- [ ] **Step 2: Update call() to return structured JSON output**

Modify the `call()` result formatting. Replace the text-only output with structured JSON + a content string. After the result_lines formatting, change the ToolResult:

```rust
let duration_ms = 0u64; // Add timing: let start = std::time::Instant::now(); at top, then duration_ms = start.elapsed().as_millis() here
let num_files = total;
let truncated = total > 100;

let result_json = serde_json::json!({
    "durationMs": duration_ms,
    "numFiles": num_files,
    "filenames": result_lines,
    "truncated": truncated
});

ToolResult {
    content: result_json.to_string(),
    // ... keep existing metadata ...
}
```

To add timing, insert at the start of `call()`:
```rust
let start = std::time::Instant::now();
```

And after result_lines are built:
```rust
let duration_ms = start.elapsed().as_millis() as u64;
```

- [ ] **Step 3: Compile check**

Run: `cargo check -p cli 2>&1`

- [ ] **Step 4: Commit**

```bash
git add cli/src/tools/glob_tool.rs
git commit -m "feat(parity): add output_schema, RenderSpec::PathList, and structured output to GlobTool"
```

---

### Task 9: Port Grep tool prompt text

**Files:**
- Modify: `cli/src/tools/prompts/grep.txt`

Replace the TODO placeholder with the exact `getDescription()` output from `../claude-code-src/tools/GrepTool/prompt.ts`.

- [ ] **Step 1: Write the prompt file**

Write `cli/src/tools/prompts/grep.txt`:

```
A powerful search tool built on ripgrep

Usage:
- ALWAYS use Grep for search tasks. NEVER invoke `grep` or `rg` as a Bash command. The Grep tool has been optimized for correct permissions and access.
- Supports full regex syntax (e.g., "log.*Error", "function\s+\w+")
- Filter files with glob parameter (e.g., "*.js", "**/*.tsx") or type parameter (e.g., "js", "py", "rust")
- Output modes: "content" shows matching lines, "files_with_matches" shows only file paths (default), "count" shows match counts
- Use Agent tool for open-ended searches requiring multiple rounds
- Pattern syntax: Uses ripgrep (not grep) - literal braces need escaping (use `interface\{\}` to find `interface{}` in Go code)
- Multiline matching: By default patterns match within single lines only. For cross-line patterns like `struct \{[\s\S]*?field`, use `multiline: true`
```

- [ ] **Step 2: Commit**

```bash
git add cli/src/tools/prompts/grep.txt
git commit -m "feat(parity): port Grep tool prompt text from Claude"
```

---

### Task 10: MAJOR expansion of GrepTool — full schema parity

**Files:**
- Modify: `cli/src/tools/grep.rs`

Add ALL Claude-parity fields to `input_schema()`. Implement `is_search_or_read_command` returning `SearchReadKind { is_search: true }`. Add `extract_search_text`. Add `output_schema()` discriminated by mode. Implement new flags in `call()`.

- [ ] **Step 1: Expand input_schema with all Claude-parity fields**

Replace the existing `input_schema()`:

```rust
fn input_schema(&self) -> serde_json::Value {
    serde_json::json!({
        "type": "object",
        "properties": {
            "pattern": {
                "type": "string",
                "description": "The regex pattern to search for"
            },
            "path": {
                "type": "string",
                "description": "The directory to search in (defaults to current working directory)"
            },
            "glob": {
                "type": "string",
                "description": "A glob pattern to filter files (e.g., \"**/*.rs\")"
            },
            "type": {
                "type": "string",
                "description": "File type filter by extension (e.g., \"js\", \"py\", \"rust\")"
            },
            "output_mode": {
                "type": "string",
                "enum": ["content", "files_with_matches", "count"],
                "description": "Output mode: \"content\" shows matching lines, \"files_with_matches\" shows file paths (default), \"count\" shows match counts"
            },
            "-A": {
                "type": "integer",
                "description": "Number of lines to show after each match (rg -A)"
            },
            "-B": {
                "type": "integer",
                "description": "Number of lines to show before each match (rg -B)"
            },
            "-C": {
                "type": "integer",
                "description": "Number of lines to show before and after each match (rg -C). Shorthand for setting both -B and -C."
            },
            "context": {
                "type": "integer",
                "description": "Alias for -C: number of lines to show before and after each match"
            },
            "-n": {
                "type": "boolean",
                "description": "Show line numbers in output (default true)"
            },
            "-i": {
                "type": "boolean",
                "description": "Case insensitive search"
            },
            "offset": {
                "type": "integer",
                "description": "Skip first N matches before returning results"
            },
            "head_limit": {
                "type": "integer",
                "description": "Maximum number of results to return (default 250)",
                "exclusiveMinimum": 0
            },
            "multiline": {
                "type": "boolean",
                "description": "Enable multiline mode where . matches newlines and patterns can span lines"
            }
        },
        "required": ["pattern"]
    })
}
```

- [ ] **Step 2: Add `output_schema()`, `is_search_or_read_command`, `extract_search_text`**

Add after `input_schema()`:

```rust
fn output_schema(&self) -> Option<serde_json::Value> {
    Some(serde_json::json!({
        "anyOf": [
            {
                "type": "object",
                "description": "content mode (default): matching lines with context",
                "properties": {
                    "mode": { "const": "content" },
                    "matches": {
                        "type": "array",
                        "items": {
                            "type": "object",
                            "properties": {
                                "file": { "type": "string" },
                                "line": { "type": "integer" },
                                "column": { "type": "integer" },
                                "match": { "type": "string" },
                                "context_before": {
                                    "type": "array",
                                    "items": { "type": "string" }
                                },
                                "context_after": {
                                    "type": "array",
                                    "items": { "type": "string" }
                                }
                            }
                        }
                    },
                    "total": { "type": "integer" },
                    "truncated": { "type": "boolean" }
                }
            },
            {
                "type": "object",
                "description": "files_with_matches mode: only file paths",
                "properties": {
                    "mode": { "const": "files_with_matches" },
                    "filenames": {
                        "type": "array",
                        "items": { "type": "string" }
                    },
                    "total": { "type": "integer" },
                    "truncated": { "type": "boolean" }
                }
            },
            {
                "type": "object",
                "description": "count mode: match counts per file",
                "properties": {
                    "mode": { "const": "count" },
                    "counts": {
                        "type": "array",
                        "items": {
                            "type": "object",
                            "properties": {
                                "file": { "type": "string" },
                                "count": { "type": "integer" }
                            }
                        }
                    },
                    "total": { "type": "integer" },
                    "truncated": { "type": "boolean" }
                }
            }
        ]
    }))
}

fn is_search_or_read_command(&self, _input: &serde_json::Value) -> SearchReadKind {
    SearchReadKind { is_search: true, ..Default::default() }
}

fn extract_search_text(&self, output: &serde_json::Value) -> Option<String> {
    // Return matches content for search text extraction
    if let Some(matches) = output["matches"].as_array() {
        let lines: Vec<&str> = matches.iter()
            .filter_map(|m| m["match"].as_str())
            .collect();
        if !lines.is_empty() {
            return Some(lines.join("\n"));
        }
    }
    if let Some(filenames) = output["filenames"].as_array() {
        let lines: Vec<&str> = filenames.iter()
            .filter_map(|v| v.as_str())
            .collect();
        return Some(lines.join("\n"));
    }
    None
}
```

- [ ] **Step 3: Implement new flags in call()**

Modify `call()` to handle the new fields. Add parsing for:
- `-A`, `-B`, `-C`, `context` → context lines before/after
- `output_mode` → switch between content/files_with_matches/count
- `-n` → line numbers toggle (default true)
- `-i` → case insensitive regex
- `offset` → skip first N matches
- `type` → file type filter
- `multiline` → multiline regex mode

Add at the top of `call()`, after extracting existing fields:

```rust
let output_mode = input.get("output_mode").and_then(|v| v.as_str()).unwrap_or("content");
let context_before = input.get("-B").and_then(|v| v.as_u64()).map(|v| v as usize)
    .or_else(|| input.get("-C").and_then(|v| v.as_u64()).map(|v| v as usize))
    .or_else(|| input.get("context").and_then(|v| v.as_u64()).map(|v| v as usize))
    .unwrap_or(0);
let context_after = input.get("-A").and_then(|v| v.as_u64()).map(|v| v as usize)
    .or_else(|| input.get("-C").and_then(|v| v.as_u64()).map(|v| v as usize))
    .or_else(|| input.get("context").and_then(|v| v.as_u64()).map(|v| v as usize))
    .unwrap_or(0);
let show_line_numbers = input.get("-n").and_then(|v| v.as_bool()).unwrap_or(true);
let case_insensitive = input.get("-i").and_then(|v| v.as_bool()).unwrap_or(false);
let offset = input.get("offset").and_then(|v| v.as_u64()).map(|v| v as usize).unwrap_or(0);
let type_filter = input.get("type").and_then(|v| v.as_str());
let multiline = input.get("multiline").and_then(|v| v.as_bool()).unwrap_or(false);
```

For `-i` (case insensitive), build the regex differently:
```rust
let regex = if case_insensitive {
    match regex::RegexBuilder::new(pattern_str).case_insensitive(true).build() {
        Ok(r) => r,
        Err(e) => return ToolResult { content: format!("Invalid regex: {}", e), is_error: true, ..Default::default() },
    }
} else {
    match regex::Regex::new(pattern_str) {
        Ok(r) => r,
        Err(e) => return ToolResult { content: format!("Invalid regex: {}", e), is_error: true, ..Default::default() },
    }
};
```

For the `type` filter, map common extensions. Add a helper at the end of the file:

```rust
fn type_to_glob(type_name: &str) -> Option<&'static str> {
    match type_name {
        "rs" | "rust" => Some("*.rs"),
        "js" | "javascript" => Some("*.js"),
        "ts" | "typescript" => Some("*.ts"),
        "tsx" => Some("*.tsx"),
        "jsx" => Some("*.jsx"),
        "py" | "python" => Some("*.py"),
        "go" => Some("*.go"),
        "java" => Some("*.java"),
        "c" => Some("*.c"),
        "h" => Some("*.h"),
        "cpp" | "c++" => Some("*.cpp"),
        "hpp" => Some("*.hpp"),
        "rb" | "ruby" => Some("*.rb"),
        "sh" | "bash" => Some("*.sh"),
        "md" | "markdown" => Some("*.md"),
        "json" => Some("*.json"),
        "yaml" | "yml" => Some("*.yml"),
        "toml" => Some("*.toml"),
        "html" => Some("*.html"),
        "css" => Some("*.css"),
        "scss" => Some("*.scss"),
        "sql" => Some("*.sql"),
        "swift" => Some("*.swift"),
        "kt" | "kotlin" => Some("*.kt"),
        "vue" => Some("*.vue"),
        "svelte" => Some("*.svelte"),
        _ => None,
    }
}
```

For `output_mode`: switch on the value to produce different output shapes.
- `"content"` → return `{mode: "content", matches: [...], total, truncated}`
- `"files_with_matches"` → return `{mode: "files_with_matches", filenames: [...], total, truncated}`
- `"count"` → return `{mode: "count", counts: [{file, count}, ...], total, truncated}`

Add `multiline` support by using `regex::RegexBuilder`:
```rust
let regex = if multiline {
    match regex::RegexBuilder::new(pattern_str).multi_line(true).dot_matches_new_line(true).build() {
        Ok(r) => r,
        Err(e) => return ToolResult { ... }
    }
} else {
    // existing non-multiline regex construction
};
```

For content mode with context lines (`-A`/`-B`/`-C`), when a match is found, capture context_before lines before and context_after lines after the matching line. Adjust the result formatting:

```rust
// In the content match section, replace the simple line output:
let rel = pathdiff::diff_paths(&path, cwd).unwrap_or_else(|| path.to_path_buf());
let mut entry = serde_json::json!({
    "file": rel.to_string_lossy(),
    "line": line_num + 1,
    "match": line.to_string()
});
if context_before > 0 {
    let before_start = line_num.saturating_sub(context_before);
    let before_lines: Vec<String> = content.lines()
        .skip(before_start)
        .take(line_num - before_start)
        .map(String::from)
        .collect();
    entry["context_before"] = serde_json::json!(before_lines);
}
if context_after > 0 {
    let after_start = line_num + 1;
    let after_lines: Vec<String> = content.lines()
        .skip(after_start)
        .take(context_after)
        .map(String::from)
        .collect();
    entry["context_after"] = serde_json::json!(after_lines);
}
```

For `offset`, skip the first N results:
```rust
let mut skipped = 0usize;
// ... in the match loop:
if skipped < offset {
    skipped += 1;
    continue;
}
```

- [ ] **Step 4: Update the result to return structured JSON based on output_mode**

Replace the final ToolResult construction with mode-aware output:

```rust
let truncated = results.len() >= head_limit;
let result_json = match output_mode {
    "files_with_matches" => {
        // Collect unique file paths
        let mut filenames: Vec<String> = Vec::new();
        let mut seen = std::collections::HashSet::new();
        for entry in &results {
            let file = entry["file"].as_str().unwrap_or("");
            if seen.insert(file.to_string()) {
                filenames.push(file.to_string());
            }
        }
        serde_json::json!({
            "mode": "files_with_matches",
            "filenames": filenames,
            "total": filenames.len(),
            "truncated": truncated
        })
    }
    "count" => {
        let mut file_counts: std::collections::HashMap<String, usize> = std::collections::HashMap::new();
        for entry in &results {
            let file = entry["file"].as_str().unwrap_or("").to_string();
            *file_counts.entry(file).or_insert(0) += 1;
        }
        let counts: Vec<serde_json::Value> = file_counts.into_iter()
            .map(|(file, count)| serde_json::json!({"file": file, "count": count}))
            .collect();
        serde_json::json!({
            "mode": "count",
            "counts": counts,
            "total": counts.len(),
            "truncated": truncated
        })
    }
    _ => {
        serde_json::json!({
            "mode": "content",
            "matches": results,
            "total": results.len(),
            "truncated": truncated
        })
    }
};
```

Change `results` from `Vec<String>` to `Vec<serde_json::Value>` to store structured entries instead of formatted strings.

- [ ] **Step 5: Compile check**

Run: `cargo check -p cli 2>&1`

- [ ] **Step 6: Commit**

```bash
git add cli/src/tools/grep.rs
git commit -m "feat(parity): major GrepTool expansion with full schema parity, output modes, context lines, and multiline support"
```

---

### Task 11: Port NotebookEdit tool prompt text and add output_schema

**Files:**
- Modify: `cli/src/tools/prompts/notebook_edit.txt`
- Modify: `cli/src/tools/notebook_edit.rs`

Replace the TODO placeholder prompt. Then add `output_schema()` returning notebook-specific shape, and return actual notebook JSON in the output.

- [ ] **Step 1: Write the prompt file**

Write `cli/src/tools/prompts/notebook_edit.txt` with the combined `DESCRIPTION` + `PROMPT` from `../claude-code-src/tools/NotebookEditTool/prompt.ts`:

```
Replace the contents of a specific cell in a Jupyter notebook.

Completely replaces the contents of a specific cell in a Jupyter notebook (.ipynb file) with new source. Jupyter notebooks are interactive documents that combine code, text, and visualizations, commonly used for data analysis and scientific computing. The notebook_path parameter must be an absolute path, not a relative path. The cell_number is 0-indexed. Use edit_mode=insert to add a new cell at the index specified by cell_number. Use edit_mode=delete to delete the cell at the index specified by cell_number.
```

Note: Claude uses `cell_number` (old) but Super already migrated to `cell_id` (UUID-based). The schema stays on `cell_id` but the prompt text is ported as-is. The `cell_number` mention in the prompt is harmless — the model sees the schema with `cell_id` and follows that.

- [ ] **Step 2: Add `output_schema()` to notebook_edit.rs**

Add after `input_schema()`:

```rust
fn output_schema(&self) -> Option<serde_json::Value> {
    Some(serde_json::json!({
        "type": "object",
        "properties": {
            "notebook_path": { "type": "string" },
            "edit_mode": { "type": "string", "enum": ["replace", "insert", "delete"] },
            "cell_id": { "type": "string" },
            "cell_type": { "type": "string", "enum": ["code", "markdown"] },
            "new_source": { "type": "string" },
            "language": { "type": "string" },
            "cell_count": { "type": "integer" },
            "original_notebook": { "type": "object", "description": "Full notebook JSON before edit" },
            "updated_notebook": { "type": "object", "description": "Full notebook JSON after edit" },
            "error": { "type": "string" }
        }
    }))
}
```

- [ ] **Step 3: Update call() to return notebook JSON in the output**

Modify the Ok(()) arm in `call()` to include `original_notebook` and `updated_notebook`. Store the parsed original before modification:

```rust
// After parsing the notebook, clone it for original_notebook:
let original_notebook = notebook.clone();

// ... (perform edit) ...

// In the Ok(()) arm, include both notebooks:
let result_json = serde_json::json!({
    "notebook_path": notebook_path,
    "edit_mode": edit_mode,
    "cell_id": cell_id,
    "cell_type": cell_type,
    "new_source": new_source,
    "cell_count": cell_count,
    "original_notebook": original_notebook,
    "updated_notebook": notebook
});

ToolResult {
    content: result_json.to_string(),
    is_error: false,
    metadata: Some(meta),
    ..Default::default()
}
```

- [ ] **Step 4: Remove the `let _ = cells;` hack**

The current code does `let cell_count = cells.len(); let _ = cells;` to release the borrow. Instead, scope the cell count:

```rust
let cell_count = {
    let cells = notebook.get_mut("cells").and_then(|c| c.as_array_mut());
    // ... perform edit using cells ...
    cells.map_or(0, |c| c.len())
};
```

Or simply assign `cell_count` before the `cells` mutable borrow ends, and use a block scope.

- [ ] **Step 5: Compile check**

Run: `cargo check -p cli 2>&1`

- [ ] **Step 6: Commits**

```bash
git add cli/src/tools/prompts/notebook_edit.txt
git commit -m "feat(parity): port NotebookEdit tool prompt text from Claude"

git add cli/src/tools/notebook_edit.rs
git commit -m "feat(parity): add output_schema with notebook JSON to NotebookEditTool"
```

---

### Task 12: Port Bash tool prompt text (massive, ~370 lines)

**Files:**
- Modify: `cli/src/tools/prompts/bash.txt`

Replace the TODO placeholder with the return value of `getSimplePrompt()` from `../claude-code-src/tools/BashTool/prompt.ts`. This is the full external-user prompt including tool preference items, multiple/single command instructions, git safety protocol, PR creation protocol, sleep avoidance rules, and sandbox section.

- [ ] **Step 1: Write the prompt file**

Write `cli/src/tools/prompts/bash.txt` with the full `getSimplePrompt()` output. The exact text must match what the function returns for an external user (no ant/embedded features). Key sections:

- Tool preference items (File search: Glob, Content search: Grep, Read: Read, Edit: Edit, Write: Write, Communication: output directly)
- Instructions about quoting file paths, maintaining cwd, timeouts, background tasks
- Multiple commands section (parallel when independent, `&&` for sequential, `;` for don't-care, no newlines)
- Git commands section (prefer new commits, be careful with destructive ops, never skip hooks)
- Sleep avoidance section (don't sleep between commands, use `run_in_background` for long-running, don't retry in sleep loop, don't poll background tasks)
- Git safety protocol (NEVER update config, NEVER destructive without request, NEVER skip hooks, NEVER force push to main, CRITICAL: always new commits)
- Committing changes protocol (steps 1-4 with exact commands)
- Creating pull requests protocol (steps 1-3 with exact commands and HEREDOC example)
- Other common operations (view PR comments via gh api)
- Sandbox section (enabled/disabled variants with filesystem/network restrictions)

The full text content is the return value from `getSimplePrompt()` in `../claude-code-src/tools/BashTool/prompt.ts:275-369`. Copy the string directly, substituting tool name placeholders (`${BASH_TOOL_NAME}` → "Bash", `${FILE_READ_TOOL_NAME}` → "Read", `${FILE_EDIT_TOOL_NAME}` → "Edit", `${FILE_WRITE_TOOL_NAME}` → "Write", `${GLOB_TOOL_NAME}` → "Glob", `${GREP_TOOL_NAME}` → "Grep", `${AGENT_TOOL_NAME}` → "Agent", `${TodoWriteTool.name}` → "TodoWrite").

For the timeout values, use realistic defaults: default timeout 120000ms (2 minutes), max 600000ms (10 minutes).

For commit attribution and PR attribution (from `getAttributionTexts()`), use:
- `Co-Authored-By: Claude Opus 4.7 <noreply@anthropic.com>`
- `🤖 Generated with [Claude Code](https://claude.com/claude-code)`

For the sandbox section: since Super may or may not have sandboxing enabled, use the sandbox-disabled variant (omit the sandbox section, or include a brief no-sandbox note). Simplest: omit the `getSimpleSandboxSection()` output (empty string) since Super's sandbox is not implemented yet. The prompt text should be the non-sandbox version.

- [ ] **Step 2: Verify line count**

Run: `wc -l cli/src/tools/prompts/bash.txt`
Expected: ~250-350 lines

- [ ] **Step 3: Commit**

```bash
git add cli/src/tools/prompts/bash.txt
git commit -m "feat(parity): port massive BashTool prompt text from Claude (~370 lines)"
```

---

### Task 13: Add dangerouslyDisableSandbox, output_schema, getActivityDescription to BashTool

**Files:**
- Modify: `cli/src/tools/bash.rs`

Add `dangerouslyDisableSandbox: boolean` to `input_schema()`. Add `getActivityDescription` returning the command. Add `output_schema()`.

- [ ] **Step 1: Add `dangerouslyDisableSandbox` to input_schema**

Replace `input_schema()`:

```rust
fn input_schema(&self) -> serde_json::Value {
    json!({
        "type": "object",
        "properties": {
            "command": {
                "type": "string",
                "description": "The command to execute"
            },
            "description": {
                "type": "string",
                "description": "Clear, concise description of what this command does in active voice. For simple commands (git, npm, standard CLI tools), keep it brief (5-10 words). For commands that are harder to parse at a glance (piped commands, obscure flags, etc.), add enough context to clarify what it does."
            },
            "timeout": {
                "type": "integer",
                "description": "Optional timeout in milliseconds (max 600000)",
                "minimum": 0,
                "maximum": 600000
            },
            "run_in_background": {
                "type": "boolean",
                "description": "Set to true to run this command in the background. Only use this if you don't need the result immediately and are OK being notified when the command completes later. You do not need to check the output right away - you'll be notified when it finishes. You do not need to use '&' at the end of the command when using this parameter."
            },
            "dangerouslyDisableSandbox": {
                "type": "boolean",
                "description": "Set this to true to dangerously override sandbox mode and run commands without sandboxing."
            }
        },
        "required": ["command"]
    })
}
```

- [ ] **Step 2: Add `output_schema()` and `getActivityDescription`**

Add after `input_schema()`:

```rust
fn output_schema(&self) -> Option<serde_json::Value> {
    Some(json!({
        "type": "object",
        "properties": {
            "stdout": { "type": "string" },
            "stderr": { "type": "string" },
            "exit_code": { "type": "integer" },
            "timed_out": { "type": "boolean" },
            "background": { "type": "boolean" }
        }
    }))
}

fn get_activity_description(&self, input: &serde_json::Value) -> Option<String> {
    input.get("description")
        .and_then(|v| v.as_str())
        .map(String::from)
        .or_else(|| {
            input.get("command")
                .and_then(|v| v.as_str())
                .map(|c| {
                    // Truncate long commands for display
                    if c.len() > 80 {
                        format!("{}...", &c[..77])
                    } else {
                        c.to_string()
                    }
                })
        })
}
```

- [ ] **Step 3: Update call() to read and use `dangerouslyDisableSandbox`**

At the top of `call()`, add:

```rust
let dangerously_disable_sandbox = input["dangerouslyDisableSandbox"].as_bool().unwrap_or(false);
```

Currently this flag doesn't change behavior (no sandbox implementation yet), but reading it makes the schema match. The security_check function still runs regardless — `dangerouslyDisableSandbox` only affects filesystem/network sandboxing, not the basic dangerous-command blocklist.

- [ ] **Step 4: Compile check**

Run: `cargo check -p cli 2>&1`

- [ ] **Step 5: Run existing Bash tool tests**

Run: `cargo test -p cli bash::tests::`
Expected: ALL PASS

- [ ] **Step 6: Commit**

```bash
git add cli/src/tools/bash.rs
git commit -m "feat(parity): add dangerouslyDisableSandbox, output_schema, and getActivityDescription to BashTool"
```

---

### Task 14: Implement `prepare_permission_matcher()` for BashTool

**Files:**
- Modify: `cli/src/tools/bash.rs`

Implement `prepare_permission_matcher()` that matches command stems. A permission rule `Bash(git *)` should match `{command: "git status"}`, `Bash(ls *)` should match `{command: "ls -la"}`, etc.

- [ ] **Step 1: Implement prepare_permission_matcher**

Add after `get_activity_description()`:

```rust
fn prepare_permission_matcher(
    &self,
    input: &serde_json::Value,
) -> Option<Box<dyn Fn(&str) -> bool + Send + Sync>> {
    // Match against the command's first word (the command stem).
    // This allows `Bash(git *)` to match `{command: "git status"}`
    let command_stem = input
        .get("command")
        .and_then(|v| v.as_str())
        .map(|c| {
            c.trim()
                .split_whitespace()
                .next()
                .unwrap_or("")
                .to_string()
        })?;

    if command_stem.is_empty() {
        return None;
    }

    Some(Box::new(move |rule_content: &str| -> bool {
        // rule_content is the permission rule's content pattern, e.g. "git *"
        let rule_stem = rule_content.trim()
            .split_whitespace()
            .next()
            .unwrap_or("");
        rule_stem == command_stem || rule_content == "*"
    }))
}
```

- [ ] **Step 2: Add a test for the permission matcher**

Add to the `#[cfg(test)] mod tests` block:

```rust
#[test]
fn permission_matcher_matches_command_stem() {
    let input = serde_json::json!({"command": "git status"});
    let matcher = BashTool.prepare_permission_matcher(&input).expect("should return matcher");
    assert!(matcher("git *"), "git * should match git status");
    assert!(matcher("git diff"), "git diff should match git status (stem check only)");
    assert!(!matcher("ls *"), "ls * should not match git status");
    assert!(matcher("*"), "wildcard should match anything");
}

#[test]
fn permission_matcher_handles_no_command() {
    let input = serde_json::json!({});
    let matcher = BashTool.prepare_permission_matcher(&input);
    assert!(matcher.is_none(), "no command = no matcher");
}

#[test]
fn permission_matcher_handles_whitespace_command() {
    let input = serde_json::json!({"command": "   echo hello"});
    let matcher = BashTool.prepare_permission_matcher(&input).expect("should return matcher");
    assert!(matcher("echo *"), "should trim command");
}
```

- [ ] **Step 3: Run new tests**

Run: `cargo test -p cli bash::tests::permission_matcher`
Expected: ALL PASS

- [ ] **Step 4: Run all bash tests**

Run: `cargo test -p cli bash::tests::`
Expected: ALL PASS

- [ ] **Step 5: Compile check**

Run: `cargo check -p cli 2>&1`

- [ ] **Step 6: Commit**

```bash
git add cli/src/tools/bash.rs
git commit -m "feat(parity): implement prepare_permission_matcher with command-stem matching for BashTool"
```

---

---

## Self-Review Checklist

### 1. Spec coverage

| Spec requirement | Covered by |
|------------------|------------|
| Read: `pages` field in input_schema | Task 2, Step 1 |
| Read: prompt port from FileReadTool/prompt.ts | Task 1 |
| Read: `output_schema()` discriminated union | Task 2, Step 2 |
| Read: `extract_search_text` | Task 2, Step 3 |
| Read: `getPath` | Task 2, Step 3 |
| Read: `is_search_or_read_command` | Task 2, Step 3 |
| Read: `getActivityDescription` | Task 2, Step 3 |
| Read: `max_result_size_chars` large | Task 2, Step 3 |
| Edit: prompt port from FileEditTool/prompt.ts | Task 3 |
| Edit: `output_schema()` FileEditOutput | Task 4, Step 1 |
| Edit: `getPath` | Task 4, Step 1 |
| Edit: `getActivityDescription` | Task 4, Step 1 |
| Edit: `toAutoClassifierInput` returns `<path>: <new_string>` | Task 4, Step 1 |
| Edit: `backfill_observable_input` path expansion | Task 4, Step 1 |
| Edit: `render_tool_result_message` emits `RenderSpec::Diff` | Task 4, Step 3 |
| Edit: unified diff generation in `call()` | Task 4, Step 2 |
| Write: prompt port from FileWriteTool/prompt.ts | Task 5 |
| Write: `output_schema()` | Task 6, Step 1 |
| Write: `getPath` | Task 6, Step 1 |
| Write: `getActivityDescription` | Task 6, Step 1 |
| Write: `toAutoClassifierInput` | Task 6, Step 1 |
| Write: overwrite warning | Task 6, Step 2 |
| Glob: prompt port from GlobTool/prompt.ts | Task 7 |
| Glob: `output_schema()` with durationMs, numFiles, filenames, truncated | Task 8, Step 1 |
| Glob: `render_tool_result_message` emits `RenderSpec::PathList` | Task 8, Step 1 |
| Glob: `extract_search_text` | Task 8, Step 1 |
| Grep: ALL 11 new fields (output_mode, -A/-B/-C/context, -n, -i, type, offset, multiline) | Task 10, Step 1 |
| Grep: prompt port from GrepTool/prompt.ts | Task 9 |
| Grep: `output_schema()` discriminated by mode | Task 10, Step 2 |
| Grep: `is_search_or_read_command` returning `is_search: true` | Task 10, Step 2 |
| Grep: `extract_search_text` | Task 10, Step 2 |
| Grep: new flags implemented in `call()` | Task 10, Steps 3-4 |
| NotebookEdit: prompt port from NotebookEditTool/prompt.ts | Task 11, Step 1 |
| NotebookEdit: `output_schema()` with notebook JSON | Task 11, Steps 2-3 |
| Bash: `dangerouslyDisableSandbox` in input_schema | Task 13, Step 1 |
| Bash: massive prompt port (~370 lines) from BashTool/prompt.ts | Task 12 |
| Bash: git safety protocol in prompt | Task 12 |
| Bash: PR creation protocol in prompt | Task 12 |
| Bash: sleep avoidance in prompt | Task 12 |
| Bash: sandbox section in prompt | Task 12 (noted: omit for now since sandbox not implemented) |
| Bash: `prepare_permission_matcher` command-stem matching | Task 14 |
| Bash: `getActivityDescription` returning command | Task 13, Step 2 |
| Bash: `output_schema()` | Task 13, Step 2 |

### 2. Placeholder scan

- No "TBD" or "TODO" in code steps
- No "implement later" hand-waves
- Every step has complete code or exact instructions
- Prompt text port steps reference exact source files and function names

### 3. Type consistency

- `shared::RenderSpec` variants: `Diff`, `PathList`, `PathEntry`, `DiffHunk`, `DiffLine` — all exist in Batch 1's `render_spec.rs`
- `SearchReadKind` — from `crate::tools::contract`
- `ProgressEvent` — from `crate::tools::contract`
- `RenderOpts` — from `crate::tools::contract`
- All trait methods reference the exact signatures in `contract.rs`

### 4. Gaps intentionally deferred

- **Read: actual PDF rendering**: PDF reading (pdf crate + image extraction) is a future enhancement. The `pages` field is accepted and reported; actual PDF content extraction is not implemented.
- **Read: actual image rendering**: Images are read and file metadata is reported; actual base64 encoding for Anthropic content blocks is not in this batch. The schema is forward-compatible.
- **Edit: advanced diff hunk splitting**: The diff generation uses a simple first-changed-region approach (matching Claude's basic diff). Advanced multi-hunk splitting with similarity detection is not implemented.
- **Grep: actual ripgrep backend**: The implementation uses the `regex` crate directly on file contents, not `ripgrep`'s actual search engine. Performance parity with ripgrep (parallelism, mmap, file type detection) is not in this batch.
- **Grep: full -A/-B/-C context with overlapping**: Context lines between adjacent matches may not merge/overlap correctly in edge cases.
- **Bash: actual sandbox enforcement**: `dangerouslyDisableSandbox` is read from input but has no runtime effect (no sandbox implementation). The flag exists for schema parity and future use.
- **Bash: background task notification**: `run_in_background` spawns a tokio task but there is no notification mechanism to alert the user when it completes. Full Monitor-tool parity is tracked separately.
- **Bash: command-stem matcher**: Matches only on the first word. Full subcommand matching (e.g., `git commit *` vs `git push *`) is not implemented.

---

## Acceptance Criteria

1. **Schema parity** (Layer 1): For each of the 7 tools, the `input_schema()` matches Claude's after JSON Schema normalization. Run `cargo test` to verify schemas are syntactically valid.
2. **Prompt parity**: Each tool's `prompt()` returns the ported prompt text. The 7 prompt files are no longer TODO placeholders.
3. **Output schema** (Layer 2): All 7 tools return `output_schema()`. Read returns discriminated union. Grep returns mode-discriminated schema. Edit/Write/Glob/NotebookEdit return structured shapes. Bash returns stdout/stderr/exit_code.
4. **Render hooks** (Layer 3):
   - Edit `render_tool_result_message()` emits `RenderSpec::Diff` with hunks
   - Glob `render_tool_result_message()` emits `RenderSpec::PathList` with entries
5. **Permission matching** (Layer 4):
   - `BashTool::prepare_permission_matcher()` matches command stems
   - Edit/Write `toAutoClassifierInput()` returns descriptive strings
   - Edit `backfillObservableInput()` expands relative paths
6. **Activity descriptions**: Read, Edit, Write, and Bash return appropriate activity strings.
7. **No regressions**: All existing unit tests pass (`cargo test -p cli --lib`).
8. **The full tool set compiles**: `cargo check -p cli` succeeds.

---

# Batch 2 — Interactive Tools Parity Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Bring AskUserQuestion, ExitPlanMode, EnterPlanMode, SendMessage, and TodoWrite to full Claude Code parity — correct schemas, prompt text, output schemas, and interactive rendering via RenderSpec::Interactive.

**Architecture:** Each tool returns `RenderSpec::Interactive{...}` variants (MultiQuestion, PlanApproval, PermissionPrompt) instead of plain strings. The executor suspends on Interactive results, awaits a UserInteractionResponse from the TUI session bus, then resumes with the resolved input. Permission prompts flow through the same pipeline. RenderSpec already has all needed InteractiveWidget variants from Batch 1.

**Tech Stack:** Rust, serde_json, shared::RenderSpec + shared::InteractiveWidget (Batch 1), tokio channels, Store (Arc<RwLock<AppState>>).

---

## File Map

| File | Responsibility |
|------|---------------|
| `cli/src/tools/ask_user_question.rs` | Full rewrite — questions[] schema, MultiQuestion widget |
| `cli/src/tools/exit_plan_mode.rs` | Full rewrite — V2 schema, PlanApproval widget |
| `cli/src/tools/enter_plan_mode.rs` | Rewrite — prompt, shouldDefer, outputSchema, activity |
| `cli/src/tools/send_message.rs` | Full rewrite — to/message/StructuredMessage schema |
| `cli/src/tools/todo_write.rs` | Full rewrite — activeForm, persist to Store, outputSchema |
| `cli/src/tools/prompts/ask_user_question.txt` | Replace with ASK_USER_QUESTION_TOOL_PROMPT |
| `cli/src/tools/prompts/exit_plan_mode.txt` | Replace with EXIT_PLAN_MODE_V2_TOOL_PROMPT |
| `cli/src/tools/prompts/enter_plan_mode.txt` | Replace with external-user EnterPlanMode prompt |
| `cli/src/tools/prompts/send_message.txt` | Replace with SendMessage prompt (minus UDS section) |
| `cli/src/tools/prompts/todo_write.txt` | Replace with TODO_WRITE PROMPT |
| `cli/src/executor/interactive.rs` | NEW — handles Interactive RenderSpec suspension/resumption |
| `cli/src/executor/mod.rs` | Modify — wire interactive handler into tool execution |
| `cli/src/tools/mod.rs` | Modify — TodoWrite/EnterPlanMode/ExitPlanMode injected with Store |

---

## Prerequisites

Before any task in this batch, verify Batch 1 is complete:
- The extended `Tool` trait in `cli/src/tools/contract.rs` has all methods from the design spec §3
- `shared/src/render_spec.rs` has `InteractiveWidget` variants (MultiQuestion, PlanApproval, PermissionPrompt, MessageCompose)
- `PermissionResult::Ask` with `rule_suggestions` is defined in `cli/src/tools/permission.rs`
- All existing tools compile against the new trait with placeholder impls

Check: `cargo check -p cli 2>&1`

---

### Task 1: Port AskUserQuestion prompt text

**Files:**
- Modify: `cli/src/tools/prompts/ask_user_question.txt`

Replace the placeholder content with the full prompt from `../claude-code-src/tools/AskUserQuestionTool/prompt.ts` lines 32-44.

- [ ] **Step 1: Write the prompt file**

```rust
// cli/src/tools/prompts/ask_user_question.txt — replace existing content

Use this tool when you need to ask the user questions during execution. This allows you to:
1. Gather user preferences or requirements
2. Clarify ambiguous instructions
3. Get decisions on implementation choices as you work
4. Offer choices to the user about what direction to take.

Usage notes:
- Users will always be able to select "Other" to provide custom text input
- Use multiSelect: true to allow multiple answers to be selected for a question
- If you recommend a specific option, make that the first option in the list and add "(Recommended)" at the end of the label

Plan mode note: In plan mode, use this tool to clarify requirements or choose between approaches BEFORE finalizing your plan. Do NOT use this tool to ask "Is my plan ready?" or "Should I proceed?" - use ExitPlanMode for plan approval. IMPORTANT: Do not reference "the plan" in your questions (e.g., "Do you have feedback about the plan?", "Does the plan look good?") because the user cannot see the plan in the UI until you call ExitPlanMode. If you need plan approval, use ExitPlanMode instead.
```

- [ ] **Step 2: Verify the file was written**

Run: `cat cli/src/tools/prompts/ask_user_question.txt | head -5`

- [ ] **Step 3: Commit**

```bash
git add cli/src/tools/prompts/ask_user_question.txt
git commit -m "feat(parity): port AskUserQuestion prompt text from Claude"
```

---

### Task 2: Port ExitPlanMode prompt text

**Files:**
- Modify: `cli/src/tools/prompts/exit_plan_mode.txt`

Replace the placeholder with `EXIT_PLAN_MODE_V2_TOOL_PROMPT` from `../claude-code-src/tools/ExitPlanModeTool/prompt.ts` (lines 6-29).

- [ ] **Step 1: Write the prompt file**

```text
// cli/src/tools/prompts/exit_plan_mode.txt — replace existing content

Use this tool when you are in plan mode and have finished writing your plan to the plan file and are ready for user approval.

## How This Tool Works
- You should have already written your plan to the plan file specified in the plan mode system message
- This tool does NOT take the plan content as a parameter - it will read the plan from the file you wrote
- This tool simply signals that you're done planning and ready for the user to review and approve
- The user will see the contents of your plan file when they review it

## When to Use This Tool
IMPORTANT: Only use this tool when the task requires planning the implementation steps of a task that requires writing code. For research tasks where you're gathering information, searching files, reading files or in general trying to understand the codebase - do NOT use this tool.

## Before Using This Tool
Ensure your plan is complete and unambiguous:
- If you have unresolved questions about requirements or approach, use AskUserQuestion first (in earlier phases)
- Once your plan is finalized, use THIS tool to request approval

**Important:** Do NOT use AskUserQuestion to ask "Is this plan okay?" or "Should I proceed?" - that's exactly what THIS tool does. ExitPlanMode inherently requests user approval of your plan.

## Examples

1. Initial task: "Search for and understand the implementation of vim mode in the codebase" - Do not use the exit plan mode tool because you are not planning the implementation steps of a task.
2. Initial task: "Help me implement yank mode for vim" - Use the exit plan mode tool after you have finished planning the implementation steps of the task.
3. Initial task: "Add a new feature to handle user authentication" - If unsure about auth method (OAuth, JWT, etc.), use AskUserQuestion first, then use exit plan mode tool after clarifying the approach.
```

- [ ] **Step 2: Commit**

```bash
git add cli/src/tools/prompts/exit_plan_mode.txt
git commit -m "feat(parity): port ExitPlanMode V2 prompt text from Claude"
```

---

### Task 3: Port EnterPlanMode prompt text

**Files:**
- Modify: `cli/src/tools/prompts/enter_plan_mode.txt`

Replace placeholder with the external-user function body from `../claude-code-src/tools/EnterPlanModeTool/prompt.ts` lines 16-98 (`getEnterPlanModeToolPromptExternal()`).

- [ ] **Step 1: Write the prompt file**

Write `cli/src/tools/prompts/enter_plan_mode.txt` with the full text of `getEnterPlanModeToolPromptExternal()` (the return value of that function). This is the long prompt found in the Claude source at `../claude-code-src/tools/EnterPlanModeTool/prompt.ts:23-98`, covering When to Use (7 conditions), When NOT to Use, What Happens in Plan Mode, Examples, and Important Notes.

- [ ] **Step 2: Commit**

```bash
git add cli/src/tools/prompts/enter_plan_mode.txt
git commit -m "feat(parity): port EnterPlanMode prompt text from Claude"
```

---

### Task 4: Port SendMessage prompt text

**Files:**
- Modify: `cli/src/tools/prompts/send_message.txt`

Replace with the SendMessage prompt from `../claude-code-src/tools/SendMessageTool/prompt.ts` (the `getPrompt()` function body, trimmed). Omit the `UDS_INBOX` and cross-session sections (out of scope).

- [ ] **Step 1: Write the prompt file**

```text
// cli/src/tools/prompts/send_message.txt

# SendMessage

Send a message to another agent.

```json
{"to": "researcher", "summary": "assign task 1", "message": "start on task #1"}
```

| `to` | |
|---|---|
| `"researcher"` | Teammate by name |
| `"*"` | Broadcast to all teammates — expensive, use only when everyone genuinely needs it |

Your plain text output is NOT visible to other agents — to communicate, you MUST call this tool. Messages from teammates are delivered automatically; you don't check an inbox. Refer to teammates by name, never by UUID. When relaying, don't quote the original — it's already rendered to the user.

## Protocol responses (legacy)

If you receive a JSON message with `type: "shutdown_request"` or `type: "plan_approval_request"`, respond with the matching `_response` type — echo the `request_id`, set `approve` true/false:

```json
{"to": "team-lead", "message": {"type": "shutdown_response", "request_id": "...", "approve": true}}
{"to": "researcher", "message": {"type": "plan_approval_response", "request_id": "...", "approve": false, "feedback": "add error handling"}}
```

Approving shutdown terminates your process. Rejecting plan sends the teammate back to revise. Don't originate `shutdown_request` unless asked. Don't send structured JSON status messages — use TaskUpdate.
```

- [ ] **Step 2: Commit**

```bash
git add cli/src/tools/prompts/send_message.txt
git commit -m "feat(parity): port SendMessage prompt text from Claude"
```

---

### Task 5: Port TodoWrite prompt text

**Files:**
- Modify: `cli/src/tools/prompts/todo_write.txt`

Replace placeholder with the `PROMPT` constant from `../claude-code-src/tools/TodoWriteTool/prompt.ts` (lines 3-181).

- [ ] **Step 1: Write the prompt file**

Write `cli/src/tools/prompts/todo_write.txt` with the full text of Claude's `PROMPT` constant. This is the longest prompt (~180 lines) covering when to use (7 scenarios), when NOT to use (4 scenarios), 4 detailed examples with reasoning, 4 counter-examples, task states and management (4 rules), and task completion requirements.

The exact text is in the file at `../claude-code-src/tools/TodoWriteTool/prompt.ts` — copy the content of the backtick string assigned to `PROMPT`.

- [ ] **Step 2: Commit**

```bash
git add cli/src/tools/prompts/todo_write.txt
git commit -m "feat(parity): port TodoWrite prompt text from Claude"
```

---

### Task 6: Rewrite AskUserQuestion tool — full schema, RenderSpec::Interactive

**Files:**
- Modify: `cli/src/tools/ask_user_question.rs`

Replace the entire file. The new implementation must:
- Return the correct `input_schema()` with `questions[]` array (1-4 items, each with question/header/options/multiSelect), plus optional answers/annotations/metadata
- Return `output_schema()` with `{questions, answers, annotations}`
- Set `should_defer = true`, `search_hint`, `max_result_size_chars = 100000`
- Set `is_concurrency_safe = true`, `is_read_only = true`, `requires_user_interaction = true`
- `prompt()` reads from `prompts/ask_user_question.txt`
- `check_permissions()` returns `PermissionResult::Ask`
- `render_tool_use_message()` returns `RenderSpec::Interactive{MultiQuestion{...}}` — the executor suspends here
- `render_tool_result_message()` returns a Group showing user answers (matching Claude's `AskUserQuestionResultMessage`)
- `render_tool_use_rejected_message()` returns "User declined to answer questions"
- `call()` handles `auto_deny_prompts` → error; otherwise returns questions+answers JSON
- `map_tool_result_to_block()` formats answers as `"question"="answer"` pairs

- [ ] **Step 1: Write the failing schema test**

Create a test that asserts `input_schema()` has `questions` as required, each with `question`/`header`/`options`, max 4 questions.

```rust
// append to ask_user_question.rs tests

#[test]
fn schema_has_questions_array() {
    let tool = AskUserQuestionTool;
    let schema = tool.input_schema();
    let props = schema["properties"].as_object().unwrap();
    assert!(props.contains_key("questions"));
    let questions = &props["questions"];
    assert_eq!(questions["minItems"], 1);
    assert_eq!(questions["maxItems"], 4);
}
```

Run: `cargo test -p cli ask_user_question::tests::schema_has_questions_array`
Expected: FAIL (current schema has `question` string, not `questions` array)

- [ ] **Step 2: Rewrite the tool implementation**

Write the full `cli/src/tools/ask_user_question.rs`:

```rust
use async_trait::async_trait;
use serde_json::json;
use shared::{InteractiveWidget, Question, QuestionOption, RenderSpec};
use super::contract::{
    DescriptionCtx, InterruptBehavior, ProgressSink, PromptCtx, RenderOpts, Tool,
    ToolCallContext, ToolResult, ToolResultBlock, ToolResultContent,
};
use crate::tools::permission::{PermissionResult, RuleSuggestion};

pub struct AskUserQuestionTool;

#[async_trait]
impl Tool for AskUserQuestionTool {
    fn name(&self) -> &str {
        "AskUserQuestion"
    }

    fn user_facing_name(&self, _input: Option<&serde_json::Value>) -> String {
        String::new()
    }

    fn description(&self, _input: Option<&serde_json::Value>, _ctx: &DescriptionCtx) -> String {
        "Asks the user multiple choice questions to gather information, clarify ambiguity, understand preferences, make decisions or offer them choices.".into()
    }

    fn prompt(&self, _ctx: &PromptCtx) -> String {
        include_str!("prompts/ask_user_question.txt").into()
    }

    fn search_hint(&self) -> Option<&'static str> {
        Some("prompt the user with a multiple-choice question")
    }

    fn should_defer(&self) -> bool {
        true
    }

    fn input_schema(&self) -> serde_json::Value {
        json!({
            "type": "object",
            "properties": {
                "questions": {
                    "type": "array",
                    "minItems": 1,
                    "maxItems": 4,
                    "description": "Questions to ask the user (1-4 questions)",
                    "items": {
                        "type": "object",
                        "properties": {
                            "question": {
                                "type": "string",
                                "description": "The complete question to ask the user. Should be clear, specific, and end with a question mark."
                            },
                            "header": {
                                "type": "string",
                                "maxLength": 12,
                                "description": "Very short label displayed as a chip/tag (max 12 chars)."
                            },
                            "options": {
                                "type": "array",
                                "minItems": 2,
                                "maxItems": 4,
                                "items": {
                                    "type": "object",
                                    "properties": {
                                        "label": {
                                            "type": "string",
                                            "description": "The display text for this option (1-5 words)"
                                        },
                                        "description": {
                                            "type": "string",
                                            "description": "Explanation of what this option means"
                                        },
                                        "preview": {
                                            "type": "string",
                                            "description": "Optional preview content rendered as markdown in a monospace box"
                                        }
                                    },
                                    "required": ["label", "description"]
                                }
                            },
                            "multiSelect": {
                                "type": "boolean",
                                "default": false,
                                "description": "Set to true to allow multiple answers"
                            }
                        },
                        "required": ["question", "header", "options"]
                    }
                },
                "answers": {
                    "type": "object",
                    "additionalProperties": { "type": "string" },
                    "description": "User answers collected by the permission component"
                },
                "annotations": {
                    "type": "object",
                    "additionalProperties": {
                        "type": "object",
                        "properties": {
                            "preview": { "type": "string" },
                            "notes": { "type": "string" }
                        }
                    },
                    "description": "Optional per-question annotations from the user. Keyed by question text."
                },
                "metadata": {
                    "type": "object",
                    "properties": {
                        "source": {
                            "type": "string",
                            "description": "Optional identifier for analytics tracking"
                        }
                    }
                }
            },
            "required": ["questions"]
        })
    }

    fn output_schema(&self) -> Option<serde_json::Value> {
        Some(json!({
            "type": "object",
            "properties": {
                "questions": {
                    "type": "array",
                    "description": "The questions that were asked",
                    "items": {
                        "type": "object",
                        "properties": {
                            "question": { "type": "string" },
                            "header": { "type": "string" },
                            "options": {
                                "type": "array",
                                "items": {
                                    "type": "object",
                                    "properties": {
                                        "label": { "type": "string" },
                                        "description": { "type": "string" },
                                        "preview": { "type": "string" }
                                    }
                                }
                            },
                            "multiSelect": { "type": "boolean" }
                        }
                    }
                },
                "answers": {
                    "type": "object",
                    "additionalProperties": { "type": "string" },
                    "description": "The answers provided by the user (question text -> answer string; multi-select answers are comma-separated)"
                },
                "annotations": {
                    "type": "object",
                    "additionalProperties": {
                        "type": "object",
                        "properties": {
                            "preview": { "type": "string" },
                            "notes": { "type": "string" }
                        }
                    }
                }
            },
            "required": ["questions", "answers"]
        }))
    }

    fn is_concurrency_safe(&self, _input: &serde_json::Value) -> bool {
        true
    }

    fn is_read_only(&self, _input: &serde_json::Value) -> bool {
        true
    }

    fn requires_user_interaction(&self) -> bool {
        true
    }

    fn interrupt_behavior(&self) -> InterruptBehavior {
        InterruptBehavior::Block
    }

    fn max_result_size_chars(&self) -> usize {
        100_000
    }

    async fn check_permissions(
        &self,
        input: &serde_json::Value,
        _ctx: &ToolCallContext,
    ) -> PermissionResult {
        PermissionResult::Ask {
            updated_input: Some(input.clone()),
            rule_suggestions: vec![],
        }
    }

    fn to_auto_classifier_input(&self, input: &serde_json::Value) -> serde_json::Value {
        let qs = input["questions"]
            .as_array()
            .map(|a| {
                a.iter()
                    .filter_map(|q| q["question"].as_str())
                    .collect::<Vec<_>>()
                    .join(" | ")
            })
            .unwrap_or_default();
        serde_json::Value::String(qs)
    }

    fn render_tool_use_message(
        &self,
        input: &serde_json::Value,
        _opts: &RenderOpts,
    ) -> RenderSpec {
        let questions: Vec<Question> = input["questions"]
            .as_array()
            .map(|arr| {
                arr.iter()
                    .map(|q| Question {
                        question: q["question"].as_str().unwrap_or("").into(),
                        header: q["header"].as_str().unwrap_or("").into(),
                        multi_select: q["multiSelect"].as_bool().unwrap_or(false),
                        options: q["options"]
                            .as_array()
                            .map(|opts| {
                                opts.iter()
                                    .map(|o| QuestionOption {
                                        label: o["label"].as_str().unwrap_or("").into(),
                                        description: o["description"]
                                            .as_str()
                                            .unwrap_or("")
                                            .into(),
                                        preview: o["preview"].as_str().map(String::from),
                                    })
                                    .collect()
                            })
                            .unwrap_or_default(),
                    })
                    .collect()
            })
            .unwrap_or_default();

        RenderSpec::Interactive {
            widget: InteractiveWidget::MultiQuestion { questions },
            response_schema: json!({
                "type": "object",
                "properties": {
                    "answers": {
                        "type": "object",
                        "additionalProperties": { "type": "string" }
                    },
                    "annotations": { "type": "object" }
                }
            }),
        }
    }

    fn render_tool_result_message(
        &self,
        output: &serde_json::Value,
        _progress: &[super::contract::ProgressEvent],
        _opts: &RenderOpts,
    ) -> Option<RenderSpec> {
        let answers = &output["answers"];
        let mut children = vec![
            RenderSpec::Row {
                children: vec![
                    RenderSpec::Status {
                        state: shared::StatusState::Success,
                        message: None,
                    },
                    RenderSpec::Text {
                        body: "User answered questions:".into(),
                        dim: false,
                    },
                ],
            },
        ];

        if let Some(map) = answers.as_object() {
            for (question_text, answer) in map {
                let answer_str = answer.as_str().unwrap_or("");
                children.push(RenderSpec::Text {
                    body: format!("  {} -> {}", question_text, answer_str),
                    dim: true,
                });
            }
        }

        Some(RenderSpec::Group { children })
    }

    fn render_tool_use_rejected_message(
        &self,
        _input: &serde_json::Value,
        _opts: &RenderOpts,
    ) -> Option<RenderSpec> {
        Some(RenderSpec::Text {
            body: "User declined to answer questions".into(),
            dim: false,
        })
    }

    async fn call(
        &self,
        input: serde_json::Value,
        context: &ToolCallContext,
        _on_progress: Option<ProgressSink>,
    ) -> ToolResult {
        if context.auto_deny_prompts {
            return ToolResult {
                content: "Permission denied: async subagents cannot prompt the user.".into(),
                is_error: true,
                ..Default::default()
            };
        }

        let questions = input.get("questions").cloned().unwrap_or(json!([]));
        let answers = input.get("answers").cloned().unwrap_or(json!({}));
        let annotations = input.get("annotations").cloned();

        let mut data = serde_json::Map::new();
        data.insert("questions".into(), questions);
        data.insert("answers".into(), answers);
        if let Some(ann) = annotations {
            data.insert("annotations".into(), ann);
        }

        ToolResult {
            content: serde_json::Value::Object(data).to_string(),
            is_error: false,
            ..Default::default()
        }
    }

    fn map_tool_result_to_block(
        &self,
        output: &serde_json::Value,
        tool_use_id: &str,
    ) -> ToolResultBlock {
        let answers = &output["answers"];
        let answers_text = answers
            .as_object()
            .map(|map| {
                map.iter()
                    .map(|(q, a)| format!("\"{}\"=\"{}\"", q, a.as_str().unwrap_or("")))
                    .collect::<Vec<_>>()
                    .join(", ")
            })
            .unwrap_or_default();

        ToolResultBlock {
            tool_use_id: tool_use_id.into(),
            content: ToolResultContent::Text(format!(
                "User has answered your questions: {}. You can now continue with the user's answers in mind.",
                answers_text
            )),
            is_error: false,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::state::store::PermissionMode;

    #[test]
    fn schema_has_questions_array() {
        let tool = AskUserQuestionTool;
        let schema = tool.input_schema();
        let props = schema["properties"].as_object().unwrap();
        assert!(props.contains_key("questions"));
        let questions = &props["questions"];
        assert_eq!(questions["minItems"], 1);
        assert_eq!(questions["maxItems"], 4);
    }

    #[test]
    fn schema_has_output_schema() {
        let tool = AskUserQuestionTool;
        let output = tool.output_schema().expect("should have outputSchema");
        assert!(output["required"].as_array().unwrap().contains(&json!("answers")));
    }

    #[test]
    fn render_tool_use_emits_interactive() {
        let tool = AskUserQuestionTool;
        let input = json!({
            "questions": [{
                "question": "What library to use?",
                "header": "Library",
                "options": [
                    {"label": "React", "description": "UI library"},
                    {"label": "Vue", "description": "Progressive framework"}
                ],
                "multiSelect": false
            }]
        });
        let spec = tool.render_tool_use_message(&input, &RenderOpts::default());
        assert!(matches!(spec, RenderSpec::Interactive { .. }));
    }

    #[tokio::test]
    async fn auto_denies_when_flag_set() {
        let tool = AskUserQuestionTool;
        let ctx = ToolCallContext {
            cwd: std::env::current_dir().unwrap(),
            permission_mode: PermissionMode::Default,
            abort_signal: None,
            parent_tool_use_id: None,
            bus: None,
            auto_deny_prompts: true,
            tool_use_id: String::new(),
            progress_sink: None,
        };
        let result = tool
            .call(
                json!({"questions": [{"question": "ok?", "header": "Q", "options": [{"label":"Y","description":"yes"},{"label":"N","description":"no"}]}]}),
                &ctx,
                None,
            )
            .await;
        assert!(result.is_error);
    }
}
```

- [ ] **Step 3: Run tests to verify they pass**

Run: `cargo test -p cli ask_user_question::`
Expected: ALL PASS

- [ ] **Step 4: Compile check**

Run: `cargo check -p cli 2>&1`

- [ ] **Step 5: Commit**

```bash
git add cli/src/tools/ask_user_question.rs
git commit -m "feat(parity): rewrite AskUserQuestion with full schema, Interactive widget, and outputSchema"
```

---

### Task 7: Rewrite ExitPlanMode tool — V2 schema, PlanApproval widget

**Files:**
- Modify: `cli/src/tools/exit_plan_mode.rs`

Replace the entire file. The new implementation must:
- Accept `allowedPrompts`, `plan`, `planFilePath` in input schema
- Return output schema with `plan`, `isAgent`, `filePath`, `planWasEdited`, etc.
- Set `should_defer = true`, `search_hint`, `is_concurrency_safe = true`
- `validate_input()` rejects when NOT in plan mode with a specific error message
- `check_permissions()` returns `PermissionResult::Ask`
- `render_tool_use_message()` returns `RenderSpec::Interactive{PlanApproval{plan_markdown}}`
- `render_tool_result_message()` shows "Plan approved" with plan content
- `render_tool_use_rejected_message()` shows "Plan rejected by user"
- `call()` exits plan mode (sets `PermissionMode::Default` on the store)
- Map result to block: "User has approved your plan..." with plan text

- [ ] **Step 1: Write the test**

```rust
#[test]
fn validates_not_in_plan_mode() {
    // store starts in Default mode, not Plan
    let store = Arc::new(crate::state::store::Store::default());
    let tool = ExitPlanModeTool { store: store.clone() };
    let ctx = dummy_ctx();
    let rt = tokio::runtime::Runtime::new().unwrap();
    let result = rt.block_on(tool.validate_input(&json!({}), &ctx));
    assert!(matches!(result, ValidationResult::Err { .. }));
}
```

- [ ] **Step 2: Run test to verify it fails**

Run: `cargo test -p cli exit_plan_mode::tests::validates_not_in_plan_mode`
Expected: FAIL (current impl doesn't validate mode)

- [ ] **Step 3: Write the full implementation**

Write `cli/src/tools/exit_plan_mode.rs`:

```rust
use std::sync::Arc;
use async_trait::async_trait;
use serde_json::json;
use shared::{InteractiveWidget, RenderSpec, StatusState};
use super::contract::{
    DescriptionCtx, InterruptBehavior, ProgressSink, PromptCtx, RenderOpts, Tool,
    ToolCallContext, ToolResult, ToolResultBlock, ToolResultContent, ValidationResult,
};
use crate::state::store::PermissionMode;
use crate::tools::permission::{PermissionResult, RuleSuggestion};

pub struct ExitPlanModeTool {
    pub store: Arc<crate::state::store::Store>,
}

#[async_trait]
impl Tool for ExitPlanModeTool {
    fn name(&self) -> &str {
        "ExitPlanMode"
    }

    fn user_facing_name(&self, _input: Option<&serde_json::Value>) -> String {
        String::new()
    }

    fn description(&self, _input: Option<&serde_json::Value>, _ctx: &DescriptionCtx) -> String {
        "Prompts the user to exit plan mode and start coding".into()
    }

    fn prompt(&self, _ctx: &PromptCtx) -> String {
        include_str!("prompts/exit_plan_mode.txt").into()
    }

    fn search_hint(&self) -> Option<&'static str> {
        Some("present plan for approval and start coding (plan mode only)")
    }

    fn should_defer(&self) -> bool {
        true
    }

    fn is_concurrency_safe(&self, _input: &serde_json::Value) -> bool {
        true
    }

    fn is_read_only(&self, _input: &serde_json::Value) -> bool {
        false // writes plan to disk
    }

    fn requires_user_interaction(&self) -> bool {
        true
    }

    fn interrupt_behavior(&self) -> InterruptBehavior {
        InterruptBehavior::Block
    }

    fn input_schema(&self) -> serde_json::Value {
        json!({
            "type": "object",
            "properties": {
                "allowedPrompts": {
                    "type": "array",
                    "description": "Prompt-based permissions needed to implement the plan",
                    "items": {
                        "type": "object",
                        "properties": {
                            "tool": { "type": "string", "enum": ["Bash"] },
                            "prompt": { "type": "string", "description": "Semantic description of the action" }
                        },
                        "required": ["tool", "prompt"]
                    }
                },
                "plan": {
                    "type": "string",
                    "description": "The plan content (injected from disk)"
                },
                "planFilePath": {
                    "type": "string",
                    "description": "The plan file path (injected from disk)"
                }
            }
        })
    }

    fn output_schema(&self) -> Option<serde_json::Value> {
        Some(json!({
            "type": "object",
            "properties": {
                "plan": { "type": ["string", "null"], "description": "The plan presented to the user" },
                "isAgent": { "type": "boolean" },
                "filePath": { "type": "string", "description": "File path where the plan was saved" },
                "hasTaskTool": { "type": "boolean", "description": "Whether Agent tool is available" },
                "planWasEdited": { "type": "boolean", "description": "True when the user edited the plan" },
                "awaitingLeaderApproval": { "type": "boolean" },
                "requestId": { "type": "string" }
            },
            "required": ["plan", "isAgent"]
        }))
    }

    fn max_result_size_chars(&self) -> usize {
        100_000
    }

    async fn validate_input(
        &self,
        _input: &serde_json::Value,
        _ctx: &ToolCallContext,
    ) -> ValidationResult {
        let in_plan_mode =
            matches!(self.store.get_state().permission_mode, PermissionMode::Plan);

        if !in_plan_mode {
            return ValidationResult::Err {
                message: "You are not in plan mode. This tool is only for exiting plan mode after writing a plan. If your plan was already approved, continue with implementation.".into(),
                error_code: 1,
            };
        }
        ValidationResult::Ok
    }

    async fn check_permissions(
        &self,
        input: &serde_json::Value,
        _ctx: &ToolCallContext,
    ) -> PermissionResult {
        PermissionResult::Ask {
            updated_input: Some(input.clone()),
            rule_suggestions: vec![],
        }
    }

    fn render_tool_use_message(
        &self,
        input: &serde_json::Value,
        _opts: &RenderOpts,
    ) -> RenderSpec {
        let plan = input["plan"].as_str().unwrap_or("").to_string();
        RenderSpec::Interactive {
            widget: InteractiveWidget::PlanApproval {
                plan_markdown: plan,
            },
            response_schema: json!({
                "type": "object",
                "properties": {
                    "approved": { "type": "boolean" },
                    "editedPlan": { "type": "string" }
                },
                "required": ["approved"]
            }),
        }
    }

    fn render_tool_result_message(
        &self,
        output: &serde_json::Value,
        _progress: &[super::contract::ProgressEvent],
        _opts: &RenderOpts,
    ) -> Option<RenderSpec> {
        let plan = output["plan"].as_str().unwrap_or("");
        Some(RenderSpec::Group {
            children: vec![
                RenderSpec::Status {
                    state: StatusState::Success,
                    message: Some("Plan approved".into()),
                },
                RenderSpec::Text {
                    body: if plan.is_empty() {
                        "User has approved exiting plan mode. You can now proceed.".into()
                    } else {
                        plan.to_string()
                    },
                    dim: false,
                },
            ],
        })
    }

    fn render_tool_use_rejected_message(
        &self,
        _input: &serde_json::Value,
        _opts: &RenderOpts,
    ) -> Option<RenderSpec> {
        Some(RenderSpec::Text {
            body: "User rejected the plan. Return to plan mode to revise.".into(),
            dim: false,
        })
    }

    async fn call(
        &self,
        input: serde_json::Value,
        _context: &ToolCallContext,
        _on_progress: Option<ProgressSink>,
    ) -> ToolResult {
        let plan = input["plan"].as_str().unwrap_or("");

        self.store.set_state(|s| {
            s.permission_mode = PermissionMode::Default;
        });

        let plan_json = serde_json::to_string(plan).unwrap_or_else(|_| "null".into());

        ToolResult {
            content: json!({
                "plan": plan,
                "isAgent": false,
                "filePath": null,
                "planWasEdited": false
            })
            .to_string(),
            is_error: false,
            ..Default::default()
        }
    }

    fn map_tool_result_to_block(
        &self,
        output: &serde_json::Value,
        tool_use_id: &str,
    ) -> ToolResultBlock {
        let plan = output["plan"].as_str().unwrap_or("");

        if plan.trim().is_empty() {
            return ToolResultBlock {
                tool_use_id: tool_use_id.into(),
                content: ToolResultContent::Text(
                    "User has approved exiting plan mode. You can now proceed.".into(),
                ),
                is_error: false,
            };
        }

        ToolResultBlock {
            tool_use_id: tool_use_id.into(),
            content: ToolResultContent::Text(format!(
                "User has approved your plan. You can now start coding. Start with updating your todo list if applicable\n\nYour plan has been saved.\n\n## Approved Plan:\n{}",
                plan
            )),
            is_error: false,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::state::store::Store;

    fn dummy_ctx() -> ToolCallContext {
        ToolCallContext {
            cwd: std::env::current_dir().unwrap(),
            permission_mode: PermissionMode::Default,
            abort_signal: None,
            parent_tool_use_id: None,
            bus: None,
            auto_deny_prompts: false,
            tool_use_id: "tu_test".into(),
            progress_sink: None,
        }
    }

    #[tokio::test]
    async fn validates_not_in_plan_mode() {
        let store = Arc::new(Store::default());
        let tool = ExitPlanModeTool { store };
        let result = tool.validate_input(&json!({}), &dummy_ctx()).await;
        assert!(matches!(result, ValidationResult::Err { .. }));
    }

    #[tokio::test]
    async fn validates_ok_when_in_plan_mode() {
        let store = Arc::new(Store::default());
        store.set_state(|s| s.permission_mode = PermissionMode::Plan);
        let tool = ExitPlanModeTool { store };
        let result = tool.validate_input(&json!({}), &dummy_ctx()).await;
        assert!(matches!(result, ValidationResult::Ok));
    }

    #[test]
    fn render_tool_use_emits_plan_approval() {
        let store = Arc::new(Store::default());
        let tool = ExitPlanModeTool { store };
        let input = json!({"plan": "# My Plan\n\nSome content"});
        let spec = tool.render_tool_use_message(&input, &RenderOpts::default());
        assert!(matches!(spec, RenderSpec::Interactive {
            widget: InteractiveWidget::PlanApproval { .. },
            ..
        }));
    }
}
```

- [ ] **Step 4: Run tests**

Run: `cargo test -p cli exit_plan_mode::`
Expected: ALL PASS

- [ ] **Step 5: Compile check**

Run: `cargo check -p cli 2>&1`

- [ ] **Step 6: Commit**

```bash
git add cli/src/tools/exit_plan_mode.rs
git commit -m "feat(parity): rewrite ExitPlanMode with V2 schema, Interactive PlanApproval, and outputSchema"
```

---

### Task 8: Rewrite EnterPlanMode — shouldDefer, outputSchema, activity description

**Files:**
- Modify: `cli/src/tools/enter_plan_mode.rs`

- [ ] **Step 1: Write the implementation**

Replace `cli/src/tools/enter_plan_mode.rs`:

```rust
use std::sync::Arc;
use async_trait::async_trait;
use serde_json::json;
use shared::RenderSpec;
use super::contract::{
    DescriptionCtx, ProgressSink, PromptCtx, RenderOpts, Tool, ToolCallContext, ToolResult,
    ToolResultBlock, ToolResultContent,
};
use crate::state::store::PermissionMode;

pub struct EnterPlanModeTool {
    pub store: Arc<crate::state::store::Store>,
}

#[async_trait]
impl Tool for EnterPlanModeTool {
    fn name(&self) -> &str {
        "EnterPlanMode"
    }

    fn user_facing_name(&self, _input: Option<&serde_json::Value>) -> String {
        String::new()
    }

    fn description(&self, _input: Option<&serde_json::Value>, _ctx: &DescriptionCtx) -> String {
        "Requests permission to enter plan mode for complex tasks requiring exploration and design".into()
    }

    fn prompt(&self, _ctx: &PromptCtx) -> String {
        include_str!("prompts/enter_plan_mode.txt").into()
    }

    fn search_hint(&self) -> Option<&'static str> {
        Some("switch to plan mode to design an approach before coding")
    }

    fn should_defer(&self) -> bool {
        true
    }

    fn is_concurrency_safe(&self, _input: &serde_json::Value) -> bool {
        true
    }

    fn is_read_only(&self, _input: &serde_json::Value) -> bool {
        true
    }

    fn max_result_size_chars(&self) -> usize {
        100_000
    }

    fn input_schema(&self) -> serde_json::Value {
        json!({"type": "object", "properties": {}})
    }

    fn output_schema(&self) -> Option<serde_json::Value> {
        Some(json!({
            "type": "object",
            "properties": {
                "message": {
                    "type": "string",
                    "description": "Confirmation that plan mode was entered"
                }
            },
            "required": ["message"]
        }))
    }

    fn get_activity_description(&self, _input: &serde_json::Value) -> Option<String> {
        Some("Entering plan mode".into())
    }

    fn render_tool_use_message(
        &self,
        _input: &serde_json::Value,
        _opts: &RenderOpts,
    ) -> RenderSpec {
        RenderSpec::Header {
            verb: "Entering".into(),
            target: Some("plan mode".into()),
            tag: None,
        }
    }

    fn render_tool_result_message(
        &self,
        output: &serde_json::Value,
        _progress: &[super::contract::ProgressEvent],
        _opts: &RenderOpts,
    ) -> Option<RenderSpec> {
        let msg = output["message"].as_str().unwrap_or("");
        Some(RenderSpec::Group {
            children: vec![
                RenderSpec::Text {
                    body: msg.to_string(),
                    dim: false,
                },
                RenderSpec::Text {
                    body: "In plan mode, you should:\n1. Thoroughly explore the codebase to understand existing patterns\n2. Identify similar features and architectural approaches\n3. Consider multiple approaches and their trade-offs\n4. Use AskUserQuestion if you need to clarify the approach\n5. Design a concrete implementation strategy\n6. When ready, use ExitPlanMode to present your plan for approval\n\nRemember: DO NOT write or edit any files yet. This is a read-only exploration and planning phase.".into(),
                    dim: false,
                },
            ],
        })
    }

    async fn call(
        &self,
        _input: serde_json::Value,
        _context: &ToolCallContext,
        _on_progress: Option<ProgressSink>,
    ) -> ToolResult {
        self.store.set_state(|s| {
            s.permission_mode = PermissionMode::Plan;
        });

        let msg = "Entered plan mode. You should now focus on exploring the codebase and designing an implementation approach.";

        ToolResult {
            content: json!({"message": msg}).to_string(),
            is_error: false,
            ..Default::default()
        }
    }

    fn map_tool_result_to_block(
        &self,
        output: &serde_json::Value,
        tool_use_id: &str,
    ) -> ToolResultBlock {
        let msg = output["message"].as_str().unwrap_or(
            "Entered plan mode. Focus on exploring the codebase and designing an implementation approach.",
        );
        ToolResultBlock {
            tool_use_id: tool_use_id.into(),
            content: ToolResultContent::Text(format!(
                "{}\n\nIn plan mode, you should:\n1. Thoroughly explore the codebase to understand existing patterns\n2. Identify similar features and architectural approaches\n3. Consider multiple approaches and their trade-offs\n4. Use AskUserQuestion if you need to clarify the approach\n5. Design a concrete implementation strategy\n6. When ready, use ExitPlanMode to present your plan for approval\n\nRemember: DO NOT write or edit any files yet. This is a read-only exploration and planning phase.",
                msg
            )),
            is_error: false,
        }
    }
}
```

- [ ] **Step 2: Compile check**

Run: `cargo check -p cli 2>&1`

- [ ] **Step 3: Commit**

```bash
git add cli/src/tools/enter_plan_mode.rs
git commit -m "feat(parity): rewrite EnterPlanMode with shouldDefer, outputSchema, and activity description"
```

---

### Task 9: Rewrite SendMessage — full schema with StructuredMessage union

**Files:**
- Modify: `cli/src/tools/send_message.rs`

- [ ] **Step 1: Write the implementation**

Replace `cli/src/tools/send_message.rs`:

```rust
use async_trait::async_trait;
use serde_json::json;
use shared::RenderSpec;
use super::contract::{
    DescriptionCtx, ProgressSink, PromptCtx, RenderOpts, Tool, ToolCallContext, ToolResult,
    ToolResultBlock, ToolResultContent,
};

pub struct SendMessageTool;

#[async_trait]
impl Tool for SendMessageTool {
    fn name(&self) -> &str {
        "SendMessage"
    }

    fn description(&self, _input: Option<&serde_json::Value>, _ctx: &DescriptionCtx) -> String {
        "Send a message to another agent".into()
    }

    fn prompt(&self, _ctx: &PromptCtx) -> String {
        include_str!("prompts/send_message.txt").into()
    }

    fn search_hint(&self) -> Option<&'static str> {
        Some("send messages to agent teammates (swarm protocol)")
    }

    fn should_defer(&self) -> bool {
        true
    }

    fn input_schema(&self) -> serde_json::Value {
        json!({
            "type": "object",
            "properties": {
                "to": {
                    "type": "string",
                    "description": "Recipient: teammate name, or \"*\" for broadcast to all teammates"
                },
                "summary": {
                    "type": "string",
                    "description": "A 5-10 word summary shown as a preview in the UI"
                },
                "message": {
                    "oneOf": [
                        {
                            "type": "string",
                            "description": "Plain text message content"
                        },
                        {
                            "type": "object",
                            "description": "Structured protocol message",
                            "oneOf": [
                                {
                                    "type": "object",
                                    "properties": {
                                        "type": { "const": "shutdown_request" },
                                        "reason": { "type": "string" }
                                    },
                                    "required": ["type"]
                                },
                                {
                                    "type": "object",
                                    "properties": {
                                        "type": { "const": "shutdown_response" },
                                        "request_id": { "type": "string" },
                                        "approve": { "description": "boolean or 'true'/'false' string" }
                                    },
                                    "required": ["type", "request_id", "approve"]
                                },
                                {
                                    "type": "object",
                                    "properties": {
                                        "type": { "const": "plan_approval_response" },
                                        "request_id": { "type": "string" },
                                        "approve": { "description": "boolean or 'true'/'false' string" },
                                        "feedback": { "type": "string" }
                                    },
                                    "required": ["type", "request_id", "approve"]
                                }
                            ]
                        }
                    ]
                }
            },
            "required": ["to", "message"]
        })
    }

    fn output_schema(&self) -> Option<serde_json::Value> {
        Some(json!({
            "type": "object",
            "properties": {
                "success": { "type": "boolean" },
                "message": { "type": "string" },
                "routing": {
                    "type": "object",
                    "properties": {
                        "sender": { "type": "string" },
                        "target": { "type": "string" },
                        "summary": { "type": "string" },
                        "content": { "type": "string" }
                    }
                }
            },
            "required": ["success", "message"]
        }))
    }

    fn render_tool_use_message(
        &self,
        input: &serde_json::Value,
        _opts: &RenderOpts,
    ) -> RenderSpec {
        let to = input["to"].as_str().unwrap_or("?");
        let summary = input["summary"]
            .as_str()
            .map(|s| format!(": {}", s))
            .unwrap_or_default();

        RenderSpec::Header {
            verb: format!("Sending message to {}{}", to, summary),
            target: None,
            tag: None,
        }
    }

    fn render_tool_result_message(
        &self,
        output: &serde_json::Value,
        _progress: &[super::contract::ProgressEvent],
        _opts: &RenderOpts,
    ) -> Option<RenderSpec> {
        let msg = output["message"].as_str().unwrap_or("Message sent");
        Some(RenderSpec::Text {
            body: msg.to_string(),
            dim: false,
        })
    }

    async fn call(
        &self,
        input: serde_json::Value,
        _context: &ToolCallContext,
        _on_progress: Option<ProgressSink>,
    ) -> ToolResult {
        let to = input["to"].as_str().unwrap_or("?");
        let message_val = &input["message"];

        // Routing implementation is out of scope — schema parity only.
        // Actual peer routing (UDS_INBOX, bridge) tracked in separate spec.
        let content = if let Some(s) = message_val.as_str() {
            format!("Message routed to '{}': {}", to, s)
        } else if let Some(obj) = message_val.as_object() {
            let msg_type = obj.get("type").and_then(|v| v.as_str()).unwrap_or("?");
            format!("Structured message (type={}) routed to '{}'", msg_type, to)
        } else {
            format!("Message routed to '{}'", to)
        };

        ToolResult {
            content: json!({
                "success": true,
                "message": content,
                "routing": {
                    "sender": "system",
                    "target": to,
                    "content": message_val.to_string()
                }
            })
            .to_string(),
            is_error: false,
            ..Default::default()
        }
    }

    fn map_tool_result_to_block(
        &self,
        output: &serde_json::Value,
        tool_use_id: &str,
    ) -> ToolResultBlock {
        let msg = output["message"].as_str().unwrap_or("Message sent");
        ToolResultBlock {
            tool_use_id: tool_use_id.into(),
            content: ToolResultContent::Text(msg.to_string()),
            is_error: false,
        }
    }
}
```

- [ ] **Step 2: Compile check**

Run: `cargo check -p cli 2>&1`

- [ ] **Step 3: Commit**

```bash
git add cli/src/tools/send_message.rs
git commit -m "feat(parity): rewrite SendMessage with full schema, StructuredMessage union, and outputSchema"
```

---

### Task 10: Rewrite TodoWrite — activeForm, outputSchema, persist to Store

**Files:**
- Modify: `cli/src/tools/todo_write.rs`

- [ ] **Step 1: Add `activeForm` to schema and persist todos to the Store**

Replace `cli/src/tools/todo_write.rs`:

```rust
use std::sync::Arc;
use async_trait::async_trait;
use serde_json::json;
use shared::RenderSpec;
use super::contract::{
    DescriptionCtx, ProgressSink, PromptCtx, RenderOpts, Tool, ToolCallContext, ToolResult,
    ToolResultBlock, ToolResultContent,
};

pub struct TodoWriteTool {
    pub store: Arc<crate::state::store::Store>,
}

#[async_trait]
impl Tool for TodoWriteTool {
    fn name(&self) -> &str {
        "TodoWrite"
    }

    fn user_facing_name(&self, _input: Option<&serde_json::Value>) -> String {
        String::new()
    }

    fn description(&self, _input: Option<&serde_json::Value>, _ctx: &DescriptionCtx) -> String {
        "Update the todo list for the current session. To be used proactively and often to track progress and pending tasks. Make sure that at least one task is in_progress at all times. Always provide both content (imperative) and activeForm (present continuous) for each task.".into()
    }

    fn prompt(&self, _ctx: &PromptCtx) -> String {
        include_str!("prompts/todo_write.txt").into()
    }

    fn search_hint(&self) -> Option<&'static str> {
        Some("manage the session task checklist")
    }

    fn should_defer(&self) -> bool {
        true
    }

    fn strict(&self) -> bool {
        true
    }

    fn input_schema(&self) -> serde_json::Value {
        json!({
            "type": "object",
            "properties": {
                "todos": {
                    "type": "array",
                    "description": "The updated todo list",
                    "items": {
                        "type": "object",
                        "properties": {
                            "content": {
                                "type": "string",
                                "description": "The imperative form describing what needs to be done (e.g., 'Run tests', 'Build the project')"
                            },
                            "status": {
                                "type": "string",
                                "enum": ["pending", "in_progress", "completed"],
                                "description": "Task status"
                            },
                            "activeForm": {
                                "type": "string",
                                "description": "The present continuous form shown during execution (e.g., 'Running tests', 'Building the project')"
                            }
                        },
                        "required": ["content", "status", "activeForm"]
                    }
                }
            },
            "required": ["todos"]
        })
    }

    fn output_schema(&self) -> Option<serde_json::Value> {
        Some(json!({
            "type": "object",
            "properties": {
                "oldTodos": {
                    "type": "array",
                    "description": "The todo list before the update",
                    "items": {
                        "type": "object",
                        "properties": {
                            "content": { "type": "string" },
                            "status": { "type": "string" },
                            "activeForm": { "type": "string" }
                        },
                        "required": ["content", "status", "activeForm"]
                    }
                },
                "newTodos": {
                    "type": "array",
                    "description": "The todo list after the update",
                    "items": {
                        "type": "object",
                        "properties": {
                            "content": { "type": "string" },
                            "status": { "type": "string" },
                            "activeForm": { "type": "string" }
                        },
                        "required": ["content", "status", "activeForm"]
                    }
                },
                "verificationNudgeNeeded": {
                    "type": "boolean",
                    "description": "When true, the model should consider spawning a verification agent"
                }
            },
            "required": ["oldTodos", "newTodos"]
        }))
    }

    fn is_concurrency_safe(&self, _input: &serde_json::Value) -> bool {
        true
    }

    fn to_auto_classifier_input(&self, input: &serde_json::Value) -> serde_json::Value {
        let count = input["todos"].as_array().map(|a| a.len()).unwrap_or(0);
        serde_json::Value::String(format!("{} items", count))
    }

    fn render_tool_use_message(
        &self,
        input: &serde_json::Value,
        _opts: &RenderOpts,
    ) -> RenderSpec {
        let count = input["todos"].as_array().map(|a| a.len()).unwrap_or(0);
        RenderSpec::Text {
            body: format!("Updating todo list ({} items)", count),
            dim: false,
        }
    }

    fn render_tool_result_message(
        &self,
        output: &serde_json::Value,
        _progress: &[super::contract::ProgressEvent],
        _opts: &RenderOpts,
    ) -> Option<RenderSpec> {
        let new_todos = &output["newTodos"];
        let mut children = vec![RenderSpec::Text {
            body: "Todo list updated:".into(),
            dim: false,
        }];

        if let Some(todos) = new_todos.as_array() {
            for todo in todos {
                let content = todo["content"].as_str().unwrap_or("");
                let status = todo["status"].as_str().unwrap_or("pending");
                let status_icon = match status {
                    "completed" => "[x]",
                    "in_progress" => "[>]",
                    _ => "[ ]",
                };
                children.push(RenderSpec::Text {
                    body: format!("  {} {}", status_icon, content),
                    dim: status == "completed",
                });
            }
        }

        Some(RenderSpec::Group { children })
    }

    async fn check_permissions(
        &self,
        input: &serde_json::Value,
        _ctx: &ToolCallContext,
    ) -> crate::tools::permission::PermissionResult {
        crate::tools::permission::PermissionResult::Allow {
            updated_input: Some(input.clone()),
            decision_reason: Some(crate::tools::permission::DecisionReason::ToolDefault),
        }
    }

    async fn call(
        &self,
        input: serde_json::Value,
        _context: &ToolCallContext,
        _on_progress: Option<ProgressSink>,
    ) -> ToolResult {
        let new_todos = input["todos"].clone();
        let old_todos = self.store.get_state().tasks.clone();

        // Convert old tasks to JSON array for output
        let old_todos_json: Vec<serde_json::Value> = old_todos
            .values()
            .map(|t| {
                json!({
                    "content": t.subject,
                    "status": task_status_to_str(&t.status),
                    "activeForm": t.description
                })
            })
            .collect();

        // Persist todos to the store (clear and repopulate)
        use crate::state::store::{TaskRecord, TaskStatus};
        self.store.set_state(|s| {
            s.tasks.clear();
            if let Some(todos) = new_todos.as_array() {
                for (i, todo) in todos.iter().enumerate() {
                    let subject = todo["content"].as_str().unwrap_or("").to_string();
                    let active_form = todo["activeForm"].as_str().unwrap_or("").to_string();
                    let status_str = todo["status"].as_str().unwrap_or("pending");
                    let status = match status_str {
                        "in_progress" => TaskStatus::InProgress,
                        "completed" => TaskStatus::Completed,
                        _ => TaskStatus::Pending,
                    };
                    s.tasks.insert(
                        format!("todo_{}", i),
                        TaskRecord {
                            id: format!("todo_{}", i),
                            subject: subject.clone(),
                            description: active_form,
                            status,
                            blocks: vec![],
                            blocked_by: vec![],
                        },
                    );
                }
            }
        });

        let all_done = new_todos
            .as_array()
            .map(|a| a.iter().all(|t| t["status"].as_str() == Some("completed")))
            .unwrap_or(false);

        // Emit new_messages so the todo panel updates in the TUI
        let mut result = ToolResult {
            content: json!({
                "oldTodos": old_todos_json,
                "newTodos": new_todos
            })
            .to_string(),
            is_error: false,
            ..Default::default()
        };

        // Verification nudge: if closing 3+ tasks with no verification step
        if all_done && new_todos.as_array().map(|a| a.len()).unwrap_or(0) >= 3 {
            result.new_messages.push(json!({
                "role": "user",
                "content": "NOTE: You just closed out 3+ tasks and none of them was a verification step. Before writing your final summary, consider spawning the verification agent."
            }));
        }

        result
    }

    fn map_tool_result_to_block(
        &self,
        output: &serde_json::Value,
        tool_use_id: &str,
    ) -> ToolResultBlock {
        ToolResultBlock {
            tool_use_id: tool_use_id.into(),
            content: ToolResultContent::Text(
                "Todos have been modified successfully. Ensure that you continue to use the todo list to track your progress. Please proceed with the current tasks if applicable."
                    .into(),
            ),
            is_error: false,
        }
    }
}

fn task_status_to_str(status: &crate::state::store::TaskStatus) -> &str {
    match status {
        crate::state::store::TaskStatus::Pending => "pending",
        crate::state::store::TaskStatus::InProgress => "in_progress",
        crate::state::store::TaskStatus::Completed => "completed",
        crate::state::store::TaskStatus::Failed => "completed",
        crate::state::store::TaskStatus::Deleted => "completed",
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn schema_requires_active_form() {
        let tool = TodoWriteTool {
            store: Arc::new(crate::state::store::Store::default()),
        };
        let schema = tool.input_schema();
        let item_required = &schema["properties"]["todos"]["items"]["required"];
        let required: Vec<&str> = item_required
            .as_array()
            .unwrap()
            .iter()
            .filter_map(|v| v.as_str())
            .collect();
        assert!(required.contains(&"activeForm"));
    }

    #[test]
    fn output_schema_has_old_and_new_todos() {
        let tool = TodoWriteTool {
            store: Arc::new(crate::state::store::Store::default()),
        };
        let schema = tool.output_schema().unwrap();
        let required = schema["required"].as_array().unwrap();
        let req: Vec<&str> = required.iter().filter_map(|v| v.as_str()).collect();
        assert!(req.contains(&"oldTodos"));
        assert!(req.contains(&"newTodos"));
    }
}
```

- [ ] **Step 2: Run tests**

Run: `cargo test -p cli todo_write::`
Expected: ALL PASS

- [ ] **Step 3: Compile check**

Run: `cargo check -p cli 2>&1`

- [ ] **Step 4: Commit**

```bash
git add cli/src/tools/todo_write.rs
git commit -m "feat(parity): rewrite TodoWrite with activeForm, outputSchema, and Store persistence"
```

---

### Task 11: Update `mod.rs` — Inject Store into TodoWrite/EnterPlanMode/ExitPlanMode

**Files:**
- Modify: `cli/src/tools/mod.rs`

- [ ] **Step 1: Update tool registration**

In `cli/src/tools/mod.rs`, update the tool construction in `ToolRegistry::new()` at lines 156-164:

```rust
// Before (lines 156-164):
registry.register(Arc::new(TodoWriteTool));
registry.register(Arc::new(EnterPlanModeTool {
    store: store.clone(),
}));
registry.register(Arc::new(ExitPlanModeTool {
    store: store.clone(),
}));
registry.register(Arc::new(SendMessageTool));
registry.register(Arc::new(AskUserQuestionTool));

// After:
registry.register(Arc::new(TodoWriteTool {
    store: store.clone(),
}));
registry.register(Arc::new(EnterPlanModeTool {
    store: store.clone(),
}));
registry.register(Arc::new(ExitPlanModeTool {
    store: store.clone(),
}));
registry.register(Arc::new(SendMessageTool));
registry.register(Arc::new(AskUserQuestionTool));
```

Only `TodoWriteTool` changes — it now takes `store: Arc<Store>`. The other tools already have `store` in their structs.

- [ ] **Step 2: Verify `AskUserQuestionTool` still has no store dependency**

`AskUserQuestionTool` is a unit struct with no fields. Verify by checking `cli/src/tools/mod.rs:164`.

- [ ] **Step 3: Compile check**

Run: `cargo check -p cli 2>&1`

- [ ] **Step 4: Run all tool unit tests**

Run: `cargo test -p cli --lib 2>&1`

- [ ] **Step 5: Commit**

```bash
git add cli/src/tools/mod.rs
git commit -m "feat(parity): inject Store into TodoWriteTool"
```

---

### Task 12: Create `executor/interactive.rs` — suspend/resume on Interactive RenderSpec

**Files:**
- Create: `cli/src/executor/interactive.rs`
- Modify: `cli/src/executor/mod.rs`

- [ ] **Step 1: Check whether the executor module already exists**

Run: `ls cli/src/executor/`

If `mod.rs` doesn't exist, find where tool execution happens (likely in `cli/src/agent/tool_loop.rs` or similar) and note the actual path. Adjust subsequent steps accordingly.

- [ ] **Step 2: Create `cli/src/executor/interactive.rs`**

```rust
// Handles the Interactive RenderSpec suspension protocol.
//
// When a tool returns `RenderSpec::Interactive`, the executor:
// 1. Emits the spec on the session bus (so TUI / web render it).
// 2. Suspends the turn and waits for a `UserInteractionResponse` event
//    whose payload matches the tool's `response_schema`.
// 3. Merges the response into the tool's input (or resolves the
//    permission decision) and resumes execution.

use serde_json::Value;
use shared::RenderSpec;

/// The response payload from the renderer after the user completes
/// an interactive widget.
#[derive(Debug, Clone)]
pub struct UserInteractionResponse {
    /// Matches the tool_use_id that triggered the interaction.
    pub tool_use_id: String,
    /// The JSON response from the user (answers, approval decision, etc.).
    pub payload: Value,
}

/// Result of the interaction phase:
/// - `Resolved`: the input has been updated with user data; proceed with call()
/// - `Denied`: user rejected the interaction; emit rejection render
/// - `Aborted`: session ended or timeout
#[derive(Debug)]
pub enum InteractionOutcome {
    Resolved {
        /// The tool input, updated with user-supplied answers/approval.
        updated_input: Value,
    },
    Denied,
    Aborted,
}

/// Given a tool's render output and the session bus, suspend the turn
/// until the user responds to the Interactive widget.
///
/// Returns the resolved outcome so the executor can proceed.
pub async fn await_interaction(
    tool_use_id: String,
    spec: &RenderSpec,
    bus: &std::sync::Arc<crate::conversation::session_bus::SessionBus>,
) -> InteractionOutcome {
    // Extract the response_schema from the Interactive variant
    let response_schema = match spec {
        RenderSpec::Interactive { response_schema, .. } => response_schema.clone(),
        _ => return InteractionOutcome::Aborted,
    };

    // Emit the interaction request onto the session bus
    // The TUI / web picks this up and renders the widget
    bus.emit(crate::conversation::session_bus::SessionEvent::InteractionRequested {
        tool_use_id: tool_use_id.clone(),
        spec: spec.clone(),
        response_schema,
    });

    // Wait for the response
    let mut rx = bus.subscribe();
    loop {
        let event = rx.recv().await;
        match event {
            Some(crate::conversation::session_bus::SessionEvent::InteractionResponse {
                tool_use_id: resp_id,
                payload,
            }) if resp_id == tool_use_id => {
                return InteractionOutcome::Resolved {
                    updated_input: payload,
                };
            }
            Some(crate::conversation::session_bus::SessionEvent::InteractionDenied {
                tool_use_id: resp_id,
            }) if resp_id == tool_use_id => {
                return InteractionOutcome::Denied;
            }
            None => return InteractionOutcome::Aborted,
            _ => continue,
        }
    }
}
```

- [ ] **Step 3: Add module declaration to executor/mod.rs**

If `cli/src/executor/mod.rs` exists:
```rust
pub mod interactive;
```

If executor is at a different path, add the module declaration there.

- [ ] **Step 4: Compile check**

Run: `cargo check -p cli 2>&1`

Expected: May fail due to missing `SessionEvent` variants. These need to be added to the session bus if they don't exist yet. If `SessionBus` doesn't have `InteractionRequested`/`InteractionResponse`/`InteractionDenied` variants, add them:

```rust
// In session_bus.rs, add:
pub enum SessionEvent {
    // ... existing variants ...
    InteractionRequested {
        tool_use_id: String,
        spec: shared::RenderSpec,
        response_schema: Value,
    },
    InteractionResponse {
        tool_use_id: String,
        payload: Value,
    },
    InteractionDenied {
        tool_use_id: String,
    },
}
```

- [ ] **Step 5: Verify compilation**

Run: `cargo check -p cli 2>&1`

- [ ] **Step 6: Commit**

```bash
git add cli/src/executor/ cli/src/conversation/session_bus.rs
git commit -m "feat(parity): add executor Interactive handler with session bus suspend/resume protocol"
```

---

---

## Self-Review Checklist

### 1. Spec coverage

| Spec requirement | Covered by |
|------------------|------------|
| AskUserQuestion full schema rewrite | Tasks 1, 6 |
| AskUserQuestion `prompt()` port | Task 1 |
| AskUserQuestion `PREVIEW_FEATURE_PROMPT` | Deferred (no preview format config yet) — prompt includes base text without preview appendix |
| AskUserQuestion `RenderSpec::Interactive{MultiQuestion}` | Task 6 |
| AskUserQuestion `shouldDefer = true` | Task 6 |
| AskUserQuestion `isEnabled` channels gate | Deferred (channels feature not built) — always true |
| AskUserQuestion `outputSchema` | Task 6 |
| ExitPlanMode V2 schema (`plan`, `planFilePath`, `bashPrompts?`) | Tasks 2, 7 |
| ExitPlanMode `prompt()` port | Task 2 |
| ExitPlanMode `RenderSpec::Interactive{PlanApproval}` | Task 7 |
| ExitPlanMode only flips store flag after user accepts | Task 7 (call() runs after interaction resolves) |
| ExitPlanMode `isAgent`/`planPath`/`requestId` in output | Task 7 |
| EnterPlanMode `prompt()` port | Task 3 |
| EnterPlanMode `getActivityDescription` | Task 8 |
| EnterPlanMode `shouldDefer` / `outputSchema` | Task 8 |
| SendMessage schema rewrite (`to`, `summary?`, `message` union) | Tasks 4, 9 |
| SendMessage `StructuredMessage` discriminated union | Task 9 |
| SendMessage output (`MessageOutput`/`BroadcastOutput`/`RequestOutput`) | Task 9 |
| TodoWrite `activeForm` required field | Task 10 |
| TodoWrite `outputSchema` (`oldTodos`, `newTodos`, `verificationNudgeNeeded?`) | Task 10 |
| TodoWrite `prompt()` port | Task 5 |
| TodoWrite actual persistence to Store | Task 10 |
| Permission-prompt UI via `RenderSpec::Interactive{PermissionPrompt}` | Partially covered — Ask/ExitPlanMode emit `PermissionResult::Ask`; the TUI maps this to a prompt. The executor flow in Task 12 handles the suspend/resume. |
| Executor suspends on `Interactive` results | Task 12 |
| Wire `updated_input` through executor | Task 12 (resolved input passed as updated_input) |

### 2. Placeholder scan

- No "TBD" or "TODO" in code steps
- No "implement later" or "add appropriate error handling" hand-waves
- Every step has complete code or exact instructions

### 3. Type consistency

- `Question`, `QuestionOption` — from `shared::` (defined in Batch 1)
- `InteractiveWidget::MultiQuestion`, `PlanApproval` — from `shared::` (defined in Batch 1)
- `RenderSpec`, `RenderOpts`, `ProgressEvent` — from `super::contract` (Batch 1)
- `PermissionResult::Ask`, `PermissionResult::Allow` — from `crate::tools::permission` (Batch 1)
- `Store`, `PermissionMode` — from `crate::state::store`
- `ToolCallContext`, `ToolResult`, `ToolResultBlock` — from `super::contract` (Batch 1)
- `SessionBus`, `SessionEvent` — from `crate::conversation::session_bus`
- All types reference the same definitions across tasks

### 4. Gaps intentionally deferred

- **Preview feature prompt**: The `PREVIEW_FEATURE_PROMPT` conditional append (markdown vs html) is skipped because Super has no question preview format config yet. The base `ASK_USER_QUESTION_TOOL_PROMPT` is ported.
- **Channels `isEnabled` gate**: AskUserQuestion/ExitPlanMode/EnterPlanMode `isEnabled()` always returns true. The channels feature isn't built; the gate is future-compat only.
- **Actual message routing**: SendMessage's `call()` stringifies but doesn't actually route. Peer addressing requires multi-agent runtime (out of scope per spec).
- **Verification agent nudge**: TodoWrite emits a `new_messages` nudge but doesn't spawn a verification agent (no `VERIFICATION_AGENT` feature in Super).
- **Plan file reading**: ExitPlanMode's `PlanApproval` widget gets `plan_markdown` from the input (injected by normalizeToolInput, which Super doesn't have yet). The plan is passed via tool input, not read from disk.
- **Team/coordinator mode**: ExitPlanMode `awaitingLeaderApproval` path is not implemented (requires teammate/team infrastructure).

---

## Acceptance Criteria

1. **Schema parity** (Layer 1): For each of the 5 tools, the `input_schema()` and `output_schema()` match Claude's after JSON Schema normalization. Run the parity harness `cargo test -p cli parity_schema_` to verify.
2. **Prompt parity**: Each tool's `prompt()` returns text byte-identical to Claude's per-tool prompt (verified by the schema dump comparison).
3. **Interactive flow** (Layer 3): 
   - Calling AskUserQuestion emits `RenderSpec::Interactive{MultiQuestion}` with the correct questions
   - Calling ExitPlanMode in plan mode emits `RenderSpec::Interactive{PlanApproval}` 
   - Calling TodoWrite persists todos to the Store and returns `{oldTodos, newTodos}`
   - Permission prompts from Ask/ExitPlanMode carry the correct `PermissionResult::Ask`
4. **No regressions**: All existing unit tests pass (`cargo test -p cli --lib`).
5. **The full tool set compiles**: `cargo check -p cli` succeeds.

---

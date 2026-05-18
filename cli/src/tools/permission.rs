use crate::state::store::PermissionMode;
use serde::{Deserialize, Serialize};
use serde_json::Value;

// ── New permission result types (Batch 1) ────────────────────────────────

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "behavior", rename_all = "snake_case")]
pub enum PermissionResult {
    Allow {
        #[serde(skip_serializing_if = "Option::is_none")]
        updated_input: Option<Value>,
        #[serde(skip_serializing_if = "Option::is_none")]
        decision_reason: Option<DecisionReason>,
    },
    Deny {
        reason: String,
        #[serde(skip_serializing_if = "Option::is_none")]
        decision_reason: Option<DecisionReason>,
    },
    Ask {
        #[serde(skip_serializing_if = "Option::is_none")]
        updated_input: Option<Value>,
        rule_suggestions: Vec<RuleSuggestion>,
    },
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum DecisionReason {
    Rule {
        source: RuleSource,
        pattern: String,
    },
    Hook {
        hook_name: String,
    },
    Mode {
        mode: crate::state::store::PermissionMode,
    },
    Classifier {
        confidence: f32,
    },
    ToolDefault,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RuleSuggestion {
    /// Human-readable label for the prompt UI ("Always allow `git *`").
    pub label: String,
    pub rule: PermissionRule,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum RuleBehavior {
    Allow,
    Deny,
    Ask,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PermissionRule {
    pub tool_name: String,
    /// Input-matcher pattern. None = match any input for this tool.
    /// Tool-specific syntax interpreted by `prepare_permission_matcher`.
    /// Examples: "git *", "git status", "Read(/etc/*)".
    pub content: Option<String>,
    pub behavior: RuleBehavior,
    pub source: RuleSource,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum RuleSource {
    User,
    Project,
    Local,
    Flag,
    Policy,
    Session,
    BuiltIn,
}

impl RuleSource {
    pub fn priority(&self) -> u8 {
        match self {
            Self::Policy => 7,
            Self::Flag => 6,
            Self::Session => 5,
            Self::Local => 4,
            Self::Project => 3,
            Self::User => 2,
            Self::BuiltIn => 1,
        }
    }
}

pub struct PermissionSystem {
    rules: Vec<PermissionRule>,
    mode: PermissionMode,
}

impl PermissionSystem {
    pub fn new(mode: PermissionMode) -> Self {
        Self {
            rules: Vec::new(),
            mode,
        }
    }

    pub fn set_mode(&mut self, mode: PermissionMode) {
        self.mode = mode;
    }

    pub fn add_rule(&mut self, rule: PermissionRule) {
        self.rules.push(rule);
    }

    pub async fn evaluate(
        &self,
        tool: &dyn crate::tools::contract::Tool,
        input: &serde_json::Value,
        ctx: &crate::tools::contract::ToolCallContext,
    ) -> PermissionResult {
        // 1. Mode shortcuts.
        match self.mode {
            PermissionMode::BypassPermissions => {
                return PermissionResult::Allow {
                    updated_input: None,
                    decision_reason: Some(DecisionReason::Mode { mode: self.mode }),
                };
            }
            PermissionMode::Plan if !tool.is_read_only(input) => {
                return PermissionResult::Deny {
                    reason: "Plan mode allows read-only tools only.".into(),
                    decision_reason: Some(DecisionReason::Mode { mode: self.mode }),
                };
            }
            _ => {}
        }

        // 2. Rule matching with input-aware patterns.
        let tool_name = tool.name();
        let matcher = tool.prepare_permission_matcher(input).await;
        let mut sorted_rules = self.rules.clone();
        sorted_rules.sort_by_key(|r| -(r.source.priority() as i32));

        for rule in &sorted_rules {
            if rule.tool_name != tool_name {
                continue;
            }
            let matches = match (&rule.content, &matcher) {
                (None, _) => true,
                (Some(_), None) => false,
                (Some(pat), Some(m)) => m(pat),
            };
            if !matches {
                continue;
            }
            let reason = Some(DecisionReason::Rule {
                source: rule.source,
                pattern: rule.content.clone().unwrap_or_else(|| tool_name.into()),
            });
            return match rule.behavior {
                RuleBehavior::Allow => PermissionResult::Allow {
                    updated_input: None,
                    decision_reason: reason,
                },
                RuleBehavior::Deny => PermissionResult::Deny {
                    reason: format!("Denied by {:?} rule.", rule.source),
                    decision_reason: reason,
                },
                RuleBehavior::Ask if ctx.auto_deny_prompts => PermissionResult::Deny {
                    reason: "Permission denied: async subagents cannot prompt the user.".into(),
                    decision_reason: reason,
                },
                RuleBehavior::Ask => PermissionResult::Ask {
                    updated_input: None,
                    rule_suggestions: vec![],
                },
            };
        }

        // 3. Auto mode (was DontAsk): anything that would normally Ask is auto-denied.
        if matches!(self.mode, PermissionMode::Auto)
            && matches!(
                tool.check_permissions(input, ctx).await,
                PermissionResult::Ask { .. }
            )
        {
            return PermissionResult::Deny {
                reason: "Auto mode: prompts are auto-denied.".into(),
                decision_reason: Some(DecisionReason::Mode { mode: self.mode }),
            };
        }

        // 4. Defer to the tool's own check.
        tool.check_permissions(input, ctx).await
    }
}

#[cfg(test)]
mod permission_result_tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn allow_serializes_with_behavior_tag() {
        let r = PermissionResult::Allow {
            updated_input: Some(json!({"path": "/tmp/x"})),
            decision_reason: Some(DecisionReason::ToolDefault),
        };
        let s = serde_json::to_string(&r).unwrap();
        assert!(s.contains(r#""behavior":"allow""#));
        assert!(s.contains(r#""updated_input""#));
    }

    #[test]
    fn deny_serializes_with_reason() {
        let r = PermissionResult::Deny {
            reason: "blocked".into(),
            decision_reason: None,
        };
        let s = serde_json::to_string(&r).unwrap();
        assert!(s.contains(r#""behavior":"deny""#));
        assert!(s.contains(r#""reason":"blocked""#));
    }

    #[test]
    fn ask_includes_empty_rule_suggestions_when_none() {
        let r = PermissionResult::Ask {
            updated_input: None,
            rule_suggestions: vec![],
        };
        let s = serde_json::to_string(&r).unwrap();
        assert!(s.contains(r#""behavior":"ask""#));
        assert!(s.contains(r#""rule_suggestions":[]"#));
    }
}

#[cfg(test)]
mod evaluate_v2_tests {
    use super::*;
    use crate::state::store::PermissionMode;
    use crate::tools::contract::{
        DescriptionCtx, ProgressSink, PromptCtx, Tool, ToolCallContext, ToolResult,
        ToolResultBlock, ToolResultContent,
    };
    use async_trait::async_trait;
    use serde_json::{json, Value};

    struct DummyTool {
        name: &'static str,
        read_only: bool,
    }

    #[async_trait]
    impl Tool for DummyTool {
        fn name(&self) -> &str {
            self.name
        }
        fn description(&self, _input: Option<&Value>, _ctx: &DescriptionCtx) -> String {
            "test".into()
        }
        fn prompt(&self, _ctx: &PromptCtx) -> String {
            "".into()
        }
        fn input_schema(&self) -> Value {
            json!({"type":"object"})
        }
        fn is_read_only(&self, _input: &Value) -> bool {
            self.read_only
        }
        async fn check_permissions(
            &self,
            _input: &Value,
            _ctx: &ToolCallContext,
        ) -> PermissionResult {
            PermissionResult::Allow {
                updated_input: None,
                decision_reason: Some(DecisionReason::ToolDefault),
            }
        }
        async fn call(
            &self,
            _input: Value,
            _ctx: &ToolCallContext,
            _progress: Option<ProgressSink>,
        ) -> ToolResult {
            unreachable!("not called in this test")
        }
        fn render_tool_use_message(
            &self,
            _input: &Value,
            _opts: &crate::tools::contract::RenderOpts,
        ) -> shared::RenderSpec {
            shared::RenderSpec::Nothing
        }
        fn map_tool_result_to_block(&self, _output: &Value, tool_use_id: &str) -> ToolResultBlock {
            ToolResultBlock {
                tool_use_id: tool_use_id.into(),
                content: ToolResultContent::Text("ok".into()),
                is_error: false,
            }
        }
    }

    #[tokio::test]
    async fn bypass_mode_returns_allow_with_mode_reason() {
        let sys = PermissionSystem::new(PermissionMode::BypassPermissions);
        let tool = DummyTool {
            name: "X",
            read_only: false,
        };
        let result = sys.evaluate(&tool, &json!({}), &dummy_ctx()).await;
        match result {
            PermissionResult::Allow {
                decision_reason: Some(DecisionReason::Mode { mode }),
                ..
            } => {
                assert_eq!(mode, PermissionMode::BypassPermissions);
            }
            other => panic!("expected Allow via Mode reason, got {:?}", other),
        }
    }

    #[tokio::test]
    async fn plan_mode_denies_non_read_only_tool() {
        let sys = PermissionSystem::new(PermissionMode::Plan);
        let tool = DummyTool {
            name: "Y",
            read_only: false,
        };
        let result = sys.evaluate(&tool, &json!({}), &dummy_ctx()).await;
        assert!(matches!(result, PermissionResult::Deny { .. }));
    }

    #[tokio::test]
    async fn plan_mode_allows_read_only_tool() {
        let sys = PermissionSystem::new(PermissionMode::Plan);
        let tool = DummyTool {
            name: "Z",
            read_only: true,
        };
        let result = sys.evaluate(&tool, &json!({}), &dummy_ctx()).await;
        assert!(matches!(result, PermissionResult::Allow { .. }));
    }

    #[tokio::test]
    async fn defaults_to_tool_check_when_no_rules_match() {
        let sys = PermissionSystem::new(PermissionMode::Default);
        let tool = DummyTool {
            name: "W",
            read_only: false,
        };
        let result = sys.evaluate(&tool, &json!({}), &dummy_ctx()).await;
        assert!(matches!(
            result,
            PermissionResult::Allow {
                decision_reason: Some(DecisionReason::ToolDefault),
                ..
            }
        ));
    }

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
}

#[cfg(test)]
mod matcher_tests {
    use super::*;
    use crate::state::store::PermissionMode;
    use crate::tools::contract::{
        DescriptionCtx, ProgressSink, PromptCtx, Tool, ToolCallContext, ToolResult,
    };
    use async_trait::async_trait;
    use serde_json::{json, Value};

    /// Stand-in for a Bash-style tool whose permission rules match on
    /// the `command` field rather than the tool name.
    struct CommandTool;

    #[async_trait]
    impl Tool for CommandTool {
        fn name(&self) -> &str {
            "Bash"
        }
        fn description(&self, _input: Option<&Value>, _ctx: &DescriptionCtx) -> String {
            "Bash test stand-in".into()
        }
        fn prompt(&self, _ctx: &PromptCtx) -> String {
            String::new()
        }
        fn input_schema(&self) -> Value {
            json!({"type":"object"})
        }

        async fn prepare_permission_matcher(
            &self,
            input: &Value,
        ) -> Option<Box<dyn Fn(&str) -> bool + Send + Sync>> {
            let command = input.get("command").and_then(|v| v.as_str())?.to_string();
            Some(Box::new(move |pattern: &str| {
                let cmd = command.as_str();
                let first = cmd.split_whitespace().next().unwrap_or("");
                match pattern.split_once(' ') {
                    None => first == pattern,
                    Some((stem, "*")) => first == stem,
                    Some(_) => cmd == pattern,
                }
            }))
        }

        async fn call(
            &self,
            _input: Value,
            _ctx: &ToolCallContext,
            _on_progress: Option<ProgressSink>,
        ) -> ToolResult {
            unreachable!()
        }
    }

    #[tokio::test]
    async fn rule_with_input_pattern_matches_command_stem() {
        let mut sys = PermissionSystem::new(PermissionMode::Default);
        sys.add_rule(PermissionRule {
            tool_name: "Bash".into(),
            content: Some("git *".into()),
            behavior: RuleBehavior::Allow,
            source: RuleSource::User,
        });
        let tool = CommandTool;
        let result = sys
            .evaluate(&tool, &json!({"command": "git status"}), &dummy_ctx())
            .await;
        match result {
            PermissionResult::Allow {
                decision_reason: Some(DecisionReason::Rule { pattern, .. }),
                ..
            } => {
                assert_eq!(pattern, "git *");
            }
            other => panic!("expected Allow via Rule(git *), got {:?}", other),
        }
    }

    #[tokio::test]
    async fn rule_with_input_pattern_does_not_match_other_command() {
        let mut sys = PermissionSystem::new(PermissionMode::Default);
        sys.add_rule(PermissionRule {
            tool_name: "Bash".into(),
            content: Some("git *".into()),
            behavior: RuleBehavior::Allow,
            source: RuleSource::User,
        });
        let tool = CommandTool;
        let result = sys
            .evaluate(&tool, &json!({"command": "rm -rf /tmp/x"}), &dummy_ctx())
            .await;
        // Falls through to the tool's check_permissions (default: Allow ToolDefault).
        assert!(matches!(
            result,
            PermissionResult::Allow {
                decision_reason: Some(DecisionReason::ToolDefault),
                ..
            }
        ));
    }

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
}

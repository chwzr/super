use serde::{Deserialize, Serialize};
use serde_json::Value;
use crate::state::store::PermissionMode;

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
    Rule { source: RuleSource, pattern: String },
    Hook { hook_name: String },
    Mode { mode: crate::state::store::PermissionMode },
    Classifier { confidence: f32 },
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

// ── Legacy types (deprecated) ────────────────────────────────────────────

#[deprecated(note = "Use PermissionResult instead. To be removed once all tools migrate.")]
#[derive(Debug, Clone, PartialEq)]
#[allow(dead_code)]
pub enum Decision {
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

    #[allow(deprecated)]
    pub fn evaluate_legacy(&self, tool_name: &str, _input: &serde_json::Value) -> Decision {
        // Mode-based shortcuts
        match self.mode {
            PermissionMode::BypassPermissions => return Decision::Allow,
            PermissionMode::Auto => {
                // Auto: auto-deny anything that would normally ask
            }
            _ => {}
        }

        // Sort rules by priority (highest first)
        let mut rules = self.rules.clone();
        rules.sort_by_key(|r| -(r.source.priority() as i32));

        for rule in &rules {
            if rule.tool_name == tool_name {
                return match rule.behavior {
                    RuleBehavior::Allow => Decision::Allow,
                    RuleBehavior::Deny => Decision::Deny,
                    RuleBehavior::Ask => Decision::Ask,
                };
            }
        }

        // No matching rule: ask user
        Decision::Ask
    }

    /// Input-aware evaluation. Returns Claude-shaped PermissionResult.
    /// Stub during Batch 1: defers to evaluate_legacy and lifts Decision
    /// to PermissionResult. The full input-aware path (rule pattern
    /// matching via prepare_permission_matcher) is wired in Task D.4.
    pub async fn evaluate(
        &self,
        tool: &dyn crate::tools::contract::Tool,
        input: &serde_json::Value,
        _ctx: &crate::tools::contract::ToolCallContext,
    ) -> PermissionResult {
        // Mode-based shortcuts
        match self.mode {
            PermissionMode::BypassPermissions => {
                return PermissionResult::Allow {
                    updated_input: None,
                    decision_reason: Some(DecisionReason::Mode {
                        mode: self.mode,
                    }),
                };
            }
            PermissionMode::Plan => {
                if !tool.is_read_only(input) {
                    return PermissionResult::Deny {
                        reason: "Plan mode: only read-only tools are allowed".into(),
                        decision_reason: Some(DecisionReason::Mode {
                            mode: self.mode,
                        }),
                    };
                }
                return PermissionResult::Allow {
                    updated_input: None,
                    decision_reason: Some(DecisionReason::Mode {
                        mode: self.mode,
                    }),
                };
            }
            _ => {}
        }

        // No matching rule: defer to the tool's own check.
        // Full rule-matching path is wired in Task D.4.
        let tool_result = tool.check_permissions(input, _ctx).await;
        match tool_result {
            PermissionResult::Allow { .. } => PermissionResult::Allow {
                updated_input: None,
                decision_reason: Some(DecisionReason::ToolDefault),
            },
            other => other,
        }
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
    use crate::tools::contract::{Tool, ToolCallContext, ToolResult, ToolResultBlock, ToolResultContent, DescriptionCtx, PromptCtx, ProgressSink};
    use async_trait::async_trait;
    use serde_json::{json, Value};

    struct DummyTool {
        name: &'static str,
        read_only: bool,
    }

    #[async_trait]
    impl Tool for DummyTool {
        fn name(&self) -> &str { self.name }
        fn description(&self, _input: Option<&Value>, _ctx: &DescriptionCtx) -> String {
            "test".into()
        }
        fn prompt(&self, _ctx: &PromptCtx) -> String { "".into() }
        fn input_schema(&self) -> Value { json!({"type":"object"}) }
        fn is_read_only(&self, _input: &Value) -> bool { self.read_only }
        async fn check_permissions(&self, _input: &Value, _ctx: &ToolCallContext) -> PermissionResult {
            PermissionResult::Allow { updated_input: None, decision_reason: Some(DecisionReason::ToolDefault) }
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
        fn map_tool_result_to_block(
            &self,
            _output: &Value,
            tool_use_id: &str,
        ) -> ToolResultBlock {
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
        let tool = DummyTool { name: "X", read_only: false };
        let result = sys.evaluate(&tool, &json!({}), &dummy_ctx()).await;
        match result {
            PermissionResult::Allow { decision_reason: Some(DecisionReason::Mode { mode }), .. } => {
                assert_eq!(mode, PermissionMode::BypassPermissions);
            }
            other => panic!("expected Allow via Mode reason, got {:?}", other),
        }
    }

    #[tokio::test]
    async fn plan_mode_denies_non_read_only_tool() {
        let sys = PermissionSystem::new(PermissionMode::Plan);
        let tool = DummyTool { name: "Y", read_only: false };
        let result = sys.evaluate(&tool, &json!({}), &dummy_ctx()).await;
        assert!(matches!(result, PermissionResult::Deny { .. }));
    }

    #[tokio::test]
    async fn plan_mode_allows_read_only_tool() {
        let sys = PermissionSystem::new(PermissionMode::Plan);
        let tool = DummyTool { name: "Z", read_only: true };
        let result = sys.evaluate(&tool, &json!({}), &dummy_ctx()).await;
        assert!(matches!(result, PermissionResult::Allow { .. }));
    }

    #[tokio::test]
    async fn defaults_to_tool_check_when_no_rules_match() {
        let sys = PermissionSystem::new(PermissionMode::Default);
        let tool = DummyTool { name: "W", read_only: false };
        let result = sys.evaluate(&tool, &json!({}), &dummy_ctx()).await;
        assert!(matches!(result, PermissionResult::Allow { decision_reason: Some(DecisionReason::ToolDefault), .. }));
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
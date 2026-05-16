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
    pub fn evaluate(&self, tool_name: &str, _input: &serde_json::Value) -> Decision {
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
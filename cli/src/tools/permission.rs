use regex::Regex;
use crate::state::store::PermissionMode;

#[derive(Debug, Clone, PartialEq)]
pub enum Decision {
    Allow,
    Deny,
    Ask,
}

#[derive(Debug, Clone)]
pub struct PermissionRule {
    pub source: RuleSource,
    pub pattern: String,
    pub decision: Decision,
    pub compiled: Regex,
}

#[derive(Debug, Clone, PartialEq, Eq)]
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

    pub fn evaluate(&self, tool_name: &str, _input: &serde_json::Value) -> Decision {
        // Mode-based shortcuts
        match self.mode {
            PermissionMode::Bypass => return Decision::Allow,
            PermissionMode::DontAsk => {
                // In DontAsk, auto-deny anything that would normally ask
            }
            _ => {}
        }

        // Sort rules by priority (highest first)
        let mut rules = self.rules.clone();
        rules.sort_by_key(|r| -(r.source.priority() as i32));

        for rule in &rules {
            if rule.compiled.is_match(tool_name) {
                return rule.decision.clone();
            }
        }

        // No matching rule: ask user
        Decision::Ask
    }
}
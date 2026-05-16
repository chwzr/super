use crate::state::store::PermissionMode;

/// Mirrors claude-code-src/tools/AgentTool/runAgent.ts lines 415-451.
///
/// Rules:
/// 1. If parent is in Bypass or AcceptEdits, parent always wins.
/// 2. Else if the agent definition specifies a mode, use the agent's.
/// 3. Else inherit the parent's.
///
/// `is_async` is accepted for symmetry with the TS source; in v1 the flag does
/// not change the returned mode itself (auto-deny behaviour for async is
/// handled by `ToolCallContext::auto_deny_prompts`, set by the caller).
pub fn resolve_permission_mode(
    parent: &PermissionMode,
    agent: Option<&PermissionMode>,
    _is_async: bool,
) -> PermissionMode {
    match parent {
        PermissionMode::BypassPermissions | PermissionMode::AcceptEdits => parent.clone(),
        _ => match agent {
            Some(m) => m.clone(),
            None => parent.clone(),
        },
    }
}

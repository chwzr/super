//! Placeholders for prompt text.
//!
//! Some text the model reads as guidance is not written yet: a tool
//! description, a parameter description, the fixed sentences of a notice, the
//! instructions of an agent type, a section of the system prompt. Each such
//! slot holds a placeholder instead, so that the wording can be written later
//! and put in its place.
//!
//! A placeholder has the form `SUPER_PROMPT_TODO(<key>)`. The key names the
//! slot:
//!
//! - a tool description: `Read.description`, from [`tool_description`]
//! - a parameter description: `Read.param.file_path`, from [`tool_parameter`];
//!   a nested parameter joins its path with dots, as in `Monitor.param.ws.url`
//! - anything else: `<Area>.<name>`, from [`prompt_todo`], for example
//!   `Notification.header`, `AgentType.Explore.instructions` or
//!   `SystemPrompt.advanced`
//!
//! Every slot is listed once in [`SLOTS`] with its text, so one search for
//! [`PREFIX`] finds every placeholder there. The functions below hand out the
//! text a slot has in that table; writing the prompt for a slot means putting
//! it in place of the slot's placeholder in the table. A new slot is added to
//! the table, in the order of its key; asking for a key the table does not
//! list is a bug that debug builds stop at.
//!
//! A placeholder is the whole text of its slot, with nothing before or after
//! it. Write one with these functions, never by hand, so the form stays the
//! same everywhere.
//!
//! An error message a tool returns is not prompt text and gets no placeholder.

/// The text every placeholder starts with.
pub const PREFIX: &str = "SUPER_PROMPT_TODO(";

/// Every slot of prompt text by its key, in the order of the keys, with the
/// text it has: its placeholder until the prompt is written.
pub const SLOTS: &[(&str, &str)] = &[
    ("Agent.description", "SUPER_PROMPT_TODO(Agent.description)"),
    (
        "Agent.param.description",
        "SUPER_PROMPT_TODO(Agent.param.description)",
    ),
    (
        "Agent.param.effort",
        "SUPER_PROMPT_TODO(Agent.param.effort)",
    ),
    (
        "Agent.param.isolation",
        "SUPER_PROMPT_TODO(Agent.param.isolation)",
    ),
    ("Agent.param.model", "SUPER_PROMPT_TODO(Agent.param.model)"),
    (
        "Agent.param.prompt",
        "SUPER_PROMPT_TODO(Agent.param.prompt)",
    ),
    (
        "Agent.param.run_in_background",
        "SUPER_PROMPT_TODO(Agent.param.run_in_background)",
    ),
    (
        "Agent.param.subagent_type",
        "SUPER_PROMPT_TODO(Agent.param.subagent_type)",
    ),
    (
        "Agent.system_section",
        "SUPER_PROMPT_TODO(Agent.system_section)",
    ),
    ("Agent.type_list", "SUPER_PROMPT_TODO(Agent.type_list)"),
    (
        "AgentType.Explore.description",
        "SUPER_PROMPT_TODO(AgentType.Explore.description)",
    ),
    (
        "AgentType.Explore.instructions",
        "SUPER_PROMPT_TODO(AgentType.Explore.instructions)",
    ),
    (
        "AgentType.Plan.description",
        "SUPER_PROMPT_TODO(AgentType.Plan.description)",
    ),
    (
        "AgentType.Plan.instructions",
        "SUPER_PROMPT_TODO(AgentType.Plan.instructions)",
    ),
    (
        "AgentType.fork.description",
        "SUPER_PROMPT_TODO(AgentType.fork.description)",
    ),
    (
        "AgentType.fork.instructions",
        "SUPER_PROMPT_TODO(AgentType.fork.instructions)",
    ),
    (
        "AgentType.fork.pending_result",
        "SUPER_PROMPT_TODO(AgentType.fork.pending_result)",
    ),
    (
        "AgentType.general-purpose.description",
        "SUPER_PROMPT_TODO(AgentType.general-purpose.description)",
    ),
    (
        "AgentType.general-purpose.instructions",
        "SUPER_PROMPT_TODO(AgentType.general-purpose.instructions)",
    ),
    ("Bash.description", "SUPER_PROMPT_TODO(Bash.description)"),
    (
        "Bash.description.cmd",
        "SUPER_PROMPT_TODO(Bash.description.cmd)",
    ),
    (
        "Bash.description.powershell",
        "SUPER_PROMPT_TODO(Bash.description.powershell)",
    ),
    (
        "Bash.description.pwsh",
        "SUPER_PROMPT_TODO(Bash.description.pwsh)",
    ),
    (
        "Bash.notice.large_output",
        "SUPER_PROMPT_TODO(Bash.notice.large_output)",
    ),
    (
        "Bash.param.command",
        "SUPER_PROMPT_TODO(Bash.param.command)",
    ),
    (
        "Bash.param.dangerouslyDisableSandbox",
        "SUPER_PROMPT_TODO(Bash.param.dangerouslyDisableSandbox)",
    ),
    (
        "Bash.param.description",
        "SUPER_PROMPT_TODO(Bash.param.description)",
    ),
    (
        "Bash.param.run_in_background",
        "SUPER_PROMPT_TODO(Bash.param.run_in_background)",
    ),
    (
        "Bash.param.timeout",
        "SUPER_PROMPT_TODO(Bash.param.timeout)",
    ),
    ("Edit.description", "SUPER_PROMPT_TODO(Edit.description)"),
    (
        "Edit.param.file_path",
        "SUPER_PROMPT_TODO(Edit.param.file_path)",
    ),
    (
        "Edit.param.new_string",
        "SUPER_PROMPT_TODO(Edit.param.new_string)",
    ),
    (
        "Edit.param.old_string",
        "SUPER_PROMPT_TODO(Edit.param.old_string)",
    ),
    (
        "Edit.param.replace_all",
        "SUPER_PROMPT_TODO(Edit.param.replace_all)",
    ),
    (
        "EnterWorktree.description",
        "SUPER_PROMPT_TODO(EnterWorktree.description)",
    ),
    (
        "EnterWorktree.param.name",
        "SUPER_PROMPT_TODO(EnterWorktree.param.name)",
    ),
    (
        "EnterWorktree.param.path",
        "SUPER_PROMPT_TODO(EnterWorktree.param.path)",
    ),
    (
        "ExitWorktree.description",
        "SUPER_PROMPT_TODO(ExitWorktree.description)",
    ),
    (
        "ExitWorktree.param.action",
        "SUPER_PROMPT_TODO(ExitWorktree.param.action)",
    ),
    (
        "ExitWorktree.param.discard_changes",
        "SUPER_PROMPT_TODO(ExitWorktree.param.discard_changes)",
    ),
    (
        "ListAgents.description",
        "SUPER_PROMPT_TODO(ListAgents.description)",
    ),
    (
        "ListAgents.param.channel",
        "SUPER_PROMPT_TODO(ListAgents.param.channel)",
    ),
    (
        "ListAgents.param.cursor",
        "SUPER_PROMPT_TODO(ListAgents.param.cursor)",
    ),
    (
        "ListAgents.param.q",
        "SUPER_PROMPT_TODO(ListAgents.param.q)",
    ),
    (
        "Loop.self_paced_instructions",
        "SUPER_PROMPT_TODO(Loop.self_paced_instructions)",
    ),
    (
        "Monitor.description",
        "SUPER_PROMPT_TODO(Monitor.description)",
    ),
    (
        "Monitor.param.command",
        "SUPER_PROMPT_TODO(Monitor.param.command)",
    ),
    (
        "Monitor.param.description",
        "SUPER_PROMPT_TODO(Monitor.param.description)",
    ),
    (
        "Monitor.param.timeout_ms",
        "SUPER_PROMPT_TODO(Monitor.param.timeout_ms)",
    ),
    ("Monitor.param.ws", "SUPER_PROMPT_TODO(Monitor.param.ws)"),
    (
        "Monitor.param.ws.protocols",
        "SUPER_PROMPT_TODO(Monitor.param.ws.protocols)",
    ),
    (
        "Monitor.param.ws.url",
        "SUPER_PROMPT_TODO(Monitor.param.ws.url)",
    ),
    (
        "Notification.agent_message",
        "SUPER_PROMPT_TODO(Notification.agent_message)",
    ),
    (
        "Notification.header",
        "SUPER_PROMPT_TODO(Notification.header)",
    ),
    (
        "Notification.monitor_event",
        "SUPER_PROMPT_TODO(Notification.monitor_event)",
    ),
    ("Read.description", "SUPER_PROMPT_TODO(Read.description)"),
    ("Read.empty_file", "SUPER_PROMPT_TODO(Read.empty_file)"),
    (
        "Read.notice.partial",
        "SUPER_PROMPT_TODO(Read.notice.partial)",
    ),
    (
        "Read.param.allow_large",
        "SUPER_PROMPT_TODO(Read.param.allow_large)",
    ),
    (
        "Read.param.file_path",
        "SUPER_PROMPT_TODO(Read.param.file_path)",
    ),
    ("Read.param.limit", "SUPER_PROMPT_TODO(Read.param.limit)"),
    ("Read.param.offset", "SUPER_PROMPT_TODO(Read.param.offset)"),
    (
        "ScheduleWakeup.description",
        "SUPER_PROMPT_TODO(ScheduleWakeup.description)",
    ),
    (
        "ScheduleWakeup.param.delaySeconds",
        "SUPER_PROMPT_TODO(ScheduleWakeup.param.delaySeconds)",
    ),
    (
        "ScheduleWakeup.param.noop",
        "SUPER_PROMPT_TODO(ScheduleWakeup.param.noop)",
    ),
    (
        "ScheduleWakeup.param.prompt",
        "SUPER_PROMPT_TODO(ScheduleWakeup.param.prompt)",
    ),
    (
        "ScheduleWakeup.param.reason",
        "SUPER_PROMPT_TODO(ScheduleWakeup.param.reason)",
    ),
    (
        "ScheduleWakeup.param.stop",
        "SUPER_PROMPT_TODO(ScheduleWakeup.param.stop)",
    ),
    (
        "SendMessage.description",
        "SUPER_PROMPT_TODO(SendMessage.description)",
    ),
    (
        "SendMessage.param.message",
        "SUPER_PROMPT_TODO(SendMessage.param.message)",
    ),
    (
        "SendMessage.param.notify_when_idle",
        "SUPER_PROMPT_TODO(SendMessage.param.notify_when_idle)",
    ),
    (
        "SendMessage.param.summary",
        "SUPER_PROMPT_TODO(SendMessage.param.summary)",
    ),
    (
        "SendMessage.param.to",
        "SUPER_PROMPT_TODO(SendMessage.param.to)",
    ),
    (
        "Skill.agent_prompt",
        "SUPER_PROMPT_TODO(Skill.agent_prompt)",
    ),
    ("Skill.description", "SUPER_PROMPT_TODO(Skill.description)"),
    ("Skill.list", "SUPER_PROMPT_TODO(Skill.list)"),
    ("Skill.loaded", "SUPER_PROMPT_TODO(Skill.loaded)"),
    ("Skill.param.args", "SUPER_PROMPT_TODO(Skill.param.args)"),
    ("Skill.param.skill", "SUPER_PROMPT_TODO(Skill.param.skill)"),
    (
        "Skill.workflow-authoring.description",
        "SUPER_PROMPT_TODO(Skill.workflow-authoring.description)",
    ),
    (
        "SystemPrompt.advanced",
        "SUPER_PROMPT_TODO(SystemPrompt.advanced)",
    ),
    (
        "TaskStop.description",
        "SUPER_PROMPT_TODO(TaskStop.description)",
    ),
    (
        "TaskStop.param.shell_id",
        "SUPER_PROMPT_TODO(TaskStop.param.shell_id)",
    ),
    (
        "TaskStop.param.task_id",
        "SUPER_PROMPT_TODO(TaskStop.param.task_id)",
    ),
    (
        "Workflow.description",
        "SUPER_PROMPT_TODO(Workflow.description)",
    ),
    (
        "Workflow.notice.cut_off",
        "SUPER_PROMPT_TODO(Workflow.notice.cut_off)",
    ),
    (
        "Workflow.notice.stopped",
        "SUPER_PROMPT_TODO(Workflow.notice.stopped)",
    ),
    ("Workflow.opt_in", "SUPER_PROMPT_TODO(Workflow.opt_in)"),
    (
        "Workflow.param.args",
        "SUPER_PROMPT_TODO(Workflow.param.args)",
    ),
    (
        "Workflow.param.description",
        "SUPER_PROMPT_TODO(Workflow.param.description)",
    ),
    (
        "Workflow.param.name",
        "SUPER_PROMPT_TODO(Workflow.param.name)",
    ),
    (
        "Workflow.param.resumeFromRunId",
        "SUPER_PROMPT_TODO(Workflow.param.resumeFromRunId)",
    ),
    (
        "Workflow.param.script",
        "SUPER_PROMPT_TODO(Workflow.param.script)",
    ),
    (
        "Workflow.param.scriptPath",
        "SUPER_PROMPT_TODO(Workflow.param.scriptPath)",
    ),
    (
        "Workflow.param.title",
        "SUPER_PROMPT_TODO(Workflow.param.title)",
    ),
    (
        "Worktree.exit_question",
        "SUPER_PROMPT_TODO(Worktree.exit_question)",
    ),
    ("Write.description", "SUPER_PROMPT_TODO(Write.description)"),
    (
        "Write.param.content",
        "SUPER_PROMPT_TODO(Write.param.content)",
    ),
    (
        "Write.param.file_path",
        "SUPER_PROMPT_TODO(Write.param.file_path)",
    ),
];

/// The text of the slot `key`, such as `SystemPrompt.advanced`: its
/// placeholder, `SUPER_PROMPT_TODO(SystemPrompt.advanced)`, while the prompt
/// is not written.
///
/// The key has an area and a name joined by a dot, holds no spaces or
/// parentheses, and is listed in [`SLOTS`].
pub fn prompt_todo(key: &str) -> String {
    debug_assert!(
        is_key(key),
        "a placeholder key is `<Area>.<name>` without spaces or parentheses: {key:?}"
    );
    let slot = SLOTS
        .binary_search_by(|(listed, _)| (*listed).cmp(key))
        .ok()
        .map(|index| SLOTS[index].1);
    debug_assert!(
        slot.is_some(),
        "the placeholder key {key:?} is not listed in SLOTS; add it there in the order of the keys"
    );
    match slot {
        Some(text) => text.to_string(),
        None => format!("{PREFIX}{key})"),
    }
}

/// The text of the description of the tool `tool`.
pub fn tool_description(tool: &str) -> String {
    prompt_todo(&format!("{tool}.description"))
}

/// The text of the description of the parameter `parameter` of the tool
/// `tool`. A nested parameter is its path joined with dots, such as
/// `ws.url`.
pub fn tool_parameter(tool: &str, parameter: &str) -> String {
    prompt_todo(&format!("{tool}.param.{parameter}"))
}

fn is_key(key: &str) -> bool {
    key.contains('.')
        && !key.starts_with('.')
        && !key.ends_with('.')
        && !key.contains("..")
        && !key
            .chars()
            .any(|character| character.is_whitespace() || matches!(character, '(' | ')'))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn placeholders_have_the_agreed_form() {
        assert_eq!(
            tool_description("Read"),
            "SUPER_PROMPT_TODO(Read.description)"
        );
        assert_eq!(
            tool_parameter("Read", "file_path"),
            "SUPER_PROMPT_TODO(Read.param.file_path)"
        );
        assert_eq!(
            tool_parameter("Monitor", "ws.url"),
            "SUPER_PROMPT_TODO(Monitor.param.ws.url)"
        );
        assert_eq!(
            prompt_todo("Notification.header"),
            "SUPER_PROMPT_TODO(Notification.header)"
        );
        assert_eq!(
            prompt_todo("AgentType.Explore.instructions"),
            "SUPER_PROMPT_TODO(AgentType.Explore.instructions)"
        );
        assert_eq!(
            prompt_todo("SystemPrompt.advanced"),
            "SUPER_PROMPT_TODO(SystemPrompt.advanced)"
        );
        assert!(prompt_todo("Read.description").starts_with(PREFIX));
    }

    #[test]
    fn every_slot_is_listed_once_in_order_with_its_placeholder() {
        for pair in SLOTS.windows(2) {
            assert!(
                pair[0].0 < pair[1].0,
                "{} comes before {}",
                pair[1].0,
                pair[0].0
            );
        }
        for (key, text) in SLOTS {
            assert!(is_key(key), "{key}");
            assert_eq!(*text, format!("{PREFIX}{key})"));
            assert_eq!(prompt_todo(key), *text);
        }
    }

    #[test]
    #[cfg(debug_assertions)]
    #[should_panic(expected = "not listed in SLOTS")]
    fn a_key_the_table_does_not_list_is_refused() {
        prompt_todo("Read.unlisted");
    }

    #[test]
    #[cfg(debug_assertions)]
    #[should_panic(expected = "a placeholder key")]
    fn a_key_without_an_area_is_refused() {
        prompt_todo("description");
    }

    #[test]
    #[cfg(debug_assertions)]
    #[should_panic(expected = "a placeholder key")]
    fn a_key_with_a_space_is_refused() {
        prompt_todo("Read.file path");
    }
}

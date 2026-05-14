use crate::state::store::Store;
use super::registry::{Command, CommandKind};

/// Dispatch a slash command. Returns the text to display.
/// For Prompt commands, returns text to be fed to the LLM.
/// For Local commands, executes directly and returns output.
pub fn dispatch(command: &Command, input: &str, store: &Store) -> CommandResult {
    match command.name.as_str() {
        "/clear" => {
            store.set_state(|s| s.messages.clear());
            CommandResult::Display("Conversation cleared.".into())
        }
        "/quit" | "/exit" => {
            CommandResult::Quit
        }
        "/model" => {
            let model = store.get_state().model.clone();
            let args = input.strip_prefix(&command.name).unwrap_or("").trim();
            if args.is_empty() || args == "current" || args == "status" {
                CommandResult::Display(format!("Current model: {model}"))
            } else if args == "help" || args == "-h" || args == "--help" {
                CommandResult::Display("Available models:\n  anthropic/claude-sonnet-4-6\n  anthropic/claude-haiku-4-5\n  anthropic/claude-opus-4-7\n  deepseek/deepseek-chat\n\nUsage: /model [name|default|current|help]".into())
            } else if args == "default" {
                store.set_state(|s| s.model = "anthropic/claude-sonnet-4-6".into());
                CommandResult::Display("Model reset to default: anthropic/claude-sonnet-4-6".into())
            } else {
                store.set_state(|s| s.model = args.to_string());
                CommandResult::Display(format!("Model set to: {args}"))
            }
        }
        "/think" => {
            let current = store.get_state().thinking_enabled;
            store.set_state(|s| s.thinking_enabled = !current);
            CommandResult::Display(format!("Thinking: {}", if !current { "enabled" } else { "disabled" }))
        }
        "/version" => {
            CommandResult::Display(format!("Super CLI v{}", env!("CARGO_PKG_VERSION")))
        }
        "/status" => {
            let state = store.get_state();
            CommandResult::Display(format!(
                "Model: {}\nThinking: {}\nMessages: {}\nTasks: {}",
                state.model,
                state.thinking_enabled,
                state.messages.len(),
                state.tasks.len()
            ))
        }
        "/diff" => {
            CommandResult::Display("Use /review or /commit for git operations.".into())
        }
        "/export" => {
            let messages = &store.get_state().messages;
            let output: String = messages.iter().map(|m| format!("{m:?}")).collect::<Vec<_>>().join("\n");
            CommandResult::Display(format!("Conversation export:\n\n{output}"))
        }
        "/context" | "/ctx" => {
            let count = store.get_state().messages.len();
            CommandResult::Display(format!("Context: {count} messages (~{} tokens)", count * 250))
        }
        "/help" => {
            let help_text = r#"Super CLI — Available Commands

/help          Show this help
/clear         Clear conversation history
/compact       Compact conversation context
/model         Show or change the model
/think         Toggle thinking mode
/status        Show session status
/context       Show context usage
/diff          Show working tree changes
/export        Export conversation
/init          Initialize CLAUDE.md
/review        Review pending changes
/commit        Create a git commit
/PR            Create a pull request
/login         Log in to Super
/logout        Log out
/quit, /exit   Exit Super
/version       Show version
/permissions   Manage tool permissions
/mcp           Manage MCP servers
/agents        Manage agent definitions
/memory        Manage agent memory
/rename        Rename session
/config        Open configuration
/bug           Report a bug
"#;
            CommandResult::Display(help_text.into())
        }
        // Prompt commands: return text to feed to the LLM
        _ => match command.kind {
            CommandKind::Prompt => {
                let text = input.strip_prefix(&command.name).unwrap_or("").trim();
                CommandResult::Prompt(text.to_string())
            }
            CommandKind::Local => {
                CommandResult::Display(format!("Command {} is not yet implemented.", command.name))
            }
        },
    }
}

pub enum CommandResult {
    /// Display text in the scroll area
    Display(String),
    /// Feed text to the LLM as a prompt
    Prompt(String),
    /// Exit the application
    Quit,
}

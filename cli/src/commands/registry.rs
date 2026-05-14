use std::collections::HashMap;

#[derive(Clone)]
pub enum CommandKind {
    /// Returns text fed to the LLM (e.g. /init, /review, /commit).
    Prompt,
    /// Runs synchronously, returns text result (e.g. /clear, /version).
    Local,
}

pub struct Command {
    pub name: String,
    pub aliases: Vec<String>,
    pub description: String,
    pub kind: CommandKind,
    pub is_enabled: bool,
}

pub struct CommandRegistry {
    commands: HashMap<String, Command>,
}

impl CommandRegistry {
    pub fn new() -> Self {
        let mut registry = Self {
            commands: HashMap::new(),
        };
        registry.register_builtins();
        registry
    }

    fn register_builtins(&mut self) {
        // Names, aliases, and descriptions are kept in sync with
        // claude-code-src/commands/<name>/index.ts. Commands intentionally
        // excluded from super are recorded in PLAN.md under "Feature
        // Blacklist (slash commands, 2026-05-14)".
        let builtins: Vec<(&str, &[&str], &str, CommandKind)> = vec![
            // ----- Local commands -----
            ("/help", &[] as &[&str], "Show help and available commands", CommandKind::Local),
            ("/clear", &["/reset", "/new"], "Clear conversation history and free up context", CommandKind::Local),
            ("/exit", &["/quit"], "Exit the REPL", CommandKind::Local),
            ("/version", &[], "Print the version this session is running", CommandKind::Local),
            ("/status", &[], "Show Super status including version, model, account, and tool statuses", CommandKind::Local),
            ("/model", &[], "Set the AI model for Super", CommandKind::Local),
            ("/effort", &[], "Set effort level for model usage", CommandKind::Local),
            ("/think", &[], "Toggle extended thinking", CommandKind::Local),
            ("/context", &[], "Visualize current context usage", CommandKind::Local),
            ("/diff", &[], "View uncommitted changes and per-turn diffs", CommandKind::Local),
            ("/export", &[], "Export the current conversation to a file", CommandKind::Local),
            ("/rename", &[], "Rename the current conversation", CommandKind::Local),
            ("/resume", &["/continue"], "Resume a previous conversation", CommandKind::Local),
            ("/login", &[], "Sign in with your Super account", CommandKind::Local),
            ("/logout", &[], "Sign out from your Super account", CommandKind::Local),
            ("/memory", &[], "Edit Super memory files", CommandKind::Local),
            ("/agents", &[], "Manage agent configurations", CommandKind::Local),
            ("/mcp", &[], "Manage MCP servers", CommandKind::Local),
            ("/plugin", &["/plugins", "/marketplace"], "Manage Super plugins", CommandKind::Local),
            ("/sandbox", &[], "Toggle sandbox settings", CommandKind::Local),
            ("/config", &["/settings"], "Open config panel", CommandKind::Local),
            ("/permissions", &["/allowed-tools"], "Manage allow & deny tool permission rules", CommandKind::Local),
            ("/feedback", &["/bug"], "Submit feedback about Claude Code", CommandKind::Local),
            // ----- Prompt commands -----
            ("/init", &[], "Initialize a new CLAUDE.md file with codebase documentation", CommandKind::Prompt),
            ("/compact", &[], "Clear conversation history but keep a summary in context", CommandKind::Prompt),
            ("/review", &[], "Review a pull request", CommandKind::Prompt),
            ("/commit", &[], "Create a git commit", CommandKind::Prompt),
            ("/commit-push-pr", &["/pr"], "Commit, push, and open a PR", CommandKind::Prompt),
        ];

        for (name, aliases, description, kind) in builtins {
            self.commands.insert(
                name.to_string(),
                Command {
                    name: name.to_string(),
                    aliases: aliases.iter().map(|s| s.to_string()).collect(),
                    description: description.to_string(),
                    kind,
                    is_enabled: true,
                },
            );
        }
    }

    /// Resolve `/<name>` or any alias to the canonical command.
    pub fn resolve(&self, input: &str) -> Option<&Command> {
        let name = input
            .split_whitespace()
            .next()
            .unwrap_or("")
            .to_lowercase();
        self.commands.get(&name).or_else(|| {
            self.commands.values().find(|c| {
                c.aliases.iter().any(|a| a.to_lowercase() == name)
            })
        })
    }

    /// All registered commands, sorted by name. Used by the slash menu
    /// and by /help to render the same list.
    pub fn list(&self) -> Vec<&Command> {
        let mut cmds: Vec<&Command> = self.commands.values().collect();
        cmds.sort_by_key(|c| c.name.as_str());
        cmds
    }
}

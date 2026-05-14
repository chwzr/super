use std::collections::HashMap;

#[derive(Clone)]
pub enum CommandKind {
    /// Returns text fed to the LLM (e.g. /init, /review)
    Prompt,
    /// Runs synchronously, returns text result (e.g. /clear, /model)
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
        let mut registry = Self { commands: HashMap::new() };
        registry.register_builtins();
        registry
    }

    fn register_builtins(&mut self) {
        let builtins: Vec<(&str, &[&str], &str, CommandKind)> = vec![
            ("/help", &[] as &[&str], "Show help information", CommandKind::Local),
            ("/clear", &[], "Clear conversation history", CommandKind::Local),
            ("/compact", &[], "Compact conversation context", CommandKind::Prompt),
            ("/config", &["/settings"], "Open configuration", CommandKind::Prompt),
            ("/model", &[], "Show or change the model", CommandKind::Local),
            ("/effort", &[], "Set effort level", CommandKind::Local),
            ("/fast", &[], "Toggle fast mode", CommandKind::Local),
            ("/login", &[], "Log in to Super", CommandKind::Local),
            ("/logout", &[], "Log out of Super", CommandKind::Local),
            ("/resume", &["/continue"], "Resume previous session", CommandKind::Local),
            ("/init", &[], "Initialize a CLAUDE.md file", CommandKind::Prompt),
            ("/review", &[], "Review pending changes", CommandKind::Prompt),
            ("/commit", &[], "Create a git commit", CommandKind::Prompt),
            ("/pr", &[], "Create a pull request", CommandKind::Prompt),
            ("/status", &[], "Show session status", CommandKind::Local),
            ("/quit", &["/exit"], "Exit Super", CommandKind::Local),
            ("/memory", &[], "Manage agent memory", CommandKind::Prompt),
            ("/agents", &[], "Manage agent definitions", CommandKind::Prompt),
            ("/mcp", &[], "Manage MCP servers", CommandKind::Prompt),
            ("/plugin", &["/plugins"], "Manage plugins", CommandKind::Prompt),
            ("/sandbox", &[], "Manage sandbox settings", CommandKind::Prompt),
            ("/diff", &[], "Show working tree diff", CommandKind::Local),
            ("/export", &[], "Export conversation", CommandKind::Local),
            ("/rename", &[], "Rename the current session", CommandKind::Local),
            ("/voice", &[], "Toggle voice mode", CommandKind::Local),
            ("/context", &["/ctx"], "Show context usage", CommandKind::Local),
            ("/keybindings", &[], "Configure keybindings", CommandKind::Local),
            ("/permissions", &["/allowed-tools"], "Manage permissions", CommandKind::Prompt),
            ("/think", &[], "Toggle thinking mode", CommandKind::Local),
            ("/ide", &[], "Manage IDE integration", CommandKind::Prompt),
            ("/desktop", &[], "Manage desktop integration", CommandKind::Local),
            ("/version", &[], "Show version info", CommandKind::Local),
            ("/bug", &[], "Report a bug", CommandKind::Prompt),
        ];

        for (name, aliases, description, kind) in builtins {
            self.commands.insert(name.to_string(), Command {
                name: name.to_string(),
                aliases: aliases.iter().map(|s| s.to_string()).collect(),
                description: description.to_string(),
                kind,
                is_enabled: true,
            });
        }
    }

    pub fn resolve(&self, input: &str) -> Option<&Command> {
        let name = input.split_whitespace().next().unwrap_or("").to_lowercase();
        self.commands.get(&name).or_else(|| {
            self.commands.values().find(|c| {
                c.aliases.iter().any(|a| a.to_lowercase() == name)
            })
        })
    }

    pub fn list(&self) -> Vec<&Command> {
        let mut cmds: Vec<&Command> = self.commands.values().collect();
        cmds.sort_by_key(|c| c.name.as_str());
        cmds
    }
}

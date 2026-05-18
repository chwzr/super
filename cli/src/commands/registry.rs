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
    pub argument_hint: Option<&'static str>,
    pub kind: CommandKind,
    pub is_enabled: bool,
}

pub struct CommandRegistry {
    commands: HashMap<String, Command>,
}

impl Default for CommandRegistry {
    fn default() -> Self {
        Self::new()
    }
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
        // Tuple layout: (name, aliases, description, argument_hint, kind)
        type BuiltinDef<'a> = (
            &'a str,
            &'a [&'a str],
            &'a str,
            Option<&'static str>,
            CommandKind,
        );
        let builtins: Vec<BuiltinDef> = vec![
            // ----- Local commands -----
            ("/help",        &[] as &[&str],              "Show help and available commands",                                                                None,                              CommandKind::Local),
            ("/clear",       &["/reset", "/new"],         "Clear conversation history and free up context",                                                  None,                              CommandKind::Local),
            ("/exit",        &["/quit"],                  "Exit the REPL",                                                                                   None,                              CommandKind::Local),
            ("/version",     &[],                         "Print the version this session is running",                                                       None,                              CommandKind::Local),
            ("/status",      &[],                         "Show Super status including version, model, account, API connectivity, and tool statuses",         None,                              CommandKind::Local),
            ("/model",       &[],                         "Set the AI model for Super",                                                                       Some("[model]"),                   CommandKind::Local),
            ("/provider",   &[],                         "Select model provider (Anthropic, Z.ai, Moonshot, Deepseek, Free)",                                None,                              CommandKind::Local),
            ("/effort",      &[],                         "Set effort level for model usage",                                                                 Some("[low|medium|high|max|auto]"),CommandKind::Local),
            ("/think",       &[],                         "Toggle extended thinking",                                                                         None,                              CommandKind::Local),
            ("/context",     &[],                         "Show current context usage",                                                                       None,                              CommandKind::Local),
            ("/diff",        &[],                         "View uncommitted changes and per-turn diffs",                                                      None,                              CommandKind::Local),
            ("/export",      &[],                         "Export the current conversation to a file or clipboard",                                           Some("[filename]"),                CommandKind::Local),
            ("/rename",      &[],                         "Rename the current conversation",                                                                  Some("[name]"),                    CommandKind::Local),
            ("/resume",      &["/continue"],              "Resume a previous conversation",                                                                   Some("[conversation id or search term]"), CommandKind::Local),
            ("/login",       &[],                         "Sign in with your Super account",                                                                  None,                              CommandKind::Local),
            ("/logout",      &[],                         "Sign out from your Super account",                                                                 None,                              CommandKind::Local),
            ("/memory",      &[],                         "Edit Super memory files",                                                                          None,                              CommandKind::Local),
            ("/agents",      &[],                         "Manage agent configurations",                                                                      None,                              CommandKind::Local),
            ("/mcp",         &[],                         "Manage MCP servers",                                                                               Some("[enable|disable [server-name]]"), CommandKind::Local),
            ("/plugin",      &["/plugins", "/marketplace"],"Manage Super plugins",                                                                            None,                              CommandKind::Local),
            ("/config",      &["/settings"],              "Open config panel",                                                                                None,                              CommandKind::Local),
            ("/permissions", &["/allowed-tools"],         "Manage allow & deny tool permission rules",                                                        None,                              CommandKind::Local),
            // ----- Prompt commands -----
            ("/init",          &[], "Initialize a new CLAUDE.md file with codebase documentation",                                                            None,                              CommandKind::Prompt),
            ("/compact",       &[], "Clear conversation history but keep a summary in context. Optional: /compact [instructions for summarization]",           Some("<optional custom summarization instructions>"), CommandKind::Prompt),
            ("/review",        &[], "Review a pull request",                                                                                                  None,                              CommandKind::Prompt),
            ("/commit",        &[], "Create a git commit",                                                                                                    None,                              CommandKind::Prompt),
            ("/commit-push-pr",&["/pr"], "Commit, push, and open a PR",                                                                                      None,                              CommandKind::Prompt),
        ];

        for (name, aliases, description, argument_hint, kind) in builtins {
            self.commands.insert(
                name.to_string(),
                Command {
                    name: name.to_string(),
                    aliases: aliases.iter().map(|s| s.to_string()).collect(),
                    description: description.to_string(),
                    argument_hint,
                    kind,
                    is_enabled: true,
                },
            );
        }
    }

    /// Resolve `/<name>` or any alias to the canonical command.
    pub fn resolve(&self, input: &str) -> Option<&Command> {
        let name = input.split_whitespace().next().unwrap_or("").to_lowercase();
        self.commands.get(&name).or_else(|| {
            self.commands
                .values()
                .find(|c| c.aliases.iter().any(|a| a.to_lowercase() == name))
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

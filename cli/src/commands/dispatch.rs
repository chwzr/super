use crate::config::load_config;
use crate::state::store::Store;
use super::prompts;
use super::registry::{Command, CommandKind, CommandRegistry};

/// Dispatch a slash command. Returns the action the TUI should take.
///
/// `input` is the entire raw line typed by the user (e.g. "/model haiku"),
/// `command` is the resolved command from the registry. Any text after the
/// command name is treated as args.
pub fn dispatch(command: &Command, input: &str, store: &Store) -> CommandResult {
    let args = extract_args(command, input);
    match command.name.as_str() {
        "/help" => help(),
        "/clear" => clear(store),
        "/exit" => CommandResult::Quit,
        "/version" => version(),
        "/status" => status(store),
        "/model" => model(args, store),
        "/effort" => effort(args, store),
        "/think" => think(store),
        "/context" => context(store),
        "/diff" => diff(),
        "/export" => export(store),
        "/rename" => rename(args),
        "/resume" => resume(),
        "/login" => login(),
        "/logout" => logout(),
        "/memory" => memory(),
        "/agents" => agents(),
        "/mcp" => mcp(),
        "/plugin" => plugin(),
        "/sandbox" => sandbox(),
        "/config" => config_panel(),
        "/permissions" => permissions(),
        "/feedback" => feedback(args),

        "/init" => CommandResult::Prompt(prompts::init_prompt().to_string()),
        "/compact" => CommandResult::Prompt(prompts::compact_prompt(args)),
        "/review" => CommandResult::Prompt(prompts::review_prompt(args)),
        "/commit" => CommandResult::Prompt(prompts::commit_prompt().to_string()),
        "/commit-push-pr" => CommandResult::Prompt(prompts::commit_push_pr_prompt().to_string()),

        _ => match command.kind {
            CommandKind::Prompt => CommandResult::Prompt(args.to_string()),
            CommandKind::Local => CommandResult::Display(format!(
                "Command {} is not yet implemented.",
                command.name
            )),
        },
    }
}

/// Extract args by stripping the canonical name OR any alias from the front
/// of the typed input. Falls back to the whole input minus the first token
/// if neither matches.
fn extract_args<'a>(command: &Command, input: &'a str) -> &'a str {
    if let Some(rest) = input.strip_prefix(&command.name) {
        return rest.trim();
    }
    for alias in &command.aliases {
        if let Some(rest) = input.strip_prefix(alias) {
            return rest.trim();
        }
    }
    input
        .split_once(char::is_whitespace)
        .map(|(_, rest)| rest.trim())
        .unwrap_or("")
}

// ============================================================================
// Per-command handlers
// ============================================================================

fn help() -> CommandResult {
    let registry = CommandRegistry::new();
    let mut out = String::from("Super — Available Commands\n\n");
    let mut commands = registry.list();
    commands.sort_by_key(|c| c.name.clone());
    let name_width = commands
        .iter()
        .map(|c| c.name.chars().count())
        .max()
        .unwrap_or(20);
    for cmd in commands {
        out.push_str(&format!(
            "  {:<width$}  {}\n",
            cmd.name,
            cmd.description,
            width = name_width,
        ));
    }
    out.push_str("\nType / and start typing to filter; Tab autocompletes.\n");
    CommandResult::Display(out)
}

fn clear(store: &Store) -> CommandResult {
    store.set_state(|s| s.messages.clear());
    CommandResult::Cleared
}

fn version() -> CommandResult {
    CommandResult::Display(format!("v{}", env!("CARGO_PKG_VERSION")))
}

fn status(store: &Store) -> CommandResult {
    let state = store.get_state();
    let config = load_config();
    let signed_in = config.access_token.is_some();
    let auth_status = if signed_in {
        "Signed in to Super (OpenRouter via platform server)"
    } else {
        "Not signed in — run /login"
    };
    let cwd = std::env::current_dir()
        .map(|p| p.display().to_string())
        .unwrap_or_else(|_| "?".into());
    // Display model using the same friendly short name as the header.
    let model_display = friendly_model_short_name(&state.model);
    let body = format!(
        "Super status\n\
         \n\
         Version:         v{version}\n\
         Model:           {model}\n\
         Extended thinking: {thinking}\n\
         Auth:            {auth}\n\
         Working dir:     {cwd}\n\
         Messages:        {messages}\n\
         \n\
         Tools\n\
         \n\
           Bash            Execute shell commands\n\
           Read            Read files from disk\n\
           Write           Write files to disk\n\
           Edit            Make targeted edits to files\n\
           WebFetch        Fetch a URL and return its content\n\
           WebSearch       Search the web\n\
           Agent           Spawn a sub-agent (task delegation)\n\
         \n\
         MCP servers: see /mcp for connected server list",
        version = env!("CARGO_PKG_VERSION"),
        model = model_display,
        thinking = if state.thinking_enabled { "enabled" } else { "disabled" },
        auth = auth_status,
        cwd = cwd,
        messages = state.messages.len(),
    );
    CommandResult::Display(body)
}

/// Convert an OpenRouter slug to a short display name matching CC's labels.
/// e.g. "anthropic/claude-sonnet-4-6" → "claude-sonnet-4-6"
fn friendly_model_short_name(slug: &str) -> String {
    // Strip provider prefix for display.
    let bare = slug.rsplit_once('/').map(|(_, r)| r).unwrap_or(slug);
    bare.to_string()
}

fn model(args: &str, store: &Store) -> CommandResult {
    // Table of (openrouter-slug, short-display-name, description) matching CC's picker labels.
    let known: &[(&str, &str, &str)] = &[
        ("anthropic/claude-opus-4-6",   "claude-opus-4-6",   "Most capable model"),
        ("anthropic/claude-sonnet-4-6", "claude-sonnet-4-6", "Balanced speed and intelligence"),
        ("anthropic/claude-haiku-4-5",  "claude-haiku-4-5",  "Fastest model"),
    ];
    let current = store.get_state().model.clone();
    let args = args.trim();
    if args.is_empty() || args == "current" || args == "status" {
        // Display the short slug (without provider prefix) for the current model.
        let current_display = friendly_model_short_name(&current);
        let mut out = format!("Current model: {current_display}\n\nAvailable models:\n");
        for (slug, short, desc) in known {
            let marker = if *slug == current { "❯" } else { " " };
            out.push_str(&format!("  {marker} {short:<28}  {desc}\n"));
        }
        out.push_str("\nUsage: /model <name>   e.g. /model sonnet\n");
        return CommandResult::Display(out);
    }
    if args == "default" {
        let slug = "anthropic/claude-sonnet-4-6";
        store.set_state(|s| s.model = slug.into());
        let short = friendly_model_short_name(slug);
        return CommandResult::Display(format!("Model reset to default: {short}"));
    }
    // Accept short names, aliases, or full slugs.
    let resolved = match args {
        "opus" | "opus-4.6" | "claude-opus-4-6" => "anthropic/claude-opus-4-6",
        "sonnet" | "sonnet-4.6" | "claude-sonnet-4-6" => "anthropic/claude-sonnet-4-6",
        "haiku" | "haiku-4.5" | "claude-haiku-4-5" => "anthropic/claude-haiku-4-5",
        other => other,
    };
    store.set_state(|s| s.model = resolved.into());
    let short = friendly_model_short_name(resolved);
    CommandResult::Display(format!("Model set to: {short}"))
}

fn effort(args: &str, store: &Store) -> CommandResult {
    let args = args.trim().to_lowercase();
    let args = args.as_str();

    // Help args — match CC's exact help text format.
    if matches!(args, "help" | "-h" | "--help") {
        return CommandResult::Display(
            "Usage: /effort [low|medium|high|max|auto]\n\n\
             Effort levels:\n\
             - low: Quick, straightforward implementation with minimal overhead\n\
             - medium: Balanced approach with standard implementation and testing\n\
             - high: Comprehensive implementation with extensive testing and documentation\n\
             - max: Maximum capability with deepest reasoning (Opus 4.6 only)\n\
             - auto: Use the default effort level for your model"
                .into(),
        );
    }

    // No args or "status"/"current" — show current level like CC does.
    if args.is_empty() || args == "current" || args == "status" {
        let level = store.get_state().effort_level.clone();
        let msg = match level.as_deref() {
            None | Some("auto") => format!(
                "Effort level: auto (currently high)"
            ),
            Some(l) => {
                let desc = effort_level_description(l);
                format!("Current effort level: {l} ({desc})")
            }
        };
        return CommandResult::Display(msg);
    }

    // "auto" / "unset" — clear back to auto.
    if args == "auto" || args == "unset" {
        store.set_state(|s| s.effort_level = None);
        return CommandResult::Display("Effort level set to auto".into());
    }

    let valid = ["low", "medium", "high", "max"];
    if !valid.contains(&args) {
        return CommandResult::Display(format!(
            "Invalid argument: {args}. Valid options are: low, medium, high, max, auto"
        ));
    }

    let desc = effort_level_description(args);
    store.set_state(|s| s.effort_level = Some(args.to_string()));
    CommandResult::Display(format!("Set effort level to {args}: {desc}"))
}

fn effort_level_description(level: &str) -> &'static str {
    match level {
        "low" => "Quick, straightforward implementation with minimal overhead",
        "medium" => "Balanced approach with standard implementation and testing",
        "high" => "Comprehensive implementation with extensive testing and documentation",
        "max" => "Maximum capability with deepest reasoning (Opus 4.6 only)",
        _ => "Use the default effort level for your model",
    }
}

fn think(store: &Store) -> CommandResult {
    let current = store.get_state().thinking_enabled;
    store.set_state(|s| s.thinking_enabled = !current);
    CommandResult::Display(format!(
        "Extended thinking: {}",
        if !current { "enabled" } else { "disabled" }
    ))
}

fn context(store: &Store) -> CommandResult {
    let state = store.get_state();
    let count = state.messages.len();
    let model = friendly_model_short_name(&state.model);
    // Approximate token counts (1 token ≈ 4 chars). Full tokenizer is a follow-up.
    let msg_chars: usize = state.messages.iter().map(approx_message_chars).sum();
    let msg_tokens = msg_chars / 4;
    // Rough system prompt estimate (tools + instructions overhead)
    let system_tokens: usize = 8_000;
    let total_tokens = system_tokens + msg_tokens;
    let budget = 200_000usize; // Sonnet/Opus default context window
    let pct = ((total_tokens as f64 / budget as f64) * 100.0).min(100.0);
    let free = budget.saturating_sub(total_tokens);
    let body = format!(
        "Context window usage\n\
         \n\
         Model:  {model}\n\
         Tokens: ~{total_tokens} / {budget} ({pct:.1}%)\n\
         \n\
         Estimated usage by category\n\
         \n\
         | Category      | Tokens        | % of window |\n\
         |---------------|---------------|-------------|\n\
         | System prompt | ~{system_tokens:<13} | {sys_pct:.1}%        |\n\
         | Messages      | ~{msg_tokens:<13} | {msg_pct:.1}%        |\n\
         | Free space    | ~{free:<13} | {free_pct:.1}%        |\n\
         \n\
         {count} message(s) in context.\n\
         Run /compact to summarize older turns and free up space.",
        sys_pct = (system_tokens as f64 / budget as f64) * 100.0,
        msg_pct = (msg_tokens as f64 / budget as f64) * 100.0,
        free_pct = (free as f64 / budget as f64) * 100.0,
    );
    CommandResult::Display(body)
}

fn approx_message_chars(m: &crate::tui::scroll_area::Message) -> usize {
    use crate::tui::scroll_area::Message::*;
    match m {
        User(s) | Assistant(s) | System(s) | Trail(s) => s.len(),
        ToolCall { input, result, .. } => input.len() + result.as_ref().map_or(0, |r| r.len()),
        Thinking => 0,
    }
}

fn diff() -> CommandResult {
    // Inline git diff so /diff works in any TUI. Long diffs scroll inside
    // the scroll area.
    match std::process::Command::new("git")
        .args(["--no-pager", "diff", "--no-color"])
        .output()
    {
        Ok(out) if out.status.success() => {
            let s = String::from_utf8_lossy(&out.stdout).trim_end().to_string();
            if s.is_empty() {
                CommandResult::Display("No uncommitted changes.".into())
            } else {
                CommandResult::Display(s)
            }
        }
        Ok(out) => CommandResult::Display(format!(
            "git diff failed:\n{}",
            String::from_utf8_lossy(&out.stderr)
        )),
        Err(e) => CommandResult::Display(format!("Failed to run git: {e}")),
    }
}

fn export(store: &Store) -> CommandResult {
    use std::io::Write;
    let state = store.get_state();
    if state.messages.is_empty() {
        return CommandResult::Display("Nothing to export — conversation is empty.".into());
    }
    let ts = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0);
    let path = std::env::temp_dir().join(format!("super-export-{ts}.md"));
    let Ok(mut file) = std::fs::File::create(&path) else {
        return CommandResult::Display(format!(
            "Failed to create export file at {}",
            path.display()
        ));
    };
    let _ = writeln!(file, "# Super conversation export\n");
    for msg in &state.messages {
        use crate::tui::scroll_area::Message::*;
        match msg {
            User(s) => {
                let _ = writeln!(file, "## User\n\n{}\n", s);
            }
            Assistant(s) => {
                let _ = writeln!(file, "## Assistant\n\n{}\n", s);
            }
            System(s) => {
                let _ = writeln!(file, "## System\n\n{}\n", s);
            }
            ToolCall { name, input, result, .. } => {
                let _ = writeln!(file, "## Tool call: {}\n\n```\n{}\n```\n", name, input);
                if let Some(r) = result {
                    let _ = writeln!(file, "Result:\n\n```\n{}\n```\n", r);
                }
            }
            Trail(_) | Thinking => {}
        }
    }
    CommandResult::Display(format!("Exported conversation to {}", path.display()))
}

fn rename(args: &str) -> CommandResult {
    let name = args.trim();
    if name.is_empty() {
        return CommandResult::Display(
            "Usage: /rename <new session name>\n\nSession persistence is not yet wired in super — naming is a no-op for now.".into(),
        );
    }
    CommandResult::Display(format!(
        "Session named: {name}\n\nNote: super does not yet persist sessions, so this name is in-memory only."
    ))
}

fn resume() -> CommandResult {
    CommandResult::Display(
        "/resume — no persisted sessions yet.\n\nSuper does not yet save conversations to disk; once it does, /resume will open a picker of previous sessions.".into(),
    )
}

fn login() -> CommandResult {
    CommandResult::Login
}

fn logout() -> CommandResult {
    CommandResult::Logout
}

fn memory() -> CommandResult {
    let home = dirs::home_dir().unwrap_or_default();
    let project_md = std::env::current_dir()
        .map(|p| p.join("CLAUDE.md"))
        .unwrap_or_default();
    let user_md = home.join(".claude").join("CLAUDE.md");
    CommandResult::Display(format!(
        "Memory files\n\n  Project:   {}{}\n  User:      {}{}\n\nEdit these files directly with your editor. They are loaded automatically each session.",
        project_md.display(),
        if project_md.exists() { "" } else { "  (not present)" },
        user_md.display(),
        if user_md.exists() { "" } else { "  (not present)" },
    ))
}

fn agents() -> CommandResult {
    let agents_dir = std::env::current_dir()
        .map(|p| p.join(".claude").join("agents"))
        .unwrap_or_default();
    let entries: Vec<String> = std::fs::read_dir(&agents_dir)
        .ok()
        .into_iter()
        .flatten()
        .filter_map(|e| e.ok())
        .filter_map(|e| e.file_name().into_string().ok())
        .collect();
    if entries.is_empty() {
        CommandResult::Display(format!(
            "No agents found in {}.\n\nCreate <name>.md files in that directory to define agents.",
            agents_dir.display(),
        ))
    } else {
        let mut out = String::from("Available agents:\n\n");
        for name in entries {
            out.push_str(&format!("  {name}\n"));
        }
        out.push_str(&format!("\nDirectory: {}", agents_dir.display()));
        CommandResult::Display(out)
    }
}

fn mcp() -> CommandResult {
    // Claude Code reads MCP server definitions from ~/.claude.json under
    // the `mcpServers` key. Super reads the same file so users get a
    // unified view of any servers they've already configured for claude.
    let path = dirs::home_dir()
        .map(|h| h.join(".claude.json"))
        .unwrap_or_default();
    let servers: Vec<(String, String)> = std::fs::read_to_string(&path)
        .ok()
        .and_then(|s| serde_json::from_str::<serde_json::Value>(&s).ok())
        .and_then(|v| v.get("mcpServers").cloned())
        .and_then(|v| v.as_object().cloned())
        .map(|m| {
            m.into_iter()
                .map(|(name, def)| {
                    let cmd = def
                        .get("command")
                        .and_then(|c| c.as_str())
                        .unwrap_or("(unknown)")
                        .to_string();
                    (name, cmd)
                })
                .collect()
        })
        .unwrap_or_default();
    if servers.is_empty() {
        CommandResult::Display(format!(
            "No MCP servers configured.\n\nLooked in: {}\n\nAdd servers under the `mcpServers` key of that file.",
            path.display(),
        ))
    } else {
        let mut out = String::from("MCP servers:\n\n");
        for (name, cmd) in servers {
            out.push_str(&format!("  {name:<24}  {cmd}\n"));
        }
        CommandResult::Display(out)
    }
}

fn plugin() -> CommandResult {
    CommandResult::Display(
        "Plugins\n\n  Superpowers (bundled): on\n\nSuper ships with the Superpowers plugin baked in (CLAUDE.md). A user-extensible plugin marketplace is not part of v1.".into(),
    )
}

fn sandbox() -> CommandResult {
    CommandResult::Display(
        "Sandbox\n\n  E2B sandbox integration is planned for post-v1 (PLAN.md).\n  No sandbox is active in this session.".into(),
    )
}

fn config_panel() -> CommandResult {
    let path = dirs::home_dir()
        .map(|h| h.join(".super").join("config.json"))
        .unwrap_or_default();
    CommandResult::Display(format!(
        "Config\n\n  File: {}\n\nEdit this file directly to change settings (auth token, model, MCP servers).",
        path.display()
    ))
}

fn permissions() -> CommandResult {
    CommandResult::Display(
        "Permissions\n\n  Mode: default\n\nSuper does not yet expose per-tool allow/deny rules. The /permissions UI is planned alongside the platform-server-side permissions sync.".into(),
    )
}

fn feedback(args: &str) -> CommandResult {
    let body = args.trim();
    if body.is_empty() {
        CommandResult::Display(
            "Usage: /feedback <your feedback>\n\nFeedback is logged locally for now. The submission endpoint on the Super platform server is not yet wired.".into(),
        )
    } else {
        // Best-effort: append to ~/.super/feedback.log
        if let Some(home) = dirs::home_dir() {
            let path = home.join(".super").join("feedback.log");
            if let Some(parent) = path.parent() {
                let _ = std::fs::create_dir_all(parent);
            }
            use std::io::Write;
            if let Ok(mut f) = std::fs::OpenOptions::new().create(true).append(true).open(&path) {
                let _ = writeln!(f, "[{}] {body}", chrono_now());
            }
        }
        CommandResult::Display(format!(
            "Thanks for the feedback! It's logged locally and will be submitted once the platform-server endpoint is live.\n\n> {body}"
        ))
    }
}

fn chrono_now() -> String {
    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0);
    now.to_string()
}

/// What `dispatch` returns. The TUI decides what to do based on the variant.
pub enum CommandResult {
    /// Display text in the scroll area.
    Display(String),
    /// Feed text to the LLM as a prompt.
    Prompt(String),
    /// Exit the application.
    Quit,
    /// Wipe the scroll area (caller already cleared store.messages).
    Cleared,
    /// Run the PKCE login flow against the Super platform server.
    Login,
    /// Clear stored auth tokens.
    Logout,
}

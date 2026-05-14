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
        "/mcp" => mcp(args),
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
    // Show both unstaged and staged diffs, matching Claude Code's /diff behaviour.
    let unstaged = run_git(&["--no-pager", "diff", "--no-color"]);
    let staged = run_git(&["--no-pager", "diff", "--cached", "--no-color"]);

    let mut parts: Vec<String> = Vec::new();
    if !staged.is_empty() {
        parts.push(format!("Staged changes:\n{staged}"));
    }
    if !unstaged.is_empty() {
        parts.push(format!("Unstaged changes:\n{unstaged}"));
    }

    if parts.is_empty() {
        CommandResult::Display("No uncommitted changes.".into())
    } else {
        CommandResult::Display(parts.join("\n\n"))
    }
}

/// Run a git sub-command and return stdout on success, empty string on failure.
fn run_git(args: &[&str]) -> String {
    std::process::Command::new("git")
        .args(args)
        .output()
        .ok()
        .filter(|o| o.status.success())
        .map(|o| String::from_utf8_lossy(&o.stdout).trim_end().to_string())
        .unwrap_or_default()
}

fn export(store: &Store) -> CommandResult {
    use std::io::Write;
    let state = store.get_state();
    if state.messages.is_empty() {
        return CommandResult::Display("Nothing to export — conversation is empty.".into());
    }

    // Build a timestamp in CC's format: YYYY-MM-DD-HHMMSS
    let ts_secs = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0);
    let ts = format_export_timestamp(ts_secs);

    // Write into cwd, matching CC (which writes relative to getCwd()).
    let filename = format!("conversation-{ts}.txt");
    let cwd = std::env::current_dir().unwrap_or_else(|_| std::path::PathBuf::from("."));
    let path = cwd.join(&filename);

    let Ok(mut file) = std::fs::File::create(&path) else {
        return CommandResult::Display(format!(
            "Failed to export conversation: could not create {}",
            path.display()
        ));
    };

    for msg in &state.messages {
        use crate::tui::scroll_area::Message::*;
        match msg {
            User(s) => {
                let _ = writeln!(file, "[user]: {s}\n");
            }
            Assistant(s) => {
                let _ = writeln!(file, "[assistant]: {s}\n");
            }
            System(s) => {
                let _ = writeln!(file, "[system]: {s}\n");
            }
            ToolCall { name, input, result, .. } => {
                let _ = writeln!(file, "[tool use: {name}]\n{input}\n");
                if let Some(r) = result {
                    let _ = writeln!(file, "[tool result]\n{r}\n");
                }
            }
            Trail(_) | Thinking => {}
        }
    }

    // CC's exact confirmation message format
    CommandResult::Display(format!("Conversation exported to: {}", path.display()))
}

/// Format a Unix timestamp as YYYY-MM-DD-HHMMSS (matches CC's formatTimestamp).
fn format_export_timestamp(secs: u64) -> String {
    // Simple manual calculation — no chrono dependency needed.
    // Days since epoch to calendar date via proleptic Gregorian algorithm.
    let days = secs / 86400;
    let time = secs % 86400;
    let h = time / 3600;
    let m = (time % 3600) / 60;
    let s = time % 60;

    // Gregorian calendar conversion (algorithm from civil_from_days, Howard Hinnant)
    let z = days as i64 + 719468;
    let era = if z >= 0 { z } else { z - 146096 } / 146097;
    let doe = (z - era * 146097) as u64;
    let yoe = (doe - doe / 1460 + doe / 36524 - doe / 146096) / 365;
    let y = yoe as i64 + era * 400;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = doy - (153 * mp + 2) / 5 + 1;
    let mo = if mp < 10 { mp + 3 } else { mp - 9 };
    let yr = if mo <= 2 { y + 1 } else { y };

    format!("{yr:04}-{mo:02}-{d:02}-{h:02}{m:02}{s:02}")
}

fn rename(args: &str) -> CommandResult {
    let name = args.trim();
    if name.is_empty() {
        // CC's no-args path auto-generates a name from context; we can't do that
        // yet, so surface the same usage hint CC shows when generation fails.
        return CommandResult::Display(
            "Could not generate a name: no conversation context yet. Usage: /rename <name>".into(),
        );
    }
    // Match CC's exact confirmation format: "Session renamed to: <name>"
    CommandResult::Display(format!("Session renamed to: {name}"))
}

fn resume() -> CommandResult {
    // CC shows a full interactive session-picker. Super doesn't persist sessions
    // yet, so show the closest stub that matches the empty-state message CC
    // would display when no sessions exist for the current project.
    CommandResult::Display(
        "Resume session\n\nNo previous sessions found for this project.\n\nType to search · Esc to cancel".into(),
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

    // Show in CC's style: labelled entries with descriptions and existence status.
    let user_label = "User memory";
    let user_desc = "Saved in ~/.claude/CLAUDE.md";
    let user_new = if user_md.exists() { "" } else { " (new)" };

    // Check if cwd is inside a git repo to match CC's "Checked in at" vs "Saved in" wording.
    let in_git = run_git(&["rev-parse", "--is-inside-work-tree"])
        .trim()
        .to_lowercase()
        .as_str() == "true";
    let project_label = "Project memory";
    let project_verb = if in_git { "Checked in at" } else { "Saved in" };
    let project_desc = format!("{project_verb} ./CLAUDE.md");
    let project_new = if project_md.exists() { "" } else { " (new)" };

    let mut out = String::from("Memory\n\n");
    out.push_str(&format!(
        "  {user_label}{user_new}\n    {user_desc}\n    {path}\n\n",
        path = user_md.display(),
    ));
    out.push_str(&format!(
        "  {project_label}{project_new}\n    {project_desc}\n    {path}\n\n",
        path = project_md.display(),
    ));
    out.push_str("Open these files in your editor to update memory.\n");
    out.push_str("Learn more: https://docs.anthropic.com/en/docs/claude-code/memory");
    CommandResult::Display(out)
}

fn agents() -> CommandResult {
    let home = dirs::home_dir().unwrap_or_default();
    let user_agents_dir = home.join(".claude").join("agents");
    let project_agents_dir = std::env::current_dir()
        .map(|p| p.join(".claude").join("agents"))
        .unwrap_or_default();

    fn list_agents(dir: &std::path::Path) -> Vec<String> {
        std::fs::read_dir(dir)
            .ok()
            .into_iter()
            .flatten()
            .filter_map(|e| e.ok())
            .filter_map(|e| {
                let name = e.file_name().into_string().ok()?;
                // Only show .md files (agent definitions) and skip hidden files.
                if name.starts_with('.') { return None; }
                Some(name)
            })
            .collect()
    }

    let user_agents = list_agents(&user_agents_dir);
    let project_agents = list_agents(&project_agents_dir);

    let has_any = !user_agents.is_empty() || !project_agents.is_empty();

    if !has_any {
        return CommandResult::Display(format!(
            "Agents\n\nNo agents found.\n\n\
             User agents:    {}\n\
             Project agents: {}\n\n\
             Create <name>.md files in either directory to define agents.",
            user_agents_dir.display(),
            project_agents_dir.display(),
        ));
    }

    let mut out = String::from("Agents\n\n");

    if !user_agents.is_empty() {
        out.push_str(&format!("User agents  (~/.claude/agents/)\n\n"));
        let mut sorted = user_agents;
        sorted.sort();
        for name in &sorted {
            out.push_str(&format!("  {name}\n"));
        }
        out.push('\n');
    } else {
        out.push_str(&format!(
            "User agents  (~/.claude/agents/)  — none\n\n"
        ));
    }

    if !project_agents.is_empty() {
        out.push_str("Project agents  (.claude/agents/)\n\n");
        let mut sorted = project_agents;
        sorted.sort();
        for name in &sorted {
            out.push_str(&format!("  {name}\n"));
        }
        out.push('\n');
    } else {
        out.push_str("Project agents  (.claude/agents/)  — none\n\n");
    }

    out.push_str("Create <name>.md files in either agents/ directory to define agents.");
    CommandResult::Display(out)
}

fn mcp(args: &str) -> CommandResult {
    let parts: Vec<&str> = args.trim().splitn(2, ' ').collect();
    match parts.as_slice() {
        ["enable", name] => mcp_toggle(name, true),
        ["disable", name] => mcp_toggle(name, false),
        ["enable"] => mcp_toggle("all", true),
        ["disable"] => mcp_toggle("all", false),
        _ => mcp_list(),
    }
}

fn mcp_list() -> CommandResult {
    // Claude Code reads MCP server definitions from ~/.claude.json under
    // the `mcpServers` key (user-level) and each project entry's `mcpServers`
    // key. Super reads the same file so users get a unified view of any
    // servers they've already configured for claude.
    let path = dirs::home_dir()
        .map(|h| h.join(".claude.json"))
        .unwrap_or_default();

    // Parse the full claude.json once.
    let root: serde_json::Value = std::fs::read_to_string(&path)
        .ok()
        .and_then(|s| serde_json::from_str(&s).ok())
        .unwrap_or(serde_json::Value::Null);

    // User-level servers under the top-level `mcpServers` key.
    let user_servers: std::collections::BTreeMap<String, String> = root
        .get("mcpServers")
        .and_then(|v| v.as_object())
        .map(|m| {
            m.iter()
                .map(|(name, def)| {
                    let cmd = def
                        .get("command")
                        .and_then(|c| c.as_str())
                        .unwrap_or_else(|| {
                            def.get("url")
                                .and_then(|u| u.as_str())
                                .unwrap_or("(unknown)")
                        })
                        .to_string();
                    (name.clone(), cmd)
                })
                .collect()
        })
        .unwrap_or_default();

    // Project-level servers from projects[cwd].mcpServers.
    let cwd_key = std::env::current_dir()
        .map(|p| p.display().to_string())
        .unwrap_or_default();
    let project_servers: std::collections::BTreeMap<String, String> = root
        .get("projects")
        .and_then(|p| p.get(&cwd_key))
        .and_then(|p| p.get("mcpServers"))
        .and_then(|v| v.as_object())
        .map(|m| {
            m.iter()
                .map(|(name, def)| {
                    let cmd = def
                        .get("command")
                        .and_then(|c| c.as_str())
                        .unwrap_or_else(|| {
                            def.get("url")
                                .and_then(|u| u.as_str())
                                .unwrap_or("(unknown)")
                        })
                        .to_string();
                    (name.clone(), cmd)
                })
                .collect()
        })
        .unwrap_or_default();

    // Collect disabled server names from project config.
    let disabled: Vec<String> = root
        .get("projects")
        .and_then(|p| p.get(&cwd_key))
        .and_then(|p| p.get("disabledMcpServers"))
        .and_then(|v| v.as_array())
        .map(|arr| {
            arr.iter()
                .filter_map(|v| v.as_str().map(|s| s.to_string()))
                .collect()
        })
        .unwrap_or_default();

    if user_servers.is_empty() && project_servers.is_empty() {
        return CommandResult::Display(format!(
            "No MCP servers configured.\n\nLooked in: {}\n\nAdd servers under the `mcpServers` key of that file.",
            path.display(),
        ));
    }

    let mut out = String::from("MCP servers\n\n");

    // Merge all servers; project-level wins on name collision.
    let mut all: std::collections::BTreeMap<String, (String, &str)> =
        std::collections::BTreeMap::new();
    for (name, cmd) in &user_servers {
        all.insert(name.clone(), (cmd.clone(), "user"));
    }
    for (name, cmd) in &project_servers {
        all.insert(name.clone(), (cmd.clone(), "project"));
    }

    for (name, (cmd, _scope)) in &all {
        let status = if disabled.contains(name) {
            " (disabled)"
        } else {
            ""
        };
        out.push_str(&format!("  {name:<24}  {cmd}{status}\n"));
    }
    out.push_str(&format!(
        "\nTo enable/disable: /mcp enable <name> | /mcp disable <name>"
    ));
    CommandResult::Display(out)
}

/// Toggle a named MCP server's enabled/disabled state.
///
/// CC persists enabled/disabled state in `~/.claude.json` under
/// `projects[<cwd>].disabledMcpServers: string[]`.  A server whose name
/// appears in that array is disabled; absent means enabled.
///
/// `name` may be a server name or "all" (bulk toggle matching CC semantics).
fn mcp_toggle(name: &str, enable: bool) -> CommandResult {
    let path = dirs::home_dir()
        .map(|h| h.join(".claude.json"))
        .unwrap_or_default();

    // Load existing config (must be a JSON object).
    let mut root: serde_json::Value = std::fs::read_to_string(&path)
        .ok()
        .and_then(|s| serde_json::from_str(&s).ok())
        .unwrap_or_else(|| serde_json::json!({}));

    let cwd_key = std::env::current_dir()
        .map(|p| p.display().to_string())
        .unwrap_or_default();

    // Collect all known server names (user + project level).
    let user_server_names: Vec<String> = root
        .get("mcpServers")
        .and_then(|v| v.as_object())
        .map(|m| m.keys().cloned().collect())
        .unwrap_or_default();

    let project_server_names: Vec<String> = root
        .get("projects")
        .and_then(|p| p.get(&cwd_key))
        .and_then(|p| p.get("mcpServers"))
        .and_then(|v| v.as_object())
        .map(|m| m.keys().cloned().collect())
        .unwrap_or_default();

    let mut all_names: Vec<String> = user_server_names;
    for n in project_server_names {
        if !all_names.contains(&n) {
            all_names.push(n);
        }
    }

    // Ensure projects[cwd] exists.
    if root.get("projects").is_none() {
        root["projects"] = serde_json::json!({});
    }
    if root["projects"].get(&cwd_key).is_none() {
        root["projects"][&cwd_key] = serde_json::json!({});
    }

    // Read current disabled list.
    let mut disabled: Vec<String> = root["projects"][&cwd_key]
        .get("disabledMcpServers")
        .and_then(|v| v.as_array())
        .map(|arr| {
            arr.iter()
                .filter_map(|v| v.as_str().map(|s| s.to_string()))
                .collect()
        })
        .unwrap_or_default();

    let action_str = if enable { "enabled" } else { "disabled" };

    if name == "all" {
        // Bulk operation: collect servers that would actually change state.
        let to_toggle: Vec<String> = all_names
            .iter()
            .filter(|n| {
                let currently_disabled = disabled.contains(n);
                if enable {
                    currently_disabled // only enable those that are disabled
                } else {
                    !currently_disabled // only disable those that are enabled
                }
            })
            .cloned()
            .collect();

        if to_toggle.is_empty() {
            return CommandResult::Display(format!(
                "All MCP servers are already {action_str}"
            ));
        }

        // Apply changes.
        if enable {
            disabled.retain(|n| !to_toggle.contains(n));
        } else {
            for n in &to_toggle {
                if !disabled.contains(n) {
                    disabled.push(n.clone());
                }
            }
        }

        root["projects"][&cwd_key]["disabledMcpServers"] =
            serde_json::Value::Array(disabled.iter().map(|s| serde_json::json!(s)).collect());

        if let Ok(serialized) = serde_json::to_string_pretty(&root) {
            let _ = std::fs::write(&path, serialized);
        }

        let verb = if enable { "Enabled" } else { "Disabled" };
        return CommandResult::Display(format!(
            "{verb} {} MCP server(s)",
            to_toggle.len()
        ));
    }

    // Single-server toggle.
    if !all_names.contains(&name.to_string()) {
        return CommandResult::Display(format!(
            "MCP server \"{name}\" not found"
        ));
    }

    let currently_disabled = disabled.contains(&name.to_string());
    if enable {
        disabled.retain(|n| n != name);
    } else if !currently_disabled {
        disabled.push(name.to_string());
    }

    root["projects"][&cwd_key]["disabledMcpServers"] =
        serde_json::Value::Array(disabled.iter().map(|s| serde_json::json!(s)).collect());

    if let Ok(serialized) = serde_json::to_string_pretty(&root) {
        let _ = std::fs::write(&path, serialized);
    }

    CommandResult::Display(format!("MCP server \"{name}\" {action_str}"))
}

fn plugin() -> CommandResult {
    // CC description: "Manage Claude Code plugins"
    // CC opens an interactive marketplace/plugin manager. Super ships with the
    // Superpowers plugin bundled. A full marketplace is out of scope for v1.
    CommandResult::Display(
        "Plugins\n\
         \n\
         Installed\n\
         \n\
           superpowers  bundled  Superpowers skills + hook auto-loader\n\
         \n\
         Super ships with the Superpowers plugin built in.\n\
         A user-extensible plugin marketplace is not part of v1.\n\
         \n\
         See https://github.com/obra/superpowers for documentation."
            .into(),
    )
}

fn sandbox() -> CommandResult {
    // CC description dynamically shows: "○ sandbox disabled (⏎ to configure)"
    // CC opens an interactive toggle for macOS/Linux sandboxing (seatbelt/bubblewrap).
    // Super does not yet have sandbox support; E2B is planned post-v1.
    CommandResult::Display(
        "Sandbox\n\
         \n\
         Status: disabled\n\
         \n\
         Sandboxing restricts shell commands to a safe environment.\n\
         E2B sandbox integration is planned for a future release.\n\
         \n\
         No sandbox is active in this session."
            .into(),
    )
}

fn config_panel() -> CommandResult {
    // CC opens Settings dialog at "Config" tab. Key settings shown:
    //   Auto-compact, Show tips, Reduce motion, Thinking mode, Model, Theme,
    //   Verbose output, etc.
    // Super: show current live values and config file path.
    let path = dirs::home_dir()
        .map(|h| h.join(".super").join("config.json"))
        .unwrap_or_default();
    CommandResult::Display(format!(
        "Config\n\
         \n\
         Settings are stored in: {path}\n\
         Edit that file directly to change persistent settings.\n\
         \n\
         To change settings for this session use slash commands:\n\
         \n\
           /model <name>         Switch the active model\n\
           /think                Toggle extended thinking mode\n\
           /effort <level>       Set effort level (low/medium/high/max)\n\
           /mcp                  Manage MCP servers\n\
           /permissions          Manage tool allow/deny rules\n\
           /memory               View and edit CLAUDE.md memory files",
        path = path.display(),
    ))
}

fn permissions() -> CommandResult {
    // CC description: "Manage allow & deny tool permission rules"
    // CC opens PermissionRuleList with tabs: Recent denials, Allow rules, Ask rules,
    // Deny rules, Workspace directories.
    // Super: show allowed/denied rules from ~/.claude.json (same source as CC).
    let path = dirs::home_dir()
        .map(|h| h.join(".claude.json"))
        .unwrap_or_default();

    let root: serde_json::Value = std::fs::read_to_string(&path)
        .ok()
        .and_then(|s| serde_json::from_str(&s).ok())
        .unwrap_or(serde_json::Value::Null);

    let cwd_key = std::env::current_dir()
        .map(|p| p.display().to_string())
        .unwrap_or_default();

    // Read allow rules from projects[cwd].allowedTools
    let allow_rules: Vec<String> = root
        .get("projects")
        .and_then(|p| p.get(&cwd_key))
        .and_then(|p| p.get("allowedTools"))
        .and_then(|v| v.as_array())
        .map(|arr| {
            arr.iter()
                .filter_map(|v| v.as_str().map(|s| s.to_string()))
                .collect()
        })
        .unwrap_or_default();

    // Read deny rules from projects[cwd].deniedTools (if present)
    let deny_rules: Vec<String> = root
        .get("projects")
        .and_then(|p| p.get(&cwd_key))
        .and_then(|p| p.get("deniedTools"))
        .and_then(|v| v.as_array())
        .map(|arr| {
            arr.iter()
                .filter_map(|v| v.as_str().map(|s| s.to_string()))
                .collect()
        })
        .unwrap_or_default();

    let mut out = String::from("Permissions\n\nManage allow & deny tool permission rules.\n\n");

    out.push_str("Allow rules\n\n");
    if allow_rules.is_empty() {
        out.push_str("  (none)\n");
    } else {
        for rule in &allow_rules {
            out.push_str(&format!("  + {rule}\n"));
        }
    }

    out.push('\n');
    out.push_str("Deny rules\n\n");
    if deny_rules.is_empty() {
        out.push_str("  (none)\n");
    } else {
        for rule in &deny_rules {
            out.push_str(&format!("  - {rule}\n"));
        }
    }

    out.push_str(
        "\nRules are stored in ~/.claude.json under projects[<cwd>].allowedTools / .deniedTools.\n\
         Edit that file directly to add or remove rules.",
    );

    CommandResult::Display(out)
}

fn feedback(args: &str) -> CommandResult {
    // CC description: "Submit feedback about Claude Code"
    // CC opens a form that submits to GitHub Issues:
    //   https://github.com/anthropics/claude-code/issues
    // Super: open the GitHub Issues URL with the report pre-filled.
    let body = args.trim();
    let base_url = "https://github.com/anthropics/claude-code/issues/new";

    if body.is_empty() {
        CommandResult::Display(format!(
            "Submit feedback about Super\n\
             \n\
             Usage: /feedback <your report>\n\
             \n\
             Or open an issue directly:\n\
             {base_url}"
        ))
    } else {
        // URL-encode the body for the GitHub new-issue URL.
        let encoded = url_encode(body);
        let url = format!("{base_url}?body={encoded}");
        // Best-effort open in browser.
        let _ = std::process::Command::new("open").arg(&url).status();
        CommandResult::Display(format!(
            "Thanks for the feedback!\n\
             \n\
             Opening GitHub Issues with your report pre-filled.\n\
             If the browser didn't open, visit:\n\
             {base_url}"
        ))
    }
}

/// Minimal percent-encoding for URL query values (RFC 3986 unreserved chars pass through).
fn url_encode(s: &str) -> String {
    s.bytes()
        .flat_map(|b| match b {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'_' | b'.' | b'~' => {
                vec![b as char]
            }
            b' ' => vec!['+'],
            _ => format!("%{b:02X}").chars().collect(),
        })
        .collect()
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

use super::prompts;
use super::registry::{Command, CommandKind, CommandRegistry};
use crate::config::load_config;
use crate::state::store::Store;
use crate::tui::modal::Modal;
use crate::tui::modals::agents_view::AgentsView;
use crate::tui::modals::config_view::ConfigView;
use crate::tui::modals::effort_picker::EffortPicker;
use crate::tui::modals::mcp_list::McpList;
use crate::tui::modals::model_picker::ModelPicker;
use crate::tui::modals::provider_picker::ProviderPicker;
use crate::tui::modals::resume_picker::ResumePicker;
use crate::tui::modals::status_view::{StatusSnapshot, StatusView};

/// Dispatch a slash command. Returns the action the TUI should take.
///
/// `input` is the entire raw line typed by the user (e.g. "/model haiku"),
/// `command` is the resolved command from the registry. Any text after the
/// command name is treated as args.
pub fn dispatch(command: &Command, input: &str, store: &Store, session_id: &str) -> CommandResult {
    let args = extract_args(command, input);
    match command.name.as_str() {
        "/help" => help(),
        "/clear" => clear(store),
        "/exit" => CommandResult::Quit,
        "/version" => version(),
        "/status" => status(store),
        "/model" => model(args, store),
        "/provider" => provider(store),
        "/effort" => effort(args, store),
        "/think" => think(store),
        "/context" => context(store),
        "/diff" => diff(),
        "/export" => export(store),
        "/rename" => rename(args, session_id),
        "/resume" => resume(),
        "/login" => login(),
        "/logout" => logout(),
        "/memory" => memory(),
        "/agents" => agents(),
        "/mcp" => mcp(args),
        "/plugin" => plugin(),
        "/config" => config_panel(),
        "/permissions" => permissions(),

        "/init" => CommandResult::Prompt(prompts::init_prompt().to_string()),
        "/compact" => CommandResult::Prompt(prompts::compact_prompt(args)),
        "/review" => CommandResult::Prompt(prompts::review_prompt(args)),
        "/commit" => CommandResult::Prompt(prompts::commit_prompt().to_string()),
        "/commit-push-pr" => CommandResult::Prompt(prompts::commit_push_pr_prompt().to_string()),

        _ => match command.kind {
            CommandKind::Prompt => CommandResult::Prompt(args.to_string()),
            CommandKind::Local => {
                CommandResult::Display(format!("Command {} is not yet implemented.", command.name))
            }
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
    let commands = registry.list();

    // Compute max width of "name [hint]" for alignment.
    let col_width = commands
        .iter()
        .map(|c| c.name.len() + c.argument_hint.map(|h| h.len() + 1).unwrap_or(0))
        .max()
        .unwrap_or(20)
        + 2;

    let mut out = String::from("Super — Available Commands\n\n");
    for cmd in &commands {
        let name_hint = match cmd.argument_hint {
            Some(hint) => format!("{} {}", cmd.name, hint),
            None => cmd.name.clone(),
        };
        out.push_str(&format!(
            "  {:<width$}  {}\n",
            name_hint,
            cmd.description,
            width = col_width,
        ));
    }
    out.push_str(
        "\nType / to open the command menu · Tab to autocomplete · ? for keyboard shortcuts\n",
    );
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
    let cwd = std::env::current_dir()
        .map(|p| p.display().to_string())
        .unwrap_or_else(|_| "?".into());
    let slug = crate::providers::resolve_slug(&state.provider, &state.model_class);
    let snap = StatusSnapshot {
        version: env!("CARGO_PKG_VERSION").to_string(),
        model: slug.to_string(),
        provider: crate::providers::provider_display_name(&state.provider).to_string(),
        thinking: state.thinking_enabled,
        effort: state.effort_level.clone(),
        email: config
            .access_token
            .as_ref()
            .map(|_| "signed in".to_string()),
        messages: state.messages.len(),
        cwd,
    };
    CommandResult::OpenModal(Modal::Status(StatusView::new(snap)))
}

/// Convert an OpenRouter slug to a short display name matching CC's labels.
/// e.g. "anthropic/claude-sonnet-4-6" → "claude-sonnet-4-6"
fn friendly_model_short_name(slug: &str) -> String {
    // Strip provider prefix for display.
    let bare = slug.rsplit_once('/').map(|(_, r)| r).unwrap_or(slug);
    bare.to_string()
}

fn model(_args: &str, store: &Store) -> CommandResult {
    let state = store.get_state();
    CommandResult::OpenModal(Modal::Model(ModelPicker::new(
        state.provider.clone(),
        state.model_class.clone(),
    )))
}

fn provider(store: &Store) -> CommandResult {
    let current = store.get_state().provider.clone();
    CommandResult::OpenModal(Modal::Provider(ProviderPicker::new(current)))
}

fn effort(_args: &str, store: &Store) -> CommandResult {
    let current = store.get_state().effort_level.clone();
    CommandResult::OpenModal(Modal::Effort(EffortPicker::new(current)))
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
    let slug = crate::providers::resolve_slug(&state.provider, &state.model_class);
    let model = friendly_model_short_name(slug);
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
            ToolCall {
                name,
                input,
                result,
                ..
            } => {
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

fn rename(args: &str, session_id: &str) -> CommandResult {
    let name = args.trim();
    if name.is_empty() {
        return CommandResult::Display(
            "Could not generate a name: no conversation context yet. Usage: /rename <name>".into(),
        );
    }
    crate::conversation::transcript::write_meta_entry(
        session_id,
        &crate::conversation::transcript::TranscriptMeta::CustomTitle {
            custom_title: name.to_string(),
            session_id: session_id.to_string(),
        },
    );
    CommandResult::Display(format!("Session renamed to: {name}"))
}

fn resume() -> CommandResult {
    CommandResult::OpenModal(Modal::Resume(ResumePicker::new()))
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
        .as_str()
        == "true";
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
    CommandResult::OpenModal(Modal::Agents(AgentsView::new()))
}

fn mcp(_args: &str) -> CommandResult {
    CommandResult::OpenModal(Modal::Mcp(McpList::new()))
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

fn config_panel() -> CommandResult {
    CommandResult::OpenModal(Modal::Config(ConfigView::new()))
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
    /// Open an interactive modal, replacing the input bar.
    OpenModal(crate::tui::modal::Modal),
}

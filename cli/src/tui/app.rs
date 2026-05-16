use std::collections::VecDeque;
use std::sync::Arc;
use std::time::Duration;

use crossterm::event::{self, Event, KeyCode, KeyEventKind, KeyModifiers};
use crossterm::execute;
use crossterm::terminal::{disable_raw_mode, enable_raw_mode};
use tokio::sync::{broadcast, mpsc};
use ratatui::{
    backend::CrosstermBackend,
    layout::{Constraint, Direction, Layout, Rect},
    style::{Color, Style},
    text::{Line, Span},
    widgets::{Paragraph, Widget, Wrap},
    Frame, Terminal, TerminalOptions, Viewport,
};
use shared::CliConfig;

use super::activity::ActivityState;
use super::header::Header;
use super::input_bar::InputBar;
use super::scroll_area::{lines_height, Message, ScrollArea};
use super::slash_menu::SlashMenu;
use super::splash::MASCOT;
use crate::conversation::engine::ConversationEngine;
use crate::conversation::session_bus::SessionBus;
use crate::conversation::system_prompt::SystemPrompt;
use crate::sdk::protocol::BusMessage;
use crate::state::store::Store;
use crate::tools::ToolRegistry;
use crate::tui::modal::{Modal, ModalAction};

const SHORTCUTS_HELP: &str = "Shortcuts\n\
    enter        submit prompt\n\
    /            open command menu\n\
    ↑ / ↓        history · menu navigation\n\
    tab          autocomplete selected command\n\
    esc          dismiss menu · interrupt response · clear input\n\
    ctrl+c       clear input · exit when empty\n\
    ctrl+l       clear scrollback\n\
    ctrl+a/e     move cursor to start/end\n\
    page up/dn   scroll history\n\
    ?            show this help";

enum EngineEvent {
    Done {
        result: Result<String, String>,
        elapsed_secs: u64,
    },
}

enum AuthEvent {
    LoginDone(Result<String, String>),
}

pub struct App {
    header: Header,
    scroll_area: ScrollArea,
    activity: ActivityState,
    input: InputBar,
    slash_menu: SlashMenu,
    slash_menu_open: bool,
    _config: CliConfig,
    store: Arc<Store>,
    engine: ConversationEngine,
    /// Kept alive so the broadcast channel underlying `bus_rx` doesn't close
    /// when the original Arc is dropped after `App::new` returns.
    #[allow(dead_code)]
    bus: Arc<SessionBus>,
    bus_rx: broadcast::Receiver<BusMessage>,
    system_prompt: SystemPrompt,
    should_quit: bool,
    history: Vec<String>,
    history_idx: Option<usize>,
    inflight: Option<mpsc::UnboundedReceiver<EngineEvent>>,
    /// Prompts the user submitted while an engine call was in flight. Spawned
    /// FIFO when the current call ends, so a second prompt isn't dropped and
    /// doesn't race the first one's history write-back.
    queued_prompts: VecDeque<String>,
    /// Set when a root-level `Result` bus event arrives. The run loop flushes
    /// any rendered transcript content above the inline viewport via
    /// `terminal.insert_before`, so completed turns live in the terminal's
    /// native scrollback and the live region stays bounded.
    flush_to_scrollback: bool,
    auth_inflight: Option<mpsc::UnboundedReceiver<AuthEvent>>,
    status_fetch: Option<tokio::sync::mpsc::UnboundedReceiver<Result<(f64, f64), String>>>,
    modal: Option<Modal>,
}

fn verb_for_tool(name: &str) -> String {
    match name {
        "Read" => "Reading".into(),
        "Bash" => "Running".into(),
        "Edit" | "Write" | "NotebookEdit" => "Editing".into(),
        "Glob" | "Grep" | "ToolSearch" => "Searching".into(),
        "WebFetch" | "WebSearch" => "Browsing".into(),
        _ => format!("Running {}", name),
    }
}

impl App {
    pub fn new(
        config: CliConfig,
        store: Arc<Store>,
        engine: ConversationEngine,
        bus: Arc<SessionBus>,
        system_prompt: SystemPrompt,
    ) -> Self {
        let cwd = std::env::current_dir()
            .ok()
            .and_then(|p| {
                let home = dirs::home_dir()?;
                p.strip_prefix(&home).ok().map(|rel| format!("~/{}", rel.display()))
                    .or_else(|| Some(p.display().to_string()))
            })
            .unwrap_or_else(|| ".".to_string());
        let model = friendly_model_name(&config.model);
        let provider_label = crate::providers::provider_display_name(&config.provider).to_string();
        let header = Header::new(
            env!("CARGO_PKG_VERSION").to_string(),
            model,
            provider_label,
            cwd,
        );
        let bus_rx = bus.subscribe();
        Self {
            header,
            scroll_area: ScrollArea::new(),
            activity: ActivityState::idle(),
            input: InputBar::new(),
            slash_menu: SlashMenu::new(),
            slash_menu_open: false,
            _config: config,
            store,
            engine,
            bus: bus.clone(),
            bus_rx,
            system_prompt,
            should_quit: false,
            history: Vec::new(),
            history_idx: None,
            inflight: None,
            queued_prompts: VecDeque::new(),
            flush_to_scrollback: false,
            auth_inflight: None,
            status_fetch: None,
            modal: None,
        }
    }

    fn submit_text(&mut self, text: String) {
        if text.is_empty() {
            return;
        }
        self.history.push(text.clone());
        self.history_idx = None;
        if text.starts_with('/') {
            let registry = crate::commands::registry::CommandRegistry::new();
            if let Some(cmd) = registry.resolve(&text) {
                use crate::commands::dispatch::CommandResult;
                match crate::commands::dispatch::dispatch(cmd, &text, &self.store) {
                    CommandResult::Display(output) => {
                        self.scroll_area.push(Message::System(output));
                    }
                    CommandResult::Cleared => {
                        self.scroll_area.clear();
                    }
                    CommandResult::Prompt(prompt_text) => {
                        self.spawn_engine(prompt_text);
                    }
                    CommandResult::Quit => {
                        self.should_quit = true;
                    }
                    CommandResult::Login => {
                        self.spawn_login();
                    }
                    CommandResult::Logout => {
                        self.do_logout();
                    }
                    CommandResult::OpenModal(m) => {
                        let is_status = matches!(m, crate::tui::modal::Modal::Status(_));
                        self.modal = Some(m);
                        if is_status {
                            self.spawn_status_fetch();
                        }
                    }
                }
            } else {
                self.scroll_area.push(Message::System(format!(
                    "Unknown command: {text}. Type /help for available commands."
                )));
            }
            return;
        }
        self.spawn_engine(text);
    }

    fn spawn_engine(&mut self, prompt: String) {
        // Don't run two engines concurrently: they race on the shared history
        // store (engine 2 reads stale state, then overwrites engine 1's
        // write-back on completion). Queue and dispatch in process_pending
        // when the in-flight call finishes.
        if self.inflight.is_some() {
            self.queued_prompts.push_back(prompt);
            return;
        }
        let engine = self.engine.clone();
        let sp = self.system_prompt.clone();
        let (tx, rx) = mpsc::unbounded_channel();
        tokio::spawn(async move {
            let started = std::time::Instant::now();
            let result = engine.process_prompt(prompt, &sp, None).await;
            let _ = tx.send(EngineEvent::Done {
                result,
                elapsed_secs: started.elapsed().as_secs(),
            });
        });
        self.inflight = Some(rx);
        self.activity = ActivityState::active("Thinking");
    }

    fn spawn_login(&mut self) {
        if self.auth_inflight.is_some() {
            self.scroll_area
                .push(Message::System("Login already in progress.".into()));
            return;
        }
        let base_url = self._config.api_base_url.clone();
        self.scroll_area.push(Message::System(
            "Opening your browser to sign in to your Super account…".to_string(),
        ));
        let (tx, rx) = mpsc::unbounded_channel();
        tokio::spawn(async move {
            let client = crate::auth::AuthClient::new(base_url);
            let result = client
                .login_flow()
                .await
                .map(|cfg| {
                    cfg.access_token
                        .as_deref()
                        .map(|_| "Login successful.".to_string())
                        .unwrap_or_else(|| "Login successful (no token returned).".to_string())
                })
                .map_err(|e| e.to_string());
            let _ = tx.send(AuthEvent::LoginDone(result));
        });
        self.auth_inflight = Some(rx);
        self.activity = ActivityState::active("Signing in");
    }

    fn do_logout(&mut self) {
        let mut cfg = crate::config::load_config();
        let was_signed_in = cfg.access_token.is_some();
        cfg.access_token = None;
        cfg.refresh_token = None;
        cfg.openrouter_api_key = None;
        crate::config::save_config(&cfg);
        if was_signed_in {
            self.scroll_area
                .push(Message::System("Successfully logged out from your Super account.".into()));
        } else {
            self.scroll_area
                .push(Message::System("Not logged in.".into()));
        }
    }

    fn spawn_status_fetch(&mut self) {
        let base_url = self._config.api_base_url.clone();
        let token = match self._config.access_token.clone() {
            Some(t) => t,
            None => return, // not signed in; leave modal in Loading state
        };
        let (tx, rx) = tokio::sync::mpsc::unbounded_channel();
        tokio::spawn(async move {
            let result: Result<(f64, f64), String> = async {
                let resp = reqwest::Client::new()
                    .get(format!("{}/auth/usage", base_url))
                    .bearer_auth(&token)
                    .send()
                    .await
                    .map_err(|e| e.to_string())?;
                if !resp.status().is_success() {
                    return Err(format!("HTTP {}", resp.status()));
                }
                let body: serde_json::Value = resp.json().await.map_err(|e| e.to_string())?;
                let used  = body["used_usd"].as_f64().unwrap_or(0.0);
                let limit = body["limit_usd"].as_f64().unwrap_or(f64::MAX);
                Ok((used, limit))
            }.await;
            let _ = tx.send(result);
        });
        self.status_fetch = Some(rx);
    }

    fn handle_event(&mut self) -> std::io::Result<()> {
        if !event::poll(Duration::from_millis(50))? {
            return Ok(());
        }
        let Event::Key(key) = event::read()? else {
            return Ok(());
        };
        if key.kind != KeyEventKind::Press {
            return Ok(());
        }

        if let Some(ref mut modal) = self.modal {
            match modal.handle_key(key) {
                ModalAction::Continue => return Ok(()),
                ModalAction::Close => {
                    self.modal = None;
                    return Ok(());
                }
                ModalAction::SetClass(c) => {
                    self.store.set_model_class(&c);
                    self._config.model_class = c.clone();
                    self._config.model = crate::providers::resolve_slug(
                        &self._config.provider, &c
                    ).to_string();
                    crate::config::save_config(&self._config);
                    self.modal = None;
                    return Ok(());
                }
                ModalAction::SetProvider(p) => {
                    self.store.set_provider(&p);
                    self._config.provider = p.clone();
                    self._config.model = crate::providers::resolve_slug(
                        &p, &self._config.model_class
                    ).to_string();
                    crate::config::save_config(&self._config);
                    self.modal = None;
                    return Ok(());
                }
                ModalAction::SetEffort(e) => {
                    self.store.set_effort(e);
                    self.modal = None;
                    return Ok(());
                }
            }
        }

        let entries = if self.slash_menu_open {
            self.slash_menu.filter(&self.input.content)
        } else {
            Vec::new()
        };

        // Ctrl-bindings shared across modes.
        if key.modifiers.contains(KeyModifiers::CONTROL) {
            match key.code {
                KeyCode::Char('c') => {
                    if !self.input.content.is_empty() {
                        self.input.clear();
                        self.slash_menu_open = false;
                    } else {
                        self.should_quit = true;
                    }
                    return Ok(());
                }
                KeyCode::Char('l') => {
                    self.scroll_area.clear();
                    return Ok(());
                }
                KeyCode::Char('a') => {
                    self.input.move_home();
                    return Ok(());
                }
                KeyCode::Char('e') => {
                    self.input.move_end();
                    return Ok(());
                }
                KeyCode::Char('o') => {
                    self.scroll_area.toggle_detailed_transcript();
                    return Ok(());
                }
                _ => {}
            }
        }

        match key.code {
            KeyCode::Enter => {
                if self.slash_menu_open && !entries.is_empty() {
                    if let Some(entry) = entries.get(self.slash_menu.selected) {
                        self.input.clear();
                        let cmd = entry.name.clone();
                        self.slash_menu_open = false;
                        self.submit_text(cmd);
                    } else {
                        self.slash_menu_open = false;
                    }
                } else {
                    self.slash_menu_open = false;
                    let text = self.input.submit();
                    if !text.is_empty() {
                        self.submit_text(text);
                    }
                }
            }
            KeyCode::Char('?') if self.input.content.is_empty() => {
                self.scroll_area.push(Message::System(SHORTCUTS_HELP.to_string()));
            }
            KeyCode::Char(c) => {
                self.input.push_char(c);
                if self.input.content.starts_with('/') {
                    self.slash_menu_open = true;
                    self.slash_menu.selected = 0;
                } else {
                    self.slash_menu_open = false;
                }
            }
            KeyCode::Backspace => {
                self.input.delete_prev();
                if !self.input.content.starts_with('/') {
                    self.slash_menu_open = false;
                }
            }
            KeyCode::Left => self.input.move_left(),
            KeyCode::Right => self.input.move_right(),
            KeyCode::Home => self.input.move_home(),
            KeyCode::End => self.input.move_end(),
            KeyCode::Up => {
                if self.slash_menu_open {
                    self.slash_menu.cursor_up(&entries);
                } else if !self.history.is_empty() {
                    let idx = match self.history_idx {
                        None => self.history.len() - 1,
                        Some(0) => 0,
                        Some(i) => i - 1,
                    };
                    self.history_idx = Some(idx);
                    self.input.clear();
                    for c in self.history[idx].chars() {
                        self.input.push_char(c);
                    }
                }
            }
            KeyCode::Down => {
                if self.slash_menu_open {
                    self.slash_menu.cursor_down(&entries);
                } else if let Some(idx) = self.history_idx {
                    if idx + 1 >= self.history.len() {
                        self.history_idx = None;
                        self.input.clear();
                    } else {
                        let next = idx + 1;
                        self.history_idx = Some(next);
                        self.input.clear();
                        for c in self.history[next].chars() {
                            self.input.push_char(c);
                        }
                    }
                }
            }
            KeyCode::PageUp => self.scroll_area.scroll_up(),
            KeyCode::PageDown => self.scroll_area.scroll_down(),
            KeyCode::Tab => {
                if self.slash_menu_open {
                    if let Some(entry) = entries.get(self.slash_menu.selected) {
                        self.input.clear();
                        for c in entry.name.chars() {
                            self.input.push_char(c);
                        }
                    }
                }
            }
            KeyCode::Esc => {
                if self.slash_menu_open {
                    self.slash_menu_open = false;
                } else if !self.input.content.is_empty() {
                    self.input.clear();
                } else if self.inflight.is_some() {
                    // best-effort interrupt indicator; engine doesn't yet support cancellation
                    self.inflight = None;
                    self.activity = ActivityState::idle();
                }
            }
            _ => {}
        }
        Ok(())
    }

    fn process_pending(&mut self) {
        // Drain any BusMessage events the engine has emitted since last tick.
        loop {
            match self.bus_rx.try_recv() {
                Ok(msg) => {
                    // Side effects driven by specific bus events.
                    match &msg {
                        BusMessage::ToolProgress { tool_name, elapsed_seconds, .. } => {
                            // Update activity row to reflect the running tool.
                            // Keep verb stable across rapid Tool emissions: only
                            // recreate Activity if we were Idle or the verb changed.
                            let want_verb = verb_for_tool(tool_name);
                            let should_swap = match &self.activity {
                                ActivityState::Idle => true,
                                ActivityState::Active { verb, .. } => verb != &want_verb,
                            };
                            if should_swap {
                                self.activity = ActivityState::active(&want_verb);
                            }
                            // The elapsed-seconds value is already reflected by
                            // ActivityState::tick(); no extra wiring needed.
                            let _ = elapsed_seconds;
                        }
                        BusMessage::Result { parent_tool_use_id: None, .. } => {
                            self.activity = ActivityState::idle();
                            // Root-level Result marks the end of a turn. The
                            // run loop will flush the current transcript to
                            // terminal scrollback on the next iteration.
                            self.flush_to_scrollback = true;
                        }
                        BusMessage::Result { .. } => {
                            self.activity = ActivityState::idle();
                        }
                        _ => {}
                    }
                    self.scroll_area.push_event(msg);
                }
                Err(broadcast::error::TryRecvError::Empty) => break,
                Err(broadcast::error::TryRecvError::Closed) => break,
                Err(broadcast::error::TryRecvError::Lagged(_n)) => {
                    // Subscriber fell behind. Bus is sized for ~256 outstanding
                    // events; reaching here means a long-stuck render. Resync silently.
                }
            }
        }
        if let Some(rx) = self.status_fetch.as_mut() {
            match rx.try_recv() {
                Ok(result) => {
                    if let Some(crate::tui::modal::Modal::Status(ref mut sv)) = self.modal {
                        sv.set_usage(result.map_err(|e| e));
                    }
                    self.status_fetch = None;
                }
                Err(tokio::sync::mpsc::error::TryRecvError::Empty) => {}
                Err(tokio::sync::mpsc::error::TryRecvError::Disconnected) => {
                    self.status_fetch = None;
                }
            }
        }
        // Auth flow result first — it's small and orthogonal to engine events.
        if let Some(rx) = self.auth_inflight.as_mut() {
            match rx.try_recv() {
                Ok(AuthEvent::LoginDone(Ok(msg))) => {
                    self.scroll_area.push(Message::System(msg));
                    self.activity = ActivityState::idle();
                    self.auth_inflight = None;
                }
                Ok(AuthEvent::LoginDone(Err(e))) => {
                    self.scroll_area
                        .push(Message::System(format!("Login failed: {e}")));
                    self.activity = ActivityState::idle();
                    self.auth_inflight = None;
                }
                Err(mpsc::error::TryRecvError::Empty) => {}
                Err(mpsc::error::TryRecvError::Disconnected) => {
                    self.activity = ActivityState::idle();
                    self.auth_inflight = None;
                }
            }
        }
        let Some(rx) = self.inflight.as_mut() else {
            return;
        };
        match rx.try_recv() {
            Ok(EngineEvent::Done {
                result: Ok(_response),
                elapsed_secs,
            }) => {
                let verb = self
                    .activity
                    .verb()
                    .map(|v| past_tense(v))
                    .unwrap_or_else(|| "Cogitated".to_string());
                self.scroll_area
                    .push(Message::Trail(format!("{} for {}s", verb, elapsed_secs)));
                self.activity = ActivityState::idle();
                self.inflight = None;
                self.dispatch_next_queued();
            }
            Ok(EngineEvent::Done {
                result: Err(e),
                elapsed_secs,
            }) => {
                self.scroll_area
                    .push(Message::Assistant(format!("Error: {e}")));
                self.scroll_area
                    .push(Message::Trail(format!("Failed after {}s", elapsed_secs)));
                self.activity = ActivityState::idle();
                self.inflight = None;
                self.dispatch_next_queued();
            }
            Err(mpsc::error::TryRecvError::Empty) => {}
            Err(mpsc::error::TryRecvError::Disconnected) => {
                self.activity = ActivityState::idle();
                self.inflight = None;
                self.dispatch_next_queued();
            }
        }
    }

    /// Pull the next queued prompt (if any) and spawn an engine for it. Called
    /// after each inflight completion so user-queued prompts run FIFO.
    fn dispatch_next_queued(&mut self) {
        if let Some(next) = self.queued_prompts.pop_front() {
            self.spawn_engine(next);
        }
    }

    pub fn run(&mut self, mut terminal: Terminal<CrosstermBackend<std::io::Stdout>>) -> std::io::Result<()> {
        while !self.should_quit {
            self.activity.tick();
            // Flush completed turns into the terminal's native scrollback so
            // the inline live region stays bounded and the user can scroll up
            // in the terminal to see history.
            if self.flush_to_scrollback {
                self.flush_to_scrollback = false;
                let width = terminal.size().map(|s| s.width).unwrap_or(80);
                let lines = self.scroll_area.drain_to_lines();
                let height = lines_height(&lines, width);
                if height > 0 {
                    // Redraw the viewport WITHOUT the drained content first so
                    // the on-screen viewport reflects the post-drain state. If
                    // we skip this, ratatui's insert_before scrolls the pre-
                    // drain viewport (which still contains the turn) up into
                    // scrollback alongside the flushed lines — duplicating the
                    // content.
                    terminal.draw(|f| self.render(f))?;
                    terminal.insert_before(height, |buf| {
                        let area = buf.area;
                        Paragraph::new(lines).wrap(Wrap { trim: false }).render(area, buf);
                    })?;
                }
            }
            terminal.draw(|f| self.render(f))?;
            self.handle_event()?;
            self.process_pending();
        }
        // Best-effort cleanup of any async subagents still running.
        let _ = self.store.shutdown_async_agents();
        Ok(())
    }

    fn render_hint(&self, f: &mut Frame, area: Rect) {
        if area.height == 0 {
            return;
        }
        let dim = Style::default().fg(Color::DarkGray);
        let line = if self.slash_menu_open {
            Line::from(vec![
                Span::styled("  ↑↓ select · enter to run · esc to dismiss", dim),
            ])
        } else if self.scroll_area.is_detailed_transcript() {
            Line::from(vec![Span::styled("  Showing detailed transcript · ctrl+o to toggle", dim)])
        } else if matches!(self.activity, ActivityState::Active { .. }) {
            Line::from(vec![Span::styled("  esc to interrupt · ? for shortcuts", dim)])
        } else {
            Line::from(vec![Span::styled("  ? for shortcuts", dim)])
        };
        f.render_widget(Paragraph::new(line), area);
    }

    fn render(&mut self, f: &mut Frame) {
        let area = f.area();

        let activity_height = self.activity.height();
        let header_height   = MASCOT.len() as u16;

        if self.modal.is_some() {
            let modal_layout = Layout::default()
                .direction(Direction::Vertical)
                .constraints([
                    Constraint::Length(header_height),
                    Constraint::Length(1),      // spacer
                    Constraint::Min(1),         // scroll area
                    Constraint::Length(activity_height),
                    Constraint::Min(4),         // modal area
                ])
                .split(area);
            self.header.render(f, modal_layout[0]);
            self.scroll_area.render(f, modal_layout[2]);
            self.activity.render(f, modal_layout[3]);
            // Re-borrow modal immutably after mutable borrows are complete.
            if let Some(ref modal) = self.modal {
                modal.render(f, modal_layout[4]);
            }
        } else {
            let entries = if self.slash_menu_open {
                self.slash_menu.filter(&self.input.content)
            } else {
                Vec::new()
            };
            let menu_height = if self.slash_menu_open {
                SlashMenu::height(&entries)
            } else {
                0
            };
            let input_height = 3u16;
            let hint_height  = 1u16;
            let fixed = header_height + 1 + menu_height + activity_height + input_height + hint_height;
            let available_for_scroll = area.height.saturating_sub(fixed);
            let scroll_height = self.scroll_area
                .content_height(area.width)
                .min(available_for_scroll);

            let layout = Layout::default()
                .direction(Direction::Vertical)
                .constraints([
                    Constraint::Length(header_height),
                    Constraint::Length(1),
                    Constraint::Length(scroll_height),
                    Constraint::Length(menu_height),
                    Constraint::Length(activity_height),
                    Constraint::Length(input_height),
                    Constraint::Length(hint_height),
                ])
                .split(area);

            self.header.render(f, layout[0]);
            self.scroll_area.render(f, layout[2]);
            if self.slash_menu_open {
                self.slash_menu.render(f, layout[3], &entries);
            }
            self.activity.render(f, layout[4]);
            self.input.render(f, layout[5]);
            self.render_hint(f, layout[6]);
        }
    }
}

/// Maps a present-participle working verb to past tense for the trail line.
fn past_tense(verb: &str) -> String {
    match verb {
        "Thinking" => "Thought".to_string(),
        "Cogitating" => "Cogitated".to_string(),
        "Churning" => "Churned".to_string(),
        "Pondering" => "Pondered".to_string(),
        "Shenaniganing" => "Shenaniganed".to_string(),
        "Jitterbugging" => "Jitterbugged".to_string(),
        "Ruminating" => "Ruminated".to_string(),
        "Hatching" => "Hatched".to_string(),
        "Deliberating" => "Deliberated".to_string(),
        "Brewing" => "Brewed".to_string(),
        other => other.trim_end_matches("ing").to_string(),
    }
}

fn friendly_model_name(slug: &str) -> String {
    match slug {
        "anthropic/claude-opus-4-7" => "Opus 4.7 (1M context)".to_string(),
        "anthropic/claude-sonnet-4-6" => "Sonnet 4.6".to_string(),
        "anthropic/claude-haiku-4-5" | "anthropic/claude-haiku-4-5-20251001" => "Haiku 4.5".to_string(),
        other => other
            .rsplit_once('/')
            .map(|(_, rest)| rest.to_string())
            .unwrap_or_else(|| other.to_string()),
    }
}

pub async fn run_with_engine(
    config: CliConfig,
    store: Arc<Store>,
    engine: ConversationEngine,
    _registry: Arc<ToolRegistry>,
    bus: Arc<SessionBus>,
    system_prompt: SystemPrompt,
) {
    let height = crossterm::terminal::size().map(|(_, h)| h).unwrap_or(24);
    let _ = enable_raw_mode();
    let backend = CrosstermBackend::new(std::io::stdout());
    let terminal = Terminal::with_options(backend, TerminalOptions {
        viewport: Viewport::Inline(height),
    });
    if let Ok(terminal) = terminal {
        let mut app = App::new(config, store, engine, bus, system_prompt);
        let _ = app.run(terminal);
    }
    let _ = disable_raw_mode();
    let _ = execute!(std::io::stdout(), crossterm::cursor::Show);
    println!();
}

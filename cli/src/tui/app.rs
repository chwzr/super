use std::sync::Arc;
use std::time::Duration;

use crossterm::event::{self, Event, KeyCode, KeyEventKind, KeyModifiers};
use tokio::sync::mpsc;
use ratatui::{
    layout::{Constraint, Direction, Layout, Rect},
    style::{Color, Style},
    text::{Line, Span},
    widgets::Paragraph,
    DefaultTerminal, Frame,
};
use shared::CliConfig;

use super::activity::ActivityState;
use super::header::Header;
use super::input_bar::InputBar;
use super::scroll_area::{Message, ScrollArea};
use super::slash_menu::SlashMenu;
use crate::conversation::engine::ConversationEngine;
use crate::conversation::system_prompt::SystemPrompt;
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
    _registry: Arc<ToolRegistry>,
    system_prompt: SystemPrompt,
    should_quit: bool,
    history: Vec<String>,
    history_idx: Option<usize>,
    inflight: Option<mpsc::UnboundedReceiver<EngineEvent>>,
    auth_inflight: Option<mpsc::UnboundedReceiver<AuthEvent>>,
    modal: Option<Modal>,
}

impl App {
    pub fn new(
        config: CliConfig,
        store: Arc<Store>,
        engine: ConversationEngine,
        registry: Arc<ToolRegistry>,
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
        let header = Header::new(
            env!("CARGO_PKG_VERSION").to_string(),
            model,
            "OpenRouter".to_string(),
            cwd,
        );
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
            _registry: registry,
            system_prompt,
            should_quit: false,
            history: Vec::new(),
            history_idx: None,
            inflight: None,
            auth_inflight: None,
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
                        self.scroll_area.push(Message::User(text.clone()));
                        self.store.set_state(|s| {
                            s.messages.push(Message::User(prompt_text.clone()));
                        });
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
                        self.modal = Some(m);
                    }
                }
            } else {
                self.scroll_area.push(Message::System(format!(
                    "Unknown command: {text}. Type /help for available commands."
                )));
            }
            return;
        }
        let msg = Message::User(text.clone());
        self.scroll_area.push(msg);
        self.store.set_state(|s| {
            s.messages.push(Message::User(text.clone()));
        });
        self.spawn_engine(text);
    }

    fn spawn_engine(&mut self, prompt: String) {
        let engine = self.engine.clone();
        let sp = self.system_prompt.clone();
        let (tx, rx) = mpsc::unbounded_channel();
        tokio::spawn(async move {
            let started = std::time::Instant::now();
            let result = engine
                .process_prompt(prompt, &sp)
                .await
                .map_err(|e| e.to_string());
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
                ModalAction::SetModel(m) => {
                    self.store.set_model(m);
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
                result: Ok(response),
                elapsed_secs,
            }) => {
                let verb = self
                    .activity
                    .verb()
                    .map(|v| past_tense(v))
                    .unwrap_or_else(|| "Cogitated".to_string());
                self.scroll_area.push(Message::Assistant(response));
                self.scroll_area
                    .push(Message::Trail(format!("{} for {}s", verb, elapsed_secs)));
                self.activity = ActivityState::idle();
                self.inflight = None;
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
            }
            Err(mpsc::error::TryRecvError::Empty) => {}
            Err(mpsc::error::TryRecvError::Disconnected) => {
                self.activity = ActivityState::idle();
                self.inflight = None;
            }
        }
    }

    pub fn run(&mut self, mut terminal: DefaultTerminal) -> std::io::Result<()> {
        while !self.should_quit {
            self.activity.tick();
            terminal.draw(|f| self.render(f))?;
            self.handle_event()?;
            self.process_pending();
        }
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
        } else if matches!(self.activity, ActivityState::Active { .. }) {
            Line::from(vec![Span::styled("  esc to interrupt · ? for shortcuts", dim)])
        } else {
            Line::from(vec![Span::styled("  ? for shortcuts", dim)])
        };
        f.render_widget(Paragraph::new(line), area);
    }

    fn render(&self, f: &mut Frame) {
        let area = f.area();

        let activity_height = self.activity.height();
        let header_height   = 3u16;

        if let Some(ref modal) = self.modal {
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
            modal.render(f, modal_layout[4]);
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

            let layout = Layout::default()
                .direction(Direction::Vertical)
                .constraints([
                    Constraint::Length(header_height),
                    Constraint::Length(1),
                    Constraint::Min(1),
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
        "anthropic/claude-haiku-4-5" => "Haiku 4.5".to_string(),
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
    registry: Arc<ToolRegistry>,
    system_prompt: SystemPrompt,
) {
    let terminal = ratatui::init();
    let mut app = App::new(config, store, engine, registry, system_prompt);
    let _ = app.run(terminal);
    ratatui::restore();
}

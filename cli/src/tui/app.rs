use std::collections::{HashMap, VecDeque};
use std::sync::Arc;
use std::time::Duration;

use crossterm::event::{self, Event, KeyCode, KeyEventKind, KeyModifiers};
use crossterm::execute;
use crossterm::terminal::{disable_raw_mode, enable_raw_mode};
use ratatui::{
    backend::CrosstermBackend,
    layout::{Constraint, Direction, Layout, Rect},
    style::{Color, Style},
    text::{Line, Span},
    widgets::{Paragraph, Widget, Wrap},
    Frame, Terminal, TerminalOptions, Viewport,
};
use shared::CliConfig;
use tokio::sync::{broadcast, mpsc};

use super::activity::ActivityState;
use super::input_bar::InputBar;
use super::scroll_area::{Message, ScrollArea};
use super::slash_menu::SlashMenu;
use super::splash;
use crate::conversation::engine::ConversationEngine;
use crate::conversation::message_queue::{MessageQueue, PromptInputMode, QueuePriority};
use crate::conversation::session_bus::SessionBus;
use crate::conversation::system_prompt::SystemPrompt;
use crate::sdk::protocol::BusMessage;
use crate::state::store::Store;
use crate::tools::ToolRegistry;
use crate::tui::modal::{Modal, ModalAction};
use crate::tui::render::{item_to_lines, lines_height, message_to_lines};
use crate::tui::transcript::{fold, group_tool_batches, TranscriptItem};

const SHORTCUTS_HELP: &str = "Shortcuts\n\
    enter        submit prompt\n\
    /            open command menu\n\
    ↑ / ↓        history · menu navigation\n\
    tab          autocomplete selected command\n\
    esc          dismiss menu · interrupt response · clear input\n\
    ctrl+c       clear input · exit when empty\n\
    ctrl+l       clear the terminal screen\n\
    ctrl+a/e     move cursor to start/end\n\
    ?            show this help";

/// Fixed height of the inline viewport. Reserves enough rows at the bottom of
/// the terminal for: input bar (3) + hint (1) + activity (≤2) + slash menu
/// (≤8) + a small in-flight tail (≈3 rows). The rest of the terminal scrolls
/// normally — completed transcript items are written into that scrollback via
/// `Terminal::insert_before`.
const VIEWPORT_HEIGHT: u16 = 14;

enum EngineEvent {
    Done {
        result: Result<String, String>,
        elapsed_secs: u64,
    },
}

enum AuthEvent {
    LoginDone(Result<String, String>),
}

/// Tracks a pending interactive tool invocation while the modal is displayed.
struct PendingInteraction {
    tool_use_id: String,
    parent_tool_use_id: Option<String>,
}

pub struct App {
    scroll_area: ScrollArea,
    activity: ActivityState,
    input: InputBar,
    slash_menu: SlashMenu,
    slash_menu_open: bool,
    _config: CliConfig,
    store: Arc<Store>,
    engine: ConversationEngine,
    /// Shared message queue for mid-turn user input and system notifications.
    queue: Arc<MessageQueue>,
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
    /// Abort handle for the in-flight engine task. ESC uses this to actually
    /// stop processing — the engine has no cooperative cancellation point in
    /// its turn/SSE loop, so aborting the JoinHandle is what reliably halts
    /// further tool calls and bus emits.
    inflight_abort: Option<tokio::task::AbortHandle>,
    /// Prompts the user submitted while an engine call was in flight. Spawned
    /// FIFO when the current call ends, so a second prompt isn't dropped and
    /// doesn't race the first one's history write-back.
    queued_prompts: VecDeque<String>,
    auth_inflight: Option<mpsc::UnboundedReceiver<AuthEvent>>,
    status_fetch: Option<tokio::sync::mpsc::UnboundedReceiver<Result<(f64, f64), String>>>,
    modal: Option<Modal>,
    /// When `Some`, a tool is awaiting user interaction (AskUserQuestion,
    /// permission prompt, etc.). The modal is showing; on submit/cancel we
    /// emit InteractionResponse / InteractionDenied so the tool loop resumes.
    pending_interaction: Option<PendingInteraction>,
    /// Splash banner lines written to scrollback once on first tick.
    /// `Some` until flushed, then `None`.
    splash_pending: Option<Vec<Line<'static>>>,
    /// Number of `scroll_area.messages` entries already pushed to scrollback.
    flushed_message_count: usize,
    /// Number of folded transcript items already pushed to scrollback (i.e.
    /// items 0..next_flush_idx are in scrollback). Items at or after this
    /// index are in flight and render in the live tail of the viewport.
    next_flush_idx: usize,
    /// For in-flight `AssistantText` items at `next_flush_idx`: number of
    /// leading chars already flushed to scrollback line-by-line. The live
    /// tail renders only `text[chars..]`.
    flushed_chars_per_block: HashMap<usize, usize>,
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

/// An item is "stable" once we know its rendering will not change again, so
/// it can be safely pushed to terminal scrollback as a complete unit.
fn is_stable(item: &TranscriptItem) -> bool {
    match item {
        TranscriptItem::User { .. } => true,
        TranscriptItem::AssistantText { complete, .. } => *complete,
        TranscriptItem::Thinking { complete, .. } => *complete,
        TranscriptItem::ToolCall { result, .. } => result.is_some(),
        TranscriptItem::ToolBatch { calls } => calls.iter().all(|c| c.result.is_some()),
        TranscriptItem::System { .. } => true,
    }
}

impl App {
    pub fn new(
        config: CliConfig,
        store: Arc<Store>,
        engine: ConversationEngine,
        bus: Arc<SessionBus>,
        system_prompt: SystemPrompt,
        queue: Arc<MessageQueue>,
    ) -> Self {
        let cwd = std::env::current_dir()
            .ok()
            .and_then(|p| {
                let home = dirs::home_dir()?;
                p.strip_prefix(&home)
                    .ok()
                    .map(|rel| format!("~/{}", rel.display()))
                    .or_else(|| Some(p.display().to_string()))
            })
            .unwrap_or_else(|| ".".to_string());
        let model = friendly_model_name(&config.model);
        let provider_label = crate::providers::provider_display_name(&config.provider).to_string();
        let splash_lines = splash::banner_lines(
            env!("CARGO_PKG_VERSION"),
            &model,
            &provider_label,
            "super",
            &cwd,
        );
        let bus_rx = bus.subscribe();
        Self {
            scroll_area: ScrollArea::new(),
            activity: ActivityState::idle(),
            input: InputBar::new(),
            slash_menu: SlashMenu::new(),
            slash_menu_open: false,
            _config: config,
            store,
            engine,
            queue,
            bus: bus.clone(),
            bus_rx,
            system_prompt,
            should_quit: false,
            history: Vec::new(),
            history_idx: None,
            inflight: None,
            inflight_abort: None,
            queued_prompts: VecDeque::new(),
            auth_inflight: None,
            status_fetch: None,
            modal: None,
            pending_interaction: None,
            splash_pending: Some(splash_lines),
            flushed_message_count: 0,
            next_flush_idx: 0,
            flushed_chars_per_block: HashMap::new(),
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
                        self.reset_scrollback_state();
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

    /// Wipe accumulated transcript state and reset flush counters. Used by
    /// `/clear` — the next render cycle will clear the live tail; terminal
    /// scrollback is cleared separately via Ctrl+L.
    fn reset_scrollback_state(&mut self) {
        self.scroll_area.clear();
        self.flushed_message_count = 0;
        self.next_flush_idx = 0;
        self.flushed_chars_per_block.clear();
    }

    fn spawn_engine(&mut self, prompt: String) {
        // Don't run two engines concurrently: they race on the shared history
        // store (engine 2 reads stale state, then overwrites engine 1's
        // write-back on completion). Queue and dispatch in process_pending
        // when the in-flight call finishes.
        if self.inflight.is_some() {
            // Enqueue to message queue so engine drain picks it up mid-turn.
            // Do NOT also push to queued_prompts — that would cause the same
            // input to be processed twice (once mid-turn, once post-turn).
            self.queue
                .enqueue(crate::conversation::message_queue::QueuedCommand {
                    value: prompt,
                    mode: PromptInputMode::Prompt,
                    priority: QueuePriority::Next,
                    agent_id: None,
                    is_meta: false,
                    uuid: uuid::Uuid::new_v4(),
                });
            return;
        }
        let engine = self.engine.clone();
        let sp = self.system_prompt.clone();
        let (tx, rx) = mpsc::unbounded_channel();
        let handle = tokio::spawn(async move {
            let started = std::time::Instant::now();
            let result = engine.process_prompt(prompt, &sp, None).await;
            let _ = tx.send(EngineEvent::Done {
                result,
                elapsed_secs: started.elapsed().as_secs(),
            });
        });
        self.inflight_abort = Some(handle.abort_handle());
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
            self.scroll_area.push(Message::System(
                "Successfully logged out from your Super account.".into(),
            ));
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
                let used = body["used_usd"].as_f64().unwrap_or(0.0);
                let limit = body["limit_usd"].as_f64().unwrap_or(f64::MAX);
                Ok((used, limit))
            }
            .await;
            let _ = tx.send(result);
        });
        self.status_fetch = Some(rx);
    }

    fn handle_event(
        &mut self,
        terminal: &mut Terminal<CrosstermBackend<std::io::Stdout>>,
    ) -> std::io::Result<()> {
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
                    if let Some(ref interaction) = self.pending_interaction {
                        self.bus.emit(BusMessage::InteractionDenied {
                            tool_use_id: interaction.tool_use_id.clone(),
                            parent_tool_use_id: interaction.parent_tool_use_id.clone(),
                            uuid: uuid::Uuid::new_v4(),
                            session_id: self.bus.session_id().to_string(),
                        });
                        self.pending_interaction = None;
                    }
                    self.modal = None;
                    return Ok(());
                }
                ModalAction::SetClass(c) => {
                    self.store.set_model_class(&c);
                    self._config.model_class = c.clone();
                    self._config.model =
                        crate::providers::resolve_slug(&self._config.provider, &c).to_string();
                    crate::config::save_config(&self._config);
                    self.modal = None;
                    return Ok(());
                }
                ModalAction::SetProvider(p) => {
                    self.store.set_provider(&p);
                    self._config.provider = p.clone();
                    self._config.model =
                        crate::providers::resolve_slug(&p, &self._config.model_class).to_string();
                    crate::config::save_config(&self._config);
                    self.modal = None;
                    return Ok(());
                }
                ModalAction::SetEffort(e) => {
                    self.store.set_effort(e);
                    self.modal = None;
                    return Ok(());
                }
                ModalAction::SubmitAnswers(payload) => {
                    // The modal was an InteractionRequested widget
                    // (MultiQuestion). Emit the response so the suspended
                    // tool loop can resume with user answers.
                    if let Some(ref interaction) = self.pending_interaction {
                        self.bus.emit(BusMessage::InteractionResponse {
                            tool_use_id: interaction.tool_use_id.clone(),
                            payload,
                            parent_tool_use_id: interaction.parent_tool_use_id.clone(),
                            uuid: uuid::Uuid::new_v4(),
                            session_id: self.bus.session_id().to_string(),
                        });
                    }
                    self.modal = None;
                    self.pending_interaction = None;
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
                    // Clear the terminal screen (chrome + scrollback). Keeps
                    // conversation state intact, but freshly-arrived content
                    // is the only thing that will re-appear. Use `/clear` to
                    // also wipe history.
                    let _ = terminal.clear();
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
                self.scroll_area
                    .push(Message::System(SHORTCUTS_HELP.to_string()));
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
            KeyCode::Tab if self.slash_menu_open => {
                if let Some(entry) = entries.get(self.slash_menu.selected) {
                    self.input.clear();
                    for c in entry.name.chars() {
                        self.input.push_char(c);
                    }
                }
            }
            KeyCode::Esc => {
                if self.slash_menu_open {
                    self.slash_menu_open = false;
                } else if !self.input.content.is_empty() {
                    self.input.clear();
                } else if self.inflight.is_some() {
                    if let Some(h) = self.inflight_abort.take() {
                        h.abort();
                    }
                    self.inflight = None;
                    self.queued_prompts.clear();
                    self.queue.clear();
                    self.activity = ActivityState::idle();
                    self.scroll_area
                        .push(Message::Trail("Interrupted by user".to_string()));
                    if let Some(last) = self.history.last().cloned() {
                        self.input.clear();
                        for c in last.chars() {
                            self.input.push_char(c);
                        }
                        self.history_idx = None;
                    }
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
                        BusMessage::ToolProgress {
                            tool_name,
                            elapsed_seconds,
                            ..
                        } => {
                            let want_verb = verb_for_tool(tool_name);
                            let should_swap = match &self.activity {
                                ActivityState::Idle => true,
                                ActivityState::Active { verb, .. } => verb != &want_verb,
                            };
                            if should_swap {
                                self.activity = ActivityState::active(&want_verb);
                            }
                            let _ = elapsed_seconds;
                        }
                        BusMessage::Result { .. } => {
                            self.activity = ActivityState::idle();
                        }
                        BusMessage::RenderEvent { .. } => {
                            // Batch 1: tools only emit RenderSpec::Nothing,
                            // which renders to nothing. Batches 2-5 wire this
                            // into the scrollback / live region.
                        }
                        BusMessage::InteractionRequested {
                            tool_use_id,
                            spec:
                                shared::RenderSpec::Interactive {
                                    widget: shared::InteractiveWidget::MultiQuestion { questions },
                                    ..
                                },
                            parent_tool_use_id,
                            ..
                        } => {
                            self.pending_interaction = Some(PendingInteraction {
                                tool_use_id: tool_use_id.clone(),
                                parent_tool_use_id: parent_tool_use_id.clone(),
                            });
                            self.modal = Some(Modal::Question(
                                crate::tui::modals::question::QuestionModal::new(questions.clone()),
                            ));
                        }
                        _ => {}
                    }
                    self.scroll_area.push_event(msg);
                }
                Err(broadcast::error::TryRecvError::Empty) => break,
                Err(broadcast::error::TryRecvError::Closed) => break,
                Err(broadcast::error::TryRecvError::Lagged(_n)) => {
                    // Resync silently — see SessionBus comment on capacity.
                }
            }
        }
        if let Some(rx) = self.status_fetch.as_mut() {
            match rx.try_recv() {
                Ok(result) => {
                    if let Some(crate::tui::modal::Modal::Status(ref mut sv)) = self.modal {
                        sv.set_usage(result);
                    }
                    self.status_fetch = None;
                }
                Err(tokio::sync::mpsc::error::TryRecvError::Empty) => {}
                Err(tokio::sync::mpsc::error::TryRecvError::Disconnected) => {
                    self.status_fetch = None;
                }
            }
        }
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
                    .map(past_tense)
                    .unwrap_or_else(|| "Cogitated".to_string());
                self.scroll_area
                    .push(Message::Trail(format!("{} for {}s", verb, elapsed_secs)));
                self.activity = ActivityState::idle();
                self.inflight = None;
                self.inflight_abort = None;
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
                self.inflight_abort = None;
                self.dispatch_next_queued();
            }
            Err(mpsc::error::TryRecvError::Empty) => {}
            Err(mpsc::error::TryRecvError::Disconnected) => {
                self.activity = ActivityState::idle();
                self.inflight = None;
                self.inflight_abort = None;
                self.dispatch_next_queued();
            }
        }
    }

    fn dispatch_next_queued(&mut self) {
        // First check local deque (prompts submitted while engine was idle)
        if let Some(next) = self.queued_prompts.pop_front() {
            self.spawn_engine(next);
            return;
        }
        // Then check message queue for any pending Prompt commands
        // (e.g. cron firings queued while engine was running)
        let prompts = self
            .queue
            .drain(QueuePriority::Later, Some(None));
        for cmd in prompts {
            if matches!(cmd.mode, PromptInputMode::Prompt) {
                self.spawn_engine(cmd.value);
                return;
            }
        }
    }

    /// Push newly-finalized items (legacy `Message`s and `TranscriptItem`s
    /// folded from bus events) into the terminal's native scroll buffer via
    /// `Terminal::insert_before`. Runs once per loop iteration before draw.
    ///
    /// For an in-flight `AssistantText` at `next_flush_idx`, this also pushes
    /// any newly-complete lines (up to the last `\n`) so streaming text moves
    /// into scrollback line-by-line — matches Claude Code's behaviour.
    fn flush_to_scrollback(
        &mut self,
        terminal: &mut Terminal<CrosstermBackend<std::io::Stdout>>,
    ) -> std::io::Result<()> {
        let width = terminal.size().map(|s| s.width).unwrap_or(80);

        if let Some(lines) = self.splash_pending.take() {
            self.insert_lines(terminal, lines, width)?;
        }

        // Flush folded items first, then legacy messages. The engine's
        // post-turn Trail line (`Message::Trail("Cogitated for Ns")`) is
        // pushed in the same tick as the final bus events are drained, so
        // items-first ensures Trail lands AFTER the conversation in
        // scrollback. Slash-command outputs (`Message::System`) are pushed
        // before any engine activity, so they flush in earlier ticks
        // before any items exist — the ordering is preserved across ticks
        // even with items-first within a single tick.
        let items = group_tool_batches(fold(&self.scroll_area.events, None));
        while let Some(item) = items.get(self.next_flush_idx) {
            if is_stable(item) {
                let already = self
                    .flushed_chars_per_block
                    .get(&self.next_flush_idx)
                    .copied()
                    .unwrap_or(0);
                let detailed = self.scroll_area.is_detailed_transcript();
                let lines = item_to_lines(item, already, detailed);
                self.insert_lines(terminal, lines, width)?;
                self.flushed_chars_per_block.remove(&self.next_flush_idx);
                self.next_flush_idx += 1;
                continue;
            }

            // In-flight item. For streaming assistant text, push completed
            // lines (everything up to and including the last `\n`) and leave
            // the trailing partial line in the live tail.
            if let TranscriptItem::AssistantText { text, .. } = item {
                let already = self
                    .flushed_chars_per_block
                    .get(&self.next_flush_idx)
                    .copied()
                    .unwrap_or(0);
                if let Some(rel_nl) = text[already..].rfind('\n') {
                    let until = already + rel_nl + 1;
                    let chunk_item = TranscriptItem::AssistantText {
                        text: text[..until].to_string(),
                        complete: false,
                    };
                    let detailed = self.scroll_area.is_detailed_transcript();
                    let lines = item_to_lines(&chunk_item, already, detailed);
                    self.insert_lines(terminal, lines, width)?;
                    self.flushed_chars_per_block
                        .insert(self.next_flush_idx, until);
                }
            }
            break;
        }

        while self.flushed_message_count < self.scroll_area.messages.len() {
            let msg = self.scroll_area.messages[self.flushed_message_count].clone();
            let lines = message_to_lines(&msg);
            self.insert_lines(terminal, lines, width)?;
            self.flushed_message_count += 1;
        }

        Ok(())
    }

    fn insert_lines(
        &self,
        terminal: &mut Terminal<CrosstermBackend<std::io::Stdout>>,
        lines: Vec<Line<'static>>,
        width: u16,
    ) -> std::io::Result<()> {
        let h = lines_height(&lines, width);
        if h == 0 {
            return Ok(());
        }
        terminal.insert_before(h, |buf| {
            let area = buf.area;
            Paragraph::new(lines)
                .wrap(Wrap { trim: false })
                .render(area, buf);
        })
    }

    pub fn run(
        &mut self,
        mut terminal: Terminal<CrosstermBackend<std::io::Stdout>>,
    ) -> std::io::Result<()> {
        while !self.should_quit {
            self.activity.tick();
            self.flush_to_scrollback(&mut terminal)?;
            terminal.draw(|f| self.render(f))?;
            self.handle_event(&mut terminal)?;
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
            Line::from(vec![Span::styled(
                "  ↑↓ select · enter to run · esc to dismiss",
                dim,
            )])
        } else if self.scroll_area.is_detailed_transcript() {
            Line::from(vec![Span::styled(
                "  Showing detailed transcript · ctrl+o to collapse",
                dim,
            )])
        } else if matches!(self.activity, ActivityState::Active { .. }) {
            Line::from(vec![Span::styled(
                "  esc to interrupt · ? for shortcuts",
                dim,
            )])
        } else {
            Line::from(vec![Span::styled("  ? for shortcuts", dim)])
        };
        f.render_widget(Paragraph::new(line), area);
    }

    fn render_live_tail(&self, f: &mut Frame, area: Rect) {
        if area.height == 0 {
            return;
        }
        let items = group_tool_batches(fold(&self.scroll_area.events, None));
        let mut lines: Vec<Line<'static>> = Vec::new();
        for (i, item) in items.iter().enumerate().skip(self.next_flush_idx) {
            let already = self.flushed_chars_per_block.get(&i).copied().unwrap_or(0);
            lines.extend(item_to_lines(
                item,
                already,
                self.scroll_area.is_detailed_transcript(),
            ));
        }
        if lines.is_empty() {
            return;
        }
        let para = Paragraph::new(lines).wrap(Wrap { trim: false });
        f.render_widget(para, area);
    }

    fn render(&mut self, f: &mut Frame) {
        let area = f.area();

        if self.modal.is_some() {
            // Modal occupies the live area; chrome shrinks away.
            let modal_layout = Layout::default()
                .direction(Direction::Vertical)
                .constraints([Constraint::Min(4), Constraint::Length(1)])
                .split(area);
            if let Some(ref modal) = self.modal {
                modal.render(f, modal_layout[0]);
            }
            return;
        }

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
        let hint_height = 1u16;
        let activity_height = self.activity.height();

        let layout = Layout::default()
            .direction(Direction::Vertical)
            .constraints([
                Constraint::Min(0),
                Constraint::Length(menu_height),
                Constraint::Length(activity_height),
                Constraint::Length(input_height),
                Constraint::Length(hint_height),
            ])
            .split(area);

        self.render_live_tail(f, layout[0]);
        if self.slash_menu_open {
            self.slash_menu.render(f, layout[1], &entries);
        }
        self.activity.render(f, layout[2]);
        self.input.render(f, layout[3]);
        self.render_hint(f, layout[4]);
    }
}

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
        "anthropic/claude-haiku-4-5" | "anthropic/claude-haiku-4-5-20251001" => {
            "Haiku 4.5".to_string()
        }
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
    queue: Arc<MessageQueue>,
) {
    let term_height = crossterm::terminal::size().map(|(_, h)| h).unwrap_or(24);
    let viewport_height = VIEWPORT_HEIGHT.min(term_height);
    let _ = enable_raw_mode();
    let backend = CrosstermBackend::new(std::io::stdout());
    let terminal = Terminal::with_options(
        backend,
        TerminalOptions {
            viewport: Viewport::Inline(viewport_height),
        },
    );
    if let Ok(terminal) = terminal {
        let mut app = App::new(config, store, engine, bus, system_prompt, queue);
        let _ = app.run(terminal);
    }
    let _ = disable_raw_mode();
    let _ = execute!(std::io::stdout(), crossterm::cursor::Show);
    println!();
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::sdk::protocol::SystemSubtype;

    fn user_item(text: &str) -> TranscriptItem {
        TranscriptItem::User { text: text.into() }
    }
    fn assistant(text: &str, complete: bool) -> TranscriptItem {
        TranscriptItem::AssistantText {
            text: text.into(),
            complete,
        }
    }
    fn thinking(complete: bool) -> TranscriptItem {
        TranscriptItem::Thinking {
            text: "x".into(),
            collapsed: true,
            elapsed_ms: 0,
            complete,
        }
    }
    fn tool_call(result: Option<&str>) -> TranscriptItem {
        TranscriptItem::ToolCall {
            tool_use_id: "tu_1".into(),
            name: "Read".into(),
            input: serde_json::json!({"file_path": "/x"}),
            result: result.map(|c| crate::tui::transcript::ToolResultRender {
                content: c.into(),
                is_error: false,
            }),
            elapsed_ms: 0,
        }
    }

    #[test]
    fn is_stable_classifies_each_variant() {
        assert!(is_stable(&user_item("hi")));
        assert!(is_stable(&assistant("done", true)));
        assert!(!is_stable(&assistant("partial", false)));
        assert!(is_stable(&thinking(true)));
        assert!(!is_stable(&thinking(false)));
        assert!(is_stable(&tool_call(Some("ok"))));
        assert!(!is_stable(&tool_call(None)));
        assert!(is_stable(&TranscriptItem::System {
            subtype: SystemSubtype::Notice,
            message: "x".into(),
        }));
    }

    #[test]
    fn is_stable_toolbatch_requires_all_results() {
        use crate::tui::transcript::BatchCall;
        let mk = |has_result: bool| TranscriptItem::ToolBatch {
            calls: vec![BatchCall {
                tool_use_id: "1".into(),
                name: "Read".into(),
                input: serde_json::json!({}),
                result: if has_result {
                    Some(crate::tui::transcript::ToolResultRender {
                        content: "ok".into(),
                        is_error: false,
                    })
                } else {
                    None
                },
            }],
        };
        assert!(is_stable(&mk(true)));
        assert!(!is_stable(&mk(false)));
    }

    fn next_flush_chunk(text: &str, already: usize) -> Option<(String, usize)> {
        let tail = &text[already..];
        let rel_nl = tail.rfind('\n')?;
        let until = already + rel_nl + 1;
        Some((text[already..until].to_string(), until))
    }

    #[test]
    fn streaming_flush_pushes_completed_lines_only() {
        let text = "first line\nsecond line\npartial";
        let (chunk, cursor) = next_flush_chunk(text, 0).expect("has a complete line");
        assert_eq!(chunk, "first line\nsecond line\n");
        assert_eq!(cursor, 23);
        assert!(next_flush_chunk(text, cursor).is_none());
    }

    #[test]
    fn streaming_flush_pushes_nothing_when_no_newline_yet() {
        let text = "still streaming partial line";
        assert!(next_flush_chunk(text, 0).is_none());
    }

    #[test]
    fn streaming_flush_advances_past_blank_lines() {
        let text = "para 1\n\npara 2\n";
        let (chunk, cursor) = next_flush_chunk(text, 0).unwrap();
        assert_eq!(chunk, "para 1\n\npara 2\n");
        assert_eq!(cursor, text.len());
        assert!(next_flush_chunk(text, cursor).is_none());
    }
}

use std::sync::Arc;
use std::time::Duration;

use crossterm::event::{self, Event, KeyCode, KeyEventKind};
use ratatui::{
    layout::{Constraint, Direction, Layout},
    style::{Color, Style},
    widgets::Paragraph,
    DefaultTerminal, Frame,
};
use shared::CliConfig;

use super::activity::ActivityState;
use super::input_bar::InputBar;
use super::scroll_area::Message;
use super::scroll_area::ScrollArea;
use super::splash::SPLASH;
use crate::conversation::engine::ConversationEngine;
use crate::conversation::system_prompt::SystemPrompt;
use crate::state::store::Store;
use crate::tools::ToolRegistry;

pub struct App {
    scroll_area: ScrollArea,
    activity: ActivityState,
    input: InputBar,
    show_splash: bool,
    _config: CliConfig,
    store: Arc<Store>,
    engine: ConversationEngine,
    _registry: Arc<ToolRegistry>,
    system_prompt: SystemPrompt,
    should_quit: bool,
    pending_prompt: Option<String>,
}

impl App {
    pub fn new(
        config: CliConfig,
        store: Arc<Store>,
        engine: ConversationEngine,
        registry: Arc<ToolRegistry>,
        system_prompt: SystemPrompt,
    ) -> Self {
        Self {
            scroll_area: ScrollArea::new(),
            activity: ActivityState::idle(),
            input: InputBar::new(),
            show_splash: true,
            _config: config,
            store,
            engine,
            _registry: registry,
            system_prompt,
            should_quit: false,
            pending_prompt: None,
        }
    }

    fn handle_event(&mut self) -> std::io::Result<()> {
        if event::poll(Duration::from_millis(16))? {
            match event::read()? {
                Event::Key(key) if key.kind == KeyEventKind::Press => match key.code {
                    KeyCode::Enter => {
                        let text = self.input.submit();
                        if !text.is_empty() {
                            self.show_splash = false;
                            // Check for slash commands via the dispatch system
                            if text.starts_with('/') {
                                let registry = crate::commands::registry::CommandRegistry::new();
                                if let Some(cmd) = registry.resolve(&text) {
                                    match crate::commands::dispatch::dispatch(cmd, &text, &self.store) {
                                        crate::commands::dispatch::CommandResult::Display(output) => {
                                            self.scroll_area.push(Message::System(output));
                                        }
                                        crate::commands::dispatch::CommandResult::Prompt(prompt_text) => {
                                            self.scroll_area.push(Message::User(text.clone()));
                                            self.scroll_area.push(Message::User(format!("(via {})", cmd.name)));
                                            self.store.set_state(|s| {
                                                s.messages.push(Message::User(prompt_text.clone()));
                                            });
                                            self.activity = ActivityState::active("Thinking");
                                            self.pending_prompt = Some(prompt_text);
                                        }
                                        crate::commands::dispatch::CommandResult::Quit => {
                                            self.should_quit = true;
                                        }
                                    }
                                } else {
                                    self.scroll_area.push(Message::System(format!("Unknown command: {text}. Type /help for available commands.")));
                                }
                                return Ok(());
                            }
                            // Push user message to both the scroll area (for display)
                            // and the store (for engine context)
                            let msg = Message::User(text.clone());
                            self.scroll_area.push(msg);
                            self.store.set_state(|s| {
                                s.messages.push(Message::User(text.clone()));
                            });
                            self.activity = ActivityState::active("Thinking");
                            self.pending_prompt = Some(text);
                        }
                    }
                    KeyCode::Char(c) => self.input.push_char(c),
                    KeyCode::Backspace => self.input.delete_prev(),
                    KeyCode::Esc => self.should_quit = true,
                    _ => {}
                },
                _ => {}
            }
        }
        Ok(())
    }

    fn process_pending(&mut self) {
        if let Some(prompt) = self.pending_prompt.take() {
            let engine = self.engine.clone();
            let sp = self.system_prompt.clone();

            // We are inside #[tokio::main] so the runtime Handle is available.
            // Use block_in_place to run async code from the sync event loop.
            let result = tokio::task::block_in_place(|| {
                tokio::runtime::Handle::current().block_on(engine.process_prompt(prompt, &sp))
            });

            match result {
                Ok(response) => {
                    self.scroll_area.push(Message::Assistant(response));
                    self.activity = ActivityState::idle();
                }
                Err(e) => {
                    let err_msg = format!("Error: {e}");
                    self.scroll_area.push(Message::Assistant(err_msg));
                    self.activity = ActivityState::idle();
                }
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

    fn render(&self, f: &mut Frame) {
        let area = f.area();
        let layout = Layout::default()
            .direction(Direction::Vertical)
            .constraints([
                Constraint::Min(3),
                Constraint::Length(1),
                Constraint::Length(1),
            ])
            .split(area);

        if self.show_splash {
            let splash = Paragraph::new(SPLASH)
                .style(Style::default().fg(Color::White))
                .block(Default::default());
            f.render_widget(splash, layout[0]);
        } else {
            self.scroll_area.render(f, layout[0]);
        }

        let activity_line = self.activity.render();
        f.render_widget(Paragraph::new(activity_line), layout[1]);

        self.input.render(f, layout[2]);
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

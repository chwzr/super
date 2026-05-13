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

#[allow(dead_code)]
pub struct App {
    scroll_area: ScrollArea,
    activity: ActivityState,
    input: InputBar,
    show_splash: bool,
    config: CliConfig,
    should_quit: bool,
}

impl App {
    pub fn new(config: CliConfig) -> Self {
        Self {
            scroll_area: ScrollArea::new(),
            activity: ActivityState::idle(),
            input: InputBar::new(),
            show_splash: true,
            config,
            should_quit: false,
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
                            self.scroll_area.push(Message::User(text.clone()));
                            // NOTE: prompt processing will be wired in later tasks
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

    pub fn run(&mut self, mut terminal: DefaultTerminal) -> std::io::Result<()> {
        while !self.should_quit {
            self.activity.tick();
            terminal.draw(|f| self.render(f))?;
            self.handle_event()?;
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

pub async fn run(config: CliConfig) {
    let terminal = ratatui::init();
    let mut app = App::new(config);
    let _ = app.run(terminal);
    ratatui::restore();
}

use std::io::{self, Stdout};
use std::time::{Duration, Instant};

use anyhow::Result;
use crossterm::event::{self, Event, KeyCode, KeyEvent, KeyModifiers};
use crossterm::execute;
use crossterm::terminal::{
    disable_raw_mode, enable_raw_mode, EnterAlternateScreen, LeaveAlternateScreen,
};
use ratatui::backend::CrosstermBackend;
use ratatui::Terminal;

use crate::adapters::{DiscoveryEngine, DiscoveryRequest};
use crate::app::{App, AppAction, Overlay};
use crate::ui;

pub fn run_dashboard(
    engine: &DiscoveryEngine,
    request: &DiscoveryRequest,
    refresh_interval: Duration,
) -> Result<()> {
    let snapshot = engine.discover(request);
    let mut app = App::new(snapshot);
    let mut terminal = TerminalSession::enter()?;
    let mut last_refresh = Instant::now();

    loop {
        terminal.terminal.draw(|frame| ui::render(frame, &app))?;
        if app.should_quit {
            break;
        }

        let until_refresh = refresh_interval.saturating_sub(last_refresh.elapsed());
        if event::poll(until_refresh.min(Duration::from_millis(100)))? {
            if let Event::Key(key) = event::read()? {
                let action = handle_key(&mut app, key);
                handle_action(&mut app, action);
            }
        }
        if last_refresh.elapsed() >= refresh_interval {
            app.replace_snapshot(engine.discover(request));
            last_refresh = Instant::now();
        }
    }

    Ok(())
}

fn handle_key(app: &mut App, key: KeyEvent) -> AppAction {
    if key.modifiers.contains(KeyModifiers::CONTROL) {
        return match key.code {
            KeyCode::Char('s') => {
                app.toggle_view();
                AppAction::None
            }
            KeyCode::Char('r') => {
                app.start_rename();
                AppAction::None
            }
            KeyCode::Char('x') => match app.overlay.clone() {
                Overlay::Confirm(_) => app.activate(),
                _ => {
                    app.start_confirm();
                    AppAction::None
                }
            },
            KeyCode::Char('j') => {
                app.push_input('\n');
                AppAction::None
            }
            _ => AppAction::None,
        };
    }

    match key.code {
        KeyCode::Esc => app.escape(),
        KeyCode::Char('?') => {
            app.toggle_help();
            AppAction::None
        }
        KeyCode::Up | KeyCode::Char('k') if app.overlay == Overlay::None => {
            app.select_previous();
            AppAction::None
        }
        KeyCode::Down | KeyCode::Char('j') if app.overlay == Overlay::None => {
            app.select_next();
            AppAction::None
        }
        KeyCode::Enter => app.activate(),
        KeyCode::Char(' ') if app.overlay == Overlay::None || app.overlay == Overlay::Peek => {
            app.toggle_peek();
            AppAction::None
        }
        KeyCode::Backspace => {
            app.pop_input();
            AppAction::None
        }
        KeyCode::Tab if app.overlay == Overlay::None => {
            app.start_new_session(None);
            AppAction::None
        }
        KeyCode::Char('/') if app.overlay == Overlay::None => {
            app.start_filter();
            AppAction::None
        }
        KeyCode::Char('q') if app.overlay == Overlay::None && app.selection.is_none() => {
            app.should_quit = true;
            AppAction::Quit
        }
        KeyCode::Char(character) if app.overlay == Overlay::None => {
            app.start_new_session(Some(character));
            AppAction::None
        }
        KeyCode::Char(character) => {
            app.push_input(character);
            AppAction::None
        }
        _ => AppAction::None,
    }
}

fn handle_action(app: &mut App, action: AppAction) {
    match action {
        AppAction::None | AppAction::Refresh | AppAction::Quit => {}
        AppAction::Launch { .. } => {
            app.set_notice("launch is waiting for a configured managed provider")
        }
        AppAction::Reply { .. } => {
            app.set_notice("reply is unavailable for this read-only provider session")
        }
        AppAction::Rename { .. } => {
            app.set_notice("rename is unavailable for this read-only provider session")
        }
        AppAction::Interrupt { .. } => {
            app.set_notice("interrupt is unavailable for this read-only provider session")
        }
        AppAction::Delete { .. } => {
            app.set_notice("delete is unavailable for this read-only provider session")
        }
    }
}

struct TerminalSession {
    terminal: Terminal<CrosstermBackend<Stdout>>,
}

impl TerminalSession {
    fn enter() -> Result<Self> {
        enable_raw_mode()?;
        let mut stdout = io::stdout();
        execute!(stdout, EnterAlternateScreen, crossterm::cursor::Hide)?;
        let terminal = Terminal::new(CrosstermBackend::new(stdout))?;
        Ok(Self { terminal })
    }
}

impl Drop for TerminalSession {
    fn drop(&mut self) {
        let _ = disable_raw_mode();
        let _ = execute!(
            self.terminal.backend_mut(),
            LeaveAlternateScreen,
            crossterm::cursor::Show
        );
        let _ = self.terminal.show_cursor();
    }
}


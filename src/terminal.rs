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
use crate::control::ControlHub;
use crate::ui;

pub fn run_dashboard(
    engine: &DiscoveryEngine,
    request: &DiscoveryRequest,
    refresh_interval: Duration,
    control: &ControlHub,
) -> Result<()> {
    let mut snapshot = engine.discover(request);
    control.enrich(&mut snapshot);
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
                let refresh = handle_action(&mut terminal, &mut app, action, control);
                if refresh {
                    let mut snapshot = engine.discover(request);
                    control.enrich(&mut snapshot);
                    app.replace_snapshot(snapshot);
                    last_refresh = Instant::now();
                }
            }
        }
        if last_refresh.elapsed() >= refresh_interval {
            let mut snapshot = engine.discover(request);
            control.enrich(&mut snapshot);
            app.replace_snapshot(snapshot);
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
        KeyCode::Char('?') if app.overlay == Overlay::None || app.overlay == Overlay::Help => {
            app.toggle_help();
            AppAction::None
        }
        KeyCode::Up if app.overlay == Overlay::None => {
            app.select_previous();
            AppAction::None
        }
        KeyCode::Down if app.overlay == Overlay::None => {
            app.select_next();
            AppAction::None
        }
        KeyCode::Enter => app.activate(),
        KeyCode::Char(' ') if app.overlay == Overlay::None || app.overlay == Overlay::Peek => {
            app.toggle_peek();
            if app.overlay == Overlay::Peek {
                app.selected_session()
                    .map(|session| AppAction::Inspect {
                        session_id: session.id.clone(),
                    })
                    .unwrap_or(AppAction::None)
            } else {
                AppAction::None
            }
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

fn handle_action(
    terminal: &mut TerminalSession,
    app: &mut App,
    action: AppAction,
    control: &ControlHub,
) -> bool {
    match action {
        AppAction::None | AppAction::Quit => false,
        AppAction::Refresh => true,
        AppAction::Inspect { session_id } => {
            let Some(session) = app
                .snapshot
                .sessions
                .iter()
                .find(|session| session.id == session_id)
                .cloned()
            else {
                app.set_notice("the selected session disappeared during refresh");
                return false;
            };
            match control.inspect(&session) {
                Ok(detail) => app.set_detail(session_id, detail),
                Err(error) => app.set_notice(format!("inspect failed: {error:#}")),
            }
            false
        }
        AppAction::Open { session_id } => {
            let Some(session) = app
                .snapshot
                .sessions
                .iter()
                .find(|session| session.id == session_id)
                .cloned()
            else {
                app.set_notice("the selected session disappeared during refresh");
                return false;
            };
            if let Err(error) = terminal.suspend() {
                app.set_notice(format!("failed to suspend dashboard: {error:#}"));
                return false;
            }
            let result = control.open(&session);
            let resume_result = terminal.resume();
            match (result, resume_result) {
                (Ok(outcome), Ok(())) => app.set_notice(outcome.message),
                (Err(error), Ok(())) => {
                    app.set_notice(format!("failed to open session: {error:#}"))
                }
                (_, Err(error)) => {
                    app.set_notice(format!("failed to restore dashboard: {error:#}"))
                }
            }
            true
        }
        AppAction::Launch { prompt } => {
            match control.launch(prompt) {
                Ok(outcome) => app.set_notice(outcome.message),
                Err(error) => app.set_notice(format!("launch failed: {error:#}")),
            }
            true
        }
        AppAction::Reply { session_id, .. } => {
            app.set_notice(format!(
                "inline reply is not supported safely for {session_id}; press enter to attach"
            ));
            false
        }
        AppAction::Rename { .. } => {
            app.set_notice("rename is unavailable through the supported provider CLI");
            false
        }
        AppAction::Interrupt { session_id } => {
            let Some(session) = app
                .snapshot
                .sessions
                .iter()
                .find(|session| session.id == session_id)
                .cloned()
            else {
                app.set_notice("the selected session disappeared during refresh");
                return false;
            };
            match control.interrupt(&session) {
                Ok(outcome) => app.set_notice(outcome.message),
                Err(error) => app.set_notice(format!("stop refused: {error:#}")),
            }
            true
        }
        AppAction::Delete { .. } => {
            app.set_notice("delete is unavailable through the supported provider CLI");
            false
        }
    }
}

struct TerminalSession {
    terminal: Terminal<CrosstermBackend<Stdout>>,
    active: bool,
}

impl TerminalSession {
    fn enter() -> Result<Self> {
        enable_raw_mode()?;
        let mut stdout = io::stdout();
        execute!(stdout, EnterAlternateScreen, crossterm::cursor::Hide)?;
        let terminal = Terminal::new(CrosstermBackend::new(stdout))?;
        Ok(Self {
            terminal,
            active: true,
        })
    }

    fn suspend(&mut self) -> Result<()> {
        if !self.active {
            return Ok(());
        }
        disable_raw_mode()?;
        execute!(
            self.terminal.backend_mut(),
            LeaveAlternateScreen,
            crossterm::cursor::Show
        )?;
        self.active = false;
        Ok(())
    }

    fn resume(&mut self) -> Result<()> {
        if self.active {
            return Ok(());
        }
        enable_raw_mode()?;
        execute!(
            self.terminal.backend_mut(),
            EnterAlternateScreen,
            crossterm::cursor::Hide
        )?;
        self.terminal.clear()?;
        self.active = true;
        Ok(())
    }
}

impl Drop for TerminalSession {
    fn drop(&mut self) {
        if !self.active {
            return;
        }
        let _ = disable_raw_mode();
        let _ = execute!(
            self.terminal.backend_mut(),
            LeaveAlternateScreen,
            crossterm::cursor::Show
        );
        let _ = self.terminal.show_cursor();
    }
}

#[cfg(test)]
mod tests {
    use std::collections::BTreeSet;
    use std::path::PathBuf;

    use crossterm::event::KeyEventKind;

    use crate::app::{ComposerMode, SelectionKey};
    use crate::domain::{
        AgentSession, Capability, Provider, Runtime, SessionKind, SessionSnapshot, SessionState,
    };

    use super::*;

    fn app() -> App {
        App::new(SessionSnapshot {
            sessions: vec![AgentSession {
                id: "worker".into(),
                provider_session_id: "worker".into(),
                provider: Provider::Claude,
                runtime: Runtime::Host,
                kind: SessionKind::Background,
                name: "worker".into(),
                cwd: PathBuf::from("/work"),
                state: SessionState::Working,
                summary: "working".into(),
                raw_state: None,
                pid: None,
                started_at: None,
                updated_at: None,
                pull_requests: None,
                capabilities: BTreeSet::from([Capability::Inspect]),
            }],
            warnings: vec![],
        })
    }

    fn key(code: KeyCode) -> KeyEvent {
        KeyEvent {
            code,
            modifiers: KeyModifiers::NONE,
            kind: KeyEventKind::Press,
            state: crossterm::event::KeyEventState::NONE,
        }
    }

    #[test]
    fn printable_j_starts_a_task_instead_of_moving_the_list() {
        let mut app = app();
        assert_eq!(
            app.selection,
            Some(SelectionKey::Session("worker".into()))
        );

        assert_eq!(
            handle_key(&mut app, key(KeyCode::Char('j'))),
            AppAction::None
        );
        assert_eq!(app.overlay, Overlay::Composer(ComposerMode::NewSession));
        assert_eq!(app.input, "j");
    }

    #[test]
    fn question_mark_is_text_inside_the_composer() {
        let mut app = app();
        app.start_new_session(Some('w'));

        assert_eq!(
            handle_key(&mut app, key(KeyCode::Char('?'))),
            AppAction::None
        );
        assert_eq!(app.overlay, Overlay::Composer(ComposerMode::NewSession));
        assert_eq!(app.input, "w?");
    }

    #[test]
    fn question_mark_toggles_help_from_the_list() {
        let mut app = app();

        handle_key(&mut app, key(KeyCode::Char('?')));
        assert_eq!(app.overlay, Overlay::Help);
        handle_key(&mut app, key(KeyCode::Char('?')));
        assert_eq!(app.overlay, Overlay::None);
    }
}

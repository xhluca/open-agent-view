use std::io::{self, Stdout};
use std::sync::mpsc::{self, SyncSender, TryRecvError, TrySendError};
use std::thread;
use std::time::{Duration, Instant};

use anyhow::{anyhow, Context, Result};
use crossterm::event::{self, Event, KeyCode, KeyEvent, KeyEventKind, KeyModifiers};
use crossterm::execute;
use crossterm::terminal::{
    disable_raw_mode, enable_raw_mode, EnterAlternateScreen, LeaveAlternateScreen,
};
use ratatui::backend::CrosstermBackend;
use ratatui::Terminal;

use crate::adapters::{DiscoveryEngine, DiscoveryRequest};
use crate::app::{App, AppAction, Overlay};
use crate::control::{ControlHub, ControlOutcome};
use crate::domain::{AgentSession, Capability, Provider, SessionSnapshot};
use crate::hidden::HiddenSessions;
use crate::ui;

// Apply a burst of already-buffered terminal input before drawing. Holding an
// arrow key can otherwise enqueue hundreds of repeat events, each of which
// would repaint a frame that the user will never see and can saturate a remote
// terminal or tmux pane.
const MAX_READY_EVENTS_PER_TICK: usize = 256;
const LAUNCH_DISCOVERY_RETRY_INTERVAL: Duration = Duration::from_millis(250);
const LAUNCH_DISCOVERY_TIMEOUT: Duration = Duration::from_secs(5);

#[derive(Clone, Debug, Eq, PartialEq)]
struct PendingLaunch {
    provider: Provider,
    provider_session_id: String,
    deadline: Instant,
}

#[derive(Debug, Default)]
struct ActionEffect {
    refresh: bool,
    pending_launch: Option<(Provider, String)>,
    completed_visibility: Option<bool>,
    load_models: Option<Provider>,
    hide_session_ids: Vec<String>,
}

pub fn run_dashboard(
    engine: &DiscoveryEngine,
    request: &DiscoveryRequest,
    refresh_interval: Duration,
    control: &ControlHub,
    hidden_sessions: HiddenSessions,
) -> Result<()> {
    let (refresh_tx, refresh_rx) = mpsc::sync_channel(1);
    let (snapshot_tx, snapshot_rx) = mpsc::channel::<(SessionSnapshot, bool)>();
    let (models_tx, models_rx) = mpsc::channel::<(Provider, Result<Vec<String>, String>)>();
    let worker_engine = (*engine).clone();
    let worker_control = control.clone();
    let worker_hidden_sessions = hidden_sessions.clone();
    let _refresh_worker = thread::spawn(move || {
        let mut first_refresh = true;
        while let Ok(worker_request) = refresh_rx.recv() {
            let partial_sender = snapshot_tx.clone();
            let mut snapshot = if first_refresh {
                worker_engine.discover_progressively(
                    &worker_request,
                    |partial, completed, total| {
                        let mut partial = partial.clone();
                        worker_hidden_sessions.filter_snapshot(&mut partial);
                        partial.warnings.push(format!(
                            "loading remaining providers… ({completed}/{total})"
                        ));
                        let _ = partial_sender.send((partial, false));
                    },
                )
            } else {
                worker_engine.discover(&worker_request)
            };
            worker_control.enrich(&mut snapshot);
            worker_hidden_sessions.filter_snapshot(&mut snapshot);
            if snapshot_tx.send((snapshot, true)).is_err() {
                break;
            }
            first_refresh = false;
        }
    });

    let mut initial = SessionSnapshot::default();
    initial.warnings.push("loading provider sessions…".into());
    let mut app = App::with_launch_targets(
        initial,
        request.include_completed,
        control.default_launch_provider(),
        control.launch_targets(),
    );
    let mut terminal = TerminalSession::enter()?;
    let mut last_refresh = Instant::now();
    let mut current_request = request.clone();
    let mut refresh_in_flight = false;
    let mut refresh_after_current = false;
    let mut pending_launch: Option<PendingLaunch> = None;
    let mut pending_launch_retry_at: Option<Instant> = None;
    let mut needs_draw = true;
    schedule_refresh(
        &refresh_tx,
        &current_request,
        &mut refresh_in_flight,
    )?;

    let result = 'dashboard: loop {
        loop {
            match snapshot_rx.try_recv() {
                Ok((snapshot, complete)) => {
                    let changed = snapshot != app.snapshot;
                    app.replace_snapshot(snapshot);
                    if select_pending_launch(&mut app, pending_launch.as_ref()) {
                        pending_launch = None;
                        pending_launch_retry_at = None;
                    }
                    if complete {
                        refresh_in_flight = false;
                        last_refresh = Instant::now();
                        if let Some(pending) = pending_launch.as_ref() {
                            if Instant::now() < pending.deadline {
                                pending_launch_retry_at = Some(
                                    Instant::now() + LAUNCH_DISCOVERY_RETRY_INTERVAL,
                                );
                            } else {
                                app.set_notice(
                                    "task launched, but its provider record is not visible yet; press ctrl+l to retry",
                                );
                                pending_launch = None;
                                pending_launch_retry_at = None;
                            }
                        }
                    }
                    needs_draw |= changed;
                }
                Err(TryRecvError::Empty) => break,
                Err(TryRecvError::Disconnected) => {
                    break 'dashboard Err(anyhow!("provider refresh worker stopped unexpectedly"));
                }
            }
        }
        loop {
            match models_rx.try_recv() {
                Ok((provider, result)) => {
                    app.set_available_models(provider, result);
                    needs_draw = true;
                }
                Err(TryRecvError::Empty) => break,
                Err(TryRecvError::Disconnected) => break,
            }
        }
        if refresh_after_current && !refresh_in_flight {
            schedule_refresh(
                &refresh_tx,
                &current_request,
                &mut refresh_in_flight,
            )?;
            refresh_after_current = false;
            last_refresh = Instant::now();
        }

        if app.should_quit {
            break Ok(());
        }
        if needs_draw {
            terminal.terminal.draw(|frame| ui::render(frame, &app))?;
            needs_draw = false;
        }

        let until_refresh = if refresh_in_flight {
            Duration::from_millis(50)
        } else {
            refresh_interval.saturating_sub(last_refresh.elapsed())
        };
        if event::poll(until_refresh.min(Duration::from_millis(50)))? {
            for event_index in 0..MAX_READY_EVENTS_PER_TICK {
                match event::read()? {
                    Event::Key(key) => {
                        let action = handle_key(&mut app, key);
                        let effect = dispatch_action(&mut terminal, &mut app, action, control);
                        if let Some(include_completed) = effect.completed_visibility {
                            current_request.include_completed = include_completed;
                            app.set_completed_visibility(include_completed);
                            if !include_completed {
                                let mut snapshot = app.snapshot.clone();
                                snapshot.sessions.retain(|session| {
                                    session.state != crate::domain::SessionState::Completed
                                });
                                app.replace_snapshot(snapshot);
                            }
                        }
                        if let Some(provider) = effect.load_models {
                            schedule_model_load(control.clone(), provider, models_tx.clone());
                        }
                        if let Some((provider, provider_session_id)) = effect.pending_launch {
                            pending_launch = Some(PendingLaunch {
                                provider,
                                provider_session_id,
                                deadline: Instant::now() + LAUNCH_DISCOVERY_TIMEOUT,
                            });
                            pending_launch_retry_at = None;
                        }
                        if !effect.hide_session_ids.is_empty() {
                            match hide_sessions_from_app(
                                &mut app,
                                &hidden_sessions,
                                &effect.hide_session_ids,
                            ) {
                                Ok(count) => app.set_notice(format!(
                                    "hid {count} session{} locally; provider history was retained",
                                    if count == 1 { "" } else { "s" }
                                )),
                                Err(error) => {
                                    app.set_notice(format!("failed to hide session: {error:#}"))
                                }
                            }
                        }
                        needs_draw = true;
                        if effect.refresh {
                            if refresh_in_flight {
                                refresh_after_current = true;
                            } else {
                                schedule_refresh(
                                    &refresh_tx,
                                    &current_request,
                                    &mut refresh_in_flight,
                                )?;
                                last_refresh = Instant::now();
                            }
                        }
                    }
                    Event::Resize(_, _) => needs_draw = true,
                    _ => {}
                }
                if app.should_quit
                    || event_index + 1 == MAX_READY_EVENTS_PER_TICK
                    || !event::poll(Duration::ZERO)?
                {
                    break;
                }
            }
        }
        if !refresh_in_flight
            && pending_launch_retry_at.is_some_and(|retry_at| Instant::now() >= retry_at)
        {
            schedule_refresh(
                &refresh_tx,
                &current_request,
                &mut refresh_in_flight,
            )?;
            pending_launch_retry_at = None;
            last_refresh = Instant::now();
        }
        if !refresh_in_flight && last_refresh.elapsed() >= refresh_interval {
            schedule_refresh(
                &refresh_tx,
                &current_request,
                &mut refresh_in_flight,
            )?;
            last_refresh = Instant::now();
        }
    };

    engine.cancel();
    drop(refresh_tx);
    result
}

fn schedule_refresh(
    sender: &SyncSender<DiscoveryRequest>,
    request: &DiscoveryRequest,
    refresh_in_flight: &mut bool,
) -> Result<()> {
    match sender.try_send(request.clone()) {
        Ok(()) | Err(TrySendError::Full(_)) => {
            *refresh_in_flight = true;
            Ok(())
        }
        Err(TrySendError::Disconnected(_)) => {
            Err(anyhow!("provider refresh worker stopped unexpectedly"))
        }
    }
}

fn schedule_model_load(
    control: ControlHub,
    provider: Provider,
    sender: mpsc::Sender<(Provider, Result<Vec<String>, String>)>,
) {
    let _model_worker = thread::spawn(move || {
        let result = control
            .available_models(&provider)
            .map_err(|error| format!("{error:#}"));
        let _ = sender.send((provider, result));
    });
}

fn select_pending_launch(app: &mut App, pending: Option<&PendingLaunch>) -> bool {
    let Some(pending) = pending else {
        return false;
    };
    let Some(session) = app.snapshot.sessions.iter().find(|session| {
        session.provider == pending.provider
            && session.provider_session_id == pending.provider_session_id
    }) else {
        return false;
    };
    app.selection = Some(crate::app::SelectionKey::Session(session.id.clone()));
    true
}

fn hide_sessions_from_app(
    app: &mut App,
    hidden_sessions: &HiddenSessions,
    session_ids: &[String],
) -> Result<usize> {
    let sessions = session_ids
        .iter()
        .map(|id| {
            app.snapshot
                .sessions
                .iter()
                .find(|session| &session.id == id)
                .cloned()
        })
        .collect::<Option<Vec<_>>>()
        .context("a selected session disappeared during refresh")?;
    let count = hidden_sessions.hide_sessions(&sessions)?;
    let mut snapshot = app.snapshot.clone();
    hidden_sessions.filter_snapshot(&mut snapshot);
    app.replace_snapshot(snapshot);
    Ok(count)
}

fn handle_key(app: &mut App, key: KeyEvent) -> AppAction {
    if key.kind == KeyEventKind::Release {
        return AppAction::None;
    }
    if key.modifiers.contains(KeyModifiers::CONTROL) {
        return match key.code {
            KeyCode::Char('s') if app.overlay == Overlay::None => {
                app.toggle_view();
                AppAction::None
            }
            KeyCode::Char('r') if app.overlay == Overlay::None => {
                app.start_rename();
                AppAction::None
            }
            KeyCode::Char('f') if app.overlay == Overlay::None => {
                app.start_filter();
                AppAction::None
            }
            KeyCode::Char('l') if app.overlay == Overlay::None => AppAction::Refresh,
            KeyCode::Char('x') => match app.overlay.clone() {
                Overlay::Confirm(_) => app.activate(),
                Overlay::None | Overlay::Peek => {
                    app.start_confirm();
                    AppAction::None
                }
                _ => AppAction::None,
            },
            KeyCode::Char('a') if app.overlay == Overlay::None => {
                app.start_archive_confirm();
                AppAction::None
            }
            KeyCode::Char('j') => {
                app.push_input('\n');
                AppAction::None
            }
            _ => AppAction::None,
        };
    }

    match key.code {
        KeyCode::Esc => app.escape(),
        KeyCode::Char('y')
            if app.overlay == Overlay::Peek
                && app
                    .selected_session()
                    .is_some_and(|session| session.capabilities.contains(&Capability::Approve)) =>
        {
            app.resolve_approval(true)
        }
        KeyCode::Char('n')
            if app.overlay == Overlay::Peek
                && app
                    .selected_session()
                    .is_some_and(|session| session.capabilities.contains(&Capability::Decline)) =>
        {
            app.resolve_approval(false)
        }
        KeyCode::Char('?') if app.overlay == Overlay::None || app.overlay == Overlay::Help => {
            app.toggle_help();
            AppAction::None
        }
        KeyCode::Up if app.overlay == Overlay::ModelPicker => {
            app.move_model_selection(-1);
            AppAction::None
        }
        KeyCode::Down if app.overlay == Overlay::ModelPicker => {
            app.move_model_selection(1);
            AppAction::None
        }
        KeyCode::PageUp if app.overlay == Overlay::ModelPicker => {
            app.move_model_page(-1);
            AppAction::None
        }
        KeyCode::PageDown if app.overlay == Overlay::ModelPicker => {
            app.move_model_page(1);
            AppAction::None
        }
        KeyCode::Up | KeyCode::Left if app.overlay == Overlay::HarnessPicker => {
            app.move_harness_selection(-1);
            AppAction::None
        }
        KeyCode::Down | KeyCode::Right if app.overlay == Overlay::HarnessPicker => {
            app.move_harness_selection(1);
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
        KeyCode::Backspace if app.overlay != Overlay::HarnessPicker => {
            app.pop_input();
            AppAction::None
        }
        KeyCode::Tab if app.overlay == Overlay::None => {
            app.start_new_session(None);
            AppAction::None
        }
        KeyCode::Tab if app.overlay == Overlay::Composer(crate::app::ComposerMode::NewSession) => {
            app.open_harness_picker();
            AppAction::None
        }
        KeyCode::Tab if app.overlay == Overlay::HarnessPicker => {
            app.move_harness_selection(1);
            AppAction::None
        }
        KeyCode::Tab if app.overlay == Overlay::ModelPicker => {
            app.move_model_selection(1);
            AppAction::None
        }
        KeyCode::BackTab if app.overlay == Overlay::HarnessPicker => {
            app.move_harness_selection(-1);
            AppAction::None
        }
        KeyCode::BackTab if app.overlay == Overlay::ModelPicker => {
            app.move_model_selection(-1);
            AppAction::None
        }
        KeyCode::Char(digit) if app.overlay == Overlay::HarnessPicker && digit.is_ascii_digit() => {
            if let Some(number) = digit.to_digit(10) {
                app.choose_harness_number(number as usize);
            }
            AppAction::None
        }
        KeyCode::Char('/') if app.overlay == Overlay::None => {
            app.start_new_session(Some('/'));
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

trait DashboardTerminal {
    fn suspend_dashboard(&mut self) -> Result<()>;
    fn resume_dashboard(&mut self) -> Result<()>;
}

trait DashboardControl {
    fn inspect_session(&self, session: &AgentSession) -> Result<String>;
    fn open_session(&self, session: &AgentSession) -> Result<ControlOutcome>;
    fn launch_session(
        &self,
        provider: Provider,
        model: Option<String>,
        prompt: String,
    ) -> Result<ControlOutcome>;
    fn reply_session(&self, session: &AgentSession, prompt: &str) -> Result<ControlOutcome>;
    fn resolve_session_approval(
        &self,
        session: &AgentSession,
        accept: bool,
    ) -> Result<ControlOutcome>;
    fn respond_session_input(&self, session: &AgentSession, answer: &str)
        -> Result<ControlOutcome>;
    fn interrupt_session(&self, session: &AgentSession) -> Result<ControlOutcome>;
    fn archive_session(&self, session: &AgentSession) -> Result<ControlOutcome>;
    fn delete_session(&self, session: &AgentSession) -> Result<ControlOutcome>;
}

impl DashboardControl for ControlHub {
    fn inspect_session(&self, session: &AgentSession) -> Result<String> {
        self.inspect(session)
    }

    fn open_session(&self, session: &AgentSession) -> Result<ControlOutcome> {
        self.open(session)
    }

    fn launch_session(
        &self,
        provider: Provider,
        model: Option<String>,
        prompt: String,
    ) -> Result<ControlOutcome> {
        self.launch_with(provider, model, prompt)
    }

    fn reply_session(&self, session: &AgentSession, prompt: &str) -> Result<ControlOutcome> {
        self.reply(session, prompt)
    }

    fn resolve_session_approval(
        &self,
        session: &AgentSession,
        accept: bool,
    ) -> Result<ControlOutcome> {
        self.resolve_approval(session, accept)
    }

    fn respond_session_input(
        &self,
        session: &AgentSession,
        answer: &str,
    ) -> Result<ControlOutcome> {
        self.respond_input(session, answer)
    }

    fn interrupt_session(&self, session: &AgentSession) -> Result<ControlOutcome> {
        self.interrupt(session)
    }

    fn archive_session(&self, session: &AgentSession) -> Result<ControlOutcome> {
        self.archive(session)
    }

    fn delete_session(&self, session: &AgentSession) -> Result<ControlOutcome> {
        self.delete(session)
    }
}

fn dispatch_action<T: DashboardTerminal, C: DashboardControl>(
    terminal: &mut T,
    app: &mut App,
    action: AppAction,
    control: &C,
) -> ActionEffect {
    match action {
        AppAction::SetCompletedVisibility { include_completed } => ActionEffect {
            refresh: true,
            completed_visibility: Some(include_completed),
            ..ActionEffect::default()
        },
        AppAction::LoadModels { provider } => ActionEffect {
            load_models: Some(provider),
            ..ActionEffect::default()
        },
        AppAction::Hide { session_ids } => ActionEffect {
            hide_session_ids: session_ids,
            ..ActionEffect::default()
        },
        AppAction::Launch {
            provider,
            model,
            prompt,
        } => {
            let result = control.launch_session(provider.clone(), model, prompt);
            match result {
                Ok(outcome) => {
                    app.set_notice(outcome.message);
                    ActionEffect {
                        refresh: true,
                        pending_launch: outcome
                            .provider_session_hint
                            .map(|provider_session_id| (provider, provider_session_id)),
                        ..ActionEffect::default()
                    }
                }
                Err(error) => {
                    app.set_notice(format!("launch failed: {error:#}"));
                    ActionEffect {
                        refresh: true,
                        ..ActionEffect::default()
                    }
                }
            }
        }
        other => ActionEffect {
            refresh: handle_action_legacy(terminal, app, other, control),
            ..ActionEffect::default()
        },
    }
}

#[cfg(test)]
fn handle_action<T: DashboardTerminal, C: DashboardControl>(
    terminal: &mut T,
    app: &mut App,
    action: AppAction,
    control: &C,
) -> bool {
    dispatch_action(terminal, app, action, control).refresh
}

fn handle_action_legacy<T: DashboardTerminal, C: DashboardControl>(
    terminal: &mut T,
    app: &mut App,
    action: AppAction,
    control: &C,
) -> bool {
    match action {
        AppAction::None
        | AppAction::Quit
        | AppAction::SetCompletedVisibility { .. }
        | AppAction::LoadModels { .. }
        | AppAction::Hide { .. } => false,
        AppAction::Refresh => {
            app.set_notice("refreshing provider sessions…");
            true
        }
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
            match control.inspect_session(&session) {
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
            if let Err(error) = terminal.suspend_dashboard() {
                app.set_notice(format!("failed to suspend dashboard: {error:#}"));
                return false;
            }
            let result = control.open_session(&session);
            let resume_result = terminal.resume_dashboard();
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
        AppAction::Launch { .. } => unreachable!("launch actions are dispatched with effects"),
        AppAction::Reply { session_id, prompt } => {
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
            match control.reply_session(&session, &prompt) {
                Ok(outcome) => app.set_notice(outcome.message),
                Err(error) => app.set_notice(format!("reply refused: {error:#}")),
            }
            true
        }
        AppAction::ResolveApproval { session_id, accept } => {
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
            match control.resolve_session_approval(&session, accept) {
                Ok(outcome) => {
                    app.set_notice(outcome.message);
                    if let Ok(detail) = control.inspect_session(&session) {
                        app.set_detail(session_id, detail);
                    }
                }
                Err(error) => app.set_notice(format!("approval response refused: {error:#}")),
            }
            true
        }
        AppAction::RespondInput { session_id, answer } => {
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
            match control.respond_session_input(&session, &answer) {
                Ok(outcome) => {
                    app.set_notice(outcome.message);
                    if let Ok(detail) = control.inspect_session(&session) {
                        app.set_detail(session_id, detail);
                    }
                }
                Err(error) => app.set_notice(format!("input response refused: {error:#}")),
            }
            true
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
            match control.interrupt_session(&session) {
                Ok(outcome) => app.set_notice(outcome.message),
                Err(error) => app.set_notice(format!("stop refused: {error:#}")),
            }
            true
        }
        AppAction::Archive { session_id } => {
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
            match control.archive_session(&session) {
                Ok(outcome) => app.set_notice(outcome.message),
                Err(error) => app.set_notice(format!("archive refused: {error:#}")),
            }
            true
        }
        AppAction::Delete { session_ids } => {
            let sessions = session_ids
                .iter()
                .map(|id| {
                    app.snapshot
                        .sessions
                        .iter()
                        .find(|session| &session.id == id)
                        .cloned()
                })
                .collect::<Option<Vec<_>>>();
            let Some(sessions) = sessions else {
                app.set_notice("a selected session disappeared during refresh");
                return false;
            };
            let count = sessions.len();
            for session in sessions {
                if let Err(error) = control.delete_session(&session) {
                    app.set_notice(format!("delete refused for {}: {error:#}", session.name));
                    return true;
                }
            }
            app.set_notice(format!("deleted {count} managed Codex session(s)"));
            true
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

impl DashboardTerminal for TerminalSession {
    fn suspend_dashboard(&mut self) -> Result<()> {
        self.suspend()
    }

    fn resume_dashboard(&mut self) -> Result<()> {
        self.resume()
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
    use std::sync::Mutex;

    use crossterm::event::KeyEventKind;

    use crate::app::{ComposerMode, SelectionKey};
    use crate::domain::{
        AgentSession, Capability, LaunchTarget, Provider, Runtime, SessionKind, SessionSnapshot,
        SessionState,
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

    fn control_key(character: char) -> KeyEvent {
        KeyEvent {
            code: KeyCode::Char(character),
            modifiers: KeyModifiers::CONTROL,
            kind: KeyEventKind::Press,
            state: crossterm::event::KeyEventState::NONE,
        }
    }

    #[test]
    fn printable_j_starts_a_task_instead_of_moving_the_list() {
        let mut app = app();
        assert_eq!(app.selection, Some(SelectionKey::Session("worker".into())));

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

    #[test]
    fn control_a_opens_archive_confirmation_only_with_authority() {
        let mut app = app();
        app.snapshot.sessions[0].state = SessionState::Completed;
        app.snapshot.sessions[0]
            .capabilities
            .insert(Capability::Archive);

        assert_eq!(handle_key(&mut app, control_key('a')), AppAction::None);
        assert_eq!(
            app.overlay,
            Overlay::Confirm(crate::app::ConfirmTarget::Archive {
                id: "worker".into()
            })
        );
    }

    #[test]
    fn denial_key_is_active_only_for_an_exact_pending_request() {
        let mut app = app();
        app.snapshot.sessions[0].state = SessionState::NeedsInput;
        app.snapshot.sessions[0]
            .capabilities
            .insert(Capability::Decline);
        app.toggle_peek();

        assert_eq!(
            handle_key(&mut app, key(KeyCode::Char('n'))),
            AppAction::ResolveApproval {
                session_id: "worker".into(),
                accept: false,
            }
        );
        assert_eq!(
            handle_key(&mut app, key(KeyCode::Char('y'))),
            AppAction::None
        );
    }

    #[test]
    fn key_release_events_are_ignored() {
        let mut app = app();
        let mut released = key(KeyCode::Esc);
        released.kind = KeyEventKind::Release;

        assert_eq!(handle_key(&mut app, released), AppAction::None);
        assert!(!app.should_quit);
    }

    #[test]
    fn navigation_keys_wrap_only_from_the_list() {
        let mut app = app();
        let initial = app.selection.clone();
        handle_key(&mut app, key(KeyCode::Up));
        assert_ne!(app.selection, initial);
        let mut repeated_down = key(KeyCode::Down);
        repeated_down.kind = KeyEventKind::Repeat;
        handle_key(&mut app, repeated_down);
        assert_eq!(app.selection, initial);

        app.start_new_session(None);
        let selected = app.selection.clone();
        handle_key(&mut app, key(KeyCode::Up));
        handle_key(&mut app, key(KeyCode::Down));
        assert_eq!(app.selection, selected);
    }

    #[test]
    fn control_s_toggles_views_only_from_the_list() {
        let mut app = app();
        handle_key(&mut app, control_key('s'));
        assert_eq!(app.view_mode, crate::app::ViewMode::Directory);
        app.start_new_session(None);
        handle_key(&mut app, control_key('s'));
        assert_eq!(app.view_mode, crate::app::ViewMode::Directory);
    }

    #[test]
    fn control_l_requests_refresh_only_from_the_list() {
        let mut app = app();
        assert_eq!(handle_key(&mut app, control_key('l')), AppAction::Refresh);
        app.start_new_session(None);
        assert_eq!(handle_key(&mut app, control_key('l')), AppAction::None);
    }

    #[test]
    fn control_r_starts_rename_only_from_the_list() {
        let mut app = app();
        handle_key(&mut app, control_key('r'));
        assert_eq!(
            app.overlay,
            Overlay::Composer(ComposerMode::Rename {
                session_id: "worker".into()
            })
        );
        assert_eq!(app.input, "worker");

        app.overlay = Overlay::Help;
        app.input.clear();
        handle_key(&mut app, control_key('r'));
        assert_eq!(app.overlay, Overlay::Help);
        assert!(app.input.is_empty());
    }

    #[test]
    fn control_x_is_a_two_step_exact_confirmation_and_is_inert_in_other_overlays() {
        let mut app = app();
        app.snapshot.sessions[0]
            .capabilities
            .insert(Capability::Interrupt);

        assert_eq!(handle_key(&mut app, control_key('x')), AppAction::None);
        assert!(matches!(app.overlay, Overlay::Confirm(_)));
        assert_eq!(
            handle_key(&mut app, control_key('x')),
            AppAction::Interrupt {
                session_id: "worker".into()
            }
        );
        assert_eq!(app.overlay, Overlay::None);

        app.start_new_session(None);
        handle_key(&mut app, control_key('x'));
        assert_eq!(app.overlay, Overlay::Composer(ComposerMode::NewSession));
    }

    #[test]
    fn control_x_starts_the_same_exact_stop_confirmation_from_peek() {
        let mut app = app();
        app.snapshot.sessions[0]
            .capabilities
            .insert(Capability::Interrupt);
        app.toggle_peek();

        assert_eq!(handle_key(&mut app, control_key('x')), AppAction::None);
        assert!(matches!(
            app.overlay,
            Overlay::Confirm(crate::app::ConfirmTarget::Session {
                ref id,
                running: true
            }) if id == "worker"
        ));
        assert_eq!(
            handle_key(&mut app, control_key('x')),
            AppAction::Interrupt {
                session_id: "worker".into()
            }
        );
    }

    #[test]
    fn control_a_refuses_active_and_unowned_completed_sessions() {
        let mut app = app();
        handle_key(&mut app, control_key('a'));
        assert!(app.notice.as_deref().unwrap().contains("must be idle"));

        app.snapshot.sessions[0].state = SessionState::Completed;
        app.notice = None;
        handle_key(&mut app, control_key('a'));
        assert!(app.notice.as_deref().unwrap().contains("Archive authority"));
        assert_eq!(app.overlay, Overlay::None);
    }

    #[test]
    fn control_j_inserts_newline_only_into_a_writable_input() {
        let mut app = app();
        handle_key(&mut app, control_key('j'));
        assert!(app.input.is_empty());
        app.start_new_session(Some('a'));
        handle_key(&mut app, control_key('j'));
        handle_key(&mut app, key(KeyCode::Char('b')));
        assert_eq!(app.input, "a\nb");
    }

    #[test]
    fn escape_closes_overlay_then_quits_the_dashboard() {
        let mut app = app();
        app.start_new_session(Some('a'));
        assert_eq!(handle_key(&mut app, key(KeyCode::Esc)), AppAction::None);
        assert_eq!(app.overlay, Overlay::None);
        assert_eq!(handle_key(&mut app, key(KeyCode::Esc)), AppAction::Quit);
        assert!(app.should_quit);
    }

    #[test]
    fn approval_keys_preempt_text_only_for_their_exact_capabilities() {
        let mut app = app();
        app.snapshot.sessions[0].state = SessionState::NeedsInput;
        app.snapshot.sessions[0]
            .capabilities
            .extend([Capability::Approve, Capability::Decline]);
        app.toggle_peek();

        assert_eq!(
            handle_key(&mut app, key(KeyCode::Char('y'))),
            AppAction::ResolveApproval {
                session_id: "worker".into(),
                accept: true
            }
        );
        assert_eq!(
            handle_key(&mut app, key(KeyCode::Char('n'))),
            AppAction::ResolveApproval {
                session_id: "worker".into(),
                accept: false
            }
        );
        assert!(app.input.is_empty());
    }

    #[test]
    fn enter_activates_composer_group_and_session_paths() {
        let mut app = app();
        assert_eq!(handle_key(&mut app, key(KeyCode::Enter)), AppAction::None);
        assert_eq!(
            app.overlay,
            Overlay::Confirm(crate::app::ConfirmTarget::OpenClaude {
                id: "worker".into()
            })
        );
        assert_eq!(
            handle_key(&mut app, key(KeyCode::Enter)),
            AppAction::Open {
                session_id: "worker".into()
            }
        );

        app.selection = Some(SelectionKey::Group("state:Working".into()));
        assert_eq!(handle_key(&mut app, key(KeyCode::Enter)), AppAction::None);
        assert!(app.collapsed.contains("state:Working"));

        app.selection = Some(SelectionKey::Session("worker".into()));
        app.start_new_session(None);
        app.input = "ship".into();
        assert_eq!(
            handle_key(&mut app, key(KeyCode::Enter)),
            AppAction::Launch {
                provider: Provider::Claude,
                model: None,
                prompt: "ship".into()
            }
        );
    }

    #[test]
    fn space_opens_and_closes_peek_with_one_inspection_action() {
        let mut app = app();
        assert_eq!(
            handle_key(&mut app, key(KeyCode::Char(' '))),
            AppAction::Inspect {
                session_id: "worker".into()
            }
        );
        assert_eq!(app.overlay, Overlay::Peek);
        assert_eq!(
            handle_key(&mut app, key(KeyCode::Char(' '))),
            AppAction::None
        );
        assert_eq!(app.overlay, Overlay::None);

        app.snapshot.sessions[0].capabilities.clear();
        assert_eq!(
            handle_key(&mut app, key(KeyCode::Char(' '))),
            AppAction::None
        );
        assert_eq!(app.overlay, Overlay::None);
        assert!(app
            .notice
            .as_deref()
            .unwrap()
            .contains("inspection was not granted"));
    }

    #[test]
    fn tab_slash_backspace_and_q_follow_overlay_context() {
        let mut empty = App::new(SessionSnapshot::default());
        assert_eq!(
            handle_key(&mut empty, key(KeyCode::Char('q'))),
            AppAction::Quit
        );

        let mut app = app();
        handle_key(&mut app, key(KeyCode::Tab));
        assert_eq!(app.overlay, Overlay::Composer(ComposerMode::NewSession));
        handle_key(&mut app, key(KeyCode::Char('/')));
        assert_eq!(app.input, "/");
        handle_key(&mut app, key(KeyCode::Backspace));
        assert!(app.input.is_empty());
        handle_key(&mut app, key(KeyCode::Tab));
        assert_eq!(app.overlay, Overlay::HarnessPicker);
        handle_key(&mut app, key(KeyCode::Backspace));
        assert!(app.input.is_empty());
        handle_key(&mut app, key(KeyCode::Esc));
        assert_eq!(app.overlay, Overlay::Composer(ComposerMode::NewSession));

        app.escape();
        handle_key(&mut app, key(KeyCode::Char('/')));
        assert_eq!(app.overlay, Overlay::Composer(ComposerMode::NewSession));
        assert_eq!(app.input, "/");
        app.escape();
        handle_key(&mut app, control_key('f'));
        assert_eq!(app.overlay, Overlay::Composer(ComposerMode::Filter));
        app.escape();
        handle_key(&mut app, key(KeyCode::Char('q')));
        assert_eq!(app.overlay, Overlay::Composer(ComposerMode::NewSession));
        assert_eq!(app.input, "q");
    }

    #[test]
    fn harness_picker_keys_preview_cancel_and_select_without_editing_the_draft() {
        let mut app = App::with_launch_targets(
            SessionSnapshot::default(),
            false,
            Provider::Claude,
            vec![
                LaunchTarget {
                    provider: Provider::Claude,
                    supports_model: true,
                },
                LaunchTarget {
                    provider: Provider::Codex,
                    supports_model: true,
                },
                LaunchTarget {
                    provider: Provider::Pi,
                    supports_model: false,
                },
            ],
        );
        app.start_new_session(Some('x'));
        handle_key(&mut app, key(KeyCode::Tab));
        assert_eq!(app.overlay, Overlay::HarnessPicker);
        handle_key(&mut app, key(KeyCode::Right));
        assert_eq!(app.harness_selection, 1);
        handle_key(&mut app, key(KeyCode::BackTab));
        assert_eq!(app.harness_selection, 0);
        handle_key(&mut app, key(KeyCode::Char('z')));
        assert_eq!(app.input, "x");
        handle_key(&mut app, key(KeyCode::Esc));
        assert_eq!(app.overlay, Overlay::Composer(ComposerMode::NewSession));
        assert_eq!(app.launch_provider, Provider::Claude);

        handle_key(&mut app, key(KeyCode::Tab));
        handle_key(&mut app, key(KeyCode::Char('3')));
        assert_eq!(app.overlay, Overlay::Composer(ComposerMode::NewSession));
        assert_eq!(app.launch_provider, Provider::Pi);
        assert_eq!(app.input, "x");
    }

    #[test]
    fn model_picker_keys_filter_page_select_and_cancel_without_editing_the_draft() {
        let mut app = App::with_launch_targets(
            SessionSnapshot::default(),
            false,
            Provider::Pi,
            vec![LaunchTarget {
                provider: Provider::Pi,
                supports_model: true,
            }],
        );
        app.start_new_session(Some('x'));
        assert_eq!(app.open_model_picker(), AppAction::LoadModels { provider: Provider::Pi });
        app.set_available_models(
            Provider::Pi,
            Ok((0..25).map(|index| format!("provider/model-{index:02}")).collect()),
        );

        handle_key(&mut app, key(KeyCode::Down));
        assert_eq!(app.model_selection, 1);
        handle_key(&mut app, key(KeyCode::PageDown));
        assert_eq!(app.model_selection, 11);
        handle_key(&mut app, key(KeyCode::PageUp));
        assert_eq!(app.model_selection, 1);
        handle_key(&mut app, key(KeyCode::Tab));
        assert_eq!(app.model_selection, 2);
        handle_key(&mut app, key(KeyCode::BackTab));
        assert_eq!(app.model_selection, 1);
        handle_key(&mut app, key(KeyCode::Char('2')));
        assert_eq!(app.model_filter, "2");
        handle_key(&mut app, key(KeyCode::Backspace));
        assert!(app.model_filter.is_empty());
        assert_eq!(app.input, "x");
        handle_key(&mut app, key(KeyCode::Esc));
        assert_eq!(app.overlay, Overlay::Composer(ComposerMode::NewSession));
        assert_eq!(app.input, "x");
    }

    #[derive(Default)]
    struct FakeTerminal {
        calls: Vec<&'static str>,
        suspend_error: bool,
        resume_error: bool,
    }

    impl DashboardTerminal for FakeTerminal {
        fn suspend_dashboard(&mut self) -> Result<()> {
            self.calls.push("suspend");
            if self.suspend_error {
                anyhow::bail!("synthetic suspend failure");
            }
            Ok(())
        }

        fn resume_dashboard(&mut self) -> Result<()> {
            self.calls.push("resume");
            if self.resume_error {
                anyhow::bail!("synthetic resume failure");
            }
            Ok(())
        }
    }

    #[derive(Default)]
    struct FakeControl {
        calls: Mutex<Vec<String>>,
        fail_on: Option<&'static str>,
        launch_hint: Option<&'static str>,
    }

    impl FakeControl {
        fn invoke(&self, operation: &'static str, detail: String) -> Result<ControlOutcome> {
            self.calls
                .lock()
                .unwrap()
                .push(format!("{operation}:{detail}"));
            if self.fail_on == Some(operation) {
                anyhow::bail!("synthetic {operation} failure");
            }
            Ok(ControlOutcome {
                message: format!("{operation} ok"),
                provider_session_hint: None,
            })
        }
    }

    impl DashboardControl for FakeControl {
        fn inspect_session(&self, session: &AgentSession) -> Result<String> {
            self.invoke("inspect", session.id.clone())?;
            Ok(format!("detail for {}", session.id))
        }

        fn open_session(&self, session: &AgentSession) -> Result<ControlOutcome> {
            self.invoke("open", session.id.clone())
        }

        fn launch_session(
            &self,
            _provider: Provider,
            _model: Option<String>,
            prompt: String,
        ) -> Result<ControlOutcome> {
            let mut outcome = self.invoke("launch", prompt)?;
            outcome.provider_session_hint = self.launch_hint.map(str::to_owned);
            Ok(outcome)
        }

        fn reply_session(&self, session: &AgentSession, prompt: &str) -> Result<ControlOutcome> {
            self.invoke("reply", format!("{}:{prompt}", session.id))
        }

        fn resolve_session_approval(
            &self,
            session: &AgentSession,
            accept: bool,
        ) -> Result<ControlOutcome> {
            self.invoke("approval", format!("{}:{accept}", session.id))
        }

        fn respond_session_input(
            &self,
            session: &AgentSession,
            answer: &str,
        ) -> Result<ControlOutcome> {
            self.invoke("input", format!("{}:{answer}", session.id))
        }

        fn interrupt_session(&self, session: &AgentSession) -> Result<ControlOutcome> {
            self.invoke("interrupt", session.id.clone())
        }

        fn archive_session(&self, session: &AgentSession) -> Result<ControlOutcome> {
            self.invoke("archive", session.id.clone())
        }

        fn delete_session(&self, session: &AgentSession) -> Result<ControlOutcome> {
            self.invoke("delete", session.id.clone())
        }
    }

    #[test]
    fn action_dispatch_handles_noop_quit_and_rename_without_provider_calls() {
        let mut app = app();
        let mut terminal = FakeTerminal::default();
        let control = FakeControl::default();

        assert!(!handle_action(
            &mut terminal,
            &mut app,
            AppAction::None,
            &control
        ));
        assert!(handle_action(
            &mut terminal,
            &mut app,
            AppAction::Refresh,
            &control
        ));
        assert_eq!(app.notice.as_deref(), Some("refreshing provider sessions…"));
        assert!(!handle_action(
            &mut terminal,
            &mut app,
            AppAction::Quit,
            &control
        ));
        assert!(!handle_action(
            &mut terminal,
            &mut app,
            AppAction::Rename {
                session_id: "worker".into(),
                name: "new".into()
            },
            &control
        ));
        assert!(app
            .notice
            .as_deref()
            .unwrap()
            .contains("rename is unavailable"));
        assert!(control.calls.lock().unwrap().is_empty());
    }

    #[test]
    fn action_dispatch_routes_every_non_open_operation_and_refreshes_mutations() {
        let mut app = app();
        let mut terminal = FakeTerminal::default();
        let control = FakeControl::default();

        assert!(!handle_action(
            &mut terminal,
            &mut app,
            AppAction::Inspect {
                session_id: "worker".into()
            },
            &control
        ));
        assert_eq!(app.selected_detail(), Some("detail for worker"));

        let actions = vec![
            AppAction::Launch {
                provider: Provider::Claude,
                model: None,
                prompt: "build".into(),
            },
            AppAction::Reply {
                session_id: "worker".into(),
                prompt: "continue".into(),
            },
            AppAction::ResolveApproval {
                session_id: "worker".into(),
                accept: true,
            },
            AppAction::RespondInput {
                session_id: "worker".into(),
                answer: "choice".into(),
            },
            AppAction::Interrupt {
                session_id: "worker".into(),
            },
            AppAction::Archive {
                session_id: "worker".into(),
            },
            AppAction::Delete {
                session_ids: vec!["worker".into()],
            },
        ];
        for action in actions {
            assert!(handle_action(&mut terminal, &mut app, action, &control));
        }

        assert_eq!(
            *control.calls.lock().unwrap(),
            vec![
                "inspect:worker",
                "launch:build",
                "reply:worker:continue",
                "approval:worker:true",
                "inspect:worker",
                "input:worker:choice",
                "inspect:worker",
                "interrupt:worker",
                "archive:worker",
                "delete:worker",
            ]
        );
        assert_eq!(
            app.notice.as_deref(),
            Some("deleted 1 managed Codex session(s)")
        );
    }

    #[test]
    fn launch_effect_carries_exact_provider_hint_for_post_launch_selection() {
        let mut app = app();
        let mut terminal = FakeTerminal::default();
        let control = FakeControl {
            launch_hint: Some("provider-id"),
            ..FakeControl::default()
        };

        let effect = dispatch_action(
            &mut terminal,
            &mut app,
            AppAction::Launch {
                provider: Provider::Pi,
                model: Some("openai/gpt-5".into()),
                prompt: "build".into(),
            },
            &control,
        );

        assert!(effect.refresh);
        assert_eq!(
            effect.pending_launch,
            Some((Provider::Pi, "provider-id".into()))
        );
        assert_eq!(*control.calls.lock().unwrap(), vec!["launch:build"]);
    }

    #[test]
    fn pending_launch_selection_requires_both_provider_and_provider_session_id() {
        let mut app = app();
        app.snapshot.sessions[0].provider = Provider::Claude;
        app.snapshot.sessions[0].provider_session_id = "same".into();
        let mut pi = app.snapshot.sessions[0].clone();
        pi.id = "pi:host:same".into();
        pi.provider = Provider::Pi;
        app.snapshot.sessions.push(pi);
        let pending = PendingLaunch {
            provider: Provider::Pi,
            provider_session_id: "same".into(),
            deadline: Instant::now() + Duration::from_secs(1),
        };

        assert!(select_pending_launch(&mut app, Some(&pending)));
        assert_eq!(
            app.selection,
            Some(SelectionKey::Session("pi:host:same".into()))
        );
        let missing = PendingLaunch {
            provider: Provider::Codex,
            ..pending
        };
        assert!(!select_pending_launch(&mut app, Some(&missing)));
    }

    #[test]
    fn completed_and_model_actions_are_returned_as_nonblocking_loop_effects() {
        let mut app = app();
        let mut terminal = FakeTerminal::default();
        let control = FakeControl::default();
        let completed = dispatch_action(
            &mut terminal,
            &mut app,
            AppAction::SetCompletedVisibility {
                include_completed: true,
            },
            &control,
        );
        assert!(completed.refresh);
        assert_eq!(completed.completed_visibility, Some(true));

        let models = dispatch_action(
            &mut terminal,
            &mut app,
            AppAction::LoadModels {
                provider: Provider::OpenCode,
            },
            &control,
        );
        assert!(!models.refresh);
        assert_eq!(models.load_models, Some(Provider::OpenCode));
        assert!(control.calls.lock().unwrap().is_empty());
    }

    #[test]
    fn refresh_queue_carries_the_latest_discovery_request() {
        let (sender, receiver) = mpsc::sync_channel(1);
        let mut in_flight = false;
        let mut request = DiscoveryRequest {
            include_completed: false,
            include_interactive: false,
            cwd: None,
        };
        schedule_refresh(&sender, &request, &mut in_flight).unwrap();
        assert!(!receiver.recv().unwrap().include_completed);

        in_flight = false;
        request.include_completed = true;
        schedule_refresh(&sender, &request, &mut in_flight).unwrap();
        assert!(receiver.recv().unwrap().include_completed);
        assert!(in_flight);
    }

    #[test]
    fn action_dispatch_rejects_every_stale_session_id_without_side_effects() {
        let actions = vec![
            AppAction::Inspect {
                session_id: "gone".into(),
            },
            AppAction::Open {
                session_id: "gone".into(),
            },
            AppAction::Reply {
                session_id: "gone".into(),
                prompt: "x".into(),
            },
            AppAction::ResolveApproval {
                session_id: "gone".into(),
                accept: false,
            },
            AppAction::RespondInput {
                session_id: "gone".into(),
                answer: "x".into(),
            },
            AppAction::Interrupt {
                session_id: "gone".into(),
            },
            AppAction::Archive {
                session_id: "gone".into(),
            },
            AppAction::Delete {
                session_ids: vec!["worker".into(), "gone".into()],
            },
        ];
        for action in actions {
            let mut app = app();
            let mut terminal = FakeTerminal::default();
            let control = FakeControl::default();
            assert!(!handle_action(&mut terminal, &mut app, action, &control));
            assert!(app
                .notice
                .as_deref()
                .unwrap()
                .contains("disappeared during refresh"));
            assert!(terminal.calls.is_empty());
            assert!(control.calls.lock().unwrap().is_empty());
        }
    }

    #[test]
    fn provider_refusals_are_reported_for_every_mutating_action() {
        let cases = vec![
            (
                "launch",
                AppAction::Launch {
                    provider: Provider::Claude,
                    model: None,
                    prompt: "x".into(),
                },
                "launch failed",
            ),
            (
                "reply",
                AppAction::Reply {
                    session_id: "worker".into(),
                    prompt: "x".into(),
                },
                "reply refused",
            ),
            (
                "approval",
                AppAction::ResolveApproval {
                    session_id: "worker".into(),
                    accept: false,
                },
                "approval response refused",
            ),
            (
                "input",
                AppAction::RespondInput {
                    session_id: "worker".into(),
                    answer: "x".into(),
                },
                "input response refused",
            ),
            (
                "interrupt",
                AppAction::Interrupt {
                    session_id: "worker".into(),
                },
                "stop refused",
            ),
            (
                "archive",
                AppAction::Archive {
                    session_id: "worker".into(),
                },
                "archive refused",
            ),
        ];
        for (operation, action, expected) in cases {
            let mut app = app();
            let mut terminal = FakeTerminal::default();
            let control = FakeControl {
                calls: Mutex::default(),
                fail_on: Some(operation),
                launch_hint: None,
            };
            assert!(handle_action(&mut terminal, &mut app, action, &control));
            assert!(app.notice.as_deref().unwrap().contains(expected));
        }
    }

    #[test]
    fn inspect_failure_is_non_refreshing_and_does_not_replace_detail() {
        let mut app = app();
        app.set_detail("worker".into(), "old detail".into());
        let mut terminal = FakeTerminal::default();
        let control = FakeControl {
            calls: Mutex::default(),
            fail_on: Some("inspect"),
            launch_hint: None,
        };

        assert!(!handle_action(
            &mut terminal,
            &mut app,
            AppAction::Inspect {
                session_id: "worker".into()
            },
            &control
        ));
        assert_eq!(app.selected_detail(), Some("old detail"));
        assert!(app.notice.as_deref().unwrap().contains("inspect failed"));
    }

    #[test]
    fn open_suspends_and_always_attempts_resume_before_reporting_outcome() {
        let action = AppAction::Open {
            session_id: "worker".into(),
        };
        let mut first_app = app();
        let mut terminal = FakeTerminal::default();
        let control = FakeControl::default();
        assert!(handle_action(
            &mut terminal,
            &mut first_app,
            action.clone(),
            &control
        ));
        assert_eq!(terminal.calls, vec!["suspend", "resume"]);
        assert_eq!(first_app.notice.as_deref(), Some("open ok"));

        let mut failed_open_app = app();
        let mut terminal = FakeTerminal::default();
        let control = FakeControl {
            calls: Mutex::default(),
            fail_on: Some("open"),
            launch_hint: None,
        };
        assert!(handle_action(
            &mut terminal,
            &mut failed_open_app,
            action.clone(),
            &control
        ));
        assert_eq!(terminal.calls, vec!["suspend", "resume"]);
        assert!(failed_open_app
            .notice
            .as_deref()
            .unwrap()
            .contains("failed to open session"));

        let mut failed_suspend_app = app();
        let mut terminal = FakeTerminal {
            suspend_error: true,
            ..FakeTerminal::default()
        };
        let control = FakeControl::default();
        assert!(!handle_action(
            &mut terminal,
            &mut failed_suspend_app,
            action.clone(),
            &control
        ));
        assert_eq!(terminal.calls, vec!["suspend"]);
        assert!(control.calls.lock().unwrap().is_empty());
        assert!(failed_suspend_app
            .notice
            .as_deref()
            .unwrap()
            .contains("failed to suspend"));

        let mut failed_resume_app = app();
        let mut terminal = FakeTerminal {
            resume_error: true,
            ..FakeTerminal::default()
        };
        let control = FakeControl::default();
        assert!(handle_action(
            &mut terminal,
            &mut failed_resume_app,
            action,
            &control
        ));
        assert!(failed_resume_app
            .notice
            .as_deref()
            .unwrap()
            .contains("failed to restore"));
    }

    #[test]
    fn bulk_delete_stops_on_first_refusal_and_reports_exact_session() {
        let mut second = app().snapshot.sessions.remove(0);
        second.id = "second".into();
        second.name = "second-name".into();
        let mut app = app();
        app.snapshot.sessions.push(second);
        let mut terminal = FakeTerminal::default();
        let control = FakeControl {
            calls: Mutex::default(),
            fail_on: Some("delete"),
            launch_hint: None,
        };

        assert!(handle_action(
            &mut terminal,
            &mut app,
            AppAction::Delete {
                session_ids: vec!["worker".into(), "second".into()]
            },
            &control
        ));
        assert!(app.notice.as_deref().unwrap().contains("worker"));
        assert_eq!(*control.calls.lock().unwrap(), vec!["delete:worker"]);
    }
}

use std::collections::BTreeSet;
use std::io::{self, Stdout};
use std::sync::mpsc::{self, SyncSender, TryRecvError, TrySendError};
use std::thread;
use std::time::{Duration, Instant};

use anyhow::{anyhow, Context, Result};
use crossterm::event::{
    self, Event, KeyCode, KeyEvent, KeyEventKind, KeyModifiers, KeyboardEnhancementFlags,
    PopKeyboardEnhancementFlags, PushKeyboardEnhancementFlags,
};
use crossterm::execute;
use crossterm::terminal::{
    disable_raw_mode, enable_raw_mode, EnterAlternateScreen, LeaveAlternateScreen,
};
use ratatui::backend::CrosstermBackend;
use ratatui::Terminal;

use crate::adapters::{DiscoveryEngine, DiscoveryRequest};
use crate::aliases::SessionAliases;
use crate::app::{App, AppAction, Overlay, SESSION_PAGE_SIZE};
use crate::control::{ControlHub, ControlOutcome, LaunchPresentation};
use crate::domain::{AgentSession, Capability, Provider, SessionSnapshot, SessionState};
use crate::hidden::HiddenSessions;
use crate::migration::{MigrationClient, MigrationOutcome, MigrationRegistry, MigrationRequest};
use crate::ui;

// Apply a burst of already-buffered terminal input before drawing. Holding an
// arrow key can otherwise enqueue hundreds of repeat events, each of which
// would repaint a frame that the user will never see and can saturate a remote
// terminal or tmux pane.
const MAX_READY_EVENTS_PER_TICK: usize = 256;
const LAUNCH_DISCOVERY_RETRY_INTERVAL: Duration = Duration::from_millis(250);
const LAUNCH_DISCOVERY_TIMEOUT: Duration = Duration::from_secs(5);
const LAUNCH_ANIMATION_INTERVAL: Duration = Duration::from_millis(120);
const LIVE_SESSION_ANIMATION_INTERVAL: Duration = Duration::from_millis(550);
const LAUNCH_SPINNER: [&str; 8] = ["⠋", "⠙", "⠹", "⠸", "⠼", "⠴", "⠦", "⠧"];

#[derive(Clone, Debug, Eq, PartialEq)]
struct PendingLaunch {
    provider: Provider,
    provider_session_id: String,
    known_session_ids: BTreeSet<String>,
    open_when_visible: bool,
    deadline: Instant,
}

#[derive(Debug, Eq, PartialEq)]
struct PendingLaunchIntent {
    provider: Provider,
    provider_session_id: String,
    known_session_ids: BTreeSet<String>,
}

#[derive(Debug, Default)]
struct ActionEffect {
    refresh: bool,
    pending_launch: Option<PendingLaunchIntent>,
    completed_visibility: Option<bool>,
    load_models: Option<Provider>,
    hide_session_ids: Vec<String>,
    session_alias: Option<(String, String)>,
}

#[derive(Debug)]
struct LaunchWorkerResult {
    sequence: u64,
    provider: Provider,
    model: Option<String>,
    prompt: String,
    open_when_visible: bool,
    known_session_ids: BTreeSet<String>,
    result: Result<ControlOutcome, String>,
}

#[derive(Debug)]
struct LaunchJob {
    sequence: u64,
    provider: Provider,
    model: Option<String>,
    prompt: String,
    open_when_visible: bool,
    known_session_ids: BTreeSet<String>,
}

#[derive(Debug)]
struct MigrationWorkerResult {
    sequence: u64,
    request: MigrationRequest,
    result: Result<MigrationOutcome, String>,
}

pub struct MigrationServices {
    client: MigrationClient,
    registry: MigrationRegistry,
}

impl MigrationServices {
    pub fn new(client: MigrationClient, registry: MigrationRegistry) -> Self {
        Self { client, registry }
    }
}

pub fn run_dashboard(
    engine: &DiscoveryEngine,
    request: &DiscoveryRequest,
    refresh_interval: Duration,
    control: &ControlHub,
    hidden_sessions: HiddenSessions,
    session_aliases: SessionAliases,
    migrations: MigrationServices,
) -> Result<()> {
    let MigrationServices {
        client: migration_client,
        registry: migration_registry,
    } = migrations;
    let (refresh_tx, refresh_rx) = mpsc::sync_channel(1);
    let (snapshot_tx, snapshot_rx) = mpsc::channel::<(SessionSnapshot, bool)>();
    let (models_tx, models_rx) = mpsc::channel::<(Provider, bool, Result<Vec<String>, String>)>();
    let (launch_tx, launch_rx) = mpsc::channel::<LaunchWorkerResult>();
    let (migration_tx, migration_rx) = mpsc::channel::<MigrationWorkerResult>();
    let worker_engine = (*engine).clone();
    let worker_control = control.clone();
    let worker_hidden_sessions = hidden_sessions.clone();
    let worker_session_aliases = session_aliases.clone();
    let _refresh_worker = thread::spawn(move || {
        let mut first_refresh = true;
        while let Ok(worker_request) = refresh_rx.recv() {
            let partial_sender = snapshot_tx.clone();
            let mut snapshot = if first_refresh {
                worker_engine.discover_progressively(
                    &worker_request,
                    |partial, completed, total| {
                        let mut partial = partial.clone();
                        worker_control.enrich(&mut partial);
                        if !worker_request.include_external {
                            worker_control.retain_owned(&mut partial);
                        }
                        worker_hidden_sessions.filter_snapshot(&mut partial);
                        if let Err(error) = worker_session_aliases.reload() {
                            partial
                                .warnings
                                .push(format!("failed to reload local session names: {error:#}"));
                        }
                        worker_session_aliases.apply_snapshot(&mut partial);
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
            if !worker_request.include_external {
                worker_control.retain_owned(&mut snapshot);
            }
            worker_hidden_sessions.filter_snapshot(&mut snapshot);
            if let Err(error) = worker_session_aliases.reload() {
                snapshot
                    .warnings
                    .push(format!("failed to reload local session names: {error:#}"));
            }
            worker_session_aliases.apply_snapshot(&mut snapshot);
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
    let initial_size = terminal.terminal.size()?;
    app.set_session_page_size(session_page_size_for_terminal(initial_size.height));
    let mut last_refresh = Instant::now();
    let mut current_request = request.clone();
    let mut refresh_in_flight = false;
    let mut refresh_after_current = false;
    let mut pending_launch: Option<PendingLaunch> = None;
    let mut pending_launch_retry_at: Option<Instant> = None;
    let mut latest_launch_sequence = 0u64;
    let mut launching_provider: Option<Provider> = None;
    let mut latest_migration_sequence = 0u64;
    let mut migrating_target: Option<Provider> = None;
    let mut launch_animation_tick = 0usize;
    let mut next_launch_animation = Instant::now();
    let mut next_live_animation = Instant::now() + LIVE_SESSION_ANIMATION_INTERVAL;
    let mut needs_draw = true;
    schedule_refresh(
        &refresh_tx,
        &discovery_request_for_pending_launch(&current_request, pending_launch.as_ref()),
        &mut refresh_in_flight,
    )?;

    let result = 'dashboard: loop {
        loop {
            match snapshot_rx.try_recv() {
                Ok((mut snapshot, complete)) => {
                    if let Some(pending) = pending_launch.as_ref() {
                        if let Some(launched) = snapshot.sessions.iter().find(|session| {
                            session.provider == pending.provider
                                && session.provider_session_id == pending.provider_session_id
                        }) {
                            if launched.state == SessionState::Completed
                                && !current_request.include_completed
                            {
                                current_request.include_completed = true;
                                app.reveal_completed_after_launch();
                            }
                        }
                    }
                    if !current_request.include_completed {
                        snapshot
                            .sessions
                            .retain(|session| session.state != SessionState::Completed);
                    }
                    if !current_request.include_interactive {
                        snapshot.sessions.retain(|session| {
                            session.kind != crate::domain::SessionKind::Interactive
                                || pending_launch.as_ref().is_some_and(|pending| {
                                    session.provider == pending.provider
                                        && session.provider_session_id
                                            == pending.provider_session_id
                                })
                        });
                    }
                    let changed = snapshot != app.snapshot;
                    app.replace_snapshot(snapshot);
                    if let Some(session_id) =
                        select_pending_launch(&mut app, pending_launch.as_ref())
                    {
                        let open_when_visible = pending_launch
                            .as_ref()
                            .is_some_and(|pending| pending.open_when_visible);
                        pending_launch = None;
                        pending_launch_retry_at = None;
                        if open_when_visible {
                            refresh_after_current |= handle_action_legacy(
                                &mut terminal,
                                &mut app,
                                AppAction::Open { session_id },
                                control,
                            );
                        }
                    }
                    if complete {
                        refresh_in_flight = false;
                        last_refresh = Instant::now();
                        if let Some(pending) = pending_launch.as_ref() {
                            if Instant::now() < pending.deadline {
                                pending_launch_retry_at =
                                    Some(Instant::now() + LAUNCH_DISCOVERY_RETRY_INTERVAL);
                            } else {
                                app.set_notice(
                                    "task launched, but its provider record is not visible yet; press ctrl+l to retry",
                                );
                                pending_launch = None;
                                pending_launch_retry_at = None;
                            }
                        }
                    }
                    // Relative ages are computed while drawing. A completed
                    // refresh must repaint even if provider bytes are
                    // identical, otherwise the visible age can freeze.
                    needs_draw |= changed || complete;
                }
                Err(TryRecvError::Empty) => break,
                Err(TryRecvError::Disconnected) => {
                    break 'dashboard Err(anyhow!("provider refresh worker stopped unexpectedly"));
                }
            }
        }
        loop {
            match models_rx.try_recv() {
                Ok((provider, auth_available, result)) => {
                    app.set_models_auth_available(&provider, auth_available);
                    app.set_available_models(provider, result);
                    needs_draw = true;
                }
                Err(TryRecvError::Empty) => break,
                Err(TryRecvError::Disconnected) => break,
            }
        }
        let mut completed_launch_needs_refresh = false;
        loop {
            match launch_rx.try_recv() {
                Ok(completed) => {
                    completed_launch_needs_refresh = true;
                    if completed.sequence == latest_launch_sequence {
                        launching_provider = None;
                        match completed.result {
                            Ok(outcome) => {
                                app.set_notice(outcome.message);
                                pending_launch =
                                    outcome.provider_session_hint.map(|provider_session_id| {
                                        PendingLaunch {
                                            provider: completed.provider,
                                            provider_session_id,
                                            known_session_ids: completed.known_session_ids,
                                            open_when_visible: completed.open_when_visible,
                                            deadline: Instant::now() + LAUNCH_DISCOVERY_TIMEOUT,
                                        }
                                    });
                                pending_launch_retry_at = None;
                            }
                            Err(error)
                                if control.supports_authentication(&completed.provider)
                                    && looks_like_authentication_error(&error) =>
                            {
                                app.require_authentication(
                                    completed.provider,
                                    completed.model,
                                    completed.prompt,
                                    error,
                                );
                            }
                            Err(error) => app.set_notice(format!("launch failed: {error}")),
                        }
                    }
                    needs_draw = true;
                }
                Err(TryRecvError::Empty) => break,
                Err(TryRecvError::Disconnected) => break,
            }
        }
        let mut completed_migration_needs_refresh = false;
        loop {
            match migration_rx.try_recv() {
                Ok(completed) => {
                    if completed.sequence == latest_migration_sequence {
                        migrating_target = None;
                        match completed.result {
                            Ok(outcome) => {
                                let warning_count = outcome.warnings.len();
                                match migration_registry.record(&completed.request, &outcome) {
                                    Ok(_) => {
                                        let alias_result = session_aliases.set_for_id(
                                            &outcome.normalized_id,
                                            &completed.request.name,
                                        );
                                        match alias_result {
                                            Ok(_) => app.set_notice(format!(
                                                "migrated to {} as {}{}",
                                                completed.request.target.label(),
                                                completed.request.name,
                                                if warning_count == 0 {
                                                    String::new()
                                                } else {
                                                    format!(" · {warning_count} warning{}", if warning_count == 1 { "" } else { "s" })
                                                }
                                            )),
                                            Err(error) => app.set_notice(format!(
                                                "migration succeeded, but its local name could not be saved: {error:#}"
                                            )),
                                        }
                                        pending_launch = Some(PendingLaunch {
                                            provider: completed.request.target,
                                            provider_session_id: outcome.session_id,
                                            known_session_ids: BTreeSet::new(),
                                            open_when_visible: false,
                                            deadline: Instant::now() + LAUNCH_DISCOVERY_TIMEOUT,
                                        });
                                        pending_launch_retry_at = None;
                                        completed_migration_needs_refresh = true;
                                    }
                                    Err(error) => app.set_notice(format!(
                                        "migration succeeded, but OAV could not index it: {error:#}"
                                    )),
                                }
                            }
                            Err(error) => app.set_notice(format!("migration failed: {error}")),
                        }
                    }
                    needs_draw = true;
                }
                Err(TryRecvError::Empty) => break,
                Err(TryRecvError::Disconnected) => break,
            }
        }
        if completed_migration_needs_refresh {
            if refresh_in_flight {
                refresh_after_current = true;
            } else {
                schedule_refresh(
                    &refresh_tx,
                    &discovery_request_for_pending_launch(
                        &current_request,
                        pending_launch.as_ref(),
                    ),
                    &mut refresh_in_flight,
                )?;
                last_refresh = Instant::now();
            }
        }
        if completed_launch_needs_refresh {
            if refresh_in_flight {
                refresh_after_current = true;
            } else {
                schedule_refresh(
                    &refresh_tx,
                    &discovery_request_for_pending_launch(
                        &current_request,
                        pending_launch.as_ref(),
                    ),
                    &mut refresh_in_flight,
                )?;
                last_refresh = Instant::now();
            }
        }
        if refresh_after_current && !refresh_in_flight {
            schedule_refresh(
                &refresh_tx,
                &discovery_request_for_pending_launch(&current_request, pending_launch.as_ref()),
                &mut refresh_in_flight,
            )?;
            refresh_after_current = false;
            last_refresh = Instant::now();
        }

        if app.should_quit {
            break Ok(());
        }
        if (launching_provider.is_some() || migrating_target.is_some())
            && Instant::now() >= next_launch_animation
        {
            let provider = launching_provider
                .as_ref()
                .or(migrating_target.as_ref())
                .expect("checked above");
            app.set_notice(format!(
                "{} {} {}…",
                LAUNCH_SPINNER[launch_animation_tick % LAUNCH_SPINNER.len()],
                if migrating_target.is_some() {
                    "migrating to"
                } else {
                    "launching"
                },
                provider.label()
            ));
            launch_animation_tick = launch_animation_tick.wrapping_add(1);
            next_launch_animation = Instant::now() + LAUNCH_ANIMATION_INTERVAL;
            needs_draw = true;
        }
        if Instant::now() >= next_live_animation {
            needs_draw |= app.advance_live_animation();
            next_live_animation = Instant::now() + LIVE_SESSION_ANIMATION_INTERVAL;
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
                        let mut action = handle_key(&mut app, key);
                        if action == AppAction::Quit && migrating_target.is_some() {
                            app.should_quit = false;
                            app.set_notice(
                                "migration is still running; OAV will be ready to exit when it finishes",
                            );
                            action = AppAction::None;
                        }
                        let mut effect = match action {
                            AppAction::Migrate {
                                session_id,
                                target,
                                name,
                            } => {
                                if migrating_target.is_some() {
                                    app.set_notice("one session migration is already running");
                                    needs_draw = true;
                                    continue;
                                }
                                let Some(source) = app
                                    .snapshot
                                    .sessions
                                    .iter()
                                    .find(|session| session.id == session_id)
                                    .cloned()
                                else {
                                    app.set_notice(
                                        "the selected session disappeared during refresh",
                                    );
                                    needs_draw = true;
                                    continue;
                                };
                                latest_migration_sequence =
                                    latest_migration_sequence.wrapping_add(1);
                                migrating_target = Some(target.clone());
                                launch_animation_tick = 0;
                                next_launch_animation = Instant::now();
                                app.set_notice(format!("migrating to {}…", target.label()));
                                schedule_migration(
                                    migration_client.clone(),
                                    latest_migration_sequence,
                                    MigrationRequest {
                                        source,
                                        target,
                                        name,
                                    },
                                    migration_tx.clone(),
                                );
                                ActionEffect::default()
                            }
                            AppAction::Launch {
                                provider,
                                model,
                                prompt,
                            } => match control.launch_presentation(&provider) {
                                Ok(LaunchPresentation::Foreground) => {
                                    app.set_notice(format!(
                                        "starting {} native session…",
                                        provider.label()
                                    ));
                                    terminal.terminal.draw(|frame| ui::render(frame, &app))?;
                                    dispatch_foreground_launch(
                                        &mut terminal,
                                        &mut app,
                                        provider,
                                        model,
                                        prompt,
                                        control,
                                    )
                                }
                                Ok(
                                    presentation @ (LaunchPresentation::Background
                                    | LaunchPresentation::DeferredForeground),
                                ) => {
                                    let known_session_ids = provider_session_ids(&app, &provider);
                                    latest_launch_sequence = latest_launch_sequence.wrapping_add(1);
                                    launching_provider = Some(provider.clone());
                                    launch_animation_tick = 0;
                                    next_launch_animation = Instant::now();
                                    app.set_notice(format!("launching {}…", provider.label()));
                                    schedule_launch(
                                        control.clone(),
                                        LaunchJob {
                                            sequence: latest_launch_sequence,
                                            provider,
                                            model,
                                            prompt,
                                            open_when_visible: presentation
                                                == LaunchPresentation::DeferredForeground,
                                            known_session_ids,
                                        },
                                        launch_tx.clone(),
                                    );
                                    ActionEffect::default()
                                }
                                Err(error) => {
                                    app.set_notice(format!("launch failed: {error:#}"));
                                    ActionEffect::default()
                                }
                            },
                            other => dispatch_action(&mut terminal, &mut app, other, control),
                        };
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
                        if let Some(intent) = effect.pending_launch {
                            pending_launch = Some(PendingLaunch {
                                provider: intent.provider,
                                provider_session_id: intent.provider_session_id,
                                known_session_ids: intent.known_session_ids,
                                open_when_visible: false,
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
                        if let Some((session_id, alias)) = effect.session_alias.take() {
                            match update_session_alias_from_app(
                                &mut app,
                                &session_aliases,
                                &session_id,
                                &alias,
                            ) {
                                Ok(SessionAliasUpdate::Set) => app.set_notice(format!(
                                    "renamed locally to {alias}; the provider title was not changed"
                                )),
                                Ok(SessionAliasUpdate::Unchanged) => app.set_notice(format!(
                                    "local name is already {alias}; the provider title was not changed"
                                )),
                                Ok(SessionAliasUpdate::Cleared) => app.set_notice(
                                    "cleared local name; refreshing the latest provider title",
                                ),
                                Ok(SessionAliasUpdate::NotSet) => app.set_notice(
                                    "no local name was set; refreshing the latest provider title",
                                ),
                                Err(error) => app
                                    .set_notice(format!("failed to rename session locally: {error:#}")),
                            }
                            effect.refresh = true;
                        }
                        needs_draw = true;
                        if effect.refresh {
                            if refresh_in_flight {
                                refresh_after_current = true;
                            } else {
                                schedule_refresh(
                                    &refresh_tx,
                                    &discovery_request_for_pending_launch(
                                        &current_request,
                                        pending_launch.as_ref(),
                                    ),
                                    &mut refresh_in_flight,
                                )?;
                                last_refresh = Instant::now();
                            }
                        }
                    }
                    Event::Resize(_, height) => {
                        app.set_session_page_size(session_page_size_for_terminal(height));
                        needs_draw = true;
                    }
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
                &discovery_request_for_pending_launch(&current_request, pending_launch.as_ref()),
                &mut refresh_in_flight,
            )?;
            pending_launch_retry_at = None;
            last_refresh = Instant::now();
        }
        if !refresh_in_flight && last_refresh.elapsed() >= refresh_interval {
            schedule_refresh(
                &refresh_tx,
                &discovery_request_for_pending_launch(&current_request, pending_launch.as_ref()),
                &mut refresh_in_flight,
            )?;
            last_refresh = Instant::now();
        }
    };

    engine.cancel();
    drop(refresh_tx);
    crate::native_session::shutdown_all();
    result
}

fn session_page_size_for_terminal(height: u16) -> usize {
    let header_height = if height < 16 { 2 } else { 4 };
    let list_height = height.saturating_sub(header_height + 3 + 1);
    // Reserve a heading, a Show-more row, and a little separation so the
    // pagination control does not start just below the viewport.
    usize::from(list_height.saturating_sub(3).max(1)).min(SESSION_PAGE_SIZE)
}

fn discovery_request_for_pending_launch(
    request: &DiscoveryRequest,
    pending: Option<&PendingLaunch>,
) -> DiscoveryRequest {
    let mut request = request.clone();
    if pending.is_some() {
        // A task can finish before the first post-launch refresh, and some
        // providers temporarily classify newly created sessions as
        // interactive. Query both sets while resolving the exact returned ID;
        // the receiving side still applies the user's normal visibility until
        // that exact task is found.
        request.include_completed = true;
        request.include_interactive = true;
    }
    request
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
    sender: mpsc::Sender<(Provider, bool, Result<Vec<String>, String>)>,
) {
    let _model_worker = thread::spawn(move || {
        let auth_available = control.supports_authentication(&provider);
        let result = control
            .available_models(&provider)
            .map_err(|error| format!("{error:#}"));
        let _ = sender.send((provider, auth_available, result));
    });
}

fn schedule_launch(control: ControlHub, job: LaunchJob, sender: mpsc::Sender<LaunchWorkerResult>) {
    let operation_provider = job.provider.clone();
    let operation_model = job.model.clone();
    let operation_prompt = job.prompt.clone();
    schedule_launch_job(job, sender, move || {
        control.launch_with(operation_provider, operation_model, operation_prompt)
    });
}

fn schedule_migration(
    client: MigrationClient,
    sequence: u64,
    request: MigrationRequest,
    sender: mpsc::Sender<MigrationWorkerResult>,
) {
    let _migration_worker = thread::spawn(move || {
        let result = client
            .migrate(&request)
            .map_err(|error| format!("{error:#}"));
        let _ = sender.send(MigrationWorkerResult {
            sequence,
            request,
            result,
        });
    });
}

fn schedule_launch_job(
    job: LaunchJob,
    sender: mpsc::Sender<LaunchWorkerResult>,
    operation: impl FnOnce() -> Result<ControlOutcome> + Send + 'static,
) {
    let _launch_worker = thread::spawn(move || {
        let result = operation().map_err(|error| format!("{error:#}"));
        let _ = sender.send(LaunchWorkerResult {
            sequence: job.sequence,
            provider: job.provider,
            model: job.model,
            prompt: job.prompt,
            open_when_visible: job.open_when_visible,
            known_session_ids: job.known_session_ids,
            result,
        });
    });
}

fn dispatch_foreground_launch<T: DashboardTerminal, C: DashboardControl>(
    terminal: &mut T,
    app: &mut App,
    provider: Provider,
    model: Option<String>,
    prompt: String,
    control: &C,
) -> ActionEffect {
    let known_session_ids = provider_session_ids(app, &provider);
    if let Err(error) = terminal.suspend_dashboard() {
        app.set_notice(format!("failed to suspend dashboard: {error:#}"));
        return ActionEffect::default();
    }
    let retry_model = model.clone();
    let retry_prompt = prompt.clone();
    let result = control.launch_foreground_session(provider.clone(), model, prompt);
    let resume = terminal.resume_dashboard();
    match (result, resume) {
        (Ok(outcome), Ok(())) => {
            app.set_notice(outcome.message);
            ActionEffect {
                refresh: true,
                pending_launch: outcome.provider_session_hint.map(|provider_session_id| {
                    PendingLaunchIntent {
                        provider,
                        provider_session_id,
                        known_session_ids,
                    }
                }),
                ..ActionEffect::default()
            }
        }
        (Err(error), Ok(())) => {
            let error = format!("{error:#}");
            if control.supports_authentication(&provider) && looks_like_authentication_error(&error)
            {
                app.require_authentication(provider, retry_model, retry_prompt, error);
            } else {
                app.set_notice(format!("launch failed: {error}"));
            }
            ActionEffect {
                refresh: true,
                ..ActionEffect::default()
            }
        }
        (_, Err(error)) => {
            app.set_notice(format!("failed to restore dashboard: {error:#}"));
            ActionEffect::default()
        }
    }
}

fn provider_session_ids(app: &App, provider: &Provider) -> BTreeSet<String> {
    app.snapshot
        .sessions
        .iter()
        .filter(|session| &session.provider == provider)
        .map(|session| session.id.clone())
        .collect()
}

fn select_pending_launch(app: &mut App, pending: Option<&PendingLaunch>) -> Option<String> {
    let Some(pending) = pending else {
        return None;
    };
    let exact = app.snapshot.sessions.iter().find(|session| {
        session.provider == pending.provider
            && (session.provider_session_id == pending.provider_session_id
                || session
                    .provider_session_id
                    .starts_with(&pending.provider_session_id)
                || pending
                    .provider_session_id
                    .starts_with(&session.provider_session_id))
    });
    let session = exact.or_else(|| {
        let mut newly_discovered = app.snapshot.sessions.iter().filter(|session| {
            session.provider == pending.provider && !pending.known_session_ids.contains(&session.id)
        });
        let candidate = newly_discovered.next()?;
        newly_discovered.next().is_none().then_some(candidate)
    })?;
    let session_id = session.id.clone();
    app.select_and_reveal_session(&session_id)
        .then_some(session_id)
}

fn looks_like_authentication_error(error: &str) -> bool {
    let error = error.to_ascii_lowercase();
    error.contains("not authenticated")
        || error.contains("authentication required")
        || error.contains("sign in")
        || error.contains("login required")
        || error.contains("not logged in")
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

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum SessionAliasUpdate {
    Set,
    Unchanged,
    Cleared,
    NotSet,
}

fn update_session_alias_from_app(
    app: &mut App,
    session_aliases: &SessionAliases,
    session_id: &str,
    alias: &str,
) -> Result<SessionAliasUpdate> {
    let session = app
        .snapshot
        .sessions
        .iter()
        .find(|session| session.id == session_id)
        .cloned()
        .context("the selected session disappeared during refresh")?;
    if alias.trim().is_empty() {
        return Ok(if session_aliases.clear(session_id)?.is_some() {
            SessionAliasUpdate::Cleared
        } else {
            SessionAliasUpdate::NotSet
        });
    }
    let changed = session_aliases.set_for_session(&session, alias)?;
    let mut snapshot = app.snapshot.clone();
    session_aliases.apply_snapshot(&mut snapshot);
    app.replace_snapshot(snapshot);
    Ok(if changed {
        SessionAliasUpdate::Set
    } else {
        SessionAliasUpdate::Unchanged
    })
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
            KeyCode::Char('m') if app.overlay == Overlay::None => {
                app.start_migration();
                AppAction::None
            }
            KeyCode::Char('r') if app.overlay == Overlay::ModelPicker => {
                let provider = app.launch_provider.clone();
                app.retry_model_load(&provider);
                AppAction::LoadModels { provider }
            }
            KeyCode::Char('f') if app.overlay == Overlay::None => {
                app.start_filter();
                AppAction::None
            }
            KeyCode::Char('l') if app.overlay == Overlay::None => AppAction::Refresh,
            KeyCode::Char('x') => match app.overlay.clone() {
                Overlay::Confirm(_) => app.activate(),
                Overlay::None | Overlay::Peek => app.start_confirm(),
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
            KeyCode::Char('w') | KeyCode::Backspace => {
                app.delete_previous_word();
                AppAction::None
            }
            KeyCode::Char('u') => {
                app.delete_to_line_start();
                AppAction::None
            }
            _ => AppAction::None,
        };
    }

    if key.modifiers.contains(KeyModifiers::SUPER) && key.code == KeyCode::Backspace {
        app.delete_to_line_start();
        return AppAction::None;
    }
    if key.modifiers.contains(KeyModifiers::ALT) && key.code == KeyCode::Backspace {
        app.delete_previous_word();
        return AppAction::None;
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
        KeyCode::Up | KeyCode::Left
            if matches!(app.overlay, Overlay::MigrationTargetPicker { .. }) =>
        {
            app.move_migration_selection(-1);
            AppAction::None
        }
        KeyCode::Down | KeyCode::Right
            if matches!(app.overlay, Overlay::MigrationTargetPicker { .. }) =>
        {
            app.move_migration_selection(1);
            AppAction::None
        }
        KeyCode::PageUp if matches!(app.overlay, Overlay::MigrationTargetPicker { .. }) => {
            app.move_migration_page(-1);
            AppAction::None
        }
        KeyCode::PageDown if matches!(app.overlay, Overlay::MigrationTargetPicker { .. }) => {
            app.move_migration_page(1);
            AppAction::None
        }
        KeyCode::Char('l')
            if app.overlay == Overlay::ModelPicker
                && app.models_error.is_some()
                && app.models_auth_available =>
        {
            app.activate()
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
        KeyCode::Right if app.overlay == Overlay::None => app.activate(),
        KeyCode::Left if app.overlay == Overlay::Peek => app.escape(),
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
        KeyCode::BackTab
            if app.overlay == Overlay::Composer(crate::app::ComposerMode::NewSession) =>
        {
            app.open_model_picker()
        }
        KeyCode::Tab if app.overlay == Overlay::HarnessPicker => {
            app.move_harness_selection(1);
            AppAction::None
        }
        KeyCode::Tab if app.overlay == Overlay::ModelPicker => {
            app.move_model_selection(1);
            AppAction::None
        }
        KeyCode::Tab if matches!(app.overlay, Overlay::MigrationTargetPicker { .. }) => {
            app.move_migration_selection(1);
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
        KeyCode::BackTab if matches!(app.overlay, Overlay::MigrationTargetPicker { .. }) => {
            app.move_migration_selection(-1);
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
    fn launch_foreground_session(
        &self,
        provider: Provider,
        model: Option<String>,
        prompt: String,
    ) -> Result<ControlOutcome> {
        self.launch_session(provider, model, prompt)
    }
    fn authenticate_provider(&self, provider: &Provider) -> Result<ControlOutcome> {
        Err(anyhow!(
            "{} does not expose interactive login",
            provider.label()
        ))
    }
    fn setup_provider(&self, provider: &Provider) -> Result<ControlOutcome> {
        Err(anyhow!("{} setup is unavailable", provider.label()))
    }
    fn setup_launch_option(&self, provider: &Provider, _option: &str) -> Result<ControlOutcome> {
        Err(anyhow!(
            "{} launch-option setup is unavailable",
            provider.label()
        ))
    }
    fn supports_authentication(&self, _provider: &Provider) -> bool {
        false
    }
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

    fn launch_foreground_session(
        &self,
        provider: Provider,
        model: Option<String>,
        prompt: String,
    ) -> Result<ControlOutcome> {
        self.launch_foreground_with(provider, model, prompt)
    }

    fn authenticate_provider(&self, provider: &Provider) -> Result<ControlOutcome> {
        self.authenticate(provider)
    }

    fn setup_provider(&self, provider: &Provider) -> Result<ControlOutcome> {
        self.setup_provider(provider)
    }

    fn setup_launch_option(&self, provider: &Provider, option: &str) -> Result<ControlOutcome> {
        self.setup_launch_option(provider, option)
    }

    fn supports_authentication(&self, provider: &Provider) -> bool {
        self.supports_authentication(provider)
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
        AppAction::Authenticate { provider } => {
            if let Err(error) = terminal.suspend_dashboard() {
                app.set_notice(format!("failed to suspend dashboard: {error:#}"));
                return ActionEffect::default();
            }
            let result = control.authenticate_provider(&provider);
            let resume = terminal.resume_dashboard();
            match (result, resume) {
                (Ok(outcome), Ok(())) => {
                    app.set_notice(outcome.message);
                    app.retry_model_load(&provider);
                    ActionEffect {
                        refresh: true,
                        load_models: Some(provider),
                        ..ActionEffect::default()
                    }
                }
                (Err(error), Ok(())) => {
                    app.set_notice(format!("login failed: {error:#}"));
                    ActionEffect::default()
                }
                (_, Err(error)) => {
                    app.set_notice(format!("failed to restore dashboard: {error:#}"));
                    ActionEffect::default()
                }
            }
        }
        AppAction::SetupProvider { provider } => {
            if let Err(error) = terminal.suspend_dashboard() {
                app.set_notice(format!("failed to suspend dashboard: {error:#}"));
                return ActionEffect::default();
            }
            let result = control.setup_provider(&provider);
            let resume = terminal.resume_dashboard();
            match (result, resume) {
                (Ok(outcome), Ok(())) => {
                    app.set_notice(outcome.message);
                    ActionEffect {
                        refresh: true,
                        ..ActionEffect::default()
                    }
                }
                (Err(error), Ok(())) => {
                    app.set_notice(format!("setup failed: {error:#}"));
                    ActionEffect::default()
                }
                (_, Err(error)) => {
                    app.set_notice(format!("failed to restore dashboard: {error:#}"));
                    ActionEffect::default()
                }
            }
        }
        AppAction::SetupLaunchOption { provider, option } => {
            if let Err(error) = terminal.suspend_dashboard() {
                app.set_notice(format!("failed to suspend dashboard: {error:#}"));
                return ActionEffect::default();
            }
            let result = control.setup_launch_option(&provider, &option);
            let resume = terminal.resume_dashboard();
            match (result, resume) {
                (Ok(outcome), Ok(())) => {
                    app.set_notice(outcome.message);
                    app.retry_model_load(&provider);
                    ActionEffect {
                        refresh: true,
                        load_models: Some(provider),
                        ..ActionEffect::default()
                    }
                }
                (Err(error), Ok(())) => {
                    app.set_notice(format!("shell setup failed: {error:#}"));
                    ActionEffect::default()
                }
                (_, Err(error)) => {
                    app.set_notice(format!("failed to restore dashboard: {error:#}"));
                    ActionEffect::default()
                }
            }
        }
        AppAction::Hide { session_ids } => ActionEffect {
            hide_session_ids: session_ids,
            ..ActionEffect::default()
        },
        AppAction::Rename { session_id, name } => ActionEffect {
            refresh: true,
            session_alias: Some((session_id, name)),
            ..ActionEffect::default()
        },
        AppAction::Migrate { .. } => {
            unreachable!("migration actions are dispatched by the asynchronous worker")
        }
        AppAction::Launch {
            provider,
            model,
            prompt,
        } => {
            let known_session_ids = provider_session_ids(app, &provider);
            let result = control.launch_session(provider.clone(), model, prompt);
            match result {
                Ok(outcome) => {
                    app.set_notice(outcome.message);
                    ActionEffect {
                        refresh: true,
                        pending_launch: outcome.provider_session_hint.map(|provider_session_id| {
                            PendingLaunchIntent {
                                provider,
                                provider_session_id,
                                known_session_ids,
                            }
                        }),
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
        | AppAction::Authenticate { .. }
        | AppAction::SetupProvider { .. }
        | AppAction::SetupLaunchOption { .. }
        | AppAction::Migrate { .. }
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
        AppAction::Rename { .. } => unreachable!("rename actions are dispatched with effects"),
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
            app.set_notice(format!(
                "deleted {count} managed session{}",
                if count == 1 { "" } else { "s" }
            ));
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
        execute!(
            stdout,
            EnterAlternateScreen,
            PushKeyboardEnhancementFlags(
                KeyboardEnhancementFlags::DISAMBIGUATE_ESCAPE_CODES
                    | KeyboardEnhancementFlags::REPORT_EVENT_TYPES
            ),
            crossterm::cursor::Hide
        )?;
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
            PopKeyboardEnhancementFlags,
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
            PushKeyboardEnhancementFlags(
                KeyboardEnhancementFlags::DISAMBIGUATE_ESCAPE_CODES
                    | KeyboardEnhancementFlags::REPORT_EVENT_TYPES
            ),
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
            PopKeyboardEnhancementFlags,
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

    #[test]
    fn terminal_height_keeps_the_show_more_control_in_view() {
        assert_eq!(session_page_size_for_terminal(8), 1);
        assert_eq!(session_page_size_for_terminal(18), 7);
        assert_eq!(session_page_size_for_terminal(28), 17);
        assert_eq!(session_page_size_for_terminal(34), 23);
        assert_eq!(session_page_size_for_terminal(36), SESSION_PAGE_SIZE);
        assert_eq!(session_page_size_for_terminal(u16::MAX), SESSION_PAGE_SIZE);
    }

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

    fn modified_key(code: KeyCode, modifiers: KeyModifiers) -> KeyEvent {
        KeyEvent {
            code,
            modifiers,
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
    fn control_m_opens_the_two_step_migration_flow_without_replacing_enter() {
        let mut dashboard = app();
        assert_eq!(
            handle_key(&mut dashboard, control_key('m')),
            AppAction::None
        );
        assert_eq!(
            dashboard.overlay,
            Overlay::MigrationTargetPicker {
                session_id: "worker".into()
            }
        );
        handle_key(&mut dashboard, key(KeyCode::Down));
        assert_eq!(dashboard.migration_selection, 1);
        handle_key(&mut dashboard, key(KeyCode::PageDown));
        assert_eq!(dashboard.migration_selection, 11);
        handle_key(&mut dashboard, key(KeyCode::Enter));
        assert!(matches!(
            dashboard.overlay,
            Overlay::Composer(ComposerMode::MigrationName { .. })
        ));
        dashboard.input = "worker port".into();
        assert_eq!(
            handle_key(&mut dashboard, key(KeyCode::Enter)),
            AppAction::Migrate {
                session_id: "worker".into(),
                target: Provider::Grok,
                name: "worker port".into(),
            }
        );

        let mut normal_enter = app();
        assert_eq!(
            handle_key(&mut normal_enter, key(KeyCode::Enter)),
            AppAction::Open {
                session_id: "worker".into()
            }
        );
    }

    #[test]
    fn control_x_stops_active_then_deletes_after_the_row_becomes_idle() {
        let mut dashboard = app();
        dashboard.snapshot.sessions[0]
            .capabilities
            .insert(Capability::Interrupt);

        assert_eq!(
            handle_key(&mut dashboard, control_key('x')),
            AppAction::Interrupt {
                session_id: "worker".into()
            }
        );
        assert_eq!(dashboard.overlay, Overlay::None);

        dashboard.snapshot.sessions[0].state = SessionState::Completed;
        dashboard.snapshot.sessions[0]
            .capabilities
            .insert(Capability::Delete);
        assert_eq!(
            handle_key(&mut dashboard, control_key('x')),
            AppAction::Interrupt {
                session_id: "worker".into()
            }
        );
        dashboard.snapshot.sessions[0]
            .capabilities
            .remove(&Capability::Interrupt);
        assert_eq!(
            handle_key(&mut dashboard, control_key('x')),
            AppAction::Delete {
                session_ids: vec!["worker".into()]
            }
        );

        let mut locally_removed = app();
        locally_removed.snapshot.sessions[0].state = SessionState::Completed;
        assert_eq!(
            handle_key(&mut locally_removed, control_key('x')),
            AppAction::Hide {
                session_ids: vec!["worker".into()]
            }
        );

        dashboard.start_new_session(None);
        handle_key(&mut dashboard, control_key('x'));
        assert_eq!(
            dashboard.overlay,
            Overlay::Composer(ComposerMode::NewSession)
        );
    }

    #[test]
    fn control_x_stops_the_same_exact_session_from_peek() {
        let mut app = app();
        app.snapshot.sessions[0]
            .capabilities
            .insert(Capability::Interrupt);
        app.toggle_peek();

        assert_eq!(
            handle_key(&mut app, control_key('x')),
            AppAction::Interrupt {
                session_id: "worker".into()
            }
        );
        assert_eq!(app.overlay, Overlay::None);
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
    fn right_opens_the_selected_session_and_left_returns_from_peek() {
        let mut claude = app();
        assert_eq!(
            handle_key(&mut claude, key(KeyCode::Right)),
            AppAction::Open {
                session_id: "worker".into()
            }
        );
        assert_eq!(claude.overlay, Overlay::None);

        let mut managed = app();
        managed.snapshot.sessions[0].provider = Provider::Codex;
        managed.snapshot.sessions[0]
            .capabilities
            .insert(Capability::Reply);
        assert_eq!(
            handle_key(&mut managed, key(KeyCode::Right)),
            AppAction::Open {
                session_id: "worker".into()
            }
        );
        assert_eq!(managed.overlay, Overlay::None);

        handle_key(&mut managed, key(KeyCode::Char(' ')));
        assert_eq!(managed.overlay, Overlay::Peek);
        assert_eq!(
            handle_key(&mut managed, key(KeyCode::Left)),
            AppAction::None
        );
        assert_eq!(managed.overlay, Overlay::None);
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
    fn macos_and_shell_deletion_shortcuts_edit_the_active_field() {
        let mut app = app();
        app.start_rename();
        app.input = "greeting message content".into();

        handle_key(
            &mut app,
            modified_key(KeyCode::Backspace, KeyModifiers::ALT),
        );
        assert_eq!(app.input, "greeting message ");
        handle_key(&mut app, control_key('w'));
        assert_eq!(app.input, "greeting ");
        handle_key(
            &mut app,
            modified_key(KeyCode::Backspace, KeyModifiers::SUPER),
        );
        assert!(app.input.is_empty());

        app.start_new_session(None);
        app.input = "first line\nsecond line".into();
        handle_key(&mut app, control_key('u'));
        assert_eq!(app.input, "first line\n");
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
        assert_eq!(
            handle_key(&mut app, key(KeyCode::BackTab)),
            AppAction::LoadModels {
                provider: Provider::Pi
            }
        );
        app.set_available_models(
            Provider::Pi,
            Ok((0..25)
                .map(|index| format!("provider/model-{index:02}"))
                .collect()),
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

    #[test]
    fn model_picker_error_has_a_direct_retry_key() {
        let mut app = App::new(SessionSnapshot::default());
        app.require_authentication(
            Provider::Antigravity,
            None,
            "keep the task".into(),
            "agy models timed out".into(),
        );

        assert_eq!(
            handle_key(&mut app, control_key('r')),
            AppAction::LoadModels {
                provider: Provider::Antigravity
            }
        );
        assert!(app.models_loading);
        assert!(app.models_error.is_none());
        assert_eq!(app.input, "keep the task");
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

        fn authenticate_provider(&self, provider: &Provider) -> Result<ControlOutcome> {
            self.invoke("authenticate", provider.label().into())
        }

        fn setup_provider(&self, provider: &Provider) -> Result<ControlOutcome> {
            self.invoke("setup", provider.label().into())
        }

        fn setup_launch_option(&self, provider: &Provider, option: &str) -> Result<ControlOutcome> {
            self.invoke("setup-option", format!("{}:{option}", provider.label()))
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
    fn action_dispatch_handles_noop_quit_and_local_rename_without_provider_calls() {
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
        let effect = dispatch_action(
            &mut terminal,
            &mut app,
            AppAction::Rename {
                session_id: "worker".into(),
                name: "new".into(),
            },
            &control,
        );
        assert!(effect.refresh);
        assert_eq!(effect.session_alias, Some(("worker".into(), "new".into())));
        assert!(control.calls.lock().unwrap().is_empty());
    }

    #[test]
    fn local_rename_updates_the_row_without_touching_provider_control() {
        let directory = tempfile::tempdir().unwrap();
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            std::fs::set_permissions(directory.path(), std::fs::Permissions::from_mode(0o700))
                .unwrap();
        }
        let aliases = SessionAliases::load(directory.path().join("aliases.json")).unwrap();
        let mut app = app();

        assert_eq!(
            update_session_alias_from_app(&mut app, &aliases, "worker", "dashboard label").unwrap(),
            SessionAliasUpdate::Set
        );
        assert_eq!(app.snapshot.sessions[0].name, "dashboard label");
        assert_eq!(
            aliases.list()[0].provider_name_at_creation.as_deref(),
            Some("worker")
        );
        assert_eq!(
            update_session_alias_from_app(&mut app, &aliases, "worker", "").unwrap(),
            SessionAliasUpdate::Cleared
        );
        assert!(aliases.list().is_empty());
    }

    #[test]
    fn authentication_handoff_restores_the_dashboard_and_reloads_models() {
        let mut app = app();
        app.open_model_picker();
        app.set_available_models(Provider::Claude, Err("authentication required".into()));
        app.set_models_auth_available(&Provider::Claude, true);
        let mut terminal = FakeTerminal::default();
        let control = FakeControl::default();

        let effect = dispatch_action(
            &mut terminal,
            &mut app,
            AppAction::Authenticate {
                provider: Provider::Claude,
            },
            &control,
        );

        assert_eq!(terminal.calls, vec!["suspend", "resume"]);
        assert_eq!(effect.load_models, Some(Provider::Claude));
        assert!(app.models_loading);
        assert!(app.models_error.is_none());
        assert_eq!(
            control.calls.lock().unwrap().as_slice(),
            ["authenticate:Claude"]
        );

        let effect = dispatch_action(
            &mut terminal,
            &mut app,
            AppAction::SetupProvider {
                provider: Provider::GitHubCopilot,
            },
            &control,
        );
        assert!(effect.refresh);
        assert_eq!(
            terminal.calls,
            vec!["suspend", "resume", "suspend", "resume"]
        );
        assert_eq!(
            control.calls.lock().unwrap().as_slice(),
            ["authenticate:Claude", "setup:GitHub Copilot"]
        );

        let effect = dispatch_action(
            &mut terminal,
            &mut app,
            AppAction::SetupLaunchOption {
                provider: Provider::Terminal,
                option: "install-shell:fish".into(),
            },
            &control,
        );
        assert!(effect.refresh);
        assert_eq!(effect.load_models, Some(Provider::Terminal));
        assert_eq!(
            terminal.calls,
            vec!["suspend", "resume", "suspend", "resume", "suspend", "resume"]
        );
        assert_eq!(
            control.calls.lock().unwrap().as_slice(),
            [
                "authenticate:Claude",
                "setup:GitHub Copilot",
                "setup-option:Terminal:install-shell:fish"
            ]
        );
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
        assert_eq!(app.notice.as_deref(), Some("deleted 1 managed session"));
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
        let pending = effect.pending_launch.expect("launch hint");
        assert_eq!(pending.provider, Provider::Pi);
        assert_eq!(pending.provider_session_id, "provider-id");
        assert_eq!(*control.calls.lock().unwrap(), vec!["launch:build"]);
    }

    #[test]
    fn slow_launch_job_does_not_block_keyboard_state_changes() {
        let (sender, receiver) = mpsc::channel();
        schedule_launch_job(
            LaunchJob {
                sequence: 7,
                provider: Provider::Pi,
                model: None,
                prompt: "build".into(),
                open_when_visible: false,
                known_session_ids: BTreeSet::from(["pi:host:old".into()]),
            },
            sender,
            || {
                thread::sleep(Duration::from_millis(100));
                Ok(ControlOutcome {
                    message: "launched".into(),
                    provider_session_hint: Some("pi-id".into()),
                })
            },
        );

        assert!(receiver.recv_timeout(Duration::from_millis(10)).is_err());
        let mut app = app();
        assert_eq!(
            handle_key(&mut app, key(KeyCode::Char('x'))),
            AppAction::None
        );
        assert_eq!(app.overlay, Overlay::Composer(ComposerMode::NewSession));
        assert_eq!(app.input, "x");

        let completed = receiver.recv_timeout(Duration::from_secs(1)).unwrap();
        assert_eq!(completed.sequence, 7);
        assert_eq!(completed.provider, Provider::Pi);
        assert_eq!(completed.prompt, "build");
        assert!(!completed.open_when_visible);
        assert_eq!(
            completed.known_session_ids,
            BTreeSet::from(["pi:host:old".into()])
        );
        assert_eq!(
            completed.result.unwrap().provider_session_hint.as_deref(),
            Some("pi-id")
        );
    }

    #[test]
    fn pending_launch_selection_requires_both_provider_and_provider_session_id() {
        let mut app = app();
        let mut claude = app.snapshot.sessions[0].clone();
        claude.provider = Provider::Claude;
        claude.provider_session_id = "same".into();
        let mut pi = claude.clone();
        pi.id = "pi:host:same".into();
        pi.provider = Provider::Pi;
        app.replace_snapshot(SessionSnapshot {
            sessions: vec![claude, pi],
            warnings: Vec::new(),
        });
        let pending = PendingLaunch {
            provider: Provider::Pi,
            provider_session_id: "same".into(),
            known_session_ids: BTreeSet::new(),
            open_when_visible: false,
            deadline: Instant::now() + Duration::from_secs(1),
        };

        assert!(select_pending_launch(&mut app, Some(&pending)).is_some());
        assert_eq!(
            app.selection,
            Some(SelectionKey::Session("pi:host:same".into()))
        );
        let missing = PendingLaunch {
            provider: Provider::Codex,
            ..pending
        };
        assert!(select_pending_launch(&mut app, Some(&missing)).is_none());
    }

    #[test]
    fn pending_launch_selects_the_only_new_provider_row_when_native_id_changes() {
        let mut app = app();
        let mut old = app.snapshot.sessions[0].clone();
        old.id = "muse:host:old".into();
        old.provider_session_id = "old".into();
        old.provider = Provider::MuseCode;
        let mut launched = old.clone();
        launched.id = "muse:host:final".into();
        launched.provider_session_id = "final".into();
        app.replace_snapshot(SessionSnapshot {
            sessions: vec![old, launched],
            warnings: Vec::new(),
        });
        let pending = PendingLaunch {
            provider: Provider::MuseCode,
            provider_session_id: "provisional".into(),
            known_session_ids: BTreeSet::from(["muse:host:old".into()]),
            open_when_visible: false,
            deadline: Instant::now() + Duration::from_secs(1),
        };

        assert_eq!(
            select_pending_launch(&mut app, Some(&pending)).as_deref(),
            Some("muse:host:final")
        );
        assert_eq!(
            app.selection,
            Some(SelectionKey::Session("muse:host:final".into()))
        );

        let mut second = app.snapshot.sessions[1].clone();
        second.id = "muse:host:another".into();
        second.provider_session_id = "another".into();
        app.snapshot.sessions.push(second);
        app.replace_snapshot(app.snapshot.clone());
        assert!(select_pending_launch(&mut app, Some(&pending)).is_none());
    }

    #[test]
    fn pending_launch_refresh_temporarily_queries_hidden_result_classes() {
        let request = DiscoveryRequest::default();
        let pending = PendingLaunch {
            provider: Provider::Codex,
            provider_session_id: "exact".into(),
            known_session_ids: BTreeSet::new(),
            open_when_visible: false,
            deadline: Instant::now() + Duration::from_secs(1),
        };

        let regular = discovery_request_for_pending_launch(&request, None);
        let resolving = discovery_request_for_pending_launch(&request, Some(&pending));

        assert!(!regular.include_completed);
        assert!(!regular.include_interactive);
        assert!(resolving.include_completed);
        assert!(resolving.include_interactive);
        assert_eq!(resolving.include_external, request.include_external);
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

        let hide = dispatch_action(
            &mut terminal,
            &mut app,
            AppAction::Hide {
                session_ids: vec!["worker".into()],
            },
            &control,
        );
        assert_eq!(hide.hide_session_ids, vec!["worker"]);
        assert!(!hide.refresh);
    }

    #[test]
    fn local_hide_is_persisted_and_removes_the_row_without_provider_calls() {
        let directory = tempfile::tempdir().unwrap();
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            std::fs::set_permissions(directory.path(), std::fs::Permissions::from_mode(0o700))
                .unwrap();
        }
        let hidden = HiddenSessions::load(directory.path().join("hidden.json")).unwrap();
        let mut app = app();

        assert_eq!(
            hide_sessions_from_app(&mut app, &hidden, &["worker".into()]).unwrap(),
            1
        );
        assert!(app.snapshot.sessions.is_empty());
        assert!(hidden.contains("worker"));
        assert!(HiddenSessions::load(directory.path().join("hidden.json"))
            .unwrap()
            .contains("worker"));
    }

    #[test]
    fn refresh_queue_carries_the_latest_discovery_request() {
        let (sender, receiver) = mpsc::sync_channel(1);
        let mut in_flight = false;
        let mut request = DiscoveryRequest {
            include_completed: false,
            include_interactive: false,
            cwd: None,
            ..DiscoveryRequest::default()
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

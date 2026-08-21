#![cfg(target_os = "linux")]

use std::path::Path;
use std::sync::Arc;
use std::thread;
use std::time::{Duration, Instant};

use anyhow::{bail, Context, Result};
use open_agent_view::adapters::{CodexSource, DiscoveryRequest, PiSource, SessionSource};
use open_agent_view::codex_supervisor::CodexSupervisor;
use open_agent_view::domain::{AgentSession, SessionSnapshot, SessionState};
use open_agent_view::pi_supervisor::PiSupervisor;
use tempfile::tempdir;

const PROVIDER_TIMEOUT: Duration = Duration::from_secs(120);

#[test]
#[ignore = "set OAV_REAL_CODEX_BIN to run a credentialed, cleanup-safe Codex lifecycle"]
fn real_codex_launch_interrupt_delete_and_server_cleanup() -> Result<()> {
    let executable = std::env::var("OAV_REAL_CODEX_BIN")
        .context("OAV_REAL_CODEX_BIN must point to an authenticated Codex executable")?;
    let directory = tempdir()?;
    let supervisor = Arc::new(CodexSupervisor::with_isolated_state_dir(
        executable,
        directory.path().join("codex-state"),
    )?);
    let mut cleanup = CodexCleanup::new(supervisor.clone());
    let source = CodexSource::managed_owned(supervisor.clone());

    let thread_id = supervisor.launch(
        "Do not use tools or modify files. Reply exactly OAV-CODEX-SMOKE.",
        Path::new("/tmp"),
    )?;
    cleanup.thread_id = Some(thread_id.clone());
    let completed = wait_for_codex(
        &source,
        &supervisor,
        &thread_id,
        PROVIDER_TIMEOUT,
        true,
        |session| session.state == SessionState::Completed,
    )?;
    let transcript = supervisor.inspect(&completed)?;
    assert!(
        transcript.contains("OAV-CODEX-SMOKE"),
        "Codex transcript omitted the smoke marker: {transcript}"
    );

    supervisor.reply(
        &completed,
        "Run the shell command `sleep 30`. Do not do anything else.",
    )?;
    let active = wait_for_codex(
        &source,
        &supervisor,
        &thread_id,
        PROVIDER_TIMEOUT,
        true,
        |session| {
            matches!(
                session.state,
                SessionState::Working | SessionState::NeedsInput
            )
        },
    )?;
    if active.state == SessionState::NeedsInput {
        let request = supervisor.inspect(&active)?;
        assert!(
            request.contains("sleep 30"),
            "Codex requested unexpected approval during smoke test: {request}"
        );
        supervisor.respond_approval(&active, true)?;
    }
    let interruptible = wait_for_codex(
        &source,
        &supervisor,
        &thread_id,
        PROVIDER_TIMEOUT,
        false,
        |session| session.state == SessionState::Working,
    )?;
    let interrupt_result = supervisor.interrupt(&interruptible);
    if let Err(error) = &interrupt_result {
        let message = format!("{error:#}");
        if !message.contains("App Server turn/interrupt failed")
            || !message.contains("no active turn to interrupt")
        {
            return Err(anyhow::Error::msg(message));
        }
    }
    // A very fast provider can complete after the exact active turn is
    // observed but before it processes turn/interrupt. Accept only Codex's
    // precise no-active-turn response, and only after the same owned task
    // reaches authoritative terminal state.
    let idle = wait_for_codex(
        &source,
        &supervisor,
        &thread_id,
        PROVIDER_TIMEOUT,
        true,
        |session| session.state == SessionState::Completed,
    )?;
    supervisor.delete(&idle)?;
    cleanup.thread_id = None;
    assert!(discover_codex(&source, &supervisor)?.is_empty());

    supervisor.shutdown_server()?;
    cleanup.server_stopped = true;
    Ok(())
}

#[test]
#[ignore = "set OAV_REAL_PI_BIN to run a credentialed, isolated Pi lifecycle"]
fn real_pi_launch_interrupt_and_daemon_cleanup() -> Result<()> {
    let executable = std::env::var("OAV_REAL_PI_BIN")
        .context("OAV_REAL_PI_BIN must point to an authenticated Pi executable")?;
    let directory = tempdir()?;
    let supervisor = Arc::new(PiSupervisor::with_state_dir_and_exe(
        executable,
        directory.path().join("pi-state"),
        Path::new(env!("CARGO_BIN_EXE_open-agent-view")).to_path_buf(),
    )?);
    let mut cleanup = PiCleanup::new(supervisor.clone());
    let source = PiSource::managed(
        directory.path().join("external-history"),
        supervisor.clone(),
    );

    let launched = supervisor.launch(
        "Do not use tools or modify files. Reply exactly OAV-PI-SMOKE.",
        Path::new("/tmp"),
    )?;
    cleanup.provider_pid = Some(launched.pid);
    let completed = wait_for_pi(&source, &supervisor, &launched.id, |session| {
        session.state == SessionState::Completed
    })?;
    let transcript = supervisor.inspect(&completed.provider_session_id)?;
    assert!(
        transcript.contains("Assistant: OAV-PI-SMOKE"),
        "Pi transcript omitted the exact assistant smoke marker: {transcript}"
    );

    supervisor.reply(
        &completed.provider_session_id,
        "Without tools, write the integers 1 through 200, one per line.",
    )?;
    let working = wait_for_pi(&source, &supervisor, &launched.id, |session| {
        session.state == SessionState::Working
    })?;
    supervisor.interrupt(&working.provider_session_id)?;
    wait_for_pi(&source, &supervisor, &launched.id, |session| {
        session.state == SessionState::Completed
    })?;

    supervisor.shutdown_daemon()?;
    cleanup.daemon_stopped = true;
    wait_for_pid_exit(launched.pid, Duration::from_secs(5))?;
    Ok(())
}

struct CodexCleanup {
    supervisor: Arc<CodexSupervisor>,
    thread_id: Option<String>,
    server_stopped: bool,
}

impl CodexCleanup {
    fn new(supervisor: Arc<CodexSupervisor>) -> Self {
        Self {
            supervisor,
            thread_id: None,
            server_stopped: false,
        }
    }
}

impl Drop for CodexCleanup {
    fn drop(&mut self) {
        if let Some(thread_id) = self.thread_id.take() {
            let source = CodexSource::managed_owned(self.supervisor.clone());
            if let Ok(Some(session)) = discover_codex(&source, &self.supervisor).map(|sessions| {
                sessions
                    .into_iter()
                    .find(|session| session.provider_session_id == thread_id)
            }) {
                if session.state != SessionState::Completed {
                    let _ = self.supervisor.interrupt(&session);
                }
                if let Ok(idle) = wait_for_codex(
                    &source,
                    &self.supervisor,
                    &thread_id,
                    Duration::from_secs(5),
                    false,
                    |candidate| candidate.state == SessionState::Completed,
                ) {
                    let _ = self.supervisor.delete(&idle);
                }
            }
        }
        if !self.server_stopped {
            let _ = self.supervisor.shutdown_server();
        }
    }
}

struct PiCleanup {
    supervisor: Arc<PiSupervisor>,
    provider_pid: Option<u32>,
    daemon_stopped: bool,
}

impl PiCleanup {
    fn new(supervisor: Arc<PiSupervisor>) -> Self {
        Self {
            supervisor,
            provider_pid: None,
            daemon_stopped: false,
        }
    }
}

impl Drop for PiCleanup {
    fn drop(&mut self) {
        if !self.daemon_stopped {
            let _ = self.supervisor.shutdown_daemon();
        }
        if let Some(pid) = self.provider_pid {
            let _ = wait_for_pid_exit(pid, Duration::from_secs(5));
        }
    }
}

fn discover_codex(source: &CodexSource, supervisor: &CodexSupervisor) -> Result<Vec<AgentSession>> {
    let sessions = source.discover(&DiscoveryRequest {
        include_completed: true,
        ..DiscoveryRequest::default()
    })?;
    let mut snapshot = SessionSnapshot {
        sessions,
        warnings: Vec::new(),
    };
    supervisor.enrich(&mut snapshot);
    if !snapshot.warnings.is_empty() {
        bail!("Codex lifecycle warnings: {:?}", snapshot.warnings);
    }
    Ok(snapshot.sessions)
}

fn wait_for_codex(
    source: &CodexSource,
    supervisor: &CodexSupervisor,
    thread_id: &str,
    timeout: Duration,
    needs_input_is_error: bool,
    predicate: impl Fn(&AgentSession) -> bool,
) -> Result<AgentSession> {
    let deadline = Instant::now() + timeout;
    loop {
        let session = discover_codex(source, supervisor)?
            .into_iter()
            .find(|session| session.provider_session_id == thread_id)
            .with_context(|| format!("managed Codex thread {thread_id} disappeared"))?;
        if predicate(&session) {
            return Ok(session);
        }
        if needs_input_is_error && session.state == SessionState::NeedsInput {
            bail!(
                "managed Codex smoke unexpectedly needs input: {}",
                session.summary
            );
        }
        if Instant::now() >= deadline {
            bail!(
                "timed out waiting for managed Codex thread {thread_id}; last state {:?}: {}",
                session.state,
                session.summary
            );
        }
        thread::sleep(Duration::from_millis(100));
    }
}

fn discover_pi(source: &PiSource, supervisor: &PiSupervisor) -> Result<Vec<AgentSession>> {
    let sessions = source.discover(&DiscoveryRequest {
        include_completed: true,
        ..DiscoveryRequest::default()
    })?;
    let managed = supervisor.list()?;
    let managed_by_id = managed
        .into_iter()
        .map(|session| (session.id.clone(), session))
        .collect::<std::collections::BTreeMap<_, _>>();
    Ok(sessions
        .into_iter()
        .map(|mut session| {
            if let Some(owned) = managed_by_id.get(&session.provider_session_id) {
                session.state = owned.state;
                session.summary.clone_from(&owned.summary);
                session.pid = Some(owned.pid);
            }
            session
        })
        .collect())
}

fn wait_for_pi(
    source: &PiSource,
    supervisor: &PiSupervisor,
    session_id: &str,
    predicate: impl Fn(&AgentSession) -> bool,
) -> Result<AgentSession> {
    let deadline = Instant::now() + PROVIDER_TIMEOUT;
    loop {
        let session = discover_pi(source, supervisor)?
            .into_iter()
            .find(|session| session.provider_session_id == session_id)
            .with_context(|| format!("managed Pi session {session_id} disappeared"))?;
        if predicate(&session) {
            return Ok(session);
        }
        if session.state == SessionState::NeedsInput {
            bail!(
                "managed Pi smoke unexpectedly needs input: {}",
                session.summary
            );
        }
        if Instant::now() >= deadline {
            bail!(
                "timed out waiting for managed Pi session {session_id}; last state {:?}: {}",
                session.state,
                session.summary
            );
        }
        thread::sleep(Duration::from_millis(100));
    }
}

fn wait_for_pid_exit(pid: u32, timeout: Duration) -> Result<()> {
    let deadline = Instant::now() + timeout;
    let path = format!("/proc/{pid}");
    while Path::new(&path).exists() {
        if Instant::now() >= deadline {
            bail!("provider process {pid} remained after isolated supervisor shutdown");
        }
        thread::sleep(Duration::from_millis(20));
    }
    Ok(())
}

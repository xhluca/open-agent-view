use std::collections::{BTreeMap, BTreeSet};
use std::fs::{self, File, OpenOptions};
use std::io::{Read, Seek, SeekFrom, Write};
use std::os::unix::fs::{OpenOptionsExt, PermissionsExt};
use std::os::unix::process::CommandExt;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::sync::{Arc, Mutex};
use std::thread;
use std::time::{Duration, Instant, SystemTime};

use anyhow::{bail, Context, Result};
use serde::{Deserialize, Serialize};

use super::cursor::{
    parse_cursor_chat_id, parse_cursor_stream_event, CursorInvocation, CursorStreamEvent,
};
use super::{DiscoveryRequest, SessionSource};
use crate::domain::{
    AgentSession, Capability, Provider, Runtime, SessionKind, SessionSnapshot, SessionState,
};
use crate::process::{CommandRunner, ProcessRunner};

const REGISTRY_VERSION: u32 = 1;
const MODEL_PREFLIGHT_TIMEOUT: Duration = Duration::from_secs(4);
const CREATE_TIMEOUT: Duration = Duration::from_secs(8);
const IDENTITY_TIMEOUT: Duration = Duration::from_secs(3);
const MAX_LOG_BYTES: u64 = 4 * 1024 * 1024;
const MAX_TRANSCRIPT_CHARS: usize = 32 * 1024;
const EXECUTABLE_BUSY_RETRIES: usize = 5;

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
struct CursorProcessIdentity {
    pid: u32,
    start_token: String,
    cmdline: Vec<u8>,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
struct OwnedCursorSession {
    session_id: String,
    cwd: PathBuf,
    name: String,
    created_at_ms: u64,
    updated_at_ms: u64,
    stdout_path: PathBuf,
    stderr_path: PathBuf,
    #[serde(default)]
    process: Option<CursorProcessIdentity>,
    #[serde(default)]
    model: Option<String>,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
struct CursorRegistry {
    version: u32,
    sessions: BTreeMap<String, OwnedCursorSession>,
}

impl Default for CursorRegistry {
    fn default() -> Self {
        Self {
            version: REGISTRY_VERSION,
            sessions: BTreeMap::new(),
        }
    }
}

#[derive(Default)]
struct CursorLogState {
    transcript: String,
    summary: String,
    terminal_result: Option<bool>,
}

/// Owns exact Cursor print-mode processes launched by Open Agent View.
///
/// The provider session remains Cursor's source of conversation truth. This
/// supervisor persists only ownership, process identity, and bounded logs.
pub struct CursorSupervisor {
    executable: String,
    state_dir: PathBuf,
    registry_path: PathBuf,
    lock_path: PathBuf,
    invocation: CursorInvocation,
    runner: Arc<dyn CommandRunner>,
    /// Initial foreground prompts are process-local and are never written to
    /// the ownership registry or provider logs by OAV.
    pending_native: Mutex<BTreeMap<String, (String, Option<String>)>>,
}

impl CursorSupervisor {
    pub fn host(executable: impl Into<String>) -> Result<Self> {
        Self::with_state_dir(executable, default_cursor_state_dir()?)
    }

    pub fn with_state_dir(executable: impl Into<String>, state_dir: PathBuf) -> Result<Self> {
        ensure_private_directory(&state_dir)?;
        let logs = state_dir.join("logs");
        ensure_private_directory(&logs)?;
        let executable = executable.into();
        Ok(Self {
            invocation: CursorInvocation::host(executable.clone()),
            executable,
            registry_path: state_dir.join("sessions.json"),
            lock_path: state_dir.join("sessions.lock"),
            state_dir,
            runner: Arc::new(ProcessRunner),
            pending_native: Mutex::new(BTreeMap::new()),
        })
    }

    pub fn executable(&self) -> &str {
        &self.executable
    }

    pub fn launch(&self, prompt: &str, cwd: &Path) -> Result<String> {
        self.launch_with_model(prompt, cwd, None)
    }

    pub fn launch_with_model(
        &self,
        prompt: &str,
        cwd: &Path,
        model: Option<&str>,
    ) -> Result<String> {
        require_process_identity_support()?;
        let prompt = prompt.trim();
        if prompt.is_empty() {
            bail!("the Cursor launch prompt cannot be empty");
        }
        require_absolute_workspace(cwd)?;
        let available_models = self.available_models()?;
        if let Some(model) = model {
            if !available_models.iter().any(|candidate| candidate == model) {
                bail!("Cursor model `{model}` is not available to the authenticated account");
            }
        }
        let spec = self.invocation.create_chat_with_model(cwd, model)?;
        let mut request = crate::process::CommandRequest::new(spec.program, spec.args);
        request.current_dir = Some(spec.current_dir);
        request.timeout = CREATE_TIMEOUT;
        let output = self.runner.run_until_stdout_line(&request).with_context(|| {
            format!(
                "Cursor create-chat did not complete; run `cursor-agent login` and `cursor-agent models` before retrying (configured executable: {})",
                self.executable
            )
        })?;
        if output.status != 0 {
            bail!(
                "Cursor create-chat exited with status {}: {}",
                output.status,
                output.stderr_lossy()
            );
        }
        let session_id = parse_cursor_chat_id(output.stdout_text()?)?;
        self.spawn_turn(&session_id, cwd, prompt, model, true)?;
        Ok(session_id)
    }

    /// Allocate and persist an exact Cursor chat without starting a detached
    /// print-mode worker. The controller immediately resumes this ID in the
    /// foreground native interface.
    pub fn allocate_chat_with_model(
        &self,
        prompt: &str,
        cwd: &Path,
        model: Option<&str>,
    ) -> Result<String> {
        require_process_identity_support()?;
        let prompt = prompt.trim();
        if prompt.is_empty() {
            bail!("the Cursor launch prompt cannot be empty");
        }
        require_absolute_workspace(cwd)?;
        let available_models = self.available_models()?;
        if let Some(model) = model {
            if !available_models.iter().any(|candidate| candidate == model) {
                bail!("Cursor model `{model}` is not available to the authenticated account");
            }
        }
        let spec = self.invocation.create_chat_with_model(cwd, model)?;
        let mut request = crate::process::CommandRequest::new(spec.program, spec.args);
        request.current_dir = Some(spec.current_dir);
        request.timeout = CREATE_TIMEOUT;
        let output = self.runner.run_until_stdout_line(&request).with_context(|| {
            format!(
                "Cursor create-chat did not complete; sign in before retrying (configured executable: {})",
                self.executable
            )
        })?;
        if output.status != 0 {
            bail!(
                "Cursor create-chat exited with status {}: {}",
                output.status,
                output.stderr_lossy()
            );
        }
        let session_id = parse_cursor_chat_id(output.stdout_text()?)?;
        let stdout_path = self
            .state_dir
            .join("logs")
            .join(format!("{session_id}.ndjson"));
        let stderr_path = self
            .state_dir
            .join("logs")
            .join(format!("{session_id}.stderr.log"));
        drop(private_append_file(&stdout_path)?);
        drop(private_append_file(&stderr_path)?);
        let now = now_millis();
        let record = OwnedCursorSession {
            session_id: session_id.clone(),
            cwd: cwd.to_owned(),
            name: prompt_name(prompt),
            created_at_ms: now,
            updated_at_ms: now,
            stdout_path,
            stderr_path,
            process: None,
            model: model.map(str::to_owned),
        };
        self.with_locked_registry(|registry| {
            if registry.sessions.contains_key(&session_id) {
                bail!("Cursor create-chat returned an already-owned session ID");
            }
            registry.sessions.insert(session_id.clone(), record);
            Ok(())
        })?;
        self.pending_native
            .lock()
            .map_err(|_| anyhow::anyhow!("Cursor pending-launch lock was poisoned"))?
            .insert(
                session_id.clone(),
                (prompt.to_owned(), model.map(str::to_owned)),
            );
        Ok(session_id)
    }

    pub fn mark_native_opened(&self, session_id: &str) -> Result<()> {
        require_safe_session_id(session_id)?;
        self.with_locked_registry(|registry| {
            let record = registry
                .sessions
                .get_mut(session_id)
                .with_context(|| format!("Cursor session {session_id} is not owned"))?;
            record.updated_at_ms = now_millis();
            Ok(())
        })?;
        self.pending_native
            .lock()
            .map_err(|_| anyhow::anyhow!("Cursor pending-launch lock was poisoned"))?
            .remove(session_id);
        Ok(())
    }

    pub fn pending_native_launch(
        &self,
        session: &AgentSession,
    ) -> Result<Option<(String, Option<String>)>> {
        self.require_owned_host(session)?;
        Ok(self
            .pending_native
            .lock()
            .map_err(|_| anyhow::anyhow!("Cursor pending-launch lock was poisoned"))?
            .get(&session.provider_session_id)
            .cloned())
    }

    pub fn available_models(&self) -> Result<Vec<String>> {
        let mut request =
            crate::process::CommandRequest::new(self.executable.clone(), vec!["models".into()]);
        request.timeout = MODEL_PREFLIGHT_TIMEOUT;
        let output = self.run_command(&request).with_context(|| {
            format!(
                "Cursor model catalog did not respond (configured executable: {})",
                self.executable
            )
        })?;
        let stdout = output.stdout_text()?;
        let stderr = output.stderr_lossy();
        if output.status != 0 {
            let detail = format!("{stdout}\n{stderr}").to_ascii_lowercase();
            if detail.contains("auth")
                || detail.contains("login")
                || detail.contains("no models available")
            {
                bail!(
                    "Cursor is not authenticated or this account has no models; press Enter to sign in"
                );
            }
            bail!(
                "Cursor model catalog failed with status {}: {}",
                output.status,
                stderr
            );
        }
        if stdout.contains("No models available for this account") {
            bail!(
                "Cursor is not authenticated or this account has no models; press Enter to sign in"
            );
        }
        let models = parse_cursor_models(stdout);
        if models.is_empty() {
            bail!("Cursor returned no account models; press Enter to sign in or check plan access");
        }
        Ok(models)
    }

    pub fn reply(&self, session: &AgentSession, prompt: &str) -> Result<()> {
        self.require_owned_host(session)?;
        let prompt = prompt.trim();
        if prompt.is_empty() {
            bail!("the Cursor reply cannot be empty");
        }
        let record = self.lookup(&session.provider_session_id)?;
        if self
            .pending_native
            .lock()
            .map_err(|_| anyhow::anyhow!("Cursor pending-launch lock was poisoned"))?
            .contains_key(&session.provider_session_id)
        {
            bail!("open the new Cursor session before sending an inline reply");
        }
        if record
            .process
            .as_ref()
            .map(verify_process)
            .transpose()?
            .unwrap_or(false)
        {
            bail!("Cursor session is still working and has no documented live-steer API");
        }
        self.spawn_turn(
            &record.session_id,
            &record.cwd,
            prompt,
            record.model.as_deref(),
            false,
        )
    }

    pub fn interrupt(&self, session: &AgentSession) -> Result<()> {
        self.require_owned_host(session)?;
        let record = self.lookup(&session.provider_session_id)?;
        let process = record
            .process
            .as_ref()
            .context("Cursor session has no active owned process to interrupt")?;
        signal_verified_process(process, libc::SIGINT)
            .context("failed to interrupt the exact owned Cursor process")
    }

    pub fn inspect(&self, session: &AgentSession) -> Result<String> {
        self.require_owned_host(session)?;
        let record = self.lookup(&session.provider_session_id)?;
        let state = read_cursor_log(&record.stdout_path)?;
        if state.transcript.trim().is_empty() {
            let stderr = read_bounded_text(&record.stderr_path, 8 * 1024)?;
            if stderr.trim().is_empty() {
                return Ok("No Cursor output is available yet.".into());
            }
            return Ok(stderr);
        }
        Ok(state.transcript)
    }

    pub fn owns(&self, session: &AgentSession) -> bool {
        session.provider == Provider::Cursor
            && session.runtime == Runtime::Host
            && self.lookup(&session.provider_session_id).is_ok()
    }

    pub fn is_running(&self, session: &AgentSession) -> Result<bool> {
        self.require_owned_host(session)?;
        let record = self.lookup(&session.provider_session_id)?;
        record
            .process
            .as_ref()
            .map(verify_process)
            .transpose()
            .map(|running| running.unwrap_or(false))
    }

    pub fn discover(&self, request: &DiscoveryRequest) -> Result<Vec<AgentSession>> {
        let registry = self.with_locked_registry(|registry| Ok(registry.clone()))?;
        let pending_native = self
            .pending_native
            .lock()
            .map_err(|_| anyhow::anyhow!("Cursor pending-launch lock was poisoned"))?
            .keys()
            .cloned()
            .collect::<BTreeSet<_>>();
        let mut sessions = Vec::with_capacity(registry.sessions.len());
        for record in registry.sessions.into_values() {
            let running = record
                .process
                .as_ref()
                .map(verify_process)
                .transpose()?
                .unwrap_or(false);
            let native_backgrounded = crate::native_session::is_backgrounded(&format!(
                "cursor:host:{}",
                record.session_id
            ));
            let log = read_cursor_log(&record.stdout_path)?;
            let (state, raw_state) = if native_backgrounded {
                (SessionState::NeedsInput, "native_backgrounded")
            } else if running {
                (SessionState::Working, "working")
            } else if record.process.is_none() {
                (SessionState::Completed, "native_idle")
            } else {
                match log.terminal_result {
                    Some(false) => (SessionState::Completed, "completed"),
                    Some(true) => (SessionState::NeedsInput, "failed"),
                    None => (SessionState::NeedsInput, "exited_without_result"),
                }
            };
            if !request.include_completed && state == SessionState::Completed {
                continue;
            }
            if request
                .cwd
                .as_ref()
                .map(|cwd| !record.cwd.starts_with(cwd))
                .unwrap_or(false)
            {
                continue;
            }
            let mut capabilities = BTreeSet::from([Capability::Inspect]);
            if native_backgrounded {
                // Enter/Right resumes the exact retained native PTY. Inline
                // reply would create a concurrent provider frontend.
            } else if running {
                capabilities.insert(Capability::Interrupt);
            } else if !pending_native.contains(&record.session_id) {
                capabilities.insert(Capability::Reply);
            }
            sessions.push(AgentSession {
                id: format!("cursor:host:{}", record.session_id),
                provider_session_id: record.session_id,
                provider: Provider::Cursor,
                runtime: Runtime::Host,
                kind: SessionKind::Managed,
                name: record.name,
                cwd: record.cwd,
                state,
                summary: if native_backgrounded {
                    "Cursor native session is backgrounded".into()
                } else if log.summary.is_empty() {
                    match state {
                        SessionState::Working => "Cursor agent is working".into(),
                        SessionState::Completed if record.process.is_none() => {
                            "Cursor native session is ready".into()
                        }
                        SessionState::NeedsInput => "Cursor run exited without success".into(),
                        _ => "Cursor run completed".into(),
                    }
                } else {
                    log.summary
                },
                raw_state: Some(raw_state.into()),
                pid: running
                    .then(|| record.process.as_ref().map(|process| process.pid))
                    .flatten(),
                started_at: millis_to_time(record.created_at_ms),
                updated_at: file_updated_at(&record.stdout_path)
                    .or_else(|| millis_to_time(record.updated_at_ms)),
                pull_requests: None,
                capabilities,
            });
        }
        Ok(sessions)
    }

    pub fn enrich(&self, snapshot: &mut SessionSnapshot) {
        // Managed Cursor sessions are authoritative from CursorSource. This
        // pass only prevents a fixture or another source from gaining control
        // merely by reusing an owned ID with a different runtime/cwd.
        for session in &mut snapshot.sessions {
            if session.provider != Provider::Cursor
                || session.runtime != Runtime::Host
                || session.kind == SessionKind::Managed
            {
                continue;
            }
            session.capabilities.clear();
        }
    }

    fn spawn_turn(
        &self,
        session_id: &str,
        cwd: &Path,
        prompt: &str,
        model: Option<&str>,
        new: bool,
    ) -> Result<()> {
        require_safe_session_id(session_id)?;
        require_absolute_workspace(cwd)?;
        let existing =
            self.with_locked_registry(|registry| Ok(registry.sessions.get(session_id).cloned()))?;
        if let Some(record) = existing.as_ref() {
            if record
                .process
                .as_ref()
                .map(verify_process)
                .transpose()?
                .unwrap_or(false)
            {
                bail!("Cursor session {session_id} already has an active owned process");
            }
            if new {
                bail!("Cursor create-chat returned an already-owned session ID");
            }
            if record.cwd != cwd {
                bail!("Cursor session workspace changed from its owned record");
            }
        }

        let stdout_path = self
            .state_dir
            .join("logs")
            .join(format!("{session_id}.ndjson"));
        let stderr_path = self
            .state_dir
            .join("logs")
            .join(format!("{session_id}.stderr.log"));
        let stdout = private_append_file(&stdout_path)?;
        let stderr = private_append_file(&stderr_path)?;
        let spec = self.invocation.print_turn(session_id, cwd, prompt, model)?;
        let mut command = spec.command();
        command
            .stdin(Stdio::null())
            .stdout(Stdio::from(stdout))
            .stderr(Stdio::from(stderr));
        unsafe {
            command.pre_exec(|| {
                if libc::setsid() < 0 {
                    return Err(std::io::Error::last_os_error());
                }
                Ok(())
            });
        }
        let mut child =
            spawn_with_busy_retry(&mut command).context("failed to launch managed Cursor turn")?;
        let process = match wait_for_identity(child.id(), IDENTITY_TIMEOUT) {
            Ok(identity) => identity,
            Err(error) => {
                let _ = child.kill();
                let _ = child.wait();
                return Err(error).context("Cursor child did not establish a verifiable identity");
            }
        };
        let now = now_millis();
        let name = existing
            .as_ref()
            .map(|record| record.name.clone())
            .unwrap_or_else(|| prompt_name(prompt));
        let created_at_ms = existing
            .as_ref()
            .map(|record| record.created_at_ms)
            .unwrap_or(now);
        let record = OwnedCursorSession {
            session_id: session_id.into(),
            cwd: cwd.to_owned(),
            name,
            created_at_ms,
            updated_at_ms: now,
            stdout_path,
            stderr_path,
            process: Some(process),
            model: model
                .map(str::to_owned)
                .or_else(|| existing.as_ref().and_then(|record| record.model.clone())),
        };
        let process_for_cleanup = record.process.clone();
        if let Err(error) = self.with_locked_registry(|registry| {
            registry.sessions.insert(session_id.into(), record);
            Ok(())
        }) {
            if process_for_cleanup
                .as_ref()
                .map(verify_process)
                .transpose()
                .unwrap_or_default()
                .unwrap_or(false)
            {
                let _ = child.kill();
            }
            let _ = child.wait();
            return Err(error);
        }
        thread::spawn(move || {
            let _ = child.wait();
        });
        Ok(())
    }

    fn lookup(&self, session_id: &str) -> Result<OwnedCursorSession> {
        require_safe_session_id(session_id)?;
        self.with_locked_registry(|registry| {
            registry
                .sessions
                .get(session_id)
                .cloned()
                .with_context(|| format!("Cursor session {session_id} is not owned"))
        })
    }

    fn run_command(
        &self,
        request: &crate::process::CommandRequest,
    ) -> Result<crate::process::CommandOutput> {
        let mut attempts = 0;
        loop {
            match self.runner.run(request) {
                Ok(output) => return Ok(output),
                Err(error)
                    if error_has_errno(&error, libc::ETXTBSY)
                        && attempts < EXECUTABLE_BUSY_RETRIES =>
                {
                    attempts += 1;
                    thread::sleep(Duration::from_millis(10));
                }
                Err(error) => return Err(error),
            }
        }
    }

    fn require_owned_host(&self, session: &AgentSession) -> Result<()> {
        if session.provider != Provider::Cursor || session.runtime != Runtime::Host {
            bail!("the managed Cursor controller does not own this provider/runtime");
        }
        let record = self.lookup(&session.provider_session_id)?;
        if record.cwd != session.cwd {
            bail!("the Cursor session workspace does not match its ownership record");
        }
        Ok(())
    }

    fn with_locked_registry<T>(
        &self,
        operation: impl FnOnce(&mut CursorRegistry) -> Result<T>,
    ) -> Result<T> {
        let lock = private_lock_file(&self.lock_path)?;
        flock_exclusive(&lock)?;
        let mut registry = read_registry(&self.registry_path)?;
        let before = registry.clone();
        let result = operation(&mut registry);
        if result.is_ok() && registry != before {
            write_registry(&self.registry_path, &registry)?;
        }
        let unlock_result = flock_unlock(&lock);
        match (result, unlock_result) {
            (Ok(value), Ok(())) => Ok(value),
            (Err(error), _) => Err(error),
            (Ok(_), Err(error)) => Err(error),
        }
    }
}

pub struct CursorSource {
    supervisor: Arc<CursorSupervisor>,
}

impl CursorSource {
    pub fn managed(supervisor: Arc<CursorSupervisor>) -> Self {
        Self { supervisor }
    }
}

impl SessionSource for CursorSource {
    fn label(&self) -> &str {
        "Cursor (managed host)"
    }

    fn discover(&self, request: &DiscoveryRequest) -> Result<Vec<AgentSession>> {
        self.supervisor.discover(request)
    }
}

pub fn default_cursor_state_dir() -> Result<PathBuf> {
    if let Some(state_home) = std::env::var_os("XDG_STATE_HOME") {
        return Ok(PathBuf::from(state_home).join("open-agent-view/cursor"));
    }
    let home = std::env::var_os("HOME").context("HOME is not set")?;
    Ok(PathBuf::from(home).join(".local/state/open-agent-view/cursor"))
}

fn read_cursor_log(path: &Path) -> Result<CursorLogState> {
    let input = read_bounded_text(path, MAX_LOG_BYTES)?;
    let mut state = CursorLogState::default();
    for line in input.lines() {
        let event = match parse_cursor_stream_event(line) {
            Ok(event) => event,
            Err(_) => continue,
        };
        match event {
            CursorStreamEvent::AssistantText { text, .. } => {
                state.transcript.push_str(&text);
                state.summary = last_nonempty_line(&text).unwrap_or_default();
            }
            CursorStreamEvent::Finished {
                result, is_error, ..
            } => {
                state.terminal_result = Some(is_error);
                if !result.trim().is_empty() {
                    state.summary = last_nonempty_line(&result).unwrap_or_default();
                    if state.transcript.trim().is_empty() {
                        state.transcript = result;
                    }
                }
            }
            CursorStreamEvent::Initialized { .. }
            | CursorStreamEvent::ToolStarted { .. }
            | CursorStreamEvent::ToolCompleted { .. }
            | CursorStreamEvent::Other(_) => {}
        }
    }
    state.transcript = tail_chars(&state.transcript, MAX_TRANSCRIPT_CHARS);
    state.summary = tail_chars(&state.summary, 240);
    Ok(state)
}

fn read_bounded_text(path: &Path, max_bytes: u64) -> Result<String> {
    let mut file = match File::open(path) {
        Ok(file) => file,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(String::new()),
        Err(error) => return Err(error.into()),
    };
    let length = file.metadata()?.len();
    if length > max_bytes {
        file.seek(SeekFrom::Start(length - max_bytes))?;
    }
    let mut bytes = Vec::new();
    file.read_to_end(&mut bytes)?;
    if length > max_bytes {
        if let Some(newline) = bytes.iter().position(|byte| *byte == b'\n') {
            bytes.drain(..=newline);
        }
    }
    Ok(String::from_utf8_lossy(&bytes).into_owned())
}

fn last_nonempty_line(value: &str) -> Option<String> {
    value
        .lines()
        .rev()
        .find(|line| !line.trim().is_empty())
        .map(|line| line.trim().to_owned())
}

fn tail_chars(value: &str, limit: usize) -> String {
    let count = value.chars().count();
    value.chars().skip(count.saturating_sub(limit)).collect()
}

fn prompt_name(prompt: &str) -> String {
    let normalized = prompt.split_whitespace().collect::<Vec<_>>().join(" ");
    if normalized.chars().count() <= 48 {
        normalized
    } else {
        let mut value = normalized.chars().take(47).collect::<String>();
        value.push('…');
        value
    }
}

fn parse_cursor_models(output: &str) -> Vec<String> {
    let rendered = if output.contains('\x1b') {
        let mut parser = vt100::Parser::new(200, 240, 0);
        parser.process(output.as_bytes());
        parser.screen().contents()
    } else {
        output.to_owned()
    };
    let mut models = BTreeSet::new();
    for line in rendered.lines() {
        let line = line.trim().trim_start_matches(|character: char| {
            character.is_whitespace() || matches!(character, '-' | '*' | '•' | '›' | '>' | '✓')
        });
        let Some(candidate) = line.split_whitespace().next() else {
            continue;
        };
        let candidate = candidate.trim_matches(|character: char| matches!(character, ':' | ','));
        let lower = candidate.to_ascii_lowercase();
        if matches!(
            lower.as_str(),
            "loading" | "models" | "model" | "available" | "no" | "name" | "id"
        ) {
            continue;
        }
        if candidate.is_empty()
            || candidate.len() > 128
            || !candidate.chars().all(|character| {
                character.is_ascii_alphanumeric()
                    || matches!(character, '-' | '_' | '.' | ':' | '/' | '@')
            })
        {
            continue;
        }
        models.insert(if candidate.eq_ignore_ascii_case("auto") {
            "auto".into()
        } else {
            candidate.into()
        });
    }
    models.into_iter().collect()
}

fn private_append_file(path: &Path) -> Result<File> {
    let mut options = OpenOptions::new();
    options.create(true).append(true).mode(0o600);
    let file = options
        .open(path)
        .with_context(|| format!("failed to open private Cursor log {}", path.display()))?;
    ensure_private_file(path)?;
    Ok(file)
}

fn private_lock_file(path: &Path) -> Result<File> {
    let mut options = OpenOptions::new();
    options.create(true).read(true).write(true).mode(0o600);
    let file = options
        .open(path)
        .with_context(|| format!("failed to open Cursor registry lock {}", path.display()))?;
    ensure_private_file(path)?;
    Ok(file)
}

fn read_registry(path: &Path) -> Result<CursorRegistry> {
    let input = match fs::read_to_string(path) {
        Ok(input) => input,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
            return Ok(CursorRegistry::default())
        }
        Err(error) => return Err(error.into()),
    };
    ensure_private_file(path)?;
    let registry: CursorRegistry = serde_json::from_str(&input)
        .with_context(|| format!("invalid Cursor ownership registry {}", path.display()))?;
    if registry.version != REGISTRY_VERSION {
        bail!(
            "unsupported Cursor ownership registry version {}",
            registry.version
        );
    }
    for (id, record) in &registry.sessions {
        require_safe_session_id(id)?;
        if record.session_id != *id {
            bail!("Cursor registry key does not match its session ID");
        }
        require_absolute_workspace(&record.cwd)?;
        if !record
            .stdout_path
            .starts_with(path.parent().unwrap_or(Path::new("/invalid")))
            || !record
                .stderr_path
                .starts_with(path.parent().unwrap_or(Path::new("/invalid")))
        {
            bail!("Cursor registry log path escaped its private state directory");
        }
    }
    Ok(registry)
}

fn write_registry(path: &Path, registry: &CursorRegistry) -> Result<()> {
    let temporary = path.with_extension(format!("tmp-{}", std::process::id()));
    let mut options = OpenOptions::new();
    options.create(true).truncate(true).write(true).mode(0o600);
    let mut file = options
        .open(&temporary)
        .with_context(|| format!("failed to write Cursor registry {}", temporary.display()))?;
    serde_json::to_writer_pretty(&mut file, registry)?;
    file.write_all(b"\n")?;
    file.sync_all()?;
    fs::rename(&temporary, path)
        .with_context(|| format!("failed to replace Cursor registry {}", path.display()))?;
    ensure_private_file(path)
}

fn ensure_private_directory(path: &Path) -> Result<()> {
    fs::create_dir_all(path).with_context(|| format!("failed to create {}", path.display()))?;
    let permissions = fs::metadata(path)?.permissions().mode() & 0o777;
    if permissions != 0o700 {
        fs::set_permissions(path, fs::Permissions::from_mode(0o700))?;
    }
    Ok(())
}

fn ensure_private_file(path: &Path) -> Result<()> {
    let permissions = fs::metadata(path)?.permissions().mode() & 0o777;
    if permissions & 0o077 != 0 {
        bail!(
            "private Cursor state file {} is accessible by other users",
            path.display()
        );
    }
    Ok(())
}

fn flock_exclusive(file: &File) -> Result<()> {
    use std::os::fd::AsRawFd;
    let result = unsafe { libc::flock(file.as_raw_fd(), libc::LOCK_EX) };
    if result < 0 {
        return Err(std::io::Error::last_os_error()).context("failed to lock Cursor registry");
    }
    Ok(())
}

fn flock_unlock(file: &File) -> Result<()> {
    use std::os::fd::AsRawFd;
    let result = unsafe { libc::flock(file.as_raw_fd(), libc::LOCK_UN) };
    if result < 0 {
        return Err(std::io::Error::last_os_error()).context("failed to unlock Cursor registry");
    }
    Ok(())
}

fn wait_for_identity(pid: u32, timeout: Duration) -> Result<CursorProcessIdentity> {
    let deadline = Instant::now() + timeout;
    loop {
        if let (Ok(start_token), Ok(cmdline)) = (process_start_token(pid), process_cmdline(pid)) {
            if !cmdline.is_empty() {
                return Ok(CursorProcessIdentity {
                    pid,
                    start_token,
                    cmdline,
                });
            }
        }
        if Instant::now() >= deadline {
            bail!("timed out waiting for Cursor process identity");
        }
        thread::sleep(Duration::from_millis(15));
    }
}

fn spawn_with_busy_retry(command: &mut Command) -> std::io::Result<std::process::Child> {
    let mut attempts = 0;
    loop {
        match command.spawn() {
            Ok(child) => return Ok(child),
            Err(error)
                if error.raw_os_error() == Some(libc::ETXTBSY)
                    && attempts < EXECUTABLE_BUSY_RETRIES =>
            {
                attempts += 1;
                thread::sleep(Duration::from_millis(10));
            }
            Err(error) => return Err(error),
        }
    }
}

fn error_has_errno(error: &anyhow::Error, errno: i32) -> bool {
    error.chain().any(|cause| {
        cause
            .downcast_ref::<std::io::Error>()
            .map(|error| error.raw_os_error() == Some(errno))
            .unwrap_or(false)
    })
}

fn verify_process(identity: &CursorProcessIdentity) -> Result<bool> {
    let (start_token, state) = match process_stat(identity.pid) {
        Ok(value) => value,
        Err(error) if is_missing_process(&error) => return Ok(false),
        Err(error) => return Err(error),
    };
    if state == "Z" {
        return Ok(false);
    }
    let cmdline = match process_cmdline(identity.pid) {
        Ok(value) => value,
        Err(error) if is_missing_process(&error) => return Ok(false),
        Err(error) => return Err(error),
    };
    Ok(start_token == identity.start_token && cmdline == identity.cmdline)
}

#[cfg(target_os = "linux")]
fn signal_verified_process(identity: &CursorProcessIdentity, signal: i32) -> Result<()> {
    let pidfd = unsafe { libc::syscall(libc::SYS_pidfd_open, identity.pid, 0) } as i32;
    if pidfd < 0 {
        return Err(std::io::Error::last_os_error()).context("pidfd_open failed");
    }
    struct Fd(i32);
    impl Drop for Fd {
        fn drop(&mut self) {
            unsafe { libc::close(self.0) };
        }
    }
    let pidfd = Fd(pidfd);
    if !verify_process(identity)? {
        bail!("persisted Cursor process identity is no longer live");
    }
    let result = unsafe {
        libc::syscall(
            libc::SYS_pidfd_send_signal,
            pidfd.0,
            signal,
            std::ptr::null::<libc::siginfo_t>(),
            0,
        )
    };
    if result < 0 {
        return Err(std::io::Error::last_os_error()).context("pidfd_send_signal failed");
    }
    Ok(())
}

#[cfg(not(target_os = "linux"))]
fn signal_verified_process(_: &CursorProcessIdentity, _: i32) -> Result<()> {
    bail!("race-safe Cursor interruption requires Linux pidfds")
}

#[cfg(target_os = "linux")]
fn process_stat(pid: u32) -> Result<(String, String)> {
    let stat = fs::read_to_string(format!("/proc/{pid}/stat"))?;
    let suffix = stat
        .rsplit_once(')')
        .map(|(_, suffix)| suffix)
        .context("invalid /proc process stat")?;
    let fields = suffix.split_whitespace().collect::<Vec<_>>();
    let state = fields.first().context("/proc process stat omitted state")?;
    let start_token = fields
        .get(19)
        .context("/proc process stat omitted starttime")?;
    Ok(((*start_token).into(), (*state).into()))
}

#[cfg(not(target_os = "linux"))]
fn process_stat(_: u32) -> Result<(String, String)> {
    bail!("process identity verification is unavailable on this platform")
}

fn process_start_token(pid: u32) -> Result<String> {
    process_stat(pid).map(|(token, _)| token)
}

#[cfg(target_os = "linux")]
fn process_cmdline(pid: u32) -> Result<Vec<u8>> {
    fs::read(format!("/proc/{pid}/cmdline")).map_err(Into::into)
}

#[cfg(not(target_os = "linux"))]
fn process_cmdline(_: u32) -> Result<Vec<u8>> {
    bail!("process identity verification is unavailable on this platform")
}

fn is_missing_process(error: &anyhow::Error) -> bool {
    error.chain().any(|cause| {
        cause
            .downcast_ref::<std::io::Error>()
            .map(|error| {
                error.kind() == std::io::ErrorKind::NotFound
                    || matches!(error.raw_os_error(), Some(libc::ENOENT) | Some(libc::ESRCH))
            })
            .unwrap_or(false)
    })
}

fn require_safe_session_id(session_id: &str) -> Result<()> {
    if session_id.is_empty()
        || !session_id
            .chars()
            .all(|character| character.is_ascii_alphanumeric() || matches!(character, '-' | '_'))
    {
        bail!("Cursor session ID contains unsupported path characters");
    }
    Ok(())
}

fn require_absolute_workspace(cwd: &Path) -> Result<()> {
    if !cwd.is_absolute() {
        bail!("Cursor workspace must be absolute");
    }
    Ok(())
}

#[cfg(target_os = "linux")]
fn require_process_identity_support() -> Result<()> {
    Ok(())
}

#[cfg(not(target_os = "linux"))]
fn require_process_identity_support() -> Result<()> {
    bail!("safe managed Cursor processes currently require Linux process identity support")
}

fn file_updated_at(path: &Path) -> Option<SystemTime> {
    fs::metadata(path)
        .and_then(|metadata| metadata.modified())
        .ok()
}

fn millis_to_time(milliseconds: u64) -> Option<SystemTime> {
    Some(SystemTime::UNIX_EPOCH + Duration::from_millis(milliseconds))
}

fn now_millis() -> u64 {
    SystemTime::now()
        .duration_since(SystemTime::UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis() as u64
}

#[cfg(test)]
mod tests {
    #[cfg(target_os = "linux")]
    use std::os::unix::fs::PermissionsExt;
    use tempfile::tempdir;

    use super::*;

    #[test]
    fn fixture_log_reduces_transcript_and_terminal_state() {
        let directory = tempdir().unwrap();
        let log = directory.path().join("run.ndjson");
        fs::write(
            &log,
            concat!(
                "{\"type\":\"system\",\"subtype\":\"init\",\"cwd\":\"/work\",\"session_id\":\"id\"}\n",
                "{\"type\":\"assistant\",\"message\":{\"content\":[{\"type\":\"text\",\"text\":\"first \"}]},\"session_id\":\"id\"}\n",
                "{\"type\":\"assistant\",\"message\":{\"content\":[{\"type\":\"text\",\"text\":\"done\"}]},\"session_id\":\"id\"}\n",
                "{\"type\":\"result\",\"subtype\":\"success\",\"is_error\":false,\"result\":\"first done\",\"session_id\":\"id\"}\n"
            ),
        )
        .unwrap();
        let state = read_cursor_log(&log).unwrap();
        assert_eq!(state.transcript, "first done");
        assert_eq!(state.summary, "first done");
        assert_eq!(state.terminal_result, Some(false));
    }

    #[test]
    fn account_model_catalog_strips_terminal_progress_and_keeps_exact_ids() {
        assert_eq!(
            parse_cursor_models(
                "\x1b[2K\rLoading models…\n  auto  Recommended\n  claude-sonnet-4.6  Sonnet\n"
            ),
            vec!["auto", "claude-sonnet-4.6"]
        );
    }

    #[test]
    fn vanished_process_errors_reconcile_as_not_running_even_when_wrapped() {
        let missing = anyhow::Error::new(std::io::Error::from_raw_os_error(libc::ESRCH))
            .context("process disappeared during discovery");
        assert!(is_missing_process(&missing));
        let absent = anyhow::Error::new(std::io::Error::from_raw_os_error(libc::ENOENT));
        assert!(is_missing_process(&absent));
        let denied = anyhow::Error::new(std::io::Error::from_raw_os_error(libc::EACCES));
        assert!(!is_missing_process(&denied));
    }

    #[test]
    #[cfg(target_os = "linux")]
    fn managed_launch_discovery_inspection_interrupt_and_reply_are_isolated() {
        let directory = tempdir().unwrap();
        let executable = directory.path().join("cursor-agent-mock");
        fs::write(
            &executable,
            r##"#!/bin/sh
if [ "${1:-}" = "models" ]; then
  printf '%s\n' 'auto'
  exit 0
fi
if [ "${1:-}" = "create-chat" ]; then
  printf '%s\n' 'mock-session-1'
  exit 0
fi
session='mock-session-1'
workspace=''
for arg in "$@"; do
  case "$arg" in
    --workspace) next_workspace=1 ;;
    *)
      if [ "${next_workspace:-0}" = 1 ]; then workspace="$arg"; next_workspace=0; fi
      ;;
  esac
done
printf '{"type":"system","subtype":"init","cwd":"%s","session_id":"%s"}\n' "$workspace" "$session"
printf '{"type":"assistant","message":{"content":[{"type":"text","text":"working"}]},"session_id":"%s"}\n' "$session"
trap 'printf "{\"type\":\"result\",\"subtype\":\"error\",\"is_error\":true,\"result\":\"interrupted\",\"session_id\":\"%s\"}\\n" "$session"; exit 130' INT TERM
while :; do sleep 1; done
"##,
        )
        .unwrap();
        let mut permissions = fs::metadata(&executable).unwrap().permissions();
        permissions.set_mode(0o700);
        fs::set_permissions(&executable, permissions).unwrap();
        let workspace = directory.path().join("workspace");
        fs::create_dir(&workspace).unwrap();
        let supervisor = CursorSupervisor::with_state_dir(
            executable.display().to_string(),
            directory.path().join("state"),
        )
        .unwrap();

        let session_id = supervisor
            .launch("test managed Cursor", &workspace)
            .unwrap();
        assert_eq!(session_id, "mock-session-1");
        let request = DiscoveryRequest {
            include_completed: true,
            include_interactive: true,
            cwd: None,
            ..DiscoveryRequest::default()
        };
        let session = wait_for_session(&supervisor, &request, SessionState::Working);
        assert!(session.pid.is_some());
        assert!(session.capabilities.contains(&Capability::Interrupt));
        assert_eq!(supervisor.inspect(&session).unwrap(), "working");

        supervisor.interrupt(&session).unwrap();
        let interrupted = wait_for_session(&supervisor, &request, SessionState::NeedsInput);
        assert!(interrupted.capabilities.contains(&Capability::Reply));
        assert!(!interrupted.capabilities.contains(&Capability::Interrupt));

        supervisor.reply(&interrupted, "retry safely").unwrap();
        let retried = wait_for_session(&supervisor, &request, SessionState::Working);
        supervisor.interrupt(&retried).unwrap();
        let _ = wait_for_session(&supervisor, &request, SessionState::NeedsInput);

        let registry_permissions = fs::metadata(directory.path().join("state/sessions.json"))
            .unwrap()
            .permissions()
            .mode()
            & 0o777;
        assert_eq!(registry_permissions, 0o600);
        assert_eq!(
            fs::metadata(directory.path().join("state"))
                .unwrap()
                .permissions()
                .mode()
                & 0o777,
            0o700
        );
    }

    #[test]
    #[cfg(target_os = "linux")]
    fn launch_refuses_an_account_without_models_before_creating_a_chat() {
        let directory = tempdir().unwrap();
        let executable = directory.path().join("cursor-agent-mock");
        fs::write(
            &executable,
            r##"#!/bin/sh
if [ "${1:-}" = "models" ]; then
  printf '\033[2K\033[GNo models available for this account.\n'
  exit 0
fi
printf '%s\n' create-chat-was-not-expected >&2
exit 91
"##,
        )
        .unwrap();
        let mut permissions = fs::metadata(&executable).unwrap().permissions();
        permissions.set_mode(0o700);
        fs::set_permissions(&executable, permissions).unwrap();
        let workspace = directory.path().join("workspace");
        fs::create_dir(&workspace).unwrap();
        let supervisor = CursorSupervisor::with_state_dir(
            executable.display().to_string(),
            directory.path().join("state"),
        )
        .unwrap();

        let error = supervisor
            .launch("must not create", &workspace)
            .unwrap_err();
        assert!(format!("{error:#}").contains("has no models"), "{error:#}");
        assert!(!directory.path().join("state/sessions.json").exists());
    }

    #[test]
    #[cfg(target_os = "linux")]
    fn tampered_process_identity_is_never_signalled() {
        let directory = tempdir().unwrap();
        let _supervisor =
            CursorSupervisor::with_state_dir("unused", directory.path().join("state")).unwrap();
        let mut child = Command::new("sh")
            .args(["-c", "trap 'exit 0' INT; while :; do sleep 1; done"])
            .spawn()
            .unwrap();
        let mut identity = wait_for_identity(child.id(), Duration::from_secs(1)).unwrap();
        identity.cmdline = b"not-the-real-command".to_vec();
        assert!(signal_verified_process(&identity, libc::SIGINT).is_err());
        assert!(child.try_wait().unwrap().is_none());
        let _ = child.kill();
        let _ = child.wait();
    }

    #[cfg(target_os = "linux")]
    fn wait_for_session(
        supervisor: &CursorSupervisor,
        request: &DiscoveryRequest,
        expected: SessionState,
    ) -> AgentSession {
        let deadline = Instant::now() + Duration::from_secs(3);
        loop {
            let session = supervisor.discover(request).unwrap().remove(0);
            if session.state == expected {
                return session;
            }
            assert!(
                Instant::now() < deadline,
                "session never reached {expected:?}"
            );
            thread::sleep(Duration::from_millis(25));
        }
    }
}

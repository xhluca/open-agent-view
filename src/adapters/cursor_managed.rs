use std::collections::{BTreeMap, BTreeSet};
use std::fs::{self, File, OpenOptions};
use std::io::{Read, Seek, SeekFrom, Write};
use std::os::unix::fs::{OpenOptionsExt, PermissionsExt};
use std::os::unix::process::CommandExt;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::sync::Arc;
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
const CREATE_TIMEOUT: Duration = Duration::from_secs(15);
const IDENTITY_TIMEOUT: Duration = Duration::from_secs(3);
const MAX_LOG_BYTES: u64 = 4 * 1024 * 1024;
const MAX_TRANSCRIPT_CHARS: usize = 32 * 1024;

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
    process: CursorProcessIdentity,
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
        })
    }

    pub fn executable(&self) -> &str {
        &self.executable
    }

    pub fn launch(&self, prompt: &str, cwd: &Path) -> Result<String> {
        require_process_identity_support()?;
        let prompt = prompt.trim();
        if prompt.is_empty() {
            bail!("the Cursor launch prompt cannot be empty");
        }
        require_absolute_workspace(cwd)?;
        let spec = self.invocation.create_chat(cwd)?;
        let mut request = crate::process::CommandRequest::new(spec.program, spec.args);
        request.current_dir = Some(spec.current_dir);
        request.timeout = CREATE_TIMEOUT;
        let output = self.runner.run(&request)?;
        if output.status != 0 {
            bail!(
                "Cursor create-chat exited with status {}: {}",
                output.status,
                output.stderr_lossy()
            );
        }
        let session_id = parse_cursor_chat_id(output.stdout_text()?)?;
        self.spawn_turn(&session_id, cwd, prompt, true)?;
        Ok(session_id)
    }

    pub fn reply(&self, session: &AgentSession, prompt: &str) -> Result<()> {
        self.require_owned_host(session)?;
        let prompt = prompt.trim();
        if prompt.is_empty() {
            bail!("the Cursor reply cannot be empty");
        }
        let record = self.lookup(&session.provider_session_id)?;
        if verify_process(&record.process)? {
            bail!("Cursor session is still working and has no documented live-steer API");
        }
        self.spawn_turn(&record.session_id, &record.cwd, prompt, false)
    }

    pub fn interrupt(&self, session: &AgentSession) -> Result<()> {
        self.require_owned_host(session)?;
        let record = self.lookup(&session.provider_session_id)?;
        signal_verified_process(&record.process, libc::SIGINT)
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
        verify_process(&record.process)
    }

    pub fn discover(&self, request: &DiscoveryRequest) -> Result<Vec<AgentSession>> {
        let registry = self.with_locked_registry(|registry| Ok(registry.clone()))?;
        let mut sessions = Vec::with_capacity(registry.sessions.len());
        for record in registry.sessions.into_values() {
            let running = verify_process(&record.process)?;
            let log = read_cursor_log(&record.stdout_path)?;
            let (state, raw_state) = if running {
                (SessionState::Working, "working")
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
            if running {
                capabilities.insert(Capability::Interrupt);
            } else {
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
                summary: if log.summary.is_empty() {
                    match state {
                        SessionState::Working => "Cursor agent is working".into(),
                        SessionState::NeedsInput => "Cursor run exited without success".into(),
                        _ => "Cursor run completed".into(),
                    }
                } else {
                    log.summary
                },
                raw_state: Some(raw_state.into()),
                pid: running.then_some(record.process.pid),
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
            if session.provider != Provider::Cursor || session.kind == SessionKind::Managed {
                continue;
            }
            session.capabilities.remove(&Capability::Reply);
            session.capabilities.remove(&Capability::Interrupt);
        }
    }

    fn spawn_turn(&self, session_id: &str, cwd: &Path, prompt: &str, new: bool) -> Result<()> {
        require_safe_session_id(session_id)?;
        require_absolute_workspace(cwd)?;
        let existing =
            self.with_locked_registry(|registry| Ok(registry.sessions.get(session_id).cloned()))?;
        if let Some(record) = existing.as_ref() {
            if verify_process(&record.process)? {
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
        let spec = self.invocation.print_turn(session_id, cwd, prompt)?;
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
        let mut child = command
            .spawn()
            .context("failed to launch managed Cursor turn")?;
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
            process,
        };
        let process_for_cleanup = record.process.clone();
        if let Err(error) = self.with_locked_registry(|registry| {
            registry.sessions.insert(session_id.into(), record);
            Ok(())
        }) {
            if verify_process(&process_for_cleanup).unwrap_or(false) {
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
    error
        .downcast_ref::<std::io::Error>()
        .map(|error| error.kind() == std::io::ErrorKind::NotFound)
        .unwrap_or(false)
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
    #[cfg(target_os = "linux")]
    fn managed_launch_discovery_inspection_interrupt_and_reply_are_isolated() {
        let directory = tempdir().unwrap();
        let executable = directory.path().join("cursor-agent-mock");
        fs::write(
            &executable,
            r##"#!/bin/sh
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
        };
        let session = wait_for_session(&supervisor, &request, SessionState::Working);
        assert_eq!(session.pid.is_some(), true);
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

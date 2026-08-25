use std::collections::{BTreeSet, HashSet};
use std::fs::{self, OpenOptions};
use std::io::{BufRead, BufReader, Read, Write};
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};
use std::sync::{Arc, Mutex};
use std::thread;
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use anyhow::{anyhow, bail, Context, Result};
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};

use super::{DiscoveryRequest, SessionSource};
use crate::control::{
    run_native_authentication, ControlOutcome, LaunchMode, LaunchPresentation, LaunchRequest,
    ProviderController,
};
use crate::domain::{
    AgentSession, Capability, Provider, Runtime, SessionKind, SessionSnapshot, SessionState,
};

const RPC_TIMEOUT: Duration = Duration::from_secs(10);
const MAX_RPC_OUTPUT: usize = 8 * 1024 * 1024;
const SESSION_CORRELATION_TIMEOUT: Duration = Duration::from_secs(5);
const SESSION_CORRELATION_INTERVAL: Duration = Duration::from_millis(100);

#[derive(Clone, Debug, Deserialize, Eq, PartialEq)]
#[serde(rename_all = "camelCase")]
struct VibeSession {
    id: String,
    title: Option<String>,
    #[serde(default)]
    preview: String,
    status: VibeStatus,
    created_at: u64,
    updated_at: u64,
    cwd: Option<PathBuf>,
    model: Option<String>,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq)]
#[serde(tag = "type", rename_all = "snake_case")]
enum VibeStatus {
    Idle,
    Running {
        active_turn_id: String,
    },
    Blocked {
        active_turn_id: String,
        callback_id: String,
        reason: String,
    },
    Failed {
        message: String,
    },
}

#[derive(Clone, Debug, Deserialize, Eq, Ord, PartialEq, PartialOrd, Serialize)]
#[serde(rename_all = "camelCase")]
struct OwnedVibeSession {
    session_id: String,
    cwd: PathBuf,
    created_at_ms: u64,
    name: String,
}

pub struct MistralVibeOwnership {
    path: PathBuf,
    records: Mutex<BTreeSet<OwnedVibeSession>>,
}

impl MistralVibeOwnership {
    pub fn load_default() -> Result<Arc<Self>> {
        Self::load(default_mistral_vibe_ownership_path()?)
    }

    pub fn load(path: PathBuf) -> Result<Arc<Self>> {
        validate_private_state_path(&path)?;
        let records = match fs::read_to_string(&path) {
            Ok(input) => serde_json::from_str(&input).with_context(|| {
                format!("invalid Mistral Vibe ownership registry {}", path.display())
            })?,
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => BTreeSet::new(),
            Err(error) => return Err(error.into()),
        };
        Ok(Arc::new(Self {
            path,
            records: Mutex::new(records),
        }))
    }

    fn owns(&self, session_id: &str) -> bool {
        self.records
            .lock()
            .map(|records| records.iter().any(|record| record.session_id == session_id))
            .unwrap_or(false)
    }

    fn recorded_cwd(&self, session_id: &str) -> Option<PathBuf> {
        self.records.lock().ok().and_then(|records| {
            records
                .iter()
                .find(|record| record.session_id == session_id)
                .map(|record| record.cwd.clone())
        })
    }

    fn record(&self, session: &VibeSession, fallback_cwd: &Path, name: &str) -> Result<()> {
        let mut records = self
            .records
            .lock()
            .map_err(|_| anyhow!("Mistral Vibe ownership registry lock was poisoned"))?;
        let mut next = records.clone();
        next.retain(|record| record.session_id != session.id);
        next.insert(OwnedVibeSession {
            session_id: session.id.clone(),
            cwd: session
                .cwd
                .clone()
                .unwrap_or_else(|| fallback_cwd.to_owned()),
            created_at_ms: session.created_at,
            name: name.into(),
        });
        persist_private_registry(&self.path, &next)?;
        *records = next;
        Ok(())
    }
}

pub fn default_mistral_vibe_ownership_path() -> Result<PathBuf> {
    if let Some(state_home) = std::env::var_os("XDG_STATE_HOME") {
        return Ok(PathBuf::from(state_home)
            .join("open-agent-view")
            .join("mistral-vibe-owned.json"));
    }
    let home = std::env::var_os("HOME").context("HOME is not set")?;
    Ok(PathBuf::from(home).join(".local/state/open-agent-view/mistral-vibe-owned.json"))
}

trait VibeRpc: Send + Sync {
    fn sessions(&self, cwd: Option<&Path>, limit: usize) -> Result<Vec<VibeSession>>;
    fn models(&self, cwd: &Path) -> Result<Vec<String>>;
}

struct ProcessVibeRpc {
    app_server: String,
}

impl ProcessVibeRpc {
    fn new(app_server: impl Into<String>) -> Self {
        Self {
            app_server: app_server.into(),
        }
    }

    fn request(&self, method: &str, params: Value, cwd: Option<&Path>) -> Result<Value> {
        let mut command = Command::new(&self.app_server);
        command
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped());
        if let Some(cwd) = cwd {
            command.current_dir(cwd);
        }
        #[cfg(unix)]
        {
            use std::os::unix::process::CommandExt;
            command.process_group(0);
        }
        let mut child = command.spawn().with_context(|| {
            format!(
                "failed to start Mistral Vibe app server {}",
                self.app_server
            )
        })?;
        let mut stdin = child.stdin.take().context("Vibe app server has no stdin")?;
        for message in [
            json!({
                "jsonrpc": "2.0", "id": 1, "method": "initialize",
                "params": {"clientInfo": {"name": "open-agent-view", "version": env!("CARGO_PKG_VERSION"), "entrypoint": "programmatic"}, "capabilities": {}}
            }),
            json!({"jsonrpc": "2.0", "method": "initialized", "params": {}}),
            json!({"jsonrpc": "2.0", "id": 2, "method": method, "params": params}),
        ] {
            serde_json::to_writer(&mut stdin, &message)?;
            stdin.write_all(b"\n")?;
        }
        stdin.flush()?;
        drop(stdin);
        let output = wait_with_output(child, RPC_TIMEOUT)?;
        if output.0 != 0 {
            bail!(
                "Mistral Vibe app server exited with status {}: {}",
                output.0,
                String::from_utf8_lossy(&output.2).trim()
            );
        }
        if output.1.len() > MAX_RPC_OUTPUT {
            bail!("Mistral Vibe app-server response exceeded the 8 MiB safety limit");
        }
        for line in BufReader::new(output.1.as_slice()).lines() {
            let value: Value = serde_json::from_str(&line.context("failed to read Vibe RPC")?)
                .context("invalid Mistral Vibe app-server JSON-RPC")?;
            if value.get("id") == Some(&json!(2)) {
                if let Some(error) = value.get("error") {
                    bail!(
                        "Mistral Vibe app-server request failed: {}",
                        rpc_error(error)
                    );
                }
                return value
                    .get("result")
                    .cloned()
                    .context("Mistral Vibe app-server response has no result");
            }
        }
        bail!("Mistral Vibe app server returned no matching response")
    }
}

impl VibeRpc for ProcessVibeRpc {
    fn sessions(&self, cwd: Option<&Path>, limit: usize) -> Result<Vec<VibeSession>> {
        let result = self.request(
            "session/list",
            json!({"limit": limit.clamp(1, 500), "cwd": cwd.map(|path| path.display().to_string())}),
            cwd,
        )?;
        serde_json::from_value(
            result
                .get("items")
                .cloned()
                .context("Mistral Vibe session/list omitted items")?,
        )
        .context("invalid Mistral Vibe session/list result")
    }

    fn models(&self, cwd: &Path) -> Result<Vec<String>> {
        let result = self.request(
            "config/read",
            json!({"cwd": cwd.display().to_string()}),
            Some(cwd),
        )?;
        let models = result
            .pointer("/config/models")
            .and_then(Value::as_array)
            .context("Mistral Vibe config/read omitted models")?;
        let mut aliases = models
            .iter()
            .filter_map(|model| model.get("alias").and_then(Value::as_str))
            .filter(|alias| valid_identifier(alias))
            .map(str::to_owned)
            .collect::<Vec<_>>();
        aliases.sort();
        aliases.dedup();
        Ok(aliases)
    }
}

pub struct MistralVibeSource {
    rpc: Arc<dyn VibeRpc>,
    ownership: Arc<MistralVibeOwnership>,
}

impl MistralVibeSource {
    pub fn host(app_server: impl Into<String>, ownership: Arc<MistralVibeOwnership>) -> Self {
        Self {
            rpc: Arc::new(ProcessVibeRpc::new(app_server)),
            ownership,
        }
    }

    #[cfg(test)]
    fn with_rpc(rpc: Arc<dyn VibeRpc>, ownership: Arc<MistralVibeOwnership>) -> Self {
        Self { rpc, ownership }
    }
}

impl SessionSource for MistralVibeSource {
    fn label(&self) -> &str {
        "Mistral Vibe (host)"
    }

    fn discover(&self, request: &DiscoveryRequest) -> Result<Vec<AgentSession>> {
        let mut sessions = self
            .rpc
            .sessions(request.cwd.as_deref(), request.history_limit.max(1))?
            .into_iter()
            .filter(|session| request.include_external || self.ownership.owns(&session.id))
            .filter(|session| {
                request.include_completed || state(&session.status) != SessionState::Completed
            })
            .filter_map(|session| {
                let owned = self.ownership.owns(&session.id);
                let recorded_cwd = self.ownership.recorded_cwd(&session.id);
                normalize_session(session, recorded_cwd.as_deref(), owned)
            })
            .collect::<Vec<_>>();
        sessions.sort_by(|left, right| right.updated_at.cmp(&left.updated_at));
        Ok(sessions)
    }
}

pub struct MistralVibeController {
    executable: String,
    rpc: Arc<dyn VibeRpc>,
    ownership: Arc<MistralVibeOwnership>,
    launch_keys: Mutex<std::collections::BTreeMap<String, String>>,
    cwd: PathBuf,
}

impl MistralVibeController {
    pub fn host(
        executable: impl Into<String>,
        app_server: impl Into<String>,
        ownership: Arc<MistralVibeOwnership>,
        cwd: PathBuf,
    ) -> Self {
        Self {
            executable: executable.into(),
            rpc: Arc::new(ProcessVibeRpc::new(app_server)),
            ownership,
            launch_keys: Mutex::new(Default::default()),
            cwd,
        }
    }

    #[cfg(test)]
    fn with_rpc(
        executable: impl Into<String>,
        rpc: Arc<dyn VibeRpc>,
        ownership: Arc<MistralVibeOwnership>,
        cwd: PathBuf,
    ) -> Self {
        Self {
            executable: executable.into(),
            rpc,
            ownership,
            launch_keys: Mutex::new(Default::default()),
            cwd,
        }
    }

    fn correlate(
        &self,
        before: &HashSet<String>,
        request: &LaunchRequest,
        launched_at_ms: u64,
    ) -> Result<Option<VibeSession>> {
        let deadline = Instant::now() + SESSION_CORRELATION_TIMEOUT;
        loop {
            let matches = self
                .rpc
                .sessions(Some(&request.cwd), 100)?
                .into_iter()
                .filter(|session| !before.contains(&session.id))
                .filter(|session| session.updated_at.saturating_add(2_000) >= launched_at_ms)
                .filter(|session| {
                    session
                        .cwd
                        .as_deref()
                        .map(|cwd| cwd == request.cwd)
                        .unwrap_or(true)
                })
                .collect::<Vec<_>>();
            match matches.len() {
                1 => return Ok(matches.into_iter().next()),
                // Once two candidates are visible, exact ownership cannot be
                // established. Waiting longer cannot make that observation safe.
                count if count > 1 => return Ok(None),
                _ if Instant::now() < deadline => {
                    thread::sleep(SESSION_CORRELATION_INTERVAL);
                }
                _ => return Ok(None),
            }
        }
    }
}

impl ProviderController for MistralVibeController {
    fn provider(&self) -> Provider {
        Provider::MistralVibe
    }

    fn launch_mode(&self) -> LaunchMode {
        LaunchMode::SelectableModel
    }

    fn available_models(&self) -> Result<Vec<String>> {
        self.rpc.models(&self.cwd)
    }

    fn launch_presentation(&self) -> LaunchPresentation {
        LaunchPresentation::Foreground
    }

    fn supports_authentication(&self) -> bool {
        true
    }

    fn authenticate(&self) -> Result<ControlOutcome> {
        run_native_authentication(&self.executable, &["--setup"], Provider::MistralVibe)
    }

    fn enrich(&self, snapshot: &mut SessionSnapshot) {
        let keys = self.launch_keys.lock().ok();
        for session in snapshot.sessions.iter_mut().filter(|session| {
            session.provider == Provider::MistralVibe
                && self.ownership.owns(&session.provider_session_id)
        }) {
            if keys
                .as_ref()
                .and_then(|keys| keys.get(&session.provider_session_id))
                .is_some_and(|key| crate::native_session::is_backgrounded(key))
            {
                session.capabilities.insert(Capability::Interrupt);
                session.state = SessionState::Working;
                session.kind = SessionKind::Managed;
            }
        }
    }

    fn launch_foreground(&self, request: &LaunchRequest) -> Result<ControlOutcome> {
        if request.provider != Provider::MistralVibe {
            bail!("the Mistral Vibe controller cannot launch another provider");
        }
        if request.prompt.trim().is_empty() {
            bail!("the Mistral Vibe launch prompt cannot be empty");
        }
        let before = self
            .rpc
            .sessions(Some(&request.cwd), 100)?
            .into_iter()
            .map(|session| session.id)
            .collect::<HashSet<_>>();
        let launched_at_ms = now_millis();
        let launch_key = format!(
            "mistral-vibe:launch:{}",
            crate::native_session::new_session_id()?
        );
        let mut command = Command::new(&self.executable);
        command.arg(request.prompt.trim()).current_dir(&request.cwd);
        if let Some(model) = request.model.as_deref() {
            command.env("VIBE_ACTIVE_MODEL", model);
        }
        let native_exit = crate::native_session::run(command, &launch_key)?;
        if let crate::native_session::NativeSessionExit::Exited(status) = &native_exit {
            if !status.success() {
                bail!("Mistral Vibe session exited with status {status}");
            }
        }
        let correlated = match self.correlate(&before, request, launched_at_ms) {
            Ok(session) => session,
            Err(error) => {
                if matches!(
                    native_exit,
                    crate::native_session::NativeSessionExit::Backgrounded
                ) {
                    let _ = crate::native_session::terminate(&launch_key);
                }
                return Err(error).context(
                    "could not correlate the new Mistral Vibe session; stopped its live foreground bridge",
                );
            }
        };
        let Some(session) = correlated else {
            if matches!(
                native_exit,
                crate::native_session::NativeSessionExit::Backgrounded
            ) {
                let _ = crate::native_session::terminate(&launch_key);
            }
            return Ok(ControlOutcome {
                message: "returned from Mistral Vibe; its app server did not expose one unambiguous new session, so OAV did not claim ownership or keep a live frontend".into(),
                provider_session_hint: None,
            });
        };
        if let Err(error) =
            self.ownership
                .record(&session, &request.cwd, &summarize(&request.prompt, 48))
        {
            if matches!(
                native_exit,
                crate::native_session::NativeSessionExit::Backgrounded
            ) {
                let _ = crate::native_session::terminate(&launch_key);
            }
            return Err(error).context(
                "could not persist Mistral Vibe ownership; stopped its live foreground bridge",
            );
        }
        self.launch_keys
            .lock()
            .map_err(|_| anyhow!("Mistral Vibe launch-key lock was poisoned"))?
            .insert(session.id.clone(), launch_key);
        let message = match native_exit {
            crate::native_session::NativeSessionExit::Backgrounded => format!(
                "backgrounded Mistral Vibe session {}; Enter/Right resumes it",
                session.id.chars().take(8).collect::<String>()
            ),
            crate::native_session::NativeSessionExit::Exited(status) if status.success() => {
                format!(
                    "returned from Mistral Vibe session {}",
                    session.id.chars().take(8).collect::<String>()
                )
            }
            crate::native_session::NativeSessionExit::Exited(status) => {
                bail!("Mistral Vibe session exited with status {status}")
            }
        };
        Ok(ControlOutcome {
            message,
            provider_session_hint: Some(session.id),
        })
    }

    fn open(&self, session: &AgentSession) -> Result<ControlOutcome> {
        validate_session(session)?;
        if !self.ownership.owns(&session.provider_session_id) {
            bail!("refusing to open a Mistral Vibe session not created by Open Agent View");
        }
        if let Some(key) = self
            .launch_keys
            .lock()
            .ok()
            .and_then(|keys| keys.get(&session.provider_session_id).cloned())
        {
            if crate::native_session::is_backgrounded(&key) {
                return native_outcome(
                    crate::native_session::resume(&key)?,
                    &session.provider_session_id,
                    &session.name,
                );
            }
        }
        let mut command = Command::new(&self.executable);
        command
            .args(["--resume", &session.provider_session_id])
            .current_dir(&session.cwd);
        native_outcome(
            crate::native_session::run(command, &session.id)?,
            &session.provider_session_id,
            &session.name,
        )
    }

    fn interrupt(&self, session: &AgentSession) -> Result<ControlOutcome> {
        validate_session(session)?;
        if !self.ownership.owns(&session.provider_session_id) {
            bail!("refusing to stop a Mistral Vibe session not created by Open Agent View");
        }
        let key = self
            .launch_keys
            .lock()
            .map_err(|_| anyhow!("Mistral Vibe launch-key lock was poisoned"))?
            .get(&session.provider_session_id)
            .cloned()
            .context("Mistral Vibe is not backgrounded in this dashboard process")?;
        crate::native_session::terminate(&key)?;
        Ok(ControlOutcome {
            message: format!("stopped Mistral Vibe session {}", session.name),
            provider_session_hint: Some(session.provider_session_id.clone()),
        })
    }

    fn inspect(&self, session: &AgentSession) -> Result<String> {
        validate_session(session)?;
        Ok(format!(
            "Mistral Vibe: {}\nSession: {}\nDirectory: {}\nState: {}\n\nEnter or Right opens the exact provider session in Vibe's native TUI.",
            session.name,
            session.provider_session_id,
            session.cwd.display(),
            session.state.heading()
        ))
    }
}

fn normalize_session(
    session: VibeSession,
    recorded_cwd: Option<&Path>,
    owned: bool,
) -> Option<AgentSession> {
    let state = state(&session.status);
    let raw_state = match &session.status {
        VibeStatus::Idle => "idle",
        VibeStatus::Running { .. } => "running",
        VibeStatus::Blocked { .. } => "blocked",
        VibeStatus::Failed { .. } => "failed",
    };
    let status_summary = match &session.status {
        VibeStatus::Blocked { reason, .. } => Some(reason.as_str()),
        VibeStatus::Failed { message } => Some(message.as_str()),
        _ => None,
    };
    let name = session
        .title
        .as_deref()
        .filter(|title| !title.trim().is_empty())
        .map(|title| summarize(title, 48))
        .unwrap_or_else(|| summarize(&session.preview, 48));
    // The native resume path must never silently fall back to the dashboard's
    // current directory. Prefer the provider-reported cwd; only an exact OAV
    // ownership record may supply a fallback. External rows without either are
    // omitted because they cannot be opened safely.
    let cwd = session
        .cwd
        .clone()
        .or_else(|| recorded_cwd.map(Path::to_path_buf))?;
    Some(AgentSession {
        id: format!("mistral_vibe:host:{}", session.id),
        provider_session_id: session.id,
        provider: Provider::MistralVibe,
        runtime: Runtime::Host,
        kind: if owned {
            SessionKind::Managed
        } else {
            SessionKind::Unknown
        },
        name,
        cwd,
        state,
        summary: summarize(status_summary.unwrap_or(&session.preview), 160),
        raw_state: Some(raw_state.into()),
        pid: None,
        started_at: Some(UNIX_EPOCH + Duration::from_millis(session.created_at)),
        updated_at: Some(UNIX_EPOCH + Duration::from_millis(session.updated_at)),
        pull_requests: None,
        capabilities: BTreeSet::from([Capability::Inspect]),
    })
}

fn state(status: &VibeStatus) -> SessionState {
    match status {
        VibeStatus::Running { .. } => SessionState::Working,
        VibeStatus::Blocked { .. } => SessionState::NeedsInput,
        VibeStatus::Idle | VibeStatus::Failed { .. } => SessionState::Completed,
    }
}

fn native_outcome(
    exit: crate::native_session::NativeSessionExit,
    id: &str,
    name: &str,
) -> Result<ControlOutcome> {
    match exit {
        crate::native_session::NativeSessionExit::Backgrounded => Ok(ControlOutcome {
            message: format!("backgrounded Mistral Vibe session {name}; Enter/Right resumes it"),
            provider_session_hint: Some(id.into()),
        }),
        crate::native_session::NativeSessionExit::Exited(status) if status.success() => {
            Ok(ControlOutcome {
                message: format!("returned from Mistral Vibe session {name}"),
                provider_session_hint: Some(id.into()),
            })
        }
        crate::native_session::NativeSessionExit::Exited(status) => {
            bail!("Mistral Vibe session exited with status {status}")
        }
    }
}

fn validate_session(session: &AgentSession) -> Result<()> {
    if session.provider != Provider::MistralVibe || session.runtime != Runtime::Host {
        bail!("the host Mistral Vibe controller does not own this runtime");
    }
    if !valid_identifier(&session.provider_session_id) {
        bail!("invalid Mistral Vibe session ID");
    }
    Ok(())
}

fn valid_identifier(value: &str) -> bool {
    !value.is_empty()
        && value.len() <= 256
        && !value
            .chars()
            .any(|character| character.is_control() || character.is_whitespace())
}

fn rpc_error(error: &Value) -> String {
    error
        .get("message")
        .and_then(Value::as_str)
        .unwrap_or("unknown app-server error")
        .chars()
        .filter(|character| !character.is_control())
        .take(240)
        .collect()
}

fn summarize(value: &str, limit: usize) -> String {
    let clean = value
        .chars()
        .map(|character| {
            if character.is_control() {
                ' '
            } else {
                character
            }
        })
        .collect::<String>();
    let clean = clean.split_whitespace().collect::<Vec<_>>().join(" ");
    if clean.is_empty() {
        return "Mistral Vibe session".into();
    }
    let mut result = clean.chars().take(limit).collect::<String>();
    if clean.chars().count() > limit {
        result.push('…');
    }
    result
}

fn now_millis() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis() as u64
}

fn wait_with_output(mut child: Child, timeout: Duration) -> Result<(i32, Vec<u8>, Vec<u8>)> {
    let mut stdout = child
        .stdout
        .take()
        .context("Vibe app server has no stdout")?;
    let mut stderr = child
        .stderr
        .take()
        .context("Vibe app server has no stderr")?;
    let stdout_reader = thread::spawn(move || {
        let mut output = Vec::new();
        stdout.read_to_end(&mut output).map(|_| output)
    });
    let stderr_reader = thread::spawn(move || {
        let mut output = Vec::new();
        stderr.read_to_end(&mut output).map(|_| output)
    });
    let deadline = Instant::now() + timeout;
    let status = loop {
        if let Some(status) = child.try_wait()? {
            break status;
        }
        if Instant::now() >= deadline {
            #[cfg(unix)]
            unsafe {
                libc::kill(-(child.id() as i32), libc::SIGKILL);
            }
            let _ = child.kill();
            let _ = child.wait();
            let _ = stdout_reader.join();
            let _ = stderr_reader.join();
            bail!("Mistral Vibe app-server request timed out after 10000 ms");
        }
        thread::sleep(Duration::from_millis(10));
    };
    let stdout = stdout_reader
        .join()
        .map_err(|_| anyhow!("Vibe stdout reader panicked"))??;
    let stderr = stderr_reader
        .join()
        .map_err(|_| anyhow!("Vibe stderr reader panicked"))??;
    Ok((status.code().unwrap_or(-1), stdout, stderr))
}

fn persist_private_registry(path: &Path, records: &BTreeSet<OwnedVibeSession>) -> Result<()> {
    let parent = path
        .parent()
        .context("Mistral Vibe registry has no parent")?;
    ensure_private_directory(parent)?;
    validate_private_state_path(path)?;
    let temporary = path.with_extension(format!("tmp-{}-{}", std::process::id(), now_millis()));
    reject_symlink(&temporary)?;
    let mut options = OpenOptions::new();
    options.create_new(true).write(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.mode(0o600);
    }
    let result = (|| -> Result<()> {
        let mut file = options.open(&temporary)?;
        serde_json::to_writer_pretty(&mut file, records)?;
        file.write_all(b"\n")?;
        file.sync_all()?;
        fs::rename(&temporary, path)?;
        Ok(())
    })();
    if result.is_err() {
        let _ = fs::remove_file(&temporary);
    }
    result
}

fn validate_private_state_path(path: &Path) -> Result<()> {
    if let Some(parent) = path.parent() {
        if parent.exists() {
            ensure_private_directory(parent)?;
        }
    }
    match fs::symlink_metadata(path) {
        Ok(metadata) => {
            if metadata.file_type().is_symlink() || !metadata.is_file() {
                bail!(
                    "Mistral Vibe state path {} must be a regular file",
                    path.display()
                );
            }
            #[cfg(unix)]
            {
                use std::os::unix::fs::{MetadataExt, PermissionsExt};
                if metadata.uid() != unsafe { libc::geteuid() } {
                    bail!(
                        "Mistral Vibe state path {} has the wrong owner",
                        path.display()
                    );
                }
                if metadata.permissions().mode() & 0o777 != 0o600 {
                    bail!(
                        "Mistral Vibe state path {} must have mode 0600",
                        path.display()
                    );
                }
            }
            Ok(())
        }
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(()),
        Err(error) => Err(error.into()),
    }
}

fn ensure_private_directory(path: &Path) -> Result<()> {
    if !path.exists() {
        fs::create_dir_all(path)?;
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            fs::set_permissions(path, fs::Permissions::from_mode(0o700))?;
        }
    }
    let metadata = fs::symlink_metadata(path)?;
    if metadata.file_type().is_symlink() || !metadata.is_dir() {
        bail!(
            "Mistral Vibe state directory {} must be a real directory",
            path.display()
        );
    }
    #[cfg(unix)]
    {
        use std::os::unix::fs::{MetadataExt, PermissionsExt};
        if metadata.uid() != unsafe { libc::geteuid() } {
            bail!(
                "Mistral Vibe state directory {} has the wrong owner",
                path.display()
            );
        }
        if metadata.permissions().mode() & 0o777 != 0o700 {
            bail!(
                "Mistral Vibe state directory {} must have mode 0700",
                path.display()
            );
        }
    }
    Ok(())
}

fn reject_symlink(path: &Path) -> Result<()> {
    match fs::symlink_metadata(path) {
        Ok(metadata) if metadata.file_type().is_symlink() => {
            bail!(
                "refusing symlinked Mistral Vibe state path {}",
                path.display()
            )
        }
        Ok(_) => Ok(()),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(()),
        Err(error) => Err(error.into()),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::VecDeque;

    struct FakeRpc {
        sessions: Vec<VibeSession>,
        models: Vec<String>,
    }

    impl VibeRpc for FakeRpc {
        fn sessions(&self, _cwd: Option<&Path>, _limit: usize) -> Result<Vec<VibeSession>> {
            Ok(self.sessions.clone())
        }

        fn models(&self, _cwd: &Path) -> Result<Vec<String>> {
            Ok(self.models.clone())
        }
    }

    struct DelayedRpc {
        responses: Mutex<VecDeque<Vec<VibeSession>>>,
    }

    impl VibeRpc for DelayedRpc {
        fn sessions(&self, _cwd: Option<&Path>, _limit: usize) -> Result<Vec<VibeSession>> {
            Ok(self
                .responses
                .lock()
                .unwrap()
                .pop_front()
                .unwrap_or_default())
        }

        fn models(&self, _cwd: &Path) -> Result<Vec<String>> {
            Ok(Vec::new())
        }
    }

    fn session(id: &str, status: VibeStatus) -> VibeSession {
        VibeSession {
            id: id.into(),
            title: Some("Parser work".into()),
            preview: "Fix the parser".into(),
            status,
            created_at: 1_700_000_000_000,
            updated_at: 1_700_000_005_000,
            cwd: Some(PathBuf::from("/work")),
            model: Some("devstral".into()),
        }
    }

    #[test]
    fn source_filters_external_and_maps_protocol_state() {
        let directory = tempfile::tempdir().unwrap();
        let ownership = MistralVibeOwnership::load(directory.path().join("owned.json")).unwrap();
        let owned = session(
            "11111111-2222-3333-4444-555555555555",
            VibeStatus::Blocked {
                active_turn_id: "turn".into(),
                callback_id: "callback".into(),
                reason: "approve edit".into(),
            },
        );
        ownership
            .record(&owned, Path::new("/work"), "owned")
            .unwrap();
        let source = MistralVibeSource::with_rpc(
            Arc::new(FakeRpc {
                sessions: vec![
                    owned,
                    session("aaaaaaaa-bbbb-cccc-dddd-eeeeeeeeeeee", VibeStatus::Idle),
                ],
                models: Vec::new(),
            }),
            ownership,
        );
        let sessions = source.discover(&DiscoveryRequest::default()).unwrap();
        assert_eq!(sessions.len(), 1);
        assert_eq!(sessions[0].provider, Provider::MistralVibe);
        assert_eq!(sessions[0].state, SessionState::NeedsInput);
        assert_eq!(sessions[0].summary, "approve edit");

        let external = source
            .discover(&DiscoveryRequest {
                include_completed: true,
                include_external: true,
                ..DiscoveryRequest::default()
            })
            .unwrap();
        assert_eq!(external.len(), 2);
        let unowned = external
            .iter()
            .find(|session| session.provider_session_id.starts_with("aaaaaaaa"))
            .unwrap();
        assert_eq!(unowned.kind, SessionKind::Unknown);
        assert_eq!(unowned.capabilities, BTreeSet::from([Capability::Inspect]));
    }

    #[test]
    fn model_catalog_comes_from_provider_config_rpc() {
        let directory = tempfile::tempdir().unwrap();
        let ownership = MistralVibeOwnership::load(directory.path().join("owned.json")).unwrap();
        let controller = MistralVibeController::with_rpc(
            "vibe",
            Arc::new(FakeRpc {
                sessions: Vec::new(),
                models: vec!["devstral".into(), "codestral".into()],
            }),
            ownership,
            PathBuf::from("/work"),
        );
        assert_eq!(
            controller.available_models().unwrap(),
            vec!["devstral", "codestral"]
        );
    }

    #[test]
    fn missing_provider_cwd_uses_only_exact_owned_fallback() {
        let directory = tempfile::tempdir().unwrap();
        let ownership = MistralVibeOwnership::load(directory.path().join("owned.json")).unwrap();
        let mut owned = session("owned", VibeStatus::Idle);
        owned.cwd = None;
        ownership
            .record(&owned, Path::new("/verified/work"), "owned")
            .unwrap();
        let mut external = session("external", VibeStatus::Idle);
        external.cwd = None;
        let source = MistralVibeSource::with_rpc(
            Arc::new(FakeRpc {
                sessions: vec![owned, external],
                models: Vec::new(),
            }),
            ownership,
        );

        let sessions = source
            .discover(&DiscoveryRequest {
                include_completed: true,
                include_external: true,
                ..DiscoveryRequest::default()
            })
            .unwrap();
        assert_eq!(sessions.len(), 1);
        assert_eq!(sessions[0].provider_session_id, "owned");
        assert_eq!(sessions[0].cwd, PathBuf::from("/verified/work"));
    }

    #[cfg(unix)]
    #[test]
    fn ownership_registry_refuses_symlinks_and_permissive_modes() {
        use std::os::unix::fs::{symlink, PermissionsExt};

        let directory = tempfile::tempdir().unwrap();
        let target = directory.path().join("target.json");
        fs::write(&target, "[]").unwrap();
        let link = directory.path().join("linked.json");
        symlink(&target, &link).unwrap();
        assert!(MistralVibeOwnership::load(link).is_err());

        let path = directory.path().join("owned.json");
        fs::write(&path, "[]").unwrap();
        fs::set_permissions(&path, fs::Permissions::from_mode(0o644)).unwrap();
        assert!(MistralVibeOwnership::load(path.clone()).is_err());

        fs::set_permissions(&path, fs::Permissions::from_mode(0o600)).unwrap();
        fs::set_permissions(directory.path(), fs::Permissions::from_mode(0o755)).unwrap();
        assert!(MistralVibeOwnership::load(path).is_err());
    }

    #[cfg(unix)]
    #[test]
    fn failed_registry_persistence_does_not_claim_in_memory_ownership() {
        use std::os::unix::fs::PermissionsExt;

        let directory = tempfile::tempdir().unwrap();
        let ownership = MistralVibeOwnership::load(directory.path().join("owned.json")).unwrap();
        fs::set_permissions(directory.path(), fs::Permissions::from_mode(0o500)).unwrap();
        let candidate = session("not-owned", VibeStatus::Idle);

        assert!(ownership
            .record(&candidate, Path::new("/work"), "not owned")
            .is_err());
        assert!(!ownership.owns("not-owned"));
    }

    #[test]
    fn ambiguous_post_launch_correlation_refuses_ownership() {
        let directory = tempfile::tempdir().unwrap();
        let ownership = MistralVibeOwnership::load(directory.path().join("owned.json")).unwrap();
        let rpc = Arc::new(FakeRpc {
            sessions: vec![
                session("11111111-2222-3333-4444-555555555555", VibeStatus::Idle),
                session("aaaaaaaa-bbbb-cccc-dddd-eeeeeeeeeeee", VibeStatus::Idle),
            ],
            models: Vec::new(),
        });
        let controller =
            MistralVibeController::with_rpc("vibe", rpc, ownership, PathBuf::from("/work"));
        let request = LaunchRequest {
            provider: Provider::MistralVibe,
            model: None,
            prompt: "fix parser".into(),
            cwd: PathBuf::from("/work"),
        };
        assert!(controller
            .correlate(&HashSet::new(), &request, 0)
            .unwrap()
            .is_none());
    }

    #[test]
    fn post_launch_correlation_waits_for_delayed_provider_visibility() {
        let directory = tempfile::tempdir().unwrap();
        let ownership = MistralVibeOwnership::load(directory.path().join("owned.json")).unwrap();
        let rpc = Arc::new(DelayedRpc {
            responses: Mutex::new(VecDeque::from([
                Vec::new(),
                Vec::new(),
                vec![session("delayed", VibeStatus::Idle)],
            ])),
        });
        let controller =
            MistralVibeController::with_rpc("vibe", rpc, ownership, PathBuf::from("/work"));
        let request = LaunchRequest {
            provider: Provider::MistralVibe,
            model: None,
            prompt: "delayed session".into(),
            cwd: PathBuf::from("/work"),
        };
        let started = Instant::now();

        let correlated = controller
            .correlate(&HashSet::new(), &request, 0)
            .unwrap()
            .unwrap();
        assert_eq!(correlated.id, "delayed");
        assert!(started.elapsed() >= SESSION_CORRELATION_INTERVAL * 2);
        assert!(started.elapsed() < SESSION_CORRELATION_TIMEOUT);
    }
}

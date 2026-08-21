//! Durable ownership and control for Pi's stdio-only RPC protocol.
//!
//! A small `open-agent-view` daemon owns the actual Pi stdin/stdout pipes and
//! exposes a user-private Unix socket. Dashboard processes can reconnect to the
//! daemon, while unrelated Pi processes remain read-only history.

use std::collections::{BTreeMap, HashMap};
use std::fs::{self, File, OpenOptions};
use std::io::{BufRead, BufReader, Read, Write};
use std::path::{Path, PathBuf};
use std::process::{ChildStdin, Command, Stdio};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{mpsc, Arc, Mutex};
use std::thread;
use std::time::{Duration, Instant, SystemTime};

use anyhow::{anyhow, bail, Context, Result};
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};

use crate::domain::SessionState;

const RECORD_VERSION: u32 = 1;
#[cfg(target_os = "linux")]
const STARTUP_TIMEOUT: Duration = Duration::from_secs(8);
const RPC_TIMEOUT: Duration = Duration::from_secs(8);
const MAX_MESSAGE_BYTES: usize = 1024 * 1024;
const MODEL_LAUNCH_FEATURE: &str = "launch_with_model";
const STOP_SESSION_FEATURE: &str = "stop_session";
const DELETE_SESSION_FEATURE: &str = "delete_session";

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
struct SupervisorRecord {
    version: u32,
    pid: u32,
    process_start_token: String,
    process_cmdline: Vec<u8>,
    pi_bin: String,
    socket_path: PathBuf,
    created_at_ms: u64,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum PiPendingKind {
    Confirm,
    Select,
    Input,
    Editor,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct PiPendingRequest {
    pub id: String,
    pub kind: PiPendingKind,
    pub title: String,
    pub message: String,
    #[serde(default)]
    pub options: Vec<String>,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ManagedPiSession {
    pub id: String,
    pub cwd: PathBuf,
    pub name: String,
    pub pid: u32,
    pub process_start_token: String,
    pub state: SessionState,
    pub summary: String,
    pub session_file: Option<PathBuf>,
    pub pending: Option<PiPendingRequest>,
    pub alive: bool,
    pub created_at_ms: u64,
    pub updated_at_ms: u64,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(tag = "op", rename_all = "snake_case")]
enum DaemonRequest {
    Ping,
    List,
    Launch {
        prompt: String,
        cwd: PathBuf,
    },
    LaunchWithModel {
        prompt: String,
        cwd: PathBuf,
        model: String,
    },
    Inspect {
        session_id: String,
    },
    Reply {
        session_id: String,
        prompt: String,
    },
    Interrupt {
        session_id: String,
    },
    ResolveConfirm {
        session_id: String,
        accept: bool,
    },
    RespondInput {
        session_id: String,
        answer: String,
    },
    Stop {
        session_id: String,
    },
    Delete {
        session_id: String,
    },
    Shutdown,
}

#[derive(Debug, Deserialize, Serialize)]
struct DaemonResponse {
    ok: bool,
    #[serde(default)]
    data: Value,
    error: Option<String>,
}

impl DaemonResponse {
    fn success(data: impl Serialize) -> Result<Self> {
        Ok(Self {
            ok: true,
            data: serde_json::to_value(data)?,
            error: None,
        })
    }

    fn failure(error: anyhow::Error) -> Self {
        Self {
            ok: false,
            data: Value::Null,
            error: Some(format!("{error:#}")),
        }
    }
}

/// Reconnectable client for the OAV-owned Pi RPC daemon.
pub struct PiSupervisor {
    pi_bin: String,
    state_dir: PathBuf,
    record_path: PathBuf,
    lock_path: PathBuf,
    #[cfg_attr(not(target_os = "linux"), allow(dead_code))]
    daemon_exe: PathBuf,
}

impl PiSupervisor {
    pub fn host(pi_bin: impl Into<String>) -> Result<Self> {
        Self::with_state_dir_and_exe(pi_bin, default_state_dir()?, std::env::current_exe()?)
    }

    pub fn with_state_dir_and_exe(
        pi_bin: impl Into<String>,
        state_dir: PathBuf,
        daemon_exe: PathBuf,
    ) -> Result<Self> {
        ensure_private_directory(&state_dir)?;
        Ok(Self {
            pi_bin: pi_bin.into(),
            record_path: state_dir.join("supervisor.json"),
            lock_path: state_dir.join("supervisor.lock"),
            state_dir,
            daemon_exe,
        })
    }

    /// Persisted history written by Pi processes owned by this supervisor.
    pub fn session_dir(&self) -> PathBuf {
        self.state_dir.join("sessions")
    }

    pub fn launch(&self, prompt: &str, cwd: &Path) -> Result<ManagedPiSession> {
        self.launch_with_model(prompt, cwd, None)
    }

    pub fn launch_with_model(
        &self,
        prompt: &str,
        cwd: &Path,
        model: Option<&str>,
    ) -> Result<ManagedPiSession> {
        let prompt = prompt.trim();
        if prompt.is_empty() {
            bail!("the Pi launch prompt cannot be empty");
        }
        if !cwd.is_absolute() {
            bail!("the Pi launch directory must be absolute");
        }
        let mut record = self.ensure_endpoint()?;
        let request = match model {
            Some(model) => {
                record = self.model_launch_endpoint(record)?;
                DaemonRequest::LaunchWithModel {
                    prompt: prompt.into(),
                    cwd: cwd.into(),
                    model: validate_pi_model(model)?.into(),
                }
            }
            None => DaemonRequest::Launch {
                prompt: prompt.into(),
                cwd: cwd.into(),
            },
        };
        self.request(&record, &request)
    }

    fn model_launch_endpoint(&self, record: SupervisorRecord) -> Result<SupervisorRecord> {
        let capabilities: Value = self.request(&record, &DaemonRequest::Ping)?;
        if supports_model_launch(&capabilities) {
            return Ok(record);
        }

        // v0.1.10 and earlier already understand Ping/List/Shutdown, but do not
        // understand LaunchWithModel. Check their owned state before replacing
        // the exact daemon; never risk terminating active provider work merely
        // to upgrade this optional capability.
        let sessions: Vec<ManagedPiSession> = self.request(&record, &DaemonRequest::List)?;
        let active = sessions
            .iter()
            .filter(|session| session.state != SessionState::Completed)
            .map(|session| session.name.as_str())
            .take(3)
            .collect::<Vec<_>>();
        if !active.is_empty() {
            bail!(
                "the running Pi supervisor predates model selection and still owns active work ({}); finish or interrupt those sessions, then retry the model launch",
                active.join(", ")
            );
        }

        self.request::<Value>(&record, &DaemonRequest::Shutdown)?;
        wait_for_verified_process_exit(&record, Duration::from_secs(2))
            .context("timed out safely upgrading the idle Pi supervisor")?;
        let upgraded = self.ensure_endpoint()?;
        let capabilities: Value = self.request(&upgraded, &DaemonRequest::Ping)?;
        if !supports_model_launch(&capabilities) {
            bail!("the restarted Pi supervisor does not advertise model-selection support");
        }
        Ok(upgraded)
    }

    /// List only sessions owned by an already-running daemon. Discovery never
    /// starts a daemon as a side effect.
    pub fn list(&self) -> Result<Vec<ManagedPiSession>> {
        let Some(record) = self.live_record()? else {
            return Ok(Vec::new());
        };
        self.request(&record, &DaemonRequest::List)
    }

    pub fn inspect(&self, session_id: &str) -> Result<String> {
        let record = self.required_live_record()?;
        self.request(
            &record,
            &DaemonRequest::Inspect {
                session_id: session_id.into(),
            },
        )
    }

    pub fn reply(&self, session_id: &str, prompt: &str) -> Result<()> {
        let prompt = prompt.trim();
        if prompt.is_empty() {
            bail!("the Pi reply cannot be empty");
        }
        let record = self.required_live_record()?;
        self.request::<Value>(
            &record,
            &DaemonRequest::Reply {
                session_id: session_id.into(),
                prompt: prompt.into(),
            },
        )?;
        Ok(())
    }

    pub fn interrupt(&self, session_id: &str) -> Result<()> {
        let record = self.required_live_record()?;
        self.request::<Value>(
            &record,
            &DaemonRequest::Interrupt {
                session_id: session_id.into(),
            },
        )?;
        Ok(())
    }

    pub fn resolve_confirm(&self, session_id: &str, accept: bool) -> Result<()> {
        let record = self.required_live_record()?;
        self.request::<Value>(
            &record,
            &DaemonRequest::ResolveConfirm {
                session_id: session_id.into(),
                accept,
            },
        )?;
        Ok(())
    }

    pub fn respond_input(&self, session_id: &str, answer: &str) -> Result<()> {
        let answer = answer.trim();
        if answer.is_empty() {
            bail!("the Pi input response cannot be empty");
        }
        let record = self.required_live_record()?;
        self.request::<Value>(
            &record,
            &DaemonRequest::RespondInput {
                session_id: session_id.into(),
                answer: answer.into(),
            },
        )?;
        Ok(())
    }

    /// Close the exact OAV-owned Pi RPC transport. Older verified daemons did
    /// not expose per-session stop; they may be replaced only when doing so
    /// cannot terminate another active session.
    pub fn stop(&self, session_id: &str) -> Result<()> {
        let record = self.required_live_record()?;
        let capabilities: Value = self.request(&record, &DaemonRequest::Ping)?;
        if supports_feature(&capabilities, STOP_SESSION_FEATURE) {
            self.request::<Value>(
                &record,
                &DaemonRequest::Stop {
                    session_id: session_id.into(),
                },
            )?;
            return Ok(());
        }

        let sessions: Vec<ManagedPiSession> = self.request(&record, &DaemonRequest::List)?;
        let target = sessions
            .iter()
            .find(|session| session.id == session_id)
            .context("refusing to stop a Pi session not owned by this supervisor")?;
        if !target.alive {
            return Ok(());
        }
        let other_active = sessions
            .iter()
            .filter(|session| {
                session.id != session_id
                    && session.alive
                    && session.state != SessionState::Completed
            })
            .map(|session| session.name.as_str())
            .take(3)
            .collect::<Vec<_>>();
        if !other_active.is_empty() {
            bail!(
                "the running Pi supervisor predates per-session stop and owns other active work ({}); finish or interrupt that work before stopping this session",
                other_active.join(", ")
            );
        }
        self.request::<Value>(&record, &DaemonRequest::Shutdown)?;
        Ok(())
    }

    pub fn delete(&self, session_id: &str) -> Result<()> {
        let record = self.required_live_record()?;
        let capabilities: Value = self.request(&record, &DaemonRequest::Ping)?;
        if !supports_feature(&capabilities, DELETE_SESSION_FEATURE) {
            bail!("the running Pi supervisor predates exact session deletion; stop its idle sessions and restart Open Agent View before retrying");
        }
        self.request::<Value>(
            &record,
            &DaemonRequest::Delete {
                session_id: session_id.into(),
            },
        )?;
        Ok(())
    }

    pub fn wait_until_stopped(&self, session_id: &str, timeout: Duration) -> Result<()> {
        let deadline = Instant::now() + timeout;
        loop {
            let transport_error = match self.list() {
                Ok(sessions) => {
                    if sessions
                        .iter()
                        .find(|session| session.id == session_id)
                        .map(|session| !session.alive)
                        .unwrap_or(true)
                    {
                        return Ok(());
                    }
                    None
                }
                Err(error) => {
                    if !self.verified_daemon_process_is_live()? {
                        return Ok(());
                    }
                    Some(error)
                }
            };
            if Instant::now() >= deadline {
                if let Some(error) = transport_error {
                    return Err(error).context(
                        "Pi supervisor transport disappeared while its verified process remained live",
                    );
                }
                bail!("managed Pi RPC process did not stop before the deadline");
            }
            thread::sleep(Duration::from_millis(20));
        }
    }

    /// Stop the exact verified daemon. Intended for isolated tests and explicit
    /// operator cleanup, never dashboard shutdown.
    pub fn shutdown_daemon(&self) -> Result<()> {
        let Some(record) = self.live_record()? else {
            return Ok(());
        };
        self.request::<Value>(&record, &DaemonRequest::Shutdown)?;
        wait_for_verified_process_exit(&record, Duration::from_secs(2))
            .context("timed out waiting for the exact Pi supervisor process to exit")
    }

    fn ensure_endpoint(&self) -> Result<SupervisorRecord> {
        let _lock = StateLock::acquire(&self.lock_path)?;
        if let Some(record) = load_record(&self.record_path, &self.state_dir)? {
            if record.version != RECORD_VERSION {
                bail!(
                    "unsupported Pi supervisor record version {}",
                    record.version
                );
            }
            let live = verify_process(&record)?;
            if live && !record_uses_pi_executable(&record, &self.pi_bin) {
                bail!(
                    "a verified Pi supervisor is already running with executable {}; configured executable is {}",
                    record.pi_bin,
                    self.pi_bin
                );
            }
            if live {
                wait_for_socket(&record.socket_path, Duration::from_secs(2))?;
                return Ok(record);
            }
        }
        self.start_endpoint()
    }

    fn start_endpoint(&self) -> Result<SupervisorRecord> {
        #[cfg(not(target_os = "linux"))]
        bail!("durable Pi supervision currently requires Linux process identity verification");

        #[cfg(target_os = "linux")]
        {
            let socket_path =
                self.state_dir
                    .join(format!("rpc-{}-{}.sock", std::process::id(), now_millis()));
            let log = private_append_file(&self.state_dir.join("supervisor.log"))?;
            let mut command = Command::new(&self.daemon_exe);
            command
                .arg("__pi-supervisor")
                .arg("--state-dir")
                .arg(&self.state_dir)
                .arg("--socket")
                .arg(&socket_path)
                .arg("--pi-bin")
                .arg(&self.pi_bin)
                .stdin(Stdio::null())
                .stdout(Stdio::from(log.try_clone()?))
                .stderr(Stdio::from(log));
            use std::os::unix::process::CommandExt;
            command.process_group(0);
            let mut child = command.spawn().with_context(|| {
                format!(
                    "failed to start Pi supervisor via {}",
                    self.daemon_exe.display()
                )
            })?;
            let pid = child.id();
            let deadline = Instant::now() + STARTUP_TIMEOUT;
            if let Err(error) = wait_for_socket(
                &socket_path,
                deadline.saturating_duration_since(Instant::now()),
            ) {
                let _ = child.kill();
                let _ = child.wait();
                return Err(error).context("Pi supervisor never opened its socket");
            }
            let process_start_token = process_start_token(pid)?;
            let process_cmdline = process_cmdline(pid)?;
            if process_cmdline.is_empty() {
                let _ = child.kill();
                let _ = child.wait();
                bail!("new Pi supervisor exposed an empty process command line");
            }
            let record = SupervisorRecord {
                version: RECORD_VERSION,
                pid,
                process_start_token,
                process_cmdline,
                pi_bin: self.pi_bin.clone(),
                socket_path,
                created_at_ms: now_millis(),
            };
            save_record(&self.record_path, &record)?;
            drop(child);
            Ok(record)
        }
    }

    fn live_record(&self) -> Result<Option<SupervisorRecord>> {
        let _lock = StateLock::acquire(&self.lock_path)?;
        let Some(record) = load_record(&self.record_path, &self.state_dir)? else {
            return Ok(None);
        };
        if !verify_process(&record)? {
            return Ok(None);
        }
        wait_for_socket(&record.socket_path, Duration::from_millis(500))?;
        Ok(Some(record))
    }

    fn verified_daemon_process_is_live(&self) -> Result<bool> {
        let _lock = StateLock::acquire(&self.lock_path)?;
        let Some(record) = load_record(&self.record_path, &self.state_dir)? else {
            return Ok(false);
        };
        verify_process(&record)
    }

    fn required_live_record(&self) -> Result<SupervisorRecord> {
        self.live_record()?
            .context("Pi supervisor has no live owned session transport")
    }

    fn request<T: for<'de> Deserialize<'de>>(
        &self,
        record: &SupervisorRecord,
        request: &DaemonRequest,
    ) -> Result<T> {
        if !verify_process(record)? {
            bail!("persisted Pi supervisor process identity is no longer live");
        }
        let response = request_daemon(&record.socket_path, request)?;
        if !response.ok {
            bail!(
                "Pi supervisor request failed: {}",
                response.error.as_deref().unwrap_or("unknown error")
            );
        }
        serde_json::from_value(response.data).context("invalid Pi supervisor response")
    }
}

#[derive(Clone, Debug)]
struct LiveState {
    name: String,
    state: SessionState,
    summary: String,
    pending: Option<PiPendingRequest>,
    alive: bool,
    updated_at_ms: u64,
}

struct RpcProcess {
    id: String,
    cwd: PathBuf,
    pid: u32,
    process_start_token: String,
    session_file: Option<PathBuf>,
    created_at_ms: u64,
    stdin: Mutex<Option<ChildStdin>>,
    pending_responses: Arc<Mutex<HashMap<String, mpsc::Sender<Value>>>>,
    state: Arc<Mutex<LiveState>>,
    next_request_id: AtomicU64,
}

impl RpcProcess {
    fn spawn(
        pi_bin: &str,
        session_dir: &Path,
        prompt: &str,
        cwd: &Path,
        model: Option<&str>,
        log: File,
    ) -> Result<Self> {
        let name = prompt
            .split_whitespace()
            .take(8)
            .collect::<Vec<_>>()
            .join(" ");
        let mut command = Command::new(pi_bin);
        command
            .args(["--mode", "rpc", "--no-approve", "--session-dir"])
            .arg(session_dir)
            .args(["--name", &name]);
        if let Some(model) = model {
            command.args(["--model", validate_pi_model(model)?]);
        }
        let mut child = command
            .current_dir(cwd)
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::from(log))
            .spawn()
            .with_context(|| format!("failed to start managed Pi via {pi_bin}"))?;
        let pid = child.id();
        let process_start_token = process_start_token(pid)?;
        let stdin = Mutex::new(Some(child.stdin.take().context("Pi stdin unavailable")?));
        let stdout = child.stdout.take().context("Pi stdout unavailable")?;
        let pending_responses = Arc::new(Mutex::new(HashMap::new()));
        let state = Arc::new(Mutex::new(LiveState {
            name: name.clone(),
            state: SessionState::Unknown,
            summary: String::new(),
            pending: None,
            alive: true,
            updated_at_ms: now_millis(),
        }));
        spawn_rpc_reader(stdout, pending_responses.clone(), state.clone());
        thread::spawn(move || {
            let _ = child.wait();
        });

        let mut process = Self {
            id: String::new(),
            cwd: cwd.into(),
            pid,
            process_start_token,
            session_file: None,
            created_at_ms: now_millis(),
            stdin,
            pending_responses,
            state,
            next_request_id: AtomicU64::new(1),
        };
        let response = process.send(json!({"type": "get_state"}))?;
        let data = response.get("data").context("Pi get_state omitted data")?;
        process.id = data
            .get("sessionId")
            .and_then(Value::as_str)
            .context("Pi get_state omitted sessionId")?
            .to_owned();
        process.session_file = data
            .get("sessionFile")
            .and_then(Value::as_str)
            .map(PathBuf::from);
        mark_working(&process.state)?;
        process.send(json!({"type": "prompt", "message": prompt}))?;
        Ok(process)
    }

    fn send(&self, mut command: Value) -> Result<Value> {
        let id = format!(
            "oav-{}-{}",
            std::process::id(),
            self.next_request_id.fetch_add(1, Ordering::Relaxed)
        );
        command
            .as_object_mut()
            .context("Pi RPC command must be an object")?
            .insert("id".into(), Value::String(id.clone()));
        let (sender, receiver) = mpsc::channel();
        self.pending_responses
            .lock()
            .map_err(|_| anyhow!("Pi pending-response lock was poisoned"))?
            .insert(id.clone(), sender);
        let bytes = serde_json::to_vec(&command)?;
        let write_result = (|| -> Result<()> {
            let mut stdin = self
                .stdin
                .lock()
                .map_err(|_| anyhow!("Pi stdin lock was poisoned"))?;
            let stdin = stdin.as_mut().context("Pi RPC process input is closed")?;
            stdin.write_all(&bytes)?;
            stdin.write_all(b"\n")?;
            stdin.flush()?;
            Ok(())
        })();
        if let Err(error) = write_result {
            let _ = self.pending_responses.lock().map(|mut map| map.remove(&id));
            return Err(error).context("failed to write Pi RPC command");
        }
        let response = receiver
            .recv_timeout(RPC_TIMEOUT)
            .with_context(|| format!("timed out waiting for Pi RPC response {id}"))?;
        if response.get("success").and_then(Value::as_bool) == Some(false) {
            bail!(
                "Pi RPC command failed: {}",
                response
                    .get("error")
                    .and_then(Value::as_str)
                    .unwrap_or("unknown provider error")
            );
        }
        Ok(response)
    }

    fn send_extension_response(&self, response: Value) -> Result<()> {
        let bytes = serde_json::to_vec(&response)?;
        let mut stdin = self
            .stdin
            .lock()
            .map_err(|_| anyhow!("Pi stdin lock was poisoned"))?;
        let stdin = stdin.as_mut().context("Pi RPC process input is closed")?;
        stdin.write_all(&bytes)?;
        stdin.write_all(b"\n")?;
        stdin.flush()?;
        Ok(())
    }

    fn stop(&self) -> Result<()> {
        {
            let mut live = self
                .state
                .lock()
                .map_err(|_| anyhow!("Pi live-state lock was poisoned"))?;
            live.state = SessionState::Completed;
            live.pending = None;
            live.updated_at_ms = now_millis();
        }
        self.stdin
            .lock()
            .map_err(|_| anyhow!("Pi stdin lock was poisoned"))?
            .take();
        Ok(())
    }

    fn snapshot(&self) -> Result<ManagedPiSession> {
        let live = self
            .state
            .lock()
            .map_err(|_| anyhow!("Pi live-state lock was poisoned"))?
            .clone();
        Ok(ManagedPiSession {
            id: self.id.clone(),
            cwd: self.cwd.clone(),
            name: live.name,
            pid: self.pid,
            process_start_token: self.process_start_token.clone(),
            state: live.state,
            summary: live.summary,
            session_file: self.session_file.clone(),
            pending: live.pending,
            alive: live.alive,
            created_at_ms: self.created_at_ms,
            updated_at_ms: live.updated_at_ms,
        })
    }
}

struct DaemonState {
    pi_bin: String,
    session_dir: PathBuf,
    log_path: PathBuf,
    sessions: Mutex<BTreeMap<String, Arc<RpcProcess>>>,
}

impl DaemonState {
    fn launch(&self, prompt: &str, cwd: &Path, model: Option<&str>) -> Result<ManagedPiSession> {
        if !cwd.is_absolute() {
            bail!("managed Pi cwd must be absolute");
        }
        let log = private_append_file(&self.log_path)?;
        let process = Arc::new(RpcProcess::spawn(
            &self.pi_bin,
            &self.session_dir,
            prompt,
            cwd,
            model,
            log,
        )?);
        let snapshot = process.snapshot()?;
        let mut sessions = self
            .sessions
            .lock()
            .map_err(|_| anyhow!("Pi daemon session lock was poisoned"))?;
        if sessions.insert(snapshot.id.clone(), process).is_some() {
            bail!("Pi returned a duplicate managed session ID");
        }
        Ok(snapshot)
    }

    fn session(&self, id: &str) -> Result<Arc<RpcProcess>> {
        self.sessions
            .lock()
            .map_err(|_| anyhow!("Pi daemon session lock was poisoned"))?
            .get(id)
            .cloned()
            .context("refusing to control a Pi session not owned by this daemon")
    }

    fn list(&self) -> Result<Vec<ManagedPiSession>> {
        self.sessions
            .lock()
            .map_err(|_| anyhow!("Pi daemon session lock was poisoned"))?
            .values()
            .map(|process| process.snapshot())
            .collect()
    }

    fn dispatch(&self, request: DaemonRequest) -> Result<(Value, bool)> {
        match request {
            DaemonRequest::Ping => Ok((
                json!({
                    "version": RECORD_VERSION,
                    "features": [MODEL_LAUNCH_FEATURE, STOP_SESSION_FEATURE, DELETE_SESSION_FEATURE]
                }),
                false,
            )),
            DaemonRequest::List => Ok((serde_json::to_value(self.list()?)?, false)),
            DaemonRequest::Launch { prompt, cwd } => Ok((
                serde_json::to_value(self.launch(&prompt, &cwd, None)?)?,
                false,
            )),
            DaemonRequest::LaunchWithModel { prompt, cwd, model } => Ok((
                serde_json::to_value(self.launch(&prompt, &cwd, Some(&model))?)?,
                false,
            )),
            DaemonRequest::Inspect { session_id } => {
                let response = self
                    .session(&session_id)?
                    .send(json!({"type": "get_messages"}))?;
                Ok((Value::String(format_rpc_messages(&response)?), false))
            }
            DaemonRequest::Reply { session_id, prompt } => {
                let process = self.session(&session_id)?;
                let streaming = process.snapshot()?.state == SessionState::Working;
                let command = if streaming {
                    json!({"type": "prompt", "message": prompt, "streamingBehavior": "steer"})
                } else {
                    json!({"type": "prompt", "message": prompt})
                };
                // Mark the new turn before sending. Pi may emit its correlated
                // response and a terminal/dialog event back-to-back; updating
                // after `send` would overwrite that newer event state.
                mark_working(&process.state)?;
                process.send(command)?;
                Ok((Value::Null, false))
            }
            DaemonRequest::Interrupt { session_id } => {
                self.session(&session_id)?.send(json!({"type": "abort"}))?;
                Ok((Value::Null, false))
            }
            DaemonRequest::ResolveConfirm { session_id, accept } => {
                let process = self.session(&session_id)?;
                let pending = process
                    .snapshot()?
                    .pending
                    .filter(|pending| pending.kind == PiPendingKind::Confirm)
                    .context("no Pi confirmation request is pending")?;
                clear_pending(&process.state)?;
                process.send_extension_response(json!({
                    "type": "extension_ui_response",
                    "id": pending.id,
                    "confirmed": accept
                }))?;
                Ok((Value::Null, false))
            }
            DaemonRequest::RespondInput { session_id, answer } => {
                let process = self.session(&session_id)?;
                let pending = process
                    .snapshot()?
                    .pending
                    .filter(|pending| pending.kind != PiPendingKind::Confirm)
                    .context("no Pi text or selection request is pending")?;
                if pending.kind == PiPendingKind::Select
                    && !pending.options.iter().any(|option| option == &answer)
                {
                    bail!("the Pi response must exactly match a presented option");
                }
                clear_pending(&process.state)?;
                process.send_extension_response(json!({
                    "type": "extension_ui_response",
                    "id": pending.id,
                    "value": answer
                }))?;
                Ok((Value::Null, false))
            }
            DaemonRequest::Stop { session_id } => {
                self.session(&session_id)?.stop()?;
                Ok((Value::Null, false))
            }
            DaemonRequest::Delete { session_id } => {
                self.delete(&session_id)?;
                Ok((Value::Null, false))
            }
            DaemonRequest::Shutdown => Ok((Value::Null, true)),
        }
    }

    fn delete(&self, session_id: &str) -> Result<()> {
        let process = self.session(session_id)?;
        let snapshot = process.snapshot()?;
        if snapshot.alive {
            bail!("the managed Pi RPC process must stop before deletion");
        }
        let path = snapshot
            .session_file
            .context("the managed Pi session file is unavailable")?;
        let metadata = fs::symlink_metadata(&path)
            .with_context(|| format!("failed to inspect Pi session {}", path.display()))?;
        if !metadata.file_type().is_file() || metadata.file_type().is_symlink() {
            bail!("refusing to delete a non-regular Pi session file");
        }
        let owned_root = fs::canonicalize(&self.session_dir).with_context(|| {
            format!(
                "failed to resolve managed Pi session root {}",
                self.session_dir.display()
            )
        })?;
        let canonical = fs::canonicalize(&path)
            .with_context(|| format!("failed to resolve Pi session {}", path.display()))?;
        if !canonical.starts_with(owned_root) {
            bail!("refusing to delete a Pi session outside the managed store");
        }
        let file = File::open(&canonical)?;
        let first_line = BufReader::new(file)
            .lines()
            .next()
            .transpose()?
            .context("managed Pi session file is empty")?;
        let header: Value = serde_json::from_str(&first_line)
            .context("managed Pi session header is invalid JSON")?;
        if header.get("type").and_then(Value::as_str) != Some("session")
            || header.get("id").and_then(Value::as_str) != Some(session_id)
        {
            bail!("managed Pi session header does not match the exact owned ID");
        }
        fs::remove_file(&canonical)
            .with_context(|| format!("failed to delete Pi session {}", canonical.display()))?;
        self.sessions
            .lock()
            .map_err(|_| anyhow!("Pi daemon session lock was poisoned"))?
            .remove(session_id);
        Ok(())
    }
}

/// Run the hidden durable Pi daemon. The caller must have already resolved an
/// exact private state directory and unique socket path.
pub fn run_pi_supervisor_daemon(
    state_dir: PathBuf,
    socket_path: PathBuf,
    pi_bin: String,
) -> Result<()> {
    #[cfg(not(unix))]
    {
        let _ = (state_dir, socket_path, pi_bin);
        bail!("Pi supervisor daemon requires Unix sockets")
    }
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        use std::os::unix::net::UnixListener;

        ensure_private_directory(&state_dir)?;
        if socket_path.parent() != Some(state_dir.as_path()) {
            bail!("Pi supervisor socket escaped the private state directory");
        }
        if fs::symlink_metadata(&socket_path).is_ok() {
            bail!("refusing to replace existing Pi supervisor socket");
        }
        fs::create_dir_all(state_dir.join("sessions"))?;
        let listener = UnixListener::bind(&socket_path)
            .with_context(|| format!("failed to bind {}", socket_path.display()))?;
        fs::set_permissions(&socket_path, fs::Permissions::from_mode(0o600))?;
        let state = DaemonState {
            pi_bin,
            session_dir: state_dir.join("sessions"),
            log_path: state_dir.join("pi-rpc.log"),
            sessions: Mutex::new(BTreeMap::new()),
        };
        for stream in listener.incoming() {
            let mut stream = stream?;
            let mut bytes = Vec::new();
            std::io::Read::by_ref(&mut stream)
                .take((MAX_MESSAGE_BYTES + 1) as u64)
                .read_to_end(&mut bytes)?;
            // Readiness probes connect without a request. They must not be
            // treated as protocol errors or terminate the durable daemon.
            if bytes.is_empty() {
                continue;
            }
            let (response, shutdown) = if bytes.len() > MAX_MESSAGE_BYTES {
                (
                    DaemonResponse::failure(anyhow!("Pi daemon request exceeded size limit")),
                    false,
                )
            } else {
                match serde_json::from_slice::<DaemonRequest>(&bytes)
                    .context("invalid Pi daemon request")
                    .and_then(|request| state.dispatch(request))
                {
                    Ok((data, shutdown)) => (DaemonResponse::success(data)?, shutdown),
                    Err(error) => (DaemonResponse::failure(error), false),
                }
            };
            serde_json::to_writer(&mut stream, &response)?;
            stream.flush()?;
            if shutdown {
                break;
            }
        }
        let _ = fs::remove_file(&socket_path);
        Ok(())
    }
}

fn spawn_rpc_reader(
    stdout: impl Read + Send + 'static,
    pending_responses: Arc<Mutex<HashMap<String, mpsc::Sender<Value>>>>,
    state: Arc<Mutex<LiveState>>,
) {
    thread::spawn(move || {
        for line in BufReader::new(stdout).lines() {
            let Ok(line) = line else {
                break;
            };
            let Ok(event) = serde_json::from_str::<Value>(&line) else {
                if let Ok(mut live) = state.lock() {
                    live.state = SessionState::NeedsInput;
                    live.summary = "Pi emitted invalid RPC JSON".into();
                    live.updated_at_ms = now_millis();
                }
                continue;
            };
            if event.get("type").and_then(Value::as_str) == Some("response") {
                if let Some(id) = event.get("id").and_then(Value::as_str) {
                    if let Ok(mut pending) = pending_responses.lock() {
                        if let Some(sender) = pending.remove(id) {
                            let _ = sender.send(event);
                            continue;
                        }
                    }
                }
            }
            reconcile_rpc_event(&state, &event);
        }
        if let Ok(mut live) = state.lock() {
            live.alive = false;
            if live.state == SessionState::Working {
                live.state = SessionState::NeedsInput;
                live.summary = "Managed Pi RPC process exited".into();
            }
            live.updated_at_ms = now_millis();
        }
        if let Ok(mut pending) = pending_responses.lock() {
            pending.clear();
        }
    });
}

fn reconcile_rpc_event(state: &Arc<Mutex<LiveState>>, event: &Value) {
    let Ok(mut live) = state.lock() else {
        return;
    };
    live.updated_at_ms = now_millis();
    match event.get("type").and_then(Value::as_str) {
        Some("agent_start" | "turn_start") => {
            live.state = SessionState::Working;
            live.pending = None;
        }
        Some("agent_end" | "agent_settled") => {
            live.state = SessionState::Completed;
            live.pending = None;
        }
        Some("message_update") => {
            if let Some(delta) = event
                .pointer("/assistantMessageEvent/delta")
                .and_then(Value::as_str)
            {
                live.summary.push_str(delta);
                if live.summary.chars().count() > 512 {
                    live.summary = live.summary.chars().rev().take(512).collect::<String>();
                    live.summary = live.summary.chars().rev().collect();
                }
            }
        }
        Some("extension_ui_request") => {
            let method = event.get("method").and_then(Value::as_str).unwrap_or("");
            let kind = match method {
                "confirm" => Some(PiPendingKind::Confirm),
                "select" => Some(PiPendingKind::Select),
                "input" => Some(PiPendingKind::Input),
                "editor" => Some(PiPendingKind::Editor),
                _ => None,
            };
            if let (Some(kind), Some(id)) = (kind, event.get("id").and_then(Value::as_str)) {
                live.state = SessionState::NeedsInput;
                live.pending = Some(PiPendingRequest {
                    id: id.into(),
                    kind,
                    title: event
                        .get("title")
                        .and_then(Value::as_str)
                        .unwrap_or("Pi request")
                        .into(),
                    message: event
                        .get("message")
                        .or_else(|| event.get("placeholder"))
                        .and_then(Value::as_str)
                        .unwrap_or_default()
                        .into(),
                    options: event
                        .get("options")
                        .and_then(Value::as_array)
                        .into_iter()
                        .flatten()
                        .filter_map(Value::as_str)
                        .map(ToOwned::to_owned)
                        .collect(),
                });
            }
        }
        Some("session_info_changed") => {
            if let Some(name) = event.get("name").and_then(Value::as_str) {
                live.name = name.into();
            }
        }
        _ => {}
    }
}

fn clear_pending(state: &Arc<Mutex<LiveState>>) -> Result<()> {
    let mut live = state
        .lock()
        .map_err(|_| anyhow!("Pi live-state lock was poisoned"))?;
    live.pending = None;
    live.state = SessionState::Working;
    live.updated_at_ms = now_millis();
    Ok(())
}

fn mark_working(state: &Arc<Mutex<LiveState>>) -> Result<()> {
    let mut live = state
        .lock()
        .map_err(|_| anyhow!("Pi live-state lock was poisoned"))?;
    live.pending = None;
    live.state = SessionState::Working;
    live.updated_at_ms = now_millis();
    Ok(())
}

fn format_rpc_messages(response: &Value) -> Result<String> {
    let messages = response
        .pointer("/data/messages")
        .or_else(|| response.get("data").filter(|value| value.is_array()))
        .and_then(Value::as_array)
        .context("Pi get_messages response omitted messages")?;
    let mut output = Vec::new();
    for message in messages {
        let role = message
            .get("role")
            .and_then(Value::as_str)
            .unwrap_or("event");
        let text = message
            .get("content")
            .and_then(|content| {
                content.as_str().map(ToOwned::to_owned).or_else(|| {
                    Some(
                        content
                            .as_array()?
                            .iter()
                            .filter_map(|block| block.get("text").and_then(Value::as_str))
                            .collect::<Vec<_>>()
                            .join("\n"),
                    )
                })
            })
            .unwrap_or_default();
        if !text.trim().is_empty() {
            output.push(format!("{}: {}", capitalize(role), text.trim()));
        }
    }
    Ok(if output.is_empty() {
        "No text messages are available in this managed Pi session.".into()
    } else {
        output.join("\n\n")
    })
}

fn capitalize(value: &str) -> String {
    let mut chars = value.chars();
    chars
        .next()
        .map(|first| first.to_uppercase().chain(chars).collect())
        .unwrap_or_default()
}

fn request_daemon(path: &Path, request: &DaemonRequest) -> Result<DaemonResponse> {
    #[cfg(not(unix))]
    {
        let _ = (path, request);
        bail!("Pi supervisor client requires Unix sockets")
    }
    #[cfg(unix)]
    {
        use std::net::Shutdown;
        use std::os::unix::net::UnixStream;

        let mut stream = UnixStream::connect(path)
            .with_context(|| format!("failed to connect to {}", path.display()))?;
        stream.set_read_timeout(Some(RPC_TIMEOUT))?;
        stream.set_write_timeout(Some(RPC_TIMEOUT))?;
        serde_json::to_writer(&mut stream, request)?;
        stream.shutdown(Shutdown::Write)?;
        let mut bytes = Vec::new();
        stream
            .take((MAX_MESSAGE_BYTES + 1) as u64)
            .read_to_end(&mut bytes)?;
        if bytes.len() > MAX_MESSAGE_BYTES {
            bail!("Pi supervisor response exceeded size limit");
        }
        serde_json::from_slice(&bytes).context("invalid Pi supervisor response JSON")
    }
}

fn default_state_dir() -> Result<PathBuf> {
    if let Some(path) = std::env::var_os("XDG_STATE_HOME") {
        return Ok(PathBuf::from(path).join("open-agent-view/pi"));
    }
    let home = std::env::var_os("HOME").context("HOME is not set")?;
    Ok(PathBuf::from(home).join(".local/state/open-agent-view/pi"))
}

fn ensure_private_directory(path: &Path) -> Result<()> {
    match fs::symlink_metadata(path) {
        Ok(metadata) => {
            if !metadata.file_type().is_dir() || metadata.file_type().is_symlink() {
                bail!("Pi supervisor state path is not a real directory");
            }
            verify_current_owner(&metadata, "Pi supervisor state directory")?;
        }
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
            fs::create_dir_all(path)?;
        }
        Err(error) => return Err(error).context("failed to inspect Pi supervisor state directory"),
    }
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        fs::set_permissions(path, fs::Permissions::from_mode(0o700))?;
    }
    let metadata = fs::symlink_metadata(path)?;
    if !metadata.file_type().is_dir() || metadata.file_type().is_symlink() {
        bail!("Pi supervisor state path changed while securing it");
    }
    verify_current_owner(&metadata, "Pi supervisor state directory")?;
    Ok(())
}

fn private_append_file(path: &Path) -> Result<File> {
    let mut options = OpenOptions::new();
    options.create(true).append(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.mode(0o600).custom_flags(libc::O_NOFOLLOW);
    }
    let file = options
        .open(path)
        .with_context(|| format!("failed to open {}", path.display()))?;
    let metadata = file.metadata()?;
    if !metadata.file_type().is_file() {
        bail!("refusing to use a non-regular Pi supervisor log");
    }
    verify_current_owner(&metadata, "Pi supervisor log")?;
    verify_private_mode(&metadata, "Pi supervisor log")?;
    Ok(file)
}

fn load_record(path: &Path, state_dir: &Path) -> Result<Option<SupervisorRecord>> {
    let mut options = OpenOptions::new();
    options.read(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.custom_flags(libc::O_NOFOLLOW);
    }
    let file = match options.open(path) {
        Ok(file) => file,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(None),
        Err(error) => return Err(error).context("failed to read Pi supervisor record"),
    };
    let metadata = file.metadata()?;
    if !metadata.file_type().is_file() {
        bail!("refusing to use a non-regular Pi supervisor record");
    }
    verify_current_owner(&metadata, "Pi supervisor record")?;
    verify_private_mode(&metadata, "Pi supervisor record")?;
    let mut input = Vec::new();
    file.take((MAX_MESSAGE_BYTES + 1) as u64)
        .read_to_end(&mut input)?;
    if input.len() > MAX_MESSAGE_BYTES {
        bail!("Pi supervisor record exceeded size limit");
    }
    let record: SupervisorRecord =
        serde_json::from_slice(&input).context("invalid Pi supervisor record")?;
    if record.socket_path.parent() != Some(state_dir) {
        bail!("Pi supervisor socket escaped its private state directory");
    }
    Ok(Some(record))
}

#[cfg_attr(not(target_os = "linux"), allow(dead_code))]
fn save_record(path: &Path, record: &SupervisorRecord) -> Result<()> {
    let temporary = path.with_extension(format!("tmp-{}-{}", std::process::id(), now_millis()));
    let mut options = OpenOptions::new();
    options.create_new(true).write(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.mode(0o600).custom_flags(libc::O_NOFOLLOW);
    }
    let result = (|| -> Result<()> {
        let mut file = options.open(&temporary)?;
        serde_json::to_writer_pretty(&mut file, record)?;
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

fn verify_current_owner(metadata: &fs::Metadata, description: &str) -> Result<()> {
    #[cfg(unix)]
    {
        use std::os::unix::fs::MetadataExt;
        if metadata.uid() != unsafe { libc::geteuid() } {
            bail!("{description} is not owned by the current user");
        }
    }
    #[cfg(not(unix))]
    let _ = (metadata, description);
    Ok(())
}

fn verify_private_mode(metadata: &fs::Metadata, description: &str) -> Result<()> {
    #[cfg(unix)]
    {
        use std::os::unix::fs::MetadataExt;
        if metadata.mode() & 0o077 != 0 {
            bail!("{description} is accessible by another user");
        }
    }
    #[cfg(not(unix))]
    let _ = (metadata, description);
    Ok(())
}

fn verify_process(record: &SupervisorRecord) -> Result<bool> {
    let (start, state) = match process_stat(record.pid) {
        Ok(value) => value,
        Err(error) if is_missing_process(&error) => return Ok(false),
        Err(error) => return Err(error),
    };
    if state == "Z" {
        return Ok(false);
    }
    let cmdline = match process_cmdline(record.pid) {
        Ok(value) => value,
        Err(error) if is_missing_process(&error) => return Ok(false),
        Err(error) => return Err(error),
    };
    Ok(start == record.process_start_token && cmdline == record.process_cmdline)
}

fn record_uses_pi_executable(record: &SupervisorRecord, configured: &str) -> bool {
    let (recorded_path, recorded_home) = recorded_process_environment(record);
    let configured_path = std::env::var_os("PATH");
    let configured_home = std::env::var_os("HOME");
    pi_executables_match_from(
        &record.pi_bin,
        configured,
        recorded_path.as_deref(),
        recorded_home.as_deref(),
        configured_path.as_deref(),
        configured_home.as_deref(),
    )
}

fn pi_executables_match_from(
    recorded: &str,
    configured: &str,
    recorded_path: Option<&std::ffi::OsStr>,
    recorded_home: Option<&std::ffi::OsStr>,
    configured_path: Option<&std::ffi::OsStr>,
    configured_home: Option<&std::ffi::OsStr>,
) -> bool {
    if recorded == configured {
        return true;
    }
    let Some(recorded) = resolve_pi_executable(recorded, recorded_path, recorded_home) else {
        return false;
    };
    let Some(configured) = resolve_pi_executable(configured, configured_path, configured_home)
    else {
        return false;
    };
    recorded == configured
}

#[cfg(target_os = "linux")]
fn recorded_process_environment(
    record: &SupervisorRecord,
) -> (Option<std::ffi::OsString>, Option<std::ffi::OsString>) {
    use std::os::unix::ffi::OsStringExt;

    let Ok(environment) = fs::read(format!("/proc/{}/environ", record.pid)) else {
        return (None, None);
    };
    let value = |name: &[u8]| {
        environment.split(|byte| *byte == 0).find_map(|entry| {
            let separator = entry.iter().position(|byte| *byte == b'=')?;
            let (key, value) = entry.split_at(separator);
            let value = value.get(1..)?;
            (key == name).then(|| std::ffi::OsString::from_vec(value.to_vec()))
        })
    };
    (value(b"PATH"), value(b"HOME"))
}

#[cfg(not(target_os = "linux"))]
fn recorded_process_environment(
    _record: &SupervisorRecord,
) -> (Option<std::ffi::OsString>, Option<std::ffi::OsString>) {
    (std::env::var_os("PATH"), std::env::var_os("HOME"))
}

fn resolve_pi_executable(
    program: &str,
    search_path: Option<&std::ffi::OsStr>,
    home: Option<&std::ffi::OsStr>,
) -> Option<PathBuf> {
    let path = Path::new(program);
    let candidate = if path.components().count() > 1 {
        executable_file(path).then(|| path.to_path_buf())
    } else {
        search_path
            .and_then(|value| {
                std::env::split_paths(value)
                    .map(|directory| directory.join(program))
                    .find(|candidate| executable_file(candidate))
            })
            .or_else(|| {
                let home = PathBuf::from(home?);
                [".local/bin", ".npm-global/bin"]
                    .into_iter()
                    .map(|directory| home.join(directory).join(program))
                    .find(|candidate| executable_file(candidate))
            })
    }?;
    fs::canonicalize(candidate).ok()
}

fn executable_file(path: &Path) -> bool {
    let Ok(metadata) = path.metadata() else {
        return false;
    };
    if !metadata.is_file() {
        return false;
    }
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        metadata.permissions().mode() & 0o111 != 0
    }
    #[cfg(not(unix))]
    {
        true
    }
}

fn supports_model_launch(capabilities: &Value) -> bool {
    supports_feature(capabilities, MODEL_LAUNCH_FEATURE)
}

fn supports_feature(capabilities: &Value, expected: &str) -> bool {
    capabilities
        .get("features")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
        .filter_map(Value::as_str)
        .any(|feature| feature == expected)
}

fn wait_for_verified_process_exit(record: &SupervisorRecord, timeout: Duration) -> Result<()> {
    let deadline = Instant::now() + timeout;
    loop {
        if !verify_process(record)? {
            return Ok(());
        }
        if Instant::now() >= deadline {
            bail!("verified Pi supervisor process did not exit before the deadline");
        }
        thread::sleep(Duration::from_millis(10));
    }
}

fn is_missing_process(error: &anyhow::Error) -> bool {
    error
        .downcast_ref::<std::io::Error>()
        .map(|error| error.kind() == std::io::ErrorKind::NotFound)
        .unwrap_or(false)
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
    let start = fields
        .get(19)
        .context("/proc process stat omitted starttime")?;
    Ok(((*start).into(), (*state).into()))
}

#[cfg(not(target_os = "linux"))]
fn process_stat(_: u32) -> Result<(String, String)> {
    bail!("process start-token verification is unavailable on this platform")
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
    bail!("process command-line verification is unavailable on this platform")
}

fn wait_for_socket(path: &Path, timeout: Duration) -> Result<()> {
    #[cfg(not(unix))]
    {
        let _ = (path, timeout);
        bail!("Pi supervisor socket is unavailable on this platform")
    }
    #[cfg(unix)]
    {
        use std::os::unix::fs::FileTypeExt;
        use std::os::unix::net::UnixStream;

        let deadline = Instant::now() + timeout;
        loop {
            let ready = fs::symlink_metadata(path)
                .map(|metadata| metadata.file_type().is_socket())
                .unwrap_or(false)
                && UnixStream::connect(path).is_ok();
            if ready {
                return Ok(());
            }
            if Instant::now() >= deadline {
                bail!("timed out waiting for Pi supervisor socket");
            }
            thread::sleep(Duration::from_millis(25));
        }
    }
}

struct StateLock {
    file: File,
}

impl StateLock {
    fn acquire(path: &Path) -> Result<Self> {
        let mut options = OpenOptions::new();
        options.create(true).read(true).write(true);
        #[cfg(unix)]
        {
            use std::os::unix::fs::OpenOptionsExt;
            options.mode(0o600).custom_flags(libc::O_NOFOLLOW);
        }
        let file = options.open(path)?;
        let metadata = file.metadata()?;
        if !metadata.file_type().is_file() {
            bail!("refusing to use a non-regular Pi supervisor lock");
        }
        verify_current_owner(&metadata, "Pi supervisor lock")?;
        verify_private_mode(&metadata, "Pi supervisor lock")?;
        #[cfg(unix)]
        {
            use std::os::unix::io::AsRawFd;
            if unsafe { libc::flock(file.as_raw_fd(), libc::LOCK_EX) } != 0 {
                return Err(std::io::Error::last_os_error()).context("failed to lock Pi state");
            }
        }
        Ok(Self { file })
    }
}

impl Drop for StateLock {
    fn drop(&mut self) {
        #[cfg(unix)]
        {
            use std::os::unix::io::AsRawFd;
            let _ = unsafe { libc::flock(self.file.as_raw_fd(), libc::LOCK_UN) };
        }
    }
}

fn now_millis() -> u64 {
    SystemTime::now()
        .duration_since(SystemTime::UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis() as u64
}

fn validate_pi_model(model: &str) -> Result<&str> {
    let model = model.trim();
    if model.is_empty() || model.len() > 128 {
        bail!("the Pi model name must contain between 1 and 128 bytes");
    }
    if model
        .chars()
        .any(|character| character.is_control() || character.is_whitespace())
    {
        bail!("the Pi model name cannot contain whitespace or control characters");
    }
    Ok(model)
}

#[cfg(test)]
mod tests {
    use super::*;
    #[cfg(unix)]
    use std::os::unix::fs::PermissionsExt;

    #[test]
    fn detects_model_launch_protocol_capability() {
        assert!(!supports_model_launch(&json!({"version": 1})));
        assert!(supports_model_launch(&json!({
            "version": 1,
            "features": ["launch_with_model"]
        })));
    }

    #[cfg(unix)]
    #[test]
    fn legacy_bare_pi_name_matches_the_same_resolved_executable() {
        let directory = tempfile::tempdir().unwrap();
        let bin = directory.path().join(".local/bin");
        fs::create_dir_all(&bin).unwrap();
        let executable = bin.join("pi");
        fs::write(&executable, b"#!/bin/sh\n").unwrap();
        fs::set_permissions(&executable, fs::Permissions::from_mode(0o700)).unwrap();

        assert_eq!(
            resolve_pi_executable("pi", None, Some(directory.path().as_os_str())),
            fs::canonicalize(&executable).ok()
        );
        assert!(pi_executables_match_from(
            "pi",
            executable.to_str().unwrap(),
            None,
            Some(directory.path().as_os_str()),
            None,
            Some(directory.path().as_os_str())
        ));
        assert_eq!(
            resolve_pi_executable(
                executable.to_str().unwrap(),
                None,
                Some(directory.path().as_os_str())
            ),
            fs::canonicalize(&executable).ok()
        );
    }

    #[cfg(unix)]
    #[test]
    fn executable_resolution_does_not_equate_different_pi_binaries() {
        let directory = tempfile::tempdir().unwrap();
        let first = directory.path().join("first-pi");
        let second = directory.path().join("second-pi");
        for executable in [&first, &second] {
            fs::write(executable, b"#!/bin/sh\n").unwrap();
            fs::set_permissions(executable, fs::Permissions::from_mode(0o700)).unwrap();
        }

        assert_ne!(
            resolve_pi_executable(first.to_str().unwrap(), None, None),
            resolve_pi_executable(second.to_str().unwrap(), None, None)
        );
    }

    #[cfg(target_os = "linux")]
    #[test]
    fn old_protocol_mock_refuses_upgrade_while_it_owns_active_work() {
        use std::os::unix::net::UnixListener;

        let directory = tempfile::tempdir().unwrap();
        let state_dir = directory.path().join("state");
        let supervisor = PiSupervisor::with_state_dir_and_exe(
            "pi",
            state_dir.clone(),
            std::env::current_exe().unwrap(),
        )
        .unwrap();
        let socket_path = state_dir.join("old.sock");
        let listener = UnixListener::bind(&socket_path).unwrap();
        let server = thread::spawn(move || {
            for request_number in 0..2 {
                let (mut stream, _) = listener.accept().unwrap();
                let request: DaemonRequest = serde_json::from_reader(&mut stream).unwrap();
                let response = match (request_number, request) {
                    (0, DaemonRequest::Ping) => {
                        DaemonResponse::success(json!({"version": 1})).unwrap()
                    }
                    (1, DaemonRequest::List) => DaemonResponse::success(vec![ManagedPiSession {
                        id: "old-active".into(),
                        cwd: PathBuf::from("/work"),
                        name: "important active task".into(),
                        pid: 42,
                        process_start_token: "token".into(),
                        state: SessionState::Working,
                        summary: String::new(),
                        session_file: None,
                        pending: None,
                        alive: true,
                        created_at_ms: 1,
                        updated_at_ms: 1,
                    }])
                    .unwrap(),
                    (_, request) => panic!("unexpected old-daemon request: {request:?}"),
                };
                serde_json::to_writer(&mut stream, &response).unwrap();
                stream.flush().unwrap();
            }
        });
        let pid = std::process::id();
        let record = SupervisorRecord {
            version: RECORD_VERSION,
            pid,
            process_start_token: process_start_token(pid).unwrap(),
            process_cmdline: process_cmdline(pid).unwrap(),
            pi_bin: "pi".into(),
            socket_path,
            created_at_ms: 1,
        };

        let error = supervisor.model_launch_endpoint(record).unwrap_err();

        assert!(format!("{error:#}").contains("predates model selection"));
        assert!(format!("{error:#}").contains("important active task"));
        server.join().unwrap();
    }

    #[cfg(target_os = "linux")]
    #[test]
    fn wait_for_stop_tolerates_a_legacy_socket_disappearing_before_process_exit() {
        let directory = tempfile::tempdir().unwrap();
        let state_dir = directory.path().join("state");
        let supervisor = PiSupervisor::with_state_dir_and_exe(
            "pi",
            state_dir.clone(),
            std::env::current_exe().unwrap(),
        )
        .unwrap();
        let mut child = Command::new("sleep").arg("0.15").spawn().unwrap();
        let pid = child.id();
        let record = SupervisorRecord {
            version: RECORD_VERSION,
            pid,
            process_start_token: process_start_token(pid).unwrap(),
            process_cmdline: process_cmdline(pid).unwrap(),
            pi_bin: "pi".into(),
            socket_path: state_dir.join("already-removed.sock"),
            created_at_ms: 1,
        };
        save_record(&supervisor.record_path, &record).unwrap();

        supervisor
            .wait_until_stopped("legacy-id", Duration::from_secs(1))
            .unwrap();
        child.wait().unwrap();
    }

    #[test]
    fn formats_rpc_message_content() {
        let response = json!({
            "data": {"messages": [
                {"role": "user", "content": "Build it"},
                {"role": "assistant", "content": [{"type": "text", "text": "Done"}]}
            ]}
        });
        assert_eq!(
            format_rpc_messages(&response).unwrap(),
            "User: Build it\n\nAssistant: Done"
        );
    }

    #[test]
    fn reconciles_confirmation_without_granting_it_implicitly() {
        let state = Arc::new(Mutex::new(LiveState {
            name: "task".into(),
            state: SessionState::Working,
            summary: String::new(),
            pending: None,
            alive: true,
            updated_at_ms: 0,
        }));
        reconcile_rpc_event(
            &state,
            &json!({
                "type": "extension_ui_request",
                "id": "exact-id",
                "method": "confirm",
                "title": "Dangerous command",
                "message": "Allow?"
            }),
        );
        let state = state.lock().unwrap();
        assert_eq!(state.state, SessionState::NeedsInput);
        assert_eq!(state.pending.as_ref().unwrap().id, "exact-id");
        assert_eq!(state.pending.as_ref().unwrap().kind, PiPendingKind::Confirm);
    }

    #[cfg(unix)]
    #[test]
    fn refuses_symlinked_state_directories() {
        let directory = tempfile::tempdir().unwrap();
        let real = directory.path().join("real");
        fs::create_dir(&real).unwrap();
        let link = directory.path().join("state");
        std::os::unix::fs::symlink(real, &link).unwrap();

        assert!(ensure_private_directory(&link).is_err());
    }

    #[cfg(unix)]
    #[test]
    fn refuses_a_permissive_or_replaced_ownership_record() {
        let directory = tempfile::tempdir().unwrap();
        ensure_private_directory(directory.path()).unwrap();
        let record_path = directory.path().join("supervisor.json");
        let record = SupervisorRecord {
            version: RECORD_VERSION,
            pid: std::process::id(),
            process_start_token: "token".into(),
            process_cmdline: b"command".to_vec(),
            pi_bin: "pi".into(),
            socket_path: directory.path().join("rpc.sock"),
            created_at_ms: 1,
        };
        save_record(&record_path, &record).unwrap();
        fs::set_permissions(&record_path, fs::Permissions::from_mode(0o644)).unwrap();
        assert!(load_record(&record_path, directory.path()).is_err());

        fs::remove_file(&record_path).unwrap();
        std::os::unix::fs::symlink("missing", &record_path).unwrap();
        assert!(load_record(&record_path, directory.path()).is_err());
    }
}

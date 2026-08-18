use std::collections::{BTreeMap, BTreeSet};
use std::fs::{self, File, OpenOptions};
use std::io::Write;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::sync::Mutex;
use std::thread;
use std::time::{Duration, Instant, SystemTime};

use anyhow::{anyhow, bail, Context, Result};
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};

use crate::codex_rpc::{AppServerClient, AppServerInvocation};
use crate::domain::{AgentSession, Capability, Provider, Runtime, SessionSnapshot, SessionState};

const RECORD_VERSION: u32 = 1;
const STARTUP_TIMEOUT: Duration = Duration::from_secs(8);
const MAX_TRANSCRIPT_CHARS: usize = 32 * 1024;

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
struct OwnedThread {
    cwd: PathBuf,
    created_at_ms: u64,
    active_turn_id: Option<String>,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
struct SupervisorRecord {
    version: u32,
    pid: u32,
    process_start_token: String,
    process_cmdline: Vec<u8>,
    codex_bin: String,
    socket_path: PathBuf,
    created_at_ms: u64,
    #[serde(default)]
    threads: BTreeMap<String, OwnedThread>,
}

impl SupervisorRecord {
    fn same_process(&self, other: &Self) -> bool {
        self.pid == other.pid
            && self.process_start_token == other.process_start_token
            && self.process_cmdline == other.process_cmdline
    }
}

#[derive(Default)]
struct ControlConnection {
    server: Option<SupervisorRecord>,
    client: Option<AppServerClient>,
    pending_requests: Vec<PendingRequest>,
}

#[derive(Clone, Debug)]
struct PendingRequest {
    id: Value,
    thread_id: String,
    turn_id: String,
    item_id: String,
    kind: PendingRequestKind,
    resolving: bool,
    expires_at: Option<Instant>,
}

#[derive(Clone, Debug)]
enum PendingRequestKind {
    Approval {
        summary: String,
        accept_result: Option<Value>,
        decline_result: Option<Value>,
    },
    UserInput {
        questions: Vec<PendingQuestion>,
        answers: BTreeMap<String, Vec<String>>,
    },
    Unsupported {
        summary: String,
    },
}

#[derive(Clone, Debug)]
struct PendingQuestion {
    id: String,
    header: String,
    question: String,
    options: Vec<String>,
    allow_other: bool,
    secret: bool,
}

/// Owns a durable, reconnectable host Codex App Server.
///
/// The server listens on a user-private Unix socket and intentionally outlives
/// the dashboard process. Each dashboard connection speaks the App Server's
/// WebSocket protocol directly over that socket.
/// Persisted PID identity is verified before reuse; this type never signals a
/// PID loaded from disk.
pub struct CodexSupervisor {
    codex_bin: String,
    state_dir: PathBuf,
    record_path: PathBuf,
    lock_path: PathBuf,
    client_transport: SupervisorClientTransport,
    response_lease: Option<ResponseLease>,
    control: Mutex<ControlConnection>,
}

#[derive(Clone, Copy)]
enum SupervisorClientTransport {
    UnixWebSocket,
    #[cfg(test)]
    ProcessProxy,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum CodexReplyMode {
    Started,
    Steered,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct CodexInputProgress {
    pub answered: usize,
    pub total: usize,
    pub submitted: bool,
}

impl CodexSupervisor {
    pub fn host(codex_bin: impl Into<String>) -> Result<Self> {
        Self::with_state_dir_and_transport(
            codex_bin,
            default_state_dir()?,
            SupervisorClientTransport::UnixWebSocket,
        )
    }

    #[cfg(test)]
    fn with_state_dir(codex_bin: impl Into<String>, state_dir: PathBuf) -> Result<Self> {
        Self::with_state_dir_and_transport(
            codex_bin,
            state_dir,
            SupervisorClientTransport::ProcessProxy,
        )
    }

    fn with_state_dir_and_transport(
        codex_bin: impl Into<String>,
        state_dir: PathBuf,
        client_transport: SupervisorClientTransport,
    ) -> Result<Self> {
        ensure_private_directory(&state_dir)?;
        let response_lease = ResponseLease::try_acquire(&state_dir.join("controller.lock"))?;
        Ok(Self {
            codex_bin: codex_bin.into(),
            record_path: state_dir.join("supervisor.json"),
            lock_path: state_dir.join("supervisor.lock"),
            state_dir,
            client_transport,
            response_lease,
            control: Mutex::new(ControlConnection::default()),
        })
    }

    pub(crate) fn connect_client(&self) -> Result<AppServerClient> {
        let record = self.ensure_endpoint()?;
        AppServerClient::connect(&self.client_invocation(&record))
    }

    pub fn launch(&self, prompt: &str, cwd: &Path) -> Result<String> {
        let prompt = prompt.trim();
        if prompt.is_empty() {
            bail!("the launch prompt cannot be empty");
        }
        if !cwd.is_absolute() {
            bail!("the Codex launch directory must be absolute");
        }
        let server = self.ensure_endpoint()?;
        let mut control = self
            .control
            .lock()
            .map_err(|_| anyhow!("Codex supervisor connection lock was poisoned"))?;
        let client = self.control_client(&mut control, &server)?;
        let started = client.request(
            "thread/start",
            json!({
                "cwd": cwd,
                "approvalPolicy": "on-request",
                "sandbox": "workspace-write",
                "serviceName": "open_agent_view"
            }),
        )?;
        let thread_id = started
            .pointer("/thread/id")
            .and_then(Value::as_str)
            .context("thread/start response omitted thread.id")?
            .to_owned();
        self.update_record_for_server(&server, |record| {
            record.threads.insert(
                thread_id.clone(),
                OwnedThread {
                    cwd: cwd.to_path_buf(),
                    created_at_ms: now_millis(),
                    active_turn_id: None,
                },
            );
            Ok(())
        })?;

        let turn = client.request(
            "turn/start",
            json!({
                "threadId": thread_id,
                "input": [{"type": "text", "text": prompt}]
            }),
        )?;
        let turn_id = turn
            .pointer("/turn/id")
            .and_then(Value::as_str)
            .context("turn/start response omitted turn.id")?
            .to_owned();
        self.update_record_for_server(&server, |record| {
            let owned = record
                .threads
                .get_mut(&thread_id)
                .context("new Codex thread disappeared from the ownership record")?;
            owned.active_turn_id = Some(turn_id);
            Ok(())
        })?;
        Ok(thread_id)
    }

    pub fn interrupt(&self, session: &AgentSession) -> Result<()> {
        if session.provider != Provider::Codex || session.runtime != Runtime::Host {
            bail!("the host Codex supervisor does not own this runtime");
        }
        let server = self.ensure_endpoint()?;
        let turn_id = server
            .threads
            .get(&session.provider_session_id)
            .and_then(|thread| thread.active_turn_id.clone())
            .context(
                "refusing to interrupt a Codex thread not actively owned by this supervisor",
            )?;
        let mut control = self
            .control
            .lock()
            .map_err(|_| anyhow!("Codex supervisor connection lock was poisoned"))?;
        self.control_client(&mut control, &server)?.request(
            "turn/interrupt",
            json!({
                "threadId": session.provider_session_id,
                "turnId": turn_id
            }),
        )?;
        self.update_record_for_server(&server, |record| {
            if let Some(thread) = record.threads.get_mut(&session.provider_session_id) {
                thread.active_turn_id = None;
            }
            Ok(())
        })?;
        Ok(())
    }

    pub fn inspect(&self, session: &AgentSession) -> Result<String> {
        let (server, _) = self.owned_thread(session)?;
        let mut control = self
            .control
            .lock()
            .map_err(|_| anyhow!("Codex supervisor connection lock was poisoned"))?;
        let response = self.control_client(&mut control, &server)?.request(
            "thread/read",
            json!({
                "threadId": session.provider_session_id,
                "includeTurns": true
            }),
        )?;
        self.refresh_pending_locked(&mut control, &server)?;
        let mut transcript = format_thread_transcript(&response)?;
        if let Some(request) = control
            .pending_requests
            .iter()
            .find(|request| request.thread_id == session.provider_session_id)
        {
            transcript.push_str("\n\n");
            transcript.push_str(&format_pending_request(request));
        }
        Ok(limit_transcript(transcript))
    }

    pub fn reply(&self, session: &AgentSession, prompt: &str) -> Result<CodexReplyMode> {
        let prompt = prompt.trim();
        if prompt.is_empty() {
            bail!("the Codex reply cannot be empty");
        }
        let (server, owned) = self.owned_thread(session)?;
        let mut control = self
            .control
            .lock()
            .map_err(|_| anyhow!("Codex supervisor connection lock was poisoned"))?;
        let client = self.control_client(&mut control, &server)?;
        if let Some(expected_turn_id) = owned.active_turn_id {
            if session.state != SessionState::Working {
                bail!("refusing to steer without an active provider state");
            }
            let response = client.request(
                "turn/steer",
                json!({
                    "threadId": session.provider_session_id,
                    "input": [{"type": "text", "text": prompt}],
                    "expectedTurnId": expected_turn_id
                }),
            )?;
            let accepted_turn_id = response
                .get("turnId")
                .and_then(Value::as_str)
                .context("turn/steer response omitted turnId")?;
            if accepted_turn_id != expected_turn_id {
                bail!("turn/steer accepted an unexpected active turn ID");
            }
            return Ok(CodexReplyMode::Steered);
        }
        if session.state != SessionState::Completed {
            bail!("refusing to start a turn unless the owned thread is known idle");
        }
        let response = client.request(
            "turn/start",
            json!({
                "threadId": session.provider_session_id,
                "input": [{"type": "text", "text": prompt}]
            }),
        )?;
        let turn_id = response
            .pointer("/turn/id")
            .and_then(Value::as_str)
            .context("turn/start response omitted turn.id")?
            .to_owned();
        self.update_record_for_server(&server, |record| {
            let thread = record
                .threads
                .get_mut(&session.provider_session_id)
                .context("owned Codex thread disappeared while recording its new turn")?;
            thread.active_turn_id = Some(turn_id);
            Ok(())
        })?;
        Ok(CodexReplyMode::Started)
    }

    pub fn respond_approval(&self, session: &AgentSession, accept: bool) -> Result<()> {
        if self.response_lease.is_none() {
            bail!(
                "another Open Agent View process holds inline Codex response authority; use that dashboard or the native session"
            );
        }
        let (server, owned) = self.owned_thread(session)?;
        let mut control = self
            .control
            .lock()
            .map_err(|_| anyhow!("Codex supervisor connection lock was poisoned"))?;
        self.refresh_pending_locked(&mut control, &server)?;
        let position = control
            .pending_requests
            .iter()
            .position(|request| {
                request.thread_id == session.provider_session_id
                    && !request.resolving
                    && matches!(request.kind, PendingRequestKind::Approval { .. })
            })
            .context("no actionable approval request is pending for this Codex thread")?;
        let request = &control.pending_requests[position];
        if owned.active_turn_id.as_deref() != Some(request.turn_id.as_str()) {
            bail!("the pending approval no longer belongs to the exact active turn");
        }
        let response = match &request.kind {
            PendingRequestKind::Approval {
                accept_result,
                decline_result,
                ..
            } => {
                if accept {
                    accept_result.clone()
                } else {
                    decline_result.clone()
                }
            }
            _ => unreachable!("position selected an approval"),
        }
        .context(if accept {
            "this request cannot be safely accepted inline; open the native session"
        } else {
            "this request does not offer a safe inline decline"
        })?;
        let id = request.id.clone();
        control
            .client
            .as_mut()
            .context("Codex control connection disappeared")?
            .respond(id, response)?;
        control.pending_requests[position].resolving = true;
        Ok(())
    }

    pub fn respond_user_input(
        &self,
        session: &AgentSession,
        answer: &str,
    ) -> Result<CodexInputProgress> {
        if self.response_lease.is_none() {
            bail!(
                "another Open Agent View process holds inline Codex response authority; use that dashboard or the native session"
            );
        }
        let answer = answer.trim();
        if answer.is_empty() {
            bail!("the Codex user-input answer cannot be empty");
        }
        let (server, owned) = self.owned_thread(session)?;
        let mut control = self
            .control
            .lock()
            .map_err(|_| anyhow!("Codex supervisor connection lock was poisoned"))?;
        self.refresh_pending_locked(&mut control, &server)?;
        let position = control
            .pending_requests
            .iter()
            .position(|request| {
                request.thread_id == session.provider_session_id
                    && !request.resolving
                    && matches!(request.kind, PendingRequestKind::UserInput { .. })
            })
            .context("no actionable structured-input request is pending for this Codex thread")?;
        if owned.active_turn_id.as_deref()
            != Some(control.pending_requests[position].turn_id.as_str())
        {
            bail!("the pending input request no longer belongs to the exact active turn");
        }
        if control.pending_requests[position]
            .expires_at
            .is_some_and(|deadline| Instant::now() >= deadline)
        {
            bail!("the Codex structured-input request has reached its auto-resolution deadline");
        }

        let (answered, total, completed_response) = {
            let request = &mut control.pending_requests[position];
            let PendingRequestKind::UserInput { questions, answers } = &mut request.kind else {
                unreachable!("position selected a user-input request")
            };
            if let Some(question) = questions
                .iter()
                .find(|question| !answers.contains_key(&question.id))
            {
                if question.secret {
                    bail!(
                        "secret Codex input is unavailable in the visible dashboard composer; open the native session"
                    );
                }
                let answer = normalize_question_answer(question, answer)?;
                answers.insert(question.id.clone(), vec![answer]);
            }
            let answered = answers.len();
            let total = questions.len();
            let completed = (answered == total).then(|| {
                let id = request.id.clone();
                let values = answers
                    .iter()
                    .map(|(id, answers)| (id.clone(), json!({"answers": answers})))
                    .collect::<serde_json::Map<_, _>>();
                (id, json!({"answers": values}))
            });
            (answered, total, completed)
        };

        let submitted = if let Some((id, response)) = completed_response {
            control
                .client
                .as_mut()
                .context("Codex control connection disappeared")?
                .respond(id, response)?;
            control.pending_requests[position].resolving = true;
            true
        } else {
            false
        };
        Ok(CodexInputProgress {
            answered,
            total,
            submitted,
        })
    }

    pub fn archive(&self, session: &AgentSession) -> Result<()> {
        let (server, owned) = self.owned_thread(session)?;
        require_idle_mutation(session, &owned, "archive")?;
        let mut control = self
            .control
            .lock()
            .map_err(|_| anyhow!("Codex supervisor connection lock was poisoned"))?;
        self.control_client(&mut control, &server)?.request(
            "thread/archive",
            json!({"threadId": session.provider_session_id}),
        )?;
        Ok(())
    }

    pub fn delete(&self, session: &AgentSession) -> Result<()> {
        let (server, owned) = self.owned_thread(session)?;
        require_idle_mutation(session, &owned, "delete")?;
        let mut control = self
            .control
            .lock()
            .map_err(|_| anyhow!("Codex supervisor connection lock was poisoned"))?;
        self.control_client(&mut control, &server)?.request(
            "thread/delete",
            json!({"threadId": session.provider_session_id}),
        )?;
        self.update_record_for_server(&server, |record| {
            record.threads.remove(&session.provider_session_id);
            Ok(())
        })?;
        Ok(())
    }

    pub fn enrich(&self, snapshot: &mut SessionSnapshot) {
        let Ok(mut record) = self.live_record() else {
            return;
        };
        let pending_requests = match self.control.lock() {
            Ok(mut control) => {
                if self.response_lease.is_some()
                    && record
                        .threads
                        .values()
                        .any(|thread| thread.active_turn_id.is_some())
                {
                    if let Err(error) = self.refresh_pending_locked(&mut control, &record) {
                        control.client = None;
                        control.server = None;
                        control.pending_requests.clear();
                        snapshot.warnings.push(format!(
                            "Codex control request synchronization failed: {error:#}"
                        ));
                    }
                }
                control.pending_requests.clone()
            }
            Err(_) => {
                snapshot
                    .warnings
                    .push("Codex supervisor connection lock was poisoned".into());
                Vec::new()
            }
        };
        let mut changed = false;
        for session in &mut snapshot.sessions {
            if session.provider != Provider::Codex || session.runtime != Runtime::Host {
                continue;
            }
            let Some(owned) = record.threads.get_mut(&session.provider_session_id) else {
                continue;
            };
            session.capabilities.insert(Capability::Inspect);
            let pending = pending_requests.iter().find(|request| {
                request.thread_id == session.provider_session_id
                    && owned.active_turn_id.as_deref() == Some(request.turn_id.as_str())
            });
            if let Some(request) = pending {
                session.state = SessionState::NeedsInput;
                match &request.kind {
                    PendingRequestKind::Approval {
                        accept_result,
                        decline_result,
                        ..
                    } if self.response_lease.is_some() && !request.resolving => {
                        if accept_result.is_some() {
                            session.capabilities.insert(Capability::Approve);
                        }
                        if decline_result.is_some() {
                            session.capabilities.insert(Capability::Decline);
                        }
                    }
                    PendingRequestKind::UserInput { questions, answers }
                        if self.response_lease.is_some()
                            && !request.resolving
                            && !request
                                .expires_at
                                .is_some_and(|deadline| Instant::now() >= deadline)
                            && questions
                                .iter()
                                .find(|question| !answers.contains_key(&question.id))
                                .is_some_and(|question| !question.secret) =>
                    {
                        session.capabilities.insert(Capability::Respond);
                    }
                    _ => {}
                }
            }
            if matches!(
                session.state,
                SessionState::Working | SessionState::NeedsInput
            ) && owned.active_turn_id.is_some()
            {
                session.capabilities.insert(Capability::Interrupt);
                if session.state == SessionState::Working {
                    session.capabilities.insert(Capability::Reply);
                }
            } else if session.state == SessionState::Completed
                && owned.active_turn_id.take().is_some()
            {
                changed = true;
            }
            if session.state == SessionState::Completed && owned.active_turn_id.is_none() {
                session.capabilities.insert(Capability::Reply);
                session.capabilities.insert(Capability::Archive);
                session.capabilities.insert(Capability::Delete);
            }
        }
        if changed {
            let _ = self.replace_record_if_same_server(&record);
        }
    }

    pub fn remote_url_if_owned(&self, session: &AgentSession) -> Option<String> {
        if session.provider != Provider::Codex || session.runtime != Runtime::Host {
            return None;
        }
        let record = self.live_record().ok()?;
        record
            .threads
            .contains_key(&session.provider_session_id)
            .then(|| format!("unix://{}", record.socket_path.display()))
    }

    fn control_client<'a>(
        &self,
        control: &'a mut ControlConnection,
        server: &SupervisorRecord,
    ) -> Result<&'a mut AppServerClient> {
        let current_matches = control
            .server
            .as_ref()
            .map(|current| current.same_process(server))
            .unwrap_or(false);
        if !current_matches || control.client.is_none() {
            let mut client = AppServerClient::connect(&self.client_invocation(server))?;
            for (thread_id, thread) in &server.threads {
                if thread.active_turn_id.is_none() {
                    continue;
                }
                client.request(
                    "thread/resume",
                    json!({
                        "threadId": thread_id,
                        "cwd": thread.cwd,
                        "approvalPolicy": "on-request",
                        "sandbox": "workspace-write"
                    }),
                )?;
            }
            control.pending_requests.clear();
            control.client = Some(client);
            control.server = Some(server.clone());
        }
        Ok(control.client.as_mut().expect("Codex client initialized"))
    }

    fn refresh_pending_locked(
        &self,
        control: &mut ControlConnection,
        server: &SupervisorRecord,
    ) -> Result<()> {
        let events = self.control_client(control, server)?.drain_events()?;
        for event in events {
            reconcile_pending_event(&mut control.pending_requests, server, event)?;
        }
        Ok(())
    }

    fn client_invocation(&self, server: &SupervisorRecord) -> AppServerInvocation {
        match self.client_transport {
            SupervisorClientTransport::UnixWebSocket => {
                AppServerInvocation::unix_websocket(server.socket_path.clone())
            }
            #[cfg(test)]
            SupervisorClientTransport::ProcessProxy => {
                AppServerInvocation::proxy(self.codex_bin.clone(), &server.socket_path)
            }
        }
    }

    fn owned_thread(&self, session: &AgentSession) -> Result<(SupervisorRecord, OwnedThread)> {
        if session.provider != Provider::Codex || session.runtime != Runtime::Host {
            bail!("the host Codex supervisor does not own this runtime");
        }
        let server = self.ensure_endpoint()?;
        let owned = server
            .threads
            .get(&session.provider_session_id)
            .cloned()
            .context("refusing to control a Codex thread not owned by this supervisor")?;
        Ok((server, owned))
    }

    fn ensure_endpoint(&self) -> Result<SupervisorRecord> {
        let _lock = StateLock::acquire(&self.lock_path)?;
        if let Some(record) = load_record(&self.record_path, &self.state_dir)? {
            if record.version != RECORD_VERSION {
                bail!(
                    "unsupported Codex supervisor record version {}",
                    record.version
                );
            }
            let live = verify_process(&record)?;
            if record.codex_bin != self.codex_bin && live {
                bail!(
                    "a verified Codex supervisor is already running with executable {}; configured executable is {}",
                    record.codex_bin,
                    self.codex_bin
                );
            }
            if live {
                wait_for_socket(&record.socket_path, Duration::from_secs(2)).with_context(|| {
                    format!(
                        "verified Codex supervisor {} is alive but its socket is unavailable; refusing to replace or signal it",
                        record.pid
                    )
                })?;
                return Ok(record);
            }
        }
        self.start_endpoint()
    }

    fn start_endpoint(&self) -> Result<SupervisorRecord> {
        let socket_path = self.state_dir.join(format!(
            "app-server-{}-{}.sock",
            std::process::id(),
            now_millis()
        ));
        if fs::symlink_metadata(&socket_path).is_ok() {
            bail!(
                "refusing to replace existing socket {}",
                socket_path.display()
            );
        }
        let log_path = self.state_dir.join("app-server.log");
        let log = private_append_file(&log_path)?;
        let listen = format!("unix://{}", socket_path.display());
        let mut command = Command::new(&self.codex_bin);
        command
            .args(["app-server", "--listen", &listen])
            .stdin(Stdio::null())
            .stdout(Stdio::from(log.try_clone()?))
            .stderr(Stdio::from(log));
        #[cfg(unix)]
        {
            use std::os::unix::process::CommandExt;
            command.process_group(0);
        }
        let mut child = command.spawn().with_context(|| {
            format!(
                "failed to start durable Codex App Server via {}",
                self.codex_bin
            )
        })?;
        let pid = child.id();
        let deadline = Instant::now() + STARTUP_TIMEOUT;
        loop {
            if let Some(status) = child
                .try_wait()
                .context("failed to poll Codex App Server")?
            {
                bail!("Codex App Server exited during startup with {status}");
            }
            if process_start_token(pid).is_ok() {
                break;
            }
            if Instant::now() >= deadline {
                let _ = child.kill();
                let _ = child.wait();
                bail!("could not verify the newly started Codex App Server process");
            }
            thread::sleep(Duration::from_millis(20));
        }
        if let Err(error) = wait_for_socket(
            &socket_path,
            deadline.saturating_duration_since(Instant::now()),
        ) {
            // The Child handle is the exact process just created by this call;
            // no persisted or unverified PID is signalled.
            let _ = child.kill();
            let _ = child.wait();
            return Err(error).context("new Codex App Server never opened its socket");
        }
        // Read identity only after the executable has opened the socket. This
        // avoids persisting the transient pre-exec command line of shebang or
        // npm launcher processes.
        let process_start_token = process_start_token(pid)?;
        let process_cmdline = process_cmdline(pid)?;
        if process_cmdline.is_empty() {
            let _ = child.kill();
            let _ = child.wait();
            bail!("new Codex App Server exposed an empty process command line");
        }

        let record = SupervisorRecord {
            version: RECORD_VERSION,
            pid,
            process_start_token,
            process_cmdline,
            codex_bin: self.codex_bin.clone(),
            socket_path,
            created_at_ms: now_millis(),
            threads: BTreeMap::new(),
        };
        save_record(&self.record_path, &record)?;
        // Dropping Child does not kill it. The new process group and redirected
        // standard streams allow this verified server to outlive the dashboard.
        drop(child);
        Ok(record)
    }

    fn live_record(&self) -> Result<SupervisorRecord> {
        let _lock = StateLock::acquire(&self.lock_path)?;
        let record = load_record(&self.record_path, &self.state_dir)?
            .context("Codex supervisor has not been started")?;
        if !verify_process(&record)? {
            bail!("persisted Codex supervisor process identity is no longer live");
        }
        Ok(record)
    }

    fn update_record_for_server(
        &self,
        server: &SupervisorRecord,
        update: impl FnOnce(&mut SupervisorRecord) -> Result<()>,
    ) -> Result<()> {
        let _lock = StateLock::acquire(&self.lock_path)?;
        let mut current = load_record(&self.record_path, &self.state_dir)?
            .context("Codex supervisor ownership record disappeared")?;
        if !current.same_process(server) || !verify_process(&current)? {
            bail!("Codex supervisor identity changed during the control operation");
        }
        update(&mut current)?;
        save_record(&self.record_path, &current)
    }

    fn replace_record_if_same_server(&self, replacement: &SupervisorRecord) -> Result<()> {
        let _lock = StateLock::acquire(&self.lock_path)?;
        let current = load_record(&self.record_path, &self.state_dir)?
            .context("Codex supervisor ownership record disappeared")?;
        if !current.same_process(replacement) || !verify_process(&current)? {
            bail!("Codex supervisor identity changed before ownership reconciliation");
        }
        save_record(&self.record_path, replacement)
    }
}

fn reconcile_pending_event(
    pending: &mut Vec<PendingRequest>,
    server: &SupervisorRecord,
    event: Value,
) -> Result<()> {
    let Some(method) = event.get("method").and_then(Value::as_str) else {
        return Ok(());
    };
    if method == "serverRequest/resolved" {
        if let (Some(thread_id), Some(request_id)) = (
            event.pointer("/params/threadId").and_then(Value::as_str),
            event.pointer("/params/requestId"),
        ) {
            pending.retain(|request| request.thread_id != thread_id || request.id != *request_id);
        }
        return Ok(());
    }
    if !matches!(
        method,
        "item/commandExecution/requestApproval"
            | "item/fileChange/requestApproval"
            | "item/tool/requestUserInput"
            | "item/permissions/requestApproval"
            | "mcpServer/elicitation/request"
    ) {
        return Ok(());
    }

    let Some(id) = event.get("id").cloned() else {
        return Ok(());
    };
    if !id.is_string() && id.as_i64().is_none() {
        return Ok(());
    }
    let Some(params) = event.get("params") else {
        return Ok(());
    };
    let Some(thread_id) = params.get("threadId").and_then(Value::as_str) else {
        return Ok(());
    };
    let Some(turn_id) = params.get("turnId").and_then(Value::as_str) else {
        return Ok(());
    };
    let Some(owned) = server.threads.get(thread_id) else {
        return Ok(());
    };
    if owned.active_turn_id.as_deref() != Some(turn_id) {
        return Ok(());
    }
    let item_id = params
        .get("itemId")
        .and_then(Value::as_str)
        .unwrap_or(method)
        .to_owned();
    let kind = match method {
        "item/commandExecution/requestApproval" => {
            let available = params.get("availableDecisions").and_then(Value::as_array);
            let accept_supported = available
                .map(|decisions| decisions.iter().any(|value| value.as_str() == Some("accept")))
                .unwrap_or(true);
            let decline_supported = available
                .map(|decisions| decisions.iter().any(|value| value.as_str() == Some("decline")))
                .unwrap_or(true);
            let command = params
                .get("command")
                .and_then(Value::as_str)
                .filter(|value| !value.is_empty());
            let cwd = params
                .get("cwd")
                .and_then(Value::as_str)
                .filter(|value| !value.is_empty());
            let mut lines = vec!["Codex requests permission to run a command.".to_owned()];
            push_optional_line(&mut lines, "Command", params.get("command"));
            push_optional_line(&mut lines, "Directory", params.get("cwd"));
            push_optional_line(&mut lines, "Reason", params.get("reason"));
            let network_visible = params
                .get("networkApprovalContext")
                .and_then(Value::as_object)
                .and_then(|network| {
                    let host = network
                        .get("host")
                        .and_then(Value::as_str)
                        .filter(|value| !value.is_empty())?;
                    let protocol = network.get("protocol").and_then(Value::as_str)?;
                    matches!(protocol, "http" | "https" | "socks5Tcp" | "socks5Udp")
                        .then(|| (host, protocol))
                });
            if let Some((host, protocol)) = network_visible {
                lines.push(format!("Network: {protocol}://{host}"));
            }
            let has_additional_permissions = params
                .get("additionalPermissions")
                .is_some_and(|value| !value.is_null());
            if has_additional_permissions {
                lines.push("Additional filesystem or network permissions are requested.".into());
            }
            PendingRequestKind::Approval {
                summary: lines.join("\n"),
                accept_result: (accept_supported
                    && !has_additional_permissions
                    && ((command.is_some() && cwd.is_some()) || network_visible.is_some()))
                .then(|| json!({"decision": "accept"})),
                decline_result: decline_supported.then(|| json!({"decision": "decline"})),
            }
        }
        "item/fileChange/requestApproval" => {
            let mut lines = vec!["Codex requests permission to apply file changes.".to_owned()];
            push_optional_line(&mut lines, "Reason", params.get("reason"));
            push_optional_line(&mut lines, "Requested write root", params.get("grantRoot"));
            PendingRequestKind::Approval {
                summary: lines.join("\n"),
                // The approval request has no diff. Decline is safe; accepting
                // requires a complete correlated item/started fileChange.
                accept_result: None,
                decline_result: Some(json!({"decision": "decline"})),
            }
        }
        "item/tool/requestUserInput" => {
            if let Some(questions) = parse_pending_questions(params) {
                PendingRequestKind::UserInput {
                    questions,
                    answers: BTreeMap::new(),
                }
            } else {
                PendingRequestKind::Unsupported {
                    summary: "Codex sent an empty or malformed structured-input request; open the native session to resolve it.".into(),
                }
            }
        }
        "item/permissions/requestApproval" => PendingRequestKind::Approval {
            summary: "Codex requests additional filesystem or network permissions. Open Agent View can deny the request inline, but will not synthesize or broaden a grant.".into(),
            accept_result: None,
            decline_result: Some(json!({"permissions": {}, "scope": "turn"})),
        },
        "mcpServer/elicitation/request" => PendingRequestKind::Approval {
            summary: "An MCP server requests structured input. Open Agent View can decline inline; open the native Codex session to review or accept the server-specific form or URL.".into(),
            accept_result: None,
            decline_result: Some(json!({"action": "decline", "content": null, "_meta": null})),
        },
        _ => unreachable!("method allowlist checked above"),
    };

    pending.retain(|request| request.thread_id != thread_id || request.id != id);
    let expires_at = (method == "item/tool/requestUserInput")
        .then(|| params.get("autoResolutionMs").and_then(Value::as_u64))
        .flatten()
        .and_then(|milliseconds| Instant::now().checked_add(Duration::from_millis(milliseconds)));
    pending.push(PendingRequest {
        id,
        thread_id: thread_id.to_owned(),
        turn_id: turn_id.to_owned(),
        item_id,
        kind,
        resolving: false,
        expires_at,
    });
    Ok(())
}

fn parse_pending_questions(params: &Value) -> Option<Vec<PendingQuestion>> {
    let values = params.get("questions")?.as_array()?;
    if values.is_empty() {
        return None;
    }
    let mut ids = BTreeSet::new();
    let mut questions = Vec::with_capacity(values.len());
    for value in values {
        let id = value
            .get("id")?
            .as_str()
            .filter(|value| !value.is_empty())?
            .to_owned();
        if !ids.insert(id.clone()) {
            return None;
        }
        let header = value
            .get("header")?
            .as_str()
            .filter(|value| !value.is_empty())?
            .to_owned();
        let question = value
            .get("question")?
            .as_str()
            .filter(|value| !value.is_empty())?
            .to_owned();
        let options = match value.get("options") {
            None | Some(Value::Null) => Vec::new(),
            Some(Value::Array(options)) => options
                .iter()
                .map(|option| {
                    option
                        .get("label")?
                        .as_str()
                        .filter(|value| !value.is_empty())
                        .map(ToOwned::to_owned)
                })
                .collect::<Option<Vec<_>>>()?,
            Some(_) => return None,
        };
        questions.push(PendingQuestion {
            id,
            header,
            question,
            options,
            allow_other: value.get("isOther")?.as_bool()?,
            secret: value.get("isSecret")?.as_bool()?,
        });
    }
    Some(questions)
}

fn push_optional_line(lines: &mut Vec<String>, label: &str, value: Option<&Value>) {
    if let Some(value) = value
        .and_then(Value::as_str)
        .filter(|value| !value.is_empty())
    {
        lines.push(format!("{label}: {value}"));
    }
}

fn normalize_question_answer(question: &PendingQuestion, answer: &str) -> Result<String> {
    if question.options.is_empty() {
        return Ok(answer.to_owned());
    }
    if let Ok(index) = answer.parse::<usize>() {
        if let Some(option) = index
            .checked_sub(1)
            .and_then(|index| question.options.get(index))
        {
            return Ok(option.clone());
        }
    }
    if question.options.iter().any(|option| option == answer) || question.allow_other {
        return Ok(answer.to_owned());
    }
    bail!(
        "answer must be an offered option (type its number or exact label): {}",
        question.options.join(", ")
    )
}

fn format_pending_request(request: &PendingRequest) -> String {
    let body = match &request.kind {
        PendingRequestKind::Approval {
            summary,
            accept_result,
            decline_result,
        } => {
            let decisions = match (accept_result.is_some(), decline_result.is_some()) {
                (true, true) => "y allow once · n deny",
                (true, false) => "y allow once",
                (false, true) => "n deny",
                (false, false) => "no supported inline decision; open the native session",
            };
            format!("{summary}\n\n{decisions}")
        }
        PendingRequestKind::UserInput { questions, answers } => {
            if let Some(question) = questions
                .iter()
                .find(|question| !answers.contains_key(&question.id))
            {
                let mut lines = vec![
                    format!(
                        "Codex asks for input ({}/{}): {}",
                        answers.len() + 1,
                        questions.len(),
                        question.header
                    ),
                    question.question.clone(),
                ];
                if question.secret {
                    lines.push(
                        "This answer is secret; open the native session so it is not echoed here."
                            .into(),
                    );
                } else if !question.options.is_empty() {
                    lines.extend(
                        question
                            .options
                            .iter()
                            .enumerate()
                            .map(|(index, option)| format!("{}. {option}", index + 1)),
                    );
                    if question.allow_other {
                        lines.push("Or type a custom answer.".into());
                    }
                }
                lines.join("\n")
            } else {
                "Codex structured input is ready to submit.".into()
            }
        }
        PendingRequestKind::Unsupported { summary } => summary.clone(),
    };
    let status = if request.resolving {
        "\n\nResponse sent; waiting for Codex to resolve the request."
    } else if request
        .expires_at
        .is_some_and(|deadline| Instant::now() >= deadline)
    {
        "\n\nThe provider's auto-resolution deadline has passed; this request is no longer actionable here."
    } else {
        ""
    };
    sanitize_terminal_text(&format!(
        "Pending request {}\n{body}{status}",
        request.item_id
    ))
}

fn require_idle_mutation(
    session: &AgentSession,
    owned: &OwnedThread,
    operation: &str,
) -> Result<()> {
    if owned.active_turn_id.is_some() || session.state != SessionState::Completed {
        bail!("refusing to {operation} a Codex thread that is not known idle");
    }
    Ok(())
}

fn format_thread_transcript(response: &Value) -> Result<String> {
    let thread = response
        .get("thread")
        .and_then(Value::as_object)
        .context("thread/read response omitted thread")?;
    let turns = thread
        .get("turns")
        .and_then(Value::as_array)
        .context("thread/read response omitted included turns")?;
    let mut sections = Vec::new();
    for turn in turns {
        let Some(items) = turn.get("items").and_then(Value::as_array) else {
            continue;
        };
        for item in items {
            let kind = item
                .get("type")
                .and_then(Value::as_str)
                .unwrap_or("unknown");
            match kind {
                "userMessage" => {
                    let text = item
                        .get("content")
                        .and_then(Value::as_array)
                        .into_iter()
                        .flatten()
                        .filter(|content| {
                            content.get("type").and_then(Value::as_str) == Some("text")
                        })
                        .filter_map(|content| content.get("text").and_then(Value::as_str))
                        .collect::<Vec<_>>()
                        .join("\n");
                    if !text.is_empty() {
                        sections.push(format!("You\n{text}"));
                    }
                }
                "agentMessage" => {
                    if let Some(text) = item.get("text").and_then(Value::as_str) {
                        sections.push(format!("Codex\n{text}"));
                    }
                }
                "plan" => {
                    if let Some(text) = item.get("text").and_then(Value::as_str) {
                        sections.push(format!("Plan\n{text}"));
                    }
                }
                "commandExecution" => {
                    let command = item
                        .get("command")
                        .and_then(Value::as_str)
                        .unwrap_or("command");
                    let status = item
                        .get("status")
                        .and_then(Value::as_str)
                        .unwrap_or("unknown");
                    let output = item
                        .get("aggregatedOutput")
                        .and_then(Value::as_str)
                        .unwrap_or("");
                    let rendered = if output.is_empty() {
                        format!("Command ({status})\n$ {command}")
                    } else {
                        format!("Command ({status})\n$ {command}\n{output}")
                    };
                    sections.push(rendered);
                }
                "fileChange" => sections.push("Codex applied file changes".into()),
                "enteredReviewMode" | "exitedReviewMode" => {
                    if let Some(review) = item.get("review").and_then(Value::as_str) {
                        sections.push(format!("Review\n{review}"));
                    }
                }
                _ => {}
            }
        }
    }
    if sections.is_empty() {
        Ok("No persisted Codex transcript items are available.".into())
    } else {
        Ok(limit_transcript(sanitize_terminal_text(
            &sections.join("\n\n"),
        )))
    }
}

fn sanitize_terminal_text(input: &str) -> String {
    input
        .chars()
        .map(|character| match character {
            '\n' | '\t' => character,
            character if character.is_control() => '�',
            character => character,
        })
        .collect()
}

fn limit_transcript(transcript: String) -> String {
    let count = transcript.chars().count();
    if count <= MAX_TRANSCRIPT_CHARS {
        return transcript;
    }
    let tail = transcript
        .chars()
        .skip(count - MAX_TRANSCRIPT_CHARS)
        .collect::<String>();
    format!("[earlier transcript truncated]\n{tail}")
}

fn default_state_dir() -> Result<PathBuf> {
    if let Some(state_home) = std::env::var_os("XDG_STATE_HOME") {
        return Ok(PathBuf::from(state_home)
            .join("open-agent-view")
            .join("codex-supervisor"));
    }
    let home = std::env::var_os("HOME").context("HOME is not set")?;
    Ok(PathBuf::from(home).join(".local/state/open-agent-view/codex-supervisor"))
}

fn ensure_private_directory(path: &Path) -> Result<()> {
    #[cfg(not(target_os = "linux"))]
    {
        let _ = path;
        bail!("durable Codex supervision currently requires Linux PID identity verification")
    }
    #[cfg(target_os = "linux")]
    {
        use std::os::unix::fs::{MetadataExt, PermissionsExt};

        match fs::symlink_metadata(path) {
            Ok(metadata) if metadata.file_type().is_symlink() => {
                bail!("Codex supervisor state directory cannot be a symlink")
            }
            Ok(metadata) if !metadata.is_dir() => {
                bail!("Codex supervisor state path is not a directory")
            }
            Ok(_) => {}
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
                fs::create_dir_all(path)
                    .with_context(|| format!("failed to create {}", path.display()))?;
            }
            Err(error) => return Err(error).context("failed to inspect Codex state directory"),
        }
        let metadata = fs::symlink_metadata(path)?;
        if metadata.uid() != effective_uid()? {
            bail!("Codex supervisor state directory is not owned by the current user");
        }
        fs::set_permissions(path, fs::Permissions::from_mode(0o700))?;
        let mode = fs::metadata(path)?.permissions().mode();
        if mode & 0o077 != 0 {
            bail!("Codex supervisor state directory must be user-only (mode 0700)");
        }
        Ok(())
    }
}

#[cfg(target_os = "linux")]
fn effective_uid() -> Result<u32> {
    let status = fs::read_to_string("/proc/self/status")?;
    let line = status
        .lines()
        .find(|line| line.starts_with("Uid:"))
        .context("/proc/self/status omitted Uid")?;
    line.split_whitespace()
        .nth(2)
        .context("/proc/self/status omitted effective uid")?
        .parse()
        .context("invalid effective uid in /proc/self/status")
}

fn private_append_file(path: &Path) -> Result<File> {
    reject_unsafe_existing_file(path, "Codex App Server log")?;
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
    verify_private_file(&file, "Codex App Server log")?;
    Ok(file)
}

fn load_record(path: &Path, state_dir: &Path) -> Result<Option<SupervisorRecord>> {
    let metadata = match fs::symlink_metadata(path) {
        Ok(metadata) => metadata,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(None),
        Err(error) => return Err(error).context("failed to inspect Codex supervisor record"),
    };
    if metadata.file_type().is_symlink() || !metadata.is_file() {
        bail!("Codex supervisor record must be a regular file");
    }
    #[cfg(unix)]
    {
        use std::os::unix::fs::{MetadataExt, PermissionsExt};
        if metadata.uid() != effective_uid()? || metadata.permissions().mode() & 0o077 != 0 {
            bail!("Codex supervisor record must be current-user-owned and mode 0600");
        }
    }
    let input = fs::read(path)?;
    let record: SupervisorRecord = serde_json::from_slice(&input)
        .with_context(|| format!("invalid Codex supervisor record {}", path.display()))?;
    if record.socket_path.parent() != Some(state_dir) {
        bail!("Codex supervisor socket escaped the private state directory");
    }
    Ok(Some(record))
}

fn save_record(path: &Path, record: &SupervisorRecord) -> Result<()> {
    let temporary = path.with_extension(format!("tmp-{}-{}", std::process::id(), now_millis()));
    let mut options = OpenOptions::new();
    options.create_new(true).write(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.mode(0o600);
    }
    let mut file = options
        .open(&temporary)
        .with_context(|| format!("failed to write {}", temporary.display()))?;
    let bytes = serde_json::to_vec_pretty(record)?;
    file.write_all(&bytes)?;
    file.sync_all()?;
    fs::rename(&temporary, path).with_context(|| format!("failed to replace {}", path.display()))
}

fn verify_process(record: &SupervisorRecord) -> Result<bool> {
    let start = match process_start_token(record.pid) {
        Ok(start) => start,
        Err(error) if is_missing_process(&error) => return Ok(false),
        Err(error) => return Err(error),
    };
    let cmdline = match process_cmdline(record.pid) {
        Ok(cmdline) => cmdline,
        Err(error) if is_missing_process(&error) => return Ok(false),
        Err(error) => return Err(error),
    };
    Ok(start == record.process_start_token && cmdline == record.process_cmdline)
}

fn is_missing_process(error: &anyhow::Error) -> bool {
    error
        .downcast_ref::<std::io::Error>()
        .map(|error| error.kind() == std::io::ErrorKind::NotFound)
        .unwrap_or(false)
}

#[cfg(target_os = "linux")]
fn process_start_token(pid: u32) -> Result<String> {
    let stat = fs::read_to_string(format!("/proc/{pid}/stat"))?;
    let suffix = stat
        .rsplit_once(')')
        .map(|(_, suffix)| suffix)
        .context("invalid /proc process stat")?;
    // After the command name, index 0 is field 3 (state); starttime is field 22.
    suffix
        .split_whitespace()
        .nth(19)
        .map(ToOwned::to_owned)
        .context("/proc process stat omitted starttime")
}

#[cfg(not(target_os = "linux"))]
fn process_start_token(_: u32) -> Result<String> {
    bail!("process start-token verification is unavailable on this platform")
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
        bail!("Unix socket supervision is unavailable on this platform")
    }
    #[cfg(unix)]
    {
        use std::os::unix::fs::FileTypeExt;
        use std::os::unix::net::UnixStream;

        let deadline = Instant::now() + timeout;
        loop {
            let is_socket = fs::symlink_metadata(path)
                .map(|metadata| metadata.file_type().is_socket())
                .unwrap_or(false);
            if is_socket && UnixStream::connect(path).is_ok() {
                return Ok(());
            }
            if Instant::now() >= deadline {
                bail!("timed out waiting for Unix socket {}", path.display());
            }
            thread::sleep(Duration::from_millis(25));
        }
    }
}

struct StateLock {
    file: File,
}

struct ResponseLease {
    file: File,
}

impl ResponseLease {
    fn try_acquire(path: &Path) -> Result<Option<Self>> {
        reject_unsafe_existing_file(path, "Codex controller lease")?;
        let mut options = OpenOptions::new();
        options.create(true).read(true).write(true);
        #[cfg(unix)]
        {
            use std::os::unix::fs::OpenOptionsExt;
            options.mode(0o600).custom_flags(libc::O_NOFOLLOW);
        }
        let file = options
            .open(path)
            .with_context(|| format!("failed to open controller lease {}", path.display()))?;
        verify_private_file(&file, "Codex controller lease")?;
        #[cfg(unix)]
        {
            use std::os::fd::AsRawFd;
            let result = unsafe { libc::flock(file.as_raw_fd(), libc::LOCK_EX | libc::LOCK_NB) };
            if result != 0 {
                let error = std::io::Error::last_os_error();
                if error
                    .raw_os_error()
                    .is_some_and(|code| code == libc::EAGAIN || code == libc::EWOULDBLOCK)
                {
                    return Ok(None);
                }
                return Err(error).context("failed to acquire Codex controller lease");
            }
        }
        Ok(Some(Self { file }))
    }
}

impl StateLock {
    fn acquire(path: &Path) -> Result<Self> {
        reject_unsafe_existing_file(path, "Codex supervisor lock")?;
        let mut options = OpenOptions::new();
        options.create(true).read(true).write(true);
        #[cfg(unix)]
        {
            use std::os::unix::fs::OpenOptionsExt;
            options.mode(0o600).custom_flags(libc::O_NOFOLLOW);
        }
        let file = options
            .open(path)
            .with_context(|| format!("failed to open supervisor lock {}", path.display()))?;
        verify_private_file(&file, "Codex supervisor lock")?;
        #[cfg(unix)]
        {
            use std::os::fd::AsRawFd;
            let result = unsafe { libc::flock(file.as_raw_fd(), libc::LOCK_EX) };
            if result != 0 {
                return Err(std::io::Error::last_os_error())
                    .context("failed to lock supervisor state");
            }
        }
        Ok(Self { file })
    }
}

fn reject_unsafe_existing_file(path: &Path, label: &str) -> Result<()> {
    match fs::symlink_metadata(path) {
        Ok(metadata) if metadata.file_type().is_symlink() || !metadata.is_file() => {
            bail!("{label} must be a regular file")
        }
        Ok(_) => Ok(()),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(()),
        Err(error) => Err(error).with_context(|| format!("failed to inspect {label}")),
    }
}

fn verify_private_file(file: &File, label: &str) -> Result<()> {
    #[cfg(unix)]
    {
        use std::os::unix::fs::{MetadataExt, PermissionsExt};
        let metadata = file.metadata()?;
        if metadata.uid() != effective_uid()? || metadata.permissions().mode() & 0o077 != 0 {
            bail!("{label} must be current-user-owned and mode 0600");
        }
    }
    Ok(())
}

impl Drop for StateLock {
    fn drop(&mut self) {
        #[cfg(unix)]
        {
            use std::os::fd::AsRawFd;
            let _ = unsafe { libc::flock(self.file.as_raw_fd(), libc::LOCK_UN) };
        }
    }
}

impl Drop for ResponseLease {
    fn drop(&mut self) {
        #[cfg(unix)]
        {
            use std::os::fd::AsRawFd;
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

#[cfg(test)]
mod tests {
    use std::collections::BTreeSet;
    use std::os::unix::fs::PermissionsExt;
    use std::sync::Arc;

    use tempfile::tempdir;

    use crate::adapters::{CodexSource, DiscoveryRequest, SessionSource};
    use crate::domain::{SessionKind, SessionState};

    use super::*;

    #[test]
    fn stale_pid_identity_is_never_treated_as_live() {
        let record = SupervisorRecord {
            version: RECORD_VERSION,
            pid: std::process::id(),
            process_start_token: "definitely-wrong".into(),
            process_cmdline: process_cmdline(std::process::id()).unwrap(),
            codex_bin: "codex".into(),
            socket_path: PathBuf::from("/tmp/not-used.sock"),
            created_at_ms: 1,
            threads: BTreeMap::new(),
        };

        assert!(!verify_process(&record).unwrap());
    }

    #[test]
    fn transcript_rendering_is_bounded_and_keeps_recent_unicode() {
        let old = "x".repeat(MAX_TRANSCRIPT_CHARS + 100);
        let response = json!({
            "thread": {
                "turns": [{
                    "items": [
                        {"type": "agentMessage", "text": old},
                        {"type": "agentMessage", "text": "recent 🦀"}
                    ]
                }]
            }
        });

        let rendered = format_thread_transcript(&response).unwrap();

        assert!(rendered.starts_with("[earlier transcript truncated]"));
        assert!(rendered.ends_with("recent 🦀"));
        assert!(rendered.chars().count() <= MAX_TRANSCRIPT_CHARS + 32);
    }

    #[test]
    fn pending_reducer_requires_exact_ownership_and_preserves_request_ids() {
        let mut threads = BTreeMap::new();
        threads.insert(
            "owned-thread".into(),
            OwnedThread {
                cwd: PathBuf::from("/tmp"),
                created_at_ms: 1,
                active_turn_id: Some("owned-turn".into()),
            },
        );
        let server = SupervisorRecord {
            version: RECORD_VERSION,
            pid: std::process::id(),
            process_start_token: "test".into(),
            process_cmdline: vec![1],
            codex_bin: "codex".into(),
            socket_path: PathBuf::from("/tmp/test.sock"),
            created_at_ms: 1,
            threads,
        };
        let mut pending = Vec::new();

        reconcile_pending_event(
            &mut pending,
            &server,
            json!({
                "id": -7,
                "method": "item/commandExecution/requestApproval",
                "params": {
                    "threadId": "owned-thread",
                    "turnId": "owned-turn",
                    "itemId": "command-1",
                    "startedAtMs": 1,
                    "command": "printf '\u{1b}[2J'"
                }
            }),
        )
        .unwrap();
        reconcile_pending_event(
            &mut pending,
            &server,
            json!({
                "id": "file-1",
                "method": "item/fileChange/requestApproval",
                "params": {
                    "threadId": "owned-thread",
                    "turnId": "owned-turn",
                    "itemId": "file-1",
                    "startedAtMs": 1
                }
            }),
        )
        .unwrap();
        reconcile_pending_event(
            &mut pending,
            &server,
            json!({
                "id": "wrong-turn",
                "method": "item/fileChange/requestApproval",
                "params": {
                    "threadId": "owned-thread",
                    "turnId": "other-turn",
                    "itemId": "file-2",
                    "startedAtMs": 1
                }
            }),
        )
        .unwrap();

        assert_eq!(pending.len(), 2);
        assert_eq!(pending[0].id, json!(-7));
        assert!(!format_pending_request(&pending[0]).contains('\u{1b}'));
        let PendingRequestKind::Approval { accept_result, .. } = &pending[0].kind else {
            panic!("command request should be an approval")
        };
        assert!(
            accept_result.is_none(),
            "command acceptance requires both the exact command and working directory"
        );
        let PendingRequestKind::Approval {
            accept_result,
            decline_result,
            ..
        } = &pending[1].kind
        else {
            panic!("file request should be an approval")
        };
        assert!(accept_result.is_none());
        assert_eq!(decline_result, &Some(json!({"decision": "decline"})));

        reconcile_pending_event(
            &mut pending,
            &server,
            json!({
                "method": "serverRequest/resolved",
                "params": {"threadId": "owned-thread", "requestId": -7}
            }),
        )
        .unwrap();
        assert_eq!(pending.len(), 1);
        assert_eq!(pending[0].id, json!("file-1"));
    }

    #[test]
    fn malformed_or_expired_structured_input_never_becomes_actionable() {
        let mut threads = BTreeMap::new();
        threads.insert(
            "owned-thread".into(),
            OwnedThread {
                cwd: PathBuf::from("/tmp"),
                created_at_ms: 1,
                active_turn_id: Some("owned-turn".into()),
            },
        );
        let server = SupervisorRecord {
            version: RECORD_VERSION,
            pid: std::process::id(),
            process_start_token: "test".into(),
            process_cmdline: vec![1],
            codex_bin: "codex".into(),
            socket_path: PathBuf::from("/tmp/test.sock"),
            created_at_ms: 1,
            threads,
        };
        let mut pending = Vec::new();
        reconcile_pending_event(
            &mut pending,
            &server,
            json!({
                "id": "input-1",
                "method": "item/tool/requestUserInput",
                "params": {
                    "threadId": "owned-thread",
                    "turnId": "owned-turn",
                    "itemId": "input-1",
                    "questions": [
                        {"id": "duplicate", "header": "One", "question": "First?", "isOther": false, "isSecret": false, "options": null},
                        {"id": "duplicate", "header": "Two", "question": "Second?", "isOther": false, "isSecret": false, "options": null}
                    ]
                }
            }),
        )
        .unwrap();
        assert!(matches!(
            pending[0].kind,
            PendingRequestKind::Unsupported { .. }
        ));

        pending[0].expires_at = Some(Instant::now() - Duration::from_millis(1));
        assert!(format_pending_request(&pending[0]).contains("deadline has passed"));
    }

    #[test]
    fn only_one_process_instance_holds_inline_response_authority() {
        let directory = tempdir().unwrap();
        let state = directory.path().join("state");
        let first = CodexSupervisor::with_state_dir("codex", state.clone()).unwrap();
        let second = CodexSupervisor::with_state_dir("codex", state.clone()).unwrap();

        assert!(first.response_lease.is_some());
        assert!(second.response_lease.is_none());
        drop(first);

        let third = CodexSupervisor::with_state_dir("codex", state).unwrap();
        assert!(third.response_lease.is_some());
    }

    #[test]
    fn durable_server_reconnects_and_preserves_exact_thread_ownership() {
        let directory = tempdir().unwrap();
        let state_dir = directory.path().join("state");
        let mock = directory.path().join("mock-codex.py");
        fs::write(&mock, MOCK_CODEX).unwrap();
        fs::set_permissions(&mock, fs::Permissions::from_mode(0o700)).unwrap();

        let first = Arc::new(
            CodexSupervisor::with_state_dir(mock.to_string_lossy(), state_dir.clone()).unwrap(),
        );
        let thread_id = first
            .launch("Implement the test", Path::new("/tmp"))
            .unwrap();
        let first_record = first.live_record().unwrap();
        let _process_guard = VerifiedTestProcess(first_record.clone());
        assert_eq!(
            fs::metadata(&state_dir).unwrap().permissions().mode() & 0o777,
            0o700
        );
        assert_eq!(
            fs::metadata(state_dir.join("supervisor.json"))
                .unwrap()
                .permissions()
                .mode()
                & 0o777,
            0o600
        );
        drop(first);

        let second =
            Arc::new(CodexSupervisor::with_state_dir(mock.to_string_lossy(), state_dir).unwrap());
        let second_record = second.live_record().unwrap();
        assert!(first_record.same_process(&second_record));

        let source = CodexSource::managed(second.clone());
        let sessions = source
            .discover(&DiscoveryRequest {
                include_completed: true,
                include_interactive: true,
                cwd: None,
            })
            .unwrap();
        let mut snapshot = SessionSnapshot {
            sessions,
            warnings: vec![],
        };
        second.enrich(&mut snapshot);
        let session = snapshot
            .sessions
            .iter()
            .find(|session| session.provider_session_id == thread_id)
            .unwrap()
            .clone();
        assert_eq!(session.state, SessionState::Working);
        assert!(session.capabilities.contains(&Capability::Interrupt));
        assert!(session.capabilities.contains(&Capability::Reply));
        let transcript = second.inspect(&session).unwrap();
        assert!(transcript.contains("You\nImplement the test"));
        assert!(transcript.contains("Codex\nWorking on it"));
        assert_eq!(
            second.reply(&session, "Keep the tests focused").unwrap(),
            CodexReplyMode::Steered
        );

        second.interrupt(&session).unwrap();
        let mut external = session.clone();
        external.provider_session_id = "external-thread".into();
        external.id = "codex:host:external-thread".into();
        assert!(second.interrupt(&external).is_err());

        let idle = discover_owned(&second, &thread_id);
        assert_eq!(idle.state, SessionState::Completed);
        assert!(idle.capabilities.contains(&Capability::Reply));
        assert!(idle.capabilities.contains(&Capability::Archive));
        assert!(idle.capabilities.contains(&Capability::Delete));
        assert_eq!(
            second.reply(&idle, "Run one more check").unwrap(),
            CodexReplyMode::Started
        );
        let active = discover_owned(&second, &thread_id);
        second.interrupt(&active).unwrap();
        let idle = discover_owned(&second, &thread_id);
        second.archive(&idle).unwrap();
        second.delete(&idle).unwrap();
        assert!(second.delete(&idle).is_err());
    }

    #[test]
    fn pending_approval_replays_after_reconnect_and_clears_only_when_resolved() {
        let directory = tempdir().unwrap();
        let state_dir = directory.path().join("state");
        let mock = directory.path().join("mock-codex.py");
        fs::write(&mock, MOCK_CODEX).unwrap();
        fs::set_permissions(&mock, fs::Permissions::from_mode(0o700)).unwrap();

        let first = Arc::new(
            CodexSupervisor::with_state_dir(mock.to_string_lossy(), state_dir.clone()).unwrap(),
        );
        let thread_id = first.launch("Request approval", Path::new("/tmp")).unwrap();
        let record = first.live_record().unwrap();
        let _process_guard = VerifiedTestProcess(record);
        let pending = discover_owned(&first, &thread_id);
        assert_eq!(pending.state, SessionState::NeedsInput);
        assert!(pending.capabilities.contains(&Capability::Approve));
        assert!(pending.capabilities.contains(&Capability::Decline));
        assert!(first.inspect(&pending).unwrap().contains("cargo test"));
        drop(first);

        let second =
            Arc::new(CodexSupervisor::with_state_dir(mock.to_string_lossy(), state_dir).unwrap());
        let replayed = discover_owned_until(&second, &thread_id, |session| {
            session.capabilities.contains(&Capability::Approve)
        });
        assert!(replayed.capabilities.contains(&Capability::Approve));
        second.respond_approval(&replayed, false).unwrap();

        let resolving = discover_owned(&second, &thread_id);
        assert!(!resolving.capabilities.contains(&Capability::Approve));
        assert!(!resolving.capabilities.contains(&Capability::Decline));
        let resumed = discover_owned(&second, &thread_id);
        assert_eq!(resumed.state, SessionState::Working);
        assert!(resumed.capabilities.contains(&Capability::Reply));
    }

    #[test]
    fn structured_input_answers_questions_sequentially_without_persisting_values() {
        let directory = tempdir().unwrap();
        let state_dir = directory.path().join("state");
        let mock = directory.path().join("mock-codex.py");
        fs::write(&mock, MOCK_CODEX).unwrap();
        fs::set_permissions(&mock, fs::Permissions::from_mode(0o700)).unwrap();
        let supervisor =
            Arc::new(CodexSupervisor::with_state_dir(mock.to_string_lossy(), state_dir).unwrap());
        let thread_id = supervisor
            .launch("Request input", Path::new("/tmp"))
            .unwrap();
        let record = supervisor.live_record().unwrap();
        let _process_guard = VerifiedTestProcess(record);
        let pending = discover_owned(&supervisor, &thread_id);
        assert!(pending.capabilities.contains(&Capability::Respond));
        assert!(supervisor
            .inspect(&pending)
            .unwrap()
            .contains("Environment"));

        let first = supervisor.respond_user_input(&pending, "staging").unwrap();
        assert_eq!(first.answered, 1);
        assert_eq!(first.total, 2);
        assert!(!first.submitted);
        let still_pending = discover_owned(&supervisor, &thread_id);
        assert!(supervisor
            .inspect(&still_pending)
            .unwrap()
            .contains("Checks"));
        let second = supervisor.respond_user_input(&still_pending, "2").unwrap();
        assert_eq!(second.answered, 2);
        assert!(second.submitted);

        let resumed = discover_owned(&supervisor, &thread_id);
        assert_eq!(resumed.state, SessionState::Working);
        assert!(!resumed.capabilities.contains(&Capability::Respond));
        let record_text = fs::read_to_string(supervisor.record_path.clone()).unwrap();
        assert!(!record_text.contains("staging"));
        assert!(!record_text.contains("Thorough"));
    }

    fn discover_owned(supervisor: &Arc<CodexSupervisor>, thread_id: &str) -> AgentSession {
        let source = CodexSource::managed(supervisor.clone());
        let sessions = source
            .discover(&DiscoveryRequest {
                include_completed: true,
                include_interactive: true,
                cwd: None,
            })
            .unwrap();
        let mut snapshot = SessionSnapshot {
            sessions,
            warnings: vec![],
        };
        supervisor.enrich(&mut snapshot);
        snapshot
            .sessions
            .into_iter()
            .find(|session| session.provider_session_id == thread_id)
            .unwrap()
    }

    fn discover_owned_until(
        supervisor: &Arc<CodexSupervisor>,
        thread_id: &str,
        predicate: impl Fn(&AgentSession) -> bool,
    ) -> AgentSession {
        let deadline = Instant::now() + Duration::from_secs(2);
        loop {
            let session = discover_owned(supervisor, thread_id);
            if predicate(&session) || Instant::now() >= deadline {
                return session;
            }
            thread::sleep(Duration::from_millis(10));
        }
    }

    /// Reaps the exact mock child created by this test, including on panic.
    /// It never signals a PID after either identity token stops matching.
    struct VerifiedTestProcess(SupervisorRecord);

    impl Drop for VerifiedTestProcess {
        fn drop(&mut self) {
            if !verify_process(&self.0).unwrap_or(false) {
                return;
            }
            unsafe { libc::kill(self.0.pid as i32, libc::SIGTERM) };
            if reap_test_process(self.0.pid, Duration::from_secs(1)) {
                return;
            }
            if verify_process(&self.0).unwrap_or(false) {
                unsafe { libc::kill(self.0.pid as i32, libc::SIGKILL) };
                let _ = reap_test_process(self.0.pid, Duration::from_secs(1));
            }
        }
    }

    fn reap_test_process(pid: u32, timeout: Duration) -> bool {
        let deadline = Instant::now() + timeout;
        loop {
            let mut status = 0;
            let waited = unsafe { libc::waitpid(pid as i32, &mut status, libc::WNOHANG) };
            if waited == pid as i32 || (waited < 0 && !Path::new(&format!("/proc/{pid}")).exists())
            {
                return true;
            }
            if Instant::now() >= deadline {
                return false;
            }
            thread::sleep(Duration::from_millis(20));
        }
    }

    #[test]
    fn ownership_enrichment_does_not_grant_docker_interrupt() {
        let session = AgentSession {
            id: "codex:container:thread".into(),
            provider_session_id: "thread".into(),
            provider: Provider::Codex,
            runtime: Runtime::Docker {
                container_id: "container".into(),
                container_name: "test".into(),
                image: "image".into(),
            },
            kind: SessionKind::Background,
            name: "test".into(),
            cwd: PathBuf::from("/work"),
            state: SessionState::Working,
            summary: String::new(),
            raw_state: None,
            pid: None,
            started_at: None,
            updated_at: None,
            pull_requests: None,
            capabilities: BTreeSet::new(),
        };
        assert_eq!(session.runtime.label(), "test");
        assert!(!session.capabilities.contains(&Capability::Interrupt));
    }

    const MOCK_CODEX: &str = r#"#!/usr/bin/env python3
import json, os, socket, sys, threading

args = sys.argv[1:]
if args[:2] == ["app-server", "--listen"]:
    path = args[2][len("unix://"):]
    server = socket.socket(socket.AF_UNIX, socket.SOCK_STREAM)
    server.bind(path)
    os.chmod(path, 0o600)
    server.listen()
    state = {"thread": None, "turn": None, "turn_seq": 0, "active": False, "archived": False, "pending": False, "pending_kind": None, "pending_id": "approval-1"}
    def pending_event():
        if state["pending_kind"] == "approval":
            return {"id": state["pending_id"], "method": "item/commandExecution/requestApproval", "params": {"threadId": state["thread"], "turnId": state["turn"], "itemId": "command-1", "startedAtMs": 1, "command": "cargo test", "cwd": "/tmp", "reason": "verify the change"}}
        if state["pending_kind"] == "input":
            return {"id": state["pending_id"], "method": "item/tool/requestUserInput", "params": {"threadId": state["thread"], "turnId": state["turn"], "itemId": "input-1", "autoResolutionMs": None, "questions": [
                {"id": "environment", "header": "Environment", "question": "Which environment?", "isOther": True, "isSecret": False, "options": [{"label": "production", "description": "Live"}]},
                {"id": "checks", "header": "Checks", "question": "How much validation?", "isOther": False, "isSecret": False, "options": [{"label": "Quick", "description": "Focused"}, {"label": "Thorough", "description": "Full"}]}
            ]}}
        return None
    def handle(conn):
        stream = conn.makefile("rwb")
        for raw in stream:
            message = json.loads(raw)
            method = message.get("method")
            ident = message.get("id")
            event = None
            if method is None:
                if ident == state["pending_id"] and "result" in message:
                    state["pending"] = False
                    state["pending_kind"] = None
                    stream.write((json.dumps({"method": "serverRequest/resolved", "params": {"threadId": state["thread"], "requestId": ident}})+"\n").encode()); stream.flush()
                continue
            if method == "initialize": result = {"userAgent": "mock/1"}
            elif method == "thread/start":
                state.update(thread="owned-thread", active=False, archived=False)
                result = {"thread": {"id": state["thread"]}}
            elif method == "turn/start":
                state["turn_seq"] += 1
                prompt = message["params"]["input"][0]["text"]
                pending_kind = "approval" if prompt == "Request approval" else ("input" if prompt == "Request input" else None)
                state.update(turn="owned-turn-"+str(state["turn_seq"]), active=True, pending=pending_kind is not None, pending_kind=pending_kind)
                result = {"turn": {"id": state["turn"], "status": "inProgress", "items": []}}
                if state["pending"]:
                    event = pending_event()
            elif method == "thread/resume":
                result = {"thread": {"id": state["thread"], "turns": []}}
                if state["pending"]:
                    event = pending_event()
            elif method == "turn/steer":
                if message["params"]["threadId"] != state["thread"] or message["params"]["expectedTurnId"] != state["turn"] or not state["active"]:
                    stream.write((json.dumps({"id": ident, "error": {"code": -32600, "message": "stale turn"}})+"\n").encode()); stream.flush(); continue
                result = {"turnId": state["turn"]}
            elif method == "turn/interrupt":
                if message["params"]["threadId"] != state["thread"] or not state["active"]:
                    stream.write((json.dumps({"id": ident, "error": {"code": -32600, "message": "not active"}})+"\n").encode()); stream.flush(); continue
                state["active"] = False; result = {}
            elif method == "thread/read":
                result = {"thread": {"id": state["thread"], "turns": [{"items": [
                    {"type": "userMessage", "content": [{"type": "text", "text": "Implement the test"}]},
                    {"type": "agentMessage", "text": "Working on it"}
                ]}]}}
            elif method == "thread/archive":
                if state["active"]:
                    stream.write((json.dumps({"id": ident, "error": {"code": -32600, "message": "active"}})+"\n").encode()); stream.flush(); continue
                state["archived"] = True; result = {}
            elif method == "thread/delete":
                if state["active"]:
                    stream.write((json.dumps({"id": ident, "error": {"code": -32600, "message": "active"}})+"\n").encode()); stream.flush(); continue
                state.update(thread=None, turn=None, archived=False); result = {}
            elif method == "thread/list":
                data = [] if state["thread"] is None or state["archived"] else [{
                    "id": state["thread"], "cwd": "/tmp", "createdAt": 1,
                    "updatedAt": 2, "preview": "Implement the test", "name": None,
                    "status": {"type": "active" if state["active"] else "idle", "activeFlags": (["waitingOnUserInput"] if state["pending_kind"] == "input" else ["waitingOnApproval"]) if state["pending"] else []},
                    "source": "appServer"
                }]
                result = {"data": data, "nextCursor": None}
            else: result = {}
            if ident is not None:
                stream.write((json.dumps({"id": ident, "result": result})+"\n").encode()); stream.flush()
            if event is not None:
                stream.write((json.dumps(event)+"\n").encode()); stream.flush()
    while True:
        conn, _ = server.accept()
        threading.Thread(target=handle, args=(conn,), daemon=True).start()
elif args[:2] == ["app-server", "proxy"]:
    path = args[args.index("--sock") + 1]
    client = socket.socket(socket.AF_UNIX, socket.SOCK_STREAM); client.connect(path)
    def copy_in():
        while True:
            data = os.read(0, 65536)
            if not data: break
            client.sendall(data)
        try: client.shutdown(socket.SHUT_WR)
        except OSError: pass
    threading.Thread(target=copy_in, daemon=True).start()
    while True:
        data = client.recv(65536)
        if not data: break
        os.write(1, data)
else:
    raise SystemExit(2)
"#;
}

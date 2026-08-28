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
    terminal_turns: Vec<(String, String)>,
    deleted_threads: BTreeSet<String>,
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
/// Persisted PID identity is verified before reuse. Normal dashboard lifecycle
/// never signals a PID loaded from disk; explicit [`Self::shutdown_server`]
/// cleanup uses a stable Linux pidfd before signaling. macOS deliberately does
/// not expose explicit supervisor shutdown until an equivalent stable signaling
/// primitive is available; normal launch, reconnect, and control remain durable.
pub struct CodexSupervisor {
    codex_bin: String,
    state_dir: PathBuf,
    record_path: PathBuf,
    lock_path: PathBuf,
    recovery_lock_path: PathBuf,
    client_transport: SupervisorClientTransport,
    response_lease: Option<ResponseLease>,
    control: Mutex<ControlConnection>,
}

#[derive(Clone, Copy)]
enum SupervisorClientTransport {
    UnixWebSocket,
    #[cfg(all(test, unix))]
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

    /// Construct a supervisor rooted in an explicit private state directory.
    ///
    /// This is intended for isolated integration tests and operator-managed
    /// sandboxes. Normal dashboards should use [`Self::host`].
    pub fn with_isolated_state_dir(
        codex_bin: impl Into<String>,
        state_dir: PathBuf,
    ) -> Result<Self> {
        Self::with_state_dir_and_transport(
            codex_bin,
            state_dir,
            SupervisorClientTransport::UnixWebSocket,
        )
    }

    #[cfg(all(test, unix))]
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
            recovery_lock_path: state_dir.join("recovery.lock"),
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

    /// List the visible models exposed by the exact owning App Server.
    ///
    /// The catalog is account/configuration aware and cursor paginated. Keep
    /// it on the supervisor connection so model discovery and launch use the
    /// same durable Codex process.
    pub fn available_models(&self) -> Result<Vec<String>> {
        const PAGE_SIZE: u64 = 100;
        const MAX_PAGES: usize = 200;
        const MAX_MODELS: usize = 20_000;

        let server = self.ensure_endpoint()?;
        let mut control = self
            .control
            .lock()
            .map_err(|_| anyhow!("Codex supervisor connection lock was poisoned"))?;
        let client = self.control_client(&mut control, &server)?;
        let mut cursor: Option<String> = None;
        let mut models = Vec::new();
        let mut seen = BTreeSet::new();

        for _ in 0..MAX_PAGES {
            let response = client.request(
                "model/list",
                json!({
                    "cursor": cursor,
                    "limit": PAGE_SIZE,
                    "includeHidden": false
                }),
            )?;
            let page = response
                .get("data")
                .and_then(Value::as_array)
                .context("model/list response omitted data")?;
            for item in page {
                if item.get("hidden").and_then(Value::as_bool) == Some(true) {
                    continue;
                }
                let model = item
                    .get("model")
                    .or_else(|| item.get("id"))
                    .and_then(Value::as_str)
                    .context("model/list item omitted model and id")?;
                validate_catalog_model(model, "Codex")?;
                if seen.insert(model.to_owned()) {
                    models.push(model.to_owned());
                }
                if models.len() > MAX_MODELS {
                    bail!("Codex model catalog exceeded {MAX_MODELS} entries");
                }
            }
            cursor = response
                .get("nextCursor")
                .and_then(Value::as_str)
                .map(ToOwned::to_owned);
            if cursor.is_none() {
                return Ok(models);
            }
        }
        bail!("Codex model catalog pagination did not terminate")
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
        let mut start_params = json!({
            "cwd": cwd,
            "approvalPolicy": "on-request",
            "sandbox": "workspace-write",
            "serviceName": "open_agent_view"
        });
        if let Some(model) = model {
            start_params["model"] = Value::String(model.to_owned());
        }
        let started = client.request("thread/start", start_params)?;
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
        let terminal_turns = self.refresh_pending_locked(&mut control, &server)?;
        let mut transcript = format_thread_transcript(&response)?;
        let pending = control
            .pending_requests
            .iter()
            .find(|request| request.thread_id == session.provider_session_id)
            .cloned();
        drop(control);
        self.apply_terminal_turns(&server, &terminal_turns)?;
        if let Some(request) = pending {
            transcript.push_str("\n\n");
            transcript.push_str(&format_pending_request(&request));
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
        let terminal_turns = self.refresh_pending_locked(&mut control, &server)?;
        if !terminal_turns.is_empty() {
            drop(control);
            self.apply_terminal_turns(&server, &terminal_turns)?;
            control = self
                .control
                .lock()
                .map_err(|_| anyhow!("Codex supervisor connection lock was poisoned"))?;
        }
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
        let terminal_turns = self.refresh_pending_locked(&mut control, &server)?;
        if !terminal_turns.is_empty() {
            drop(control);
            self.apply_terminal_turns(&server, &terminal_turns)?;
            control = self
                .control
                .lock()
                .map_err(|_| anyhow!("Codex supervisor connection lock was poisoned"))?;
        }
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
        // Unload the idle runtime before destructive deletion. Current Codex
        // App Servers can otherwise apply deletion while indefinitely
        // withholding the response from the connection that still owns the
        // loaded thread. Archive is recoverable if the later delete fails.
        self.control_client(&mut control, &server)?
            .request_with_timeout(
                "thread/archive",
                json!({"threadId": session.provider_session_id}),
                Duration::from_secs(10),
            )?;
        let params = json!({"threadId": session.provider_session_id});
        let first = self
            .control_client(&mut control, &server)?
            .request_with_timeout("thread/delete", params, Duration::from_secs(5));
        if let Err(first_error) = first {
            let terminal_turns = self
                .refresh_pending_locked(&mut control, &server)
                .unwrap_or_default();
            let deleted = control.deleted_threads.remove(&session.provider_session_id);
            drop(control);
            self.apply_terminal_turns(&server, &terminal_turns)?;
            if deleted {
                return self.remove_owned_thread_after_delete(&server, session);
            }
            if !app_server_request_timed_out("thread/delete", &first_error) {
                return Err(first_error);
            }
            return self.recover_idle_server_after_delete_timeout(
                &server,
                &session.provider_session_id,
                &first_error,
            );
        }
        self.remove_owned_thread_after_delete(&server, session)
    }

    fn remove_owned_thread_after_delete(
        &self,
        server: &SupervisorRecord,
        session: &AgentSession,
    ) -> Result<()> {
        self.update_record_for_server(server, |record| {
            record.threads.remove(&session.provider_session_id);
            Ok(())
        })
    }

    fn recover_idle_server_after_delete_timeout(
        &self,
        server: &SupervisorRecord,
        thread_id: &str,
        delete_error: &anyhow::Error,
    ) -> Result<()> {
        let active = server
            .threads
            .iter()
            .filter(|(_, thread)| thread.active_turn_id.is_some())
            .map(|(thread_id, _)| thread_id.as_str())
            .take(3)
            .collect::<Vec<_>>();
        if !active.is_empty() {
            bail!(
                "Codex applied no confirmable delete response and the owning server still has active work ({}); refusing to restart it: {delete_error:#}",
                active.join(", ")
            );
        }

        let _recovery = RecoveryLock::acquire_exclusive(&self.recovery_lock_path)?;
        self.shutdown_server_inner()
            .context("failed to stop the exact idle Codex App Server after a timed-out deletion")?;
        let verification =
            match codex_thread_is_missing_from_fresh_process(&self.codex_bin, thread_id) {
                Ok(false) => delete_codex_thread_from_fresh_process(&self.codex_bin, thread_id)
                    .map(|()| true),
                result => result,
            };

        // Restore durable ownership on a fresh server whether verification
        // succeeds or fails. Only the exact confirmed-deleted target is
        // omitted; all other idle owned tasks remain reconnectable.
        let replacement = self.ensure_endpoint_inner()?;
        let missing = matches!(&verification, Ok(true));
        self.update_record_for_server(&replacement, |record| {
            record.threads = server.threads.clone();
            if missing {
                record.threads.remove(thread_id);
            }
            Ok(())
        })?;

        match verification {
            Ok(true) => Ok(()),
            Ok(false) => bail!(
                "Codex thread {thread_id} still exists after the idle server recovered from a timed-out delete: {delete_error:#}"
            ),
            Err(error) => Err(error).context(format!(
                "failed to verify Codex thread {thread_id} after idle server recovery; original delete: {delete_error:#}"
            )),
        }
    }

    /// Stop the exact verified App Server process.
    ///
    /// Normal dashboard exit deliberately leaves the server running for
    /// reconnect. This explicit operation is reserved for isolated tests and
    /// operator cleanup. It requires this process to hold the controller lease,
    /// opens a stable kernel pidfd, then revalidates the complete persisted
    /// process identity before sending SIGTERM through that descriptor.
    pub fn shutdown_server(&self) -> Result<()> {
        let _recovery = RecoveryLock::acquire_exclusive(&self.recovery_lock_path)?;
        self.shutdown_server_inner()
    }

    fn shutdown_server_inner(&self) -> Result<()> {
        if self.response_lease.is_none() {
            bail!(
                "another Open Agent View process holds Codex control authority; refusing to stop its App Server"
            );
        }
        let _lock = StateLock::acquire(&self.lock_path)?;
        let Some(record) = load_record(&self.record_path, &self.state_dir)? else {
            return Ok(());
        };
        if record.version != RECORD_VERSION {
            bail!(
                "unsupported Codex supervisor record version {}",
                record.version
            );
        }

        #[cfg(target_os = "linux")]
        {
            use std::os::fd::{AsRawFd, FromRawFd};

            let raw_fd =
                unsafe { libc::syscall(libc::SYS_pidfd_open, record.pid as libc::pid_t, 0_u32) };
            if raw_fd < 0 {
                let error = std::io::Error::last_os_error();
                if error.raw_os_error() == Some(libc::ESRCH) {
                    return Ok(());
                }
                return Err(error).context("failed to open exact Codex App Server pidfd");
            }
            let pidfd = unsafe { File::from_raw_fd(raw_fd as i32) };
            if !verify_process(&record)? {
                bail!("Codex App Server identity changed before shutdown");
            }

            // Drop the local protocol connection before asking the server to
            // exit. The pidfd remains a stable reference even if the numeric
            // PID is concurrently recycled after termination.
            let mut control = self
                .control
                .lock()
                .map_err(|_| anyhow!("Codex supervisor connection lock was poisoned"))?;
            *control = ControlConnection::default();
            drop(control);

            let sent = unsafe {
                libc::syscall(
                    libc::SYS_pidfd_send_signal,
                    pidfd.as_raw_fd(),
                    libc::SIGTERM,
                    std::ptr::null::<libc::siginfo_t>(),
                    0_u32,
                )
            };
            if sent != 0 {
                return Err(std::io::Error::last_os_error())
                    .context("failed to stop exact Codex App Server through pidfd");
            }
            let mut descriptor = libc::pollfd {
                fd: pidfd.as_raw_fd(),
                events: libc::POLLIN,
                revents: 0,
            };
            let polled = unsafe { libc::poll(&mut descriptor, 1, 5_000) };
            if polled < 0 {
                return Err(std::io::Error::last_os_error())
                    .context("failed while waiting for exact Codex App Server exit");
            }
            if polled == 0 {
                let killed = unsafe {
                    libc::syscall(
                        libc::SYS_pidfd_send_signal,
                        pidfd.as_raw_fd(),
                        libc::SIGKILL,
                        std::ptr::null::<libc::siginfo_t>(),
                        0_u32,
                    )
                };
                if killed != 0 {
                    return Err(std::io::Error::last_os_error())
                        .context("failed to force-stop exact Codex App Server through pidfd");
                }
                descriptor.revents = 0;
                let killed_poll = unsafe { libc::poll(&mut descriptor, 1, 2_000) };
                if killed_poll < 0 {
                    return Err(std::io::Error::last_os_error())
                        .context("failed while waiting for force-stopped Codex App Server exit");
                }
                if killed_poll == 0 {
                    bail!("timed out waiting for force-stopped exact Codex App Server to exit");
                }
            }

            // Reap only when this dashboard is still the process parent. A
            // reconnected dashboard receives ECHILD and safely ignores it.
            let mut status = 0;
            let _ = unsafe { libc::waitpid(record.pid as i32, &mut status, libc::WNOHANG) };
            remove_record_if_same_server(&self.record_path, &self.state_dir, &record)?;
            Ok(())
        }
        #[cfg(not(target_os = "linux"))]
        {
            let _ = record;
            bail!("durable Codex App Server shutdown currently requires Linux")
        }
    }

    pub fn enrich(&self, snapshot: &mut SessionSnapshot) {
        let Ok(mut record) = self.live_record() else {
            return;
        };
        let (pending_requests, terminal_turns) = match self.control.lock() {
            Ok(mut control) => {
                let mut terminal_turns = Vec::new();
                if self.response_lease.is_some()
                    && record
                        .threads
                        .values()
                        .any(|thread| thread.active_turn_id.is_some())
                {
                    match self.refresh_pending_locked(&mut control, &record) {
                        Ok(observed) => terminal_turns = observed,
                        Err(error) => {
                            control.client = None;
                            control.server = None;
                            control.pending_requests.clear();
                            control.terminal_turns.clear();
                            snapshot.warnings.push(format!(
                                "Codex control request synchronization failed: {error:#}"
                            ));
                        }
                    }
                }
                (control.pending_requests.clone(), terminal_turns)
            }
            Err(_) => {
                snapshot
                    .warnings
                    .push("Codex supervisor connection lock was poisoned".into());
                (Vec::new(), Vec::new())
            }
        };
        let changed = clear_terminal_turns(&mut record, &terminal_turns);
        for session in &mut snapshot.sessions {
            if session.provider != Provider::Codex || session.runtime != Runtime::Host {
                continue;
            }
            let Some(owned) = record.threads.get_mut(&session.provider_session_id) else {
                continue;
            };
            if session.state == SessionState::Unknown && owned.active_turn_id.is_none() {
                session.state = SessionState::Completed;
                session.raw_state = Some("owned_idle_not_loaded".into());
            }
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
            // A just-started turn can briefly race a stale idle thread/read
            // snapshot. Keep the exact owned turn authoritative until its
            // matching turn/completed notification (or resume payload) is
            // observed, so interrupt/reply authority cannot disappear.
            if session.state == SessionState::Completed && owned.active_turn_id.is_some() {
                session.state = SessionState::Working;
                session.raw_state = Some("owned_active_turn_awaiting_terminal_event".into());
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

    /// Exact thread IDs created through this durable App Server process.
    /// Discovery uses this to avoid confusing other persisted Codex history
    /// with sessions that Open Agent View can actually control.
    pub fn owned_thread_ids(&self) -> Result<Vec<String>> {
        Ok(self.live_record()?.threads.into_keys().collect())
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
                let Some(active_turn_id) = thread.active_turn_id.as_deref() else {
                    continue;
                };
                let resumed = client.request(
                    "thread/resume",
                    json!({
                        "threadId": thread_id,
                        "cwd": thread.cwd,
                        "approvalPolicy": "on-request",
                        "sandbox": "workspace-write"
                    }),
                )?;
                if resume_reports_terminal_turn(&resumed, active_turn_id) {
                    control
                        .terminal_turns
                        .push((thread_id.clone(), active_turn_id.to_owned()));
                }
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
    ) -> Result<Vec<(String, String)>> {
        let events = self.control_client(control, server)?.drain_events()?;
        for event in events {
            if let Some(terminal) = terminal_turn_from_event(server, &event) {
                control.pending_requests.retain(|request| {
                    request.thread_id != terminal.0 || request.turn_id != terminal.1
                });
                control.terminal_turns.push(terminal);
            }
            if let Some(thread_id) = deleted_thread_from_event(server, &event) {
                control.deleted_threads.insert(thread_id);
            }
            reconcile_pending_event(&mut control.pending_requests, server, event)?;
        }
        Ok(std::mem::take(&mut control.terminal_turns))
    }

    fn apply_terminal_turns(
        &self,
        server: &SupervisorRecord,
        terminal_turns: &[(String, String)],
    ) -> Result<()> {
        if terminal_turns.is_empty() {
            return Ok(());
        }
        self.update_record_for_server(server, |record| {
            clear_terminal_turns(record, terminal_turns);
            Ok(())
        })
    }

    fn client_invocation(&self, server: &SupervisorRecord) -> AppServerInvocation {
        match self.client_transport {
            SupervisorClientTransport::UnixWebSocket => {
                AppServerInvocation::unix_websocket(server.socket_path.clone())
            }
            #[cfg(all(test, unix))]
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
        let _recovery = RecoveryLock::acquire_shared(&self.recovery_lock_path)?;
        self.ensure_endpoint_inner()
    }

    fn ensure_endpoint_inner(&self) -> Result<SupervisorRecord> {
        let _lock = StateLock::acquire(&self.lock_path)?;
        if let Some(record) = load_record(&self.record_path, &self.state_dir)? {
            if record.version != RECORD_VERSION {
                bail!(
                    "unsupported Codex supervisor record version {}",
                    record.version
                );
            }
            let live = verify_process(&record)?;
            if !record_uses_executable(&record, &self.codex_bin) && live {
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
        // npm launchers may remain as a signal-forwarding parent while a native
        // child owns the actual listener. Persist the exact process holding the
        // listening Unix socket, not merely Command::spawn's wrapper PID.
        let server_pid = match unix_socket_listener_owner(
            &socket_path,
            deadline.saturating_duration_since(Instant::now()),
        ) {
            Ok(owner) => owner,
            Err(error) => {
                let _ = child.kill();
                let _ = child.wait();
                return Err(error).context("could not identify the Codex App Server listener");
            }
        };
        let process_start_token = process_start_token(server_pid)?;
        let process_cmdline = process_cmdline(server_pid)?;
        if process_cmdline.is_empty() {
            let _ = child.kill();
            let _ = child.wait();
            bail!("new Codex App Server exposed an empty process command line");
        }

        let record = SupervisorRecord {
            version: RECORD_VERSION,
            pid: server_pid,
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

fn terminal_turn_from_event(server: &SupervisorRecord, event: &Value) -> Option<(String, String)> {
    if event.get("method").and_then(Value::as_str) != Some("turn/completed") {
        return None;
    }
    let thread_id = event.pointer("/params/threadId")?.as_str()?;
    let turn_id = event.pointer("/params/turn/id")?.as_str()?;
    let status = event.pointer("/params/turn/status")?.as_str()?;
    if !turn_status_is_terminal(status)
        || server
            .threads
            .get(thread_id)
            .and_then(|thread| thread.active_turn_id.as_deref())
            != Some(turn_id)
    {
        return None;
    }
    Some((thread_id.to_owned(), turn_id.to_owned()))
}

fn resume_reports_terminal_turn(response: &Value, active_turn_id: &str) -> bool {
    response
        .pointer("/thread/turns")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
        .any(|turn| {
            turn.get("id").and_then(Value::as_str) == Some(active_turn_id)
                && turn
                    .get("status")
                    .and_then(Value::as_str)
                    .is_some_and(turn_status_is_terminal)
        })
}

fn turn_status_is_terminal(status: &str) -> bool {
    matches!(status, "completed" | "interrupted" | "failed")
}

fn deleted_thread_from_event(server: &SupervisorRecord, event: &Value) -> Option<String> {
    if event.get("method").and_then(Value::as_str) != Some("thread/deleted") {
        return None;
    }
    let thread_id = event.pointer("/params/threadId")?.as_str()?;
    server
        .threads
        .contains_key(thread_id)
        .then(|| thread_id.to_owned())
}

fn app_server_request_timed_out(method: &str, error: &anyhow::Error) -> bool {
    let message = format!("{error:#}").to_ascii_lowercase();
    message.contains(&format!("waiting for {method}"))
        && (message.contains("timed out") || message.contains("resource temporarily unavailable"))
}

fn codex_thread_is_missing(client: &mut AppServerClient, thread_id: &str) -> Result<bool> {
    const PAGE_SIZE: u64 = 100;
    const MAX_PAGES: usize = 100;
    for archived in [false, true] {
        let mut cursor: Option<String> = None;
        for _ in 0..MAX_PAGES {
            let response = client.request_with_timeout(
                "thread/list",
                json!({
                    "archived": archived,
                    "cursor": cursor,
                    "limit": PAGE_SIZE,
                    "sortKey": "updated_at",
                    "sortDirection": "desc"
                }),
                Duration::from_secs(15),
            )?;
            let data = response
                .get("data")
                .and_then(Value::as_array)
                .context("thread/list deletion check omitted data")?;
            if data
                .iter()
                .any(|thread| thread.get("id").and_then(Value::as_str) == Some(thread_id))
            {
                return Ok(false);
            }
            cursor = response
                .get("nextCursor")
                .and_then(Value::as_str)
                .map(ToOwned::to_owned);
            if cursor.is_none() {
                break;
            }
        }
        if cursor.is_some() {
            bail!(
                "Codex {} thread inventory exceeded the 10,000-record deletion verification cap",
                if archived { "archived" } else { "ordinary" }
            );
        }
    }
    Ok(true)
}

fn codex_thread_is_missing_from_fresh_process(codex_bin: &str, thread_id: &str) -> Result<bool> {
    let mut last_error = None;
    for attempt in 0..3 {
        if attempt > 0 {
            thread::sleep(Duration::from_millis(250 * attempt as u64));
        }
        let result = AppServerClient::connect(&AppServerInvocation::direct(codex_bin))
            .context("failed to start an independent Codex deletion observer")
            .and_then(|mut observer| codex_thread_is_missing(&mut observer, thread_id));
        match result {
            Ok(missing) => return Ok(missing),
            Err(error) => last_error = Some(error),
        }
    }
    Err(last_error.expect("fresh Codex observer attempted at least once"))
        .context("Codex deletion observer did not recover after three isolated attempts")
}

fn delete_codex_thread_from_fresh_process(codex_bin: &str, thread_id: &str) -> Result<()> {
    let mut client = AppServerClient::connect(&AppServerInvocation::direct(codex_bin))
        .context("failed to start an independent Codex deletion worker")?;
    let result = client.request_with_timeout(
        "thread/delete",
        json!({"threadId": thread_id}),
        Duration::from_secs(15),
    );
    if result.is_ok() {
        return Ok(());
    }
    let notification = client
        .drain_events()
        .unwrap_or_default()
        .into_iter()
        .any(|event| {
            event.get("method").and_then(Value::as_str) == Some("thread/deleted")
                && event.pointer("/params/threadId").and_then(Value::as_str) == Some(thread_id)
        });
    if notification {
        Ok(())
    } else {
        Err(result.expect_err("failed deletion result checked above"))
            .context("fresh Codex deletion worker did not confirm the exact thread")
    }
}

fn clear_terminal_turns(
    record: &mut SupervisorRecord,
    terminal_turns: &[(String, String)],
) -> bool {
    let mut changed = false;
    for (thread_id, turn_id) in terminal_turns {
        if let Some(thread) = record.threads.get_mut(thread_id) {
            if thread.active_turn_id.as_deref() == Some(turn_id) {
                thread.active_turn_id = None;
                changed = true;
            }
        }
    }
    changed
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
    #[cfg(not(unix))]
    {
        let _ = path;
        bail!("durable Codex supervision requires Unix process identity verification")
    }
    #[cfg(unix)]
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

#[cfg(all(unix, not(target_os = "linux")))]
fn effective_uid() -> Result<u32> {
    Ok(unsafe { libc::geteuid() })
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
    crate::fs_util::replace_file(&temporary, path)
        .with_context(|| format!("failed to replace {}", path.display()))
}

#[cfg(target_os = "linux")]
fn remove_record_if_same_server(
    record_path: &Path,
    state_dir: &Path,
    stopped: &SupervisorRecord,
) -> Result<()> {
    let Some(current) = load_record(record_path, state_dir)? else {
        return Ok(());
    };
    if !current.same_process(stopped) {
        bail!("Codex supervisor identity changed before shutdown cleanup");
    }
    if let Ok(metadata) = fs::symlink_metadata(&current.socket_path) {
        #[cfg(unix)]
        {
            use std::os::unix::fs::FileTypeExt;
            if !metadata.file_type().is_socket() {
                bail!("refusing to remove a non-socket Codex endpoint during cleanup");
            }
        }
        #[cfg(not(unix))]
        {
            let _ = metadata;
        }
        fs::remove_file(&current.socket_path).with_context(|| {
            format!(
                "failed to remove stopped Codex socket {}",
                current.socket_path.display()
            )
        })?;
    }
    fs::remove_file(record_path).with_context(|| {
        format!(
            "failed to remove stopped Codex record {}",
            record_path.display()
        )
    })
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

fn record_uses_executable(record: &SupervisorRecord, configured: &str) -> bool {
    if record.codex_bin == configured {
        return true;
    }
    let Ok(configured) = fs::canonicalize(configured) else {
        return false;
    };
    // npm-style launchers replace the requested shim with an interpreter and
    // an absolute script path. The verified live cmdline is immutable for the
    // lifetime of this record, so accept a newly resolved path only when it
    // canonicalizes to argv[0] (standalone binary) or argv[1] (interpreter
    // script). A changed symlink target remains a hard mismatch.
    record
        .process_cmdline
        .split(|byte| *byte == 0)
        .filter(|argument| !argument.is_empty())
        .take(2)
        .filter_map(|argument| std::str::from_utf8(argument).ok())
        .filter_map(|argument| fs::canonicalize(argument).ok())
        .any(|argument| argument == configured)
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

#[cfg(target_os = "macos")]
fn process_start_token(pid: u32) -> Result<String> {
    let info = darwin_process_info(pid)?;
    Ok(format!(
        "{}:{}",
        info.pbi_start_tvsec, info.pbi_start_tvusec
    ))
}

#[cfg(not(any(target_os = "linux", target_os = "macos")))]
fn process_start_token(_: u32) -> Result<String> {
    bail!("process start-token verification is unavailable on this platform")
}

#[cfg(target_os = "linux")]
fn process_cmdline(pid: u32) -> Result<Vec<u8>> {
    fs::read(format!("/proc/{pid}/cmdline")).map_err(Into::into)
}

#[cfg(target_os = "macos")]
fn process_cmdline(pid: u32) -> Result<Vec<u8>> {
    use std::mem::size_of;

    let mut argument_limit = 0_i32;
    let mut argument_limit_size = size_of::<libc::c_int>();
    let mut argument_limit_mib = [libc::CTL_KERN, libc::KERN_ARGMAX];
    let result = unsafe {
        libc::sysctl(
            argument_limit_mib.as_mut_ptr(),
            argument_limit_mib.len() as libc::c_uint,
            (&mut argument_limit as *mut libc::c_int).cast(),
            &mut argument_limit_size,
            std::ptr::null_mut(),
            0,
        )
    };
    if result != 0 {
        return Err(std::io::Error::last_os_error())
            .context("failed to read the macOS process argument limit");
    }
    if argument_limit <= 0 || argument_limit as usize > 16 * 1024 * 1024 {
        bail!("macOS reported an invalid process argument limit");
    }

    let mut bytes = vec![0_u8; argument_limit as usize];
    let mut bytes_len = bytes.len();
    let mut arguments_mib = [libc::CTL_KERN, libc::KERN_PROCARGS2, pid as libc::c_int];
    let result = unsafe {
        libc::sysctl(
            arguments_mib.as_mut_ptr(),
            arguments_mib.len() as libc::c_uint,
            bytes.as_mut_ptr().cast(),
            &mut bytes_len,
            std::ptr::null_mut(),
            0,
        )
    };
    if result != 0 {
        return Err(std::io::Error::last_os_error())
            .with_context(|| format!("failed to read command line for macOS process {pid}"));
    }
    bytes.truncate(bytes_len);
    parse_darwin_process_arguments(&bytes)
}

#[cfg(target_os = "macos")]
fn darwin_process_info(pid: u32) -> Result<libc::proc_bsdinfo> {
    use std::mem::{size_of, MaybeUninit};

    let mut info = MaybeUninit::<libc::proc_bsdinfo>::zeroed();
    let expected = size_of::<libc::proc_bsdinfo>();
    let returned = unsafe {
        libc::proc_pidinfo(
            pid as libc::c_int,
            libc::PROC_PIDTBSDINFO,
            0,
            info.as_mut_ptr().cast(),
            expected as libc::c_int,
        )
    };
    if returned == 0 {
        return Err(std::io::Error::last_os_error())
            .with_context(|| format!("failed to inspect macOS process {pid}"));
    }
    if returned != expected as libc::c_int {
        bail!("macOS returned incomplete identity information for process {pid}");
    }
    let info = unsafe { info.assume_init() };
    if info.pbi_pid != pid {
        bail!("macOS returned identity information for the wrong process");
    }
    Ok(info)
}

#[cfg(target_os = "macos")]
fn parse_darwin_process_arguments(bytes: &[u8]) -> Result<Vec<u8>> {
    use std::mem::size_of;

    let header = bytes
        .get(..size_of::<libc::c_int>())
        .context("macOS process arguments omitted argc")?;
    let argc = libc::c_int::from_ne_bytes(
        header
            .try_into()
            .expect("argc slice has the platform integer width"),
    );
    if argc <= 0 || argc as usize > 100_000 {
        bail!("macOS process arguments reported an invalid argc");
    }

    let mut cursor = size_of::<libc::c_int>();
    while bytes.get(cursor).is_some_and(|byte| *byte != 0) {
        cursor += 1;
    }
    if cursor >= bytes.len() {
        bail!("macOS process arguments omitted the executable terminator");
    }
    while bytes.get(cursor) == Some(&0) {
        cursor += 1;
    }

    let mut command_line = Vec::new();
    for _ in 0..argc {
        let remaining = bytes
            .get(cursor..)
            .context("macOS process arguments ended before argc")?;
        let argument_len = remaining
            .iter()
            .position(|byte| *byte == 0)
            .context("macOS process argument was not NUL terminated")?;
        command_line.extend_from_slice(&remaining[..argument_len]);
        command_line.push(0);
        cursor += argument_len + 1;
    }
    Ok(command_line)
}

#[cfg(not(any(target_os = "linux", target_os = "macos")))]
fn process_cmdline(_: u32) -> Result<Vec<u8>> {
    bail!("process command-line verification is unavailable on this platform")
}

#[cfg(target_os = "linux")]
fn unix_socket_listener_owner(path: &Path, timeout: Duration) -> Result<u32> {
    use std::os::unix::fs::MetadataExt;

    let deadline = Instant::now() + timeout;
    let expected_path = path.to_string_lossy();
    let expected_listen = format!("unix://{expected_path}");
    loop {
        let table = fs::read_to_string("/proc/net/unix")?;
        let inode = table.lines().skip(1).find_map(|line| {
            let fields = line.split_whitespace().collect::<Vec<_>>();
            (fields.len() >= 8 && fields[7..].join(" ") == expected_path)
                .then(|| fields[6].to_owned())
        });
        if let Some(inode) = inode {
            let descriptor = PathBuf::from(format!("socket:[{inode}]"));
            for entry in fs::read_dir("/proc")? {
                let entry = entry?;
                let Some(pid) = entry
                    .file_name()
                    .to_str()
                    .and_then(|value| value.parse::<u32>().ok())
                else {
                    continue;
                };
                let Ok(metadata) = entry.metadata() else {
                    continue;
                };
                if metadata.uid() != effective_uid()? {
                    continue;
                }
                let Ok(fds) = fs::read_dir(entry.path().join("fd")) else {
                    continue;
                };
                let owns_descriptor = fds
                    .filter_map(Result::ok)
                    .any(|fd| fs::read_link(fd.path()).ok().as_ref() == Some(&descriptor));
                if !owns_descriptor {
                    continue;
                }
                let cmdline = process_cmdline(pid)?;
                let exact_listener = cmdline
                    .split(|byte| *byte == 0)
                    .filter_map(|argument| std::str::from_utf8(argument).ok())
                    .any(|argument| argument == expected_listen);
                if exact_listener {
                    return Ok(pid);
                }
            }
        }
        if Instant::now() >= deadline {
            bail!(
                "timed out resolving the process that owns Unix socket {}",
                path.display()
            );
        }
        thread::sleep(Duration::from_millis(20));
    }
}

#[cfg(target_os = "macos")]
fn unix_socket_listener_owner(path: &Path, timeout: Duration) -> Result<u32> {
    use std::os::unix::ffi::OsStrExt;

    let deadline = Instant::now() + timeout;
    let expected_path = path.to_string_lossy();
    let expected_listen = format!("unix://{expected_path}");
    loop {
        let output = Command::new("/usr/sbin/lsof")
            .args(["-n", "-a", "-U", "-F0pn", "--"])
            .arg(path)
            .env("LC_ALL", "C")
            .output()
            .context("failed to inspect the macOS Unix socket owner with /usr/sbin/lsof")?;
        if output.status.success() || output.status.code() == Some(1) {
            let mut current_pid = None;
            let mut candidates = BTreeSet::new();
            for field in output.stdout.split(|byte| *byte == 0) {
                let field = field
                    .strip_prefix(b"\n")
                    .or_else(|| field.strip_prefix(b"\r\n"))
                    .unwrap_or(field);
                match field.split_first() {
                    Some((b'p', value)) => {
                        current_pid = std::str::from_utf8(value)
                            .ok()
                            .and_then(|value| value.parse::<u32>().ok());
                    }
                    Some((b'n', value))
                        if value == expected_path.as_bytes()
                            || value == path.as_os_str().as_bytes() =>
                    {
                        if let Some(pid) = current_pid {
                            candidates.insert(pid);
                        }
                    }
                    _ => {}
                }
            }
            for pid in candidates {
                let Ok(info) = darwin_process_info(pid) else {
                    continue;
                };
                if info.pbi_uid != effective_uid()? {
                    continue;
                }
                let Ok(cmdline) = process_cmdline(pid) else {
                    continue;
                };
                let exact_listener = cmdline
                    .split(|byte| *byte == 0)
                    .filter_map(|argument| std::str::from_utf8(argument).ok())
                    .any(|argument| argument == expected_listen);
                if exact_listener {
                    return Ok(pid);
                }
            }
        } else if Instant::now() >= deadline {
            bail!(
                "failed to resolve macOS Unix socket owner: {}",
                String::from_utf8_lossy(&output.stderr).trim()
            );
        }
        if Instant::now() >= deadline {
            bail!(
                "timed out resolving the process that owns Unix socket {}",
                path.display()
            );
        }
        thread::sleep(Duration::from_millis(40));
    }
}

#[cfg(not(any(target_os = "linux", target_os = "macos")))]
fn unix_socket_listener_owner(_: &Path, _: Duration) -> Result<u32> {
    bail!("Unix socket listener ownership verification is unavailable on this platform")
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

struct RecoveryLock {
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

impl RecoveryLock {
    fn acquire_shared(path: &Path) -> Result<Self> {
        Self::acquire(path, false)
    }

    fn acquire_exclusive(path: &Path) -> Result<Self> {
        Self::acquire(path, true)
    }

    fn acquire(path: &Path, exclusive: bool) -> Result<Self> {
        reject_unsafe_existing_file(path, "Codex recovery lock")?;
        let mut options = OpenOptions::new();
        options.create(true).read(true).write(true);
        #[cfg(unix)]
        {
            use std::os::unix::fs::OpenOptionsExt;
            options.mode(0o600).custom_flags(libc::O_NOFOLLOW);
        }
        let file = options
            .open(path)
            .with_context(|| format!("failed to open recovery lock {}", path.display()))?;
        verify_private_file(&file, "Codex recovery lock")?;
        #[cfg(unix)]
        {
            use std::os::fd::AsRawFd;
            let operation = if exclusive {
                libc::LOCK_EX
            } else {
                libc::LOCK_SH
            };
            let result = unsafe { libc::flock(file.as_raw_fd(), operation) };
            if result != 0 {
                return Err(std::io::Error::last_os_error())
                    .context("failed to coordinate Codex server recovery");
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

impl Drop for RecoveryLock {
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

fn validate_catalog_model<'a>(model: &'a str, provider: &str) -> Result<&'a str> {
    if model.is_empty() || model.len() > 128 {
        bail!("{provider} model name must contain between 1 and 128 bytes");
    }
    if model
        .chars()
        .any(|character| character.is_control() || character.is_whitespace())
    {
        bail!("{provider} model name contains whitespace or control characters");
    }
    Ok(model)
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
    #[cfg(any(target_os = "linux", target_os = "macos"))]
    use std::os::unix::fs::PermissionsExt;
    #[cfg(any(target_os = "linux", target_os = "macos"))]
    use std::sync::Arc;

    #[cfg(any(target_os = "linux", target_os = "macos"))]
    use tempfile::tempdir;

    #[cfg(target_os = "linux")]
    use crate::adapters::{CodexSource, DiscoveryRequest, SessionSource};
    use crate::domain::{SessionKind, SessionState};

    use super::*;

    #[cfg(any(target_os = "linux", target_os = "macos"))]
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

    #[cfg(target_os = "macos")]
    #[test]
    fn macos_durable_server_starts_with_exact_identity_and_reconnects() {
        let directory = tempdir().unwrap();
        let state_dir = directory.path().join("state");
        let mock = directory.path().join("mock-codex.py");
        fs::write(&mock, MOCK_CODEX).unwrap();
        fs::set_permissions(&mock, fs::Permissions::from_mode(0o700)).unwrap();

        let first = Arc::new(
            CodexSupervisor::with_state_dir(mock.to_string_lossy(), state_dir.clone()).unwrap(),
        );
        assert_eq!(
            first.available_models().unwrap(),
            vec!["gpt-visible", "gpt-second"]
        );
        let first_record = first.live_record().unwrap();
        let _process_guard = VerifiedTestProcess(first_record.clone());
        assert!(verify_process(&first_record).unwrap());
        assert_eq!(
            fs::metadata(&state_dir).unwrap().permissions().mode() & 0o777,
            0o700
        );

        let second = CodexSupervisor::with_state_dir(mock.to_string_lossy(), state_dir).unwrap();
        assert_eq!(
            second.available_models().unwrap(),
            vec!["gpt-visible", "gpt-second"]
        );
        assert!(second.live_record().unwrap().same_process(&first_record));
    }

    #[cfg(target_os = "linux")]
    #[test]
    fn resolved_npm_shim_reconnects_only_to_the_same_verified_script() {
        use std::os::unix::fs::symlink;

        let directory = tempdir().unwrap();
        let script = directory.path().join("codex.js");
        let changed_script = directory.path().join("changed-codex.js");
        let shim = directory.path().join("codex");
        fs::write(&script, b"original").unwrap();
        fs::write(&changed_script, b"changed").unwrap();
        symlink(&script, &shim).unwrap();
        let process_cmdline = format!("node\0{}\0app-server\0", script.display()).into_bytes();
        let record = SupervisorRecord {
            version: RECORD_VERSION,
            pid: std::process::id(),
            process_start_token: "test".into(),
            process_cmdline,
            codex_bin: "codex".into(),
            socket_path: directory.path().join("server.sock"),
            created_at_ms: 1,
            threads: BTreeMap::new(),
        };

        assert!(record_uses_executable(&record, shim.to_str().unwrap()));
        assert!(record_uses_executable(&record, "codex"));

        fs::remove_file(&shim).unwrap();
        symlink(&changed_script, &shim).unwrap();
        assert!(!record_uses_executable(&record, shim.to_str().unwrap()));
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
    fn terminal_reconciliation_requires_the_exact_owned_thread_and_turn() {
        let mut threads = BTreeMap::new();
        threads.insert(
            "owned-thread".into(),
            OwnedThread {
                cwd: PathBuf::from("/tmp"),
                created_at_ms: 1,
                active_turn_id: Some("owned-turn".into()),
            },
        );
        let mut server = SupervisorRecord {
            version: RECORD_VERSION,
            pid: std::process::id(),
            process_start_token: "test".into(),
            process_cmdline: vec![1],
            codex_bin: "codex".into(),
            socket_path: PathBuf::from("/tmp/test.sock"),
            created_at_ms: 1,
            threads,
        };
        let wrong = json!({
            "method": "turn/completed",
            "params": {
                "threadId": "owned-thread",
                "turn": {"id": "other-turn", "status": "completed", "items": []}
            }
        });
        assert_eq!(terminal_turn_from_event(&server, &wrong), None);

        let exact = json!({
            "method": "turn/completed",
            "params": {
                "threadId": "owned-thread",
                "turn": {"id": "owned-turn", "status": "completed", "items": []}
            }
        });
        let terminal = terminal_turn_from_event(&server, &exact).unwrap();
        assert!(clear_terminal_turns(&mut server, &[terminal]));
        assert_eq!(server.threads["owned-thread"].active_turn_id, None);

        assert!(resume_reports_terminal_turn(
            &json!({"thread": {"turns": [
                {"id": "owned-turn", "status": "interrupted", "items": []}
            ]}}),
            "owned-turn"
        ));
        assert!(!resume_reports_terminal_turn(
            &json!({"thread": {"turns": [
                {"id": "owned-turn", "status": "inProgress", "items": []}
            ]}}),
            "owned-turn"
        ));
        assert_eq!(
            deleted_thread_from_event(
                &server,
                &json!({
                    "method": "thread/deleted",
                    "params": {"threadId": "owned-thread"}
                })
            ),
            Some("owned-thread".into())
        );
        assert_eq!(
            deleted_thread_from_event(
                &server,
                &json!({
                    "method": "thread/deleted",
                    "params": {"threadId": "external-thread"}
                })
            ),
            None
        );
    }

    #[cfg(target_os = "linux")]
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

    #[cfg(target_os = "linux")]
    #[test]
    fn explicit_shutdown_requires_authority_and_exact_process_identity() {
        let directory = tempdir().unwrap();
        let state_dir = directory.path().join("state");
        let mock = directory.path().join("mock-codex.py");
        fs::write(&mock, MOCK_CODEX).unwrap();
        fs::set_permissions(&mock, fs::Permissions::from_mode(0o700)).unwrap();

        let first =
            CodexSupervisor::with_state_dir(mock.to_string_lossy(), state_dir.clone()).unwrap();
        first.available_models().unwrap();
        let original = first.live_record().unwrap();
        let _process_guard = VerifiedTestProcess(original.clone());

        let second =
            CodexSupervisor::with_state_dir(mock.to_string_lossy(), state_dir.clone()).unwrap();
        assert!(second.shutdown_server().is_err());
        assert!(verify_process(&original).unwrap());

        let mut tampered = original.clone();
        tampered.process_cmdline.push(b'x');
        save_record(&first.record_path, &tampered).unwrap();
        assert!(first.shutdown_server().is_err());
        assert!(verify_process(&original).unwrap());

        save_record(&first.record_path, &original).unwrap();
        first.shutdown_server().unwrap();
        assert!(!verify_process(&original).unwrap());
        assert!(!first.record_path.exists());
    }

    #[cfg(target_os = "linux")]
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
            first.available_models().unwrap(),
            vec!["gpt-visible", "gpt-second"]
        );
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
                ..DiscoveryRequest::default()
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

    #[cfg(target_os = "linux")]
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

    #[cfg(target_os = "linux")]
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

    #[cfg(target_os = "linux")]
    fn discover_owned(supervisor: &Arc<CodexSupervisor>, thread_id: &str) -> AgentSession {
        let source = CodexSource::managed_owned(supervisor.clone());
        let sessions = source
            .discover(&DiscoveryRequest {
                include_completed: true,
                include_interactive: true,
                cwd: None,
                ..DiscoveryRequest::default()
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

    #[cfg(target_os = "linux")]
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
    #[cfg(any(target_os = "linux", target_os = "macos"))]
    struct VerifiedTestProcess(SupervisorRecord);

    #[cfg(any(target_os = "linux", target_os = "macos"))]
    impl Drop for VerifiedTestProcess {
        fn drop(&mut self) {
            if !verify_process(&self.0).unwrap_or(false) {
                return;
            }
            unsafe { libc::kill(self.0.pid as i32, libc::SIGTERM) };
            if reap_test_process(&self.0, Duration::from_secs(1)) {
                return;
            }
            if verify_process(&self.0).unwrap_or(false) {
                unsafe { libc::kill(self.0.pid as i32, libc::SIGKILL) };
                let _ = reap_test_process(&self.0, Duration::from_secs(1));
            }
        }
    }

    #[cfg(any(target_os = "linux", target_os = "macos"))]
    fn reap_test_process(record: &SupervisorRecord, timeout: Duration) -> bool {
        let deadline = Instant::now() + timeout;
        loop {
            let mut status = 0;
            let waited = unsafe { libc::waitpid(record.pid as i32, &mut status, libc::WNOHANG) };
            if waited == record.pid as i32
                || (waited < 0 && !verify_process(record).unwrap_or(false))
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

    #[cfg(any(target_os = "linux", target_os = "macos"))]
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
                result = {"thread": {
                    "id": state["thread"], "cwd": "/tmp", "createdAt": 1,
                    "updatedAt": 2, "preview": "Implement the test", "name": None,
                    "status": {"type": "active" if state["active"] else "idle", "activeFlags": []},
                    "source": "appServer", "turns": [{"items": [
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
            elif method == "model/list":
                if message.get("params", {}).get("cursor") is None:
                    result = {"data": [
                        {"model": "gpt-visible", "hidden": False},
                        {"model": "gpt-hidden", "hidden": True}
                    ], "nextCursor": "models-page-2"}
                else:
                    result = {"data": [{"id": "gpt-second", "hidden": False}], "nextCursor": None}
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

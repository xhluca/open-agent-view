use std::collections::{BTreeSet, HashMap, VecDeque};
use std::io::{BufRead, BufReader, Read, Write};
use std::path::{Path, PathBuf};
use std::process::{Child, ChildStdin, Command, Stdio};
use std::sync::{mpsc, Arc, Mutex};
use std::thread::{self, JoinHandle};
use std::time::{Duration, Instant, SystemTime};

use anyhow::{anyhow, bail, Context, Result};
use serde::Deserialize;
use serde_json::{json, Value};

use super::copilot_managed::CopilotSupervisor;
use super::{DiscoveryRequest, SessionSource};
use crate::control::{
    run_native_authentication, ControlOutcome, LaunchMode, LaunchPresentation, LaunchRequest,
    ProviderController,
};
use crate::domain::{AgentSession, Provider, Runtime, SessionKind, SessionSnapshot, SessionState};

const REQUEST_TIMEOUT: Duration = Duration::from_secs(8);
const MAX_MESSAGE_BYTES: u64 = 8 * 1024 * 1024;
const MAX_STDERR_BYTES: u64 = 1024 * 1024;
const MAX_QUEUED_MESSAGES: usize = 1024;
const MAX_PENDING_PERMISSIONS: usize = 256;
const MAX_LIST_PAGES: usize = 100;
const EXECUTABLE_BUSY_RETRIES: usize = 5;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum CopilotAcpMode {
    /// Starts only the protocol surface needed for read-only session listing.
    Discovery,
    /// Starts an interactive protocol controller without broad permission flags.
    Control,
}

#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct CopilotAcpCapabilities {
    pub list_sessions: bool,
    pub load_session: bool,
    pub close_session: bool,
    pub delete_session: bool,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct CopilotPermissionOption {
    pub id: String,
    pub name: String,
    pub kind: String,
}

#[derive(Clone, Debug, PartialEq)]
pub struct CopilotPermissionRequest {
    pub request_id: Value,
    pub session_id: String,
    pub options: Vec<CopilotPermissionOption>,
    pub raw_params: Value,
}

#[derive(Clone, Debug, PartialEq)]
pub enum CopilotAcpMessage {
    Response {
        id: Value,
        result: std::result::Result<Value, Value>,
    },
    SessionUpdate {
        session_id: String,
        update: Value,
    },
    PermissionRequest(CopilotPermissionRequest),
    Notification {
        method: String,
        params: Value,
    },
    UnsupportedRequest {
        id: Value,
        method: String,
        params: Value,
    },
}

#[derive(Clone, Debug)]
struct PendingPermission {
    request_id: Value,
    session_id: String,
    option_ids: BTreeSet<String>,
}

/// One exact Copilot ACP process and its connection-owned authority.
///
/// This type deliberately does not reconnect behind the caller's back. Active
/// prompts and permission request IDs belong to this process connection; a new
/// connection must load a persisted session and establish its own authority.
pub struct CopilotAcpConnection {
    child: Child,
    stdin: Option<ChildStdin>,
    messages: mpsc::Receiver<std::result::Result<Value, String>>,
    stdout_thread: Option<JoinHandle<()>>,
    stderr: Arc<Mutex<Vec<u8>>>,
    stderr_thread: Option<JoinHandle<()>>,
    next_id: u64,
    queued: VecDeque<CopilotAcpMessage>,
    pending_permissions: HashMap<String, PendingPermission>,
    capabilities: CopilotAcpCapabilities,
}

impl CopilotAcpConnection {
    pub fn connect(executable: impl AsRef<str>, mode: CopilotAcpMode) -> Result<Self> {
        let executable = executable.as_ref();
        let mut command = Command::new(executable);
        command.args([
            "--acp",
            "--stdio",
            "--no-auto-update",
            "--no-remote",
            "--no-remote-export",
        ]);
        if mode == CopilotAcpMode::Discovery {
            // Listing sessions must not start repository customizations or MCP
            // servers. Control connections retain the user's ordinary setup.
            command.args(["--disable-builtin-mcps", "--no-custom-instructions"]);
        }
        command
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped());

        let mut attempts = 0;
        let mut child = loop {
            match command.spawn() {
                Ok(child) => break child,
                Err(error)
                    if error.raw_os_error() == Some(libc::ETXTBSY)
                        && attempts < EXECUTABLE_BUSY_RETRIES =>
                {
                    attempts += 1;
                    thread::sleep(Duration::from_millis(10));
                }
                Err(error) => {
                    return Err(error).with_context(|| {
                        format!("failed to start Copilot ACP executable `{executable}`")
                    })
                }
            }
        };
        let stdin = child
            .stdin
            .take()
            .context("failed to open Copilot ACP stdin")?;
        let stdout = child
            .stdout
            .take()
            .context("failed to open Copilot ACP stdout")?;
        let child_stderr = child
            .stderr
            .take()
            .context("failed to open Copilot ACP stderr")?;

        let (sender, messages) = mpsc::sync_channel(MAX_QUEUED_MESSAGES);
        let stdout_thread = thread::spawn(move || read_ndjson(stdout, sender));
        let stderr = Arc::new(Mutex::new(Vec::new()));
        let stderr_sink = stderr.clone();
        let stderr_thread = thread::spawn(move || {
            let mut reader = BufReader::new(child_stderr);
            let mut bytes = Vec::new();
            let _ = reader
                .by_ref()
                .take(MAX_STDERR_BYTES + 1)
                .read_to_end(&mut bytes);
            bytes.truncate(MAX_STDERR_BYTES as usize);
            if let Ok(mut sink) = stderr_sink.lock() {
                *sink = bytes;
            }
        });

        let mut connection = Self {
            child,
            stdin: Some(stdin),
            messages,
            stdout_thread: Some(stdout_thread),
            stderr,
            stderr_thread: Some(stderr_thread),
            next_id: 1,
            queued: VecDeque::new(),
            pending_permissions: HashMap::new(),
            capabilities: CopilotAcpCapabilities::default(),
        };
        connection.initialize()?;
        Ok(connection)
    }

    pub fn capabilities(&self) -> &CopilotAcpCapabilities {
        &self.capabilities
    }

    pub fn list_sessions(&mut self) -> Result<Vec<CopilotSessionInfo>> {
        if !self.capabilities.list_sessions {
            bail!("installed Copilot ACP server does not advertise session/list");
        }
        let mut sessions = Vec::new();
        let mut cursor: Option<String> = None;
        let mut seen_cursors = BTreeSet::new();
        for _ in 0..MAX_LIST_PAGES {
            let params = match cursor.as_ref() {
                Some(cursor) => json!({"cursor": cursor}),
                None => json!({}),
            };
            let response = self.request_sync("session/list", params, REQUEST_TIMEOUT)?;
            let page = parse_copilot_session_page(&response)?;
            sessions.extend(page.sessions);
            let Some(next) = page.next_cursor else {
                return Ok(sessions);
            };
            if !seen_cursors.insert(next.clone()) {
                bail!("Copilot ACP session/list repeated a pagination cursor");
            }
            cursor = Some(next);
        }
        bail!("Copilot ACP session/list exceeded the {MAX_LIST_PAGES}-page safety cap")
    }

    pub fn begin_new_session(&mut self, cwd: &Path) -> Result<u64> {
        require_absolute_cwd(cwd)?;
        self.send_request("session/new", json!({"cwd": cwd, "mcpServers": []}))
    }

    pub fn begin_load_session(&mut self, session_id: &str, cwd: &Path) -> Result<u64> {
        if !self.capabilities.load_session {
            bail!("installed Copilot ACP server does not advertise session/load");
        }
        require_session_id(session_id)?;
        require_absolute_cwd(cwd)?;
        self.send_request(
            "session/load",
            json!({"sessionId": session_id, "cwd": cwd, "mcpServers": []}),
        )
    }

    pub fn begin_prompt(&mut self, session_id: &str, prompt: &str) -> Result<u64> {
        require_session_id(session_id)?;
        if prompt.is_empty() {
            bail!("Copilot prompt must not be empty");
        }
        self.send_request(
            "session/prompt",
            json!({
                "sessionId": session_id,
                "prompt": [{"type": "text", "text": prompt}]
            }),
        )
    }

    pub fn begin_set_config_option(
        &mut self,
        session_id: &str,
        config_id: &str,
        value: &str,
    ) -> Result<u64> {
        require_session_id(session_id)?;
        if config_id.is_empty() || value.is_empty() {
            bail!("Copilot ACP config option ID and value must not be empty");
        }
        self.send_request(
            "session/set_config_option",
            json!({"sessionId": session_id, "configId": config_id, "value": value}),
        )
    }

    pub fn cancel_session(&mut self, session_id: &str) -> Result<()> {
        require_session_id(session_id)?;
        let pending = self
            .pending_permissions
            .values()
            .filter(|request| request.session_id == session_id)
            .map(|request| request.request_id.clone())
            .collect::<Vec<_>>();
        for request_id in pending {
            self.respond_permission_cancelled(&request_id)?;
        }
        self.send_notification("session/cancel", json!({"sessionId": session_id}))
    }

    pub fn begin_close_session(&mut self, session_id: &str) -> Result<u64> {
        if !self.capabilities.close_session {
            bail!("installed Copilot ACP server does not advertise session/close");
        }
        require_session_id(session_id)?;
        let pending = self
            .pending_permissions
            .values()
            .filter(|request| request.session_id == session_id)
            .map(|request| request.request_id.clone())
            .collect::<Vec<_>>();
        for request_id in pending {
            self.respond_permission_cancelled(&request_id)?;
        }
        self.send_request("session/close", json!({"sessionId": session_id}))
    }

    pub fn respond_permission_selected(
        &mut self,
        request_id: &Value,
        option_id: &str,
    ) -> Result<()> {
        let key = request_key(request_id)?;
        let pending = self
            .pending_permissions
            .get(&key)
            .context("Copilot permission request is not pending on this connection")?;
        if !pending.option_ids.contains(option_id) {
            bail!("Copilot permission option `{option_id}` was not offered");
        }
        self.write_message(&json!({
            "jsonrpc": "2.0",
            "id": request_id,
            "result": {"outcome": {"outcome": "selected", "optionId": option_id}}
        }))?;
        self.pending_permissions.remove(&key);
        Ok(())
    }

    pub fn respond_permission_cancelled(&mut self, request_id: &Value) -> Result<()> {
        let key = request_key(request_id)?;
        if !self.pending_permissions.contains_key(&key) {
            bail!("Copilot permission request is not pending on this connection");
        }
        self.write_message(&json!({
            "jsonrpc": "2.0",
            "id": request_id,
            "result": {"outcome": {"outcome": "cancelled"}}
        }))?;
        self.pending_permissions.remove(&key);
        Ok(())
    }

    pub fn receive(&mut self, timeout: Duration) -> Result<CopilotAcpMessage> {
        if let Some(message) = self.queued.pop_front() {
            return Ok(message);
        }
        self.receive_wire(timeout)
    }

    pub fn try_receive(&mut self) -> Result<Option<CopilotAcpMessage>> {
        if let Some(message) = self.queued.pop_front() {
            return Ok(Some(message));
        }
        let value = match self.messages.try_recv() {
            Ok(Ok(value)) => value,
            Ok(Err(error)) => bail!("invalid Copilot ACP output: {error}"),
            Err(mpsc::TryRecvError::Empty) => return Ok(None),
            Err(mpsc::TryRecvError::Disconnected) => {
                let stderr = self.stderr_text();
                if stderr.is_empty() {
                    bail!("Copilot ACP process closed its protocol stream")
                }
                bail!("Copilot ACP process closed its protocol stream: {stderr}")
            }
        };
        self.classify_message(value).map(Some)
    }

    pub fn wait_for_response(&mut self, request_id: u64, timeout: Duration) -> Result<Value> {
        let deadline = Instant::now() + timeout;
        loop {
            let remaining = deadline.saturating_duration_since(Instant::now());
            if remaining.is_zero() {
                bail!(
                    "Copilot ACP request {request_id} timed out after {} ms",
                    timeout.as_millis()
                );
            }
            match self.receive_wire(remaining)? {
                CopilotAcpMessage::Response { id, result } if id.as_u64() == Some(request_id) => {
                    return result.map_err(|error| {
                        anyhow!(
                            "Copilot ACP request {request_id} failed: {}",
                            compact_json(&error)
                        )
                    });
                }
                message => self.queue_message(message)?,
            }
        }
    }

    pub fn reject_unsupported_request(&mut self, request_id: &Value, message: &str) -> Result<()> {
        request_key(request_id)?;
        self.write_message(&json!({
            "jsonrpc": "2.0",
            "id": request_id,
            "error": {"code": -32601, "message": message}
        }))
    }

    fn initialize(&mut self) -> Result<()> {
        let response = self.request_sync(
            "initialize",
            json!({
                "protocolVersion": 1,
                "clientCapabilities": {},
                "clientInfo": {
                    "name": "open-agent-view",
                    "title": "Open Agent View",
                    "version": env!("CARGO_PKG_VERSION")
                }
            }),
            REQUEST_TIMEOUT,
        )?;
        if response.get("protocolVersion").and_then(Value::as_u64) != Some(1) {
            bail!("Copilot ACP server negotiated an unsupported protocol version");
        }
        let agent = response
            .get("agentCapabilities")
            .context("Copilot ACP initialize omitted agentCapabilities")?;
        let session = agent.get("sessionCapabilities");
        self.capabilities = CopilotAcpCapabilities {
            list_sessions: capability_present(session, "list"),
            load_session: agent
                .get("loadSession")
                .and_then(Value::as_bool)
                .unwrap_or(false),
            close_session: capability_present(session, "close"),
            delete_session: capability_present(session, "delete"),
        };
        Ok(())
    }

    fn request_sync(&mut self, method: &str, params: Value, timeout: Duration) -> Result<Value> {
        let request_id = self.send_request(method, params)?;
        let deadline = Instant::now() + timeout;
        loop {
            let remaining = deadline.saturating_duration_since(Instant::now());
            if remaining.is_zero() {
                bail!(
                    "Copilot ACP `{method}` timed out after {} ms",
                    timeout.as_millis()
                );
            }
            match self.receive_wire(remaining)? {
                CopilotAcpMessage::Response { id, result } if id.as_u64() == Some(request_id) => {
                    return result.map_err(|error| {
                        anyhow!("Copilot ACP `{method}` failed: {}", compact_json(&error))
                    });
                }
                message => self.queue_message(message)?,
            }
        }
    }

    fn send_request(&mut self, method: &str, params: Value) -> Result<u64> {
        let id = self.next_id;
        self.next_id = self
            .next_id
            .checked_add(1)
            .context("Copilot ACP request ID overflow")?;
        self.write_message(&json!({
            "jsonrpc": "2.0",
            "id": id,
            "method": method,
            "params": params
        }))?;
        Ok(id)
    }

    fn send_notification(&mut self, method: &str, params: Value) -> Result<()> {
        self.write_message(&json!({
            "jsonrpc": "2.0",
            "method": method,
            "params": params
        }))
    }

    fn write_message(&mut self, message: &Value) -> Result<()> {
        let stdin = self
            .stdin
            .as_mut()
            .context("Copilot ACP connection is closed")?;
        serde_json::to_writer(&mut *stdin, message)?;
        stdin.write_all(b"\n")?;
        stdin.flush()?;
        Ok(())
    }

    fn receive_wire(&mut self, timeout: Duration) -> Result<CopilotAcpMessage> {
        let value = match self.messages.recv_timeout(timeout) {
            Ok(Ok(value)) => value,
            Ok(Err(error)) => bail!("invalid Copilot ACP output: {error}"),
            Err(mpsc::RecvTimeoutError::Timeout) => bail!(
                "Copilot ACP response timed out after {} ms",
                timeout.as_millis()
            ),
            Err(mpsc::RecvTimeoutError::Disconnected) => {
                let stderr = self.stderr_text();
                if stderr.is_empty() {
                    bail!("Copilot ACP process closed its protocol stream")
                }
                bail!("Copilot ACP process closed its protocol stream: {stderr}")
            }
        };
        self.classify_message(value)
    }

    fn classify_message(&mut self, value: Value) -> Result<CopilotAcpMessage> {
        if let Some(id) = value.get("id").cloned() {
            if let Some(method) = value.get("method").and_then(Value::as_str) {
                let params = value.get("params").cloned().unwrap_or(Value::Null);
                if method == "session/request_permission" {
                    let request = parse_permission_request(id, params)?;
                    let key = request_key(&request.request_id)?;
                    let pending = PendingPermission {
                        request_id: request.request_id.clone(),
                        session_id: request.session_id.clone(),
                        option_ids: request
                            .options
                            .iter()
                            .map(|option| option.id.clone())
                            .collect(),
                    };
                    if self.pending_permissions.len() >= MAX_PENDING_PERMISSIONS {
                        bail!(
                            "Copilot ACP exceeded the {MAX_PENDING_PERMISSIONS}-request pending permission limit"
                        );
                    }
                    if self.pending_permissions.insert(key, pending).is_some() {
                        bail!("Copilot ACP reused a pending permission request ID");
                    }
                    return Ok(CopilotAcpMessage::PermissionRequest(request));
                }
                return Ok(CopilotAcpMessage::UnsupportedRequest {
                    id,
                    method: method.to_owned(),
                    params,
                });
            }
            if let Some(result) = value.get("result") {
                return Ok(CopilotAcpMessage::Response {
                    id,
                    result: Ok(result.clone()),
                });
            }
            if let Some(error) = value.get("error") {
                return Ok(CopilotAcpMessage::Response {
                    id,
                    result: Err(error.clone()),
                });
            }
            bail!("Copilot ACP message with an ID was neither request nor response");
        }
        let method = value
            .get("method")
            .and_then(Value::as_str)
            .context("Copilot ACP notification omitted method")?
            .to_owned();
        let params = value.get("params").cloned().unwrap_or(Value::Null);
        if method == "session/update" {
            let session_id = params
                .get("sessionId")
                .and_then(Value::as_str)
                .context("Copilot ACP session/update omitted sessionId")?
                .to_owned();
            let update = params
                .get("update")
                .cloned()
                .context("Copilot ACP session/update omitted update")?;
            return Ok(CopilotAcpMessage::SessionUpdate { session_id, update });
        }
        Ok(CopilotAcpMessage::Notification { method, params })
    }

    fn stderr_text(&self) -> String {
        self.stderr
            .lock()
            .map(|bytes| String::from_utf8_lossy(&bytes).trim().to_owned())
            .unwrap_or_default()
    }

    fn queue_message(&mut self, message: CopilotAcpMessage) -> Result<()> {
        if self.queued.len() >= MAX_QUEUED_MESSAGES {
            bail!("Copilot ACP exceeded the {MAX_QUEUED_MESSAGES}-message deferred queue limit");
        }
        self.queued.push_back(message);
        Ok(())
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct CopilotCommandSpec {
    pub program: String,
    pub args: Vec<String>,
    pub current_dir: PathBuf,
}

impl CopilotCommandSpec {
    pub fn command(&self) -> Command {
        let mut command = Command::new(&self.program);
        command.args(&self.args).current_dir(&self.current_dir);
        command
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct CopilotInvocation {
    executable: String,
}

impl CopilotInvocation {
    pub fn host(executable: impl Into<String>) -> Self {
        Self {
            executable: executable.into(),
        }
    }

    pub fn resume(&self, session_id: &str, cwd: &Path) -> Result<CopilotCommandSpec> {
        require_session_id(session_id)?;
        require_absolute_cwd(cwd)?;
        Ok(CopilotCommandSpec {
            program: self.executable.clone(),
            args: vec![
                format!("--resume={session_id}"),
                "-C".into(),
                cwd.display().to_string(),
            ],
            current_dir: cwd.to_owned(),
        })
    }

    pub fn launch(
        &self,
        session_id: &str,
        cwd: &Path,
        prompt: &str,
        model: Option<&str>,
    ) -> Result<CopilotCommandSpec> {
        require_session_id(session_id)?;
        require_absolute_cwd(cwd)?;
        let prompt = prompt.trim();
        if prompt.is_empty() {
            bail!("Copilot prompt must not be empty");
        }
        let mut args = vec![
            "--session-id".into(),
            session_id.into(),
            "-C".into(),
            cwd.display().to_string(),
        ];
        if let Some(model) = model {
            if model.is_empty()
                || model.len() > 128
                || model
                    .chars()
                    .any(|character| character.is_control() || character.is_whitespace())
            {
                bail!("invalid Copilot model ID");
            }
            args.extend(["--model".into(), model.into()]);
        }
        args.extend(["--interactive".into(), prompt.into()]);
        Ok(CopilotCommandSpec {
            program: self.executable.clone(),
            args,
            current_dir: cwd.to_owned(),
        })
    }
}

/// Native-open controller for external persisted Copilot sessions.
///
/// Inline authority is intentionally left to an exact `CopilotAcpConnection`;
/// it is never inferred merely because ACP session/list returned a record.
pub struct CopilotController {
    invocation: CopilotInvocation,
    supervisor: Option<Arc<CopilotSupervisor>>,
}

impl CopilotController {
    pub fn host(executable: impl Into<String>) -> Self {
        Self {
            invocation: CopilotInvocation::host(executable),
            supervisor: None,
        }
    }

    pub fn managed(supervisor: Arc<CopilotSupervisor>) -> Self {
        Self {
            invocation: CopilotInvocation::host(supervisor.executable()),
            supervisor: Some(supervisor),
        }
    }
}

impl ProviderController for CopilotController {
    fn provider(&self) -> Provider {
        Provider::GitHubCopilot
    }

    fn launch_mode(&self) -> LaunchMode {
        if self.supervisor.is_some() {
            LaunchMode::SelectableModel
        } else {
            LaunchMode::Unavailable
        }
    }

    fn launch_presentation(&self) -> LaunchPresentation {
        LaunchPresentation::Foreground
    }

    fn available_models(&self) -> Result<Vec<String>> {
        list_copilot_models(&self.invocation.executable)
    }

    fn supports_authentication(&self) -> bool {
        true
    }

    fn authenticate(&self) -> Result<ControlOutcome> {
        let outcome = run_native_authentication(
            &self.invocation.executable,
            &["login"],
            Provider::GitHubCopilot,
        )?;
        if let Some(supervisor) = &self.supervisor {
            supervisor.reset_unowned_connection_after_authentication()?;
        }
        Ok(outcome)
    }

    fn enrich(&self, snapshot: &mut SessionSnapshot) {
        if let Some(supervisor) = &self.supervisor {
            supervisor.enrich(snapshot);
        }
    }

    fn launch(&self, request: &LaunchRequest) -> Result<ControlOutcome> {
        if request.provider != Provider::GitHubCopilot {
            bail!("the GitHub Copilot controller cannot launch another provider");
        }
        let session_id = self.managed_supervisor()?.launch_with_model(
            &request.prompt,
            &request.cwd,
            request.model.as_deref(),
        )?;
        Ok(ControlOutcome {
            message: format!("launched managed GitHub Copilot session {session_id}"),
            provider_session_hint: Some(session_id),
        })
    }

    fn launch_foreground(&self, request: &LaunchRequest) -> Result<ControlOutcome> {
        if request.provider != Provider::GitHubCopilot {
            bail!("the GitHub Copilot controller cannot launch another provider");
        }
        let supervisor = self.managed_supervisor()?;
        let session_id = supervisor.reserve_native(&request.prompt, &request.cwd)?;
        let spec = match self.invocation.launch(
            &session_id,
            &request.cwd,
            &request.prompt,
            request.model.as_deref(),
        ) {
            Ok(spec) => spec,
            Err(error) => {
                supervisor.discard_native_reservation(&session_id)?;
                return Err(error);
            }
        };
        let key = format!("github_copilot:host:{session_id}");
        let native_result = crate::native_session::run(spec.command(), &key);
        match native_result {
            Err(error) => {
                supervisor.discard_native_reservation(&session_id)?;
                Err(error)
            }
            Ok(crate::native_session::NativeSessionExit::Backgrounded) => {
                supervisor.mark_native_backgrounded(&session_id)?;
                Ok(ControlOutcome {
                    message: format!(
                        "backgrounded GitHub Copilot session {}; Enter/Right resumes it",
                        &session_id[..8]
                    ),
                    provider_session_hint: Some(session_id),
                })
            }
            Ok(crate::native_session::NativeSessionExit::Exited(status)) if status.success() => {
                supervisor.mark_native_exited(&session_id, true)?;
                Ok(ControlOutcome {
                    message: format!("returned from GitHub Copilot session {}", &session_id[..8]),
                    provider_session_hint: Some(session_id),
                })
            }
            Ok(crate::native_session::NativeSessionExit::Exited(status)) => {
                supervisor.mark_native_exited(&session_id, false)?;
                bail!("GitHub Copilot session exited with status {status}")
            }
        }
    }

    fn interrupt(&self, session: &AgentSession) -> Result<ControlOutcome> {
        if crate::native_session::is_backgrounded(&session.id) {
            crate::native_session::terminate(&session.id)?;
            self.managed_supervisor()?
                .mark_native_exited(&session.provider_session_id, true)?;
            return Ok(ControlOutcome {
                message: format!("stopped native GitHub Copilot session {}", session.name),
                provider_session_hint: Some(session.provider_session_id.clone()),
            });
        }
        self.managed_supervisor()?.interrupt(session)?;
        Ok(ControlOutcome {
            message: format!("cancelled {}", session.name),
            provider_session_hint: None,
        })
    }

    fn inspect(&self, session: &AgentSession) -> Result<String> {
        self.managed_supervisor()?.inspect(session)
    }

    fn reply(&self, session: &AgentSession, prompt: &str) -> Result<ControlOutcome> {
        self.managed_supervisor()?.reply(session, prompt)?;
        Ok(ControlOutcome {
            message: format!("sent a prompt to {}", session.name),
            provider_session_hint: None,
        })
    }

    fn resolve_approval(&self, session: &AgentSession, accept: bool) -> Result<ControlOutcome> {
        self.managed_supervisor()?
            .resolve_approval(session, accept)?;
        Ok(ControlOutcome {
            message: format!(
                "{} the pending request for {}",
                if accept { "allowed once" } else { "rejected" },
                session.name
            ),
            provider_session_hint: None,
        })
    }

    fn open(&self, session: &AgentSession) -> Result<ControlOutcome> {
        if session.provider != Provider::GitHubCopilot || session.runtime != Runtime::Host {
            bail!("the GitHub Copilot host controller cannot open this session");
        }
        let tracked = self
            .supervisor
            .as_ref()
            .map(|supervisor| supervisor.tracks(session))
            .unwrap_or(false);
        if let Some(supervisor) = &self.supervisor {
            if supervisor.owns(session) {
                supervisor.release_for_native(session)?;
            }
        }
        let spec = self
            .invocation
            .resume(&session.provider_session_id, &session.cwd)?;
        let native_result = crate::native_session::run(spec.command(), &session.id);
        let reclaim = |supervisor: &Arc<CopilotSupervisor>| {
            supervisor
                .load(session)
                .context("could not return the Copilot session to inline control")
        };
        match native_result {
            Err(error) => {
                if tracked {
                    if let Some(supervisor) = &self.supervisor {
                        if let Err(reclaim_error) = reclaim(supervisor) {
                            return Err(error)
                                .context(format!("native Copilot open failed; {reclaim_error:#}"));
                        }
                    }
                }
                Err(error)
            }
            Ok(crate::native_session::NativeSessionExit::Backgrounded) => Ok(ControlOutcome {
                message: format!("backgrounded {}; Enter/Right resumes it", session.name),
                provider_session_hint: Some(session.provider_session_id.clone()),
            }),
            Ok(crate::native_session::NativeSessionExit::Exited(status)) if status.success() => {
                if tracked {
                    if let Some(supervisor) = &self.supervisor {
                        reclaim(supervisor)?;
                    }
                }
                Ok(ControlOutcome {
                    message: format!("returned from {}", session.name),
                    provider_session_hint: None,
                })
            }
            Ok(crate::native_session::NativeSessionExit::Exited(status)) => {
                if tracked {
                    if let Some(supervisor) = &self.supervisor {
                        let _ = reclaim(supervisor);
                    }
                }
                bail!("GitHub Copilot session exited with status {status}")
            }
        }
    }
}

impl CopilotController {
    fn managed_supervisor(&self) -> Result<&CopilotSupervisor> {
        self.supervisor
            .as_deref()
            .context("managed GitHub Copilot ACP control is not configured")
    }
}

fn list_copilot_models(executable: &str) -> Result<Vec<String>> {
    let mut command = Command::new(executable);
    command
        .args(["--headless", "--no-auto-update", "--stdio"])
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::null());
    let mut attempts = 0;
    let mut child = loop {
        match command.spawn() {
            Ok(child) => break child,
            Err(error)
                if error.raw_os_error() == Some(libc::ETXTBSY)
                    && attempts < EXECUTABLE_BUSY_RETRIES =>
            {
                attempts += 1;
                thread::sleep(Duration::from_millis(10));
            }
            Err(error) => {
                return Err(error).with_context(|| {
                    format!("failed to start Copilot model catalog from `{executable}`")
                })
            }
        }
    };
    let mut stdin = child
        .stdin
        .take()
        .context("Copilot headless stdin is unavailable")?;
    let stdout = child
        .stdout
        .take()
        .context("Copilot headless stdout is unavailable")?;
    let (sender, receiver) = mpsc::sync_channel(8);
    let reader = thread::spawn(move || {
        let mut reader = BufReader::new(stdout);
        loop {
            match read_lsp_message(&mut reader) {
                Ok(Some(value)) => {
                    if sender.send(Ok(value)).is_err() {
                        break;
                    }
                }
                Ok(None) => break,
                Err(error) => {
                    let _ = sender.send(Err(format!("{error:#}")));
                    break;
                }
            }
        }
    });
    let operation = (|| -> Result<Vec<String>> {
        write_lsp_message(
            &mut stdin,
            &json!({"jsonrpc":"2.0","id":1,"method":"connect","params":{}}),
        )?;
        wait_lsp_response(&receiver, 1).map_err(map_copilot_catalog_error)?;
        write_lsp_message(
            &mut stdin,
            &json!({"jsonrpc":"2.0","id":2,"method":"models.list","params":{}}),
        )?;
        let result = wait_lsp_response(&receiver, 2).map_err(map_copilot_catalog_error)?;
        let models = result
            .get("models")
            .and_then(Value::as_array)
            .context("Copilot models.list omitted models")?;
        let mut ids = BTreeSet::new();
        for model in models {
            let Some(id) = model.get("id").and_then(Value::as_str) else {
                continue;
            };
            if !id.is_empty()
                && id.len() <= 128
                && !id
                    .chars()
                    .any(|character| character.is_control() || character.is_whitespace())
            {
                ids.insert(id.to_owned());
            }
        }
        if ids.is_empty() {
            bail!("Copilot returned no models for the authenticated account");
        }
        Ok(ids.into_iter().collect())
    })();
    drop(stdin);
    let _ = child.kill();
    let _ = child.wait();
    let _ = reader.join();
    operation
}

fn map_copilot_catalog_error(error: anyhow::Error) -> anyhow::Error {
    if format!("{error:#}").to_ascii_lowercase().contains("auth") {
        anyhow!("GitHub Copilot is not authenticated; press Enter to sign in")
    } else {
        error
    }
}

fn write_lsp_message(writer: &mut impl Write, value: &Value) -> Result<()> {
    let bytes = serde_json::to_vec(value)?;
    write!(writer, "Content-Length: {}\r\n\r\n", bytes.len())?;
    writer.write_all(&bytes)?;
    writer.flush()?;
    Ok(())
}

fn read_lsp_message(reader: &mut impl BufRead) -> Result<Option<Value>> {
    let mut content_length = None;
    loop {
        let mut line = String::new();
        if reader.read_line(&mut line)? == 0 {
            return Ok(None);
        }
        if line == "\r\n" || line == "\n" {
            break;
        }
        if let Some(value) = line
            .strip_prefix("Content-Length:")
            .or_else(|| line.strip_prefix("content-length:"))
        {
            content_length = Some(
                value
                    .trim()
                    .parse::<usize>()
                    .context("invalid Copilot Content-Length header")?,
            );
        }
    }
    let length = content_length.context("Copilot headless frame omitted Content-Length")?;
    if length > MAX_MESSAGE_BYTES as usize {
        bail!("Copilot headless frame exceeded the message size limit");
    }
    let mut bytes = vec![0; length];
    reader.read_exact(&mut bytes)?;
    Ok(Some(
        serde_json::from_slice(&bytes).context("invalid Copilot headless JSON-RPC frame")?,
    ))
}

fn wait_lsp_response(
    receiver: &mpsc::Receiver<std::result::Result<Value, String>>,
    id: u64,
) -> Result<Value> {
    let deadline = Instant::now() + REQUEST_TIMEOUT;
    loop {
        let value = receiver
            .recv_timeout(deadline.saturating_duration_since(Instant::now()))
            .map_err(|error| anyhow!("Copilot model catalog response failed: {error}"))?
            .map_err(|error| anyhow!(error))?;
        if value.get("id").and_then(Value::as_u64) != Some(id) {
            continue;
        }
        if let Some(error) = value.get("error") {
            bail!(
                "Copilot model catalog request failed: {}",
                compact_json(error)
            );
        }
        return value
            .get("result")
            .cloned()
            .context("Copilot model catalog response omitted result");
    }
}

impl Drop for CopilotAcpConnection {
    fn drop(&mut self) {
        self.stdin.take();
        let deadline = Instant::now() + Duration::from_millis(500);
        loop {
            match self.child.try_wait() {
                Ok(Some(_)) => break,
                Ok(None) if Instant::now() < deadline => {
                    thread::sleep(Duration::from_millis(10));
                }
                Ok(None) | Err(_) => {
                    let _ = self.child.kill();
                    let _ = self.child.wait();
                    break;
                }
            }
        }
        if let Some(reader) = self.stdout_thread.take() {
            let _ = reader.join();
        }
        if let Some(reader) = self.stderr_thread.take() {
            let _ = reader.join();
        }
    }
}

pub struct CopilotSource {
    executable: String,
    connection: Mutex<Option<CopilotAcpConnection>>,
}

impl CopilotSource {
    pub fn host(executable: impl Into<String>) -> Self {
        Self {
            executable: executable.into(),
            connection: Mutex::new(None),
        }
    }
}

impl SessionSource for CopilotSource {
    fn label(&self) -> &str {
        "GitHub Copilot (host)"
    }

    fn discover(&self, request: &DiscoveryRequest) -> Result<Vec<AgentSession>> {
        if !request.include_external {
            return Ok(Vec::new());
        }
        let mut connection = self
            .connection
            .lock()
            .map_err(|_| anyhow!("Copilot ACP connection lock was poisoned"))?;
        if connection.is_none() {
            *connection = Some(CopilotAcpConnection::connect(
                &self.executable,
                CopilotAcpMode::Discovery,
            )?);
        }
        let result = connection
            .as_mut()
            .expect("connection initialized")
            .list_sessions();
        let sessions = match result {
            Ok(sessions) => sessions,
            Err(error) => {
                *connection = None;
                return Err(error);
            }
        };
        Ok(normalize_copilot_sessions(sessions, Runtime::Host)
            .into_iter()
            .filter(|session| {
                request
                    .cwd
                    .as_ref()
                    .map(|cwd| session.cwd.starts_with(cwd))
                    .unwrap_or(true)
            })
            .collect())
    }
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct CopilotSessionInfo {
    pub session_id: String,
    pub cwd: PathBuf,
    pub title: Option<String>,
    pub updated_at: Option<String>,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct CopilotSessionPage {
    pub sessions: Vec<CopilotSessionInfo>,
    pub next_cursor: Option<String>,
}

pub fn parse_copilot_session_page(value: &Value) -> Result<CopilotSessionPage> {
    serde_json::from_value(value.clone()).context("invalid Copilot ACP session/list response")
}

pub fn normalize_copilot_sessions(
    sessions: Vec<CopilotSessionInfo>,
    runtime: Runtime,
) -> Vec<AgentSession> {
    sessions
        .into_iter()
        .map(|session| normalize_copilot_session(session, runtime.clone()))
        .collect()
}

fn normalize_copilot_session(session: CopilotSessionInfo, runtime: Runtime) -> AgentSession {
    let runtime_id = match &runtime {
        Runtime::Host => "host",
        Runtime::Docker { container_id, .. } => container_id,
    };
    let fallback_name = session
        .cwd
        .file_name()
        .and_then(|name| name.to_str())
        .filter(|name| !name.is_empty())
        .map(ToOwned::to_owned)
        .unwrap_or_else(|| format!("copilot-{}", short_id(&session.session_id)));
    let name = session
        .title
        .filter(|title| !title.is_empty())
        .unwrap_or(fallback_name);
    AgentSession {
        id: format!("github_copilot:{runtime_id}:{}", session.session_id),
        provider_session_id: session.session_id,
        provider: Provider::GitHubCopilot,
        runtime,
        kind: SessionKind::Unknown,
        name,
        cwd: session.cwd,
        // ACP session/list is persisted-history metadata, not a live state API.
        state: SessionState::Unknown,
        summary: "Persisted Copilot session".into(),
        raw_state: Some("persisted".into()),
        pid: None,
        started_at: None,
        updated_at: session.updated_at.as_deref().and_then(parse_rfc3339),
        pull_requests: None,
        // A controller grants capabilities only after it owns an ACP process
        // and has loaded this exact session on that connection.
        capabilities: BTreeSet::new(),
    }
}

fn parse_permission_request(id: Value, params: Value) -> Result<CopilotPermissionRequest> {
    #[derive(Deserialize)]
    #[serde(rename_all = "camelCase")]
    struct Params {
        session_id: String,
        options: Vec<OptionRecord>,
    }
    #[derive(Deserialize)]
    #[serde(rename_all = "camelCase")]
    struct OptionRecord {
        option_id: String,
        name: String,
        kind: String,
    }

    request_key(&id)?;
    let parsed: Params =
        serde_json::from_value(params.clone()).context("invalid Copilot permission request")?;
    require_session_id(&parsed.session_id)?;
    if parsed.options.is_empty() {
        bail!("Copilot permission request did not offer any options");
    }
    let mut ids = BTreeSet::new();
    let mut options = Vec::with_capacity(parsed.options.len());
    for option in parsed.options {
        if option.option_id.is_empty() || !ids.insert(option.option_id.clone()) {
            bail!("Copilot permission request contained an empty or duplicate option ID");
        }
        options.push(CopilotPermissionOption {
            id: option.option_id,
            name: option.name,
            kind: option.kind,
        });
    }
    Ok(CopilotPermissionRequest {
        request_id: id,
        session_id: parsed.session_id,
        options,
        raw_params: params,
    })
}

fn read_ndjson(stdout: impl Read, sender: mpsc::SyncSender<std::result::Result<Value, String>>) {
    let mut reader = BufReader::new(stdout);
    loop {
        let mut bytes = Vec::new();
        let result = reader
            .by_ref()
            .take(MAX_MESSAGE_BYTES + 1)
            .read_until(b'\n', &mut bytes);
        match result {
            Ok(0) => break,
            Ok(_) if bytes.len() as u64 > MAX_MESSAGE_BYTES => {
                let _ = sender.send(Err(format!(
                    "message exceeded the {MAX_MESSAGE_BYTES}-byte limit"
                )));
                break;
            }
            Ok(_) => {
                while matches!(bytes.last(), Some(b'\n' | b'\r')) {
                    bytes.pop();
                }
                if bytes.is_empty() {
                    continue;
                }
                let value = serde_json::from_slice(&bytes).map_err(|error| error.to_string());
                if sender.send(value).is_err() {
                    break;
                }
            }
            Err(error) => {
                let _ = sender.send(Err(error.to_string()));
                break;
            }
        }
    }
}

fn capability_present(session: Option<&Value>, name: &str) -> bool {
    session
        .and_then(|session| session.get(name))
        .map(Value::is_object)
        .unwrap_or(false)
}

fn require_absolute_cwd(cwd: &Path) -> Result<()> {
    if !cwd.is_absolute() {
        bail!("Copilot ACP working directory must be absolute");
    }
    Ok(())
}

fn require_session_id(session_id: &str) -> Result<()> {
    if session_id.is_empty() || session_id.chars().any(char::is_control) {
        bail!("Copilot session ID is empty or contains control characters");
    }
    Ok(())
}

fn request_key(id: &Value) -> Result<String> {
    match id {
        Value::String(value) => Ok(format!("s:{value}")),
        Value::Number(value) => Ok(format!("n:{value}")),
        _ => bail!("Copilot ACP request ID must be a string or number"),
    }
}

fn compact_json(value: &Value) -> String {
    let encoded = value.to_string();
    if encoded.chars().count() <= 300 {
        encoded
    } else {
        let mut truncated = encoded.chars().take(299).collect::<String>();
        truncated.push('…');
        truncated
    }
}

fn short_id(id: &str) -> String {
    id.chars().take(8).collect()
}

fn parse_rfc3339(value: &str) -> Option<SystemTime> {
    let (date, rest) = value.split_once('T')?;
    let mut date_parts = date.split('-');
    let year: i64 = date_parts.next()?.parse().ok()?;
    let month: i64 = date_parts.next()?.parse().ok()?;
    let day: i64 = date_parts.next()?.parse().ok()?;
    if date_parts.next().is_some() || !(1..=12).contains(&month) || !(1..=31).contains(&day) {
        return None;
    }
    let (time, offset_seconds) = if let Some(time) = rest.strip_suffix('Z') {
        (time, 0_i64)
    } else {
        let position = rest.rfind(['+', '-'])?;
        let sign = if rest.as_bytes().get(position) == Some(&b'+') {
            1
        } else {
            -1
        };
        let offset = &rest[position + 1..];
        let (hours, minutes) = offset.split_once(':')?;
        let hours: i64 = hours.parse().ok()?;
        let minutes: i64 = minutes.parse().ok()?;
        if hours > 23 || minutes > 59 {
            return None;
        }
        (&rest[..position], sign * (hours * 3600 + minutes * 60))
    };
    let mut time_parts = time.split(':');
    let hour: i64 = time_parts.next()?.parse().ok()?;
    let minute: i64 = time_parts.next()?.parse().ok()?;
    let seconds = time_parts.next()?;
    if time_parts.next().is_some() || hour > 23 || minute > 59 {
        return None;
    }
    let (second, fraction) = seconds.split_once('.').unwrap_or((seconds, ""));
    let second: i64 = second.parse().ok()?;
    if second > 59 || !fraction.chars().all(|character| character.is_ascii_digit()) {
        return None;
    }
    let mut nanos = fraction.chars().take(9).collect::<String>();
    while nanos.len() < 9 {
        nanos.push('0');
    }
    let nanos: u32 = if nanos.is_empty() {
        0
    } else {
        nanos.parse().ok()?
    };
    let seconds = days_from_civil(year, month, day)
        .checked_mul(86_400)?
        .checked_add(hour * 3600 + minute * 60 + second)?
        .checked_sub(offset_seconds)?;
    let seconds = u64::try_from(seconds).ok()?;
    Some(SystemTime::UNIX_EPOCH + Duration::new(seconds, nanos))
}

fn days_from_civil(year: i64, month: i64, day: i64) -> i64 {
    let year = year - i64::from(month <= 2);
    let era = if year >= 0 { year } else { year - 399 } / 400;
    let year_of_era = year - era * 400;
    let shifted_month = month + if month > 2 { -3 } else { 9 };
    let day_of_year = (153 * shifted_month + 2) / 5 + day - 1;
    let day_of_era = year_of_era * 365 + year_of_era / 4 - year_of_era / 100 + day_of_year;
    era * 146_097 + day_of_era - 719_468
}

#[cfg(test)]
mod tests {
    use std::fs;
    use std::os::unix::fs::PermissionsExt;

    use tempfile::tempdir;

    use super::*;

    #[test]
    fn parses_and_normalizes_acp_session_page() {
        let page = parse_copilot_session_page(&json!({
            "sessions": [{
                "sessionId": "sess_abc123",
                "cwd": "/work/project",
                "title": "Fix the parser",
                "updatedAt": "2025-10-29T14:22:15.125Z",
                "_meta": {"messageCount": 12}
            }],
            "nextCursor": "page-2"
        }))
        .unwrap();
        assert_eq!(page.next_cursor.as_deref(), Some("page-2"));

        let sessions = normalize_copilot_sessions(page.sessions, Runtime::Host);
        assert_eq!(sessions[0].provider, Provider::GitHubCopilot);
        assert_eq!(sessions[0].state, SessionState::Unknown);
        assert!(sessions[0].capabilities.is_empty());
        assert_eq!(
            sessions[0].updated_at,
            Some(SystemTime::UNIX_EPOCH + Duration::new(1_761_747_735, 125_000_000))
        );
    }

    #[test]
    fn parses_timezone_offsets() {
        assert_eq!(
            parse_rfc3339("1970-01-01T01:00:00+01:00"),
            Some(SystemTime::UNIX_EPOCH)
        );
    }

    #[test]
    fn builds_shell_free_native_resume_without_permission_expansion() {
        let invocation = CopilotInvocation::host("copilot");
        let spec = invocation
            .resume("session-id", Path::new("/work/project"))
            .unwrap();
        assert_eq!(
            spec,
            CopilotCommandSpec {
                program: "copilot".into(),
                args: vec![
                    "--resume=session-id".into(),
                    "-C".into(),
                    "/work/project".into()
                ],
                current_dir: "/work/project".into(),
            }
        );
        assert!(!spec
            .args
            .iter()
            .any(|arg| matches!(arg.as_str(), "--allow-all" | "--allow-all-tools" | "--yolo")));
    }

    #[test]
    fn foreground_launch_uses_exact_native_identity_prompt_and_model() {
        let directory = tempdir().unwrap();
        let executable = directory.path().join("copilot-native-mock");
        let argv = directory.path().join("argv");
        fs::write(
            &executable,
            format!("#!/bin/sh\nprintf '%s\\n' \"$@\" > '{}'\n", argv.display()),
        )
        .unwrap();
        let mut permissions = fs::metadata(&executable).unwrap().permissions();
        permissions.set_mode(0o700);
        fs::set_permissions(&executable, permissions).unwrap();
        let workspace = directory.path().join("workspace");
        fs::create_dir(&workspace).unwrap();
        let supervisor = Arc::new(CopilotSupervisor::host(executable.display().to_string()));
        let controller = CopilotController::managed(supervisor);
        let request = LaunchRequest {
            provider: Provider::GitHubCopilot,
            model: Some("gpt-5.4".into()),
            prompt: "show this in front".into(),
            cwd: workspace.clone(),
        };

        assert_eq!(
            controller.launch_presentation(),
            LaunchPresentation::Foreground
        );
        let outcome = controller.launch_foreground(&request).unwrap();
        let session_id = outcome.provider_session_hint.unwrap();
        let args = fs::read_to_string(argv).unwrap();
        assert_eq!(
            args,
            format!(
                "--session-id\n{session_id}\n-C\n{}\n--model\ngpt-5.4\n--interactive\nshow this in front\n",
                workspace.display()
            )
        );
        assert!(!args.contains("--allow-all"));
        assert!(!args.contains("--yolo"));
    }

    #[test]
    fn headless_catalog_uses_account_scoped_model_ids_without_creating_a_session() {
        let directory = tempdir().unwrap();
        let script = directory.path().join("copilot-models-mock");
        fs::write(
            &script,
            r##"#!/usr/bin/env python3
import json, sys
def receive():
    length = None
    while True:
        line = sys.stdin.buffer.readline()
        if not line: raise SystemExit(0)
        if line in (b'\n', b'\r\n'): break
        if line.lower().startswith(b'content-length:'):
            length = int(line.split(b':', 1)[1])
    return json.loads(sys.stdin.buffer.read(length))
def send(value):
    body = json.dumps(value, separators=(',', ':')).encode()
    sys.stdout.buffer.write(b'Content-Length: %d\r\n\r\n' % len(body) + body)
    sys.stdout.buffer.flush()
request = receive()
assert request['method'] == 'connect'
send({'jsonrpc':'2.0','id':request['id'],'result':{'ok':True,'protocolVersion':3,'version':'test'}})
request = receive()
assert request['method'] == 'models.list'
send({'jsonrpc':'2.0','id':request['id'],'result':{'models':[
  {'id':'gpt-5.4','name':'GPT 5.4'}, {'id':'claude-sonnet-4.6','name':'Sonnet'}
]}})
"##,
        )
        .unwrap();
        let mut permissions = fs::metadata(&script).unwrap().permissions();
        permissions.set_mode(0o700);
        fs::set_permissions(&script, permissions).unwrap();

        assert_eq!(
            list_copilot_models(script.to_str().unwrap()).unwrap(),
            vec!["claude-sonnet-4.6", "gpt-5.4"]
        );
    }

    #[test]
    fn rejects_malformed_pages_and_timestamps() {
        assert!(parse_copilot_session_page(&json!({"sessions": "nope"})).is_err());
        assert_eq!(parse_rfc3339("not-a-time"), None);
        assert_eq!(parse_rfc3339("2025-99-99T99:99:99Z"), None);
    }

    #[test]
    fn isolated_mock_acp_lists_pages_and_preserves_unknown_state() {
        let directory = tempdir().unwrap();
        let script = directory.path().join("copilot-mock");
        fs::write(
            &script,
            r##"#!/bin/sh
read first
printf '%s\n' '{"jsonrpc":"2.0","id":1,"result":{"protocolVersion":1,"agentCapabilities":{"loadSession":true,"sessionCapabilities":{"list":{},"close":{}}}}}'
read second
printf '%s\n' '{"jsonrpc":"2.0","id":2,"result":{"sessions":[{"sessionId":"one","cwd":"/work/a","title":"one"}],"nextCursor":"next"}}'
read third
printf '%s\n' '{"jsonrpc":"2.0","id":3,"result":{"sessions":[{"sessionId":"two","cwd":"/work/b"}]}}'
"##,
        )
        .unwrap();
        let mut permissions = fs::metadata(&script).unwrap().permissions();
        permissions.set_mode(0o700);
        fs::set_permissions(&script, permissions).unwrap();

        let mut connection =
            CopilotAcpConnection::connect(script.to_str().unwrap(), CopilotAcpMode::Discovery)
                .unwrap();
        assert_eq!(
            connection.capabilities(),
            &CopilotAcpCapabilities {
                list_sessions: true,
                load_session: true,
                close_session: true,
                delete_session: false,
            }
        );
        let sessions = connection.list_sessions().unwrap();
        assert_eq!(sessions.len(), 2);
        assert_eq!(sessions[1].session_id, "two");
    }

    #[test]
    fn permission_choices_are_exact_and_cancel_clears_pending_requests() {
        let directory = tempdir().unwrap();
        let script = directory.path().join("copilot-mock");
        fs::write(
            &script,
            r##"#!/bin/sh
read first
printf '%s\n' '{"jsonrpc":"2.0","id":1,"result":{"protocolVersion":1,"agentCapabilities":{"loadSession":true,"sessionCapabilities":{"list":{},"close":{}}}}}'
read prompt
printf '%s\n' '{"jsonrpc":"2.0","id":"permission-1","method":"session/request_permission","params":{"sessionId":"session-1","toolCall":{"toolCallId":"tool-1"},"options":[{"optionId":"allow-once","name":"Allow once","kind":"allow_once"},{"optionId":"reject-once","name":"Reject","kind":"reject_once"}]}}'
read decision
printf '%s\n' "$decision"
read cancel
printf '%s\n' "$cancel"
"##,
        )
        .unwrap();
        let mut permissions = fs::metadata(&script).unwrap().permissions();
        permissions.set_mode(0o700);
        fs::set_permissions(&script, permissions).unwrap();

        let mut connection =
            CopilotAcpConnection::connect(script.to_str().unwrap(), CopilotAcpMode::Control)
                .unwrap();
        connection.begin_prompt("session-1", "test").unwrap();
        let request = match connection.receive(Duration::from_secs(2)).unwrap() {
            CopilotAcpMessage::PermissionRequest(request) => request,
            message => panic!("unexpected message: {message:?}"),
        };
        assert!(connection
            .respond_permission_selected(&request.request_id, "not-offered")
            .is_err());
        connection
            .respond_permission_selected(&request.request_id, "reject-once")
            .unwrap();
        let echoed = connection.receive(Duration::from_secs(2)).unwrap();
        match echoed {
            CopilotAcpMessage::Notification { method, params } => {
                assert_eq!(method, "echo");
                assert_eq!(params, Value::Null);
            }
            CopilotAcpMessage::UnsupportedRequest { .. }
            | CopilotAcpMessage::PermissionRequest(_)
            | CopilotAcpMessage::Response { .. }
            | CopilotAcpMessage::SessionUpdate { .. } => {}
        }
        // The mock echoes JSON without a method; receiving it is expected to
        // fail, but only after the selected option was serialized exactly.
        assert!(connection.cancel_session("session-1").is_ok());
    }
}

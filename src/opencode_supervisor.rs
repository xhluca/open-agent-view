//! Durable ownership and HTTP control for OAV-managed OpenCode sessions.
//!
//! OpenCode's documented server is reconnectable, so the dashboard persists an
//! authenticated loopback endpoint plus exact Linux process identity and the
//! canonical session IDs it created. Unrelated history never enters this
//! ownership record.

use std::collections::BTreeMap;
#[cfg(target_os = "linux")]
use std::collections::BTreeSet;
#[cfg(target_os = "linux")]
use std::fmt::Write as FmtWrite;
use std::fs::{self, File, OpenOptions};
use std::io::{Read, Write};
#[cfg(target_os = "linux")]
use std::net::TcpListener;
use std::net::{Ipv4Addr, SocketAddrV4, TcpStream};
use std::path::{Path, PathBuf};
use std::process::Command;
#[cfg(target_os = "linux")]
use std::process::Stdio;
#[cfg(target_os = "linux")]
use std::thread;
#[cfg(target_os = "linux")]
use std::time::Instant;
use std::time::{Duration, SystemTime};

use anyhow::{bail, Context, Result};
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};

use crate::domain::SessionState;

const RECORD_VERSION: u32 = 1;
#[cfg(target_os = "linux")]
const STARTUP_TIMEOUT: Duration = Duration::from_secs(10);
const HTTP_TIMEOUT: Duration = Duration::from_secs(8);
const MAX_HEADER_BYTES: usize = 64 * 1024;
const MAX_BODY_BYTES: usize = 2 * 1024 * 1024;

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
struct ServerRecord {
    version: u32,
    pid: u32,
    process_start_token: String,
    process_cmdline: Vec<u8>,
    executable: String,
    port: u16,
    username: String,
    password: String,
    created_at_ms: u64,
    #[serde(default)]
    sessions: BTreeMap<String, OwnedSession>,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
struct OwnedSession {
    id: String,
    cwd: PathBuf,
    title: String,
    summary: String,
    created_at_ms: u64,
    updated_at_ms: u64,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ManagedOpenCodeSession {
    pub id: String,
    pub cwd: PathBuf,
    pub title: String,
    pub summary: String,
    pub state: SessionState,
    pub server_pid: u32,
    pub created_at_ms: u64,
    pub updated_at_ms: u64,
}

/// Reconnectable controller for one OAV-owned authenticated OpenCode server.
pub struct OpenCodeSupervisor {
    executable: String,
    state_dir: PathBuf,
    record_path: PathBuf,
    lock_path: PathBuf,
}

impl OpenCodeSupervisor {
    pub fn host(executable: impl Into<String>) -> Result<Self> {
        Self::with_state_dir(executable, default_state_dir()?)
    }

    pub fn with_state_dir(executable: impl Into<String>, state_dir: PathBuf) -> Result<Self> {
        ensure_private_directory(&state_dir)?;
        Ok(Self {
            executable: executable.into(),
            record_path: state_dir.join("server.json"),
            lock_path: state_dir.join("server.lock"),
            state_dir,
        })
    }

    pub fn launch(&self, prompt: &str, cwd: &Path) -> Result<ManagedOpenCodeSession> {
        self.launch_with_model(prompt, cwd, None)
    }

    pub fn launch_with_model(
        &self,
        prompt: &str,
        cwd: &Path,
        model: Option<&str>,
    ) -> Result<ManagedOpenCodeSession> {
        let prompt = prompt.trim();
        if prompt.is_empty() {
            bail!("the OpenCode launch prompt cannot be empty");
        }
        validate_cwd(cwd)?;
        // Validate the complete turn body before creating a provider session.
        // A malformed selector must never leave an empty owned session behind.
        let prompt_body = opencode_prompt_body(prompt, model)?;
        let _lock = StateLock::acquire(&self.lock_path)?;
        let mut record = self.ensure_server_locked()?;
        let title = prompt
            .split_whitespace()
            .take(8)
            .collect::<Vec<_>>()
            .join(" ");
        let path = with_directory_query("/session", cwd);
        let response = self.request_json(&record, "POST", &path, Some(&json!({"title": title})))?;
        let id = response
            .get("id")
            .and_then(Value::as_str)
            .context("OpenCode session creation omitted its canonical ID")?
            .to_owned();
        if record.sessions.contains_key(&id) {
            bail!("OpenCode returned a duplicate managed session ID");
        }
        let now = now_millis();
        record.sessions.insert(
            id.clone(),
            OwnedSession {
                id: id.clone(),
                cwd: cwd.into(),
                title,
                summary: prompt.into(),
                created_at_ms: response
                    .pointer("/time/created")
                    .and_then(Value::as_u64)
                    .unwrap_or(now),
                updated_at_ms: now,
            },
        );
        // Persist ownership before starting the turn. A 2xx async acceptance
        // cannot prove that a provider/model will eventually succeed, but it
        // does prove which exact session this server created.
        save_record(&self.record_path, &record)?;
        let path = with_directory_query(&format!("/session/{id}/prompt_async"), cwd);
        if let Err(error) = self.request_empty(&record, "POST", &path, Some(&prompt_body)) {
            return Err(error).context(format!(
                "OpenCode created owned session {id}, but rejected its initial prompt"
            ));
        }
        self.session_snapshot(&record, record.sessions.get(&id).expect("inserted session"))
    }

    /// List only sessions backed by an already-running, exactly verified
    /// server. Read-only discovery never starts a server as a side effect.
    pub fn list(&self) -> Result<Vec<ManagedOpenCodeSession>> {
        let _lock = StateLock::acquire(&self.lock_path)?;
        let Some(mut record) = self.live_record_locked()? else {
            return Ok(Vec::new());
        };
        let mut statuses_by_directory = BTreeMap::new();
        let mut sessions_by_directory = BTreeMap::new();
        for owned in record.sessions.values() {
            if !statuses_by_directory.contains_key(&owned.cwd) {
                let path = with_directory_query("/session/status", &owned.cwd);
                let statuses = self.request_json(&record, "GET", &path, None)?;
                statuses_by_directory.insert(owned.cwd.clone(), statuses);

                let path = with_directory_query("/session", &owned.cwd);
                let sessions = self.request_json(&record, "GET", &path, None)?;
                sessions_by_directory.insert(owned.cwd.clone(), sessions);
            }
        }
        let mut refreshed = BTreeMap::new();
        for owned in record.sessions.values() {
            let Some((title, updated_at_ms)) = provider_session_metadata(
                sessions_by_directory
                    .get(&owned.cwd)
                    .expect("sessions were fetched for each owned directory"),
                &owned.id,
            )?
            else {
                continue;
            };
            if updated_at_ms <= owned.updated_at_ms && title == owned.title {
                continue;
            }
            let path = with_directory_query(
                &format!("/session/{}/message", url_path_segment(&owned.id)),
                &owned.cwd,
            );
            let summary = self
                .request_json(&record, "GET", &path, None)
                .ok()
                .and_then(|messages| latest_assistant_summary(&messages));
            refreshed.insert(owned.id.clone(), (title, summary, updated_at_ms));
        }
        if !refreshed.is_empty() {
            for (id, (title, summary, updated_at_ms)) in refreshed {
                let Some(owned) = record.sessions.get_mut(&id) else {
                    continue;
                };
                owned.title = title;
                if let Some(summary) = summary {
                    owned.summary = summary;
                }
                owned.updated_at_ms = updated_at_ms;
            }
            save_record(&self.record_path, &record)?;
        }
        Ok(record
            .sessions
            .values()
            .map(|owned| {
                let statuses = statuses_by_directory
                    .get(&owned.cwd)
                    .expect("status was fetched for each owned directory");
                self.snapshot_with_state(&record, owned, state_from_statuses(statuses, &owned.id))
            })
            .collect())
    }

    pub fn inspect(&self, session_id: &str) -> Result<String> {
        let _lock = StateLock::acquire(&self.lock_path)?;
        let record = self.required_live_record_locked()?;
        let owned = require_owned(&record, session_id)?;
        let path = with_directory_query(
            &format!("/session/{}/message", url_path_segment(session_id)),
            &owned.cwd,
        );
        let messages = self.request_json(&record, "GET", &path, None)?;
        render_messages(&messages)
    }

    pub fn reply(&self, session_id: &str, prompt: &str) -> Result<()> {
        let prompt = prompt.trim();
        if prompt.is_empty() {
            bail!("the OpenCode reply cannot be empty");
        }
        let _lock = StateLock::acquire(&self.lock_path)?;
        let mut record = self.required_live_record_locked()?;
        let owned = require_owned(&record, session_id)?.clone();
        let path = with_directory_query(
            &format!("/session/{}/prompt_async", url_path_segment(session_id)),
            &owned.cwd,
        );
        self.request_empty(
            &record,
            "POST",
            &path,
            Some(&json!({"parts": [{"type": "text", "text": prompt}]})),
        )?;
        if let Some(session) = record.sessions.get_mut(session_id) {
            session.summary = prompt.into();
            session.updated_at_ms = now_millis();
        }
        save_record(&self.record_path, &record)
    }

    pub fn interrupt(&self, session_id: &str) -> Result<()> {
        let _lock = StateLock::acquire(&self.lock_path)?;
        let record = self.required_live_record_locked()?;
        let owned = require_owned(&record, session_id)?;
        let status = self.status_for(&record, owned)?;
        if status != SessionState::Working {
            bail!("the managed OpenCode session is not currently working");
        }
        let path = with_directory_query(
            &format!("/session/{}/abort", url_path_segment(session_id)),
            &owned.cwd,
        );
        self.request_empty(&record, "POST", &path, Some(&json!({})))
    }

    /// Build a native TUI client attached to the exact authenticated server
    /// that owns this session. The password stays in the child environment;
    /// it is never placed in argv, logs, or a dashboard notice.
    pub fn native_attach_command(&self, session_id: &str) -> Result<Command> {
        let _lock = StateLock::acquire(&self.lock_path)?;
        let record = self.required_live_record_locked()?;
        let owned = require_owned(&record, session_id)?;
        Ok(build_native_attach_command(
            &self.executable,
            &record,
            owned,
        ))
    }

    /// Stop the exact verified test/development server. Normal dashboard exit
    /// deliberately leaves it running for reconnect.
    pub fn shutdown_server(&self) -> Result<()> {
        let _lock = StateLock::acquire(&self.lock_path)?;
        let Some(record) = self.live_record_locked()? else {
            return Ok(());
        };
        #[cfg(target_os = "linux")]
        {
            use std::os::unix::io::{AsRawFd, FromRawFd};

            let raw_fd =
                unsafe { libc::syscall(libc::SYS_pidfd_open, record.pid as libc::pid_t, 0_u32) };
            if raw_fd < 0 {
                return Err(std::io::Error::last_os_error())
                    .context("failed to open exact OpenCode server pidfd");
            }
            let pidfd = unsafe { File::from_raw_fd(raw_fd as i32) };
            // Open the stable kernel reference first, then revalidate that the
            // PID still denotes the recorded process before signaling through
            // the pidfd. No persisted numeric PID is ever passed to kill(2).
            if !verify_server(&record)? {
                bail!("OpenCode server identity changed before shutdown");
            }
            verify_listener_owner(record.pid, record.port)?;
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
                    .context("failed to stop exact OpenCode server through pidfd");
            }
            let mut descriptor = libc::pollfd {
                fd: pidfd.as_raw_fd(),
                events: libc::POLLIN,
                revents: 0,
            };
            let polled = unsafe { libc::poll(&mut descriptor, 1, 5_000) };
            if polled < 0 {
                return Err(std::io::Error::last_os_error())
                    .context("failed while waiting for exact OpenCode server exit");
            }
            if polled == 0 {
                bail!("timed out waiting for exact OpenCode server to exit");
            }
            Ok(())
        }
        #[cfg(not(target_os = "linux"))]
        {
            let _ = record;
            bail!("durable OpenCode supervision currently requires Linux")
        }
    }

    fn ensure_server_locked(&self) -> Result<ServerRecord> {
        let previous = load_record(&self.record_path, &self.state_dir)?;
        if let Some(record) = &previous {
            if record.version != RECORD_VERSION {
                bail!(
                    "unsupported OpenCode supervisor record version {}",
                    record.version
                );
            }
            if verify_server(record)? {
                if !record_uses_executable(record, &self.executable) {
                    bail!(
                        "a verified OpenCode server is already running with executable {}; configured executable is {}",
                        record.executable,
                        self.executable
                    );
                }
                self.verify_http_endpoint(record)?;
                return Ok(record.clone());
            }
        }
        self.start_server(previous.map(|record| record.sessions).unwrap_or_default())
    }

    fn start_server(&self, sessions: BTreeMap<String, OwnedSession>) -> Result<ServerRecord> {
        #[cfg(not(target_os = "linux"))]
        {
            let _ = sessions;
            bail!("durable OpenCode supervision currently requires Linux process identity verification")
        }
        #[cfg(target_os = "linux")]
        {
            let listener = TcpListener::bind(SocketAddrV4::new(Ipv4Addr::LOCALHOST, 0))?;
            let port = listener.local_addr()?.port();
            drop(listener);
            let password = random_secret()?;
            let username = "opencode".to_owned();
            let log = private_append_file(&self.state_dir.join("server.log"))?;
            let mut child = Command::new(&self.executable)
                .args([
                    "serve",
                    "--hostname",
                    "127.0.0.1",
                    "--port",
                    &port.to_string(),
                    "--pure",
                ])
                .env("OPENCODE_SERVER_USERNAME", &username)
                .env("OPENCODE_SERVER_PASSWORD", &password)
                .stdin(Stdio::null())
                .stdout(Stdio::from(log.try_clone()?))
                .stderr(Stdio::from(log))
                .spawn()
                .with_context(|| {
                    format!("failed to start OpenCode server via {}", self.executable)
                })?;
            let pid = child.id();
            let mut record = ServerRecord {
                version: RECORD_VERSION,
                pid,
                process_start_token: String::new(),
                process_cmdline: Vec::new(),
                executable: self.executable.clone(),
                port,
                username,
                password,
                created_at_ms: now_millis(),
                sessions,
            };
            let deadline = Instant::now() + STARTUP_TIMEOUT;
            let result = loop {
                match self.verify_http_endpoint(&record) {
                    Ok(()) => break Ok(()),
                    Err(error) if Instant::now() < deadline => {
                        let _ = error;
                        thread::sleep(Duration::from_millis(40));
                    }
                    Err(error) => break Err(error),
                }
            };
            if let Err(error) = result {
                let _ = child.kill();
                let _ = child.wait();
                return Err(error).context("OpenCode server failed readiness/ownership checks");
            }
            record.process_start_token = process_start_token(pid)?;
            record.process_cmdline = process_cmdline(pid)?;
            if record.process_cmdline.is_empty() {
                let _ = child.kill();
                let _ = child.wait();
                bail!("new OpenCode server exposed an empty process command line");
            }
            // Bind the now-stable process identity to the listener and secret
            // one final time before persisting authority.
            if let Err(error) = self.verify_http_endpoint(&record) {
                let _ = child.kill();
                let _ = child.wait();
                return Err(error).context("OpenCode server identity changed during startup");
            }
            save_record(&self.record_path, &record)?;
            thread::spawn(move || {
                let _ = child.wait();
            });
            Ok(record)
        }
    }

    fn live_record_locked(&self) -> Result<Option<ServerRecord>> {
        let Some(record) = load_record(&self.record_path, &self.state_dir)? else {
            return Ok(None);
        };
        if !verify_server(&record)? {
            return Ok(None);
        }
        self.verify_http_endpoint(&record)?;
        Ok(Some(record))
    }

    fn required_live_record_locked(&self) -> Result<ServerRecord> {
        self.live_record_locked()?
            .context("OpenCode supervisor has no live owned server")
    }

    fn verify_http_endpoint(&self, record: &ServerRecord) -> Result<()> {
        verify_listener_owner(record.pid, record.port)?;
        let health = self.request_json(record, "GET", "/global/health", None)?;
        if health.get("healthy").and_then(Value::as_bool) != Some(true) {
            bail!("OpenCode health endpoint did not report healthy");
        }
        Ok(())
    }

    fn session_snapshot(
        &self,
        record: &ServerRecord,
        owned: &OwnedSession,
    ) -> Result<ManagedOpenCodeSession> {
        let state = self.status_for(record, owned)?;
        Ok(self.snapshot_with_state(record, owned, state))
    }

    fn snapshot_with_state(
        &self,
        record: &ServerRecord,
        owned: &OwnedSession,
        state: SessionState,
    ) -> ManagedOpenCodeSession {
        ManagedOpenCodeSession {
            id: owned.id.clone(),
            cwd: owned.cwd.clone(),
            title: owned.title.clone(),
            summary: owned.summary.clone(),
            state,
            server_pid: record.pid,
            created_at_ms: owned.created_at_ms,
            updated_at_ms: owned.updated_at_ms,
        }
    }

    fn status_for(&self, record: &ServerRecord, owned: &OwnedSession) -> Result<SessionState> {
        let path = with_directory_query("/session/status", &owned.cwd);
        let statuses = self.request_json(record, "GET", &path, None)?;
        Ok(state_from_statuses(&statuses, &owned.id))
    }

    fn request_json(
        &self,
        record: &ServerRecord,
        method: &str,
        path: &str,
        body: Option<&Value>,
    ) -> Result<Value> {
        let response = request_http(record, method, path, body)?;
        if !(200..300).contains(&response.status) {
            return Err(HttpStatusError {
                status: response.status,
                body: String::from_utf8_lossy(&response.body).into_owned(),
            }
            .into());
        }
        if response.body.is_empty() {
            return Ok(Value::Null);
        }
        serde_json::from_slice(&response.body).context("invalid OpenCode server JSON")
    }

    fn request_empty(
        &self,
        record: &ServerRecord,
        method: &str,
        path: &str,
        body: Option<&Value>,
    ) -> Result<()> {
        self.request_json(record, method, path, body).map(|_| ())
    }
}

fn require_owned<'a>(record: &'a ServerRecord, session_id: &str) -> Result<&'a OwnedSession> {
    record
        .sessions
        .get(session_id)
        .context("refusing to control an OpenCode session not created by this supervisor")
}

fn build_native_attach_command(
    executable: &str,
    record: &ServerRecord,
    owned: &OwnedSession,
) -> Command {
    let mut command = Command::new(executable);
    command
        .arg("attach")
        .arg(format!("http://127.0.0.1:{}", record.port))
        .args(["--session", &owned.id, "--dir"])
        .arg(&owned.cwd)
        .env("OPENCODE_SERVER_USERNAME", &record.username)
        .env("OPENCODE_SERVER_PASSWORD", &record.password)
        .current_dir(&owned.cwd);
    command
}

#[derive(Debug)]
struct HttpStatusError {
    status: u16,
    body: String,
}

impl std::fmt::Display for HttpStatusError {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(
            formatter,
            "OpenCode HTTP status {}: {}",
            self.status,
            self.body.trim()
        )
    }
}

impl std::error::Error for HttpStatusError {}

fn state_from_statuses(statuses: &Value, session_id: &str) -> SessionState {
    let status = statuses
        .get(session_id)
        .and_then(|status| status.get("type").or(Some(status)))
        .and_then(Value::as_str);
    match status {
        Some("busy" | "active" | "running") => SessionState::Working,
        Some("retry" | "error") => SessionState::NeedsInput,
        Some("idle") | None => SessionState::Completed,
        Some(_) => SessionState::Unknown,
    }
}

struct HttpResponse {
    status: u16,
    body: Vec<u8>,
}

fn request_http(
    record: &ServerRecord,
    method: &str,
    path: &str,
    body: Option<&Value>,
) -> Result<HttpResponse> {
    let body = body
        .map(serde_json::to_vec)
        .transpose()?
        .unwrap_or_default();
    let authorization =
        base64_encode(format!("{}:{}", record.username, record.password).as_bytes());
    let mut stream = TcpStream::connect_timeout(
        &SocketAddrV4::new(Ipv4Addr::LOCALHOST, record.port).into(),
        HTTP_TIMEOUT,
    )?;
    stream.set_read_timeout(Some(HTTP_TIMEOUT))?;
    stream.set_write_timeout(Some(HTTP_TIMEOUT))?;
    write!(
        stream,
        "{method} {path} HTTP/1.1\r\nHost: 127.0.0.1:{}\r\nAuthorization: Basic {authorization}\r\nAccept: application/json\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
        record.port,
        body.len()
    )?;
    stream.write_all(&body)?;
    stream.flush()?;
    read_http_response(&mut stream)
}

fn read_http_response(stream: &mut TcpStream) -> Result<HttpResponse> {
    let mut bytes = Vec::new();
    stream
        .take((MAX_HEADER_BYTES + MAX_BODY_BYTES + 1) as u64)
        .read_to_end(&mut bytes)?;
    if bytes.len() > MAX_HEADER_BYTES + MAX_BODY_BYTES {
        bail!("OpenCode HTTP response exceeded size limit");
    }
    let split =
        find_bytes(&bytes, b"\r\n\r\n").context("OpenCode HTTP response omitted headers")?;
    if split > MAX_HEADER_BYTES {
        bail!("OpenCode HTTP headers exceeded size limit");
    }
    let headers =
        std::str::from_utf8(&bytes[..split]).context("OpenCode HTTP headers were not UTF-8")?;
    let status = headers
        .lines()
        .next()
        .and_then(|line| line.split_whitespace().nth(1))
        .context("OpenCode HTTP response omitted status")?
        .parse::<u16>()?;
    let mut body = bytes[split + 4..].to_vec();
    let chunked = headers.lines().any(|line| {
        line.split_once(':')
            .map(|(name, value)| {
                name.eq_ignore_ascii_case("transfer-encoding")
                    && value.to_ascii_lowercase().contains("chunked")
            })
            .unwrap_or(false)
    });
    if chunked {
        body = decode_chunked(&body)?;
    }
    if body.len() > MAX_BODY_BYTES {
        bail!("OpenCode HTTP body exceeded size limit");
    }
    Ok(HttpResponse { status, body })
}

fn decode_chunked(input: &[u8]) -> Result<Vec<u8>> {
    let mut remaining = input;
    let mut output = Vec::new();
    loop {
        let line_end = find_bytes(remaining, b"\r\n").context("invalid chunked response")?;
        let size_text = std::str::from_utf8(&remaining[..line_end])?
            .split(';')
            .next()
            .unwrap_or_default();
        let size = usize::from_str_radix(size_text.trim(), 16)?;
        remaining = &remaining[line_end + 2..];
        if size == 0 {
            break;
        }
        if remaining.len() < size + 2 || &remaining[size..size + 2] != b"\r\n" {
            bail!("truncated chunked OpenCode response");
        }
        output.extend_from_slice(&remaining[..size]);
        if output.len() > MAX_BODY_BYTES {
            bail!("OpenCode HTTP body exceeded size limit");
        }
        remaining = &remaining[size + 2..];
    }
    Ok(output)
}

fn find_bytes(haystack: &[u8], needle: &[u8]) -> Option<usize> {
    haystack
        .windows(needle.len())
        .position(|window| window == needle)
}

fn render_messages(value: &Value) -> Result<String> {
    let messages = value
        .as_array()
        .context("OpenCode message endpoint did not return an array")?;
    let mut transcript = Vec::new();
    for message in messages {
        let role = message
            .pointer("/info/role")
            .and_then(Value::as_str)
            .unwrap_or("event");
        let text = message
            .get("parts")
            .and_then(Value::as_array)
            .into_iter()
            .flatten()
            .filter(|part| part.get("type").and_then(Value::as_str) == Some("text"))
            .filter_map(|part| part.get("text").and_then(Value::as_str))
            .collect::<Vec<_>>()
            .join("\n");
        if !text.trim().is_empty() {
            transcript.push(format!("{}: {}", capitalize(role), text.trim()));
        }
    }
    let transcript = if transcript.is_empty() {
        "No text messages are available in this managed OpenCode session.".into()
    } else {
        transcript.join("\n\n")
    };
    Ok(limit_chars(transcript, 32 * 1024))
}

fn provider_session_metadata(value: &Value, session_id: &str) -> Result<Option<(String, u64)>> {
    let sessions = value
        .as_array()
        .context("OpenCode session endpoint did not return an array")?;
    let Some(session) = sessions
        .iter()
        .find(|session| session.get("id").and_then(Value::as_str) == Some(session_id))
    else {
        return Ok(None);
    };
    let title = session
        .get("title")
        .and_then(Value::as_str)
        .context("OpenCode session omitted its title")?;
    let updated_at_ms = session
        .pointer("/time/updated")
        .and_then(Value::as_u64)
        .context("OpenCode session omitted its updated time")?;
    Ok(Some((title.to_owned(), updated_at_ms)))
}

fn latest_assistant_summary(value: &Value) -> Option<String> {
    let messages = value.as_array()?;
    messages
        .iter()
        .filter(|message| {
            message.pointer("/info/role").and_then(Value::as_str) == Some("assistant")
        })
        .filter_map(|message| {
            let text = message
                .get("parts")
                .and_then(Value::as_array)
                .into_iter()
                .flatten()
                .filter(|part| part.get("type").and_then(Value::as_str) == Some("text"))
                .filter_map(|part| part.get("text").and_then(Value::as_str))
                .collect::<Vec<_>>()
                .join("\n");
            let text = text.trim();
            (!text.is_empty()).then(|| {
                let normalized = text.split_whitespace().collect::<Vec<_>>().join(" ");
                if normalized.chars().count() <= 160 {
                    normalized
                } else {
                    let mut summary = normalized.chars().take(159).collect::<String>();
                    summary.push('…');
                    summary
                }
            })
        })
        .last()
}

fn capitalize(value: &str) -> String {
    let mut chars = value.chars();
    chars
        .next()
        .map(|first| first.to_uppercase().chain(chars).collect())
        .unwrap_or_default()
}

fn limit_chars(value: String, limit: usize) -> String {
    if value.chars().count() <= limit {
        return value;
    }
    let tail = value
        .chars()
        .rev()
        .take(limit.saturating_sub(24))
        .collect::<String>()
        .chars()
        .rev()
        .collect::<String>();
    format!("[earlier output omitted]\n{tail}")
}

fn with_directory_query(path: &str, cwd: &Path) -> String {
    format!("{path}?directory={}", url_query_path(cwd))
}

fn url_query_path(path: &Path) -> String {
    #[cfg(unix)]
    {
        use std::os::unix::ffi::OsStrExt;
        percent_encode(path.as_os_str().as_bytes(), false)
    }
    #[cfg(not(unix))]
    percent_encode(path.to_string_lossy().as_bytes(), false)
}

fn url_path_segment(value: &str) -> String {
    percent_encode(value.as_bytes(), true)
}

fn percent_encode(bytes: &[u8], path_segment: bool) -> String {
    let mut output = String::new();
    for &byte in bytes {
        let safe = byte.is_ascii_alphanumeric()
            || matches!(byte, b'-' | b'.' | b'_' | b'~')
            || (!path_segment && byte == b'/');
        if safe {
            output.push(byte as char);
        } else {
            output.push_str(&format!("%{byte:02X}"));
        }
    }
    output
}

fn base64_encode(input: &[u8]) -> String {
    const TABLE: &[u8; 64] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";
    let mut output = String::with_capacity(input.len().div_ceil(3) * 4);
    for chunk in input.chunks(3) {
        let first = chunk[0];
        let second = chunk.get(1).copied().unwrap_or(0);
        let third = chunk.get(2).copied().unwrap_or(0);
        output.push(TABLE[(first >> 2) as usize] as char);
        output.push(TABLE[(((first & 0x03) << 4) | (second >> 4)) as usize] as char);
        output.push(if chunk.len() > 1 {
            TABLE[(((second & 0x0f) << 2) | (third >> 6)) as usize] as char
        } else {
            '='
        });
        output.push(if chunk.len() > 2 {
            TABLE[(third & 0x3f) as usize] as char
        } else {
            '='
        });
    }
    output
}

fn validate_cwd(cwd: &Path) -> Result<()> {
    if !cwd.is_absolute() {
        bail!("the OpenCode working directory must be absolute");
    }
    let metadata = fs::metadata(cwd).context("failed to inspect OpenCode working directory")?;
    if !metadata.is_dir() {
        bail!("the OpenCode working directory is not a directory");
    }
    Ok(())
}

fn opencode_prompt_body(prompt: &str, model: Option<&str>) -> Result<Value> {
    let mut body = json!({"parts": [{"type": "text", "text": prompt}]});
    let Some(identifier) = model else {
        return Ok(body);
    };
    let identifier = identifier.trim();
    if identifier.is_empty()
        || identifier.len() > 128
        || identifier
            .chars()
            .any(|character| character.is_control() || character.is_whitespace())
    {
        bail!("the OpenCode model name must contain 1 to 128 non-whitespace bytes");
    }
    let (provider_id, model_id) = identifier
        .split_once('/')
        .context("the OpenCode model must use provider/model format")?;
    if provider_id.is_empty() || model_id.is_empty() {
        bail!("the OpenCode model must use provider/model format");
    }
    body.as_object_mut()
        .expect("the OpenCode prompt body is an object")
        .insert(
            "model".into(),
            json!({"providerID": provider_id, "modelID": model_id}),
        );
    Ok(body)
}

#[cfg(target_os = "linux")]
fn random_secret() -> Result<String> {
    let mut bytes = [0_u8; 32];
    File::open("/dev/urandom")?.read_exact(&mut bytes)?;
    let mut secret = String::with_capacity(bytes.len() * 2);
    for byte in bytes {
        FmtWrite::write_fmt(&mut secret, format_args!("{byte:02x}"))?;
    }
    Ok(secret)
}

fn default_state_dir() -> Result<PathBuf> {
    if let Some(path) = std::env::var_os("XDG_STATE_HOME") {
        return Ok(PathBuf::from(path).join("open-agent-view/opencode"));
    }
    let home = std::env::var_os("HOME").context("HOME is not set")?;
    Ok(PathBuf::from(home).join(".local/state/open-agent-view/opencode"))
}

fn ensure_private_directory(path: &Path) -> Result<()> {
    match fs::symlink_metadata(path) {
        Ok(metadata) => {
            if !metadata.file_type().is_dir() || metadata.file_type().is_symlink() {
                bail!("OpenCode supervisor state path is not a real directory");
            }
            verify_current_owner(&metadata, "OpenCode supervisor state directory")?;
        }
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => fs::create_dir_all(path)?,
        Err(error) => return Err(error).context("failed to inspect OpenCode supervisor state"),
    }
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        fs::set_permissions(path, fs::Permissions::from_mode(0o700))?;
    }
    Ok(())
}

#[cfg(target_os = "linux")]
fn private_append_file(path: &Path) -> Result<File> {
    let mut options = OpenOptions::new();
    options.create(true).append(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.mode(0o600).custom_flags(libc::O_NOFOLLOW);
    }
    let file = options.open(path)?;
    let metadata = file.metadata()?;
    if !metadata.is_file() {
        bail!("refusing to use a non-regular OpenCode supervisor log");
    }
    verify_current_owner(&metadata, "OpenCode supervisor log")?;
    verify_private_mode(&metadata, "OpenCode supervisor log")?;
    Ok(file)
}

fn load_record(path: &Path, state_dir: &Path) -> Result<Option<ServerRecord>> {
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
        Err(error) => return Err(error).context("failed to open OpenCode server record"),
    };
    let metadata = file.metadata()?;
    if !metadata.is_file() {
        bail!("refusing to use a non-regular OpenCode server record");
    }
    verify_current_owner(&metadata, "OpenCode server record")?;
    verify_private_mode(&metadata, "OpenCode server record")?;
    let mut input = Vec::new();
    file.take((MAX_BODY_BYTES + 1) as u64)
        .read_to_end(&mut input)?;
    if input.len() > MAX_BODY_BYTES {
        bail!("OpenCode server record exceeded size limit");
    }
    let record: ServerRecord = serde_json::from_slice(&input)?;
    validate_record(&record)?;
    if state_dir != path.parent().unwrap_or(state_dir) {
        bail!("OpenCode server record escaped its state directory");
    }
    Ok(Some(record))
}

fn save_record(path: &Path, record: &ServerRecord) -> Result<()> {
    validate_record(record)?;
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

fn validate_record(record: &ServerRecord) -> Result<()> {
    if record.version != RECORD_VERSION
        || record.port == 0
        || record.executable.is_empty()
        || record.username != "opencode"
        || record.password.len() != 64
        || !record.password.bytes().all(|byte| byte.is_ascii_hexdigit())
        || record.process_cmdline.len() > MAX_HEADER_BYTES
    {
        bail!("invalid OpenCode server authority record");
    }
    for (key, session) in &record.sessions {
        if key != &session.id
            || key.is_empty()
            || key.len() > 512
            || key.chars().any(char::is_control)
            || !session.cwd.is_absolute()
            || session.title.len() > MAX_HEADER_BYTES
            || session.summary.len() > MAX_BODY_BYTES
        {
            bail!("invalid owned OpenCode session record");
        }
    }
    Ok(())
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

fn verify_server(record: &ServerRecord) -> Result<bool> {
    if record.process_start_token.is_empty() || record.process_cmdline.is_empty() {
        return Ok(false);
    }
    if process_state(record.pid)?.as_deref() == Some("Z") {
        return Ok(false);
    }
    let start = match process_start_token(record.pid) {
        Ok(value) => value,
        Err(error) if is_missing_process(&error) => return Ok(false),
        Err(error) => return Err(error),
    };
    let cmdline = match process_cmdline(record.pid) {
        Ok(value) => value,
        Err(error) if is_missing_process(&error) => return Ok(false),
        Err(error) => return Err(error),
    };
    Ok(start == record.process_start_token && cmdline == record.process_cmdline)
}

fn record_uses_executable(record: &ServerRecord, configured: &str) -> bool {
    if record.executable == configured {
        return true;
    }
    #[cfg(target_os = "linux")]
    {
        let actual = fs::read_link(format!("/proc/{}/exe", record.pid))
            .ok()
            .and_then(|path| fs::canonicalize(path).ok());
        actual.is_some() && actual == resolve_host_executable(configured)
    }
    #[cfg(not(target_os = "linux"))]
    false
}

#[cfg(target_os = "linux")]
fn resolve_host_executable(executable: &str) -> Option<PathBuf> {
    let path = Path::new(executable);
    if path.components().count() > 1 {
        return fs::canonicalize(path).ok();
    }
    if let Some(found) = std::env::var_os("PATH").and_then(|search| {
        std::env::split_paths(&search)
            .map(|directory| directory.join(executable))
            .find(|candidate| candidate.is_file())
    }) {
        return fs::canonicalize(found).ok();
    }
    let home = PathBuf::from(std::env::var_os("HOME")?);
    [".local/bin", ".opencode/bin", ".bun/bin"]
        .iter()
        .map(|directory| home.join(directory).join(executable))
        .find(|candidate| candidate.is_file())
        .and_then(|candidate| fs::canonicalize(candidate).ok())
}

#[cfg(target_os = "linux")]
fn process_state(pid: u32) -> Result<Option<String>> {
    let stat = match fs::read_to_string(format!("/proc/{pid}/stat")) {
        Ok(stat) => stat,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(None),
        Err(error) => return Err(error.into()),
    };
    Ok(Some(parse_process_stat(&stat)?.0))
}

#[cfg(not(target_os = "linux"))]
fn process_state(_: u32) -> Result<Option<String>> {
    bail!("process-state verification is unavailable on this platform")
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
    Ok(parse_process_stat(&stat)?.1)
}

#[cfg(target_os = "linux")]
fn parse_process_stat(stat: &str) -> Result<(String, String)> {
    let suffix = stat
        .rsplit_once(')')
        .map(|(_, suffix)| suffix)
        .context("invalid /proc process stat")?;
    let fields = suffix.split_whitespace().collect::<Vec<_>>();
    let state = fields
        .first()
        .map(|value| (*value).to_owned())
        .context("/proc process stat omitted state")?;
    let start = fields
        .get(19)
        .map(|value| (*value).to_owned())
        .context("/proc process stat omitted starttime")?;
    Ok((state, start))
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

#[cfg(target_os = "linux")]
fn verify_listener_owner(pid: u32, port: u16) -> Result<()> {
    let mut socket_inodes = BTreeSet::new();
    for entry in fs::read_dir(format!("/proc/{pid}/fd"))? {
        let target = fs::read_link(entry?.path())?;
        let target = target.to_string_lossy();
        if let Some(inode) = target
            .strip_prefix("socket:[")
            .and_then(|value| value.strip_suffix(']'))
        {
            socket_inodes.insert(inode.to_owned());
        }
    }
    let expected_address = format!("0100007F:{port:04X}");
    let found = fs::read_to_string("/proc/net/tcp")?
        .lines()
        .skip(1)
        .filter_map(|line| {
            let fields = line.split_whitespace().collect::<Vec<_>>();
            (fields.len() > 9).then_some((fields[1], fields[3], fields[9]))
        })
        .any(|(address, state, inode)| {
            address == expected_address && state == "0A" && socket_inodes.contains(inode)
        });
    if !found {
        bail!("verified OpenCode process does not own the recorded loopback listener");
    }
    Ok(())
}

#[cfg(not(target_os = "linux"))]
fn verify_listener_owner(_: u32, _: u16) -> Result<()> {
    bail!("listener ownership verification is unavailable on this platform")
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
        if !metadata.is_file() {
            bail!("refusing to use a non-regular OpenCode server lock");
        }
        verify_current_owner(&metadata, "OpenCode server lock")?;
        verify_private_mode(&metadata, "OpenCode server lock")?;
        #[cfg(unix)]
        {
            use std::os::unix::io::AsRawFd;
            if unsafe { libc::flock(file.as_raw_fd(), libc::LOCK_EX) } != 0 {
                return Err(std::io::Error::last_os_error())
                    .context("failed to lock OpenCode server state");
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn encodes_basic_auth_and_url_components() {
        assert_eq!(base64_encode(b"opencode:secret"), "b3BlbmNvZGU6c2VjcmV0");
        assert_eq!(url_path_segment("ses_/ ?"), "ses_%2F%20%3F");
    }

    #[test]
    fn renders_bounded_message_text() {
        let value = json!([
            {"info":{"role":"user"},"parts":[{"type":"text","text":"Build"}]},
            {"info":{"role":"assistant"},"parts":[{"type":"text","text":"Done"}]}
        ]);
        assert_eq!(
            render_messages(&value).unwrap(),
            "User: Build\n\nAssistant: Done"
        );
    }

    #[test]
    fn extracts_current_provider_metadata_and_latest_assistant_summary() {
        let sessions = json!([
            {"id":"other","title":"other","time":{"updated":2}},
            {"id":"ses_owned","title":"renamed task","time":{"updated":42}}
        ]);
        assert_eq!(
            provider_session_metadata(&sessions, "ses_owned").unwrap(),
            Some(("renamed task".into(), 42))
        );
        let messages = json!([
            {"info":{"role":"assistant"},"parts":[{"type":"text","text":"old answer"}]},
            {"info":{"role":"user"},"parts":[{"type":"text","text":"new question"}]},
            {"info":{"role":"assistant"},"parts":[
                {"type":"text","text":"  latest"},
                {"type":"tool"},
                {"type":"text","text":"answer  "}
            ]}
        ]);
        assert_eq!(
            latest_assistant_summary(&messages).as_deref(),
            Some("latest answer")
        );
    }

    #[test]
    fn builds_documented_model_selector_for_async_prompt() {
        assert_eq!(
            opencode_prompt_body("Build", Some("anthropic/claude-sonnet-4-5")).unwrap(),
            json!({
                "parts": [{"type": "text", "text": "Build"}],
                "model": {
                    "providerID": "anthropic",
                    "modelID": "claude-sonnet-4-5"
                }
            })
        );
        assert_eq!(
            opencode_prompt_body("Build", Some("openrouter/vendor/model")).unwrap()["model"],
            json!({"providerID": "openrouter", "modelID": "vendor/model"})
        );
        assert!(opencode_prompt_body("Build", Some("missing-provider")).is_err());
        assert!(opencode_prompt_body("Build", Some("openai/ ")).is_err());
    }

    #[cfg(target_os = "linux")]
    #[test]
    fn parses_zombie_state_separately_from_start_identity() {
        let stat = "12 (provider worker) Z 1 2 3 4 5 6 7 8 9 10 11 12 13 14 15 16 17 18 4242";
        assert_eq!(
            parse_process_stat(stat).unwrap(),
            ("Z".into(), "4242".into())
        );
    }

    #[test]
    fn rejects_unbounded_or_relative_owned_session_records() {
        let record = ServerRecord {
            version: RECORD_VERSION,
            pid: 1,
            process_start_token: "start".into(),
            process_cmdline: b"command".to_vec(),
            executable: "opencode".into(),
            port: 1234,
            username: "opencode".into(),
            password: "a".repeat(64),
            created_at_ms: 1,
            sessions: BTreeMap::from([(
                "ses_owned".into(),
                OwnedSession {
                    id: "ses_owned".into(),
                    cwd: PathBuf::from("relative"),
                    title: "task".into(),
                    summary: String::new(),
                    created_at_ms: 1,
                    updated_at_ms: 1,
                },
            )]),
        };
        assert!(validate_record(&record).is_err());
    }

    #[test]
    fn native_attach_keeps_the_server_secret_out_of_argv() {
        let owned = OwnedSession {
            id: "ses_owned".into(),
            cwd: PathBuf::from("/work/project"),
            title: "task".into(),
            summary: String::new(),
            created_at_ms: 1,
            updated_at_ms: 1,
        };
        let record = ServerRecord {
            version: RECORD_VERSION,
            pid: 1,
            process_start_token: "start".into(),
            process_cmdline: b"command".to_vec(),
            executable: "opencode".into(),
            port: 4242,
            username: "private-user".into(),
            password: "private-password".into(),
            created_at_ms: 1,
            sessions: BTreeMap::from([(owned.id.clone(), owned.clone())]),
        };

        let command = build_native_attach_command("/bin/opencode", &record, &owned);
        let arguments = command
            .get_args()
            .map(|argument| argument.to_string_lossy().into_owned())
            .collect::<Vec<_>>();
        assert_eq!(
            arguments,
            vec![
                "attach",
                "http://127.0.0.1:4242",
                "--session",
                "ses_owned",
                "--dir",
                "/work/project"
            ]
        );
        assert!(!arguments.iter().any(|value| value.contains("private")));
        assert_eq!(command.get_current_dir(), Some(Path::new("/work/project")));
        assert!(command.get_envs().any(|(name, value)| {
            name == "OPENCODE_SERVER_PASSWORD"
                && value == Some(std::ffi::OsStr::new("private-password"))
        }));
    }

    #[cfg(target_os = "linux")]
    #[test]
    fn a_bare_recorded_name_matches_the_same_canonical_running_executable() {
        let pid = std::process::id();
        let record = ServerRecord {
            version: RECORD_VERSION,
            pid,
            process_start_token: process_start_token(pid).unwrap(),
            process_cmdline: process_cmdline(pid).unwrap(),
            executable: "opencode".into(),
            port: 4242,
            username: "opencode".into(),
            password: "x".repeat(64),
            created_at_ms: 1,
            sessions: BTreeMap::new(),
        };

        assert!(record_uses_executable(
            &record,
            std::env::current_exe().unwrap().to_str().unwrap()
        ));
        assert!(!record_uses_executable(&record, "/bin/sh"));
    }
}

use std::collections::BTreeSet;
use std::io::{BufRead, BufReader, Read, Write};
use std::path::PathBuf;
use std::process::{Child, ChildStdin, Command, Stdio};
use std::sync::mpsc::{self, Receiver};
use std::thread::{self, JoinHandle};
use std::time::{Duration, SystemTime};

use anyhow::{anyhow, bail, Context, Result};
use serde::Deserialize;
use serde_json::{json, Value};

use super::{DiscoveryRequest, SessionSource};
use crate::domain::{
    AgentSession, Capability, Provider, Runtime, SessionKind, SessionState,
};

const SOURCE_KINDS: [&str; 10] = [
    "cli",
    "vscode",
    "exec",
    "appServer",
    "subAgent",
    "subAgentReview",
    "subAgentCompact",
    "subAgentThreadSpawn",
    "subAgentOther",
    "unknown",
];

#[derive(Clone, Debug, Eq, PartialEq)]
struct Invocation {
    program: String,
    prefix_args: Vec<String>,
}

impl Invocation {
    fn host(executable: impl Into<String>) -> Self {
        Self {
            program: executable.into(),
            prefix_args: Vec::new(),
        }
    }

    fn docker(container: impl Into<String>) -> Self {
        Self {
            program: "docker".into(),
            prefix_args: vec!["exec".into(), "-i".into(), container.into(), "codex".into()],
        }
    }
}

pub struct CodexSource {
    label: String,
    invocation: Invocation,
    runtime: Runtime,
}

impl CodexSource {
    pub fn host(executable: impl Into<String>) -> Self {
        Self {
            label: "Codex (host)".into(),
            invocation: Invocation::host(executable),
            runtime: Runtime::Host,
        }
    }

    pub fn docker(
        container_name: impl Into<String>,
        container_id: impl Into<String>,
        image: impl Into<String>,
    ) -> Self {
        let container_name = container_name.into();
        let container_id = container_id.into();
        Self {
            label: format!("Codex ({container_name})"),
            invocation: Invocation::docker(container_id.clone()),
            runtime: Runtime::Docker {
                container_id,
                container_name,
                image: image.into(),
            },
        }
    }
}


impl SessionSource for CodexSource {
    fn label(&self) -> &str {
        &self.label
    }

    fn discover(&self, request: &DiscoveryRequest) -> Result<Vec<AgentSession>> {
        let mut transport = AppServerTransport::spawn(&self.invocation)?;
        transport.request(
            1,
            "initialize",
            json!({
                "clientInfo": {
                    "name": "open_agent_view",
                    "title": "Open Agent View",
                    "version": env!("CARGO_PKG_VERSION")
                }
            }),
        )?;
        transport.notify("initialized", json!({}))?;

        let mut cursor: Option<String> = None;
        let mut records = Vec::new();
        for request_id in 2..=12 {
            let response = transport.request(
                request_id,
                "thread/list",
                json!({
                    "archived": false,
                    "cursor": cursor,
                    "limit": 100,
                    "sortKey": "updated_at",
                    "sortDirection": "desc",
                    "sourceKinds": SOURCE_KINDS,
                    "useStateDbOnly": false
                }),
            )?;
            let page = parse_codex_thread_list(&response, self.runtime.clone())?;
            records.extend(page.sessions);
            cursor = page.next_cursor;
            if cursor.is_none() {
                break;
            }
        }
        if cursor.is_some() {
            bail!("Codex thread list exceeded the 1,100-session safety cap");
        }

        records.retain(|session| {
            (request.include_completed || session.state != SessionState::Completed)
                && request
                    .cwd
                    .as_ref()
                    .map(|cwd| session.cwd.starts_with(cwd))
                    .unwrap_or(true)
        });
        Ok(records)
    }
}

struct AppServerTransport {
    child: Child,
    stdin: ChildStdin,
    lines: Receiver<Result<String, String>>,
    stdout_reader: Option<JoinHandle<()>>,
    stderr_reader: Option<JoinHandle<Vec<u8>>>,
}

impl AppServerTransport {
    fn spawn(invocation: &Invocation) -> Result<Self> {
        let mut args = invocation.prefix_args.clone();
        args.extend([
            "app-server".into(),
            "--listen".into(),
            "stdio://".into(),
        ]);
        let mut child = Command::new(&invocation.program)
            .args(args)
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()
            .with_context(|| format!("failed to start {} app-server", invocation.program))?;
        let stdin = child.stdin.take().context("failed to capture app-server stdin")?;
        let stdout = child
            .stdout
            .take()
            .context("failed to capture app-server stdout")?;
        let stderr = child
            .stderr
            .take()
            .context("failed to capture app-server stderr")?;
        let (sender, lines) = mpsc::channel();
        let stdout_reader = thread::spawn(move || {
            for line in BufReader::new(stdout).lines() {
                let message = line.map_err(|error| error.to_string());
                if sender.send(message).is_err() {
                    break;
                }
            }
        });
        let stderr_reader = thread::spawn(move || {
            let mut stderr = stderr.take(128 * 1024);
            let mut bytes = Vec::new();
            let _ = stderr.read_to_end(&mut bytes);
            bytes
        });

        Ok(Self {
            child,
            stdin,
            lines,
            stdout_reader: Some(stdout_reader),
            stderr_reader: Some(stderr_reader),
        })
    }

    fn request(&mut self, id: u64, method: &str, params: Value) -> Result<Value> {
        self.send(json!({"method": method, "id": id, "params": params}))?;
        let deadline = std::time::Instant::now() + Duration::from_secs(8);
        loop {
            let remaining = deadline.saturating_duration_since(std::time::Instant::now());
            let line = self
                .lines
                .recv_timeout(remaining)
                .map_err(|error| anyhow!("timed out waiting for {method}: {error}"))?
                .map_err(|error| anyhow!("app-server stdout error: {error}"))?;
            let message: Value = serde_json::from_str(&line)
                .with_context(|| format!("invalid app-server JSONL: {line}"))?;
            if message.get("id").and_then(Value::as_u64) != Some(id) {
                continue;
            }
            if let Some(error) = message.get("error") {
                bail!("app-server {method} failed: {error}");
            }
            return message
                .get("result")
                .cloned()
                .context("app-server response omitted result");
        }
    }

    fn notify(&mut self, method: &str, params: Value) -> Result<()> {
        self.send(json!({"method": method, "params": params}))
    }

    fn send(&mut self, message: Value) -> Result<()> {
        serde_json::to_writer(&mut self.stdin, &message)?;
        self.stdin.write_all(b"\n")?;
        self.stdin.flush()?;
        Ok(())
    }
}

impl Drop for AppServerTransport {
    fn drop(&mut self) {
        let _ = self.child.kill();
        let _ = self.child.wait();
        if let Some(reader) = self.stdout_reader.take() {
            let _ = reader.join();
        }
        if let Some(reader) = self.stderr_reader.take() {
            let _ = reader.join();
        }
    }
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct ThreadListResponse {
    data: Vec<CodexThread>,
    next_cursor: Option<String>,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct CodexThread {
    id: String,
    cwd: PathBuf,
    created_at: i64,
    updated_at: i64,
    preview: String,
    name: Option<String>,
    status: Value,
    source: Value,
}

pub struct CodexThreadPage {
    pub sessions: Vec<AgentSession>,
    pub next_cursor: Option<String>,
}

pub fn parse_codex_thread_list(input: &Value, runtime: Runtime) -> Result<CodexThreadPage> {
    let response: ThreadListResponse =
        serde_json::from_value(input.clone()).context("invalid Codex thread/list response")?;
    Ok(CodexThreadPage {
        sessions: response
            .data
            .into_iter()
            .map(|thread| normalize_thread(thread, runtime.clone()))
            .collect(),
        next_cursor: response.next_cursor,
    })
}

fn normalize_thread(thread: CodexThread, runtime: Runtime) -> AgentSession {
    let raw_state = thread
        .status
        .get("type")
        .and_then(Value::as_str)
        .unwrap_or("unknown")
        .to_owned();
    let state = map_status(&thread.status);
    let source = thread
        .source
        .get("type")
        .and_then(Value::as_str)
        .or_else(|| thread.source.as_str())
        .unwrap_or("unknown");
    let kind = match source {
        "cli" | "vscode" => SessionKind::Interactive,
        "appServer" | "exec" | "subAgent" | "subAgentReview" | "subAgentCompact"
        | "subAgentThreadSpawn" | "subAgentOther" => SessionKind::Background,
        _ => SessionKind::Unknown,
    };
    let name = thread.name.unwrap_or_else(|| {
        let preview = truncate_words(&thread.preview, 36);
        if preview.is_empty() {
            thread
                .cwd
                .file_name()
                .and_then(|name| name.to_str())
                .unwrap_or("codex-thread")
                .to_owned()
        } else {
            preview
        }
    });
    let runtime_id = match &runtime {
        Runtime::Host => "host",
        Runtime::Docker { container_id, .. } => container_id,
    };
    let capabilities = BTreeSet::from([Capability::Inspect]);

    AgentSession {
        id: format!("codex:{runtime_id}:{}", thread.id),
        provider_session_id: thread.id,
        provider: Provider::Codex,
        runtime,
        kind,
        name,
        cwd: thread.cwd,
        state,
        summary: thread.preview,
        raw_state: Some(raw_state),
        pid: None,
        started_at: unix_seconds(thread.created_at),
        updated_at: unix_seconds(thread.updated_at),
        pull_requests: None,
        capabilities,
    }
}

fn map_status(status: &Value) -> SessionState {
    match status.get("type").and_then(Value::as_str) {
        Some("active") => {
            let waiting = status
                .get("activeFlags")
                .and_then(Value::as_array)
                .map(|flags| {
                    flags.iter().any(|flag| {
                        matches!(
                            flag.as_str(),
                            Some("waitingOnApproval" | "waitingOnUserInput")
                        )
                    })
                })
                .unwrap_or(false);
            if waiting {
                SessionState::NeedsInput
            } else {
                SessionState::Working
            }
        }
        Some("idle") => SessionState::Completed,
        Some("systemError") => SessionState::NeedsInput,
        Some("notLoaded") | Some(_) | None => SessionState::Unknown,
    }
}

fn unix_seconds(seconds: i64) -> Option<SystemTime> {
    u64::try_from(seconds)
        .ok()
        .map(|seconds| SystemTime::UNIX_EPOCH + Duration::from_secs(seconds))
}

fn truncate_words(input: &str, max_chars: usize) -> String {
    let normalized = input.split_whitespace().collect::<Vec<_>>().join(" ");
    if normalized.chars().count() <= max_chars {
        return normalized;
    }
    let mut truncated: String = normalized.chars().take(max_chars.saturating_sub(1)).collect();
    truncated.push('…');
    truncated
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_thread_list_and_maps_waiting_state() {
        let response = json!({
            "data": [{
                "id": "0198-thread",
                "cwd": "/work/project",
                "createdAt": 1_700_000_000,
                "updatedAt": 1_700_000_100,
                "preview": "Implement the dashboard",
                "name": "dashboard",
                "status": {
                    "type": "active",
                    "activeFlags": ["waitingOnApproval"]
                },
                "source": {"type": "appServer"}
            }],
            "nextCursor": null
        });

        let page = parse_codex_thread_list(&response, Runtime::Host).unwrap();

        assert_eq!(page.sessions.len(), 1);
        assert_eq!(page.sessions[0].provider, Provider::Codex);
        assert_eq!(page.sessions[0].state, SessionState::NeedsInput);
        assert_eq!(page.sessions[0].kind, SessionKind::Background);
        assert_eq!(page.sessions[0].summary, "Implement the dashboard");
    }

    #[test]
    fn not_loaded_history_remains_unknown() {
        assert_eq!(
            map_status(&json!({"type": "notLoaded"})),
            SessionState::Unknown
        );
    }
}

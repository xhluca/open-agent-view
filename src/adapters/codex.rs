use std::collections::BTreeSet;
use std::path::PathBuf;
use std::sync::{Arc, Mutex};
use std::time::{Duration, SystemTime};

use anyhow::{anyhow, Context, Result};
use serde::Deserialize;
use serde_json::{json, Value};

use super::{DiscoveryRequest, SessionSource, SourceDiscovery};
use crate::codex_rpc::{AppServerClient, AppServerInvocation};
use crate::codex_supervisor::CodexSupervisor;
use crate::domain::{AgentSession, Provider, Runtime, SessionKind, SessionState};

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

enum ConnectionFactory {
    Direct(AppServerInvocation),
    Managed(Arc<CodexSupervisor>),
}

pub struct CodexSource {
    label: String,
    factory: ConnectionFactory,
    runtime: Runtime,
    owned_only: bool,
    connection: Mutex<Option<AppServerClient>>,
}

impl CodexSource {
    pub fn host(executable: impl Into<String>) -> Self {
        Self {
            label: "Codex (host)".into(),
            factory: ConnectionFactory::Direct(AppServerInvocation::direct(executable)),
            runtime: Runtime::Host,
            owned_only: false,
            connection: Mutex::new(None),
        }
    }

    pub fn managed(supervisor: Arc<CodexSupervisor>) -> Self {
        Self {
            label: "Codex (managed host)".into(),
            factory: ConnectionFactory::Managed(supervisor),
            runtime: Runtime::Host,
            owned_only: false,
            connection: Mutex::new(None),
        }
    }

    /// Maintenance inventory restricted to exact durable-supervisor records.
    pub fn managed_owned(supervisor: Arc<CodexSupervisor>) -> Self {
        Self {
            label: "Codex (owned host maintenance)".into(),
            factory: ConnectionFactory::Managed(supervisor),
            runtime: Runtime::Host,
            owned_only: true,
            connection: Mutex::new(None),
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
            factory: ConnectionFactory::Direct(AppServerInvocation::docker(container_id.clone())),
            runtime: Runtime::Docker {
                container_id,
                container_name,
                image: image.into(),
            },
            owned_only: false,
            connection: Mutex::new(None),
        }
    }

    fn discover_once(&self, request: &DiscoveryRequest) -> Result<SourceDiscovery> {
        let mut connection = self
            .connection
            .lock()
            .map_err(|_| anyhow!("Codex connection lock was poisoned"))?;
        if connection.is_none() {
            *connection = Some(match &self.factory {
                ConnectionFactory::Direct(invocation) => AppServerClient::connect(invocation)?,
                ConnectionFactory::Managed(supervisor) => supervisor.connect_client()?,
            });
        }
        let transport = connection.as_mut().expect("connection initialized");

        if self.owned_only {
            let ConnectionFactory::Managed(supervisor) = &self.factory else {
                unreachable!("owned-only Codex discovery requires a supervisor")
            };
            let sessions = mark_managed_threads(read_threads(
                transport,
                supervisor.owned_thread_ids()?,
                self.runtime.clone(),
            )?);
            return Ok(SourceDiscovery {
                sessions,
                warnings: Vec::new(),
            });
        }

        if !request.include_completed {
            let loaded =
                parse_loaded_thread_ids(&transport.request("thread/loaded/list", json!({}))?)?;
            let owned = match &self.factory {
                ConnectionFactory::Managed(supervisor) => Some(
                    supervisor
                        .owned_thread_ids()?
                        .into_iter()
                        .collect::<BTreeSet<_>>(),
                ),
                ConnectionFactory::Direct(_) => None,
            };
            let loaded = loaded
                .into_iter()
                .filter(|thread_id| {
                    owned
                        .as_ref()
                        .map(|owned| owned.contains(thread_id))
                        .unwrap_or(true)
                })
                .collect();
            let sessions = read_threads(transport, loaded, self.runtime.clone())?;
            return Ok(SourceDiscovery {
                sessions,
                warnings: Vec::new(),
            });
        }

        let mut cursor: Option<String> = None;
        let mut records = Vec::new();
        let history_limit = request.history_limit.max(1);
        while records.len() < history_limit {
            let page_limit = (history_limit - records.len()).min(100);
            let response = transport.request(
                "thread/list",
                json!({
                    "archived": false,
                    "cursor": cursor,
                    "limit": page_limit,
                    "sortKey": "updated_at",
                    "sortDirection": if request.history_oldest_first { "asc" } else { "desc" },
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

        records.retain(|session| {
            (request.include_completed || session.state != SessionState::Completed)
                && request
                    .cwd
                    .as_ref()
                    .map(|cwd| session.cwd.starts_with(cwd))
                    .unwrap_or(true)
        });
        let warnings = cursor
            .is_some()
            .then(|| {
                format!(
                    "Codex history is limited to {} records for this refresh; increase --history-limit to load more",
                    history_limit
                )
            })
            .into_iter()
            .collect();
        Ok(SourceDiscovery {
            sessions: records,
            warnings,
        })
    }
}

impl SessionSource for CodexSource {
    fn label(&self) -> &str {
        &self.label
    }

    fn discover(&self, request: &DiscoveryRequest) -> Result<Vec<AgentSession>> {
        Ok(self.discover_with_warnings(request)?.sessions)
    }

    fn discover_with_warnings(&self, request: &DiscoveryRequest) -> Result<SourceDiscovery> {
        for attempt in 0..2 {
            match self.discover_once(request) {
                Ok(discovery) => return Ok(discovery),
                Err(_) if attempt == 0 => {
                    let mut connection = self
                        .connection
                        .lock()
                        .map_err(|_| anyhow!("Codex connection lock was poisoned"))?;
                    *connection = None;
                    drop(connection);
                }
                Err(error) => return Err(error),
            }
        }
        unreachable!("Codex discovery has exactly two attempts")
    }
}

fn read_threads(
    transport: &mut AppServerClient,
    thread_ids: Vec<String>,
    runtime: Runtime,
) -> Result<Vec<AgentSession>> {
    thread_ids
        .into_iter()
        .map(|thread_id| {
            let response = transport.request(
                "thread/read",
                json!({"threadId": thread_id, "includeTurns": false}),
            )?;
            parse_codex_thread_read(&response, runtime.clone())
        })
        .collect()
}

fn mark_managed_threads(mut sessions: Vec<AgentSession>) -> Vec<AgentSession> {
    // Codex currently reports threads created through App Server as `cli` on
    // some releases. These exact IDs came from OAV's durable ownership record,
    // so their product semantics are managed background tasks regardless of
    // that provider-side source label. Leaving them Interactive makes the
    // dashboard's default foreground filter hide a task immediately after OAV
    // launches it.
    for session in &mut sessions {
        session.kind = SessionKind::Background;
    }
    sessions
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct ThreadListResponse {
    data: Vec<CodexThread>,
    next_cursor: Option<String>,
}

#[derive(Debug, Deserialize)]
struct LoadedThreadListResponse {
    data: Vec<String>,
}

#[derive(Debug, Deserialize)]
struct ThreadReadResponse {
    thread: CodexThread,
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

fn parse_loaded_thread_ids(input: &Value) -> Result<Vec<String>> {
    let response: LoadedThreadListResponse = serde_json::from_value(input.clone())
        .context("invalid Codex thread/loaded/list response")?;
    Ok(response.data)
}

fn parse_codex_thread_read(input: &Value, runtime: Runtime) -> Result<AgentSession> {
    let response: ThreadReadResponse =
        serde_json::from_value(input.clone()).context("invalid Codex thread/read response")?;
    Ok(normalize_thread(response.thread, runtime))
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
        "appServer"
        | "exec"
        | "subAgent"
        | "subAgentReview"
        | "subAgentCompact"
        | "subAgentThreadSpawn"
        | "subAgentOther" => SessionKind::Background,
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
    // The summary remains observable, but transcript/control capabilities are
    // granted only after the host supervisor proves exact ownership.
    let capabilities = BTreeSet::new();

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
    let mut truncated: String = normalized
        .chars()
        .take(max_chars.saturating_sub(1))
        .collect();
    truncated.push('…');
    truncated
}

#[cfg(test)]
mod tests {
    use super::*;

    #[cfg(unix)]
    fn mock_app_server() -> tempfile::TempDir {
        use std::fs;
        use std::os::unix::fs::PermissionsExt;

        let directory = tempfile::tempdir().unwrap();
        let executable = directory.path().join("mock-codex");
        fs::write(
            &executable,
            r#"#!/usr/bin/env python3
import json, sys

def thread(identifier, status):
    return {
        "id": identifier,
        "cwd": "/work/codex",
        "createdAt": 1700000000,
        "updatedAt": 1700000100,
        "preview": "Bounded Codex discovery",
        "name": "bounded-codex",
        "status": status,
        "source": {"type": "appServer"},
    }

for line in sys.stdin:
    message = json.loads(line)
    method = message.get("method")
    ident = message.get("id")
    if ident is None:
        continue
    if method == "initialize":
        result = {"userAgent": "mock/1"}
    elif method == "thread/loaded/list":
        result = {"data": ["loaded-thread"]}
    elif method == "thread/read":
        result = {"thread": thread(message["params"]["threadId"], {"type": "active", "activeFlags": []})}
    elif method == "thread/list":
        limit = message["params"]["limit"]
        result = {"data": [thread("history-" + str(index), {"type": "idle"}) for index in range(limit)], "nextCursor": "more"}
    else:
        result = {}
    print(json.dumps({"id": ident, "result": result}), flush=True)
"#,
        )
        .unwrap();
        let mut permissions = fs::metadata(&executable).unwrap().permissions();
        permissions.set_mode(0o700);
        fs::set_permissions(&executable, permissions).unwrap();
        directory
    }

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

    #[test]
    fn exact_managed_threads_are_background_even_when_codex_labels_them_cli() {
        let response = json!({
            "thread": {
                "id": "owned-thread",
                "cwd": "/work",
                "createdAt": 1,
                "updatedAt": 2,
                "preview": "managed task",
                "name": null,
                "status": {"type": "active", "activeFlags": []},
                "source": {"type": "cli"}
            }
        });
        let parsed = parse_codex_thread_read(&response, Runtime::Host).unwrap();
        assert_eq!(parsed.kind, SessionKind::Interactive);

        let managed = mark_managed_threads(vec![parsed]);

        assert_eq!(managed[0].kind, SessionKind::Background);
        assert_eq!(managed[0].provider_session_id, "owned-thread");
    }

    #[cfg(unix)]
    #[test]
    fn active_discovery_reads_only_the_loaded_inventory() {
        let directory = mock_app_server();
        let source = CodexSource::host(directory.path().join("mock-codex").display().to_string());

        let sessions = source.discover(&DiscoveryRequest::default()).unwrap();

        assert_eq!(sessions.len(), 1);
        assert_eq!(sessions[0].provider_session_id, "loaded-thread");
        assert_eq!(sessions[0].state, SessionState::Working);
    }

    #[cfg(unix)]
    #[test]
    fn discovery_reconnects_once_after_a_stale_process_transport() {
        use std::fs;
        use std::os::unix::fs::PermissionsExt;

        let directory = tempfile::tempdir().unwrap();
        let executable = directory.path().join("flaky-codex");
        let counter = directory.path().join("starts");
        fs::write(
            &executable,
            format!(
                r#"#!/usr/bin/env python3
import json, pathlib, sys
counter = pathlib.Path({counter:?})
attempt = int(counter.read_text()) + 1 if counter.exists() else 1
counter.write_text(str(attempt))
for line in sys.stdin:
    message = json.loads(line)
    method = message.get("method")
    ident = message.get("id")
    if ident is None:
        continue
    if method == "initialize":
        result = {{"userAgent": "flaky/1"}}
    elif method == "thread/loaded/list" and attempt == 1:
        raise SystemExit(0)
    elif method == "thread/loaded/list":
        result = {{"data": []}}
    else:
        result = {{}}
    print(json.dumps({{"id": ident, "result": result}}), flush=True)
"#,
                counter = counter.display().to_string()
            ),
        )
        .unwrap();
        fs::set_permissions(&executable, fs::Permissions::from_mode(0o700)).unwrap();
        let source = CodexSource::host(executable.display().to_string());

        assert!(source
            .discover(&DiscoveryRequest::default())
            .unwrap()
            .is_empty());
        assert_eq!(fs::read_to_string(counter).unwrap(), "2");
    }

    #[cfg(unix)]
    #[test]
    fn large_history_returns_the_budget_and_a_nonfatal_warning() {
        let directory = mock_app_server();
        let source = CodexSource::host(directory.path().join("mock-codex").display().to_string());

        let result = source
            .discover_with_warnings(&DiscoveryRequest {
                include_completed: true,
                history_limit: 3,
                ..DiscoveryRequest::default()
            })
            .unwrap();

        assert_eq!(result.sessions.len(), 3);
        assert_eq!(result.warnings.len(), 1);
        assert!(result.warnings[0].contains("limited to 3 records"));
    }
}

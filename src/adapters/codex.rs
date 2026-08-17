use std::collections::BTreeSet;
use std::path::PathBuf;
use std::sync::{Arc, Mutex};
use std::time::{Duration, SystemTime};

use anyhow::{anyhow, bail, Context, Result};
use serde::Deserialize;
use serde_json::{json, Value};

use super::{DiscoveryRequest, SessionSource};
use crate::codex_rpc::{AppServerClient, AppServerInvocation};
use crate::codex_supervisor::CodexSupervisor;
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

enum ConnectionFactory {
    Direct(AppServerInvocation),
    Managed(Arc<CodexSupervisor>),
}

pub struct CodexSource {
    label: String,
    factory: ConnectionFactory,
    runtime: Runtime,
    connection: Mutex<Option<AppServerClient>>,
}

impl CodexSource {
    pub fn host(executable: impl Into<String>) -> Self {
        Self {
            label: "Codex (host)".into(),
            factory: ConnectionFactory::Direct(AppServerInvocation::direct(executable)),
            runtime: Runtime::Host,
            connection: Mutex::new(None),
        }
    }

    pub fn managed(supervisor: Arc<CodexSupervisor>) -> Self {
        Self {
            label: "Codex (managed host)".into(),
            factory: ConnectionFactory::Managed(supervisor),
            runtime: Runtime::Host,
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
            connection: Mutex::new(None),
        }
    }
}


impl SessionSource for CodexSource {
    fn label(&self) -> &str {
        &self.label
    }

    fn discover(&self, request: &DiscoveryRequest) -> Result<Vec<AgentSession>> {
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

        let mut cursor: Option<String> = None;
        let mut records = Vec::new();
        for _ in 0..=10 {
            let response = transport.request(
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

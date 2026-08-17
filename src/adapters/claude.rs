use std::collections::BTreeSet;
use std::path::PathBuf;
use std::sync::Arc;
use std::time::{Duration, SystemTime};

use anyhow::{bail, Context, Result};
use serde::Deserialize;

use super::{DiscoveryRequest, SessionSource};
use crate::domain::{
    AgentSession, Capability, Provider, Runtime, SessionKind, SessionState,
};
use crate::process::{CommandRequest, CommandRunner, ProcessRunner};

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct Invocation {
    pub program: String,
    pub prefix_args: Vec<String>,
}

impl Invocation {
    pub fn host(executable: impl Into<String>) -> Self {
        Self {
            program: executable.into(),
            prefix_args: Vec::new(),
        }
    }

    pub fn docker(container: impl Into<String>) -> Self {
        Self {
            program: "docker".into(),
            prefix_args: vec!["exec".into(), container.into(), "claude".into()],
        }
    }
}

pub struct ClaudeSource {
    label: String,
    invocation: Invocation,
    runtime: Runtime,
    runner: Arc<dyn CommandRunner>,
}

impl ClaudeSource {
    pub fn host(executable: impl Into<String>) -> Self {
        Self {
            label: "Claude (host)".into(),
            invocation: Invocation::host(executable),
            runtime: Runtime::Host,
            runner: Arc::new(ProcessRunner),
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
            label: format!("Claude ({container_name})"),
            invocation: Invocation::docker(container_id.clone()),
            runtime: Runtime::Docker {
                container_id,
                container_name,
                image: image.into(),
            },
            runner: Arc::new(ProcessRunner),
        }
    }

    #[cfg(test)]
    fn with_runner(
        label: impl Into<String>,
        invocation: Invocation,
        runtime: Runtime,
        runner: Arc<dyn CommandRunner>,
    ) -> Self {
        Self {
            label: label.into(),
            invocation,
            runtime,
            runner,
        }
    }
}

impl SessionSource for ClaudeSource {
    fn label(&self) -> &str {
        &self.label
    }

    fn discover(&self, request: &DiscoveryRequest) -> Result<Vec<AgentSession>> {
        let mut args = self.invocation.prefix_args.clone();
        args.extend(["agents".into(), "--json".into()]);
        if request.include_completed {
            args.push("--all".into());
        }
        if let Some(cwd) = &request.cwd {
            args.extend(["--cwd".into(), cwd.display().to_string()]);
        }
        let mut command = CommandRequest::new(self.invocation.program.clone(), args);
        command.timeout = Duration::from_secs(8);
        let output = self.runner.run(&command)?;
        if output.status != 0 {
            bail!(
                "claude agents exited with status {}: {}",
                output.status,
                output.stderr_lossy()
            );
        }

        let sessions = parse_claude_sessions(output.stdout_text()?, self.runtime.clone())?;
        Ok(sessions
            .into_iter()
            .filter(|session| {
                request.include_interactive || session.kind != SessionKind::Interactive
            })
            .collect())
    }
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct ClaudeRecord {
    pid: Option<u32>,
    id: Option<String>,
    cwd: PathBuf,
    kind: Option<String>,
    started_at: Option<u64>,
    session_id: String,
    name: Option<String>,
    status: Option<String>,
    state: Option<String>,
}

pub fn parse_claude_sessions(input: &str, runtime: Runtime) -> Result<Vec<AgentSession>> {
    let records: Vec<ClaudeRecord> =
        serde_json::from_str(input).context("invalid `claude agents --json` output")?;
    Ok(records
        .into_iter()
        .map(|record| normalize_record(record, runtime.clone()))
        .collect())
}

fn normalize_record(record: ClaudeRecord, runtime: Runtime) -> AgentSession {
    let raw_state = record.state.clone();
    let state = map_state(record.state.as_deref(), record.status.as_deref());
    let kind = match record.kind.as_deref() {
        Some("interactive") => SessionKind::Interactive,
        Some("background") => SessionKind::Background,
        Some(_) | None => SessionKind::Unknown,
    };
    let short_id = record
        .id
        .clone()
        .unwrap_or_else(|| record.session_id.chars().take(8).collect());
    let name = record.name.unwrap_or_else(|| {
        record
            .cwd
            .file_name()
            .and_then(|name| name.to_str())
            .map(ToOwned::to_owned)
            .unwrap_or_else(|| format!("claude-{short_id}"))
    });
    let summary = match record.status.as_deref() {
        Some(status) if !status.eq_ignore_ascii_case("busy") => status.to_owned(),
        _ if kind == SessionKind::Interactive => "Interactive Claude session".into(),
        _ => String::new(),
    };
    let started_at = record
        .started_at
        .map(|milliseconds| SystemTime::UNIX_EPOCH + Duration::from_millis(milliseconds));
    // Discovery and control are separate contracts. Control capabilities are
    // added only by an owning controller, never inferred from provider state.
    let capabilities = BTreeSet::from([Capability::Inspect]);
    let runtime_id = match &runtime {
        Runtime::Host => "host",
        Runtime::Docker { container_id, .. } => container_id,
    };

    AgentSession {
        id: format!("claude:{runtime_id}:{}", record.session_id),
        provider_session_id: record.session_id,
        provider: Provider::Claude,
        runtime,
        kind,
        name,
        cwd: record.cwd,
        state,
        summary,
        raw_state,
        pid: record.pid,
        started_at,
        updated_at: None,
        pull_requests: None,
        capabilities,
    }
}

fn map_state(state: Option<&str>, status: Option<&str>) -> SessionState {
    match state.map(str::to_ascii_lowercase).as_deref() {
        Some("working") | Some("busy") | Some("running") => SessionState::Working,
        Some("blocked") | Some("needs_input") | Some("waiting") => SessionState::NeedsInput,
        Some("ready") | Some("ready_for_review") => SessionState::ReadyForReview,
        Some("done") | Some("completed") | Some("stopped") => SessionState::Completed,
        Some(_) | None if status == Some("blocked") => SessionState::NeedsInput,
        Some(_) | None => SessionState::Unknown,
    }
}

#[cfg(test)]
mod tests {
    use std::sync::Mutex;

    use super::*;
    use crate::process::CommandOutput;

    struct FakeRunner {
        expected: CommandRequest,
        output: Mutex<Option<CommandOutput>>,
    }

    impl CommandRunner for FakeRunner {
        fn run(&self, request: &CommandRequest) -> Result<CommandOutput> {
            assert_eq!(request, &self.expected);
            Ok(self.output.lock().unwrap().take().unwrap())
        }
    }

    #[test]
    fn parses_current_claude_json_shape() {
        let input = r#"[
          {
            "pid": 42,
            "id": "9dd51534",
            "cwd": "/work/repo",
            "kind": "background",
            "startedAt": 1784989094589,
            "sessionId": "14b62dfb-c56b-47c4-ad13-3f2963268463",
            "name": "manager",
            "status": "busy",
            "state": "working"
          }
        ]"#;

        let sessions = parse_claude_sessions(input, Runtime::Host).unwrap();

        assert_eq!(sessions.len(), 1);
        assert_eq!(sessions[0].name, "manager");
        assert_eq!(sessions[0].state, SessionState::Working);
        assert_eq!(sessions[0].kind, SessionKind::Background);
        assert_eq!(
            sessions[0].capabilities,
            BTreeSet::from([Capability::Inspect])
        );
    }

    #[test]
    fn source_constructs_filtered_cli_request() {
        let mut expected = CommandRequest::new(
            "claude",
            vec![
                "agents".into(),
                "--json".into(),
                "--all".into(),
                "--cwd".into(),
                "/work".into(),
            ],
        );
        expected.timeout = Duration::from_secs(8);
        let runner = Arc::new(FakeRunner {
            expected,
            output: Mutex::new(Some(CommandOutput {
                status: 0,
                stdout: b"[]".to_vec(),
                stderr: vec![],
            })),
        });
        let source = ClaudeSource::with_runner(
            "test",
            Invocation::host("claude"),
            Runtime::Host,
            runner,
        );

        let sessions = source
            .discover(&DiscoveryRequest {
                include_completed: true,
                include_interactive: false,
                cwd: Some(PathBuf::from("/work")),
            })
            .unwrap();

        assert!(sessions.is_empty());
    }
}

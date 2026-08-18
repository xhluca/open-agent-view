use std::collections::BTreeSet;
use std::path::PathBuf;
use std::sync::Arc;
use std::time::{Duration, SystemTime};

use anyhow::{bail, Context, Result};
use serde::Deserialize;

use super::{DiscoveryRequest, SessionSource};
use crate::domain::{AgentSession, Capability, Provider, Runtime, SessionKind, SessionState};
use crate::process::{CommandRequest, CommandRunner, ProcessRunner};

/// A command prefix for an OpenCode installation on the host or in Docker.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct OpenCodeInvocation {
    pub program: String,
    pub prefix_args: Vec<String>,
}

impl OpenCodeInvocation {
    pub fn host(executable: impl Into<String>) -> Self {
        Self {
            program: executable.into(),
            prefix_args: Vec::new(),
        }
    }

    pub fn docker(container: impl Into<String>) -> Self {
        Self {
            program: "docker".into(),
            prefix_args: vec!["exec".into(), container.into(), "opencode".into()],
        }
    }
}

/// Read-only discovery of sessions persisted by OpenCode.
///
/// OpenCode's session-list command intentionally does not report live state.
/// Consequently, this source reports persisted sessions as completed history.
/// A controller that owns an OpenCode server may enrich those records with live
/// state and additional capabilities, but discovery never infers authority.
pub struct OpenCodeSource {
    label: String,
    invocation: OpenCodeInvocation,
    runtime: Runtime,
    runner: Arc<dyn CommandRunner>,
}

impl OpenCodeSource {
    pub fn host(executable: impl Into<String>) -> Self {
        Self {
            label: "OpenCode (host)".into(),
            invocation: OpenCodeInvocation::host(executable),
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
            label: format!("OpenCode ({container_name})"),
            invocation: OpenCodeInvocation::docker(container_id.clone()),
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
        invocation: OpenCodeInvocation,
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

impl SessionSource for OpenCodeSource {
    fn label(&self) -> &str {
        &self.label
    }

    fn discover(&self, request: &DiscoveryRequest) -> Result<Vec<AgentSession>> {
        let mut args = self.invocation.prefix_args.clone();
        args.extend([
            "session".into(),
            "list".into(),
            "--format".into(),
            "json".into(),
        ]);
        let mut command = CommandRequest::new(self.invocation.program.clone(), args);
        command.timeout = Duration::from_secs(8);
        let output = self.runner.run(&command)?;
        if output.status != 0 {
            bail!(
                "opencode session list exited with status {}: {}",
                output.status,
                output.stderr_lossy()
            );
        }

        let mut sessions = parse_opencode_session_list(output.stdout_text()?, self.runtime.clone())?;
        sessions.retain(|session| {
            request.include_completed
                && request
                    .cwd
                    .as_ref()
                    .map(|cwd| session.cwd.starts_with(cwd))
                    .unwrap_or(true)
        });
        Ok(sessions)
    }
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct OpenCodeRecord {
    id: String,
    title: String,
    updated: u64,
    created: u64,
    #[allow(dead_code)]
    project_id: String,
    directory: PathBuf,
}

pub fn parse_opencode_session_list(input: &str, runtime: Runtime) -> Result<Vec<AgentSession>> {
    // OpenCode 1.18 emits no bytes, rather than `[]`, when its store is empty.
    if input.trim().is_empty() {
        return Ok(Vec::new());
    }
    let records: Vec<OpenCodeRecord> = serde_json::from_str(input)
        .context("invalid `opencode session list --format json` output")?;
    Ok(records
        .into_iter()
        .map(|record| normalize_record(record, runtime.clone()))
        .collect())
}

fn normalize_record(record: OpenCodeRecord, runtime: Runtime) -> AgentSession {
    let runtime_id = match &runtime {
        Runtime::Host => "host",
        Runtime::Docker { container_id, .. } => container_id,
    };
    AgentSession {
        id: format!("opencode:{runtime_id}:{}", record.id),
        provider_session_id: record.id,
        provider: Provider::Other("OpenCode".into()),
        runtime,
        kind: SessionKind::Unknown,
        name: record.title.clone(),
        cwd: record.directory,
        // The list command is a history API and exposes no live status.
        state: SessionState::Completed,
        summary: record.title,
        raw_state: Some("persisted".into()),
        pid: None,
        started_at: Some(SystemTime::UNIX_EPOCH + Duration::from_millis(record.created)),
        updated_at: Some(SystemTime::UNIX_EPOCH + Duration::from_millis(record.updated)),
        pull_requests: None,
        capabilities: BTreeSet::from([Capability::Inspect]),
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
    fn parses_current_opencode_json_shape() {
        let input = r#"[{
          "id": "ses_123",
          "title": "Implement the dashboard",
          "updated": 1787089210008,
          "created": 1787089195916,
          "projectId": "global",
          "directory": "/work/project"
        }]"#;

        let sessions = parse_opencode_session_list(input, Runtime::Host).unwrap();

        assert_eq!(sessions.len(), 1);
        assert_eq!(sessions[0].provider, Provider::Other("OpenCode".into()));
        assert_eq!(sessions[0].provider_session_id, "ses_123");
        assert_eq!(sessions[0].state, SessionState::Completed);
        assert_eq!(sessions[0].cwd, PathBuf::from("/work/project"));
        assert_eq!(
            sessions[0].capabilities,
            BTreeSet::from([Capability::Inspect])
        );
    }

    #[test]
    fn accepts_the_empty_store_output_from_opencode() {
        assert!(parse_opencode_session_list("\n", Runtime::Host)
            .unwrap()
            .is_empty());
    }

    #[test]
    fn source_uses_documented_json_session_command_and_filters_cwd() {
        let mut expected = CommandRequest::new(
            "opencode",
            vec![
                "session".into(),
                "list".into(),
                "--format".into(),
                "json".into(),
            ],
        );
        expected.timeout = Duration::from_secs(8);
        let runner = Arc::new(FakeRunner {
            expected,
            output: Mutex::new(Some(CommandOutput {
                status: 0,
                stdout: br#"[{"id":"ses_1","title":"one","updated":2,"created":1,"projectId":"global","directory":"/work/one"},{"id":"ses_2","title":"two","updated":2,"created":1,"projectId":"global","directory":"/else"}]"#.to_vec(),
                stderr: vec![],
            })),
        });
        let source = OpenCodeSource::with_runner(
            "test",
            OpenCodeInvocation::host("opencode"),
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

        assert_eq!(sessions.len(), 1);
        assert_eq!(sessions[0].provider_session_id, "ses_1");
    }

    #[test]
    fn completed_history_respects_include_completed() {
        let mut expected = CommandRequest::new(
            "opencode",
            vec![
                "session".into(),
                "list".into(),
                "--format".into(),
                "json".into(),
            ],
        );
        expected.timeout = Duration::from_secs(8);
        let runner = Arc::new(FakeRunner {
            expected,
            output: Mutex::new(Some(CommandOutput {
                status: 0,
                stdout: br#"[{"id":"ses_1","title":"one","updated":2,"created":1,"projectId":"global","directory":"/work"}]"#.to_vec(),
                stderr: vec![],
            })),
        });
        let source = OpenCodeSource::with_runner(
            "test",
            OpenCodeInvocation::host("opencode"),
            Runtime::Host,
            runner,
        );

        assert!(source
            .discover(&DiscoveryRequest::default())
            .unwrap()
            .is_empty());
    }
}

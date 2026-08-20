use std::collections::{BTreeMap, BTreeSet};
use std::path::PathBuf;
use std::process::{Command, Stdio};
use std::sync::Arc;
use std::time::{Duration, SystemTime};

use anyhow::{bail, Context, Result};
use serde::Deserialize;

use super::{DiscoveryRequest, SessionSource, SourceDiscovery};
use crate::control::{ControlOutcome, LaunchMode, LaunchRequest, ProviderController};
use crate::domain::{
    AgentSession, Capability, Provider, Runtime, SessionKind, SessionSnapshot, SessionState,
};
use crate::opencode_supervisor::{ManagedOpenCodeSession, OpenCodeSupervisor};
use crate::process::{CancellableProcessRunner, CommandRequest, CommandRunner};

// `opencode session list` is workspace-scoped in OpenCode 1.18.18 despite its
// generic help text. The official read-only `db` command is the only current
// CLI surface that projects every root and child session across workspaces.
// Ask SQLite to encode each row separately. OpenCode 1.17 truncates a large
// JSON-array result when stdout is a pipe, while TSV rows stream completely.
// json_object also preserves tabs/newlines in user titles and paths safely.
const GLOBAL_SESSION_ROWS: &str = "SELECT json_object('id', id, 'title', title, 'created', time_created, 'updated', time_updated, 'projectId', project_id, 'directory', directory) AS record FROM session";
const MAX_MODEL_CATALOG_BYTES: usize = 4 * 1024 * 1024;

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
    supervisor: Option<Arc<OpenCodeSupervisor>>,
}

/// Read-only history control plus optional exact owned-server lifecycle.
///
/// Managed HTTP authority comes only from `OpenCodeSupervisor`; it is never
/// inferred from the history commands used by `OpenCodeSource`.
pub struct OpenCodeController {
    executable: String,
    source: OpenCodeSource,
    supervisor: Option<Arc<OpenCodeSupervisor>>,
}

impl OpenCodeController {
    pub fn host(executable: impl Into<String>) -> Self {
        let executable = executable.into();
        Self {
            source: OpenCodeSource::host(executable.clone()),
            executable,
            supervisor: None,
        }
    }

    pub fn managed(executable: impl Into<String>, supervisor: Arc<OpenCodeSupervisor>) -> Self {
        let executable = executable.into();
        Self {
            source: OpenCodeSource::managed(executable.clone(), supervisor.clone()),
            executable,
            supervisor: Some(supervisor),
        }
    }
}

impl ProviderController for OpenCodeController {
    fn provider(&self) -> Provider {
        Provider::OpenCode
    }

    fn launch_mode(&self) -> LaunchMode {
        if self.supervisor.is_some() {
            LaunchMode::SelectableModel
        } else {
            LaunchMode::Unavailable
        }
    }

    fn available_models(&self) -> Result<Vec<String>> {
        self.source.available_models()
    }

    fn enrich(&self, snapshot: &mut SessionSnapshot) {
        let Some(supervisor) = &self.supervisor else {
            return;
        };
        let managed = match supervisor.list() {
            Ok(managed) => managed,
            Err(error) => {
                snapshot
                    .warnings
                    .push(format!("OpenCode managed control: {error:#}"));
                return;
            }
        };
        let managed: BTreeMap<_, _> = managed
            .into_iter()
            .map(|session| (session.id.clone(), session))
            .collect();
        for session in snapshot.sessions.iter_mut().filter(|session| {
            session.provider == Provider::OpenCode && session.runtime == Runtime::Host
        }) {
            let Some(owned) = managed.get(&session.provider_session_id) else {
                continue;
            };
            overlay_managed(session, owned);
            grant_managed_capabilities(session, owned);
        }
    }

    fn launch(&self, request: &LaunchRequest) -> Result<ControlOutcome> {
        if request.provider != Provider::OpenCode {
            bail!("the OpenCode controller cannot launch another provider");
        }
        let session = self
            .supervisor
            .as_ref()
            .context("managed OpenCode launch is not configured")?
            .launch_with_model(&request.prompt, &request.cwd, request.model.as_deref())?;
        Ok(ControlOutcome {
            message: format!("started managed OpenCode session {}", session.title),
            provider_session_hint: Some(session.id),
        })
    }

    fn inspect(&self, session: &AgentSession) -> Result<String> {
        if self.owned_session(session)?.is_some() {
            return self
                .supervisor
                .as_ref()
                .context("managed OpenCode control is not configured")?
                .inspect(&session.provider_session_id);
        }
        self.source.inspect(session)
    }

    fn reply(&self, session: &AgentSession, prompt: &str) -> Result<ControlOutcome> {
        let owned = self.require_owned(session)?;
        if owned.state == SessionState::NeedsInput {
            bail!("the managed OpenCode session needs provider-native recovery");
        }
        self.supervisor
            .as_ref()
            .context("managed OpenCode control is not configured")?
            .reply(&owned.id, prompt)?;
        Ok(ControlOutcome {
            message: format!("sent a reply to OpenCode session {}", session.name),
            provider_session_hint: Some(owned.id),
        })
    }

    fn interrupt(&self, session: &AgentSession) -> Result<ControlOutcome> {
        let owned = self.require_owned(session)?;
        if owned.state != SessionState::Working {
            bail!("the managed OpenCode session is not currently working");
        }
        self.supervisor
            .as_ref()
            .context("managed OpenCode control is not configured")?
            .interrupt(&owned.id)?;
        Ok(ControlOutcome {
            message: format!("interrupted OpenCode session {}", session.name),
            provider_session_hint: Some(owned.id),
        })
    }

    fn open(&self, session: &AgentSession) -> Result<ControlOutcome> {
        if session.provider != Provider::OpenCode || session.runtime != Runtime::Host {
            bail!("the host OpenCode controller does not own this runtime");
        }
        if self.owned_session(session)?.is_some() {
            bail!("a live managed OpenCode session cannot be opened through a second server; use inline controls");
        }
        let status = Command::new(&self.executable)
            .args(["--session", &session.provider_session_id])
            .current_dir(&session.cwd)
            .stdin(Stdio::inherit())
            .stdout(Stdio::inherit())
            .stderr(Stdio::inherit())
            .status()
            .context("failed to open the OpenCode session")?;
        if !status.success() {
            bail!("OpenCode session exited with status {status}");
        }
        Ok(ControlOutcome {
            message: format!("returned from OpenCode session {}", session.name),
            provider_session_hint: Some(session.provider_session_id.clone()),
        })
    }
}

impl OpenCodeController {
    fn owned_session(&self, session: &AgentSession) -> Result<Option<ManagedOpenCodeSession>> {
        if session.provider != Provider::OpenCode || session.runtime != Runtime::Host {
            bail!("the host OpenCode controller does not own this runtime");
        }
        let Some(supervisor) = &self.supervisor else {
            return Ok(None);
        };
        Ok(supervisor
            .list()?
            .into_iter()
            .find(|owned| owned.id == session.provider_session_id))
    }

    fn require_owned(&self, session: &AgentSession) -> Result<ManagedOpenCodeSession> {
        self.owned_session(session)?
            .context("refusing to control an OpenCode session not created by this supervisor")
    }
}

impl OpenCodeSource {
    pub fn host(executable: impl Into<String>) -> Self {
        Self {
            label: "OpenCode (host)".into(),
            invocation: OpenCodeInvocation::host(executable),
            runtime: Runtime::Host,
            runner: Arc::new(CancellableProcessRunner::default()),
            supervisor: None,
        }
    }

    pub fn managed(executable: impl Into<String>, supervisor: Arc<OpenCodeSupervisor>) -> Self {
        Self {
            label: "OpenCode (host)".into(),
            invocation: OpenCodeInvocation::host(executable),
            runtime: Runtime::Host,
            runner: Arc::new(CancellableProcessRunner::default()),
            supervisor: Some(supervisor),
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
            runner: Arc::new(CancellableProcessRunner::default()),
            supervisor: None,
        }
    }

    fn available_models(&self) -> Result<Vec<String>> {
        let mut args = self.invocation.prefix_args.clone();
        args.push("models".into());
        let mut command = CommandRequest::new(self.invocation.program.clone(), args);
        command.timeout = Duration::from_secs(8);
        let output = self.runner.run(&command)?;
        if output.status != 0 {
            bail!(
                "OpenCode model discovery exited with status {}: {}",
                output.status,
                output.stderr_lossy()
            );
        }
        if output.stdout.len() > MAX_MODEL_CATALOG_BYTES {
            bail!("OpenCode model catalog exceeded the 4 MiB safety limit");
        }
        parse_opencode_models(output.stdout_text()?)
    }

    /// Render a persisted session transcript using OpenCode's read-only export
    /// command. This does not attach to, steer, or otherwise mutate a session.
    pub fn inspect(&self, session: &AgentSession) -> Result<String> {
        if session.provider != Provider::OpenCode || session.runtime != self.runtime {
            bail!("the OpenCode source does not own this provider runtime");
        }
        let mut args = self.invocation.prefix_args.clone();
        args.extend(["export".into(), session.provider_session_id.clone()]);
        let mut command = CommandRequest::new(self.invocation.program.clone(), args);
        command.timeout = Duration::from_secs(8);
        let output = self.runner.run(&command)?;
        if output.status != 0 {
            bail!(
                "opencode export exited with status {}: {}",
                output.status,
                output.stderr_lossy()
            );
        }
        render_opencode_export(output.stdout_text()?)
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
            supervisor: None,
        }
    }
}

impl SessionSource for OpenCodeSource {
    fn label(&self) -> &str {
        &self.label
    }

    fn discover(&self, request: &DiscoveryRequest) -> Result<Vec<AgentSession>> {
        Ok(self.discover_with_warnings(request)?.sessions)
    }

    fn discover_with_warnings(&self, request: &DiscoveryRequest) -> Result<SourceDiscovery> {
        let mut sessions = BTreeMap::new();
        let mut warnings = Vec::new();
        // Persisted OpenCode history has no live-state signal and every record
        // normalizes as Completed. Avoid starting the potentially enormous
        // global database query when completed sessions were not requested.
        if request.include_completed {
            let mut args = self.invocation.prefix_args.clone();
            args.extend([
                "db".into(),
                global_session_query(
                    request.history_limit.max(1).saturating_add(1),
                    request.history_oldest_first,
                ),
                "--format".into(),
                "tsv".into(),
            ]);
            let mut command = CommandRequest::new(self.invocation.program.clone(), args);
            command.timeout = Duration::from_secs(8);
            let mut output = self.runner.run(&command)?;
            let mut used_global_query = output.status == 0;
            if output.status != 0 {
                // Older OpenCode builds do not have `db`; retain their supported,
                // though potentially workspace-scoped, session-list behavior.
                let mut args = self.invocation.prefix_args.clone();
                args.extend([
                    "session".into(),
                    "list".into(),
                    "--format".into(),
                    "json".into(),
                ]);
                let mut fallback = CommandRequest::new(self.invocation.program.clone(), args);
                fallback.timeout = Duration::from_secs(8);
                output = self.runner.run(&fallback)?;
                used_global_query = false;
                if output.status != 0 {
                    bail!(
                        "OpenCode global discovery and session-list fallback failed with status {}: {}",
                        output.status,
                        output.stderr_lossy()
                    );
                }
            }

            let mut history = if used_global_query {
                parse_opencode_db_rows(output.stdout_text()?, self.runtime.clone())?
            } else {
                parse_opencode_session_list(output.stdout_text()?, self.runtime.clone())?
            };
            let history_limit = request.history_limit.max(1);
            if history.len() > history_limit {
                history.truncate(history_limit);
                warnings.push(format!(
                    "OpenCode history is limited to {} records for this refresh; increase --history-limit to load more",
                    history_limit
                ));
            }
            sessions.extend(
                history
                    .into_iter()
                    .filter(|session| {
                        request
                            .cwd
                            .as_ref()
                            .map(|cwd| session.cwd.starts_with(cwd))
                            .unwrap_or(true)
                    })
                    .map(|session| (session.provider_session_id.clone(), session)),
            );
        }
        if let Some(supervisor) = &self.supervisor {
            for managed in supervisor.list()? {
                let session = agent_session_from_managed(&managed);
                if (request.include_completed || session.state != SessionState::Completed)
                    && request
                        .cwd
                        .as_ref()
                        .map(|cwd| session.cwd.starts_with(cwd))
                        .unwrap_or(true)
                {
                    sessions.insert(session.provider_session_id.clone(), session);
                }
            }
        }
        Ok(SourceDiscovery {
            sessions: sessions.into_values().collect(),
            warnings,
        })
    }

    fn cancel(&self) {
        self.runner.cancel();
    }
}

fn global_session_query(limit: usize, oldest_first: bool) -> String {
    format!(
        "{GLOBAL_SESSION_ROWS} ORDER BY time_updated {} LIMIT {}",
        if oldest_first { "ASC" } else { "DESC" },
        limit.max(1)
    )
}

fn parse_opencode_db_rows(input: &str, runtime: Runtime) -> Result<Vec<AgentSession>> {
    let mut lines = input.lines();
    let Some(header) = lines.next() else {
        return Ok(Vec::new());
    };
    if header.trim_end_matches('\r') != "record" {
        bail!("invalid OpenCode db TSV header");
    }
    lines
        .filter(|line| !line.trim().is_empty())
        .enumerate()
        .map(|(index, line)| {
            let record: OpenCodeRecord = serde_json::from_str(line)
                .with_context(|| format!("invalid OpenCode db record on row {}", index + 2))?;
            Ok(normalize_record(record, runtime.clone()))
        })
        .collect()
}

fn agent_session_from_managed(managed: &ManagedOpenCodeSession) -> AgentSession {
    AgentSession {
        id: format!("opencode:host:{}", managed.id),
        provider_session_id: managed.id.clone(),
        provider: Provider::OpenCode,
        runtime: Runtime::Host,
        kind: SessionKind::Managed,
        name: managed.title.clone(),
        cwd: managed.cwd.clone(),
        state: managed.state,
        summary: managed.summary.clone(),
        raw_state: Some("managed_server".into()),
        pid: Some(managed.server_pid),
        started_at: Some(SystemTime::UNIX_EPOCH + Duration::from_millis(managed.created_at_ms)),
        updated_at: Some(SystemTime::UNIX_EPOCH + Duration::from_millis(managed.updated_at_ms)),
        pull_requests: None,
        capabilities: BTreeSet::from([Capability::Inspect]),
    }
}

fn overlay_managed(session: &mut AgentSession, managed: &ManagedOpenCodeSession) {
    session.kind = SessionKind::Managed;
    session.name = managed.title.clone();
    session.cwd = managed.cwd.clone();
    session.state = managed.state;
    session.summary = managed.summary.clone();
    session.raw_state = Some("managed_server".into());
    session.pid = Some(managed.server_pid);
    session.started_at =
        Some(SystemTime::UNIX_EPOCH + Duration::from_millis(managed.created_at_ms));
    session.updated_at =
        Some(SystemTime::UNIX_EPOCH + Duration::from_millis(managed.updated_at_ms));
}

fn grant_managed_capabilities(session: &mut AgentSession, managed: &ManagedOpenCodeSession) {
    session.capabilities.clear();
    session.capabilities.insert(Capability::Inspect);
    match managed.state {
        SessionState::Working => {
            session.capabilities.insert(Capability::Reply);
            session.capabilities.insert(Capability::Interrupt);
        }
        SessionState::Completed | SessionState::ReadyForReview => {
            session.capabilities.insert(Capability::Reply);
        }
        SessionState::NeedsInput | SessionState::Unknown => {}
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
    let records: Vec<OpenCodeRecord> =
        serde_json::from_str(input).context("invalid OpenCode session-list JSON")?;
    Ok(records
        .into_iter()
        .map(|record| normalize_record(record, runtime.clone()))
        .collect())
}

fn render_opencode_export(input: &str) -> Result<String> {
    let value: serde_json::Value =
        serde_json::from_str(input).context("invalid `opencode export` output")?;
    let messages = value
        .get("messages")
        .and_then(serde_json::Value::as_array)
        .context("opencode export omitted messages")?;
    let mut transcript = Vec::new();
    for message in messages {
        let role = message
            .pointer("/info/role")
            .and_then(serde_json::Value::as_str)
            .unwrap_or("event");
        let text = message
            .get("parts")
            .and_then(serde_json::Value::as_array)
            .into_iter()
            .flatten()
            .filter_map(|part| {
                (part.get("type").and_then(serde_json::Value::as_str) == Some("text"))
                    .then(|| part.get("text").and_then(serde_json::Value::as_str))
                    .flatten()
            })
            .collect::<Vec<_>>()
            .join("\n");
        if !text.trim().is_empty() {
            transcript.push(format!("{}: {}", capitalize(role), text.trim()));
        }
    }
    Ok(limit_transcript(if transcript.is_empty() {
        "No text messages are available in this OpenCode session.".into()
    } else {
        transcript.join("\n\n")
    }))
}

fn capitalize(value: &str) -> String {
    let mut characters = value.chars();
    match characters.next() {
        Some(first) => first.to_uppercase().chain(characters).collect(),
        None => String::new(),
    }
}

fn limit_transcript(mut value: String) -> String {
    const MAX_CHARS: usize = 32 * 1024;
    if value.chars().count() <= MAX_CHARS {
        return value;
    }
    value = value
        .chars()
        .rev()
        .take(MAX_CHARS.saturating_sub(24))
        .collect::<String>()
        .chars()
        .rev()
        .collect();
    format!("[earlier output omitted]\n{value}")
}

fn parse_opencode_models(input: &str) -> Result<Vec<String>> {
    let mut models = BTreeSet::new();
    for (index, line) in input.lines().enumerate() {
        let identifier = line.trim();
        if identifier.is_empty() {
            continue;
        }
        let Some((provider, model)) = identifier.split_once('/') else {
            bail!("OpenCode model catalog row {} is malformed", index + 1);
        };
        if provider.is_empty()
            || model.is_empty()
            || identifier.len() > 128
            || identifier
                .chars()
                .any(|character| character.is_control() || character.is_whitespace())
        {
            bail!("OpenCode model catalog row {} is invalid", index + 1);
        }
        models.insert(identifier.to_owned());
        if models.len() > 20_000 {
            bail!("OpenCode model catalog exceeded the 20,000-model safety limit");
        }
    }
    Ok(models.into_iter().collect())
}

fn normalize_record(record: OpenCodeRecord, runtime: Runtime) -> AgentSession {
    let runtime_id = match &runtime {
        Runtime::Host => "host",
        Runtime::Docker { container_id, .. } => container_id,
    };
    let capabilities = if runtime == Runtime::Host {
        BTreeSet::from([Capability::Inspect])
    } else {
        // A host controller cannot safely route inspection into an arbitrary
        // container. Explicit Docker control needs its own enrolled controller.
        BTreeSet::new()
    };
    AgentSession {
        id: format!("opencode:{runtime_id}:{}", record.id),
        provider_session_id: record.id,
        provider: Provider::OpenCode,
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
        capabilities,
    }
}

#[cfg(test)]
mod tests {
    #[cfg(unix)]
    use std::os::unix::fs::PermissionsExt;
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
        assert_eq!(sessions[0].provider, Provider::OpenCode);
        assert_eq!(sessions[0].provider_session_id, "ses_123");
        assert_eq!(sessions[0].state, SessionState::Completed);
        assert_eq!(sessions[0].cwd, PathBuf::from("/work/project"));
        assert_eq!(
            sessions[0].capabilities,
            BTreeSet::from([Capability::Inspect])
        );
    }

    #[test]
    fn parses_exact_opencode_model_identifiers() {
        assert_eq!(
            parse_opencode_models("openai/gpt-5.4\nanthropic/claude-sonnet-4-5\nopenai/gpt-5.4\n")
                .unwrap(),
            vec!["anthropic/claude-sonnet-4-5", "openai/gpt-5.4"]
        );
        assert!(parse_opencode_models("gpt-5.4\n").is_err());
        assert!(parse_opencode_models("openai/\n").is_err());
    }

    #[test]
    fn controller_uses_the_documented_opencode_models_command() {
        let mut expected = CommandRequest::new("opencode", vec!["models".into()]);
        expected.timeout = Duration::from_secs(8);
        let runner = Arc::new(FakeRunner {
            expected,
            output: Mutex::new(Some(CommandOutput {
                status: 0,
                stdout: b"openai/gpt-5.4\nanthropic/claude-sonnet-4-5\n".to_vec(),
                stderr: Vec::new(),
            })),
        });
        let source = OpenCodeSource::with_runner(
            "test",
            OpenCodeInvocation::host("opencode"),
            Runtime::Host,
            runner,
        );
        let controller = OpenCodeController {
            executable: "opencode".into(),
            source,
            supervisor: None,
        };

        assert_eq!(
            controller.available_models().unwrap(),
            vec!["anthropic/claude-sonnet-4-5", "openai/gpt-5.4"]
        );
    }

    #[test]
    fn accepts_the_empty_store_output_from_opencode() {
        assert!(parse_opencode_session_list("\n", Runtime::Host)
            .unwrap()
            .is_empty());
    }

    #[test]
    fn docker_history_does_not_claim_host_inspection_authority() {
        let runtime = Runtime::Docker {
            container_id: "sha256:exact".into(),
            container_name: "isolated".into(),
            image: "opencode@test".into(),
        };
        let session = parse_opencode_session_list(
            r#"[{"id":"ses_1","title":"one","updated":2,"created":1,"projectId":"global","directory":"/work"}]"#,
            runtime,
        )
        .unwrap()
        .remove(0);

        assert!(session.capabilities.is_empty());
    }

    #[test]
    fn source_uses_bounded_streaming_db_rows_and_filters_cwd() {
        let mut expected = CommandRequest::new(
            "opencode",
            vec![
                "db".into(),
                global_session_query(101, false),
                "--format".into(),
                "tsv".into(),
            ],
        );
        expected.timeout = Duration::from_secs(8);
        let runner = Arc::new(FakeRunner {
            expected,
            output: Mutex::new(Some(CommandOutput {
                status: 0,
                stdout: b"record\n{\"id\":\"ses_1\",\"title\":\"one\",\"updated\":2,\"created\":1,\"projectId\":\"global\",\"directory\":\"/work/one\"}\n{\"id\":\"ses_2\",\"title\":\"two\",\"updated\":2,\"created\":1,\"projectId\":\"global\",\"directory\":\"/else\"}\n".to_vec(),
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
                ..DiscoveryRequest::default()
            })
            .unwrap();

        assert_eq!(sessions.len(), 1);
        assert_eq!(sessions[0].provider_session_id, "ses_1");
    }

    #[test]
    fn source_returns_a_nonfatal_warning_when_more_history_exists() {
        let mut expected = CommandRequest::new(
            "opencode",
            vec![
                "db".into(),
                global_session_query(3, false),
                "--format".into(),
                "tsv".into(),
            ],
        );
        expected.timeout = Duration::from_secs(8);
        let rows = (1..=3)
            .map(|id| {
                format!(
                    "{{\"id\":\"ses_{id}\",\"title\":\"row {id}\",\"updated\":{id},\"created\":1,\"projectId\":\"global\",\"directory\":\"/work\"}}"
                )
            })
            .collect::<Vec<_>>()
            .join("\n");
        let runner = Arc::new(FakeRunner {
            expected,
            output: Mutex::new(Some(CommandOutput {
                status: 0,
                stdout: format!("record\n{rows}\n").into_bytes(),
                stderr: vec![],
            })),
        });
        let source = OpenCodeSource::with_runner(
            "test",
            OpenCodeInvocation::host("opencode"),
            Runtime::Host,
            runner,
        );

        let result = source
            .discover_with_warnings(&DiscoveryRequest {
                include_completed: true,
                history_limit: 2,
                ..DiscoveryRequest::default()
            })
            .unwrap();

        assert_eq!(result.sessions.len(), 2);
        assert_eq!(result.warnings.len(), 1);
        assert!(result.warnings[0].contains("limited to 2 records"));
    }

    #[test]
    fn inspect_uses_export_and_formats_text_messages() {
        let mut expected = CommandRequest::new("opencode", vec!["export".into(), "ses_1".into()]);
        expected.timeout = Duration::from_secs(8);
        let runner = Arc::new(FakeRunner {
            expected,
            output: Mutex::new(Some(CommandOutput {
                status: 0,
                stdout: br#"{"info":{"id":"ses_1"},"messages":[{"info":{"role":"user"},"parts":[{"type":"text","text":"Build it"}]},{"info":{"role":"assistant"},"parts":[{"type":"text","text":"Done"}]}]}"#.to_vec(),
                stderr: b"Exporting session: ses_1".to_vec(),
            })),
        });
        let source = OpenCodeSource::with_runner(
            "test",
            OpenCodeInvocation::host("opencode"),
            Runtime::Host,
            runner,
        );
        let session = parse_opencode_session_list(
            r#"[{"id":"ses_1","title":"one","updated":2,"created":1,"projectId":"global","directory":"/work"}]"#,
            Runtime::Host,
        )
        .unwrap()
        .remove(0);

        assert_eq!(
            source.inspect(&session).unwrap(),
            "User: Build it\n\nAssistant: Done"
        );
    }

    #[cfg(unix)]
    #[test]
    fn controller_opens_the_exact_native_session() {
        let directory = tempfile::tempdir().unwrap();
        let executable = directory.path().join("opencode-test");
        std::fs::write(
            &executable,
            "#!/bin/sh\n[ \"$1\" = --session ] && [ \"$2\" = ses_1 ]\n",
        )
        .unwrap();
        let mut permissions = std::fs::metadata(&executable).unwrap().permissions();
        permissions.set_mode(0o700);
        std::fs::set_permissions(&executable, permissions).unwrap();
        let mut session = parse_opencode_session_list(
            r#"[{"id":"ses_1","title":"one","updated":2,"created":1,"projectId":"global","directory":"/work"}]"#,
            Runtime::Host,
        )
        .unwrap()
        .remove(0);
        session.cwd = directory.path().to_path_buf();
        let controller = OpenCodeController::host(executable.display().to_string());

        let outcome = controller.open(&session).unwrap();

        assert_eq!(outcome.provider_session_hint, Some("ses_1".into()));
    }

    #[test]
    fn completed_history_respects_include_completed() {
        let mut expected = CommandRequest::new(
            "opencode",
            vec![
                "db".into(),
                global_session_query(101, false),
                "--format".into(),
                "tsv".into(),
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
            runner.clone(),
        );

        assert!(source
            .discover(&DiscoveryRequest::default())
            .unwrap()
            .is_empty());
        assert!(
            runner.output.lock().unwrap().is_some(),
            "completed-history discovery should not run at all when it is hidden"
        );
    }
}

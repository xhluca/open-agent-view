use std::collections::{BTreeMap, BTreeSet};
use std::fs::{self, OpenOptions};
use std::io::Write;
use std::path::PathBuf;
use std::process::{Command, Stdio};
use std::sync::{Arc, Mutex};
use std::time::{Duration, SystemTime};

use anyhow::{bail, Context, Result};
use serde::{Deserialize, Serialize};

use crate::codex_supervisor::{CodexReplyMode, CodexSupervisor};
use crate::domain::{AgentSession, Capability, Provider, Runtime, SessionSnapshot};
use crate::process::{CommandRequest, CommandRunner, ProcessRunner};

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct LaunchRequest {
    pub provider: Provider,
    pub prompt: String,
    pub cwd: PathBuf,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ControlOutcome {
    pub message: String,
    pub provider_session_hint: Option<String>,
}

pub struct ControlHubConfig {
    pub claude_enabled: bool,
    pub codex_enabled: bool,
    pub claude_bin: String,
    pub codex_bin: String,
    pub docker_bin: String,
    pub launch_provider: Provider,
    pub launch_cwd: PathBuf,
    pub provider_io_enabled: bool,
}

/// Provider-specific lifecycle operations registered with the shared dashboard.
///
/// Implementations must grant capabilities only for sessions they can prove they
/// own. The hub checks those capabilities again before dispatching mutations.
pub trait ProviderController: Send + Sync {
    fn provider(&self) -> Provider;

    fn enrich(&self, _snapshot: &mut SessionSnapshot) {}

    fn launch(&self, _request: &LaunchRequest) -> Result<ControlOutcome> {
        bail!("{} launch is unavailable", self.provider().label())
    }

    fn interrupt(&self, _session: &AgentSession) -> Result<ControlOutcome> {
        bail!("{} interrupt is unavailable", self.provider().label())
    }

    fn inspect(&self, _session: &AgentSession) -> Result<String> {
        bail!("{} inspection is unavailable", self.provider().label())
    }

    fn reply(&self, _session: &AgentSession, _prompt: &str) -> Result<ControlOutcome> {
        bail!("{} inline reply is unavailable", self.provider().label())
    }

    fn archive(&self, _session: &AgentSession) -> Result<ControlOutcome> {
        bail!("{} archive is unavailable", self.provider().label())
    }

    fn resolve_approval(&self, _session: &AgentSession, _accept: bool) -> Result<ControlOutcome> {
        bail!("{} inline approval is unavailable", self.provider().label())
    }

    fn respond_input(&self, _session: &AgentSession, _answer: &str) -> Result<ControlOutcome> {
        bail!(
            "{} structured input is unavailable",
            self.provider().label()
        )
    }

    fn delete(&self, _session: &AgentSession) -> Result<ControlOutcome> {
        bail!("{} delete is unavailable", self.provider().label())
    }

    fn open(&self, _session: &AgentSession) -> Result<ControlOutcome> {
        bail!("{} native open is unavailable", self.provider().label())
    }
}

#[derive(Clone)]
pub struct ControlHub {
    controllers: BTreeMap<Provider, Arc<dyn ProviderController>>,
    codex: Option<Arc<CodexSupervisor>>,
    launch_provider: Provider,
    launch_cwd: PathBuf,
    provider_io_enabled: bool,
}

impl ControlHub {
    pub fn new(config: ControlHubConfig) -> Result<Self> {
        let mut controllers: BTreeMap<Provider, Arc<dyn ProviderController>> = BTreeMap::new();
        if config.provider_io_enabled && config.claude_enabled {
            let registry = OwnershipRegistry::load(default_registry_path()?)?;
            controllers.insert(
                Provider::Claude,
                Arc::new(ClaudeController::host(
                    config.claude_bin.clone(),
                    config.docker_bin.clone(),
                    registry,
                )),
            );
        }
        let codex = if config.provider_io_enabled && config.codex_enabled {
            let supervisor = Arc::new(CodexSupervisor::host(config.codex_bin.clone())?);
            controllers.insert(
                Provider::Codex,
                Arc::new(CodexController {
                    supervisor: supervisor.clone(),
                    codex_bin: config.codex_bin.clone(),
                    docker_bin: config.docker_bin.clone(),
                }),
            );
            Some(supervisor)
        } else {
            None
        };
        Ok(Self {
            controllers,
            codex,
            launch_provider: config.launch_provider,
            launch_cwd: config.launch_cwd,
            provider_io_enabled: config.provider_io_enabled,
        })
    }

    pub fn register_controller(&mut self, controller: Arc<dyn ProviderController>) -> Result<()> {
        let provider = controller.provider();
        if self.controllers.contains_key(&provider) {
            bail!("a {} controller is already registered", provider.label());
        }
        self.controllers.insert(provider, controller);
        Ok(())
    }

    pub fn enrich(&self, snapshot: &mut SessionSnapshot) {
        for controller in self.controllers.values() {
            controller.enrich(snapshot);
        }
    }

    pub fn codex_supervisor(&self) -> Option<Arc<CodexSupervisor>> {
        self.codex.clone()
    }

    pub fn launch(&self, prompt: String) -> Result<ControlOutcome> {
        self.ensure_provider_io()?;
        let request = LaunchRequest {
            provider: self.launch_provider.clone(),
            prompt,
            cwd: self.launch_cwd.clone(),
        };
        self.controller(&request.provider)?.launch(&request)
    }

    pub fn interrupt(&self, session: &AgentSession) -> Result<ControlOutcome> {
        self.ensure_provider_io()?;
        if !session.capabilities.contains(&Capability::Interrupt) {
            bail!("interrupt authority was not granted for this session");
        }
        self.controller(&session.provider)?.interrupt(session)
    }

    pub fn inspect(&self, session: &AgentSession) -> Result<String> {
        self.ensure_provider_io()?;
        if !session.capabilities.contains(&Capability::Inspect) {
            bail!("inspection authority was not granted for this session");
        }
        self.controller(&session.provider)?.inspect(session)
    }

    pub fn reply(&self, session: &AgentSession, prompt: &str) -> Result<ControlOutcome> {
        self.ensure_provider_io()?;
        if !session.capabilities.contains(&Capability::Reply) {
            bail!("reply authority was not granted for this session");
        }
        self.controller(&session.provider)?.reply(session, prompt)
    }

    pub fn archive(&self, session: &AgentSession) -> Result<ControlOutcome> {
        self.ensure_provider_io()?;
        if !session.capabilities.contains(&Capability::Archive) {
            bail!("archive authority was not granted for this session");
        }
        self.controller(&session.provider)?.archive(session)
    }

    pub fn resolve_approval(&self, session: &AgentSession, accept: bool) -> Result<ControlOutcome> {
        self.ensure_provider_io()?;
        let required = if accept {
            Capability::Approve
        } else {
            Capability::Decline
        };
        if !session.capabilities.contains(&required) {
            bail!("inline approval authority was not granted for this session");
        }
        self.controller(&session.provider)?
            .resolve_approval(session, accept)
    }

    pub fn respond_input(&self, session: &AgentSession, answer: &str) -> Result<ControlOutcome> {
        self.ensure_provider_io()?;
        if !session.capabilities.contains(&Capability::Respond) {
            bail!("structured-input authority was not granted for this session");
        }
        self.controller(&session.provider)?
            .respond_input(session, answer)
    }

    pub fn delete(&self, session: &AgentSession) -> Result<ControlOutcome> {
        self.ensure_provider_io()?;
        if !session.capabilities.contains(&Capability::Delete) {
            bail!("delete authority was not granted for this session");
        }
        self.controller(&session.provider)?.delete(session)
    }

    pub fn open(&self, session: &AgentSession) -> Result<ControlOutcome> {
        self.ensure_provider_io()?;
        self.controller(&session.provider)?.open(session)
    }

    fn controller(&self, provider: &Provider) -> Result<&Arc<dyn ProviderController>> {
        self.controllers
            .get(provider)
            .with_context(|| format!("no {} controller is configured", provider.label()))
    }

    fn ensure_provider_io(&self) -> Result<()> {
        if !self.provider_io_enabled {
            bail!("provider actions are disabled while reading a fixture");
        }
        Ok(())
    }
}

struct ClaudeController {
    invocation: ControlInvocation,
    docker_bin: String,
    runtime: Runtime,
    runner: Arc<dyn CommandRunner>,
    registry: Arc<Mutex<OwnershipRegistry>>,
}

impl ClaudeController {
    fn host(executable: String, docker_bin: String, registry: OwnershipRegistry) -> Self {
        Self {
            invocation: ControlInvocation {
                program: executable,
                prefix_args: Vec::new(),
            },
            docker_bin,
            runtime: Runtime::Host,
            runner: Arc::new(ProcessRunner),
            registry: Arc::new(Mutex::new(registry)),
        }
    }

    fn launch(&self, request: &LaunchRequest) -> Result<ControlOutcome> {
        if request.prompt.trim().is_empty() {
            bail!("the launch prompt cannot be empty");
        }
        let mut args = self.invocation.prefix_args.clone();
        args.extend(["--background".into(), request.prompt.trim().to_owned()]);
        let mut command = CommandRequest::new(self.invocation.program.clone(), args);
        command.current_dir = Some(request.cwd.clone());
        command.timeout = Duration::from_secs(15);
        let output = self.runner.run(&command)?;
        if output.status != 0 {
            bail!(
                "Claude background launch exited with status {}: {}",
                output.status,
                output.stderr_lossy()
            );
        }
        let stdout = output.stdout_text()?.trim();
        let short_id = parse_background_id(stdout)
            .context("Claude launch succeeded but did not return a background session ID")?;
        self.registry
            .lock()
            .map_err(|_| anyhow::anyhow!("ownership registry lock was poisoned"))?
            .record(OwnedSession {
                provider: Provider::Claude,
                runtime_key: runtime_key(&self.runtime),
                provider_id_prefix: short_id.clone(),
                created_at_ms: now_millis(),
            })?;

        Ok(ControlOutcome {
            message: format!("started Claude background session {short_id}"),
            provider_session_hint: Some(short_id),
        })
    }

    fn stop(&self, session: &AgentSession) -> Result<ControlOutcome> {
        if !self.owns(session) {
            bail!("refusing to stop a Claude session not launched by coding-agents");
        }
        if session.runtime != self.runtime {
            bail!("the configured controller does not own this runtime");
        }
        let short_id = short_claude_id(&session.provider_session_id);
        let mut args = self.invocation.prefix_args.clone();
        args.extend(["stop".into(), short_id.clone()]);
        let mut command = CommandRequest::new(self.invocation.program.clone(), args);
        command.timeout = Duration::from_secs(8);
        let output = self.runner.run(&command)?;
        if output.status != 0 {
            bail!(
                "Claude stop exited with status {}: {}",
                output.status,
                output.stderr_lossy()
            );
        }
        Ok(ControlOutcome {
            message: format!("stopped Claude background session {short_id}"),
            provider_session_hint: Some(short_id),
        })
    }

    fn owns(&self, session: &AgentSession) -> bool {
        session.provider == Provider::Claude
            && self
                .registry
                .lock()
                .map(|registry| registry.owns(session))
                .unwrap_or(false)
    }
}

impl ProviderController for ClaudeController {
    fn provider(&self) -> Provider {
        Provider::Claude
    }

    fn enrich(&self, snapshot: &mut SessionSnapshot) {
        for session in &mut snapshot.sessions {
            if self.owns(session) {
                session.capabilities.insert(Capability::Interrupt);
            }
        }
    }

    fn launch(&self, request: &LaunchRequest) -> Result<ControlOutcome> {
        ClaudeController::launch(self, request)
    }

    fn interrupt(&self, session: &AgentSession) -> Result<ControlOutcome> {
        self.stop(session)
    }

    fn inspect(&self, session: &AgentSession) -> Result<String> {
        let mut request = match &session.runtime {
            Runtime::Host => CommandRequest::new(
                self.invocation.program.clone(),
                vec!["logs".into(), short_claude_id(&session.provider_session_id)],
            ),
            Runtime::Docker { container_id, .. } => CommandRequest::new(
                self.docker_bin.clone(),
                vec![
                    "exec".into(),
                    container_id.clone(),
                    "claude".into(),
                    "logs".into(),
                    short_claude_id(&session.provider_session_id),
                ],
            ),
        };
        request.timeout = Duration::from_secs(8);
        read_command_output(&self.runner.run(&request)?, "Claude logs")
    }

    fn open(&self, session: &AgentSession) -> Result<ControlOutcome> {
        let mut command = match &session.runtime {
            Runtime::Host => {
                let mut command = Command::new(&self.invocation.program);
                command
                    .args(["attach", &short_claude_id(&session.provider_session_id)])
                    .current_dir(&session.cwd);
                command
            }
            Runtime::Docker { container_id, .. } => {
                let mut command = Command::new(&self.docker_bin);
                command.args([
                    "exec",
                    "-it",
                    container_id,
                    "claude",
                    "attach",
                    &short_claude_id(&session.provider_session_id),
                ]);
                command
            }
        };
        run_interactive(&mut command, session)
    }
}

struct CodexController {
    supervisor: Arc<CodexSupervisor>,
    codex_bin: String,
    docker_bin: String,
}

impl ProviderController for CodexController {
    fn provider(&self) -> Provider {
        Provider::Codex
    }

    fn enrich(&self, snapshot: &mut SessionSnapshot) {
        self.supervisor.enrich(snapshot);
    }

    fn launch(&self, request: &LaunchRequest) -> Result<ControlOutcome> {
        let thread_id = self.supervisor.launch(&request.prompt, &request.cwd)?;
        Ok(ControlOutcome {
            message: format!("started managed Codex thread {thread_id}"),
            provider_session_hint: Some(thread_id),
        })
    }

    fn interrupt(&self, session: &AgentSession) -> Result<ControlOutcome> {
        self.supervisor.interrupt(session)?;
        Ok(ControlOutcome {
            message: format!(
                "interrupted managed Codex thread {}",
                session.provider_session_id
            ),
            provider_session_hint: Some(session.provider_session_id.clone()),
        })
    }

    fn inspect(&self, session: &AgentSession) -> Result<String> {
        match session.runtime {
            Runtime::Host => self.supervisor.inspect(session),
            Runtime::Docker { .. } => {
                bail!("Docker Codex transcript inspection is observe-only")
            }
        }
    }

    fn reply(&self, session: &AgentSession, prompt: &str) -> Result<ControlOutcome> {
        if session.runtime != Runtime::Host {
            bail!("inline reply is supported only for owned host Codex threads");
        }
        let mode = self.supervisor.reply(session, prompt)?;
        let verb = match mode {
            CodexReplyMode::Started => "started a new turn for",
            CodexReplyMode::Steered => "steered the active turn for",
        };
        Ok(ControlOutcome {
            message: format!(
                "{verb} managed Codex thread {}",
                session.provider_session_id
            ),
            provider_session_hint: Some(session.provider_session_id.clone()),
        })
    }

    fn archive(&self, session: &AgentSession) -> Result<ControlOutcome> {
        self.supervisor.archive(session)?;
        Ok(ControlOutcome {
            message: format!(
                "archived managed Codex thread {}",
                session.provider_session_id
            ),
            provider_session_hint: Some(session.provider_session_id.clone()),
        })
    }

    fn resolve_approval(&self, session: &AgentSession, accept: bool) -> Result<ControlOutcome> {
        if session.runtime != Runtime::Host {
            bail!("inline approvals are supported only for owned host Codex threads");
        }
        self.supervisor.respond_approval(session, accept)?;
        Ok(ControlOutcome {
            message: format!(
                "sent {} for managed Codex thread {}; awaiting provider resolution",
                if accept {
                    "one-time approval"
                } else {
                    "denial"
                },
                session.provider_session_id
            ),
            provider_session_hint: Some(session.provider_session_id.clone()),
        })
    }

    fn respond_input(&self, session: &AgentSession, answer: &str) -> Result<ControlOutcome> {
        if session.runtime != Runtime::Host {
            bail!("structured input is supported only for owned host Codex threads");
        }
        let progress = self.supervisor.respond_user_input(session, answer)?;
        let message = if progress.submitted {
            format!(
                "submitted {}/{} Codex answers; awaiting provider resolution",
                progress.answered, progress.total
            )
        } else {
            format!(
                "recorded Codex answer {}/{}; answer the next question",
                progress.answered, progress.total
            )
        };
        Ok(ControlOutcome {
            message,
            provider_session_hint: Some(session.provider_session_id.clone()),
        })
    }

    fn delete(&self, session: &AgentSession) -> Result<ControlOutcome> {
        self.supervisor.delete(session)?;
        Ok(ControlOutcome {
            message: format!(
                "deleted managed Codex thread {}",
                session.provider_session_id
            ),
            provider_session_hint: Some(session.provider_session_id.clone()),
        })
    }

    fn open(&self, session: &AgentSession) -> Result<ControlOutcome> {
        let remote = self.supervisor.remote_url_if_owned(session);
        let mut command = match &session.runtime {
            Runtime::Host => {
                let mut command = Command::new(&self.codex_bin);
                if let Some(remote) = remote.as_deref() {
                    command.args(["--remote", remote, "resume", &session.provider_session_id]);
                } else {
                    command.args(["resume", &session.provider_session_id]);
                }
                command.current_dir(&session.cwd);
                command
            }
            Runtime::Docker { container_id, .. } => {
                let mut command = Command::new(&self.docker_bin);
                command.args([
                    "exec",
                    "-it",
                    container_id,
                    "codex",
                    "resume",
                    &session.provider_session_id,
                ]);
                command
            }
        };
        run_interactive(&mut command, session)
    }
}

fn run_interactive(command: &mut Command, session: &AgentSession) -> Result<ControlOutcome> {
    let status = command
        .stdin(Stdio::inherit())
        .stdout(Stdio::inherit())
        .stderr(Stdio::inherit())
        .status()
        .context("failed to open provider session")?;
    if !status.success() {
        bail!("provider session exited with status {status}");
    }
    Ok(ControlOutcome {
        message: format!("returned from {}", session.name),
        provider_session_hint: None,
    })
}

struct ControlInvocation {
    program: String,
    prefix_args: Vec<String>,
}

#[derive(Clone, Debug, Deserialize, Eq, Ord, PartialEq, PartialOrd, Serialize)]
struct OwnedSession {
    provider: Provider,
    runtime_key: String,
    provider_id_prefix: String,
    created_at_ms: u64,
}

#[derive(Debug)]
struct OwnershipRegistry {
    path: PathBuf,
    records: BTreeSet<OwnedSession>,
}

impl OwnershipRegistry {
    fn load(path: PathBuf) -> Result<Self> {
        let records = match fs::read_to_string(&path) {
            Ok(input) => serde_json::from_str(&input)
                .with_context(|| format!("invalid ownership registry {}", path.display()))?,
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => BTreeSet::new(),
            Err(error) => {
                return Err(error).with_context(|| {
                    format!("failed to read ownership registry {}", path.display())
                })
            }
        };
        Ok(Self { path, records })
    }

    fn record(&mut self, record: OwnedSession) -> Result<()> {
        self.records.replace(record);
        self.save()
    }

    fn owns(&self, session: &AgentSession) -> bool {
        let runtime = runtime_key(&session.runtime);
        self.records.iter().any(|record| {
            record.provider == session.provider
                && record.runtime_key == runtime
                && session
                    .provider_session_id
                    .starts_with(&record.provider_id_prefix)
        })
    }

    fn save(&self) -> Result<()> {
        let parent = self
            .path
            .parent()
            .context("ownership registry path has no parent")?;
        fs::create_dir_all(parent)
            .with_context(|| format!("failed to create {}", parent.display()))?;
        let temporary = self
            .path
            .with_extension(format!("tmp-{}", std::process::id()));
        let bytes = serde_json::to_vec_pretty(&self.records)?;
        let mut options = OpenOptions::new();
        options.create(true).truncate(true).write(true);
        #[cfg(unix)]
        {
            use std::os::unix::fs::OpenOptionsExt;
            options.mode(0o600);
        }
        let mut file = options
            .open(&temporary)
            .with_context(|| format!("failed to write {}", temporary.display()))?;
        file.write_all(&bytes)?;
        file.sync_all()?;
        fs::rename(&temporary, &self.path).with_context(|| {
            format!(
                "failed to replace ownership registry {}",
                self.path.display()
            )
        })?;
        Ok(())
    }
}

fn default_registry_path() -> Result<PathBuf> {
    if let Some(state_home) = std::env::var_os("XDG_STATE_HOME") {
        return Ok(PathBuf::from(state_home)
            .join("open-agent-view")
            .join("ownership.json"));
    }
    let home = std::env::var_os("HOME").context("HOME is not set")?;
    Ok(PathBuf::from(home).join(".local/state/open-agent-view/ownership.json"))
}

fn runtime_key(runtime: &Runtime) -> String {
    match runtime {
        Runtime::Host => "host".into(),
        Runtime::Docker { container_id, .. } => format!("docker:{container_id}"),
    }
}

fn parse_background_id(output: &str) -> Option<String> {
    output.lines().find_map(|line| {
        let mut parts = line.split('·').map(str::trim);
        (parts.next()? == "backgrounded")
            .then(|| parts.next().map(ToOwned::to_owned))
            .flatten()
    })
}

fn short_claude_id(provider_session_id: &str) -> String {
    provider_session_id.chars().take(8).collect()
}

fn now_millis() -> u64 {
    SystemTime::now()
        .duration_since(SystemTime::UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis() as u64
}

fn read_command_output(output: &crate::process::CommandOutput, operation: &str) -> Result<String> {
    if output.status != 0 {
        bail!(
            "{operation} exited with status {}: {}",
            output.status,
            output.stderr_lossy()
        );
    }
    let text = if output.stdout.contains(&0x1b) {
        recent_terminal_screen(&output.stdout)
    } else {
        output.stdout_text()?.trim().to_owned()
    };
    Ok(if text.is_empty() {
        "No recent provider output is available.".into()
    } else {
        text
    })
}

fn recent_terminal_screen(bytes: &[u8]) -> String {
    let mut parser = vt100::Parser::new(100, 200, 0);
    parser.process(bytes);
    let contents = parser.screen().contents();
    let lines: Vec<&str> = contents.lines().collect();
    let start = lines
        .iter()
        .rposition(|line| line.trim_start().starts_with('●'))
        .unwrap_or_else(|| lines.len().saturating_sub(12));
    let end = lines
        .iter()
        .enumerate()
        .skip(start + 1)
        .find(|(_, line)| {
            let line = line.trim();
            line.starts_with("Brewed for")
                || line.starts_with("new task?")
                || line.contains("auto mode on")
        })
        .map(|(index, _)| index)
        .unwrap_or(lines.len());
    let meaningful: Vec<_> = lines[start..end]
        .iter()
        .map(|line| line.trim())
        .filter(|line| {
            !line.is_empty()
                && !line
                    .chars()
                    .all(|character| matches!(character, '─' | '━' | '┄' | '┈' | ' '))
        })
        .collect();
    meaningful
        .into_iter()
        .rev()
        .take(8)
        .collect::<Vec<_>>()
        .into_iter()
        .rev()
        .collect::<Vec<_>>()
        .join("\n")
}

#[cfg(test)]
mod tests {
    use std::collections::BTreeSet;
    use std::sync::Mutex;

    use tempfile::tempdir;

    use crate::domain::{Runtime, SessionKind, SessionState};
    use crate::process::CommandOutput;

    use super::*;

    #[test]
    fn parses_supported_background_launch_output() {
        let output = "backgrounded · 4b34abd1 · oav-probe\n  claude agents  list";
        assert_eq!(parse_background_id(output), Some("4b34abd1".into()));
    }

    #[test]
    fn persisted_prefix_ownership_matches_full_claude_uuid() {
        let directory = tempdir().unwrap();
        let path = directory.path().join("ownership.json");
        let mut registry = OwnershipRegistry::load(path.clone()).unwrap();
        registry
            .record(OwnedSession {
                provider: Provider::Claude,
                runtime_key: "host".into(),
                provider_id_prefix: "4b34abd1".into(),
                created_at_ms: 1,
            })
            .unwrap();
        let registry = OwnershipRegistry::load(path).unwrap();

        assert!(registry.owns(&session("4b34abd1-91dc-4b50-a43f-6db2837576fe")));
        assert!(!registry.owns(&session("deadbeef-91dc-4b50-a43f-6db2837576fe")));
    }

    #[test]
    fn terminal_output_is_reduced_to_the_latest_assistant_message() {
        let input =
            b"\x1b[2J\x1b[H\x1b[2;1H\xe2\x97\x8f final answer\r\ncontinued\r\nBrewed for 3m";

        let recent = recent_terminal_screen(input);

        assert!(recent.contains("final answer"));
        assert!(recent.contains("continued"));
        assert!(!recent.contains("Brewed for"));
    }

    #[test]
    fn launch_records_ownership_and_uses_argument_arrays() {
        let directory = tempdir().unwrap();
        let registry = OwnershipRegistry::load(directory.path().join("ownership.json")).unwrap();
        let runner = Arc::new(FakeRunner {
            requests: Mutex::new(Vec::new()),
            output: Mutex::new(Some(CommandOutput {
                status: 0,
                stdout: b"backgrounded \xc2\xb7 4b34abd1 \xc2\xb7 dashboard\n".to_vec(),
                stderr: vec![],
            })),
        });
        let controller = ClaudeController {
            invocation: ControlInvocation {
                program: "claude-test".into(),
                prefix_args: Vec::new(),
            },
            docker_bin: "must-not-run-docker".into(),
            runtime: Runtime::Host,
            runner: runner.clone(),
            registry: Arc::new(Mutex::new(registry)),
        };

        let outcome = controller
            .launch(&LaunchRequest {
                provider: Provider::Claude,
                prompt: "Implement the dashboard".into(),
                cwd: PathBuf::from("/work/project"),
            })
            .unwrap();

        assert_eq!(outcome.provider_session_hint, Some("4b34abd1".into()));
        let requests = runner.requests.lock().unwrap();
        assert_eq!(requests.len(), 1);
        assert_eq!(requests[0].program, "claude-test");
        assert_eq!(
            requests[0].args,
            vec!["--background", "Implement the dashboard"]
        );
        assert_eq!(
            requests[0].current_dir,
            Some(PathBuf::from("/work/project"))
        );
        assert!(controller.owns(&session("4b34abd1-91dc-4b50-a43f-6db2837576fe")));
    }

    #[test]
    fn fixture_mode_fences_every_provider_io_path_before_dispatch() {
        let directory = tempdir().unwrap();
        let hub = ControlHub {
            controllers: BTreeMap::new(),
            codex: None,
            launch_provider: Provider::Claude,
            launch_cwd: directory.path().into(),
            provider_io_enabled: false,
        };
        let mut item = session("fixture-session");
        item.provider = Provider::Codex;
        item.capabilities.extend([
            Capability::Inspect,
            Capability::Reply,
            Capability::Approve,
            Capability::Decline,
            Capability::Respond,
            Capability::Interrupt,
            Capability::Archive,
            Capability::Delete,
        ]);

        let assert_fenced = |result: Result<ControlOutcome>| {
            let error = result.unwrap_err().to_string();
            assert_eq!(
                error,
                "provider actions are disabled while reading a fixture"
            );
        };
        assert_fenced(hub.launch("prompt".into()));
        assert_fenced(hub.interrupt(&item));
        assert_eq!(
            hub.inspect(&item).unwrap_err().to_string(),
            "provider actions are disabled while reading a fixture"
        );
        assert_fenced(hub.reply(&item, "reply"));
        assert_fenced(hub.archive(&item));
        assert_fenced(hub.resolve_approval(&item, true));
        assert_fenced(hub.resolve_approval(&item, false));
        assert_fenced(hub.respond_input(&item, "answer"));
        assert_fenced(hub.delete(&item));
        assert_fenced(hub.open(&item));
        assert!(hub.codex_supervisor().is_none());
    }

    fn uncontrolled_hub(launch_provider: Provider) -> ControlHub {
        ControlHub {
            controllers: BTreeMap::new(),
            codex: None,
            launch_provider,
            launch_cwd: PathBuf::from("/work"),
            provider_io_enabled: true,
        }
    }

    #[test]
    fn control_hub_refuses_missing_capabilities_before_codex_dispatch() {
        let hub = uncontrolled_hub(Provider::Codex);
        let mut item = session("thread");
        item.provider = Provider::Codex;

        assert_eq!(
            hub.reply(&item, "reply").unwrap_err().to_string(),
            "reply authority was not granted for this session"
        );
        assert_eq!(
            hub.archive(&item).unwrap_err().to_string(),
            "archive authority was not granted for this session"
        );
        assert_eq!(
            hub.resolve_approval(&item, true).unwrap_err().to_string(),
            "inline approval authority was not granted for this session"
        );
        assert_eq!(
            hub.resolve_approval(&item, false).unwrap_err().to_string(),
            "inline approval authority was not granted for this session"
        );
        assert_eq!(
            hub.respond_input(&item, "answer").unwrap_err().to_string(),
            "structured-input authority was not granted for this session"
        );
        assert_eq!(
            hub.delete(&item).unwrap_err().to_string(),
            "delete authority was not granted for this session"
        );
    }

    #[test]
    fn inline_codex_controls_require_the_exact_host_provider_runtime() {
        let hub = uncontrolled_hub(Provider::Codex);
        let mut claude = session("claude");
        claude
            .capabilities
            .extend([Capability::Reply, Capability::Approve, Capability::Respond]);
        assert_eq!(
            hub.reply(&claude, "reply").unwrap_err().to_string(),
            "no Claude controller is configured"
        );
        assert_eq!(
            hub.resolve_approval(&claude, true).unwrap_err().to_string(),
            "no Claude controller is configured"
        );
        assert_eq!(
            hub.respond_input(&claude, "answer")
                .unwrap_err()
                .to_string(),
            "no Claude controller is configured"
        );

        let mut docker_codex = claude;
        docker_codex.provider = Provider::Codex;
        docker_codex.runtime = Runtime::Docker {
            container_name: "fixture".into(),
            container_id: "immutable".into(),
            image: "fixture@sha256:exact".into(),
        };
        assert!(hub.reply(&docker_codex, "reply").is_err());
        assert!(hub.resolve_approval(&docker_codex, false).is_err());
        assert!(hub.respond_input(&docker_codex, "answer").is_err());
    }

    #[test]
    fn unsupported_or_disabled_provider_routes_fail_locally() {
        let unsupported = Provider::Other("future-agent".into());
        let hub = uncontrolled_hub(unsupported.clone());
        let mut item = session("other");
        item.provider = unsupported;

        assert!(hub
            .launch("prompt".into())
            .unwrap_err()
            .to_string()
            .contains("no future-agent controller"));
        assert!(hub
            .interrupt(&item)
            .unwrap_err()
            .to_string()
            .contains("interrupt authority was not granted"));
        assert!(hub
            .inspect(&item)
            .unwrap_err()
            .to_string()
            .contains("inspection authority was not granted"));

        let claude_hub = uncontrolled_hub(Provider::Claude);
        assert_eq!(
            claude_hub.launch("prompt".into()).unwrap_err().to_string(),
            "no Claude controller is configured"
        );
        let codex_hub = uncontrolled_hub(Provider::Codex);
        assert_eq!(
            codex_hub.launch("prompt".into()).unwrap_err().to_string(),
            "no Codex controller is configured"
        );
    }

    #[test]
    fn docker_codex_inspection_is_refused_without_invoking_docker() {
        let hub = uncontrolled_hub(Provider::Codex);
        let mut item = session("docker-thread");
        item.provider = Provider::Codex;
        item.runtime = Runtime::Docker {
            container_name: "fixture".into(),
            container_id: "immutable".into(),
            image: "fixture@sha256:exact".into(),
        };
        item.capabilities.insert(Capability::Inspect);

        assert_eq!(
            hub.inspect(&item).unwrap_err().to_string(),
            "no Codex controller is configured"
        );
    }

    #[test]
    fn claude_launch_rejects_empty_failed_and_unparseable_results() {
        let directory = tempdir().unwrap();
        let registry = OwnershipRegistry::load(directory.path().join("ownership.json")).unwrap();
        let runner = Arc::new(FakeRunner {
            requests: Mutex::new(Vec::new()),
            output: Mutex::new(Some(CommandOutput {
                status: 7,
                stdout: vec![],
                stderr: b"provider failure".to_vec(),
            })),
        });
        let controller = ClaudeController {
            invocation: ControlInvocation {
                program: "claude-test".into(),
                prefix_args: Vec::new(),
            },
            docker_bin: "must-not-run-docker".into(),
            runtime: Runtime::Host,
            runner: runner.clone(),
            registry: Arc::new(Mutex::new(registry)),
        };
        assert_eq!(
            controller
                .launch(&LaunchRequest {
                    provider: Provider::Claude,
                    prompt: "   ".into(),
                    cwd: PathBuf::from("/work"),
                })
                .unwrap_err()
                .to_string(),
            "the launch prompt cannot be empty"
        );
        assert!(runner.requests.lock().unwrap().is_empty());
        assert!(controller
            .launch(&LaunchRequest {
                provider: Provider::Claude,
                prompt: "prompt".into(),
                cwd: PathBuf::from("/work"),
            })
            .unwrap_err()
            .to_string()
            .contains("status 7"));

        let registry = OwnershipRegistry::load(directory.path().join("other.json")).unwrap();
        let controller = ClaudeController {
            invocation: ControlInvocation {
                program: "claude-test".into(),
                prefix_args: Vec::new(),
            },
            docker_bin: "must-not-run-docker".into(),
            runtime: Runtime::Host,
            runner: Arc::new(FakeRunner {
                requests: Mutex::new(Vec::new()),
                output: Mutex::new(Some(CommandOutput {
                    status: 0,
                    stdout: b"unexpected success output".to_vec(),
                    stderr: vec![],
                })),
            }),
            registry: Arc::new(Mutex::new(registry)),
        };
        assert!(controller
            .launch(&LaunchRequest {
                provider: Provider::Claude,
                prompt: "prompt".into(),
                cwd: PathBuf::from("/work"),
            })
            .unwrap_err()
            .to_string()
            .contains("did not return a background session ID"));
    }

    struct FakeRunner {
        requests: Mutex<Vec<CommandRequest>>,
        output: Mutex<Option<CommandOutput>>,
    }

    struct StubController {
        provider: Provider,
        marker: &'static str,
    }

    impl ProviderController for StubController {
        fn provider(&self) -> Provider {
            self.provider.clone()
        }

        fn enrich(&self, snapshot: &mut SessionSnapshot) {
            for session in &mut snapshot.sessions {
                if session.provider == self.provider {
                    session.capabilities.insert(Capability::Inspect);
                }
            }
        }

        fn launch(&self, request: &LaunchRequest) -> Result<ControlOutcome> {
            assert_eq!(request.provider, self.provider);
            Ok(ControlOutcome {
                message: format!("launched {}", self.marker),
                provider_session_hint: Some(self.marker.into()),
            })
        }

        fn inspect(&self, session: &AgentSession) -> Result<String> {
            assert_eq!(session.provider, self.provider);
            Ok(format!("inspected {}", self.marker))
        }
    }

    impl CommandRunner for FakeRunner {
        fn run(&self, request: &CommandRequest) -> Result<CommandOutput> {
            self.requests.lock().unwrap().push(request.clone());
            Ok(self.output.lock().unwrap().take().unwrap())
        }
    }

    #[test]
    fn registered_provider_controller_enriches_and_dispatches_without_replacement() {
        let mut hub = uncontrolled_hub(Provider::Pi);
        hub.register_controller(Arc::new(StubController {
            provider: Provider::Pi,
            marker: "first",
        }))
        .unwrap();
        assert_eq!(
            hub.register_controller(Arc::new(StubController {
                provider: Provider::Pi,
                marker: "replacement",
            }))
            .unwrap_err()
            .to_string(),
            "a Pi controller is already registered"
        );

        let mut pi = session("pi-session");
        pi.provider = Provider::Pi;
        let claude = session("claude-session");
        let mut snapshot = SessionSnapshot {
            sessions: vec![pi, claude],
            warnings: Vec::new(),
        };
        hub.enrich(&mut snapshot);

        assert!(snapshot.sessions[0]
            .capabilities
            .contains(&Capability::Inspect));
        assert!(!snapshot.sessions[1]
            .capabilities
            .contains(&Capability::Inspect));
        assert_eq!(
            hub.launch("prompt".into()).unwrap().message,
            "launched first"
        );
        assert_eq!(
            hub.inspect(&snapshot.sessions[0]).unwrap(),
            "inspected first"
        );
    }

    fn session(id: &str) -> AgentSession {
        AgentSession {
            id: format!("claude:host:{id}"),
            provider_session_id: id.into(),
            provider: Provider::Claude,
            runtime: Runtime::Host,
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
        }
    }
}

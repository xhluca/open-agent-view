use std::collections::BTreeSet;
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

pub struct ControlHub {
    claude: Option<ClaudeController>,
    codex: Option<Arc<CodexSupervisor>>,
    claude_bin: String,
    codex_bin: String,
    docker_bin: String,
    launch_provider: Provider,
    launch_cwd: PathBuf,
    provider_io_enabled: bool,
}

impl ControlHub {
    pub fn new(config: ControlHubConfig) -> Result<Self> {
        let claude = if config.provider_io_enabled && config.claude_enabled {
            let registry = OwnershipRegistry::load(default_registry_path()?)?;
            Some(ClaudeController::host(config.claude_bin.clone(), registry))
        } else {
            None
        };
        Ok(Self {
            claude,
            codex: if config.provider_io_enabled && config.codex_enabled {
                Some(Arc::new(CodexSupervisor::host(config.codex_bin.clone())?))
            } else {
                None
            },
            claude_bin: config.claude_bin,
            codex_bin: config.codex_bin,
            docker_bin: config.docker_bin,
            launch_provider: config.launch_provider,
            launch_cwd: config.launch_cwd,
            provider_io_enabled: config.provider_io_enabled,
        })
    }

    pub fn enrich(&self, snapshot: &mut SessionSnapshot) {
        for session in &mut snapshot.sessions {
            if self
                .claude
                .as_ref()
                .map(|controller| controller.owns(session))
                .unwrap_or(false)
            {
                session.capabilities.insert(Capability::Interrupt);
            }
        }
        if let Some(supervisor) = &self.codex {
            supervisor.enrich(snapshot);
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
        match request.provider {
            Provider::Claude => self
                .claude
                .as_ref()
                .context("host Claude launch is disabled")?
                .launch(&request),
            Provider::Codex => {
                let thread_id = self
                    .codex
                    .as_ref()
                    .context("host Codex launch is disabled")?
                    .launch(&request.prompt, &request.cwd)?;
                Ok(ControlOutcome {
                    message: format!("started managed Codex thread {thread_id}"),
                    provider_session_hint: Some(thread_id),
                })
            }
            provider => bail!(
                "no launch controller is configured for {}",
                provider.label()
            ),
        }
    }

    pub fn interrupt(&self, session: &AgentSession) -> Result<ControlOutcome> {
        self.ensure_provider_io()?;
        match session.provider {
            Provider::Claude => self
                .claude
                .as_ref()
                .context("Claude control is disabled")?
                .stop(session),
            Provider::Codex => {
                self.codex
                    .as_ref()
                    .context("host Codex control is disabled")?
                    .interrupt(session)?;
                Ok(ControlOutcome {
                    message: format!(
                        "interrupted managed Codex thread {}",
                        session.provider_session_id
                    ),
                    provider_session_hint: Some(session.provider_session_id.clone()),
                })
            }
            ref provider => bail!("no controller is configured for {}", provider.label()),
        }
    }

    pub fn inspect(&self, session: &AgentSession) -> Result<String> {
        self.ensure_provider_io()?;
        match (&session.provider, &session.runtime) {
            (Provider::Claude, Runtime::Host) => {
                let mut request = CommandRequest::new(
                    self.claude_bin.clone(),
                    vec!["logs".into(), short_claude_id(&session.provider_session_id)],
                );
                request.timeout = Duration::from_secs(8);
                read_command_output(&ProcessRunner.run(&request)?, "Claude logs")
            }
            (Provider::Claude, Runtime::Docker { container_id, .. }) => {
                let mut request = CommandRequest::new(
                    self.docker_bin.clone(),
                    vec![
                        "exec".into(),
                        container_id.clone(),
                        "claude".into(),
                        "logs".into(),
                        short_claude_id(&session.provider_session_id),
                    ],
                );
                request.timeout = Duration::from_secs(8);
                read_command_output(&ProcessRunner.run(&request)?, "Claude logs")
            }
            (Provider::Codex, Runtime::Host) => self
                .codex
                .as_ref()
                .context("host Codex control is disabled")?
                .inspect(session),
            (Provider::Codex, Runtime::Docker { .. }) => {
                bail!("Docker Codex transcript inspection is observe-only")
            }
            (provider, _) => bail!("cannot inspect unsupported provider {}", provider.label()),
        }
    }

    pub fn reply(&self, session: &AgentSession, prompt: &str) -> Result<ControlOutcome> {
        self.ensure_provider_io()?;
        if !session.capabilities.contains(&Capability::Reply) {
            bail!("reply authority was not granted for this session");
        }
        if session.provider != Provider::Codex || session.runtime != Runtime::Host {
            bail!("inline reply is supported only for owned host Codex threads");
        }
        let mode = self
            .codex
            .as_ref()
            .context("host Codex control is disabled")?
            .reply(session, prompt)?;
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

    pub fn archive(&self, session: &AgentSession) -> Result<ControlOutcome> {
        self.ensure_provider_io()?;
        if !session.capabilities.contains(&Capability::Archive) {
            bail!("archive authority was not granted for this session");
        }
        self.codex
            .as_ref()
            .context("host Codex control is disabled")?
            .archive(session)?;
        Ok(ControlOutcome {
            message: format!(
                "archived managed Codex thread {}",
                session.provider_session_id
            ),
            provider_session_hint: Some(session.provider_session_id.clone()),
        })
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
        if session.provider != Provider::Codex || session.runtime != Runtime::Host {
            bail!("inline approvals are supported only for owned host Codex threads");
        }
        self.codex
            .as_ref()
            .context("host Codex control is disabled")?
            .respond_approval(session, accept)?;
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

    pub fn respond_input(&self, session: &AgentSession, answer: &str) -> Result<ControlOutcome> {
        self.ensure_provider_io()?;
        if !session.capabilities.contains(&Capability::Respond) {
            bail!("structured-input authority was not granted for this session");
        }
        if session.provider != Provider::Codex || session.runtime != Runtime::Host {
            bail!("structured input is supported only for owned host Codex threads");
        }
        let progress = self
            .codex
            .as_ref()
            .context("host Codex control is disabled")?
            .respond_user_input(session, answer)?;
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

    pub fn delete(&self, session: &AgentSession) -> Result<ControlOutcome> {
        self.ensure_provider_io()?;
        if !session.capabilities.contains(&Capability::Delete) {
            bail!("delete authority was not granted for this session");
        }
        self.codex
            .as_ref()
            .context("host Codex control is disabled")?
            .delete(session)?;
        Ok(ControlOutcome {
            message: format!(
                "deleted managed Codex thread {}",
                session.provider_session_id
            ),
            provider_session_hint: Some(session.provider_session_id.clone()),
        })
    }

    pub fn open(&self, session: &AgentSession) -> Result<ControlOutcome> {
        self.ensure_provider_io()?;
        let managed_codex_remote = self
            .codex
            .as_ref()
            .and_then(|supervisor| supervisor.remote_url_if_owned(session));
        let mut command = match (&session.provider, &session.runtime) {
            (Provider::Claude, Runtime::Host) => {
                let mut command = Command::new(&self.claude_bin);
                command
                    .args(["attach", &short_claude_id(&session.provider_session_id)])
                    .current_dir(&session.cwd);
                command
            }
            (Provider::Codex, Runtime::Host) => {
                let mut command = Command::new(&self.codex_bin);
                if let Some(remote) = managed_codex_remote.as_deref() {
                    command.args(["--remote", remote, "resume", &session.provider_session_id]);
                } else {
                    command.args(["resume", &session.provider_session_id]);
                }
                command.current_dir(&session.cwd);
                command
            }
            (Provider::Claude, Runtime::Docker { container_id, .. }) => {
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
            (Provider::Codex, Runtime::Docker { container_id, .. }) => {
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
            (provider, _) => bail!("cannot open unsupported provider {}", provider.label()),
        };
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

    fn ensure_provider_io(&self) -> Result<()> {
        if !self.provider_io_enabled {
            bail!("provider actions are disabled while reading a fixture");
        }
        Ok(())
    }
}

struct ClaudeController {
    invocation: ControlInvocation,
    runtime: Runtime,
    runner: Arc<dyn CommandRunner>,
    registry: Arc<Mutex<OwnershipRegistry>>,
}

impl ClaudeController {
    fn host(executable: String, registry: OwnershipRegistry) -> Self {
        Self {
            invocation: ControlInvocation {
                program: executable,
                prefix_args: Vec::new(),
            },
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
            claude: None,
            codex: None,
            claude_bin: "must-not-run-claude".into(),
            codex_bin: "must-not-run-codex".into(),
            docker_bin: "must-not-run-docker".into(),
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
            claude: None,
            codex: None,
            claude_bin: "must-not-run-claude".into(),
            codex_bin: "must-not-run-codex".into(),
            docker_bin: "must-not-run-docker".into(),
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
            "inline reply is supported only for owned host Codex threads"
        );
        assert_eq!(
            hub.resolve_approval(&claude, true).unwrap_err().to_string(),
            "inline approvals are supported only for owned host Codex threads"
        );
        assert_eq!(
            hub.respond_input(&claude, "answer")
                .unwrap_err()
                .to_string(),
            "structured input is supported only for owned host Codex threads"
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
            .contains("no launch controller"));
        assert!(hub
            .interrupt(&item)
            .unwrap_err()
            .to_string()
            .contains("no controller"));
        assert!(hub
            .inspect(&item)
            .unwrap_err()
            .to_string()
            .contains("cannot inspect unsupported provider"));

        let claude_hub = uncontrolled_hub(Provider::Claude);
        assert_eq!(
            claude_hub.launch("prompt".into()).unwrap_err().to_string(),
            "host Claude launch is disabled"
        );
        let codex_hub = uncontrolled_hub(Provider::Codex);
        assert_eq!(
            codex_hub.launch("prompt".into()).unwrap_err().to_string(),
            "host Codex launch is disabled"
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

        assert_eq!(
            hub.inspect(&item).unwrap_err().to_string(),
            "Docker Codex transcript inspection is observe-only"
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

    impl CommandRunner for FakeRunner {
        fn run(&self, request: &CommandRequest) -> Result<CommandOutput> {
            self.requests.lock().unwrap().push(request.clone());
            Ok(self.output.lock().unwrap().take().unwrap())
        }
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

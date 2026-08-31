use std::collections::{BTreeMap, BTreeSet};
use std::fs::{self, OpenOptions};
use std::io::Write;
use std::path::PathBuf;
use std::process::Command;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant, SystemTime};

use anyhow::{bail, Context, Result};
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};

use crate::adapters::parse_claude_sessions;
use crate::codex_rpc::{AppServerClient, AppServerInvocation};
use crate::codex_supervisor::{CodexReplyMode, CodexSupervisor};
use crate::domain::{
    AgentSession, Capability, LaunchTarget, Provider, Runtime, SessionKind, SessionSnapshot,
    SessionState,
};
use crate::migration::MigrationRegistry;
use crate::process::{CommandRequest, CommandRunner, ProcessRunner};

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct LaunchRequest {
    pub provider: Provider,
    pub model: Option<String>,
    pub prompt: String,
    pub cwd: PathBuf,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum LaunchMode {
    Unavailable,
    DefaultModel,
    SelectableModel,
}

/// Whether starting a task stays inside the dashboard or hands the terminal
/// to the provider's native interactive UI.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum LaunchPresentation {
    Background,
    /// Bootstrap without blocking the dashboard, then open the exact returned
    /// provider session once discovery confirms its stable ID.
    DeferredForeground,
    Foreground,
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

    fn launch_mode(&self) -> LaunchMode {
        LaunchMode::Unavailable
    }

    /// Return model identifiers accepted by this provider's launch API.
    ///
    /// Providers without a stable machine-readable catalog may return an
    /// empty list while still accepting an explicitly typed model name.
    fn available_models(&self) -> Result<Vec<String>> {
        Ok(Vec::new())
    }

    /// Perform an explicit setup action offered by a launch-option picker.
    /// Terminal uses this for missing shells; providers otherwise keep this
    /// unavailable rather than interpreting model text as an installer.
    fn setup_launch_option(&self, _option: &str) -> Result<ControlOutcome> {
        bail!(
            "{} launch-option setup is unavailable",
            self.provider().label()
        )
    }

    fn launch_presentation(&self) -> LaunchPresentation {
        LaunchPresentation::Background
    }

    /// Whether the provider has a native, interactive authentication flow.
    fn supports_authentication(&self) -> bool {
        false
    }

    /// Run the provider's own login UI in an isolated provider-native PTY.
    /// The dashboard suspends raw/alternate-screen mode before calling this;
    /// The native return gesture backgrounds setup without colliding with another session.
    fn authenticate(&self) -> Result<ControlOutcome> {
        bail!(
            "{} does not expose interactive login",
            self.provider().label()
        )
    }

    fn enrich(&self, _snapshot: &mut SessionSnapshot) {}

    fn launch(&self, _request: &LaunchRequest) -> Result<ControlOutcome> {
        bail!("{} launch is unavailable", self.provider().label())
    }

    fn launch_foreground(&self, request: &LaunchRequest) -> Result<ControlOutcome> {
        self.launch(request)
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

    /// Open one exact session imported by session-migrate. The hub calls this
    /// only after matching OAV's private migration record; mutation authority
    /// is deliberately not inherited.
    fn open_imported(&self, session: &AgentSession) -> Result<ControlOutcome> {
        self.open(session)
    }
}

#[derive(Clone)]
pub struct ControlHub {
    controllers: BTreeMap<Provider, Arc<dyn ProviderController>>,
    codex: Option<Arc<CodexSupervisor>>,
    claude_registry: Option<Arc<Mutex<OwnershipRegistry>>>,
    launch_provider: Provider,
    launch_cwd: PathBuf,
    provider_io_enabled: bool,
    migration_registry: Option<MigrationRegistry>,
}

impl ControlHub {
    pub fn new(config: ControlHubConfig) -> Result<Self> {
        let mut controllers: BTreeMap<Provider, Arc<dyn ProviderController>> = BTreeMap::new();
        let mut claude_registry = None;
        if config.provider_io_enabled && config.claude_enabled {
            let registry = Arc::new(Mutex::new(OwnershipRegistry::load(
                default_registry_path()?
            )?));
            controllers.insert(
                Provider::Claude,
                Arc::new(ClaudeController::host(
                    config.claude_bin.clone(),
                    config.docker_bin.clone(),
                    registry.clone(),
                )),
            );
            claude_registry = Some(registry);
        }
        let codex = if config.provider_io_enabled && config.codex_enabled && cfg!(unix) {
            let supervisor = Arc::new(CodexSupervisor::host(config.codex_bin.clone())?);
            controllers.insert(
                Provider::Codex,
                Arc::new(CodexController {
                    supervisor: Some(supervisor.clone()),
                    codex_bin: config.codex_bin.clone(),
                    docker_bin: config.docker_bin.clone(),
                }),
            );
            Some(supervisor)
        } else {
            if config.provider_io_enabled && config.codex_enabled {
                controllers.insert(
                    Provider::Codex,
                    Arc::new(CodexController {
                        supervisor: None,
                        codex_bin: config.codex_bin.clone(),
                        docker_bin: config.docker_bin.clone(),
                    }),
                );
            }
            None
        };
        Ok(Self {
            controllers,
            codex,
            claude_registry,
            launch_provider: config.launch_provider,
            launch_cwd: config.launch_cwd,
            provider_io_enabled: config.provider_io_enabled,
            migration_registry: None,
        })
    }

    pub fn register_migration_registry(&mut self, registry: MigrationRegistry) {
        self.migration_registry = Some(registry);
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

    /// Remove host Claude sessions not recorded by OAV's launch registry.
    /// Other default sources are owned-by-construction (durable supervisors or
    /// private managed registries), so they do not need a second history scan.
    pub fn retain_owned(&self, snapshot: &mut SessionSnapshot) {
        let migrated = |session: &AgentSession| {
            self.migration_registry
                .as_ref()
                .is_some_and(|registry| registry.contains_exact(session))
        };
        let Some(registry) = &self.claude_registry else {
            snapshot.sessions.retain(|session| {
                session.provider != Provider::Claude
                    || session.runtime != Runtime::Host
                    || migrated(session)
            });
            return;
        };
        match registry.lock() {
            Ok(registry) => snapshot.sessions.retain(|session| {
                session.provider != Provider::Claude
                    || session.runtime != Runtime::Host
                    || registry.owns(session)
                    || migrated(session)
            }),
            Err(_) => {
                snapshot
                    .warnings
                    .push("Claude ownership registry lock was poisoned".into());
                snapshot.sessions.retain(|session| {
                    session.provider != Provider::Claude
                        || session.runtime != Runtime::Host
                        || migrated(session)
                });
            }
        }
    }

    pub fn codex_supervisor(&self) -> Option<Arc<CodexSupervisor>> {
        self.codex.clone()
    }

    pub fn default_launch_provider(&self) -> Provider {
        self.launch_provider.clone()
    }

    pub fn launch_targets(&self) -> Vec<LaunchTarget> {
        self.controllers
            .values()
            .filter_map(|controller| match controller.launch_mode() {
                LaunchMode::Unavailable => None,
                LaunchMode::DefaultModel => Some(LaunchTarget {
                    provider: controller.provider(),
                    supports_model: false,
                }),
                LaunchMode::SelectableModel => Some(LaunchTarget {
                    provider: controller.provider(),
                    supports_model: true,
                }),
            })
            .collect()
    }

    pub fn available_models(&self, provider: &Provider) -> Result<Vec<String>> {
        self.ensure_provider_io()?;
        let controller = self.controller(provider)?;
        if controller.launch_mode() != LaunchMode::SelectableModel {
            bail!("{} does not expose model selection", provider.label());
        }
        controller.available_models()
    }

    pub fn setup_launch_option(&self, provider: &Provider, option: &str) -> Result<ControlOutcome> {
        self.ensure_provider_io()?;
        self.controller(provider)?.setup_launch_option(option)
    }

    pub fn launch_presentation(&self, provider: &Provider) -> Result<LaunchPresentation> {
        self.ensure_provider_io()?;
        Ok(self.controller(provider)?.launch_presentation())
    }

    pub fn supports_authentication(&self, provider: &Provider) -> bool {
        self.provider_io_enabled
            && self
                .controllers
                .get(provider)
                .is_some_and(|controller| controller.supports_authentication())
    }

    pub fn authenticate(&self, provider: &Provider) -> Result<ControlOutcome> {
        self.ensure_provider_io()?;
        self.controller(provider)?.authenticate()
    }

    /// Open the CLI's interactive install/login wizard in the same isolated
    /// setup PTY used by direct provider authentication. This route is also
    /// available for a harness that was missing when the dashboard started.
    pub fn setup_provider(&self, provider: &Provider) -> Result<ControlOutcome> {
        self.ensure_provider_io()?;
        if provider == &Provider::Terminal {
            return Ok(ControlOutcome {
                message: "Terminal is built into Open Agent View and needs no setup".into(),
                provider_session_hint: None,
            });
        }
        let value = match provider {
            Provider::Claude => "claude",
            Provider::Codex => "codex",
            Provider::Pi => "pi",
            Provider::OpenCode => "opencode",
            Provider::Cursor => "cursor",
            Provider::GitHubCopilot => "copilot",
            Provider::Antigravity => "antigravity",
            Provider::MistralVibe => "mistral-vibe",
            Provider::MuseCode => "muse",
            Provider::QwenCode => "qwen",
            Provider::KimiCode => "kimi",
            Provider::OhMyPi => "omp",
            Provider::Grok => "grok",
            Provider::KiloCode => "kilo",
            Provider::OpenHands => "openhands",
            Provider::Terminal => unreachable!("handled above"),
            Provider::Other(_) => bail!("setup is unavailable for this provider"),
        };
        let executable = std::env::current_exe().context("failed to resolve the OAV executable")?;
        let mut command = Command::new(executable);
        command.args(["setup", value]);
        let session_key = format!("setup:{}", provider.label());
        match crate::native_session::run(command, &session_key)? {
            crate::native_session::NativeSessionExit::Backgrounded => Ok(ControlOutcome {
                message: format!(
                    "backgrounded {} setup; run `/setup {value}` to resume it",
                    provider.label()
                ),
                provider_session_hint: None,
            }),
            crate::native_session::NativeSessionExit::Exited(status) if status.success() => {
                Ok(ControlOutcome {
                    message: format!(
                        "{} setup completed; restart OAV if it was newly installed",
                        provider.label()
                    ),
                    provider_session_hint: None,
                })
            }
            crate::native_session::NativeSessionExit::Exited(status) => {
                bail!("{} setup exited with status {status}", provider.label())
            }
        }
    }

    pub fn launch(&self, prompt: String) -> Result<ControlOutcome> {
        self.launch_with(self.launch_provider.clone(), None, prompt)
    }

    pub fn launch_with(
        &self,
        provider: Provider,
        model: Option<String>,
        prompt: String,
    ) -> Result<ControlOutcome> {
        self.ensure_provider_io()?;
        let controller = self.controller(&provider)?;
        let model = validate_model(model)?;
        match (controller.launch_mode(), model.as_ref()) {
            (LaunchMode::Unavailable, _) => {
                bail!("{} launch is unavailable", provider.label())
            }
            (LaunchMode::DefaultModel, Some(_)) => {
                bail!("{} does not expose model selection", provider.label())
            }
            _ => {}
        }
        let request = LaunchRequest {
            provider,
            model,
            prompt,
            cwd: self.launch_cwd.clone(),
        };
        controller.launch(&request)
    }

    pub fn launch_foreground_with(
        &self,
        provider: Provider,
        model: Option<String>,
        prompt: String,
    ) -> Result<ControlOutcome> {
        self.ensure_provider_io()?;
        let controller = self.controller(&provider)?;
        let model = validate_model(model)?;
        match (controller.launch_mode(), model.as_ref()) {
            (LaunchMode::Unavailable, _) => {
                bail!("{} launch is unavailable", provider.label())
            }
            (LaunchMode::DefaultModel, Some(_)) => {
                bail!("{} does not expose model selection", provider.label())
            }
            _ => {}
        }
        controller.launch_foreground(&LaunchRequest {
            provider,
            model,
            prompt,
            cwd: self.launch_cwd.clone(),
        })
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
        let controller = self.controller(&session.provider)?;
        if self
            .migration_registry
            .as_ref()
            .is_some_and(|registry| registry.contains_exact(session))
        {
            controller.open_imported(session)
        } else {
            controller.open(session)
        }
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
    summary_cache: Mutex<BTreeMap<String, CachedClaudeSummary>>,
}

#[derive(Clone)]
struct CachedClaudeSummary {
    summary: String,
    refreshed_at: Instant,
}

impl ClaudeController {
    fn host(
        executable: String,
        docker_bin: String,
        registry: Arc<Mutex<OwnershipRegistry>>,
    ) -> Self {
        Self {
            invocation: ControlInvocation {
                program: executable,
                prefix_args: Vec::new(),
            },
            docker_bin,
            runtime: Runtime::Host,
            runner: Arc::new(ProcessRunner),
            registry,
            summary_cache: Mutex::new(BTreeMap::new()),
        }
    }

    fn enrich_summaries(&self, snapshot: &mut SessionSnapshot, owned_indices: &[usize]) {
        const CACHE_TTL: Duration = Duration::from_secs(10);
        const MAX_SUMMARY_PROBES: usize = 24;
        const MAX_WORKERS: usize = 4;

        let now = Instant::now();
        let mut candidates = Vec::new();
        if let Ok(cache) = self.summary_cache.lock() {
            for &index in owned_indices {
                let session = &mut snapshot.sessions[index];
                if !session.summary.is_empty() {
                    continue;
                }
                let cached = cache.get(&session.provider_session_id);
                if let Some(cached) = cached.filter(|cached| !cached.summary.is_empty()) {
                    session.summary = cached.summary.clone();
                }
                let cache_is_fresh = cached.is_some_and(|cached| {
                    now.saturating_duration_since(cached.refreshed_at) < CACHE_TTL
                });
                let completed_is_cached = session.state == SessionState::Completed
                    && cached.is_some_and(|cached| !cached.summary.is_empty());
                if !cache_is_fresh && !completed_is_cached {
                    candidates.push((index, session.provider_session_id.clone()));
                }
            }
        }

        candidates
            .sort_by_key(|(index, _)| std::cmp::Reverse(snapshot.sessions[*index].started_at));
        candidates.truncate(MAX_SUMMARY_PROBES);
        if candidates.is_empty() {
            return;
        }

        let worker_count = candidates.len().min(MAX_WORKERS);
        let chunk_size = (candidates.len() + worker_count - 1) / worker_count;
        let batches = std::thread::scope(|scope| {
            let mut handles = Vec::new();
            for chunk in candidates.chunks(chunk_size) {
                let work = chunk.to_vec();
                let runner = Arc::clone(&self.runner);
                let program = self.invocation.program.clone();
                let prefix_args = self.invocation.prefix_args.clone();
                handles.push(scope.spawn(move || {
                    work.into_iter()
                        .map(|(index, session_id)| {
                            let mut args = prefix_args.clone();
                            args.extend(["logs".into(), session_id.chars().take(8).collect()]);
                            let mut request = CommandRequest::new(program.clone(), args);
                            request.timeout = Duration::from_secs(4);
                            let summary = runner
                                .run(&request)
                                .ok()
                                .filter(|output| output.status == 0)
                                .and_then(|output| claude_summary_from_terminal(&output.stdout));
                            (index, session_id, summary)
                        })
                        .collect::<Vec<_>>()
                }));
            }
            handles
                .into_iter()
                .filter_map(|handle| handle.join().ok())
                .flatten()
                .collect::<Vec<_>>()
        });

        let refreshed_at = Instant::now();
        if let Ok(mut cache) = self.summary_cache.lock() {
            for (index, session_id, summary) in batches {
                let summary = summary.unwrap_or_default();
                if !summary.is_empty() {
                    snapshot.sessions[index].summary = summary.clone();
                }
                cache.insert(
                    session_id,
                    CachedClaudeSummary {
                        summary,
                        refreshed_at,
                    },
                );
            }
        }
    }

    fn launch(&self, request: &LaunchRequest) -> Result<ControlOutcome> {
        let session_id = self.start_background(request)?;
        Ok(ControlOutcome {
            message: format!(
                "started Claude background session {}",
                short_claude_id(&session_id)
            ),
            provider_session_hint: Some(session_id),
        })
    }

    fn start_background(&self, request: &LaunchRequest) -> Result<String> {
        if request.prompt.trim().is_empty() {
            bail!("the launch prompt cannot be empty");
        }
        let mut args = self.invocation.prefix_args.clone();
        if let Some(model) = &request.model {
            args.extend(["--model".into(), model.clone()]);
        }
        args.extend(["--background".into(), request.prompt.trim().to_owned()]);
        let mut command = CommandRequest::new(self.invocation.program.clone(), args);
        command.current_dir = Some(request.cwd.clone());
        // Recent Claude releases allocate their own background session ID and
        // print it only after the background supervisor accepts the task. Run
        // this bounded bootstrap off the dashboard thread, capture that exact
        // ID, and never combine --background with --session-id (Claude ignores
        // the latter and warns, which previously orphaned OAV ownership).
        command.timeout = Duration::from_secs(45);
        let output = self.runner.run(&command)?;
        if output.status != 0 {
            bail!(
                "Claude background launch exited with status {}: {}",
                output.status,
                output.stderr_lossy()
            );
        }
        let short_id = parse_claude_background_id(output.stdout_text()?)?;
        let session_id = self.wait_until_listed(&short_id)?;
        let ownership = OwnedSession {
            provider: Provider::Claude,
            runtime_key: runtime_key(&self.runtime),
            provider_id_prefix: session_id.clone(),
            created_at_ms: now_millis(),
        };
        if let Err(error) = self
            .registry
            .lock()
            .map_err(|_| anyhow::anyhow!("ownership registry lock was poisoned"))?
            .record(ownership)
        {
            // The provider already accepted the task. Best-effort stop the
            // exact returned ID rather than leave an untracked background job.
            let mut stop_args = self.invocation.prefix_args.clone();
            stop_args.extend(["stop".into(), short_id]);
            let mut stop = CommandRequest::new(self.invocation.program.clone(), stop_args);
            stop.timeout = Duration::from_secs(8);
            let _ = self.runner.run(&stop);
            return Err(error).context(
                "Claude launched a task, but its ownership record could not be saved; the exact task was stopped",
            );
        }
        Ok(session_id)
    }

    fn launch_foreground(&self, request: &LaunchRequest) -> Result<ControlOutcome> {
        if request.prompt.trim().is_empty() {
            bail!("the launch prompt cannot be empty");
        }
        if self.runtime != Runtime::Host {
            bail!("foreground Claude launch is supported only on the host");
        }
        let session_id = self.start_background(request)?;
        let mut attach = Command::new(&self.invocation.program);
        attach
            .args(&self.invocation.prefix_args)
            .args(["attach", &short_claude_id(&session_id)])
            .current_dir(&request.cwd);
        let session_key = format!("claude:host:{session_id}");
        let exit = crate::native_session::run(attach, &session_key)?;
        match exit {
            crate::native_session::NativeSessionExit::Backgrounded => Ok(ControlOutcome {
                message: format!(
                    "backgrounded Claude session {}; Enter/Right resumes it",
                    &session_id[..8]
                ),
                provider_session_hint: Some(session_id),
            }),
            crate::native_session::NativeSessionExit::Exited(status) if status.success() => {
                Ok(ControlOutcome {
                    message: format!("Claude session {} finished", &session_id[..8]),
                    provider_session_hint: Some(session_id),
                })
            }
            crate::native_session::NativeSessionExit::Exited(status) => {
                bail!("Claude session exited with status {status}")
            }
        }
    }

    fn wait_until_listed(&self, session_id: &str) -> Result<String> {
        let deadline = std::time::Instant::now() + Duration::from_secs(15);
        loop {
            let mut args = self.invocation.prefix_args.clone();
            args.extend(["agents".into(), "--json".into(), "--all".into()]);
            let mut request = CommandRequest::new(self.invocation.program.clone(), args);
            request.timeout = Duration::from_secs(3);
            if let Ok(output) = self.runner.run(&request) {
                if output.status == 0 {
                    if let Ok(current) = output
                        .stdout_text()
                        .and_then(|text| parse_claude_sessions(text, Runtime::Host))
                    {
                        if let Some(session) = current.iter().find(|session| {
                            session.provider_session_id == session_id
                                || session_id.starts_with(&session.provider_session_id)
                                || session.provider_session_id.starts_with(session_id)
                        }) {
                            return Ok(session.provider_session_id.clone());
                        }
                    }
                }
            }
            if std::time::Instant::now() >= deadline {
                bail!(
                    "Claude accepted background launch {}, but did not list it within 15 seconds",
                    short_claude_id(session_id)
                );
            }
            std::thread::sleep(Duration::from_millis(150));
        }
    }

    fn stop(&self, session: &AgentSession) -> Result<ControlOutcome> {
        self.revalidate_interrupt_target(session)?;
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

    fn revalidate_interrupt_target(&self, session: &AgentSession) -> Result<()> {
        if session.provider != Provider::Claude || session.runtime != Runtime::Host {
            bail!("the Claude host controller cannot stop this provider runtime");
        }
        let mut args = self.invocation.prefix_args.clone();
        args.extend(["agents".into(), "--json".into()]);
        let mut request = CommandRequest::new(self.invocation.program.clone(), args);
        request.timeout = Duration::from_secs(8);
        let output = self.runner.run(&request)?;
        if output.status != 0 {
            bail!(
                "Claude session revalidation exited with status {}: {}",
                output.status,
                output.stderr_lossy()
            );
        }
        let current = parse_claude_sessions(output.stdout_text()?, Runtime::Host)?;
        let exact = current
            .iter()
            .find(|candidate| candidate.provider_session_id == session.provider_session_id)
            .context("the exact Claude session is no longer listed")?;
        if !is_interruptible_claude_session(exact) {
            bail!("the exact Claude session is no longer an active background session");
        }
        Ok(())
    }

    #[cfg(test)]
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

    fn launch_mode(&self) -> LaunchMode {
        LaunchMode::SelectableModel
    }

    fn available_models(&self) -> Result<Vec<String>> {
        let mut args = self.invocation.prefix_args.clone();
        args.push("--help".into());
        let mut request = CommandRequest::new(self.invocation.program.clone(), args);
        request.timeout = Duration::from_secs(5);
        let output = self.runner.run(&request)?;
        if output.status != 0 {
            bail!(
                "Claude model discovery exited with status {}: {}",
                output.status,
                output.stderr_lossy()
            );
        }
        parse_claude_model_aliases(output.stdout_text()?)
    }

    fn launch_presentation(&self) -> LaunchPresentation {
        if self.runtime == Runtime::Host {
            LaunchPresentation::DeferredForeground
        } else {
            LaunchPresentation::Background
        }
    }

    fn supports_authentication(&self) -> bool {
        self.runtime == Runtime::Host
    }

    fn authenticate(&self) -> Result<ControlOutcome> {
        run_native_authentication(
            &self.invocation.program,
            &["auth", "login"],
            Provider::Claude,
        )
    }

    fn enrich(&self, snapshot: &mut SessionSnapshot) {
        let registry = match self.registry.lock() {
            Ok(registry) => registry,
            Err(_) => {
                snapshot
                    .warnings
                    .push("Claude ownership registry lock was poisoned".into());
                return;
            }
        };
        let mut owned_indices = Vec::new();
        for (index, session) in snapshot.sessions.iter_mut().enumerate() {
            if registry.owns(session) {
                owned_indices.push(index);
                if is_interruptible_claude_session(session) {
                    session.capabilities.insert(Capability::Interrupt);
                }
            }
        }
        drop(registry);
        self.enrich_summaries(snapshot, &owned_indices);
    }

    fn launch(&self, request: &LaunchRequest) -> Result<ControlOutcome> {
        ClaudeController::launch(self, request)
    }

    fn launch_foreground(&self, request: &LaunchRequest) -> Result<ControlOutcome> {
        ClaudeController::launch_foreground(self, request)
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
        match &session.runtime {
            Runtime::Host => {
                let mut command = Command::new(&self.invocation.program);
                command
                    .args(["attach", &short_claude_id(&session.provider_session_id)])
                    .current_dir(&session.cwd);
                match crate::native_session::run(command, &session.id)? {
                    crate::native_session::NativeSessionExit::Backgrounded => Ok(ControlOutcome {
                        message: format!("backgrounded {}; Enter/Right resumes it", session.name),
                        provider_session_hint: Some(session.provider_session_id.clone()),
                    }),
                    crate::native_session::NativeSessionExit::Exited(status)
                        if status.success() =>
                    {
                        Ok(ControlOutcome {
                            message: format!("returned from {}", session.name),
                            provider_session_hint: None,
                        })
                    }
                    crate::native_session::NativeSessionExit::Exited(status) => {
                        bail!("Claude session exited with status {status}")
                    }
                }
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
                run_interactive(command, session)
            }
        }
    }
}

struct CodexController {
    supervisor: Option<Arc<CodexSupervisor>>,
    codex_bin: String,
    docker_bin: String,
}

impl ProviderController for CodexController {
    fn provider(&self) -> Provider {
        Provider::Codex
    }

    fn launch_mode(&self) -> LaunchMode {
        LaunchMode::SelectableModel
    }

    fn launch_presentation(&self) -> LaunchPresentation {
        if self.supervisor.is_some() {
            LaunchPresentation::DeferredForeground
        } else {
            LaunchPresentation::Foreground
        }
    }

    fn available_models(&self) -> Result<Vec<String>> {
        match &self.supervisor {
            Some(supervisor) => supervisor.available_models(),
            None => portable_codex_models(&self.codex_bin),
        }
    }

    fn supports_authentication(&self) -> bool {
        true
    }

    fn authenticate(&self) -> Result<ControlOutcome> {
        run_native_authentication(&self.codex_bin, &["login"], Provider::Codex)
    }

    fn enrich(&self, snapshot: &mut SessionSnapshot) {
        if let Some(supervisor) = &self.supervisor {
            supervisor.enrich(snapshot);
        }
    }

    fn launch(&self, request: &LaunchRequest) -> Result<ControlOutcome> {
        let supervisor = self
            .supervisor
            .as_ref()
            .context("durable Codex background launch is unavailable on this platform")?;
        let thread_id = supervisor.launch_with_model(
            &request.prompt,
            &request.cwd,
            request.model.as_deref(),
        )?;
        Ok(ControlOutcome {
            message: format!("started managed Codex thread {thread_id}"),
            provider_session_hint: Some(thread_id),
        })
    }

    fn launch_foreground(&self, request: &LaunchRequest) -> Result<ControlOutcome> {
        if self.supervisor.is_some() {
            return self.launch(request);
        }
        if request.provider != Provider::Codex || request.prompt.trim().is_empty() {
            bail!("the Codex launch prompt cannot be empty");
        }
        let mut command = Command::new(&self.codex_bin);
        if let Some(model) = request.model.as_deref() {
            command.args(["--model", model]);
        }
        command.arg(request.prompt.trim()).current_dir(&request.cwd);
        match crate::native_session::run(command, "codex:portable:new")? {
            crate::native_session::NativeSessionExit::Backgrounded => Ok(ControlOutcome {
                message: "backgrounded Codex session; refresh to discover its thread".into(),
                provider_session_hint: None,
            }),
            crate::native_session::NativeSessionExit::Exited(status) if status.success() => {
                Ok(ControlOutcome {
                    message: "returned from Codex; refresh to discover its thread".into(),
                    provider_session_hint: None,
                })
            }
            crate::native_session::NativeSessionExit::Exited(status) => {
                bail!("Codex exited with status {status}")
            }
        }
    }

    fn interrupt(&self, session: &AgentSession) -> Result<ControlOutcome> {
        self.supervisor
            .as_ref()
            .context("managed Codex interrupt is unavailable on this platform")?
            .interrupt(session)?;
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
            Runtime::Host => self
                .supervisor
                .as_ref()
                .context("managed Codex inspection is unavailable on this platform")?
                .inspect(session),
            Runtime::Docker { .. } => {
                bail!("Docker Codex transcript inspection is observe-only")
            }
        }
    }

    fn reply(&self, session: &AgentSession, prompt: &str) -> Result<ControlOutcome> {
        if session.runtime != Runtime::Host {
            bail!("inline reply is supported only for owned host Codex threads");
        }
        let mode = self
            .supervisor
            .as_ref()
            .context("managed Codex reply is unavailable on this platform")?
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

    fn archive(&self, session: &AgentSession) -> Result<ControlOutcome> {
        self.supervisor
            .as_ref()
            .context("managed Codex archive is unavailable on this platform")?
            .archive(session)?;
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
        self.supervisor
            .as_ref()
            .context("managed Codex approval is unavailable on this platform")?
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

    fn respond_input(&self, session: &AgentSession, answer: &str) -> Result<ControlOutcome> {
        if session.runtime != Runtime::Host {
            bail!("structured input is supported only for owned host Codex threads");
        }
        let progress = self
            .supervisor
            .as_ref()
            .context("managed Codex input is unavailable on this platform")?
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

    fn delete(&self, session: &AgentSession) -> Result<ControlOutcome> {
        self.supervisor
            .as_ref()
            .context("managed Codex deletion is unavailable on this platform")?
            .delete(session)?;
        Ok(ControlOutcome {
            message: format!(
                "deleted managed Codex thread {}",
                session.provider_session_id
            ),
            provider_session_hint: Some(session.provider_session_id.clone()),
        })
    }

    fn open(&self, session: &AgentSession) -> Result<ControlOutcome> {
        let remote = self
            .supervisor
            .as_ref()
            .and_then(|supervisor| supervisor.remote_url_if_owned(session));
        let command = match &session.runtime {
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
        run_interactive(command, session)
    }
}

/// Query the account-aware Codex catalog through a short-lived stdio App
/// Server. Windows cannot use the durable Unix-socket supervisor, but model
/// selection should remain available before handing the console to Codex.
fn portable_codex_models(executable: &str) -> Result<Vec<String>> {
    const PAGE_SIZE: u64 = 100;
    const MAX_PAGES: usize = 200;
    const MAX_MODELS: usize = 20_000;

    let mut client = AppServerClient::connect(&AppServerInvocation::direct(executable))?;
    let mut cursor: Option<String> = None;
    let mut seen = BTreeSet::new();
    let mut models = Vec::new();
    for _ in 0..MAX_PAGES {
        let response = client.request(
            "model/list",
            json!({
                "cursor": cursor,
                "limit": PAGE_SIZE,
                "includeHidden": false
            }),
        )?;
        let page = response
            .get("data")
            .and_then(Value::as_array)
            .context("model/list response omitted data")?;
        for item in page {
            if item.get("hidden").and_then(Value::as_bool) == Some(true) {
                continue;
            }
            let model = item
                .get("model")
                .or_else(|| item.get("id"))
                .and_then(Value::as_str)
                .context("model/list item omitted model and id")?;
            let model = validate_model(Some(model.to_owned()))?
                .context("Codex model/list returned an empty model")?;
            if seen.insert(model.clone()) {
                models.push(model);
            }
            if models.len() > MAX_MODELS {
                bail!("Codex model catalog exceeded {MAX_MODELS} entries");
            }
        }
        cursor = response
            .get("nextCursor")
            .and_then(Value::as_str)
            .map(ToOwned::to_owned);
        if cursor.is_none() {
            return Ok(models);
        }
    }
    bail!("Codex model catalog pagination did not terminate")
}

fn run_interactive(command: Command, session: &AgentSession) -> Result<ControlOutcome> {
    match crate::native_session::run(command, &session.id)? {
        crate::native_session::NativeSessionExit::Backgrounded => Ok(ControlOutcome {
            message: format!("backgrounded {}; Enter/Right resumes it", session.name),
            provider_session_hint: Some(session.provider_session_id.clone()),
        }),
        crate::native_session::NativeSessionExit::Exited(status) if status.success() => {
            Ok(ControlOutcome {
                message: format!("returned from {}", session.name),
                provider_session_hint: None,
            })
        }
        crate::native_session::NativeSessionExit::Exited(status) => {
            bail!("provider session exited with status {status}")
        }
    }
}

pub(crate) fn run_native_authentication(
    program: &str,
    args: &[&str],
    provider: Provider,
) -> Result<ControlOutcome> {
    let mut command = Command::new(program);
    command.args(args);
    let session_key = format!("setup:{}", provider.label());
    match crate::native_session::run(command, &session_key)
        .with_context(|| format!("failed to start {} login", provider.label()))?
    {
        crate::native_session::NativeSessionExit::Backgrounded => Ok(ControlOutcome {
            message: format!(
                "backgrounded {} setup; Enter/l resumes the same login terminal",
                provider.label()
            ),
            provider_session_hint: None,
        }),
        crate::native_session::NativeSessionExit::Exited(status) if status.success() => {
            Ok(ControlOutcome {
                message: format!(
                    "{} login completed; refreshing available models",
                    provider.label()
                ),
                provider_session_hint: None,
            })
        }
        crate::native_session::NativeSessionExit::Exited(status) => {
            bail!("{} login exited with status {status}", provider.label())
        }
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
        crate::fs_util::replace_file(&temporary, &self.path).with_context(|| {
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

fn parse_claude_background_id(output: &str) -> Result<String> {
    let rendered = output.trim();
    let id = rendered
        .lines()
        .find_map(|line| line.trim().strip_prefix("backgrounded · "))
        .map(str::trim)
        .context("Claude background launch did not report `backgrounded · SESSION_ID`")?;
    if id.len() != 8 || !id.bytes().all(|byte| byte.is_ascii_hexdigit()) {
        bail!("Claude background launch returned an invalid session ID");
    }
    Ok(id.to_ascii_lowercase())
}

#[cfg(test)]
fn generate_session_id() -> Result<String> {
    let mut bytes = [0_u8; 16];
    getrandom::getrandom(&mut bytes).context("failed to generate a Claude session ID")?;
    bytes[6] = (bytes[6] & 0x0f) | 0x40;
    bytes[8] = (bytes[8] & 0x3f) | 0x80;
    Ok(format!(
        "{:02x}{:02x}{:02x}{:02x}-{:02x}{:02x}-{:02x}{:02x}-{:02x}{:02x}-{:02x}{:02x}{:02x}{:02x}{:02x}{:02x}",
        bytes[0], bytes[1], bytes[2], bytes[3], bytes[4], bytes[5], bytes[6], bytes[7],
        bytes[8], bytes[9], bytes[10], bytes[11], bytes[12], bytes[13], bytes[14], bytes[15]
    ))
}

fn parse_claude_model_aliases(help: &str) -> Result<Vec<String>> {
    let mut description = String::new();
    let mut found = false;
    for line in help.lines() {
        if !found {
            if line.contains("--model <model>") {
                found = true;
                description.push_str(line);
                description.push('\n');
            }
            continue;
        }
        if line.trim_start().starts_with('-') {
            break;
        }
        description.push_str(line);
        description.push('\n');
        if description.lines().count() >= 8 {
            break;
        }
    }
    if !found {
        bail!("Claude help omitted --model <model>");
    }

    let mut models = BTreeSet::new();
    // Inspect each apostrophe-delimited segment independently. This remains
    // correct when prose includes an apostrophe such as "model's full name"
    // immediately before a quoted model identifier.
    for candidate in description.split('\'') {
        if let Ok(Some(model)) = validate_model(Some(candidate.to_owned())) {
            if model
                .chars()
                .next()
                .is_some_and(|character| character.is_ascii_alphanumeric())
                && model.chars().all(|character| {
                    character.is_ascii_alphanumeric()
                        || matches!(character, '-' | '_' | '.' | ':' | '/' | '@')
                })
            {
                models.insert(model);
            }
        }
    }
    Ok(models.into_iter().collect())
}

fn short_claude_id(provider_session_id: &str) -> String {
    provider_session_id.chars().take(8).collect()
}

fn is_interruptible_claude_session(session: &AgentSession) -> bool {
    session.provider == Provider::Claude
        && session.runtime == Runtime::Host
        && session.kind == SessionKind::Background
        && matches!(
            session.state,
            SessionState::Working | SessionState::NeedsInput | SessionState::ReadyForReview
        )
}

fn validate_model(model: Option<String>) -> Result<Option<String>> {
    let Some(model) = model else {
        return Ok(None);
    };
    let model = model.trim();
    if model.is_empty() || model.len() > 128 {
        bail!("the model name must contain between 1 and 128 bytes");
    }
    if model
        .chars()
        .any(|character| character.is_control() || character.is_whitespace())
    {
        bail!("the model name cannot contain whitespace or control characters");
    }
    Ok(Some(model.to_owned()))
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

fn claude_summary_from_terminal(bytes: &[u8]) -> Option<String> {
    let mut parser = vt100::Parser::new(100, 200, 0);
    parser.process(bytes);
    summarize_claude_screen(&parser.screen().contents())
}

fn summarize_claude_screen(screen: &str) -> Option<String> {
    let lines = screen.lines().collect::<Vec<_>>();
    let recap = lines.iter().rposition(|line| {
        line.trim_start()
            .to_ascii_lowercase()
            .starts_with("※ recap:")
    });
    if let Some(start) = recap {
        let first = lines[start]
            .trim()
            .strip_prefix("※ recap:")
            .unwrap_or(lines[start].trim());
        return bounded_claude_message(first, &lines[start + 1..]);
    }

    let assistant = lines
        .iter()
        .rposition(|line| line.trim_start().starts_with('●'))?;
    let first = lines[assistant]
        .trim_start()
        .strip_prefix('●')
        .unwrap_or(lines[assistant])
        .trim();
    bounded_claude_message(first, &lines[assistant + 1..])
}

fn bounded_claude_message(first: &str, continuation: &[&str]) -> Option<String> {
    const MAX_LINES: usize = 4;
    const MAX_CHARS: usize = 320;
    let mut parts = Vec::new();
    if !first.trim().is_empty() {
        parts.push(first.trim());
    }
    for line in continuation.iter().take(MAX_LINES.saturating_sub(1)) {
        let line = line.trim();
        if line.is_empty() || is_claude_screen_boundary(line) {
            break;
        }
        parts.push(line);
    }
    let normalized = parts
        .join(" ")
        .split_whitespace()
        .collect::<Vec<_>>()
        .join(" ");
    if normalized.is_empty() {
        None
    } else {
        Some(normalized.chars().take(MAX_CHARS).collect())
    }
}

fn is_claude_screen_boundary(line: &str) -> bool {
    line.starts_with(['❯', '✻', '※', '✔', '⏵', '─', '━'])
        || line.starts_with("Brewed for")
        || line.starts_with("Churned for")
        || line.starts_with("new task?")
        || line.contains("auto mode on")
}

#[cfg(test)]
mod tests {
    use std::collections::{BTreeSet, VecDeque};
    use std::sync::Mutex;

    use tempfile::tempdir;

    use crate::domain::{Runtime, SessionKind, SessionState};
    use crate::migration::{MigrationOutcome, MigrationRequest};
    use crate::process::CommandOutput;

    use super::*;

    #[test]
    fn generated_claude_session_ids_are_distinct_version_four_uuids() {
        let first = generate_session_id().unwrap();
        let second = generate_session_id().unwrap();

        assert_ne!(first, second);
        assert_eq!(first.len(), 36);
        assert_eq!(&first[14..15], "4");
        assert!(matches!(&first[19..20], "8" | "9" | "a" | "b"));
    }

    #[test]
    fn parses_current_claude_model_aliases_without_guessing_other_options() {
        let help = "  --fallback-model <model> fallback\n  --model <model> Model for the current session. Provide\n                  an alias for the latest model (e.g.\n                  'fable', 'opus', or 'sonnet') or a\n                  model's full name (e.g. 'claude-fable-5').\n  -n, --name <name> display name\n";
        assert_eq!(
            parse_claude_model_aliases(help).unwrap(),
            vec!["claude-fable-5", "fable", "opus", "sonnet"]
        );
        assert!(parse_claude_model_aliases("--model-id value").is_err());
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
    fn owned_view_removes_external_host_claude_without_hiding_other_managed_sources() {
        let directory = tempdir().unwrap();
        let mut registry =
            OwnershipRegistry::load(directory.path().join("ownership.json")).unwrap();
        registry
            .record(OwnedSession {
                provider: Provider::Claude,
                runtime_key: "host".into(),
                provider_id_prefix: "owned123".into(),
                created_at_ms: 1,
            })
            .unwrap();
        let hub = ControlHub {
            controllers: BTreeMap::new(),
            codex: None,
            claude_registry: Some(Arc::new(Mutex::new(registry))),
            launch_provider: Provider::Claude,
            launch_cwd: PathBuf::from("/work"),
            provider_io_enabled: true,
            migration_registry: None,
        };
        let mut owned = session("owned123-full");
        let external = session("external-full");
        let mut docker = session("docker-full");
        docker.runtime = Runtime::Docker {
            container_id: "exact".into(),
            container_name: "managed".into(),
            image: "image@sha256:exact".into(),
        };
        let mut codex = session("codex-owned");
        codex.provider = Provider::Codex;
        owned.name = "owned".into();
        let mut snapshot = SessionSnapshot {
            sessions: vec![owned, external, docker, codex],
            warnings: Vec::new(),
        };

        hub.retain_owned(&mut snapshot);

        assert_eq!(
            snapshot
                .sessions
                .iter()
                .map(|session| session.provider_session_id.as_str())
                .collect::<Vec<_>>(),
            vec!["owned123-full", "docker-full", "codex-owned"]
        );
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
    fn claude_terminal_summary_prefers_the_latest_recap_and_stops_before_chrome() {
        let screen = "● Earlier response\n\n※ recap: The provider metadata is now fresh and\n  the dashboard shows the latest answer.\n✔ Update installed\n──────────────── title\n❯ next prompt\n⏵ auto mode on";

        assert_eq!(
            summarize_claude_screen(screen).as_deref(),
            Some("The provider metadata is now fresh and the dashboard shows the latest answer.")
        );
    }

    #[test]
    fn claude_terminal_summary_falls_back_to_the_latest_assistant_paragraph() {
        let screen =
            "❯ task\n\n● First line of the latest answer\n  continues here.\n\n✻ Churned for 3s\n❯";

        assert_eq!(
            summarize_claude_screen(screen).as_deref(),
            Some("First line of the latest answer continues here.")
        );
    }

    #[test]
    fn launch_records_ownership_and_uses_argument_arrays() {
        let directory = tempdir().unwrap();
        let registry = OwnershipRegistry::load(directory.path().join("ownership.json")).unwrap();
        let session_id = "deadbeef-91dc-4b50-a43f-6db2837576fe";
        let inventory = format!(
            r#"[{{"id":"deadbeef","sessionId":"{session_id}","cwd":"/work/project","kind":"background","name":"dashboard","state":"working"}}]"#
        );
        let runner = Arc::new(QueueRunner {
            requests: Mutex::new(Vec::new()),
            outputs: Mutex::new(VecDeque::from([
                CommandOutput {
                    status: 0,
                    stdout: "backgrounded · deadbeef\n".as_bytes().to_vec(),
                    stderr: vec![],
                },
                CommandOutput {
                    status: 0,
                    stdout: inventory.into_bytes(),
                    stderr: vec![],
                },
            ])),
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
            summary_cache: Mutex::new(BTreeMap::new()),
        };

        let outcome = controller
            .launch(&LaunchRequest {
                provider: Provider::Claude,
                model: Some("opus".into()),
                prompt: "Implement the dashboard".into(),
                cwd: PathBuf::from("/work/project"),
            })
            .unwrap();

        assert_eq!(outcome.provider_session_hint.as_deref(), Some(session_id));
        let requests = runner.requests.lock().unwrap();
        assert_eq!(requests.len(), 2);
        assert_eq!(requests[0].program, "claude-test");
        assert_eq!(
            requests[0].args,
            vec!["--model", "opus", "--background", "Implement the dashboard"]
        );
        assert_eq!(
            requests[0].current_dir,
            Some(PathBuf::from("/work/project"))
        );
        assert_eq!(requests[0].timeout, Duration::from_secs(45));
        assert_eq!(requests[1].args, vec!["agents", "--json", "--all"]);
        assert!(controller.owns(&session(session_id)));
    }

    #[test]
    fn claude_background_id_parser_accepts_only_the_current_exact_contract() {
        assert_eq!(
            parse_claude_background_id("backgrounded · A1b2C3d4\n").unwrap(),
            "a1b2c3d4"
        );
        assert_eq!(
            parse_claude_background_id("status\n  backgrounded · deadbeef\n").unwrap(),
            "deadbeef"
        );
        for malformed in [
            "",
            "warning: --bg manages the session id",
            "backgrounded deadbeef",
            "backgrounded · short",
            "backgrounded · zzzzzzzz",
            "backgrounded · deadbeef-extra",
        ] {
            assert!(
                parse_claude_background_id(malformed).is_err(),
                "{malformed}"
            );
        }
    }

    #[cfg(unix)]
    #[test]
    fn foreground_claude_launch_waits_for_the_exact_background_row_then_attaches() {
        use std::os::unix::fs::PermissionsExt;

        let directory = tempdir().unwrap();
        let executable = directory.path().join("claude-mock");
        let state = directory.path().join("session-id");
        let attached = directory.path().join("attached");
        fs::write(
            &executable,
            format!(
                r##"#!/bin/sh
if [ "${{1:-}}" = agents ]; then
  id=$(cat '{}')
  printf '[{{"cwd":"{}","kind":"background","sessionId":"%s","name":"test","state":"working"}}]\n' "$id"
  exit 0
fi
if [ "${{1:-}}" = attach ]; then
  printf '%s' "${{2:-}}" > '{}'
  exit 0
fi
printf '%s' 'deadbeef-91dc-4b50-a43f-6db2837576fe' > '{}'
printf '%s\n' 'backgrounded · deadbeef'
exit 0
"##,
                state.display(),
                directory.path().display(),
                attached.display(),
                state.display(),
            ),
        )
        .unwrap();
        fs::set_permissions(&executable, fs::Permissions::from_mode(0o700)).unwrap();
        let registry = OwnershipRegistry::load(directory.path().join("ownership.json")).unwrap();
        let controller = ClaudeController {
            invocation: ControlInvocation {
                program: executable.display().to_string(),
                prefix_args: Vec::new(),
            },
            docker_bin: "must-not-run-docker".into(),
            runtime: Runtime::Host,
            runner: Arc::new(ProcessRunner),
            registry: Arc::new(Mutex::new(registry)),
            summary_cache: Mutex::new(BTreeMap::new()),
        };

        let outcome = controller
            .launch_foreground(&LaunchRequest {
                provider: Provider::Claude,
                model: Some("sonnet".into()),
                prompt: "work in background".into(),
                cwd: directory.path().to_path_buf(),
            })
            .unwrap();

        let id = outcome.provider_session_hint.unwrap();
        assert_eq!(fs::read_to_string(attached).unwrap(), &id[..8]);
        assert!(controller.owns(&session(&id)));
    }

    #[test]
    fn claude_interrupt_is_granted_only_to_an_owned_exact_background_session() {
        let directory = tempdir().unwrap();
        let provider_id = "4b34abd1-91dc-4b50-a43f-6db2837576fe";
        let mut registry =
            OwnershipRegistry::load(directory.path().join("ownership.json")).unwrap();
        registry
            .record(OwnedSession {
                provider: Provider::Claude,
                runtime_key: "host".into(),
                provider_id_prefix: "4b34abd1".into(),
                created_at_ms: 1,
            })
            .unwrap();
        let inventory = format!(
            r#"[{{"pid":42,"id":"4b34abd1","cwd":"/work","kind":"background","startedAt":1,"sessionId":"{provider_id}","name":"external","status":"busy","state":"working"}}]"#
        );
        let runner = Arc::new(QueueRunner {
            requests: Mutex::new(Vec::new()),
            outputs: Mutex::new(VecDeque::from([
                CommandOutput {
                    status: 0,
                    stdout: b"\x1b[2J\x1b[H\x1b[2;1H\xe2\x80\xbb recap: latest Claude result\r\n\xe2\x9d\xaf next"
                        .to_vec(),
                    stderr: vec![],
                },
                CommandOutput {
                    status: 0,
                    stdout: inventory.into_bytes(),
                    stderr: vec![],
                },
                CommandOutput {
                    status: 0,
                    stdout: vec![],
                    stderr: vec![],
                },
            ])),
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
            summary_cache: Mutex::new(BTreeMap::new()),
        };
        let external = session(provider_id);
        let mut snapshot = SessionSnapshot {
            sessions: vec![external.clone()],
            warnings: Vec::new(),
        };

        controller.enrich(&mut snapshot);
        assert!(snapshot.sessions[0]
            .capabilities
            .contains(&Capability::Interrupt));
        assert_eq!(snapshot.sessions[0].summary, "latest Claude result");
        let mut refreshed_snapshot = SessionSnapshot {
            sessions: vec![external.clone()],
            warnings: Vec::new(),
        };
        controller.enrich(&mut refreshed_snapshot);
        assert_eq!(
            refreshed_snapshot.sessions[0].summary,
            "latest Claude result"
        );
        assert!(controller.owns(&external));
        controller.interrupt(&external).unwrap();

        let requests = runner.requests.lock().unwrap();
        assert_eq!(requests.len(), 3);
        assert_eq!(requests[0].args, vec!["logs", "4b34abd1"]);
        assert_eq!(requests[1].args, vec!["agents", "--json"]);
        assert_eq!(requests[2].args, vec!["stop", "4b34abd1"]);
    }

    #[test]
    fn claude_interrupt_never_targets_interactive_completed_or_stale_sessions() {
        let directory = tempdir().unwrap();
        let registry = OwnershipRegistry::load(directory.path().join("ownership.json")).unwrap();
        let provider_id = "4b34abd1-91dc-4b50-a43f-6db2837576fe";
        let runner = Arc::new(QueueRunner {
            requests: Mutex::new(Vec::new()),
            outputs: Mutex::new(VecDeque::from([CommandOutput {
                status: 0,
                stdout: b"[]".to_vec(),
                stderr: vec![],
            }])),
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
            summary_cache: Mutex::new(BTreeMap::new()),
        };
        let mut completed = session("completed");
        completed.state = SessionState::Completed;
        let mut interactive = session("interactive");
        interactive.kind = SessionKind::Interactive;
        let mut docker = session("docker");
        docker.runtime = Runtime::Docker {
            container_id: "exact".into(),
            container_name: "container".into(),
            image: "image@sha256:exact".into(),
        };
        let mut snapshot = SessionSnapshot {
            sessions: vec![completed, interactive, docker],
            warnings: Vec::new(),
        };

        controller.enrich(&mut snapshot);
        assert!(snapshot
            .sessions
            .iter()
            .all(|item| !item.capabilities.contains(&Capability::Interrupt)));
        assert_eq!(
            controller
                .interrupt(&session(provider_id))
                .unwrap_err()
                .to_string(),
            "the exact Claude session is no longer listed"
        );
        assert_eq!(runner.requests.lock().unwrap().len(), 1);
    }

    #[test]
    fn fixture_mode_fences_every_provider_io_path_before_dispatch() {
        let directory = tempdir().unwrap();
        let hub = ControlHub {
            controllers: BTreeMap::new(),
            codex: None,
            claude_registry: None,
            launch_provider: Provider::Claude,
            launch_cwd: directory.path().into(),
            provider_io_enabled: false,
            migration_registry: None,
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
        assert_fenced(hub.authenticate(&Provider::Terminal));
        assert_fenced(hub.setup_provider(&Provider::Terminal));
        assert_fenced(hub.setup_launch_option(&Provider::Terminal, "install-shell:fish"));
        assert_eq!(
            hub.available_models(&Provider::Terminal)
                .unwrap_err()
                .to_string(),
            "provider actions are disabled while reading a fixture"
        );
        assert!(hub.codex_supervisor().is_none());
    }

    fn uncontrolled_hub(launch_provider: Provider) -> ControlHub {
        ControlHub {
            controllers: BTreeMap::new(),
            codex: None,
            claude_registry: None,
            launch_provider,
            launch_cwd: PathBuf::from("/work"),
            provider_io_enabled: true,
            migration_registry: None,
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
    fn claude_launch_rejects_empty_input_and_rolls_back_a_spawn_failure() {
        let directory = tempdir().unwrap();
        let registry = OwnershipRegistry::load(directory.path().join("ownership.json")).unwrap();
        let runner = Arc::new(QueueRunner {
            requests: Mutex::new(Vec::new()),
            outputs: Mutex::new(VecDeque::new()),
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
            summary_cache: Mutex::new(BTreeMap::new()),
        };
        assert_eq!(
            controller
                .launch(&LaunchRequest {
                    provider: Provider::Claude,
                    model: None,
                    prompt: "   ".into(),
                    cwd: PathBuf::from("/work"),
                })
                .unwrap_err()
                .to_string(),
            "the launch prompt cannot be empty"
        );
        assert!(runner.requests.lock().unwrap().is_empty());
        let error = controller
            .launch(&LaunchRequest {
                provider: Provider::Claude,
                model: None,
                prompt: "prompt".into(),
                cwd: PathBuf::from("/work"),
            })
            .unwrap_err()
            .to_string();
        assert!(error.contains("test runner exhausted"));
        assert_eq!(runner.requests.lock().unwrap().len(), 1);
        assert!(controller.registry.lock().unwrap().records.is_empty());
    }

    struct QueueRunner {
        requests: Mutex<Vec<CommandRequest>>,
        outputs: Mutex<VecDeque<CommandOutput>>,
    }

    struct StubController {
        provider: Provider,
        marker: &'static str,
    }

    impl ProviderController for StubController {
        fn provider(&self) -> Provider {
            self.provider.clone()
        }

        fn launch_mode(&self) -> LaunchMode {
            LaunchMode::DefaultModel
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

        fn open(&self, session: &AgentSession) -> Result<ControlOutcome> {
            assert_eq!(session.provider, self.provider);
            Ok(ControlOutcome {
                message: format!("opened {} normally", self.marker),
                provider_session_hint: None,
            })
        }

        fn open_imported(&self, session: &AgentSession) -> Result<ControlOutcome> {
            assert_eq!(session.provider, self.provider);
            Ok(ControlOutcome {
                message: format!("opened {} import", self.marker),
                provider_session_hint: None,
            })
        }
    }

    impl CommandRunner for QueueRunner {
        fn run(&self, request: &CommandRequest) -> Result<CommandOutput> {
            self.requests.lock().unwrap().push(request.clone());
            self.outputs
                .lock()
                .unwrap()
                .pop_front()
                .context("test runner exhausted")
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
        assert_eq!(
            hub.launch_targets(),
            vec![LaunchTarget {
                provider: Provider::Pi,
                supports_model: false,
            }]
        );
        assert_eq!(
            hub.launch_with(Provider::Pi, Some("custom".into()), "prompt".into())
                .unwrap_err()
                .to_string(),
            "Pi does not expose model selection"
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

    #[test]
    fn exact_migration_record_selects_import_open_and_survives_claude_filter() {
        let directory = tempdir().unwrap();
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            fs::set_permissions(directory.path(), fs::Permissions::from_mode(0o700)).unwrap();
        }
        let registry = MigrationRegistry::load(directory.path().join("migrations.json")).unwrap();
        let mut source_session = session("source");
        source_session.provider = Provider::Codex;
        source_session.id = "codex:host:source".into();
        registry
            .record(
                &MigrationRequest {
                    source: source_session,
                    target: Provider::Claude,
                    name: "source (Claude)".into(),
                },
                &MigrationOutcome {
                    session_id: "target".into(),
                    normalized_id: "claude:host:target".into(),
                    warnings: Vec::new(),
                },
            )
            .unwrap();

        let mut hub = uncontrolled_hub(Provider::Claude);
        hub.register_migration_registry(registry);
        hub.register_controller(Arc::new(StubController {
            provider: Provider::Claude,
            marker: "claude",
        }))
        .unwrap();
        let imported = session("target");
        assert_eq!(hub.open(&imported).unwrap().message, "opened claude import");

        let lookalike = session("target-other");
        assert_eq!(
            hub.open(&lookalike).unwrap().message,
            "opened claude normally"
        );
        let mut snapshot = SessionSnapshot {
            sessions: vec![imported, lookalike],
            warnings: Vec::new(),
        };
        hub.retain_owned(&mut snapshot);
        assert_eq!(snapshot.sessions.len(), 1);
        assert_eq!(snapshot.sessions[0].provider_session_id, "target");
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

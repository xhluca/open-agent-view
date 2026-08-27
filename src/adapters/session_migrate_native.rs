//! Native integrations shared with Session Migrate's extended harness matrix.
//!
//! Oh My Pi, Grok, Kilo Code, and OpenHands all expose durable native session
//! identifiers and a native resume command. OAV observes their bounded public
//! session stores, but grants stop authority only after it has correlated one
//! new exact ID with a foreground launch and persisted that ownership privately.

use std::collections::{BTreeMap, BTreeSet};
use std::fs::{self, File, OpenOptions};
use std::io::{BufRead, BufReader, Read};
#[cfg(unix)]
use std::os::unix::fs::OpenOptionsExt;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::Arc;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use anyhow::{bail, Context, Result};
use serde_json::Value;

use super::native_owned::{
    poll_unique, sanitize, validate_id, NativeOwnership, OwnedNativeSession,
};
use super::{DiscoveryRequest, SessionSource};
use crate::control::{
    run_native_authentication, ControlOutcome, LaunchMode, LaunchPresentation, LaunchRequest,
    ProviderController,
};
use crate::domain::{
    AgentSession, Capability, Provider, Runtime, SessionKind, SessionSnapshot, SessionState,
};
use crate::process::{CommandRequest, CommandRunner, ProcessRunner};

const MAX_JSON_BYTES: u64 = 8 * 1024 * 1024;
const MAX_JOURNAL_BYTES: u64 = 32 * 1024 * 1024;
const MAX_SESSION_SCAN: usize = 10_000;

pub struct SessionMigrateNativeOwnership {
    provider: Provider,
    inner: NativeOwnership,
}

impl SessionMigrateNativeOwnership {
    pub fn load_default(provider: Provider) -> Result<Arc<Self>> {
        require_supported(&provider)?;
        let slug = provider_slug(&provider);
        let path = if let Some(state_home) = std::env::var_os("XDG_STATE_HOME") {
            PathBuf::from(state_home).join(format!("open-agent-view/{slug}-owned.json"))
        } else {
            let home = std::env::var_os("HOME").context("HOME is not set")?;
            PathBuf::from(home).join(format!(".local/state/open-agent-view/{slug}-owned.json"))
        };
        Self::load(provider, path)
    }

    pub fn load(provider: Provider, path: PathBuf) -> Result<Arc<Self>> {
        require_supported(&provider)?;
        let label = provider.label().to_owned();
        Ok(Arc::new(Self {
            provider,
            inner: NativeOwnership::load(path, &label)?,
        }))
    }
}

pub struct SessionMigrateNativeSource {
    provider: Provider,
    executable: String,
    data_root: PathBuf,
    ownership: Arc<SessionMigrateNativeOwnership>,
    runner: Arc<dyn CommandRunner>,
}

impl SessionMigrateNativeSource {
    pub fn host_default(
        provider: Provider,
        executable: impl Into<String>,
        ownership: Arc<SessionMigrateNativeOwnership>,
    ) -> Result<Self> {
        let data_root = default_data_root(&provider)?;
        Self::host(provider, executable, data_root, ownership)
    }

    /// Construct a host source with an explicit provider state root. This is
    /// primarily useful for isolated containers and native contract tests.
    pub fn host(
        provider: Provider,
        executable: impl Into<String>,
        data_root: PathBuf,
        ownership: Arc<SessionMigrateNativeOwnership>,
    ) -> Result<Self> {
        require_matching_ownership(&provider, &ownership)?;
        Ok(Self {
            provider,
            executable: executable.into(),
            data_root,
            ownership,
            runner: Arc::new(ProcessRunner),
        })
    }
}

impl SessionSource for SessionMigrateNativeSource {
    fn label(&self) -> &str {
        match self.provider {
            Provider::OhMyPi => "Oh My Pi (host)",
            Provider::Grok => "Grok (host)",
            Provider::KiloCode => "Kilo Code (host)",
            Provider::OpenHands => "OpenHands (host)",
            _ => "unsupported native harness",
        }
    }

    fn discover(&self, request: &DiscoveryRequest) -> Result<Vec<AgentSession>> {
        let owned = self
            .ownership
            .inner
            .records()
            .into_iter()
            .map(|record| (record.session_id.clone(), record))
            .collect::<BTreeMap<_, _>>();

        let mut stored = if request.include_external || self.provider == Provider::KiloCode {
            list_sessions(
                &self.provider,
                &self.executable,
                &self.data_root,
                if request.include_external {
                    request.history_limit.max(1).min(MAX_SESSION_SCAN)
                } else {
                    MAX_SESSION_SCAN
                },
                self.runner.as_ref(),
            )?
        } else {
            owned
                .values()
                .filter_map(|record| {
                    read_owned_session(&self.provider, &self.data_root, record).transpose()
                })
                .collect::<Result<Vec<_>>>()?
        };
        stored.retain(|record| request.include_external || owned.contains_key(&record.session_id));
        stored
            .into_iter()
            .map(|record| {
                let owner = owned.get(&record.session_id);
                stored_to_agent(&self.provider, record, owner)
            })
            .collect()
    }
}

pub struct SessionMigrateNativeController {
    provider: Provider,
    executable: String,
    data_root: PathBuf,
    ownership: Arc<SessionMigrateNativeOwnership>,
    runner: Arc<dyn CommandRunner>,
}

impl SessionMigrateNativeController {
    pub fn host_default(
        provider: Provider,
        executable: impl Into<String>,
        ownership: Arc<SessionMigrateNativeOwnership>,
    ) -> Result<Self> {
        let data_root = default_data_root(&provider)?;
        Self::host(provider, executable, data_root, ownership)
    }

    /// Construct a controller with an explicit provider state root. Provider
    /// credentials still remain entirely inside the native CLI.
    pub fn host(
        provider: Provider,
        executable: impl Into<String>,
        data_root: PathBuf,
        ownership: Arc<SessionMigrateNativeOwnership>,
    ) -> Result<Self> {
        require_matching_ownership(&provider, &ownership)?;
        Ok(Self {
            data_root,
            provider,
            executable: executable.into(),
            ownership,
            runner: Arc::new(ProcessRunner),
        })
    }
}

impl ProviderController for SessionMigrateNativeController {
    fn provider(&self) -> Provider {
        self.provider.clone()
    }

    fn launch_mode(&self) -> LaunchMode {
        LaunchMode::SelectableModel
    }

    fn launch_presentation(&self) -> LaunchPresentation {
        LaunchPresentation::Foreground
    }

    fn available_models(&self) -> Result<Vec<String>> {
        match self.provider {
            Provider::OhMyPi => run_models(
                &self.executable,
                &["models", "list", "--no-extensions", "--json"],
                parse_omp_models,
                self.runner.as_ref(),
            ),
            Provider::Grok => run_models(
                &self.executable,
                &["models"],
                parse_grok_models,
                self.runner.as_ref(),
            ),
            Provider::KiloCode => run_models(
                &self.executable,
                &["models"],
                parse_line_models,
                self.runner.as_ref(),
            ),
            Provider::OpenHands => {
                let mut models = BTreeSet::new();
                if let Some(model) = std::env::var_os("LLM_MODEL") {
                    let model = model.to_string_lossy();
                    if valid_text(&model) {
                        models.insert(model.into_owned());
                    }
                }
                for model in list_openhands_models(&self.data_root, MAX_SESSION_SCAN)? {
                    models.insert(model);
                }
                Ok(models.into_iter().collect())
            }
            _ => bail!("unsupported native harness"),
        }
    }

    fn supports_authentication(&self) -> bool {
        true
    }

    fn authenticate(&self) -> Result<ControlOutcome> {
        match self.provider {
            Provider::OhMyPi => {
                run_native_authentication(&self.executable, &["--no-session"], Provider::OhMyPi)
            }
            Provider::Grok => {
                run_native_authentication(&self.executable, &["login"], Provider::Grok)
            }
            Provider::KiloCode => {
                run_native_authentication(&self.executable, &["auth", "login"], Provider::KiloCode)
            }
            Provider::OpenHands => {
                run_native_authentication(&self.executable, &["login"], Provider::OpenHands)
            }
            _ => bail!("unsupported native harness"),
        }
    }

    fn enrich(&self, snapshot: &mut SessionSnapshot) {
        for session in snapshot.sessions.iter_mut().filter(|session| {
            session.provider == self.provider
                && self.ownership.inner.owns(&session.provider_session_id)
        }) {
            if crate::native_session::is_backgrounded(&session.id) {
                session.state = SessionState::Working;
                session.kind = SessionKind::Managed;
                session.capabilities.insert(Capability::Interrupt);
            }
        }
    }

    fn launch_foreground(&self, request: &LaunchRequest) -> Result<ControlOutcome> {
        if request.provider != self.provider {
            bail!(
                "the {} controller cannot launch another provider",
                self.provider.label()
            );
        }
        require_cwd(&request.cwd, self.provider.label())?;
        if !valid_text(&request.prompt) {
            bail!("{} prompt is invalid", self.provider.label());
        }
        if request
            .model
            .as_deref()
            .is_some_and(|model| !valid_text(model))
        {
            bail!("{} model is invalid", self.provider.label());
        }

        let before = list_sessions(
            &self.provider,
            &self.executable,
            &self.data_root,
            MAX_SESSION_SCAN,
            self.runner.as_ref(),
        )?
        .into_iter()
        .map(|record| record.session_id)
        .collect::<BTreeSet<_>>();
        let launch_nonce = crate::native_session::new_session_id()?;
        let launch_key = format!(
            "{}:host:launch-{launch_nonce}",
            provider_slug(&self.provider)
        );
        let command = launch_command(&self.provider, &self.executable, request)?;
        let exit = crate::native_session::run(command, &launch_key)?;
        let record = poll_unique(
            &format!(
                "one new {} session in the requested workspace",
                self.provider.label()
            ),
            Duration::from_secs(8),
            || {
                Ok(list_sessions(
                    &self.provider,
                    &self.executable,
                    &self.data_root,
                    MAX_SESSION_SCAN,
                    self.runner.as_ref(),
                )?
                .into_iter()
                .filter(|record| {
                    !before.contains(&record.session_id)
                        && same_workspace(&record.cwd, &request.cwd)
                })
                .collect())
            },
        )?;
        self.ownership.inner.record(
            &record.session_id,
            &request.cwd,
            &request.prompt,
            record.path.as_deref(),
            self.provider.label(),
        )?;
        if matches!(exit, crate::native_session::NativeSessionExit::Backgrounded) {
            crate::native_session::rename_key(
                &launch_key,
                &session_key(&self.provider, &record.session_id),
            )?;
        }
        native_outcome(exit, &self.provider, &record.session_id)
    }

    fn open(&self, session: &AgentSession) -> Result<ControlOutcome> {
        validate_session(&self.provider, session)?;
        if crate::native_session::is_backgrounded(&session.id) {
            return native_outcome(
                crate::native_session::resume(&session.id)?,
                &self.provider,
                &session.provider_session_id,
            );
        }
        let resume_path = self
            .ownership
            .inner
            .records()
            .into_iter()
            .find(|record| record.session_id == session.provider_session_id)
            .and_then(|record| record.session_path);
        let command = resume_command(
            &self.provider,
            &self.executable,
            &session.cwd,
            &session.provider_session_id,
            resume_path.as_deref(),
        )?;
        native_outcome(
            crate::native_session::run(command, &session.id)?,
            &self.provider,
            &session.provider_session_id,
        )
    }

    fn interrupt(&self, session: &AgentSession) -> Result<ControlOutcome> {
        validate_session(&self.provider, session)?;
        if !self.ownership.inner.owns(&session.provider_session_id) {
            bail!(
                "refusing to stop a {} session not created by Open Agent View",
                self.provider.label()
            );
        }
        if !crate::native_session::is_backgrounded(&session.id) {
            bail!(
                "{} is not backgrounded in this dashboard process",
                self.provider.label()
            );
        }
        crate::native_session::terminate(&session.id)?;
        Ok(ControlOutcome {
            message: format!("stopped {} session {}", self.provider.label(), session.name),
            provider_session_hint: Some(session.provider_session_id.clone()),
        })
    }

    fn inspect(&self, session: &AgentSession) -> Result<String> {
        validate_session(&self.provider, session)?;
        Ok(format!(
            "{}: {}\nSession: {}\nDirectory: {}\nState: {}\n\nEnter or Right opens the exact session in the provider's native TUI.",
            self.provider.label(),
            session.name,
            session.provider_session_id,
            session.cwd.display(),
            session.state.heading()
        ))
    }
}

#[derive(Clone, Debug)]
struct StoredSession {
    session_id: String,
    cwd: PathBuf,
    name: String,
    summary: String,
    model: Option<String>,
    created_at: Option<SystemTime>,
    updated_at: Option<SystemTime>,
    path: Option<PathBuf>,
}

fn require_supported(provider: &Provider) -> Result<()> {
    if matches!(
        provider,
        Provider::OhMyPi | Provider::Grok | Provider::KiloCode | Provider::OpenHands
    ) {
        Ok(())
    } else {
        bail!(
            "{} is not a Session Migrate native harness",
            provider.label()
        )
    }
}

fn require_matching_ownership(
    provider: &Provider,
    ownership: &SessionMigrateNativeOwnership,
) -> Result<()> {
    require_supported(provider)?;
    if provider != &ownership.provider {
        bail!("native harness ownership registry belongs to another provider");
    }
    Ok(())
}

fn default_data_root(provider: &Provider) -> Result<PathBuf> {
    let home = || {
        std::env::var_os("HOME")
            .map(PathBuf::from)
            .context("HOME is not set")
    };
    match provider {
        Provider::OhMyPi => Ok(std::env::var_os("PI_CODING_AGENT_DIR")
            .map(PathBuf::from)
            .unwrap_or(home()?.join(".omp/agent"))),
        Provider::Grok => Ok(std::env::var_os("GROK_HOME")
            .map(PathBuf::from)
            .unwrap_or(home()?.join(".grok"))),
        Provider::KiloCode => Ok(std::env::var_os("XDG_DATA_HOME")
            .map(PathBuf::from)
            .unwrap_or(home()?.join(".local/share"))
            .join("kilo")),
        Provider::OpenHands => Ok(std::env::var_os("OPENHANDS_CONVERSATIONS_DIR")
            .map(PathBuf::from)
            .unwrap_or(home()?.join(".openhands/conversations"))),
        _ => bail!("unsupported native harness"),
    }
}

fn provider_slug(provider: &Provider) -> &'static str {
    match provider {
        Provider::OhMyPi => "omp",
        Provider::Grok => "grok",
        Provider::KiloCode => "kilo",
        Provider::OpenHands => "openhands",
        _ => "unsupported",
    }
}

fn session_key(provider: &Provider, session_id: &str) -> String {
    format!("{}:host:{session_id}", provider_slug(provider))
}

fn list_sessions(
    provider: &Provider,
    executable: &str,
    data_root: &Path,
    limit: usize,
    runner: &dyn CommandRunner,
) -> Result<Vec<StoredSession>> {
    let mut sessions = match provider {
        Provider::OhMyPi => list_omp_sessions(data_root, limit)?,
        Provider::Grok => list_grok_sessions(data_root, limit)?,
        Provider::KiloCode => list_kilo_sessions(executable, limit, runner)?,
        Provider::OpenHands => list_openhands_sessions(data_root, limit)?,
        _ => bail!("unsupported native harness"),
    };
    sessions.sort_by(|left, right| right.updated_at.cmp(&left.updated_at));
    sessions.truncate(limit);
    Ok(sessions)
}

fn read_owned_session(
    provider: &Provider,
    data_root: &Path,
    record: &OwnedNativeSession,
) -> Result<Option<StoredSession>> {
    let Some(path) = record.session_path.as_deref() else {
        return Ok(None);
    };
    if let Err(error) = ensure_real_descendant(data_root, path) {
        if error
            .downcast_ref::<std::io::Error>()
            .is_some_and(|error| error.kind() == std::io::ErrorKind::NotFound)
        {
            return Ok(None);
        }
        return Err(error);
    }
    let parsed = match provider {
        Provider::OhMyPi => parse_omp_session(path),
        Provider::Grok => parse_grok_session(path),
        Provider::OpenHands => parse_openhands_session(path),
        _ => return Ok(None),
    };
    match parsed {
        Ok(session) => Ok(Some(session)),
        Err(error)
            if error
                .downcast_ref::<std::io::Error>()
                .is_some_and(|error| error.kind() == std::io::ErrorKind::NotFound) =>
        {
            Ok(None)
        }
        Err(error) => Err(error),
    }
}

fn stored_to_agent(
    provider: &Provider,
    record: StoredSession,
    owned: Option<&OwnedNativeSession>,
) -> Result<AgentSession> {
    validate_provider_id(provider, &record.session_id)?;
    let id = session_key(provider, &record.session_id);
    let backgrounded = owned.is_some() && crate::native_session::is_backgrounded(&id);
    let mut capabilities = BTreeSet::from([Capability::Inspect]);
    if backgrounded {
        capabilities.insert(Capability::Interrupt);
    }
    let fallback_name = owned
        .map(|record| record.name.clone())
        .unwrap_or_else(|| format!("{} session", provider.label()));
    let lifecycle = if backgrounded {
        "backgrounded"
    } else {
        "saved"
    };
    let raw_state = record
        .model
        .as_deref()
        .map(|model| format!("{lifecycle}; model={}", sanitize(model, 160, "unknown")))
        .unwrap_or_else(|| lifecycle.into());
    Ok(AgentSession {
        id,
        provider_session_id: record.session_id,
        provider: provider.clone(),
        runtime: Runtime::Host,
        kind: if owned.is_some() {
            SessionKind::Managed
        } else {
            SessionKind::Interactive
        },
        name: sanitize(&record.name, 80, &fallback_name),
        cwd: record.cwd,
        state: if backgrounded {
            SessionState::Working
        } else {
            SessionState::Completed
        },
        summary: sanitize(&record.summary, 180, &fallback_name),
        raw_state: Some(raw_state),
        pid: None,
        started_at: record.created_at.or_else(|| {
            owned.map(|record| UNIX_EPOCH + Duration::from_millis(record.created_at_ms))
        }),
        updated_at: record.updated_at,
        pull_requests: None,
        capabilities,
    })
}

fn launch_command(
    provider: &Provider,
    executable: &str,
    request: &LaunchRequest,
) -> Result<Command> {
    let mut command = Command::new(executable);
    command.current_dir(&request.cwd);
    match provider {
        Provider::OhMyPi => {
            command.arg("--no-title");
            if let Some(model) = &request.model {
                command.args(["--model", model]);
            }
            command.args(["--", request.prompt.trim()]);
        }
        Provider::Grok => {
            command.arg("--no-auto-update");
            if let Some(model) = &request.model {
                command.args(["--model", model]);
            }
            command.args(["--", request.prompt.trim()]);
        }
        Provider::KiloCode => {
            command.args(["run", "--interactive"]);
            if let Some(model) = &request.model {
                command.args(["--model", model]);
            }
            command.arg(request.prompt.trim());
        }
        Provider::OpenHands => {
            if let Some(model) = &request.model {
                command.env("LLM_MODEL", model).arg("--override-with-envs");
            }
            command.args(["--task", request.prompt.trim()]);
        }
        _ => bail!("unsupported native harness"),
    }
    Ok(command)
}

fn resume_command(
    provider: &Provider,
    executable: &str,
    cwd: &Path,
    session_id: &str,
    session_path: Option<&Path>,
) -> Result<Command> {
    require_cwd(cwd, provider.label())?;
    validate_provider_id(provider, session_id)?;
    let mut command = Command::new(executable);
    command.current_dir(cwd);
    match provider {
        Provider::OhMyPi => {
            let target = session_path
                .map(|path| path.as_os_str())
                .unwrap_or_else(|| std::ffi::OsStr::new(session_id));
            command.arg("--resume").arg(target);
        }
        Provider::Grok => {
            command.args(["--no-auto-update", "--resume", session_id]);
        }
        Provider::KiloCode => {
            command.args(["--session", session_id]);
        }
        Provider::OpenHands => {
            command.args(["--resume", session_id]);
        }
        _ => bail!("unsupported native harness"),
    }
    Ok(command)
}

fn run_models(
    executable: &str,
    args: &[&str],
    parser: fn(&str) -> Result<Vec<String>>,
    runner: &dyn CommandRunner,
) -> Result<Vec<String>> {
    let mut request = CommandRequest::new(
        executable.to_owned(),
        args.iter().map(|value| (*value).to_owned()).collect(),
    );
    request.timeout = Duration::from_secs(12);
    let output = runner.run(&request)?;
    if output.status != 0 {
        bail!(
            "model discovery exited with status {}: {}",
            output.status,
            output.stderr_lossy()
        );
    }
    if output.stdout.len() as u64 > MAX_JSON_BYTES {
        bail!("model discovery exceeded the 8 MiB safety limit");
    }
    parser(output.stdout_text()?)
}

fn parse_omp_models(input: &str) -> Result<Vec<String>> {
    let value: Value = serde_json::from_str(input).context("invalid Oh My Pi model JSON")?;
    let rows = value
        .get("models")
        .and_then(Value::as_array)
        .context("Oh My Pi model output omitted models")?;
    let mut models = rows
        .iter()
        .filter_map(|row| row.get("selector").and_then(Value::as_str))
        .filter(|model| valid_text(model))
        .map(str::to_owned)
        .collect::<Vec<_>>();
    models.sort();
    models.dedup();
    Ok(models)
}

fn parse_grok_models(input: &str) -> Result<Vec<String>> {
    let mut in_models = false;
    let mut models = Vec::new();
    for line in input.lines() {
        let trimmed = line.trim();
        if trimmed == "Available models:" {
            in_models = true;
            continue;
        }
        if !in_models {
            continue;
        }
        let model = trimmed
            .strip_prefix("* ")
            .or_else(|| trimmed.strip_prefix("- "))
            .and_then(|value| value.strip_suffix(" (default)").or(Some(value)));
        if let Some(model) = model.filter(|model| valid_text(model)) {
            models.push(model.to_owned());
        }
    }
    models.sort();
    models.dedup();
    Ok(models)
}

fn parse_line_models(input: &str) -> Result<Vec<String>> {
    let mut models = input
        .lines()
        .map(str::trim)
        .filter(|line| valid_text(line) && line.contains('/'))
        .map(str::to_owned)
        .collect::<Vec<_>>();
    models.sort();
    models.dedup();
    Ok(models)
}

fn list_omp_sessions(root: &Path, limit: usize) -> Result<Vec<StoredSession>> {
    let files = walk_matching(&root.join("sessions"), 3, limit, |path| {
        path.extension().and_then(|value| value.to_str()) == Some("jsonl")
    })?;
    files
        .into_iter()
        .filter_map(|path| match parse_omp_session(&path) {
            Ok(session) => Some(Ok(session)),
            Err(_) => None,
        })
        .collect()
}

fn parse_omp_session(path: &Path) -> Result<StoredSession> {
    let (file, metadata) = open_checked_file(path, MAX_JOURNAL_BYTES, "Oh My Pi session")?;
    let mut title = None;
    let mut session_id = None;
    let mut cwd = None;
    let mut created_at = None;
    let mut summary = None;
    let mut model = None;
    for line in BufReader::new(file).lines() {
        let line = line?;
        if line.trim().is_empty() {
            continue;
        }
        let Ok(value) = serde_json::from_str::<Value>(&line) else {
            continue;
        };
        match value.get("type").and_then(Value::as_str) {
            Some("title") => {
                title = text_at(&value, "/title").or(title);
            }
            Some("session") => {
                session_id = text_at(&value, "/id").or(session_id);
                cwd = text_at(&value, "/cwd").map(PathBuf::from).or(cwd);
                created_at = text_at(&value, "/timestamp")
                    .and_then(|value| crate::adapters::copilot::parse_copilot_updated_at(&value))
                    .or(created_at);
                title = text_at(&value, "/title").or(title);
            }
            Some("title_change") => title = text_at(&value, "/title").or(title),
            Some("model_change") => model = text_at(&value, "/model").or(model),
            Some("message") => {
                if let Some(message) = value.get("message") {
                    if let Some(value) = message_text(message) {
                        summary = Some(value);
                    }
                    model = text_at(message, "/model").or(model);
                }
            }
            _ => {}
        }
    }
    let session_id = session_id.context("Oh My Pi session omitted its ID")?;
    validate_provider_id(&Provider::OhMyPi, &session_id)?;
    let cwd = cwd.context("Oh My Pi session omitted its workspace")?;
    require_cwd(&cwd, "Oh My Pi")?;
    let name =
        title.unwrap_or_else(|| summary.clone().unwrap_or_else(|| "Oh My Pi session".into()));
    Ok(StoredSession {
        session_id,
        cwd,
        summary: summary.unwrap_or_else(|| name.clone()),
        name,
        model,
        created_at,
        updated_at: metadata.modified().ok(),
        path: Some(path.to_owned()),
    })
}

fn list_grok_sessions(root: &Path, limit: usize) -> Result<Vec<StoredSession>> {
    let files = walk_matching(&root.join("sessions"), 3, limit, |path| {
        path.file_name().and_then(|value| value.to_str()) == Some("summary.json")
    })?;
    files
        .into_iter()
        .filter_map(|path| {
            let directory = path.parent()?.to_owned();
            parse_grok_session(&directory).ok().map(Ok)
        })
        .collect()
}

fn parse_grok_session(directory: &Path) -> Result<StoredSession> {
    let summary_path = if directory.is_dir() {
        directory.join("summary.json")
    } else {
        directory.to_owned()
    };
    let metadata = checked_file(&summary_path, MAX_JSON_BYTES, "Grok summary")?;
    let value = read_json(&summary_path, MAX_JSON_BYTES, "Grok summary")?;
    let session_id = text_at(&value, "/info/id").context("Grok summary omitted its ID")?;
    validate_provider_id(&Provider::Grok, &session_id)?;
    let cwd = text_at(&value, "/info/cwd")
        .map(PathBuf::from)
        .context("Grok summary omitted its workspace")?;
    require_cwd(&cwd, "Grok")?;
    let name = text_at(&value, "/generated_title")
        .or_else(|| text_at(&value, "/session_summary"))
        .unwrap_or_else(|| "Grok session".into());
    let mut latest = text_at(&value, "/session_summary").unwrap_or_else(|| name.clone());
    let updates = summary_path.with_file_name("updates.jsonl");
    match checked_file(&updates, MAX_JOURNAL_BYTES, "Grok updates") {
        Ok(_) => {
            let (file, _) = open_checked_file(&updates, MAX_JOURNAL_BYTES, "Grok updates")?;
            for line in BufReader::new(file).lines() {
                let line = line?;
                let Ok(update) = serde_json::from_str::<Value>(&line) else {
                    continue;
                };
                let kind = text_at(&update, "/params/update/sessionUpdate");
                if matches!(
                    kind.as_deref(),
                    Some("user_message_chunk" | "agent_message_chunk")
                ) {
                    if text_at(&update, "/params/sessionId").as_deref() != Some(&session_id) {
                        bail!("Grok update session ID disagreed with its summary");
                    }
                    if let Some(text) = text_at(&update, "/params/update/content/text") {
                        latest = text;
                    }
                }
            }
        }
        Err(error)
            if error
                .downcast_ref::<std::io::Error>()
                .is_some_and(|error| error.kind() == std::io::ErrorKind::NotFound) => {}
        Err(error) => return Err(error),
    }
    Ok(StoredSession {
        session_id,
        cwd,
        name,
        summary: latest,
        model: text_at(&value, "/current_model_id"),
        created_at: text_at(&value, "/created_at")
            .and_then(|value| crate::adapters::copilot::parse_copilot_updated_at(&value)),
        updated_at: text_at(&value, "/updated_at")
            .and_then(|value| crate::adapters::copilot::parse_copilot_updated_at(&value))
            .or_else(|| metadata.modified().ok()),
        path: Some(summary_path.parent().unwrap_or(directory).to_owned()),
    })
}

fn list_kilo_sessions(
    executable: &str,
    limit: usize,
    runner: &dyn CommandRunner,
) -> Result<Vec<StoredSession>> {
    // Kilo 7.5.x's higher-level `session list --format json` crashes when an
    // otherwise valid row has no `time.updated` value. Its documented `db`
    // surface gives us the same bounded metadata without loading message
    // bodies and works for a brand-new, unauthenticated installation.
    let query = format!(
        "SELECT id, title, directory, time_created AS created, \
         time_updated AS updated FROM session ORDER BY time_updated DESC LIMIT {}",
        limit.max(1)
    );
    let mut request = CommandRequest::new(
        executable.to_owned(),
        vec!["db".into(), query, "--format".into(), "json".into()],
    );
    request.timeout = Duration::from_secs(10);
    let output = runner.run(&request)?;
    if output.status != 0 {
        bail!(
            "Kilo Code session discovery exited with status {}: {}",
            output.status,
            output.stderr_lossy()
        );
    }
    if output.stdout.len() as u64 > MAX_JSON_BYTES {
        bail!("Kilo Code session catalog exceeded the 8 MiB safety limit");
    }
    parse_kilo_sessions(output.stdout_text()?)
}

fn parse_kilo_sessions(input: &str) -> Result<Vec<StoredSession>> {
    if input.trim().is_empty() {
        return Ok(Vec::new());
    }
    let rows: Value = serde_json::from_str(input).context("invalid Kilo Code session JSON")?;
    let rows = rows
        .as_array()
        .context("Kilo Code session output was not an array")?;
    rows.iter()
        .map(|row| {
            let session_id = text_at(row, "/id").context("Kilo Code session omitted its ID")?;
            validate_provider_id(&Provider::KiloCode, &session_id)?;
            let cwd = text_at(row, "/directory")
                .map(PathBuf::from)
                .context("Kilo Code session omitted its workspace")?;
            require_cwd(&cwd, "Kilo Code")?;
            let name = text_at(row, "/title").unwrap_or_else(|| "Kilo Code session".into());
            Ok(StoredSession {
                session_id,
                cwd,
                summary: name.clone(),
                name,
                model: None,
                created_at: millis_at(row, "/created"),
                updated_at: millis_at(row, "/updated"),
                path: None,
            })
        })
        .collect()
}

fn list_openhands_sessions(root: &Path, limit: usize) -> Result<Vec<StoredSession>> {
    let metadata = match fs::symlink_metadata(root) {
        Ok(metadata) => metadata,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(Vec::new()),
        Err(error) => return Err(error.into()),
    };
    if metadata.file_type().is_symlink() || !metadata.is_dir() {
        bail!("OpenHands conversations root must be a real directory");
    }
    let mut sessions = Vec::new();
    for entry in fs::read_dir(root)?.take(limit) {
        let entry = entry?;
        if entry.file_type()?.is_dir() && entry.path().join("events").is_dir() {
            if let Ok(session) = parse_openhands_session(&entry.path()) {
                sessions.push(session);
            }
        }
    }
    Ok(sessions)
}

fn list_openhands_models(root: &Path, limit: usize) -> Result<Vec<String>> {
    let metadata = match fs::symlink_metadata(root) {
        Ok(metadata) => metadata,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(Vec::new()),
        Err(error) => return Err(error.into()),
    };
    if metadata.file_type().is_symlink() || !metadata.is_dir() {
        bail!("OpenHands conversations root must be a real directory");
    }
    let mut models = BTreeSet::new();
    for entry in fs::read_dir(root)?.take(limit) {
        let entry = entry?;
        if !entry.file_type()?.is_dir() {
            continue;
        }
        let path = entry.path().join("base_state.json");
        let Ok(value) = read_json(&path, MAX_JSON_BYTES, "OpenHands base state") else {
            continue;
        };
        if let Some(model) = text_at(&value, "/agent/llm/model") {
            models.insert(model);
        }
    }
    Ok(models.into_iter().collect())
}

fn parse_openhands_session(directory: &Path) -> Result<StoredSession> {
    let conversation = if directory.file_name().and_then(|value| value.to_str()) == Some("events") {
        directory
            .parent()
            .context("OpenHands events directory has no conversation")?
    } else {
        directory
    };
    let directory_name = conversation
        .file_name()
        .and_then(|value| value.to_str())
        .context("OpenHands conversation has no ID")?;
    let mut session_id = normalize_openhands_id(directory_name)?;
    let events = conversation.join("events");
    let events_metadata = fs::symlink_metadata(&events)?;
    if events_metadata.file_type().is_symlink() || !events_metadata.is_dir() {
        bail!("OpenHands events root must be a real directory");
    }
    let base_state_path = conversation.join("base_state.json");
    let base = match read_json(&base_state_path, MAX_JSON_BYTES, "OpenHands base state") {
        Ok(value) => Some(value),
        Err(error)
            if error
                .downcast_ref::<std::io::Error>()
                .is_some_and(|error| error.kind() == std::io::ErrorKind::NotFound) =>
        {
            None
        }
        Err(error) => return Err(error),
    };
    if let Some(id) = base.as_ref().and_then(|value| text_at(value, "/id")) {
        validate_provider_id(&Provider::OpenHands, &id)?;
        if id != session_id {
            bail!("OpenHands base state ID disagreed with its conversation directory");
        }
        session_id = id;
    }
    let cwd = base
        .as_ref()
        .and_then(|value| text_at(value, "/workspace/working_dir"))
        .map(PathBuf::from)
        .unwrap_or_else(|| conversation.to_owned());
    require_cwd(&cwd, "OpenHands")?;
    let model = base
        .as_ref()
        .and_then(|value| text_at(value, "/agent/llm/model"));
    let mut event_paths = fs::read_dir(&events)?
        .filter_map(|entry| entry.ok())
        .filter(|entry| entry.file_type().ok().is_some_and(|kind| kind.is_file()))
        .map(|entry| entry.path())
        .filter(|path| {
            path.file_name()
                .and_then(|value| value.to_str())
                .is_some_and(|name| name.starts_with("event-") && name.ends_with(".json"))
        })
        .collect::<Vec<_>>();
    event_paths.sort();
    if event_paths.len() > MAX_SESSION_SCAN {
        bail!("OpenHands event log exceeded the record safety limit");
    }
    let mut name = None;
    let mut summary = None;
    let mut created_at = None;
    let mut updated_at = None;
    let mut total_bytes = 0_u64;
    for path in event_paths {
        let metadata = checked_file(&path, MAX_JSON_BYTES, "OpenHands event")?;
        total_bytes = total_bytes.saturating_add(metadata.len());
        if total_bytes > MAX_JOURNAL_BYTES {
            bail!("OpenHands event log exceeded the 32 MiB safety limit");
        }
        updated_at = metadata.modified().ok().or(updated_at);
        let value = read_json(&path, MAX_JSON_BYTES, "OpenHands event")?;
        let timestamp = text_at(&value, "/timestamp")
            .and_then(|value| crate::adapters::copilot::parse_copilot_updated_at(&value));
        created_at = created_at.or(timestamp);
        updated_at = timestamp.or(updated_at);
        if text_at(&value, "/kind").as_deref() == Some("MessageEvent") {
            let role = text_at(&value, "/llm_message/role");
            if let Some(text) = value.pointer("/llm_message/content").and_then(content_text) {
                if role.as_deref() == Some("user") && name.is_none() {
                    name = Some(text.clone());
                }
                summary = Some(text);
            }
        }
    }
    let name = name.unwrap_or_else(|| "OpenHands session".into());
    Ok(StoredSession {
        session_id,
        cwd,
        summary: summary.unwrap_or_else(|| name.clone()),
        name,
        model,
        created_at,
        updated_at,
        path: Some(conversation.to_owned()),
    })
}

fn walk_matching(
    root: &Path,
    max_depth: usize,
    limit: usize,
    matches: impl Fn(&Path) -> bool + Copy,
) -> Result<Vec<PathBuf>> {
    let metadata = match fs::symlink_metadata(root) {
        Ok(metadata) => metadata,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(Vec::new()),
        Err(error) => return Err(error.into()),
    };
    if metadata.file_type().is_symlink() || !metadata.is_dir() {
        bail!("provider session root must be a real directory");
    }
    let mut result = Vec::new();
    let mut stack = vec![(root.to_owned(), 0_usize)];
    while let Some((directory, depth)) = stack.pop() {
        for entry in fs::read_dir(directory)? {
            let entry = entry?;
            let kind = entry.file_type()?;
            if kind.is_symlink() {
                continue;
            }
            let path = entry.path();
            if kind.is_file() && matches(&path) {
                result.push(path);
                if result.len() >= limit {
                    return Ok(result);
                }
            } else if kind.is_dir() && depth < max_depth {
                stack.push((path, depth + 1));
            }
        }
    }
    Ok(result)
}

fn checked_file(path: &Path, limit: u64, label: &str) -> Result<fs::Metadata> {
    let metadata = fs::symlink_metadata(path)?;
    if metadata.file_type().is_symlink() || !metadata.is_file() {
        bail!("{label} must be a real file");
    }
    if metadata.len() > limit {
        bail!("{label} exceeded its safety limit");
    }
    Ok(metadata)
}

fn open_checked_file(path: &Path, limit: u64, label: &str) -> Result<(File, fs::Metadata)> {
    checked_file(path, limit, label)?;
    let mut options = OpenOptions::new();
    options.read(true);
    #[cfg(unix)]
    options.custom_flags(libc::O_NOFOLLOW);
    let file = options
        .open(path)
        .with_context(|| format!("{label} must be a real file"))?;
    let metadata = file.metadata()?;
    if !metadata.is_file() {
        bail!("{label} must be a real file");
    }
    if metadata.len() > limit {
        bail!("{label} exceeded its safety limit");
    }
    Ok((file, metadata))
}

fn ensure_real_descendant(root: &Path, candidate: &Path) -> Result<()> {
    let relative = candidate
        .strip_prefix(root)
        .with_context(|| format!("provider session path escaped {}", root.display()))?;
    let root_metadata = fs::symlink_metadata(root)?;
    if root_metadata.file_type().is_symlink() || !root_metadata.is_dir() {
        bail!("provider session root must be a real directory");
    }
    let mut current = root.to_owned();
    for component in relative.components() {
        let std::path::Component::Normal(component) = component else {
            bail!("provider session path contained an invalid component");
        };
        current.push(component);
        if fs::symlink_metadata(&current)?.file_type().is_symlink() {
            bail!("provider session path must not contain symlinks");
        }
    }
    Ok(())
}

fn read_json(path: &Path, limit: u64, label: &str) -> Result<Value> {
    let (file, _) = open_checked_file(path, limit, label)?;
    let mut input = String::new();
    file.take(limit + 1).read_to_string(&mut input)?;
    serde_json::from_str(&input).with_context(|| format!("invalid {label} JSON"))
}

fn text_at(value: &Value, pointer: &str) -> Option<String> {
    value
        .pointer(pointer)
        .and_then(Value::as_str)
        .filter(|value| valid_text(value))
        .map(str::to_owned)
}

fn message_text(value: &Value) -> Option<String> {
    value
        .get("content")
        .and_then(content_text)
        .or_else(|| text_at(value, "/text"))
}

fn content_text(value: &Value) -> Option<String> {
    match value {
        Value::String(text) if valid_text(text) => Some(text.to_owned()),
        Value::Array(items) => {
            let joined = items
                .iter()
                .filter_map(|item| {
                    item.get("text")
                        .and_then(Value::as_str)
                        .filter(|text| valid_text(text))
                })
                .collect::<Vec<_>>()
                .join(" ");
            valid_text(&joined).then_some(joined)
        }
        Value::Object(_) => text_at(value, "/text"),
        _ => None,
    }
}

fn millis_at(value: &Value, pointer: &str) -> Option<SystemTime> {
    let value = value.pointer(pointer)?;
    let millis = value
        .as_u64()
        .or_else(|| value.as_str()?.parse::<u64>().ok())?;
    Some(UNIX_EPOCH + Duration::from_millis(millis))
}

fn normalize_openhands_id(value: &str) -> Result<String> {
    if value.len() == 32 && value.bytes().all(|byte| byte.is_ascii_hexdigit()) {
        return Ok(format!(
            "{}-{}-{}-{}-{}",
            &value[0..8],
            &value[8..12],
            &value[12..16],
            &value[16..20],
            &value[20..32]
        ));
    }
    validate_provider_id(&Provider::OpenHands, value)?;
    Ok(value.to_owned())
}

fn validate_provider_id(provider: &Provider, value: &str) -> Result<()> {
    validate_id(value, provider.label())?;
    let valid = match provider {
        Provider::Grok | Provider::OpenHands => is_uuid(value),
        Provider::OhMyPi | Provider::KiloCode => value
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'_' | b'.')),
        _ => false,
    };
    if !valid {
        bail!("invalid {} session ID", provider.label());
    }
    Ok(())
}

fn is_uuid(value: &str) -> bool {
    value.len() == 36
        && value.bytes().enumerate().all(|(index, byte)| {
            if matches!(index, 8 | 13 | 18 | 23) {
                byte == b'-'
            } else {
                byte.is_ascii_hexdigit()
            }
        })
}

fn valid_text(value: &str) -> bool {
    !value.trim().is_empty()
        && value.len() <= 256 * 1024
        && !value.chars().any(|character| character == '\0')
}

fn require_cwd(cwd: &Path, label: &str) -> Result<()> {
    if !cwd.is_absolute() {
        bail!("{label} workspace must be absolute");
    }
    Ok(())
}

fn same_workspace(left: &Path, right: &Path) -> bool {
    if left == right {
        return true;
    }
    match (left.canonicalize(), right.canonicalize()) {
        (Ok(left), Ok(right)) => left == right,
        _ => false,
    }
}

fn validate_session(provider: &Provider, session: &AgentSession) -> Result<()> {
    if &session.provider != provider || session.runtime != Runtime::Host {
        bail!(
            "the host {} controller does not own this runtime",
            provider.label()
        );
    }
    validate_provider_id(provider, &session.provider_session_id)
}

fn native_outcome(
    exit: crate::native_session::NativeSessionExit,
    provider: &Provider,
    session_id: &str,
) -> Result<ControlOutcome> {
    match exit {
        crate::native_session::NativeSessionExit::Backgrounded => Ok(ControlOutcome {
            message: format!(
                "backgrounded {} session; Enter/Right resumes it",
                provider.label()
            ),
            provider_session_hint: Some(session_id.into()),
        }),
        crate::native_session::NativeSessionExit::Exited(status) if status.success() => {
            Ok(ControlOutcome {
                message: format!("returned from {} session", provider.label()),
                provider_session_hint: Some(session_id.into()),
            })
        }
        crate::native_session::NativeSessionExit::Exited(status) => {
            bail!("{} session exited with status {status}", provider.label())
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::test_support::tempfile;

    const UUID: &str = "aaaaaaaa-aaaa-4aaa-8aaa-aaaaaaaaaaaa";

    #[test]
    fn omp_current_title_slot_and_latest_message_are_projected() {
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("sessions/work/session.jsonl");
        fs::create_dir_all(path.parent().unwrap()).unwrap();
        fs::write(
            &path,
            concat!(
                "{\"type\":\"title\",\"v\":1,\"title\":\"Parser repair\",\"updatedAt\":\"2026-08-26T12:00:00Z\",\"pad\":\"\"}\n",
                "{\"type\":\"session\",\"version\":3,\"id\":\"abc-123\",\"cwd\":\"/work\",\"timestamp\":\"2026-08-26T12:00:00Z\"}\n",
                "{\"type\":\"message\",\"message\":{\"role\":\"assistant\",\"model\":\"fixture\",\"content\":[{\"type\":\"text\",\"text\":\"Latest answer\"}]}}\n"
            ),
        )
        .unwrap();
        let session = parse_omp_session(&path).unwrap();
        assert_eq!(session.session_id, "abc-123");
        assert_eq!(session.name, "Parser repair");
        assert_eq!(session.summary, "Latest answer");
        assert_eq!(session.model.as_deref(), Some("fixture"));
    }

    #[test]
    fn grok_summary_and_update_log_are_projected() {
        let directory = tempfile::tempdir().unwrap();
        let session = directory.path().join(UUID);
        fs::create_dir(&session).unwrap();
        fs::write(
            session.join("summary.json"),
            format!(
                "{{\"info\":{{\"id\":\"{UUID}\",\"cwd\":\"/work\"}},\"generated_title\":\"Grok task\",\"session_summary\":\"first\",\"current_model_id\":\"grok-4.6\",\"created_at\":\"2026-08-26T12:00:00Z\",\"updated_at\":\"2026-08-26T12:00:01Z\"}}"
            ),
        )
        .unwrap();
        fs::write(
            session.join("updates.jsonl"),
            format!("{{\"params\":{{\"sessionId\":\"{UUID}\",\"update\":{{\"sessionUpdate\":\"agent_message_chunk\",\"content\":{{\"type\":\"text\",\"text\":\"latest answer\"}}}}}}}}\n"),
        )
        .unwrap();
        let parsed = parse_grok_session(&session).unwrap();
        assert_eq!(parsed.name, "Grok task");
        assert_eq!(parsed.summary, "latest answer");
        assert_eq!(parsed.model.as_deref(), Some("grok-4.6"));
    }

    #[test]
    fn kilo_json_uses_documented_fields_and_millisecond_times() {
        let parsed = parse_kilo_sessions(
            r#"[{"id":"ses_fixture","title":"Kilo task","created":1000,"updated":2000,"projectId":"p","directory":"/work"}]"#,
        )
        .unwrap();
        assert_eq!(parsed[0].session_id, "ses_fixture");
        assert_eq!(parsed[0].cwd, Path::new("/work"));
        assert_eq!(
            parsed[0].updated_at,
            Some(UNIX_EPOCH + Duration::from_secs(2))
        );
    }

    #[test]
    fn openhands_base_state_and_latest_message_are_projected() {
        let directory = tempfile::tempdir().unwrap();
        let conversation = directory.path().join(UUID.replace('-', ""));
        let events = conversation.join("events");
        fs::create_dir_all(&events).unwrap();
        fs::write(
            conversation.join("base_state.json"),
            format!(r#"{{"id":"{UUID}","agent":{{"llm":{{"model":"openai/fixture"}}}},"workspace":{{"working_dir":"/work"}}}}"#),
        )
        .unwrap();
        for (index, role, text) in [(0, "user", "First task"), (1, "assistant", "Latest reply")] {
            fs::write(
                events.join(format!("event-{index:05}-{UUID}.json")),
                format!(r#"{{"id":"{UUID}","timestamp":"2026-08-26T12:00:0{index}.000001","source":"agent","kind":"MessageEvent","llm_message":{{"role":"{role}","content":[{{"type":"text","text":"{text}"}}]}}}}"#),
            )
            .unwrap();
        }
        let parsed = parse_openhands_session(&conversation).unwrap();
        assert_eq!(parsed.name, "First task");
        assert_eq!(parsed.summary, "Latest reply");
        assert_eq!(parsed.model.as_deref(), Some("openai/fixture"));
    }

    #[test]
    fn all_extended_model_catalog_shapes_are_parsed() {
        assert_eq!(
            parse_omp_models(r#"{"models":[{"selector":"anthropic/opus"}]}"#).unwrap(),
            vec!["anthropic/opus"]
        );
        assert_eq!(
            parse_grok_models("Default model: grok-4.6\n\nAvailable models:\n  * grok-4.6 (default)\n  - grok-code-fast\n").unwrap(),
            vec!["grok-4.6", "grok-code-fast"]
        );
        assert_eq!(
            parse_line_models("anthropic/claude\nopenai/gpt\n").unwrap(),
            vec!["anthropic/claude", "openai/gpt"]
        );
    }

    #[test]
    fn source_defaults_to_owned_rows_and_external_is_explicit() {
        let directory = tempfile::tempdir().unwrap();
        let data = directory.path().join("omp");
        let path = data.join("sessions/work/session.jsonl");
        fs::create_dir_all(path.parent().unwrap()).unwrap();
        fs::write(
            &path,
            "{\"type\":\"session\",\"version\":3,\"id\":\"abc-123\",\"cwd\":\"/work\",\"timestamp\":\"2026-08-26T12:00:00Z\"}\n",
        )
        .unwrap();
        let state = directory.path().join("state/owned.json");
        fs::create_dir(state.parent().unwrap()).unwrap();
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            fs::set_permissions(state.parent().unwrap(), fs::Permissions::from_mode(0o700))
                .unwrap();
        }
        let ownership = SessionMigrateNativeOwnership::load(Provider::OhMyPi, state).unwrap();
        let source =
            SessionMigrateNativeSource::host(Provider::OhMyPi, "omp", data, ownership).unwrap();
        assert!(source
            .discover(&DiscoveryRequest::default())
            .unwrap()
            .is_empty());
        let external = source
            .discover(&DiscoveryRequest {
                include_external: true,
                include_completed: true,
                ..DiscoveryRequest::default()
            })
            .unwrap();
        assert_eq!(external.len(), 1);
        assert_eq!(external[0].provider, Provider::OhMyPi);
    }

    #[test]
    fn provider_session_ids_fail_closed() {
        for (provider, session_id) in [
            (Provider::OhMyPi, "../../outside"),
            (Provider::Grok, "not-a-uuid"),
            (Provider::KiloCode, "session/id"),
            (Provider::OpenHands, "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa"),
        ] {
            assert!(
                validate_provider_id(&provider, session_id).is_err(),
                "{provider:?} accepted {session_id:?}"
            );
        }
    }

    #[test]
    fn malformed_catalogs_and_oversized_state_are_rejected() {
        assert!(parse_kilo_sessions("not json").is_err());
        assert!(parse_kilo_sessions(r#"{"id":"not-an-array"}"#).is_err());

        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("oversized.json");
        let file = File::create(&path).unwrap();
        file.set_len(MAX_JSON_BYTES + 1).unwrap();
        assert!(read_json(&path, MAX_JSON_BYTES, "fixture").is_err());
    }

    #[cfg(unix)]
    #[test]
    fn symlinked_provider_state_is_never_followed() {
        use std::os::unix::fs::symlink;

        let directory = tempfile::tempdir().unwrap();
        let target = directory.path().join("target.json");
        let link = directory.path().join("summary.json");
        fs::write(&target, "{}").unwrap();
        symlink(&target, &link).unwrap();
        let error = read_json(&link, MAX_JSON_BYTES, "fixture").unwrap_err();
        assert!(error.to_string().contains("must be a real file"));

        let root_target = directory.path().join("sessions-real");
        let root_link = directory.path().join("sessions-link");
        fs::create_dir(&root_target).unwrap();
        symlink(&root_target, &root_link).unwrap();
        let error = walk_matching(&root_link, 1, 1, |_| true).unwrap_err();
        assert!(error.to_string().contains("must be a real directory"));

        let nested_link = root_target.join("nested");
        symlink(directory.path(), &nested_link).unwrap();
        let error =
            ensure_real_descendant(&root_target, &nested_link.join("target.json")).unwrap_err();
        assert!(error.to_string().contains("must not contain symlinks"));
    }

    #[test]
    fn malformed_optional_grok_and_openhands_files_are_rejected() {
        let directory = tempfile::tempdir().unwrap();
        let grok = directory.path().join(UUID);
        fs::create_dir(&grok).unwrap();
        fs::write(
            grok.join("summary.json"),
            format!(r#"{{"info":{{"id":"{UUID}","cwd":"/work"}}}}"#),
        )
        .unwrap();
        fs::write(
            grok.join("updates.jsonl"),
            vec![b'x'; MAX_JOURNAL_BYTES as usize + 1],
        )
        .unwrap();
        assert!(parse_grok_session(&grok).is_err());

        let openhands = directory.path().join(UUID.replace('-', ""));
        fs::create_dir_all(openhands.join("events")).unwrap();
        fs::write(openhands.join("base_state.json"), "not json").unwrap();
        assert!(parse_openhands_session(&openhands).is_err());
    }

    #[test]
    fn linked_native_state_must_use_one_session_id() {
        let directory = tempfile::tempdir().unwrap();
        let grok = directory.path().join(UUID);
        fs::create_dir(&grok).unwrap();
        fs::write(
            grok.join("summary.json"),
            format!(r#"{{"info":{{"id":"{UUID}","cwd":"/work"}}}}"#),
        )
        .unwrap();
        fs::write(
            grok.join("updates.jsonl"),
            r#"{"params":{"sessionId":"bbbbbbbb-bbbb-4bbb-8bbb-bbbbbbbbbbbb","update":{"sessionUpdate":"agent_message_chunk","content":{"type":"text","text":"wrong"}}}}"#,
        )
        .unwrap();
        assert!(parse_grok_session(&grok).is_err());

        let openhands = directory.path().join(UUID.replace('-', ""));
        fs::create_dir_all(openhands.join("events")).unwrap();
        fs::write(
            openhands.join("base_state.json"),
            r#"{"id":"bbbbbbbb-bbbb-4bbb-8bbb-bbbbbbbbbbbb","workspace":{"working_dir":"/work"}}"#,
        )
        .unwrap();
        assert!(parse_openhands_session(&openhands).is_err());
    }

    #[test]
    fn openhands_model_discovery_reads_only_bounded_base_state() {
        let directory = tempfile::tempdir().unwrap();
        let conversation = directory.path().join(UUID.replace('-', ""));
        fs::create_dir_all(conversation.join("events")).unwrap();
        fs::write(
            conversation.join("base_state.json"),
            r#"{"agent":{"llm":{"model":"openai/fixture"}}}"#,
        )
        .unwrap();
        fs::write(conversation.join("events/event-00000-bad.json"), "not json").unwrap();

        assert_eq!(
            list_openhands_models(directory.path(), 10).unwrap(),
            ["openai/fixture"]
        );
    }
}

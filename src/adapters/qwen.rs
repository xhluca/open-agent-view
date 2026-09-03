use std::collections::{BTreeMap, BTreeSet};
use std::fs::{self, OpenOptions};
use std::io::Write;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::{Arc, Mutex};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use anyhow::{anyhow, bail, Context, Result};
use serde::{de::Error as _, Deserialize, Deserializer, Serialize};

use super::{DiscoveryRequest, SessionSource, SourceDiscovery};
use crate::control::{
    run_native_authentication, ControlOutcome, LaunchMode, LaunchPresentation, LaunchRequest,
    ProviderController,
};
use crate::domain::{
    AgentSession, Capability, Provider, Runtime, SessionKind, SessionSnapshot, SessionState,
};
use crate::process::{CancellableProcessRunner, CommandRequest, CommandRunner};

const MAX_CATALOG_BYTES: usize = 8 * 1024 * 1024;

#[derive(Clone, Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct QwenHistoryRecord {
    session_id: String,
    cwd: PathBuf,
    #[serde(deserialize_with = "deserialize_millis")]
    mtime: u64,
    prompt: String,
    custom_title: Option<String>,
}

#[derive(Clone, Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct QwenLiveRecord {
    pid: u32,
    session_id: String,
    cwd: PathBuf,
    name: String,
    #[serde(deserialize_with = "deserialize_millis")]
    started_at: u64,
}

fn deserialize_millis<'de, D>(deserializer: D) -> std::result::Result<u64, D::Error>
where
    D: Deserializer<'de>,
{
    let value = serde_json::Value::deserialize(deserializer)?;
    let millis = match value {
        serde_json::Value::Number(number) => {
            if let Some(value) = number.as_u64() {
                return Ok(value);
            }
            number.as_f64()
        }
        serde_json::Value::String(value) => value.parse::<f64>().ok(),
        _ => None,
    }
    .ok_or_else(|| D::Error::custom("expected a millisecond timestamp"))?;
    if !millis.is_finite() || millis < 0.0 || millis > u64::MAX as f64 {
        return Err(D::Error::custom("millisecond timestamp is out of range"));
    }
    Ok(millis.floor() as u64)
}

#[derive(Clone, Debug, Deserialize, Eq, Ord, PartialEq, PartialOrd, Serialize)]
#[serde(rename_all = "camelCase")]
struct OwnedQwenSession {
    session_id: String,
    cwd: PathBuf,
    created_at_ms: u64,
    name: String,
}

/// Private record of the exact UUIDs allocated by OAV through Qwen Code's
/// documented `--session-id` option. It never stores provider credentials or
/// transcript text.
pub struct QwenOwnership {
    path: PathBuf,
    records: Mutex<BTreeSet<OwnedQwenSession>>,
}

impl QwenOwnership {
    pub fn load_default() -> Result<Arc<Self>> {
        Self::load(default_qwen_ownership_path()?)
    }

    pub fn load(path: PathBuf) -> Result<Arc<Self>> {
        validate_private_state_path(&path)?;
        let records = match fs::read_to_string(&path) {
            Ok(input) => serde_json::from_str(&input)
                .with_context(|| format!("invalid Qwen ownership registry {}", path.display()))?,
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => BTreeSet::new(),
            Err(error) => return Err(error.into()),
        };
        Ok(Arc::new(Self {
            path,
            records: Mutex::new(records),
        }))
    }

    fn owns(&self, session_id: &str) -> bool {
        self.records
            .lock()
            .map(|records| records.iter().any(|record| record.session_id == session_id))
            .unwrap_or(false)
    }

    fn snapshot(&self) -> Vec<OwnedQwenSession> {
        let mut records = self
            .records
            .lock()
            .map(|records| records.clone())
            .unwrap_or_default();
        // Discovery and control normally share one Arc, but reloading this
        // tiny private registry also makes ownership visible across
        // independently constructed handles and guards future process-boundary
        // refactors.
        // Only the already-validated owner-only file may add authority.
        if validate_private_state_path(&self.path).is_ok() {
            if let Ok(input) = fs::read_to_string(&self.path) {
                if let Ok(persisted) = serde_json::from_str::<BTreeSet<OwnedQwenSession>>(&input) {
                    records.extend(persisted);
                }
            }
        }
        records.into_iter().collect()
    }

    fn record(&self, session_id: &str, cwd: &Path, name: &str) -> Result<()> {
        let mut records = self
            .records
            .lock()
            .map_err(|_| anyhow!("Qwen ownership registry lock was poisoned"))?;
        let mut next = records.clone();
        next.retain(|record| record.session_id != session_id);
        next.insert(OwnedQwenSession {
            session_id: session_id.into(),
            cwd: cwd.to_owned(),
            created_at_ms: now_millis(),
            name: name.into(),
        });
        persist_private_registry(&self.path, &next)?;
        *records = next;
        Ok(())
    }
}

pub fn default_qwen_ownership_path() -> Result<PathBuf> {
    if let Some(state_home) = std::env::var_os("XDG_STATE_HOME") {
        return Ok(PathBuf::from(state_home)
            .join("open-agent-view")
            .join("qwen-owned.json"));
    }
    let home = std::env::var_os("HOME").context("HOME is not set")?;
    Ok(PathBuf::from(home).join(".local/state/open-agent-view/qwen-owned.json"))
}

pub struct QwenSource {
    executable: String,
    ownership: Arc<QwenOwnership>,
    runner: Arc<dyn CommandRunner>,
}

impl QwenSource {
    pub fn host(executable: impl Into<String>, ownership: Arc<QwenOwnership>) -> Self {
        Self {
            executable: executable.into(),
            ownership,
            runner: Arc::new(CancellableProcessRunner::default()),
        }
    }

    #[cfg(test)]
    fn with_runner(
        executable: impl Into<String>,
        ownership: Arc<QwenOwnership>,
        runner: Arc<dyn CommandRunner>,
    ) -> Self {
        Self {
            executable: executable.into(),
            ownership,
            runner,
        }
    }

    fn history(&self, limit: usize) -> Result<Vec<QwenHistoryRecord>> {
        let mut request = CommandRequest::new(
            self.executable.clone(),
            vec![
                "sessions".into(),
                "list".into(),
                "--json".into(),
                "--limit".into(),
                limit.max(1).to_string(),
            ],
        );
        request.timeout = Duration::from_secs(8);
        let output = self.runner.run(&request)?;
        if output.status != 0 {
            bail!(
                "Qwen session discovery exited with status {}: {}",
                output.status,
                output.stderr_lossy()
            );
        }
        if output.stdout.len() > MAX_CATALOG_BYTES {
            bail!("Qwen session catalog exceeded the 8 MiB safety limit");
        }
        parse_json_lines(output.stdout_text()?, "Qwen session list")
    }

    fn live(&self) -> Result<Vec<QwenLiveRecord>> {
        let mut request = CommandRequest::new(
            self.executable.clone(),
            vec!["sessions".into(), "ps".into(), "--json".into()],
        );
        request.timeout = Duration::from_secs(5);
        let output = self.runner.run(&request)?;
        if output.status != 0 {
            bail!(
                "Qwen live-session discovery exited with status {}: {}",
                output.status,
                output.stderr_lossy()
            );
        }
        if output.stdout.len() > MAX_CATALOG_BYTES {
            bail!("Qwen live-session catalog exceeded the 8 MiB safety limit");
        }
        parse_json_lines(output.stdout_text()?, "Qwen live-session list")
    }
}

impl SessionSource for QwenSource {
    fn label(&self) -> &str {
        "Qwen Code (host)"
    }

    fn discover(&self, request: &DiscoveryRequest) -> Result<Vec<AgentSession>> {
        Ok(self.discover_with_warnings(request)?.sessions)
    }

    fn discover_with_warnings(&self, request: &DiscoveryRequest) -> Result<SourceDiscovery> {
        let mut warnings = Vec::new();
        // A dashboard keeps discovery on a long-lived worker while foreground
        // launches run through the control handle. Snapshot the owner-only
        // registry once per refresh so a launch persisted by either handle is
        // used consistently for catalog matching and fallback rows.
        let owned_records = self.ownership.snapshot();
        let owned_ids = owned_records
            .iter()
            .map(|record| record.session_id.clone())
            .collect::<BTreeSet<_>>();
        let live = match self.live() {
            Ok(records) => records,
            Err(error) => {
                warnings.push(format!("Qwen live-session discovery: {error:#}"));
                Vec::new()
            }
        }
        .into_iter()
        .map(|record| (record.session_id.clone(), record))
        .collect::<BTreeMap<_, _>>();
        let history = match self.history(request.history_limit.max(1)) {
            Ok(records) => records,
            Err(error) => {
                warnings.push(format!("Qwen history discovery: {error:#}"));
                Vec::new()
            }
        };
        let mut sessions = Vec::new();
        let mut discovered_ids = BTreeSet::new();
        for record in history {
            let owned = owned_ids.contains(&record.session_id);
            if !request.include_external && !owned {
                continue;
            }
            let active = live.get(&record.session_id);
            let name = record
                .custom_title
                .filter(|title| !title.trim().is_empty())
                .or_else(|| active.map(|record| record.name.clone()))
                .unwrap_or_else(|| summarize(&record.prompt, 48));
            let updated_at = Some(UNIX_EPOCH + Duration::from_millis(record.mtime));
            let started_at =
                active.map(|record| UNIX_EPOCH + Duration::from_millis(record.started_at));
            discovered_ids.insert(record.session_id.clone());
            sessions.push(AgentSession {
                id: format!("qwen:host:{}", record.session_id),
                provider_session_id: record.session_id,
                provider: Provider::QwenCode,
                runtime: Runtime::Host,
                kind: if owned {
                    SessionKind::Managed
                } else if active.is_some() {
                    SessionKind::Interactive
                } else {
                    SessionKind::Unknown
                },
                name,
                cwd: active
                    .map(|record| record.cwd.clone())
                    .unwrap_or(record.cwd),
                state: if active.is_some() {
                    SessionState::Working
                } else {
                    SessionState::Completed
                },
                summary: summarize(&record.prompt, 160),
                raw_state: Some(if active.is_some() { "running" } else { "saved" }.into()),
                pid: active.map(|record| record.pid),
                started_at,
                updated_at,
                pull_requests: None,
                capabilities: BTreeSet::from([Capability::Inspect]),
            });
        }

        // Qwen writes its durable history asynchronously. The exact UUID,
        // cwd, title, and creation time in this private registry were all
        // allocated and persisted by OAV after a successful foreground
        // launch, so they are sufficient to keep the newly backgrounded row
        // visible until Qwen's richer history record appears. Never synthesize
        // an external row or infer authority from a provider process.
        for owned in owned_records {
            if discovered_ids.contains(&owned.session_id) {
                continue;
            }
            let session_key = format!("qwen:host:{}", owned.session_id);
            let backgrounded = crate::native_session::is_backgrounded(&session_key);
            sessions.push(AgentSession {
                id: session_key,
                provider_session_id: owned.session_id,
                provider: Provider::QwenCode,
                runtime: Runtime::Host,
                kind: SessionKind::Managed,
                name: owned.name.clone(),
                cwd: owned.cwd,
                state: if backgrounded {
                    SessionState::Working
                } else {
                    SessionState::Completed
                },
                summary: owned.name,
                raw_state: Some(
                    if backgrounded {
                        "backgrounded; awaiting Qwen history"
                    } else {
                        "owned; awaiting Qwen history"
                    }
                    .into(),
                ),
                pid: None,
                started_at: Some(UNIX_EPOCH + Duration::from_millis(owned.created_at_ms)),
                updated_at: Some(UNIX_EPOCH + Duration::from_millis(owned.created_at_ms)),
                pull_requests: None,
                capabilities: BTreeSet::from([Capability::Inspect]),
            });
        }
        Ok(SourceDiscovery { sessions, warnings })
    }

    fn cancel(&self) {
        self.runner.cancel();
    }
}

pub struct QwenController {
    executable: String,
    ownership: Arc<QwenOwnership>,
}

impl QwenController {
    pub fn host(executable: impl Into<String>, ownership: Arc<QwenOwnership>) -> Self {
        Self {
            executable: executable.into(),
            ownership,
        }
    }

    fn open_native(&self, session: &AgentSession) -> Result<ControlOutcome> {
        if crate::native_session::is_backgrounded(&session.id) {
            return resume_native(&session.id, &session.provider_session_id, &session.name);
        }
        let mut command = Command::new(&self.executable);
        command
            .args(["--resume", &session.provider_session_id])
            .current_dir(&session.cwd);
        run_native(command, &session.id, &session.provider_session_id)
    }

    fn launch_foreground_with_security(
        &self,
        request: &LaunchRequest,
        yolo: bool,
    ) -> Result<ControlOutcome> {
        if request.provider != Provider::QwenCode {
            bail!("the Qwen Code controller cannot launch another provider");
        }
        if request.prompt.trim().is_empty() {
            bail!("the Qwen Code launch prompt cannot be empty");
        }
        let session_id = crate::native_session::new_session_id()?;
        let command = qwen_launch_command(&self.executable, request, &session_id, yolo);
        let launch_key = format!("qwen:host:{session_id}");
        let outcome = run_native_with_security(command, &launch_key, &session_id, yolo)?;
        // Persist ownership only after the native provider started and either
        // returned successfully or handed an exact live PTY back to OAV. A
        // spawn error or immediate non-zero exit therefore leaves no stale
        // ownership claim.
        if let Err(error) =
            self.ownership
                .record(&session_id, &request.cwd, &summarize(&request.prompt, 48))
        {
            if crate::native_session::is_backgrounded(&launch_key) {
                let _ = crate::native_session::terminate(&launch_key);
            }
            return Err(error).context(
                "could not persist Qwen Code ownership; stopped its live foreground bridge",
            );
        }
        Ok(outcome)
    }
}

impl ProviderController for QwenController {
    fn provider(&self) -> Provider {
        Provider::QwenCode
    }

    fn launch_mode(&self) -> LaunchMode {
        LaunchMode::SelectableModel
    }

    fn available_models(&self) -> Result<Vec<String>> {
        // Qwen Code 0.22 exposes provider-specific model selection through
        // `--model` and its native `/model` UI, but no stable, account-aware
        // machine-readable catalog. An empty catalog keeps OAV's exact-ID
        // entry path available without inventing aliases.
        Ok(Vec::new())
    }

    fn launch_presentation(&self) -> LaunchPresentation {
        LaunchPresentation::Foreground
    }

    fn supports_yolo(&self) -> bool {
        true
    }

    fn supports_authentication(&self) -> bool {
        true
    }

    fn authenticate(&self) -> Result<ControlOutcome> {
        // Qwen Code removed its old `qwen auth` subcommands. Authentication is
        // intentionally delegated to the provider's current native `/auth` UI.
        run_native_authentication(&self.executable, &[], Provider::QwenCode)
    }

    fn enrich(&self, snapshot: &mut SessionSnapshot) {
        for session in snapshot.sessions.iter_mut().filter(|session| {
            session.provider == Provider::QwenCode
                && self.ownership.owns(&session.provider_session_id)
        }) {
            if crate::native_session::is_backgrounded(&session.id) {
                session.capabilities.insert(Capability::Interrupt);
                session.state = SessionState::Working;
                session.kind = SessionKind::Managed;
            }
        }
    }

    fn launch_foreground(&self, request: &LaunchRequest) -> Result<ControlOutcome> {
        self.launch_foreground_with_security(request, false)
    }

    fn launch_yolo(&self, request: &LaunchRequest) -> Result<ControlOutcome> {
        self.launch_foreground_with_security(request, true)
    }

    fn launch_foreground_yolo(&self, request: &LaunchRequest) -> Result<ControlOutcome> {
        self.launch_foreground_with_security(request, true)
    }

    fn open(&self, session: &AgentSession) -> Result<ControlOutcome> {
        validate_session(session)?;
        if !self.ownership.owns(&session.provider_session_id) {
            bail!("refusing to open a Qwen Code session not created by Open Agent View");
        }
        self.open_native(session)
    }

    fn open_imported(&self, session: &AgentSession) -> Result<ControlOutcome> {
        validate_session(session)?;
        self.open_native(session)
    }

    fn interrupt(&self, session: &AgentSession) -> Result<ControlOutcome> {
        validate_session(session)?;
        if !self.ownership.owns(&session.provider_session_id) {
            bail!("refusing to stop a Qwen Code session not created by Open Agent View");
        }
        if !crate::native_session::is_backgrounded(&session.id) {
            bail!("Qwen Code is not backgrounded in this dashboard process");
        }
        crate::native_session::terminate(&session.id)?;
        Ok(ControlOutcome {
            message: format!("stopped Qwen Code session {}", session.name),
            provider_session_hint: Some(session.provider_session_id.clone()),
        })
    }

    fn inspect(&self, session: &AgentSession) -> Result<String> {
        validate_session(session)?;
        Ok(format!(
            "Qwen Code: {}\nSession: {}\nDirectory: {}\nState: {}\n\nEnter or Right opens the exact provider session in Qwen Code's native TUI.",
            session.name,
            session.provider_session_id,
            session.cwd.display(),
            session.state.heading()
        ))
    }
}

fn qwen_launch_command(
    executable: &str,
    request: &LaunchRequest,
    session_id: &str,
    yolo: bool,
) -> Command {
    let mut command = Command::new(executable);
    if yolo {
        command.arg("--yolo");
    }
    command.args(["--session-id", session_id]);
    if let Some(model) = request.model.as_deref() {
        command.args(["--model", model]);
    }
    command
        .args(["--prompt-interactive", request.prompt.trim()])
        .current_dir(&request.cwd);
    command
}

fn run_native(command: Command, key: &str, provider_id: &str) -> Result<ControlOutcome> {
    run_native_with_security(command, key, provider_id, false)
}

fn run_native_with_security(
    command: Command,
    key: &str,
    provider_id: &str,
    yolo: bool,
) -> Result<ControlOutcome> {
    let exit = if yolo {
        crate::native_session::run_yolo(command, key, "Qwen Code")?
    } else {
        crate::native_session::run(command, key)?
    };
    match exit {
        crate::native_session::NativeSessionExit::Backgrounded => Ok(ControlOutcome {
            message: format!(
                "backgrounded Qwen Code session {}; Enter/Right resumes it",
                provider_id.chars().take(8).collect::<String>()
            ),
            provider_session_hint: Some(provider_id.into()),
        }),
        crate::native_session::NativeSessionExit::Exited(status) if status.success() => {
            Ok(ControlOutcome {
                message: format!(
                    "returned from Qwen Code session {}",
                    provider_id.chars().take(8).collect::<String>()
                ),
                provider_session_hint: Some(provider_id.into()),
            })
        }
        crate::native_session::NativeSessionExit::Exited(status) => {
            bail!("Qwen Code session exited with status {status}")
        }
    }
}

fn resume_native(key: &str, provider_id: &str, name: &str) -> Result<ControlOutcome> {
    match crate::native_session::resume(key)? {
        crate::native_session::NativeSessionExit::Backgrounded => Ok(ControlOutcome {
            message: format!("backgrounded Qwen Code session {name}; Enter/Right resumes it"),
            provider_session_hint: Some(provider_id.into()),
        }),
        crate::native_session::NativeSessionExit::Exited(status) if status.success() => {
            Ok(ControlOutcome {
                message: format!("returned from Qwen Code session {name}"),
                provider_session_hint: Some(provider_id.into()),
            })
        }
        crate::native_session::NativeSessionExit::Exited(status) => {
            bail!("Qwen Code session exited with status {status}")
        }
    }
}

fn validate_session(session: &AgentSession) -> Result<()> {
    if session.provider != Provider::QwenCode || session.runtime != Runtime::Host {
        bail!("the host Qwen Code controller does not own this runtime");
    }
    if session.provider_session_id.trim().is_empty()
        || session
            .provider_session_id
            .chars()
            .any(|character| character.is_control() || character.is_whitespace())
    {
        bail!("invalid Qwen Code session ID");
    }
    Ok(())
}

fn parse_json_lines<T: for<'de> Deserialize<'de>>(input: &str, label: &str) -> Result<Vec<T>> {
    input
        .lines()
        .filter(|line| !line.trim().is_empty())
        .map(|line| serde_json::from_str(line).with_context(|| format!("invalid {label} JSONL")))
        .collect()
}

fn summarize(value: &str, limit: usize) -> String {
    let clean = value
        .chars()
        .map(|character| {
            if character.is_control() {
                ' '
            } else {
                character
            }
        })
        .collect::<String>();
    let clean = clean.split_whitespace().collect::<Vec<_>>().join(" ");
    if clean.is_empty() {
        return "Qwen Code session".into();
    }
    let mut result = clean.chars().take(limit).collect::<String>();
    if clean.chars().count() > limit {
        result.push('…');
    }
    result
}

fn now_millis() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis() as u64
}

fn persist_private_registry(path: &Path, records: &BTreeSet<OwnedQwenSession>) -> Result<()> {
    let parent = path.parent().context("Qwen registry has no parent")?;
    ensure_private_directory(parent)?;
    validate_private_state_path(path)?;
    let temporary = path.with_extension(format!("tmp-{}-{}", std::process::id(), now_millis()));
    reject_symlink(&temporary)?;
    let mut options = OpenOptions::new();
    options.create_new(true).write(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.mode(0o600);
    }
    let result = (|| -> Result<()> {
        let mut file = options.open(&temporary)?;
        serde_json::to_writer_pretty(&mut file, records)?;
        file.write_all(b"\n")?;
        file.sync_all()?;
        crate::fs_util::replace_file(&temporary, path)?;
        Ok(())
    })();
    if result.is_err() {
        let _ = fs::remove_file(&temporary);
    }
    result
}

fn validate_private_state_path(path: &Path) -> Result<()> {
    if let Some(parent) = path.parent() {
        if parent.exists() {
            ensure_private_directory(parent)?;
        }
    }
    match fs::symlink_metadata(path) {
        Ok(metadata) => {
            if metadata.file_type().is_symlink() || !metadata.is_file() {
                bail!("Qwen state path {} must be a regular file", path.display());
            }
            #[cfg(unix)]
            {
                use std::os::unix::fs::{MetadataExt, PermissionsExt};
                if metadata.uid() != unsafe { libc::geteuid() } {
                    bail!("Qwen state path {} has the wrong owner", path.display());
                }
                if metadata.permissions().mode() & 0o777 != 0o600 {
                    bail!("Qwen state path {} must have mode 0600", path.display());
                }
            }
            Ok(())
        }
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(()),
        Err(error) => Err(error.into()),
    }
}

fn ensure_private_directory(path: &Path) -> Result<()> {
    if !path.exists() {
        fs::create_dir_all(path)?;
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            fs::set_permissions(path, fs::Permissions::from_mode(0o700))?;
        }
    }
    let metadata = fs::symlink_metadata(path)?;
    if metadata.file_type().is_symlink() || !metadata.is_dir() {
        bail!(
            "Qwen state directory {} must be a real directory",
            path.display()
        );
    }
    #[cfg(unix)]
    {
        use std::os::unix::fs::{MetadataExt, PermissionsExt};
        if metadata.uid() != unsafe { libc::geteuid() } {
            bail!(
                "Qwen state directory {} has the wrong owner",
                path.display()
            );
        }
        if metadata.permissions().mode() & 0o777 != 0o700 {
            bail!(
                "Qwen state directory {} must have mode 0700",
                path.display()
            );
        }
    }
    Ok(())
}

fn reject_symlink(path: &Path) -> Result<()> {
    match fs::symlink_metadata(path) {
        Ok(metadata) if metadata.file_type().is_symlink() => {
            bail!("refusing symlinked Qwen state path {}", path.display())
        }
        Ok(_) => Ok(()),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(()),
        Err(error) => Err(error.into()),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::process::CommandOutput;
    use crate::test_support::tempfile;
    #[cfg(unix)]
    use std::os::unix::fs::PermissionsExt;

    #[derive(Default)]
    struct FakeRunner;

    impl CommandRunner for FakeRunner {
        fn run(&self, request: &CommandRequest) -> Result<CommandOutput> {
            let stdout = if request.args.get(1).map(String::as_str) == Some("ps") {
                r#"{"pid":42,"sessionId":"11111111-2222-4333-8444-555555555555","cwd":"/work","name":"qwen-live","startedAt":1700000000000,"qwenVersion":"0.22.1"}
"#
            } else {
                r#"{"sessionId":"11111111-2222-4333-8444-555555555555","startTime":"2026-08-25T00:00:00.000Z","mtime":1700000005000,"prompt":"fix the parser","gitBranch":null,"customTitle":"Parser work","titleSource":"manual","filePath":"/private/transcript.jsonl","cwd":"/work"}
{"sessionId":"aaaaaaaa-bbbb-4ccc-8ddd-eeeeeeeeeeee","startTime":"2026-08-24T00:00:00.000Z","mtime":1699990000000,"prompt":"external","gitBranch":null,"customTitle":null,"titleSource":null,"filePath":"/private/external.jsonl","cwd":"/else"}
"#
            };
            Ok(CommandOutput {
                status: 0,
                stdout: stdout.as_bytes().to_vec(),
                stderr: Vec::new(),
            })
        }
    }

    struct EmptyRunner;

    impl CommandRunner for EmptyRunner {
        fn run(&self, _request: &CommandRequest) -> Result<CommandOutput> {
            Ok(CommandOutput {
                status: 0,
                stdout: Vec::new(),
                stderr: Vec::new(),
            })
        }
    }

    struct FailingRunner;

    impl CommandRunner for FailingRunner {
        fn run(&self, _request: &CommandRequest) -> Result<CommandOutput> {
            Ok(CommandOutput {
                status: 17,
                stdout: Vec::new(),
                stderr: b"provider store is busy".to_vec(),
            })
        }
    }

    #[test]
    fn qwen_yolo_launch_uses_only_the_verified_native_flag() {
        let request = LaunchRequest {
            provider: Provider::QwenCode,
            model: Some("qwen3-coder-plus".into()),
            prompt: "fix the parser".into(),
            cwd: PathBuf::from("/work"),
        };
        let safe = qwen_launch_command("qwen", &request, "session-id", false);
        assert_eq!(
            safe.get_args().collect::<Vec<_>>(),
            [
                "--session-id",
                "session-id",
                "--model",
                "qwen3-coder-plus",
                "--prompt-interactive",
                "fix the parser",
            ]
        );
        let yolo = qwen_launch_command("qwen", &request, "session-id", true);
        assert_eq!(
            yolo.get_args().collect::<Vec<_>>(),
            [
                "--yolo",
                "--session-id",
                "session-id",
                "--model",
                "qwen3-coder-plus",
                "--prompt-interactive",
                "fix the parser",
            ]
        );
    }

    #[test]
    fn discovery_is_owned_by_default_and_merges_verified_live_registry() {
        let directory = tempfile::tempdir().unwrap();
        let ownership = QwenOwnership::load(directory.path().join("owned.json")).unwrap();
        ownership
            .record(
                "11111111-2222-4333-8444-555555555555",
                Path::new("/work"),
                "Parser work",
            )
            .unwrap();
        let source = QwenSource::with_runner("qwen", ownership, Arc::new(FakeRunner));
        let sessions = source.discover(&DiscoveryRequest::default()).unwrap();
        assert_eq!(sessions.len(), 1);
        assert_eq!(sessions[0].provider, Provider::QwenCode);
        assert_eq!(sessions[0].state, SessionState::Working);
        assert_eq!(sessions[0].kind, SessionKind::Managed);
        assert_eq!(sessions[0].pid, Some(42));
        assert_eq!(sessions[0].name, "Parser work");
    }

    #[test]
    fn qwen_timestamps_accept_fractional_milliseconds() {
        let history: QwenHistoryRecord = serde_json::from_str(
            r#"{"sessionId":"fractional","cwd":"/work","mtime":1787843988568.1904,"prompt":"hello","customTitle":null}"#,
        )
        .unwrap();
        let live: QwenLiveRecord = serde_json::from_str(
            r#"{"pid":42,"sessionId":"fractional","cwd":"/work","name":"hello","startedAt":"1787843988567.9"}"#,
        )
        .unwrap();
        assert_eq!(history.mtime, 1_787_843_988_568);
        assert_eq!(live.started_at, 1_787_843_988_567);
    }

    #[test]
    fn include_external_exposes_saved_history_without_control_authority() {
        let directory = tempfile::tempdir().unwrap();
        let ownership = QwenOwnership::load(directory.path().join("owned.json")).unwrap();
        let source = QwenSource::with_runner("qwen", ownership, Arc::new(FakeRunner));
        let sessions = source
            .discover(&DiscoveryRequest {
                include_external: true,
                ..DiscoveryRequest::default()
            })
            .unwrap();
        assert_eq!(sessions.len(), 2);
        assert!(sessions
            .iter()
            .all(|session| { session.capabilities == BTreeSet::from([Capability::Inspect]) }));
        assert!(sessions
            .iter()
            .filter(|session| session.provider_session_id.starts_with("aaaaaaaa"))
            .all(|session| session.kind == SessionKind::Unknown));
    }

    #[test]
    fn owned_launch_is_visible_before_qwen_flushes_history() {
        let directory = tempfile::tempdir().unwrap();
        let ownership = QwenOwnership::load(directory.path().join("owned.json")).unwrap();
        ownership
            .record(
                "11111111-2222-4333-8444-555555555555",
                Path::new("/work"),
                "hello",
            )
            .unwrap();
        let source = QwenSource::with_runner("qwen", ownership, Arc::new(EmptyRunner));

        let sessions = source.discover(&DiscoveryRequest::default()).unwrap();

        assert_eq!(sessions.len(), 1);
        assert_eq!(
            sessions[0].provider_session_id,
            "11111111-2222-4333-8444-555555555555"
        );
        assert_eq!(sessions[0].name, "hello");
        assert_eq!(sessions[0].kind, SessionKind::Managed);
        assert_eq!(sessions[0].state, SessionState::Completed);
        assert_eq!(
            sessions[0].raw_state.as_deref(),
            Some("owned; awaiting Qwen history")
        );
    }

    #[test]
    fn discovery_reloads_ownership_persisted_by_an_independent_handle() {
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("owned.json");
        let discovery_ownership = QwenOwnership::load(path.clone()).unwrap();
        let control_ownership = QwenOwnership::load(path).unwrap();
        control_ownership
            .record(
                "11111111-2222-4333-8444-555555555555",
                Path::new("/work"),
                "cross-handle session",
            )
            .unwrap();
        let source = QwenSource::with_runner("qwen", discovery_ownership, Arc::new(EmptyRunner));

        let sessions = source.discover(&DiscoveryRequest::default()).unwrap();

        assert_eq!(sessions.len(), 1);
        assert_eq!(sessions[0].name, "cross-handle session");
        assert_eq!(sessions[0].provider, Provider::QwenCode);
    }

    #[test]
    fn independent_handle_ownership_matches_the_richer_provider_record() {
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("owned.json");
        let discovery_ownership = QwenOwnership::load(path.clone()).unwrap();
        let control_ownership = QwenOwnership::load(path).unwrap();
        control_ownership
            .record(
                "11111111-2222-4333-8444-555555555555",
                Path::new("/work"),
                "launch prompt",
            )
            .unwrap();
        let source = QwenSource::with_runner("qwen", discovery_ownership, Arc::new(FakeRunner));

        let sessions = source.discover(&DiscoveryRequest::default()).unwrap();

        assert_eq!(sessions.len(), 1);
        assert_eq!(sessions[0].name, "Parser work");
        assert_eq!(sessions[0].state, SessionState::Working);
        assert_eq!(sessions[0].pid, Some(42));
        assert_eq!(sessions[0].raw_state.as_deref(), Some("running"));
    }

    #[test]
    fn owned_launch_remains_visible_when_qwen_catalog_commands_are_temporarily_unavailable() {
        let directory = tempfile::tempdir().unwrap();
        let ownership = QwenOwnership::load(directory.path().join("owned.json")).unwrap();
        ownership
            .record(
                "11111111-2222-4333-8444-555555555555",
                Path::new("/work"),
                "hello",
            )
            .unwrap();
        let source = QwenSource::with_runner("qwen", ownership, Arc::new(FailingRunner));

        let discovery = source
            .discover_with_warnings(&DiscoveryRequest {
                include_completed: true,
                ..DiscoveryRequest::default()
            })
            .unwrap();

        assert_eq!(discovery.sessions.len(), 1);
        assert_eq!(discovery.sessions[0].name, "hello");
        assert_eq!(discovery.warnings.len(), 2);
        assert!(discovery
            .warnings
            .iter()
            .any(|warning| warning.contains("live-session")));
        assert!(discovery
            .warnings
            .iter()
            .any(|warning| warning.contains("history")));
    }

    #[test]
    fn private_registry_refuses_symlink_target() {
        #[cfg(unix)]
        {
            use std::os::unix::fs::symlink;
            let directory = tempfile::tempdir().unwrap();
            let target = directory.path().join("target");
            fs::write(&target, "[]").unwrap();
            let link = directory.path().join("owned.json");
            symlink(target, &link).unwrap();
            assert!(QwenOwnership::load(link).is_err());
        }
    }

    #[cfg(unix)]
    #[test]
    fn private_registry_refuses_permissive_file_and_parent_modes() {
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("owned.json");
        fs::write(&path, "[]").unwrap();
        fs::set_permissions(&path, fs::Permissions::from_mode(0o644)).unwrap();
        assert!(QwenOwnership::load(path.clone()).is_err());

        fs::set_permissions(&path, fs::Permissions::from_mode(0o600)).unwrap();
        fs::set_permissions(directory.path(), fs::Permissions::from_mode(0o755)).unwrap();
        assert!(QwenOwnership::load(path).is_err());
    }

    #[cfg(unix)]
    #[test]
    fn failed_registry_persistence_does_not_claim_in_memory_ownership() {
        let directory = tempfile::tempdir().unwrap();
        let ownership = QwenOwnership::load(directory.path().join("owned.json")).unwrap();
        fs::set_permissions(directory.path(), fs::Permissions::from_mode(0o500)).unwrap();

        assert!(ownership
            .record("not-owned", Path::new("/work"), "not owned")
            .is_err());
        assert!(!ownership.owns("not-owned"));
    }

    #[test]
    fn invalid_json_lines_fail_closed() {
        assert!(parse_json_lines::<QwenHistoryRecord>("not-json\n", "Qwen").is_err());
    }

    #[cfg(unix)]
    #[test]
    fn failed_native_launch_does_not_record_false_ownership() {
        let directory = tempfile::tempdir().unwrap();
        let executable = directory.path().join("qwen");
        fs::write(&executable, "#!/bin/sh\nexit 17\n").unwrap();
        fs::set_permissions(&executable, fs::Permissions::from_mode(0o700)).unwrap();
        let ownership = QwenOwnership::load(directory.path().join("owned.json")).unwrap();
        let controller = QwenController::host(executable.display().to_string(), ownership.clone());
        let result = controller.launch_foreground(&LaunchRequest {
            provider: Provider::QwenCode,
            model: Some("qwen3-coder-plus".into()),
            prompt: "verify ownership rollback".into(),
            cwd: directory.path().to_owned(),
        });

        assert!(result.is_err());
        assert!(ownership.records.lock().unwrap().is_empty());
        assert!(!directory.path().join("owned.json").exists());
    }
}

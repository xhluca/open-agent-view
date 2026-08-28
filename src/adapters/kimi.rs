//! Current (Node/native, `~/.kimi-code`) Kimi Code integration.
//!
//! The older Python `kimi-cli` uses a different store. This adapter follows
//! the current official `session_index.jsonl` + per-session `state.json`
//! layout and resumes only exact IDs produced by the installed `kimi` binary.

use std::collections::{BTreeMap, BTreeSet};
use std::fs::{self, File};
use std::io::{BufRead, BufReader};
use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::Arc;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use anyhow::{bail, Context, Result};
use serde::Deserialize;
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

const MAX_INDEX_BYTES: u64 = 16 * 1024 * 1024;
const MAX_MODEL_BYTES: usize = 8 * 1024 * 1024;
const MAX_STATE_BYTES: u64 = 2 * 1024 * 1024;

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct KimiCommandSpec {
    pub program: String,
    pub args: Vec<String>,
    pub current_dir: PathBuf,
    /// The current Kimi TUI has no interactive prompt flag. These bytes are
    /// queued into its fresh private PTY after startup, preserving approvals
    /// and the native UI instead of falling back to autonomous `--prompt`.
    pub initial_input: Vec<u8>,
}

impl KimiCommandSpec {
    pub fn command(&self) -> Command {
        let mut command = Command::new(&self.program);
        command.args(&self.args).current_dir(&self.current_dir);
        command
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct KimiInvocation {
    executable: String,
}

impl KimiInvocation {
    pub fn host(executable: impl Into<String>) -> Self {
        Self {
            executable: executable.into(),
        }
    }

    pub fn launch(&self, cwd: &Path, prompt: &str, model: Option<&str>) -> Result<KimiCommandSpec> {
        require_cwd(cwd)?;
        require_text(prompt, "prompt")?;
        let mut args = Vec::new();
        if let Some(model) = model {
            require_text(model, "model")?;
            args.extend(["--model".into(), model.into()]);
        }
        let mut initial_input = prompt.trim().as_bytes().to_vec();
        initial_input.push(b'\r');
        Ok(KimiCommandSpec {
            program: self.executable.clone(),
            args,
            current_dir: cwd.to_owned(),
            initial_input,
        })
    }

    pub fn resume(&self, cwd: &Path, session_id: &str) -> Result<KimiCommandSpec> {
        require_cwd(cwd)?;
        validate_kimi_id(session_id)?;
        Ok(KimiCommandSpec {
            program: self.executable.clone(),
            args: vec!["--session".into(), session_id.into()],
            current_dir: cwd.to_owned(),
            initial_input: Vec::new(),
        })
    }
}

pub struct KimiOwnership {
    inner: NativeOwnership,
}

impl KimiOwnership {
    pub fn load_default() -> Result<Arc<Self>> {
        Self::load(default_kimi_ownership_path()?)
    }

    pub fn load(path: PathBuf) -> Result<Arc<Self>> {
        Ok(Arc::new(Self {
            inner: NativeOwnership::load(path, "Kimi Code")?,
        }))
    }
}

pub fn default_kimi_data_root() -> Result<PathBuf> {
    if let Some(home) = std::env::var_os("KIMI_CODE_HOME") {
        return Ok(PathBuf::from(home));
    }
    let home = std::env::var_os("HOME").context("HOME is not set")?;
    Ok(PathBuf::from(home).join(".kimi-code"))
}

pub fn default_kimi_ownership_path() -> Result<PathBuf> {
    if let Some(state_home) = std::env::var_os("XDG_STATE_HOME") {
        return Ok(PathBuf::from(state_home).join("open-agent-view/kimi-owned.json"));
    }
    let home = std::env::var_os("HOME").context("HOME is not set")?;
    Ok(PathBuf::from(home).join(".local/state/open-agent-view/kimi-owned.json"))
}

pub struct KimiSource {
    data_root: PathBuf,
    ownership: Arc<KimiOwnership>,
}

impl KimiSource {
    pub fn host(data_root: PathBuf, ownership: Arc<KimiOwnership>) -> Self {
        Self {
            data_root,
            ownership,
        }
    }

    pub fn host_default(ownership: Arc<KimiOwnership>) -> Result<Self> {
        Ok(Self::host(default_kimi_data_root()?, ownership))
    }
}

impl SessionSource for KimiSource {
    fn label(&self) -> &str {
        "Kimi Code (host)"
    }

    fn discover(&self, request: &DiscoveryRequest) -> Result<Vec<AgentSession>> {
        let index = read_kimi_index(&self.data_root)?;
        let owned = self
            .ownership
            .inner
            .records()
            .into_iter()
            .map(|record| (record.session_id.clone(), record))
            .collect::<BTreeMap<_, _>>();
        index
            .into_values()
            .filter(|entry| request.include_external || owned.contains_key(&entry.session_id))
            .map(|entry| kimi_session(&entry, owned.get(&entry.session_id)))
            .collect()
    }
}

pub struct KimiController {
    executable: String,
    invocation: KimiInvocation,
    data_root: PathBuf,
    ownership: Arc<KimiOwnership>,
    runner: Arc<dyn CommandRunner>,
}

impl KimiController {
    pub fn host(
        executable: impl Into<String>,
        data_root: PathBuf,
        ownership: Arc<KimiOwnership>,
    ) -> Self {
        let executable = executable.into();
        Self {
            invocation: KimiInvocation::host(executable.clone()),
            executable,
            data_root,
            ownership,
            runner: Arc::new(ProcessRunner),
        }
    }

    pub fn host_default(
        executable: impl Into<String>,
        ownership: Arc<KimiOwnership>,
    ) -> Result<Self> {
        Ok(Self::host(executable, default_kimi_data_root()?, ownership))
    }

    #[cfg(test)]
    fn with_runner(
        executable: impl Into<String>,
        data_root: PathBuf,
        ownership: Arc<KimiOwnership>,
        runner: Arc<dyn CommandRunner>,
    ) -> Self {
        let mut controller = Self::host(executable, data_root, ownership);
        controller.runner = runner;
        controller
    }
}

impl ProviderController for KimiController {
    fn provider(&self) -> Provider {
        Provider::KimiCode
    }

    fn launch_mode(&self) -> LaunchMode {
        LaunchMode::SelectableModel
    }

    fn launch_presentation(&self) -> LaunchPresentation {
        LaunchPresentation::Foreground
    }

    fn available_models(&self) -> Result<Vec<String>> {
        let mut request = CommandRequest::new(
            self.executable.clone(),
            vec!["provider".into(), "list".into(), "--json".into()],
        );
        request.timeout = Duration::from_secs(8);
        let output = self.runner.run(&request)?;
        if output.status != 0 {
            bail!(
                "Kimi Code model discovery exited with status {}: {}",
                output.status,
                output.stderr_lossy()
            );
        }
        if output.stdout.len() > MAX_MODEL_BYTES {
            bail!("Kimi Code model catalog exceeded the 8 MiB safety limit");
        }
        parse_kimi_models(output.stdout_text()?)
    }

    fn supports_authentication(&self) -> bool {
        true
    }

    fn authenticate(&self) -> Result<ControlOutcome> {
        run_native_authentication(&self.executable, &["login"], Provider::KimiCode)
    }

    fn enrich(&self, snapshot: &mut SessionSnapshot) {
        for session in snapshot.sessions.iter_mut().filter(|session| {
            session.provider == Provider::KimiCode
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
        if request.provider != Provider::KimiCode {
            bail!("the Kimi Code controller cannot launch another provider");
        }
        let before = read_kimi_index(&self.data_root)?
            .into_keys()
            .collect::<BTreeSet<_>>();
        let launch_id = crate::native_session::new_session_id()?;
        let launch_key = format!("kimi:host:launch-{launch_id}");
        let spec =
            self.invocation
                .launch(&request.cwd, &request.prompt, request.model.as_deref())?;
        let exit = crate::native_session::run_with_initial_input_after_screen(
            spec.command(),
            &launch_key,
            &spec.initial_input,
            "Send /help for help information.",
        )?;
        let entry = poll_unique(
            "one new Kimi Code session in the requested workspace",
            Duration::from_secs(5),
            || {
                Ok(read_kimi_index(&self.data_root)?
                    .into_values()
                    .filter(|entry| {
                        !before.contains(&entry.session_id) && entry.work_dir == request.cwd
                    })
                    .collect())
            },
        )?;
        self.ownership.inner.record(
            &entry.session_id,
            &request.cwd,
            &request.prompt,
            Some(&entry.session_dir),
            "Kimi Code",
        )?;
        if matches!(exit, crate::native_session::NativeSessionExit::Backgrounded) {
            crate::native_session::rename_key(
                &launch_key,
                &format!("kimi:host:{}", entry.session_id),
            )?;
        }
        native_outcome(exit, &entry.session_id)
    }

    fn open(&self, session: &AgentSession) -> Result<ControlOutcome> {
        validate_session(session)?;
        if crate::native_session::is_backgrounded(&session.id) {
            return native_outcome(
                crate::native_session::resume(&session.id)?,
                &session.provider_session_id,
            );
        }
        let spec = self
            .invocation
            .resume(&session.cwd, &session.provider_session_id)?;
        native_outcome(
            crate::native_session::run(spec.command(), &session.id)?,
            &session.provider_session_id,
        )
    }

    fn interrupt(&self, session: &AgentSession) -> Result<ControlOutcome> {
        validate_session(session)?;
        if !self.ownership.inner.owns(&session.provider_session_id) {
            bail!("refusing to stop a Kimi Code session not created by Open Agent View");
        }
        if !crate::native_session::is_backgrounded(&session.id) {
            bail!("Kimi Code is not backgrounded in this dashboard process");
        }
        crate::native_session::terminate(&session.id)?;
        Ok(ControlOutcome {
            message: format!("stopped Kimi Code session {}", session.name),
            provider_session_hint: Some(session.provider_session_id.clone()),
        })
    }

    fn inspect(&self, session: &AgentSession) -> Result<String> {
        validate_session(session)?;
        Ok(format!(
            "Kimi Code: {}\nSession: {}\nDirectory: {}\nState: {}\n\nEnter or Right opens the exact session in Kimi Code's native TUI.",
            session.name,
            session.provider_session_id,
            session.cwd.display(),
            session.state.heading()
        ))
    }
}

#[derive(Clone, Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct RawKimiIndexEntry {
    session_id: String,
    session_dir: Option<PathBuf>,
    work_dir: Option<PathBuf>,
    #[serde(default)]
    deleted: bool,
}

#[derive(Clone, Debug)]
struct KimiIndexEntry {
    session_id: String,
    session_dir: PathBuf,
    work_dir: PathBuf,
}

#[derive(Debug, Default, Deserialize)]
#[serde(rename_all = "camelCase")]
struct KimiState {
    title: Option<String>,
    last_prompt: Option<String>,
    created_at: Option<Value>,
    updated_at: Option<Value>,
    last_turn_reason: Option<String>,
    cwd: Option<PathBuf>,
    work_dir: Option<PathBuf>,
}

fn read_kimi_index(data_root: &Path) -> Result<BTreeMap<String, KimiIndexEntry>> {
    let path = data_root.join("session_index.jsonl");
    let metadata = match fs::symlink_metadata(&path) {
        Ok(metadata) => metadata,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(BTreeMap::new()),
        Err(error) => return Err(error.into()),
    };
    if metadata.file_type().is_symlink() || !metadata.is_file() {
        bail!("Kimi Code session index must be a real file");
    }
    if metadata.len() > MAX_INDEX_BYTES {
        bail!("Kimi Code session index exceeded the 16 MiB safety limit");
    }
    let file = File::open(&path)?;
    let sessions_path = data_root.join("sessions");
    let sessions_metadata = match fs::symlink_metadata(&sessions_path) {
        Ok(metadata) => metadata,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(BTreeMap::new()),
        Err(error) => return Err(error.into()),
    };
    if sessions_metadata.file_type().is_symlink() || !sessions_metadata.is_dir() {
        bail!("Kimi Code sessions root must be a real directory");
    }
    let mut entries = BTreeMap::new();
    let sessions_root = fs::canonicalize(sessions_path)?;
    for line in BufReader::new(file).lines() {
        let line = line?;
        if line.trim().is_empty() {
            continue;
        }
        // The provider appends concurrently. Match its own parser by ignoring a
        // malformed/truncated record instead of hiding every valid session.
        let Ok(entry) = serde_json::from_str::<RawKimiIndexEntry>(&line) else {
            continue;
        };
        if validate_kimi_id(&entry.session_id).is_err() {
            continue;
        }
        if entry.deleted {
            entries.remove(&entry.session_id);
        } else {
            let (Some(session_dir), Some(work_dir)) = (entry.session_dir, entry.work_dir) else {
                continue;
            };
            if !session_dir.is_absolute() || !work_dir.is_absolute() {
                continue;
            }
            let Ok(session_dir) = fs::canonicalize(session_dir) else {
                continue;
            };
            if session_dir.strip_prefix(&sessions_root).is_err()
                || session_dir.file_name().and_then(|value| value.to_str())
                    != Some(entry.session_id.as_str())
            {
                continue;
            }
            entries.insert(
                entry.session_id.clone(),
                KimiIndexEntry {
                    session_id: entry.session_id,
                    session_dir,
                    work_dir,
                },
            );
        }
    }
    Ok(entries)
}

fn kimi_session(
    entry: &KimiIndexEntry,
    owned: Option<&OwnedNativeSession>,
) -> Result<AgentSession> {
    let state_path = entry.session_dir.join("state.json");
    let state: KimiState = match fs::symlink_metadata(&state_path) {
        Ok(metadata) => {
            if metadata.file_type().is_symlink() || !metadata.is_file() {
                bail!("Kimi Code state.json must be a real file");
            }
            if metadata.len() > MAX_STATE_BYTES {
                bail!("Kimi Code state.json exceeded the 2 MiB safety limit");
            }
            let file = File::open(&state_path)?;
            serde_json::from_reader(BufReader::new(file)).context("invalid Kimi Code state.json")?
        }
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => KimiState::default(),
        Err(error) => return Err(error.into()),
    };
    let id = format!("kimi:host:{}", entry.session_id);
    let backgrounded = owned.is_some() && crate::native_session::is_backgrounded(&id);
    let mut capabilities = BTreeSet::from([Capability::Inspect]);
    if backgrounded {
        capabilities.insert(Capability::Interrupt);
    }
    let name = state
        .title
        .filter(|value| !value.trim().is_empty())
        .or_else(|| owned.map(|record| record.name.clone()))
        .unwrap_or_else(|| "Kimi Code session".into());
    let summary = state
        .last_prompt
        .map(|value| sanitize(&value, 180, &name))
        .unwrap_or_else(|| name.clone());
    let created_at = timestamp(state.created_at.as_ref())
        .or_else(|| owned.map(|record| UNIX_EPOCH + Duration::from_millis(record.created_at_ms)));
    let cwd = state
        .cwd
        .or(state.work_dir)
        .filter(|path| path.is_absolute())
        .unwrap_or_else(|| entry.work_dir.clone());
    Ok(AgentSession {
        id,
        provider_session_id: entry.session_id.clone(),
        provider: Provider::KimiCode,
        runtime: Runtime::Host,
        kind: if owned.is_some() {
            SessionKind::Managed
        } else {
            SessionKind::Interactive
        },
        name,
        cwd,
        state: if backgrounded {
            SessionState::Working
        } else {
            SessionState::Completed
        },
        summary,
        raw_state: Some(
            if backgrounded {
                "backgrounded"
            } else {
                state.last_turn_reason.as_deref().unwrap_or("saved")
            }
            .into(),
        ),
        pid: None,
        started_at: created_at,
        updated_at: timestamp(state.updated_at.as_ref())
            .or_else(|| fs::metadata(&state_path).ok()?.modified().ok()),
        pull_requests: None,
        capabilities,
    })
}

fn timestamp(value: Option<&Value>) -> Option<SystemTime> {
    match value? {
        Value::Number(number) => Some(UNIX_EPOCH + Duration::from_millis(number.as_u64()?)),
        // Current v2 sessions write millisecond numbers. Legacy/current-v1
        // state.json uses RFC3339 strings, as confirmed in the official store.
        Value::String(value) => value
            .parse::<u64>()
            .ok()
            .map(|millis| UNIX_EPOCH + Duration::from_millis(millis))
            .or_else(|| super::copilot::parse_copilot_updated_at(value)),
        _ => None,
    }
}

fn parse_kimi_models(input: &str) -> Result<Vec<String>> {
    let value: Value = serde_json::from_str(input).context("invalid Kimi Code provider JSON")?;
    let models = value
        .get("models")
        .and_then(Value::as_object)
        .context("Kimi Code provider output omitted models")?;
    let mut ids = models
        .keys()
        .filter(|model| require_text(model, "model").is_ok())
        .cloned()
        .collect::<Vec<_>>();
    ids.sort();
    ids.dedup();
    Ok(ids)
}

fn validate_session(session: &AgentSession) -> Result<()> {
    if session.provider != Provider::KimiCode || session.runtime != Runtime::Host {
        bail!("the host Kimi Code controller does not own this runtime");
    }
    validate_kimi_id(&session.provider_session_id)
}

fn validate_kimi_id(value: &str) -> Result<()> {
    validate_id(value, "Kimi Code")?;
    let suffix = value
        .strip_prefix("session_")
        .filter(|suffix| !suffix.is_empty())
        .context("invalid Kimi Code session ID")?;
    if !suffix
        .bytes()
        .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'.' | b'_' | b'-'))
    {
        bail!("invalid Kimi Code session ID");
    }
    Ok(())
}

fn require_cwd(cwd: &Path) -> Result<()> {
    if !cwd.is_absolute() {
        bail!("Kimi Code workspace must be absolute");
    }
    Ok(())
}

fn require_text(value: &str, field: &str) -> Result<()> {
    if value.trim().is_empty() || value.chars().any(char::is_control) {
        bail!("Kimi Code {field} is invalid");
    }
    Ok(())
}

fn native_outcome(
    exit: crate::native_session::NativeSessionExit,
    session_id: &str,
) -> Result<ControlOutcome> {
    match exit {
        crate::native_session::NativeSessionExit::Backgrounded => Ok(ControlOutcome {
            message: "backgrounded Kimi Code session; Enter/Right resumes it".into(),
            provider_session_hint: Some(session_id.into()),
        }),
        crate::native_session::NativeSessionExit::Exited(status) if status.success() => {
            Ok(ControlOutcome {
                message: "returned from Kimi Code session".into(),
                provider_session_hint: Some(session_id.into()),
            })
        }
        crate::native_session::NativeSessionExit::Exited(status) => {
            bail!("Kimi Code session exited with status {status}")
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::process::CommandOutput;
    use crate::test_support::tempfile;

    fn fixture(root: &Path) -> PathBuf {
        let session = root.join("sessions/wd_work/session_owned");
        fs::create_dir_all(&session).unwrap();
        fs::write(
            root.join("session_index.jsonl"),
            format!(
                "{{\"sessionId\":\"session_owned\",\"sessionDir\":{:?},\"workDir\":\"/work\"}}\n{{\"sessionId\":\"session_deleted\",\"sessionDir\":\"/gone\",\"workDir\":\"/work\"}}\n{{\"sessionId\":\"session_deleted\",\"deleted\":true}}\n",
                session.display().to_string()
            ),
        )
        .unwrap();
        fs::write(
            session.join("state.json"),
            r#"{"title":"Parser work","lastPrompt":"Fix the parser edge case","createdAt":"2026-05-20T05:59:51.085Z","updatedAt":"2026-05-21T03:12:08.000Z","lastTurnReason":"completed","workDir":"/work/from-state"}"#,
        )
        .unwrap();
        session
    }

    #[cfg(unix)]
    #[test]
    fn invocation_queues_prompt_in_native_tui_and_uses_documented_resume() {
        let invocation = KimiInvocation::host("kimi");
        let launch = invocation
            .launch(Path::new("/work"), "fix parser", Some("kimi-code/model"))
            .unwrap();
        assert_eq!(launch.args, ["--model", "kimi-code/model"]);
        assert_eq!(launch.initial_input, b"fix parser\r");
        assert_eq!(
            invocation
                .resume(Path::new("/work"), "session_owned")
                .unwrap()
                .args,
            ["--session", "session_owned"]
        );
        assert!(invocation
            .resume(Path::new("/work"), "../session_bad")
            .is_err());
        assert!(invocation
            .resume(Path::new("/work"), "not-a-kimi-id")
            .is_err());
    }

    #[cfg(unix)]
    #[test]
    fn index_tombstones_and_owned_filter_are_applied() {
        let directory = tempfile::tempdir().unwrap();
        let root = directory.path().join("kimi");
        fs::create_dir_all(&root).unwrap();
        let session = fixture(&root);
        let ownership = KimiOwnership::load(directory.path().join("owned.json")).unwrap();
        ownership
            .inner
            .record(
                "session_owned",
                Path::new("/work"),
                "Parser",
                Some(&session),
                "Kimi Code",
            )
            .unwrap();
        let sessions = KimiSource::host(root, ownership)
            .discover(&DiscoveryRequest {
                include_completed: true,
                ..DiscoveryRequest::default()
            })
            .unwrap();
        assert_eq!(sessions.len(), 1);
        assert_eq!(sessions[0].provider, Provider::KimiCode);
        assert_eq!(sessions[0].name, "Parser work");
        assert_eq!(sessions[0].summary, "Fix the parser edge case");
        assert_eq!(sessions[0].cwd, Path::new("/work/from-state"));
        assert_eq!(
            sessions[0].started_at,
            Some(UNIX_EPOCH + Duration::from_millis(1_779_256_791_085))
        );
        assert_eq!(
            sessions[0].updated_at,
            Some(UNIX_EPOCH + Duration::from_millis(1_779_333_128_000))
        );
    }

    #[cfg(unix)]
    #[test]
    fn external_history_is_explicit_and_never_marked_managed() {
        let directory = tempfile::tempdir().unwrap();
        let root = directory.path().join("kimi");
        fs::create_dir_all(&root).unwrap();
        fixture(&root);
        let ownership = KimiOwnership::load(directory.path().join("owned.json")).unwrap();
        let source = KimiSource::host(root, ownership);
        assert!(source
            .discover(&DiscoveryRequest::default())
            .unwrap()
            .is_empty());
        let external = source
            .discover(&DiscoveryRequest {
                include_external: true,
                include_completed: true,
                include_interactive: true,
                ..DiscoveryRequest::default()
            })
            .unwrap();
        assert_eq!(external.len(), 1);
        assert_eq!(external[0].kind, SessionKind::Interactive);
        assert_eq!(
            external[0].capabilities,
            BTreeSet::from([Capability::Inspect])
        );
    }

    struct ModelRunner;

    impl CommandRunner for ModelRunner {
        fn run(&self, request: &CommandRequest) -> Result<CommandOutput> {
            assert_eq!(request.args, ["provider", "list", "--json"]);
            Ok(CommandOutput {
                status: 0,
                stdout: br#"{"providers":{"kimi-code":{}},"models":{"kimi-code/zeta":{},"kimi-code/alpha":{}}}"#.to_vec(),
                stderr: Vec::new(),
            })
        }
    }

    #[test]
    fn configured_models_are_machine_readable_and_sorted() {
        let directory = tempfile::tempdir().unwrap();
        let ownership = KimiOwnership::load(directory.path().join("owned.json")).unwrap();
        let controller = KimiController::with_runner(
            "kimi",
            directory.path().join("kimi"),
            ownership,
            Arc::new(ModelRunner),
        );
        assert_eq!(
            controller.available_models().unwrap(),
            ["kimi-code/alpha", "kimi-code/zeta"]
        );
    }

    #[test]
    fn malformed_and_unsafe_index_records_are_skipped() {
        let directory = tempfile::tempdir().unwrap();
        let root = directory.path().join("kimi");
        fs::create_dir_all(root.join("sessions/wd_work")).unwrap();
        let outside = directory.path().join("session_unsafe");
        fs::create_dir(&outside).unwrap();
        fs::write(
            root.join("session_index.jsonl"),
            format!(
                "not-json\n{{\"sessionId\":\"session_unsafe\",\"sessionDir\":{:?},\"workDir\":\"/work\"}}\n",
                outside.display().to_string()
            ),
        )
        .unwrap();
        assert!(read_kimi_index(&root).unwrap().is_empty());
    }

    #[cfg(unix)]
    #[test]
    fn provider_index_and_state_never_follow_symlinks() {
        use std::os::unix::fs::symlink;

        let directory = tempfile::tempdir().unwrap();
        let root = directory.path().join("kimi");
        fs::create_dir_all(&root).unwrap();
        let external_index = directory.path().join("external-index.jsonl");
        fs::write(&external_index, "").unwrap();
        symlink(&external_index, root.join("session_index.jsonl")).unwrap();
        assert!(read_kimi_index(&root).is_err());

        fs::remove_file(root.join("session_index.jsonl")).unwrap();
        let session = fixture(&root);
        let state = session.join("state.json");
        fs::remove_file(&state).unwrap();
        let external_state = directory.path().join("external-state.json");
        fs::write(&external_state, r#"{"title":"outside"}"#).unwrap();
        symlink(external_state, state).unwrap();
        let ownership = KimiOwnership::load(directory.path().join("owned.json")).unwrap();
        assert!(KimiSource::host(root, ownership)
            .discover(&DiscoveryRequest {
                include_external: true,
                include_completed: true,
                include_interactive: true,
                ..DiscoveryRequest::default()
            })
            .is_err());
    }
}

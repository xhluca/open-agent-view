//! Muse Code host integration.
//!
//! Muse does not expose a machine-readable list command. OAV therefore owns
//! only sessions it observed being created through its native foreground PTY,
//! persists their exact UUID/log path, and reads the provider's append-only log
//! without modifying it.

use std::collections::BTreeSet;
use std::fs::{self, File};
use std::io::{BufReader, Read, Seek, SeekFrom};
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

const MAX_LOG_TAIL_BYTES: u64 = 4 * 1024 * 1024;
const MAX_MODEL_CATALOG_BYTES: u64 = 8 * 1024 * 1024;

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct MuseCommandSpec {
    pub program: String,
    pub args: Vec<String>,
    pub current_dir: PathBuf,
}

impl MuseCommandSpec {
    pub fn command(&self) -> Command {
        let mut command = Command::new(&self.program);
        command.args(&self.args).current_dir(&self.current_dir);
        command.env("MUSE_NO_AUTO_UPDATE", "1");
        command
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct MuseInvocation {
    executable: String,
}

impl MuseInvocation {
    pub fn host(executable: impl Into<String>) -> Self {
        Self {
            executable: executable.into(),
        }
    }

    pub fn launch(&self, cwd: &Path, prompt: &str, model: Option<&str>) -> Result<MuseCommandSpec> {
        require_cwd(cwd)?;
        require_text(prompt, "prompt")?;
        let mut args = Vec::new();
        if let Some(model) = model {
            require_text(model, "model")?;
            args.extend(["--model".into(), model.into()]);
        }
        // End option parsing so a valid user task beginning with `-` remains a
        // task instead of becoming a Muse permission/configuration flag.
        args.push("--".into());
        args.push(prompt.trim().into());
        Ok(MuseCommandSpec {
            program: self.executable.clone(),
            args,
            current_dir: cwd.to_owned(),
        })
    }

    pub fn resume(&self, cwd: &Path, session_id: &str) -> Result<MuseCommandSpec> {
        require_cwd(cwd)?;
        validate_muse_id(session_id)?;
        Ok(MuseCommandSpec {
            program: self.executable.clone(),
            args: vec!["resume".into(), session_id.into()],
            current_dir: cwd.to_owned(),
        })
    }
}

pub struct MuseOwnership {
    inner: NativeOwnership,
}

impl MuseOwnership {
    pub fn load_default() -> Result<Arc<Self>> {
        Self::load(default_muse_ownership_path()?)
    }

    pub fn load(path: PathBuf) -> Result<Arc<Self>> {
        Ok(Arc::new(Self {
            inner: NativeOwnership::load(path, "Muse Code")?,
        }))
    }
}

pub fn default_muse_data_root() -> Result<PathBuf> {
    if let Some(data_home) = std::env::var_os("XDG_DATA_HOME") {
        return Ok(PathBuf::from(data_home).join("muse"));
    }
    let home = std::env::var_os("HOME").context("HOME is not set")?;
    Ok(PathBuf::from(home).join(".local/share/muse"))
}

pub fn default_muse_ownership_path() -> Result<PathBuf> {
    if let Some(state_home) = std::env::var_os("XDG_STATE_HOME") {
        return Ok(PathBuf::from(state_home).join("open-agent-view/muse-owned.json"));
    }
    let home = std::env::var_os("HOME").context("HOME is not set")?;
    Ok(PathBuf::from(home).join(".local/state/open-agent-view/muse-owned.json"))
}

pub struct MuseSource {
    data_root: PathBuf,
    ownership: Arc<MuseOwnership>,
}

impl MuseSource {
    pub fn host(data_root: PathBuf, ownership: Arc<MuseOwnership>) -> Self {
        Self {
            data_root,
            ownership,
        }
    }

    pub fn host_default(ownership: Arc<MuseOwnership>) -> Result<Self> {
        Ok(Self::host(default_muse_data_root()?, ownership))
    }
}

impl SessionSource for MuseSource {
    fn label(&self) -> &str {
        "Muse Code (host)"
    }

    fn discover(&self, request: &DiscoveryRequest) -> Result<Vec<AgentSession>> {
        let mut records = self
            .ownership
            .inner
            .records()
            .into_iter()
            .map(|record| (record, true))
            .collect::<Vec<_>>();
        if request.include_external {
            let owned = records
                .iter()
                .map(|(record, _)| record.session_id.clone())
                .collect::<BTreeSet<_>>();
            for (session_id, path) in list_muse_sessions(&self.data_root)? {
                if owned.contains(&session_id) {
                    continue;
                }
                let parsed = parse_muse_log(&path)?;
                let Some(cwd) = parsed.cwd else { continue };
                records.push((
                    OwnedNativeSession {
                        session_id,
                        cwd,
                        created_at_ms: 0,
                        name: "Muse Code session".into(),
                        session_path: Some(path),
                    },
                    false,
                ));
            }
        }

        records
            .into_iter()
            .filter_map(
                |(record, owned)| match muse_session(&self.data_root, &record, owned) {
                    Ok(session) => Some(Ok(session)),
                    Err(error)
                        if error
                            .downcast_ref::<std::io::Error>()
                            .is_some_and(|error| error.kind() == std::io::ErrorKind::NotFound) =>
                    {
                        None
                    }
                    Err(error) => Some(Err(error)),
                },
            )
            .collect()
    }
}

pub struct MuseController {
    invocation: MuseInvocation,
    data_root: PathBuf,
    ownership: Arc<MuseOwnership>,
}

impl MuseController {
    pub fn host(
        executable: impl Into<String>,
        data_root: PathBuf,
        ownership: Arc<MuseOwnership>,
    ) -> Self {
        Self {
            invocation: MuseInvocation::host(executable),
            data_root,
            ownership,
        }
    }

    pub fn host_default(
        executable: impl Into<String>,
        ownership: Arc<MuseOwnership>,
    ) -> Result<Self> {
        Ok(Self::host(executable, default_muse_data_root()?, ownership))
    }
}

impl ProviderController for MuseController {
    fn provider(&self) -> Provider {
        Provider::MuseCode
    }

    fn launch_mode(&self) -> LaunchMode {
        LaunchMode::SelectableModel
    }

    fn launch_presentation(&self) -> LaunchPresentation {
        LaunchPresentation::Foreground
    }

    fn available_models(&self) -> Result<Vec<String>> {
        parse_muse_model_catalogs(&self.data_root.join("model-catalog"))
    }

    fn supports_authentication(&self) -> bool {
        true
    }

    fn authenticate(&self) -> Result<ControlOutcome> {
        run_native_authentication(&self.invocation.executable, &["login"], Provider::MuseCode)
    }

    fn enrich(&self, snapshot: &mut SessionSnapshot) {
        for session in snapshot.sessions.iter_mut().filter(|session| {
            session.provider == Provider::MuseCode
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
        if request.provider != Provider::MuseCode {
            bail!("the Muse Code controller cannot launch another provider");
        }
        let before = list_muse_sessions(&self.data_root)?
            .into_iter()
            .map(|(id, _)| id)
            .collect::<BTreeSet<_>>();
        let launch_id = crate::native_session::new_session_id()?;
        let launch_key = format!("muse:host:launch-{launch_id}");
        let spec =
            self.invocation
                .launch(&request.cwd, &request.prompt, request.model.as_deref())?;
        let exit = crate::native_session::run(spec.command(), &launch_key)?;
        let (session_id, path) = poll_unique(
            "one new Muse Code session in the requested workspace",
            Duration::from_secs(5),
            || {
                Ok(list_muse_sessions(&self.data_root)?
                    .into_iter()
                    .filter(|(id, _)| !before.contains(id))
                    .filter_map(|(id, path)| {
                        parse_muse_log(&path)
                            .ok()
                            .filter(|log| log.cwd.as_deref() == Some(request.cwd.as_path()))
                            .map(|_| (id, path))
                    })
                    .collect())
            },
        )?;
        self.ownership.inner.record(
            &session_id,
            &request.cwd,
            &request.prompt,
            Some(&path),
            "Muse Code",
        )?;
        if matches!(exit, crate::native_session::NativeSessionExit::Backgrounded) {
            crate::native_session::rename_key(&launch_key, &format!("muse:host:{session_id}"))?;
        }
        native_outcome(exit, &session_id, "Muse Code")
    }

    fn open(&self, session: &AgentSession) -> Result<ControlOutcome> {
        validate_session(session)?;
        if crate::native_session::is_backgrounded(&session.id) {
            return native_outcome(
                crate::native_session::resume(&session.id)?,
                &session.provider_session_id,
                "Muse Code",
            );
        }
        let spec = self
            .invocation
            .resume(&session.cwd, &session.provider_session_id)?;
        native_outcome(
            crate::native_session::run(spec.command(), &session.id)?,
            &session.provider_session_id,
            "Muse Code",
        )
    }

    fn interrupt(&self, session: &AgentSession) -> Result<ControlOutcome> {
        validate_session(session)?;
        if !self.ownership.inner.owns(&session.provider_session_id) {
            bail!("refusing to stop a Muse Code session not created by Open Agent View");
        }
        if !crate::native_session::is_backgrounded(&session.id) {
            bail!("Muse Code is not backgrounded in this dashboard process");
        }
        crate::native_session::terminate(&session.id)?;
        Ok(ControlOutcome {
            message: format!("stopped Muse Code session {}", session.name),
            provider_session_hint: Some(session.provider_session_id.clone()),
        })
    }

    fn inspect(&self, session: &AgentSession) -> Result<String> {
        validate_session(session)?;
        Ok(format!(
            "Muse Code: {}\nSession: {}\nDirectory: {}\nState: {}\n\nEnter or Right opens the exact session in Muse Code's native TUI.",
            session.name,
            session.provider_session_id,
            session.cwd.display(),
            session.state.heading()
        ))
    }
}

#[derive(Default)]
struct ParsedMuseLog {
    cwd: Option<PathBuf>,
    summary: String,
    updated_at: Option<SystemTime>,
}

fn muse_session(
    data_root: &Path,
    record: &OwnedNativeSession,
    owned: bool,
) -> Result<AgentSession> {
    validate_muse_id(&record.session_id)?;
    let path = record
        .session_path
        .as_ref()
        .context("owned Muse Code session has no provider log path")?;
    let sessions_root = fs::canonicalize(data_root.join("sessions"))?;
    let path = fs::canonicalize(path)?;
    if path.strip_prefix(&sessions_root).is_err()
        || path.file_name().and_then(|value| value.to_str()) != Some("session.jsonl")
        || path
            .parent()
            .and_then(Path::file_name)
            .and_then(|value| value.to_str())
            != Some(record.session_id.as_str())
    {
        bail!("Muse Code session log is outside the verified provider store");
    }
    let parsed = parse_muse_log(&path)?;
    let id = format!("muse:host:{}", record.session_id);
    let backgrounded = owned && crate::native_session::is_backgrounded(&id);
    let mut capabilities = BTreeSet::from([Capability::Inspect]);
    if backgrounded {
        capabilities.insert(Capability::Interrupt);
    }
    Ok(AgentSession {
        id,
        provider_session_id: record.session_id.clone(),
        provider: Provider::MuseCode,
        runtime: Runtime::Host,
        kind: if owned {
            SessionKind::Managed
        } else {
            SessionKind::Interactive
        },
        name: record.name.clone(),
        cwd: parsed.cwd.unwrap_or_else(|| record.cwd.clone()),
        state: if backgrounded {
            SessionState::Working
        } else {
            SessionState::Completed
        },
        summary: if parsed.summary.is_empty() {
            record.name.clone()
        } else {
            parsed.summary
        },
        raw_state: Some(
            if backgrounded {
                "backgrounded"
            } else {
                "saved"
            }
            .into(),
        ),
        pid: None,
        started_at: (record.created_at_ms > 0)
            .then(|| UNIX_EPOCH + Duration::from_millis(record.created_at_ms)),
        updated_at: parsed.updated_at,
        pull_requests: None,
        capabilities,
    })
}

fn parse_muse_log(path: &Path) -> Result<ParsedMuseLog> {
    let mut file = File::open(path)?;
    let metadata = file.metadata()?;
    // Workspace metadata is written at the beginning of the append-only log.
    // Read a bounded prefix separately so large transcripts do not lose cwd.
    let mut prefix = String::new();
    (&file)
        .take(64 * 1024)
        .read_to_string(&mut prefix)
        .context("failed to read Muse Code session metadata")?;
    let cwd = prefix.lines().find_map(|line| {
        serde_json::from_str::<Value>(line)
            .ok()?
            .pointer("/payload/record/workspace_root")?
            .as_str()
            .map(PathBuf::from)
    });
    let start = metadata.len().saturating_sub(MAX_LOG_TAIL_BYTES);
    file.seek(SeekFrom::Start(start))?;
    let mut input = String::new();
    file.take(MAX_LOG_TAIL_BYTES + 1)
        .read_to_string(&mut input)?;
    if start > 0 {
        input = input
            .split_once('\n')
            .map(|(_, rest)| rest)
            .unwrap_or("")
            .into();
    }
    let mut parsed = ParsedMuseLog {
        cwd,
        updated_at: metadata.modified().ok(),
        ..ParsedMuseLog::default()
    };
    for line in input.lines().filter(|line| !line.trim().is_empty()) {
        let Ok(value) = serde_json::from_str::<Value>(line) else {
            continue;
        };
        if let Some(text) = value
            .pointer("/payload/event/text")
            .and_then(Value::as_str)
            .or_else(|| {
                value
                    .pointer("/payload/event/prompt")
                    .and_then(Value::as_str)
            })
        {
            parsed.summary = sanitize(text, 180, "Muse Code session");
        }
    }
    Ok(parsed)
}

fn list_muse_sessions(data_root: &Path) -> Result<Vec<(String, PathBuf)>> {
    let root = data_root.join("sessions");
    let metadata = match fs::symlink_metadata(&root) {
        Ok(metadata) => metadata,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(Vec::new()),
        Err(error) => return Err(error.into()),
    };
    if metadata.file_type().is_symlink() || !metadata.is_dir() {
        bail!("Muse Code sessions root must be a real directory");
    }
    let mut current = vec![root];
    for _ in 0..4 {
        let mut next = Vec::new();
        for directory in current {
            for entry in fs::read_dir(directory)? {
                let entry = entry?;
                if entry.file_type()?.is_dir() {
                    next.push(entry.path());
                }
            }
        }
        current = next;
    }
    let mut sessions = current
        .into_iter()
        .filter_map(|directory| {
            let path = directory.join("session.jsonl");
            if !path.is_file() {
                return None;
            }
            let id = directory.file_name()?.to_str()?.to_owned();
            validate_muse_id(&id).ok()?;
            Some((id, path))
        })
        .collect::<Vec<_>>();
    sessions.sort_by(|left, right| left.0.cmp(&right.0));
    Ok(sessions)
}

fn parse_muse_model_catalogs(directory: &Path) -> Result<Vec<String>> {
    let metadata = match fs::symlink_metadata(directory) {
        Ok(metadata) => metadata,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(Vec::new()),
        Err(error) => return Err(error.into()),
    };
    if metadata.file_type().is_symlink() || !metadata.is_dir() {
        bail!("Muse Code model catalog root must be a real directory");
    }
    let mut models = BTreeSet::new();
    for entry in fs::read_dir(directory)? {
        let entry = entry?;
        if !entry.file_type()?.is_file()
            || entry.path().extension().and_then(|v| v.to_str()) != Some("json")
        {
            continue;
        }
        if entry.metadata()?.len() > MAX_MODEL_CATALOG_BYTES {
            bail!("Muse Code model catalog exceeded the 8 MiB safety limit");
        }
        let value: Value = serde_json::from_reader(BufReader::new(File::open(entry.path())?))?;
        if let Some(rows) = value.get("rows").and_then(Value::as_array) {
            for model in rows
                .iter()
                .filter_map(|row| row.get("model_id").and_then(Value::as_str))
            {
                if require_text(model, "model").is_ok() {
                    models.insert(model.to_owned());
                }
            }
        }
    }
    Ok(models.into_iter().collect())
}

fn validate_session(session: &AgentSession) -> Result<()> {
    if session.provider != Provider::MuseCode || session.runtime != Runtime::Host {
        bail!("the host Muse Code controller does not own this runtime");
    }
    validate_muse_id(&session.provider_session_id)
}

fn validate_muse_id(value: &str) -> Result<()> {
    validate_id(value, "Muse Code")?;
    if value.len() != 36
        || value.bytes().enumerate().any(|(index, byte)| {
            if matches!(index, 8 | 13 | 18 | 23) {
                byte != b'-'
            } else {
                !byte.is_ascii_hexdigit()
            }
        })
    {
        bail!("invalid Muse Code session UUID");
    }
    Ok(())
}

fn require_cwd(cwd: &Path) -> Result<()> {
    if !cwd.is_absolute() {
        bail!("Muse Code workspace must be absolute");
    }
    Ok(())
}

fn require_text(value: &str, field: &str) -> Result<()> {
    if value.trim().is_empty() || value.chars().any(char::is_control) {
        bail!("Muse Code {field} is invalid");
    }
    Ok(())
}

fn native_outcome(
    exit: crate::native_session::NativeSessionExit,
    session_id: &str,
    label: &str,
) -> Result<ControlOutcome> {
    match exit {
        crate::native_session::NativeSessionExit::Backgrounded => Ok(ControlOutcome {
            message: format!("backgrounded {label} session; Enter/Right resumes it"),
            provider_session_hint: Some(session_id.into()),
        }),
        crate::native_session::NativeSessionExit::Exited(status) if status.success() => {
            Ok(ControlOutcome {
                message: format!("returned from {label} session"),
                provider_session_hint: Some(session_id.into()),
            })
        }
        crate::native_session::NativeSessionExit::Exited(status) => {
            bail!("{label} session exited with status {status}")
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn fixture_log(path: &Path, id: &str, cwd: &str) -> PathBuf {
        let directory = path.join("sessions/2026/08/25").join(id);
        fs::create_dir_all(&directory).unwrap();
        let log = directory.join("session.jsonl");
        let lines = [
            serde_json::json!({
                "payload_type": "runtime.session.metadata",
                "payload": {"record": {"workspace_root": cwd}}
            })
            .to_string(),
            serde_json::json!({
                "payload": {"event": {"kind": "started", "prompt": "first prompt"}}
            })
            .to_string(),
            "{broken".into(),
            serde_json::json!({
                "payload": {"event": {
                    "kind": "assistant_message_committed",
                    "text": "latest answer"
                }}
            })
            .to_string(),
        ];
        fs::write(&log, format!("{}\n", lines.join("\n"))).unwrap();
        log
    }

    #[test]
    fn invocation_uses_documented_launch_and_resume_argv() {
        let invocation = MuseInvocation::host("muse");
        assert_eq!(
            invocation
                .launch(Path::new("/work"), "fix parser", Some("muse-spark"))
                .unwrap()
                .args,
            ["--model", "muse-spark", "--", "fix parser"]
        );
        assert_eq!(
            invocation
                .resume(Path::new("/work"), "11111111-2222-4333-8444-555555555555")
                .unwrap()
                .args,
            ["resume", "11111111-2222-4333-8444-555555555555"]
        );
        assert!(invocation.launch(Path::new("work"), "x", None).is_err());
        assert!(invocation.launch(Path::new("/work"), "\n", None).is_err());
        assert!(invocation.resume(Path::new("/work"), "--help").is_err());
    }

    #[test]
    fn owned_discovery_reads_latest_summary_and_ignores_malformed_tail_lines() {
        let directory = tempfile::tempdir().unwrap();
        let data = directory.path().join("data/muse");
        let id = "11111111-2222-4333-8444-555555555555";
        let log = fixture_log(&data, id, "/work");
        let ownership = MuseOwnership::load(directory.path().join("owned.json")).unwrap();
        ownership
            .inner
            .record(id, Path::new("/work"), "Parser", Some(&log), "Muse Code")
            .unwrap();
        let sessions = MuseSource::host(data, ownership)
            .discover(&DiscoveryRequest {
                include_completed: true,
                ..DiscoveryRequest::default()
            })
            .unwrap();
        assert_eq!(sessions.len(), 1);
        assert_eq!(sessions[0].provider, Provider::MuseCode);
        assert_eq!(sessions[0].summary, "latest answer");
        assert_eq!(sessions[0].cwd, Path::new("/work"));
        assert_eq!(
            sessions[0].capabilities,
            BTreeSet::from([Capability::Inspect])
        );
    }

    #[test]
    fn external_history_is_opt_in() {
        let directory = tempfile::tempdir().unwrap();
        let data = directory.path().join("data/muse");
        fixture_log(&data, "aaaaaaaa-bbbb-4ccc-8ddd-eeeeeeeeeeee", "/else");
        let ownership = MuseOwnership::load(directory.path().join("owned.json")).unwrap();
        let source = MuseSource::host(data, ownership);
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

    #[test]
    fn model_catalogs_are_deduplicated_and_bounded() {
        let directory = tempfile::tempdir().unwrap();
        fs::write(
            directory.path().join("meta.json"),
            r#"{"rows":[{"model_id":"muse-spark-1.2"},{"model_id":"muse-spark-1.2"},{"model_id":"muse-spark-1.1"}]}"#,
        )
        .unwrap();
        assert_eq!(
            parse_muse_model_catalogs(directory.path()).unwrap(),
            ["muse-spark-1.1", "muse-spark-1.2"]
        );
    }

    #[cfg(unix)]
    #[test]
    fn provider_session_and_model_roots_never_follow_symlinks() {
        use std::os::unix::fs::symlink;

        let directory = tempfile::tempdir().unwrap();
        let data = directory.path().join("data/muse");
        fs::create_dir_all(&data).unwrap();
        let external_sessions = directory.path().join("external-sessions");
        let external_models = directory.path().join("external-models");
        fs::create_dir(&external_sessions).unwrap();
        fs::create_dir(&external_models).unwrap();
        symlink(&external_sessions, data.join("sessions")).unwrap();
        symlink(&external_models, data.join("model-catalog")).unwrap();

        assert!(list_muse_sessions(&data).is_err());
        assert!(parse_muse_model_catalogs(&data.join("model-catalog")).is_err());
    }
}

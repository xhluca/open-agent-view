//! Safe bridge to the external `session-migrate` CLI.
//!
//! Open Agent View never rewrites provider transcripts itself. It passes one
//! exact native session ID to session-migrate, validates its JSON result, and
//! records only enough metadata to keep the imported row visible in OAV.

use std::collections::{BTreeMap, BTreeSet};
use std::fs::{self, File, OpenOptions};
use std::io::Write;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use anyhow::{bail, Context, Result};
use serde::{Deserialize, Serialize};

use crate::adapters::{DiscoveryRequest, SessionSource};
use crate::domain::{AgentSession, Provider, Runtime, SessionKind, SessionState};
use crate::process::{CommandRequest, CommandRunner, ProcessRunner};

const REGISTRY_VERSION: u32 = 1;
const MIGRATION_TIMEOUT: Duration = Duration::from_secs(300);
const MAX_TEXT_BYTES: usize = 4096;
const MAX_NAME_BYTES: usize = 240;
static TEMPORARY_SEQUENCE: AtomicU64 = AtomicU64::new(1);

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct MigrationRequest {
    pub source: AgentSession,
    pub target: Provider,
    pub name: String,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct MigrationOutcome {
    pub session_id: String,
    pub normalized_id: String,
    pub warnings: Vec<String>,
}

#[derive(Clone)]
pub struct MigrationClient {
    executable: String,
    source_clis: BTreeMap<Provider, String>,
    runner: Arc<dyn CommandRunner>,
}

impl MigrationClient {
    pub fn host(executable: impl Into<String>) -> Self {
        Self::with_runner(executable, Arc::new(ProcessRunner))
    }

    pub fn with_runner(executable: impl Into<String>, runner: Arc<dyn CommandRunner>) -> Self {
        Self {
            executable: executable.into(),
            source_clis: BTreeMap::from([
                (Provider::OpenCode, "opencode".into()),
                (Provider::KiloCode, "kilo".into()),
            ]),
            runner,
        }
    }

    pub fn with_source_cli(mut self, provider: Provider, executable: impl Into<String>) -> Self {
        if matches!(provider, Provider::OpenCode | Provider::KiloCode) {
            self.source_clis.insert(provider, executable.into());
        }
        self
    }

    pub fn migrate(&self, request: &MigrationRequest) -> Result<MigrationOutcome> {
        let source_format = provider_format(&request.source.provider)
            .context("the selected session's harness is not supported by session-migrate")?;
        let target_format = provider_format(&request.target)
            .context("the selected target harness is not supported by session-migrate")?;
        if request.source.provider == request.target {
            bail!("source and target harness must be different");
        }
        validate_text(&request.source.provider_session_id, "source session ID")?;
        validate_name(&request.name)?;

        let mut args = vec![
            "transfer".into(),
            request.source.provider_session_id.clone(),
            "--from".into(),
            source_format.into(),
            "--to".into(),
            target_format.into(),
            "--cwd".into(),
            request.source.cwd.to_string_lossy().into_owned(),
        ];
        if supports_source_cwd(&request.source.provider) {
            args.push("--source-cwd".into());
            args.push(request.source.cwd.to_string_lossy().into_owned());
        }
        if let Some(executable) = self.source_clis.get(&request.source.provider) {
            args.push("--source-cli".into());
            args.push(executable.clone());
        }
        let mut command = CommandRequest::new(self.executable.clone(), args);
        command.timeout = MIGRATION_TIMEOUT;
        let output = self.runner.run(&command).with_context(|| {
            "could not run session-migrate; install it with `curl -LsSf https://session-migrate.github.io/install.sh | sh`"
        })?;
        if output.status != 0 {
            let detail = output.stderr_lossy();
            bail!(
                "session-migrate exited with status {}{}",
                output.status,
                if detail.is_empty() {
                    String::new()
                } else {
                    format!(": {detail}")
                }
            );
        }
        let result: CliResult = serde_json::from_slice(&output.stdout)
            .context("session-migrate returned invalid JSON")?;
        if result.target_format != target_format {
            bail!(
                "session-migrate returned target {:?}, expected {target_format}",
                result.target_format
            );
        }
        validate_text(&result.session_id, "migrated session ID")?;
        let normalized_id = normalized_session_id(&request.target, &result.session_id)?;
        let warnings = result
            .warnings
            .into_iter()
            .map(CliWarning::into_text)
            .collect::<Result<Vec<_>>>()?;
        Ok(MigrationOutcome {
            session_id: result.session_id,
            normalized_id,
            warnings,
        })
    }
}

#[derive(Debug, Deserialize)]
struct CliResult {
    session_id: String,
    target_format: String,
    #[serde(default)]
    warnings: Vec<CliWarning>,
}

/// session-migrate 0.4 and newer returns structured warning objects. Accept
/// the earlier string shape as well so OAV remains compatible with both sides
/// of the CLI boundary while exposing one concise message to the dashboard.
#[derive(Debug, Deserialize)]
#[serde(untagged)]
enum CliWarning {
    Text(String),
    Structured { code: Option<String>, message: String },
}

impl CliWarning {
    fn into_text(self) -> Result<String> {
        let value = match self {
            Self::Text(value) => value,
            Self::Structured { code, message } => match code {
                Some(code) if !code.trim().is_empty() => {
                    format!("{}: {}", code.trim(), message.trim())
                }
                Some(_) | None => message.trim().to_owned(),
            },
        };
        validate_text(&value, "session-migrate warning")?;
        Ok(value)
    }
}

pub fn migration_targets(source: &Provider) -> Vec<Provider> {
    Provider::CODING_HARNESSES
        .iter()
        .filter(|provider| *provider != source)
        .cloned()
        .collect()
}

pub fn provider_format(provider: &Provider) -> Option<&'static str> {
    Some(match provider {
        Provider::Claude => "claude",
        Provider::Codex => "codex",
        Provider::Pi => "pi",
        Provider::OpenCode => "opencode",
        Provider::Cursor => "cursor",
        Provider::GitHubCopilot => "copilot",
        Provider::Antigravity => "antigravity",
        Provider::MistralVibe => "vibe",
        Provider::MuseCode => "muse",
        Provider::QwenCode => "qwen",
        Provider::KimiCode => "kimi",
        Provider::OhMyPi => "omp",
        Provider::Grok => "grok",
        Provider::KiloCode => "kilo",
        Provider::OpenHands => "openhands",
        Provider::Terminal | Provider::Other(_) => return None,
    })
}

fn supports_source_cwd(provider: &Provider) -> bool {
    matches!(
        provider,
        Provider::Claude
            | Provider::Pi
            | Provider::Cursor
            | Provider::MistralVibe
            | Provider::QwenCode
            | Provider::KimiCode
            | Provider::OhMyPi
            | Provider::Grok
    )
}

pub fn normalized_session_id(provider: &Provider, provider_session_id: &str) -> Result<String> {
    validate_text(provider_session_id, "migrated session ID")?;
    let slug = match provider {
        Provider::Claude => "claude",
        Provider::Codex => "codex",
        Provider::Pi => "pi",
        Provider::OpenCode => "opencode",
        Provider::Cursor => "cursor",
        Provider::GitHubCopilot => "github_copilot",
        Provider::Antigravity => "antigravity",
        Provider::MistralVibe => "mistral_vibe",
        Provider::MuseCode => "muse",
        Provider::QwenCode => "qwen",
        Provider::KimiCode => "kimi",
        Provider::OhMyPi => "omp",
        Provider::Grok => "grok",
        Provider::KiloCode => "kilo",
        Provider::OpenHands => "openhands",
        Provider::Terminal | Provider::Other(_) => bail!("unsupported migration target"),
    };
    Ok(format!("{slug}:host:{provider_session_id}"))
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct MigrationRecord {
    pub id: String,
    pub provider_session_id: String,
    pub provider: Provider,
    pub source_provider: Provider,
    pub source_session_id: String,
    pub name: String,
    pub cwd: PathBuf,
    pub migrated_at_ms: u64,
}

#[derive(Debug, Deserialize, Serialize)]
struct RegistryDocument {
    version: u32,
    sessions: Vec<MigrationRecord>,
}

/// Private local index of successful imports. It does not claim provider
/// mutation ownership; it only keeps exact imported IDs visible in OAV.
#[derive(Clone, Debug)]
pub struct MigrationRegistry {
    path: PathBuf,
    records: Arc<Mutex<BTreeMap<String, MigrationRecord>>>,
}

impl MigrationRegistry {
    pub fn load_default() -> Result<Self> {
        let path = if let Some(state_home) = std::env::var_os("XDG_STATE_HOME") {
            PathBuf::from(state_home)
                .join("open-agent-view")
                .join("migrations.json")
        } else {
            let home = std::env::var_os("HOME").context("HOME is not set")?;
            PathBuf::from(home).join(".local/state/open-agent-view/migrations.json")
        };
        Self::load(path)
    }

    pub fn load(path: impl Into<PathBuf>) -> Result<Self> {
        let path = path.into();
        let parent = path
            .parent()
            .context("migration registry path has no parent")?;
        ensure_private_directory(parent)?;
        let records = read_registry(&path)?;
        Ok(Self {
            path,
            records: Arc::new(Mutex::new(records)),
        })
    }

    pub fn record(
        &self,
        request: &MigrationRequest,
        outcome: &MigrationOutcome,
    ) -> Result<MigrationRecord> {
        let record = MigrationRecord {
            id: outcome.normalized_id.clone(),
            provider_session_id: outcome.session_id.clone(),
            provider: request.target.clone(),
            source_provider: request.source.provider.clone(),
            source_session_id: request.source.provider_session_id.clone(),
            name: request.name.trim().to_owned(),
            cwd: request.source.cwd.clone(),
            migrated_at_ms: now_millis(),
        };
        validate_record(&record)?;
        let parent = self
            .path
            .parent()
            .context("migration registry path has no parent")?;
        let _lock = RegistryLock::acquire(&parent.join("migrations.lock"))?;
        let mut records = read_registry(&self.path)?;
        records.insert(record.id.clone(), record.clone());
        write_registry(&self.path, &records)?;
        *self
            .records
            .lock()
            .expect("migration registry mutex poisoned") = records;
        Ok(record)
    }

    pub fn list(&self) -> Vec<MigrationRecord> {
        self.records
            .lock()
            .expect("migration registry mutex poisoned")
            .values()
            .cloned()
            .collect()
    }

    pub fn contains_exact(&self, session: &AgentSession) -> bool {
        self.records
            .lock()
            .expect("migration registry mutex poisoned")
            .get(&session.id)
            .is_some_and(|record| {
                record.provider == session.provider
                    && record.provider_session_id == session.provider_session_id
            })
    }
}

impl SessionSource for MigrationRegistry {
    fn label(&self) -> &str {
        "session-migrate imports"
    }

    fn discover(&self, _request: &DiscoveryRequest) -> Result<Vec<AgentSession>> {
        let records = read_registry(&self.path)?;
        *self
            .records
            .lock()
            .expect("migration registry mutex poisoned") = records.clone();
        Ok(records.values().map(record_to_session).collect())
    }
}

fn record_to_session(record: &MigrationRecord) -> AgentSession {
    let migrated_at = UNIX_EPOCH + Duration::from_millis(record.migrated_at_ms);
    AgentSession {
        id: record.id.clone(),
        provider_session_id: record.provider_session_id.clone(),
        provider: record.provider.clone(),
        runtime: Runtime::Host,
        kind: SessionKind::Managed,
        name: record.name.clone(),
        cwd: record.cwd.clone(),
        state: SessionState::Completed,
        summary: format!("Migrated from {}", record.source_provider.label()),
        raw_state: Some("session-migrate import".into()),
        pid: None,
        started_at: Some(migrated_at),
        updated_at: Some(migrated_at),
        pull_requests: None,
        capabilities: BTreeSet::new(),
    }
}

fn validate_record(record: &MigrationRecord) -> Result<()> {
    validate_text(&record.id, "normalized session ID")?;
    validate_text(&record.provider_session_id, "provider session ID")?;
    validate_text(&record.source_session_id, "source session ID")?;
    validate_name(&record.name)?;
    if provider_format(&record.provider).is_none()
        || provider_format(&record.source_provider).is_none()
    {
        bail!("migration registry contains an unsupported harness");
    }
    if record.id != normalized_session_id(&record.provider, &record.provider_session_id)? {
        bail!("migration registry contains a mismatched normalized ID");
    }
    Ok(())
}

fn validate_text(value: &str, label: &str) -> Result<()> {
    let value = value.trim();
    if value.is_empty() || value.len() > MAX_TEXT_BYTES {
        bail!("{label} must contain between 1 and {MAX_TEXT_BYTES} bytes");
    }
    if value.chars().any(char::is_control) {
        bail!("{label} cannot contain control characters");
    }
    Ok(())
}

fn validate_name(value: &str) -> Result<()> {
    let value = value.trim();
    if value.is_empty() || value.len() > MAX_NAME_BYTES {
        bail!("migration name must contain between 1 and {MAX_NAME_BYTES} bytes");
    }
    if value.chars().any(char::is_control) {
        bail!("migration name cannot contain control characters");
    }
    Ok(())
}

fn read_registry(path: &Path) -> Result<BTreeMap<String, MigrationRecord>> {
    match fs::symlink_metadata(path) {
        Ok(_) => ensure_private_regular_file(path)?,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(BTreeMap::new()),
        Err(error) => return Err(error).context("failed to inspect migration registry"),
    }
    let input = fs::read_to_string(path)
        .with_context(|| format!("failed to read migration registry {}", path.display()))?;
    let document: RegistryDocument = serde_json::from_str(&input)
        .with_context(|| format!("invalid migration registry {}", path.display()))?;
    if document.version != REGISTRY_VERSION {
        bail!(
            "unsupported migration registry version {}",
            document.version
        );
    }
    let mut records = BTreeMap::new();
    for record in document.sessions {
        validate_record(&record)?;
        if records.insert(record.id.clone(), record).is_some() {
            bail!("duplicate migrated session ID in {}", path.display());
        }
    }
    Ok(records)
}

fn write_registry(path: &Path, records: &BTreeMap<String, MigrationRecord>) -> Result<()> {
    let document = RegistryDocument {
        version: REGISTRY_VERSION,
        sessions: records.values().cloned().collect(),
    };
    let temporary = temporary_path(path)?;
    let result = (|| {
        let mut options = OpenOptions::new();
        options.create_new(true).write(true);
        #[cfg(unix)]
        {
            use std::os::unix::fs::OpenOptionsExt;
            options.mode(0o600);
        }
        let mut file = options.open(&temporary)?;
        serde_json::to_writer_pretty(&mut file, &document)?;
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
        bail!("{} must be a real directory", path.display());
    }
    #[cfg(unix)]
    {
        use std::os::unix::fs::{MetadataExt, PermissionsExt};
        if metadata.uid() != unsafe { libc::geteuid() } {
            bail!("{} is not owned by the current user", path.display());
        }
        if metadata.permissions().mode() & 0o777 != 0o700 {
            bail!("{} must have mode 0700", path.display());
        }
    }
    Ok(())
}

fn ensure_private_regular_file(path: &Path) -> Result<()> {
    let metadata = fs::symlink_metadata(path)
        .with_context(|| format!("failed to inspect {}", path.display()))?;
    if metadata.file_type().is_symlink() || !metadata.is_file() {
        bail!("{} must be a regular file", path.display());
    }
    #[cfg(unix)]
    {
        use std::os::unix::fs::{MetadataExt, PermissionsExt};
        if metadata.uid() != unsafe { libc::geteuid() } {
            bail!("{} is not owned by the current user", path.display());
        }
        if metadata.permissions().mode() & 0o777 != 0o600 {
            bail!("{} must have mode 0600", path.display());
        }
    }
    Ok(())
}

#[derive(Debug)]
struct RegistryLock {
    file: File,
}

impl RegistryLock {
    fn acquire(path: &Path) -> Result<Self> {
        match fs::symlink_metadata(path) {
            Ok(_) => ensure_private_regular_file(path)?,
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
            Err(error) => {
                return Err(error).with_context(|| format!("failed to inspect {}", path.display()))
            }
        }
        let mut options = OpenOptions::new();
        options.create(true).read(true).write(true);
        #[cfg(unix)]
        {
            use std::os::unix::fs::OpenOptionsExt;
            options.mode(0o600);
        }
        let file = options
            .open(path)
            .with_context(|| format!("failed to open {}", path.display()))?;
        #[cfg(unix)]
        {
            use std::os::fd::AsRawFd;
            if unsafe { libc::flock(file.as_raw_fd(), libc::LOCK_EX) } != 0 {
                return Err(std::io::Error::last_os_error())
                    .context("failed to lock migration registry");
            }
        }
        Ok(Self { file })
    }
}

impl Drop for RegistryLock {
    fn drop(&mut self) {
        #[cfg(unix)]
        {
            use std::os::fd::AsRawFd;
            let _ = unsafe { libc::flock(self.file.as_raw_fd(), libc::LOCK_UN) };
        }
    }
}

fn temporary_path(path: &Path) -> Result<PathBuf> {
    let name = path
        .file_name()
        .context("migration registry path has no file name")?;
    let sequence = TEMPORARY_SEQUENCE.fetch_add(1, Ordering::Relaxed);
    Ok(path.with_file_name(format!(
        ".{}.tmp-{}-{sequence}",
        name.to_string_lossy(),
        std::process::id()
    )))
}

fn now_millis() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis() as u64
}

#[cfg(test)]
mod tests {
    use std::sync::Mutex;

    use tempfile::TempDir;

    use crate::process::CommandOutput;

    use super::*;

    fn private_tempdir() -> TempDir {
        let directory = crate::test_support::tempfile::tempdir().unwrap();
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            fs::set_permissions(directory.path(), fs::Permissions::from_mode(0o700)).unwrap();
        }
        directory
    }

    #[derive(Default)]
    struct FakeRunner {
        requests: Mutex<Vec<CommandRequest>>,
        output: Mutex<Option<Result<CommandOutput, String>>>,
    }

    impl CommandRunner for FakeRunner {
        fn run(&self, request: &CommandRequest) -> Result<CommandOutput> {
            self.requests.lock().unwrap().push(request.clone());
            match self.output.lock().unwrap().take().unwrap() {
                Ok(output) => Ok(output),
                Err(error) => bail!(error),
            }
        }
    }

    fn source(provider: Provider) -> AgentSession {
        AgentSession {
            id: "claude:host:source-id".into(),
            provider_session_id: "source-id".into(),
            provider,
            runtime: Runtime::Host,
            kind: SessionKind::Managed,
            name: "source".into(),
            cwd: PathBuf::from("/work/project"),
            state: SessionState::Completed,
            summary: String::new(),
            raw_state: None,
            pid: None,
            started_at: None,
            updated_at: None,
            pull_requests: None,
            capabilities: BTreeSet::new(),
        }
    }

    fn success() -> CommandOutput {
        CommandOutput {
            status: 0,
            stdout: br#"{"session_id":"target-id","target_format":"codex","warnings":["one"]}"#
                .to_vec(),
            stderr: Vec::new(),
        }
    }

    #[test]
    fn every_coding_harness_has_an_exact_session_migrate_mapping() {
        assert_eq!(Provider::CODING_HARNESS_COUNT, 15);
        for provider in Provider::CODING_HARNESSES {
            assert!(provider_format(&provider).is_some(), "{provider:?}");
            assert_eq!(migration_targets(&provider).len(), 14);
            assert!(!migration_targets(&provider).contains(&provider));
        }
        assert_eq!(provider_format(&Provider::Terminal), None);
    }

    #[test]
    fn invokes_transfer_with_exact_ids_and_supported_source_cwd() {
        let runner = Arc::new(FakeRunner::default());
        *runner.output.lock().unwrap() = Some(Ok(success()));
        let client = MigrationClient::with_runner("smigrate", runner.clone());
        let outcome = client
            .migrate(&MigrationRequest {
                source: source(Provider::Claude),
                target: Provider::Codex,
                name: "source (Codex)".into(),
            })
            .unwrap();
        assert_eq!(outcome.normalized_id, "codex:host:target-id");
        assert_eq!(outcome.warnings, vec!["one"]);
        let request = &runner.requests.lock().unwrap()[0];
        assert_eq!(request.program, "smigrate");
        assert_eq!(
            request.args,
            [
                "transfer",
                "source-id",
                "--from",
                "claude",
                "--to",
                "codex",
                "--cwd",
                "/work/project",
                "--source-cwd",
                "/work/project"
            ]
        );
        assert_eq!(request.timeout, MIGRATION_TIMEOUT);
    }

    #[test]
    fn accepts_current_structured_warnings_and_legacy_strings() {
        let runner = Arc::new(FakeRunner::default());
        *runner.output.lock().unwrap() = Some(Ok(CommandOutput {
            status: 0,
            stdout: br#"{
              "session_id":"target-id",
              "target_format":"codex",
              "warnings":[
                {"code":"retained_context","message":"context was retained"},
                "legacy warning"
              ]
            }"#
            .to_vec(),
            stderr: Vec::new(),
        }));
        let outcome = MigrationClient::with_runner("session-migrate", runner)
            .migrate(&MigrationRequest {
                source: source(Provider::Claude),
                target: Provider::Codex,
                name: "copy".into(),
            })
            .unwrap();
        assert_eq!(
            outcome.warnings,
            [
                "retained_context: context was retained",
                "legacy warning",
            ]
        );
    }

    #[test]
    fn rejects_control_characters_in_cli_warnings() {
        let runner = Arc::new(FakeRunner::default());
        *runner.output.lock().unwrap() = Some(Ok(CommandOutput {
            status: 0,
            stdout: b"{\"session_id\":\"target-id\",\"target_format\":\"codex\",\"warnings\":[{\"message\":\"bad\\nnotice\"}]}".to_vec(),
            stderr: Vec::new(),
        }));
        let error = MigrationClient::with_runner("session-migrate", runner)
            .migrate(&MigrationRequest {
                source: source(Provider::Claude),
                target: Provider::Codex,
                name: "copy".into(),
            })
            .unwrap_err()
            .to_string();
        assert!(error.contains("cannot contain control characters"));
    }

    #[test]
    fn virtual_sources_use_the_configured_cli_without_unsupported_source_cwd() {
        let runner = Arc::new(FakeRunner::default());
        *runner.output.lock().unwrap() = Some(Ok(success()));
        MigrationClient::with_runner("session-migrate", runner.clone())
            .with_source_cli(Provider::OpenCode, "/custom/opencode")
            .migrate(&MigrationRequest {
                source: source(Provider::OpenCode),
                target: Provider::Codex,
                name: "copy".into(),
            })
            .unwrap();
        assert!(!runner.requests.lock().unwrap()[0]
            .args
            .contains(&"--source-cwd".to_owned()));
        assert!(runner.requests.lock().unwrap()[0]
            .args
            .windows(2)
            .any(|args| args == ["--source-cli", "/custom/opencode"]));
    }

    #[test]
    fn grok_source_receives_workspace_disambiguation() {
        let runner = Arc::new(FakeRunner::default());
        *runner.output.lock().unwrap() = Some(Ok(success()));
        MigrationClient::with_runner("session-migrate", runner.clone())
            .migrate(&MigrationRequest {
                source: source(Provider::Grok),
                target: Provider::Codex,
                name: "copy".into(),
            })
            .unwrap();
        assert!(runner.requests.lock().unwrap()[0]
            .args
            .windows(2)
            .any(|args| args == ["--source-cwd", "/work/project"]));
    }

    #[test]
    fn nonzero_exit_is_reported() {
        let runner = Arc::new(FakeRunner::default());
        *runner.output.lock().unwrap() = Some(Ok(CommandOutput {
            status: 2,
            stdout: Vec::new(),
            stderr: b"source missing".to_vec(),
        }));
        let error = MigrationClient::with_runner("session-migrate", runner)
            .migrate(&MigrationRequest {
                source: source(Provider::Claude),
                target: Provider::Codex,
                name: "copy".into(),
            })
            .unwrap_err()
            .to_string();
        assert!(error.contains("status 2: source missing"));
    }

    #[test]
    fn invalid_or_mismatched_json_is_rejected() {
        let invalid = Arc::new(FakeRunner::default());
        *invalid.output.lock().unwrap() = Some(Ok(CommandOutput {
            status: 0,
            stdout: b"not JSON".to_vec(),
            stderr: Vec::new(),
        }));
        let request = MigrationRequest {
            source: source(Provider::Claude),
            target: Provider::Codex,
            name: "copy".into(),
        };
        assert!(MigrationClient::with_runner("session-migrate", invalid)
            .migrate(&request)
            .unwrap_err()
            .to_string()
            .contains("invalid JSON"));

        let mismatch = Arc::new(FakeRunner::default());
        *mismatch.output.lock().unwrap() = Some(Ok(CommandOutput {
            status: 0,
            stdout: br#"{"session_id":"target-id","target_format":"pi"}"#.to_vec(),
            stderr: Vec::new(),
        }));
        assert!(MigrationClient::with_runner("session-migrate", mismatch)
            .migrate(&request)
            .unwrap_err()
            .to_string()
            .contains("expected codex"));
    }

    #[test]
    fn same_harness_is_rejected_before_starting_the_cli() {
        let runner = Arc::new(FakeRunner::default());
        let error = MigrationClient::with_runner("session-migrate", runner.clone())
            .migrate(&MigrationRequest {
                source: source(Provider::Codex),
                target: Provider::Codex,
                name: "copy".into(),
            })
            .unwrap_err()
            .to_string();
        assert_eq!(error, "source and target harness must be different");
        assert!(runner.requests.lock().unwrap().is_empty());
    }

    #[test]
    fn registry_persists_and_discovers_imported_rows() {
        let directory = private_tempdir();
        let registry = MigrationRegistry::load(directory.path().join("migrations.json")).unwrap();
        let request = MigrationRequest {
            source: source(Provider::Claude),
            target: Provider::Codex,
            name: "source (Codex)".into(),
        };
        registry
            .record(
                &request,
                &MigrationOutcome {
                    session_id: "target-id".into(),
                    normalized_id: "codex:host:target-id".into(),
                    warnings: Vec::new(),
                },
            )
            .unwrap();
        let reloaded = MigrationRegistry::load(directory.path().join("migrations.json")).unwrap();
        let sessions = reloaded.discover(&DiscoveryRequest::default()).unwrap();
        assert_eq!(sessions.len(), 1);
        assert_eq!(sessions[0].name, "source (Codex)");
        assert_eq!(sessions[0].summary, "Migrated from Claude");
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            assert_eq!(
                fs::metadata(directory.path().join("migrations.json"))
                    .unwrap()
                    .permissions()
                    .mode()
                    & 0o777,
                0o600
            );
        }
    }

    #[test]
    fn registry_rejects_public_files_symlinks_and_oversized_names() {
        let directory = private_tempdir();
        let path = directory.path().join("migrations.json");
        let registry = MigrationRegistry::load(&path).unwrap();
        let request = MigrationRequest {
            source: source(Provider::Claude),
            target: Provider::Codex,
            name: "x".repeat(MAX_NAME_BYTES + 1),
        };
        assert!(registry
            .record(
                &request,
                &MigrationOutcome {
                    session_id: "target-id".into(),
                    normalized_id: "codex:host:target-id".into(),
                    warnings: Vec::new(),
                },
            )
            .is_err());

        #[cfg(unix)]
        {
            use std::os::unix::fs::{symlink, PermissionsExt};
            fs::write(&path, r#"{"version":1,"sessions":[]}"#).unwrap();
            fs::set_permissions(&path, fs::Permissions::from_mode(0o644)).unwrap();
            assert!(MigrationRegistry::load(&path).is_err());

            let target = directory.path().join("target.json");
            fs::write(&target, r#"{"version":1,"sessions":[]}"#).unwrap();
            let link = directory.path().join("link.json");
            symlink(&target, &link).unwrap();
            assert!(MigrationRegistry::load(&link).is_err());
        }
    }
}

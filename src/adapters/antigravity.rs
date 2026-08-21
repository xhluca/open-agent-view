use std::collections::{BTreeMap, BTreeSet};
use std::fs::{self, OpenOptions};
use std::io::Write;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::{Arc, Mutex};
use std::thread;
use std::time::{Duration, SystemTime};

use anyhow::{anyhow, bail, Context, Result};
use serde::{Deserialize, Serialize};

use super::{DiscoveryRequest, SessionSource};
use crate::control::{
    run_native_authentication, ControlOutcome, LaunchMode, LaunchPresentation, LaunchRequest,
    ProviderController,
};
use crate::domain::{AgentSession, Provider, Runtime, SessionKind, SessionState};
use crate::process::{CommandRequest, CommandRunner, ProcessRunner};

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct AntigravityCommandSpec {
    pub program: String,
    pub args: Vec<String>,
    pub current_dir: PathBuf,
}

impl AntigravityCommandSpec {
    pub fn command(&self) -> Command {
        let mut command = Command::new(&self.program);
        command.args(&self.args).current_dir(&self.current_dir);
        command
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct AntigravityInvocation {
    executable: String,
}

impl AntigravityInvocation {
    pub fn host(executable: impl Into<String>) -> Self {
        Self {
            executable: executable.into(),
        }
    }

    /// Build Antigravity's documented native conversation-resume command.
    pub fn resume(&self, conversation_id: &str, cwd: &Path) -> Result<AntigravityCommandSpec> {
        require_conversation_id(conversation_id)?;
        require_absolute_workspace(cwd)?;
        Ok(AntigravityCommandSpec {
            program: self.executable.clone(),
            args: vec!["--conversation".into(), conversation_id.into()],
            current_dir: cwd.to_owned(),
        })
    }

    /// Build a sandboxed new-session command without permission bypass flags.
    pub fn sandboxed_launch(
        &self,
        cwd: &Path,
        prompt: &str,
        model: Option<&str>,
    ) -> Result<AntigravityCommandSpec> {
        require_absolute_workspace(cwd)?;
        if prompt.trim().is_empty() {
            bail!("Antigravity prompt must not be empty");
        }
        let mut args = vec!["--sandbox".into()];
        if let Some(model) = model {
            require_model(model)?;
            args.extend(["--model".into(), model.into()]);
        }
        args.extend(["--prompt-interactive".into(), prompt.trim().into()]);
        Ok(AntigravityCommandSpec {
            program: self.executable.clone(),
            args,
            current_dir: cwd.to_owned(),
        })
    }
}

#[derive(Clone, Debug, Deserialize, Eq, Ord, PartialEq, PartialOrd, Serialize)]
#[serde(rename_all = "camelCase")]
struct OwnedAntigravityConversation {
    workspace: PathBuf,
    conversation_id: String,
    created_at_ms: u64,
}

/// Durable ownership boundary for conversations created through OAV.
pub struct AntigravityOwnership {
    path: PathBuf,
    records: Mutex<BTreeSet<OwnedAntigravityConversation>>,
}

impl AntigravityOwnership {
    pub fn load_default() -> Result<Arc<Self>> {
        let path = default_antigravity_ownership_path()?;
        reject_symlink(&path)?;
        reject_insecure_registry_permissions(&path)?;
        let records = match fs::read_to_string(&path) {
            Ok(input) => serde_json::from_str(&input).with_context(|| {
                format!("invalid Antigravity ownership registry {}", path.display())
            })?,
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => BTreeSet::new(),
            Err(error) => return Err(error.into()),
        };
        Ok(Arc::new(Self {
            path,
            records: Mutex::new(records),
        }))
    }

    fn owns(&self, workspace: &Path, conversation_id: &str) -> bool {
        self.records
            .lock()
            .map(|records| {
                records.iter().any(|record| {
                    record.workspace == workspace && record.conversation_id == conversation_id
                })
            })
            .unwrap_or(false)
    }

    fn record(&self, workspace: &Path, conversation_id: &str) -> Result<()> {
        let mut records = self
            .records
            .lock()
            .map_err(|_| anyhow!("Antigravity ownership registry lock was poisoned"))?;
        records.insert(OwnedAntigravityConversation {
            workspace: workspace.to_owned(),
            conversation_id: conversation_id.into(),
            created_at_ms: now_millis(),
        });
        let parent = self
            .path
            .parent()
            .context("Antigravity registry has no parent")?;
        reject_symlink(parent)?;
        reject_symlink(&self.path)?;
        reject_insecure_registry_permissions(&self.path)?;
        fs::create_dir_all(parent)?;
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            fs::set_permissions(parent, fs::Permissions::from_mode(0o700))?;
        }
        let temporary = self
            .path
            .with_extension(format!("tmp-{}", std::process::id()));
        let mut options = OpenOptions::new();
        options.create(true).truncate(true).write(true);
        #[cfg(unix)]
        {
            use std::os::unix::fs::OpenOptionsExt;
            options.mode(0o600);
        }
        let mut file = options.open(&temporary)?;
        serde_json::to_writer_pretty(&mut file, &*records)?;
        file.write_all(b"\n")?;
        file.sync_all()?;
        fs::rename(temporary, &self.path)?;
        Ok(())
    }
}

pub fn default_antigravity_last_conversations_path() -> Result<PathBuf> {
    let home = std::env::var_os("HOME").context("HOME is not set")?;
    Ok(PathBuf::from(home).join(".gemini/antigravity-cli/cache/last_conversations.json"))
}

pub fn default_antigravity_ownership_path() -> Result<PathBuf> {
    if let Some(state_home) = std::env::var_os("XDG_STATE_HOME") {
        return Ok(PathBuf::from(state_home).join("open-agent-view/antigravity/sessions.json"));
    }
    let home = std::env::var_os("HOME").context("HOME is not set")?;
    Ok(PathBuf::from(home).join(".local/state/open-agent-view/antigravity/sessions.json"))
}

pub struct AntigravitySource {
    path: PathBuf,
    ownership: Option<Arc<AntigravityOwnership>>,
}

impl AntigravitySource {
    pub fn host(path: PathBuf) -> Self {
        Self {
            path,
            ownership: None,
        }
    }

    pub fn default_host() -> Result<Self> {
        Ok(Self::host(default_antigravity_last_conversations_path()?))
    }

    pub fn managed(ownership: Arc<AntigravityOwnership>) -> Result<Self> {
        Ok(Self {
            path: default_antigravity_last_conversations_path()?,
            ownership: Some(ownership),
        })
    }
}

impl SessionSource for AntigravitySource {
    fn label(&self) -> &str {
        "Antigravity (host)"
    }

    fn discover(&self, request: &DiscoveryRequest) -> Result<Vec<AgentSession>> {
        let input = match fs::read_to_string(&self.path) {
            Ok(input) => input,
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(Vec::new()),
            Err(error) => {
                return Err(error).with_context(|| {
                    format!(
                        "failed to read Antigravity conversation cache {}",
                        self.path.display()
                    )
                })
            }
        };
        let sessions = parse_antigravity_last_conversations(&input, Runtime::Host)?;
        Ok(sessions
            .into_iter()
            .filter(|session| {
                let visible = request.include_external
                    || self.ownership.as_ref().is_some_and(|ownership| {
                        ownership.owns(&session.cwd, &session.provider_session_id)
                    });
                visible
                    && session.cwd.is_dir()
                    && request
                        .cwd
                        .as_ref()
                        .map(|cwd| session.cwd.starts_with(cwd))
                        .unwrap_or(true)
            })
            .collect())
    }
}

pub fn parse_antigravity_last_conversations(
    input: &str,
    runtime: Runtime,
) -> Result<Vec<AgentSession>> {
    let records: BTreeMap<String, String> =
        serde_json::from_str(input).context("invalid Antigravity last_conversations.json cache")?;
    let mut sessions = Vec::with_capacity(records.len());
    for (workspace, conversation_id) in records {
        let cwd = PathBuf::from(workspace);
        require_absolute_workspace(&cwd)?;
        require_conversation_id(&conversation_id)?;
        let runtime_id = match &runtime {
            Runtime::Host => "host",
            Runtime::Docker { container_id, .. } => container_id,
        };
        let workspace_name = cwd
            .file_name()
            .and_then(|name| name.to_str())
            .filter(|name| !name.is_empty())
            .unwrap_or("antigravity");
        sessions.push(AgentSession {
            id: format!("antigravity:{runtime_id}:{conversation_id}"),
            provider_session_id: conversation_id,
            provider: Provider::Antigravity,
            runtime: runtime.clone(),
            kind: SessionKind::Unknown,
            name: format!("{workspace_name} (last conversation)"),
            cwd,
            // The documented cache contains no lifecycle or activity fields.
            state: SessionState::Unknown,
            summary: "Most recent Antigravity conversation for this workspace".into(),
            raw_state: Some("cached_last_conversation".into()),
            pid: None,
            started_at: None,
            updated_at: None,
            pull_requests: None,
            // Native resume is routed separately; no inline authority is
            // inferred from a workspace-to-ID cache entry.
            capabilities: BTreeSet::new(),
        });
    }
    Ok(sessions)
}

/// Observe-only controller for documented Antigravity cache entries.
pub struct AntigravityController {
    invocation: AntigravityInvocation,
    ownership: Option<Arc<AntigravityOwnership>>,
    cache_path: PathBuf,
    runner: Arc<dyn CommandRunner>,
}

impl AntigravityController {
    pub fn host(executable: impl Into<String>) -> Self {
        Self {
            invocation: AntigravityInvocation::host(executable),
            ownership: None,
            cache_path: default_antigravity_last_conversations_path()
                .unwrap_or_else(|_| PathBuf::from("/nonexistent/last_conversations.json")),
            runner: Arc::new(ProcessRunner),
        }
    }

    pub fn managed(
        executable: impl Into<String>,
        ownership: Arc<AntigravityOwnership>,
    ) -> Result<Self> {
        Ok(Self {
            invocation: AntigravityInvocation::host(executable),
            ownership: Some(ownership),
            cache_path: default_antigravity_last_conversations_path()?,
            runner: Arc::new(ProcessRunner),
        })
    }

    fn available_model_ids(&self) -> Result<Vec<String>> {
        let mut request =
            CommandRequest::new(self.invocation.executable.clone(), vec!["models".into()]);
        request.timeout = Duration::from_secs(20);
        let output = self.runner.run(&request).map_err(|error| {
            anyhow!(
                "Antigravity could not load this account's model catalog: {error:#}. Press l for native setup, or type an exact model ID and press Enter"
            )
        })?;
        if output.status != 0 {
            let detail = output.stderr_lossy();
            if detail.to_ascii_lowercase().contains("auth")
                || detail.to_ascii_lowercase().contains("sign in")
            {
                bail!("Antigravity is not authenticated; press Enter to sign in")
            }
            bail!(
                "Antigravity model discovery exited with status {}: {}",
                output.status,
                detail
            );
        }
        let models = parse_antigravity_models(output.stdout_text()?);
        if models.is_empty() {
            bail!(
                "Antigravity returned no available models. Press l for native setup, or type an exact model ID and press Enter"
            )
        }
        Ok(models)
    }

    fn launch_native(&self, request: &LaunchRequest) -> Result<ControlOutcome> {
        let ownership = self
            .ownership
            .as_ref()
            .context("managed Antigravity launch is not configured")?;
        let before = cached_conversation(&self.cache_path, &request.cwd)?;
        let model = request.model.as_deref().context(
            "Antigravity requires an exact model selection because its CLI can terminate a default-model launch when the account advertises no PlanModel/RequestedModel",
        )?;
        let spec = self
            .invocation
            .sandboxed_launch(&request.cwd, &request.prompt, Some(model))?;
        let launch_key = format!("antigravity:new:{}", now_millis());
        let exit = crate::native_session::run(spec.command(), &launch_key)?;
        let conversation_id =
            wait_for_new_cached_conversation(&self.cache_path, &request.cwd, before.as_deref())?;
        let hint = if let Some(conversation_id) = conversation_id {
            ownership.record(&request.cwd, &conversation_id)?;
            Some(conversation_id)
        } else {
            None
        };
        if matches!(
            &exit,
            crate::native_session::NativeSessionExit::Backgrounded
        ) {
            if let Some(conversation_id) = hint.as_deref() {
                crate::native_session::rename_key(
                    &launch_key,
                    &format!("antigravity:host:{conversation_id}"),
                )?;
            }
        }
        match exit {
            crate::native_session::NativeSessionExit::Backgrounded => Ok(ControlOutcome {
                message: if hint.is_some() {
                    "backgrounded Antigravity session; Enter/Right resumes it".into()
                } else {
                    "backgrounded Antigravity; its conversation ID is not visible yet".into()
                },
                provider_session_hint: hint,
            }),
            crate::native_session::NativeSessionExit::Exited(status) if status.success() => {
                Ok(ControlOutcome {
                    message: "returned from Antigravity".into(),
                    provider_session_hint: hint,
                })
            }
            crate::native_session::NativeSessionExit::Exited(status) => {
                bail!("Antigravity session exited with status {status}")
            }
        }
    }
}

impl ProviderController for AntigravityController {
    fn provider(&self) -> Provider {
        Provider::Antigravity
    }

    fn launch_mode(&self) -> LaunchMode {
        if self.ownership.is_some() {
            LaunchMode::SelectableModel
        } else {
            LaunchMode::Unavailable
        }
    }

    fn launch_presentation(&self) -> LaunchPresentation {
        LaunchPresentation::Foreground
    }

    fn available_models(&self) -> Result<Vec<String>> {
        self.available_model_ids()
    }

    fn supports_authentication(&self) -> bool {
        true
    }

    fn authenticate(&self) -> Result<ControlOutcome> {
        run_native_authentication(&self.invocation.executable, &[], Provider::Antigravity)
    }

    fn launch(&self, request: &LaunchRequest) -> Result<ControlOutcome> {
        self.launch_native(request)
    }

    fn launch_foreground(&self, request: &LaunchRequest) -> Result<ControlOutcome> {
        self.launch_native(request)
    }

    fn open(&self, session: &AgentSession) -> Result<ControlOutcome> {
        if session.provider != Provider::Antigravity || session.runtime != Runtime::Host {
            bail!("the Antigravity host controller cannot open this session");
        }
        if !session.cwd.is_dir() {
            bail!(
                "the cached Antigravity workspace no longer exists: {}",
                session.cwd.display()
            );
        }
        let spec = self
            .invocation
            .resume(&session.provider_session_id, &session.cwd)?;
        match crate::native_session::run(spec.command(), &session.id)? {
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
                bail!("Antigravity conversation exited with status {status}")
            }
        }
    }
}

fn require_conversation_id(conversation_id: &str) -> Result<()> {
    if conversation_id.is_empty()
        || conversation_id.chars().any(char::is_control)
        || conversation_id.chars().any(char::is_whitespace)
    {
        bail!("Antigravity conversation ID is empty or contains whitespace/control characters");
    }
    Ok(())
}

fn require_absolute_workspace(cwd: &Path) -> Result<()> {
    if !cwd.is_absolute() {
        bail!("Antigravity workspace path must be absolute");
    }
    Ok(())
}

fn require_model(model: &str) -> Result<()> {
    if model.is_empty()
        || model.len() > 128
        || model
            .chars()
            .any(|character| character.is_control() || character.is_whitespace())
    {
        bail!("Antigravity model must contain 1 to 128 non-whitespace bytes");
    }
    Ok(())
}

fn parse_antigravity_models(output: &str) -> Vec<String> {
    let rendered = if output.contains('\x1b') {
        let mut parser = vt100::Parser::new(200, 240, 0);
        parser.process(output.as_bytes());
        parser.screen().contents()
    } else {
        output.to_owned()
    };
    let mut models = BTreeSet::new();
    for line in rendered.lines() {
        let line = line.trim().trim_start_matches(|character: char| {
            character.is_whitespace() || matches!(character, '-' | '*' | '•' | '›' | '>' | '✓')
        });
        let Some(candidate) = line.split_whitespace().next() else {
            continue;
        };
        let candidate = candidate.trim_matches(|character: char| matches!(character, ':' | ','));
        let lower = candidate.to_ascii_lowercase();
        if matches!(
            lower.as_str(),
            "fetching" | "loading" | "models" | "model" | "available" | "name" | "id"
        ) {
            continue;
        }
        if !candidate.is_empty()
            && candidate.len() <= 128
            && candidate.chars().all(|character| {
                character.is_ascii_alphanumeric()
                    || matches!(character, '-' | '_' | '.' | ':' | '/' | '@')
            })
        {
            models.insert(candidate.to_owned());
        }
    }
    models.into_iter().collect()
}

fn cached_conversation(path: &Path, cwd: &Path) -> Result<Option<String>> {
    let input = match fs::read_to_string(path) {
        Ok(input) => input,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(None),
        Err(error) => return Err(error.into()),
    };
    let values: BTreeMap<String, String> =
        serde_json::from_str(&input).context("invalid Antigravity conversation cache")?;
    Ok(values.get(&cwd.to_string_lossy().into_owned()).cloned())
}

fn wait_for_new_cached_conversation(
    path: &Path,
    cwd: &Path,
    before: Option<&str>,
) -> Result<Option<String>> {
    let deadline = std::time::Instant::now() + Duration::from_secs(3);
    loop {
        if let Some(current) = cached_conversation(path, cwd)? {
            if before != Some(current.as_str()) {
                return Ok(Some(current));
            }
        }
        if std::time::Instant::now() >= deadline {
            return Ok(None);
        }
        thread::sleep(Duration::from_millis(50));
    }
}

fn now_millis() -> u64 {
    SystemTime::now()
        .duration_since(SystemTime::UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis() as u64
}

fn reject_symlink(path: &Path) -> Result<()> {
    match fs::symlink_metadata(path) {
        Ok(metadata) if metadata.file_type().is_symlink() => {
            bail!(
                "refusing symlinked Antigravity state path {}",
                path.display()
            )
        }
        Ok(_) => Ok(()),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(()),
        Err(error) => Err(error.into()),
    }
}

fn reject_insecure_registry_permissions(path: &Path) -> Result<()> {
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        match fs::metadata(path) {
            Ok(metadata) if metadata.permissions().mode() & 0o077 != 0 => {
                bail!(
                    "Antigravity ownership registry {} must not be accessible by group or other users",
                    path.display()
                )
            }
            Ok(_) => {}
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
            Err(error) => return Err(error.into()),
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use tempfile::tempdir;

    use super::*;

    #[test]
    fn parses_only_the_documented_workspace_cache_shape() {
        let sessions = parse_antigravity_last_conversations(
            r#"{
              "/work/one": "a1b2c3d4-e5f6-7890-abcd-ef1234567890",
              "/work/two": "f9e8d7c6-b5a4-3210-fedc-ba9876543210"
            }"#,
            Runtime::Host,
        )
        .unwrap();
        assert_eq!(sessions.len(), 2);
        assert_eq!(sessions[0].provider, Provider::Antigravity);
        assert_eq!(sessions[0].state, SessionState::Unknown);
        assert!(sessions[0].capabilities.is_empty());
        assert_eq!(sessions[1].cwd, PathBuf::from("/work/two"));
    }

    #[test]
    fn source_filters_by_workspace_prefix_and_tolerates_missing_cache() {
        let directory = tempdir().unwrap();
        let missing = AntigravitySource::host(directory.path().join("missing.json"));
        assert!(missing
            .discover(&DiscoveryRequest::default())
            .unwrap()
            .is_empty());

        let path = directory.path().join("last_conversations.json");
        let workspace = directory.path().join("work/one");
        let other = directory.path().join("other/two");
        fs::create_dir_all(&workspace).unwrap();
        fs::create_dir_all(&other).unwrap();
        fs::write(
            &path,
            serde_json::json!({
                workspace.display().to_string(): "one",
                other.display().to_string(): "two"
            })
            .to_string(),
        )
        .unwrap();
        let sessions = AntigravitySource::host(path)
            .discover(&DiscoveryRequest {
                include_external: true,
                cwd: Some(directory.path().join("work")),
                ..DiscoveryRequest::default()
            })
            .unwrap();
        assert_eq!(sessions.len(), 1);
        assert_eq!(sessions[0].provider_session_id, "one");
    }

    #[test]
    fn source_hides_cached_workspaces_that_no_longer_exist() {
        let directory = tempdir().unwrap();
        let path = directory.path().join("last_conversations.json");
        let stale = directory.path().join("deleted-workspace");
        fs::write(
            &path,
            serde_json::json!({stale.display().to_string(): "stale-id"}).to_string(),
        )
        .unwrap();

        assert!(AntigravitySource::host(path)
            .discover(&DiscoveryRequest::default())
            .unwrap()
            .is_empty());
    }

    #[test]
    fn controller_explains_a_stale_workspace_before_spawning() {
        let directory = tempdir().unwrap();
        let stale = directory.path().join("deleted-workspace");
        let session = parse_antigravity_last_conversations(
            &serde_json::json!({stale.display().to_string(): "stale-id"}).to_string(),
            Runtime::Host,
        )
        .unwrap()
        .remove(0);

        let error = AntigravityController::host("must-not-run")
            .open(&session)
            .unwrap_err()
            .to_string();

        assert!(error.contains("cached Antigravity workspace no longer exists"));
        assert!(error.contains("deleted-workspace"));
    }

    #[test]
    fn builds_shell_free_native_resume_and_never_bypasses_permissions() {
        let invocation = AntigravityInvocation::host("agy");
        let resume = invocation
            .resume("conversation-id", Path::new("/work/repo"))
            .unwrap();
        assert_eq!(
            resume,
            AntigravityCommandSpec {
                program: "agy".into(),
                args: vec!["--conversation".into(), "conversation-id".into()],
                current_dir: "/work/repo".into(),
            }
        );
        let launch = invocation
            .sandboxed_launch(Path::new("/work/repo"), "fix tests", Some("gemini-3-pro"))
            .unwrap();
        assert_eq!(
            launch.args,
            vec![
                "--sandbox",
                "--model",
                "gemini-3-pro",
                "--prompt-interactive",
                "fix tests"
            ]
        );
        assert!(!launch
            .args
            .contains(&"--dangerously-skip-permissions".into()));
    }

    #[test]
    fn rejects_undocumented_or_unsafe_cache_records() {
        assert!(
            parse_antigravity_last_conversations(r#"{"relative":"id"}"#, Runtime::Host).is_err()
        );
        assert!(
            parse_antigravity_last_conversations("{\"/work\":\"bad\\nid\"}", Runtime::Host)
                .is_err()
        );
        assert!(parse_antigravity_last_conversations("[]", Runtime::Host).is_err());
    }

    #[test]
    fn managed_source_shows_only_exact_oav_owned_cached_conversations() {
        let directory = tempdir().unwrap();
        let workspace = directory.path().join("workspace");
        fs::create_dir(&workspace).unwrap();
        let cache = directory.path().join("last_conversations.json");
        fs::write(
            &cache,
            serde_json::json!({workspace.display().to_string(): "owned-conversation"}).to_string(),
        )
        .unwrap();
        let ownership = Arc::new(AntigravityOwnership {
            path: directory.path().join("state/sessions.json"),
            records: Mutex::new(BTreeSet::new()),
        });
        let source = AntigravitySource {
            path: cache,
            ownership: Some(ownership.clone()),
        };
        assert!(source
            .discover(&DiscoveryRequest::default())
            .unwrap()
            .is_empty());

        ownership.record(&workspace, "owned-conversation").unwrap();
        let sessions = source.discover(&DiscoveryRequest::default()).unwrap();
        assert_eq!(sessions.len(), 1);
        assert_eq!(sessions[0].provider_session_id, "owned-conversation");
    }

    #[cfg(unix)]
    #[test]
    fn ownership_registry_rejects_symlinks_and_public_permissions() {
        use std::os::unix::fs::{symlink, PermissionsExt};

        let directory = tempdir().unwrap();
        let target = directory.path().join("target.json");
        fs::write(&target, "[]").unwrap();
        let link = directory.path().join("link.json");
        symlink(&target, &link).unwrap();
        assert!(reject_symlink(&link).is_err());

        fs::set_permissions(&target, fs::Permissions::from_mode(0o644)).unwrap();
        assert!(reject_insecure_registry_permissions(&target).is_err());
        fs::set_permissions(&target, fs::Permissions::from_mode(0o600)).unwrap();
        reject_insecure_registry_permissions(&target).unwrap();
    }

    #[test]
    fn model_catalog_parser_ignores_progress_copy() {
        assert_eq!(
            parse_antigravity_models(
                "Fetching available models...\n  gemini-3-pro  Gemini 3 Pro\n  claude-sonnet-4.6  Sonnet\n"
            ),
            vec!["claude-sonnet-4.6", "gemini-3-pro"]
        );
    }

    #[test]
    fn managed_launch_refuses_to_start_without_an_exact_model() {
        let directory = tempdir().unwrap();
        let ownership = Arc::new(AntigravityOwnership {
            path: directory.path().join("state/sessions.json"),
            records: Mutex::new(BTreeSet::new()),
        });
        let controller = AntigravityController {
            invocation: AntigravityInvocation::host("must-not-run"),
            ownership: Some(ownership),
            cache_path: directory.path().join("last_conversations.json"),
            runner: Arc::new(ProcessRunner),
        };
        let error = controller
            .launch(&LaunchRequest {
                provider: Provider::Antigravity,
                model: None,
                prompt: "do not execute this".into(),
                cwd: directory.path().to_path_buf(),
            })
            .unwrap_err()
            .to_string();
        assert!(error.contains("requires an exact model selection"));
    }
}

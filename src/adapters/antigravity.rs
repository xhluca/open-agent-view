use std::collections::{BTreeMap, BTreeSet};
use std::fs::{self, OpenOptions};
use std::io::{Read, Write};
use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{mpsc, Arc, Mutex};
use std::thread;
use std::time::{Duration, Instant, SystemTime};

use anyhow::{anyhow, bail, Context, Result};
use serde::{Deserialize, Serialize};

use super::{DiscoveryRequest, SessionSource};
use crate::control::{
    run_native_authentication, ControlOutcome, LaunchMode, LaunchPresentation, LaunchRequest,
    ProviderController,
};
use crate::domain::{AgentSession, Provider, Runtime, SessionKind, SessionState};
use crate::process::{CommandRequest, CommandRunner, ProcessRunner};

const MODEL_CACHE_SCHEMA: u32 = 1;
const MODEL_CACHE_FRESH_MS: u64 = 24 * 60 * 60 * 1_000;
const MODEL_CACHE_STALE_MS: u64 = 7 * 24 * 60 * 60 * 1_000;
const CONVERSATION_CORRELATION_TIMEOUT: Duration = Duration::from_secs(30);
const CONVERSATION_CORRELATION_POLL: Duration = Duration::from_millis(50);

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
        // An OAV task must allocate a distinct Antigravity project/session.
        // Without this flag Antigravity may continue the workspace's existing
        // default conversation, leaving the documented last-conversation
        // cache unchanged and making exact post-launch correlation impossible.
        let mut args = vec!["--sandbox".into(), "--new-project".into()];
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
    #[serde(default, skip_serializing_if = "Option::is_none")]
    name: Option<String>,
}

#[derive(Clone, Debug, Eq, PartialEq)]
struct PendingAntigravityConversation {
    launch_key: String,
    workspace: PathBuf,
    name: String,
    created_at_ms: u64,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
struct CachedAntigravityModels {
    schema_version: u32,
    executable: String,
    fetched_at_ms: u64,
    models: Vec<String>,
}

/// Durable ownership boundary for conversations created through OAV.
pub struct AntigravityOwnership {
    path: PathBuf,
    records: Mutex<BTreeSet<OwnedAntigravityConversation>>,
    pending: Mutex<BTreeMap<String, PendingAntigravityConversation>>,
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
            pending: Mutex::new(BTreeMap::new()),
        }))
    }

    #[cfg(test)]
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

    #[cfg(test)]
    fn record(&self, workspace: &Path, conversation_id: &str) -> Result<()> {
        self.record_named(workspace, conversation_id, None)
    }

    fn record_named(
        &self,
        workspace: &Path,
        conversation_id: &str,
        name: Option<&str>,
    ) -> Result<()> {
        let mut records = self
            .records
            .lock()
            .map_err(|_| anyhow!("Antigravity ownership registry lock was poisoned"))?;
        let previous = records
            .iter()
            .find(|record| {
                record.workspace == workspace && record.conversation_id == conversation_id
            })
            .cloned();
        records.retain(|record| {
            record.workspace != workspace || record.conversation_id != conversation_id
        });
        records.insert(OwnedAntigravityConversation {
            workspace: workspace.to_owned(),
            conversation_id: conversation_id.into(),
            created_at_ms: previous
                .as_ref()
                .map(|record| record.created_at_ms)
                .unwrap_or_else(now_millis),
            name: name
                .map(str::to_owned)
                .or_else(|| previous.and_then(|record| record.name)),
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
        crate::fs_util::replace_file(&temporary, &self.path)?;
        Ok(())
    }

    fn records(&self) -> Vec<OwnedAntigravityConversation> {
        self.records
            .lock()
            .map(|records| records.iter().cloned().collect())
            .unwrap_or_default()
    }

    fn begin(&self, pending: PendingAntigravityConversation) -> Result<()> {
        self.pending
            .lock()
            .map_err(|_| anyhow!("Antigravity pending-session lock was poisoned"))?
            .insert(pending.launch_key.clone(), pending);
        Ok(())
    }

    fn complete(&self, launch_key: &str) {
        if let Ok(mut pending) = self.pending.lock() {
            pending.remove(launch_key);
        }
    }

    fn pending(&self) -> Vec<PendingAntigravityConversation> {
        self.pending
            .lock()
            .map(|pending| pending.values().cloned().collect())
            .unwrap_or_default()
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

pub fn default_antigravity_model_cache_path() -> Result<PathBuf> {
    if let Some(cache_home) = std::env::var_os("XDG_CACHE_HOME") {
        return Ok(PathBuf::from(cache_home).join("open-agent-view/antigravity/models.json"));
    }
    let home = std::env::var_os("HOME").context("HOME is not set")?;
    Ok(PathBuf::from(home).join(".cache/open-agent-view/antigravity/models.json"))
}

pub fn default_antigravity_brain_path() -> Result<PathBuf> {
    let home = std::env::var_os("HOME").context("HOME is not set")?;
    Ok(PathBuf::from(home).join(".gemini/antigravity-cli/brain"))
}

pub struct AntigravitySource {
    path: PathBuf,
    brain_path: PathBuf,
    ownership: Option<Arc<AntigravityOwnership>>,
}

impl AntigravitySource {
    pub fn host(path: PathBuf) -> Self {
        Self {
            path,
            brain_path: default_antigravity_brain_path()
                .unwrap_or_else(|_| PathBuf::from("/nonexistent/brain")),
            ownership: None,
        }
    }

    pub fn default_host() -> Result<Self> {
        Ok(Self::host(default_antigravity_last_conversations_path()?))
    }

    pub fn managed(ownership: Arc<AntigravityOwnership>) -> Result<Self> {
        Ok(Self {
            path: default_antigravity_last_conversations_path()?,
            brain_path: default_antigravity_brain_path()?,
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
            Ok(input) => Some(input),
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => None,
            Err(error) => {
                return Err(error).with_context(|| {
                    format!(
                        "failed to read Antigravity conversation cache {}",
                        self.path.display()
                    )
                })
            }
        };
        let cached = input
            .as_deref()
            .map(|input| parse_antigravity_last_conversations(input, Runtime::Host))
            .transpose()?
            .unwrap_or_default();
        let mut sessions = BTreeMap::new();
        if let Some(ownership) = self.ownership.as_ref() {
            for record in ownership.records() {
                let session = owned_antigravity_session(&record, &self.brain_path);
                sessions.insert(session.id.clone(), session);
            }
            for pending in ownership.pending() {
                let session = pending_antigravity_session(&pending);
                sessions.insert(session.id.clone(), session);
            }
        }
        if request.include_external {
            for session in cached {
                sessions.entry(session.id.clone()).or_insert(session);
            }
        }
        Ok(sessions
            .into_values()
            .filter(|session| {
                session.cwd.is_dir()
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

fn owned_antigravity_session(
    record: &OwnedAntigravityConversation,
    brain_path: &Path,
) -> AgentSession {
    let id = format!("antigravity:host:{}", record.conversation_id);
    let backgrounded = crate::native_session::is_backgrounded(&id);
    let transcript = read_antigravity_transcript(brain_path, &record.conversation_id).ok();
    let workspace_name = record
        .workspace
        .file_name()
        .and_then(|name| name.to_str())
        .filter(|name| !name.is_empty())
        .unwrap_or("Antigravity task");
    let mut capabilities = BTreeSet::new();
    if backgrounded {
        capabilities.insert(crate::domain::Capability::Interrupt);
    }
    AgentSession {
        id,
        provider_session_id: record.conversation_id.clone(),
        provider: Provider::Antigravity,
        runtime: Runtime::Host,
        kind: SessionKind::Managed,
        name: record
            .name
            .clone()
            .unwrap_or_else(|| workspace_name.to_owned()),
        cwd: record.workspace.clone(),
        state: if backgrounded {
            SessionState::Working
        } else {
            SessionState::Completed
        },
        summary: transcript
            .as_ref()
            .and_then(|transcript| transcript.summary.clone())
            .unwrap_or_else(|| {
                if backgrounded {
                    "Antigravity native session is backgrounded".into()
                } else {
                    "Antigravity conversation completed".into()
                }
            }),
        raw_state: Some(if backgrounded {
            "native_backgrounded".into()
        } else {
            "owned_conversation".into()
        }),
        pid: None,
        started_at: Some(SystemTime::UNIX_EPOCH + Duration::from_millis(record.created_at_ms)),
        updated_at: transcript.and_then(|transcript| transcript.updated_at),
        pull_requests: None,
        capabilities,
    }
}

fn pending_antigravity_session(pending: &PendingAntigravityConversation) -> AgentSession {
    let backgrounded = crate::native_session::is_backgrounded(&pending.launch_key);
    let mut capabilities = BTreeSet::new();
    if backgrounded {
        capabilities.insert(crate::domain::Capability::Interrupt);
    }
    AgentSession {
        id: pending.launch_key.clone(),
        provider_session_id: pending.launch_key.clone(),
        provider: Provider::Antigravity,
        runtime: Runtime::Host,
        kind: SessionKind::Managed,
        name: pending.name.clone(),
        cwd: pending.workspace.clone(),
        state: if backgrounded {
            SessionState::Working
        } else {
            SessionState::Unknown
        },
        summary: if backgrounded {
            "Antigravity native session is backgrounded".into()
        } else {
            "Antigravity is starting".into()
        },
        raw_state: Some("native_launch_pending".into()),
        pid: None,
        started_at: Some(SystemTime::UNIX_EPOCH + Duration::from_millis(pending.created_at_ms)),
        updated_at: Some(SystemTime::now()),
        pull_requests: None,
        capabilities,
    }
}

struct AntigravityTranscript {
    summary: Option<String>,
    updated_at: Option<SystemTime>,
    user_text: String,
}

fn read_antigravity_transcript(
    brain_path: &Path,
    conversation_id: &str,
) -> Result<AntigravityTranscript> {
    require_conversation_id(conversation_id)?;
    reject_symlink(brain_path)?;
    let conversation_path = brain_path.join(conversation_id);
    let generated_path = conversation_path.join(".system_generated");
    let logs_path = generated_path.join("logs");
    let path = logs_path.join("transcript.jsonl");
    reject_symlink(&conversation_path)?;
    reject_symlink(&generated_path)?;
    reject_symlink(&logs_path)?;
    reject_symlink(&path)?;
    let metadata = fs::metadata(&path)?;
    if !metadata.is_file() || metadata.len() > 8 * 1024 * 1024 {
        bail!("Antigravity transcript is not a bounded regular file");
    }
    let mut bytes = Vec::with_capacity(metadata.len() as usize);
    fs::File::open(&path)?
        .take(8 * 1024 * 1024 + 1)
        .read_to_end(&mut bytes)?;
    if bytes.len() > 8 * 1024 * 1024 {
        bail!("Antigravity transcript exceeded the 8 MiB safety cap");
    }
    let input = String::from_utf8(bytes).context("Antigravity transcript is not UTF-8")?;
    let mut summary = None;
    let mut user_text = String::new();
    for line in input.lines() {
        let Ok(value) = serde_json::from_str::<serde_json::Value>(line) else {
            continue;
        };
        let kind = value.get("type").and_then(serde_json::Value::as_str);
        let Some(content) = value.get("content").and_then(serde_json::Value::as_str) else {
            continue;
        };
        if kind == Some("USER_INPUT") {
            user_text.push_str(content);
            user_text.push('\n');
        } else if kind == Some("PLANNER_RESPONSE") || kind == Some("MODEL") {
            let normalized = compact_antigravity_text(content, 240);
            if !normalized.is_empty() {
                summary = Some(normalized);
            }
        }
    }
    Ok(AntigravityTranscript {
        summary,
        updated_at: metadata.modified().ok(),
        user_text,
    })
}

fn compact_antigravity_text(input: &str, limit: usize) -> String {
    input
        .split_whitespace()
        .collect::<Vec<_>>()
        .join(" ")
        .chars()
        .take(limit)
        .collect()
}

fn antigravity_task_name(prompt: &str) -> String {
    let compact = compact_antigravity_text(prompt, 64);
    if compact.is_empty() {
        "Antigravity task".into()
    } else {
        compact
    }
}

fn antigravity_brain_ids(path: &Path) -> Result<BTreeSet<String>> {
    reject_symlink(path)?;
    let entries = match fs::read_dir(path) {
        Ok(entries) => entries,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(BTreeSet::new()),
        Err(error) => return Err(error.into()),
    };
    let mut ids = BTreeSet::new();
    for entry in entries.take(20_001) {
        let entry = entry?;
        let file_type = entry.file_type()?;
        let Some(id) = entry.file_name().to_str().map(str::to_owned) else {
            continue;
        };
        if file_type.is_dir() && !file_type.is_symlink() && require_conversation_id(&id).is_ok() {
            ids.insert(id);
        }
        if ids.len() > 20_000 {
            bail!("Antigravity brain store exceeded the 20,000-conversation safety cap");
        }
    }
    Ok(ids)
}

/// Observe-only controller for documented Antigravity cache entries.
pub struct AntigravityController {
    invocation: AntigravityInvocation,
    ownership: Option<Arc<AntigravityOwnership>>,
    cache_path: PathBuf,
    brain_path: PathBuf,
    model_cache_path: PathBuf,
    model_cache: Mutex<Option<CachedAntigravityModels>>,
    runner: Arc<dyn CommandRunner>,
}

impl AntigravityController {
    pub fn host(executable: impl Into<String>) -> Self {
        Self {
            invocation: AntigravityInvocation::host(executable),
            ownership: None,
            cache_path: default_antigravity_last_conversations_path()
                .unwrap_or_else(|_| PathBuf::from("/nonexistent/last_conversations.json")),
            brain_path: default_antigravity_brain_path()
                .unwrap_or_else(|_| PathBuf::from("/nonexistent/brain")),
            model_cache_path: default_antigravity_model_cache_path()
                .unwrap_or_else(|_| PathBuf::from("/nonexistent/models.json")),
            model_cache: Mutex::new(None),
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
            brain_path: default_antigravity_brain_path()?,
            model_cache_path: default_antigravity_model_cache_path()?,
            model_cache: Mutex::new(None),
            runner: Arc::new(ProcessRunner),
        })
    }

    fn available_model_ids(&self) -> Result<Vec<String>> {
        let now = now_millis();
        let cached = self.load_cached_models();
        if let Some(cached) = cached
            .as_ref()
            .filter(|cached| now.saturating_sub(cached.fetched_at_ms) <= MODEL_CACHE_FRESH_MS)
        {
            return Ok(cached.models.clone());
        }
        let fetched = self.fetch_model_ids();
        match fetched {
            Ok(models) => {
                let cache = CachedAntigravityModels {
                    schema_version: MODEL_CACHE_SCHEMA,
                    executable: self.invocation.executable.clone(),
                    fetched_at_ms: now_millis(),
                    models: models.clone(),
                };
                self.store_cached_models(cache);
                Ok(models)
            }
            Err(error) => {
                if let Some(cached) = cached.filter(|cached| {
                    now.saturating_sub(cached.fetched_at_ms) <= MODEL_CACHE_STALE_MS
                }) {
                    Ok(cached.models)
                } else {
                    Err(error)
                }
            }
        }
    }

    fn fetch_model_ids(&self) -> Result<Vec<String>> {
        let mut request =
            CommandRequest::new(self.invocation.executable.clone(), vec!["models".into()]);
        request.timeout = Duration::from_secs(10);
        let output = self.runner.run(&request).map_err(|error| {
            let detail = format!("{error:#}");
            if detail.to_ascii_lowercase().contains("timed out") {
                anyhow!(
                    "Antigravity's own `agy models` command timed out. Press Enter/l to open Antigravity and verify that native `/model` lists models, then return and press Ctrl+R to retry"
                )
            } else {
                anyhow!(
                    "Antigravity could not run its `agy models` catalog command: {detail}. Press Enter/l to open native setup, then Ctrl+R to retry"
                )
            }
        })?;
        if output.status != 0 {
            let detail = [
                String::from_utf8_lossy(&output.stdout).trim().to_owned(),
                output.stderr_lossy(),
            ]
            .into_iter()
            .filter(|part| !part.is_empty())
            .collect::<Vec<_>>()
            .join(" · ");
            let lower = detail.to_ascii_lowercase();
            if lower.contains("timed out waiting for available models") {
                bail!(
                    "Antigravity's own `agy models` command timed out. Press Enter/l to open Antigravity and verify that native `/model` lists models, then return and press Ctrl+R to retry"
                )
            }
            if lower.contains("no models available")
                || lower.contains("no model configuration is available")
            {
                bail!(
                    "Antigravity itself reports no models for this account. Press Enter/l to open native setup and verify `/model`, then return and press Ctrl+R to retry"
                )
            }
            if lower.contains("auth") || lower.contains("sign in") || lower.contains("not logged") {
                bail!("Antigravity is not authenticated; press Enter/l to sign in")
            }
            bail!(
                "Antigravity `agy models` exited with status {}: {}. Press Enter/l for native setup, then Ctrl+R to retry",
                output.status,
                detail
            );
        }
        let models = parse_antigravity_models(output.stdout_text()?);
        if models.is_empty() {
            bail!(
                "Antigravity returned no available models. Press Enter/l to open native setup and verify `/model`, then return and press Ctrl+R to retry"
            )
        }
        Ok(models)
    }

    fn load_cached_models(&self) -> Option<CachedAntigravityModels> {
        if let Ok(cache) = self.model_cache.lock() {
            if let Some(cache) = cache.as_ref() {
                return Some(cache.clone());
            }
        }
        if reject_symlink(&self.model_cache_path).is_err()
            || reject_insecure_registry_permissions(&self.model_cache_path).is_err()
        {
            return None;
        }
        let input = fs::read_to_string(&self.model_cache_path).ok()?;
        let cache: CachedAntigravityModels = serde_json::from_str(&input).ok()?;
        if !valid_model_cache(&cache, &self.invocation.executable) {
            return None;
        }
        if let Ok(mut memory) = self.model_cache.lock() {
            *memory = Some(cache.clone());
        }
        Some(cache)
    }

    fn store_cached_models(&self, cache: CachedAntigravityModels) {
        if let Ok(mut memory) = self.model_cache.lock() {
            *memory = Some(cache.clone());
        }
        // Catalog caching is an optimization, not an authority boundary. A
        // protected-path or I/O failure must not hide a live provider result.
        let _ = persist_model_cache(&self.model_cache_path, &cache);
    }

    fn invalidate_model_cache(&self) {
        if let Ok(mut memory) = self.model_cache.lock() {
            *memory = None;
        }
        if reject_symlink(&self.model_cache_path).is_ok() {
            match fs::remove_file(&self.model_cache_path) {
                Ok(()) => {}
                Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
                Err(_) => {}
            }
        }
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
        let launch_key = format!(
            "antigravity:new:{}",
            crate::native_session::new_session_id()?
        );
        let task_name = antigravity_task_name(&request.prompt);
        ownership.begin(PendingAntigravityConversation {
            launch_key: launch_key.clone(),
            workspace: request.cwd.clone(),
            name: task_name.clone(),
            created_at_ms: now_millis(),
        })?;
        let brain_before = antigravity_brain_ids(&self.brain_path).unwrap_or_default();
        let cancelled = Arc::new(AtomicBool::new(false));
        let (conversation_tx, conversation_rx) = mpsc::sync_channel(1);
        let monitor = monitor_new_conversation(AntigravityConversationMonitor {
            path: self.cache_path.clone(),
            brain_path: self.brain_path.clone(),
            brain_before,
            cwd: request.cwd.clone(),
            prompt: request.prompt.clone(),
            task_name,
            launch_key: launch_key.clone(),
            before,
            ownership: ownership.clone(),
            cancelled: cancelled.clone(),
            sender: conversation_tx,
        });
        let exit = crate::native_session::run(spec.command(), &launch_key);
        let exit = match exit {
            Ok(exit) => exit,
            Err(error) => {
                cancelled.store(true, Ordering::Release);
                let _ = monitor.join();
                ownership.complete(&launch_key);
                return Err(error);
            }
        };
        let backgrounded = matches!(
            &exit,
            crate::native_session::NativeSessionExit::Backgrounded
        );
        let hint = if backgrounded {
            // Keep the bounded correlator alive after the native UI has been
            // backgrounded. Antigravity may create its transcript just after
            // the handoff; the provisional row remains usable in the meantime.
            conversation_rx.try_recv().ok().and_then(Result::ok)
        } else {
            let hint = conversation_rx
                .recv_timeout(Duration::from_millis(750))
                .ok()
                .and_then(Result::ok);
            cancelled.store(true, Ordering::Release);
            let _ = monitor.join();
            ownership.complete(&launch_key);
            hint
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
                message: "backgrounded Antigravity session; Enter/Right resumes it".into(),
                provider_session_hint: Some(hint.unwrap_or(launch_key)),
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
        let outcome =
            run_native_authentication(&self.invocation.executable, &[], Provider::Antigravity)?;
        self.invalidate_model_cache();
        Ok(outcome)
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
        if session.raw_state.as_deref() == Some("native_launch_pending") {
            return match crate::native_session::resume(&session.id)? {
                crate::native_session::NativeSessionExit::Backgrounded => Ok(ControlOutcome {
                    message: format!("backgrounded {}; Enter/Right resumes it", session.name),
                    provider_session_hint: Some(session.provider_session_id.clone()),
                }),
                crate::native_session::NativeSessionExit::Exited(status) if status.success() => {
                    if let Some(ownership) = self.ownership.as_ref() {
                        ownership.complete(&session.id);
                    }
                    Ok(ControlOutcome {
                        message: format!("returned from {}", session.name),
                        provider_session_hint: None,
                    })
                }
                crate::native_session::NativeSessionExit::Exited(status) => {
                    if let Some(ownership) = self.ownership.as_ref() {
                        ownership.complete(&session.id);
                    }
                    bail!("Antigravity conversation exited with status {status}")
                }
            };
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

    fn interrupt(&self, session: &AgentSession) -> Result<ControlOutcome> {
        if session.provider != Provider::Antigravity || session.runtime != Runtime::Host {
            bail!("the Antigravity host controller cannot stop this session");
        }
        if !crate::native_session::is_backgrounded(&session.id) {
            bail!("the Antigravity native frontend is not retained by this dashboard");
        }
        crate::native_session::terminate(&session.id)?;
        if session.raw_state.as_deref() == Some("native_launch_pending") {
            if let Some(ownership) = self.ownership.as_ref() {
                ownership.complete(&session.id);
            }
        }
        Ok(ControlOutcome {
            message: format!("stopped native Antigravity session {}", session.name),
            provider_session_hint: Some(session.provider_session_id.clone()),
        })
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

struct AntigravityConversationMonitor {
    path: PathBuf,
    brain_path: PathBuf,
    brain_before: BTreeSet<String>,
    cwd: PathBuf,
    prompt: String,
    task_name: String,
    launch_key: String,
    before: Option<String>,
    ownership: Arc<AntigravityOwnership>,
    cancelled: Arc<AtomicBool>,
    sender: mpsc::SyncSender<Result<String>>,
}

fn monitor_new_conversation(monitor: AntigravityConversationMonitor) -> thread::JoinHandle<()> {
    thread::spawn(move || {
        let AntigravityConversationMonitor {
            path,
            brain_path,
            brain_before,
            cwd,
            prompt,
            task_name,
            launch_key,
            before,
            ownership,
            cancelled,
            sender,
        } = monitor;
        let deadline = Instant::now() + CONVERSATION_CORRELATION_TIMEOUT;
        while !cancelled.load(Ordering::Acquire) && Instant::now() < deadline {
            if let Ok(current_ids) = antigravity_brain_ids(&brain_path) {
                for current in current_ids.difference(&brain_before) {
                    let matches_prompt = read_antigravity_transcript(&brain_path, current)
                        .map(|transcript| transcript.user_text.contains(&prompt))
                        .unwrap_or(false);
                    if matches_prompt {
                        let result = ownership
                            .record_named(&cwd, current, Some(&task_name))
                            .map(|()| current.clone());
                        if result.is_ok() {
                            ownership.complete(&launch_key);
                            let _ = crate::native_session::rename_key(
                                &launch_key,
                                &format!("antigravity:host:{current}"),
                            );
                        }
                        let _ = sender.send(result);
                        return;
                    }
                }
            }
            if let Ok(Some(current)) = cached_conversation(&path, &cwd) {
                if before.as_deref() != Some(current.as_str()) {
                    let result = ownership
                        .record_named(&cwd, &current, Some(&task_name))
                        .map(|()| current);
                    if result.is_ok() {
                        ownership.complete(&launch_key);
                        let _ = crate::native_session::rename_key(
                            &launch_key,
                            &format!("antigravity:host:{}", result.as_ref().unwrap()),
                        );
                    }
                    let _ = sender.send(result);
                    return;
                }
            }
            thread::sleep(CONVERSATION_CORRELATION_POLL);
        }
    })
}

fn valid_model_cache(cache: &CachedAntigravityModels, executable: &str) -> bool {
    cache.schema_version == MODEL_CACHE_SCHEMA
        && cache.executable == executable
        && cache.fetched_at_ms <= now_millis().saturating_add(5 * 60 * 1_000)
        && !cache.models.is_empty()
        && cache.models.len() <= 20_000
        && cache
            .models
            .iter()
            .all(|model| require_model(model).is_ok())
}

fn persist_model_cache(path: &Path, cache: &CachedAntigravityModels) -> Result<()> {
    let parent = path
        .parent()
        .context("Antigravity model cache has no parent")?;
    reject_symlink(parent)?;
    reject_symlink(path)?;
    reject_insecure_registry_permissions(path)?;
    fs::create_dir_all(parent)?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        fs::set_permissions(parent, fs::Permissions::from_mode(0o700))?;
    }
    let temporary = path.with_extension(format!("tmp-{}-{}", std::process::id(), now_millis()));
    reject_symlink(&temporary)?;
    let mut options = OpenOptions::new();
    options.create_new(true).write(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.mode(0o600);
    }
    let mut file = options.open(&temporary)?;
    serde_json::to_writer(&mut file, cache)?;
    file.write_all(b"\n")?;
    file.sync_all()?;
    crate::fs_util::replace_file(&temporary, path)?;
    Ok(())
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

    #[cfg(unix)]
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

    #[cfg(unix)]
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
                "--new-project",
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
            pending: Mutex::new(BTreeMap::new()),
        });
        let source = AntigravitySource {
            path: cache,
            brain_path: directory.path().join("brain"),
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
        assert_eq!(sessions[0].kind, SessionKind::Managed);
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

    #[cfg(unix)]
    #[test]
    fn transcript_reader_rejects_a_symlinked_internal_directory() {
        use std::os::unix::fs::symlink;

        let directory = tempdir().unwrap();
        let brain = directory.path().join("brain");
        let conversation = brain.join("8a217b29-ba04-485c-b7da-47f9414e685b");
        let outside = directory.path().join("outside");
        fs::create_dir_all(&conversation).unwrap();
        fs::create_dir_all(outside.join("logs")).unwrap();
        fs::write(outside.join("logs/transcript.jsonl"), "").unwrap();
        symlink(&outside, conversation.join(".system_generated")).unwrap();

        assert!(
            read_antigravity_transcript(&brain, "8a217b29-ba04-485c-b7da-47f9414e685b").is_err()
        );
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

    #[cfg(unix)]
    #[test]
    fn successful_model_catalog_is_private_cached_and_survives_a_transient_error() {
        use std::os::unix::fs::PermissionsExt;

        let directory = tempdir().unwrap();
        let executable = directory.path().join("agy");
        let calls = directory.path().join("calls");
        let cache = directory.path().join("cache/models.json");
        fs::write(
            &executable,
            format!(
                "#!/bin/sh\nprintf call >> '{}'\nprintf '%s\\n' 'gemini-3.7-flash-high Gemini' 'claude-sonnet-4-6 Claude'\n",
                calls.display()
            ),
        )
        .unwrap();
        fs::set_permissions(&executable, fs::Permissions::from_mode(0o700)).unwrap();
        let controller = AntigravityController {
            invocation: AntigravityInvocation::host(executable.display().to_string()),
            ownership: None,
            cache_path: directory.path().join("last.json"),
            brain_path: directory.path().join("brain"),
            model_cache_path: cache.clone(),
            model_cache: Mutex::new(None),
            runner: Arc::new(ProcessRunner),
        };

        assert_eq!(
            controller.available_models().unwrap(),
            vec!["claude-sonnet-4-6", "gemini-3.7-flash-high"]
        );
        assert_eq!(controller.available_models().unwrap().len(), 2);
        assert_eq!(fs::read_to_string(&calls).unwrap(), "call");
        assert_eq!(
            fs::metadata(&cache).unwrap().permissions().mode() & 0o777,
            0o600
        );

        let mut persisted: CachedAntigravityModels =
            serde_json::from_slice(&fs::read(&cache).unwrap()).unwrap();
        persisted.fetched_at_ms = now_millis() - MODEL_CACHE_FRESH_MS - 1;
        fs::write(&cache, serde_json::to_vec(&persisted).unwrap()).unwrap();
        fs::set_permissions(&cache, fs::Permissions::from_mode(0o600)).unwrap();
        fs::write(
            &executable,
            "#!/bin/sh\nprintf transient-error >&2\nexit 1\n",
        )
        .unwrap();
        fs::set_permissions(&executable, fs::Permissions::from_mode(0o700)).unwrap();
        let restarted = AntigravityController {
            invocation: AntigravityInvocation::host(executable.display().to_string()),
            ownership: None,
            cache_path: directory.path().join("last.json"),
            brain_path: directory.path().join("brain"),
            model_cache_path: cache,
            model_cache: Mutex::new(None),
            runner: Arc::new(ProcessRunner),
        };

        assert_eq!(
            restarted.available_models().unwrap(),
            vec!["claude-sonnet-4-6", "gemini-3.7-flash-high"]
        );
    }

    #[test]
    fn launch_monitor_records_the_exact_changed_conversation_without_post_return_waiting() {
        let directory = tempdir().unwrap();
        let workspace = directory.path().join("workspace");
        fs::create_dir(&workspace).unwrap();
        let cache = directory.path().join("last_conversations.json");
        fs::write(
            &cache,
            serde_json::json!({workspace.display().to_string(): "before"}).to_string(),
        )
        .unwrap();
        let ownership = Arc::new(AntigravityOwnership {
            path: directory.path().join("state/sessions.json"),
            records: Mutex::new(BTreeSet::new()),
            pending: Mutex::new(BTreeMap::new()),
        });
        let cancelled = Arc::new(AtomicBool::new(false));
        let (sender, receiver) = mpsc::sync_channel(1);
        let monitor = monitor_new_conversation(AntigravityConversationMonitor {
            path: cache.clone(),
            brain_path: directory.path().join("brain"),
            brain_before: BTreeSet::new(),
            cwd: workspace.clone(),
            prompt: "prompt".into(),
            task_name: "task".into(),
            launch_key: "antigravity:new:test".into(),
            before: Some("before".into()),
            ownership: ownership.clone(),
            cancelled,
            sender,
        });

        fs::write(
            &cache,
            serde_json::json!({workspace.display().to_string(): "after"}).to_string(),
        )
        .unwrap();
        let correlated = receiver
            .recv_timeout(Duration::from_secs(2))
            .unwrap()
            .unwrap();
        monitor.join().unwrap();

        assert_eq!(correlated, "after");
        assert!(ownership.owns(&workspace, "after"));
    }

    #[test]
    fn launch_monitor_correlates_live_brain_transcript_before_workspace_cache_updates() {
        let directory = tempdir().unwrap();
        let workspace = directory.path().join("workspace");
        let brain = directory.path().join("brain");
        fs::create_dir(&workspace).unwrap();
        fs::create_dir(&brain).unwrap();
        let ownership = Arc::new(AntigravityOwnership {
            path: directory.path().join("state/sessions.json"),
            records: Mutex::new(BTreeSet::new()),
            pending: Mutex::new(BTreeMap::new()),
        });
        let launch_key = "antigravity:new:test-live";
        ownership
            .begin(PendingAntigravityConversation {
                launch_key: launch_key.into(),
                workspace: workspace.clone(),
                name: "live task".into(),
                created_at_ms: now_millis(),
            })
            .unwrap();
        let cancelled = Arc::new(AtomicBool::new(false));
        let (sender, receiver) = mpsc::sync_channel(1);
        let monitor = monitor_new_conversation(AntigravityConversationMonitor {
            path: directory.path().join("missing-last-conversations.json"),
            brain_path: brain.clone(),
            brain_before: BTreeSet::new(),
            cwd: workspace.clone(),
            prompt: "exact live prompt".into(),
            task_name: "live task".into(),
            launch_key: launch_key.into(),
            before: None,
            ownership: ownership.clone(),
            cancelled,
            sender,
        });

        let conversation_id = "8a217b29-ba04-485c-b7da-47f9414e685b";
        let transcript = brain
            .join(conversation_id)
            .join(".system_generated/logs/transcript.jsonl");
        fs::create_dir_all(transcript.parent().unwrap()).unwrap();
        fs::write(
            &transcript,
            concat!(
                "{\"type\":\"USER_INPUT\",\"content\":\"<USER_REQUEST>\\nexact live prompt\\n</USER_REQUEST>\"}\n",
                "{\"type\":\"PLANNER_RESPONSE\",\"content\":\"the latest answer\"}\n"
            ),
        )
        .unwrap();

        let correlated = receiver
            .recv_timeout(Duration::from_secs(2))
            .unwrap()
            .unwrap();
        monitor.join().unwrap();
        assert_eq!(correlated, conversation_id);
        assert!(ownership.pending().is_empty());

        let source = AntigravitySource {
            path: directory.path().join("missing-last-conversations.json"),
            brain_path: brain,
            ownership: Some(ownership),
        };
        let sessions = source.discover(&DiscoveryRequest::default()).unwrap();
        assert_eq!(sessions.len(), 1);
        assert_eq!(sessions[0].provider_session_id, conversation_id);
        assert_eq!(sessions[0].name, "live task");
        assert_eq!(sessions[0].summary, "the latest answer");
        assert_eq!(sessions[0].state, SessionState::Completed);
        assert!(sessions[0].updated_at.is_some());
    }

    #[test]
    fn pending_launch_is_visible_without_any_provider_cache_entry() {
        let directory = tempdir().unwrap();
        let workspace = directory.path().join("workspace");
        fs::create_dir(&workspace).unwrap();
        let ownership = Arc::new(AntigravityOwnership {
            path: directory.path().join("state/sessions.json"),
            records: Mutex::new(BTreeSet::new()),
            pending: Mutex::new(BTreeMap::new()),
        });
        ownership
            .begin(PendingAntigravityConversation {
                launch_key: "antigravity:new:pending".into(),
                workspace,
                name: "pending task".into(),
                created_at_ms: now_millis(),
            })
            .unwrap();
        let source = AntigravitySource {
            path: directory.path().join("missing-cache.json"),
            brain_path: directory.path().join("brain"),
            ownership: Some(ownership),
        };

        let sessions = source.discover(&DiscoveryRequest::default()).unwrap();
        assert_eq!(sessions.len(), 1);
        assert_eq!(sessions[0].id, "antigravity:new:pending");
        assert_eq!(
            sessions[0].raw_state.as_deref(),
            Some("native_launch_pending")
        );
    }

    #[test]
    fn managed_launch_refuses_to_start_without_an_exact_model() {
        let directory = tempdir().unwrap();
        let ownership = Arc::new(AntigravityOwnership {
            path: directory.path().join("state/sessions.json"),
            records: Mutex::new(BTreeSet::new()),
            pending: Mutex::new(BTreeMap::new()),
        });
        let controller = AntigravityController {
            invocation: AntigravityInvocation::host("must-not-run"),
            ownership: Some(ownership),
            cache_path: directory.path().join("last_conversations.json"),
            brain_path: directory.path().join("brain"),
            model_cache_path: directory.path().join("models.json"),
            model_cache: Mutex::new(None),
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

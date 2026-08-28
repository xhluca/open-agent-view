use std::collections::{BTreeMap, BTreeSet};
use std::fs::{self, File, OpenOptions};
use std::io::Write;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use anyhow::{anyhow, bail, Context, Result};
use serde::{Deserialize, Serialize};
use serde_json::Value;

use super::copilot::{
    parse_copilot_updated_at, CopilotAcpConnection, CopilotAcpMessage, CopilotAcpMode,
    CopilotPermissionRequest, CopilotSessionInfo,
};
use super::{DiscoveryRequest, SessionSource, SourceDiscovery};
use crate::domain::{
    AgentSession, Capability, Provider, Runtime, SessionKind, SessionSnapshot, SessionState,
};

const RESPONSE_TIMEOUT: Duration = Duration::from_secs(8);
const MAX_TRANSCRIPT_CHARS: usize = 32 * 1024;
const REGISTRY_VERSION: u32 = 1;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum MessageRole {
    User,
    Assistant,
}

#[derive(Clone, Debug, PartialEq)]
struct ManagedCopilotSession {
    session_id: String,
    cwd: PathBuf,
    name: String,
    state: SessionState,
    summary: String,
    summary_from_message: bool,
    transcript: String,
    last_message_role: Option<MessageRole>,
    last_message: String,
    active_prompt: Option<u64>,
    permission: Option<CopilotPermissionRequest>,
    /// True only while this supervisor's exact ACP connection owns the
    /// session. Native resume requires releasing that connection first.
    connection_owned: bool,
    started_at: SystemTime,
    updated_at: SystemTime,
    /// True for a session created by OAV. A legacy locally-named session is
    /// retained in the dashboard after upgrade but does not silently inherit
    /// provider control authority.
    managed_authority: bool,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
struct CopilotRegistryRecord {
    session_id: String,
    cwd: PathBuf,
    name: String,
    state: SessionState,
    summary: String,
    #[serde(default)]
    summary_from_message: bool,
    started_at_ms: u64,
    updated_at_ms: u64,
    #[serde(default = "default_true")]
    managed_authority: bool,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
struct CopilotRegistry {
    version: u32,
    sessions: BTreeMap<String, CopilotRegistryRecord>,
}

impl Default for CopilotRegistry {
    fn default() -> Self {
        Self {
            version: REGISTRY_VERSION,
            sessions: BTreeMap::new(),
        }
    }
}

struct CopilotManagedState {
    connection: Option<CopilotAcpConnection>,
    sessions: BTreeMap<String, ManagedCopilotSession>,
}

/// Owns Copilot sessions created or explicitly loaded on one retained ACP
/// connection. Persisted sessions merely returned by another connection's
/// `session/list` never inherit this authority.
pub struct CopilotSupervisor {
    executable: String,
    registry_path: PathBuf,
    lock_path: PathBuf,
    state: Mutex<CopilotManagedState>,
}

/// Background discovery for OAV-owned Copilot sessions. It reconciles the
/// durable registry with ACP's persisted-session metadata and replays history
/// on a short-lived, non-prompting connection so the dashboard can display the
/// actual latest message after a restart.
pub struct CopilotOwnedSource {
    supervisor: Arc<CopilotSupervisor>,
    legacy_visible_ids: BTreeSet<String>,
}

impl CopilotOwnedSource {
    pub fn new(
        supervisor: Arc<CopilotSupervisor>,
        normalized_visible_ids: impl IntoIterator<Item = String>,
    ) -> Self {
        let prefix = "github_copilot:host:";
        let legacy_visible_ids = normalized_visible_ids
            .into_iter()
            .filter_map(|id| id.strip_prefix(prefix).map(str::to_owned))
            .collect();
        Self {
            supervisor,
            legacy_visible_ids,
        }
    }
}

impl SessionSource for CopilotOwnedSource {
    fn label(&self) -> &str {
        "GitHub Copilot (OAV-owned)"
    }

    fn discover(&self, request: &DiscoveryRequest) -> Result<Vec<AgentSession>> {
        Ok(self.discover_with_warnings(request)?.sessions)
    }

    fn discover_with_warnings(&self, _: &DiscoveryRequest) -> Result<SourceDiscovery> {
        let warnings = self
            .supervisor
            .refresh_persisted_sessions(&self.legacy_visible_ids)?;
        let state = self.supervisor.lock_state()?;
        Ok(SourceDiscovery {
            sessions: state.sessions.values().map(normalize_managed).collect(),
            warnings,
        })
    }
}

impl CopilotSupervisor {
    pub fn host(executable: impl Into<String>) -> Result<Self> {
        Self::with_state_dir(executable, default_copilot_state_dir()?)
    }

    pub fn with_state_dir(executable: impl Into<String>, state_dir: PathBuf) -> Result<Self> {
        ensure_private_directory(&state_dir)?;
        let registry_path = state_dir.join("sessions.json");
        let lock_path = state_dir.join("sessions.lock");
        let registry =
            with_locked_registry(&lock_path, &registry_path, |registry| Ok(registry.clone()))?;
        let sessions = registry
            .sessions
            .into_values()
            .map(managed_from_record)
            .map(|session| (session.session_id.clone(), session))
            .collect();
        Ok(Self {
            executable: executable.into(),
            registry_path,
            lock_path,
            state: Mutex::new(CopilotManagedState {
                connection: None,
                sessions,
            }),
        })
    }

    pub fn executable(&self) -> &str {
        &self.executable
    }

    fn refresh_persisted_sessions(
        &self,
        legacy_visible_ids: &BTreeSet<String>,
    ) -> Result<Vec<String>> {
        let persisted = with_locked_registry(&self.lock_path, &self.registry_path, |registry| {
            Ok(registry.clone())
        })?;
        {
            let mut state = self.lock_state()?;
            for record in persisted.sessions.into_values() {
                state
                    .sessions
                    .entry(record.session_id.clone())
                    .or_insert_with(|| managed_from_record(record));
            }
            if state.sessions.is_empty() && legacy_visible_ids.is_empty() {
                return Ok(Vec::new());
            }
        }

        let mut warnings = Vec::new();
        let mut connection =
            match CopilotAcpConnection::connect(&self.executable, CopilotAcpMode::Discovery) {
                Ok(connection) => connection,
                Err(error) => {
                    warnings.push(format!(
                        "could not refresh persisted Copilot text: {error:#}"
                    ));
                    return Ok(warnings);
                }
            };
        let provider_sessions = match connection.list_sessions() {
            Ok(sessions) => sessions,
            Err(error) => {
                warnings.push(format!(
                    "could not list persisted Copilot sessions: {error:#}"
                ));
                return Ok(warnings);
            }
        };
        let by_id = provider_sessions
            .into_iter()
            .map(|session| (session.session_id.clone(), session))
            .collect::<BTreeMap<_, _>>();

        let mut history_targets = Vec::new();
        {
            let mut state = self.lock_state()?;
            for session_id in legacy_visible_ids {
                if state.sessions.contains_key(session_id) {
                    continue;
                }
                let Some(provider) = by_id.get(session_id) else {
                    continue;
                };
                state
                    .sessions
                    .insert(session_id.clone(), managed_from_provider(provider, false));
            }
            for (session_id, managed) in &mut state.sessions {
                let Some(provider) = by_id.get(session_id) else {
                    continue;
                };
                if provider.cwd != managed.cwd {
                    warnings.push(format!(
                        "refused to refresh Copilot session {session_id}: provider workspace changed"
                    ));
                    continue;
                }
                if let Some(title) = provider
                    .title
                    .as_deref()
                    .map(str::trim)
                    .filter(|title| !title.is_empty())
                {
                    managed.name = compact_message_summary(title, 512);
                }
                let provider_updated = provider
                    .updated_at
                    .as_deref()
                    .and_then(parse_copilot_updated_at);
                let provider_has_newer_text = provider_updated
                    .map(|updated| updated > managed.updated_at)
                    .unwrap_or(false);
                let key = format!("github_copilot:host:{session_id}");
                if !managed.connection_owned
                    && !crate::native_session::is_backgrounded(&key)
                    && (!managed.summary_from_message || provider_has_newer_text)
                {
                    history_targets.push((session_id.clone(), managed.cwd.clone()));
                }
                if let Some(updated) = provider_updated {
                    managed.updated_at = updated;
                }
            }
        }

        for (session_id, cwd) in history_targets {
            match connection.load_session_history(&session_id, &cwd) {
                Ok(updates) => {
                    let provider_updated = by_id
                        .get(&session_id)
                        .and_then(|provider| provider.updated_at.as_deref())
                        .and_then(parse_copilot_updated_at);
                    let mut state = self.lock_state()?;
                    let Some(managed) = state.sessions.get_mut(&session_id) else {
                        continue;
                    };
                    managed.transcript.clear();
                    managed.last_message.clear();
                    managed.last_message_role = None;
                    managed.summary_from_message = false;
                    for update in updates {
                        apply_session_update(managed, &update);
                    }
                    if let Some(updated) = provider_updated {
                        managed.updated_at = updated;
                    }
                    if managed.state != SessionState::Unknown {
                        managed.state = SessionState::Completed;
                    }
                }
                Err(error) => warnings.push(format!(
                    "could not replay Copilot session {}: {error:#}",
                    short_session_id(&session_id)
                )),
            }
        }
        let state = self.lock_state()?;
        self.persist_locked(&state)?;
        Ok(warnings)
    }

    /// Create a session and begin its first prompt without enabling broad
    /// permission flags. The ACP process remains owned by this supervisor.
    pub fn launch(&self, prompt: &str, cwd: &Path) -> Result<String> {
        self.launch_with_model(prompt, cwd, None)
    }

    pub fn launch_with_model(
        &self,
        prompt: &str,
        cwd: &Path,
        model: Option<&str>,
    ) -> Result<String> {
        require_absolute_cwd(cwd)?;
        require_prompt(prompt)?;
        let mut state = self.lock_state()?;
        state.ensure_connection(&self.executable)?;
        let request_id = state
            .connection_mut()?
            .begin_new_session(cwd)
            .context("failed to request a new Copilot ACP session")?;
        let response = state
            .connection_mut()?
            .wait_for_response(request_id, RESPONSE_TIMEOUT)
            .map_err(|error| actionable_auth_error(error, &self.executable))?;
        let session_id = response
            .get("sessionId")
            .and_then(Value::as_str)
            .filter(|id| !id.is_empty())
            .context("Copilot ACP session/new omitted sessionId")?
            .to_owned();
        if let Some(model) = model {
            let available = copilot_model_options(&response)?;
            if !available.iter().any(|candidate| candidate == model) {
                bail!("Copilot model `{model}` is not available to the authenticated account");
            }
            let request_id =
                state
                    .connection_mut()?
                    .begin_set_config_option(&session_id, "model", model)?;
            state
                .connection_mut()?
                .wait_for_response(request_id, RESPONSE_TIMEOUT)
                .context("Copilot ACP rejected the selected model")?;
        }
        if state.sessions.contains_key(&session_id) {
            bail!("Copilot ACP reused an already-owned session ID");
        }
        let now = SystemTime::now();
        state.sessions.insert(
            session_id.clone(),
            ManagedCopilotSession {
                session_id: session_id.clone(),
                cwd: cwd.to_owned(),
                name: prompt_name(prompt),
                state: SessionState::Working,
                summary: "Copilot is working".into(),
                summary_from_message: false,
                transcript: String::new(),
                last_message_role: None,
                last_message: String::new(),
                active_prompt: None,
                permission: None,
                connection_owned: true,
                started_at: now,
                updated_at: now,
                managed_authority: true,
            },
        );
        let prompt_id = state
            .connection_mut()?
            .begin_prompt(&session_id, prompt.trim())?;
        state
            .sessions
            .get_mut(&session_id)
            .expect("session inserted")
            .active_prompt = Some(prompt_id);
        self.persist_locked(&state)?;
        Ok(session_id)
    }

    /// Reserve an exact OAV-owned ID for a provider-native foreground launch.
    /// No ACP process is started: the native Copilot UI is the sole live owner.
    pub fn reserve_native(&self, prompt: &str, cwd: &Path) -> Result<String> {
        require_absolute_cwd(cwd)?;
        require_prompt(prompt)?;
        let mut state = self.lock_state()?;
        let session_id = loop {
            let candidate = crate::native_session::new_session_id()?;
            if !state.sessions.contains_key(&candidate) {
                break candidate;
            }
        };
        let now = SystemTime::now();
        state.sessions.insert(
            session_id.clone(),
            ManagedCopilotSession {
                session_id: session_id.clone(),
                cwd: cwd.to_owned(),
                name: prompt_name(prompt),
                state: SessionState::Working,
                summary: "Copilot native session is running".into(),
                summary_from_message: false,
                transcript: String::new(),
                last_message_role: None,
                last_message: String::new(),
                active_prompt: None,
                permission: None,
                connection_owned: false,
                started_at: now,
                updated_at: now,
                managed_authority: true,
            },
        );
        self.persist_locked(&state)?;
        Ok(session_id)
    }

    pub fn mark_native_backgrounded(&self, session_id: &str) -> Result<()> {
        self.update_native_state(
            session_id,
            SessionState::Working,
            "Copilot native session is backgrounded",
        )
    }

    pub fn mark_native_exited(&self, session_id: &str, success: bool) -> Result<()> {
        self.update_native_state(
            session_id,
            if success {
                SessionState::Completed
            } else {
                SessionState::NeedsInput
            },
            if success {
                "Returned from Copilot native session"
            } else {
                "Copilot native session exited with an error"
            },
        )
    }

    pub fn discard_native_reservation(&self, session_id: &str) -> Result<()> {
        let mut state = self.lock_state()?;
        if state
            .sessions
            .get(session_id)
            .is_some_and(|session| !session.connection_owned)
        {
            state.sessions.remove(session_id);
            self.remove_persisted(session_id)?;
        }
        Ok(())
    }

    fn update_native_state(
        &self,
        session_id: &str,
        state_value: SessionState,
        summary: &str,
    ) -> Result<()> {
        let mut state = self.lock_state()?;
        let managed = state
            .sessions
            .get_mut(session_id)
            .context("Copilot native session reservation is no longer present")?;
        if managed.connection_owned {
            bail!("refusing to overwrite a connection-owned Copilot session");
        }
        managed.state = state_value;
        if !managed.summary_from_message {
            managed.summary = summary.into();
            managed.updated_at = SystemTime::now();
        }
        self.persist_locked(&state)?;
        Ok(())
    }

    /// Explicitly load one persisted session onto this exact connection. This
    /// is the only way an external Copilot record can become connection-owned.
    pub fn load(&self, session: &AgentSession) -> Result<()> {
        require_copilot_host(session)?;
        let mut state = self.lock_state()?;
        state.ensure_connection(&self.executable)?;
        if state
            .sessions
            .get(&session.provider_session_id)
            .map(|managed| managed.connection_owned)
            .unwrap_or(false)
        {
            return Ok(());
        }
        let cwd = state
            .sessions
            .get(&session.provider_session_id)
            .map(|managed| managed.cwd.clone())
            .unwrap_or_else(|| session.cwd.clone());
        let request_id = state
            .connection_mut()?
            .begin_load_session(&session.provider_session_id, &cwd)?;
        state
            .connection_mut()?
            .wait_for_response(request_id, RESPONSE_TIMEOUT)?;
        let now = SystemTime::now();
        if let Some(managed) = state.sessions.get_mut(&session.provider_session_id) {
            managed.connection_owned = true;
            managed.state = SessionState::Completed;
            managed.summary = "Loading Copilot conversation…".into();
            managed.summary_from_message = false;
            managed.transcript.clear();
            managed.last_message_role = None;
            managed.last_message.clear();
            managed.updated_at = now;
        } else {
            state.sessions.insert(
                session.provider_session_id.clone(),
                ManagedCopilotSession {
                    session_id: session.provider_session_id.clone(),
                    cwd,
                    name: session.name.clone(),
                    state: SessionState::NeedsInput,
                    summary: "Loaded on Open Agent View's Copilot ACP connection".into(),
                    summary_from_message: false,
                    transcript: String::new(),
                    last_message_role: None,
                    last_message: String::new(),
                    active_prompt: None,
                    permission: None,
                    connection_owned: true,
                    started_at: session.started_at.unwrap_or(now),
                    updated_at: now,
                    managed_authority: false,
                },
            );
        }
        state.drain()?;
        self.persist_locked(&state)?;
        Ok(())
    }

    /// Release an idle managed session from the retained ACP connection so a
    /// provider-native frontend can resume it without two concurrent owners.
    pub fn release_for_native(&self, session: &AgentSession) -> Result<()> {
        let mut state = self.lock_state()?;
        state.drain()?;
        {
            let managed = state.require_owned(session)?;
            if managed.active_prompt.is_some() || managed.permission.is_some() {
                bail!(
                    "finish, decline, or cancel the active Copilot request before opening it natively"
                );
            }
        }
        let supports_close = state.connection_mut()?.capabilities().close_session;
        if supports_close {
            let request_id = state
                .connection_mut()?
                .begin_close_session(&session.provider_session_id)?;
            state
                .connection_mut()?
                .wait_for_response(request_id, RESPONSE_TIMEOUT)
                .context("Copilot ACP could not release the session for native resume")?;
        } else {
            let another_active = state.sessions.values().any(|candidate| {
                candidate.session_id != session.provider_session_id
                    && candidate.connection_owned
                    && (candidate.active_prompt.is_some() || candidate.permission.is_some())
            });
            if another_active {
                bail!(
                    "this Copilot ACP build cannot release one session independently; finish or cancel the other active OAV-controlled Copilot sessions first"
                );
            }
            // `session/close` is optional in ACP. Closing the sole owning ACP
            // process releases all of its idle sessions without racing another
            // OAV task; the selected native CLI then becomes the only frontend.
            state.connection.take();
            let now = SystemTime::now();
            for candidate in state
                .sessions
                .values_mut()
                .filter(|candidate| candidate.connection_owned)
            {
                candidate.connection_owned = false;
                candidate.active_prompt = None;
                candidate.permission = None;
                candidate.state = SessionState::Completed;
                candidate.updated_at = now;
            }
        }
        let managed = state
            .sessions
            .get_mut(&session.provider_session_id)
            .expect("ownership checked");
        managed.connection_owned = false;
        managed.active_prompt = None;
        managed.permission = None;
        managed.state = SessionState::Completed;
        if !managed.summary_from_message {
            managed.updated_at = SystemTime::now();
        }
        self.persist_locked(&state)?;
        Ok(())
    }

    pub fn enrich(&self, snapshot: &mut SessionSnapshot) {
        let result = (|| -> Result<()> {
            let mut state = self.lock_state()?;
            state.drain()?;
            for managed in state.sessions.values_mut().filter(|managed| {
                !managed.connection_owned
                    && managed.state == SessionState::Working
                    && managed.summary == "Copilot native session is backgrounded"
            }) {
                let key = format!("github_copilot:host:{}", managed.session_id);
                if !crate::native_session::is_backgrounded(&key) {
                    managed.state = SessionState::Completed;
                    if !managed.summary_from_message {
                        managed.summary = "Copilot native session exited".into();
                    }
                }
            }
            for session in snapshot.sessions.iter_mut().filter(|session| {
                session.provider == Provider::GitHubCopilot
                    && session.runtime == Runtime::Host
                    && !state.sessions.contains_key(&session.provider_session_id)
            }) {
                session.capabilities.clear();
            }
            for managed in state.sessions.values() {
                let normalized = normalize_managed(managed);
                if let Some(session) = snapshot.sessions.iter_mut().find(|session| {
                    session.provider == Provider::GitHubCopilot
                        && session.runtime == Runtime::Host
                        && session.provider_session_id == managed.session_id
                }) {
                    *session = normalized;
                } else {
                    snapshot.sessions.push(normalized);
                }
            }
            self.persist_locked(&state)?;
            Ok(())
        })();
        if let Err(error) = result {
            snapshot
                .warnings
                .push(format!("GitHub Copilot managed ACP: {error:#}"));
        }
    }

    pub fn inspect(&self, session: &AgentSession) -> Result<String> {
        let mut state = self.lock_state()?;
        state.drain()?;
        let managed = state.require_tracked(session)?;
        if managed.transcript.trim().is_empty() {
            Ok(managed.summary.clone())
        } else {
            Ok(managed.transcript.clone())
        }
    }

    pub fn reply(&self, session: &AgentSession, prompt: &str) -> Result<()> {
        require_prompt(prompt)?;
        let mut state = self.lock_state()?;
        state.drain()?;
        {
            let managed = state.require_owned(session)?;
            if managed.active_prompt.is_some() || managed.permission.is_some() {
                bail!("Copilot session is already working or awaiting permission");
            }
        }
        let request_id = state
            .connection_mut()?
            .begin_prompt(&session.provider_session_id, prompt.trim())?;
        let managed = state
            .sessions
            .get_mut(&session.provider_session_id)
            .expect("ownership checked");
        managed.active_prompt = Some(request_id);
        managed.state = SessionState::Working;
        managed.summary = "Copilot is working".into();
        managed.summary_from_message = false;
        managed.last_message_role = None;
        managed.last_message.clear();
        managed.updated_at = SystemTime::now();
        self.persist_locked(&state)?;
        Ok(())
    }

    pub fn interrupt(&self, session: &AgentSession) -> Result<()> {
        let mut state = self.lock_state()?;
        state.drain()?;
        if state.require_owned(session)?.active_prompt.is_none() {
            bail!("Copilot session has no active prompt to cancel");
        }
        state
            .connection_mut()?
            .cancel_session(&session.provider_session_id)?;
        let managed = state
            .sessions
            .get_mut(&session.provider_session_id)
            .expect("ownership checked");
        managed.active_prompt = None;
        managed.permission = None;
        managed.state = SessionState::NeedsInput;
        managed.summary = "Copilot prompt cancelled".into();
        managed.summary_from_message = false;
        managed.updated_at = SystemTime::now();
        self.persist_locked(&state)?;
        Ok(())
    }

    pub fn resolve_approval(&self, session: &AgentSession, accept: bool) -> Result<()> {
        let mut state = self.lock_state()?;
        state.drain()?;
        let (request_id, option_id) = {
            let permission = state
                .require_owned(session)?
                .permission
                .as_ref()
                .context("Copilot session has no pending permission request")?;
            let required_kind = if accept { "allow_once" } else { "reject_once" };
            let option = permission
                .options
                .iter()
                .find(|option| option.kind == required_kind)
                .with_context(|| {
                    format!("Copilot did not offer a `{required_kind}` permission option")
                })?;
            (permission.request_id.clone(), option.id.clone())
        };
        state
            .connection_mut()?
            .respond_permission_selected(&request_id, &option_id)?;
        let managed = state
            .sessions
            .get_mut(&session.provider_session_id)
            .expect("ownership checked");
        managed.permission = None;
        managed.state = SessionState::Working;
        managed.summary = if accept {
            "Allowed the requested Copilot action once".into()
        } else {
            "Rejected the requested Copilot action".into()
        };
        managed.summary_from_message = false;
        managed.updated_at = SystemTime::now();
        self.persist_locked(&state)?;
        Ok(())
    }

    pub fn owns(&self, session: &AgentSession) -> bool {
        self.state
            .lock()
            .map(|state| {
                session.provider == Provider::GitHubCopilot
                    && session.runtime == Runtime::Host
                    && state
                        .sessions
                        .get(&session.provider_session_id)
                        .map(|managed| managed.connection_owned && managed.cwd == session.cwd)
                        .unwrap_or(false)
            })
            .unwrap_or(false)
    }

    /// Whether this is an OAV-created/loaded record, including while its ACP
    /// ownership is temporarily released to a native frontend.
    pub fn tracks(&self, session: &AgentSession) -> bool {
        self.state
            .lock()
            .map(|state| {
                session.provider == Provider::GitHubCopilot
                    && session.runtime == Runtime::Host
                    && state
                        .sessions
                        .get(&session.provider_session_id)
                        .map(|managed| managed.cwd == session.cwd)
                        .unwrap_or(false)
            })
            .unwrap_or(false)
    }

    pub fn reset_unowned_connection_after_authentication(&self) -> Result<()> {
        let mut state = self.lock_state()?;
        if !state
            .sessions
            .values()
            .any(|session| session.connection_owned)
        {
            state.connection = None;
        }
        Ok(())
    }

    fn lock_state(&self) -> Result<std::sync::MutexGuard<'_, CopilotManagedState>> {
        self.state
            .lock()
            .map_err(|_| anyhow!("Copilot managed connection lock was poisoned"))
    }

    fn persist_locked(&self, state: &CopilotManagedState) -> Result<()> {
        let records = state
            .sessions
            .values()
            .map(record_from_managed)
            .map(|record| (record.session_id.clone(), record))
            .collect::<BTreeMap<_, _>>();
        with_locked_registry(&self.lock_path, &self.registry_path, move |registry| {
            registry.sessions.extend(records);
            Ok(())
        })
    }

    fn remove_persisted(&self, session_id: &str) -> Result<()> {
        let session_id = session_id.to_owned();
        with_locked_registry(&self.lock_path, &self.registry_path, move |registry| {
            registry.sessions.remove(&session_id);
            Ok(())
        })
    }
}

fn copilot_model_options(response: &Value) -> Result<Vec<String>> {
    let options = response
        .get("configOptions")
        .and_then(Value::as_array)
        .and_then(|options| {
            options.iter().find(|option| {
                option.get("id").and_then(Value::as_str) == Some("model")
                    || option.get("category").and_then(Value::as_str) == Some("model")
            })
        })
        .and_then(|option| option.get("options"))
        .and_then(Value::as_array)
        .context("Copilot ACP session/new omitted its model configuration options")?;
    let values = options
        .iter()
        .filter_map(|option| option.get("value").and_then(Value::as_str))
        .filter(|value| {
            !value.is_empty()
                && value.len() <= 128
                && !value
                    .chars()
                    .any(|character| character.is_control() || character.is_whitespace())
        })
        .map(str::to_owned)
        .collect::<Vec<_>>();
    if values.is_empty() {
        bail!("Copilot ACP returned an empty model configuration");
    }
    Ok(values)
}

impl CopilotManagedState {
    fn ensure_connection(&mut self, executable: &str) -> Result<()> {
        if self.connection.is_none() {
            self.connection = Some(CopilotAcpConnection::connect(
                executable,
                CopilotAcpMode::Control,
            )?);
        }
        Ok(())
    }

    fn connection_mut(&mut self) -> Result<&mut CopilotAcpConnection> {
        self.connection
            .as_mut()
            .context("Copilot ACP control connection is not established")
    }

    fn require_owned(&self, session: &AgentSession) -> Result<&ManagedCopilotSession> {
        let managed = self.require_tracked(session)?;
        if !managed.connection_owned {
            bail!("Copilot session is currently attached to its native frontend");
        }
        Ok(managed)
    }

    fn require_tracked(&self, session: &AgentSession) -> Result<&ManagedCopilotSession> {
        require_copilot_host(session)?;
        let managed = self
            .sessions
            .get(&session.provider_session_id)
            .context("Copilot session is not owned by this ACP connection")?;
        if managed.cwd != session.cwd {
            bail!("Copilot session working directory does not match its owned record");
        }
        Ok(managed)
    }

    fn drain(&mut self) -> Result<()> {
        loop {
            let message = match self.connection.as_mut() {
                Some(connection) => connection.try_receive()?,
                None => return Ok(()),
            };
            let Some(message) = message else {
                return Ok(());
            };
            match message {
                CopilotAcpMessage::SessionUpdate { session_id, update } => {
                    if let Some(session) = self.sessions.get_mut(&session_id) {
                        if session.connection_owned {
                            apply_session_update(session, &update);
                        }
                    }
                }
                CopilotAcpMessage::PermissionRequest(request) => {
                    if let Some(session) = self.sessions.get_mut(&request.session_id) {
                        if !session.connection_owned {
                            self.connection_mut()?
                                .respond_permission_cancelled(&request.request_id)?;
                            continue;
                        }
                        if session.permission.is_some() {
                            self.connection_mut()?
                                .respond_permission_cancelled(&request.request_id)?;
                            continue;
                        }
                        session.permission = Some(request);
                        session.state = SessionState::NeedsInput;
                        session.summary = "Copilot needs permission".into();
                        session.updated_at = SystemTime::now();
                    } else {
                        self.connection_mut()?
                            .respond_permission_cancelled(&request.request_id)?;
                    }
                }
                CopilotAcpMessage::Response { id, result } => {
                    let Some(request_id) = id.as_u64() else {
                        continue;
                    };
                    let session_id = self
                        .sessions
                        .iter()
                        .find(|(_, session)| session.active_prompt == Some(request_id))
                        .map(|(session_id, _)| session_id.clone());
                    if let Some(session_id) = session_id {
                        let pending_request = self.sessions[&session_id]
                            .permission
                            .as_ref()
                            .map(|permission| permission.request_id.clone());
                        if let Some(pending_request) = pending_request {
                            self.connection_mut()?
                                .respond_permission_cancelled(&pending_request)?;
                        }
                        let session = self
                            .sessions
                            .get_mut(&session_id)
                            .expect("managed session found by ID");
                        session.active_prompt = None;
                        session.permission = None;
                        session.updated_at = SystemTime::now();
                        match result {
                            Ok(response) => {
                                let reason = response
                                    .get("stopReason")
                                    .and_then(Value::as_str)
                                    .unwrap_or("completed");
                                session.state = SessionState::Completed;
                                if !session.summary_from_message {
                                    session.summary = format!("Copilot stopped: {reason}");
                                }
                            }
                            Err(error) => {
                                session.state = SessionState::NeedsInput;
                                session.summary =
                                    format!("Copilot prompt failed: {}", compact(&error));
                            }
                        }
                    }
                }
                CopilotAcpMessage::UnsupportedRequest { id, method, .. } => {
                    self.connection_mut()?.reject_unsupported_request(
                        &id,
                        &format!("Open Agent View does not implement ACP client method `{method}`"),
                    )?;
                }
                CopilotAcpMessage::Notification { .. } => {}
            }
        }
    }
}

fn normalize_managed(session: &ManagedCopilotSession) -> AgentSession {
    let mut capabilities = BTreeSet::from([Capability::Inspect]);
    if !session.connection_owned {
        // Opening remains available through the controller's ungated native
        // path. Inline capabilities belong only to the exact ACP owner.
        if crate::native_session::is_backgrounded(&format!(
            "github_copilot:host:{}",
            session.session_id
        )) {
            capabilities.insert(Capability::Interrupt);
        }
    } else if session.active_prompt.is_some() {
        capabilities.insert(Capability::Interrupt);
    } else if session.permission.is_none() {
        capabilities.insert(Capability::Reply);
    }
    if let Some(permission) = &session.permission {
        if permission
            .options
            .iter()
            .any(|option| option.kind == "allow_once")
        {
            capabilities.insert(Capability::Approve);
        }
        if permission
            .options
            .iter()
            .any(|option| option.kind == "reject_once")
        {
            capabilities.insert(Capability::Decline);
        }
        // Cancelling the prompt is valid even while permission is pending.
        capabilities.insert(Capability::Interrupt);
    }
    AgentSession {
        id: format!("github_copilot:host:{}", session.session_id),
        provider_session_id: session.session_id.clone(),
        provider: Provider::GitHubCopilot,
        runtime: Runtime::Host,
        kind: SessionKind::Managed,
        name: session.name.clone(),
        cwd: session.cwd.clone(),
        state: session.state,
        summary: session.summary.clone(),
        raw_state: Some(if !session.connection_owned {
            "native".into()
        } else if session.permission.is_some() {
            "awaiting_permission".into()
        } else if session.active_prompt.is_some() {
            "working".into()
        } else {
            "idle".into()
        }),
        pid: None,
        started_at: Some(session.started_at),
        updated_at: Some(session.updated_at),
        pull_requests: None,
        capabilities,
    }
}

fn apply_session_update(session: &mut ManagedCopilotSession, update: &Value) {
    let update_type = update
        .get("sessionUpdate")
        .and_then(Value::as_str)
        .unwrap_or("");
    let message_role = match update_type {
        "agent_message_chunk" => Some(MessageRole::Assistant),
        "user_message_chunk" => Some(MessageRole::User),
        _ => None,
    };
    if let Some(role) = message_role {
        if let Some(text) = update
            .get("content")
            .and_then(|content| content.get("text"))
            .and_then(Value::as_str)
        {
            if session.last_message_role != Some(role) {
                if !session.transcript.is_empty() && !session.transcript.ends_with('\n') {
                    session.transcript.push('\n');
                }
                session.transcript.push_str(match role {
                    MessageRole::User => "User: ",
                    MessageRole::Assistant => "Assistant: ",
                });
                session.last_message.clear();
                session.last_message_role = Some(role);
            }
            session.transcript.push_str(text);
            session.transcript = tail_chars(&session.transcript, MAX_TRANSCRIPT_CHARS);
            session.last_message.push_str(text);
            session.last_message = head_chars(&session.last_message, MAX_TRANSCRIPT_CHARS);
            if !session.last_message.trim().is_empty() {
                session.summary = compact_message_summary(&session.last_message, 240);
                session.summary_from_message = true;
            }
        }
    } else if update_type == "tool_call" {
        if let Some(title) = update.get("title").and_then(Value::as_str) {
            session.summary = compact_message_summary(title, 240);
            session.summary_from_message = true;
            session.last_message_role = None;
            session.last_message.clear();
        }
    }
    session.updated_at = SystemTime::now();
}

fn require_copilot_host(session: &AgentSession) -> Result<()> {
    if session.provider != Provider::GitHubCopilot || session.runtime != Runtime::Host {
        bail!("the managed Copilot ACP connection does not own this provider/runtime");
    }
    Ok(())
}

fn require_absolute_cwd(cwd: &Path) -> Result<()> {
    if !cwd.is_absolute() {
        bail!("Copilot ACP working directory must be absolute");
    }
    Ok(())
}

fn require_prompt(prompt: &str) -> Result<()> {
    if prompt.trim().is_empty() {
        bail!("Copilot prompt must not be empty");
    }
    Ok(())
}

fn prompt_name(prompt: &str) -> String {
    let normalized = prompt.split_whitespace().collect::<Vec<_>>().join(" ");
    tail_chars(&normalized, 48)
}

fn tail_chars(value: &str, limit: usize) -> String {
    let count = value.chars().count();
    value.chars().skip(count.saturating_sub(limit)).collect()
}

fn compact_message_summary(value: &str, limit: usize) -> String {
    let compact = value.split_whitespace().collect::<Vec<_>>().join(" ");
    head_chars(&compact, limit)
}

fn head_chars(value: &str, limit: usize) -> String {
    if value.chars().count() <= limit {
        return value.to_owned();
    }
    let mut head = value
        .chars()
        .take(limit.saturating_sub(1))
        .collect::<String>();
    head.push('…');
    head
}

fn compact(value: &Value) -> String {
    let encoded = value.to_string();
    tail_chars(&encoded, 300)
}

fn actionable_auth_error(error: anyhow::Error, executable: &str) -> anyhow::Error {
    if format!("{error:#}").contains("Authentication required") {
        anyhow!(
            "GitHub Copilot is not authenticated; run `copilot login`, or authenticate `gh` with an account that has Copilot access (configured executable: {executable})"
        )
    } else {
        error
    }
}

pub fn default_copilot_state_dir() -> Result<PathBuf> {
    if let Some(state_home) = std::env::var_os("XDG_STATE_HOME") {
        return Ok(PathBuf::from(state_home).join("open-agent-view/copilot"));
    }
    let home = std::env::var_os("HOME").context("HOME is not set")?;
    Ok(PathBuf::from(home).join(".local/state/open-agent-view/copilot"))
}

fn default_true() -> bool {
    true
}

fn record_from_managed(session: &ManagedCopilotSession) -> CopilotRegistryRecord {
    CopilotRegistryRecord {
        session_id: session.session_id.clone(),
        cwd: session.cwd.clone(),
        name: session.name.clone(),
        state: session.state,
        summary: session.summary.clone(),
        summary_from_message: session.summary_from_message,
        started_at_ms: system_time_millis(session.started_at),
        updated_at_ms: system_time_millis(session.updated_at),
        managed_authority: session.managed_authority,
    }
}

fn managed_from_provider(
    session: &CopilotSessionInfo,
    managed_authority: bool,
) -> ManagedCopilotSession {
    let updated_at = session
        .updated_at
        .as_deref()
        .and_then(parse_copilot_updated_at)
        .unwrap_or_else(SystemTime::now);
    let name = session
        .title
        .as_deref()
        .map(str::trim)
        .filter(|title| !title.is_empty())
        .map(|title| compact_message_summary(title, 512))
        .or_else(|| {
            session
                .cwd
                .file_name()
                .and_then(|name| name.to_str())
                .map(str::to_owned)
        })
        .unwrap_or_else(|| format!("copilot-{}", short_session_id(&session.session_id)));
    ManagedCopilotSession {
        session_id: session.session_id.clone(),
        cwd: session.cwd.clone(),
        name,
        state: SessionState::Completed,
        summary: "Loading persisted Copilot conversation…".into(),
        summary_from_message: false,
        transcript: String::new(),
        last_message_role: None,
        last_message: String::new(),
        active_prompt: None,
        permission: None,
        connection_owned: false,
        started_at: updated_at,
        updated_at,
        managed_authority,
    }
}

fn short_session_id(session_id: &str) -> String {
    session_id.chars().take(8).collect()
}

fn managed_from_record(record: CopilotRegistryRecord) -> ManagedCopilotSession {
    let state = if record.state == SessionState::Unknown {
        SessionState::Unknown
    } else {
        // ACP connections and native PTYs are process-owned. After a dashboard
        // restart a persisted record is history until an exact connection is
        // re-established; it must never pretend stale live authority survived.
        SessionState::Completed
    };
    ManagedCopilotSession {
        session_id: record.session_id,
        cwd: record.cwd,
        name: record.name,
        state,
        summary: record.summary,
        summary_from_message: record.summary_from_message,
        transcript: String::new(),
        last_message_role: None,
        last_message: String::new(),
        active_prompt: None,
        permission: None,
        connection_owned: false,
        started_at: millis_system_time(record.started_at_ms),
        updated_at: millis_system_time(record.updated_at_ms),
        managed_authority: record.managed_authority,
    }
}

fn system_time_millis(time: SystemTime) -> u64 {
    time.duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis()
        .try_into()
        .unwrap_or(u64::MAX)
}

fn millis_system_time(milliseconds: u64) -> SystemTime {
    UNIX_EPOCH + Duration::from_millis(milliseconds)
}

fn ensure_private_directory(path: &Path) -> Result<()> {
    match fs::symlink_metadata(path) {
        Ok(metadata) if metadata.file_type().is_symlink() || !metadata.is_dir() => {
            bail!("{} must be a real directory", path.display())
        }
        Ok(_) => {}
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
            fs::create_dir_all(path).with_context(|| {
                format!("failed to create private directory {}", path.display())
            })?;
        }
        Err(error) => return Err(error.into()),
    }
    #[cfg(unix)]
    {
        use std::os::unix::fs::{MetadataExt, PermissionsExt};
        let metadata = fs::symlink_metadata(path)?;
        if metadata.uid() != unsafe { libc::geteuid() } {
            bail!("{} is not owned by the current user", path.display());
        }
        if metadata.permissions().mode() & 0o077 != 0 {
            fs::set_permissions(path, fs::Permissions::from_mode(0o700))?;
        }
    }
    Ok(())
}

fn ensure_private_regular_file(path: &Path) -> Result<()> {
    let metadata = fs::symlink_metadata(path)
        .with_context(|| format!("failed to inspect private file {}", path.display()))?;
    if metadata.file_type().is_symlink() || !metadata.is_file() {
        bail!("{} must be a real regular file", path.display());
    }
    #[cfg(unix)]
    {
        use std::os::unix::fs::{MetadataExt, PermissionsExt};
        if metadata.uid() != unsafe { libc::geteuid() } {
            bail!("{} is not owned by the current user", path.display());
        }
        if metadata.permissions().mode() & 0o077 != 0 {
            bail!("{} is accessible by other users", path.display());
        }
    }
    Ok(())
}

fn private_lock_file(path: &Path) -> Result<File> {
    let mut options = OpenOptions::new();
    options.create(true).read(true).write(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.mode(0o600);
    }
    let file = options
        .open(path)
        .with_context(|| format!("failed to open Copilot registry lock {}", path.display()))?;
    ensure_private_regular_file(path)?;
    Ok(file)
}

fn read_registry(path: &Path) -> Result<CopilotRegistry> {
    match fs::symlink_metadata(path) {
        Ok(_) => ensure_private_regular_file(path)?,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
            return Ok(CopilotRegistry::default())
        }
        Err(error) => return Err(error.into()),
    }
    const MAX_REGISTRY_BYTES: u64 = 4 * 1024 * 1024;
    if fs::metadata(path)?.len() > MAX_REGISTRY_BYTES {
        bail!("Copilot registry {} exceeds 4 MiB", path.display());
    }
    let input = fs::read_to_string(path)
        .with_context(|| format!("failed to read Copilot registry {}", path.display()))?;
    let registry: CopilotRegistry = serde_json::from_str(&input)
        .with_context(|| format!("invalid Copilot registry {}", path.display()))?;
    if registry.version != REGISTRY_VERSION {
        bail!(
            "unsupported Copilot registry version {} in {}",
            registry.version,
            path.display()
        );
    }
    for (session_id, record) in &registry.sessions {
        if session_id != &record.session_id {
            bail!("Copilot registry key does not match its session ID");
        }
        validate_registry_text(session_id, 512, "session ID")?;
        validate_registry_text(&record.name, 512, "session name")?;
        validate_registry_text(&record.summary, 4096, "session summary")?;
        if !record.cwd.is_absolute() {
            bail!("Copilot registry workspace must be absolute");
        }
    }
    Ok(registry)
}

fn validate_registry_text(value: &str, max_bytes: usize, label: &str) -> Result<()> {
    if value.is_empty() || value.len() > max_bytes || value.chars().any(char::is_control) {
        bail!("invalid Copilot registry {label}");
    }
    Ok(())
}

fn write_registry(path: &Path, registry: &CopilotRegistry) -> Result<()> {
    let parent = path
        .parent()
        .context("Copilot registry path has no parent")?;
    ensure_private_directory(parent)?;
    let nonce = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_nanos();
    let temporary = path.with_extension(format!("tmp-{}-{nonce}", std::process::id()));
    let result = (|| {
        let mut options = OpenOptions::new();
        options.create_new(true).write(true);
        #[cfg(unix)]
        {
            use std::os::unix::fs::OpenOptionsExt;
            options.mode(0o600);
        }
        let mut file = options
            .open(&temporary)
            .with_context(|| format!("failed to create {}", temporary.display()))?;
        serde_json::to_writer_pretty(&mut file, registry)?;
        file.write_all(b"\n")?;
        file.sync_all()?;
        crate::fs_util::replace_file(&temporary, path)
            .with_context(|| format!("failed to replace {}", path.display()))?;
        ensure_private_regular_file(path)
    })();
    if result.is_err() {
        let _ = fs::remove_file(&temporary);
    }
    result
}

fn with_locked_registry<T>(
    lock_path: &Path,
    registry_path: &Path,
    operation: impl FnOnce(&mut CopilotRegistry) -> Result<T>,
) -> Result<T> {
    let lock = private_lock_file(lock_path)?;
    #[cfg(unix)]
    {
        use std::os::fd::AsRawFd;
        if unsafe { libc::flock(lock.as_raw_fd(), libc::LOCK_EX) } < 0 {
            return Err(std::io::Error::last_os_error()).context("failed to lock Copilot registry");
        }
    }
    let mut registry = read_registry(registry_path)?;
    let before = registry.clone();
    let result = operation(&mut registry);
    if result.is_ok() && registry != before {
        write_registry(registry_path, &registry)?;
    }
    #[cfg(unix)]
    {
        use std::os::fd::AsRawFd;
        if unsafe { libc::flock(lock.as_raw_fd(), libc::LOCK_UN) } < 0 {
            return Err(std::io::Error::last_os_error())
                .context("failed to unlock Copilot registry");
        }
    }
    result
}

#[cfg(all(test, unix))]
mod tests {
    use std::fs;
    use std::os::unix::fs::PermissionsExt;
    use std::thread;
    use std::time::Instant;

    use tempfile::tempdir;

    use super::*;

    #[test]
    fn authentication_failures_become_actionable_without_echoing_protocol_payloads() {
        let error = actionable_auth_error(
            anyhow!("Copilot ACP request failed: Authentication required"),
            "/opt/copilot",
        );
        let message = error.to_string();
        assert!(message.contains("copilot login"));
        assert!(message.contains("configured executable: /opt/copilot"));
        assert!(message.contains("authenticate `gh`"));
        assert!(!message.contains("ACP request failed"));
    }

    #[test]
    fn selected_model_is_validated_and_set_before_the_first_prompt() {
        let directory = tempdir().unwrap();
        let script = directory.path().join("copilot-model-mock");
        fs::write(
            &script,
            r##"#!/bin/sh
read initialize
printf '%s\n' '{"jsonrpc":"2.0","id":1,"result":{"protocolVersion":1,"agentCapabilities":{"sessionCapabilities":{}}}}'
read new_session
printf '%s\n' '{"jsonrpc":"2.0","id":2,"result":{"sessionId":"model-session","configOptions":[{"id":"model","category":"model","type":"select","currentValue":"auto","options":[{"value":"auto","name":"Auto"},{"value":"gpt-5.4","name":"GPT 5.4"}]}]}}'
read selected
case "$selected" in
  *'"method":"session/set_config_option"'*'"configId":"model"'*'"value":"gpt-5.4"'*) ;;
  *) exit 91 ;;
esac
printf '%s\n' '{"jsonrpc":"2.0","id":3,"result":{"configOptions":[]}}'
read prompt
case "$prompt" in *'"method":"session/prompt"'*) ;; *) exit 92 ;; esac
while read remaining; do :; done
"##,
        )
        .unwrap();
        let mut permissions = fs::metadata(&script).unwrap().permissions();
        permissions.set_mode(0o700);
        fs::set_permissions(&script, permissions).unwrap();
        let workspace = directory.path().join("workspace");
        fs::create_dir(&workspace).unwrap();
        let supervisor = CopilotSupervisor::with_state_dir(
            script.display().to_string(),
            directory.path().join("state"),
        )
        .unwrap();

        assert_eq!(
            supervisor
                .launch_with_model("test model", &workspace, Some("gpt-5.4"))
                .unwrap(),
            "model-session"
        );
    }

    #[test]
    fn retained_connection_controls_only_sessions_it_created() {
        let directory = tempdir().unwrap();
        let script = directory.path().join("copilot-mock");
        fs::write(
            &script,
            r##"#!/bin/sh
read initialize
printf '%s\n' '{"jsonrpc":"2.0","id":1,"result":{"protocolVersion":1,"agentCapabilities":{"loadSession":true,"sessionCapabilities":{"list":{},"close":{}}}}}'
read new_session
printf '%s\n' '{"jsonrpc":"2.0","id":2,"result":{"sessionId":"owned-one"}}'
read first_prompt
printf '%s\n' '{"jsonrpc":"2.0","method":"session/update","params":{"sessionId":"owned-one","update":{"sessionUpdate":"agent_message_chunk","content":{"type":"text","text":"checking"}}}}'
printf '%s\n' '{"jsonrpc":"2.0","id":"permission-1","method":"session/request_permission","params":{"sessionId":"owned-one","options":[{"optionId":"allow-once","name":"Allow once","kind":"allow_once"},{"optionId":"reject-once","name":"Reject once","kind":"reject_once"}]}}'
read permission
printf '%s\n' '{"jsonrpc":"2.0","id":3,"result":{"stopReason":"end_turn"}}'
read second_prompt
printf '%s\n' '{"jsonrpc":"2.0","method":"session/update","params":{"sessionId":"owned-one","update":{"sessionUpdate":"agent_message_chunk","content":{"type":"text","text":" again"}}}}'
read cancellation
while read remaining; do :; done
"##,
        )
        .unwrap();
        let mut permissions = fs::metadata(&script).unwrap().permissions();
        permissions.set_mode(0o700);
        fs::set_permissions(&script, permissions).unwrap();
        let workspace = directory.path().join("workspace");
        fs::create_dir(&workspace).unwrap();
        let supervisor = CopilotSupervisor::with_state_dir(
            script.display().to_string(),
            directory.path().join("state"),
        )
        .unwrap();

        let id = supervisor.launch("check safely", &workspace).unwrap();
        assert_eq!(id, "owned-one");
        let mut snapshot = SessionSnapshot::default();
        let waiting = wait_for_state(&supervisor, &mut snapshot, SessionState::NeedsInput);
        assert_eq!(supervisor.inspect(&waiting).unwrap(), "Assistant: checking");
        assert!(waiting.capabilities.contains(&Capability::Approve));
        assert!(waiting.capabilities.contains(&Capability::Decline));

        supervisor.resolve_approval(&waiting, false).unwrap();
        let completed = wait_for_state(&supervisor, &mut snapshot, SessionState::Completed);
        assert!(completed.capabilities.contains(&Capability::Reply));
        supervisor.reply(&completed, "retry").unwrap();
        let working = wait_for_state(&supervisor, &mut snapshot, SessionState::Working);
        assert!(working.capabilities.contains(&Capability::Interrupt));
        supervisor.interrupt(&working).unwrap();

        let external = AgentSession {
            id: "github_copilot:host:external".into(),
            provider_session_id: "external".into(),
            provider: Provider::GitHubCopilot,
            runtime: Runtime::Host,
            kind: SessionKind::Unknown,
            name: "external".into(),
            cwd: workspace,
            state: SessionState::Unknown,
            summary: String::new(),
            raw_state: None,
            pid: None,
            started_at: None,
            updated_at: None,
            pull_requests: None,
            capabilities: BTreeSet::new(),
        };
        assert!(!supervisor.owns(&external));
        assert!(supervisor.reply(&external, "unsafe").is_err());
    }

    #[test]
    fn explicit_load_establishes_connection_ownership() {
        let directory = tempdir().unwrap();
        let script = directory.path().join("copilot-load-mock");
        fs::write(
            &script,
            r##"#!/bin/sh
read initialize
printf '%s\n' '{"jsonrpc":"2.0","id":1,"result":{"protocolVersion":1,"agentCapabilities":{"loadSession":true,"sessionCapabilities":{"list":{},"close":{}}}}}'
read load_session
printf '%s\n' '{"jsonrpc":"2.0","id":2,"result":{}}'
while read remaining; do :; done
"##,
        )
        .unwrap();
        let mut permissions = fs::metadata(&script).unwrap().permissions();
        permissions.set_mode(0o700);
        fs::set_permissions(&script, permissions).unwrap();
        let workspace = directory.path().join("workspace");
        fs::create_dir(&workspace).unwrap();
        let external = AgentSession {
            id: "github_copilot:host:persisted-one".into(),
            provider_session_id: "persisted-one".into(),
            provider: Provider::GitHubCopilot,
            runtime: Runtime::Host,
            kind: SessionKind::Unknown,
            name: "persisted".into(),
            cwd: workspace,
            state: SessionState::Unknown,
            summary: String::new(),
            raw_state: None,
            pid: None,
            started_at: None,
            updated_at: None,
            pull_requests: None,
            capabilities: BTreeSet::new(),
        };
        let supervisor = CopilotSupervisor::with_state_dir(
            script.display().to_string(),
            directory.path().join("state"),
        )
        .unwrap();

        assert!(!supervisor.owns(&external));
        supervisor.load(&external).unwrap();
        assert!(supervisor.owns(&external));
        let mut snapshot = SessionSnapshot {
            sessions: vec![external],
            warnings: Vec::new(),
        };
        supervisor.enrich(&mut snapshot);
        assert_eq!(snapshot.sessions[0].kind, SessionKind::Managed);
        assert!(snapshot.sessions[0]
            .capabilities
            .contains(&Capability::Reply));
        assert!(snapshot.sessions[0]
            .capabilities
            .contains(&Capability::Inspect));
    }

    #[test]
    fn sole_idle_session_can_handoff_when_close_is_not_advertised() {
        let directory = tempdir().unwrap();
        let script = directory.path().join("copilot-no-close-mock");
        fs::write(
            &script,
            r##"#!/bin/sh
read initialize
printf '%s\n' '{"jsonrpc":"2.0","id":1,"result":{"protocolVersion":1,"agentCapabilities":{"loadSession":true,"sessionCapabilities":{"list":{}}}}}'
read new_session
printf '%s\n' '{"jsonrpc":"2.0","id":2,"result":{"sessionId":"no-close-owned"}}'
read prompt
printf '%s\n' '{"jsonrpc":"2.0","id":3,"result":{"stopReason":"end_turn"}}'
while read remaining; do :; done
"##,
        )
        .unwrap();
        let mut permissions = fs::metadata(&script).unwrap().permissions();
        permissions.set_mode(0o700);
        fs::set_permissions(&script, permissions).unwrap();
        let workspace = directory.path().join("workspace");
        fs::create_dir(&workspace).unwrap();
        let supervisor = CopilotSupervisor::with_state_dir(
            script.display().to_string(),
            directory.path().join("state"),
        )
        .unwrap();
        supervisor.launch("complete first", &workspace).unwrap();
        let mut snapshot = SessionSnapshot::default();
        let completed = wait_for_state(&supervisor, &mut snapshot, SessionState::Completed);

        supervisor.release_for_native(&completed).unwrap();

        assert!(!supervisor.owns(&completed));
        snapshot.sessions.clear();
        supervisor.enrich(&mut snapshot);
        assert_eq!(snapshot.sessions[0].raw_state.as_deref(), Some("native"));
    }

    #[test]
    fn owned_source_replays_real_text_and_survives_dashboard_restart() {
        let directory = tempdir().unwrap();
        let script = directory.path().join("copilot-history-mock");
        let workspace = directory.path().join("workspace");
        fs::create_dir(&workspace).unwrap();
        fs::write(
            &script,
            r##"#!/bin/sh
read initialize
printf '%s\n' '{"jsonrpc":"2.0","id":1,"result":{"protocolVersion":1,"agentCapabilities":{"loadSession":true,"sessionCapabilities":{"list":{}}}}}'
read list
case "$list" in *'"method":"session/list"'*) ;; *) exit 91 ;; esac
printf '%s\n' '{"jsonrpc":"2.0","id":2,"result":{"sessions":[{"sessionId":"history-one","cwd":"WORKSPACE","title":"Provider title","updatedAt":"2026-08-25T10:51:06.934Z"}]}}'
read load
case "$load" in *'"method":"session/load"'*'"sessionId":"history-one"'*) ;; *) exit 92 ;; esac
printf '%s\n' '{"jsonrpc":"2.0","method":"session/update","params":{"sessionId":"history-one","update":{"sessionUpdate":"user_message_chunk","content":{"type":"text","text":"hello"}}}}'
printf '%s\n' '{"jsonrpc":"2.0","method":"session/update","params":{"sessionId":"history-one","update":{"sessionUpdate":"agent_message_chunk","content":{"type":"text","text":"A real "}}}}'
printf '%s\n' '{"jsonrpc":"2.0","method":"session/update","params":{"sessionId":"history-one","update":{"sessionUpdate":"agent_message_chunk","content":{"type":"text","text":"answer"}}}}'
printf '%s\n' '{"jsonrpc":"2.0","method":"session/update","params":{"sessionId":"history-one","update":{"sessionUpdate":"user_message_chunk","content":{"type":"text","text":"What about edge cases?"}}}}'
printf '%s\n' '{"jsonrpc":"2.0","id":3,"result":{}}'
while read remaining; do :; done
"##
            .replace("WORKSPACE", &workspace.display().to_string()),
        )
        .unwrap();
        let mut permissions = fs::metadata(&script).unwrap().permissions();
        permissions.set_mode(0o700);
        fs::set_permissions(&script, permissions).unwrap();
        let state_dir = directory.path().join("state");
        let supervisor = Arc::new(
            CopilotSupervisor::with_state_dir(script.display().to_string(), state_dir.clone())
                .unwrap(),
        );
        let source =
            CopilotOwnedSource::new(supervisor, ["github_copilot:host:history-one".to_owned()]);
        let discovered = source
            .discover_with_warnings(&DiscoveryRequest {
                include_completed: true,
                ..DiscoveryRequest::default()
            })
            .unwrap();

        assert!(discovered.warnings.is_empty(), "{:?}", discovered.warnings);
        assert_eq!(discovered.sessions.len(), 1);
        let session = &discovered.sessions[0];
        assert_eq!(session.name, "Provider title");
        assert_eq!(session.summary, "What about edge cases?");
        assert_eq!(
            session.updated_at,
            parse_copilot_updated_at("2026-08-25T10:51:06.934Z")
        );
        assert!(session.capabilities.contains(&Capability::Inspect));
        assert!(!session.capabilities.contains(&Capability::Reply));
        let expected_updated_at = session.updated_at;

        drop(source);
        let restarted =
            Arc::new(CopilotSupervisor::with_state_dir("/bin/false", state_dir).unwrap());
        let restarted_source = CopilotOwnedSource::new(restarted, Vec::new());
        let restarted = restarted_source
            .discover_with_warnings(&DiscoveryRequest {
                include_completed: true,
                ..DiscoveryRequest::default()
            })
            .unwrap();
        assert_eq!(restarted.sessions.len(), 1);
        assert_eq!(restarted.sessions[0].summary, "What about edge cases?");
        assert_eq!(restarted.sessions[0].updated_at, expected_updated_at);
        assert_eq!(restarted.warnings.len(), 1);
        assert!(restarted.warnings[0].contains("could not refresh persisted Copilot text"));
        #[cfg(unix)]
        {
            assert_eq!(
                fs::metadata(directory.path().join("state/sessions.json"))
                    .unwrap()
                    .permissions()
                    .mode()
                    & 0o777,
                0o600
            );
        }
    }

    #[test]
    fn lifecycle_completion_does_not_replace_provider_message_or_timestamp() {
        assert_eq!(
            compact_message_summary("first\nsecond\tthird", 64),
            "first second third"
        );
        assert_eq!(compact_message_summary("abcdefgh", 5), "abcd…");
        let now = SystemTime::now();
        let mut session = ManagedCopilotSession {
            session_id: "message-one".into(),
            cwd: PathBuf::from("/workspace"),
            name: "message".into(),
            state: SessionState::Working,
            summary: "Copilot is working".into(),
            summary_from_message: false,
            transcript: String::new(),
            last_message_role: None,
            last_message: String::new(),
            active_prompt: Some(7),
            permission: None,
            connection_owned: true,
            started_at: now,
            updated_at: now,
            managed_authority: true,
        };
        apply_session_update(
            &mut session,
            &serde_json::json!({
                "sessionUpdate": "agent_message_chunk",
                "content": {"type": "text", "text": "actual answer"}
            }),
        );
        let message_time = session.updated_at;
        assert_eq!(session.summary, "actual answer");
        assert!(session.summary_from_message);
        session.state = SessionState::Completed;
        if !session.summary_from_message {
            session.summary = "Copilot stopped: end_turn".into();
        }
        assert_eq!(session.summary, "actual answer");
        assert_eq!(session.updated_at, message_time);
    }

    #[test]
    fn registry_refuses_symlinks_and_public_files() {
        let directory = tempdir().unwrap();
        let real_state = directory.path().join("real-state");
        fs::create_dir(&real_state).unwrap();
        let linked_state = directory.path().join("linked-state");
        std::os::unix::fs::symlink(&real_state, &linked_state).unwrap();
        assert!(CopilotSupervisor::with_state_dir("unused", linked_state).is_err());

        let state = directory.path().join("state");
        fs::create_dir(&state).unwrap();
        fs::set_permissions(&state, fs::Permissions::from_mode(0o700)).unwrap();
        let registry = state.join("sessions.json");
        fs::write(
            &registry,
            serde_json::to_vec(&CopilotRegistry::default()).unwrap(),
        )
        .unwrap();
        fs::set_permissions(&registry, fs::Permissions::from_mode(0o644)).unwrap();
        assert!(CopilotSupervisor::with_state_dir("unused", state).is_err());
    }

    fn wait_for_state(
        supervisor: &CopilotSupervisor,
        snapshot: &mut SessionSnapshot,
        expected: SessionState,
    ) -> AgentSession {
        let deadline = Instant::now() + Duration::from_secs(3);
        loop {
            snapshot.sessions.clear();
            snapshot.warnings.clear();
            supervisor.enrich(snapshot);
            if let Some(session) = snapshot
                .sessions
                .first()
                .filter(|session| session.state == expected)
            {
                return session.clone();
            }
            assert!(snapshot.warnings.is_empty(), "{:?}", snapshot.warnings);
            assert!(
                Instant::now() < deadline,
                "session never reached {expected:?}"
            );
            thread::sleep(Duration::from_millis(20));
        }
    }
}

use std::collections::{BTreeMap, BTreeSet};
use std::path::{Path, PathBuf};
use std::sync::Mutex;
use std::time::{Duration, SystemTime};

use anyhow::{anyhow, bail, Context, Result};
use serde_json::Value;

use super::copilot::{
    CopilotAcpConnection, CopilotAcpMessage, CopilotAcpMode, CopilotPermissionRequest,
};
use crate::domain::{
    AgentSession, Capability, Provider, Runtime, SessionKind, SessionSnapshot, SessionState,
};

const RESPONSE_TIMEOUT: Duration = Duration::from_secs(8);
const MAX_TRANSCRIPT_CHARS: usize = 32 * 1024;

#[derive(Clone, Debug)]
struct ManagedCopilotSession {
    session_id: String,
    cwd: PathBuf,
    name: String,
    state: SessionState,
    summary: String,
    transcript: String,
    active_prompt: Option<u64>,
    permission: Option<CopilotPermissionRequest>,
    started_at: SystemTime,
    updated_at: SystemTime,
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
    state: Mutex<CopilotManagedState>,
}

impl CopilotSupervisor {
    pub fn host(executable: impl Into<String>) -> Self {
        Self {
            executable: executable.into(),
            state: Mutex::new(CopilotManagedState {
                connection: None,
                sessions: BTreeMap::new(),
            }),
        }
    }

    pub fn executable(&self) -> &str {
        &self.executable
    }

    /// Create a session and begin its first prompt without enabling broad
    /// permission flags. The ACP process remains owned by this supervisor.
    pub fn launch(&self, prompt: &str, cwd: &Path) -> Result<String> {
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
                transcript: String::new(),
                active_prompt: None,
                permission: None,
                started_at: now,
                updated_at: now,
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
        Ok(session_id)
    }

    /// Explicitly load one persisted session onto this exact connection. This
    /// is the only way an external Copilot record can become connection-owned.
    pub fn load(&self, session: &AgentSession) -> Result<()> {
        require_copilot_host(session)?;
        let mut state = self.lock_state()?;
        state.ensure_connection(&self.executable)?;
        if state.sessions.contains_key(&session.provider_session_id) {
            return Ok(());
        }
        let request_id = state
            .connection_mut()?
            .begin_load_session(&session.provider_session_id, &session.cwd)?;
        state
            .connection_mut()?
            .wait_for_response(request_id, RESPONSE_TIMEOUT)?;
        let now = SystemTime::now();
        state.sessions.insert(
            session.provider_session_id.clone(),
            ManagedCopilotSession {
                session_id: session.provider_session_id.clone(),
                cwd: session.cwd.clone(),
                name: session.name.clone(),
                state: SessionState::NeedsInput,
                summary: "Loaded on Open Agent View's Copilot ACP connection".into(),
                transcript: String::new(),
                active_prompt: None,
                permission: None,
                started_at: session.started_at.unwrap_or(now),
                updated_at: now,
            },
        );
        Ok(())
    }

    pub fn enrich(&self, snapshot: &mut SessionSnapshot) {
        let result = (|| -> Result<()> {
            let mut state = self.lock_state()?;
            state.drain()?;
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
        let managed = state.require_owned(session)?;
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
        managed.updated_at = SystemTime::now();
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
        managed.updated_at = SystemTime::now();
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
        managed.updated_at = SystemTime::now();
        Ok(())
    }

    pub fn owns(&self, session: &AgentSession) -> bool {
        self.state
            .lock()
            .map(|state| {
                session.provider == Provider::GitHubCopilot
                    && session.runtime == Runtime::Host
                    && state.sessions.contains_key(&session.provider_session_id)
            })
            .unwrap_or(false)
    }

    fn lock_state(&self) -> Result<std::sync::MutexGuard<'_, CopilotManagedState>> {
        self.state
            .lock()
            .map_err(|_| anyhow!("Copilot managed connection lock was poisoned"))
    }
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
                        apply_session_update(session, &update);
                    }
                }
                CopilotAcpMessage::PermissionRequest(request) => {
                    if let Some(session) = self.sessions.get_mut(&request.session_id) {
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
                                session.summary = format!("Copilot stopped: {reason}");
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
    if session.active_prompt.is_some() {
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
        raw_state: Some(if session.permission.is_some() {
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
    if update_type == "agent_message_chunk" {
        if let Some(text) = update
            .get("content")
            .and_then(|content| content.get("text"))
            .and_then(Value::as_str)
        {
            session.transcript.push_str(text);
            session.transcript = tail_chars(&session.transcript, MAX_TRANSCRIPT_CHARS);
            if let Some(line) = text.lines().rev().find(|line| !line.trim().is_empty()) {
                session.summary = tail_chars(line.trim(), 240);
            }
        }
    } else if update_type == "tool_call" {
        if let Some(title) = update.get("title").and_then(Value::as_str) {
            session.summary = tail_chars(title, 240);
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

#[cfg(test)]
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
        let supervisor = CopilotSupervisor::host(script.display().to_string());

        let id = supervisor.launch("check safely", &workspace).unwrap();
        assert_eq!(id, "owned-one");
        let mut snapshot = SessionSnapshot::default();
        let waiting = wait_for_state(&supervisor, &mut snapshot, SessionState::NeedsInput);
        assert_eq!(supervisor.inspect(&waiting).unwrap(), "checking");
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
        let supervisor = CopilotSupervisor::host(script.display().to_string());

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

//! Process-local plain terminal sessions managed by Open Agent View.
//!
//! The terminal harness deliberately treats the new-task text as a display
//! name, never as a shell command. OAV opens the user's interactive shell in a
//! private PTY; boundary-double-arrow or Shift+Arrow backgrounds it,
//! Enter/Right resumes it, and Ctrl+X stops
//! only the exact child held by that PTY registry.

use std::collections::{BTreeMap, BTreeSet};
use std::path::PathBuf;
use std::process::Command;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Mutex;
use std::time::{SystemTime, UNIX_EPOCH};

use anyhow::{bail, Context, Result};

use super::{DiscoveryRequest, SessionSource};
use crate::control::{
    ControlOutcome, LaunchMode, LaunchPresentation, LaunchRequest, ProviderController,
};
use crate::domain::{AgentSession, Capability, Provider, Runtime, SessionKind, SessionState};

static TERMINAL_SEQUENCE: AtomicU64 = AtomicU64::new(1);

#[derive(Clone, Debug)]
struct TerminalRecord {
    key: String,
    name: String,
    cwd: PathBuf,
    state: SessionState,
    created_at: SystemTime,
    updated_at: SystemTime,
}

#[derive(Debug, Default)]
pub struct TerminalHarness {
    records: Mutex<BTreeMap<String, TerminalRecord>>,
}

impl TerminalHarness {
    pub fn new() -> Self {
        Self::default()
    }

    fn record(&self, key: &str) -> Result<Option<TerminalRecord>> {
        Ok(self
            .records
            .lock()
            .map_err(|_| anyhow::anyhow!("terminal registry lock was poisoned"))?
            .get(key)
            .cloned())
    }

    fn set_state(&self, key: &str, state: SessionState) -> Result<()> {
        let mut records = self
            .records
            .lock()
            .map_err(|_| anyhow::anyhow!("terminal registry lock was poisoned"))?;
        let record = records
            .get_mut(key)
            .context("the managed terminal no longer exists")?;
        record.state = state;
        record.updated_at = SystemTime::now();
        Ok(())
    }

    fn run_record(&self, record: &TerminalRecord, fresh: bool) -> Result<ControlOutcome> {
        let exit = if fresh {
            let shell = configured_shell();
            let mut command = Command::new(&shell);
            command.current_dir(&record.cwd);
            command.env("OAV_TERMINAL_NAME", &record.name);
            crate::native_session::run(command, &record.key)
                .with_context(|| format!("failed to open interactive shell {shell}"))?
        } else {
            crate::native_session::resume(&record.key)?
        };
        match exit {
            crate::native_session::NativeSessionExit::Backgrounded => {
                self.set_state(&record.key, SessionState::Working)?;
                Ok(ControlOutcome {
                    message: format!(
                        "backgrounded terminal {}; Enter/Right resumes it",
                        record.name
                    ),
                    provider_session_hint: Some(record.key.clone()),
                })
            }
            crate::native_session::NativeSessionExit::Exited(status) => {
                self.set_state(&record.key, SessionState::Completed)?;
                Ok(ControlOutcome {
                    message: format!("terminal {} exited with status {status}", record.name),
                    provider_session_hint: Some(record.key.clone()),
                })
            }
        }
    }
}

impl SessionSource for TerminalHarness {
    fn label(&self) -> &str {
        "Terminal (OAV-owned)"
    }

    fn discover(&self, request: &DiscoveryRequest) -> Result<Vec<AgentSession>> {
        let detached = crate::native_session::detached_session_keys()
            .into_iter()
            .collect::<BTreeSet<_>>();
        let mut records = self
            .records
            .lock()
            .map_err(|_| anyhow::anyhow!("terminal registry lock was poisoned"))?;
        let now = SystemTime::now();
        for record in records.values_mut() {
            if record.state != SessionState::Completed && !detached.contains(&record.key) {
                record.state = SessionState::Completed;
                record.updated_at = now;
            }
        }
        let mut sessions = records
            .values()
            .filter(|record| request.include_completed || record.state != SessionState::Completed)
            .filter(|record| {
                request
                    .cwd
                    .as_ref()
                    .map(|cwd| record.cwd.starts_with(cwd))
                    .unwrap_or(true)
            })
            .map(terminal_session)
            .collect::<Vec<_>>();
        drop(records);

        // Provider setup terminals use the same isolated PTY bridge. Surface
        // backgrounded logins as terminal jobs so users can return to them
        // without accidentally re-entering an unrelated agent session.
        let cwd = std::env::current_dir().unwrap_or_else(|_| PathBuf::from("."));
        for key in detached {
            if !key.starts_with("setup:") {
                continue;
            }
            let name = format!("{} setup", key.strip_prefix("setup:").unwrap_or("Provider"));
            sessions.push(AgentSession {
                id: format!("terminal:host:{key}"),
                provider_session_id: key,
                provider: Provider::Terminal,
                runtime: Runtime::Host,
                kind: SessionKind::Managed,
                name,
                cwd: cwd.clone(),
                state: SessionState::Working,
                summary: "Interactive provider setup · Enter resumes · Ctrl+X stops".into(),
                raw_state: Some("setup_terminal_backgrounded".into()),
                pid: None,
                started_at: None,
                updated_at: None,
                pull_requests: None,
                capabilities: BTreeSet::from([Capability::Inspect, Capability::Interrupt]),
            });
        }
        Ok(sessions)
    }
}

impl ProviderController for TerminalHarness {
    fn provider(&self) -> Provider {
        Provider::Terminal
    }

    fn launch_mode(&self) -> LaunchMode {
        if cfg!(unix) {
            LaunchMode::DefaultModel
        } else {
            LaunchMode::Unavailable
        }
    }

    fn launch_presentation(&self) -> LaunchPresentation {
        LaunchPresentation::Foreground
    }

    fn launch_foreground(&self, request: &LaunchRequest) -> Result<ControlOutcome> {
        if request.provider != Provider::Terminal {
            bail!("the Terminal controller cannot launch another provider");
        }
        let now = SystemTime::now();
        let millis = now
            .duration_since(UNIX_EPOCH)
            .unwrap_or_default()
            .as_millis();
        let sequence = TERMINAL_SEQUENCE.fetch_add(1, Ordering::Relaxed);
        let key = format!("terminal:{millis}-{}-{sequence}", std::process::id());
        let record = TerminalRecord {
            key: key.clone(),
            name: terminal_name(&request.prompt),
            cwd: request.cwd.clone(),
            state: SessionState::Working,
            created_at: now,
            updated_at: now,
        };
        self.records
            .lock()
            .map_err(|_| anyhow::anyhow!("terminal registry lock was poisoned"))?
            .insert(key, record.clone());
        self.run_record(&record, true)
    }

    fn inspect(&self, session: &AgentSession) -> Result<String> {
        validate_terminal_session(session)?;
        if session.provider_session_id.starts_with("setup:") {
            return Ok(format!(
                "{}\n\nThis isolated provider setup terminal is backgrounded. Enter or Right resumes it; Ctrl+X stops it.",
                session.name
            ));
        }
        let record = self
            .record(&session.provider_session_id)?
            .context("the managed terminal no longer exists")?;
        Ok(format!(
            "Terminal: {}\nDirectory: {}\nState: {}\n\nTerminal scrollback remains in its native PTY. Enter or Right resumes the exact screen.",
            record.name,
            record.cwd.display(),
            record.state.heading()
        ))
    }

    fn open(&self, session: &AgentSession) -> Result<ControlOutcome> {
        validate_terminal_session(session)?;
        if session.provider_session_id.starts_with("setup:") {
            return match crate::native_session::resume(&session.provider_session_id)? {
                crate::native_session::NativeSessionExit::Backgrounded => Ok(ControlOutcome {
                    message: format!("backgrounded {}; Enter/Right resumes it", session.name),
                    provider_session_hint: Some(session.provider_session_id.clone()),
                }),
                crate::native_session::NativeSessionExit::Exited(status) if status.success() => {
                    Ok(ControlOutcome {
                        message: format!("{} completed", session.name),
                        provider_session_hint: None,
                    })
                }
                crate::native_session::NativeSessionExit::Exited(status) => {
                    bail!("{} exited with status {status}", session.name)
                }
            };
        }
        let record = self
            .record(&session.provider_session_id)?
            .context("the managed terminal no longer exists")?;
        if record.state == SessionState::Completed {
            bail!("this terminal has exited; press Ctrl+X again to delete its row");
        }
        self.run_record(&record, false)
    }

    fn interrupt(&self, session: &AgentSession) -> Result<ControlOutcome> {
        validate_terminal_session(session)?;
        crate::native_session::terminate(&session.provider_session_id)?;
        if !session.provider_session_id.starts_with("setup:") {
            self.set_state(&session.provider_session_id, SessionState::Completed)?;
        }
        Ok(ControlOutcome {
            message: format!(
                "stopped terminal {}; press Ctrl+X again to delete",
                session.name
            ),
            provider_session_hint: Some(session.provider_session_id.clone()),
        })
    }

    fn delete(&self, session: &AgentSession) -> Result<ControlOutcome> {
        validate_terminal_session(session)?;
        let mut records = self
            .records
            .lock()
            .map_err(|_| anyhow::anyhow!("terminal registry lock was poisoned"))?;
        let record = records
            .get(&session.provider_session_id)
            .context("the managed terminal no longer exists")?;
        if record.state != SessionState::Completed {
            bail!("stop this terminal before deleting it");
        }
        records.remove(&session.provider_session_id);
        Ok(ControlOutcome {
            message: format!("deleted terminal {}", session.name),
            provider_session_hint: None,
        })
    }
}

fn terminal_session(record: &TerminalRecord) -> AgentSession {
    let capabilities = if record.state == SessionState::Completed {
        BTreeSet::from([Capability::Inspect, Capability::Delete])
    } else {
        BTreeSet::from([Capability::Inspect, Capability::Interrupt])
    };
    AgentSession {
        id: format!("terminal:host:{}", record.key),
        provider_session_id: record.key.clone(),
        provider: Provider::Terminal,
        runtime: Runtime::Host,
        kind: SessionKind::Managed,
        name: record.name.clone(),
        cwd: record.cwd.clone(),
        state: record.state,
        summary: if record.state == SessionState::Completed {
            "Terminal exited · Ctrl+X deletes this row".into()
        } else {
            "Interactive shell · Enter resumes · ←/→ twice or Shift+Arrow returns to OAV".into()
        },
        raw_state: Some(if record.state == SessionState::Completed {
            "terminal_exited".into()
        } else {
            "terminal_backgrounded".into()
        }),
        pid: None,
        started_at: Some(record.created_at),
        updated_at: Some(record.updated_at),
        pull_requests: None,
        capabilities,
    }
}

fn validate_terminal_session(session: &AgentSession) -> Result<()> {
    if session.provider != Provider::Terminal || session.runtime != Runtime::Host {
        bail!("the Terminal controller does not own this session");
    }
    Ok(())
}

fn configured_shell() -> String {
    std::env::var("SHELL")
        .ok()
        .filter(|shell| {
            !shell.is_empty() && shell.len() <= 4096 && !shell.chars().any(char::is_control)
        })
        .unwrap_or_else(|| "/bin/sh".into())
}

fn terminal_name(prompt: &str) -> String {
    let normalized = prompt.split_whitespace().collect::<Vec<_>>().join(" ");
    let normalized = if normalized.is_empty() {
        "Terminal".to_owned()
    } else {
        normalized
    };
    let mut name = normalized.chars().take(80).collect::<String>();
    if normalized.chars().count() > 80 {
        name.pop();
        name.push('…');
    }
    name
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn terminal_prompt_is_only_a_bounded_display_name() {
        assert_eq!(terminal_name("  release   shell  "), "release shell");
        assert_eq!(terminal_name("   "), "Terminal");
        assert!(terminal_name(&"x".repeat(200)).chars().count() <= 80);
    }

    #[test]
    fn completed_terminal_requires_a_second_delete_action() {
        let harness = TerminalHarness::new();
        let now = SystemTime::now();
        let record = TerminalRecord {
            key: "terminal:test".into(),
            name: "test".into(),
            cwd: PathBuf::from("/work"),
            state: SessionState::Completed,
            created_at: now,
            updated_at: now,
        };
        harness
            .records
            .lock()
            .unwrap()
            .insert(record.key.clone(), record.clone());
        let session = terminal_session(&record);

        assert_eq!(
            session.capabilities,
            BTreeSet::from([Capability::Inspect, Capability::Delete])
        );
        harness.delete(&session).unwrap();
        assert!(harness.records.lock().unwrap().is_empty());
    }

    #[test]
    fn vanished_background_shell_is_reconciled_as_completed() {
        let harness = TerminalHarness::new();
        let now = SystemTime::now();
        harness.records.lock().unwrap().insert(
            "terminal:vanished".into(),
            TerminalRecord {
                key: "terminal:vanished".into(),
                name: "vanished".into(),
                cwd: PathBuf::from("/work"),
                state: SessionState::Working,
                created_at: now,
                updated_at: now,
            },
        );

        let sessions = harness
            .discover(&DiscoveryRequest {
                include_completed: true,
                ..DiscoveryRequest::default()
            })
            .unwrap();

        assert_eq!(sessions.len(), 1);
        assert_eq!(sessions[0].state, SessionState::Completed);
        assert!(sessions[0].capabilities.contains(&Capability::Delete));
    }
}

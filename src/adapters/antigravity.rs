use std::collections::{BTreeMap, BTreeSet};
use std::fs;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};

use anyhow::{bail, Context, Result};

use super::{DiscoveryRequest, SessionSource};
use crate::control::{ControlOutcome, ProviderController};
use crate::domain::{AgentSession, Provider, Runtime, SessionKind, SessionState};

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
    pub fn sandboxed_launch(&self, cwd: &Path) -> Result<AntigravityCommandSpec> {
        require_absolute_workspace(cwd)?;
        Ok(AntigravityCommandSpec {
            program: self.executable.clone(),
            args: vec!["--sandbox".into()],
            current_dir: cwd.to_owned(),
        })
    }
}

pub fn default_antigravity_last_conversations_path() -> Result<PathBuf> {
    let home = std::env::var_os("HOME").context("HOME is not set")?;
    Ok(PathBuf::from(home).join(".gemini/antigravity-cli/cache/last_conversations.json"))
}

pub struct AntigravitySource {
    path: PathBuf,
}

impl AntigravitySource {
    pub fn host(path: PathBuf) -> Self {
        Self { path }
    }

    pub fn default_host() -> Result<Self> {
        Ok(Self::host(default_antigravity_last_conversations_path()?))
    }
}

impl SessionSource for AntigravitySource {
    fn label(&self) -> &str {
        "Antigravity (host)"
    }

    fn discover(&self, request: &DiscoveryRequest) -> Result<Vec<AgentSession>> {
        if !request.include_external {
            return Ok(Vec::new());
        }
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

/// Observe-only controller for documented Antigravity cache entries.
pub struct AntigravityController {
    invocation: AntigravityInvocation,
}

impl AntigravityController {
    pub fn host(executable: impl Into<String>) -> Self {
        Self {
            invocation: AntigravityInvocation::host(executable),
        }
    }
}

impl ProviderController for AntigravityController {
    fn provider(&self) -> Provider {
        Provider::Antigravity
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
        let status = spec
            .command()
            .stdin(Stdio::inherit())
            .stdout(Stdio::inherit())
            .stderr(Stdio::inherit())
            .status()
            .context("failed to open Antigravity conversation")?;
        if !status.success() {
            bail!("Antigravity conversation exited with status {status}");
        }
        Ok(ControlOutcome {
            message: format!("returned from {}", session.name),
            provider_session_hint: None,
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
            .sandboxed_launch(Path::new("/work/repo"))
            .unwrap();
        assert_eq!(launch.args, vec!["--sandbox"]);
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
}

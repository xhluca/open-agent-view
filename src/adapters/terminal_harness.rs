//! Process-local plain terminal sessions managed by Open Agent View.
//!
//! The terminal harness deliberately treats the new-task text as a display
//! name, never as a shell command. OAV opens the user's interactive shell in a
//! private PTY; boundary-double-arrow or Shift+Arrow backgrounds it,
//! Enter/Right resumes it, and Ctrl+X stops
//! only the exact child held by that PTY registry.

use std::collections::{BTreeMap, BTreeSet};
#[cfg(unix)]
use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};
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

pub const SHELL_INSTALL_PREFIX: &str = "install-shell:";
const SUPPORTED_SHELLS: &[(&str, &str)] = &[
    ("bash", "bash"),
    ("zsh", "zsh"),
    ("fish", "fish"),
    ("nu", "nushell"),
    ("xonsh", "xonsh"),
    ("elvish", "elvish"),
];

#[derive(Clone, Debug)]
struct TerminalRecord {
    key: String,
    name: String,
    cwd: PathBuf,
    shell: String,
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
            let shell = record.shell.clone();
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
            LaunchMode::SelectableModel
        } else {
            LaunchMode::Unavailable
        }
    }

    fn launch_presentation(&self) -> LaunchPresentation {
        LaunchPresentation::Foreground
    }

    fn available_models(&self) -> Result<Vec<String>> {
        Ok(shell_choices())
    }

    fn setup_launch_option(&self, option: &str) -> Result<ControlOutcome> {
        install_shell(option)
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
        let shell = resolve_shell(request.model.as_deref())?;
        let record = TerminalRecord {
            key: key.clone(),
            name: terminal_name(&request.prompt),
            cwd: request.cwd.clone(),
            shell,
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
            "Terminal: {}\nShell: {}\nDirectory: {}\nState: {}\n\nTerminal scrollback remains in its native PTY. Enter or Right resumes the exact screen.",
            record.name,
            record.shell,
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
            format!(
                "Terminal exited · {} · Ctrl+X deletes this row",
                shell_label(&record.shell)
            )
        } else {
            format!(
                "Interactive shell · {} · Enter resumes · ←/→ twice or Shift+Arrow returns to OAV",
                shell_label(&record.shell)
            )
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

pub fn is_shell_install_choice(value: &str) -> bool {
    value
        .strip_prefix(SHELL_INSTALL_PREFIX)
        .is_some_and(|shell| supported_shell(shell).is_some())
}

pub fn shell_install_name(value: &str) -> Option<&str> {
    value
        .strip_prefix(SHELL_INSTALL_PREFIX)
        .filter(|shell| supported_shell(shell).is_some())
}

fn supported_shell(name: &str) -> Option<(&'static str, &'static str)> {
    SUPPORTED_SHELLS
        .iter()
        .copied()
        .find(|(shell, _)| *shell == name)
}

fn shell_label(shell: &str) -> &str {
    Path::new(shell)
        .file_name()
        .and_then(|name| name.to_str())
        .unwrap_or(shell)
}

fn executable_path(name: &str) -> Option<PathBuf> {
    if name.contains('/') {
        let path = PathBuf::from(name);
        return executable_file(&path).then_some(path);
    }
    std::env::var_os("PATH").and_then(|value| {
        std::env::split_paths(&value)
            .map(|directory| directory.join(name))
            .find(|candidate| executable_file(candidate))
    })
}

fn executable_file(path: &Path) -> bool {
    let Ok(metadata) = path.metadata() else {
        return false;
    };
    if !metadata.is_file() {
        return false;
    }
    #[cfg(unix)]
    return metadata.permissions().mode() & 0o111 != 0;
    #[cfg(not(unix))]
    true
}

fn shell_choices() -> Vec<String> {
    let configured = configured_shell();
    let configured_name = shell_label(&configured).to_owned();
    let mut choices = Vec::new();
    if executable_path(&configured).is_some() {
        choices.push(configured_name.clone());
    }
    for (shell, _) in SUPPORTED_SHELLS {
        if *shell == configured_name {
            continue;
        }
        if executable_path(shell).is_some() {
            choices.push((*shell).to_owned());
        } else {
            choices.push(format!("{SHELL_INSTALL_PREFIX}{shell}"));
        }
    }
    choices
}

fn resolve_shell(selection: Option<&str>) -> Result<String> {
    let default_shell = configured_shell();
    let requested = selection.unwrap_or_else(|| shell_label(&default_shell));
    if is_shell_install_choice(requested) {
        bail!("select the install action before launching this shell");
    }
    if requested.len() > 64
        || requested.is_empty()
        || requested.contains('/')
        || requested
            .chars()
            .any(|character| !character.is_ascii_alphanumeric() && character != '-')
    {
        bail!("shell names must be a supported executable name");
    }
    if supported_shell(requested).is_none() && requested != shell_label(&default_shell) {
        bail!("unsupported shell {requested}; use /shell to see supported shells");
    }
    if requested == shell_label(&default_shell) {
        return executable_path(&default_shell)
            .map(|path| path.to_string_lossy().into_owned())
            .context("the configured SHELL executable is unavailable");
    }
    executable_path(requested)
        .map(|path| path.to_string_lossy().into_owned())
        .with_context(|| format!("{requested} is not installed; use /shell to install it"))
}

#[derive(Debug, Eq, PartialEq)]
struct ShellInstallPlan {
    program: PathBuf,
    args: Vec<String>,
}

fn shell_install_plan(shell: &str) -> Result<ShellInstallPlan> {
    let (_, package) = supported_shell(shell).context("unsupported shell installation choice")?;
    let manager = if let Some(brew) = executable_path("brew") {
        brew
    } else if let Some(apt) = executable_path("apt-get") {
        apt
    } else if let Some(dnf) = executable_path("dnf") {
        dnf
    } else if let Some(pacman) = executable_path("pacman") {
        pacman
    } else if let Some(zypper) = executable_path("zypper") {
        zypper
    } else {
        bail!("no supported package manager was found (brew, apt-get, dnf, pacman, or zypper)");
    };
    let manager_name = manager
        .file_name()
        .and_then(|name| name.to_str())
        .context("package manager path has no executable name")?;
    let mut args = package_manager_args(manager_name, package)?;

    #[cfg(unix)]
    if manager_name != "brew" && unsafe { libc::geteuid() } != 0 {
        let sudo = executable_path("sudo")
            .context("installing this shell requires root access, but sudo was not found")?;
        args.insert(0, manager.to_string_lossy().into_owned());
        return Ok(ShellInstallPlan {
            program: sudo,
            args,
        });
    }
    Ok(ShellInstallPlan {
        program: manager,
        args,
    })
}

fn package_manager_args(manager: &str, package: &str) -> Result<Vec<String>> {
    let verb = match manager {
        "brew" | "apt-get" | "dnf" | "zypper" => "install",
        "pacman" => "-S",
        _ => bail!("unsupported package manager {manager}"),
    };
    Ok(vec![verb.to_owned(), package.to_owned()])
}

fn install_shell(option: &str) -> Result<ControlOutcome> {
    let shell = shell_install_name(option).unwrap_or(option);
    supported_shell(shell).context("unsupported shell installation choice")?;
    if executable_path(shell).is_some() {
        return Ok(ControlOutcome {
            message: format!("{shell} is already installed"),
            provider_session_hint: None,
        });
    }
    let plan = shell_install_plan(shell)?;
    let mut command = Command::new(&plan.program);
    command.args(&plan.args);
    match crate::native_session::run(command, &format!("setup:shell:{shell}"))? {
        crate::native_session::NativeSessionExit::Backgrounded => Ok(ControlOutcome {
            message: format!("backgrounded {shell} installation; resume its Terminal setup row"),
            provider_session_hint: None,
        }),
        crate::native_session::NativeSessionExit::Exited(status) if status.success() => {
            if executable_path(shell).is_none() {
                bail!("{shell} installer completed, but the shell is not visible in PATH yet");
            }
            Ok(ControlOutcome {
                message: format!("installed {shell}; reopen /shell to select it"),
                provider_session_hint: None,
            })
        }
        crate::native_session::NativeSessionExit::Exited(status) => {
            bail!("{shell} installer exited with status {status}")
        }
    }
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
    fn shell_install_markers_are_bounded_to_the_supported_catalog() {
        assert!(is_shell_install_choice("install-shell:fish"));
        assert_eq!(shell_install_name("install-shell:nu"), Some("nu"));
        assert!(!is_shell_install_choice("install-shell:made-up"));
        assert!(!is_shell_install_choice("bash"));
        assert!(resolve_shell(Some("made-up"))
            .unwrap_err()
            .to_string()
            .contains("unsupported"));
    }

    #[test]
    fn shell_catalog_distinguishes_installed_entries_from_install_actions() {
        let choices = shell_choices();
        assert!(!choices.is_empty());
        assert!(choices.iter().all(|choice| {
            is_shell_install_choice(choice) || executable_path(choice).is_some()
        }));
        for (shell, _) in SUPPORTED_SHELLS {
            assert!(choices.iter().any(|choice| {
                choice == shell || choice == &format!("{SHELL_INSTALL_PREFIX}{shell}")
            }));
        }
    }

    #[test]
    fn shell_install_plans_use_argument_arrays_and_exact_package_names() {
        assert_eq!(
            package_manager_args("apt-get", "fish").unwrap(),
            vec!["install", "fish"]
        );
        assert_eq!(
            package_manager_args("pacman", "nushell").unwrap(),
            vec!["-S", "nushell"]
        );
        assert!(package_manager_args("sh", "fish").is_err());
        assert_eq!(supported_shell("nu"), Some(("nu", "nushell")));
    }

    #[test]
    fn completed_terminal_requires_a_second_delete_action() {
        let harness = TerminalHarness::new();
        let now = SystemTime::now();
        let record = TerminalRecord {
            key: "terminal:test".into(),
            name: "test".into(),
            cwd: PathBuf::from("/work"),
            shell: "/bin/sh".into(),
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
                shell: "/bin/sh".into(),
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

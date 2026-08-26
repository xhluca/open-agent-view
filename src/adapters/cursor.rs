use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::Arc;

use anyhow::{bail, Context, Result};
use serde::Deserialize;
use serde_json::Value;

use super::cursor_managed::CursorSupervisor;
use crate::control::{
    run_native_authentication, ControlOutcome, LaunchMode, LaunchPresentation, LaunchRequest,
    ProviderController,
};
use crate::domain::{AgentSession, Provider, Runtime, SessionSnapshot};

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct CursorCommandSpec {
    pub program: String,
    pub args: Vec<String>,
    pub current_dir: PathBuf,
}

impl CursorCommandSpec {
    pub fn command(&self) -> Command {
        let mut command = Command::new(&self.program);
        command.args(&self.args).current_dir(&self.current_dir);
        command
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct CursorInvocation {
    executable: String,
}

impl CursorInvocation {
    pub fn host(executable: impl Into<String>) -> Self {
        Self {
            executable: executable.into(),
        }
    }

    /// Build Cursor's documented native resume command without a shell.
    pub fn resume(&self, session_id: &str, cwd: &Path) -> Result<CursorCommandSpec> {
        require_session_id(session_id)?;
        require_absolute_cwd(cwd)?;
        Ok(CursorCommandSpec {
            program: self.executable.clone(),
            args: vec![
                "--resume".into(),
                session_id.into(),
                "--workspace".into(),
                cwd.display().to_string(),
            ],
            current_dir: cwd.to_owned(),
        })
    }

    /// Build an interactive first turn for a preallocated chat. Unlike the
    /// managed background transport this intentionally omits `--print` so the
    /// user sees Cursor's native interface immediately.
    pub fn resume_with_prompt(
        &self,
        session_id: &str,
        cwd: &Path,
        prompt: &str,
        model: Option<&str>,
    ) -> Result<CursorCommandSpec> {
        require_session_id(session_id)?;
        require_absolute_cwd(cwd)?;
        if prompt.trim().is_empty() {
            bail!("Cursor prompt must not be empty");
        }
        let mut spec = self.resume(session_id, cwd)?;
        if let Some(model) = model {
            require_model(model)?;
            spec.args.extend(["--model".into(), model.into()]);
        }
        spec.args.push(prompt.trim().into());
        Ok(spec)
    }

    /// Build the documented empty-chat allocator used by managed integrations.
    pub fn create_chat(&self, cwd: &Path) -> Result<CursorCommandSpec> {
        self.create_chat_with_model(cwd, None)
    }

    /// Allocate the chat with its selected model already persisted.
    ///
    /// Cursor applies a model flag passed only to a later `--resume` process
    /// to that process, but a preallocated chat can retain the account's prior
    /// named model on its following turn. Supplying the documented global
    /// option to `create-chat` makes the session itself use the requested
    /// choice, including `auto` on plans that reject named models.
    pub fn create_chat_with_model(
        &self,
        cwd: &Path,
        model: Option<&str>,
    ) -> Result<CursorCommandSpec> {
        require_absolute_cwd(cwd)?;
        let mut args = vec!["create-chat".into()];
        if let Some(model) = model {
            require_model(model)?;
            args.extend(["--model".into(), model.into()]);
        }
        Ok(CursorCommandSpec {
            program: self.executable.clone(),
            args,
            current_dir: cwd.to_owned(),
        })
    }

    /// Build a single-turn managed run. The caller must retain the child and
    /// NDJSON stream; this function never adds `--force` or `--yolo`.
    pub fn print_turn(
        &self,
        session_id: &str,
        cwd: &Path,
        prompt: &str,
        model: Option<&str>,
    ) -> Result<CursorCommandSpec> {
        require_session_id(session_id)?;
        require_absolute_cwd(cwd)?;
        if prompt.trim().is_empty() {
            bail!("Cursor prompt must not be empty");
        }
        let mut args = vec![
            "--resume".into(),
            session_id.into(),
            "--print".into(),
            "--output-format".into(),
            "stream-json".into(),
            "--workspace".into(),
            cwd.display().to_string(),
        ];
        if let Some(model) = model {
            require_model(model)?;
            args.extend(["--model".into(), model.into()]);
        }
        args.push(prompt.trim().into());
        Ok(CursorCommandSpec {
            program: self.executable.clone(),
            args,
            current_dir: cwd.to_owned(),
        })
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum CursorStreamEvent {
    Initialized {
        session_id: String,
        cwd: PathBuf,
        model: Option<String>,
    },
    AssistantText {
        session_id: String,
        text: String,
    },
    ToolStarted {
        session_id: String,
        call_id: String,
    },
    ToolCompleted {
        session_id: String,
        call_id: String,
    },
    Finished {
        session_id: String,
        result: String,
        is_error: bool,
    },
    Other(Value),
}

pub fn parse_cursor_stream_event(line: &str) -> Result<CursorStreamEvent> {
    let value: Value = serde_json::from_str(line).context("invalid Cursor stream-json event")?;
    let event_type = value.get("type").and_then(Value::as_str).unwrap_or("");
    let subtype = value.get("subtype").and_then(Value::as_str).unwrap_or("");
    match (event_type, subtype) {
        ("system", "init") => Ok(CursorStreamEvent::Initialized {
            session_id: required_string(&value, "session_id")?,
            cwd: PathBuf::from(required_string(&value, "cwd")?),
            model: value
                .get("model")
                .and_then(Value::as_str)
                .map(str::to_owned),
        }),
        ("assistant", _) => Ok(CursorStreamEvent::AssistantText {
            session_id: required_string(&value, "session_id")?,
            text: assistant_text(&value)?,
        }),
        ("tool_call", "started") => Ok(CursorStreamEvent::ToolStarted {
            session_id: required_string(&value, "session_id")?,
            call_id: required_string(&value, "call_id")?,
        }),
        ("tool_call", "completed") => Ok(CursorStreamEvent::ToolCompleted {
            session_id: required_string(&value, "session_id")?,
            call_id: required_string(&value, "call_id")?,
        }),
        ("result", _) => Ok(CursorStreamEvent::Finished {
            session_id: required_string(&value, "session_id")?,
            result: value
                .get("result")
                .and_then(Value::as_str)
                .unwrap_or_default()
                .to_owned(),
            is_error: value
                .get("is_error")
                .and_then(Value::as_bool)
                .unwrap_or(subtype != "success"),
        }),
        _ => Ok(CursorStreamEvent::Other(value)),
    }
}

pub fn parse_cursor_chat_id(output: &str) -> Result<String> {
    let id = output.trim();
    require_session_id(id)?;
    if id.split_whitespace().count() != 1 {
        bail!("Cursor create-chat returned more than one token");
    }
    Ok(id.to_owned())
}

/// Observe-only controller for sessions discovered outside Open Agent View.
///
/// Cursor has no documented machine-readable global list or live-control API,
/// so this controller intentionally offers native resume only.
pub struct CursorController {
    invocation: CursorInvocation,
    supervisor: Option<Arc<CursorSupervisor>>,
}

impl CursorController {
    pub fn host(executable: impl Into<String>) -> Self {
        Self {
            invocation: CursorInvocation::host(executable),
            supervisor: None,
        }
    }

    /// Control both native external sessions and the exact processes launched
    /// through this supervisor. Only the latter gain inline capabilities.
    pub fn managed(supervisor: Arc<CursorSupervisor>) -> Self {
        Self {
            invocation: CursorInvocation::host(supervisor.executable()),
            supervisor: Some(supervisor),
        }
    }
}

impl ProviderController for CursorController {
    fn provider(&self) -> Provider {
        Provider::Cursor
    }

    fn launch_mode(&self) -> LaunchMode {
        if self.supervisor.is_some() {
            LaunchMode::SelectableModel
        } else {
            LaunchMode::Unavailable
        }
    }

    fn launch_presentation(&self) -> LaunchPresentation {
        // Account/model preflight and create-chat run on the dashboard worker.
        // Once the exact returned row appears, the terminal opens it natively.
        LaunchPresentation::DeferredForeground
    }

    fn available_models(&self) -> Result<Vec<String>> {
        self.managed_supervisor()?.available_models()
    }

    fn supports_authentication(&self) -> bool {
        true
    }

    fn authenticate(&self) -> Result<ControlOutcome> {
        run_native_authentication(&self.invocation.executable, &["login"], Provider::Cursor)
    }

    fn enrich(&self, snapshot: &mut SessionSnapshot) {
        if let Some(supervisor) = &self.supervisor {
            supervisor.enrich(snapshot);
        }
    }

    fn launch(&self, request: &LaunchRequest) -> Result<ControlOutcome> {
        if request.provider != Provider::Cursor {
            bail!("the Cursor controller cannot launch another provider");
        }
        let supervisor = self
            .supervisor
            .as_ref()
            .context("managed Cursor launch is not configured")?;
        let session_id = supervisor.allocate_chat_with_model(
            &request.prompt,
            &request.cwd,
            request.model.as_deref(),
        )?;
        Ok(ControlOutcome {
            message: format!("launched managed Cursor session {session_id}"),
            provider_session_hint: Some(session_id),
        })
    }

    fn launch_foreground(&self, request: &LaunchRequest) -> Result<ControlOutcome> {
        if request.provider != Provider::Cursor {
            bail!("the Cursor controller cannot launch another provider");
        }
        let supervisor = self.managed_supervisor()?;
        let session_id = supervisor.allocate_chat_with_model(
            &request.prompt,
            &request.cwd,
            request.model.as_deref(),
        )?;
        let spec = self.invocation.resume_with_prompt(
            &session_id,
            &request.cwd,
            &request.prompt,
            request.model.as_deref(),
        )?;
        let key = format!("cursor:host:{session_id}");
        match crate::native_session::run(spec.command(), &key)? {
            crate::native_session::NativeSessionExit::Backgrounded => {
                supervisor.mark_native_opened(&session_id)?;
                Ok(ControlOutcome {
                    message: format!(
                        "backgrounded Cursor session {session_id}; Enter/Right resumes it"
                    ),
                    provider_session_hint: Some(session_id),
                })
            }
            crate::native_session::NativeSessionExit::Exited(status) if status.success() => {
                supervisor.mark_native_opened(&session_id)?;
                Ok(ControlOutcome {
                    message: format!("returned from Cursor session {session_id}"),
                    provider_session_hint: Some(session_id),
                })
            }
            crate::native_session::NativeSessionExit::Exited(status) => {
                supervisor.mark_native_opened(&session_id)?;
                bail!("Cursor session exited with status {status}")
            }
        }
    }

    fn interrupt(&self, session: &AgentSession) -> Result<ControlOutcome> {
        self.managed_supervisor()?.interrupt(session)?;
        Ok(ControlOutcome {
            message: format!("interrupted {}", session.name),
            provider_session_hint: None,
        })
    }

    fn inspect(&self, session: &AgentSession) -> Result<String> {
        self.managed_supervisor()?.inspect(session)
    }

    fn reply(&self, session: &AgentSession, prompt: &str) -> Result<ControlOutcome> {
        self.managed_supervisor()?.reply(session, prompt)?;
        Ok(ControlOutcome {
            message: format!("sent a new turn to {}", session.name),
            provider_session_hint: None,
        })
    }

    fn open(&self, session: &AgentSession) -> Result<ControlOutcome> {
        if session.provider != Provider::Cursor || session.runtime != Runtime::Host {
            bail!("the Cursor host controller cannot open this session");
        }
        if let Some(supervisor) = &self.supervisor {
            if supervisor.owns(session) && supervisor.is_running(session)? {
                bail!("interrupt the active managed Cursor turn before opening it natively");
            }
        }
        let pending_native = self
            .supervisor
            .as_ref()
            .map(|supervisor| supervisor.pending_native_launch(session))
            .transpose()?
            .flatten();
        let spec = if let Some((prompt, model)) = pending_native {
            self.invocation.resume_with_prompt(
                &session.provider_session_id,
                &session.cwd,
                &prompt,
                model.as_deref(),
            )?
        } else {
            self.invocation
                .resume(&session.provider_session_id, &session.cwd)?
        };
        match crate::native_session::run(spec.command(), &session.id)? {
            crate::native_session::NativeSessionExit::Backgrounded => {
                if let Some(supervisor) = &self.supervisor {
                    if supervisor.owns(session) {
                        supervisor.mark_native_opened(&session.provider_session_id)?;
                    }
                }
                Ok(ControlOutcome {
                    message: format!("backgrounded {}; Enter/Right resumes it", session.name),
                    provider_session_hint: Some(session.provider_session_id.clone()),
                })
            }
            crate::native_session::NativeSessionExit::Exited(status) if status.success() => {
                if let Some(supervisor) = &self.supervisor {
                    if supervisor.owns(session) {
                        supervisor.mark_native_opened(&session.provider_session_id)?;
                    }
                }
                Ok(ControlOutcome {
                    message: format!("returned from {}", session.name),
                    provider_session_hint: None,
                })
            }
            crate::native_session::NativeSessionExit::Exited(status) => {
                bail!("Cursor session exited with status {status}")
            }
        }
    }
}

impl CursorController {
    fn managed_supervisor(&self) -> Result<&CursorSupervisor> {
        self.supervisor
            .as_deref()
            .context("managed Cursor control is not configured")
    }
}

fn required_string(value: &Value, field: &str) -> Result<String> {
    value
        .get(field)
        .and_then(Value::as_str)
        .filter(|value| !value.is_empty())
        .map(str::to_owned)
        .with_context(|| format!("Cursor event omitted {field}"))
}

fn assistant_text(value: &Value) -> Result<String> {
    #[derive(Deserialize)]
    struct Message {
        content: Vec<Content>,
    }
    #[derive(Deserialize)]
    struct Content {
        #[serde(rename = "type")]
        kind: String,
        text: Option<String>,
    }
    let message: Message = serde_json::from_value(
        value
            .get("message")
            .cloned()
            .context("Cursor assistant event omitted message")?,
    )?;
    Ok(message
        .content
        .into_iter()
        .filter(|content| content.kind == "text")
        .filter_map(|content| content.text)
        .collect())
}

fn require_session_id(session_id: &str) -> Result<()> {
    if session_id.is_empty()
        || session_id.chars().any(char::is_control)
        || session_id.chars().any(char::is_whitespace)
    {
        bail!("Cursor session ID is empty or contains whitespace/control characters");
    }
    Ok(())
}

fn require_absolute_cwd(cwd: &Path) -> Result<()> {
    if !cwd.is_absolute() {
        bail!("Cursor workspace must be absolute");
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
        bail!("Cursor model must contain 1 to 128 non-whitespace bytes");
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn builds_shell_free_resume_and_safe_print_invocations() {
        let invocation = CursorInvocation::host("cursor-agent");
        assert_eq!(
            invocation
                .resume("chat-id", Path::new("/work/repo"))
                .unwrap(),
            CursorCommandSpec {
                program: "cursor-agent".into(),
                args: vec![
                    "--resume".into(),
                    "chat-id".into(),
                    "--workspace".into(),
                    "/work/repo".into(),
                ],
                current_dir: "/work/repo".into(),
            }
        );
        let print = invocation
            .print_turn(
                "chat-id",
                Path::new("/work/repo"),
                "check tests",
                Some("auto"),
            )
            .unwrap();
        assert!(print.args.contains(&"stream-json".into()));
        assert!(print
            .args
            .windows(2)
            .any(|args| args == ["--model", "auto"]));
        assert!(!print
            .args
            .iter()
            .any(|arg| arg == "--force" || arg == "--yolo"));
        let foreground = invocation
            .resume_with_prompt(
                "chat-id",
                Path::new("/work/repo"),
                "check interactively",
                Some("auto"),
            )
            .unwrap();
        assert_eq!(
            foreground.args,
            [
                "--resume",
                "chat-id",
                "--workspace",
                "/work/repo",
                "--model",
                "auto",
                "check interactively",
            ]
        );
        assert!(!foreground.args.iter().any(|arg| arg == "--print"));
        assert_eq!(
            invocation
                .create_chat_with_model(Path::new("/work/repo"), Some("auto"))
                .unwrap()
                .args,
            ["create-chat", "--model", "auto"]
        );
    }

    #[test]
    fn parses_documented_stream_events() {
        let init = parse_cursor_stream_event(
            r#"{"type":"system","subtype":"init","cwd":"/work","session_id":"abc","model":"GPT-5"}"#,
        )
        .unwrap();
        assert_eq!(
            init,
            CursorStreamEvent::Initialized {
                session_id: "abc".into(),
                cwd: "/work".into(),
                model: Some("GPT-5".into()),
            }
        );

        let assistant = parse_cursor_stream_event(
            r#"{"type":"assistant","message":{"role":"assistant","content":[{"type":"text","text":"done"}]},"session_id":"abc"}"#,
        )
        .unwrap();
        assert_eq!(
            assistant,
            CursorStreamEvent::AssistantText {
                session_id: "abc".into(),
                text: "done".into(),
            }
        );
    }

    #[test]
    fn rejects_unsafe_ids_and_relative_workspaces() {
        let invocation = CursorInvocation::host("cursor-agent");
        assert!(invocation.resume("bad\nid", Path::new("/work")).is_err());
        assert!(invocation.resume("id", Path::new("relative")).is_err());
        assert!(parse_cursor_chat_id("one two").is_err());
    }
}

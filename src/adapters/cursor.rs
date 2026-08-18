use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};

use anyhow::{bail, Context, Result};
use serde::Deserialize;
use serde_json::Value;

use crate::control::{ControlOutcome, ProviderController};
use crate::domain::{AgentSession, Provider, Runtime};

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

    /// Build the documented empty-chat allocator used by managed integrations.
    pub fn create_chat(&self, cwd: &Path) -> Result<CursorCommandSpec> {
        require_absolute_cwd(cwd)?;
        Ok(CursorCommandSpec {
            program: self.executable.clone(),
            args: vec!["create-chat".into()],
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
    ) -> Result<CursorCommandSpec> {
        require_session_id(session_id)?;
        require_absolute_cwd(cwd)?;
        if prompt.trim().is_empty() {
            bail!("Cursor prompt must not be empty");
        }
        Ok(CursorCommandSpec {
            program: self.executable.clone(),
            args: vec![
                "--resume".into(),
                session_id.into(),
                "--print".into(),
                "--output-format".into(),
                "stream-json".into(),
                "--workspace".into(),
                cwd.display().to_string(),
                prompt.trim().into(),
            ],
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
            model: value.get("model").and_then(Value::as_str).map(str::to_owned),
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
}

impl CursorController {
    pub fn host(executable: impl Into<String>) -> Self {
        Self {
            invocation: CursorInvocation::host(executable),
        }
    }
}

impl ProviderController for CursorController {
    fn provider(&self) -> Provider {
        Provider::Cursor
    }

    fn open(&self, session: &AgentSession) -> Result<ControlOutcome> {
        if session.provider != Provider::Cursor || session.runtime != Runtime::Host {
            bail!("the Cursor host controller cannot open this session");
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
            .context("failed to open Cursor session")?;
        if !status.success() {
            bail!("Cursor session exited with status {status}");
        }
        Ok(ControlOutcome {
            message: format!("returned from {}", session.name),
            provider_session_hint: None,
        })
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn builds_shell_free_resume_and_safe_print_invocations() {
        let invocation = CursorInvocation::host("cursor-agent");
        assert_eq!(
            invocation.resume("chat-id", Path::new("/work/repo")).unwrap(),
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
            .print_turn("chat-id", Path::new("/work/repo"), "check tests")
            .unwrap();
        assert!(print.args.contains(&"stream-json".into()));
        assert!(!print.args.iter().any(|arg| arg == "--force" || arg == "--yolo"));
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

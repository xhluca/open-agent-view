use std::collections::BTreeSet;
use std::fs::{self, File};
use std::io::{BufRead, BufReader};
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::time::SystemTime;

use anyhow::{bail, Context, Result};
use serde::Deserialize;
use serde_json::Value;

use super::{DiscoveryRequest, SessionSource};
use crate::control::{ControlOutcome, ProviderController};
use crate::domain::{AgentSession, Capability, Provider, Runtime, SessionKind, SessionState};

/// Read-only discovery of Pi's documented JSONL session store.
pub struct PiSource {
    session_dir: PathBuf,
}

/// Read-only history control plus native TUI resume for Pi.
///
/// Pi's live RPC transport is stdio-only. This controller intentionally does
/// not claim reply or interrupt authority over unrelated processes.
pub struct PiController {
    executable: String,
    source: PiSource,
}

impl PiController {
    pub fn host(executable: impl Into<String>, session_dir: impl Into<PathBuf>) -> Self {
        Self {
            executable: executable.into(),
            source: PiSource::host(session_dir),
        }
    }

    pub fn host_default(executable: impl Into<String>) -> Result<Self> {
        Ok(Self::host(executable, default_pi_session_dir()?))
    }
}

impl ProviderController for PiController {
    fn provider(&self) -> Provider {
        Provider::Pi
    }

    fn inspect(&self, session: &AgentSession) -> Result<String> {
        self.source.inspect(session)
    }

    fn open(&self, session: &AgentSession) -> Result<ControlOutcome> {
        if session.provider != Provider::Pi || session.runtime != Runtime::Host {
            bail!("the host Pi controller does not own this runtime");
        }
        let status = Command::new(&self.executable)
            .args([
                "--session",
                &session.provider_session_id,
                "--session-dir",
            ])
            .arg(&self.source.session_dir)
            .current_dir(&session.cwd)
            .stdin(Stdio::inherit())
            .stdout(Stdio::inherit())
            .stderr(Stdio::inherit())
            .status()
            .context("failed to open the Pi session")?;
        if !status.success() {
            bail!("Pi session exited with status {status}");
        }
        Ok(ControlOutcome {
            message: format!("returned from Pi session {}", session.name),
            provider_session_hint: Some(session.provider_session_id.clone()),
        })
    }
}

impl PiSource {
    pub fn host(session_dir: impl Into<PathBuf>) -> Self {
        Self {
            session_dir: session_dir.into(),
        }
    }

    pub fn host_default() -> Result<Self> {
        Ok(Self::host(default_pi_session_dir()?))
    }

    /// Render a persisted Pi transcript without attaching to or changing it.
    pub fn inspect(&self, session: &AgentSession) -> Result<String> {
        if session.provider != Provider::Pi || session.runtime != Runtime::Host {
            bail!("the Pi source does not own this provider runtime");
        }
        let path = find_pi_session_file(&self.session_dir, &session.provider_session_id)?
            .context("the Pi session file is no longer present")?;
        let input = fs::read_to_string(&path)
            .with_context(|| format!("failed to read Pi session {}", path.display()))?;
        render_pi_transcript(&input)
            .with_context(|| format!("invalid Pi session {}", path.display()))
    }
}

impl SessionSource for PiSource {
    fn label(&self) -> &str {
        "Pi (host)"
    }

    fn discover(&self, request: &DiscoveryRequest) -> Result<Vec<AgentSession>> {
        if !self.session_dir.exists() {
            return Ok(Vec::new());
        }

        let mut files = Vec::new();
        collect_jsonl_files(&self.session_dir, &mut files)?;
        if files.len() > 10_000 {
            bail!("Pi session store exceeded the 10,000-file safety cap");
        }

        let mut sessions = Vec::new();
        for file in files {
            let metadata = fs::metadata(&file)
                .with_context(|| format!("failed to inspect Pi session {}", file.display()))?;
            let started_at = metadata.created().ok();
            let updated_at = metadata.modified().ok();
            let session = parse_pi_session_file(&file, started_at, updated_at)?;
            if (request.include_completed || session.state != SessionState::Completed)
                && request
                    .cwd
                    .as_ref()
                    .map(|cwd| session.cwd.starts_with(cwd))
                    .unwrap_or(true)
            {
                sessions.push(session);
            }
        }
        Ok(sessions)
    }
}

pub fn default_pi_session_dir() -> Result<PathBuf> {
    if let Some(path) = std::env::var_os("PI_CODING_AGENT_SESSION_DIR") {
        return Ok(PathBuf::from(path));
    }
    if let Some(path) = std::env::var_os("PI_CODING_AGENT_DIR") {
        return Ok(PathBuf::from(path).join("sessions"));
    }
    let home = std::env::var_os("HOME").context("HOME is not set")?;
    Ok(PathBuf::from(home).join(".pi/agent/sessions"))
}

fn collect_jsonl_files(directory: &Path, files: &mut Vec<PathBuf>) -> Result<()> {
    let entries = fs::read_dir(directory)
        .with_context(|| format!("failed to read Pi session directory {}", directory.display()))?;
    for entry in entries {
        let entry = entry?;
        let file_type = entry.file_type()?;
        // Do not follow symlinks out of the configured store.
        if file_type.is_dir() {
            collect_jsonl_files(&entry.path(), files)?;
        } else if file_type.is_file()
            && entry.path().extension().and_then(|value| value.to_str()) == Some("jsonl")
        {
            files.push(entry.path());
        }
    }
    Ok(())
}

fn find_pi_session_file(directory: &Path, session_id: &str) -> Result<Option<PathBuf>> {
    if !directory.exists() {
        return Ok(None);
    }
    let mut files = Vec::new();
    collect_jsonl_files(directory, &mut files)?;
    if files.len() > 10_000 {
        bail!("Pi session store exceeded the 10,000-file safety cap");
    }
    for path in files {
        let file = File::open(&path)?;
        let mut lines = BufReader::new(file).lines();
        let Some(line) = lines.next() else {
            continue;
        };
        let Ok(value) = serde_json::from_str::<Value>(&line?) else {
            continue;
        };
        if value.get("type").and_then(Value::as_str) == Some("session")
            && value.get("id").and_then(Value::as_str) == Some(session_id)
        {
            return Ok(Some(path));
        }
    }
    Ok(None)
}

fn parse_pi_session_file(
    path: &Path,
    started_at: Option<SystemTime>,
    updated_at: Option<SystemTime>,
) -> Result<AgentSession> {
    let file = File::open(path)
        .with_context(|| format!("failed to open Pi session {}", path.display()))?;
    let mut input = String::new();
    for line in BufReader::new(file).lines() {
        input.push_str(&line?);
        input.push('\n');
    }
    parse_pi_session(&input, path, started_at, updated_at)
        .with_context(|| format!("invalid Pi session {}", path.display()))
}

#[derive(Debug, Deserialize)]
struct PiHeader {
    id: String,
    cwd: PathBuf,
}

pub fn parse_pi_session(
    input: &str,
    path: &Path,
    started_at: Option<SystemTime>,
    updated_at: Option<SystemTime>,
) -> Result<AgentSession> {
    let mut lines = input.lines().filter(|line| !line.trim().is_empty());
    let first = lines.next().context("missing Pi session header")?;
    let header_value: Value = serde_json::from_str(first).context("invalid Pi session header JSON")?;
    if header_value.get("type").and_then(Value::as_str) != Some("session") {
        bail!("first Pi JSONL record is not a session header");
    }
    let header: PiHeader = serde_json::from_value(header_value)?;

    let mut name: Option<String> = None;
    let mut first_user_text: Option<String> = None;
    let mut latest_assistant_text: Option<String> = None;
    let mut last_role: Option<String> = None;
    let mut last_assistant_stop: Option<String> = None;
    for (index, line) in lines.enumerate() {
        let value: Value = serde_json::from_str(line)
            .with_context(|| format!("invalid JSON on Pi session line {}", index + 2))?;
        match value.get("type").and_then(Value::as_str) {
            Some("session_info") => {
                if let Some(value) = value.get("name").and_then(Value::as_str) {
                    name = Some(value.to_owned());
                }
            }
            Some("message") => {
                let Some(message) = value.get("message") else {
                    continue;
                };
                let role = message.get("role").and_then(Value::as_str);
                if let Some(role) = role {
                    last_role = Some(role.to_owned());
                }
                let text = message.get("content").and_then(message_text);
                match role {
                    Some("user") if first_user_text.is_none() => first_user_text = text,
                    Some("assistant") => {
                        if text.as_ref().map(|value| !value.is_empty()).unwrap_or(false) {
                            latest_assistant_text = text;
                        }
                        last_assistant_stop = message
                            .get("stopReason")
                            .and_then(Value::as_str)
                            .map(ToOwned::to_owned);
                    }
                    _ => {}
                }
            }
            _ => {}
        }
    }

    let fallback_name = first_user_text
        .as_deref()
        .map(|text| truncate_chars(text, 48))
        .filter(|text| !text.is_empty())
        .or_else(|| {
            path.file_stem()
                .and_then(|value| value.to_str())
                .map(ToOwned::to_owned)
        })
        .unwrap_or_else(|| format!("pi-{}", &header.id[..header.id.len().min(8)]));
    let state = if last_role.as_deref() == Some("assistant")
        && matches!(last_assistant_stop.as_deref(), Some("stop" | "length" | "error" | "aborted"))
    {
        SessionState::Completed
    } else {
        // A JSONL file alone cannot prove that an interactive or RPC process is
        // still attached, nor can it safely accept input on that process.
        SessionState::Unknown
    };

    Ok(AgentSession {
        id: format!("pi:host:{}", header.id),
        provider_session_id: header.id,
        provider: Provider::Pi,
        runtime: Runtime::Host,
        kind: SessionKind::Unknown,
        name: name.unwrap_or(fallback_name),
        cwd: header.cwd,
        state,
        summary: latest_assistant_text
            .as_deref()
            .map(|text| truncate_chars(text, 160))
            .or(first_user_text)
            .unwrap_or_default(),
        raw_state: Some(if state == SessionState::Completed {
            "persisted_complete".into()
        } else {
            "persisted_unknown".into()
        }),
        pid: None,
        started_at,
        updated_at,
        pull_requests: None,
        capabilities: BTreeSet::from([Capability::Inspect]),
    })
}

fn render_pi_transcript(input: &str) -> Result<String> {
    let mut transcript = Vec::new();
    for (index, line) in input.lines().enumerate() {
        if line.trim().is_empty() {
            continue;
        }
        let value: Value = serde_json::from_str(line)
            .with_context(|| format!("invalid JSON on Pi session line {}", index + 1))?;
        if value.get("type").and_then(Value::as_str) != Some("message") {
            continue;
        }
        let Some(message) = value.get("message") else {
            continue;
        };
        let role = message.get("role").and_then(Value::as_str).unwrap_or("event");
        let text = match role {
            "bashExecution" => message
                .get("output")
                .and_then(Value::as_str)
                .map(|output| {
                    format!(
                        "$ {}\n{}",
                        message
                            .get("command")
                            .and_then(Value::as_str)
                            .unwrap_or_default(),
                        output
                    )
                }),
            _ => message.get("content").and_then(message_text),
        };
        if let Some(text) = text.filter(|value| !value.trim().is_empty()) {
            transcript.push(format!("{}: {}", capitalize(role), text.trim()));
        }
    }
    Ok(limit_transcript(if transcript.is_empty() {
        "No text messages are available in this Pi session.".into()
    } else {
        transcript.join("\n\n")
    }))
}

fn capitalize(value: &str) -> String {
    let mut characters = value.chars();
    match characters.next() {
        Some(first) => first.to_uppercase().chain(characters).collect(),
        None => String::new(),
    }
}

fn limit_transcript(mut value: String) -> String {
    const MAX_CHARS: usize = 32 * 1024;
    if value.chars().count() <= MAX_CHARS {
        return value;
    }
    value = value
        .chars()
        .rev()
        .take(MAX_CHARS.saturating_sub(24))
        .collect::<String>()
        .chars()
        .rev()
        .collect();
    format!("[earlier output omitted]\n{value}")
}

fn message_text(content: &Value) -> Option<String> {
    if let Some(text) = content.as_str() {
        return Some(text.trim().to_owned());
    }
    let text = content
        .as_array()?
        .iter()
        .filter_map(|block| {
            (block.get("type").and_then(Value::as_str) == Some("text"))
                .then(|| block.get("text").and_then(Value::as_str))
                .flatten()
        })
        .collect::<Vec<_>>()
        .join("\n");
    Some(text.trim().to_owned())
}

fn truncate_chars(input: &str, limit: usize) -> String {
    let normalized = input.split_whitespace().collect::<Vec<_>>().join(" ");
    if normalized.chars().count() <= limit {
        return normalized;
    }
    let mut value: String = normalized.chars().take(limit.saturating_sub(1)).collect();
    value.push('…');
    value
}

#[cfg(test)]
mod tests {
    use std::fs;
    #[cfg(unix)]
    use std::os::unix::fs::PermissionsExt;

    use tempfile::tempdir;

    use super::*;

    const SESSION: &str = r#"{"type":"session","version":3,"id":"123e4567-e89b-12d3-a456-426614174000","timestamp":"2026-08-18T12:00:00.000Z","cwd":"/work/project"}
{"type":"message","id":"a1","parentId":null,"timestamp":"2026-08-18T12:00:01.000Z","message":{"role":"user","content":"Implement the provider dashboard","timestamp":1}}
{"type":"session_info","id":"a2","parentId":"a1","timestamp":"2026-08-18T12:00:02.000Z","name":"provider-work"}
{"type":"message","id":"a3","parentId":"a2","timestamp":"2026-08-18T12:00:03.000Z","message":{"role":"assistant","content":[{"type":"thinking","thinking":"hidden"},{"type":"text","text":"The adapter is ready."}],"stopReason":"stop","timestamp":2}}
"#;

    #[test]
    fn parses_current_pi_v3_jsonl_shape() {
        let session = parse_pi_session(
            SESSION,
            Path::new("/sessions/example.jsonl"),
            Some(SystemTime::UNIX_EPOCH),
            Some(SystemTime::UNIX_EPOCH),
        )
        .unwrap();

        assert_eq!(session.provider, Provider::Pi);
        assert_eq!(session.provider_session_id, "123e4567-e89b-12d3-a456-426614174000");
        assert_eq!(session.name, "provider-work");
        assert_eq!(session.state, SessionState::Completed);
        assert_eq!(session.summary, "The adapter is ready.");
        assert_eq!(session.cwd, PathBuf::from("/work/project"));
        assert_eq!(
            session.capabilities,
            BTreeSet::from([Capability::Inspect])
        );
    }

    #[test]
    fn incomplete_history_remains_unknown() {
        let input = r#"{"type":"session","version":3,"id":"abc","timestamp":"2026-08-18T12:00:00Z","cwd":"/work"}
{"type":"message","id":"a1","parentId":null,"timestamp":"2026-08-18T12:00:01Z","message":{"role":"user","content":"Keep working"}}
"#;
        let session = parse_pi_session(input, Path::new("session.jsonl"), None, None).unwrap();

        assert_eq!(session.state, SessionState::Unknown);
        assert_eq!(session.name, "Keep working");
    }

    #[test]
    fn incomplete_history_is_visible_without_include_completed() {
        let directory = tempdir().unwrap();
        let input = r#"{"type":"session","version":3,"id":"abc","timestamp":"2026-08-18T12:00:00Z","cwd":"/work"}
{"type":"message","id":"a1","parentId":null,"timestamp":"2026-08-18T12:00:01Z","message":{"role":"user","content":"Keep working"}}
"#;
        fs::write(directory.path().join("one.jsonl"), input).unwrap();
        let source = PiSource::host(directory.path());

        let sessions = source.discover(&DiscoveryRequest::default()).unwrap();

        assert_eq!(sessions.len(), 1);
        assert_eq!(sessions[0].state, SessionState::Unknown);
    }

    #[test]
    fn source_recurses_filters_and_does_not_follow_symlinks() {
        let directory = tempdir().unwrap();
        let inside = directory.path().join("sessions/project");
        fs::create_dir_all(&inside).unwrap();
        fs::write(inside.join("one.jsonl"), SESSION).unwrap();
        fs::write(inside.join("ignore.txt"), SESSION).unwrap();
        #[cfg(unix)]
        std::os::unix::fs::symlink("/does/not/exist", inside.join("escape.jsonl")).unwrap();
        let source = PiSource::host(directory.path().join("sessions"));

        let sessions = source
            .discover(&DiscoveryRequest {
                include_completed: true,
                include_interactive: false,
                cwd: Some(PathBuf::from("/work")),
            })
            .unwrap();

        assert_eq!(sessions.len(), 1);
        assert_eq!(sessions[0].name, "provider-work");
    }

    #[test]
    fn inspect_finds_exact_header_id_and_formats_transcript() {
        let directory = tempdir().unwrap();
        fs::write(directory.path().join("one.jsonl"), SESSION).unwrap();
        let source = PiSource::host(directory.path());
        let session = parse_pi_session(SESSION, Path::new("one.jsonl"), None, None).unwrap();

        assert_eq!(
            source.inspect(&session).unwrap(),
            "User: Implement the provider dashboard\n\nAssistant: The adapter is ready."
        );
    }

    #[cfg(unix)]
    #[test]
    fn controller_opens_the_exact_native_session() {
        let directory = tempdir().unwrap();
        let executable = directory.path().join("pi-test");
        fs::write(
            &executable,
            "#!/bin/sh\n[ \"$1\" = --session ] && [ \"$2\" = 123e4567-e89b-12d3-a456-426614174000 ] && [ \"$3\" = --session-dir ] && [ \"$4\" = \"$PWD\" ]\n",
        )
        .unwrap();
        let mut permissions = fs::metadata(&executable).unwrap().permissions();
        permissions.set_mode(0o700);
        fs::set_permissions(&executable, permissions).unwrap();
        let input = SESSION.replace("/work/project", &directory.path().display().to_string());
        let session = parse_pi_session(&input, Path::new("one.jsonl"), None, None).unwrap();
        let controller = PiController::host(executable.display().to_string(), directory.path());

        let outcome = controller.open(&session).unwrap();

        assert_eq!(
            outcome.provider_session_hint,
            Some("123e4567-e89b-12d3-a456-426614174000".into())
        );
    }

    #[test]
    fn completed_history_respects_include_completed() {
        let directory = tempdir().unwrap();
        fs::write(directory.path().join("one.jsonl"), SESSION).unwrap();
        let source = PiSource::host(directory.path());

        assert!(source
            .discover(&DiscoveryRequest::default())
            .unwrap()
            .is_empty());
    }

    #[test]
    fn missing_store_is_an_empty_source() {
        let directory = tempdir().unwrap();
        let source = PiSource::host(directory.path().join("missing"));

        assert!(source
            .discover(&DiscoveryRequest {
                include_completed: true,
                ..DiscoveryRequest::default()
            })
            .unwrap()
            .is_empty());
    }
}

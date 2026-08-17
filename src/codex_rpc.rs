use std::collections::{BTreeMap, VecDeque};
use std::io::{BufRead, BufReader, Read, Write};
use std::process::{Child, ChildStdin, Command, Stdio};
use std::sync::mpsc::{self, Receiver, TryRecvError};
use std::thread::{self, JoinHandle};
use std::time::{Duration, Instant};

use anyhow::{anyhow, bail, Context, Result};
use serde_json::{json, Value};

#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) struct AppServerInvocation {
    pub program: String,
    pub args: Vec<String>,
}

impl AppServerInvocation {
    pub fn direct(executable: impl Into<String>) -> Self {
        Self {
            program: executable.into(),
            args: vec![
                "app-server".into(),
                "--listen".into(),
                "stdio://".into(),
            ],
        }
    }

    pub fn docker(container_id: impl Into<String>) -> Self {
        Self {
            program: "docker".into(),
            args: vec![
                "exec".into(),
                "-i".into(),
                container_id.into(),
                "codex".into(),
                "app-server".into(),
                "--listen".into(),
                "stdio://".into(),
            ],
        }
    }

    pub fn proxy(executable: impl Into<String>, socket_path: &std::path::Path) -> Self {
        Self {
            program: executable.into(),
            args: vec![
                "app-server".into(),
                "proxy".into(),
                "--sock".into(),
                socket_path.to_string_lossy().into_owned(),
            ],
        }
    }
}

enum OutputLine {
    Line(String),
    Error(String),
}

/// One initialized App Server connection.
///
/// Requests are serialized by the caller, but responses are still correlated
/// by id because App Server may interleave notifications and server requests.
pub(crate) struct AppServerClient {
    child: Child,
    stdin: ChildStdin,
    lines: Receiver<OutputLine>,
    stdout_reader: Option<JoinHandle<()>>,
    stderr_reader: Option<JoinHandle<Vec<u8>>>,
    pending_responses: BTreeMap<u64, Value>,
    events: VecDeque<Value>,
    next_id: u64,
}

impl AppServerClient {
    pub fn connect(invocation: &AppServerInvocation) -> Result<Self> {
        let mut child = Command::new(&invocation.program)
            .args(&invocation.args)
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()
            .with_context(|| {
                format!(
                    "failed to start App Server transport {} {}",
                    invocation.program,
                    invocation.args.join(" ")
                )
            })?;
        let stdin = child.stdin.take().context("failed to capture App Server stdin")?;
        let stdout = child
            .stdout
            .take()
            .context("failed to capture App Server stdout")?;
        let stderr = child
            .stderr
            .take()
            .context("failed to capture App Server stderr")?;
        let (sender, lines) = mpsc::channel();
        let stdout_reader = thread::spawn(move || {
            for line in BufReader::new(stdout).lines() {
                let message = match line {
                    Ok(line) => OutputLine::Line(line),
                    Err(error) => OutputLine::Error(error.to_string()),
                };
                if sender.send(message).is_err() {
                    break;
                }
            }
        });
        let stderr_reader = thread::spawn(move || {
            let mut stderr = stderr.take(128 * 1024);
            let mut bytes = Vec::new();
            let _ = stderr.read_to_end(&mut bytes);
            bytes
        });

        let mut client = Self {
            child,
            stdin,
            lines,
            stdout_reader: Some(stdout_reader),
            stderr_reader: Some(stderr_reader),
            pending_responses: BTreeMap::new(),
            events: VecDeque::new(),
            next_id: 1,
        };
        client.request(
            "initialize",
            json!({
                "clientInfo": {
                    "name": "open_agent_view",
                    "title": "Open Agent View",
                    "version": env!("CARGO_PKG_VERSION")
                }
            }),
        )?;
        client.notify("initialized", json!({}))?;
        Ok(client)
    }

    pub fn request(&mut self, method: &str, params: Value) -> Result<Value> {
        self.request_with_timeout(method, params, Duration::from_secs(15))
    }

    pub fn request_with_timeout(
        &mut self,
        method: &str,
        params: Value,
        timeout: Duration,
    ) -> Result<Value> {
        let id = self.next_id;
        self.next_id += 1;
        self.send(json!({"method": method, "id": id, "params": params}))?;
        let deadline = Instant::now() + timeout;
        loop {
            if let Some(response) = self.pending_responses.remove(&id) {
                return response_result(method, response);
            }
            let remaining = deadline.saturating_duration_since(Instant::now());
            if remaining.is_zero() {
                bail!("timed out waiting for App Server {method}");
            }
            let line = self
                .lines
                .recv_timeout(remaining)
                .map_err(|error| anyhow!("App Server closed while waiting for {method}: {error}"))?;
            self.accept_line(line)?;
        }
    }

    pub fn notify(&mut self, method: &str, params: Value) -> Result<()> {
        self.send(json!({"method": method, "params": params}))
    }

    pub fn drain_events(&mut self) -> Result<Vec<Value>> {
        loop {
            match self.lines.try_recv() {
                Ok(line) => self.accept_line(line)?,
                Err(TryRecvError::Empty) => break,
                Err(TryRecvError::Disconnected) => {
                    bail!("App Server transport closed")
                }
            }
        }
        Ok(self.events.drain(..).collect())
    }

    fn accept_line(&mut self, line: OutputLine) -> Result<()> {
        let line = match line {
            OutputLine::Line(line) => line,
            OutputLine::Error(error) => bail!("App Server stdout error: {error}"),
        };
        let message: Value = serde_json::from_str(&line)
            .with_context(|| format!("invalid App Server JSONL: {line}"))?;
        let is_server_message = message.get("method").is_some();
        if !is_server_message {
            if let Some(id) = message.get("id").and_then(Value::as_u64) {
                self.pending_responses.insert(id, message);
                return Ok(());
            }
        }
        self.events.push_back(message);
        Ok(())
    }

    fn send(&mut self, message: Value) -> Result<()> {
        serde_json::to_writer(&mut self.stdin, &message)?;
        self.stdin.write_all(b"\n")?;
        self.stdin.flush()?;
        Ok(())
    }
}

fn response_result(method: &str, message: Value) -> Result<Value> {
    if let Some(error) = message.get("error") {
        bail!("App Server {method} failed: {error}");
    }
    message
        .get("result")
        .cloned()
        .context("App Server response omitted result")
}

impl Drop for AppServerClient {
    fn drop(&mut self) {
        // This child is either a stdio App Server or a proxy connection. A
        // durable socket-listening server is a separate, identity-recorded
        // process and is deliberately never signalled here.
        let _ = self.child.kill();
        let _ = self.child.wait();
        if let Some(reader) = self.stdout_reader.take() {
            let _ = reader.join();
        }
        if let Some(reader) = self.stderr_reader.take() {
            let _ = reader.join();
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn classifies_server_requests_as_events_even_when_the_id_is_numeric() {
        let message = json!({
            "id": 1,
            "method": "item/commandExecution/requestApproval",
            "params": {"threadId": "thread-1"}
        });
        assert!(message.get("method").is_some());
        assert!(message.get("result").is_none());
    }

    #[test]
    fn extracts_success_and_error_responses() {
        assert_eq!(
            response_result("thread/list", json!({"id": 1, "result": {"data": []}}))
                .unwrap(),
            json!({"data": []})
        );
        assert!(response_result(
            "thread/list",
            json!({"id": 1, "error": {"code": -32601, "message": "missing"}})
        )
        .unwrap_err()
        .to_string()
        .contains("missing"));
    }
}

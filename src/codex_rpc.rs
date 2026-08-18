use std::collections::{BTreeMap, VecDeque};
use std::io::{BufRead, BufReader, Read, Write};
use std::process::{Child, ChildStdin, Command, Stdio};
use std::sync::mpsc::{self, Receiver, TryRecvError};
use std::thread::{self, JoinHandle};
use std::time::{Duration, Instant};

#[cfg(unix)]
use std::os::unix::net::UnixStream;

use anyhow::{anyhow, bail, Context, Result};
use serde_json::{json, Value};
#[cfg(unix)]
use tungstenite::{client, Message, WebSocket};

const MAX_RPC_MESSAGE_BYTES: usize = 1024 * 1024;

#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) enum AppServerInvocation {
    Process { program: String, args: Vec<String> },
    UnixWebSocket { socket_path: std::path::PathBuf },
}

impl AppServerInvocation {
    pub fn direct(executable: impl Into<String>) -> Self {
        Self::Process {
            program: executable.into(),
            args: vec!["app-server".into(), "--listen".into(), "stdio://".into()],
        }
    }

    pub fn docker(container_id: impl Into<String>) -> Self {
        Self::Process {
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

    #[cfg(test)]
    pub fn proxy(executable: impl Into<String>, socket_path: &std::path::Path) -> Self {
        Self::Process {
            program: executable.into(),
            args: vec![
                "app-server".into(),
                "proxy".into(),
                "--sock".into(),
                socket_path.to_string_lossy().into_owned(),
            ],
        }
    }

    pub fn unix_websocket(socket_path: impl Into<std::path::PathBuf>) -> Self {
        Self::UnixWebSocket {
            socket_path: socket_path.into(),
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
struct ProcessTransport {
    child: Child,
    stdin: ChildStdin,
    lines: Receiver<OutputLine>,
    stdout_reader: Option<JoinHandle<()>>,
    stderr_reader: Option<JoinHandle<Vec<u8>>>,
}

enum ClientTransport {
    Process(ProcessTransport),
    #[cfg(unix)]
    UnixWebSocket(Box<WebSocket<UnixStream>>),
}

pub(crate) struct AppServerClient {
    transport: ClientTransport,
    pending_responses: BTreeMap<u64, Value>,
    events: VecDeque<Value>,
    next_id: u64,
}

impl AppServerClient {
    pub fn connect(invocation: &AppServerInvocation) -> Result<Self> {
        let transport = match invocation {
            AppServerInvocation::Process { program, args } => {
                ClientTransport::Process(Self::connect_process(program, args)?)
            }
            AppServerInvocation::UnixWebSocket { socket_path } => {
                #[cfg(unix)]
                {
                    let stream = UnixStream::connect(socket_path).with_context(|| {
                        format!(
                            "failed to connect to App Server socket {}",
                            socket_path.display()
                        )
                    })?;
                    let (socket, _) = client("ws://localhost/", stream)
                        .context("App Server Unix WebSocket handshake failed")?;
                    ClientTransport::UnixWebSocket(Box::new(socket))
                }
                #[cfg(not(unix))]
                {
                    let _ = socket_path;
                    bail!("App Server Unix sockets are unavailable on this platform")
                }
            }
        };

        let mut client = Self {
            transport,
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

    fn connect_process(program: &str, args: &[String]) -> Result<ProcessTransport> {
        let mut child = Command::new(program)
            .args(args)
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()
            .with_context(|| {
                format!(
                    "failed to start App Server transport {} {}",
                    program,
                    args.join(" ")
                )
            })?;
        let stdin = child
            .stdin
            .take()
            .context("failed to capture App Server stdin")?;
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

        Ok(ProcessTransport {
            child,
            stdin,
            lines,
            stdout_reader: Some(stdout_reader),
            stderr_reader: Some(stderr_reader),
        })
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
            let line = self.receive(remaining).with_context(|| {
                format!("App Server closed or timed out while waiting for {method}")
            })?;
            self.accept_line(line)?;
        }
    }

    pub fn notify(&mut self, method: &str, params: Value) -> Result<()> {
        self.send(json!({"method": method, "params": params}))
    }

    /// Answer a server-initiated request while preserving its opaque ID.
    pub fn respond(&mut self, id: Value, result: Value) -> Result<()> {
        if !id.is_string() && id.as_i64().is_none() {
            bail!("App Server request ID must be a string or signed integer");
        }
        self.send(json!({"id": id, "result": result}))
    }

    pub fn drain_events(&mut self) -> Result<Vec<Value>> {
        let mut received = Vec::new();
        match &mut self.transport {
            ClientTransport::Process(process) => loop {
                match process.lines.try_recv() {
                    Ok(line) => received.push(line),
                    Err(TryRecvError::Empty) => break,
                    Err(TryRecvError::Disconnected) => {
                        bail!("App Server transport closed")
                    }
                }
            },
            #[cfg(unix)]
            ClientTransport::UnixWebSocket(socket) => {
                socket.get_mut().set_nonblocking(true)?;
                loop {
                    match socket.read() {
                        Ok(Message::Text(text)) => received.push(OutputLine::Line(text)),
                        Ok(Message::Binary(bytes)) => received.push(OutputLine::Line(
                            String::from_utf8(bytes)
                                .context("App Server sent non-UTF-8 WebSocket data")?,
                        )),
                        Ok(Message::Ping(bytes)) => socket.send(Message::Pong(bytes))?,
                        Ok(Message::Pong(_)) | Ok(Message::Frame(_)) => {}
                        Ok(Message::Close(_)) => bail!("App Server WebSocket closed"),
                        Err(tungstenite::Error::Io(error))
                            if error.kind() == std::io::ErrorKind::WouldBlock =>
                        {
                            break
                        }
                        Err(error) => {
                            return Err(error).context("App Server WebSocket read failed")
                        }
                    }
                }
                socket.get_mut().set_nonblocking(false)?;
            }
        }
        for line in received {
            self.accept_line(line)?;
        }
        Ok(self.events.drain(..).collect())
    }

    fn receive(&mut self, timeout: Duration) -> Result<OutputLine> {
        match &mut self.transport {
            ClientTransport::Process(process) => process
                .lines
                .recv_timeout(timeout)
                .map_err(|error| anyhow!("App Server process transport failed: {error}")),
            #[cfg(unix)]
            ClientTransport::UnixWebSocket(socket) => {
                socket.get_mut().set_read_timeout(Some(timeout))?;
                loop {
                    match socket.read() {
                        Ok(Message::Text(text)) => return Ok(OutputLine::Line(text)),
                        Ok(Message::Binary(bytes)) => {
                            return Ok(OutputLine::Line(
                                String::from_utf8(bytes)
                                    .context("App Server sent non-UTF-8 WebSocket data")?,
                            ))
                        }
                        Ok(Message::Ping(bytes)) => socket.send(Message::Pong(bytes))?,
                        Ok(Message::Pong(_)) | Ok(Message::Frame(_)) => {}
                        Ok(Message::Close(_)) => bail!("App Server WebSocket closed"),
                        Err(error) => {
                            return Err(error).context("App Server WebSocket read failed")
                        }
                    }
                }
            }
        }
    }

    fn accept_line(&mut self, line: OutputLine) -> Result<()> {
        let line = match line {
            OutputLine::Line(line) => line,
            OutputLine::Error(error) => bail!("App Server stdout error: {error}"),
        };
        if line.len() > MAX_RPC_MESSAGE_BYTES {
            bail!(
                "App Server message exceeded the {}-byte safety limit",
                MAX_RPC_MESSAGE_BYTES
            );
        }
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
        match &mut self.transport {
            ClientTransport::Process(process) => {
                serde_json::to_writer(&mut process.stdin, &message)?;
                process.stdin.write_all(b"\n")?;
                process.stdin.flush()?;
            }
            #[cfg(unix)]
            ClientTransport::UnixWebSocket(socket) => {
                socket.send(Message::Text(serde_json::to_string(&message)?))?;
            }
        }
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
        match &mut self.transport {
            ClientTransport::Process(process) => {
                let _ = process.child.kill();
                let _ = process.child.wait();
                if let Some(reader) = process.stdout_reader.take() {
                    let _ = reader.join();
                }
                if let Some(reader) = process.stderr_reader.take() {
                    let _ = reader.join();
                }
            }
            #[cfg(unix)]
            ClientTransport::UnixWebSocket(socket) => {
                let _ = socket.close(None);
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[cfg(unix)]
    #[test]
    fn exchanges_json_rpc_over_a_unix_websocket() {
        use std::os::unix::net::UnixListener;

        let directory = tempfile::tempdir().unwrap();
        let socket_path = directory.path().join("app-server.sock");
        let listener = UnixListener::bind(&socket_path).unwrap();
        let server = thread::spawn(move || {
            let (stream, _) = listener.accept().unwrap();
            let mut socket = tungstenite::accept(stream).unwrap();

            let initialize: Value =
                serde_json::from_str(&socket.read().unwrap().into_text().unwrap()).unwrap();
            assert_eq!(initialize["method"], "initialize");
            let initialize_id = initialize["id"].as_u64().unwrap();
            socket
                .send(Message::Text(
                    json!({"id": initialize_id, "result": {}}).to_string(),
                ))
                .unwrap();

            let initialized: Value =
                serde_json::from_str(&socket.read().unwrap().into_text().unwrap()).unwrap();
            assert_eq!(initialized["method"], "initialized");
            assert!(initialized.get("id").is_none());

            let request: Value =
                serde_json::from_str(&socket.read().unwrap().into_text().unwrap()).unwrap();
            assert_eq!(request["method"], "thread/list");
            let request_id = request["id"].as_u64().unwrap();
            socket
                .send(Message::Text(
                    json!({
                        "id": "approval-1",
                        "method": "item/commandExecution/requestApproval",
                        "params": {
                            "threadId": "thread-1",
                            "turnId": "turn-1",
                            "itemId": "item-1",
                            "startedAtMs": 1,
                            "command": "cargo test"
                        }
                    })
                    .to_string(),
                ))
                .unwrap();
            socket
                .send(Message::Text(
                    json!({
                        "id": -7,
                        "method": "item/fileChange/requestApproval",
                        "params": {
                            "threadId": "thread-1",
                            "turnId": "turn-1",
                            "itemId": "item-2",
                            "startedAtMs": 2
                        }
                    })
                    .to_string(),
                ))
                .unwrap();
            socket
                .send(Message::Text(
                    json!({"id": request_id, "result": {"data": []}}).to_string(),
                ))
                .unwrap();

            let approval: Value =
                serde_json::from_str(&socket.read().unwrap().into_text().unwrap()).unwrap();
            assert_eq!(
                approval,
                json!({
                    "id": "approval-1",
                    "result": {"decision": "accept"}
                })
            );
            let denial: Value =
                serde_json::from_str(&socket.read().unwrap().into_text().unwrap()).unwrap();
            assert_eq!(
                denial,
                json!({
                    "id": -7,
                    "result": {"decision": "decline"}
                })
            );
        });

        let mut client =
            AppServerClient::connect(&AppServerInvocation::unix_websocket(socket_path)).unwrap();
        assert_eq!(
            client.request("thread/list", json!({})).unwrap(),
            json!({"data": []})
        );
        let events = client.drain_events().unwrap();
        assert_eq!(events.len(), 2);
        assert_eq!(events[0]["id"], "approval-1");
        assert_eq!(events[1]["id"], -7);
        client
            .respond(json!("approval-1"), json!({"decision": "accept"}))
            .unwrap();
        client
            .respond(json!(-7), json!({"decision": "decline"}))
            .unwrap();
        drop(client);
        server.join().unwrap();
    }

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
            response_result("thread/list", json!({"id": 1, "result": {"data": []}})).unwrap(),
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

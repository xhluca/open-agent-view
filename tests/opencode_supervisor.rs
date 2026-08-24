#![cfg(target_os = "linux")]

use std::fs;
use std::os::unix::fs::PermissionsExt;
use std::path::Path;
use std::sync::Arc;
use std::thread;
use std::time::{Duration, Instant};

use open_agent_view::adapters::{
    DiscoveryRequest, OpenCodeController, OpenCodeSource, SessionSource,
};
use open_agent_view::control::{LaunchRequest, ProviderController};
use open_agent_view::domain::{Capability, Provider, SessionSnapshot, SessionState};
use open_agent_view::opencode_supervisor::OpenCodeSupervisor;
use tempfile::tempdir;

#[test]
fn owned_server_survives_dashboard_reconnect_and_rejects_external_sessions() {
    let directory = tempdir().unwrap();
    let fake = directory.path().join("fake-opencode");
    write_fake_opencode(&fake);
    let first = Arc::new(
        OpenCodeSupervisor::with_state_dir(
            fake.display().to_string(),
            directory.path().join("state"),
        )
        .unwrap(),
    );
    let _shutdown = ShutdownGuard(first.clone());
    let source = OpenCodeSource::managed(fake.display().to_string(), first.clone());
    let controller = OpenCodeController::managed(fake.display().to_string(), first.clone());

    let launched = controller
        .launch(&LaunchRequest {
            provider: Provider::OpenCode,
            model: Some("anthropic/claude-sonnet-4-5".into()),
            prompt: "initial managed task".into(),
            cwd: directory.path().to_path_buf(),
        })
        .unwrap();
    assert_eq!(launched.provider_session_hint.as_deref(), Some("ses_owned"));
    let direct = first.list().unwrap();
    assert_eq!(direct.len(), 1, "managed server lost its owned session");
    assert_eq!(direct[0].state, SessionState::Working);
    assert_eq!(direct[0].summary, "answer: initial managed task");
    assert_eq!(direct[0].updated_at_ms, 9_000_000_000_001);
    assert_eq!(
        fs::metadata(directory.path().join("state"))
            .unwrap()
            .permissions()
            .mode()
            & 0o777,
        0o700
    );
    assert_eq!(
        fs::metadata(directory.path().join("state/server.json"))
            .unwrap()
            .permissions()
            .mode()
            & 0o777,
        0o600
    );
    for _ in 0..10 {
        let reconnect = OpenCodeSupervisor::with_state_dir(
            fake.display().to_string(),
            directory.path().join("state"),
        )
        .unwrap();
        assert_eq!(reconnect.list().unwrap().len(), 1);
    }
    let mut snapshot = discover_and_enrich(&source, &controller);
    let session = &snapshot.sessions[0];
    assert_eq!(session.state, SessionState::Working);
    assert_eq!(
        session.capabilities,
        [
            Capability::Inspect,
            Capability::Reply,
            Capability::Interrupt
        ]
        .into()
    );
    assert_eq!(
        controller.inspect(session).unwrap(),
        "User: initial managed task\n\nAssistant: answer: initial managed task"
    );
    controller.reply(session, "follow up").unwrap();

    // A second dashboard reconstructs authority only from the private record,
    // exact process identity, exact listener owner, Basic secret, and owned ID.
    let second = Arc::new(
        OpenCodeSupervisor::with_state_dir(
            fake.display().to_string(),
            directory.path().join("state"),
        )
        .unwrap(),
    );
    let second_source = OpenCodeSource::managed(fake.display().to_string(), second.clone());
    let second_controller = OpenCodeController::managed(fake.display().to_string(), second.clone());
    snapshot = discover_and_enrich(&second_source, &second_controller);
    assert_eq!(snapshot.sessions[0].provider_session_id, "ses_owned");
    assert!(second_controller
        .inspect(&snapshot.sessions[0])
        .unwrap()
        .contains("User: follow up"));

    let mut external = snapshot.sessions[0].clone();
    external.provider_session_id = "ses_external".into();
    assert!(second_controller.reply(&external, "must fail").is_err());

    second_controller.interrupt(&snapshot.sessions[0]).unwrap();
    snapshot = wait_for_state(&second_source, &second_controller, SessionState::Completed);
    assert_eq!(
        snapshot.sessions[0].capabilities,
        [Capability::Inspect, Capability::Reply].into()
    );
    second_controller
        .reply(&snapshot.sessions[0], "restart work")
        .unwrap();
    wait_for_state(&second_source, &second_controller, SessionState::Working);

    second.shutdown_server().unwrap();
    let deadline = Instant::now() + Duration::from_secs(3);
    loop {
        if second
            .list()
            .map(|sessions| sessions.is_empty())
            .unwrap_or(false)
        {
            break;
        }
        assert!(
            Instant::now() < deadline,
            "OpenCode test server did not stop"
        );
        thread::sleep(Duration::from_millis(25));
    }
}

#[test]
#[ignore = "set OAV_REAL_OPENCODE_BIN; OAV_REAL_OPENCODE_MODEL is optional"]
fn real_opencode_server_contract_without_model_credentials() {
    let executable = std::env::var("OAV_REAL_OPENCODE_BIN").unwrap();
    let model = std::env::var("OAV_REAL_OPENCODE_MODEL").ok();
    let directory = tempdir().unwrap();
    let supervisor = Arc::new(
        OpenCodeSupervisor::with_state_dir(executable, directory.path().join("state")).unwrap(),
    );
    let _shutdown = ShutdownGuard(supervisor.clone());

    let launched = supervisor
        .launch_with_model(
            "OAV isolated server contract probe",
            directory.path(),
            model.as_deref(),
        )
        .unwrap();
    assert!(launched.id.starts_with("ses_"));
    let sessions = supervisor.list().unwrap();
    assert_eq!(sessions.len(), 1);
    assert_eq!(sessions[0].id, launched.id);
    // With no provider credentials, 1.18.18 accepts prompt_async with 204 but
    // may publish an asynchronous startup error before persisting the message.
    // The bounded inspector must still parse the documented message response.
    assert!(!supervisor.inspect(&launched.id).unwrap().is_empty());
    supervisor.shutdown_server().unwrap();
}

struct ShutdownGuard(Arc<OpenCodeSupervisor>);

impl Drop for ShutdownGuard {
    fn drop(&mut self) {
        let _ = self.0.shutdown_server();
    }
}

fn discover_and_enrich(
    source: &OpenCodeSource,
    controller: &OpenCodeController,
) -> SessionSnapshot {
    let sessions = source
        .discover(&DiscoveryRequest {
            include_completed: true,
            ..DiscoveryRequest::default()
        })
        .unwrap();
    let mut snapshot = SessionSnapshot {
        sessions,
        warnings: Vec::new(),
    };
    controller.enrich(&mut snapshot);
    assert!(snapshot.warnings.is_empty(), "{:?}", snapshot.warnings);
    snapshot
}

fn wait_for_state(
    source: &OpenCodeSource,
    controller: &OpenCodeController,
    expected: SessionState,
) -> SessionSnapshot {
    let deadline = Instant::now() + Duration::from_secs(3);
    loop {
        let snapshot = discover_and_enrich(source, controller);
        if snapshot.sessions[0].state == expected {
            return snapshot;
        }
        assert!(
            Instant::now() < deadline,
            "timed out waiting for {expected:?}"
        );
        thread::sleep(Duration::from_millis(20));
    }
}

fn write_fake_opencode(path: &Path) {
    fs::write(
        path,
        r##"#!/usr/bin/env python3
import base64
import json
import os
import sys
from http.server import BaseHTTPRequestHandler, ThreadingHTTPServer
from urllib.parse import parse_qs, urlparse

if len(sys.argv) < 2 or sys.argv[1] != "serve":
    if len(sys.argv) > 1 and sys.argv[1] == "export":
        print('{"messages":[]}')
    elif len(sys.argv) > 1 and sys.argv[1] == "db":
        print("record")
    else:
        print('[]')
    raise SystemExit(0)

port = int(sys.argv[sys.argv.index("--port") + 1])
username = os.environ.get("OPENCODE_SERVER_USERNAME", "opencode")
password = os.environ["OPENCODE_SERVER_PASSWORD"]
authorization = "Basic " + base64.b64encode(f"{username}:{password}".encode()).decode()
sessions = {}
statuses = {}
messages = {}

class Handler(BaseHTTPRequestHandler):
    def log_message(self, *_):
        pass

    def authorized(self):
        if self.headers.get("Authorization") != authorization:
            self.send_response(401)
            self.end_headers()
            return False
        return True

    def body(self):
        length = int(self.headers.get("Content-Length", "0"))
        return json.loads(self.rfile.read(length) or b"{}")

    def respond(self, status, value=None):
        body = b"" if value is None else json.dumps(value).encode()
        self.send_response(status)
        self.send_header("Content-Type", "application/json")
        self.send_header("Content-Length", str(len(body)))
        self.end_headers()
        self.wfile.write(body)

    def do_GET(self):
        if not self.authorized(): return
        parsed = urlparse(self.path)
        if parsed.path == "/global/health":
            return self.respond(200, {"healthy": True, "version": "fake"})
        if parsed.path == "/session/status":
            return self.respond(200, {key: {"type": value} for key, value in statuses.items()})
        if parsed.path == "/session":
            return self.respond(200, list(sessions.values()))
        if parsed.path.endswith("/message"):
            session_id = parsed.path.split("/")[2]
            if session_id not in sessions: return self.respond(404, {"error": "missing"})
            return self.respond(200, messages[session_id])
        if parsed.path.startswith("/session/"):
            session_id = parsed.path.split("/")[2]
            if session_id not in sessions: return self.respond(404, {"error": "missing"})
            return self.respond(200, sessions[session_id])
        return self.respond(404, {"error": "unknown"})

    def do_POST(self):
        if not self.authorized(): return
        parsed = urlparse(self.path)
        if parsed.path == "/session":
            data = self.body()
            directory = parse_qs(parsed.query).get("directory", [os.getcwd()])[0]
            session_id = "ses_owned"
            sessions[session_id] = {"id": session_id, "title": data.get("title", "task"), "directory": directory, "time": {"created": 1, "updated": 9000000000000}}
            statuses[session_id] = "idle"
            messages[session_id] = []
            return self.respond(200, sessions[session_id])
        if parsed.path.endswith("/prompt_async"):
            session_id = parsed.path.split("/")[2]
            if session_id not in sessions: return self.respond(404, {"error": "missing"})
            data = self.body()
            if not messages[session_id] and data.get("model") != {"providerID": "anthropic", "modelID": "claude-sonnet-4-5"}:
                return self.respond(422, {"error": "model selector missing or malformed"})
            text = "\n".join(part.get("text", "") for part in data.get("parts", []) if part.get("type") == "text")
            messages[session_id].append({"info": {"role": "user"}, "parts": [{"type": "text", "text": text}]})
            messages[session_id].append({"info": {"role": "assistant", "time": {"completed": sessions[session_id]["time"]["updated"] + 1}}, "parts": [{"type": "text", "text": "answer: " + text}]})
            sessions[session_id]["time"]["updated"] += 1
            statuses[session_id] = "busy"
            return self.respond(204)
        if parsed.path.endswith("/abort"):
            session_id = parsed.path.split("/")[2]
            if session_id not in sessions: return self.respond(404, {"error": "missing"})
            self.body()
            statuses[session_id] = "idle"
            return self.respond(200, True)
        return self.respond(404, {"error": "unknown"})

ThreadingHTTPServer(("127.0.0.1", port), Handler).serve_forever()
"##,
    )
    .unwrap();
    fs::set_permissions(path, fs::Permissions::from_mode(0o700)).unwrap();
}

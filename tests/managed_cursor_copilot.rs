#![cfg(unix)]

use std::collections::BTreeSet;
use std::fs::{self, OpenOptions};
use std::io::Write;
use std::os::unix::fs::OpenOptionsExt;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::thread;
use std::time::{Duration, Instant};

use open_agent_view::adapters::{CopilotController, CopilotSupervisor};
#[cfg(target_os = "linux")]
use open_agent_view::adapters::{
    CursorController, CursorSource, CursorSupervisor, DiscoveryRequest, SessionSource,
};
use open_agent_view::control::{LaunchRequest, ProviderController};
use open_agent_view::domain::{
    AgentSession, Capability, Provider, Runtime, SessionKind, SessionSnapshot, SessionState,
};
use tempfile::tempdir;

#[test]
#[cfg(target_os = "linux")]
fn public_cursor_controller_owns_the_complete_safe_lifecycle() {
    let directory = tempdir().unwrap();
    let executable = directory.path().join("cursor-agent-mock");
    write_executable(
        &executable,
        r##"#!/bin/sh
if [ "${1:-}" = "create-chat" ]; then
  printf '%s\n' 'cursor-controller-owned'
  exit 0
fi
session='cursor-controller-owned'
workspace=''
for arg in "$@"; do
  case "$arg" in
    --workspace) next_workspace=1 ;;
    *)
      if [ "${next_workspace:-0}" = 1 ]; then workspace="$arg"; next_workspace=0; fi
      ;;
  esac
done
printf '{"type":"system","subtype":"init","cwd":"%s","session_id":"%s"}\n' "$workspace" "$session"
printf '{"type":"assistant","message":{"content":[{"type":"text","text":"controller transcript"}]},"session_id":"%s"}\n' "$session"
trap 'printf "{\"type\":\"result\",\"subtype\":\"error\",\"is_error\":true,\"result\":\"interrupted\",\"session_id\":\"%s\"}\\n" "$session"; exit 130' INT TERM
remaining=10
while [ "$remaining" -gt 0 ]; do sleep 1; remaining=$((remaining - 1)); done
printf '{"type":"result","subtype":"success","is_error":false,"result":"finished","session_id":"%s"}\n' "$session"
"##,
    );
    let workspace = directory.path().join("workspace");
    fs::create_dir(&workspace).unwrap();
    let supervisor = Arc::new(
        CursorSupervisor::with_state_dir(
            executable.display().to_string(),
            directory.path().join("state"),
        )
        .unwrap(),
    );
    let controller = CursorController::managed(supervisor.clone());
    let source = CursorSource::managed(supervisor);

    let outcome = controller
        .launch(&LaunchRequest {
            provider: Provider::Cursor,
            model: None,
            prompt: "exercise public Cursor controller".into(),
            cwd: workspace.clone(),
        })
        .unwrap();
    assert_eq!(
        outcome.provider_session_hint.as_deref(),
        Some("cursor-controller-owned")
    );
    let mut owned = wait_for_cursor(&source, SessionState::Working);
    let mut snapshot = SessionSnapshot {
        sessions: vec![owned.clone()],
        warnings: Vec::new(),
    };
    controller.enrich(&mut snapshot);
    owned = snapshot.sessions.remove(0);
    assert!(owned.capabilities.contains(&Capability::Interrupt));
    assert_eq!(controller.inspect(&owned).unwrap(), "controller transcript");

    controller.interrupt(&owned).unwrap();
    let interrupted = wait_for_cursor(&source, SessionState::NeedsInput);
    assert!(interrupted.capabilities.contains(&Capability::Reply));
    controller
        .reply(&interrupted, "run the safe next turn")
        .unwrap();
    let retried = wait_for_cursor(&source, SessionState::Working);
    controller.interrupt(&retried).unwrap();
    let _ = wait_for_cursor(&source, SessionState::NeedsInput);

    let mut external = external_session(Provider::Cursor, "cursor-external", &workspace);
    external.capabilities = BTreeSet::from([
        Capability::Inspect,
        Capability::Reply,
        Capability::Interrupt,
    ]);
    let mut snapshot = SessionSnapshot {
        sessions: vec![external.clone()],
        warnings: Vec::new(),
    };
    controller.enrich(&mut snapshot);
    assert!(snapshot.sessions[0].capabilities.is_empty());
    assert!(controller.inspect(&external).is_err());
    assert!(controller.reply(&external, "must be refused").is_err());
    assert!(controller.interrupt(&external).is_err());
}

#[test]
fn public_copilot_controller_retains_prompt_and_permission_authority() {
    let directory = tempdir().unwrap();
    let executable = directory.path().join("copilot-acp-mock");
    write_executable(
        &executable,
        r##"#!/bin/sh
read initialize
printf '%s\n' '{"jsonrpc":"2.0","id":1,"result":{"protocolVersion":1,"agentCapabilities":{"loadSession":true,"sessionCapabilities":{"list":{},"close":{}}}}}'
read new_session
printf '%s\n' '{"jsonrpc":"2.0","id":2,"result":{"sessionId":"copilot-controller-owned"}}'
read first_prompt
printf '%s\n' '{"jsonrpc":"2.0","method":"session/update","params":{"sessionId":"copilot-controller-owned","update":{"sessionUpdate":"agent_message_chunk","content":{"type":"text","text":"public controller transcript"}}}}'
printf '%s\n' '{"jsonrpc":"2.0","id":"permission-public","method":"session/request_permission","params":{"sessionId":"copilot-controller-owned","options":[{"optionId":"allow-once","name":"Allow once","kind":"allow_once"},{"optionId":"reject-once","name":"Reject once","kind":"reject_once"}]}}'
printf '%s\n' '{"jsonrpc":"2.0","id":"permission-duplicate","method":"session/request_permission","params":{"sessionId":"copilot-controller-owned","options":[{"optionId":"allow-duplicate","name":"Allow once","kind":"allow_once"}]}}'
read duplicate_cancel
case "$duplicate_cancel" in *'"id":"permission-duplicate"'*'"cancelled"'*) : ;; *) exit 65 ;; esac
read permission
printf '%s\n' '{"jsonrpc":"2.0","id":3,"result":{"stopReason":"end_turn"}}'
read second_prompt
printf '%s\n' '{"jsonrpc":"2.0","method":"session/update","params":{"sessionId":"copilot-controller-owned","update":{"sessionUpdate":"agent_message_chunk","content":{"type":"text","text":" after reply"}}}}'
read cancellation
while read remaining; do :; done
"##,
    );
    let workspace = directory.path().join("workspace");
    fs::create_dir(&workspace).unwrap();
    let supervisor = Arc::new(CopilotSupervisor::host(executable.display().to_string()));
    let controller = CopilotController::managed(supervisor);

    let outcome = controller
        .launch(&LaunchRequest {
            provider: Provider::GitHubCopilot,
            model: None,
            prompt: "exercise public Copilot controller".into(),
            cwd: workspace.clone(),
        })
        .unwrap();
    assert_eq!(
        outcome.provider_session_hint.as_deref(),
        Some("copilot-controller-owned")
    );
    let mut snapshot = SessionSnapshot::default();
    let waiting = wait_for_copilot(&controller, &mut snapshot, SessionState::NeedsInput);
    assert!(waiting.capabilities.contains(&Capability::Approve));
    assert!(waiting.capabilities.contains(&Capability::Decline));
    assert_eq!(
        controller.inspect(&waiting).unwrap(),
        "public controller transcript"
    );
    controller.resolve_approval(&waiting, true).unwrap();

    let completed = wait_for_copilot(&controller, &mut snapshot, SessionState::Completed);
    assert!(completed.capabilities.contains(&Capability::Reply));
    controller
        .reply(&completed, "continue through ACP")
        .unwrap();
    let working = wait_for_copilot(&controller, &mut snapshot, SessionState::Working);
    assert!(working.capabilities.contains(&Capability::Interrupt));
    controller.interrupt(&working).unwrap();

    let mut external = external_session(Provider::GitHubCopilot, "copilot-external", &workspace);
    external.capabilities = BTreeSet::from([
        Capability::Inspect,
        Capability::Reply,
        Capability::Approve,
        Capability::Decline,
        Capability::Interrupt,
    ]);
    let mut external_snapshot = SessionSnapshot {
        sessions: vec![external.clone()],
        warnings: Vec::new(),
    };
    controller.enrich(&mut external_snapshot);
    assert!(external_snapshot.sessions[0].capabilities.is_empty());
    assert!(controller.inspect(&external).is_err());
    assert!(controller.reply(&external, "must be refused").is_err());
    assert!(controller.interrupt(&external).is_err());
    assert!(controller.resolve_approval(&external, true).is_err());
}

fn write_executable(path: &Path, script: &str) {
    let mut file = OpenOptions::new()
        .create_new(true)
        .write(true)
        .mode(0o700)
        .open(path)
        .unwrap();
    file.write_all(script.as_bytes()).unwrap();
    file.sync_all().unwrap();
    drop(file);
}

#[cfg(target_os = "linux")]
fn wait_for_cursor(source: &CursorSource, expected: SessionState) -> AgentSession {
    let deadline = Instant::now() + Duration::from_secs(4);
    loop {
        let mut sessions = source
            .discover(&DiscoveryRequest {
                include_completed: true,
                include_interactive: true,
                cwd: None,
            })
            .unwrap();
        if let Some(session) = sessions.pop().filter(|session| session.state == expected) {
            return session;
        }
        assert!(
            Instant::now() < deadline,
            "Cursor never reached {expected:?}"
        );
        thread::sleep(Duration::from_millis(20));
    }
}

fn wait_for_copilot(
    controller: &CopilotController,
    snapshot: &mut SessionSnapshot,
    expected: SessionState,
) -> AgentSession {
    let deadline = Instant::now() + Duration::from_secs(4);
    loop {
        snapshot.sessions.clear();
        snapshot.warnings.clear();
        controller.enrich(snapshot);
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
            "Copilot never reached {expected:?}"
        );
        thread::sleep(Duration::from_millis(20));
    }
}

fn external_session(provider: Provider, id: &str, cwd: &Path) -> AgentSession {
    AgentSession {
        id: format!("{}:host:{id}", provider.label()),
        provider_session_id: id.into(),
        provider,
        runtime: Runtime::Host,
        kind: SessionKind::Unknown,
        name: id.into(),
        cwd: PathBuf::from(cwd),
        state: SessionState::Unknown,
        summary: "external record".into(),
        raw_state: Some("external".into()),
        pid: None,
        started_at: None,
        updated_at: None,
        pull_requests: None,
        capabilities: BTreeSet::new(),
    }
}

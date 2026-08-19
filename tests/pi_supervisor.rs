#![cfg(target_os = "linux")]

use std::fs;
use std::os::unix::fs::PermissionsExt;
use std::path::Path;
use std::sync::Arc;
use std::thread;
use std::time::{Duration, Instant};

use open_agent_view::adapters::{DiscoveryRequest, PiController, PiSource, SessionSource};
use open_agent_view::control::{LaunchRequest, ProviderController};
use open_agent_view::domain::{Capability, Provider, SessionSnapshot, SessionState};
use open_agent_view::pi_supervisor::PiSupervisor;
use tempfile::tempdir;

#[test]
fn managed_pi_survives_dashboard_reconnect_and_controls_exact_rpc_session() {
    let directory = tempdir().unwrap();
    let fake_pi = directory.path().join("fake-pi");
    write_fake_pi(&fake_pi);
    let state_dir = directory.path().join("state");
    let daemon_exe = Path::new(env!("CARGO_BIN_EXE_coding-agents")).to_path_buf();
    let first_supervisor = Arc::new(
        PiSupervisor::with_state_dir_and_exe(
            fake_pi.display().to_string(),
            state_dir.clone(),
            daemon_exe.clone(),
        )
        .unwrap(),
    );
    let _shutdown = ShutdownGuard(first_supervisor.clone());
    let first_controller = PiController::managed(
        fake_pi.display().to_string(),
        directory.path().join("external-history"),
        first_supervisor.clone(),
    );

    let launched = first_controller
        .launch(&LaunchRequest {
            provider: Provider::Pi,
            model: None,
            prompt: "initial task".into(),
            cwd: directory.path().to_path_buf(),
        })
        .unwrap();
    assert_eq!(
        launched.provider_session_hint.as_deref(),
        Some("fake-session")
    );

    let source = PiSource::managed(
        directory.path().join("external-history"),
        first_supervisor.clone(),
    );
    let mut snapshot = discover_and_enrich(&source, &first_controller);
    let session = &snapshot.sessions[0];
    assert_eq!(session.state, SessionState::Working);
    assert!(session.capabilities.contains(&Capability::Reply));
    assert!(session.capabilities.contains(&Capability::Interrupt));
    assert_eq!(
        first_controller.inspect(session).unwrap(),
        "User: initial task\n\nAssistant: fake reply"
    );

    first_controller.reply(session, "ask-confirm").unwrap();
    snapshot = wait_for_state(&source, &first_controller, SessionState::NeedsInput);
    let session = &snapshot.sessions[0];
    assert!(session.capabilities.contains(&Capability::Approve));
    assert!(session.capabilities.contains(&Capability::Decline));
    assert!(!session.capabilities.contains(&Capability::Reply));
    first_controller.resolve_approval(session, true).unwrap();
    wait_for_state(&source, &first_controller, SessionState::Completed);

    // A new dashboard client reconnects through the exact persisted daemon
    // identity. It never needs or receives the Pi child's stdio handles.
    let second_supervisor = Arc::new(
        PiSupervisor::with_state_dir_and_exe(fake_pi.display().to_string(), state_dir, daemon_exe)
            .unwrap(),
    );
    let second_source = PiSource::managed(
        directory.path().join("external-history"),
        second_supervisor.clone(),
    );
    let second_controller = PiController::managed(
        fake_pi.display().to_string(),
        directory.path().join("external-history"),
        second_supervisor.clone(),
    );
    snapshot = discover_and_enrich(&second_source, &second_controller);
    let session = &snapshot.sessions[0];
    assert_eq!(session.provider_session_id, "fake-session");
    assert!(session.capabilities.contains(&Capability::Reply));

    second_controller.reply(session, "ask-input").unwrap();
    snapshot = wait_for_state(&second_source, &second_controller, SessionState::NeedsInput);
    let session = &snapshot.sessions[0];
    assert_eq!(
        session.capabilities,
        [Capability::Inspect, Capability::Respond].into()
    );
    second_controller
        .respond_input(session, "typed answer")
        .unwrap();
    wait_for_state(&second_source, &second_controller, SessionState::Completed);

    snapshot = discover_and_enrich(&second_source, &second_controller);
    second_controller
        .reply(&snapshot.sessions[0], "work again")
        .unwrap();
    snapshot = wait_for_state(&second_source, &second_controller, SessionState::Working);
    second_controller.interrupt(&snapshot.sessions[0]).unwrap();
    wait_for_state(&second_source, &second_controller, SessionState::Completed);

    let mut unowned = snapshot.sessions[0].clone();
    unowned.provider_session_id = "external-session".into();
    assert!(second_controller.reply(&unowned, "must fail").is_err());

    second_supervisor.shutdown_daemon().unwrap();
    let deadline = Instant::now() + Duration::from_secs(2);
    while Instant::now() < deadline && !second_supervisor.list().unwrap().is_empty() {
        thread::sleep(Duration::from_millis(20));
    }
    assert!(second_supervisor.list().unwrap().is_empty());
}

struct ShutdownGuard(Arc<PiSupervisor>);

impl Drop for ShutdownGuard {
    fn drop(&mut self) {
        let _ = self.0.shutdown_daemon();
    }
}

fn discover_and_enrich(source: &PiSource, controller: &PiController) -> SessionSnapshot {
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
    source: &PiSource,
    controller: &PiController,
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

fn write_fake_pi(path: &Path) {
    fs::write(
        path,
        r##"#!/bin/sh
while IFS= read -r line; do
  id=$(printf '%s\n' "$line" | sed -n 's/.*"id":"\([^"]*\)".*/\1/p')
  case "$line" in
    *'"type":"get_state"'*)
      printf '{"type":"response","id":"%s","success":true,"data":{"sessionId":"fake-session","sessionFile":"/tmp/fake-session.jsonl"}}\n' "$id"
      ;;
    *'"type":"get_messages"'*)
      printf '{"type":"response","id":"%s","success":true,"data":{"messages":[{"role":"user","content":"initial task"},{"role":"assistant","content":[{"type":"text","text":"fake reply"}]}]}}\n' "$id"
      ;;
    *'"type":"abort"'*)
      printf '{"type":"response","id":"%s","success":true}\n' "$id"
      printf '{"type":"agent_end"}\n'
      ;;
    *'"type":"extension_ui_response"'*)
      printf '{"type":"agent_end"}\n'
      ;;
    *'"type":"prompt"'*)
      printf '{"type":"response","id":"%s","success":true}\n' "$id"
      printf '{"type":"agent_start"}\n'
      case "$line" in
        *ask-confirm*)
          printf '{"type":"extension_ui_request","id":"confirm-1","method":"confirm","title":"Run command","message":"Allow?"}\n'
          ;;
        *ask-input*)
          printf '{"type":"extension_ui_request","id":"input-1","method":"input","title":"Need value","placeholder":"Value"}\n'
          ;;
        *)
          printf '{"type":"message_update","assistantMessageEvent":{"delta":"fake working"}}\n'
          ;;
      esac
      ;;
  esac
done
"##,
    )
    .unwrap();
    let mut permissions = fs::metadata(path).unwrap().permissions();
    permissions.set_mode(0o700);
    fs::set_permissions(path, permissions).unwrap();
}

use std::fs;
use std::process::Command;

use open_agent_view::aliases::SessionAliasRecord;
use open_agent_view::domain::SessionSnapshot;
use tempfile::tempdir;

fn command(home: &std::path::Path) -> Command {
    let mut command = Command::new(env!("CARGO_BIN_EXE_open-agent-view"));
    command
        .env("HOME", home)
        .env("XDG_STATE_HOME", home.join("state"));
    command
}

#[test]
fn cli_alias_round_trip_changes_only_the_local_presentation_layer() {
    let home = tempdir().unwrap();
    let session_id = "claude:docker:reviewer";

    let output = command(home.path())
        .args(["sessions", "rename", session_id, "release captain"])
        .output()
        .unwrap();
    assert!(output.status.success());
    assert!(String::from_utf8_lossy(&output.stdout).contains("provider title was not changed"));

    let output = command(home.path())
        .args(["--json", "sessions", "aliases"])
        .output()
        .unwrap();
    assert!(output.status.success());
    let records: Vec<SessionAliasRecord> = serde_json::from_slice(&output.stdout).unwrap();
    assert_eq!(records.len(), 1);
    assert_eq!(records[0].id, session_id);
    assert_eq!(records[0].alias, "release captain");

    let fixture =
        std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("fixtures/populated-sessions.json");
    let output = command(home.path())
        .args([
            "--json",
            "--fixture",
            fixture.to_str().unwrap(),
            "--all",
            "--no-host-providers",
        ])
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    let snapshot: SessionSnapshot = serde_json::from_slice(&output.stdout).unwrap();
    let renamed = snapshot
        .sessions
        .iter()
        .find(|session| session.id == session_id)
        .unwrap();
    assert_eq!(renamed.name, "release captain");
    assert_eq!(
        renamed.provider_session_id,
        "11111111-1111-4111-8111-111111111111"
    );
    assert!(snapshot
        .warnings
        .iter()
        .any(|warning| warning.contains("local session name")));

    let registry = home
        .path()
        .join("state/open-agent-view/session-aliases.json");
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        assert_eq!(
            fs::metadata(registry).unwrap().permissions().mode() & 0o777,
            0o600
        );
    }

    let output = command(home.path())
        .args(["sessions", "reset-name", session_id])
        .output()
        .unwrap();
    assert!(output.status.success());
    assert!(String::from_utf8_lossy(&output.stdout).contains("latest provider title"));

    let output = command(home.path())
        .args([
            "--json",
            "--fixture",
            fixture.to_str().unwrap(),
            "--all",
            "--no-host-providers",
        ])
        .output()
        .unwrap();
    let snapshot: SessionSnapshot = serde_json::from_slice(&output.stdout).unwrap();
    assert!(!snapshot
        .warnings
        .iter()
        .any(|warning| warning.contains("local session name")));
    assert_eq!(
        snapshot
            .sessions
            .iter()
            .find(|session| session.id == session_id)
            .unwrap()
            .name,
        "release-reviewer"
    );
}

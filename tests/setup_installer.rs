#![cfg(unix)]

use std::fs;
use std::os::unix::fs::PermissionsExt;
use std::process::{Command, Stdio};

#[test]
fn setup_requires_consent_then_stages_and_runs_the_official_installer_with_progress() {
    let directory = tempfile::tempdir().expect("create isolated setup home");
    let bin = directory.path().join("bin");
    fs::create_dir(&bin).expect("create isolated PATH");
    let curl = bin.join("curl");
    fs::write(
        &curl,
        r##"#!/bin/sh
printf '%s\n' "$*" > "$OAV_SETUP_CURL_LOG"
output=''
while [ "$#" -gt 0 ]; do
  if [ "$1" = '--output' ]; then
    shift
    output=$1
  fi
  shift
done
[ -n "$output" ] || exit 81
printf '%s\n' '#!/bin/sh' 'exit 0' > "$output"
printf '%s\n' 'download progress 100%' >&2
"##,
    )
    .expect("write fake curl");
    let bash = bin.join("bash");
    fs::write(
        &bash,
        r##"#!/bin/sh
[ -f "$1" ] || exit 82
printf '%s\n' "$1" > "$OAV_SETUP_BASH_LOG"
printf '%s\n' 'provider install progress 100%'
"##,
    )
    .expect("write fake bash");
    fs::set_permissions(&curl, fs::Permissions::from_mode(0o700)).unwrap();
    fs::set_permissions(&bash, fs::Permissions::from_mode(0o700)).unwrap();
    let curl_log = directory.path().join("curl.log");
    let bash_log = directory.path().join("bash.log");
    let configure = |command: &mut Command| {
        command
            .env("HOME", directory.path())
            .env("PATH", &bin)
            .env("OAV_SETUP_CURL_LOG", &curl_log)
            .env("OAV_SETUP_BASH_LOG", &bash_log)
            .stdin(Stdio::null());
    };

    let mut unconfirmed = Command::new(env!("CARGO_BIN_EXE_coding-agents"));
    configure(&mut unconfirmed);
    let output = unconfirmed
        .args(["setup", "cursor"])
        .output()
        .expect("run unconfirmed setup");
    assert!(!output.status.success());
    assert!(String::from_utf8_lossy(&output.stderr).contains("rerun with --yes"));
    assert!(
        !curl_log.exists(),
        "unconfirmed setup invoked the installer"
    );

    let mut confirmed = Command::new(env!("CARGO_BIN_EXE_coding-agents"));
    configure(&mut confirmed);
    let output = confirmed
        .args(["setup", "cursor", "--yes"])
        .output()
        .expect("run confirmed setup");
    assert!(
        output.status.success(),
        "setup failed: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(stdout.contains("Installing Cursor Agent"));
    assert!(stdout.contains("provider install progress 100%"));
    assert!(stdout.contains("installation completed"));
    assert!(fs::read_to_string(&curl_log)
        .unwrap()
        .contains("https://cursor.com/install"));
    let staged = fs::read_to_string(&bash_log).unwrap();
    assert!(staged.contains("open-agent-view-installer-"));
    assert!(
        !std::path::Path::new(staged.trim()).exists(),
        "staged installer was not removed"
    );
}

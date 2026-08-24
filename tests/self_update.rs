#![cfg(unix)]

use std::fs;
use std::os::unix::fs::PermissionsExt;
use std::process::{Command, Stdio};

fn make_executable(path: &std::path::Path, body: &str) {
    fs::write(path, body).expect("write fake executable");
    fs::set_permissions(path, fs::Permissions::from_mode(0o700))
        .expect("make fake executable runnable");
}

#[test]
fn version_supports_long_and_both_conventional_short_flags() {
    for flag in ["--version", "-v", "-V"] {
        let output = Command::new(env!("CARGO_BIN_EXE_open-agent-view"))
            .arg(flag)
            .output()
            .expect("run version command");
        assert!(output.status.success(), "{flag} failed");
        assert_eq!(
            String::from_utf8_lossy(&output.stdout).trim(),
            format!("open-agent-view {}", env!("CARGO_PKG_VERSION"))
        );
    }
}

#[test]
fn update_and_upgrade_download_then_run_the_installer_in_the_exact_install_dir() {
    for command_name in ["update", "upgrade"] {
        let directory = tempfile::tempdir().expect("create isolated updater home");
        let bin = directory.path().join("bin");
        let install = directory.path().join("install");
        fs::create_dir(&bin).expect("create isolated PATH");
        fs::create_dir(&install).expect("create install directory");
        let gh_log = directory.path().join("gh.log");
        let bash_log = directory.path().join("bash.log");

        make_executable(
            &bin.join("gh"),
            r##"#!/bin/sh
printf '%s\n' "$*" > "$OAV_TEST_GH_LOG"
printf '%s\n' '#!/bin/sh' 'exit 0'
"##,
        );
        make_executable(
            &bin.join("bash"),
            r##"#!/bin/sh
test -f "$1" || exit 91
printf '%s\n%s\n%s\n' "$1" "$OAV_REPO" "$OAV_INSTALL_DIR" > "$OAV_TEST_BASH_LOG"
printf '%s\n' '#!/bin/sh' 'printf '\''open-agent-view 9.8.7\n'\''' > "$OAV_INSTALL_DIR/open-agent-view"
/bin/chmod 700 "$OAV_INSTALL_DIR/open-agent-view"
"##,
        );

        let output = Command::new(env!("CARGO_BIN_EXE_open-agent-view"))
            .arg(command_name)
            .env("HOME", directory.path())
            .env("PATH", &bin)
            .env("OAV_REPO", "owner/repository")
            .env("OAV_INSTALL_DIR", &install)
            .env("OAV_TEST_GH_LOG", &gh_log)
            .env("OAV_TEST_BASH_LOG", &bash_log)
            .stdin(Stdio::null())
            .output()
            .expect("run isolated updater");
        assert!(
            output.status.success(),
            "{command_name} failed: {}",
            String::from_utf8_lossy(&output.stderr)
        );
        let stdout = String::from_utf8_lossy(&output.stdout);
        assert_eq!(
            stdout.lines().last(),
            Some(
                format!(
                    "Updated Open Agent View from {} to 9.8.7.",
                    env!("CARGO_PKG_VERSION")
                )
                .as_str()
            )
        );
        let gh = fs::read_to_string(&gh_log).expect("read gh invocation");
        assert!(gh.contains("repos/owner/repository/contents/install.sh"));
        let lines = fs::read_to_string(&bash_log).expect("read bash invocation");
        let lines = lines.lines().collect::<Vec<_>>();
        assert_eq!(lines[1], "owner/repository");
        assert_eq!(lines[2], install.to_str().unwrap());
        assert!(lines[0].contains("open-agent-view-update-"));
        assert!(
            !std::path::Path::new(lines[0]).exists(),
            "staged updater was not removed"
        );
    }
}

#[test]
fn update_reports_when_the_installed_version_is_already_current() {
    let directory = tempfile::tempdir().expect("create isolated updater home");
    let bin = directory.path().join("bin");
    let install = directory.path().join("install");
    fs::create_dir(&bin).expect("create isolated PATH");
    fs::create_dir(&install).expect("create install directory");

    make_executable(
        &bin.join("gh"),
        "#!/bin/sh\nprintf '%s\\n' '#!/bin/sh' 'exit 0'\n",
    );
    make_executable(
        &bin.join("bash"),
        &format!(
            "#!/bin/sh\nprintf '%s\\n' '#!/bin/sh' \"printf 'open-agent-view {}\\\\n'\" > \"$OAV_INSTALL_DIR/open-agent-view\"\n/bin/chmod 700 \"$OAV_INSTALL_DIR/open-agent-view\"\n",
            env!("CARGO_PKG_VERSION")
        ),
    );

    let output = Command::new(env!("CARGO_BIN_EXE_open-agent-view"))
        .arg("update")
        .env("HOME", directory.path())
        .env("PATH", &bin)
        .env("OAV_REPO", "owner/repository")
        .env("OAV_INSTALL_DIR", &install)
        .stdin(Stdio::null())
        .output()
        .expect("run isolated updater");
    assert!(
        output.status.success(),
        "update failed: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert_eq!(
        stdout.lines().last(),
        Some(
            format!(
                "Open Agent View is already up to date at {}.",
                env!("CARGO_PKG_VERSION")
            )
            .as_str()
        )
    );
}

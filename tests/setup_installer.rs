#![cfg(unix)]

use std::fs;
use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};

#[derive(Clone, Copy)]
enum InstallerKind {
    Script(&'static str),
    Npm(&'static str),
}

#[derive(Clone, Copy)]
struct SetupCase {
    harness: &'static str,
    binary_flag: &'static str,
    installer: InstallerKind,
    login_args: &'static str,
}

const SETUP_CASES: &[SetupCase] = &[
    SetupCase {
        harness: "claude",
        binary_flag: "--claude-bin",
        installer: InstallerKind::Script("https://claude.ai/install.sh"),
        login_args: "auth login",
    },
    SetupCase {
        harness: "codex",
        binary_flag: "--codex-bin",
        installer: InstallerKind::Npm("@openai/codex"),
        login_args: "login",
    },
    SetupCase {
        harness: "pi",
        binary_flag: "--pi-bin",
        installer: InstallerKind::Npm("@mariozechner/pi-coding-agent"),
        login_args: "--no-session",
    },
    SetupCase {
        harness: "opencode",
        binary_flag: "--opencode-bin",
        installer: InstallerKind::Script("https://opencode.ai/install"),
        login_args: "auth login",
    },
    SetupCase {
        harness: "cursor",
        binary_flag: "--cursor-bin",
        installer: InstallerKind::Script("https://cursor.com/install"),
        login_args: "login",
    },
    SetupCase {
        harness: "copilot",
        binary_flag: "--copilot-bin",
        installer: InstallerKind::Script("https://gh.io/copilot-install"),
        login_args: "login",
    },
    SetupCase {
        harness: "antigravity",
        binary_flag: "--antigravity-bin",
        installer: InstallerKind::Script("https://antigravity.google/cli/install.sh"),
        login_args: "",
    },
];

fn write_executable(path: &Path, body: &str) {
    fs::write(path, body).expect("write fake executable");
    fs::set_permissions(path, fs::Permissions::from_mode(0o700))
        .expect("make fake executable runnable");
}

fn isolated_path(bin: &Path) -> String {
    format!("{}:/usr/bin:/bin", bin.display())
}

fn configure(command: &mut Command, root: &Path, bin: &Path, executable: &Path) {
    command
        .env("HOME", root)
        .env("PATH", isolated_path(bin))
        .env("OAV_FAKE_EXECUTABLE", executable)
        .stdin(Stdio::null());
}

#[test]
fn every_missing_harness_requires_consent_then_runs_only_its_official_installer() {
    for case in SETUP_CASES {
        let directory = tempfile::tempdir().expect("create isolated setup home");
        let bin = directory.path().join("bin");
        fs::create_dir(&bin).expect("create isolated PATH");
        let executable = directory.path().join(format!("{}-bin", case.harness));
        let curl_log = directory.path().join("curl.log");
        let npm_log = directory.path().join("npm.log");

        write_executable(
            &bin.join("curl"),
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
printf '%s\n' '#!/bin/sh' 'mkdir -p "$(dirname "$OAV_FAKE_EXECUTABLE")"' 'printf "#!/bin/sh\\nexit 0\\n" > "$OAV_FAKE_EXECUTABLE"' 'chmod 700 "$OAV_FAKE_EXECUTABLE"' > "$output"
printf '%s\n' 'download progress 100%' >&2
"##,
        );
        write_executable(
            &bin.join("bash"),
            r##"#!/bin/sh
[ -f "$1" ] || exit 82
/bin/bash "$1"
printf '%s\n' 'provider install progress 100%'
"##,
        );
        write_executable(
            &bin.join("npm"),
            r##"#!/bin/sh
printf '%s\n' "$*" > "$OAV_SETUP_NPM_LOG"
mkdir -p "$(dirname "$OAV_FAKE_EXECUTABLE")"
printf '#!/bin/sh\nexit 0\n' > "$OAV_FAKE_EXECUTABLE"
chmod 700 "$OAV_FAKE_EXECUTABLE"
printf '%s\n' 'npm install progress 100%'
"##,
        );

        let configure_case = |command: &mut Command| {
            configure(command, directory.path(), &bin, &executable);
            command
                .env("OAV_SETUP_CURL_LOG", &curl_log)
                .env("OAV_SETUP_NPM_LOG", &npm_log);
        };

        let mut unconfirmed = Command::new(env!("CARGO_BIN_EXE_open-agent-view"));
        configure_case(&mut unconfirmed);
        let output = unconfirmed
            .args([case.binary_flag, executable.to_str().unwrap()])
            .args(["setup", case.harness])
            .output()
            .expect("run unconfirmed setup");
        assert!(!output.status.success(), "{} changed state without consent", case.harness);
        assert!(String::from_utf8_lossy(&output.stderr).contains("rerun with --yes"));
        assert!(!curl_log.exists() && !npm_log.exists());

        let mut confirmed = Command::new(env!("CARGO_BIN_EXE_open-agent-view"));
        configure_case(&mut confirmed);
        let output = confirmed
            .args([case.binary_flag, executable.to_str().unwrap()])
            .args(["setup", case.harness, "--yes"])
            .output()
            .expect("run confirmed setup");
        assert!(
            output.status.success(),
            "{} setup failed: {}",
            case.harness,
            String::from_utf8_lossy(&output.stderr)
        );
        let stdout = String::from_utf8_lossy(&output.stdout);
        assert!(stdout.contains("installation completed"), "{stdout}");
        assert!(stdout.contains("Complete authentication interactively"), "{stdout}");
        assert!(executable.is_file());

        match case.installer {
            InstallerKind::Script(url) => {
                let log = fs::read_to_string(&curl_log).expect("script installer curl log");
                assert!(log.contains(url), "{} used the wrong URL: {log}", case.harness);
                assert!(!npm_log.exists());
            }
            InstallerKind::Npm(package) => {
                let log = fs::read_to_string(&npm_log).expect("npm installer log");
                assert_eq!(log.trim(), format!("install --global {package}"));
                assert!(!curl_log.exists());
            }
        }
    }
}

#[cfg(target_os = "linux")]
#[test]
fn every_installed_harness_hands_the_exact_native_login_command_to_a_real_pty() {
    for case in SETUP_CASES {
        let directory = tempfile::tempdir().expect("create isolated setup home");
        let bin = directory.path().join("bin");
        fs::create_dir(&bin).expect("create isolated PATH");
        let executable = directory.path().join(format!("{}-bin", case.harness));
        let login_log = directory.path().join("login.log");

        write_executable(
            &executable,
            r##"#!/bin/sh
printf '%s\n' "$*" > "$OAV_SETUP_LOGIN_LOG"
printf '%s\n' 'native login opened'
"##,
        );

        let cli = PathBuf::from(env!("CARGO_BIN_EXE_open-agent-view"));
        let shell_command = format!(
            "exec '{}' '{}' '{}' setup '{}' --yes",
            cli.display(),
            case.binary_flag,
            executable.display(),
            case.harness
        );
        let output = Command::new("script")
            .args(["-qefc", &shell_command, "/dev/null"])
            .env("HOME", directory.path())
            .env("PATH", isolated_path(&bin))
            .env("OAV_SETUP_LOGIN_LOG", &login_log)
            .output()
            .expect("run setup inside a real PTY");

        assert!(
            output.status.success(),
            "{} login handoff failed: {}",
            case.harness,
            String::from_utf8_lossy(&output.stderr)
        );
        assert_eq!(
            fs::read_to_string(&login_log).expect("native login argv").trim(),
            case.login_args,
            "{} used the wrong native login arguments",
            case.harness
        );
        let stdout = String::from_utf8_lossy(&output.stdout);
        assert!(stdout.contains("native login opened"), "{stdout}");
        assert!(stdout.contains("setup completed"), "{stdout}");
    }
}

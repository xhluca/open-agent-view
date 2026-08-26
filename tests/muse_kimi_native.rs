#![cfg(unix)]

use std::fs::{self, File};
use std::io::{Read, Write};
use std::os::fd::{AsRawFd, FromRawFd};
use std::os::unix::fs::PermissionsExt;
use std::os::unix::process::CommandExt;
use std::path::Path;
use std::process::{Command, Stdio};
use std::thread;
use std::time::{Duration, Instant};

use open_agent_view::adapters::{
    DiscoveryRequest, KimiController, KimiOwnership, KimiSource, MuseController, MuseOwnership,
    MuseSource, SessionSource,
};
use open_agent_view::control::{LaunchRequest, ProviderController};
use open_agent_view::domain::{Capability, Provider, SessionState};

const CHILD_PROVIDER: &str = "OAV_MUSE_KIMI_NATIVE_CHILD";

fn private_tempdir() -> tempfile::TempDir {
    let directory = tempfile::tempdir().unwrap();
    fs::set_permissions(directory.path(), fs::Permissions::from_mode(0o700)).unwrap();
    directory
}

#[test]
fn muse_controller_launch_discover_reattach_interrupt_and_exact_open_use_real_ptys() {
    if std::env::var(CHILD_PROVIDER).as_deref() == Ok("muse") {
        run_muse_child();
        return;
    }
    run_outer("muse", "MUSE", "fix the native parser");
}

#[test]
fn kimi_controller_gates_task_then_discovers_reattaches_interrupts_and_opens_exactly() {
    if std::env::var(CHILD_PROVIDER).as_deref() == Ok("kimi") {
        run_kimi_child();
        return;
    }
    run_outer("kimi", "KIMI", "fix the authenticated parser");
}

fn run_outer(provider: &str, marker: &str, task: &str) {
    let (mut master, slave) = outer_pty();
    set_nonblocking(&master);
    let test_name = if provider == "muse" {
        "muse_controller_launch_discover_reattach_interrupt_and_exact_open_use_real_ptys"
    } else {
        "kimi_controller_gates_task_then_discovers_reattaches_interrupts_and_opens_exactly"
    };
    let mut command = Command::new(std::env::current_exe().unwrap());
    command
        .args(["--exact", test_name, "--nocapture"])
        .env(CHILD_PROVIDER, provider)
        .stdin(Stdio::from(slave.try_clone().unwrap()))
        .stdout(Stdio::from(slave.try_clone().unwrap()))
        .stderr(Stdio::from(slave));
    unsafe {
        command.pre_exec(|| {
            if libc::setsid() < 0 {
                return Err(std::io::Error::last_os_error());
            }
            if libc::ioctl(libc::STDIN_FILENO, libc::TIOCSCTTY as _, 0) < 0 {
                return Err(std::io::Error::last_os_error());
            }
            Ok(())
        });
    }
    let child = command.spawn().unwrap();
    let mut child = ChildGuard::new(child);
    let mut output = Vec::new();

    if provider == "kimi" {
        read_until(
            &mut master,
            &mut output,
            b"KIMI_LOGIN_SCREEN",
            Duration::from_secs(4),
        );
        read_until(
            &mut master,
            &mut output,
            format!("KIMI_TASK:{task}").as_bytes(),
            Duration::from_secs(4),
        );
        assert!(!contains(&output, b"EARLY_TASK"));
    } else {
        read_until(
            &mut master,
            &mut output,
            b"MUSE_NATIVE_READY",
            Duration::from_secs(4),
        );
    }

    master.write_all(b"\x1b[1;2D").unwrap();
    read_until(
        &mut master,
        &mut output,
        format!("{marker}_LAUNCH_RETURNED").as_bytes(),
        Duration::from_secs(4),
    );
    read_until(
        &mut master,
        &mut output,
        format!("{marker}_REATTACHING").as_bytes(),
        Duration::from_secs(4),
    );
    let ready = if provider == "muse" {
        b"MUSE_NATIVE_READY".as_slice()
    } else {
        b"Send /help for help information.".as_slice()
    };
    read_until(&mut master, &mut output, ready, Duration::from_secs(4));
    master.write_all(b"\x1b[1;2C").unwrap();
    read_until(
        &mut master,
        &mut output,
        format!("{marker}_REATTACHED").as_bytes(),
        Duration::from_secs(4),
    );
    read_until(
        &mut master,
        &mut output,
        format!("{marker}_RESUME_EXACT").as_bytes(),
        Duration::from_secs(4),
    );
    read_until(
        &mut master,
        &mut output,
        format!("{marker}_CONTROLLER_OK").as_bytes(),
        Duration::from_secs(4),
    );

    let deadline = Instant::now() + Duration::from_secs(4);
    loop {
        if let Some(status) = child.try_wait().unwrap() {
            assert!(status.success(), "{}", String::from_utf8_lossy(&output));
            child.mark_reaped();
            break;
        }
        assert!(Instant::now() < deadline, "controller child did not exit");
        thread::sleep(Duration::from_millis(10));
    }
}

fn run_muse_child() {
    let _cleanup = NativeSessionCleanup;
    let directory = private_tempdir();
    let executable = directory.path().join("muse");
    let data_root = directory.path().join("data/muse");
    let workspace = directory.path().join("workspace");
    fs::create_dir_all(&workspace).unwrap();
    write_executable(
        &executable,
        r##"#!/usr/bin/env bash
set -euo pipefail
if [[ "${1:-}" == resume ]]; then
  [[ "${2:-}" == 11111111-2222-4333-8444-555555555555 ]]
  printf 'MUSE_RESUME_EXACT'
  exit 0
fi
[[ "${MUSE_NO_AUTO_UPDATE:-}" == 1 ]]
[[ "$#" == 4 ]]
[[ "$1" == --model && "$2" == muse-spark && "$3" == -- && "$4" == 'fix the native parser' ]]
id=11111111-2222-4333-8444-555555555555
dir="$OAV_PROVIDER_ROOT/sessions/2026/08/25/$id"
mkdir -p "$dir"
printf '{"payload":{"record":{"workspace_root":"%s"}}}\n' "$OAV_WORKSPACE" > "$dir/session.jsonl"
printf '{"payload":{"event":{"kind":"started","prompt":"fix the native parser"}}}\n' >> "$dir/session.jsonl"
printf '{"payload":{"event":{"kind":"assistant_message_committed","text":"Muse answer"}}}\n' >> "$dir/session.jsonl"
stty raw -echo
printf '\033[2J\033[HMUSE_NATIVE_READY'
while :; do sleep 1; done
"##,
    );
    let ownership = MuseOwnership::load(directory.path().join("muse-owned.json")).unwrap();
    let source = MuseSource::host(data_root.clone(), ownership.clone());
    let controller = MuseController::host(executable.display().to_string(), data_root, ownership);
    std::env::set_var("OAV_PROVIDER_ROOT", directory.path().join("data/muse"));
    std::env::set_var("OAV_WORKSPACE", &workspace);
    let request = LaunchRequest {
        provider: Provider::MuseCode,
        model: Some("muse-spark".into()),
        prompt: "fix the native parser".into(),
        cwd: workspace,
    };
    let outcome = controller.launch_foreground(&request).unwrap();
    assert_eq!(
        outcome.provider_session_hint.as_deref(),
        Some("11111111-2222-4333-8444-555555555555")
    );
    let session = only_working(source.discover(&request_all()).unwrap());
    println!("MUSE_LAUNCH_RETURNED");
    thread::sleep(Duration::from_millis(150));
    println!("MUSE_REATTACHING");
    thread::sleep(Duration::from_millis(150));
    controller.open(&session).unwrap();
    println!("MUSE_REATTACHED");
    thread::sleep(Duration::from_millis(150));
    controller.interrupt(&session).unwrap();
    assert_eq!(
        source.discover(&request_all()).unwrap()[0].state,
        SessionState::Completed
    );
    controller.open(&session).unwrap();
    thread::sleep(Duration::from_millis(150));
    println!("MUSE_CONTROLLER_OK");
}

fn run_kimi_child() {
    let _cleanup = NativeSessionCleanup;
    let directory = private_tempdir();
    let executable = directory.path().join("kimi");
    let data_root = directory.path().join("kimi-home");
    let workspace = directory.path().join("workspace");
    fs::create_dir_all(&workspace).unwrap();
    write_executable(
        &executable,
        r##"#!/usr/bin/env bash
set -euo pipefail
if [[ "${1:-}" == --session ]]; then
  [[ "${2:-}" == session_11111111-2222-4333-8444-555555555555 ]]
  printf 'KIMI_RESUME_EXACT'
  exit 0
fi
[[ "$#" == 2 ]]
[[ "$1" == --model && "$2" == kimi-code/test ]]
id=session_11111111-2222-4333-8444-555555555555
dir="$OAV_PROVIDER_ROOT/sessions/wd_test/$id"
mkdir -p "$dir"
printf '{"sessionId":"%s","sessionDir":"%s","workDir":"%s"}\n' "$id" "$dir" "$OAV_WORKSPACE" > "$OAV_PROVIDER_ROOT/session_index.jsonl"
printf '{"title":"Kimi parser","lastPrompt":"starting","createdAt":1700000000000,"updatedAt":1700000000001,"cwd":"%s"}\n' "$OAV_WORKSPACE" > "$dir/state.json"
stty raw -echo
printf '\033[2J\033[HKIMI_LOGIN_SCREEN Run /login or /provider to get started.'
if IFS= read -r -t 0.25 -n 1 early; then
  printf '\r\nEARLY_TASK:%s' "$early"
  exit 71
fi
printf '\033[2J\033[HSend /help for help information.'
prompt=$(dd bs=1 count=28 status=none)
printf '\r\nKIMI_TASK:%s' "$prompt"
printf '{"title":"Kimi parser","lastPrompt":"%s","createdAt":1700000000000,"updatedAt":1700000000002,"cwd":"%s"}\n' "$prompt" "$OAV_WORKSPACE" > "$dir/state.json"
while :; do sleep 1; done
"##,
    );
    let ownership = KimiOwnership::load(directory.path().join("kimi-owned.json")).unwrap();
    let source = KimiSource::host(data_root.clone(), ownership.clone());
    let controller = KimiController::host(executable.display().to_string(), data_root, ownership);
    std::env::set_var("OAV_PROVIDER_ROOT", directory.path().join("kimi-home"));
    std::env::set_var("OAV_WORKSPACE", &workspace);
    let request = LaunchRequest {
        provider: Provider::KimiCode,
        model: Some("kimi-code/test".into()),
        prompt: "fix the authenticated parser".into(),
        cwd: workspace,
    };
    let outcome = controller.launch_foreground(&request).unwrap();
    assert_eq!(
        outcome.provider_session_hint.as_deref(),
        Some("session_11111111-2222-4333-8444-555555555555")
    );
    let session = only_working(source.discover(&request_all()).unwrap());
    assert_eq!(session.summary, "fix the authenticated parser");
    println!("KIMI_LAUNCH_RETURNED");
    thread::sleep(Duration::from_millis(150));
    println!("KIMI_REATTACHING");
    thread::sleep(Duration::from_millis(150));
    controller.open(&session).unwrap();
    println!("KIMI_REATTACHED");
    thread::sleep(Duration::from_millis(150));
    controller.interrupt(&session).unwrap();
    assert_eq!(
        source.discover(&request_all()).unwrap()[0].state,
        SessionState::Completed
    );
    controller.open(&session).unwrap();
    thread::sleep(Duration::from_millis(150));
    println!("KIMI_CONTROLLER_OK");
}

struct NativeSessionCleanup;

impl Drop for NativeSessionCleanup {
    fn drop(&mut self) {
        open_agent_view::native_session::shutdown_all();
    }
}

struct ChildGuard {
    child: std::process::Child,
    reaped: bool,
}

impl ChildGuard {
    fn new(child: std::process::Child) -> Self {
        Self {
            child,
            reaped: false,
        }
    }

    fn mark_reaped(&mut self) {
        self.reaped = true;
    }
}

impl std::ops::Deref for ChildGuard {
    type Target = std::process::Child;

    fn deref(&self) -> &Self::Target {
        &self.child
    }
}

impl std::ops::DerefMut for ChildGuard {
    fn deref_mut(&mut self) -> &mut Self::Target {
        &mut self.child
    }
}

impl Drop for ChildGuard {
    fn drop(&mut self) {
        if self.reaped {
            return;
        }
        unsafe {
            libc::kill(-(self.child.id() as libc::pid_t), libc::SIGTERM);
        }
        let deadline = Instant::now() + Duration::from_millis(500);
        while Instant::now() < deadline {
            if matches!(self.child.try_wait(), Ok(Some(_))) {
                self.reaped = true;
                return;
            }
            thread::sleep(Duration::from_millis(10));
        }
        unsafe {
            libc::kill(-(self.child.id() as libc::pid_t), libc::SIGKILL);
        }
        let _ = self.child.wait();
        self.reaped = true;
    }
}

fn only_working(
    sessions: Vec<open_agent_view::domain::AgentSession>,
) -> open_agent_view::domain::AgentSession {
    assert_eq!(sessions.len(), 1);
    let session = sessions.into_iter().next().unwrap();
    assert_eq!(session.state, SessionState::Working);
    assert!(session.capabilities.contains(&Capability::Interrupt));
    session
}

fn request_all() -> DiscoveryRequest {
    DiscoveryRequest {
        include_completed: true,
        ..DiscoveryRequest::default()
    }
}

fn write_executable(path: &Path, body: &str) {
    fs::write(path, body).unwrap();
    fs::set_permissions(path, fs::Permissions::from_mode(0o700)).unwrap();
}

fn outer_pty() -> (File, File) {
    let mut master = -1;
    let mut slave = -1;
    let size = libc::winsize {
        ws_row: 24,
        ws_col: 100,
        ws_xpixel: 0,
        ws_ypixel: 0,
    };
    let result = unsafe {
        libc::openpty(
            &mut master,
            &mut slave,
            std::ptr::null_mut(),
            std::ptr::null(),
            &size,
        )
    };
    assert_eq!(result, 0);
    unsafe { (File::from_raw_fd(master), File::from_raw_fd(slave)) }
}

fn set_nonblocking(file: &File) {
    let flags = unsafe { libc::fcntl(file.as_raw_fd(), libc::F_GETFL) };
    assert!(flags >= 0);
    assert_eq!(
        unsafe { libc::fcntl(file.as_raw_fd(), libc::F_SETFL, flags | libc::O_NONBLOCK) },
        0
    );
}

fn read_until(master: &mut File, output: &mut Vec<u8>, needle: &[u8], timeout: Duration) {
    let previous = occurrences(output, needle);
    let deadline = Instant::now() + timeout;
    let mut bytes = [0_u8; 4096];
    loop {
        match master.read(&mut bytes) {
            Ok(0) => {}
            Ok(count) => output.extend_from_slice(&bytes[..count]),
            Err(error) if error.kind() == std::io::ErrorKind::WouldBlock => {}
            Err(error) => panic!("failed to read outer PTY: {error}"),
        }
        if occurrences(output, needle) > previous {
            return;
        }
        assert!(
            Instant::now() < deadline,
            "did not observe {:?}: {}",
            String::from_utf8_lossy(needle),
            String::from_utf8_lossy(output)
        );
        thread::sleep(Duration::from_millis(10));
    }
}

fn occurrences(haystack: &[u8], needle: &[u8]) -> usize {
    haystack
        .windows(needle.len())
        .filter(|window| *window == needle)
        .count()
}

fn contains(haystack: &[u8], needle: &[u8]) -> bool {
    haystack
        .windows(needle.len())
        .any(|window| window == needle)
}

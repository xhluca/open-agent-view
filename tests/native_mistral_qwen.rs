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
    DiscoveryRequest, MistralVibeController, MistralVibeOwnership, MistralVibeSource,
    QwenController, QwenOwnership, QwenSource, SessionSource,
};
use open_agent_view::control::{LaunchRequest, ProviderController};
use open_agent_view::domain::{Provider, Runtime, SessionSnapshot, SessionState};

const PTY_CHILD: &str = "OAV_MISTRAL_QWEN_PTY_CHILD";

#[test]
fn mistral_controller_background_reattach_interrupt_and_exact_resume_use_real_ptys() {
    if std::env::var(PTY_CHILD).as_deref() == Ok("mistral") {
        run_mistral_pty_child();
        return;
    }
    run_pty_outer(
        "mistral",
        "mistral_controller_background_reattach_interrupt_and_exact_resume_use_real_ptys",
        "VIBE",
        b"VIBE_NATIVE_READY",
    );
}

#[test]
fn qwen_controller_background_reattach_interrupt_and_exact_resume_use_real_ptys() {
    if std::env::var(PTY_CHILD).as_deref() == Ok("qwen") {
        run_qwen_pty_child();
        return;
    }
    run_pty_outer(
        "qwen",
        "qwen_controller_background_reattach_interrupt_and_exact_resume_use_real_ptys",
        "QWEN",
        b"QWEN_NATIVE_READY",
    );
}

fn executable(path: &Path, body: &str) {
    fs::write(path, body).unwrap();
    fs::set_permissions(path, fs::Permissions::from_mode(0o700)).unwrap();
}

#[test]
fn qwen_public_controller_launch_discover_open_and_refuse_unowned_interrupt() {
    let directory = tempfile::tempdir().unwrap();
    let qwen = directory.path().join("qwen");
    executable(
        &qwen,
        r##"#!/bin/sh
set -eu
root=$(CDPATH= cd -- "$(dirname -- "$0")" && pwd)
if [ "${1-} ${2-}" = 'sessions list' ]; then
  [ ! -f "$root/history.jsonl" ] || cat "$root/history.jsonl"
  exit 0
fi
if [ "${1-} ${2-}" = 'sessions ps' ]; then
  exit 0
fi
if [ "${1-}" = '--resume' ]; then
  printf 'resume %s\n' "$2" >> "$root/invocations.log"
  exit 0
fi
session=''
model=''
prompt=''
while [ "$#" -gt 0 ]; do
  case "$1" in
    --session-id) session=$2; shift 2 ;;
    --model) model=$2; shift 2 ;;
    --prompt-interactive) prompt=$2; shift 2 ;;
    *) shift ;;
  esac
done
[ -n "$session" ] && [ -n "$prompt" ] || exit 64
now=$(($(date +%s) * 1000))
cwd=$(pwd)
printf '{"sessionId":"%s","startTime":"2026-08-25T00:00:00Z","mtime":%s,"prompt":"owned task","customTitle":"Qwen owned","cwd":"%s"}\n' "$session" "$now" "$cwd" > "$root/history.jsonl"
printf 'launch %s %s %s\n' "$session" "$model" "$prompt" >> "$root/invocations.log"
"##,
    );
    let ownership = QwenOwnership::load(directory.path().join("qwen-owned.json")).unwrap();
    let controller = QwenController::host(qwen.display().to_string(), ownership.clone());
    let request = LaunchRequest {
        provider: Provider::QwenCode,
        model: Some("qwen3-coder-plus".into()),
        prompt: "owned task".into(),
        cwd: directory.path().to_owned(),
    };

    let launched = controller.launch_foreground(&request).unwrap();
    let id = launched.provider_session_hint.unwrap();
    let source = QwenSource::host(qwen.display().to_string(), ownership);
    let sessions = source.discover(&DiscoveryRequest::default()).unwrap();
    assert_eq!(sessions.len(), 1);
    assert_eq!(sessions[0].provider_session_id, id);
    assert_eq!(sessions[0].provider, Provider::QwenCode);
    assert_eq!(sessions[0].runtime, Runtime::Host);
    controller.open(&sessions[0]).unwrap();
    let invocations = fs::read_to_string(directory.path().join("invocations.log")).unwrap();
    assert!(invocations.contains("qwen3-coder-plus owned task"));
    assert!(invocations.contains(&format!("resume {id}")));

    let mut external = sessions[0].clone();
    external.provider_session_id = "external".into();
    external.id = "qwen:host:external".into();
    assert!(controller.open(&external).is_err());
    assert!(controller.interrupt(&external).is_err());
}

#[test]
fn mistral_public_controller_correlates_exact_launch_then_discovers_and_opens_it() {
    let directory = tempfile::tempdir().unwrap();
    let vibe = directory.path().join("vibe");
    let server = directory.path().join("vibe-app-server");
    executable(
        &vibe,
        r##"#!/bin/sh
set -eu
root=$(CDPATH= cd -- "$(dirname -- "$0")" && pwd)
if [ "${1-}" = '--resume' ]; then
  printf 'resume %s\n' "$2" >> "$root/invocations.log"
  exit 0
fi
now=$(($(date +%s) * 1000))
cwd=$(pwd)
printf '{"id":"vibe-owned","title":"Vibe owned","preview":"owned task","status":{"type":"idle"},"createdAt":%s,"updatedAt":%s,"cwd":"%s","model":"devstral"}\n' "$now" "$now" "$cwd" > "$root/session.json"
printf 'launch model=%s prompt=%s\n' "${VIBE_ACTIVE_MODEL-}" "${1-}" >> "$root/invocations.log"
"##,
    );
    executable(
        &server,
        r##"#!/bin/sh
set -eu
root=$(CDPATH= cd -- "$(dirname -- "$0")" && pwd)
input=$(cat)
printf '%s\n' '{"jsonrpc":"2.0","id":1,"result":{"serverInfo":{"name":"vibe-app-server","version":"test"},"capabilities":{}}}'
case "$input" in
  *'config/read'*)
    printf '%s\n' '{"jsonrpc":"2.0","id":2,"result":{"config":{"models":[{"alias":"devstral"}]}}}'
    ;;
  *)
    if [ -f "$root/session.json" ]; then
      item=$(cat "$root/session.json")
      printf '{"jsonrpc":"2.0","id":2,"result":{"items":[%s]}}\n' "$item"
    else
      printf '%s\n' '{"jsonrpc":"2.0","id":2,"result":{"items":[]}}'
    fi
    ;;
esac
"##,
    );
    let ownership = MistralVibeOwnership::load(directory.path().join("vibe-owned.json")).unwrap();
    let controller = MistralVibeController::host(
        vibe.display().to_string(),
        server.display().to_string(),
        ownership.clone(),
        directory.path().to_owned(),
    );
    assert_eq!(controller.available_models().unwrap(), vec!["devstral"]);
    let launched = controller
        .launch_foreground(&LaunchRequest {
            provider: Provider::MistralVibe,
            model: Some("devstral".into()),
            prompt: "owned task".into(),
            cwd: directory.path().to_owned(),
        })
        .unwrap();
    assert_eq!(
        launched.provider_session_hint.as_deref(),
        Some("vibe-owned")
    );

    let source = MistralVibeSource::host(server.display().to_string(), ownership);
    let sessions = source
        .discover(&DiscoveryRequest {
            include_completed: true,
            ..DiscoveryRequest::default()
        })
        .unwrap();
    assert_eq!(sessions.len(), 1);
    assert_eq!(sessions[0].provider_session_id, "vibe-owned");
    controller.open(&sessions[0]).unwrap();
    let invocations = fs::read_to_string(directory.path().join("invocations.log")).unwrap();
    assert!(invocations.contains("launch model=devstral prompt=owned task"));
    assert!(invocations.contains("resume vibe-owned"));

    let mut external = sessions[0].clone();
    external.provider_session_id = "external".into();
    external.id = "mistral_vibe:host:external".into();
    assert!(controller.open(&external).is_err());
    assert!(controller.interrupt(&external).is_err());
}

fn run_mistral_pty_child() {
    let _cleanup = NativeSessionCleanup;
    let directory = tempfile::tempdir().unwrap();
    let workspace = directory.path().join("workspace");
    fs::create_dir_all(&workspace).unwrap();
    let vibe = directory.path().join("vibe");
    let server = directory.path().join("vibe-app-server");
    executable(
        &vibe,
        r##"#!/usr/bin/env bash
set -euo pipefail
root=$(CDPATH= cd -- "$(dirname -- "$0")" && pwd)
if [[ "${1:-}" == --resume ]]; then
  [[ "${2:-}" == vibe-owned ]]
  printf 'VIBE_RESUME_EXACT'
  exit 0
fi
now=$(($(date +%s) * 1000))
printf '{"id":"vibe-owned","title":"Vibe owned","preview":"owned task","status":{"type":"idle"},"createdAt":%s,"updatedAt":%s,"cwd":"%s","model":"devstral"}\n' "$now" "$now" "$(pwd)" > "$root/session.json"
stty raw -echo
printf '\033[2J\033[HVIBE_NATIVE_READY'
while :; do sleep 1; done
"##,
    );
    executable(
        &server,
        r##"#!/bin/sh
set -eu
root=$(CDPATH= cd -- "$(dirname -- "$0")" && pwd)
input=$(cat)
printf '%s\n' '{"jsonrpc":"2.0","id":1,"result":{"serverInfo":{"name":"vibe-app-server","version":"test"},"capabilities":{}}}'
case "$input" in
  *'config/read'*)
    printf '%s\n' '{"jsonrpc":"2.0","id":2,"result":{"config":{"models":[{"alias":"devstral"}]}}}'
    ;;
  *)
    if [ -f "$root/session.json" ]; then
      item=$(cat "$root/session.json")
      printf '{"jsonrpc":"2.0","id":2,"result":{"items":[%s]}}\n' "$item"
    else
      printf '%s\n' '{"jsonrpc":"2.0","id":2,"result":{"items":[]}}'
    fi
    ;;
esac
"##,
    );
    let ownership = MistralVibeOwnership::load(directory.path().join("owned.json")).unwrap();
    let source = MistralVibeSource::host(server.display().to_string(), ownership.clone());
    let controller = MistralVibeController::host(
        vibe.display().to_string(),
        server.display().to_string(),
        ownership,
        workspace.clone(),
    );
    let request = LaunchRequest {
        provider: Provider::MistralVibe,
        model: Some("devstral".into()),
        prompt: "owned task".into(),
        cwd: workspace,
    };
    let launched = controller.launch_foreground(&request).unwrap();
    assert_eq!(
        launched.provider_session_hint.as_deref(),
        Some("vibe-owned")
    );
    let sessions = source
        .discover(&DiscoveryRequest {
            include_completed: true,
            ..DiscoveryRequest::default()
        })
        .unwrap();
    let mut snapshot = SessionSnapshot {
        sessions,
        ..SessionSnapshot::default()
    };
    controller.enrich(&mut snapshot);
    let session = only_live(snapshot);
    println!("VIBE_LAUNCH_RETURNED");
    println!("VIBE_REATTACHING");
    thread::sleep(Duration::from_millis(150));
    controller.open(&session).unwrap();
    println!("VIBE_REATTACHED");
    controller.interrupt(&session).unwrap();
    controller.open(&session).unwrap();
    println!("VIBE_CONTROLLER_OK");
}

fn run_qwen_pty_child() {
    let _cleanup = NativeSessionCleanup;
    let directory = tempfile::tempdir().unwrap();
    let workspace = directory.path().join("workspace");
    fs::create_dir_all(&workspace).unwrap();
    let qwen = directory.path().join("qwen");
    executable(
        &qwen,
        r##"#!/usr/bin/env bash
set -euo pipefail
root=$(CDPATH= cd -- "$(dirname -- "$0")" && pwd)
if [[ "${1:-} ${2:-}" == 'sessions list' ]]; then
  [[ ! -f "$root/history.jsonl" ]] || cat "$root/history.jsonl"
  exit 0
fi
if [[ "${1:-} ${2:-}" == 'sessions ps' ]]; then
  [[ ! -f "$root/live.jsonl" ]] || cat "$root/live.jsonl"
  exit 0
fi
if [[ "${1:-}" == --resume ]]; then
  printf 'QWEN_RESUME_EXACT'
  exit 0
fi
session=''
prompt=''
while [[ "$#" -gt 0 ]]; do
  case "$1" in
    --session-id) session=$2; shift 2 ;;
    --model) shift 2 ;;
    --prompt-interactive) prompt=$2; shift 2 ;;
    *) shift ;;
  esac
done
[[ -n "$session" && -n "$prompt" ]]
now=$(($(date +%s) * 1000))
cwd=$(pwd)
printf '{"sessionId":"%s","startTime":"2026-08-25T00:00:00Z","mtime":%s,"prompt":"owned task","customTitle":"Qwen owned","cwd":"%s"}\n' "$session" "$now" "$cwd" > "$root/history.jsonl"
printf '{"pid":%s,"sessionId":"%s","cwd":"%s","name":"Qwen owned","startedAt":%s}\n' "$$" "$session" "$cwd" "$now" > "$root/live.jsonl"
trap 'rm -f "$root/live.jsonl"; exit 0' TERM INT EXIT
stty raw -echo
printf '\033[2J\033[HQWEN_NATIVE_READY'
while :; do sleep 1; done
"##,
    );
    let ownership = QwenOwnership::load(directory.path().join("owned.json")).unwrap();
    let source = QwenSource::host(qwen.display().to_string(), ownership.clone());
    let controller = QwenController::host(qwen.display().to_string(), ownership);
    let request = LaunchRequest {
        provider: Provider::QwenCode,
        model: Some("qwen3-coder-plus".into()),
        prompt: "owned task".into(),
        cwd: workspace,
    };
    let launched = controller.launch_foreground(&request).unwrap();
    let id = launched.provider_session_hint.unwrap();
    let sessions = source.discover(&DiscoveryRequest::default()).unwrap();
    let mut snapshot = SessionSnapshot {
        sessions,
        ..SessionSnapshot::default()
    };
    controller.enrich(&mut snapshot);
    let session = only_live(snapshot);
    assert_eq!(session.provider_session_id, id);
    println!("QWEN_LAUNCH_RETURNED");
    println!("QWEN_REATTACHING");
    thread::sleep(Duration::from_millis(150));
    controller.open(&session).unwrap();
    println!("QWEN_REATTACHED");
    controller.interrupt(&session).unwrap();
    controller.open(&session).unwrap();
    println!("QWEN_CONTROLLER_OK");
}

fn only_live(snapshot: SessionSnapshot) -> open_agent_view::domain::AgentSession {
    assert_eq!(snapshot.sessions.len(), 1);
    let session = snapshot.sessions.into_iter().next().unwrap();
    assert_eq!(session.state, SessionState::Working);
    assert!(session
        .capabilities
        .contains(&open_agent_view::domain::Capability::Interrupt));
    session
}

fn run_pty_outer(provider: &str, test_name: &str, marker: &str, ready: &[u8]) {
    let (mut master, slave) = outer_pty();
    set_nonblocking(&master);
    let mut command = Command::new(std::env::current_exe().unwrap());
    command
        .args(["--exact", test_name, "--nocapture"])
        .env(PTY_CHILD, provider)
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
    let mut child = ChildGuard::new(command.spawn().unwrap());
    let mut output = Vec::new();
    read_until(&mut master, &mut output, ready, Duration::from_secs(5));
    master.write_all(b"\x1b[1;2D").unwrap();
    read_until_present(
        &mut master,
        &mut output,
        format!("{marker}_LAUNCH_RETURNED").as_bytes(),
        Duration::from_secs(5),
    );
    read_until_present(
        &mut master,
        &mut output,
        format!("{marker}_REATTACHING").as_bytes(),
        Duration::from_secs(5),
    );
    read_until(&mut master, &mut output, ready, Duration::from_secs(5));
    master.write_all(b"\x1b[1;2C").unwrap();
    read_until_present(
        &mut master,
        &mut output,
        format!("{marker}_REATTACHED").as_bytes(),
        Duration::from_secs(5),
    );
    read_until_present(
        &mut master,
        &mut output,
        format!("{marker}_RESUME_EXACT").as_bytes(),
        Duration::from_secs(5),
    );
    read_until_present(
        &mut master,
        &mut output,
        format!("{marker}_CONTROLLER_OK").as_bytes(),
        Duration::from_secs(5),
    );
    let deadline = Instant::now() + Duration::from_secs(5);
    loop {
        if let Some(status) = child.child.try_wait().unwrap() {
            assert!(status.success(), "{}", String::from_utf8_lossy(&output));
            child.reaped = true;
            break;
        }
        assert!(Instant::now() < deadline, "controller child did not exit");
        thread::sleep(Duration::from_millis(10));
    }
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
}

impl Drop for ChildGuard {
    fn drop(&mut self) {
        if self.reaped {
            return;
        }
        unsafe {
            libc::kill(-(self.child.id() as libc::pid_t), libc::SIGKILL);
        }
        let _ = self.child.wait();
    }
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
            Err(error) if error.raw_os_error() == Some(libc::EIO) => {}
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

fn read_until_present(master: &mut File, output: &mut Vec<u8>, needle: &[u8], timeout: Duration) {
    if occurrences(output, needle) != 0 {
        return;
    }
    read_until(master, output, needle, timeout);
}

fn occurrences(haystack: &[u8], needle: &[u8]) -> usize {
    haystack
        .windows(needle.len())
        .filter(|window| *window == needle)
        .count()
}

#![cfg(unix)]

use std::fs::File;
use std::io::{Read, Write};
use std::os::fd::FromRawFd;
use std::os::unix::process::CommandExt;
use std::process::{Command, Stdio};
use std::thread;
use std::time::{Duration, Instant};

use open_agent_view::native_session::{self, NativeSessionExit};

const CHILD_ENV: &str = "OAV_NATIVE_BACKGROUND_CHILD";
const SCREEN_GATED_CHILD_ENV: &str = "OAV_SCREEN_GATED_INPUT_CHILD";

#[test]
fn boundary_arrows_and_shift_shortcuts_background_and_reattach_the_native_screen() {
    if std::env::var_os(CHILD_ENV).is_some() {
        run_child_scenario();
        return;
    }

    let (mut master, slave) = outer_pty();
    set_nonblocking(&master);
    let mut command = Command::new(std::env::current_exe().unwrap());
    command
        .args([
            "--exact",
            "boundary_arrows_and_shift_shortcuts_background_and_reattach_the_native_screen",
            "--nocapture",
        ])
        .env(CHILD_ENV, "1")
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
    let mut child = command.spawn().unwrap();
    let mut output = Vec::new();

    read_until(
        &mut master,
        &mut output,
        b"NATIVE_READY",
        Duration::from_secs(4),
    );
    // A plain arrow must first reach the provider so line editing still works.
    master.write_all(b"\x1b[D").unwrap();
    read_until(
        &mut master,
        &mut output,
        b"PLAIN_LEFT_RECEIVED",
        Duration::from_secs(4),
    );
    assert_eq!(occurrence_count(&output, b"DASHBOARD_RETURNED"), 0);

    // At the resulting cursor boundary, the first arrow shows a bounded hint
    // and the same arrow inside the return window backgrounds the frontend.
    master.write_all(b"\x1b[D").unwrap();
    read_until(
        &mut master,
        &mut output,
        "Press ← again".as_bytes(),
        Duration::from_secs(4),
    );
    assert_eq!(occurrence_count(&output, b"DASHBOARD_RETURNED"), 0);
    thread::sleep(Duration::from_millis(1800));
    drain_for(&mut master, &mut output, Duration::from_millis(100));
    master.write_all(b"\x1b[D").unwrap();
    read_until(
        &mut master,
        &mut output,
        "Press ← again".as_bytes(),
        Duration::from_secs(4),
    );
    assert_eq!(
        occurrence_count(&output, b"DASHBOARD_RETURNED"),
        0,
        "an expired return window must forward the next arrow and re-arm"
    );
    master.write_all(b"\x1b[D").unwrap();
    read_until(
        &mut master,
        &mut output,
        b"DASHBOARD_RETURNED",
        Duration::from_secs(4),
    );
    if occurrence_count(&output, b"NATIVE_READY") < 2 {
        read_until(
            &mut master,
            &mut output,
            b"NATIVE_READY",
            Duration::from_secs(4),
        );
    }
    master.write_all(b"\x1b[C").unwrap();
    read_until(
        &mut master,
        &mut output,
        "Press → again".as_bytes(),
        Duration::from_secs(4),
    );
    master.write_all(b"\x1b[C").unwrap();
    read_until(
        &mut master,
        &mut output,
        b"SECOND_RETURN",
        Duration::from_secs(4),
    );
    if occurrence_count(&output, b"NATIVE_READY") < 3 {
        read_until(
            &mut master,
            &mut output,
            b"NATIVE_READY",
            Duration::from_secs(4),
        );
    }
    master.write_all(b"\x1b[1;2C").unwrap();
    read_until(
        &mut master,
        &mut output,
        b"THIRD_RETURN",
        Duration::from_secs(4),
    );

    // macOS CI can take several seconds to reap a stopped process group after
    // the final reattach. Keep the assertion bounded without making the
    // platform scheduler part of the behavior under test.
    let deadline = Instant::now() + Duration::from_secs(10);
    let status = loop {
        if let Some(status) = child.try_wait().unwrap() {
            break status;
        }
        assert!(Instant::now() < deadline, "child test did not exit");
        thread::sleep(Duration::from_millis(10));
    };
    assert!(status.success(), "{}", String::from_utf8_lossy(&output));
    assert!(
        occurrence_count(&output, b"NATIVE_READY") >= 3,
        "native screen was not replayed on reattach: {}",
        String::from_utf8_lossy(&output)
    );
}

#[test]
fn queued_task_reaches_only_the_authenticated_native_editor_in_a_real_pty() {
    if std::env::var_os(SCREEN_GATED_CHILD_ENV).is_some() {
        let mut provider = Command::new("bash");
        provider.args([
            "-c",
            r##"stty raw -echo
printf '\033[2J\033[HRun /login or /provider to get started.'
if IFS= read -r -t 1 -n 1 early; then
  printf '\r\nEARLY_INPUT:%s' "$early"
  exit 71
fi
printf '\r\nSend /help for help information.'
IFS= read -r -n 29 prompt
prompt=${prompt%$'\r'}
printf '\r\nRECEIVED_TASK:%s' "$prompt"
"##,
        ]);
        let exit = native_session::run_with_initial_input_after_screen(
            provider,
            "kimi:host:screen-gate-test",
            b"fix the authenticated parser\r",
            "Send /help for help information.",
        )
        .unwrap();
        assert!(matches!(exit, NativeSessionExit::Exited(status) if status.success()));
        native_session::shutdown_all();
        // This branch is already an isolated child test process with its own
        // controlling terminal. Exit directly after the exact provider has
        // completed: the macOS Intel test harness can otherwise retain that
        // handed-back PTY for several seconds after the assertion succeeds.
        std::process::exit(0);
    }

    let (mut master, slave) = outer_pty();
    set_nonblocking(&master);
    let mut command = Command::new(std::env::current_exe().unwrap());
    command
        .args([
            "--exact",
            "queued_task_reaches_only_the_authenticated_native_editor_in_a_real_pty",
            "--nocapture",
        ])
        .env(SCREEN_GATED_CHILD_ENV, "1")
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

    let mut child = command.spawn().unwrap();
    let mut output = Vec::new();
    read_until(
        &mut master,
        &mut output,
        b"Run /login or /provider to get started.",
        Duration::from_secs(4),
    );
    read_until(
        &mut master,
        &mut output,
        b"RECEIVED_TASK:fix the authenticated parser",
        Duration::from_secs(4),
    );
    assert!(
        !output
            .windows(b"EARLY_INPUT".len())
            .any(|part| part == b"EARLY_INPUT"),
        "queued task leaked into the login screen: {}",
        String::from_utf8_lossy(&output)
    );

    let deadline = Instant::now() + Duration::from_secs(4);
    loop {
        if let Some(status) = child.try_wait().unwrap() {
            assert!(status.success(), "{}", String::from_utf8_lossy(&output));
            break;
        }
        assert!(Instant::now() < deadline, "screen-gated child did not exit");
        thread::sleep(Duration::from_millis(10));
    }
}

fn run_child_scenario() {
    let mut first = Command::new("bash");
    first.args([
        "-c",
        r"stty raw -echo; printf '\033[2J\033[HNATIVE_READY'; IFS= read -r -n 3 key; if [[ $key == $'\033[D' ]]; then printf '\r\nPLAIN_LEFT_RECEIVED'; else printf '\r\nWRONG_ARROW_BYTES'; fi; exec sleep 60",
    ]);
    assert!(matches!(
        native_session::run(first, "provider:host:test").unwrap(),
        NativeSessionExit::Backgrounded
    ));
    println!("DASHBOARD_RETURNED");

    // The command is intentionally invalid: reattachment must use the exact
    // retained frontend instead of spawning this replacement.
    let second = Command::new("definitely-not-a-provider-command");
    assert!(matches!(
        native_session::run(second, "provider:host:test").unwrap(),
        NativeSessionExit::Backgrounded
    ));
    println!("SECOND_RETURN");

    let third = Command::new("definitely-not-a-provider-command");
    assert!(matches!(
        native_session::run(third, "provider:host:test").unwrap(),
        NativeSessionExit::Backgrounded
    ));
    println!("THIRD_RETURN");
    native_session::shutdown_all();
    // This scenario already runs in a dedicated subprocess because it needs a
    // controlling terminal. Exit that subprocess directly after explicit
    // cleanup: Apple's test harness can otherwise retain the handed-back PTY
    // even though the exact provider process has been reaped.
    std::process::exit(0);
}

fn outer_pty() -> (File, File) {
    let mut master = -1;
    let mut slave = -1;
    let mut size = libc::winsize {
        ws_row: 24,
        ws_col: 80,
        ws_xpixel: 0,
        ws_ypixel: 0,
    };
    let size_ptr = &mut size as *mut libc::winsize;
    let result = unsafe {
        libc::openpty(
            &mut master,
            &mut slave,
            std::ptr::null_mut(),
            std::ptr::null_mut(),
            size_ptr,
        )
    };
    assert_eq!(result, 0);
    unsafe { (File::from_raw_fd(master), File::from_raw_fd(slave)) }
}

fn set_nonblocking(file: &File) {
    let flags = unsafe { libc::fcntl(std::os::fd::AsRawFd::as_raw_fd(file), libc::F_GETFL) };
    assert!(flags >= 0);
    assert_eq!(
        unsafe {
            libc::fcntl(
                std::os::fd::AsRawFd::as_raw_fd(file),
                libc::F_SETFL,
                flags | libc::O_NONBLOCK,
            )
        },
        0
    );
}

fn read_until(master: &mut File, output: &mut Vec<u8>, needle: &[u8], timeout: Duration) {
    let previous_matches = occurrence_count(output, needle);
    let deadline = Instant::now() + timeout;
    let mut bytes = [0_u8; 4096];
    loop {
        match master.read(&mut bytes) {
            Ok(0) => {}
            Ok(count) => output.extend_from_slice(&bytes[..count]),
            Err(error) if error.kind() == std::io::ErrorKind::WouldBlock => {}
            Err(error) => panic!("failed to read outer PTY: {error}"),
        }
        let matches = occurrence_count(output, needle);
        if matches > previous_matches {
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

fn drain_for(master: &mut File, output: &mut Vec<u8>, duration: Duration) {
    let deadline = Instant::now() + duration;
    let mut bytes = [0_u8; 4096];
    while Instant::now() < deadline {
        match master.read(&mut bytes) {
            Ok(0) => {}
            Ok(count) => output.extend_from_slice(&bytes[..count]),
            Err(error) if error.kind() == std::io::ErrorKind::WouldBlock => {}
            Err(error) => panic!("failed to drain outer PTY: {error}"),
        }
        thread::sleep(Duration::from_millis(5));
    }
}

fn occurrence_count(haystack: &[u8], needle: &[u8]) -> usize {
    haystack
        .windows(needle.len())
        .filter(|item| *item == needle)
        .count()
}

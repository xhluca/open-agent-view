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

#[test]
fn shift_left_backgrounds_and_enter_style_reattach_restores_the_native_screen() {
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
            "shift_left_backgrounds_and_enter_style_reattach_restores_the_native_screen",
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
    // Plain arrows must reach the provider so users can edit its input line.
    master.write_all(b"\x1b[D\x1b[C").unwrap();
    thread::sleep(Duration::from_millis(50));
    assert_eq!(occurrence_count(&output, b"DASHBOARD_RETURNED"), 0);
    master.write_all(b"\x1b[1;2D").unwrap();
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
    master.write_all(b"\x1b[1;2D").unwrap();
    read_until(
        &mut master,
        &mut output,
        b"SECOND_RETURN",
        Duration::from_secs(4),
    );

    let deadline = Instant::now() + Duration::from_secs(4);
    let status = loop {
        if let Some(status) = child.try_wait().unwrap() {
            break status;
        }
        assert!(Instant::now() < deadline, "child test did not exit");
        thread::sleep(Duration::from_millis(10));
    };
    assert!(status.success(), "{}", String::from_utf8_lossy(&output));
    assert!(
        occurrence_count(&output, b"NATIVE_READY") >= 2,
        "native screen was not replayed on reattach: {}",
        String::from_utf8_lossy(&output)
    );
}

fn run_child_scenario() {
    let mut first = Command::new("sh");
    first.args([
        "-c",
        r"printf '\033[2J\033[HNATIVE_READY'; while :; do sleep 1; done",
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
    native_session::shutdown_all();
}

fn outer_pty() -> (File, File) {
    let mut master = -1;
    let mut slave = -1;
    let size = libc::winsize {
        ws_row: 24,
        ws_col: 80,
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

fn occurrence_count(haystack: &[u8], needle: &[u8]) -> usize {
    haystack
        .windows(needle.len())
        .filter(|item| *item == needle)
        .count()
}

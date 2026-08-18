#![cfg(unix)]

use std::fs::File;
use std::io::{self, Read, Write};
use std::os::fd::{AsRawFd, FromRawFd, RawFd};
use std::path::PathBuf;
use std::process::{Child, Command, Stdio};
use std::thread;
use std::time::{Duration, Instant};

use tempfile::TempDir;

const ESC: &[u8] = b"\x1b";
const ENTER: &[u8] = b"\r";
const UP: &[u8] = b"\x1b[A";
const DOWN: &[u8] = b"\x1b[B";
const CTRL_A: &[u8] = b"\x01";
const CTRL_J: &[u8] = b"\x0a";
const CTRL_R: &[u8] = b"\x12";
const CTRL_S: &[u8] = b"\x13";
const CTRL_X: &[u8] = b"\x18";

struct PtyApp {
    child: Child,
    master: File,
    parser: vt100::Parser,
    raw: Vec<u8>,
    _home: TempDir,
}

impl PtyApp {
    fn spawn(columns: u16, rows: u16) -> Self {
        Self::spawn_fixture(columns, rows, "populated-sessions.json")
    }

    fn spawn_fixture(columns: u16, rows: u16, fixture_name: &str) -> Self {
        let (master, slave) = open_pty(columns, rows).expect("create PTY");
        let master = unsafe { File::from_raw_fd(master) };
        set_nonblocking(&master).expect("make PTY master nonblocking");

        let stdin = duplicate_file(slave).expect("duplicate slave for stdin");
        let stdout = duplicate_file(slave).expect("duplicate slave for stdout");
        let stderr = unsafe { File::from_raw_fd(slave) };
        let home = tempfile::tempdir().expect("create isolated home");
        let fixture = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
            .join("fixtures")
            .join(fixture_name);
        let mut command = Command::new(env!("CARGO_BIN_EXE_coding-agents"));
        command
            .args([
                "--fixture",
                fixture.to_str().expect("UTF-8 fixture path"),
                "--no-host-claude",
                "--no-host-codex",
                "--include-interactive",
                "--refresh-ms",
                "60000",
            ])
            .env("TERM", "xterm-256color")
            .env("HOME", home.path())
            .env("XDG_STATE_HOME", home.path().join("state"))
            .stdin(Stdio::from(stdin))
            .stdout(Stdio::from(stdout))
            .stderr(Stdio::from(stderr));
        let child = command.spawn().expect("launch coding-agents under PTY");

        Self {
            child,
            master,
            parser: vt100::Parser::new(rows, columns, 0),
            raw: Vec::new(),
            _home: home,
        }
    }

    fn send(&mut self, bytes: &[u8]) {
        self.master.write_all(bytes).expect("write key to PTY");
        self.master.flush().expect("flush PTY input");
    }

    fn screen(&mut self) -> String {
        self.drain();
        normalize_screen(&self.parser.screen().contents())
    }

    fn wait_for(&mut self, description: &str, predicate: impl Fn(&str) -> bool) -> String {
        let deadline = Instant::now() + Duration::from_secs(4);
        let mut latest = String::new();
        while Instant::now() < deadline {
            latest = self.screen();
            if predicate(&latest) {
                return latest;
            }
            if self.child.try_wait().expect("poll child").is_some() {
                panic!(
                    "coding-agents exited while waiting for {description}\n--- screen ---\n{latest}\n--- raw ---\n{}",
                    String::from_utf8_lossy(&self.raw)
                );
            }
            thread::sleep(Duration::from_millis(20));
        }
        panic!("timed out waiting for {description}\n--- screen ---\n{latest}");
    }

    fn wait_for_output_after(&mut self, previous_length: usize, description: &str) {
        let deadline = Instant::now() + Duration::from_secs(4);
        while Instant::now() < deadline {
            self.drain();
            if self.raw.len() > previous_length {
                return;
            }
            thread::sleep(Duration::from_millis(20));
        }
        panic!("timed out waiting for {description}");
    }

    fn exit_cleanly(mut self) {
        self.send(ESC);
        let deadline = Instant::now() + Duration::from_secs(4);
        let status = loop {
            self.drain();
            if let Some(status) = self.child.try_wait().expect("poll child exit") {
                break status;
            }
            assert!(
                Instant::now() < deadline,
                "dashboard did not exit after escape"
            );
            thread::sleep(Duration::from_millis(20));
        };
        self.drain();
        assert!(status.success(), "dashboard exited with {status}");
        assert!(
            contains_bytes(&self.raw, b"\x1b[?1049h"),
            "dashboard did not enter the alternate screen"
        );
        assert!(
            contains_bytes(&self.raw, b"\x1b[?1049l"),
            "dashboard did not leave the alternate screen"
        );
        assert!(
            contains_bytes(&self.raw, b"\x1b[?25l") && contains_bytes(&self.raw, b"\x1b[?25h"),
            "dashboard did not restore cursor visibility"
        );
    }

    fn drain(&mut self) {
        let mut buffer = [0_u8; 16 * 1024];
        loop {
            match self.master.read(&mut buffer) {
                Ok(0) => break,
                Ok(count) => {
                    self.parser.process(&buffer[..count]);
                    self.raw.extend_from_slice(&buffer[..count]);
                }
                Err(error) if error.kind() == io::ErrorKind::WouldBlock => break,
                // Linux PTY masters report EIO after the slave side closes.
                Err(error) if error.raw_os_error() == Some(libc::EIO) => break,
                Err(error) => panic!("read PTY: {error}"),
            }
        }
    }
}

impl Drop for PtyApp {
    fn drop(&mut self) {
        if self.child.try_wait().ok().flatten().is_none() {
            let _ = self.child.kill();
            let _ = self.child.wait();
        }
    }
}

#[test]
fn all_supported_providers_coexist_in_one_real_terminal() {
    let mut app = PtyApp::spawn_fixture(150, 36, "all-providers-sessions.json");
    let startup = app.wait_for("seven-provider dashboard", |screen| {
        screen.contains(
            "Antigravity + Claude + Codex + Cursor + GitHub Copilot + OpenCode + Pi",
        ) && screen.contains("pi-refactor")
            && screen.contains("opencode-api")
            && screen.contains("cursor-owned-chat")
            && screen.contains("copilot-acp-session")
            && screen.contains("antigravity-last-conversa")
    });
    assert_lines_fit(&startup, 150);
    for marker in ["C@", "X@", "P@H", "O@H", "R@H", "G@H", "A@H"] {
        assert!(startup.contains(marker), "missing provider marker {marker}");
    }

    app.send(b"/");
    app.send(b"copilot");
    app.send(ENTER);
    let filtered = app.wait_for("Copilot mixed-provider filter", |screen| {
        screen.contains("copilot-acp-session") && !screen.contains("pi-refactor")
    });
    assert_lines_fit(&filtered, 150);

    app.send(CTRL_S);
    let directory = app.wait_for("Copilot directory view", |screen| {
        screen.contains("directory view") && screen.contains("/workspace/copilot-project")
    });
    assert_lines_fit(&directory, 150);
    app.exit_cleanly();
}

#[test]
fn wide_real_tty_exercises_primary_interactions_and_restores_terminal() {
    let mut app = PtyApp::spawn(120, 34);
    let startup = app.wait_for("populated startup view", |screen| {
        screen.contains("Open Agent View v0.1.0")
            && screen.contains("Ready for review")
            && screen.contains("approval-needed")
            && screen.contains("schema-migration")
            && screen.contains("2 awaiting input · 4 working · 2 completed · status view")
    });
    assert_lines_fit(&startup, 120);
    assert!(startup.contains("release-reviewer"));
    assert!(startup.contains("Unknown"));

    app.send(b"?");
    let help = app.wait_for("contextual help", |screen| {
        screen.contains("ctrl+s to switch views")
            && screen.contains("space to inspect")
            && screen.contains("? to close")
    });
    assert_lines_fit(&help, 120);
    app.send(b"?");
    app.wait_for("help close", |screen| !screen.contains("? to close"));

    app.send(CTRL_S);
    let directory = app.wait_for("directory view", |screen| {
        screen.contains("directory view")
            && screen.contains("/workspace/alpha")
            && screen.contains("/workspace/beta")
            && screen.contains("Review · Prepared the release notes")
    });
    assert_lines_fit(&directory, 120);
    app.send(CTRL_S);
    app.wait_for("status view", |screen| screen.contains("status view"));

    app.send(b"/");
    app.wait_for("filter composer", |screen| {
        screen.contains("❯ filter") && screen.contains("enter to apply · esc to cancel")
    });
    app.send(b"approval");
    app.send(ENTER);
    let filtered = app.wait_for("applied filter", |screen| {
        screen.contains("approval-needed") && !screen.contains("release-reviewer")
    });
    assert!(filtered.contains("type to start a new session · / to change filter"));

    app.send(b"/");
    app.wait_for("filter edit", |screen| screen.contains("❯ filter approval"));
    app.send(ESC);
    let filter_cancelled = app.wait_for("filter cancellation", |screen| {
        screen.contains("approval-needed") && !screen.contains("❯ filter approval")
    });
    assert!(!filter_cancelled.contains("release-reviewer"));

    app.send(b"/");
    app.send(&[0x7f; 8]);
    app.send(ENTER);
    app.wait_for("cleared filter", |screen| {
        screen.contains("release-reviewer")
    });

    app.send(b"\t");
    app.wait_for("new task composer", |screen| {
        screen.contains('❯') && screen.contains("enter to create · ctrl+j for newline")
    });
    app.send(b"draft a release");
    app.send(CTRL_J);
    app.send(b"include rollback");
    app.wait_for("multiline new task text", |screen| {
        screen.contains("draft a release") && screen.contains("include rollback")
    });
    app.send(ESC);
    app.wait_for("composer cancellation", |screen| {
        screen.contains("describe a task for a new session")
            && !screen.contains("draft a release")
            && !screen.contains("include rollback")
    });

    // Select the review row deterministically after the earlier filter changed
    // selection. Peek remains useful when provider inspection is disabled: it
    // still shows the fixture summary and a safe refusal notice.
    app.send(b"/");
    app.send(b"release-reviewer");
    app.send(ENTER);
    app.wait_for("review row", |screen| screen.contains("release-reviewer"));
    app.send(b" ");
    let peek = app.wait_for("transcript peek", |screen| {
        screen.contains("release-reviewer · Claude")
            && screen.contains("Prepared the release notes")
            && screen.contains("enter to open native session")
    });
    assert_lines_fit(&peek, 120);
    app.send(ESC);
    app.wait_for("peek close", |screen| {
        !screen.contains("release-reviewer · Claude")
    });

    app.send(CTRL_R);
    app.wait_for("rename composer", |screen| {
        screen.contains("❯ name release-reviewer")
            && screen.contains("enter to save · esc to cancel")
    });
    app.send(ESC);
    app.wait_for("rename cancellation", |screen| !screen.contains("❯ name"));

    app.send(CTRL_R);
    app.send(&[0x7f; 16]);
    app.send(b"reviewer-display-name");
    app.send(ENTER);
    let rename_refused = app.wait_for("unsupported rename submission", |screen| {
        screen.contains("rename is unavailable through the supported provider CLI")
    });
    assert!(!rename_refused.contains("reviewer-display-name"));

    app.exit_cleanly();
}

#[test]
fn fixture_fence_covers_launch_open_reply_interrupt_and_bulk_delete() {
    let mut app = PtyApp::spawn(105, 30);
    app.wait_for("startup", |screen| screen.contains("owned-codex-worker"));

    app.send(b"\t");
    app.send(b"launch-should-stay-in-fixture");
    app.send(ENTER);
    let launch_refused = app.wait_for("fixture-fenced launch", |screen| {
        screen.contains("launch failed:")
            && screen.contains("provider actions are disabled while reading a fixture")
    });
    assert!(!launch_refused.contains("launch-should-stay-in-fixture"));

    app.send(b"/");
    app.send(b"owned-codex-worker");
    app.send(ENTER);
    app.wait_for("owned Codex fixture row", |screen| {
        screen.contains("owned-codex-worker") && screen.contains("Working")
    });

    let alternate_enters_before_open = count_bytes(&app.raw, b"\x1b[?1049h");
    app.send(ENTER);
    let open_refused = app.wait_for("fixture-fenced native open", |screen| {
        screen.contains("failed to open session:")
            && screen.contains("provider actions are disabled while reading a fixture")
            && screen.contains("owned-codex-worker")
    });
    assert_lines_fit(&open_refused, 105);
    assert!(
        count_bytes(&app.raw, b"\x1b[?1049h") > alternate_enters_before_open,
        "native open did not restore the dashboard alternate screen"
    );
    assert!(
        count_bytes(&app.raw, b"\x1b[?1049l") >= 1,
        "native open did not suspend the dashboard before dispatch"
    );

    app.send(b" ");
    app.wait_for("writable reply peek", |screen| {
        screen.contains("owned-codex-worker · Codex") && screen.contains("❯ reply")
    });
    app.send(b"reply-should-stay-in-fixture");
    app.wait_for("reply composition", |screen| {
        screen.contains("reply-should-stay-in-fixture")
    });
    app.send(ENTER);
    let reply_refused = app.wait_for("fixture-fenced reply", |screen| {
        screen.contains("reply refused:")
            && screen.contains("provider actions are disabled while reading a fixture")
    });
    assert!(!reply_refused.contains("reply-should-stay-in-fixture"));
    app.send(ESC);
    app.wait_for("reply peek close", |screen| {
        !screen.contains("owned-codex-worker · Codex")
    });

    app.send(CTRL_X);
    let interrupt_confirm = app.wait_for("interrupt confirmation", |screen| {
        screen.contains("Interrupt the exact running session?")
            && screen.contains("codex:host:owned-worker")
            && screen.contains("Enter confirms; escape keeps it")
    });
    assert_lines_fit(&interrupt_confirm, 105);
    app.send(ENTER);
    app.wait_for("fixture-fenced interrupt", |screen| {
        screen.contains("stop refused:")
            && screen.contains("provider actions are disabled while reading a fixture")
            && !screen.contains("Enter confirms; escape keeps it")
    });

    app.send(b"/");
    app.send(&[0x7f; 18]);
    app.send(b"migration");
    app.send(ENTER);
    app.wait_for("two completed migration rows", |screen| {
        screen.contains("api-migration-review") && screen.contains("schema-migration")
    });
    app.send(UP);
    app.send(CTRL_X);
    let bulk_confirm = app.wait_for("completed-group bulk-delete confirmation", |screen| {
        screen.contains("Delete all 2 sessions in state:Completed?")
            && screen.contains("Enter confirms; escape keeps them")
    });
    assert_lines_fit(&bulk_confirm, 105);
    app.send(ENTER);
    app.wait_for("fixture-fenced bulk delete", |screen| {
        screen.contains("delete refused for api-migration-review:")
            && screen.contains("provider actions are disabled while reading a fixture")
            && !screen.contains("Enter confirms; escape keeps them")
    });

    app.exit_cleanly();
}

#[test]
fn real_tty_renders_actionable_request_and_confirmation_states() {
    let mut app = PtyApp::spawn(100, 28);
    app.wait_for("startup", |screen| screen.contains("release-reviewer"));

    // Filter selects the approval session directly, making approval affordances
    // deterministic without depending on the number of section headers.
    app.send(b"/");
    app.send(b"approval-needed");
    app.send(ENTER);
    app.wait_for("approval row", |screen| screen.contains("approval-needed"));
    app.send(b" ");
    let approval = app.wait_for("approval peek", |screen| {
        screen.contains("approval-needed · Codex")
            && screen.contains("y allow once · n deny")
            && screen.contains("cargo publish --dry-run")
    });
    assert_lines_fit(&approval, 100);
    app.send(b"y");
    let approval_refused = app.wait_for("disabled-controller approval refusal", |screen| {
        screen.contains("approval response refused:")
            && screen.contains("provider actions are disabled while reading a fixture")
    });
    assert!(approval_refused.contains("y allow once · n deny"));
    let output_before_denial = app.raw.len();
    app.send(b"n");
    app.wait_for_output_after(output_before_denial, "disabled-controller denial refusal");
    let denial_refused = app.screen();
    assert!(denial_refused.contains("approval response refused:"));
    assert!(denial_refused.contains("provider actions are disabled while reading a fixture"));
    app.send(ESC);
    app.wait_for("approval peek close", |screen| {
        !screen.contains("approval-needed · Codex")
    });

    // Clear the filter, then select the completed session by a new filter.
    app.send(b"/");
    app.send(&[0x7f; 15]);
    app.send(b"schema-migration");
    app.send(ENTER);
    app.wait_for("completed row", |screen| {
        screen.contains("schema-migration")
    });

    app.send(CTRL_X);
    let delete_confirm = app.wait_for("delete confirmation", |screen| {
        screen.contains("Delete the exact session record?")
            && screen.contains("codex:host:completed")
            && screen.contains("Enter confirms; escape keeps it")
    });
    assert_lines_fit(&delete_confirm, 100);
    app.send(ENTER);
    app.wait_for("disabled-controller delete refusal", |screen| {
        screen.contains("delete refused for schema-migration:")
            && screen.contains("provider actions are disabled while reading a fixture")
            && !screen.contains("Enter confirms; escape keeps it")
    });

    app.send(CTRL_A);
    let archive_confirm = app.wait_for("archive confirmation", |screen| {
        screen.contains("Archive the exact session?")
            && screen.contains("codex:host:completed")
            && screen.contains("Enter confirms; escape keeps it")
    });
    assert_lines_fit(&archive_confirm, 100);
    app.send(ENTER);
    app.wait_for("disabled-controller archive refusal", |screen| {
        screen.contains("archive refused:")
            && screen.contains("provider actions are disabled while reading a fixture")
            && !screen.contains("Enter confirms; escape keeps it")
    });

    // Structured input is accepted only into volatile UI state. A refusal from
    // the disabled fixture controller must not copy the answer into its notice.
    app.send(b"/");
    app.send(&[0x7f; 16]);
    app.send(b"needs-environment");
    app.send(ENTER);
    app.wait_for("structured-input row", |screen| {
        screen.contains("needs-environment")
    });
    app.send(b" ");
    app.wait_for("structured-input peek", |screen| {
        screen.contains("needs-environment · Codex") && screen.contains("❯ answer")
    });
    app.send(b"production-secret-value");
    app.wait_for("structured-input composition", |screen| {
        screen.contains("production-secret-value")
    });
    app.send(ENTER);
    let input_refused = app.wait_for("disabled-controller input refusal", |screen| {
        screen.contains("input response refused:")
            && screen.contains("provider actions are disabled while reading a fixture")
    });
    assert!(!input_refused.contains("production-secret-value"));
    app.send(ESC);
    app.wait_for("structured-input peek close", |screen| {
        !screen.contains("needs-environment · Codex")
    });

    app.exit_cleanly();
}

#[test]
fn narrow_and_tiny_real_ttys_have_bounded_fallback_layouts() {
    let mut narrow = PtyApp::spawn(55, 18);
    let startup = narrow.wait_for("narrow startup", |screen| {
        screen.contains("Open Agent View v0.1.0")
            && screen.contains("2 awaiting · 4 working · 2 completed")
            && screen.contains("release-reviewer")
            && screen.contains("? for shortcuts")
    });
    assert_lines_fit(&startup, 55);
    assert!(!startup.contains("status view"));

    narrow.send(b"?");
    let help = narrow.wait_for("wrapped narrow help", |screen| {
        screen.contains("ctrl+s to switch views") && screen.contains("? to close")
    });
    assert_lines_fit(&help, 55);
    narrow.send(b"?");
    narrow.wait_for("narrow help close", |screen| !screen.contains("? to close"));
    narrow.exit_cleanly();

    let mut tiny = PtyApp::spawn(31, 7);
    let fallback = tiny.wait_for("tiny terminal fallback", |screen| {
        screen.contains("coding-agents needs") && screen.contains("at least 32×8")
    });
    assert_lines_fit(&fallback, 31);
    assert!(!fallback.contains("release-reviewer"));
    tiny.exit_cleanly();
}

#[test]
fn arrow_navigation_and_group_collapse_are_real_terminal_events() {
    let mut app = PtyApp::spawn(90, 24);
    app.wait_for("startup", |screen| screen.contains("release-reviewer"));

    // Down from the selected first row lands on the next group header.
    app.send(DOWN);
    app.send(ENTER);
    let collapsed = app.wait_for("collapsed Needs input group", |screen| {
        screen.contains("Needs input 2")
            && !screen.contains("approval-needed")
            && !screen.contains("needs-environment")
    });
    assert_lines_fit(&collapsed, 90);

    app.send(ENTER);
    app.wait_for("expanded Needs input group", |screen| {
        screen.contains("approval-needed") && screen.contains("needs-environment")
    });
    app.exit_cleanly();
}

fn open_pty(columns: u16, rows: u16) -> io::Result<(RawFd, RawFd)> {
    let mut master = -1;
    let mut slave = -1;
    let size = libc::winsize {
        ws_row: rows,
        ws_col: columns,
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
    if result == -1 {
        Err(io::Error::last_os_error())
    } else {
        Ok((master, slave))
    }
}

fn duplicate_file(fd: RawFd) -> io::Result<File> {
    let duplicate = unsafe { libc::dup(fd) };
    if duplicate == -1 {
        Err(io::Error::last_os_error())
    } else {
        Ok(unsafe { File::from_raw_fd(duplicate) })
    }
}

fn set_nonblocking(file: &File) -> io::Result<()> {
    let flags = unsafe { libc::fcntl(file.as_raw_fd(), libc::F_GETFL) };
    if flags == -1 {
        return Err(io::Error::last_os_error());
    }
    if unsafe { libc::fcntl(file.as_raw_fd(), libc::F_SETFL, flags | libc::O_NONBLOCK) } == -1 {
        return Err(io::Error::last_os_error());
    }
    Ok(())
}

fn normalize_screen(screen: &str) -> String {
    let mut lines = screen.lines().map(str::trim_end).collect::<Vec<_>>();
    while lines.last().is_some_and(|line| line.is_empty()) {
        lines.pop();
    }
    lines.join("\n")
}

fn assert_lines_fit(screen: &str, width: usize) {
    for (index, line) in screen.lines().enumerate() {
        assert!(
            line.chars().count() <= width,
            "line {} exceeded {width} columns: {line:?}",
            index + 1
        );
    }
}

fn contains_bytes(haystack: &[u8], needle: &[u8]) -> bool {
    haystack
        .windows(needle.len())
        .any(|window| window == needle)
}

fn count_bytes(haystack: &[u8], needle: &[u8]) -> usize {
    haystack
        .windows(needle.len())
        .filter(|window| *window == needle)
        .count()
}

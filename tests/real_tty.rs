#![cfg(unix)]

use std::fs::{self, File};
use std::io::{self, Read, Write};
use std::os::fd::{AsRawFd, FromRawFd, RawFd};
use std::os::unix::fs::PermissionsExt;
use std::os::unix::process::CommandExt;
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};
use std::sync::{Mutex, MutexGuard};
use std::thread;
use std::time::{Duration, Instant};

use open_agent_view::domain::{SessionSnapshot, SessionState};
use tempfile::TempDir;

const ESC: &[u8] = b"\x1b";
const ENTER: &[u8] = b"\r";
const UP: &[u8] = b"\x1b[A";
const DOWN: &[u8] = b"\x1b[B";
const LEFT: &[u8] = b"\x1b[D";
const SHIFT_TAB: &[u8] = b"\x1b[Z";
const CTRL_A: &[u8] = b"\x01";
const CTRL_F: &[u8] = b"\x06";
const CTRL_J: &[u8] = b"\x0a";
const CTRL_R: &[u8] = b"\x12";
const CTRL_S: &[u8] = b"\x13";
const CTRL_X: &[u8] = b"\x18";

static PTY_TEST_LOCK: Mutex<()> = Mutex::new(());

fn serialize_real_tty_test() -> MutexGuard<'static, ()> {
    PTY_TEST_LOCK
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner())
}

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
        Self::spawn_configured(columns, rows, |command, _| {
            let fixture = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
                .join("fixtures")
                .join(fixture_name);
            command.args([
                "--fixture",
                fixture.to_str().expect("UTF-8 fixture path"),
                "--all",
                "--no-host-claude",
                "--no-host-codex",
                "--include-interactive",
                "--refresh-ms",
                "60000",
            ]);
        })
    }

    fn spawn_configured(
        columns: u16,
        rows: u16,
        configure: impl FnOnce(&mut Command, &TempDir),
    ) -> Self {
        let (master, slave) = open_pty(columns, rows).expect("create PTY");
        let master = unsafe { File::from_raw_fd(master) };
        set_nonblocking(&master).expect("make PTY master nonblocking");

        let stdin = duplicate_file(slave).expect("duplicate slave for stdin");
        let stdout = duplicate_file(slave).expect("duplicate slave for stdout");
        let stderr = unsafe { File::from_raw_fd(slave) };
        let home = tempfile::tempdir().expect("create isolated home");
        let mut command = Command::new(env!("CARGO_BIN_EXE_open-agent-view"));
        command
            .env("TERM", "xterm-256color")
            .env("HOME", home.path())
            .env("XDG_STATE_HOME", home.path().join("state"));
        configure(&mut command, &home);
        command
            .stdin(Stdio::from(stdin))
            .stdout(Stdio::from(stdout))
            .stderr(Stdio::from(stderr));
        // GitHub's ARM runner uses the conventional 0022 umask. Force that
        // exact fresh-environment condition so state-root permission ordering
        // cannot regress only on a release runner.
        unsafe {
            command.pre_exec(|| {
                libc::umask(0o022);
                Ok(())
            });
        }
        let child = command.spawn().expect("launch open-agent-view under PTY");

        Self {
            child,
            master,
            parser: vt100::Parser::new(rows, columns, 0),
            raw: Vec::new(),
            _home: home,
        }
    }

    fn home_path(&self) -> &Path {
        self._home.path()
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
                    "open-agent-view exited while waiting for {description}\n--- screen ---\n{latest}\n--- raw ---\n{}",
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
        let screen = self.screen();
        panic!("timed out waiting for {description}\n--- screen ---\n{screen}");
    }

    fn wait_for_byte_count(&mut self, needle: &[u8], count: usize, description: &str) {
        let deadline = Instant::now() + Duration::from_secs(8);
        while Instant::now() < deadline {
            self.drain();
            if count_bytes(&self.raw, needle) >= count {
                return;
            }
            if self.child.try_wait().expect("poll child").is_some() {
                panic!("open-agent-view exited while waiting for {description}");
            }
            thread::sleep(Duration::from_millis(20));
        }
        let screen = self.screen();
        panic!("timed out waiting for {description}\n--- screen ---\n{screen}");
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
    let _serial = serialize_real_tty_test();
    let mut app = PtyApp::spawn_fixture(150, 36, "all-providers-sessions.json");
    let startup = app.wait_for("seven-provider dashboard", |screen| {
        screen.contains("Antigravity + Claude + Codex + Cursor + GitHub Copilot + OpenCode + Pi")
            && screen.contains("pi-refactor")
            && screen.contains("opencode-api")
            && screen.contains("cursor-owned-chat")
            && screen.contains("copilot-acp-session")
            && screen.contains("antigravity-last-conversa")
    });
    assert_lines_fit(&startup, 150);
    for (session, provider) in [
        ("release-reviewer", "Claude"),
        ("approval-needed", "Codex"),
        ("pi-refactor", "Pi"),
        ("opencode-api", "OpenCode"),
        ("cursor-owned-chat", "Cursor"),
        ("copilot-acp-session", "GitHub Copilot"),
        ("antigravity-last-conversa", "Antigravity"),
    ] {
        let row = startup
            .lines()
            .find(|line| line.contains(session))
            .unwrap_or_else(|| panic!("missing session row {session}"));
        assert!(
            row.contains(provider),
            "session row {session} did not name {provider}: {row:?}"
        );
    }

    app.send(CTRL_F);
    app.send(b"pi-refactor");
    app.send(ENTER);
    app.wait_for("managed Pi fixture row", |screen| {
        screen.contains("pi-refactor") && !screen.contains("cursor-owned-chat")
    });
    let leaves_before_open = count_bytes(&app.raw, b"\x1b[?1049l");
    let enters_before_open = count_bytes(&app.raw, b"\x1b[?1049h");
    app.send(ENTER);
    let native_open_refused = app.wait_for("managed Pi native-open route", |screen| {
        screen.contains("failed to open session:")
            && screen.contains("provider actions are disabled while reading a fixture")
            && screen.contains("pi-refactor")
            && !screen.contains("pi-refactor · Pi · host")
    });
    assert_lines_fit(&native_open_refused, 150);
    assert!(
        count_bytes(&app.raw, b"\x1b[?1049l") > leaves_before_open,
        "Enter on managed Pi did not suspend the dashboard for native open"
    );
    assert!(
        count_bytes(&app.raw, b"\x1b[?1049h") > enters_before_open,
        "Enter on managed Pi did not restore the dashboard after native open"
    );

    // Space, and only Space, owns the bounded inline transcript/actions panel.
    app.send(b" ");
    app.wait_for("managed Pi reply affordance", |screen| {
        screen.contains("pi-refactor · Pi") && screen.contains("❯ reply")
    });
    app.send(b"fixture-pi-reply");
    app.send(ENTER);
    app.wait_for("fixture-fenced Pi reply", |screen| {
        screen.contains("reply refused:")
            && screen.contains("provider actions are disabled while reading a fixture")
    });
    // A refused reply clears the draft but remains in Peek. One Escape closes
    // Peek; sending another before observing the list can quit the dashboard.
    app.send(ESC);
    app.wait_for("managed Pi peek close", |screen| {
        !screen.contains("pi-refactor · Pi") && screen.contains("pi-refactor")
    });
    let hide_started = Instant::now();
    app.send(CTRL_X);
    app.wait_for("completed Pi Ctrl+X remains responsive", |screen| {
        screen.contains("hid 1 session locally") && !screen.contains("pi-refactor")
    });
    assert!(
        hide_started.elapsed() < Duration::from_millis(750),
        "completed Pi Ctrl+X stalled the terminal"
    );

    app.send(CTRL_F);
    app.send(&[0x7f; 64]);
    app.send(b"cursor-owned-chat");
    app.send(ENTER);
    app.wait_for("managed Cursor fixture row", |screen| {
        screen.contains("cursor-owned-chat") && screen.contains("Working")
    });
    app.send(CTRL_X);
    app.wait_for("managed Cursor interrupt affordance", |screen| {
        screen.contains("stop refused:")
            && screen.contains("provider actions are disabled while reading a fixture")
    });

    app.send(CTRL_F);
    app.send(&[0x7f; 64]);
    app.send(b"copilot-acp-session");
    app.send(ENTER);
    let filtered = app.wait_for("Copilot mixed-provider filter", |screen| {
        screen.contains("copilot-acp-session") && !screen.contains("pi-refactor")
    });
    assert_lines_fit(&filtered, 150);
    app.send(b" ");
    app.wait_for("managed Copilot approval affordance", |screen| {
        screen.contains("copilot-acp-session · GitHub Copilot")
            && screen.contains("y allow once · n deny")
    });
    app.send(b"y");
    app.wait_for("fixture-fenced Copilot approval", |screen| {
        screen.contains("approval response refused:")
            && screen.contains("provider actions are disabled while reading a fixture")
    });
    // Approval refusal also remains in Peek, so wait for one Escape to expose
    // the list before sending any subsequent global shortcut.
    app.send(ESC);
    app.wait_for("managed Copilot peek close", |screen| {
        !screen.contains("copilot-acp-session · GitHub Copilot")
            && screen.contains("copilot-acp-session")
    });

    app.send(CTRL_S);
    let directory = app.wait_for("Copilot directory view", |screen| {
        screen.contains("directory view") && screen.contains("/workspace/copilot-project")
    });
    assert_lines_fit(&directory, 150);
    app.exit_cleanly();
}

#[test]
fn slow_provider_refresh_does_not_block_arrow_navigation_or_exit() {
    let _serial = serialize_real_tty_test();
    let mut app = PtyApp::spawn_configured(100, 28, |command, home| {
        let fake_claude = home.path().join("fake-claude");
        fs::write(
            &fake_claude,
            r#"#!/bin/sh
if [ -f "$OAV_FAKE_CLAUDE_STATE" ]; then
  : > "$OAV_FAKE_CLAUDE_STALL"
  sleep 2
else
  : > "$OAV_FAKE_CLAUDE_STATE"
fi
printf '%s\n' '[{"id":"first","cwd":"/workspace/one","kind":"background","sessionId":"11111111-1111-4111-8111-111111111111","name":"first-agent","status":"busy","state":"working"},{"id":"second","cwd":"/workspace/two","kind":"background","sessionId":"22222222-2222-4222-8222-222222222222","name":"second-agent","status":"busy","state":"working"}]'
"#,
        )
        .expect("write slow fake Claude executable");
        fs::set_permissions(&fake_claude, fs::Permissions::from_mode(0o755))
            .expect("make fake Claude executable");
        command
            .args([
                "--claude-bin",
                fake_claude.to_str().expect("UTF-8 fake Claude path"),
                "--no-host-codex",
                "--no-host-pi",
                "--no-host-opencode",
                "--no-host-copilot",
                "--no-host-cursor",
                "--no-host-antigravity",
                "--include-external",
                "--refresh-ms",
                "1000",
            ])
            .env("OAV_FAKE_CLAUDE_STATE", home.path().join("claude-state"))
            .env("OAV_FAKE_CLAUDE_STALL", home.path().join("claude-stall"));
    });

    app.wait_for("initial fast Claude snapshot", |screen| {
        screen.contains("first-agent") && screen.contains("second-agent")
    });
    thread::sleep(Duration::from_millis(50));
    app.screen();
    let idle_output = app.raw.len();
    thread::sleep(Duration::from_millis(200));
    app.screen();
    assert_eq!(
        app.raw.len(),
        idle_output,
        "an unchanged dashboard emitted an idle repaint"
    );
    let first_navigation_output = app.raw.len();
    app.send(DOWN);
    app.wait_for_output_after(first_navigation_output, "first arrow-key selection repaint");
    assert!(
        contains_bytes(&app.raw[first_navigation_output..], b"first-agent"),
        "first arrow key did not repaint the first session row"
    );

    let stall_marker = app.home_path().join("claude-stall");
    let stall_deadline = Instant::now() + Duration::from_secs(2);
    while !stall_marker.exists() {
        assert!(
            Instant::now() < stall_deadline,
            "slow provider refresh never began"
        );
        app.screen();
        thread::sleep(Duration::from_millis(10));
    }

    let navigation_started = Instant::now();
    app.screen();
    let second_navigation_output = app.raw.len();
    app.send(DOWN);
    app.wait_for_output_after(
        second_navigation_output,
        "second arrow-key selection repaint during stalled refresh",
    );
    assert!(
        navigation_started.elapsed() < Duration::from_millis(750),
        "arrow navigation waited for the provider refresh"
    );
    assert!(
        contains_bytes(&app.raw[second_navigation_output..], b"second-agent"),
        "second arrow key did not repaint the second session row"
    );

    let exit_started = Instant::now();
    app.exit_cleanly();
    assert!(
        exit_started.elapsed() < Duration::from_millis(750),
        "dashboard exit waited for the provider refresh"
    );
}

#[test]
fn slow_initial_provider_does_not_hide_fast_provider_results() {
    let _serial = serialize_real_tty_test();
    let startup_started = Instant::now();
    let mut app = PtyApp::spawn_configured(100, 28, |command, home| {
        let fake_claude = home.path().join("slow-claude");
        fs::write(
            &fake_claude,
            r#"#!/bin/sh
sleep 2
printf '%s\n' '[{"id":"slow","cwd":"/workspace/slow","kind":"background","sessionId":"11111111-1111-4111-8111-111111111111","name":"slow-claude-session","status":"busy","state":"working"}]'
"#,
        )
        .expect("write slow initial Claude executable");
        fs::set_permissions(&fake_claude, fs::Permissions::from_mode(0o755))
            .expect("make slow initial Claude executable runnable");

        let fake_antigravity = home.path().join("fast-agy");
        fs::write(&fake_antigravity, "#!/bin/sh\nexit 0\n")
            .expect("write fake Antigravity executable");
        fs::set_permissions(&fake_antigravity, fs::Permissions::from_mode(0o755))
            .expect("make fake Antigravity executable runnable");
        let antigravity_cache = home
            .path()
            .join(".gemini/antigravity-cli/cache/last_conversations.json");
        let antigravity_workspace = home.path().join("fast");
        fs::create_dir_all(&antigravity_workspace).expect("create Antigravity fixture workspace");
        fs::create_dir_all(
            antigravity_cache
                .parent()
                .expect("Antigravity cache has a parent"),
        )
        .expect("create Antigravity cache directory");
        fs::write(
            antigravity_cache,
            serde_json::json!({
                antigravity_workspace.display().to_string(): "fast-conversation"
            })
            .to_string(),
        )
        .expect("write fast Antigravity cache");

        command.args([
            "--claude-bin",
            fake_claude.to_str().expect("UTF-8 Claude path"),
            "--antigravity-bin",
            fake_antigravity.to_str().expect("UTF-8 Antigravity path"),
            "--no-host-codex",
            "--no-host-pi",
            "--no-host-opencode",
            "--no-host-copilot",
            "--no-host-cursor",
            "--include-external",
            "--refresh-ms",
            "60000",
        ]);
    });

    let partial = app.wait_for("fast provider partial startup", |screen| {
        screen.contains("fast (last conversation)")
            && screen.contains("loading remaining providers… (1/2)")
            && !screen.contains("slow-claude-session")
    });
    assert!(
        startup_started.elapsed() < Duration::from_millis(750),
        "fast provider was hidden behind the slow initial provider"
    );
    assert_lines_fit(&partial, 100);

    app.wait_for("complete initial provider snapshot", |screen| {
        screen.contains("fast (last conversation)") && screen.contains("slow-claude-session")
    });
    app.exit_cleanly();
}

#[test]
fn hundreds_of_sessions_coalesce_arrow_bursts_without_output_backlog() {
    let _serial = serialize_real_tty_test();
    // The 36-row viewport admits the maximum 25-row page. A separate test
    // verifies adaptive smaller pages keep Show-more visible in 34 rows.
    let mut app = PtyApp::spawn_configured(120, 36, |command, home| {
        let template_path = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
            .join("fixtures")
            .join("all-providers-sessions.json");
        let template: SessionSnapshot =
            serde_json::from_slice(&fs::read(template_path).expect("read stress fixture template"))
                .expect("parse stress fixture template");
        let template = template
            .sessions
            .into_iter()
            .find(|session| session.state == SessionState::Working)
            .expect("fixture contains a working session");
        let sessions = (0..500)
            .map(|index| {
                let mut session = template.clone();
                session.id = format!("stress:host:{index:04}");
                session.provider_session_id = format!("stress-{index:04}");
                session.name = format!("session-{index:04}");
                session.summary = format!("stress session {index:04}");
                session.state = SessionState::Working;
                session.started_at = None;
                session.updated_at = None;
                session.pull_requests = None;
                session
            })
            .collect();
        let fixture = home.path().join("500-sessions.json");
        fs::write(
            &fixture,
            serde_json::to_vec(&SessionSnapshot {
                sessions,
                warnings: Vec::new(),
            })
            .expect("serialize stress fixture"),
        )
        .expect("write stress fixture");
        command.args([
            "--fixture",
            fixture.to_str().expect("UTF-8 stress fixture path"),
            "--all",
            "--include-interactive",
            "--refresh-ms",
            "60000",
        ]);
    });

    app.wait_for("500-session startup", |screen| {
        screen.contains("session-0000")
            && screen.contains("session-0024")
            && !screen.contains("session-0025")
            && !screen.contains("session-0400")
    });
    app.send(&DOWN.repeat(25));
    app.wait_for("selectable show-more control", |screen| {
        screen.contains("Show 25 more · 475 hidden") && screen.contains("enter to show more")
    });
    app.send(ENTER);
    app.wait_for("second bounded session page", |screen| {
        screen.contains("session-0025")
            && screen.contains("session-0026")
            && !screen.contains("session-0050")
    });

    app.screen();
    let output_before = app.raw.len();
    let navigation_started = Instant::now();
    app.send(&DOWN.repeat(200));
    app.wait_for("coalesced 200-arrow destination", |screen| {
        screen.contains("session-0017") && !screen.contains("session-0049")
    });
    let navigation_elapsed = navigation_started.elapsed();
    let navigation_bytes = app.raw.len() - output_before;

    assert!(
        navigation_elapsed < Duration::from_millis(750),
        "200 queued arrows took {navigation_elapsed:?} with 500 sessions"
    );
    assert!(
        navigation_bytes < 24 * 1024,
        "200 queued arrows emitted {navigation_bytes} bytes instead of coalescing frames"
    );
    app.send(b" ");
    app.wait_for("exact selected session after arrow burst", |screen| {
        screen.contains("session-0017 ·")
    });
    app.send(ESC);
    app.wait_for("stress peek close", |screen| {
        !screen.contains("session-0017 ·")
    });

    let command_started = Instant::now();
    app.send(b"/help");
    app.send(ENTER);
    app.wait_for("dashboard slash-command help", |screen| {
        screen.contains("commands: /harness [NAME] · /model [NAME|default]")
            && screen.contains("/completed [show|hide]")
            && screen.contains("/filter TEXT")
    });
    assert!(
        command_started.elapsed() < Duration::from_millis(750),
        "slash-command handling stalled with 500 sessions"
    );

    app.send(b"\t");
    app.wait_for("stress task composer", |screen| {
        screen.contains("new task · harness Claude · model default")
    });
    app.screen();
    let typing_output_before = app.raw.len();
    let typing_started = Instant::now();
    app.send(&[b'x'; 200]);
    app.wait_for_output_after(typing_output_before, "coalesced 200-character task input");
    assert!(
        typing_started.elapsed() < Duration::from_millis(750),
        "200 typed characters stalled with 500 sessions"
    );
    assert!(
        app.raw.len() - typing_output_before < 24 * 1024,
        "typed input produced an unbounded repaint backlog"
    );
    app.send(ESC);
    app.wait_for("stress task composer close", |screen| {
        !screen.contains("new task · harness Claude · model default")
    });
    app.exit_cleanly();
}

#[cfg(target_os = "linux")]
#[test]
fn harness_picker_switches_visible_backends_without_losing_the_draft() {
    let _serial = serialize_real_tty_test();
    let mut app = PtyApp::spawn_configured(100, 28, |command, home| {
        let fake_claude = home.path().join("fake-claude");
        fs::write(&fake_claude, "#!/bin/sh\nprintf '[]\\n'\n")
            .expect("write fake Claude executable");
        fs::set_permissions(&fake_claude, fs::Permissions::from_mode(0o755))
            .expect("make fake Claude executable runnable");
        let fake_pi = home.path().join("fake-pi");
        fs::write(&fake_pi, "#!/bin/sh\nexit 1\n").expect("write fake Pi executable");
        fs::set_permissions(&fake_pi, fs::Permissions::from_mode(0o755))
            .expect("make fake Pi executable runnable");
        command.args([
            "--claude-bin",
            fake_claude.to_str().expect("UTF-8 Claude path"),
            "--pi-bin",
            fake_pi.to_str().expect("UTF-8 Pi path"),
            "--no-host-codex",
            "--no-host-opencode",
            "--no-host-copilot",
            "--no-host-cursor",
            "--no-host-antigravity",
            "--include-external",
            "--refresh-ms",
            "60000",
        ]);
    });

    app.wait_for("isolated harness-picker startup", |screen| {
        screen.contains("Open Agent View") && !screen.contains("loading")
    });
    app.send(b"keep this draft");
    app.wait_for("Claude draft composer", |screen| {
        screen.contains("new task · harness Claude · model default")
            && screen.contains("keep this draft")
    });
    app.send(b"\t");
    app.wait_for("two-harness picker", |screen| {
        screen.contains("choose harness · 1/2")
            && screen.contains("1  Claude")
            && screen.contains("2  Pi")
    });
    app.send(DOWN);
    app.wait_for("Pi preview", |screen| {
        screen.contains("choose harness · 2/2")
    });
    app.send(ESC);
    app.wait_for("picker cancellation preserves Claude draft", |screen| {
        screen.contains("new task · harness Claude · model default")
            && screen.contains("keep this draft")
    });

    app.send(b"\t");
    app.send(b"2");
    app.wait_for("direct Pi harness selection", |screen| {
        screen.contains("new task · harness Pi · model default")
            && screen.contains("keep this draft")
    });
    app.send(b"\t");
    app.send(UP);
    app.send(ENTER);
    app.wait_for("arrow and Enter Claude selection", |screen| {
        screen.contains("new task · harness Claude · model default")
            && screen.contains("keep this draft")
    });
    app.send(ESC);
    app.wait_for("harness composer cancellation", |screen| {
        screen.contains("describe a task · /help for commands")
            && !screen.contains("keep this draft")
    });
    app.send(b"/harness\r");
    app.wait_for("slash-opened harness picker", |screen| {
        screen.contains("choose harness · 1/2")
    });
    app.send(ESC);
    app.wait_for("slash picker returns to composer", |screen| {
        screen.contains("new task · harness Claude · model default")
            && !screen.contains("┌ choose harness")
    });
    app.send(ESC);
    app.wait_for("slash harness composer close", |screen| {
        screen.contains("describe a task · /help for commands")
    });
    app.exit_cleanly();
}

#[cfg(target_os = "linux")]
#[test]
fn cursor_login_reloads_account_models_and_launches_without_freezing_the_tui() {
    let _serial = serialize_real_tty_test();
    let mut app = PtyApp::spawn_configured(110, 30, |command, home| {
        let executable = home.path().join("cursor-agent");
        fs::write(
            &executable,
            r##"#!/bin/sh
case "${1:-}" in
  models)
    if [ ! -f "$HOME/cursor-authenticated" ]; then
      printf '%s\n' 'No models available for this account'
      exit 1
    fi
    printf '%s\n' 'auto  Recommended' 'claude-sonnet-4.6  Sonnet'
    ;;
  login)
    printf '%s\n' 'CURSOR INTERACTIVE LOGIN'
    IFS= read -r answer
    : > "$HOME/cursor-authenticated"
    ;;
  create-chat)
    sleep 1
    printf '%s\n' 'cursor-auth-session'
    ;;
  *)
    printf '%s\n' "$*" > "$HOME/cursor-launch-arguments"
    printf '%s\n' '{"type":"system","subtype":"init","cwd":"/work","session_id":"cursor-auth-session","model":"claude-sonnet-4.6"}'
    printf '%s\n' '{"type":"result","subtype":"success","is_error":false,"result":"finished","session_id":"cursor-auth-session"}'
    ;;
esac
"##,
        )
        .expect("write fake Cursor executable");
        fs::set_permissions(&executable, fs::Permissions::from_mode(0o700))
            .expect("make fake Cursor executable runnable");
        command.args([
            "--cursor-bin",
            executable.to_str().expect("UTF-8 fake Cursor path"),
            "--no-host-claude",
            "--no-host-codex",
            "--no-host-pi",
            "--no-host-opencode",
            "--no-host-copilot",
            "--no-host-antigravity",
            "--refresh-ms",
            "60000",
        ]);
    });

    app.wait_for("Cursor-only startup", |screen| {
        screen.contains("Open Agent View") && !screen.contains("loading provider sessions")
    });
    app.send(b"/harness cursor");
    app.send(ENTER);
    app.wait_for("Cursor harness selected", |screen| {
        screen.contains("new tasks will use the Cursor harness")
    });
    app.send(b"cursor login task");
    app.wait_for("Cursor task draft", |screen| {
        screen.contains("new task · harness Cursor · model default")
            && screen.contains("cursor login task")
    });
    app.send(ENTER);
    app.wait_for("Cursor sign-in is actionable", |screen| {
        screen.contains("choose Cursor model")
            && screen.contains("sign in")
            && screen.contains("Cursor is not authenticated")
    });
    let leaves_before = count_bytes(&app.raw, b"\x1b[?1049l");
    app.send(ENTER);
    app.wait_for_byte_count(
        b"\x1b[?1049l",
        leaves_before + 1,
        "dashboard suspension for Cursor login",
    );
    app.wait_for("Cursor native login prompt", |screen| {
        screen.contains("CURSOR INTERACTIVE LOGIN")
    });
    app.send(ENTER);
    app.wait_for("authenticated Cursor account model catalog", |screen| {
        screen.contains("choose Cursor model")
            && screen.contains("claude-sonnet-4.6")
            && !screen.contains("sign in")
    });
    app.send(DOWN);
    app.send(DOWN);
    app.send(ENTER);
    app.wait_for("exact Cursor account model selected", |screen| {
        screen.contains("new task · harness Cursor · model claude-sonnet-4.6")
            && screen.contains("cursor login task")
    });
    app.send(ENTER);
    app.wait_for("animated slow Cursor launch", |screen| {
        screen.contains("launching Cursor")
    });
    app.wait_for("new managed Cursor row after launch", |screen| {
        screen.contains("cursor-auth-session")
    });
    let arguments = fs::read_to_string(app.home_path().join("cursor-launch-arguments"))
        .expect("read fake Cursor launch arguments");
    assert!(arguments.contains("--model claude-sonnet-4.6"));
    app.exit_cleanly();
}

#[test]
fn foreground_claude_launch_attaches_full_screen_and_left_returns_to_the_new_row() {
    let _serial = serialize_real_tty_test();
    let mut app = PtyApp::spawn_configured(110, 30, |command, home| {
        let executable = home.path().join("claude");
        fs::write(
            &executable,
            r##"#!/bin/sh
if [ "${1:-}" = "agents" ]; then
  if [ -f "$HOME/claude-session-id" ]; then
    id=$(cat "$HOME/claude-session-id")
    printf '[{"cwd":"%s","kind":"background","sessionId":"%s","name":"new-claude-task","state":"working"}]\n' "$PWD" "$id"
  else
    printf '%s\n' '[]'
  fi
  exit 0
fi
if [ "${1:-}" = "attach" ]; then
  printf '%s\n' "CLAUDE FULL SCREEN ATTACH ${2:-}"
  while :; do sleep 1; done
fi
printf '%s' 'deadbeef-91dc-4b50-a43f-6db2837576fe' > "$HOME/claude-session-id"
printf '%s\n' "$*" > "$HOME/claude-launch-arguments"
sleep 0.3
printf '%s\n' 'backgrounded · deadbeef'
"##,
        )
        .expect("write fake Claude executable");
        fs::set_permissions(&executable, fs::Permissions::from_mode(0o700))
            .expect("make fake Claude executable runnable");
        command.args([
            "--claude-bin",
            executable.to_str().expect("UTF-8 fake Claude path"),
            "--no-host-codex",
            "--no-host-pi",
            "--no-host-opencode",
            "--no-host-copilot",
            "--no-host-cursor",
            "--no-host-antigravity",
            "--refresh-ms",
            "60000",
        ]);
    });

    app.wait_for("empty Claude startup", |screen| {
        screen.contains("Open Agent View") && !screen.contains("loading provider sessions")
    });
    app.send(b"build the foreground feature");
    app.send(ENTER);
    app.wait_for("animated Claude bootstrap", |screen| {
        screen.contains("launching Claude")
    });
    app.wait_for("Claude native full-screen attach", |screen| {
        screen.contains("CLAUDE FULL SCREEN ATTACH")
    });
    let enters_before = count_bytes(&app.raw, b"\x1b[?1049h");
    app.send(LEFT);
    app.wait_for_byte_count(
        b"\x1b[?1049h",
        enters_before + 1,
        "dashboard restoration after Claude Left",
    );
    let returned = app.wait_for("new Claude row selected after backgrounding", |screen| {
        screen.contains("Open Agent View") && screen.contains("new-claude-task")
    });
    assert!(returned.contains("Claude"));
    assert!(returned.contains("backgrounded new-claude-task"));
    let arguments = fs::read_to_string(app.home_path().join("claude-launch-arguments"))
        .expect("read Claude launch arguments");
    assert!(arguments.contains("--background build the foreground feature"));
    assert!(!arguments.contains("--session-id"));
    app.exit_cleanly();
}

#[test]
fn copilot_login_reloads_the_exact_account_model_catalog() {
    let _serial = serialize_real_tty_test();
    let mut app = PtyApp::spawn_configured(110, 30, |command, home| {
        let executable = home.path().join("copilot");
        fs::write(
            &executable,
            r##"#!/usr/bin/env python3
import json, os, sys
auth = os.path.join(os.environ['HOME'], 'copilot-authenticated')
if len(sys.argv) > 1 and sys.argv[1] == 'login':
    print('COPILOT INTERACTIVE LOGIN', flush=True)
    input()
    open(auth, 'w').close()
    raise SystemExit(0)
if '--acp' in sys.argv:
    initialize = json.loads(sys.stdin.readline())
    print(json.dumps({'jsonrpc':'2.0','id':initialize['id'],'result':{
        'protocolVersion':1,
        'agentCapabilities':{'loadSession':True,'sessionCapabilities':{'list':{},'close':{}}}
    }}, separators=(',', ':')), flush=True)
    new_session = json.loads(sys.stdin.readline())
    print(json.dumps({'jsonrpc':'2.0','id':new_session['id'],'error':{
        'code':-32000,'message':'Authentication required'
    }}, separators=(',', ':')), flush=True)
    raise SystemExit(0)
def receive():
    length = None
    while True:
        line = sys.stdin.buffer.readline()
        if not line: raise SystemExit(0)
        if line in (b'\n', b'\r\n'): break
        if line.lower().startswith(b'content-length:'):
            length = int(line.split(b':', 1)[1])
    return json.loads(sys.stdin.buffer.read(length))
def send(value):
    body = json.dumps(value, separators=(',', ':')).encode()
    sys.stdout.buffer.write(b'Content-Length: %d\r\n\r\n' % len(body) + body)
    sys.stdout.buffer.flush()
request = receive()
if not os.path.exists(auth):
    send({'jsonrpc':'2.0','id':request['id'],'error':{'code':-32000,'message':'Authentication required'}})
    raise SystemExit(0)
send({'jsonrpc':'2.0','id':request['id'],'result':{'ok':True,'protocolVersion':3,'version':'test'}})
request = receive()
send({'jsonrpc':'2.0','id':request['id'],'result':{'models':[
    {'id':'gpt-5.4','name':'GPT 5.4'},
    {'id':'claude-sonnet-4.6','name':'Sonnet'}
]}})
"##,
        )
        .expect("write fake Copilot executable");
        fs::set_permissions(&executable, fs::Permissions::from_mode(0o700))
            .expect("make fake Copilot executable runnable");
        command.args([
            "--copilot-bin",
            executable.to_str().expect("UTF-8 fake Copilot path"),
            "--no-host-claude",
            "--no-host-codex",
            "--no-host-pi",
            "--no-host-opencode",
            "--no-host-cursor",
            "--no-host-antigravity",
            "--refresh-ms",
            "60000",
        ]);
    });

    app.wait_for("Copilot-only startup", |screen| {
        screen.contains("Open Agent View") && !screen.contains("loading provider sessions")
    });
    app.send(b"/harness copilot");
    app.send(ENTER);
    app.send(b"copilot account task");
    app.send(ENTER);
    app.wait_for("Copilot account requires login", |screen| {
        screen.contains("choose GitHub Copilot model")
            && screen.contains("GitHub Copilot is not authenticated")
            && screen.contains("sign in")
    });
    app.send(b"l");
    app.wait_for("Copilot native login prompt", |screen| {
        screen.contains("COPILOT INTERACTIVE LOGIN")
    });
    app.send(ENTER);
    app.wait_for("authenticated Copilot account models", |screen| {
        screen.contains("choose GitHub Copilot model")
            && screen.contains("gpt-5.4")
            && screen.contains("claude-sonnet-4.6")
            && !screen.contains("sign in")
    });
    app.send(DOWN);
    app.send(DOWN);
    app.send(ENTER);
    app.wait_for("exact Copilot account model selected", |screen| {
        screen.contains("new task · harness GitHub Copilot · model gpt-5.4")
            && screen.contains("copilot account task")
    });
    app.send(ESC);
    app.wait_for("Copilot task composer closes", |screen| {
        screen.contains("describe a task · /help for commands")
            && !screen.contains("copilot account task")
    });
    app.exit_cleanly();
}

#[test]
fn antigravity_login_model_selection_and_left_background_are_integrated() {
    let _serial = serialize_real_tty_test();
    let mut app = PtyApp::spawn_configured(110, 30, |command, home| {
        let executable = home.path().join("agy");
        let workspace = home.path().join("antigravity-workspace");
        fs::create_dir(&workspace).expect("create Antigravity workspace");
        fs::write(
            &executable,
            r##"#!/bin/sh
if [ "${1:-}" = "models" ]; then
  if [ ! -f "$HOME/antigravity-authenticated" ]; then
    printf '%s\n' 'Authentication required' >&2
    exit 1
  fi
  printf '%s\n' 'gemini-3-pro  Gemini 3 Pro'
  exit 0
fi
if [ "$#" -eq 0 ]; then
  printf '%s\n' 'ANTIGRAVITY INTERACTIVE LOGIN'
  IFS= read -r answer
  : > "$HOME/antigravity-authenticated"
  exit 0
fi
mkdir -p "$HOME/.gemini/antigravity-cli/cache"
printf '{"%s":"agy-owned-session"}\n' "$PWD" > "$HOME/.gemini/antigravity-cli/cache/last_conversations.json"
printf '%s\n' "$*" > "$HOME/antigravity-launch-arguments"
printf '%s\n' 'ANTIGRAVITY FULL SCREEN SESSION'
while :; do sleep 1; done
"##,
        )
        .expect("write fake Antigravity executable");
        fs::set_permissions(&executable, fs::Permissions::from_mode(0o700))
            .expect("make fake Antigravity executable runnable");
        command.args([
            "--cwd",
            workspace.to_str().expect("UTF-8 Antigravity workspace"),
            "--launch-cwd",
            workspace.to_str().expect("UTF-8 Antigravity workspace"),
            "--antigravity-bin",
            executable.to_str().expect("UTF-8 fake Antigravity path"),
            "--no-host-claude",
            "--no-host-codex",
            "--no-host-pi",
            "--no-host-opencode",
            "--no-host-copilot",
            "--no-host-cursor",
            "--refresh-ms",
            "60000",
        ]);
    });

    app.wait_for("Antigravity-only startup", |screen| {
        screen.contains("Open Agent View") && !screen.contains("loading provider sessions")
    });
    app.send(b"/harness antigravity");
    app.send(ENTER);
    app.send(b"antigravity account task");
    app.send(ENTER);
    app.wait_for("Antigravity requires native login", |screen| {
        screen.contains("choose Antigravity model")
            && screen.contains("not authenticated")
            && screen.contains("sign in")
    });
    app.send(ENTER);
    app.wait_for("Antigravity native login", |screen| {
        screen.contains("ANTIGRAVITY INTERACTIVE LOGIN")
    });
    app.send(ENTER);
    app.wait_for("Antigravity account models", |screen| {
        screen.contains("choose Antigravity model")
            && screen.contains("gemini-3-pro")
            && !screen.contains("sign in")
    });
    app.send(DOWN);
    app.send(ENTER);
    app.wait_for("selected Antigravity model", |screen| {
        screen.contains("new task · harness Antigravity · model gemini-3-pro")
            && screen.contains("antigravity account task")
    });
    app.send(ENTER);
    app.wait_for("Antigravity full-screen launch", |screen| {
        screen.contains("ANTIGRAVITY FULL SCREEN SESSION")
    });
    app.send(LEFT);
    let returned = app.wait_for("owned Antigravity row after Left", |screen| {
        screen.contains("Open Agent View")
            && screen.contains("antigravity-workspace")
            && screen.contains("Most recent Antigravity conversation")
    });
    assert!(returned.contains("Antigravity"));
    let arguments = fs::read_to_string(app.home_path().join("antigravity-launch-arguments"))
        .expect("read Antigravity launch arguments");
    assert!(arguments.contains("--model gemini-3-pro"));
    assert!(arguments.contains("--sandbox"));
    assert!(!arguments.contains("dangerously-skip-permissions"));
    app.exit_cleanly();
}

#[test]
fn completed_history_is_visible_by_default_and_stays_responsive() {
    let _serial = serialize_real_tty_test();
    let mut app = PtyApp::spawn_configured(120, 34, |command, home| {
        let template_path = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
            .join("fixtures")
            .join("all-providers-sessions.json");
        let template: SessionSnapshot =
            serde_json::from_slice(&fs::read(template_path).expect("read completed template"))
                .expect("parse completed template");
        let template = template
            .sessions
            .into_iter()
            .find(|session| session.state == SessionState::Completed)
            .expect("fixture contains a completed session");
        let sessions = (0..1_000)
            .map(|index| {
                let mut session = template.clone();
                session.id = format!("completed:host:{index:04}");
                session.provider_session_id = format!("completed-{index:04}");
                session.name = format!("completed-session-{index:04}");
                session.summary = format!("completed history {index:04}");
                session
            })
            .collect();
        let fixture = home.path().join("1000-completed-sessions.json");
        fs::write(
            &fixture,
            serde_json::to_vec(&SessionSnapshot {
                sessions,
                warnings: Vec::new(),
            })
            .expect("serialize completed fixture"),
        )
        .expect("write completed fixture");
        command.args([
            "--fixture",
            fixture.to_str().expect("UTF-8 completed fixture path"),
            "--no-host-providers",
            "--history-limit",
            "1000",
            "--refresh-ms",
            "60000",
        ]);
    });
    let startup = Instant::now();

    let screen = app.wait_for("default bounded completed history", |screen| {
        screen.contains("1000 completed (/completed hide)")
            && screen.contains("completed-session-0000")
            && screen.contains("completed-session-0022")
            && screen.contains("Show 23 more · 977 hidden")
            && !screen.contains("completed-session-0023")
            && !screen.contains("loading provider sessions")
    });

    assert!(
        startup.elapsed() < Duration::from_millis(750),
        "1,000 visible completed sessions delayed first usable screen"
    );
    assert_lines_fit(&screen, 120);

    app.screen();
    let output_before = app.raw.len();
    let navigation_started = Instant::now();
    let mut navigation = DOWN.repeat(208);
    navigation.extend_from_slice(b" ");
    app.send(&navigation);
    app.wait_for("coalesced completed-history arrow burst", |screen| {
        screen.contains("completed-session-0008 ·")
    });
    let navigation_elapsed = navigation_started.elapsed();
    let navigation_bytes = app.raw.len() - output_before;
    assert!(
        navigation_elapsed < Duration::from_millis(750),
        "208 queued arrows took {navigation_elapsed:?} with 1,000 completed sessions"
    );
    assert!(
        navigation_bytes < 24 * 1024,
        "208 queued arrows emitted {navigation_bytes} bytes instead of coalescing frames"
    );
    // Fixture mode refuses provider inspection without leaking into provider
    // I/O. The first Escape clears that refusal; the second closes Peek.
    app.send(ESC);
    app.send(ESC);
    app.wait_for("completed-history peek close", |screen| {
        !screen.contains("completed-session-0008 ·")
    });

    app.send(b"/completed hide");
    app.send(ENTER);
    app.wait_for("in-dashboard completed history disable", |screen| {
        screen.contains("completed hidden (/completed show)")
            && !screen.contains("completed-session-0000")
    });

    app.send(b"/completed show");
    app.send(ENTER);
    app.wait_for("in-dashboard completed history restore", |screen| {
        screen.contains("1000 completed (/completed hide)")
            && screen.contains("completed-session-0000")
            && screen.contains("Show 23 more · 977 hidden")
    });
    app.exit_cleanly();
}

#[test]
fn opencode_history_budget_stays_responsive_and_reports_more_rows() {
    let _serial = serialize_real_tty_test();
    let mut app = PtyApp::spawn_configured(120, 34, |command, home| {
        let executable = home.path().join("opencode");
        fs::write(
            &executable,
            r#"#!/usr/bin/env python3
import json, sys
if len(sys.argv) > 1 and sys.argv[1] == "db":
    print("record")
    for index in range(11):
        print(json.dumps({
            "id": "ses_" + str(index),
            "title": "bounded history " + str(index),
            "created": index,
            "updated": index,
            "projectId": "global",
            "directory": "/work",
        }))
elif "models" in sys.argv:
    print("openai/test-model")
"#,
        )
        .expect("write fake OpenCode");
        fs::set_permissions(&executable, fs::Permissions::from_mode(0o700))
            .expect("make fake OpenCode executable");
        command.args([
            "--opencode-bin",
            executable.to_str().expect("UTF-8 fake OpenCode path"),
            "--history-limit",
            "10",
            "--no-host-claude",
            "--no-host-codex",
            "--no-host-pi",
            "--no-host-copilot",
            "--no-host-cursor",
            "--no-host-antigravity",
            "--include-external",
            "--hide-completed",
            "--refresh-ms",
            "60000",
        ]);
    });

    app.wait_for("active-only OpenCode startup", |screen| {
        screen.contains("completed hidden") && screen.contains("restart without --hide-completed")
    });
    let started = Instant::now();
    app.send(b"/completed show");
    app.send(ENTER);
    let shown = app.wait_for("bounded OpenCode history", |screen| {
        screen.contains("10 completed (/completed hide)")
            && screen.contains("bounded history")
            && screen.contains("history capped")
    });
    assert!(
        started.elapsed() < Duration::from_secs(2),
        "bounded OpenCode history blocked the terminal"
    );
    assert_lines_fit(&shown, 120);

    app.send(DOWN);
    app.wait_for("nonfatal bounded-history warning", |screen| {
        screen.contains("history is limited to 10 records")
    });
    app.send(UP);
    app.send(b"draft");
    app.wait_for("contextual composer footer over warning", |screen| {
        screen.contains("draft") && screen.contains("tab harness · shift+tab model")
    });
    app.send(ESC);
    app.wait_for("composer closes before dashboard exit", |screen| {
        screen.contains("describe a task · /help for commands") && !screen.contains("draft")
    });
    app.exit_cleanly();
}

#[test]
#[ignore = "set OAV_REAL_HOST_HOME for a read-only installed-provider PTY probe"]
fn real_host_auto_resolves_harnesses_models_and_bounded_history() {
    let _serial = serialize_real_tty_test();
    let real_home = std::env::var("OAV_REAL_HOST_HOME")
        .expect("OAV_REAL_HOST_HOME must identify the existing provider home");
    let real_state = std::env::var("OAV_REAL_XDG_STATE_HOME")
        .map(PathBuf::from)
        .unwrap_or_else(|_| PathBuf::from(&real_home).join(".local/state"));
    let mut app = PtyApp::spawn_configured(120, 34, |command, _| {
        command
            .env("HOME", &real_home)
            .env("XDG_STATE_HOME", &real_state)
            .args(["--refresh-ms", "60000"]);
    });

    app.wait_for("real host startup", |screen| {
        screen.contains("Open Agent View") && !screen.contains("loading provider sessions")
    });
    app.send(b"read-only host draft");
    app.wait_for("real host composer", |screen| {
        screen.contains("read-only host draft")
    });
    app.send(b"\t");
    app.wait_for("real host harness palette", |screen| {
        screen.contains("choose harness · 1/6")
            && screen.contains("Claude")
            && screen.contains("Codex")
            && screen.contains("Pi")
            && screen.contains("OpenCode")
            && screen.contains("Cursor")
            && screen.contains("GitHub Copilot")
    });
    for expected in 2..=4 {
        app.send(DOWN);
        app.wait_for("real host harness arrow navigation", |screen| {
            screen.contains(&format!("choose harness · {expected}/6"))
        });
    }
    app.send(ENTER);
    app.wait_for("real host OpenCode selection", |screen| {
        screen.contains("new task · harness OpenCode · model default")
            && screen.contains("read-only host draft")
    });
    app.send(SHIFT_TAB);
    app.wait_for("real host OpenCode model catalog", |screen| {
        screen.contains("choose OpenCode model")
            && screen.contains("Default")
            && !screen.contains("loading models")
    });
    app.send(ESC);
    app.wait_for("real model picker cancellation", |screen| {
        screen.contains("new task · harness OpenCode · model default")
    });
    app.send(ESC);
    app.wait_for("real composer cancellation", |screen| {
        screen.contains("describe a task · /help for commands")
    });

    let started = Instant::now();
    app.send(b"/completed show");
    app.send(ENTER);
    app.wait_for("real bounded completed history", |screen| {
        screen.contains("completed (/completed hide)") && screen.contains("history capped")
    });
    assert!(started.elapsed() < Duration::from_secs(4));
    app.send(DOWN);
    app.send(UP);
    app.send(b"still responsive");
    app.wait_for("real host responsive typing", |screen| {
        screen.contains("still responsive") && screen.contains("tab harness")
    });
    app.send(ESC);
    app.wait_for("real host draft cancellation", |screen| {
        screen.contains("describe a task · /help for commands")
    });
    app.exit_cleanly();
}

#[test]
#[ignore = "set OAV_REAL_PI_SESSION_DIR and OAV_REAL_PI_SESSION_NAME for a read-only host probe"]
fn real_nested_pi_history_opens_and_returns_without_lookup_error() {
    let _serial = serialize_real_tty_test();
    let session_dir = std::env::var("OAV_REAL_PI_SESSION_DIR")
        .expect("OAV_REAL_PI_SESSION_DIR must name the recursive Pi history root");
    let session_name = std::env::var("OAV_REAL_PI_SESSION_NAME")
        .expect("OAV_REAL_PI_SESSION_NAME must identify one existing row");
    let mut app = PtyApp::spawn_configured(120, 34, |command, _| {
        command.args([
            "--pi-session-dir",
            &session_dir,
            "--no-host-claude",
            "--no-host-codex",
            "--no-host-opencode",
            "--no-host-copilot",
            "--no-host-cursor",
            "--no-host-antigravity",
            "--include-external",
            "--all",
            "--refresh-ms",
            "60000",
        ]);
    });

    app.wait_for("real Pi row", |screen| screen.contains(&session_name));
    app.send(CTRL_F);
    app.send(session_name.as_bytes());
    app.send(ENTER);
    app.wait_for("filtered real Pi row", |screen| {
        screen.contains(&session_name)
    });
    let raw_before_open = app.raw.len();
    let leaves_before = count_bytes(&app.raw, b"\x1b[?1049l");
    let enters_before = count_bytes(&app.raw, b"\x1b[?1049h");
    app.send(ENTER);
    app.wait_for_byte_count(
        b"\x1b[?1049l",
        leaves_before + 1,
        "dashboard suspension for real Pi",
    );
    let native = app.wait_for("real Pi native TUI or trust prompt", |screen| {
        (screen.contains("ctrl+c/ctrl+d") && screen.contains("clear/exit"))
            || screen.contains("Trust project folder?")
    });
    if native.contains("Trust project folder?") {
        // Choose the non-persistent refusal so this read-only probe does not
        // modify project trust state in its isolated HOME.
        app.send(&DOWN.repeat(4));
        app.send(ENTER);
        app.wait_for("real Pi native TUI input", |screen| {
            screen.contains("ctrl+c/ctrl+d") && screen.contains("clear/exit")
        });
    }
    app.send(b"\x04");
    app.wait_for_byte_count(
        b"\x1b[?1049h",
        enters_before + 1,
        "dashboard restoration after real Pi",
    );
    let returned = app.wait_for("returned Open Agent View after Pi", |screen| {
        screen.contains("Open Agent View")
    });
    assert!(returned.contains(&session_name));
    assert!(!contains_bytes(
        &app.raw[raw_before_open..],
        b"No session found matching"
    ));
    app.exit_cleanly();
}

#[test]
#[ignore = "set OAV_REAL_CLAUDE_HOME, OAV_REAL_CLAUDE_CWD, and OAV_REAL_CLAUDE_SESSION_NAME for a read-only host probe"]
fn real_claude_attach_explains_and_honors_left_background_return() {
    let _serial = serialize_real_tty_test();
    let claude_home = std::env::var("OAV_REAL_CLAUDE_HOME")
        .expect("OAV_REAL_CLAUDE_HOME must contain the provider state");
    let cwd = std::env::var("OAV_REAL_CLAUDE_CWD")
        .expect("OAV_REAL_CLAUDE_CWD must contain the selected completed session");
    let session_name = std::env::var("OAV_REAL_CLAUDE_SESSION_NAME")
        .expect("OAV_REAL_CLAUDE_SESSION_NAME must identify one completed row");
    let mut app = PtyApp::spawn_configured(120, 34, |command, _| {
        command.env("HOME", &claude_home).args([
            "--all",
            "--include-external",
            "--cwd",
            &cwd,
            "--no-host-codex",
            "--no-host-pi",
            "--no-host-opencode",
            "--no-host-copilot",
            "--no-host-cursor",
            "--no-host-antigravity",
            "--refresh-ms",
            "60000",
        ]);
    });

    app.wait_for("real completed Claude row", |screen| {
        screen.contains(&session_name)
    });
    app.send(CTRL_F);
    app.send(session_name.as_bytes());
    app.send(ENTER);
    app.wait_for("filtered real Claude row", |screen| {
        screen.contains(&session_name) && screen.contains("enter/right open · ← returns")
    });

    let raw_before_open = app.raw.len();
    let leaves_before = count_bytes(&app.raw, b"\x1b[?1049l");
    let enters_before = count_bytes(&app.raw, b"\x1b[?1049h");
    app.send(ENTER);
    app.wait_for_byte_count(
        b"\x1b[?1049l",
        leaves_before + 1,
        "dashboard suspension for real Claude",
    );
    thread::sleep(Duration::from_millis(1_000));
    app.send(b"\x1b[D");
    app.wait_for_byte_count(
        b"\x1b[?1049h",
        enters_before + 1,
        "dashboard restoration after Claude Left",
    );
    app.wait_for("returned Open Agent View after Claude", |screen| {
        screen.contains("Open Agent View") && screen.contains(&session_name)
    });
    assert!(!contains_bytes(
        &app.raw[raw_before_open..],
        b"failed to open session"
    ));
    app.exit_cleanly();
}

#[test]
#[ignore = "requires installed Claude and Pi executables; performs no provider mutation"]
fn real_host_composer_selects_provider_model_filter_and_manual_refresh() {
    let _serial = serialize_real_tty_test();
    let mut app = PtyApp::spawn_configured(120, 34, |command, _| {
        command.args([
            "--no-host-codex",
            "--no-host-opencode",
            "--no-host-copilot",
            "--no-host-cursor",
            "--no-host-antigravity",
            "--refresh-ms",
            "60000",
        ]);
    });
    app.wait_for("real host composer startup", |screen| {
        screen.contains("Open Agent View") && !screen.contains("loading")
    });

    app.send(b"/harness pi\r");
    app.wait_for("Pi harness command", |screen| {
        screen.contains("new tasks will use the Pi harness with its default model")
    });
    app.send(b"x");
    app.wait_for("Pi composer title", |screen| {
        screen.contains("new task · harness Pi · model default")
    });
    app.send(SHIFT_TAB);
    app.wait_for("account-visible Pi model picker", |screen| {
        screen.contains("choose Pi model") && screen.contains("openai/gpt-4")
    });
    app.send(ESC);
    app.wait_for("Pi draft survives model picker", |screen| {
        screen.contains("new task · harness Pi · model default") && screen.contains("❯ x")
    });
    app.send(b"\t");
    app.wait_for("visible harness picker", |screen| {
        screen.contains("choose harness · 2/2")
            && screen.contains("1  Claude")
            && screen.contains("2  Pi")
    });
    app.send(b"\t");
    app.send(ENTER);
    app.wait_for("selected Claude composer title", |screen| {
        screen.contains("new task · harness Claude · model default")
    });
    app.send(SHIFT_TAB);
    app.wait_for("account-visible Claude model picker", |screen| {
        screen.contains("choose Claude model") && screen.contains("opus")
    });
    app.send(b"opus");
    app.wait_for("filtered Claude model picker", |screen| {
        screen.contains("choose Claude model · 1 result") && screen.contains("opus")
    });
    app.send(ENTER);
    app.wait_for("Claude model composer title", |screen| {
        screen.contains("new task · harness Claude · model opus") && screen.contains("❯ x")
    });
    app.send(ESC);
    app.wait_for("model composer close", |screen| {
        !screen.contains("new task · harness Claude · model opus")
    });

    app.send(CTRL_F);
    app.wait_for("dedicated filter composer", |screen| {
        screen.contains("❯ filter") && screen.contains("enter to apply · esc to cancel")
    });
    app.send(ESC);
    app.wait_for("filter composer close", |screen| {
        !screen.contains("❯ filter")
    });
    app.send(b"\x0c");
    app.wait_for("manual provider refresh", |screen| {
        screen.contains("refreshing provider sessions")
    });
    app.exit_cleanly();
}

#[test]
fn owned_dashboard_never_starts_opencode_external_history_query_even_when_completed_is_shown() {
    let _serial = serialize_real_tty_test();
    let mut app = PtyApp::spawn_configured(120, 34, |command, home| {
        let fake_opencode = home.path().join("opencode-history-trap");
        fs::write(
            &fake_opencode,
            "#!/bin/sh\n: > \"$OAV_OPENCODE_HISTORY_CALLED\"\nsleep 2\nexit 1\n",
        )
        .expect("write OpenCode history trap");
        fs::set_permissions(&fake_opencode, fs::Permissions::from_mode(0o755))
            .expect("make OpenCode history trap executable");
        command
            .args([
                "--opencode-bin",
                fake_opencode
                    .to_str()
                    .expect("UTF-8 OpenCode history trap path"),
                "--no-host-claude",
                "--no-host-codex",
                "--no-host-pi",
                "--no-host-copilot",
                "--no-host-cursor",
                "--no-host-antigravity",
                "--refresh-ms",
                "60000",
            ])
            .env(
                "OAV_OPENCODE_HISTORY_CALLED",
                home.path().join("opencode-history-called"),
            );
    });
    let startup = Instant::now();

    app.wait_for("OpenCode history-free default startup", |screen| {
        screen.contains("0 completed (/completed hide)")
            && !screen.contains("loading provider sessions")
    });

    assert!(
        startup.elapsed() < Duration::from_millis(750),
        "default dashboard waited on OpenCode completed history"
    );
    assert!(
        !app.home_path().join("opencode-history-called").exists(),
        "OpenCode external history ran without --include-external"
    );
    app.exit_cleanly();
}

#[test]
fn wide_real_tty_exercises_primary_interactions_and_restores_terminal() {
    let _serial = serialize_real_tty_test();
    let mut app = PtyApp::spawn(120, 34);
    let expected_version = format!("Open Agent View v{}", env!("CARGO_PKG_VERSION"));
    let startup = app.wait_for("populated startup view", |screen| {
        screen.contains(&expected_version)
            && screen.contains("Ready for review")
            && screen.contains("approval-needed")
            && screen.contains("schema-migration")
            && screen.contains("release-reviewer")
            && screen.contains("Unknown")
            && screen.contains(
                "2 awaiting input · 4 working · 2 completed (/completed hide) · status view",
            )
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

    app.send(CTRL_F);
    app.wait_for("filter composer", |screen| {
        screen.contains("❯ filter") && screen.contains("enter to apply · esc to cancel")
    });
    app.send(b"approval");
    app.send(ENTER);
    let filtered = app.wait_for("applied filter", |screen| {
        screen.contains("approval-needed")
            && !screen.contains("release-reviewer")
            && screen.contains("describe a task · ctrl+f to change filter")
    });
    assert!(filtered.contains("describe a task · ctrl+f to change filter"));

    app.send(CTRL_F);
    app.wait_for("filter edit", |screen| screen.contains("❯ filter approval"));
    app.send(ESC);
    let filter_cancelled = app.wait_for("filter cancellation", |screen| {
        screen.contains("approval-needed")
            && !screen.contains("❯ filter approval")
            && !screen.contains("release-reviewer")
    });
    assert!(!filter_cancelled.contains("release-reviewer"));

    app.send(CTRL_F);
    app.send(&[0x7f; 8]);
    app.send(ENTER);
    app.wait_for("cleared filter", |screen| {
        screen.contains("release-reviewer")
    });

    app.send(b"\t");
    app.wait_for("new task composer", |screen| {
        screen.contains('❯') && screen.contains("new task · harness Claude · model default")
    });
    app.send(b"draft a release");
    app.send(CTRL_J);
    app.send(b"include rollback");
    app.wait_for("multiline new task text", |screen| {
        screen.contains("draft a release") && screen.contains("include rollback")
    });
    app.send(ESC);
    app.wait_for("composer cancellation", |screen| {
        screen.contains("describe a task · /help for commands")
            && !screen.contains("draft a release")
            && !screen.contains("include rollback")
    });

    // Select the review row deterministically after the earlier filter changed
    // selection. Peek remains useful when provider inspection is disabled: it
    // still shows the fixture summary and a safe refusal notice.
    app.send(CTRL_F);
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
            && screen.contains("empty resets to provider name")
    });
    app.send(ESC);
    app.wait_for("rename cancellation", |screen| !screen.contains("❯ name"));

    app.send(CTRL_R);
    app.send(&[0x7f; 16]);
    app.send(b"reviewer-display-name");
    app.send(ENTER);
    app.wait_for("local rename submission", |screen| {
        screen.contains("provider title was not changed")
            && screen.contains("No sessions match the current filter")
    });

    let alias_path = app
        .home_path()
        .join("state/open-agent-view/session-aliases.json");
    let stored = fs::read_to_string(&alias_path).expect("read private alias registry");
    assert!(stored.contains("reviewer-display-name"));
    assert_eq!(
        fs::metadata(&alias_path).unwrap().permissions().mode() & 0o777,
        0o600
    );

    app.send(CTRL_F);
    app.send(&[0x7f; 16]);
    app.send(b"reviewer-display-name");
    app.send(ENTER);
    app.wait_for("local alias is filterable", |screen| {
        screen.contains("reviewer-display-name")
    });
    app.send(CTRL_R);
    app.send(&[0x7f; 21]);
    app.send(ENTER);
    app.wait_for("local rename reset", |screen| {
        screen.contains("latest provider title")
            && screen.contains("No sessions match the current filter")
    });
    app.send(CTRL_F);
    app.send(&[0x7f; 21]);
    app.send(b"release-reviewer");
    app.send(ENTER);
    app.wait_for("provider name follows reset", |screen| {
        screen.contains("release-reviewer") && !screen.contains("reviewer-display-name")
    });

    app.exit_cleanly();
}

#[test]
fn fixture_fence_covers_launch_open_reply_interrupt_and_bulk_delete() {
    let _serial = serialize_real_tty_test();
    let mut app = PtyApp::spawn(105, 30);
    app.wait_for("startup", |screen| screen.contains("owned-codex-worker"));

    app.send(b"\t");
    app.send(b"launch-should-stay-in-fixture");
    app.send(ENTER);
    let launch_refused = app.wait_for("fixture-fenced launch", |screen| {
        screen.contains("launch failed:")
            && screen.contains("provider actions are disabled while reading a fixture")
            && !screen.contains("launch-should-stay-in-fixture")
    });
    assert!(!launch_refused.contains("launch-should-stay-in-fixture"));

    app.send(CTRL_F);
    app.send(b"documentation-pass");
    app.send(ENTER);
    app.wait_for("read-only Codex fixture row", |screen| {
        screen.contains("documentation-pass") && !screen.contains("owned-codex-worker")
    });

    let alternate_enters_before_open = count_bytes(&app.raw, b"\x1b[?1049h");
    app.send(ENTER);
    let open_refused = app.wait_for("fixture-fenced native open", |screen| {
        screen.contains("failed to open session:")
            && screen.contains("provider actions are disabled while reading a fixture")
            && screen.contains("documentation-pass")
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

    app.send(CTRL_F);
    app.send(&[0x7f; 64]);
    app.send(b"owned-codex-worker");
    app.send(ENTER);
    app.wait_for("owned Codex fixture row", |screen| {
        screen.contains("owned-codex-worker") && screen.contains("Working")
    });

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
            && !screen.contains("reply-should-stay-in-fixture")
    });
    assert!(!reply_refused.contains("reply-should-stay-in-fixture"));
    app.send(ESC);
    app.wait_for("reply peek close", |screen| {
        !screen.contains("owned-codex-worker · Codex")
    });

    app.send(CTRL_X);
    app.wait_for("fixture-fenced interrupt", |screen| {
        screen.contains("stop refused:")
            && screen.contains("provider actions are disabled while reading a fixture")
    });

    app.send(CTRL_F);
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
    let _serial = serialize_real_tty_test();
    let mut app = PtyApp::spawn(100, 28);
    app.wait_for("startup", |screen| screen.contains("release-reviewer"));

    // Filter selects the approval session directly, making approval affordances
    // deterministic without depending on the number of section headers.
    app.send(CTRL_F);
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
            && screen.contains("y allow once · n deny")
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
    app.send(CTRL_F);
    app.send(&[0x7f; 15]);
    app.send(b"schema-migration");
    app.send(ENTER);
    app.wait_for("completed row", |screen| {
        screen.contains("schema-migration")
    });

    app.send(CTRL_X);
    app.wait_for("disabled-controller delete refusal", |screen| {
        screen.contains("delete refused for schema-migration:")
            && screen.contains("provider actions are disabled while reading a fixture")
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
    app.send(CTRL_F);
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
            && !screen.contains("production-secret-value")
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
    let _serial = serialize_real_tty_test();
    let mut narrow = PtyApp::spawn(55, 18);
    let expected_version = format!("Open Agent View v{}", env!("CARGO_PKG_VERSION"));
    let startup = narrow.wait_for("narrow startup", |screen| {
        screen.contains(&expected_version)
            && screen.contains("2 awaiting · 4 working · 2 completed")
            && screen.contains("release-reviewer")
            && screen.contains("? shortcuts")
            && !screen.contains("status view")
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
        screen.contains("open-agent-view needs")
            && screen.contains("at least 32×8")
            && !screen.contains("release-reviewer")
    });
    assert_lines_fit(&fallback, 31);
    assert!(!fallback.contains("release-reviewer"));
    tiny.exit_cleanly();
}

#[test]
fn arrow_navigation_and_group_collapse_are_real_terminal_events() {
    let _serial = serialize_real_tty_test();
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
    #[allow(unused_mut)] // macOS libc requires a mutable winsize pointer; Linux does not.
    let mut size = libc::winsize {
        ws_row: rows,
        ws_col: columns,
        ws_xpixel: 0,
        ws_ypixel: 0,
    };
    #[cfg(target_os = "macos")]
    let result = unsafe {
        libc::openpty(
            &mut master,
            &mut slave,
            std::ptr::null_mut(),
            std::ptr::null_mut(),
            &mut size,
        )
    };
    #[cfg(not(target_os = "macos"))]
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

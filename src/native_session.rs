//! Provider-neutral native-TUI handoff with a dashboard detach key.
//!
//! Interactive provider clients run behind a private pseudo-terminal.
//! Plain Left and Right remain available to edit the provider's input line. At
//! a cursor boundary, the first arrow is still forwarded and opens a short,
//! visible return window; pressing the same arrow again backgrounds the
//! frontend. Shift+Left and Shift+Right are immediate equivalents. Selecting
//! the same row resumes the exact stopped frontend and screen.

use std::collections::BTreeMap;
use std::fs::File;
use std::io::{self, Read, Write};
#[cfg(unix)]
use std::os::fd::{AsRawFd, FromRawFd};
#[cfg(unix)]
use std::os::unix::process::CommandExt;
#[cfg(unix)]
use std::path::{Path, PathBuf};
use std::process::{Command, ExitStatus, Stdio};
use std::sync::{Mutex, OnceLock};
#[cfg(unix)]
use std::thread;
use std::time::{Duration, Instant};

use anyhow::{anyhow, bail, Context, Result};

const ESCAPE_FLUSH_DELAY: Duration = Duration::from_millis(30);
const ARROW_SETTLE_DELAY: Duration = Duration::from_millis(75);
const ARROW_RETURN_WINDOW: Duration = Duration::from_millis(1600);
const RETURN_HINT_REFRESH: Duration = Duration::from_millis(100);
const EMPTY_PROMPT_MAX_COLUMN: u16 = 4;
const MAX_INITIAL_INPUT_BYTES: usize = 256 * 1024;
#[cfg(unix)]
const FALLBACK_TERMINAL_ROWS: u16 = 24;
#[cfg(unix)]
const FALLBACK_TERMINAL_COLUMNS: u16 = 80;
#[cfg(unix)]
const STOP_GRACE: Duration = Duration::from_millis(250);

#[derive(Debug)]
pub enum NativeSessionExit {
    Backgrounded,
    Exited(ExitStatus),
}

/// Generate a provider-neutral UUIDv4 for native CLIs that accept a caller-
/// supplied session identity. Keeping this here ensures a foreground launch
/// and its later dashboard row use the same unambiguous key.
pub fn new_session_id() -> Result<String> {
    let mut bytes = [0_u8; 16];
    File::open("/dev/urandom")
        .context("failed to open the operating system random source")?
        .read_exact(&mut bytes)
        .context("failed to generate a provider session ID")?;
    bytes[6] = (bytes[6] & 0x0f) | 0x40;
    bytes[8] = (bytes[8] & 0x3f) | 0x80;
    Ok(format!(
        "{:02x}{:02x}{:02x}{:02x}-{:02x}{:02x}-{:02x}{:02x}-{:02x}{:02x}-{:02x}{:02x}{:02x}{:02x}{:02x}{:02x}",
        bytes[0], bytes[1], bytes[2], bytes[3], bytes[4], bytes[5], bytes[6], bytes[7],
        bytes[8], bytes[9], bytes[10], bytes[11], bytes[12], bytes[13], bytes[14], bytes[15]
    ))
}

#[cfg(unix)]
struct DetachedSession {
    child: std::process::Child,
    master: std::fs::File,
    screen: vt100::Parser,
}

#[cfg(unix)]
static DETACHED: OnceLock<Mutex<BTreeMap<String, DetachedSession>>> = OnceLock::new();

/// Run or reattach one provider-native client. Non-TTY callers retain the
/// ordinary inherited-stdio behavior used by scripts and unit-test fixtures.
pub fn run(mut command: Command, session_key: &str) -> Result<NativeSessionExit> {
    validate_session_key(session_key)?;
    #[cfg(unix)]
    if terminal_is_interactive() {
        return run_pty(command, session_key, None);
    }
    let status = command
        .stdin(Stdio::inherit())
        .stdout(Stdio::inherit())
        .stderr(Stdio::inherit())
        .status()
        .context("failed to open provider session")?;
    Ok(NativeSessionExit::Exited(status))
}

/// Run a fresh provider-native client and submit input only after its parsed
/// screen contains an exact readiness marker. This is for native CLIs that do
/// not accept an initial interactive prompt argument. Login, workspace-trust,
/// and setup screens cannot receive the queued task because they do not render
/// the authenticated editor marker.
pub fn run_with_initial_input_after_screen(
    command: Command,
    session_key: &str,
    initial_input: &[u8],
    ready_marker: &str,
) -> Result<NativeSessionExit> {
    validate_session_key(session_key)?;
    if initial_input.is_empty() || initial_input.len() > MAX_INITIAL_INPUT_BYTES {
        bail!("provider-native initial input must contain 1 to {MAX_INITIAL_INPUT_BYTES} bytes");
    }
    if ready_marker.is_empty()
        || ready_marker.len() > 512
        || ready_marker.chars().any(char::is_control)
    {
        bail!("provider-native readiness marker is invalid");
    }
    #[cfg(unix)]
    {
        if !terminal_is_interactive() {
            bail!("screen-gated native input requires an interactive terminal");
        }
        run_pty(
            command,
            session_key,
            Some(ScreenTriggeredInput {
                bytes: initial_input.to_vec(),
                ready_marker: ready_marker.to_owned(),
            }),
        )
    }
    #[cfg(not(unix))]
    {
        let _ = command;
        bail!("screen-gated native input is unavailable on this platform")
    }
}

/// Resume an exact frontend previously backgrounded with a return gesture. Unlike
/// [`run`], this never starts a replacement command when the key is stale.
pub fn resume(session_key: &str) -> Result<NativeSessionExit> {
    validate_session_key(session_key)?;
    #[cfg(unix)]
    {
        if !terminal_is_interactive() {
            bail!("resuming a native session requires an interactive terminal");
        }
        let detached =
            take_detached(session_key)?.context("the background terminal is no longer running")?;
        bridge_session(
            detached.child,
            detached.master,
            detached.screen,
            session_key,
            false,
            None,
        )
    }
    #[cfg(not(unix))]
    bail!("background terminal resume is unavailable on this platform")
}

/// List process-local frontend keys for dashboard discovery. Keys never grant
/// authority over arbitrary processes: every entry was spawned by this OAV
/// process and is still held by its private PTY registry.
pub fn detached_session_keys() -> Vec<String> {
    #[cfg(unix)]
    {
        let Some(registry) = DETACHED.get() else {
            return Vec::new();
        };
        return registry
            .lock()
            .map(|registry| registry.keys().cloned().collect())
            .unwrap_or_default();
    }
    #[cfg(not(unix))]
    Vec::new()
}

/// Whether this process currently retains the exact provider frontend.
pub fn is_backgrounded(session_key: &str) -> bool {
    #[cfg(unix)]
    {
        let Some(registry) = DETACHED.get() else {
            return false;
        };
        let Ok(mut registry) = registry.lock() else {
            return false;
        };
        let alive = registry
            .get_mut(session_key)
            .map(|session| matches!(session.child.try_wait(), Ok(None)))
            .unwrap_or(false);
        if !alive {
            registry.remove(session_key);
        }
        alive
    }
    #[cfg(not(unix))]
    false
}

/// Stop one exact background frontend owned by this process.
pub fn terminate(session_key: &str) -> Result<()> {
    validate_session_key(session_key)?;
    #[cfg(unix)]
    {
        let mut detached =
            take_detached(session_key)?.context("the background terminal is no longer running")?;
        terminate_detached(&mut detached);
        Ok(())
    }
    #[cfg(not(unix))]
    bail!("background terminal control is unavailable on this platform")
}

fn validate_session_key(session_key: &str) -> Result<()> {
    if session_key.is_empty()
        || session_key.len() > 512
        || session_key.chars().any(char::is_control)
    {
        bail!("invalid native session key");
    }
    Ok(())
}

#[cfg(test)]
mod id_tests {
    use super::*;

    #[test]
    fn generated_native_ids_are_distinct_uuid_v4_values() {
        let first = new_session_id().unwrap();
        let second = new_session_id().unwrap();
        assert_ne!(first, second);
        assert_eq!(first.len(), 36);
        assert_eq!(&first[14..15], "4");
        assert!(matches!(&first[19..20], "8" | "9" | "a" | "b"));
    }
}

/// Terminate detached native frontends during a normal dashboard shutdown.
/// Managed provider backends have separate verified ownership and are not
/// targeted here.
pub fn shutdown_all() {
    #[cfg(unix)]
    {
        let Some(registry) = DETACHED.get() else {
            return;
        };
        let sessions = match registry.lock() {
            Ok(mut registry) => std::mem::take(&mut *registry),
            Err(_) => return,
        };
        for (_, mut session) in sessions {
            terminate_detached(&mut session);
        }
    }
}

/// Move a detached provider frontend from a provisional launch key to the
/// stable normalized session key learned after the provider creates it.
pub fn rename_key(from: &str, to: &str) -> Result<()> {
    if from == to {
        return Ok(());
    }
    #[cfg(unix)]
    {
        let Some(registry) = DETACHED.get() else {
            return Ok(());
        };
        let mut registry = registry.lock().map_err(|_| {
            anyhow!("provider-native background session registry lock was poisoned")
        })?;
        if registry.contains_key(to) {
            bail!("a provider-native frontend already uses the stable session key");
        }
        if let Some(session) = registry.remove(from) {
            registry.insert(to.to_owned(), session);
        }
    }
    Ok(())
}

#[cfg(unix)]
fn terminal_is_interactive() -> bool {
    unsafe { libc::isatty(libc::STDIN_FILENO) == 1 && libc::isatty(libc::STDOUT_FILENO) == 1 }
}

#[cfg(unix)]
fn run_pty(
    mut command: Command,
    session_key: &str,
    initial_input: Option<ScreenTriggeredInput>,
) -> Result<NativeSessionExit> {
    let detached = take_detached(session_key)?;
    let (child, master, screen, fresh) = match detached {
        Some(detached) => (detached.child, detached.master, detached.screen, false),
        None => {
            clear_physical_screen()?;
            let (child, master) = spawn_pty(&mut command)?;
            let size = terminal_size(libc::STDIN_FILENO).unwrap_or(libc::winsize {
                ws_row: 24,
                ws_col: 80,
                ws_xpixel: 0,
                ws_ypixel: 0,
            });
            (
                child,
                master,
                vt100::Parser::new(size.ws_row, size.ws_col, 0),
                true,
            )
        }
    };
    bridge_session(
        child,
        master,
        screen,
        session_key,
        fresh,
        fresh.then_some(initial_input).flatten(),
    )
}

#[cfg(unix)]
struct ScreenTriggeredInput {
    bytes: Vec<u8>,
    ready_marker: String,
}

#[cfg(unix)]
fn take_detached(session_key: &str) -> Result<Option<DetachedSession>> {
    let detached = detached_registry()
        .lock()
        .map_err(|_| anyhow!("provider-native background session registry lock was poisoned"))?
        .remove(session_key);
    match detached {
        Some(mut detached) => {
            if detached.child.try_wait()?.is_none() {
                Ok(Some(detached))
            } else {
                Ok(None)
            }
        }
        None => Ok(None),
    }
}

#[cfg(unix)]
fn terminate_detached(session: &mut DetachedSession) {
    signal_group(session.child.id(), libc::SIGCONT);
    signal_group(session.child.id(), libc::SIGTERM);
    let deadline = Instant::now() + Duration::from_millis(300);
    loop {
        match session.child.try_wait() {
            Ok(Some(_)) => break,
            Ok(None) if Instant::now() < deadline => {
                thread::sleep(Duration::from_millis(10));
            }
            Ok(None) | Err(_) => {
                signal_group(session.child.id(), libc::SIGKILL);
                let _ = session.child.wait();
                break;
            }
        }
    }
}

#[cfg(unix)]
fn detached_registry() -> &'static Mutex<BTreeMap<String, DetachedSession>> {
    DETACHED.get_or_init(|| Mutex::new(BTreeMap::new()))
}

#[cfg(unix)]
fn spawn_pty(command: &mut Command) -> Result<(std::process::Child, std::fs::File)> {
    let program = command.get_program().to_string_lossy().into_owned();
    let working_directory = command
        .get_current_dir()
        .map(Path::to_path_buf)
        .unwrap_or_else(|| PathBuf::from("."));
    if command.get_current_dir().is_some() && !working_directory.is_dir() {
        bail!(
            "provider-native working directory does not exist: {}",
            working_directory.display()
        );
    }
    if Path::new(&program).components().count() > 1 && !Path::new(&program).is_file() {
        bail!("provider-native executable does not exist: {program}");
    }
    let mut master_fd = -1;
    let mut slave_fd = -1;
    let size = terminal_size(libc::STDIN_FILENO).unwrap_or(libc::winsize {
        ws_row: 24,
        ws_col: 80,
        ws_xpixel: 0,
        ws_ypixel: 0,
    });
    let opened = unsafe {
        libc::openpty(
            &mut master_fd,
            &mut slave_fd,
            std::ptr::null_mut(),
            std::ptr::null(),
            &size,
        )
    };
    if opened != 0 {
        return Err(std::io::Error::last_os_error())
            .context("failed to open provider pseudo-terminal");
    }
    set_close_on_exec(master_fd)?;
    set_close_on_exec(slave_fd)?;
    let master = unsafe { std::fs::File::from_raw_fd(master_fd) };
    let slave = unsafe { std::fs::File::from_raw_fd(slave_fd) };
    command
        .stdin(Stdio::from(slave.try_clone()?))
        .stdout(Stdio::from(slave.try_clone()?))
        .stderr(Stdio::from(slave));
    unsafe {
        command.pre_exec(|| {
            if libc::setsid() < 0 {
                return Err(std::io::Error::last_os_error());
            }
            if libc::ioctl(libc::STDIN_FILENO, libc::TIOCSCTTY as _, 0) < 0 {
                return Err(std::io::Error::last_os_error());
            }
            #[cfg(target_os = "linux")]
            if libc::prctl(libc::PR_SET_PDEATHSIG, libc::SIGHUP) < 0 {
                return Err(std::io::Error::last_os_error());
            }
            Ok(())
        });
    }
    let child = command
        .spawn()
        .with_context(|| {
            format!(
                "failed to start provider-native client {program} in {}",
                working_directory.display()
            )
        })?;
    Ok((child, master))
}

#[cfg(unix)]
fn bridge_session(
    mut child: std::process::Child,
    mut master: std::fs::File,
    mut screen: vt100::Parser,
    session_key: &str,
    fresh: bool,
    mut initial_input: Option<ScreenTriggeredInput>,
) -> Result<NativeSessionExit> {
    let _raw = RawModeGuard::enter()?;
    let mut stdout = io::stdout().lock();
    if !fresh {
        stdout.write_all(b"\x1b[2J\x1b[H")?;
        stdout.write_all(&screen.screen().state_formatted())?;
        stdout.flush()?;
        signal_group(child.id(), libc::SIGCONT);
        signal_group(child.id(), libc::SIGWINCH);
    }
    let mut parser = DetachParser::default();
    let mut return_gesture = ReturnGesture::default();
    let mut current_size = terminal_size(libc::STDIN_FILENO).ok();
    if let Some(size) = current_size {
        set_pty_size(master.as_raw_fd(), size)?;
    }
    loop {
        let mut descriptors = [
            libc::pollfd {
                fd: master.as_raw_fd(),
                events: libc::POLLIN,
                revents: 0,
            },
            libc::pollfd {
                fd: libc::STDIN_FILENO,
                events: libc::POLLIN,
                revents: 0,
            },
        ];
        let polled = unsafe { libc::poll(descriptors.as_mut_ptr(), descriptors.len() as _, 25) };
        if polled < 0 {
            let error = std::io::Error::last_os_error();
            if error.kind() != std::io::ErrorKind::Interrupted {
                return Err(error).context("provider-native terminal poll failed");
            }
        }
        if descriptors[0].revents & libc::POLLIN != 0 {
            copy_available(&mut master, &mut stdout, &mut screen)?;
            forward_ready_initial_input(&mut initial_input, &screen, &mut master)?;
        }
        if descriptors[1].revents & libc::POLLIN != 0 {
            let mut input = [0_u8; 256];
            let read =
                unsafe { libc::read(libc::STDIN_FILENO, input.as_mut_ptr().cast(), input.len()) };
            match read.cmp(&0) {
                std::cmp::Ordering::Less => {
                    let error = std::io::Error::last_os_error();
                    if error.kind() != std::io::ErrorKind::Interrupted {
                        return Err(error).context("failed to read provider-native keyboard input");
                    }
                }
                std::cmp::Ordering::Equal => {}
                std::cmp::Ordering::Greater => {
                    let mut detach = false;
                    for action in parser.push(&input[..read as usize]) {
                        match action {
                            InputAction::Forward(bytes) => {
                                return_gesture.clear(&mut stdout, &screen)?;
                                master.write_all(&bytes)?;
                                master.flush()?;
                            }
                            InputAction::Arrow(direction, bytes) => {
                                if return_gesture.should_detach(direction, &screen) {
                                    detach = true;
                                    break;
                                }
                                return_gesture.clear(&mut stdout, &screen)?;
                                let cursor = screen.screen().cursor_position();
                                master.write_all(bytes)?;
                                master.flush()?;
                                // Claude and a few other TUIs use Left at an
                                // empty, left-margin prompt to change their own
                                // view. Preserve the OAV second-press window in
                                // that one boundary case even if they redraw.
                                return_gesture.begin_probe(
                                    direction,
                                    cursor,
                                    direction == ArrowDirection::Left
                                        && cursor.1 <= EMPTY_PROMPT_MAX_COLUMN,
                                );
                            }
                            InputAction::Detach => {
                                detach = true;
                                break;
                            }
                        }
                    }
                    if detach {
                        stop_frontend(&mut child, &mut master, &mut stdout, &mut screen)?;
                        detached_registry()
                            .lock()
                            .map_err(|_| {
                                anyhow!("provider-native background registry lock was poisoned")
                            })?
                            .insert(
                                session_key.to_owned(),
                                DetachedSession {
                                    child,
                                    master,
                                    screen,
                                },
                            );
                        return Ok(NativeSessionExit::Backgrounded);
                    }
                }
            }
        }
        if let Some(bytes) = parser.flush_expired() {
            return_gesture.clear(&mut stdout, &screen)?;
            master.write_all(&bytes)?;
            master.flush()?;
        }
        return_gesture.update(&mut stdout, &screen)?;
        if let Ok(size) = terminal_size(libc::STDIN_FILENO) {
            if current_size
                .map(|current| !same_terminal_size(current, size))
                .unwrap_or(true)
            {
                set_pty_size(master.as_raw_fd(), size)?;
                screen.set_size(size.ws_row, size.ws_col);
                current_size = Some(size);
            }
        }
        if let Some(status) = child.try_wait()? {
            copy_available(&mut master, &mut stdout, &mut screen)?;
            return Ok(NativeSessionExit::Exited(status));
        }
    }
}

#[cfg(unix)]
fn forward_ready_initial_input(
    pending: &mut Option<ScreenTriggeredInput>,
    screen: &vt100::Parser,
    output: &mut impl Write,
) -> Result<bool> {
    let Some(input) = pending.as_ref() else {
        return Ok(false);
    };
    if !screen.screen().contents().contains(&input.ready_marker) {
        return Ok(false);
    }
    output.write_all(&input.bytes)?;
    output.flush()?;
    *pending = None;
    Ok(true)
}

#[cfg(unix)]
fn stop_frontend(
    child: &mut std::process::Child,
    master: &mut std::fs::File,
    stdout: &mut impl Write,
    screen: &mut vt100::Parser,
) -> Result<()> {
    signal_group(child.id(), libc::SIGTSTP);
    let deadline = Instant::now() + STOP_GRACE;
    let mut stopped = false;
    while Instant::now() < deadline {
        copy_available(master, stdout, screen)?;
        let mut status = 0;
        let waited = unsafe {
            libc::waitpid(
                child.id() as libc::pid_t,
                &mut status,
                libc::WNOHANG | libc::WUNTRACED,
            )
        };
        if waited == child.id() as libc::pid_t && libc::WIFSTOPPED(status) {
            stopped = true;
            break;
        }
        thread::sleep(Duration::from_millis(10));
    }
    if !stopped {
        signal_group(child.id(), libc::SIGSTOP);
    }
    copy_available(master, stdout, screen)?;
    stdout.write_all(b"\x1b[?2004l\x1b[?1000l\x1b[?1002l\x1b[?1003l\x1b[?1006l\x1b[<u\x1b[?25h")?;
    stdout.flush()?;
    Ok(())
}

#[cfg(unix)]
fn copy_available(
    master: &mut std::fs::File,
    output: &mut impl Write,
    screen: &mut vt100::Parser,
) -> Result<()> {
    set_nonblocking(master.as_raw_fd(), true)?;
    let mut bytes = [0_u8; 8192];
    loop {
        match master.read(&mut bytes) {
            Ok(0) => break,
            Ok(count) => {
                screen.process(&bytes[..count]);
                output.write_all(&bytes[..count])?;
            }
            Err(error) if error.kind() == std::io::ErrorKind::WouldBlock => break,
            Err(error) if error.raw_os_error() == Some(libc::EIO) => break,
            Err(error) => return Err(error.into()),
        }
    }
    output.flush()?;
    Ok(())
}

#[cfg(unix)]
fn signal_group(pid: u32, signal: libc::c_int) {
    let _ = unsafe { libc::kill(-(pid as libc::pid_t), signal) };
}

#[cfg(unix)]
fn clear_physical_screen() -> Result<()> {
    let mut output = io::stdout().lock();
    output.write_all(b"\x1b[2J\x1b[H")?;
    output.flush()?;
    Ok(())
}

#[cfg(unix)]
fn terminal_size(fd: libc::c_int) -> Result<libc::winsize> {
    let mut size = libc::winsize {
        ws_row: 0,
        ws_col: 0,
        ws_xpixel: 0,
        ws_ypixel: 0,
    };
    if unsafe { libc::ioctl(fd, libc::TIOCGWINSZ as _, &mut size) } < 0 {
        return Err(std::io::Error::last_os_error()).context("failed to read terminal size");
    }
    // Some PTY allocators (notably `script` in a fresh container) report a
    // successful 0x0 size until their parent performs an explicit resize.
    // Passing that through makes provider TUIs unusable and a one-column
    // vt100 parser can underflow while processing a double-width glyph.
    // Treat a missing dimension as unknown, while preserving genuine small
    // non-zero terminals for the dashboard's compact-layout handling.
    Ok(normalize_terminal_size(size))
}

#[cfg(unix)]
fn normalize_terminal_size(mut size: libc::winsize) -> libc::winsize {
    if size.ws_row == 0 {
        size.ws_row = FALLBACK_TERMINAL_ROWS;
    }
    if size.ws_col < 2 {
        size.ws_col = FALLBACK_TERMINAL_COLUMNS;
    }
    size
}

#[cfg(unix)]
fn set_pty_size(fd: libc::c_int, size: libc::winsize) -> Result<()> {
    if unsafe { libc::ioctl(fd, libc::TIOCSWINSZ as _, &size) } < 0 {
        return Err(std::io::Error::last_os_error())
            .context("failed to resize provider pseudo-terminal");
    }
    Ok(())
}

#[cfg(unix)]
fn same_terminal_size(left: libc::winsize, right: libc::winsize) -> bool {
    left.ws_row == right.ws_row
        && left.ws_col == right.ws_col
        && left.ws_xpixel == right.ws_xpixel
        && left.ws_ypixel == right.ws_ypixel
}

#[cfg(unix)]
fn set_close_on_exec(fd: libc::c_int) -> Result<()> {
    let flags = unsafe { libc::fcntl(fd, libc::F_GETFD) };
    if flags < 0 || unsafe { libc::fcntl(fd, libc::F_SETFD, flags | libc::FD_CLOEXEC) } < 0 {
        return Err(std::io::Error::last_os_error())
            .context("failed to secure pseudo-terminal descriptor");
    }
    Ok(())
}

#[cfg(unix)]
fn set_nonblocking(fd: libc::c_int, enabled: bool) -> Result<()> {
    let flags = unsafe { libc::fcntl(fd, libc::F_GETFL) };
    if flags < 0 {
        return Err(std::io::Error::last_os_error())
            .context("failed to inspect pseudo-terminal flags");
    }
    let updated = if enabled {
        flags | libc::O_NONBLOCK
    } else {
        flags & !libc::O_NONBLOCK
    };
    if unsafe { libc::fcntl(fd, libc::F_SETFL, updated) } < 0 {
        return Err(std::io::Error::last_os_error())
            .context("failed to update pseudo-terminal flags");
    }
    Ok(())
}

#[cfg(unix)]
struct RawModeGuard;

#[cfg(unix)]
impl RawModeGuard {
    fn enter() -> Result<Self> {
        crossterm::terminal::enable_raw_mode()?;
        Ok(Self)
    }
}

#[cfg(unix)]
impl Drop for RawModeGuard {
    fn drop(&mut self) {
        let _ = crossterm::terminal::disable_raw_mode();
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum ArrowDirection {
    Left,
    Right,
}

impl ArrowDirection {
    fn symbol(self) -> &'static str {
        match self {
            Self::Left => "←",
            Self::Right => "→",
        }
    }
}

#[derive(Debug, Eq, PartialEq)]
enum InputAction {
    Forward(Vec<u8>),
    Arrow(ArrowDirection, &'static [u8]),
    Detach,
}

const SHIFT_LEFT: &[u8] = b"\x1b[1;2D";
const SHIFT_RIGHT: &[u8] = b"\x1b[1;2C";
const LEFT: &[u8] = b"\x1b[D";
const RIGHT: &[u8] = b"\x1b[C";
const APPLICATION_LEFT: &[u8] = b"\x1bOD";
const APPLICATION_RIGHT: &[u8] = b"\x1bOC";
const RECOGNIZED_ARROWS: [&[u8]; 6] = [
    SHIFT_LEFT,
    SHIFT_RIGHT,
    LEFT,
    RIGHT,
    APPLICATION_LEFT,
    APPLICATION_RIGHT,
];

#[derive(Default)]
struct DetachParser {
    pending: Vec<u8>,
    pending_since: Option<Instant>,
}

impl DetachParser {
    fn push(&mut self, input: &[u8]) -> Vec<InputAction> {
        let mut bytes = std::mem::take(&mut self.pending);
        self.pending_since = None;
        bytes.extend_from_slice(input);
        let mut actions = Vec::new();
        let mut forward = Vec::new();
        let mut index = 0;
        while index < bytes.len() {
            let remaining = &bytes[index..];
            let recognized =
                if remaining.starts_with(SHIFT_LEFT) || remaining.starts_with(SHIFT_RIGHT) {
                    Some((InputAction::Detach, SHIFT_LEFT.len()))
                } else if remaining.starts_with(LEFT) {
                    Some((InputAction::Arrow(ArrowDirection::Left, LEFT), LEFT.len()))
                } else if remaining.starts_with(APPLICATION_LEFT) {
                    Some((
                        InputAction::Arrow(ArrowDirection::Left, APPLICATION_LEFT),
                        APPLICATION_LEFT.len(),
                    ))
                } else if remaining.starts_with(RIGHT) {
                    Some((
                        InputAction::Arrow(ArrowDirection::Right, RIGHT),
                        RIGHT.len(),
                    ))
                } else if remaining.starts_with(APPLICATION_RIGHT) {
                    Some((
                        InputAction::Arrow(ArrowDirection::Right, APPLICATION_RIGHT),
                        APPLICATION_RIGHT.len(),
                    ))
                } else {
                    None
                };
            if let Some((action, consumed)) = recognized {
                if !forward.is_empty() {
                    actions.push(InputAction::Forward(std::mem::take(&mut forward)));
                }
                actions.push(action);
                index += consumed;
                continue;
            }

            if remaining[0] == 0x1b
                && RECOGNIZED_ARROWS
                    .iter()
                    .any(|sequence| sequence.starts_with(remaining))
            {
                self.pending.extend_from_slice(&bytes[index..]);
                self.pending_since = Some(Instant::now());
                break;
            }
            forward.push(bytes[index]);
            index += 1;
        }
        if !forward.is_empty() {
            actions.push(InputAction::Forward(forward));
        }
        actions
    }

    fn flush_expired(&mut self) -> Option<Vec<u8>> {
        if self.pending.is_empty()
            || self
                .pending_since
                .is_some_and(|since| since.elapsed() < ESCAPE_FLUSH_DELAY)
        {
            return None;
        }
        self.pending_since = None;
        Some(std::mem::take(&mut self.pending))
    }
}

#[derive(Debug)]
struct ArrowProbe {
    direction: ArrowDirection,
    cursor: (u16, u16),
    allow_cursor_change: bool,
    started: Instant,
}

#[derive(Debug)]
struct ArmedReturn {
    direction: ArrowDirection,
    cursor_guard: Option<(u16, u16)>,
    expires: Instant,
    last_bucket: Option<u64>,
}

#[derive(Default)]
struct ReturnGesture {
    probe: Option<ArrowProbe>,
    armed: Option<ArmedReturn>,
    hint_visible: bool,
}

impl ReturnGesture {
    fn begin_probe(
        &mut self,
        direction: ArrowDirection,
        cursor: (u16, u16),
        allow_cursor_change: bool,
    ) {
        self.probe = Some(ArrowProbe {
            direction,
            cursor,
            allow_cursor_change,
            started: Instant::now(),
        });
        self.armed = None;
    }

    fn should_detach(&mut self, direction: ArrowDirection, screen: &vt100::Parser) -> bool {
        let Some(armed) = self.armed.as_ref() else {
            return false;
        };
        armed.direction == direction
            && Instant::now() < armed.expires
            && armed
                .cursor_guard
                .map(|cursor| {
                    !screen.screen().hide_cursor() && screen.screen().cursor_position() == cursor
                })
                .unwrap_or(true)
    }

    fn update(&mut self, output: &mut impl Write, screen: &vt100::Parser) -> Result<()> {
        let now = Instant::now();
        if screen.screen().hide_cursor()
            && self
                .probe
                .as_ref()
                .map_or(true, |probe| !probe.allow_cursor_change)
            && self
                .armed
                .as_ref()
                .map_or(true, |armed| armed.cursor_guard.is_some())
        {
            self.clear(output, screen)?;
            return Ok(());
        }
        if let Some(probe) = self.probe.as_ref() {
            if !probe.allow_cursor_change && screen.screen().cursor_position() != probe.cursor {
                self.clear(output, screen)?;
                return Ok(());
            }
            if now.duration_since(probe.started) >= ARROW_SETTLE_DELAY {
                self.armed = Some(ArmedReturn {
                    direction: probe.direction,
                    cursor_guard: (!probe.allow_cursor_change).then_some(probe.cursor),
                    expires: now + ARROW_RETURN_WINDOW,
                    last_bucket: None,
                });
                self.probe = None;
            }
        }

        let Some(armed) = self.armed.as_mut() else {
            return Ok(());
        };
        if now >= armed.expires
            || armed
                .cursor_guard
                .is_some_and(|cursor| screen.screen().cursor_position() != cursor)
        {
            self.clear(output, screen)?;
            return Ok(());
        }
        let remaining = armed.expires.saturating_duration_since(now);
        let bucket = remaining.as_millis() as u64 / RETURN_HINT_REFRESH.as_millis() as u64;
        if armed.last_bucket != Some(bucket) {
            write_return_hint(output, screen, armed.direction, remaining)?;
            armed.last_bucket = Some(bucket);
            self.hint_visible = true;
        }
        Ok(())
    }

    fn clear(&mut self, output: &mut impl Write, screen: &vt100::Parser) -> Result<()> {
        self.probe = None;
        self.armed = None;
        if self.hint_visible {
            restore_bottom_row(output, screen)?;
            self.hint_visible = false;
        }
        Ok(())
    }
}

fn write_return_hint(
    output: &mut impl Write,
    screen: &vt100::Parser,
    direction: ArrowDirection,
    remaining: Duration,
) -> Result<()> {
    let (rows, cols) = screen.screen().size();
    let tenths = remaining.as_millis().div_ceil(100);
    let message = format!(
        " Press {} again to return to Open Agent View · {:.1}s · Shift+←/→ anytime",
        direction.symbol(),
        tenths as f64 / 10.0
    );
    let message = truncate_to_columns(&message, usize::from(cols));
    write!(
        output,
        "\x1b7\x1b[{};1H\x1b[2K\x1b[30;46m{}\x1b[0m\x1b8",
        rows.max(1),
        message
    )?;
    output.flush()?;
    Ok(())
}

fn restore_bottom_row(output: &mut impl Write, screen: &vt100::Parser) -> Result<()> {
    let (rows, cols) = screen.screen().size();
    let last_row = screen
        .screen()
        .rows_formatted(0, cols)
        .nth(usize::from(rows.saturating_sub(1)))
        .unwrap_or_default();
    write!(output, "\x1b7\x1b[{};1H\x1b[2K", rows.max(1))?;
    output.write_all(&last_row)?;
    output.write_all(b"\x1b8")?;
    output.flush()?;
    Ok(())
}

fn truncate_to_columns(value: &str, width: usize) -> String {
    value.chars().take(width).collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[cfg(unix)]
    #[test]
    fn zero_sized_pty_uses_safe_native_terminal_dimensions() {
        let normalized = normalize_terminal_size(libc::winsize {
            ws_row: 0,
            ws_col: 0,
            ws_xpixel: 0,
            ws_ypixel: 0,
        });
        assert_eq!(normalized.ws_row, FALLBACK_TERMINAL_ROWS);
        assert_eq!(normalized.ws_col, FALLBACK_TERMINAL_COLUMNS);

        let tiny = normalize_terminal_size(libc::winsize {
            ws_row: 1,
            ws_col: 1,
            ws_xpixel: 0,
            ws_ypixel: 0,
        });
        assert_eq!(tiny.ws_row, 1);
        assert_eq!(tiny.ws_col, FALLBACK_TERMINAL_COLUMNS);
    }

    #[test]
    fn detach_parser_handles_fragmented_shift_left_sequences() {
        let mut parser = DetachParser::default();
        let first = parser.push(b"hello\x1b");
        assert_eq!(first, vec![InputAction::Forward(b"hello".to_vec())]);
        let second = parser.push(b"[1;2Dignored");
        assert_eq!(
            second,
            vec![
                InputAction::Detach,
                InputAction::Forward(b"ignored".to_vec())
            ]
        );
    }

    #[test]
    fn detach_parser_classifies_plain_arrows_and_preserves_other_input_exactly() {
        let mut parser = DetachParser::default();
        let parsed = parser.push(b"abc\x1b[A\x1b[D\x1bOD\x1b[C\x1bOC\x1b[1;2C");
        assert_eq!(
            parsed,
            vec![
                InputAction::Forward(b"abc\x1b[A".to_vec()),
                InputAction::Arrow(ArrowDirection::Left, LEFT),
                InputAction::Arrow(ArrowDirection::Left, APPLICATION_LEFT),
                InputAction::Arrow(ArrowDirection::Right, RIGHT),
                InputAction::Arrow(ArrowDirection::Right, APPLICATION_RIGHT),
                InputAction::Detach,
            ]
        );
    }

    #[test]
    fn return_hint_is_bounded_to_the_terminal_width() {
        assert_eq!(truncate_to_columns("Press ← again", 7), "Press ←");
    }

    #[cfg(unix)]
    #[test]
    fn initial_input_waits_for_the_exact_authenticated_screen_marker_and_sends_once() {
        let mut screen = vt100::Parser::new(12, 80, 0);
        let mut pending = Some(ScreenTriggeredInput {
            bytes: b"fix the parser\r".to_vec(),
            ready_marker: "Send /help for help information.".into(),
        });
        let mut forwarded = Vec::new();

        screen.process(b"Run /login or /provider to get started.");
        assert!(!forward_ready_initial_input(&mut pending, &screen, &mut forwarded).unwrap());
        assert!(forwarded.is_empty());

        screen.process(b"\r\nSend /help for help information.");
        assert!(forward_ready_initial_input(&mut pending, &screen, &mut forwarded).unwrap());
        assert_eq!(forwarded, b"fix the parser\r");
        assert!(pending.is_none());

        assert!(!forward_ready_initial_input(&mut pending, &screen, &mut forwarded).unwrap());
        assert_eq!(forwarded, b"fix the parser\r");
    }

    #[test]
    fn screen_gated_input_rejects_empty_or_oversized_payloads_before_spawning() {
        let command = || Command::new("provider-that-must-not-run");
        assert!(
            run_with_initial_input_after_screen(command(), "test:empty", b"", "ready")
                .unwrap_err()
                .to_string()
                .contains("initial input")
        );
        assert!(run_with_initial_input_after_screen(
            command(),
            "test:oversized",
            &vec![b'x'; MAX_INITIAL_INPUT_BYTES + 1],
            "ready",
        )
        .unwrap_err()
        .to_string()
        .contains("initial input"));
    }

    #[test]
    fn empty_left_margin_prompt_keeps_return_window_across_provider_redraw() {
        let mut screen = vt100::Parser::new(6, 40, 0);
        screen.process(b"\x1b[1;3H> \x1b[?25h");
        let cursor = screen.screen().cursor_position();
        assert!(cursor.1 <= EMPTY_PROMPT_MAX_COLUMN);

        let mut gesture = ReturnGesture::default();
        gesture.begin_probe(ArrowDirection::Left, cursor, true);
        screen.process(b"\x1b[2J\x1b[6;20Hprovider subview\x1b[?25l");
        std::thread::sleep(ARROW_SETTLE_DELAY + Duration::from_millis(10));
        let mut output = Vec::new();
        gesture.update(&mut output, &screen).unwrap();

        assert!(gesture.should_detach(ArrowDirection::Left, &screen));
        assert!(String::from_utf8_lossy(&output).contains("Press ← again"));
    }
}

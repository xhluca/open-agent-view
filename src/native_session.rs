//! Provider-neutral native-TUI handoff with a dashboard detach key.
//!
//! Interactive provider clients run behind a private pseudo-terminal. A plain
//! Left arrow is reserved by Open Agent View: it suspends only that frontend,
//! returns to the dashboard, and keeps the provider's managed backend alive.
//! Selecting the same row resumes the exact stopped frontend and screen.

use std::collections::BTreeMap;
use std::io::{self, Read, Write};
#[cfg(unix)]
use std::os::fd::{AsRawFd, FromRawFd};
#[cfg(unix)]
use std::os::unix::process::CommandExt;
use std::process::{Command, ExitStatus, Stdio};
use std::sync::{Mutex, OnceLock};
#[cfg(unix)]
use std::thread;
use std::time::{Duration, Instant};

use anyhow::{anyhow, bail, Context, Result};

const ESCAPE_FLUSH_DELAY: Duration = Duration::from_millis(30);
#[cfg(unix)]
const STOP_GRACE: Duration = Duration::from_millis(250);

#[derive(Debug)]
pub enum NativeSessionExit {
    Backgrounded,
    Exited(ExitStatus),
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
        return run_pty(command, session_key);
    }
    let status = command
        .stdin(Stdio::inherit())
        .stdout(Stdio::inherit())
        .stderr(Stdio::inherit())
        .status()
        .context("failed to open provider session")?;
    Ok(NativeSessionExit::Exited(status))
}

/// Resume an exact frontend previously backgrounded with Left arrow. Unlike
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
fn run_pty(mut command: Command, session_key: &str) -> Result<NativeSessionExit> {
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
                vt100::Parser::new(size.ws_row.max(1), size.ws_col.max(1), 0),
                true,
            )
        }
    };
    bridge_session(child, master, screen, session_key, fresh)
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
        .context("failed to start provider-native client")?;
    Ok((child, master))
}

#[cfg(unix)]
fn bridge_session(
    mut child: std::process::Child,
    mut master: std::fs::File,
    mut screen: vt100::Parser,
    session_key: &str,
    fresh: bool,
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
                    let parsed = parser.push(&input[..read as usize]);
                    if !parsed.forward.is_empty() {
                        master.write_all(&parsed.forward)?;
                        master.flush()?;
                    }
                    if parsed.detach {
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
            master.write_all(&bytes)?;
            master.flush()?;
        }
        if let Ok(size) = terminal_size(libc::STDIN_FILENO) {
            if current_size
                .map(|current| !same_terminal_size(current, size))
                .unwrap_or(true)
            {
                set_pty_size(master.as_raw_fd(), size)?;
                screen.set_size(size.ws_row.max(1), size.ws_col.max(1));
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
    Ok(size)
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

#[derive(Default)]
struct DetachParser {
    pending: Vec<u8>,
    pending_since: Option<Instant>,
}

struct ParsedInput {
    forward: Vec<u8>,
    detach: bool,
}

impl DetachParser {
    fn push(&mut self, input: &[u8]) -> ParsedInput {
        let mut bytes = std::mem::take(&mut self.pending);
        self.pending_since = None;
        bytes.extend_from_slice(input);
        let mut forward = Vec::with_capacity(bytes.len());
        let mut index = 0;
        while index < bytes.len() {
            if bytes[index..].starts_with(b"\x1b[D") || bytes[index..].starts_with(b"\x1bOD") {
                return ParsedInput {
                    forward,
                    detach: true,
                };
            }
            if bytes[index] == 0x1b
                && (index + 1 == bytes.len()
                    || (matches!(bytes.get(index + 1), Some(b'[' | b'O'))
                        && index + 2 == bytes.len()))
            {
                self.pending.extend_from_slice(&bytes[index..]);
                self.pending_since = Some(Instant::now());
                break;
            }
            forward.push(bytes[index]);
            index += 1;
        }
        ParsedInput {
            forward,
            detach: false,
        }
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn detach_parser_handles_fragmented_normal_and_application_left_sequences() {
        let mut parser = DetachParser::default();
        let first = parser.push(b"hello\x1b");
        assert_eq!(first.forward, b"hello");
        assert!(!first.detach);
        let second = parser.push(b"[Dignored");
        assert!(second.forward.is_empty());
        assert!(second.detach);

        let mut parser = DetachParser::default();
        let parsed = parser.push(b"\x1bOD");
        assert!(parsed.detach);
        assert!(parsed.forward.is_empty());
    }

    #[test]
    fn detach_parser_forwards_other_arrows_and_plain_text_exactly() {
        let mut parser = DetachParser::default();
        let parsed = parser.push(b"abc\x1b[A\x1b[C");
        assert_eq!(parsed.forward, b"abc\x1b[A\x1b[C");
        assert!(!parsed.detach);
    }
}

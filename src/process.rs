use std::collections::BTreeMap;
use std::io::{BufRead, BufReader, Read};
use std::path::PathBuf;
use std::process::{Child, Command, Stdio};
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::mpsc;
use std::sync::{Arc, Mutex};
use std::thread;
use std::time::{Duration, Instant};

use anyhow::{anyhow, Context, Result};

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct CommandRequest {
    pub program: String,
    pub args: Vec<String>,
    pub current_dir: Option<PathBuf>,
    pub timeout: Duration,
}

impl CommandRequest {
    pub fn new(program: impl Into<String>, args: Vec<String>) -> Self {
        Self {
            program: program.into(),
            args,
            current_dir: None,
            timeout: Duration::from_secs(5),
        }
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct CommandOutput {
    pub status: i32,
    pub stdout: Vec<u8>,
    pub stderr: Vec<u8>,
}

impl CommandOutput {
    pub fn stdout_text(&self) -> Result<&str> {
        std::str::from_utf8(&self.stdout).context("command stdout was not UTF-8")
    }

    pub fn stderr_lossy(&self) -> String {
        String::from_utf8_lossy(&self.stderr).trim().to_owned()
    }
}

pub trait CommandRunner: Send + Sync {
    fn run(&self, request: &CommandRequest) -> Result<CommandOutput>;

    /// Return after the command emits its first complete stdout line.
    ///
    /// Provider CLIs occasionally print the one-shot result and then retain a
    /// monitor child. The default keeps mock runners and ordinary commands on
    /// the normal completion contract; the host runner can safely terminate
    /// the exact child it spawned once that complete result exists.
    fn run_until_stdout_line(&self, request: &CommandRequest) -> Result<CommandOutput> {
        self.run(request)
    }

    fn cancel(&self) {}
}

#[derive(Debug, Default)]
pub struct ProcessRunner;

impl CommandRunner for ProcessRunner {
    fn run(&self, request: &CommandRequest) -> Result<CommandOutput> {
        let mut command = Command::new(&request.program);
        command
            .args(&request.args)
            .stdin(Stdio::null())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped());
        #[cfg(unix)]
        {
            use std::os::unix::process::CommandExt;
            command.process_group(0);
        }
        if let Some(current_dir) = &request.current_dir {
            command.current_dir(current_dir);
        }

        let mut child = command.spawn().with_context(|| {
            format!(
                "failed to start command {} {}",
                request.program,
                request.args.join(" ")
            )
        })?;
        let stdout = child.stdout.take().context("failed to capture stdout")?;
        let stderr = child.stderr.take().context("failed to capture stderr")?;
        let stdout_reader = thread::spawn(move || read_all(stdout));
        let stderr_reader = thread::spawn(move || read_all(stderr));
        let deadline = Instant::now() + request.timeout;

        let status = loop {
            if let Some(status) = child.try_wait().context("failed to poll command")? {
                break status;
            }
            if Instant::now() >= deadline {
                terminate_process_group(&mut child);
                let _ = child.wait();
                let _ = stdout_reader.join();
                let _ = stderr_reader.join();
                return Err(anyhow!(
                    "command timed out after {} ms: {}",
                    request.timeout.as_millis(),
                    request.program
                ));
            }
            thread::sleep(Duration::from_millis(15));
        };

        let stdout = stdout_reader
            .join()
            .map_err(|_| anyhow!("stdout reader thread panicked"))??;
        let stderr = stderr_reader
            .join()
            .map_err(|_| anyhow!("stderr reader thread panicked"))??;

        Ok(CommandOutput {
            status: status.code().unwrap_or(-1),
            stdout,
            stderr,
        })
    }

    fn run_until_stdout_line(&self, request: &CommandRequest) -> Result<CommandOutput> {
        let mut command = Command::new(&request.program);
        command
            .args(&request.args)
            .stdin(Stdio::null())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped());
        #[cfg(unix)]
        {
            use std::os::unix::process::CommandExt;
            command.process_group(0);
        }
        if let Some(current_dir) = &request.current_dir {
            command.current_dir(current_dir);
        }

        let mut child = command.spawn().with_context(|| {
            format!(
                "failed to start command {} {}",
                request.program,
                request.args.join(" ")
            )
        })?;
        let stdout = child.stdout.take().context("failed to capture stdout")?;
        let stderr = child.stderr.take().context("failed to capture stderr")?;
        let (first_line_tx, first_line_rx) = mpsc::sync_channel(1);
        let stdout_reader = thread::spawn(move || -> Result<Vec<u8>> {
            let mut reader = BufReader::new(stdout);
            let mut bytes = Vec::new();
            let first = reader
                .read_until(b'\n', &mut bytes)
                .context("failed to read command result line")?;
            let _ = first_line_tx
                .send((first > 0 && bytes.last() == Some(&b'\n')).then(|| bytes.clone()));
            reader.read_to_end(&mut bytes)?;
            Ok(bytes)
        });
        let stderr_reader = thread::spawn(move || read_all(stderr));
        let deadline = Instant::now() + request.timeout;

        let (status, accepted_line) = loop {
            if let Ok(line) = first_line_rx.try_recv() {
                if line.is_some() {
                    terminate_process_group(&mut child);
                    let status = child
                        .wait()
                        .context("failed to reap command after result")?;
                    break (status, true);
                }
            }
            if let Some(status) = child.try_wait().context("failed to poll command")? {
                break (status, false);
            }
            if Instant::now() >= deadline {
                terminate_process_group(&mut child);
                let _ = child.wait();
                let _ = stdout_reader.join();
                let _ = stderr_reader.join();
                return Err(anyhow!(
                    "command timed out after {} ms: {}",
                    request.timeout.as_millis(),
                    request.program
                ));
            }
            thread::sleep(Duration::from_millis(10));
        };

        let stdout = stdout_reader
            .join()
            .map_err(|_| anyhow!("stdout reader thread panicked"))??;
        let stderr = stderr_reader
            .join()
            .map_err(|_| anyhow!("stderr reader thread panicked"))??;
        Ok(CommandOutput {
            status: if accepted_line {
                0
            } else {
                status.code().unwrap_or(-1)
            },
            stdout,
            stderr,
        })
    }
}

/// Command runner for refresh workers whose in-flight subprocesses must be
/// stopped when the dashboard exits. Every command receives a private Unix
/// process group so cancellation also reaches provider descendants.
#[derive(Debug, Default)]
pub struct CancellableProcessRunner {
    cancelled: AtomicBool,
    next_id: AtomicU64,
    active: Mutex<BTreeMap<u64, Arc<Mutex<Child>>>>,
}

impl CommandRunner for CancellableProcessRunner {
    fn run(&self, request: &CommandRequest) -> Result<CommandOutput> {
        if self.cancelled.load(Ordering::SeqCst) {
            return Err(anyhow!("command cancelled because the dashboard exited"));
        }

        let mut command = Command::new(&request.program);
        command
            .args(&request.args)
            .stdin(Stdio::null())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped());
        #[cfg(unix)]
        {
            use std::os::unix::process::CommandExt;
            command.process_group(0);
        }
        if let Some(current_dir) = &request.current_dir {
            command.current_dir(current_dir);
        }

        let mut child = command.spawn().with_context(|| {
            format!(
                "failed to start command {} {}",
                request.program,
                request.args.join(" ")
            )
        })?;
        let stdout = child.stdout.take().context("failed to capture stdout")?;
        let stderr = child.stderr.take().context("failed to capture stderr")?;
        let stdout_reader = thread::spawn(move || read_all(stdout));
        let stderr_reader = thread::spawn(move || read_all(stderr));
        let child = Arc::new(Mutex::new(child));
        let id = self.next_id.fetch_add(1, Ordering::Relaxed);
        self.active
            .lock()
            .map_err(|_| anyhow!("active command registry lock was poisoned"))?
            .insert(id, child.clone());

        if self.cancelled.load(Ordering::SeqCst) {
            if let Ok(mut child) = child.lock() {
                terminate_process_group(&mut child);
            }
        }

        let deadline = Instant::now() + request.timeout;
        let status = loop {
            let status = child
                .lock()
                .map_err(|_| anyhow!("active command lock was poisoned"))?
                .try_wait()
                .context("failed to poll command")?;
            if let Some(status) = status {
                break Ok(status);
            }
            if Instant::now() >= deadline {
                if let Ok(mut child) = child.lock() {
                    terminate_process_group(&mut child);
                    let _ = child.wait();
                }
                break Err(anyhow!(
                    "command timed out after {} ms: {}",
                    request.timeout.as_millis(),
                    request.program
                ));
            }
            thread::sleep(Duration::from_millis(15));
        };

        if let Ok(mut active) = self.active.lock() {
            active.remove(&id);
        }
        let stdout = stdout_reader
            .join()
            .map_err(|_| anyhow!("stdout reader thread panicked"))??;
        let stderr = stderr_reader
            .join()
            .map_err(|_| anyhow!("stderr reader thread panicked"))??;
        let status = status?;

        Ok(CommandOutput {
            status: status.code().unwrap_or(-1),
            stdout,
            stderr,
        })
    }

    fn cancel(&self) {
        self.cancelled.store(true, Ordering::SeqCst);
        let children = self
            .active
            .lock()
            .map(|active| active.values().cloned().collect::<Vec<_>>())
            .unwrap_or_default();
        for child in children {
            if let Ok(mut child) = child.lock() {
                terminate_process_group(&mut child);
            }
        }
    }
}

fn terminate_process_group(child: &mut Child) {
    #[cfg(unix)]
    unsafe {
        libc::kill(-(child.id() as i32), libc::SIGKILL);
    }
    let _ = child.kill();
}

fn read_all(mut reader: impl Read) -> Result<Vec<u8>> {
    let mut bytes = Vec::new();
    reader.read_to_end(&mut bytes)?;
    Ok(bytes)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn captures_stdout_and_status() {
        let output = ProcessRunner
            .run(&CommandRequest::new(
                "sh",
                vec!["-c".into(), "printf hello".into()],
            ))
            .unwrap();

        assert_eq!(output.status, 0);
        assert_eq!(output.stdout, b"hello");
    }

    #[test]
    fn times_out_slow_commands() {
        let mut request = CommandRequest::new("sh", vec!["-c".into(), "sleep 1".into()]);
        request.timeout = Duration::from_millis(20);

        let error = ProcessRunner.run(&request).unwrap_err();
        assert!(error.to_string().contains("timed out"));
    }

    #[test]
    fn returns_after_a_complete_result_line_even_when_the_helper_stays_alive() {
        let mut request = CommandRequest::new(
            "sh",
            vec!["-c".into(), "printf 'result-id\\n'; sleep 10".into()],
        );
        request.timeout = Duration::from_secs(2);
        let started = Instant::now();

        let output = ProcessRunner.run_until_stdout_line(&request).unwrap();

        assert_eq!(output.status, 0);
        assert_eq!(output.stdout, b"result-id\n");
        assert!(started.elapsed() < Duration::from_secs(1));
    }

    #[test]
    fn partial_stdout_is_not_mistaken_for_a_complete_result_line() {
        let request = CommandRequest::new("sh", vec!["-c".into(), "printf partial; exit 7".into()]);

        let output = ProcessRunner.run_until_stdout_line(&request).unwrap();

        assert_eq!(output.status, 7);
        assert_eq!(output.stdout, b"partial");
    }

    #[test]
    fn cancellable_runner_stops_an_inflight_command_promptly() {
        let runner = Arc::new(CancellableProcessRunner::default());
        let worker = runner.clone();
        let started = Instant::now();
        let command = thread::spawn(move || {
            let mut request = CommandRequest::new("sh", vec!["-c".into(), "sleep 10".into()]);
            request.timeout = Duration::from_secs(15);
            worker.run(&request)
        });

        let deadline = Instant::now() + Duration::from_secs(1);
        while runner.active.lock().unwrap().is_empty() {
            assert!(Instant::now() < deadline, "command was never registered");
            thread::sleep(Duration::from_millis(5));
        }
        runner.cancel();
        let output = command.join().unwrap().unwrap();

        assert_eq!(output.status, -1);
        assert!(started.elapsed() < Duration::from_secs(3));
        assert!(runner.active.lock().unwrap().is_empty());
        assert!(runner
            .run(&CommandRequest::new(
                "sh",
                vec!["-c".into(), "exit 0".into()]
            ))
            .unwrap_err()
            .to_string()
            .contains("dashboard exited"));
    }
}

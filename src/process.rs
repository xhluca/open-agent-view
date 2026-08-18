use std::io::Read;
use std::path::PathBuf;
use std::process::{Command, Stdio};
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
                let _ = child.kill();
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
}

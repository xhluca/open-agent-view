use std::time::Duration;

use serde::Serialize;

use crate::adapters::DockerTarget;
use crate::process::{CommandRequest, CommandRunner, ProcessRunner};

#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum CheckStatus {
    Ok,
    Warning,
    Error,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
pub struct DoctorCheck {
    pub name: String,
    pub status: CheckStatus,
    pub detail: String,
}

#[derive(Clone, Debug, Default, Eq, PartialEq, Serialize)]
pub struct DoctorReport {
    pub checks: Vec<DoctorCheck>,
}

impl DoctorReport {
    pub fn healthy_provider_count(&self) -> usize {
        self.checks
            .iter()
            .filter(|check| {
                matches!(check.name.as_str(), "Claude" | "Codex") && check.status == CheckStatus::Ok
            })
            .count()
    }

    pub fn has_errors(&self) -> bool {
        self.checks
            .iter()
            .any(|check| check.status == CheckStatus::Error)
    }
}

pub fn diagnose(
    claude_bin: &str,
    codex_bin: &str,
    docker_bin: &str,
    docker_targets: &[String],
) -> DoctorReport {
    let mut checks = Vec::new();
    checks.push(version_check("Claude", claude_bin, &["--version"]));
    checks.push(version_check("Codex", codex_bin, &["--version"]));
    checks.push(version_check(
        "Docker",
        docker_bin,
        &[
            "version",
            "--format",
            "{{.Client.Version}} / {{.Server.Version}}",
        ],
    ));
    for target in docker_targets {
        checks.push(match DockerTarget::inspect(target, docker_bin) {
            Ok(container) => DoctorCheck {
                name: format!("Container {}", container.name),
                status: CheckStatus::Ok,
                detail: format!(
                    "running, immutable id {}, image {}",
                    &container.id[..container.id.len().min(12)],
                    container.display_image()
                ),
            },
            Err(error) => DoctorCheck {
                name: format!("Container {target}"),
                status: CheckStatus::Error,
                detail: format!("{error:#}"),
            },
        });
    }
    DoctorReport { checks }
}

pub fn render_text(report: &DoctorReport) -> String {
    let mut output = String::from("Open Agent View doctor\n\n");
    for check in &report.checks {
        let marker = match check.status {
            CheckStatus::Ok => "ok",
            CheckStatus::Warning => "warning",
            CheckStatus::Error => "error",
        };
        output.push_str(&format!(
            "[{marker:7}] {:<24} {}\n",
            check.name, check.detail
        ));
    }
    if report.healthy_provider_count() == 0 {
        output.push_str(
            "\nNo host provider is available; use --docker-container or install a provider CLI.\n",
        );
    }
    output
}

fn version_check(name: &str, program: &str, args: &[&str]) -> DoctorCheck {
    let mut request = CommandRequest::new(
        program,
        args.iter().map(|argument| (*argument).to_owned()).collect(),
    );
    request.timeout = Duration::from_secs(5);
    match ProcessRunner.run(&request) {
        Ok(output) if output.status == 0 => {
            let version = String::from_utf8_lossy(&output.stdout).trim().to_owned();
            DoctorCheck {
                name: name.into(),
                status: CheckStatus::Ok,
                detail: if version.is_empty() {
                    "available".into()
                } else {
                    version
                },
            }
        }
        Ok(output) => DoctorCheck {
            name: name.into(),
            status: CheckStatus::Warning,
            detail: format!(
                "exited with status {}: {}",
                output.status,
                output.stderr_lossy()
            ),
        },
        Err(error) => DoctorCheck {
            name: name.into(),
            status: CheckStatus::Warning,
            detail: format!("unavailable: {error:#}"),
        },
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn text_report_is_compact_and_actionable() {
        let report = DoctorReport {
            checks: vec![DoctorCheck {
                name: "Claude".into(),
                status: CheckStatus::Ok,
                detail: "2.1.234".into(),
            }],
        };

        let rendered = render_text(&report);

        assert!(rendered.contains("[ok"));
        assert!(rendered.contains("Claude"));
        assert!(rendered.contains("2.1.234"));
    }
}

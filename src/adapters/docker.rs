use std::time::Duration;

use anyhow::{bail, Context, Result};
use serde::Deserialize;

use crate::process::{CommandRequest, CommandRunner, ProcessRunner};

/// Immutable metadata for an explicitly enrolled Docker target.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct DockerTarget {
    pub id: String,
    pub name: String,
    pub image: String,
    pub image_id: String,
}

impl DockerTarget {
    pub fn inspect(reference: &str, docker_executable: &str) -> Result<Self> {
        let mut request = CommandRequest::new(
            docker_executable,
            vec!["inspect".into(), reference.into()],
        );
        request.timeout = Duration::from_secs(5);
        let output = ProcessRunner.run(&request)?;
        if output.status != 0 {
            bail!(
                "docker inspect exited with status {}: {}",
                output.status,
                output.stderr_lossy()
            );
        }
        let records: Vec<InspectRecord> = serde_json::from_str(output.stdout_text()?)
            .context("invalid docker inspect response")?;
        let record = records
            .into_iter()
            .next()
            .context("docker inspect returned no target")?;
        if !record.state.running {
            bail!(
                "container {} is {}, and refresh will not start it as a side effect",
                record.name.trim_start_matches('/'),
                record.state.status
            );
        }
        Ok(Self {
            id: record.id,
            name: record.name.trim_start_matches('/').to_owned(),
            image: record.config.image,
            image_id: record.image,
        })
    }

    pub fn display_image(&self) -> String {
        let short_id = self.image_id.strip_prefix("sha256:").unwrap_or(&self.image_id);
        format!("{}@{}", self.image, &short_id[..short_id.len().min(12)])
    }
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "PascalCase")]
struct InspectRecord {
    id: String,
    name: String,
    image: String,
    config: InspectConfig,
    state: InspectState,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "PascalCase")]
struct InspectConfig {
    image: String,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "PascalCase")]
struct InspectState {
    running: bool,
    status: String,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn display_image_uses_reference_and_immutable_id() {
        let target = DockerTarget {
            id: "container".into(),
            name: "agent".into(),
            image: "basic-claude-uv".into(),
            image_id: "sha256:1234567890abcdef".into(),
        };

        assert_eq!(target.display_image(), "basic-claude-uv@1234567890ab");
    }
}

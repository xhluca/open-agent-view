use std::fs;
use std::path::PathBuf;

use anyhow::{Context, Result};

use super::{DiscoveryRequest, SessionSource};
use crate::domain::{AgentSession, SessionSnapshot};

pub struct FixtureSource {
    path: PathBuf,
}

impl FixtureSource {
    pub fn new(path: impl Into<PathBuf>) -> Self {
        Self { path: path.into() }
    }
}

impl SessionSource for FixtureSource {
    fn label(&self) -> &str {
        "fixture"
    }

    fn discover(&self, request: &DiscoveryRequest) -> Result<Vec<AgentSession>> {
        let input = fs::read_to_string(&self.path)
            .with_context(|| format!("failed to read {}", self.path.display()))?;
        let mut sessions = match serde_json::from_str::<SessionSnapshot>(&input) {
            Ok(snapshot) => snapshot.sessions,
            Err(_) => serde_json::from_str::<Vec<AgentSession>>(&input)
                .context("fixture must be a SessionSnapshot or session array")?,
        };
        sessions.retain(|session| {
            (request.include_completed || session.state != crate::domain::SessionState::Completed)
                && (request.include_interactive
                    || session.kind != crate::domain::SessionKind::Interactive)
                && request
                    .cwd
                    .as_ref()
                    .map(|cwd| session.cwd.starts_with(cwd))
                    .unwrap_or(true)
        });
        Ok(sessions)
    }
}

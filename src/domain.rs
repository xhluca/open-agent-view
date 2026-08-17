use std::collections::BTreeSet;
use std::fmt;
use std::path::PathBuf;
use std::time::{Duration, SystemTime};

use serde::{Deserialize, Serialize};

/// The agent implementation that owns a conversation.
#[derive(Clone, Debug, Deserialize, Eq, Ord, PartialEq, PartialOrd, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum Provider {
    Claude,
    Codex,
    Other(String),
}

impl Provider {
    pub fn label(&self) -> &str {
        match self {
            Self::Claude => "Claude",
            Self::Codex => "Codex",
            Self::Other(name) => name,
        }
    }
}

impl fmt::Display for Provider {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(self.label())
    }
}

/// Where the provider process is running.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum Runtime {
    Host,
    Docker {
        container_id: String,
        container_name: String,
        image: String,
    },
}

impl Runtime {
    pub fn label(&self) -> &str {
        match self {
            Self::Host => "host",
            Self::Docker { container_name, .. } => container_name,
        }
    }
}

/// Lifecycle grouping used by the dashboard.
#[derive(
    Clone, Copy, Debug, Deserialize, Eq, Ord, PartialEq, PartialOrd, Serialize,
)]
#[serde(rename_all = "snake_case")]
pub enum SessionState {
    ReadyForReview,
    NeedsInput,
    Working,
    Completed,
    Unknown,
}

impl SessionState {
    pub const DISPLAY_ORDER: [Self; 5] = [
        Self::ReadyForReview,
        Self::NeedsInput,
        Self::Working,
        Self::Completed,
        Self::Unknown,
    ];

    pub fn heading(self) -> &'static str {
        match self {
            Self::ReadyForReview => "Ready for review",
            Self::NeedsInput => "Needs input",
            Self::Working => "Working",
            Self::Completed => "Completed",
            Self::Unknown => "Unknown",
        }
    }
}

/// An operation the selected adapter can safely perform for a session.
#[derive(
    Clone, Copy, Debug, Deserialize, Eq, Ord, PartialEq, PartialOrd, Serialize,
)]
#[serde(rename_all = "snake_case")]
pub enum Capability {
    Inspect,
    Reply,
    Resume,
    Interrupt,
    Archive,
    Delete,
}

/// A provider record normalized for display and control.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct AgentSession {
    pub id: String,
    pub provider_session_id: String,
    pub provider: Provider,
    pub runtime: Runtime,
    pub name: String,
    pub cwd: PathBuf,
    pub state: SessionState,
    pub summary: String,
    pub raw_state: Option<String>,
    pub pid: Option<u32>,
    pub started_at: Option<SystemTime>,
    pub updated_at: Option<SystemTime>,
    pub pull_requests: Option<u32>,
    #[serde(default)]
    pub capabilities: BTreeSet<Capability>,
}

impl AgentSession {
    pub fn age(&self, now: SystemTime) -> Option<Duration> {
        now.duration_since(self.updated_at.or(self.started_at)?).ok()
    }
}

#[derive(Clone, Debug, Default, Deserialize, Eq, PartialEq, Serialize)]
pub struct SessionSnapshot {
    pub sessions: Vec<AgentSession>,
    #[serde(default)]
    pub warnings: Vec<String>,
}

impl SessionSnapshot {
    pub fn count(&self, state: SessionState) -> usize {
        self.sessions
            .iter()
            .filter(|session| session.state == state)
            .count()
    }

    pub fn sort_for_display(&mut self) {
        self.sessions.sort_by(|left, right| {
            left.state
                .cmp(&right.state)
                .then_with(|| right.updated_at.cmp(&left.updated_at))
                .then_with(|| left.name.cmp(&right.name))
        });
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn state_display_order_matches_dashboard_priority() {
        assert_eq!(SessionState::DISPLAY_ORDER[0], SessionState::ReadyForReview);
        assert_eq!(SessionState::DISPLAY_ORDER[4], SessionState::Unknown);
    }

    #[test]
    fn snapshot_counts_states() {
        let session = AgentSession {
            id: "claude:host:abc".into(),
            provider_session_id: "abc".into(),
            provider: Provider::Claude,
            runtime: Runtime::Host,
            name: "example".into(),
            cwd: PathBuf::from("/work"),
            state: SessionState::Working,
            summary: String::new(),
            raw_state: Some("working".into()),
            pid: None,
            started_at: None,
            updated_at: None,
            pull_requests: None,
            capabilities: BTreeSet::new(),
        };
        let snapshot = SessionSnapshot {
            sessions: vec![session],
            warnings: vec![],
        };

        assert_eq!(snapshot.count(SessionState::Working), 1);
        assert_eq!(snapshot.count(SessionState::Completed), 0);
    }
}


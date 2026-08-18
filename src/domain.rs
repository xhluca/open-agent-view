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
    Pi,
    #[serde(rename = "opencode")]
    OpenCode,
    Cursor,
    #[serde(rename = "github_copilot")]
    GitHubCopilot,
    Antigravity,
    Other(String),
}

impl Provider {
    pub fn label(&self) -> &str {
        match self {
            Self::Claude => "Claude",
            Self::Codex => "Codex",
            Self::Pi => "Pi",
            Self::OpenCode => "OpenCode",
            Self::Cursor => "Cursor",
            Self::GitHubCopilot => "GitHub Copilot",
            Self::Antigravity => "Antigravity",
            Self::Other(name) => name,
        }
    }

    pub fn compact_marker(&self) -> &str {
        match self {
            Self::Claude => "C",
            Self::Codex => "X",
            Self::Pi => "P",
            Self::OpenCode => "O",
            Self::Cursor => "R",
            Self::GitHubCopilot => "G",
            Self::Antigravity => "A",
            Self::Other(_) => "?",
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

/// How the provider reports the session's execution style.
#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum SessionKind {
    Interactive,
    Background,
    Managed,
    Unknown,
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
#[derive(Clone, Copy, Debug, Deserialize, Eq, Ord, PartialEq, PartialOrd, Serialize)]
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
#[derive(Clone, Copy, Debug, Deserialize, Eq, Ord, PartialEq, PartialOrd, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum Capability {
    Inspect,
    Reply,
    Approve,
    Decline,
    Respond,
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
    pub kind: SessionKind,
    pub name: String,
    pub cwd: PathBuf,
    pub state: SessionState,
    pub summary: String,
    pub raw_state: Option<String>,
    pub pid: Option<u32>,
    #[serde(default, with = "optional_system_time_millis")]
    pub started_at: Option<SystemTime>,
    #[serde(default, with = "optional_system_time_millis")]
    pub updated_at: Option<SystemTime>,
    pub pull_requests: Option<u32>,
    #[serde(default)]
    pub capabilities: BTreeSet<Capability>,
}

mod optional_system_time_millis {
    use std::time::{Duration, SystemTime};

    use serde::{Deserialize, Deserializer, Serializer};

    pub fn serialize<S>(value: &Option<SystemTime>, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: Serializer,
    {
        let milliseconds = value
            .and_then(|time| time.duration_since(SystemTime::UNIX_EPOCH).ok())
            .map(|duration| duration.as_millis() as u64);
        serializer.serialize_some(&milliseconds)
    }

    pub fn deserialize<'de, D>(deserializer: D) -> Result<Option<SystemTime>, D::Error>
    where
        D: Deserializer<'de>,
    {
        let milliseconds = Option::<u64>::deserialize(deserializer)?;
        Ok(milliseconds.map(|value| SystemTime::UNIX_EPOCH + Duration::from_millis(value)))
    }
}

impl AgentSession {
    pub fn age(&self, now: SystemTime) -> Option<Duration> {
        now.duration_since(self.updated_at.or(self.started_at)?)
            .ok()
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
            kind: SessionKind::Background,
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

    #[test]
    fn timestamps_serialize_as_unix_milliseconds() {
        let time = SystemTime::UNIX_EPOCH + Duration::from_millis(1_234);

        let json = serde_json::to_string(&AgentSession {
            id: "id".into(),
            provider_session_id: "id".into(),
            provider: Provider::Claude,
            runtime: Runtime::Host,
            kind: SessionKind::Background,
            name: "name".into(),
            cwd: PathBuf::from("/work"),
            state: SessionState::Working,
            summary: String::new(),
            raw_state: None,
            pid: None,
            started_at: Some(time),
            updated_at: None,
            pull_requests: None,
            capabilities: BTreeSet::new(),
        })
        .unwrap();

        assert!(json.contains("\"started_at\":1234"));
        assert!(json.contains("\"updated_at\":null"));
    }

    #[test]
    fn supported_provider_names_are_stable_on_the_wire() {
        let cases = [
            (Provider::Claude, "\"claude\"", "Claude", "C"),
            (Provider::Codex, "\"codex\"", "Codex", "X"),
            (Provider::Pi, "\"pi\"", "Pi", "P"),
            (Provider::OpenCode, "\"opencode\"", "OpenCode", "O"),
            (Provider::Cursor, "\"cursor\"", "Cursor", "R"),
            (
                Provider::GitHubCopilot,
                "\"github_copilot\"",
                "GitHub Copilot",
                "G",
            ),
            (Provider::Antigravity, "\"antigravity\"", "Antigravity", "A"),
        ];

        for (provider, wire, label, marker) in cases {
            assert_eq!(serde_json::to_string(&provider).unwrap(), wire);
            assert_eq!(provider.label(), label);
            assert_eq!(provider.compact_marker(), marker);
        }
    }
}

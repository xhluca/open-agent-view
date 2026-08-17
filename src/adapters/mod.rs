mod claude;
mod codex;
mod docker;
mod fixture;

use std::path::PathBuf;

use anyhow::Result;

pub use claude::{parse_claude_sessions, ClaudeSource};
pub use codex::{parse_codex_thread_list, CodexSource};
pub use docker::DockerTarget;
pub use fixture::FixtureSource;

use crate::domain::{AgentSession, SessionSnapshot};

#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct DiscoveryRequest {
    pub include_completed: bool,
    pub include_interactive: bool,
    pub cwd: Option<PathBuf>,
}

pub trait SessionSource {
    fn label(&self) -> &str;
    fn discover(&self, request: &DiscoveryRequest) -> Result<Vec<AgentSession>>;
}

#[derive(Default)]
pub struct DiscoveryEngine {
    sources: Vec<Box<dyn SessionSource>>,
}

impl DiscoveryEngine {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn add_source(&mut self, source: impl SessionSource + 'static) {
        self.sources.push(Box::new(source));
    }

    pub fn discover(&self, request: &DiscoveryRequest) -> SessionSnapshot {
        let mut snapshot = SessionSnapshot::default();
        for source in &self.sources {
            match source.discover(request) {
                Ok(mut sessions) => snapshot.sessions.append(&mut sessions),
                Err(error) => snapshot
                    .warnings
                    .push(format!("{}: {error:#}", source.label())),
            }
        }
        snapshot.sort_for_display();
        snapshot
    }
}

#[cfg(test)]
mod tests {
    use anyhow::anyhow;

    use super::*;

    struct BrokenSource;

    impl SessionSource for BrokenSource {
        fn label(&self) -> &str {
            "broken"
        }

        fn discover(&self, _: &DiscoveryRequest) -> Result<Vec<AgentSession>> {
            Err(anyhow!("unavailable"))
        }
    }

    #[test]
    fn one_source_failure_becomes_a_warning() {
        let mut engine = DiscoveryEngine::new();
        engine.add_source(BrokenSource);

        let snapshot = engine.discover(&DiscoveryRequest::default());

        assert!(snapshot.sessions.is_empty());
        assert_eq!(snapshot.warnings, vec!["broken: unavailable"]);
    }
}

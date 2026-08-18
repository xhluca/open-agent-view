mod claude;
mod codex;
mod docker;
mod fixture;
mod managed_docker;
mod managed_docker_registry;

use std::path::PathBuf;

use anyhow::Result;

pub use claude::{parse_claude_sessions, ClaudeSource};
pub use codex::{parse_codex_thread_list, CodexSource};
pub use docker::DockerTarget;
pub use fixture::FixtureSource;
pub use managed_docker::{
    DockerAuthority, DockerContainer, DockerProvider, DockerProviderCommand,
    EnrolledDockerContainer, ManagedDockerContainer, ManagedDockerCreateSpec, ManagedDockerOwner,
    ManagedDockerRuntime, ENABLED_LABEL, INSTANCE_LABEL, MANAGED_LABEL, PROVIDERS_LABEL,
    VERSION_LABEL,
};
pub use managed_docker_registry::{
    default_managed_docker_registry_path, generate_managed_instance_id, ManagedDockerRegistry,
    ManagedDockerService, ManagedDockerState, ManagedDockerStatus,
};

use crate::domain::{AgentSession, SessionSnapshot};

#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct DiscoveryRequest {
    pub include_completed: bool,
    pub include_interactive: bool,
    pub cwd: Option<PathBuf>,
}

pub trait SessionSource: Send + Sync {
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

mod antigravity;
mod claude;
mod codex;
mod copilot;
mod copilot_managed;
mod cursor;
mod cursor_managed;
mod docker;
mod fixture;
mod managed_docker;
mod managed_docker_registry;
mod opencode;
mod pi;

use std::path::PathBuf;
use std::sync::Arc;

use anyhow::Result;

pub use antigravity::{
    default_antigravity_last_conversations_path, parse_antigravity_last_conversations,
    AntigravityCommandSpec, AntigravityController, AntigravityInvocation, AntigravitySource,
};
pub use claude::{parse_claude_sessions, ClaudeSource};
pub use codex::{parse_codex_thread_list, CodexSource};
pub use copilot::{
    normalize_copilot_sessions, parse_copilot_session_page, CopilotAcpCapabilities,
    CopilotAcpConnection, CopilotAcpMessage, CopilotAcpMode, CopilotCommandSpec, CopilotController,
    CopilotInvocation, CopilotPermissionOption, CopilotPermissionRequest, CopilotSessionInfo,
    CopilotSessionPage, CopilotSource,
};
pub use copilot_managed::CopilotSupervisor;
pub use cursor::{
    parse_cursor_chat_id, parse_cursor_stream_event, CursorCommandSpec, CursorController,
    CursorInvocation, CursorStreamEvent,
};
pub use cursor_managed::{default_cursor_state_dir, CursorSource, CursorSupervisor};
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
pub use opencode::{
    parse_opencode_session_list, OpenCodeController, OpenCodeInvocation, OpenCodeSource,
};
pub use pi::{default_pi_session_dir, parse_pi_session, PiController, PiSource};

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

impl<T> SessionSource for Arc<T>
where
    T: SessionSource + ?Sized,
{
    fn label(&self) -> &str {
        (**self).label()
    }

    fn discover(&self, request: &DiscoveryRequest) -> Result<Vec<AgentSession>> {
        (**self).discover(request)
    }
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
        let results = std::thread::scope(|scope| {
            self.sources
                .iter()
                .map(|source| {
                    let label = source.label().to_owned();
                    (label, scope.spawn(|| source.discover(request)))
                })
                .collect::<Vec<_>>()
                .into_iter()
                .map(|(label, worker)| (label, worker.join()))
                .collect::<Vec<_>>()
        });
        for (label, result) in results {
            match result {
                Ok(Ok(mut sessions)) => snapshot.sessions.append(&mut sessions),
                Ok(Err(error)) => snapshot.warnings.push(format!("{label}: {error:#}")),
                Err(_) => snapshot
                    .warnings
                    .push(format!("{label}: provider discovery panicked")),
            }
        }
        snapshot.sort_for_display();
        snapshot
    }
}

#[cfg(test)]
mod tests {
    use std::sync::{Arc, Condvar, Mutex};
    use std::time::Duration;

    use anyhow::anyhow;

    use super::*;

    struct BrokenSource;

    struct PanickingSource;

    struct CoordinatedSource {
        label: &'static str,
        arrivals: Arc<(Mutex<usize>, Condvar)>,
    }

    impl SessionSource for BrokenSource {
        fn label(&self) -> &str {
            "broken"
        }

        fn discover(&self, _: &DiscoveryRequest) -> Result<Vec<AgentSession>> {
            Err(anyhow!("unavailable"))
        }
    }

    impl SessionSource for PanickingSource {
        fn label(&self) -> &str {
            "panicking"
        }

        fn discover(&self, _: &DiscoveryRequest) -> Result<Vec<AgentSession>> {
            panic!("provider bug")
        }
    }

    impl SessionSource for CoordinatedSource {
        fn label(&self) -> &str {
            self.label
        }

        fn discover(&self, _: &DiscoveryRequest) -> Result<Vec<AgentSession>> {
            let (arrivals, ready) = &*self.arrivals;
            let mut count = arrivals.lock().expect("lock arrival count");
            *count += 1;
            ready.notify_all();
            let (count, _) = ready
                .wait_timeout_while(count, Duration::from_secs(2), |count| *count < 2)
                .expect("wait for concurrent source");
            if *count < 2 {
                return Err(anyhow!("other provider was not polled concurrently"));
            }
            Ok(Vec::new())
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

    #[test]
    fn one_source_panic_does_not_hide_other_providers() {
        let mut engine = DiscoveryEngine::new();
        engine.add_source(PanickingSource);
        engine.add_source(CoordinatedSource {
            label: "healthy",
            arrivals: Arc::new((Mutex::new(2), Condvar::new())),
        });

        let snapshot = engine.discover(&DiscoveryRequest::default());

        assert!(snapshot.sessions.is_empty());
        assert_eq!(
            snapshot.warnings,
            vec!["panicking: provider discovery panicked"]
        );
    }

    #[test]
    fn enabled_sources_are_polled_concurrently() {
        let arrivals = Arc::new((Mutex::new(0), Condvar::new()));
        let mut engine = DiscoveryEngine::new();
        engine.add_source(CoordinatedSource {
            label: "one",
            arrivals: arrivals.clone(),
        });
        engine.add_source(CoordinatedSource {
            label: "two",
            arrivals,
        });

        let snapshot = engine.discover(&DiscoveryRequest::default());

        assert!(snapshot.sessions.is_empty());
        assert!(snapshot.warnings.is_empty());
    }
}

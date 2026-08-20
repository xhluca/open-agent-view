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

use std::panic::{catch_unwind, AssertUnwindSafe};
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

use crate::domain::{AgentSession, SessionKind, SessionSnapshot, SessionState};

pub const DEFAULT_HISTORY_LIMIT: usize = 100;

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct DiscoveryRequest {
    pub include_completed: bool,
    pub include_interactive: bool,
    pub cwd: Option<PathBuf>,
    /// Maximum persisted-history records a provider may read in one refresh.
    /// Live/owned session inventories are not constrained by this limit.
    pub history_limit: usize,
    /// Maintenance can ask providers to start with their oldest records.
    pub history_oldest_first: bool,
}

impl Default for DiscoveryRequest {
    fn default() -> Self {
        Self {
            include_completed: false,
            include_interactive: false,
            cwd: None,
            history_limit: DEFAULT_HISTORY_LIMIT,
            history_oldest_first: false,
        }
    }
}

#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct SourceDiscovery {
    pub sessions: Vec<AgentSession>,
    pub warnings: Vec<String>,
}

pub trait SessionSource: Send + Sync {
    fn label(&self) -> &str;
    fn discover(&self, request: &DiscoveryRequest) -> Result<Vec<AgentSession>>;
    fn discover_with_warnings(&self, request: &DiscoveryRequest) -> Result<SourceDiscovery> {
        Ok(SourceDiscovery {
            sessions: self.discover(request)?,
            warnings: Vec::new(),
        })
    }
    fn cancel(&self) {}
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

    fn discover_with_warnings(&self, request: &DiscoveryRequest) -> Result<SourceDiscovery> {
        (**self).discover_with_warnings(request)
    }

    fn cancel(&self) {
        (**self).cancel();
    }
}

#[derive(Clone, Default)]
pub struct DiscoveryEngine {
    sources: Vec<Arc<dyn SessionSource>>,
}

impl DiscoveryEngine {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn add_source(&mut self, source: impl SessionSource + 'static) {
        self.sources.push(Arc::new(source));
    }

    pub fn discover(&self, request: &DiscoveryRequest) -> SessionSnapshot {
        self.discover_progressively(request, |_, _, _| {})
    }

    /// Discover every source concurrently and expose completed partial results.
    ///
    /// The callback is not invoked for the final result. Dashboard startup uses
    /// this to show fast providers while a slower CLI is still listing a large
    /// history; ordinary callers continue to receive one complete snapshot.
    pub fn discover_progressively(
        &self,
        request: &DiscoveryRequest,
        mut on_partial: impl FnMut(&SessionSnapshot, usize, usize),
    ) -> SessionSnapshot {
        let mut snapshot = SessionSnapshot::default();
        let total = self.sources.len();
        std::thread::scope(|scope| {
            let (sender, receiver) = std::sync::mpsc::channel();
            for source in &self.sources {
                let sender = sender.clone();
                let label = source.label().to_owned();
                scope.spawn(move || {
                    let result =
                        catch_unwind(AssertUnwindSafe(|| source.discover_with_warnings(request)));
                    let _ = sender.send((label, result));
                });
            }
            drop(sender);

            for completed in 1..=total {
                let Ok((label, result)) = receiver.recv() else {
                    snapshot
                        .warnings
                        .push("provider discovery workers stopped unexpectedly".into());
                    break;
                };
                match result {
                    Ok(Ok(mut discovered)) => {
                        // Provider CLIs are not consistent about honoring their
                        // own active-only and cwd filters. Enforce the public
                        // discovery contract before a large history can enter
                        // the dashboard model or any progressive update.
                        discovered
                            .sessions
                            .retain(|session| session_matches_request(session, request));
                        if bound_completed_history(&mut discovered.sessions, request) {
                            discovered.warnings.push(format!(
                                "{label} completed history is limited to {} records for this refresh; increase --history-limit to load more",
                                request.history_limit.max(1)
                            ));
                        }
                        snapshot.sessions.append(&mut discovered.sessions);
                        snapshot.warnings.append(&mut discovered.warnings);
                    }
                    Ok(Err(error)) => snapshot.warnings.push(format!("{label}: {error:#}")),
                    Err(_) => snapshot
                        .warnings
                        .push(format!("{label}: provider discovery panicked")),
                }
                snapshot.sort_for_display();
                snapshot.warnings.sort();
                if completed < total {
                    on_partial(&snapshot, completed, total);
                }
            }
        });
        snapshot.sort_for_display();
        snapshot.warnings.sort();
        snapshot.warnings.dedup();
        snapshot
    }

    pub fn cancel(&self) {
        for source in &self.sources {
            source.cancel();
        }
    }
}

fn bound_completed_history(sessions: &mut Vec<AgentSession>, request: &DiscoveryRequest) -> bool {
    if !request.include_completed {
        return false;
    }
    let limit = request.history_limit.max(1);
    let mut completed = sessions
        .iter()
        .enumerate()
        .filter(|(_, session)| session.state == SessionState::Completed)
        .map(|(index, _)| index)
        .collect::<Vec<_>>();
    if completed.len() <= limit {
        return false;
    }
    completed.sort_by(|left, right| {
        let left = &sessions[*left];
        let right = &sessions[*right];
        let order = left
            .updated_at
            .cmp(&right.updated_at)
            .then_with(|| left.id.cmp(&right.id));
        if request.history_oldest_first {
            order
        } else {
            order.reverse()
        }
    });
    completed.truncate(limit);
    let retained = completed
        .into_iter()
        .map(|index| sessions[index].id.clone())
        .collect::<std::collections::BTreeSet<_>>();
    sessions.retain(|session| {
        session.state != SessionState::Completed || retained.contains(&session.id)
    });
    true
}

fn session_matches_request(session: &AgentSession, request: &DiscoveryRequest) -> bool {
    (request.include_completed || session.state != SessionState::Completed)
        && (request.include_interactive || session.kind != SessionKind::Interactive)
        && request
            .cwd
            .as_ref()
            .map(|cwd| session.cwd.starts_with(cwd))
            .unwrap_or(true)
}

#[cfg(test)]
mod tests {
    use std::collections::BTreeSet;
    use std::path::PathBuf;
    use std::sync::{Arc, Condvar, Mutex};
    use std::time::{Duration, Instant};

    use anyhow::anyhow;

    use super::*;
    use crate::domain::{Provider, Runtime, SessionKind, SessionState};

    struct BrokenSource;

    struct PanickingSource;

    struct CoordinatedSource {
        label: &'static str,
        arrivals: Arc<(Mutex<usize>, Condvar)>,
    }

    struct DelayedSource {
        label: &'static str,
        delay: Duration,
    }

    struct UnfilteredSource;

    struct CompletedHistorySource;

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

    impl SessionSource for DelayedSource {
        fn label(&self) -> &str {
            self.label
        }

        fn discover(&self, _: &DiscoveryRequest) -> Result<Vec<AgentSession>> {
            std::thread::sleep(self.delay);
            Ok(vec![AgentSession {
                id: format!("test:host:{}", self.label),
                provider_session_id: self.label.into(),
                provider: Provider::Other(self.label.into()),
                runtime: Runtime::Host,
                kind: SessionKind::Background,
                name: self.label.into(),
                cwd: PathBuf::from("/workspace"),
                state: SessionState::Working,
                summary: String::new(),
                raw_state: None,
                pid: None,
                started_at: None,
                updated_at: None,
                pull_requests: None,
                capabilities: BTreeSet::new(),
            }])
        }
    }

    impl SessionSource for UnfilteredSource {
        fn label(&self) -> &str {
            "unfiltered"
        }

        fn discover(&self, _: &DiscoveryRequest) -> Result<Vec<AgentSession>> {
            let session = |id: &str, state, kind, cwd: &str| AgentSession {
                id: format!("test:host:{id}"),
                provider_session_id: id.into(),
                provider: Provider::Other("test".into()),
                runtime: Runtime::Host,
                kind,
                name: id.into(),
                cwd: PathBuf::from(cwd),
                state,
                summary: String::new(),
                raw_state: None,
                pid: None,
                started_at: None,
                updated_at: None,
                pull_requests: None,
                capabilities: BTreeSet::new(),
            };
            Ok(vec![
                session(
                    "active",
                    SessionState::Working,
                    SessionKind::Background,
                    "/workspace/project",
                ),
                session(
                    "completed",
                    SessionState::Completed,
                    SessionKind::Background,
                    "/workspace/project",
                ),
                session(
                    "interactive",
                    SessionState::Working,
                    SessionKind::Interactive,
                    "/workspace/project",
                ),
                session(
                    "elsewhere",
                    SessionState::Working,
                    SessionKind::Background,
                    "/other/project",
                ),
            ])
        }
    }

    impl SessionSource for CompletedHistorySource {
        fn label(&self) -> &str {
            "large-history"
        }

        fn discover(&self, _: &DiscoveryRequest) -> Result<Vec<AgentSession>> {
            Ok((0..5)
                .map(|index| AgentSession {
                    id: format!("history:host:{index}"),
                    provider_session_id: index.to_string(),
                    provider: Provider::Other("history".into()),
                    runtime: Runtime::Host,
                    kind: SessionKind::Background,
                    name: index.to_string(),
                    cwd: PathBuf::from("/workspace"),
                    state: SessionState::Completed,
                    summary: String::new(),
                    raw_state: None,
                    pid: None,
                    started_at: None,
                    updated_at: Some(
                        std::time::SystemTime::UNIX_EPOCH + Duration::from_secs(index),
                    ),
                    pull_requests: None,
                    capabilities: BTreeSet::new(),
                })
                .collect())
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

    #[test]
    fn progressive_discovery_reports_fast_provider_before_slow_provider_finishes() {
        let mut engine = DiscoveryEngine::new();
        engine.add_source(DelayedSource {
            label: "slow",
            delay: Duration::from_millis(200),
        });
        engine.add_source(DelayedSource {
            label: "fast",
            delay: Duration::ZERO,
        });
        let started = Instant::now();
        let mut partials = Vec::new();

        let snapshot = engine.discover_progressively(
            &DiscoveryRequest::default(),
            |partial, completed, total| {
                partials.push((
                    started.elapsed(),
                    completed,
                    total,
                    partial
                        .sessions
                        .iter()
                        .map(|session| session.name.clone())
                        .collect::<Vec<_>>(),
                ));
            },
        );

        assert_eq!(partials.len(), 1);
        assert!(partials[0].0 < Duration::from_millis(100));
        assert_eq!(partials[0].1, 1);
        assert_eq!(partials[0].2, 2);
        assert_eq!(partials[0].3, vec!["fast"]);
        assert_eq!(
            snapshot
                .sessions
                .iter()
                .map(|session| session.name.as_str())
                .collect::<Vec<_>>(),
            vec!["fast", "slow"]
        );
    }

    #[test]
    fn engine_enforces_filters_even_when_a_provider_ignores_them() {
        let mut engine = DiscoveryEngine::new();
        engine.add_source(UnfilteredSource);

        let snapshot = engine.discover(&DiscoveryRequest {
            include_completed: false,
            include_interactive: false,
            cwd: Some(PathBuf::from("/workspace")),
            ..DiscoveryRequest::default()
        });

        assert_eq!(
            snapshot
                .sessions
                .iter()
                .map(|session| session.provider_session_id.as_str())
                .collect::<Vec<_>>(),
            vec!["active"]
        );
    }

    #[test]
    fn engine_allows_explicit_completed_and_interactive_history() {
        let mut engine = DiscoveryEngine::new();
        engine.add_source(UnfilteredSource);

        let snapshot = engine.discover(&DiscoveryRequest {
            include_completed: true,
            include_interactive: true,
            cwd: None,
            ..DiscoveryRequest::default()
        });

        assert_eq!(snapshot.sessions.len(), 4);
    }

    #[test]
    fn engine_bounds_a_provider_that_ignores_the_history_budget() {
        let mut engine = DiscoveryEngine::new();
        engine.add_source(CompletedHistorySource);

        let snapshot = engine.discover(&DiscoveryRequest {
            include_completed: true,
            history_limit: 2,
            ..DiscoveryRequest::default()
        });

        assert_eq!(
            snapshot
                .sessions
                .iter()
                .map(|session| session.provider_session_id.as_str())
                .collect::<Vec<_>>(),
            vec!["4", "3"]
        );
        assert_eq!(snapshot.warnings.len(), 1);
        assert!(snapshot.warnings[0].contains("limited to 2 records"));
    }
}

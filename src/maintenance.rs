use std::path::{Path, PathBuf};
use std::time::SystemTime;

use serde::Serialize;

use crate::domain::{AgentSession, Capability, Provider, SessionSnapshot, SessionState};

#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
pub struct ArchiveItem {
    pub id: String,
    pub provider_session_id: String,
    pub provider: Provider,
    pub name: String,
    pub cwd: PathBuf,
    pub updated_at_ms: Option<u64>,
}

impl From<&AgentSession> for ArchiveItem {
    fn from(session: &AgentSession) -> Self {
        Self {
            id: session.id.clone(),
            provider_session_id: session.provider_session_id.clone(),
            provider: session.provider.clone(),
            name: session.name.clone(),
            cwd: session.cwd.clone(),
            updated_at_ms: session
                .updated_at
                .and_then(|updated| updated.duration_since(SystemTime::UNIX_EPOCH).ok())
                .map(|duration| duration.as_millis() as u64),
        }
    }
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
pub struct ArchiveFailure {
    pub session: ArchiveItem,
    pub error: String,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
pub struct BulkArchiveReport {
    pub dry_run: bool,
    pub completed_seen: usize,
    pub matched_scope: usize,
    pub eligible: usize,
    pub selected: Vec<ArchiveItem>,
    pub skipped_without_authority: usize,
    pub archived: Vec<ArchiveItem>,
    pub failures: Vec<ArchiveFailure>,
}

#[derive(Clone, Debug)]
pub struct BulkArchivePlan {
    report: BulkArchiveReport,
    sessions: Vec<AgentSession>,
}

impl BulkArchivePlan {
    pub fn report(&self) -> &BulkArchiveReport {
        &self.report
    }
}

pub fn plan_completed_archive(
    snapshot: &SessionSnapshot,
    cwd: Option<&Path>,
    updated_before: Option<SystemTime>,
    limit: usize,
) -> BulkArchivePlan {
    let completed_seen = snapshot
        .sessions
        .iter()
        .filter(|session| session.state == SessionState::Completed)
        .count();
    let scoped = snapshot.sessions.iter().filter(|session| {
        session.state == SessionState::Completed
            && cwd
                .map(|root| session.cwd.starts_with(root))
                .unwrap_or(true)
            && updated_before
                .map(|cutoff| session.updated_at.is_some_and(|updated| updated <= cutoff))
                .unwrap_or(true)
    });
    let mut matched_scope = 0;
    let mut skipped_without_authority = 0;
    let mut eligible_sessions = Vec::new();
    for session in scoped {
        matched_scope += 1;
        if session.capabilities.contains(&Capability::Archive) {
            eligible_sessions.push(session.clone());
        } else {
            skipped_without_authority += 1;
        }
    }
    let eligible = eligible_sessions.len();
    let sessions = eligible_sessions
        .into_iter()
        .take(limit)
        .collect::<Vec<_>>();
    let selected = sessions.iter().map(ArchiveItem::from).collect();

    BulkArchivePlan {
        report: BulkArchiveReport {
            dry_run: true,
            completed_seen,
            matched_scope,
            eligible,
            selected,
            skipped_without_authority,
            archived: Vec::new(),
            failures: Vec::new(),
        },
        sessions,
    }
}

pub fn execute_completed_archive(
    plan: &BulkArchivePlan,
    mut archive: impl FnMut(&AgentSession) -> anyhow::Result<()>,
) -> BulkArchiveReport {
    let mut report = plan.report.clone();
    report.dry_run = false;
    for session in &plan.sessions {
        let item = ArchiveItem::from(session);
        match archive(session) {
            Ok(()) => report.archived.push(item),
            Err(error) => report.failures.push(ArchiveFailure {
                session: item,
                error: format!("{error:#}"),
            }),
        }
    }
    report
}

#[cfg(test)]
mod tests {
    use std::collections::BTreeSet;
    use std::time::Duration;

    use super::*;
    use crate::domain::{Runtime, SessionKind};

    fn session(id: &str, state: SessionState, updated_days: u64, archive: bool) -> AgentSession {
        let mut capabilities = BTreeSet::new();
        if archive {
            capabilities.insert(Capability::Archive);
        }
        AgentSession {
            id: format!("codex:host:{id}"),
            provider_session_id: id.into(),
            provider: Provider::Codex,
            runtime: Runtime::Host,
            kind: SessionKind::Managed,
            name: id.into(),
            cwd: PathBuf::from("/work/project"),
            state,
            summary: String::new(),
            raw_state: None,
            pid: None,
            started_at: None,
            updated_at: Some(SystemTime::UNIX_EPOCH + Duration::from_secs(updated_days * 86_400)),
            pull_requests: None,
            capabilities,
        }
    }

    #[test]
    fn planning_requires_completed_state_scope_age_and_exact_archive_authority() {
        let mut outside = session("outside", SessionState::Completed, 1, true);
        outside.cwd = PathBuf::from("/elsewhere");
        let snapshot = SessionSnapshot {
            sessions: vec![
                session("old-owned", SessionState::Completed, 1, true),
                session("new-owned", SessionState::Completed, 9, true),
                session("old-unowned", SessionState::Completed, 1, false),
                session("working", SessionState::Working, 1, true),
                outside,
            ],
            warnings: vec![],
        };

        let plan = plan_completed_archive(
            &snapshot,
            Some(Path::new("/work")),
            Some(SystemTime::UNIX_EPOCH + Duration::from_secs(5 * 86_400)),
            100,
        );

        assert_eq!(plan.report.completed_seen, 4);
        assert_eq!(plan.report.matched_scope, 2);
        assert_eq!(plan.report.eligible, 1);
        assert_eq!(plan.report.skipped_without_authority, 1);
        assert_eq!(plan.report.selected[0].provider_session_id, "old-owned");
    }

    #[test]
    fn planning_limits_the_mutation_batch_without_hiding_total_eligibility() {
        let snapshot = SessionSnapshot {
            sessions: vec![
                session("one", SessionState::Completed, 1, true),
                session("two", SessionState::Completed, 2, true),
                session("three", SessionState::Completed, 3, true),
            ],
            warnings: vec![],
        };

        let plan = plan_completed_archive(&snapshot, None, None, 2);

        assert_eq!(plan.report.eligible, 3);
        assert_eq!(plan.report.selected.len(), 2);
        assert_eq!(plan.sessions.len(), 2);
    }

    #[test]
    fn execution_continues_after_failure_and_never_receives_unowned_sessions() {
        let snapshot = SessionSnapshot {
            sessions: vec![
                session("one", SessionState::Completed, 1, true),
                session("unowned", SessionState::Completed, 1, false),
                session("two", SessionState::Completed, 2, true),
            ],
            warnings: vec![],
        };
        let plan = plan_completed_archive(&snapshot, None, None, 100);
        let mut attempted = Vec::new();

        let report = execute_completed_archive(&plan, |session| {
            attempted.push(session.provider_session_id.clone());
            if session.provider_session_id == "one" {
                anyhow::bail!("simulated provider refusal");
            }
            Ok(())
        });

        assert_eq!(attempted, vec!["one", "two"]);
        assert_eq!(report.archived.len(), 1);
        assert_eq!(report.archived[0].provider_session_id, "two");
        assert_eq!(report.failures.len(), 1);
        assert!(report.failures[0]
            .error
            .contains("simulated provider refusal"));
    }
}

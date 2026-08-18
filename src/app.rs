use std::collections::{BTreeMap, BTreeSet};
use std::path::PathBuf;
use std::time::SystemTime;

use crate::domain::{AgentSession, Capability, SessionSnapshot, SessionState};

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ViewMode {
    Status,
    Directory,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum SelectionKey {
    Group(String),
    Session(String),
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum Overlay {
    None,
    Help,
    Peek,
    Composer(ComposerMode),
    Confirm(ConfirmTarget),
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum ComposerMode {
    NewSession,
    Rename { session_id: String },
    Filter,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum ConfirmTarget {
    Session {
        id: String,
        running: bool,
    },
    Archive {
        id: String,
    },
    Group {
        key: String,
        session_ids: Vec<String>,
    },
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum AppAction {
    None,
    Quit,
    Open { session_id: String },
    Inspect { session_id: String },
    Launch { prompt: String },
    Reply { session_id: String, prompt: String },
    ResolveApproval { session_id: String, accept: bool },
    RespondInput { session_id: String, answer: String },
    Rename { session_id: String, name: String },
    Interrupt { session_id: String },
    Archive { session_id: String },
    Delete { session_ids: Vec<String> },
}

#[derive(Clone, Debug)]
pub struct Group {
    pub key: String,
    pub label: String,
    pub sessions: Vec<usize>,
}

#[derive(Debug)]
pub struct App {
    pub snapshot: SessionSnapshot,
    pub view_mode: ViewMode,
    pub selection: Option<SelectionKey>,
    pub overlay: Overlay,
    pub input: String,
    pub filter: String,
    pub collapsed: BTreeSet<String>,
    pub notice: Option<String>,
    pub details: BTreeMap<String, String>,
    pub refreshed_at: SystemTime,
    pub should_quit: bool,
}

impl App {
    pub fn new(snapshot: SessionSnapshot) -> Self {
        let mut app = Self {
            snapshot,
            view_mode: ViewMode::Status,
            selection: None,
            overlay: Overlay::None,
            input: String::new(),
            filter: String::new(),
            collapsed: BTreeSet::new(),
            notice: None,
            details: BTreeMap::new(),
            refreshed_at: SystemTime::now(),
            should_quit: false,
        };
        app.reconcile_selection();
        app
    }

    pub fn replace_snapshot(&mut self, snapshot: SessionSnapshot) {
        self.snapshot = snapshot;
        self.refreshed_at = SystemTime::now();
        let previous_selection = self.selection.clone();
        self.reconcile_selection();
        if self.selection != previous_selection {
            self.overlay = Overlay::None;
            self.input.clear();
        }
    }

    pub fn groups(&self) -> Vec<Group> {
        match self.view_mode {
            ViewMode::Status => self.status_groups(),
            ViewMode::Directory => self.directory_groups(),
        }
    }

    pub fn selectable_keys(&self) -> Vec<SelectionKey> {
        let mut keys = Vec::new();
        for group in self.groups() {
            keys.push(SelectionKey::Group(group.key.clone()));
            if !self.collapsed.contains(&group.key) {
                keys.extend(
                    group.sessions.into_iter().map(|index| {
                        SelectionKey::Session(self.snapshot.sessions[index].id.clone())
                    }),
                );
            }
        }
        keys
    }

    pub fn select_next(&mut self) {
        self.move_selection(1);
    }

    pub fn select_previous(&mut self) {
        self.move_selection(-1);
    }

    fn move_selection(&mut self, delta: isize) {
        let keys = self.selectable_keys();
        if keys.is_empty() {
            self.selection = None;
            return;
        }
        let current = self
            .selection
            .as_ref()
            .and_then(|selected| keys.iter().position(|key| key == selected));
        let next = match current {
            Some(index) => (index as isize + delta).rem_euclid(keys.len() as isize) as usize,
            None if delta < 0 => keys.len() - 1,
            None => 0,
        };
        self.selection = Some(keys[next].clone());
        self.notice = None;
    }

    pub fn selected_session(&self) -> Option<&AgentSession> {
        let SelectionKey::Session(id) = self.selection.as_ref()? else {
            return None;
        };
        self.snapshot
            .sessions
            .iter()
            .find(|session| &session.id == id)
    }

    pub fn selected_group(&self) -> Option<Group> {
        let SelectionKey::Group(key) = self.selection.as_ref()? else {
            return None;
        };
        self.groups().into_iter().find(|group| &group.key == key)
    }

    pub fn activate(&mut self) -> AppAction {
        match self.overlay.clone() {
            Overlay::Help => {
                self.overlay = Overlay::None;
                AppAction::None
            }
            Overlay::Peek if self.input.trim().is_empty() => self
                .selected_session()
                .map(|session| AppAction::Open {
                    session_id: session.id.clone(),
                })
                .unwrap_or(AppAction::None),
            Overlay::Peek => {
                let Some(session) = self.selected_session() else {
                    return AppAction::None;
                };
                if session.capabilities.contains(&Capability::Respond) {
                    let action = AppAction::RespondInput {
                        session_id: session.id.clone(),
                        answer: self.input.trim().to_owned(),
                    };
                    self.input.clear();
                    return action;
                }
                if !session.capabilities.contains(&Capability::Reply) {
                    let name = session.name.clone();
                    self.set_notice(format!(
                        "{name} is read-only; reply authority was not granted"
                    ));
                    self.input.clear();
                    return AppAction::None;
                }
                let action = AppAction::Reply {
                    session_id: session.id.clone(),
                    prompt: self.input.trim().to_owned(),
                };
                self.input.clear();
                action
            }
            Overlay::Composer(mode) => self.submit_composer(mode),
            Overlay::Confirm(target) => self.confirm(target),
            Overlay::None => match self.selection.clone() {
                Some(SelectionKey::Group(key)) => {
                    if !self.collapsed.remove(&key) {
                        self.collapsed.insert(key);
                    }
                    AppAction::None
                }
                Some(SelectionKey::Session(_)) => {
                    let session_id = self
                        .selected_session()
                        .map(|session| session.id.clone())
                        .unwrap_or_default();
                    AppAction::Open { session_id }
                }
                None => AppAction::None,
            },
        }
    }

    pub fn escape(&mut self) -> AppAction {
        match self.overlay {
            Overlay::None => {
                self.should_quit = true;
                AppAction::Quit
            }
            _ => {
                self.overlay = Overlay::None;
                self.input.clear();
                AppAction::None
            }
        }
    }

    pub fn toggle_help(&mut self) {
        self.notice = None;
        self.overlay = if self.overlay == Overlay::Help {
            Overlay::None
        } else {
            Overlay::Help
        };
    }

    pub fn toggle_view(&mut self) {
        self.notice = None;
        self.view_mode = match self.view_mode {
            ViewMode::Status => ViewMode::Directory,
            ViewMode::Directory => ViewMode::Status,
        };
        self.collapsed.clear();
        self.reconcile_selection();
    }

    pub fn toggle_peek(&mut self) {
        let Some(session) = self.selected_session() else {
            return;
        };
        if !session.capabilities.contains(&Capability::Inspect) {
            let name = session.name.clone();
            self.set_notice(format!(
                "{name} is observe/open-only; transcript inspection was not granted"
            ));
            return;
        }
        self.overlay = if self.overlay == Overlay::Peek {
            self.input.clear();
            Overlay::None
        } else {
            Overlay::Peek
        };
        self.notice = None;
    }

    pub fn start_new_session(&mut self, first_character: Option<char>) {
        self.notice = None;
        self.input.clear();
        if let Some(character) = first_character {
            self.input.push(character);
        }
        self.overlay = Overlay::Composer(ComposerMode::NewSession);
    }

    pub fn start_filter(&mut self) {
        self.notice = None;
        self.input = self.filter.clone();
        self.overlay = Overlay::Composer(ComposerMode::Filter);
    }

    pub fn start_rename(&mut self) {
        let Some(session) = self.selected_session() else {
            return;
        };
        let session_id = session.id.clone();
        let name = session.name.clone();
        self.notice = None;
        self.input = name;
        self.overlay = Overlay::Composer(ComposerMode::Rename { session_id });
    }

    pub fn start_confirm(&mut self) {
        if let Some(session) = self.selected_session() {
            let running = is_active_session_state(session.state);
            let required = if running {
                Capability::Interrupt
            } else {
                Capability::Delete
            };
            if !session.capabilities.contains(&required) {
                self.set_notice(format!(
                    "{} is observe-only; {:?} authority was not granted",
                    session.name, required
                ));
                return;
            }
            self.overlay = Overlay::Confirm(ConfirmTarget::Session {
                id: session.id.clone(),
                running,
            });
            self.notice = None;
        } else if let Some(group) = self.selected_group() {
            if group
                .sessions
                .iter()
                .any(|index| is_active_session_state(self.snapshot.sessions[*index].state))
            {
                self.set_notice("bulk stop is unavailable; select one running session");
                return;
            }
            let deletable = group.sessions.iter().all(|index| {
                self.snapshot.sessions[*index]
                    .capabilities
                    .contains(&Capability::Delete)
            });
            if !deletable {
                self.set_notice("this group includes observe-only sessions");
                return;
            }
            self.overlay = Overlay::Confirm(ConfirmTarget::Group {
                key: group.key,
                session_ids: group
                    .sessions
                    .iter()
                    .map(|index| self.snapshot.sessions[*index].id.clone())
                    .collect(),
            });
            self.notice = None;
        }
    }

    pub fn start_archive_confirm(&mut self) {
        let Some(session) = self.selected_session() else {
            return;
        };
        if is_active_session_state(session.state) {
            let name = session.name.clone();
            self.set_notice(format!("{name} must be idle before it can be archived"));
            return;
        }
        if !session.capabilities.contains(&Capability::Archive) {
            let name = session.name.clone();
            self.set_notice(format!(
                "{name} is observe-only; Archive authority was not granted"
            ));
            return;
        }
        self.overlay = Overlay::Confirm(ConfirmTarget::Archive {
            id: session.id.clone(),
        });
        self.notice = None;
    }

    pub fn resolve_approval(&mut self, accept: bool) -> AppAction {
        if self.overlay != Overlay::Peek {
            return AppAction::None;
        }
        let Some(session) = self.selected_session() else {
            return AppAction::None;
        };
        let required = if accept {
            Capability::Approve
        } else {
            Capability::Decline
        };
        if !session.capabilities.contains(&required) {
            return AppAction::None;
        }
        AppAction::ResolveApproval {
            session_id: session.id.clone(),
            accept,
        }
    }

    pub fn push_input(&mut self, character: char) {
        let peek_is_writable = self.overlay == Overlay::Peek
            && self.selected_session().is_some_and(|session| {
                session.capabilities.contains(&Capability::Reply)
                    || session.capabilities.contains(&Capability::Respond)
            });
        if peek_is_writable || matches!(self.overlay, Overlay::Composer(_)) {
            self.input.push(character);
        }
    }

    pub fn pop_input(&mut self) {
        self.input.pop();
    }

    pub fn set_notice(&mut self, notice: impl Into<String>) {
        self.notice = Some(notice.into());
    }

    pub fn set_detail(&mut self, session_id: String, detail: String) {
        self.details.insert(session_id, detail);
    }

    pub fn selected_detail(&self) -> Option<&str> {
        let session = self.selected_session()?;
        self.details.get(&session.id).map(String::as_str)
    }

    pub fn filtered(&self, session: &AgentSession) -> bool {
        if self.filter.is_empty() {
            return true;
        }
        let needle = self.filter.to_ascii_lowercase();
        session.name.to_ascii_lowercase().contains(&needle)
            || session.summary.to_ascii_lowercase().contains(&needle)
            || session
                .cwd
                .to_string_lossy()
                .to_ascii_lowercase()
                .contains(&needle)
            || session
                .provider
                .label()
                .to_ascii_lowercase()
                .contains(&needle)
    }

    fn submit_composer(&mut self, mode: ComposerMode) -> AppAction {
        let input = self.input.trim().to_owned();
        if input.is_empty() && mode != ComposerMode::Filter {
            return AppAction::None;
        }
        self.input.clear();
        self.overlay = Overlay::None;
        match mode {
            ComposerMode::NewSession => AppAction::Launch { prompt: input },
            ComposerMode::Rename { session_id } => AppAction::Rename {
                session_id,
                name: input,
            },
            ComposerMode::Filter => {
                self.filter = input;
                self.reconcile_selection();
                AppAction::None
            }
        }
    }

    fn confirm(&mut self, target: ConfirmTarget) -> AppAction {
        self.overlay = Overlay::None;
        match target {
            ConfirmTarget::Session { id, running: true } => AppAction::Interrupt { session_id: id },
            ConfirmTarget::Session { id, running: false } => AppAction::Delete {
                session_ids: vec![id],
            },
            ConfirmTarget::Archive { id } => AppAction::Archive { session_id: id },
            ConfirmTarget::Group { session_ids, .. } => AppAction::Delete { session_ids },
        }
    }

    fn status_groups(&self) -> Vec<Group> {
        SessionState::DISPLAY_ORDER
            .iter()
            .filter_map(|state| {
                let sessions: Vec<_> = self
                    .snapshot
                    .sessions
                    .iter()
                    .enumerate()
                    .filter(|(_, session)| session.state == *state && self.filtered(session))
                    .map(|(index, _)| index)
                    .collect();
                (!sessions.is_empty()).then(|| Group {
                    key: format!("state:{state:?}"),
                    label: state.heading().into(),
                    sessions,
                })
            })
            .collect()
    }

    fn directory_groups(&self) -> Vec<Group> {
        let mut groups: BTreeMap<PathBuf, Vec<usize>> = BTreeMap::new();
        for (index, session) in self.snapshot.sessions.iter().enumerate() {
            if self.filtered(session) {
                groups
                    .entry(project_group_path(&session.cwd))
                    .or_default()
                    .push(index);
            }
        }
        groups
            .into_iter()
            .map(|(path, sessions)| Group {
                key: format!("cwd:{}", path.display()),
                label: abbreviate_home(&path),
                sessions,
            })
            .collect()
    }

    fn reconcile_selection(&mut self) {
        let keys = self.selectable_keys();
        if self
            .selection
            .as_ref()
            .is_some_and(|selection| keys.contains(selection))
        {
            return;
        }
        self.selection = keys
            .iter()
            .find(|key| matches!(key, SelectionKey::Session(_)))
            .cloned()
            .or_else(|| keys.first().cloned());
    }
}

pub(crate) fn is_active_session_state(state: SessionState) -> bool {
    state != SessionState::Completed
}

pub(crate) fn project_group_path(path: &std::path::Path) -> PathBuf {
    let components = path.components().collect::<Vec<_>>();
    let worktree_marker = components
        .windows(2)
        .position(|pair| pair[0].as_os_str() == ".claude" && pair[1].as_os_str() == "worktrees");
    worktree_marker
        .map(|index| components[..index].iter().collect())
        .unwrap_or_else(|| path.to_path_buf())
}

fn abbreviate_home(path: &std::path::Path) -> String {
    std::env::var_os("HOME")
        .map(PathBuf::from)
        .and_then(|home| path.strip_prefix(home).ok().map(PathBuf::from))
        .map(|relative| format!("~/{}", relative.display()))
        .unwrap_or_else(|| path.display().to_string())
}

#[cfg(test)]
mod tests {
    use std::collections::BTreeSet;

    use crate::domain::{Capability, Provider, Runtime, SessionKind};

    use super::*;

    fn session(id: &str, state: SessionState) -> AgentSession {
        AgentSession {
            id: id.into(),
            provider_session_id: id.into(),
            provider: Provider::Claude,
            runtime: Runtime::Host,
            kind: SessionKind::Background,
            name: id.into(),
            cwd: PathBuf::from("/work"),
            state,
            summary: format!("summary {id}"),
            raw_state: None,
            pid: None,
            started_at: None,
            updated_at: None,
            pull_requests: None,
            capabilities: BTreeSet::from([Capability::Inspect]),
        }
    }

    #[test]
    fn navigation_wraps_across_headers_and_rows() {
        let mut app = App::new(SessionSnapshot {
            sessions: vec![
                session("one", SessionState::Working),
                session("two", SessionState::Completed),
            ],
            warnings: vec![],
        });

        assert_eq!(app.selection, Some(SelectionKey::Session("one".into())));
        app.selection = Some(SelectionKey::Session("two".into()));
        assert_eq!(app.selection, Some(SelectionKey::Session("two".into())));
        app.select_next();
        assert_eq!(
            app.selection,
            Some(SelectionKey::Group("state:Working".into()))
        );
    }

    #[test]
    fn collapse_removes_group_children_from_navigation() {
        let mut app = App::new(SessionSnapshot {
            sessions: vec![session("one", SessionState::Working)],
            warnings: vec![],
        });
        app.selection = Some(SelectionKey::Group("state:Working".into()));

        app.activate();

        assert_eq!(app.selectable_keys().len(), 1);
    }

    #[test]
    fn filter_matches_summary_case_insensitively() {
        let mut app = App::new(SessionSnapshot {
            sessions: vec![session("one", SessionState::Working)],
            warnings: vec![],
        });
        app.filter = "SUMMARY ONE".into();

        assert_eq!(app.groups()[0].sessions, vec![0]);
    }

    #[test]
    fn escape_quits_directly_from_a_selected_row() {
        let mut app = App::new(SessionSnapshot {
            sessions: vec![session("one", SessionState::Working)],
            warnings: vec![],
        });

        assert_eq!(app.escape(), AppAction::Quit);
        assert!(app.should_quit);
    }

    #[test]
    fn filter_reconciles_a_selection_that_is_no_longer_visible() {
        let mut app = App::new(SessionSnapshot {
            sessions: vec![
                session("one", SessionState::Working),
                session("two", SessionState::Completed),
            ],
            warnings: vec![],
        });
        app.start_filter();
        app.input = "two".into();

        assert_eq!(app.activate(), AppAction::None);
        assert_eq!(app.selection, Some(SelectionKey::Session("two".into())));
    }

    #[test]
    fn claude_worktrees_group_under_the_owning_project() {
        let mut item = session("one", SessionState::Working);
        item.cwd = PathBuf::from("/repo/.claude/worktrees/topic/src");
        let mut app = App::new(SessionSnapshot {
            sessions: vec![item],
            warnings: vec![],
        });
        app.toggle_view();

        assert_eq!(app.groups()[0].label, "/repo");
        assert_eq!(app.groups()[0].key, "cwd:/repo");
    }

    #[test]
    fn needs_input_is_treated_as_running_for_confirmation() {
        let mut item = session("one", SessionState::NeedsInput);
        item.capabilities.insert(Capability::Interrupt);
        let mut app = App::new(SessionSnapshot {
            sessions: vec![item],
            warnings: vec![],
        });

        app.start_confirm();
        assert_eq!(
            app.overlay,
            Overlay::Confirm(ConfirmTarget::Session {
                id: "one".into(),
                running: true,
            })
        );
        assert_eq!(
            app.activate(),
            AppAction::Interrupt {
                session_id: "one".into()
            }
        );
    }

    #[test]
    fn active_groups_do_not_offer_bulk_deletion() {
        let mut app = App::new(SessionSnapshot {
            sessions: vec![session("one", SessionState::Working)],
            warnings: vec![],
        });
        app.selection = Some(SelectionKey::Group("state:Working".into()));

        app.start_confirm();

        assert_eq!(app.overlay, Overlay::None);
        assert_eq!(
            app.notice.as_deref(),
            Some("bulk stop is unavailable; select one running session")
        );
    }

    #[test]
    fn archive_requires_capability_and_an_idle_session() {
        let mut item = session("one", SessionState::Completed);
        item.capabilities.insert(Capability::Archive);
        let mut app = App::new(SessionSnapshot {
            sessions: vec![item],
            warnings: vec![],
        });

        app.start_archive_confirm();

        assert_eq!(
            app.overlay,
            Overlay::Confirm(ConfirmTarget::Archive { id: "one".into() })
        );
        assert_eq!(
            app.activate(),
            AppAction::Archive {
                session_id: "one".into()
            }
        );
    }

    #[test]
    fn read_only_peek_cannot_accept_reply_text() {
        let mut app = App::new(SessionSnapshot {
            sessions: vec![session("one", SessionState::Working)],
            warnings: vec![],
        });
        app.toggle_peek();

        app.push_input('x');

        assert_eq!(app.overlay, Overlay::Peek);
        assert!(app.input.is_empty());
    }

    #[test]
    fn pending_approval_keys_require_the_exact_granted_decision() {
        let mut item = session("one", SessionState::NeedsInput);
        item.capabilities.insert(Capability::Decline);
        let mut app = App::new(SessionSnapshot {
            sessions: vec![item],
            warnings: vec![],
        });
        app.toggle_peek();

        assert_eq!(app.resolve_approval(true), AppAction::None);
        assert_eq!(
            app.resolve_approval(false),
            AppAction::ResolveApproval {
                session_id: "one".into(),
                accept: false,
            }
        );
    }

    #[test]
    fn structured_input_uses_a_distinct_action_from_turn_reply() {
        let mut item = session("one", SessionState::NeedsInput);
        item.capabilities.insert(Capability::Respond);
        let mut app = App::new(SessionSnapshot {
            sessions: vec![item],
            warnings: vec![],
        });
        app.toggle_peek();
        for character in "staging".chars() {
            app.push_input(character);
        }

        assert_eq!(
            app.activate(),
            AppAction::RespondInput {
                session_id: "one".into(),
                answer: "staging".into(),
            }
        );
    }

    fn app_with(items: Vec<AgentSession>) -> App {
        App::new(SessionSnapshot {
            sessions: items,
            warnings: vec![],
        })
    }

    fn grant(item: &mut AgentSession, capabilities: &[Capability]) {
        item.capabilities.extend(capabilities.iter().cloned());
    }

    #[test]
    fn empty_snapshots_have_no_selection_and_navigation_is_a_noop() {
        let mut app = app_with(vec![]);

        app.select_next();
        app.select_previous();

        assert_eq!(app.selection, None);
        assert_eq!(app.activate(), AppAction::None);
    }

    #[test]
    fn reverse_navigation_from_no_selection_wraps_to_the_last_row() {
        let mut app = app_with(vec![
            session("one", SessionState::Working),
            session("two", SessionState::Completed),
        ]);
        app.selection = None;
        app.notice = Some("old notice".into());

        app.select_previous();

        assert_eq!(app.selection, Some(SelectionKey::Session("two".into())));
        assert_eq!(app.notice, None);
    }

    #[test]
    fn status_groups_follow_display_order_and_exclude_filtered_rows() {
        let mut app = app_with(vec![
            session("done", SessionState::Completed),
            session("working", SessionState::Working),
            session("input", SessionState::NeedsInput),
            session("review", SessionState::ReadyForReview),
        ]);

        assert_eq!(
            app.groups()
                .iter()
                .map(|group| group.label.as_str())
                .collect::<Vec<_>>(),
            vec!["Ready for review", "Needs input", "Working", "Completed"]
        );

        app.filter = "summary working".into();
        assert_eq!(app.groups().len(), 1);
        assert_eq!(app.groups()[0].sessions, vec![1]);
    }

    #[test]
    fn filter_matches_name_cwd_and_provider_and_rejects_unmatched_rows() {
        let mut item = session("agent-name", SessionState::Working);
        item.cwd = PathBuf::from("/projects/special-root");
        let mut app = app_with(vec![item]);

        for needle in ["AGENT-NAME", "SPECIAL-ROOT", "CLAUDE"] {
            app.filter = needle.into();
            assert_eq!(app.groups().len(), 1, "filter {needle}");
        }
        app.filter = "missing".into();
        assert!(app.groups().is_empty());
    }

    #[test]
    fn directory_view_groups_and_sorts_project_paths() {
        let mut z = session("z", SessionState::Working);
        z.cwd = PathBuf::from("/zeta");
        let mut a = session("a", SessionState::Completed);
        a.cwd = PathBuf::from("/alpha");
        let mut a_worktree = session("a-worktree", SessionState::NeedsInput);
        a_worktree.cwd = PathBuf::from("/alpha/.claude/worktrees/topic/src");
        let mut app = app_with(vec![z, a, a_worktree]);

        app.toggle_view();

        assert_eq!(
            app.groups()
                .iter()
                .map(|group| (group.key.as_str(), group.sessions.clone()))
                .collect::<Vec<_>>(),
            vec![("cwd:/alpha", vec![1, 2]), ("cwd:/zeta", vec![0])]
        );
    }

    #[test]
    fn activating_a_group_toggles_collapse_both_directions() {
        let mut app = app_with(vec![session("one", SessionState::Working)]);
        app.selection = Some(SelectionKey::Group("state:Working".into()));

        assert_eq!(app.activate(), AppAction::None);
        assert!(app.collapsed.contains("state:Working"));
        assert_eq!(app.activate(), AppAction::None);
        assert!(!app.collapsed.contains("state:Working"));
    }

    #[test]
    fn replacing_snapshot_preserves_valid_selection_but_closes_stale_overlay() {
        let mut app = app_with(vec![
            session("one", SessionState::Working),
            session("two", SessionState::Completed),
        ]);
        app.selection = Some(SelectionKey::Session("two".into()));
        app.overlay = Overlay::Peek;
        app.input = "draft".into();
        app.replace_snapshot(SessionSnapshot {
            sessions: vec![session("two", SessionState::Completed)],
            warnings: vec![],
        });
        assert_eq!(app.selection, Some(SelectionKey::Session("two".into())));
        assert_eq!(app.overlay, Overlay::Peek);
        assert_eq!(app.input, "draft");

        app.replace_snapshot(SessionSnapshot {
            sessions: vec![session("three", SessionState::Working)],
            warnings: vec![],
        });
        assert_eq!(app.selection, Some(SelectionKey::Session("three".into())));
        assert_eq!(app.overlay, Overlay::None);
        assert!(app.input.is_empty());
    }

    #[test]
    fn activate_covers_help_session_and_peek_without_selection() {
        let mut app = app_with(vec![session("one", SessionState::Working)]);
        app.overlay = Overlay::Help;
        assert_eq!(app.activate(), AppAction::None);
        assert_eq!(app.overlay, Overlay::None);

        app.overlay = Overlay::None;
        assert_eq!(
            app.activate(),
            AppAction::Open {
                session_id: "one".into()
            }
        );

        app.overlay = Overlay::Peek;
        app.selection = None;
        assert_eq!(app.activate(), AppAction::None);
    }

    #[test]
    fn escape_closes_every_overlay_before_it_quits() {
        let overlays = vec![
            Overlay::Help,
            Overlay::Peek,
            Overlay::Composer(ComposerMode::NewSession),
            Overlay::Confirm(ConfirmTarget::Archive { id: "one".into() }),
        ];
        for overlay in overlays {
            let mut app = app_with(vec![session("one", SessionState::Completed)]);
            app.overlay = overlay;
            app.input = "draft".into();
            assert_eq!(app.escape(), AppAction::None);
            assert_eq!(app.overlay, Overlay::None);
            assert!(app.input.is_empty());
            assert!(!app.should_quit);
            assert_eq!(app.escape(), AppAction::Quit);
            assert!(app.should_quit);
        }
    }

    #[test]
    fn toggling_view_clears_collapsed_groups_and_reconciles_selection() {
        let mut app = app_with(vec![session("one", SessionState::Working)]);
        app.selection = Some(SelectionKey::Group("state:Working".into()));
        app.collapsed.insert("state:Working".into());

        app.toggle_view();

        assert_eq!(app.view_mode, ViewMode::Directory);
        assert!(app.collapsed.is_empty());
        assert_eq!(app.selection, Some(SelectionKey::Session("one".into())));
        app.toggle_view();
        assert_eq!(app.view_mode, ViewMode::Status);
    }

    #[test]
    fn peek_requires_inspection_and_toggle_close_discards_input() {
        let mut item = session("one", SessionState::Working);
        item.capabilities.clear();
        let mut app = app_with(vec![item]);

        app.toggle_peek();
        assert_eq!(app.overlay, Overlay::None);
        assert!(app
            .notice
            .as_deref()
            .unwrap()
            .contains("inspection was not granted"));

        grant(
            &mut app.snapshot.sessions[0],
            &[Capability::Inspect, Capability::Reply],
        );
        app.toggle_peek();
        app.input = "draft".into();
        app.toggle_peek();
        assert_eq!(app.overlay, Overlay::None);
        assert!(app.input.is_empty());
    }

    #[test]
    fn composer_entry_points_seed_the_expected_input() {
        let mut app = app_with(vec![session("one", SessionState::Completed)]);
        app.start_new_session(Some('x'));
        assert_eq!(app.input, "x");
        assert_eq!(app.overlay, Overlay::Composer(ComposerMode::NewSession));

        app.filter = "saved".into();
        app.start_filter();
        assert_eq!(app.input, "saved");
        assert_eq!(app.overlay, Overlay::Composer(ComposerMode::Filter));

        app.start_rename();
        assert_eq!(app.input, "one");
        assert_eq!(
            app.overlay,
            Overlay::Composer(ComposerMode::Rename {
                session_id: "one".into()
            })
        );

        app.selection = None;
        app.overlay = Overlay::None;
        app.start_rename();
        assert_eq!(app.overlay, Overlay::None);
    }

    #[test]
    fn every_composer_submission_maps_to_its_exact_action() {
        let mut app = app_with(vec![session("one", SessionState::Completed)]);

        app.start_new_session(None);
        app.input = "  build it  ".into();
        assert_eq!(
            app.activate(),
            AppAction::Launch {
                prompt: "build it".into()
            }
        );

        app.overlay = Overlay::Composer(ComposerMode::Rename {
            session_id: "one".into(),
        });
        app.input = "  new name  ".into();
        assert_eq!(
            app.activate(),
            AppAction::Rename {
                session_id: "one".into(),
                name: "new name".into()
            }
        );

        app.filter = "old".into();
        app.start_filter();
        app.input = "   ".into();
        assert_eq!(app.activate(), AppAction::None);
        assert!(app.filter.is_empty());
        assert_eq!(app.overlay, Overlay::None);
    }

    #[test]
    fn empty_non_filter_composer_does_not_submit_or_close() {
        let mut app = app_with(vec![]);
        app.start_new_session(None);
        app.input = "   ".into();

        assert_eq!(app.activate(), AppAction::None);
        assert_eq!(app.overlay, Overlay::Composer(ComposerMode::NewSession));
        assert_eq!(app.input, "   ");
    }

    #[test]
    fn peek_reply_requires_capability_and_clears_rejected_or_sent_text() {
        let mut app = app_with(vec![session("one", SessionState::Working)]);
        app.toggle_peek();
        app.input = "cannot send".into();
        assert_eq!(app.activate(), AppAction::None);
        assert!(app.input.is_empty());
        assert!(app.notice.as_deref().unwrap().contains("read-only"));

        grant(&mut app.snapshot.sessions[0], &[Capability::Reply]);
        app.input = "  proceed  ".into();
        assert_eq!(
            app.activate(),
            AppAction::Reply {
                session_id: "one".into(),
                prompt: "proceed".into()
            }
        );
        assert!(app.input.is_empty());
    }

    #[test]
    fn active_and_idle_session_confirmations_require_exact_authority() {
        let mut active = session("active", SessionState::ReadyForReview);
        active.capabilities.clear();
        let mut app = app_with(vec![active]);
        app.start_confirm();
        assert_eq!(app.overlay, Overlay::None);
        assert!(app.notice.as_deref().unwrap().contains("Interrupt"));

        grant(&mut app.snapshot.sessions[0], &[Capability::Interrupt]);
        app.start_confirm();
        assert_eq!(
            app.activate(),
            AppAction::Interrupt {
                session_id: "active".into()
            }
        );

        app.snapshot.sessions[0].state = SessionState::Completed;
        app.snapshot.sessions[0].capabilities.clear();
        app.start_confirm();
        assert!(app.notice.as_deref().unwrap().contains("Delete"));
        grant(&mut app.snapshot.sessions[0], &[Capability::Delete]);
        app.start_confirm();
        assert_eq!(
            app.activate(),
            AppAction::Delete {
                session_ids: vec!["active".into()]
            }
        );
    }

    #[test]
    fn completed_group_delete_is_all_or_nothing_and_preserves_order() {
        let mut one = session("one", SessionState::Completed);
        let two = session("two", SessionState::Completed);
        grant(&mut one, &[Capability::Delete]);
        let mut app = app_with(vec![one, two]);
        app.selection = Some(SelectionKey::Group("state:Completed".into()));

        app.start_confirm();
        assert_eq!(app.overlay, Overlay::None);
        assert_eq!(
            app.notice.as_deref(),
            Some("this group includes observe-only sessions")
        );

        grant(&mut app.snapshot.sessions[1], &[Capability::Delete]);
        app.start_confirm();
        assert_eq!(
            app.activate(),
            AppAction::Delete {
                session_ids: vec!["one".into(), "two".into()]
            }
        );
    }

    #[test]
    fn archive_refuses_active_or_unowned_sessions() {
        let mut item = session("one", SessionState::Working);
        grant(&mut item, &[Capability::Archive]);
        let mut app = app_with(vec![item]);
        app.start_archive_confirm();
        assert!(app.notice.as_deref().unwrap().contains("must be idle"));
        assert_eq!(app.overlay, Overlay::None);

        app.snapshot.sessions[0].state = SessionState::Completed;
        app.snapshot.sessions[0].capabilities.clear();
        app.start_archive_confirm();
        assert!(app.notice.as_deref().unwrap().contains("Archive authority"));
    }

    #[test]
    fn approval_resolution_requires_peek_selection_and_exact_capability() {
        let mut item = session("one", SessionState::NeedsInput);
        grant(&mut item, &[Capability::Approve, Capability::Decline]);
        let mut app = app_with(vec![item]);

        assert_eq!(app.resolve_approval(true), AppAction::None);
        app.toggle_peek();
        assert_eq!(
            app.resolve_approval(true),
            AppAction::ResolveApproval {
                session_id: "one".into(),
                accept: true
            }
        );
        app.selection = None;
        assert_eq!(app.resolve_approval(false), AppAction::None);
    }

    #[test]
    fn input_editing_is_limited_to_composers_and_writable_peek() {
        let mut app = app_with(vec![session("one", SessionState::Working)]);
        app.push_input('x');
        assert!(app.input.is_empty());

        app.start_new_session(None);
        app.push_input('a');
        app.push_input('b');
        app.pop_input();
        assert_eq!(app.input, "a");

        app.overlay = Overlay::Peek;
        app.input.clear();
        grant(&mut app.snapshot.sessions[0], &[Capability::Respond]);
        app.push_input('z');
        assert_eq!(app.input, "z");
    }

    #[test]
    fn details_are_selected_by_exact_session_id() {
        let mut app = app_with(vec![
            session("one", SessionState::Working),
            session("two", SessionState::Completed),
        ]);
        app.set_detail("one".into(), "first transcript".into());
        app.set_detail("two".into(), "second transcript".into());
        assert_eq!(app.selected_detail(), Some("first transcript"));
        app.selection = Some(SelectionKey::Session("two".into()));
        assert_eq!(app.selected_detail(), Some("second transcript"));
        app.selection = Some(SelectionKey::Group("state:Completed".into()));
        assert_eq!(app.selected_detail(), None);
    }

    #[test]
    fn successful_overlay_and_view_transitions_clear_stale_notices() {
        let mut item = session("one", SessionState::Completed);
        grant(
            &mut item,
            &[Capability::Inspect, Capability::Delete, Capability::Archive],
        );
        let mut app = app_with(vec![item]);

        app.notice = Some("stale".into());
        app.toggle_help();
        assert_eq!(app.notice, None);
        app.toggle_help();

        app.notice = Some("stale".into());
        app.toggle_view();
        assert_eq!(app.notice, None);

        app.notice = Some("stale".into());
        app.toggle_peek();
        assert_eq!(app.notice, None);
        app.escape();

        app.notice = Some("stale".into());
        app.start_new_session(None);
        assert_eq!(app.notice, None);
        app.escape();

        app.notice = Some("stale".into());
        app.start_filter();
        assert_eq!(app.notice, None);
        app.escape();

        app.notice = Some("stale".into());
        app.start_rename();
        assert_eq!(app.notice, None);
        app.escape();

        app.notice = Some("stale".into());
        app.start_confirm();
        assert_eq!(app.notice, None);
        app.escape();

        app.notice = Some("stale".into());
        app.start_archive_confirm();
        assert_eq!(app.notice, None);
    }

    #[test]
    fn refused_transition_replaces_stale_notice_with_the_refusal() {
        let mut item = session("one", SessionState::Completed);
        item.capabilities.clear();
        let mut app = app_with(vec![item]);
        app.notice = Some("stale".into());

        app.toggle_peek();

        assert_eq!(app.overlay, Overlay::None);
        assert_ne!(app.notice.as_deref(), Some("stale"));
        assert!(app
            .notice
            .as_deref()
            .unwrap()
            .contains("inspection was not granted"));
    }
}

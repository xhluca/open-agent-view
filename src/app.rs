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
    Details,
    Composer(ComposerMode),
    Confirm(ConfirmTarget),
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum ComposerMode {
    NewSession,
    Reply { session_id: String },
    Rename { session_id: String },
    Filter,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum ConfirmTarget {
    Session { id: String, running: bool },
    Archive { id: String },
    Group { key: String, session_ids: Vec<String> },
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum AppAction {
    None,
    Refresh,
    Quit,
    Open { session_id: String },
    Inspect { session_id: String },
    Launch { prompt: String },
    Reply { session_id: String, prompt: String },
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
                keys.extend(group.sessions.into_iter().map(|index| {
                    SelectionKey::Session(self.snapshot.sessions[index].id.clone())
                }));
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
        self.snapshot.sessions.iter().find(|session| &session.id == id)
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
            Overlay::Details => AppAction::None,
            Overlay::Peek if self.input.trim().is_empty() => AppAction::Open {
                session_id: self
                    .selected_session()
                    .map(|session| session.id.clone())
                    .unwrap_or_default(),
            },
            Overlay::Peek => {
                let Some(session) = self.selected_session() else {
                    return AppAction::None;
                };
                if !session.capabilities.contains(&Capability::Reply) {
                    let name = session.name.clone();
                    self.set_notice(format!("{name} is read-only; reply authority was not granted"));
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
        self.overlay = if self.overlay == Overlay::Help {
            Overlay::None
        } else {
            Overlay::Help
        };
    }

    pub fn toggle_view(&mut self) {
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
    }

    pub fn start_new_session(&mut self, first_character: Option<char>) {
        self.input.clear();
        if let Some(character) = first_character {
            self.input.push(character);
        }
        self.overlay = Overlay::Composer(ComposerMode::NewSession);
    }

    pub fn start_filter(&mut self) {
        self.input = self.filter.clone();
        self.overlay = Overlay::Composer(ComposerMode::Filter);
    }

    pub fn start_rename(&mut self) {
        let Some(session) = self.selected_session() else {
            return;
        };
        let session_id = session.id.clone();
        self.input = session.name.clone();
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
    }

    pub fn push_input(&mut self, character: char) {
        let peek_is_writable = self.overlay == Overlay::Peek
            && self
                .selected_session()
                .is_some_and(|session| session.capabilities.contains(&Capability::Reply));
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
            || session.cwd.to_string_lossy().to_ascii_lowercase().contains(&needle)
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
            ComposerMode::Reply { session_id } => AppAction::Reply {
                session_id,
                prompt: input,
            },
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
            ConfirmTarget::Session { id, running: true } => {
                AppAction::Interrupt { session_id: id }
            }
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
    let worktree_marker = components.windows(2).position(|pair| {
        pair[0].as_os_str() == ".claude" && pair[1].as_os_str() == "worktrees"
    });
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
}

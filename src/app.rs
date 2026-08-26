use std::collections::{BTreeMap, BTreeSet, HashMap};
use std::path::PathBuf;
use std::time::SystemTime;

use crate::domain::{
    AgentSession, Capability, LaunchTarget, Provider, SessionSnapshot, SessionState,
};

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ViewMode {
    Status,
    Directory,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum SelectionKey {
    Group(String),
    Session(String),
    ShowMore(String),
}

pub const SESSION_PAGE_SIZE: usize = 25;
pub const MODEL_PICKER_PAGE_SIZE: usize = 10;

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum Overlay {
    None,
    Help,
    Peek,
    HarnessPicker,
    ModelPicker,
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
    Archive {
        id: String,
    },
    Hide {
        session_ids: Vec<String>,
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
    Refresh,
    SetCompletedVisibility {
        include_completed: bool,
    },
    LoadModels {
        provider: Provider,
    },
    Authenticate {
        provider: Provider,
    },
    SetupProvider {
        provider: Provider,
    },
    SetupLaunchOption {
        provider: Provider,
        option: String,
    },
    Open {
        session_id: String,
    },
    Inspect {
        session_id: String,
    },
    Launch {
        provider: Provider,
        model: Option<String>,
        prompt: String,
    },
    Reply {
        session_id: String,
        prompt: String,
    },
    ResolveApproval {
        session_id: String,
        accept: bool,
    },
    RespondInput {
        session_id: String,
        answer: String,
    },
    Rename {
        session_id: String,
        name: String,
    },
    Interrupt {
        session_id: String,
    },
    Archive {
        session_id: String,
    },
    Delete {
        session_ids: Vec<String>,
    },
    Hide {
        session_ids: Vec<String>,
    },
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
    pub includes_completed: bool,
    pub view_mode: ViewMode,
    pub selection: Option<SelectionKey>,
    pub overlay: Overlay,
    pub input: String,
    pub filter: String,
    pub launch_targets: Vec<LaunchTarget>,
    pub launch_provider: Provider,
    pub launch_model: Option<String>,
    pub harness_selection: usize,
    pub available_models: Vec<String>,
    pub model_filter: String,
    pub model_selection: usize,
    pub models_loading: bool,
    pub models_provider: Option<Provider>,
    pub models_error: Option<String>,
    pub models_auth_available: bool,
    pub collapsed: BTreeSet<String>,
    visible_limits: BTreeMap<String, usize>,
    session_page_size: usize,
    group_cache: Vec<Group>,
    session_indices: HashMap<String, usize>,
    session_counts: BTreeMap<SessionState, usize>,
    provider_labels: Vec<String>,
    live_animation_visible: bool,
    #[cfg(test)]
    group_cache_rebuilds: usize,
    pub notice: Option<String>,
    pub details: BTreeMap<String, String>,
    pub refreshed_at: SystemTime,
    pub should_quit: bool,
}

impl App {
    pub fn new(snapshot: SessionSnapshot) -> Self {
        Self::with_completed_visibility(snapshot, true)
    }

    pub fn with_completed_visibility(snapshot: SessionSnapshot, includes_completed: bool) -> Self {
        Self::with_launch_targets(
            snapshot,
            includes_completed,
            Provider::Claude,
            vec![LaunchTarget {
                provider: Provider::Claude,
                supports_model: true,
            }],
        )
    }

    pub fn with_launch_targets(
        snapshot: SessionSnapshot,
        includes_completed: bool,
        default_provider: Provider,
        launch_targets: Vec<LaunchTarget>,
    ) -> Self {
        let launch_provider = launch_targets
            .iter()
            .find(|target| target.provider == default_provider)
            .map(|target| target.provider.clone())
            .or_else(|| launch_targets.first().map(|target| target.provider.clone()))
            .unwrap_or(default_provider);
        let harness_selection = launch_targets
            .iter()
            .position(|target| target.provider == launch_provider)
            .unwrap_or(0);
        let mut app = Self {
            snapshot,
            includes_completed,
            view_mode: ViewMode::Status,
            selection: None,
            overlay: Overlay::None,
            input: String::new(),
            filter: String::new(),
            launch_targets,
            launch_provider,
            launch_model: None,
            harness_selection,
            available_models: Vec::new(),
            model_filter: String::new(),
            model_selection: 0,
            models_loading: false,
            models_provider: None,
            models_error: None,
            models_auth_available: false,
            collapsed: BTreeSet::new(),
            visible_limits: BTreeMap::new(),
            session_page_size: SESSION_PAGE_SIZE,
            group_cache: Vec::new(),
            session_indices: HashMap::new(),
            session_counts: BTreeMap::new(),
            provider_labels: Vec::new(),
            live_animation_visible: true,
            #[cfg(test)]
            group_cache_rebuilds: 0,
            notice: None,
            details: BTreeMap::new(),
            refreshed_at: SystemTime::now(),
            should_quit: false,
        };
        app.rebuild_snapshot_cache();
        app.reconcile_selection();
        app
    }

    pub fn replace_snapshot(&mut self, snapshot: SessionSnapshot) {
        self.snapshot = snapshot;
        self.rebuild_snapshot_cache();
        self.refreshed_at = SystemTime::now();
        let previous_selection = self.selection.clone();
        self.reconcile_selection();
        let selection_bound_overlay = matches!(
            self.overlay,
            Overlay::Peek | Overlay::Composer(ComposerMode::Rename { .. }) | Overlay::Confirm(_)
        );
        if self.selection != previous_selection && selection_bound_overlay {
            self.overlay = Overlay::None;
            self.input.clear();
        }
    }

    pub fn groups(&self) -> &[Group] {
        &self.group_cache
    }

    pub fn selectable_keys(&self) -> Vec<SelectionKey> {
        let mut keys = Vec::new();
        for group in self.groups() {
            keys.push(SelectionKey::Group(group.key.clone()));
            if !self.collapsed.contains(&group.key) {
                let visible = self.visible_session_count(group);
                keys.extend(
                    group.sessions.iter().take(visible).map(|index| {
                        SelectionKey::Session(self.snapshot.sessions[*index].id.clone())
                    }),
                );
                if visible < group.sessions.len() {
                    keys.push(SelectionKey::ShowMore(group.key.clone()));
                }
            }
        }
        keys
    }

    pub fn visible_session_count(&self, group: &Group) -> usize {
        self.visible_limits
            .get(&group.key)
            .copied()
            .unwrap_or(self.session_page_size)
            .min(group.sessions.len())
    }

    pub fn session_page_size(&self) -> usize {
        self.session_page_size
    }

    pub fn set_session_page_size(&mut self, page_size: usize) {
        let page_size = page_size.clamp(1, SESSION_PAGE_SIZE);
        if self.session_page_size == page_size {
            return;
        }
        self.session_page_size = page_size;
        self.visible_limits.clear();
        self.reconcile_selection();
    }

    pub fn hidden_session_count(&self, group: &Group) -> usize {
        group
            .sessions
            .len()
            .saturating_sub(self.visible_session_count(group))
    }

    pub fn session_count(&self, state: SessionState) -> usize {
        self.session_counts.get(&state).copied().unwrap_or(0)
    }

    /// Advance the low-frequency live-session marker. The snapshot and group
    /// indexes remain untouched, so animation never rebuilds a large queue.
    pub fn advance_live_animation(&mut self) -> bool {
        if self.session_count(SessionState::Working) == 0 {
            let changed = !self.live_animation_visible;
            self.live_animation_visible = true;
            return changed;
        }
        self.live_animation_visible = !self.live_animation_visible;
        true
    }

    pub fn live_animation_visible(&self) -> bool {
        self.live_animation_visible
    }

    pub fn provider_labels(&self) -> &[String] {
        &self.provider_labels
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
        let index = *self.session_indices.get(id)?;
        self.snapshot
            .sessions
            .get(index)
            .filter(|session| &session.id == id)
    }

    pub fn select_and_reveal_session(&mut self, session_id: &str) -> bool {
        let Some(index) = self.session_indices.get(session_id).copied() else {
            return false;
        };
        let Some((group_key, position, group_len)) = self.groups().iter().find_map(|group| {
            group
                .sessions
                .iter()
                .position(|candidate| *candidate == index)
                .map(|position| (group.key.clone(), position, group.sessions.len()))
        }) else {
            return false;
        };
        self.collapsed.remove(&group_key);
        let required = position.saturating_add(1).min(group_len);
        let current = self
            .visible_limits
            .get(&group_key)
            .copied()
            .unwrap_or(self.session_page_size);
        if required > current {
            self.visible_limits.insert(group_key, required);
        }
        self.selection = Some(SelectionKey::Session(session_id.to_owned()));
        true
    }

    pub fn selected_group(&self) -> Option<Group> {
        let SelectionKey::Group(key) = self.selection.as_ref()? else {
            return None;
        };
        self.groups()
            .iter()
            .find(|group| &group.key == key)
            .cloned()
    }

    pub fn activate(&mut self) -> AppAction {
        match self.overlay.clone() {
            Overlay::Help => {
                self.overlay = Overlay::None;
                AppAction::None
            }
            Overlay::Peek if self.input.trim().is_empty() => self.request_open(),
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
            Overlay::HarnessPicker => {
                self.confirm_harness_selection();
                AppAction::None
            }
            Overlay::ModelPicker => {
                if self.models_error.is_some()
                    && self.models_auth_available
                    && !self.has_valid_custom_model_input()
                {
                    AppAction::Authenticate {
                        provider: self.launch_provider.clone(),
                    }
                } else {
                    self.confirm_model_selection()
                }
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
                Some(SelectionKey::Session(_)) => self.request_open(),
                Some(SelectionKey::ShowMore(key)) => {
                    self.show_more(&key);
                    AppAction::None
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
            Overlay::HarnessPicker => {
                self.overlay = Overlay::Composer(ComposerMode::NewSession);
                self.notice = None;
                AppAction::None
            }
            Overlay::ModelPicker => {
                self.overlay = Overlay::Composer(ComposerMode::NewSession);
                self.model_filter.clear();
                self.notice = None;
                AppAction::None
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
        self.visible_limits.clear();
        self.rebuild_group_cache();
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

    pub fn open_harness_picker(&mut self) {
        if self.launch_targets.is_empty() {
            self.set_notice("no launch-capable harness is configured");
            return;
        }
        self.harness_selection = self
            .launch_targets
            .iter()
            .position(|target| target.provider == self.launch_provider)
            .unwrap_or(0);
        self.notice = None;
        self.overlay = Overlay::HarnessPicker;
    }

    pub fn move_harness_selection(&mut self, delta: isize) {
        if self.launch_targets.is_empty() {
            return;
        }
        self.harness_selection = (self.harness_selection as isize + delta)
            .rem_euclid(self.launch_targets.len() as isize)
            as usize;
    }

    pub fn choose_harness_number(&mut self, number: usize) {
        let Some(index) = number.checked_sub(1) else {
            return;
        };
        if index < self.launch_targets.len() {
            self.harness_selection = index;
            self.confirm_harness_selection();
        }
    }

    pub fn confirm_harness_selection(&mut self) {
        let Some(target) = self.launch_targets.get(self.harness_selection) else {
            self.overlay = Overlay::Composer(ComposerMode::NewSession);
            return;
        };
        if self.launch_provider != target.provider {
            self.launch_provider = target.provider.clone();
            self.launch_model = None;
        }
        self.notice = None;
        self.overlay = Overlay::Composer(ComposerMode::NewSession);
    }

    pub fn launch_target(&self) -> Option<&LaunchTarget> {
        self.launch_targets
            .iter()
            .find(|target| target.provider == self.launch_provider)
    }

    pub fn open_model_picker(&mut self) -> AppAction {
        if !self
            .launch_target()
            .is_some_and(|target| target.supports_model)
        {
            self.set_notice(format!(
                "{} does not expose model selection",
                self.launch_provider.label()
            ));
            return AppAction::None;
        }
        if self.models_provider.as_ref() != Some(&self.launch_provider) {
            self.available_models.clear();
        }
        self.model_filter.clear();
        self.model_selection = self
            .model_choices()
            .iter()
            .position(|choice| *choice == self.launch_model.as_deref())
            .unwrap_or(0);
        self.models_loading = true;
        self.models_provider = Some(self.launch_provider.clone());
        self.models_error = None;
        self.models_auth_available = false;
        self.notice = None;
        self.overlay = Overlay::ModelPicker;
        AppAction::LoadModels {
            provider: self.launch_provider.clone(),
        }
    }

    pub fn set_available_models(
        &mut self,
        provider: Provider,
        result: Result<Vec<String>, String>,
    ) {
        if self.models_provider.as_ref() != Some(&provider) {
            return;
        }
        self.models_loading = false;
        match result {
            Ok(models) => {
                let mut seen = BTreeSet::new();
                self.available_models = models
                    .into_iter()
                    .filter(|model| valid_model_name(model))
                    .filter(|model| seen.insert(model.clone()))
                    .collect();
                self.model_selection = self
                    .model_choices()
                    .iter()
                    .position(|choice| *choice == self.launch_model.as_deref())
                    .unwrap_or(0);
                self.reconcile_model_selection();
                self.models_error = None;
                self.notice = None;
            }
            Err(error) => {
                self.available_models.clear();
                self.model_selection = 0;
                self.models_error = Some(error);
                // The picker renders a bounded, wrapped recovery message. A
                // duplicate one-line notice would be clipped and would hide
                // its contextual retry/setup keys.
                self.notice = None;
            }
        }
    }

    pub fn set_models_auth_available(&mut self, provider: &Provider, available: bool) {
        if self.models_provider.as_ref() == Some(provider) {
            self.models_auth_available = available;
        }
    }

    pub fn retry_model_load(&mut self, provider: &Provider) {
        if self.models_provider.as_ref() == Some(provider) {
            self.models_loading = true;
            self.models_error = None;
            self.available_models.clear();
            self.model_selection = 0;
        }
    }

    /// Preserve an attempted task and turn an authentication failure into an
    /// explicit modal action. A plain dashboard notice is not actionable:
    /// Enter would otherwise open whichever session row remained selected.
    pub fn require_authentication(
        &mut self,
        provider: Provider,
        model: Option<String>,
        prompt: String,
        error: String,
    ) {
        self.launch_provider = provider.clone();
        self.launch_model = model;
        self.input = prompt;
        self.model_filter.clear();
        self.available_models.clear();
        self.model_selection = 0;
        self.models_loading = false;
        self.models_provider = Some(provider);
        self.models_error = Some(error);
        self.models_auth_available = true;
        self.notice = None;
        self.overlay = Overlay::ModelPicker;
    }

    pub fn model_choices(&self) -> Vec<Option<&str>> {
        let needle = self.model_filter.to_ascii_lowercase();
        let mut choices = Vec::new();
        if self.models_error.is_none() && (needle.is_empty() || "default".contains(&needle)) {
            choices.push(None);
        }
        choices.extend(
            self.available_models
                .iter()
                .filter(|model| model.to_ascii_lowercase().contains(&needle))
                .map(|model| Some(model.as_str())),
        );
        choices
    }

    pub fn has_valid_custom_model_input(&self) -> bool {
        // Antigravity validates every --model value against the same catalog
        // that just failed. Treating filter text as an exact ID in that state
        // turns a recovery Enter into another guaranteed launch failure.
        (self.models_error.is_some() && self.launch_provider != Provider::Antigravity
            || self.launch_provider == Provider::QwenCode
                && self.models_error.is_none()
                && self.available_models.is_empty())
            && valid_model_name(&self.model_filter)
    }

    pub fn move_model_selection(&mut self, delta: isize) {
        let len = self.model_choices().len();
        if len == 0 {
            self.model_selection = 0;
            return;
        }
        self.model_selection =
            (self.model_selection as isize + delta).rem_euclid(len as isize) as usize;
    }

    pub fn move_model_page(&mut self, delta: isize) {
        let len = self.model_choices().len();
        if len == 0 {
            self.model_selection = 0;
            return;
        }
        self.model_selection = (self.model_selection as isize
            + delta * MODEL_PICKER_PAGE_SIZE as isize)
            .clamp(0, len.saturating_sub(1) as isize) as usize;
    }

    pub fn confirm_model_selection(&mut self) -> AppAction {
        if self.has_valid_custom_model_input() {
            self.launch_model = Some(self.model_filter.clone());
            self.model_filter.clear();
            self.notice = None;
            self.overlay = Overlay::Composer(ComposerMode::NewSession);
            return AppAction::None;
        }
        let selection = self
            .model_choices()
            .get(self.model_selection)
            .copied()
            .flatten()
            .map(ToOwned::to_owned);
        if self.model_choices().is_empty() {
            return AppAction::None;
        }
        if self.launch_provider == Provider::Terminal {
            if let Some(option) = selection
                .as_deref()
                .filter(|value| crate::adapters::is_shell_install_choice(value))
            {
                self.model_filter.clear();
                self.notice = None;
                return AppAction::SetupLaunchOption {
                    provider: Provider::Terminal,
                    option: option.to_owned(),
                };
            }
        }
        self.launch_model = selection;
        self.model_filter.clear();
        self.notice = None;
        self.overlay = Overlay::Composer(ComposerMode::NewSession);
        AppAction::None
    }

    pub fn set_completed_visibility(&mut self, include_completed: bool) {
        self.includes_completed = include_completed;
        self.visible_limits.clear();
        self.notice = Some(if include_completed {
            "showing completed sessions".into()
        } else {
            "completed sessions hidden".into()
        });
        self.reconcile_selection();
    }

    pub fn reveal_completed_after_launch(&mut self) {
        self.includes_completed = true;
        self.visible_limits.clear();
        self.notice =
            Some("task finished before the first refresh; showing completed sessions".into());
        self.reconcile_selection();
    }

    pub fn start_filter(&mut self) {
        self.notice = None;
        self.input = self.filter.clone();
        self.overlay = Overlay::Composer(ComposerMode::Filter);
    }

    /// Replace the session filter and rebuild the grouping cache once. The
    /// cache keeps subsequent draws and arrow-key navigation independent of
    /// the full provider-history size.
    pub fn set_filter(&mut self, filter: impl Into<String>) {
        self.filter = filter.into();
        self.visible_limits.clear();
        self.rebuild_group_cache();
        self.reconcile_selection();
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

    pub fn start_confirm(&mut self) -> AppAction {
        if let Some(session) = self.selected_session() {
            let running = is_active_session_state(session.state);
            let id = session.id.clone();
            let action = if session.capabilities.contains(&Capability::Interrupt) {
                AppAction::Interrupt { session_id: id }
            } else if !running && session.capabilities.contains(&Capability::Delete) {
                AppAction::Delete {
                    session_ids: vec![id],
                }
            } else if !running {
                AppAction::Hide {
                    session_ids: vec![id],
                }
            } else {
                self.overlay = Overlay::Confirm(ConfirmTarget::Hide {
                    session_ids: vec![id],
                });
                self.notice = None;
                return AppAction::None;
            };
            self.overlay = Overlay::None;
            self.notice = None;
            return action;
        } else if let Some(group) = self.selected_group() {
            if group
                .sessions
                .iter()
                .any(|index| is_active_session_state(self.snapshot.sessions[*index].state))
            {
                self.set_notice("bulk stop is unavailable; select one running session");
                return AppAction::None;
            }
            let undeletable = group
                .sessions
                .iter()
                .filter(|index| {
                    !self.snapshot.sessions[**index]
                        .capabilities
                        .contains(&Capability::Delete)
                })
                .map(|index| self.snapshot.sessions[*index].id.clone())
                .collect::<Vec<_>>();
            if !undeletable.is_empty() {
                self.overlay = Overlay::Confirm(ConfirmTarget::Hide {
                    session_ids: undeletable,
                });
                self.notice = None;
                return AppAction::None;
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
        AppAction::None
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
        if self.overlay == Overlay::ModelPicker {
            self.model_filter.push(character);
            self.reconcile_model_selection();
            return;
        }
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
        if self.overlay == Overlay::ModelPicker {
            self.model_filter.pop();
            self.reconcile_model_selection();
        } else {
            self.input.pop();
        }
    }

    /// Delete the previous shell-style word in the active text field.
    ///
    /// Terminals report Option+Backspace as Alt+Backspace on macOS. Ctrl+W is
    /// kept as the portable equivalent. Work on Unicode scalar boundaries so
    /// slicing can never split a UTF-8 code point.
    pub fn delete_previous_word(&mut self) {
        let input = if self.overlay == Overlay::ModelPicker {
            &mut self.model_filter
        } else {
            &mut self.input
        };
        let mut boundary = input.len();
        while let Some((index, character)) = input[..boundary].char_indices().next_back() {
            if !character.is_whitespace() {
                break;
            }
            boundary = index;
        }
        while let Some((index, character)) = input[..boundary].char_indices().next_back() {
            if character.is_whitespace() {
                break;
            }
            boundary = index;
        }
        input.truncate(boundary);
        if self.overlay == Overlay::ModelPicker {
            self.reconcile_model_selection();
        }
    }

    /// Delete from the cursor (which is currently always at the end) to the
    /// beginning of the current line. Cmd+Backspace and Ctrl+U use this path.
    pub fn delete_to_line_start(&mut self) {
        let input = if self.overlay == Overlay::ModelPicker {
            &mut self.model_filter
        } else {
            &mut self.input
        };
        let boundary = input.rfind('\n').map_or(0, |index| index + 1);
        input.truncate(boundary);
        if self.overlay == Overlay::ModelPicker {
            self.reconcile_model_selection();
        }
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
        matches_filter(session, &needle)
    }

    fn status_groups(&self) -> Vec<Group> {
        let needle = self.filter.to_ascii_lowercase();
        let mut grouped: BTreeMap<SessionState, Vec<usize>> = BTreeMap::new();
        for (index, session) in self.snapshot.sessions.iter().enumerate() {
            if needle.is_empty() || matches_filter(session, &needle) {
                grouped.entry(session.state).or_default().push(index);
            }
        }
        SessionState::DISPLAY_ORDER
            .iter()
            .filter_map(|state| {
                let sessions = grouped.remove(state).unwrap_or_default();
                (!sessions.is_empty()).then(|| Group {
                    key: format!("state:{state:?}"),
                    label: state.heading().into(),
                    sessions,
                })
            })
            .collect()
    }

    fn directory_groups(&self) -> Vec<Group> {
        let needle = self.filter.to_ascii_lowercase();
        let mut groups: BTreeMap<PathBuf, Vec<usize>> = BTreeMap::new();
        for (index, session) in self.snapshot.sessions.iter().enumerate() {
            if needle.is_empty() || matches_filter(session, &needle) {
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

    fn rebuild_snapshot_cache(&mut self) {
        self.session_counts.clear();
        let mut provider_labels = BTreeSet::new();
        for session in &self.snapshot.sessions {
            *self.session_counts.entry(session.state).or_default() += 1;
            provider_labels.insert(session.provider.label().to_owned());
        }
        self.provider_labels = provider_labels.into_iter().collect();
        self.session_indices = self
            .snapshot
            .sessions
            .iter()
            .enumerate()
            .map(|(index, session)| (session.id.clone(), index))
            .collect();
        self.rebuild_group_cache();
    }

    fn rebuild_group_cache(&mut self) {
        self.group_cache = match self.view_mode {
            ViewMode::Status => self.status_groups(),
            ViewMode::Directory => self.directory_groups(),
        };
        #[cfg(test)]
        {
            self.group_cache_rebuilds += 1;
        }
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

fn matches_filter(session: &AgentSession, needle: &str) -> bool {
    session.name.to_ascii_lowercase().contains(needle)
        || session.summary.to_ascii_lowercase().contains(needle)
        || session
            .cwd
            .to_string_lossy()
            .to_ascii_lowercase()
            .contains(needle)
        || session
            .provider
            .label()
            .to_ascii_lowercase()
            .contains(needle)
}

impl App {
    fn submit_composer(&mut self, mode: ComposerMode) -> AppAction {
        let input = self.input.trim().to_owned();
        if input.is_empty() && !matches!(mode, ComposerMode::Filter | ComposerMode::Rename { .. }) {
            return AppAction::None;
        }
        self.input.clear();
        self.overlay = Overlay::None;
        match mode {
            ComposerMode::NewSession => self.submit_new_session(input),
            ComposerMode::Rename { session_id } => AppAction::Rename {
                session_id,
                name: input,
            },
            ComposerMode::Filter => {
                self.set_filter(input);
                AppAction::None
            }
        }
    }

    fn confirm(&mut self, target: ConfirmTarget) -> AppAction {
        self.overlay = Overlay::None;
        match target {
            ConfirmTarget::Archive { id } => AppAction::Archive { session_id: id },
            ConfirmTarget::Hide { session_ids } => AppAction::Hide { session_ids },
            ConfirmTarget::Group { session_ids, .. } => AppAction::Delete { session_ids },
        }
    }

    fn request_open(&mut self) -> AppAction {
        let Some(session) = self.selected_session() else {
            return AppAction::None;
        };
        AppAction::Open {
            session_id: session.id.clone(),
        }
    }

    fn submit_new_session(&mut self, input: String) -> AppAction {
        if !input.starts_with('/') {
            if self.launch_provider == Provider::Antigravity && self.launch_model.is_none() {
                // Antigravity 1.1.x can terminate a task when its account does
                // not advertise a default PlanModel/RequestedModel. Never send
                // the user's task until an exact model has been selected.
                self.input = input;
                return self.open_model_picker();
            }
            return AppAction::Launch {
                provider: self.launch_provider.clone(),
                model: self.launch_model.clone(),
                prompt: input,
            };
        }
        let (command, argument) = input
            .split_once(char::is_whitespace)
            .map(|(command, argument)| (command, argument.trim()))
            .unwrap_or((input.as_str(), ""));
        match command.to_ascii_lowercase().as_str() {
            "/help" => self.toggle_help(),
            "/harness" | "/provider" if argument.is_empty() => self.open_harness_picker(),
            "/harness" | "/provider" => self.select_launch_provider(argument),
            "/model" => return self.select_launch_model(argument),
            "/shell" if self.launch_provider == Provider::Terminal => {
                return self.select_launch_model(argument)
            }
            "/shell" => self.set_notice("/shell is available after selecting Terminal"),
            "/login" if argument.is_empty() => {
                return AppAction::Authenticate {
                    provider: self.launch_provider.clone(),
                }
            }
            "/login" => self.set_notice("use /login after selecting a harness with /harness"),
            "/setup" if argument.is_empty() => {
                return AppAction::SetupProvider {
                    provider: self.launch_provider.clone(),
                }
            }
            "/setup" => match known_provider(argument) {
                Some(provider) => return AppAction::SetupProvider { provider },
                None => self.set_notice(format!(
                    "unknown harness {argument}; use /help for supported setup names"
                )),
            },
            "/completed" => return self.select_completed_visibility(argument),
            "/filter" => {
                self.set_filter(argument);
                self.set_notice(if argument.is_empty() {
                    "session filter cleared".into()
                } else {
                    format!("session filter: {argument}")
                });
            }
            _ => self.set_notice(format!("unknown dashboard command {command}; use /help")),
        }
        AppAction::None
    }

    fn select_launch_provider(&mut self, argument: &str) {
        if argument.is_empty() {
            let choices = self
                .launch_targets
                .iter()
                .map(|target| target.provider.label())
                .collect::<Vec<_>>()
                .join(", ");
            self.set_notice(if choices.is_empty() {
                "no launch-capable harness is configured".into()
            } else {
                format!("available harnesses: {choices}")
            });
            return;
        }
        let normalized = normalize_provider_name(argument);
        let selected = self.launch_targets.iter().find(|target| {
            normalize_provider_name(target.provider.label()) == normalized
                || provider_alias(&target.provider) == normalized
        });
        if let Some(target) = selected {
            self.launch_provider = target.provider.clone();
            self.harness_selection = self
                .launch_targets
                .iter()
                .position(|candidate| candidate.provider == target.provider)
                .unwrap_or(0);
            self.launch_model = None;
            self.set_notice(format!(
                "new tasks will use the {} harness with its default model",
                target.provider.label()
            ));
        } else {
            self.set_notice(format!(
                "{argument} is not an available harness; use /harness"
            ));
        }
    }

    fn select_launch_model(&mut self, argument: &str) -> AppAction {
        if !self
            .launch_target()
            .is_some_and(|target| target.supports_model)
        {
            self.set_notice(format!(
                "{} does not expose model selection",
                self.launch_provider.label()
            ));
            return AppAction::None;
        }
        if argument.is_empty() {
            return self.open_model_picker();
        }
        if argument.eq_ignore_ascii_case("default") {
            self.launch_model = None;
            self.set_notice(format!(
                "{} will use its default {}",
                self.launch_provider.label(),
                if self.launch_provider == Provider::Terminal {
                    "shell"
                } else {
                    "model"
                }
            ));
            return AppAction::None;
        }
        if argument.len() > 128
            || argument
                .chars()
                .any(|character| character.is_control() || character.is_whitespace())
        {
            self.set_notice(if self.launch_provider == Provider::Terminal {
                "shell names must be 1–128 characters without whitespace"
            } else {
                "model names must be 1–128 characters without whitespace"
            });
            return AppAction::None;
        }
        self.launch_model = Some(argument.to_owned());
        self.set_notice(format!(
            "new {} tasks will use {} {argument}",
            self.launch_provider.label(),
            if self.launch_provider == Provider::Terminal {
                "shell"
            } else {
                "model"
            }
        ));
        AppAction::None
    }

    fn select_completed_visibility(&mut self, argument: &str) -> AppAction {
        let include_completed = match argument.to_ascii_lowercase().as_str() {
            "" | "toggle" => !self.includes_completed,
            "show" | "on" | "all" => true,
            "hide" | "off" => false,
            _ => {
                self.set_notice("use /completed, /completed show, or /completed hide");
                return AppAction::None;
            }
        };
        AppAction::SetCompletedVisibility { include_completed }
    }

    fn reconcile_model_selection(&mut self) {
        let len = self.model_choices().len();
        self.model_selection = self.model_selection.min(len.saturating_sub(1));
    }

    fn show_more(&mut self, key: &str) {
        let Some(group) = self.groups().iter().find(|group| group.key == key).cloned() else {
            self.reconcile_selection();
            return;
        };
        let previously_visible = self.visible_session_count(&group);
        let newly_visible = previously_visible
            .saturating_add(self.session_page_size)
            .min(group.sessions.len());
        let first_revealed = group
            .sessions
            .get(previously_visible)
            .map(|index| self.snapshot.sessions[*index].id.clone());

        self.visible_limits.insert(key.to_owned(), newly_visible);
        self.selection = first_revealed.map(SelectionKey::Session);
        self.notice = None;
        self.reconcile_selection();
    }
}

pub(crate) fn is_active_session_state(state: SessionState) -> bool {
    state != SessionState::Completed
}

fn normalize_provider_name(value: &str) -> String {
    value
        .chars()
        .filter(|character| character.is_ascii_alphanumeric())
        .flat_map(char::to_lowercase)
        .collect()
}

fn provider_alias(provider: &Provider) -> String {
    match provider {
        Provider::GitHubCopilot => "copilot".into(),
        Provider::OpenCode => "opencode".into(),
        Provider::MistralVibe => "vibe".into(),
        Provider::MuseCode => "muse".into(),
        Provider::QwenCode => "qwen".into(),
        Provider::KimiCode => "kimi".into(),
        _ => normalize_provider_name(provider.label()),
    }
}

fn known_provider(value: &str) -> Option<Provider> {
    match normalize_provider_name(value).as_str() {
        "claude" | "claudecode" => Some(Provider::Claude),
        "codex" => Some(Provider::Codex),
        "pi" => Some(Provider::Pi),
        "opencode" => Some(Provider::OpenCode),
        "cursor" | "cursoragent" => Some(Provider::Cursor),
        "copilot" | "githubcopilot" => Some(Provider::GitHubCopilot),
        "antigravity" | "agy" => Some(Provider::Antigravity),
        "mistral" | "mistralvibe" | "vibe" => Some(Provider::MistralVibe),
        "muse" | "musecode" => Some(Provider::MuseCode),
        "qwen" | "qwencode" => Some(Provider::QwenCode),
        "kimi" | "kimicode" => Some(Provider::KimiCode),
        "terminal" | "shell" => Some(Provider::Terminal),
        _ => None,
    }
}

fn valid_model_name(model: &str) -> bool {
    !model.is_empty()
        && model.len() <= 128
        && !model
            .chars()
            .any(|character| character.is_control() || character.is_whitespace())
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
    fn large_groups_reveal_one_bounded_page_at_a_time() {
        let sessions = (0..61)
            .map(|index| session(&format!("session-{index:02}"), SessionState::Working))
            .collect();
        let mut app = app_with(sessions);
        let group = app.groups()[0].clone();

        assert_eq!(app.visible_session_count(&group), SESSION_PAGE_SIZE);
        assert_eq!(app.hidden_session_count(&group), 36);
        assert_eq!(app.selectable_keys().len(), 27);
        assert_eq!(
            app.selectable_keys().last(),
            Some(&SelectionKey::ShowMore("state:Working".into()))
        );

        app.selection = Some(SelectionKey::ShowMore("state:Working".into()));
        assert_eq!(app.activate(), AppAction::None);
        assert_eq!(
            app.selection,
            Some(SelectionKey::Session("session-25".into()))
        );
        assert_eq!(app.visible_session_count(&group), 50);
        assert_eq!(app.hidden_session_count(&group), 11);

        app.replace_snapshot(app.snapshot.clone());
        assert_eq!(app.visible_session_count(&group), 50);
        assert_eq!(
            app.selection,
            Some(SelectionKey::Session("session-25".into()))
        );

        app.selection = Some(SelectionKey::ShowMore("state:Working".into()));
        assert_eq!(app.activate(), AppAction::None);
        assert_eq!(
            app.selection,
            Some(SelectionKey::Session("session-50".into()))
        );
        assert_eq!(app.visible_session_count(&group), 61);
        assert_eq!(app.hidden_session_count(&group), 0);
        assert!(!app
            .selectable_keys()
            .iter()
            .any(|key| matches!(key, SelectionKey::ShowMore(_))));
    }

    #[test]
    fn post_launch_selection_expands_its_group_and_reveals_the_exact_row() {
        let sessions = (0..40)
            .map(|index| session(&format!("session-{index:02}"), SessionState::Working))
            .collect();
        let mut app = App::new(SessionSnapshot {
            sessions,
            warnings: Vec::new(),
        });
        app.set_session_page_size(5);
        let group_key = app.groups()[0].key.clone();
        app.collapsed.insert(group_key.clone());

        assert!(app.select_and_reveal_session("session-30"));

        assert_eq!(
            app.selection,
            Some(SelectionKey::Session("session-30".into()))
        );
        assert!(!app.collapsed.contains(&group_key));
        assert!(app
            .selectable_keys()
            .contains(&SelectionKey::Session("session-30".into())));
        assert!(!app.select_and_reveal_session("missing"));
    }

    #[test]
    fn visible_page_size_tracks_the_viewport_and_remains_bounded() {
        let sessions = (0..61)
            .map(|index| session(&format!("session-{index:02}"), SessionState::Working))
            .collect();
        let mut app = app_with(sessions);

        app.set_session_page_size(10);
        assert_eq!(app.session_page_size(), 10);
        assert_eq!(app.visible_session_count(&app.groups()[0]), 10);

        app.selection = Some(SelectionKey::ShowMore("state:Working".into()));
        assert_eq!(app.activate(), AppAction::None);
        assert_eq!(app.visible_session_count(&app.groups()[0]), 20);

        app.set_session_page_size(0);
        assert_eq!(app.session_page_size(), 1);
        assert_eq!(app.visible_session_count(&app.groups()[0]), 1);

        app.set_session_page_size(usize::MAX);
        assert_eq!(app.session_page_size(), SESSION_PAGE_SIZE);
        assert_eq!(
            app.visible_session_count(&app.groups()[0]),
            SESSION_PAGE_SIZE
        );
    }

    #[test]
    fn seventy_thousand_rows_are_grouped_once_across_navigation_and_draw_queries() {
        let sessions = (0..70_000)
            .map(|index| session(&format!("session-{index:05}"), SessionState::Completed))
            .collect();
        let mut app = app_with(sessions);
        assert!(app.includes_completed);
        let rebuilds = app.group_cache_rebuilds;

        for _ in 0..2_000 {
            app.select_next();
            let _ = app.groups();
            let _ = app.selected_session();
        }

        assert_eq!(app.group_cache_rebuilds, rebuilds);
        assert_eq!(app.groups()[0].sessions.len(), 70_000);
        assert!(app.selectable_keys().len() <= SESSION_PAGE_SIZE + 2);

        app.set_filter("session-69999");
        assert_eq!(app.group_cache_rebuilds, rebuilds + 1);
        assert_eq!(app.groups()[0].sessions, vec![69_999]);
    }

    #[test]
    fn live_animation_never_rebuilds_session_indexes() {
        let mut app = app_with(vec![
            session("live", SessionState::Working),
            session("done", SessionState::Completed),
        ]);
        let rebuilds = app.group_cache_rebuilds;
        assert!(app.live_animation_visible());

        assert!(app.advance_live_animation());
        assert!(!app.live_animation_visible());
        assert!(app.advance_live_animation());
        assert!(app.live_animation_visible());
        assert_eq!(app.group_cache_rebuilds, rebuilds);
    }

    #[test]
    fn filtering_searches_hidden_sessions_and_resets_paging() {
        let sessions = (0..60)
            .map(|index| session(&format!("session-{index:02}"), SessionState::Working))
            .collect();
        let mut app = app_with(sessions);
        app.selection = Some(SelectionKey::ShowMore("state:Working".into()));
        app.activate();
        assert_eq!(app.visible_session_count(&app.groups()[0]), 50);

        app.start_filter();
        app.input = "session-59".into();
        assert_eq!(app.activate(), AppAction::None);

        let filtered = app.groups()[0].clone();
        assert_eq!(filtered.sessions, vec![59]);
        assert_eq!(app.visible_session_count(&filtered), 1);
        assert_eq!(
            app.selection,
            Some(SelectionKey::Session("session-59".into()))
        );

        app.start_filter();
        app.input.clear();
        app.activate();
        assert_eq!(
            app.visible_session_count(&app.groups()[0]),
            SESSION_PAGE_SIZE
        );
    }

    #[test]
    fn filter_matches_summary_case_insensitively() {
        let mut app = App::new(SessionSnapshot {
            sessions: vec![session("one", SessionState::Working)],
            warnings: vec![],
        });
        app.set_filter("SUMMARY ONE");

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
    fn needs_input_is_treated_as_running_for_lifecycle_control() {
        let mut item = session("one", SessionState::NeedsInput);
        item.capabilities.insert(Capability::Interrupt);
        let mut app = App::new(SessionSnapshot {
            sessions: vec![item],
            warnings: vec![],
        });

        assert_eq!(
            app.start_confirm(),
            AppAction::Interrupt {
                session_id: "one".into()
            }
        );
        assert_eq!(app.overlay, Overlay::None);
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

        app.set_filter("summary working");
        assert_eq!(app.groups().len(), 1);
        assert_eq!(app.groups()[0].sessions, vec![1]);
    }

    #[test]
    fn filter_matches_name_cwd_and_provider_and_rejects_unmatched_rows() {
        let mut item = session("agent-name", SessionState::Working);
        item.cwd = PathBuf::from("/projects/special-root");
        let mut app = app_with(vec![item]);

        for needle in ["AGENT-NAME", "SPECIAL-ROOT", "CLAUDE"] {
            app.set_filter(needle);
            assert_eq!(app.groups().len(), 1, "filter {needle}");
        }
        app.set_filter("missing");
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
    fn enter_opens_every_provider_natively_even_when_inline_actions_exist() {
        for provider in [
            Provider::Claude,
            Provider::Codex,
            Provider::Pi,
            Provider::OpenCode,
            Provider::Cursor,
            Provider::GitHubCopilot,
            Provider::Antigravity,
        ] {
            for inline_capability in [
                Capability::Reply,
                Capability::Approve,
                Capability::Decline,
                Capability::Respond,
            ] {
                let mut item = session("managed", SessionState::Working);
                item.provider = provider.clone();
                item.capabilities = BTreeSet::from([Capability::Inspect, inline_capability]);
                let mut app = app_with(vec![item]);

                assert_eq!(
                    app.activate(),
                    AppAction::Open {
                        session_id: "managed".into()
                    }
                );
                assert_eq!(app.overlay, Overlay::None);
            }
        }

        let mut read_only = session("external", SessionState::Completed);
        read_only.provider = Provider::Pi;
        read_only.capabilities = BTreeSet::from([Capability::Inspect]);
        let mut app = app_with(vec![read_only]);
        assert_eq!(
            app.activate(),
            AppAction::Open {
                session_id: "external".into()
            }
        );
        assert_eq!(app.overlay, Overlay::None);
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
                provider: Provider::Claude,
                model: None,
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

        app.overlay = Overlay::Composer(ComposerMode::Rename {
            session_id: "one".into(),
        });
        app.input = "   ".into();
        assert_eq!(
            app.activate(),
            AppAction::Rename {
                session_id: "one".into(),
                name: String::new()
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
    fn active_and_idle_session_confirmations_mutate_only_with_authority_or_hide_locally() {
        let mut active = session("active", SessionState::ReadyForReview);
        active.capabilities.clear();
        let mut app = app_with(vec![active]);
        assert_eq!(app.start_confirm(), AppAction::None);
        assert_eq!(
            app.activate(),
            AppAction::Hide {
                session_ids: vec!["active".into()]
            }
        );

        grant(&mut app.snapshot.sessions[0], &[Capability::Interrupt]);
        assert_eq!(
            app.start_confirm(),
            AppAction::Interrupt {
                session_id: "active".into()
            }
        );

        app.snapshot.sessions[0].state = SessionState::Completed;
        app.snapshot.sessions[0].capabilities.clear();
        assert_eq!(
            app.start_confirm(),
            AppAction::Hide {
                session_ids: vec!["active".into()]
            }
        );
        grant(&mut app.snapshot.sessions[0], &[Capability::Delete]);
        assert_eq!(
            app.start_confirm(),
            AppAction::Delete {
                session_ids: vec!["active".into()]
            }
        );
    }

    #[test]
    fn completed_group_never_mixes_provider_deletion_with_local_hiding() {
        let mut one = session("one", SessionState::Completed);
        let two = session("two", SessionState::Completed);
        grant(&mut one, &[Capability::Delete]);
        let mut app = app_with(vec![one, two]);
        app.selection = Some(SelectionKey::Group("state:Completed".into()));

        app.start_confirm();
        assert_eq!(
            app.activate(),
            AppAction::Hide {
                session_ids: vec!["two".into()]
            }
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

    #[test]
    fn task_commands_select_provider_model_and_filter_without_launching() {
        let mut app = App::with_launch_targets(
            SessionSnapshot::default(),
            false,
            Provider::Claude,
            vec![
                LaunchTarget {
                    provider: Provider::Claude,
                    supports_model: true,
                },
                LaunchTarget {
                    provider: Provider::Pi,
                    supports_model: false,
                },
            ],
        );

        app.start_new_session(None);
        app.input = "/harness".into();
        assert_eq!(app.activate(), AppAction::None);
        assert_eq!(app.overlay, Overlay::HarnessPicker);
        assert_eq!(app.escape(), AppAction::None);
        assert_eq!(app.overlay, Overlay::Composer(ComposerMode::NewSession));

        app.start_new_session(None);
        app.input = "/harness pi".into();
        assert_eq!(app.activate(), AppAction::None);
        assert_eq!(app.launch_provider, Provider::Pi);

        app.start_new_session(None);
        app.input = "/model opus".into();
        assert_eq!(app.activate(), AppAction::None);
        assert!(app.launch_model.is_none());
        assert!(app.notice.as_deref().unwrap().contains("does not expose"));

        app.start_new_session(None);
        app.input = "/provider claude".into();
        app.activate();
        app.start_new_session(None);
        app.input = "/model opus".into();
        app.activate();
        assert_eq!(app.launch_model.as_deref(), Some("opus"));

        app.start_new_session(None);
        app.input = "ship it".into();
        assert_eq!(
            app.activate(),
            AppAction::Launch {
                provider: Provider::Claude,
                model: Some("opus".into()),
                prompt: "ship it".into(),
            }
        );

        app.start_new_session(None);
        app.input = "/filter codex".into();
        assert_eq!(app.activate(), AppAction::None);
        assert_eq!(app.filter, "codex");

        app.start_new_session(None);
        app.input = "/help".into();
        assert_eq!(app.activate(), AppAction::None);
        assert_eq!(app.overlay, Overlay::Help);
        assert_eq!(app.notice, None);

        app.start_new_session(None);
        app.input = "/setup copilot".into();
        assert_eq!(
            app.activate(),
            AppAction::SetupProvider {
                provider: Provider::GitHubCopilot
            }
        );

        app.start_new_session(None);
        app.input = "/setup shell".into();
        assert_eq!(
            app.activate(),
            AppAction::SetupProvider {
                provider: Provider::Terminal
            }
        );

        for (name, provider) in [
            ("vibe", Provider::MistralVibe),
            ("muse", Provider::MuseCode),
            ("qwen", Provider::QwenCode),
            ("kimi", Provider::KimiCode),
        ] {
            app.start_new_session(None);
            app.input = format!("/setup {name}");
            assert_eq!(
                app.activate(),
                AppAction::SetupProvider { provider },
                "setup alias {name} did not resolve"
            );
        }
    }

    #[test]
    fn terminal_shell_command_aliases_model_and_install_rows_are_explicit_actions() {
        let mut app = App::with_launch_targets(
            SessionSnapshot::default(),
            false,
            Provider::Terminal,
            vec![LaunchTarget {
                provider: Provider::Terminal,
                supports_model: true,
            }],
        );

        app.start_new_session(None);
        app.input = "/shell".into();
        assert_eq!(
            app.activate(),
            AppAction::LoadModels {
                provider: Provider::Terminal
            }
        );
        app.set_available_models(
            Provider::Terminal,
            Ok(vec!["bash".into(), "install-shell:fish".into()]),
        );
        app.model_selection = 2;
        assert_eq!(
            app.activate(),
            AppAction::SetupLaunchOption {
                provider: Provider::Terminal,
                option: "install-shell:fish".into(),
            }
        );
        assert_eq!(app.overlay, Overlay::ModelPicker);
        assert_eq!(app.launch_model, None);

        app.model_selection = 1;
        assert_eq!(app.activate(), AppAction::None);
        assert_eq!(app.launch_model.as_deref(), Some("bash"));

        app.start_new_session(None);
        app.input = "/model zsh".into();
        assert_eq!(app.activate(), AppAction::None);
        assert_eq!(app.launch_model.as_deref(), Some("zsh"));
    }

    #[test]
    fn shell_command_is_scoped_to_terminal() {
        let mut app = App::new(SessionSnapshot::default());
        app.start_new_session(None);
        app.input = "/shell".into();
        assert_eq!(app.activate(), AppAction::None);
        assert_eq!(
            app.notice.as_deref(),
            Some("/shell is available after selecting Terminal")
        );
    }

    #[test]
    fn harness_picker_previews_wraps_confirms_and_preserves_the_draft() {
        let mut app = App::with_launch_targets(
            SessionSnapshot::default(),
            false,
            Provider::Claude,
            vec![
                LaunchTarget {
                    provider: Provider::Claude,
                    supports_model: true,
                },
                LaunchTarget {
                    provider: Provider::Codex,
                    supports_model: true,
                },
                LaunchTarget {
                    provider: Provider::Pi,
                    supports_model: false,
                },
            ],
        );
        app.start_new_session(None);
        app.input = "keep this draft".into();
        app.launch_model = Some("opus".into());

        app.open_harness_picker();
        assert_eq!(app.overlay, Overlay::HarnessPicker);
        app.move_harness_selection(-1);
        assert_eq!(app.harness_selection, 2);
        assert_eq!(app.launch_provider, Provider::Claude);
        assert_eq!(app.escape(), AppAction::None);
        assert_eq!(app.overlay, Overlay::Composer(ComposerMode::NewSession));
        assert_eq!(app.input, "keep this draft");
        assert_eq!(app.launch_model.as_deref(), Some("opus"));

        app.open_harness_picker();
        app.move_harness_selection(1);
        assert_eq!(app.activate(), AppAction::None);
        assert_eq!(app.overlay, Overlay::Composer(ComposerMode::NewSession));
        assert_eq!(app.launch_provider, Provider::Codex);
        assert!(app.launch_model.is_none());
        assert_eq!(app.input, "keep this draft");

        app.open_harness_picker();
        app.choose_harness_number(3);
        assert_eq!(app.launch_provider, Provider::Pi);
        assert_eq!(app.overlay, Overlay::Composer(ComposerMode::NewSession));
    }

    #[test]
    fn provider_refresh_does_not_close_the_harness_picker_or_drop_its_draft() {
        let mut app = App::with_launch_targets(
            SessionSnapshot::default(),
            false,
            Provider::Claude,
            vec![
                LaunchTarget {
                    provider: Provider::Claude,
                    supports_model: true,
                },
                LaunchTarget {
                    provider: Provider::Pi,
                    supports_model: false,
                },
            ],
        );
        app.start_new_session(None);
        app.input = "draft during discovery".into();
        app.open_harness_picker();

        app.replace_snapshot(SessionSnapshot {
            sessions: vec![session("arrived", SessionState::Working)],
            warnings: vec![],
        });

        assert_eq!(app.overlay, Overlay::HarnessPicker);
        assert_eq!(app.input, "draft during discovery");
        assert_eq!(app.selection, Some(SelectionKey::Session("arrived".into())));
    }

    #[test]
    fn task_command_validation_is_local_and_harness_switch_resets_model() {
        let mut app = App::with_launch_targets(
            SessionSnapshot::default(),
            false,
            Provider::Claude,
            vec![
                LaunchTarget {
                    provider: Provider::Claude,
                    supports_model: true,
                },
                LaunchTarget {
                    provider: Provider::Codex,
                    supports_model: true,
                },
            ],
        );
        app.launch_model = Some("opus".into());
        app.start_new_session(None);
        app.open_harness_picker();
        app.move_harness_selection(1);
        app.activate();
        assert_eq!(app.launch_provider, Provider::Codex);
        assert!(app.launch_model.is_none());

        app.start_new_session(None);
        app.input = "/model invalid model".into();
        app.activate();
        assert!(app.launch_model.is_none());
        assert!(app
            .notice
            .as_deref()
            .unwrap()
            .contains("without whitespace"));

        app.start_new_session(None);
        app.input = "/unknown".into();
        app.activate();
        assert!(app
            .notice
            .as_deref()
            .unwrap()
            .contains("unknown dashboard command"));
    }

    #[test]
    fn completed_command_requests_toggle_or_explicit_visibility_without_guessing() {
        let mut app = App::with_completed_visibility(SessionSnapshot::default(), false);

        app.start_new_session(None);
        app.input = "/completed".into();
        assert_eq!(
            app.activate(),
            AppAction::SetCompletedVisibility {
                include_completed: true
            }
        );
        assert!(!app.includes_completed);

        app.set_completed_visibility(true);
        assert!(app.includes_completed);
        assert_eq!(app.notice.as_deref(), Some("showing completed sessions"));

        app.start_new_session(None);
        app.input = "/completed hide".into();
        assert_eq!(
            app.activate(),
            AppAction::SetCompletedVisibility {
                include_completed: false
            }
        );

        app.start_new_session(None);
        app.input = "/completed sometimes".into();
        assert_eq!(app.activate(), AppAction::None);
        assert!(app.notice.as_deref().unwrap().contains("/completed show"));
    }

    #[test]
    fn model_picker_loads_filters_pages_and_preserves_the_task_draft() {
        let mut app = App::with_launch_targets(
            SessionSnapshot::default(),
            false,
            Provider::Pi,
            vec![LaunchTarget {
                provider: Provider::Pi,
                supports_model: true,
            }],
        );
        app.start_new_session(None);
        app.input = "keep this task draft".into();

        assert_eq!(
            app.open_model_picker(),
            AppAction::LoadModels {
                provider: Provider::Pi
            }
        );
        assert_eq!(app.overlay, Overlay::ModelPicker);
        assert_eq!(app.input, "keep this task draft");
        assert!(app.models_loading);
        assert_eq!(app.model_choices(), vec![None]);

        app.set_available_models(
            Provider::Pi,
            Ok(vec![
                "openai/gpt-5".into(),
                "anthropic/claude-sonnet".into(),
                "openai/gpt-5".into(),
                "invalid model".into(),
            ]),
        );
        assert!(!app.models_loading);
        assert_eq!(
            app.model_choices(),
            vec![None, Some("openai/gpt-5"), Some("anthropic/claude-sonnet")]
        );

        app.push_input('g');
        app.push_input('p');
        app.push_input('t');
        assert_eq!(app.model_choices(), vec![Some("openai/gpt-5")]);
        assert_eq!(app.model_selection, 0);
        app.confirm_model_selection();
        assert_eq!(app.launch_model.as_deref(), Some("openai/gpt-5"));
        assert_eq!(app.overlay, Overlay::Composer(ComposerMode::NewSession));
        assert_eq!(app.input, "keep this task draft");

        app.open_model_picker();
        assert_eq!(app.model_selection, 1);
        app.move_model_selection(-1);
        assert_eq!(app.model_selection, 0);
        app.move_model_selection(-1);
        assert_eq!(app.model_selection, 2);
        app.move_model_page(-1);
        assert_eq!(app.model_selection, 0);
        assert_eq!(app.escape(), AppAction::None);
        assert_eq!(app.overlay, Overlay::Composer(ComposerMode::NewSession));
        assert_eq!(app.input, "keep this task draft");
    }

    #[test]
    fn model_picker_ignores_stale_provider_results_and_surfaces_current_errors() {
        let mut app = App::new(SessionSnapshot::default());
        app.start_new_session(None);
        app.open_model_picker();

        app.set_available_models(Provider::Codex, Ok(vec!["stale".into()]));
        assert!(app.available_models.is_empty());
        assert!(app.models_loading);

        app.set_available_models(Provider::Claude, Err("command timed out".into()));
        assert!(!app.models_loading);
        assert!(app.model_choices().is_empty());
        assert_eq!(app.notice, None);
        assert_eq!(app.models_error.as_deref(), Some("command timed out"));

        app.set_models_auth_available(&Provider::Claude, true);
        assert_eq!(
            app.activate(),
            AppAction::Authenticate {
                provider: Provider::Claude
            }
        );
        app.retry_model_load(&Provider::Claude);
        assert!(app.models_loading);
        assert!(app.models_error.is_none());
    }

    #[test]
    fn launch_authentication_failure_preserves_the_task_and_makes_enter_actionable() {
        let mut app = App::new(SessionSnapshot::default());
        app.require_authentication(
            Provider::GitHubCopilot,
            Some("gpt-5.4".into()),
            "finish the release".into(),
            "GitHub Copilot is not authenticated".into(),
        );

        assert_eq!(app.overlay, Overlay::ModelPicker);
        assert_eq!(app.input, "finish the release");
        assert_eq!(app.launch_model.as_deref(), Some("gpt-5.4"));
        assert_eq!(
            app.activate(),
            AppAction::Authenticate {
                provider: Provider::GitHubCopilot
            }
        );
        assert_eq!(app.input, "finish the release");
    }

    #[test]
    fn exact_model_id_is_an_explicit_fallback_when_catalog_loading_fails() {
        let mut app = App::new(SessionSnapshot::default());
        app.require_authentication(
            Provider::Claude,
            None,
            "investigate the failure".into(),
            "model catalog timed out".into(),
        );
        for character in "gemini-3-pro".chars() {
            app.push_input(character);
        }
        assert!(app.has_valid_custom_model_input());
        assert_eq!(app.activate(), AppAction::None);
        assert_eq!(app.launch_model.as_deref(), Some("gemini-3-pro"));
        assert_eq!(app.input, "investigate the failure");
        assert_eq!(app.overlay, Overlay::Composer(ComposerMode::NewSession));
    }

    #[test]
    fn exact_model_id_is_available_when_provider_has_no_machine_readable_catalog() {
        let mut app = App::with_launch_targets(
            SessionSnapshot::default(),
            false,
            Provider::QwenCode,
            vec![LaunchTarget {
                provider: Provider::QwenCode,
                supports_model: true,
            }],
        );
        app.start_new_session(None);
        app.input = "keep the native task".into();
        assert_eq!(
            app.open_model_picker(),
            AppAction::LoadModels {
                provider: Provider::QwenCode
            }
        );
        app.set_available_models(Provider::QwenCode, Ok(Vec::new()));
        for character in "qwen3-coder-plus".chars() {
            app.push_input(character);
        }

        assert!(app.has_valid_custom_model_input());
        app.confirm_model_selection();
        assert_eq!(app.launch_model.as_deref(), Some("qwen3-coder-plus"));
        assert_eq!(app.input, "keep the native task");
    }

    #[test]
    fn antigravity_catalog_failure_keeps_enter_on_native_recovery() {
        let mut app = App::new(SessionSnapshot::default());
        app.require_authentication(
            Provider::Antigravity,
            None,
            "investigate the failure".into(),
            "agy models timed out".into(),
        );
        for character in "gemini".chars() {
            app.push_input(character);
        }

        assert!(!app.has_valid_custom_model_input());
        assert_eq!(
            app.activate(),
            AppAction::Authenticate {
                provider: Provider::Antigravity
            }
        );
        assert_eq!(app.input, "investigate the failure");
        assert_eq!(app.launch_model, None);
    }
}

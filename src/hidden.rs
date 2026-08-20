//! Local visibility overrides for provider records that Open Agent View does
//! not own and therefore cannot safely delete.
//!
//! Hiding is deliberately separate from provider lifecycle control: it stores
//! only a stable normalized session ID and filters matching rows from future
//! snapshots. Provider history and live processes are never mutated.

use std::collections::BTreeMap;
use std::fs::{self, File, OpenOptions};
use std::io::Write;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{SystemTime, UNIX_EPOCH};

use anyhow::{bail, Context, Result};
use serde::{Deserialize, Serialize};

use crate::domain::{AgentSession, Provider, SessionSnapshot};

const REGISTRY_VERSION: u32 = 1;
const MAX_SESSION_ID_BYTES: usize = 4096;
static TEMPORARY_SEQUENCE: AtomicU64 = AtomicU64::new(1);

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct HiddenSessionRecord {
    pub id: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub provider: Option<Provider>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub name: Option<String>,
    #[serde(default)]
    pub hidden_at_ms: u64,
}

impl HiddenSessionRecord {
    fn from_session(session: &AgentSession) -> Self {
        Self {
            id: session.id.clone(),
            provider: Some(session.provider.clone()),
            name: Some(session.name.clone()),
            hidden_at_ms: now_millis(),
        }
    }

    fn from_id(id: String) -> Self {
        Self {
            id,
            provider: None,
            name: None,
            hidden_at_ms: now_millis(),
        }
    }
}

#[derive(Debug, Deserialize, Serialize)]
struct RegistryDocument {
    version: u32,
    sessions: Vec<HiddenSessionRecord>,
}

impl Default for RegistryDocument {
    fn default() -> Self {
        Self {
            version: REGISTRY_VERSION,
            sessions: Vec::new(),
        }
    }
}

/// Cloneable local visibility registry shared by discovery and TUI control.
#[derive(Clone, Debug)]
pub struct HiddenSessions {
    path: PathBuf,
    records: Arc<Mutex<BTreeMap<String, HiddenSessionRecord>>>,
}

impl HiddenSessions {
    pub fn load_default() -> Result<Self> {
        Self::load(default_hidden_sessions_path()?)
    }

    pub fn load(path: impl Into<PathBuf>) -> Result<Self> {
        let path = path.into();
        let parent = path
            .parent()
            .context("hidden-session registry path has no parent")?;
        ensure_private_directory(parent)?;
        let records = read_registry(&path)?;
        Ok(Self {
            path,
            records: Arc::new(Mutex::new(records)),
        })
    }

    pub fn path(&self) -> &Path {
        &self.path
    }

    pub fn list(&self) -> Vec<HiddenSessionRecord> {
        self.records
            .lock()
            .expect("hidden-session registry mutex poisoned")
            .values()
            .cloned()
            .collect()
    }

    pub fn contains(&self, session_id: &str) -> bool {
        self.records
            .lock()
            .expect("hidden-session registry mutex poisoned")
            .contains_key(session_id)
    }

    /// Remove locally hidden records from a snapshot before the App performs
    /// grouping or its bounded 25-row pagination.
    pub fn filter_snapshot(&self, snapshot: &mut SessionSnapshot) {
        let records = self
            .records
            .lock()
            .expect("hidden-session registry mutex poisoned");
        if records.is_empty() {
            return;
        }
        snapshot
            .sessions
            .retain(|session| !records.contains_key(&session.id));
    }

    /// Hide one discovered row locally. Returns false when it was already
    /// hidden. Existing metadata is refreshed without changing provider state.
    pub fn hide_session(&self, session: &AgentSession) -> Result<bool> {
        Ok(self.hide_sessions(std::slice::from_ref(session))? == 1)
    }

    /// Atomically hide a set of exact discovered rows. Every ID is validated
    /// before the registry changes, and the provider remains untouched.
    pub fn hide_sessions(&self, sessions: &[AgentSession]) -> Result<usize> {
        for session in sessions {
            validate_session_id(&session.id)?;
        }
        self.mutate(|records| {
            let mut inserted = 0;
            for session in sessions {
                inserted += usize::from(!records.contains_key(&session.id));
                records.insert(
                    session.id.clone(),
                    HiddenSessionRecord::from_session(session),
                );
            }
            Ok(inserted)
        })
    }

    /// Add an exact normalized ID from the CLI without discovering or touching
    /// the provider. Returns false when it was already hidden.
    pub fn hide_id(&self, session_id: &str) -> Result<bool> {
        validate_session_id(session_id)?;
        let session_id = session_id.to_owned();
        self.mutate(move |records| {
            if records.contains_key(&session_id) {
                return Ok(false);
            }
            records.insert(
                session_id.clone(),
                HiddenSessionRecord::from_id(session_id),
            );
            Ok(true)
        })
    }

    /// Reveal a locally hidden ID again. This never recreates or resumes a
    /// provider session; the row returns only if provider discovery still sees
    /// it.
    pub fn unhide(&self, session_id: &str) -> Result<Option<HiddenSessionRecord>> {
        validate_session_id(session_id)?;
        let session_id = session_id.to_owned();
        self.mutate(move |records| Ok(records.remove(&session_id)))
    }

    fn mutate<T>(
        &self,
        operation: impl FnOnce(&mut BTreeMap<String, HiddenSessionRecord>) -> Result<T>,
    ) -> Result<T> {
        let parent = self
            .path
            .parent()
            .context("hidden-session registry path has no parent")?;
        let _lock = RegistryLock::acquire(&parent.join("hidden-sessions.lock"))?;
        // Reload under the cross-process lock so simultaneous dashboards and
        // CLI invocations cannot silently overwrite one another's changes.
        let mut records = read_registry(&self.path)?;
        let result = operation(&mut records)?;
        write_registry(&self.path, &records)?;
        *self
            .records
            .lock()
            .expect("hidden-session registry mutex poisoned") = records;
        Ok(result)
    }
}

pub fn default_hidden_sessions_path() -> Result<PathBuf> {
    if let Some(state_home) = std::env::var_os("XDG_STATE_HOME") {
        return Ok(PathBuf::from(state_home)
            .join("open-agent-view")
            .join("hidden-sessions.json"));
    }
    let home = std::env::var_os("HOME").context("HOME is not set")?;
    Ok(PathBuf::from(home).join(".local/state/open-agent-view/hidden-sessions.json"))
}

fn validate_session_id(session_id: &str) -> Result<()> {
    if session_id.is_empty() || session_id.len() > MAX_SESSION_ID_BYTES {
        bail!("session ID must contain between 1 and {MAX_SESSION_ID_BYTES} bytes");
    }
    if session_id.chars().any(char::is_control) {
        bail!("session ID cannot contain control characters");
    }
    Ok(())
}

fn read_registry(path: &Path) -> Result<BTreeMap<String, HiddenSessionRecord>> {
    match fs::symlink_metadata(path) {
        Ok(_) => ensure_private_regular_file(path)?,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(BTreeMap::new()),
        Err(error) => {
            return Err(error)
                .with_context(|| format!("failed to inspect hidden sessions {}", path.display()))
        }
    }
    let input = fs::read_to_string(path)
        .with_context(|| format!("failed to read hidden sessions {}", path.display()))?;
    let document: RegistryDocument = serde_json::from_str(&input)
        .with_context(|| format!("invalid hidden sessions registry {}", path.display()))?;
    if document.version != REGISTRY_VERSION {
        bail!(
            "unsupported hidden sessions registry version {} in {}",
            document.version,
            path.display()
        );
    }
    let mut records = BTreeMap::new();
    for record in document.sessions {
        validate_session_id(&record.id)
            .with_context(|| format!("invalid hidden session in {}", path.display()))?;
        if records.insert(record.id.clone(), record).is_some() {
            bail!("duplicate hidden session ID in {}", path.display());
        }
    }
    Ok(records)
}

fn write_registry(path: &Path, records: &BTreeMap<String, HiddenSessionRecord>) -> Result<()> {
    let document = RegistryDocument {
        version: REGISTRY_VERSION,
        sessions: records.values().cloned().collect(),
    };
    let temporary = temporary_path(path)?;
    let result = (|| {
        let mut options = OpenOptions::new();
        options.create_new(true).write(true);
        #[cfg(unix)]
        {
            use std::os::unix::fs::OpenOptionsExt;
            options.mode(0o600);
        }
        let mut file = options
            .open(&temporary)
            .with_context(|| format!("failed to create {}", temporary.display()))?;
        serde_json::to_writer_pretty(&mut file, &document)?;
        file.write_all(b"\n")?;
        file.sync_all()?;
        fs::rename(&temporary, path)
            .with_context(|| format!("failed to replace {}", path.display()))?;
        Ok(())
    })();
    if result.is_err() {
        let _ = fs::remove_file(&temporary);
    }
    result
}

fn ensure_private_directory(path: &Path) -> Result<()> {
    if !path.exists() {
        fs::create_dir_all(path)
            .with_context(|| format!("failed to create private directory {}", path.display()))?;
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            fs::set_permissions(path, fs::Permissions::from_mode(0o700))?;
        }
    }
    let metadata = fs::symlink_metadata(path)
        .with_context(|| format!("failed to inspect {}", path.display()))?;
    if metadata.file_type().is_symlink() || !metadata.is_dir() {
        bail!("{} must be a real directory", path.display());
    }
    #[cfg(unix)]
    {
        use std::os::unix::fs::{MetadataExt, PermissionsExt};
        if metadata.uid() != unsafe { libc::geteuid() } {
            bail!("{} is not owned by the current user", path.display());
        }
        if metadata.permissions().mode() & 0o777 != 0o700 {
            bail!("{} must have mode 0700", path.display());
        }
    }
    Ok(())
}

fn ensure_private_regular_file(path: &Path) -> Result<()> {
    let metadata = fs::symlink_metadata(path)
        .with_context(|| format!("failed to inspect {}", path.display()))?;
    if metadata.file_type().is_symlink() || !metadata.is_file() {
        bail!("{} must be a regular file", path.display());
    }
    #[cfg(unix)]
    {
        use std::os::unix::fs::{MetadataExt, PermissionsExt};
        if metadata.uid() != unsafe { libc::geteuid() } {
            bail!("{} is not owned by the current user", path.display());
        }
        if metadata.permissions().mode() & 0o777 != 0o600 {
            bail!("{} must have mode 0600", path.display());
        }
    }
    Ok(())
}

#[derive(Debug)]
struct RegistryLock {
    file: File,
}

impl RegistryLock {
    fn acquire(path: &Path) -> Result<Self> {
        match fs::symlink_metadata(path) {
            Ok(_) => ensure_private_regular_file(path)?,
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
            Err(error) => {
                return Err(error).with_context(|| format!("failed to inspect {}", path.display()))
            }
        }
        let mut options = OpenOptions::new();
        options.create(true).read(true).write(true);
        #[cfg(unix)]
        {
            use std::os::unix::fs::OpenOptionsExt;
            options.mode(0o600);
        }
        let file = options
            .open(path)
            .with_context(|| format!("failed to open {}", path.display()))?;
        #[cfg(unix)]
        {
            use std::os::fd::AsRawFd;
            if unsafe { libc::flock(file.as_raw_fd(), libc::LOCK_EX) } != 0 {
                return Err(std::io::Error::last_os_error())
                    .context("failed to lock hidden-session registry");
            }
        }
        Ok(Self { file })
    }
}

impl Drop for RegistryLock {
    fn drop(&mut self) {
        #[cfg(unix)]
        {
            use std::os::fd::AsRawFd;
            let _ = unsafe { libc::flock(self.file.as_raw_fd(), libc::LOCK_UN) };
        }
    }
}

fn temporary_path(path: &Path) -> Result<PathBuf> {
    let name = path
        .file_name()
        .context("hidden-session registry path has no file name")?
        .to_string_lossy();
    let sequence = TEMPORARY_SEQUENCE.fetch_add(1, Ordering::Relaxed);
    Ok(path.with_file_name(format!(
        ".{name}.tmp-{}-{sequence}",
        std::process::id()
    )))
}

fn now_millis() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis() as u64
}

#[cfg(test)]
mod tests {
    use std::collections::BTreeSet;
    use std::path::PathBuf;

    use tempfile::tempdir;

    use super::*;
    use crate::domain::{Capability, Runtime, SessionKind, SessionState};

    fn private_tempdir() -> tempfile::TempDir {
        let directory = tempdir().unwrap();
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            fs::set_permissions(directory.path(), fs::Permissions::from_mode(0o700)).unwrap();
        }
        directory
    }

    fn session(id: &str) -> AgentSession {
        AgentSession {
            id: id.into(),
            provider_session_id: id.into(),
            provider: Provider::Pi,
            runtime: Runtime::Host,
            kind: SessionKind::Unknown,
            name: format!("row {id}"),
            cwd: PathBuf::from("/work"),
            state: SessionState::Completed,
            summary: String::new(),
            raw_state: None,
            pid: None,
            started_at: None,
            updated_at: None,
            pull_requests: None,
            capabilities: BTreeSet::from([Capability::Inspect]),
        }
    }

    #[test]
    fn hide_filter_unhide_round_trip_is_persistent_and_non_destructive() {
        let directory = private_tempdir();
        let path = directory.path().join("hidden-sessions.json");
        let registry = HiddenSessions::load(&path).unwrap();
        let hidden = session("pi:host:old");
        let visible = session("pi:host:current");

        assert!(registry.hide_session(&hidden).unwrap());
        assert!(!registry.hide_session(&hidden).unwrap());
        let mut snapshot = SessionSnapshot {
            sessions: vec![hidden.clone(), visible.clone()],
            warnings: vec![],
        };
        registry.filter_snapshot(&mut snapshot);
        assert_eq!(snapshot.sessions, vec![visible]);

        let reloaded = HiddenSessions::load(&path).unwrap();
        assert!(reloaded.contains(&hidden.id));
        let record = reloaded.unhide(&hidden.id).unwrap().unwrap();
        assert_eq!(record.provider, Some(Provider::Pi));
        assert!(!HiddenSessions::load(&path).unwrap().contains(&hidden.id));

        // The provider-shaped session value remains untouched throughout.
        assert_eq!(hidden.provider_session_id, "pi:host:old");
        assert_eq!(hidden.state, SessionState::Completed);
    }

    #[test]
    fn exact_id_cli_entries_are_idempotent_and_can_be_listed() {
        let directory = private_tempdir();
        let registry = HiddenSessions::load(directory.path().join("hidden.json")).unwrap();
        assert!(registry.hide_id("claude:host:abc").unwrap());
        assert!(!registry.hide_id("claude:host:abc").unwrap());
        assert_eq!(registry.list()[0].id, "claude:host:abc");
        assert_eq!(registry.list()[0].provider, None);
        assert!(registry.unhide("missing").unwrap().is_none());
    }

    #[test]
    fn filtering_a_large_snapshot_keeps_order_and_scales_with_hidden_ids() {
        let directory = private_tempdir();
        let registry = HiddenSessions::load(directory.path().join("hidden.json")).unwrap();
        for index in (0..70_000).step_by(7_000) {
            registry
                .hide_id(&format!("pi:host:{index:05}"))
                .unwrap();
        }
        let mut snapshot = SessionSnapshot {
            sessions: (0..70_000)
                .map(|index| session(&format!("pi:host:{index:05}")))
                .collect(),
            warnings: vec![],
        };
        registry.filter_snapshot(&mut snapshot);
        assert_eq!(snapshot.sessions.len(), 69_990);
        assert_eq!(snapshot.sessions[0].id, "pi:host:00001");
        assert_eq!(snapshot.sessions.last().unwrap().id, "pi:host:69999");
    }

    #[test]
    fn refuses_control_characters_symlinks_and_insecure_files() {
        let directory = private_tempdir();
        let path = directory.path().join("hidden.json");
        let registry = HiddenSessions::load(&path).unwrap();
        assert!(registry.hide_id("bad\nid").is_err());
        registry.hide_id("valid").unwrap();

        #[cfg(unix)]
        {
            use std::os::unix::fs::{symlink, PermissionsExt};
            let target = directory.path().join("target.json");
            fs::write(&target, "{}").unwrap();
            let link = directory.path().join("link.json");
            symlink(&target, &link).unwrap();
            assert!(HiddenSessions::load(link).is_err());

            fs::set_permissions(&path, fs::Permissions::from_mode(0o644)).unwrap();
            assert!(HiddenSessions::load(path).is_err());
        }
    }
}

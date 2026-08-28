//! Private, local display-name overrides for normalized provider sessions.
//!
//! An alias never mutates provider history. Discovery continues to supply the
//! provider's canonical title; this registry replaces only `AgentSession.name`
//! at the final presentation boundary. Clearing an alias therefore reveals the
//! latest provider title on the next refresh.

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
const MAX_ALIAS_BYTES: usize = 240;
static TEMPORARY_SEQUENCE: AtomicU64 = AtomicU64::new(1);

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct SessionAliasRecord {
    pub id: String,
    pub alias: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub provider: Option<Provider>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub provider_name_at_creation: Option<String>,
    #[serde(default)]
    pub renamed_at_ms: u64,
}

#[derive(Debug, Deserialize, Serialize)]
struct RegistryDocument {
    version: u32,
    sessions: Vec<SessionAliasRecord>,
}

#[derive(Clone, Debug)]
pub struct SessionAliases {
    path: PathBuf,
    records: Arc<Mutex<BTreeMap<String, SessionAliasRecord>>>,
}

impl SessionAliases {
    pub fn load_default() -> Result<Self> {
        Self::load(default_session_aliases_path()?)
    }

    pub fn load(path: impl Into<PathBuf>) -> Result<Self> {
        let path = path.into();
        let parent = path
            .parent()
            .context("session-alias registry path has no parent")?;
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

    pub fn list(&self) -> Vec<SessionAliasRecord> {
        self.records
            .lock()
            .expect("session-alias registry mutex poisoned")
            .values()
            .cloned()
            .collect()
    }

    /// Reload aliases written by another dashboard/CLI process. Registry
    /// replacement is atomic, so readers see either the old or new document.
    pub fn reload(&self) -> Result<()> {
        let records = read_registry(&self.path)?;
        *self
            .records
            .lock()
            .expect("session-alias registry mutex poisoned") = records;
        Ok(())
    }

    pub fn apply_snapshot(&self, snapshot: &mut SessionSnapshot) -> usize {
        let records = self
            .records
            .lock()
            .expect("session-alias registry mutex poisoned");
        if records.is_empty() {
            return 0;
        }
        let mut applied = 0;
        for session in &mut snapshot.sessions {
            if let Some(record) = records.get(&session.id) {
                session.name.clone_from(&record.alias);
                applied += 1;
            }
        }
        applied
    }

    pub fn set_for_session(&self, session: &AgentSession, alias: &str) -> Result<bool> {
        validate_session_id(&session.id)?;
        let alias = normalize_alias(alias)?;
        let session = session.clone();
        self.mutate(move |records| {
            let changed = records
                .get(&session.id)
                .map_or(true, |record| record.alias != alias);
            let provider_name_at_creation = records
                .get(&session.id)
                .and_then(|record| record.provider_name_at_creation.clone())
                .or_else(|| Some(session.name.clone()));
            records.insert(
                session.id.clone(),
                SessionAliasRecord {
                    id: session.id,
                    alias,
                    provider: Some(session.provider),
                    provider_name_at_creation,
                    renamed_at_ms: now_millis(),
                },
            );
            Ok(changed)
        })
    }

    pub fn set_for_id(&self, session_id: &str, alias: &str) -> Result<bool> {
        validate_session_id(session_id)?;
        let session_id = session_id.to_owned();
        let alias = normalize_alias(alias)?;
        self.mutate(move |records| {
            let changed = records
                .get(&session_id)
                .map_or(true, |record| record.alias != alias);
            let previous = records.get(&session_id);
            let provider = previous.and_then(|record| record.provider.clone());
            let provider_name_at_creation =
                previous.and_then(|record| record.provider_name_at_creation.clone());
            records.insert(
                session_id.clone(),
                SessionAliasRecord {
                    id: session_id,
                    alias,
                    provider,
                    provider_name_at_creation,
                    renamed_at_ms: now_millis(),
                },
            );
            Ok(changed)
        })
    }

    pub fn clear(&self, session_id: &str) -> Result<Option<SessionAliasRecord>> {
        validate_session_id(session_id)?;
        let session_id = session_id.to_owned();
        self.mutate(move |records| Ok(records.remove(&session_id)))
    }

    fn mutate<T>(
        &self,
        operation: impl FnOnce(&mut BTreeMap<String, SessionAliasRecord>) -> Result<T>,
    ) -> Result<T> {
        let parent = self
            .path
            .parent()
            .context("session-alias registry path has no parent")?;
        let _lock = RegistryLock::acquire(&parent.join("session-aliases.lock"))?;
        let mut records = read_registry(&self.path)?;
        let result = operation(&mut records)?;
        write_registry(&self.path, &records)?;
        *self
            .records
            .lock()
            .expect("session-alias registry mutex poisoned") = records;
        Ok(result)
    }
}

pub fn default_session_aliases_path() -> Result<PathBuf> {
    if let Some(state_home) = std::env::var_os("XDG_STATE_HOME") {
        return Ok(PathBuf::from(state_home)
            .join("open-agent-view")
            .join("session-aliases.json"));
    }
    let home = std::env::var_os("HOME").context("HOME is not set")?;
    Ok(PathBuf::from(home).join(".local/state/open-agent-view/session-aliases.json"))
}

fn normalize_alias(alias: &str) -> Result<String> {
    let alias = alias.trim();
    if alias.is_empty() || alias.len() > MAX_ALIAS_BYTES {
        bail!("session alias must contain between 1 and {MAX_ALIAS_BYTES} bytes");
    }
    if alias.chars().any(char::is_control) {
        bail!("session alias cannot contain control characters");
    }
    Ok(alias.to_owned())
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

fn read_registry(path: &Path) -> Result<BTreeMap<String, SessionAliasRecord>> {
    match fs::symlink_metadata(path) {
        Ok(_) => ensure_private_regular_file(path)?,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(BTreeMap::new()),
        Err(error) => {
            return Err(error)
                .with_context(|| format!("failed to inspect session aliases {}", path.display()))
        }
    }
    let input = fs::read_to_string(path)
        .with_context(|| format!("failed to read session aliases {}", path.display()))?;
    let document: RegistryDocument = serde_json::from_str(&input)
        .with_context(|| format!("invalid session-alias registry {}", path.display()))?;
    if document.version != REGISTRY_VERSION {
        bail!(
            "unsupported session-alias registry version {} in {}",
            document.version,
            path.display()
        );
    }
    let mut records = BTreeMap::new();
    for record in document.sessions {
        validate_session_id(&record.id)?;
        normalize_alias(&record.alias)?;
        if records.insert(record.id.clone(), record).is_some() {
            bail!("duplicate session alias ID in {}", path.display());
        }
    }
    Ok(records)
}

fn write_registry(path: &Path, records: &BTreeMap<String, SessionAliasRecord>) -> Result<()> {
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
        crate::fs_util::replace_file(&temporary, path)
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
                    .context("failed to lock session-alias registry");
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
        .context("session-alias registry path has no file name")?
        .to_string_lossy();
    let sequence = TEMPORARY_SEQUENCE.fetch_add(1, Ordering::Relaxed);
    Ok(path.with_file_name(format!(".{name}.tmp-{}-{sequence}", std::process::id())))
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

    fn session(id: &str, name: &str) -> AgentSession {
        AgentSession {
            id: id.into(),
            provider_session_id: id.into(),
            provider: Provider::Codex,
            runtime: Runtime::Host,
            kind: SessionKind::Managed,
            name: name.into(),
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
    fn local_alias_overrides_provider_rename_until_cleared() {
        let directory = private_tempdir();
        let path = directory.path().join("aliases.json");
        let aliases = SessionAliases::load(path).unwrap();
        let original = session("codex:host:abc", "provider original");
        assert!(aliases.set_for_session(&original, "local label").unwrap());

        let mut provider_renamed = SessionSnapshot {
            sessions: vec![session("codex:host:abc", "provider renamed")],
            warnings: vec![],
        };
        aliases.apply_snapshot(&mut provider_renamed);
        assert_eq!(provider_renamed.sessions[0].name, "local label");
        assert_eq!(
            aliases.list()[0].provider_name_at_creation.as_deref(),
            Some("provider original")
        );

        assert!(aliases.clear("codex:host:abc").unwrap().is_some());
        let mut refreshed = SessionSnapshot {
            sessions: vec![session("codex:host:abc", "provider renamed")],
            warnings: vec![],
        };
        aliases.apply_snapshot(&mut refreshed);
        assert_eq!(refreshed.sessions[0].name, "provider renamed");
    }

    #[test]
    fn cli_aliases_reload_across_process_objects_and_are_idempotent() {
        let directory = private_tempdir();
        let path = directory.path().join("aliases.json");
        let dashboard = SessionAliases::load(&path).unwrap();
        let cli = SessionAliases::load(&path).unwrap();
        assert!(cli.set_for_id("pi:host:abc", "build docs").unwrap());
        assert!(!cli.set_for_id("pi:host:abc", "build docs").unwrap());
        assert!(dashboard.list().is_empty());
        dashboard.reload().unwrap();
        assert_eq!(dashboard.list()[0].alias, "build docs");
    }

    #[test]
    fn rejects_control_characters_oversized_values_symlinks_and_public_files() {
        let directory = private_tempdir();
        let path = directory.path().join("aliases.json");
        let aliases = SessionAliases::load(&path).unwrap();
        assert!(aliases.set_for_id("bad\nid", "name").is_err());
        assert!(aliases.set_for_id("valid", "bad\nname").is_err());
        assert!(aliases
            .set_for_id("valid", &"x".repeat(MAX_ALIAS_BYTES + 1))
            .is_err());
        aliases.set_for_id("valid", "safe").unwrap();

        #[cfg(unix)]
        {
            use std::os::unix::fs::{symlink, PermissionsExt};
            let target = directory.path().join("target.json");
            fs::write(&target, "{}").unwrap();
            let link = directory.path().join("link.json");
            symlink(&target, &link).unwrap();
            assert!(SessionAliases::load(link).is_err());

            fs::set_permissions(&path, fs::Permissions::from_mode(0o644)).unwrap();
            assert!(SessionAliases::load(path).is_err());
        }
    }
}

//! Small private ownership registry shared by native-only provider adapters.
//!
//! Records contain only provider session IDs, display names, workspaces, and
//! creation times. Provider credentials and transcript bodies never enter OAV
//! state.

use std::collections::BTreeSet;
#[cfg(unix)]
use std::fs::File;
use std::fs::{self, OpenOptions};
use std::io::Write;
use std::path::{Path, PathBuf};
use std::sync::Mutex;
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use anyhow::{anyhow, bail, Context, Result};
use serde::{Deserialize, Serialize};

#[derive(Clone, Debug, Deserialize, Eq, Ord, PartialEq, PartialOrd, Serialize)]
#[serde(rename_all = "camelCase")]
pub(super) struct OwnedNativeSession {
    pub session_id: String,
    pub cwd: PathBuf,
    pub created_at_ms: u64,
    pub name: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub session_path: Option<PathBuf>,
}

pub(super) struct NativeOwnership {
    path: PathBuf,
    records: Mutex<BTreeSet<OwnedNativeSession>>,
}

impl NativeOwnership {
    pub fn load(path: PathBuf, label: &str) -> Result<Self> {
        reject_symlink(&path, label)?;
        if let Some(parent) = path.parent().filter(|parent| parent.exists()) {
            ensure_private_directory(parent, label)?;
        }
        let records = match fs::read_to_string(&path) {
            Ok(input) => {
                ensure_private_file(&path, label)?;
                serde_json::from_str(&input).with_context(|| {
                    format!("invalid {label} ownership registry {}", path.display())
                })?
            }
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => BTreeSet::new(),
            Err(error) => return Err(error.into()),
        };
        Ok(Self {
            path,
            records: Mutex::new(records),
        })
    }

    pub fn owns(&self, session_id: &str) -> bool {
        self.records
            .lock()
            .map(|records| records.iter().any(|record| record.session_id == session_id))
            .unwrap_or(false)
    }

    pub fn records(&self) -> Vec<OwnedNativeSession> {
        self.records
            .lock()
            .map(|records| records.iter().cloned().collect())
            .unwrap_or_default()
    }

    pub fn record(
        &self,
        session_id: &str,
        cwd: &Path,
        name: &str,
        session_path: Option<&Path>,
        label: &str,
    ) -> Result<()> {
        validate_id(session_id, label)?;
        let mut records = self
            .records
            .lock()
            .map_err(|_| anyhow!("{label} ownership registry lock was poisoned"))?;
        let mut updated = records.clone();
        updated.retain(|record| record.session_id != session_id);
        updated.insert(OwnedNativeSession {
            session_id: session_id.into(),
            cwd: cwd.to_owned(),
            created_at_ms: now_millis(),
            name: sanitize(name, 80, &format!("{label} session")),
            session_path: session_path.map(Path::to_owned),
        });
        persist(&self.path, &updated, label)?;
        *records = updated;
        Ok(())
    }
}

pub(super) fn validate_id(value: &str, label: &str) -> Result<()> {
    if value.is_empty()
        || value.len() > 256
        || value
            .chars()
            .any(|character| character.is_control() || character.is_whitespace())
    {
        bail!("invalid {label} session ID");
    }
    Ok(())
}

pub(super) fn sanitize(value: &str, limit: usize, fallback: &str) -> String {
    let clean = value
        .chars()
        .map(|character| {
            if character.is_control() {
                ' '
            } else {
                character
            }
        })
        .collect::<String>();
    let clean = clean.split_whitespace().collect::<Vec<_>>().join(" ");
    if clean.is_empty() {
        return fallback.into();
    }
    let mut result = clean.chars().take(limit).collect::<String>();
    if clean.chars().count() > limit {
        result.push('…');
    }
    result
}

pub(super) fn poll_unique<T>(
    label: &str,
    timeout: Duration,
    probe: impl FnMut() -> Result<Vec<T>>,
) -> Result<T> {
    poll_unique_with_interval(label, timeout, Duration::from_millis(50), probe)
}

fn poll_unique_with_interval<T>(
    label: &str,
    timeout: Duration,
    interval: Duration,
    mut probe: impl FnMut() -> Result<Vec<T>>,
) -> Result<T> {
    let deadline = Instant::now() + timeout;
    loop {
        let mut candidates = probe()?;
        match candidates.len() {
            1 => return Ok(candidates.remove(0)),
            count if count > 1 => {
                bail!("{label} found {count} candidates; refusing ambiguous ownership")
            }
            _ if Instant::now() >= deadline => {
                bail!("timed out waiting for {label}; Open Agent View did not claim ownership")
            }
            _ => std::thread::sleep(interval),
        }
    }
}

fn now_millis() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis() as u64
}

fn persist(path: &Path, records: &BTreeSet<OwnedNativeSession>, label: &str) -> Result<()> {
    let parent = path
        .parent()
        .with_context(|| format!("{label} ownership registry has no parent"))?;
    reject_symlink(parent, label)?;
    reject_symlink(path, label)?;
    if parent.exists() {
        ensure_private_directory(parent, label)?;
    } else {
        fs::create_dir_all(parent)?;
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            fs::set_permissions(parent, fs::Permissions::from_mode(0o700))?;
        }
        ensure_private_directory(parent, label)?;
    }
    let temporary = path.with_extension(format!(
        "tmp-{}-{}",
        std::process::id(),
        crate::native_session::new_session_id()?
    ));
    let mut options = OpenOptions::new();
    options.create_new(true).write(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.mode(0o600);
    }
    let result = (|| -> Result<()> {
        let mut file = options.open(&temporary)?;
        serde_json::to_writer_pretty(&mut file, records)?;
        file.write_all(b"\n")?;
        file.sync_all()?;
        fs::rename(&temporary, path)?;
        #[cfg(unix)]
        File::open(parent)?.sync_all()?;
        Ok(())
    })();
    if result.is_err() {
        let _ = fs::remove_file(&temporary);
    }
    result
}

fn reject_symlink(path: &Path, label: &str) -> Result<()> {
    match fs::symlink_metadata(path) {
        Ok(metadata) if metadata.file_type().is_symlink() => {
            bail!("refusing symlinked {label} state path {}", path.display())
        }
        Ok(_) => Ok(()),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(()),
        Err(error) => Err(error.into()),
    }
}

fn ensure_private_directory(path: &Path, label: &str) -> Result<()> {
    let metadata = fs::symlink_metadata(path)?;
    if metadata.file_type().is_symlink() || !metadata.is_dir() {
        bail!(
            "{label} state parent {} must be a real directory",
            path.display()
        );
    }
    #[cfg(unix)]
    {
        use std::os::unix::fs::{MetadataExt, PermissionsExt};
        if metadata.uid() != unsafe { libc::geteuid() } {
            bail!(
                "{label} state parent {} is not owned by this user",
                path.display()
            );
        }
        if metadata.permissions().mode() & 0o077 != 0 {
            bail!(
                "{label} state parent {} is accessible by other users",
                path.display()
            );
        }
    }
    Ok(())
}

fn ensure_private_file(path: &Path, label: &str) -> Result<()> {
    let metadata = fs::symlink_metadata(path)?;
    if metadata.file_type().is_symlink() || !metadata.is_file() {
        bail!("{label} state {} must be a real file", path.display());
    }
    #[cfg(unix)]
    {
        use std::os::unix::fs::{MetadataExt, PermissionsExt};
        if metadata.uid() != unsafe { libc::geteuid() } {
            bail!("{label} state {} is not owned by this user", path.display());
        }
        if metadata.permissions().mode() & 0o077 != 0 {
            bail!(
                "{label} state {} is accessible by other users",
                path.display()
            );
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::test_support::tempfile;

    #[test]
    fn registry_round_trips_metadata_without_transcript_or_credentials() {
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("owned.json");
        let registry = NativeOwnership::load(path.clone(), "Test").unwrap();
        registry
            .record(
                "session-1",
                Path::new("/work"),
                "  parser\nwork  ",
                Some(Path::new("/private/session")),
                "Test",
            )
            .unwrap();
        let input = fs::read_to_string(&path).unwrap();
        assert!(input.contains("parser work"));
        assert!(!input.contains("credential"));
        let restored = NativeOwnership::load(path, "Test").unwrap();
        assert!(restored.owns("session-1"));
        assert_eq!(restored.records()[0].cwd, Path::new("/work"));
        assert_eq!(
            restored.records()[0].session_path.as_deref(),
            Some(Path::new("/private/session"))
        );
    }

    #[cfg(unix)]
    #[test]
    fn registry_refuses_symlink_target() {
        use std::os::unix::fs::symlink;

        let directory = tempfile::tempdir().unwrap();
        let target = directory.path().join("target");
        fs::write(&target, "[]").unwrap();
        let link = directory.path().join("owned.json");
        symlink(target, &link).unwrap();
        assert!(NativeOwnership::load(link, "Test").is_err());
    }

    #[cfg(unix)]
    #[test]
    fn registry_refuses_group_readable_file_and_parent() {
        use std::os::unix::fs::PermissionsExt;

        let directory = tempfile::tempdir().unwrap();
        let parent = directory.path().join("state");
        fs::create_dir(&parent).unwrap();
        fs::set_permissions(&parent, fs::Permissions::from_mode(0o700)).unwrap();
        let path = parent.join("owned.json");
        fs::write(&path, "[]").unwrap();
        fs::set_permissions(&path, fs::Permissions::from_mode(0o640)).unwrap();
        assert!(NativeOwnership::load(path.clone(), "Test").is_err());

        fs::set_permissions(&path, fs::Permissions::from_mode(0o600)).unwrap();
        fs::set_permissions(&parent, fs::Permissions::from_mode(0o750)).unwrap();
        assert!(NativeOwnership::load(path, "Test").is_err());
    }

    #[cfg(unix)]
    #[test]
    fn failed_persistence_never_grants_in_memory_ownership_or_leaves_a_temp_file() {
        use std::os::unix::fs::PermissionsExt;

        let directory = tempfile::tempdir().unwrap();
        let parent = directory.path().join("state");
        fs::create_dir(&parent).unwrap();
        fs::set_permissions(&parent, fs::Permissions::from_mode(0o700)).unwrap();
        let path = parent.join("owned.json");
        let registry = NativeOwnership::load(path.clone(), "Test").unwrap();

        // A directory at the final file path makes the atomic rename fail
        // after the private temporary file has been written.
        fs::create_dir(&path).unwrap();
        assert!(registry
            .record("session-1", Path::new("/work"), "Parser", None, "Test")
            .is_err());
        assert!(!registry.owns("session-1"));
        assert!(registry.records().is_empty());
        assert!(fs::read_dir(&parent).unwrap().all(|entry| {
            !entry
                .unwrap()
                .file_name()
                .to_string_lossy()
                .starts_with("owned.tmp-")
        }));
    }

    #[test]
    fn unique_correlation_polls_for_delayed_state_and_rejects_ambiguity() {
        let mut attempts = 0;
        let found = poll_unique_with_interval(
            "test session",
            Duration::from_millis(20),
            Duration::ZERO,
            || {
                attempts += 1;
                Ok(if attempts < 3 { Vec::new() } else { vec![42] })
            },
        )
        .unwrap();
        assert_eq!(found, 42);
        assert_eq!(attempts, 3);

        let error = poll_unique_with_interval(
            "test session",
            Duration::from_millis(20),
            Duration::ZERO,
            || Ok(vec![1, 2]),
        )
        .unwrap_err();
        assert!(error.to_string().contains("refusing ambiguous ownership"));
    }
}

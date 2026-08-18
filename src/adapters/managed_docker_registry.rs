//! Protected persistence and CLI-oriented orchestration for managed Docker.
//!
//! The registry is the external half of container ownership. Labels alone do
//! not grant lifecycle authority: an exact immutable container ID and random
//! instance ID must also be present in this atomically written registry.

use std::collections::BTreeMap;
use std::fs::{self, File, OpenOptions};
use std::io::{Read, Write};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;

use anyhow::{bail, Context, Result};
use serde::{Deserialize, Serialize};

use super::{
    DockerAuthority, ManagedDockerContainer, ManagedDockerCreateSpec, ManagedDockerOwner,
    ManagedDockerRuntime,
};
use crate::process::CommandRunner;

const REGISTRY_VERSION: u32 = 1;
static TEMPORARY_SEQUENCE: AtomicU64 = AtomicU64::new(1);

/// Registry of exact ownership proofs stored outside Docker.
#[derive(Debug)]
pub struct ManagedDockerRegistry {
    path: PathBuf,
    owners: BTreeMap<String, ManagedDockerOwner>,
    _lock: RegistryLock,
}

impl ManagedDockerRegistry {
    /// Open a registry, creating its private parent directory when needed.
    pub fn open(path: impl Into<PathBuf>) -> Result<Self> {
        let path = path.into();
        let parent = path
            .parent()
            .context("managed Docker registry path has no parent")?;
        ensure_private_directory(parent)?;
        let lock = RegistryLock::acquire(&parent.join("owners.lock"))?;
        match fs::symlink_metadata(&path) {
            Ok(_) => ensure_private_regular_file(&path)?,
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
            Err(error) => {
                return Err(error)
                    .with_context(|| format!("failed to inspect registry {}", path.display()))
            }
        }

        let document = match fs::read_to_string(&path) {
            Ok(input) => serde_json::from_str::<RegistryDocument>(&input)
                .with_context(|| format!("invalid managed Docker registry {}", path.display()))?,
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
                RegistryDocument::default()
            }
            Err(error) => {
                return Err(error)
                    .with_context(|| format!("failed to read registry {}", path.display()))
            }
        };
        if document.version != REGISTRY_VERSION {
            bail!(
                "unsupported managed Docker registry version {} in {}",
                document.version,
                path.display()
            );
        }

        let mut owners = BTreeMap::new();
        let mut instances = BTreeMap::<String, String>::new();
        for stored in document.owners {
            let owner = ManagedDockerOwner::new(stored.container_id, stored.instance_id)
                .with_context(|| format!("invalid owner record in {}", path.display()))?;
            if owners
                .insert(owner.container_id().to_owned(), owner.clone())
                .is_some()
            {
                bail!(
                    "duplicate container ID {} in {}",
                    owner.container_id(),
                    path.display()
                );
            }
            if let Some(other_id) = instances.insert(
                owner.instance_id().to_owned(),
                owner.container_id().to_owned(),
            ) {
                bail!(
                    "instance ID {} is assigned to both {} and {} in {}",
                    owner.instance_id(),
                    other_id,
                    owner.container_id(),
                    path.display()
                );
            }
        }
        Ok(Self {
            path,
            owners,
            _lock: lock,
        })
    }

    pub fn path(&self) -> &Path {
        &self.path
    }

    pub fn get(&self, container_id: &str) -> Option<&ManagedDockerOwner> {
        self.owners.get(container_id)
    }

    pub fn list(&self) -> Vec<ManagedDockerOwner> {
        self.owners.values().cloned().collect()
    }

    /// Record an exact owner proof. Re-registering the same pair is
    /// idempotent; changing either half of an existing pair is refused.
    pub fn record(&mut self, owner: ManagedDockerOwner) -> Result<bool> {
        let validated = ManagedDockerOwner::new(owner.container_id(), owner.instance_id())?;
        if let Some(existing) = self.owners.get(validated.container_id()) {
            if existing == &validated {
                return Ok(false);
            }
            bail!(
                "container {} is already registered with a different instance ID",
                validated.container_id()
            );
        }
        if let Some(existing) = self
            .owners
            .values()
            .find(|existing| existing.instance_id() == validated.instance_id())
        {
            bail!(
                "instance ID {} is already registered to container {}",
                validated.instance_id(),
                existing.container_id()
            );
        }

        self.owners
            .insert(validated.container_id().to_owned(), validated.clone());
        if let Err(error) = self.save() {
            self.owners.remove(validated.container_id());
            return Err(error);
        }
        Ok(true)
    }

    /// Forget only an exact ID+instance pair. A stale or mismatched instance
    /// can never erase the current record for an immutable container ID.
    pub fn forget_exact(&mut self, container_id: &str, instance_id: &str) -> Result<bool> {
        let requested = ManagedDockerOwner::new(container_id, instance_id)?;
        let Some(existing) = self.owners.get(container_id) else {
            return Ok(false);
        };
        if existing != &requested {
            bail!("refusing to forget container {container_id}: instance ID does not match");
        }

        let removed = self
            .owners
            .remove(container_id)
            .expect("record checked above");
        if let Err(error) = self.save() {
            self.owners.insert(container_id.to_owned(), removed);
            return Err(error);
        }
        Ok(true)
    }

    fn save(&self) -> Result<()> {
        let document = RegistryDocument {
            version: REGISTRY_VERSION,
            owners: self.owners.values().map(StoredOwner::from).collect(),
        };
        let bytes = serde_json::to_vec_pretty(&document)?;
        let parent = self
            .path
            .parent()
            .context("managed Docker registry path has no parent")?;
        ensure_private_directory(parent)?;
        let temporary = temporary_path(&self.path)?;

        let result = (|| -> Result<()> {
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
            file.write_all(&bytes)?;
            file.write_all(b"\n")?;
            file.sync_all()?;
            fs::rename(&temporary, &self.path).with_context(|| {
                format!(
                    "failed to atomically replace managed Docker registry {}",
                    self.path.display()
                )
            })?;
            File::open(parent)?.sync_all()?;
            ensure_private_regular_file(&self.path)?;
            Ok(())
        })();
        if result.is_err() {
            let _ = fs::remove_file(&temporary);
        }
        result
    }
}

/// A redacted, serializable container status suitable for text or JSON CLI
/// output. It intentionally excludes labels, environment, and mount details.
#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
pub struct ManagedDockerStatus {
    pub container_id: String,
    pub instance_id: String,
    pub name: Option<String>,
    pub image: Option<String>,
    pub state: ManagedDockerState,
    pub detail: Option<String>,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum ManagedDockerState {
    Running,
    Stopped,
    Paused,
    Restarting,
    Unavailable,
}

/// Registry-backed operations designed to map directly onto a future CLI.
pub struct ManagedDockerService {
    runtime: ManagedDockerRuntime,
    registry: ManagedDockerRegistry,
}

impl ManagedDockerService {
    pub fn open(docker_executable: impl Into<String>, registry_path: PathBuf) -> Result<Self> {
        Ok(Self {
            runtime: ManagedDockerRuntime::new(docker_executable),
            registry: ManagedDockerRegistry::open(registry_path)?,
        })
    }

    pub fn with_runner(
        docker_executable: impl Into<String>,
        registry_path: PathBuf,
        runner: Arc<dyn CommandRunner>,
    ) -> Result<Self> {
        Ok(Self {
            runtime: ManagedDockerRuntime::with_runner(docker_executable, runner),
            registry: ManagedDockerRegistry::open(registry_path)?,
        })
    }

    pub fn registry(&self) -> &ManagedDockerRegistry {
        &self.registry
    }

    /// Create a stopped container, verify its labels/identity, then atomically
    /// persist ownership. If persistence fails, the error names the stopped
    /// orphan and lifecycle authority remains unavailable.
    pub fn create(&mut self, spec: &ManagedDockerCreateSpec) -> Result<ManagedDockerStatus> {
        let managed = self.runtime.create_managed(spec)?;
        let owner = managed.owner().clone();
        self.registry.record(owner).with_context(|| {
            format!(
                "container {} was created and left stopped, but its owner record was not saved",
                managed.container().id
            )
        })?;
        Ok(status_from_managed(&managed))
    }

    /// Resolve a user-facing name/ID to an immutable ID, look up the exact
    /// external record, then re-inspect and verify all managed labels.
    pub fn enroll(&self, reference: &str) -> Result<ManagedDockerContainer> {
        let metadata = self
            .runtime
            .enroll_explicit(reference, DockerAuthority::Metadata)?;
        let id = &metadata.container().id;
        let owner = self
            .registry
            .get(id)
            .with_context(|| format!("container {id} has no managed ownership record"))?
            .clone();
        self.runtime.enroll_managed(id, owner)
    }

    /// Return status for one exact registered container ID. Docker or label
    /// failures are represented as `unavailable` rather than mutating state.
    pub fn status(&self, container_id: &str) -> Result<ManagedDockerStatus> {
        let owner = self
            .registry
            .get(container_id)
            .with_context(|| format!("container {container_id} is not registered"))?;
        Ok(self.status_for_owner(owner))
    }

    /// Return every registered owner in immutable-ID order. One missing or
    /// tampered container does not hide the status of healthy records.
    pub fn list(&self) -> Vec<ManagedDockerStatus> {
        self.registry
            .list()
            .iter()
            .map(|owner| self.status_for_owner(owner))
            .collect()
    }

    pub fn start(&self, reference: &str) -> Result<ManagedDockerStatus> {
        let managed = self.enroll(reference)?;
        let id = managed.container().id.clone();
        self.runtime.start_managed(&managed)?;
        self.status(&id)
    }

    pub fn stop(&self, reference: &str) -> Result<ManagedDockerStatus> {
        let managed = self.enroll(reference)?;
        let id = managed.container().id.clone();
        self.runtime.stop_managed(&managed)?;
        self.status(&id)
    }

    pub fn remove(&mut self, reference: &str) -> Result<ManagedDockerOwner> {
        let managed = self.enroll(reference)?;
        let owner = managed.owner().clone();
        self.runtime.remove_managed(&managed)?;
        self.registry
            .forget_exact(owner.container_id(), owner.instance_id())
            .context("container was removed but its ownership record could not be forgotten")?;
        Ok(owner)
    }

    fn status_for_owner(&self, owner: &ManagedDockerOwner) -> ManagedDockerStatus {
        match self
            .runtime
            .enroll_managed(owner.container_id(), owner.clone())
        {
            Ok(managed) => status_from_managed(&managed),
            Err(error) => ManagedDockerStatus {
                container_id: owner.container_id().to_owned(),
                instance_id: owner.instance_id().to_owned(),
                name: None,
                image: None,
                state: ManagedDockerState::Unavailable,
                detail: Some(format!("{error:#}")),
            },
        }
    }
}

pub fn default_managed_docker_registry_path() -> Result<PathBuf> {
    if let Some(state_home) = std::env::var_os("XDG_STATE_HOME") {
        return Ok(PathBuf::from(state_home)
            .join("open-agent-view")
            .join("managed-docker")
            .join("owners.json"));
    }
    let home = std::env::var_os("HOME").context("HOME is not set")?;
    Ok(PathBuf::from(home).join(".local/state/open-agent-view/managed-docker/owners.json"))
}

pub fn generate_managed_instance_id() -> Result<String> {
    let mut bytes = [0_u8; 16];
    File::open("/dev/urandom")
        .context("failed to open the operating system random source")?
        .read_exact(&mut bytes)
        .context("failed to generate a managed-container instance ID")?;
    bytes[6] = (bytes[6] & 0x0f) | 0x40;
    bytes[8] = (bytes[8] & 0x3f) | 0x80;
    Ok(format!(
        "{:02x}{:02x}{:02x}{:02x}-{:02x}{:02x}-{:02x}{:02x}-{:02x}{:02x}-{:02x}{:02x}{:02x}{:02x}{:02x}{:02x}",
        bytes[0], bytes[1], bytes[2], bytes[3], bytes[4], bytes[5], bytes[6], bytes[7],
        bytes[8], bytes[9], bytes[10], bytes[11], bytes[12], bytes[13], bytes[14], bytes[15]
    ))
}

fn status_from_managed(managed: &ManagedDockerContainer) -> ManagedDockerStatus {
    let container = managed.container();
    let state = if container.restarting {
        ManagedDockerState::Restarting
    } else if container.paused {
        ManagedDockerState::Paused
    } else if container.running {
        ManagedDockerState::Running
    } else {
        ManagedDockerState::Stopped
    };
    ManagedDockerStatus {
        container_id: container.id.clone(),
        instance_id: managed.owner().instance_id().to_owned(),
        name: Some(container.name.clone()),
        image: Some(container.image.clone()),
        state,
        detail: Some(container.status.clone()),
    }
}

#[derive(Debug, Deserialize, Serialize)]
struct RegistryDocument {
    version: u32,
    owners: Vec<StoredOwner>,
}

impl Default for RegistryDocument {
    fn default() -> Self {
        Self {
            version: REGISTRY_VERSION,
            owners: Vec::new(),
        }
    }
}

#[derive(Debug, Deserialize, Serialize)]
struct StoredOwner {
    container_id: String,
    instance_id: String,
}

impl From<&ManagedDockerOwner> for StoredOwner {
    fn from(owner: &ManagedDockerOwner) -> Self {
        Self {
            container_id: owner.container_id().to_owned(),
            instance_id: owner.instance_id().to_owned(),
        }
    }
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
        .with_context(|| format!("failed to inspect registry directory {}", path.display()))?;
    if metadata.file_type().is_symlink() || !metadata.is_dir() {
        bail!(
            "registry directory {} must be a real directory",
            path.display()
        );
    }
    #[cfg(unix)]
    {
        use std::os::unix::fs::{MetadataExt, PermissionsExt};
        if metadata.uid() != unsafe { libc::geteuid() } {
            bail!(
                "registry directory {} is not owned by the current user",
                path.display()
            );
        }
        if metadata.permissions().mode() & 0o777 != 0o700 {
            bail!("registry directory {} must have mode 0700", path.display());
        }
    }
    Ok(())
}

fn ensure_private_regular_file(path: &Path) -> Result<()> {
    let metadata = fs::symlink_metadata(path)
        .with_context(|| format!("failed to inspect registry {}", path.display()))?;
    if metadata.file_type().is_symlink() || !metadata.is_file() {
        bail!(
            "managed Docker registry {} must be a regular file",
            path.display()
        );
    }
    #[cfg(unix)]
    {
        use std::os::unix::fs::{MetadataExt, PermissionsExt};
        if metadata.uid() != unsafe { libc::geteuid() } {
            bail!(
                "managed Docker registry {} is not owned by the current user",
                path.display()
            );
        }
        if metadata.permissions().mode() & 0o777 != 0o600 {
            bail!(
                "managed Docker registry {} must have mode 0600",
                path.display()
            );
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
            Ok(metadata) if metadata.file_type().is_symlink() || !metadata.is_file() => {
                bail!("managed Docker registry lock must be a regular file")
            }
            Ok(_) => ensure_private_regular_file(path)?,
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
            Err(error) => return Err(error).context("failed to inspect registry lock"),
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
            .with_context(|| format!("failed to open registry lock {}", path.display()))?;
        #[cfg(unix)]
        {
            use std::os::fd::AsRawFd;
            let result = unsafe { libc::flock(file.as_raw_fd(), libc::LOCK_EX) };
            if result != 0 {
                return Err(std::io::Error::last_os_error())
                    .context("failed to lock managed Docker registry");
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
        .context("managed Docker registry path has no file name")?
        .to_string_lossy();
    let sequence = TEMPORARY_SEQUENCE.fetch_add(1, Ordering::Relaxed);
    Ok(path.with_file_name(format!(".{name}.tmp-{}-{sequence}", std::process::id())))
}

#[cfg(test)]
mod tests {
    use std::collections::VecDeque;
    use std::sync::Mutex;

    use anyhow::anyhow;
    use tempfile::tempdir;

    use super::*;
    use crate::process::{CommandOutput, CommandRequest};

    const ID_A: &str = "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa";
    const ID_B: &str = "bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb";
    const IMAGE_ID: &str =
        "sha256:cccccccccccccccccccccccccccccccccccccccccccccccccccccccccccccccc";
    const INSTANCE_A: &str = "01234567-89ab-cdef-0123-456789abcdef";
    const INSTANCE_B: &str = "fedcba98-7654-3210-fedc-ba9876543210";

    #[test]
    fn registry_round_trip_is_atomic_and_private() {
        let directory = private_tempdir();
        let registry_dir = directory.path().join("managed-docker");
        fs::create_dir(&registry_dir).unwrap();
        set_mode(&registry_dir, 0o700);
        let path = registry_dir.join("owners.json");
        let mut registry = ManagedDockerRegistry::open(path.clone()).unwrap();

        assert!(registry
            .record(ManagedDockerOwner::new(ID_A, INSTANCE_A).unwrap())
            .unwrap());
        assert!(!registry
            .record(ManagedDockerOwner::new(ID_A, INSTANCE_A).unwrap())
            .unwrap());

        assert_eq!(file_mode(&path), 0o600);
        assert!(fs::read_dir(&registry_dir).unwrap().all(|entry| !entry
            .unwrap()
            .file_name()
            .to_string_lossy()
            .contains(".tmp-")));
        drop(registry);
        let loaded = ManagedDockerRegistry::open(path).unwrap();
        assert_eq!(
            loaded.list(),
            vec![ManagedDockerOwner::new(ID_A, INSTANCE_A).unwrap()]
        );
    }

    #[test]
    fn registry_rejects_group_readable_file() {
        let directory = private_tempdir();
        let path = directory.path().join("owners.json");
        fs::write(&path, r#"{"version":1,"owners":[]}"#).unwrap();
        set_mode(&path, 0o640);

        let error = ManagedDockerRegistry::open(path).unwrap_err();

        assert!(error.to_string().contains("mode 0600"));
    }

    #[test]
    fn registry_rejects_invalid_or_duplicate_records() {
        let directory = private_tempdir();
        let path = directory.path().join("owners.json");
        fs::write(
            &path,
            format!(
                r#"{{"version":1,"owners":[{{"container_id":"{ID_A}","instance_id":"{INSTANCE_A}"}},{{"container_id":"{ID_B}","instance_id":"{INSTANCE_A}"}}]}}"#
            ),
        )
        .unwrap();
        set_mode(&path, 0o600);

        let error = ManagedDockerRegistry::open(path).unwrap_err();

        assert!(error.to_string().contains("assigned to both"));
    }

    #[test]
    fn exact_forget_refuses_instance_mismatch_and_preserves_record() {
        let directory = private_tempdir();
        let path = directory.path().join("owners.json");
        let mut registry = ManagedDockerRegistry::open(path).unwrap();
        registry
            .record(ManagedDockerOwner::new(ID_A, INSTANCE_A).unwrap())
            .unwrap();

        let error = registry.forget_exact(ID_A, INSTANCE_B).unwrap_err();

        assert!(error.to_string().contains("does not match"));
        assert_eq!(registry.list().len(), 1);
    }

    #[test]
    fn service_create_persists_owner_but_never_starts_container() {
        let directory = private_tempdir();
        let workspace = directory.path().join("workspace");
        let state = directory.path().join("state");
        let registry_dir = directory.path().join("registry");
        fs::create_dir(&workspace).unwrap();
        fs::create_dir(&state).unwrap();
        fs::create_dir(&registry_dir).unwrap();
        set_mode(&registry_dir, 0o700);
        let spec = ManagedDockerCreateSpec::new(
            "oav-agent",
            INSTANCE_A,
            format!("sha256:{}", "d".repeat(64)),
            workspace,
            state,
            1000,
            1000,
            "0.1.0",
        )
        .unwrap();
        let runner = fake_runner([
            success(ID_A),
            success(inspect_json(ID_A, false, true, INSTANCE_A)),
        ]);
        let registry_path = registry_dir.join("owners.json");
        let mut service =
            ManagedDockerService::with_runner("docker-test", registry_path.clone(), runner.clone())
                .unwrap();

        let status = service.create(&spec).unwrap();

        assert_eq!(status.state, ManagedDockerState::Stopped);
        assert_eq!(status.container_id, ID_A);
        assert!(service.registry().get(ID_A).is_some());
        assert!(!runner
            .requests()
            .iter()
            .any(|request| request.args.first().map(String::as_str) == Some("start")));
    }

    #[test]
    fn service_enroll_resolves_name_then_revalidates_exact_id() {
        let directory = private_tempdir();
        let path = directory.path().join("owners.json");
        let mut registry = ManagedDockerRegistry::open(path.clone()).unwrap();
        registry
            .record(ManagedDockerOwner::new(ID_A, INSTANCE_A).unwrap())
            .unwrap();
        drop(registry);
        let runner = fake_runner([
            success(inspect_json(ID_A, true, true, INSTANCE_A)),
            success(inspect_json(ID_A, true, true, INSTANCE_A)),
        ]);
        let service =
            ManagedDockerService::with_runner("docker-test", path, runner.clone()).unwrap();

        let managed = service.enroll("friendly-name").unwrap();

        assert_eq!(managed.container().id, ID_A);
        let requests = runner.requests();
        assert_eq!(requests[0].args.last().unwrap(), "friendly-name");
        assert_eq!(requests[1].args.last().unwrap(), ID_A);
    }

    #[test]
    fn list_reports_one_unavailable_container_without_hiding_healthy_records() {
        let directory = private_tempdir();
        let path = directory.path().join("owners.json");
        let mut registry = ManagedDockerRegistry::open(path.clone()).unwrap();
        registry
            .record(ManagedDockerOwner::new(ID_A, INSTANCE_A).unwrap())
            .unwrap();
        registry
            .record(ManagedDockerOwner::new(ID_B, INSTANCE_B).unwrap())
            .unwrap();
        drop(registry);
        let runner = fake_runner([
            success(inspect_json(ID_A, true, true, INSTANCE_A)),
            failed("Error: No such container"),
        ]);
        let service = ManagedDockerService::with_runner("docker-test", path, runner).unwrap();

        let statuses = service.list();

        assert_eq!(statuses.len(), 2);
        assert_eq!(statuses[0].state, ManagedDockerState::Running);
        assert_eq!(statuses[1].state, ManagedDockerState::Unavailable);
        assert!(statuses[1]
            .detail
            .as_deref()
            .unwrap()
            .contains("No such container"));
    }

    #[test]
    fn status_requires_an_exact_registered_id_without_calling_docker() {
        let directory = private_tempdir();
        let path = directory.path().join("owners.json");
        let runner = fake_runner([]);
        let service =
            ManagedDockerService::with_runner("docker-test", path, runner.clone()).unwrap();

        let error = service.status(ID_A).unwrap_err();

        assert!(error.to_string().contains("not registered"));
        assert!(runner.requests().is_empty());
    }

    #[test]
    fn generated_instance_ids_are_valid_version_four_uuids() {
        let first = generate_managed_instance_id().unwrap();
        let second = generate_managed_instance_id().unwrap();

        assert_ne!(first, second);
        assert_eq!(&first[14..15], "4");
        assert!(matches!(&first[19..20], "8" | "9" | "a" | "b"));
        ManagedDockerOwner::new(ID_A, first).unwrap();
    }

    #[test]
    fn service_start_revalidates_before_and_after_the_exact_operation() {
        let directory = private_tempdir();
        let path = directory.path().join("owners.json");
        let mut registry = ManagedDockerRegistry::open(path.clone()).unwrap();
        registry
            .record(ManagedDockerOwner::new(ID_A, INSTANCE_A).unwrap())
            .unwrap();
        drop(registry);
        let runner = fake_runner([
            success(inspect_json(ID_A, false, true, INSTANCE_A)),
            success(inspect_json(ID_A, false, true, INSTANCE_A)),
            success(inspect_json(ID_A, false, true, INSTANCE_A)),
            success(Vec::new()),
            success(inspect_json(ID_A, true, true, INSTANCE_A)),
        ]);
        let service =
            ManagedDockerService::with_runner("docker-test", path, runner.clone()).unwrap();

        let status = service.start("friendly-name").unwrap();

        assert_eq!(status.state, ManagedDockerState::Running);
        assert_eq!(runner.requests()[3].args, vec!["start", ID_A]);
    }

    #[test]
    fn service_remove_forgets_only_after_stopped_exact_removal() {
        let directory = private_tempdir();
        let path = directory.path().join("owners.json");
        let mut registry = ManagedDockerRegistry::open(path.clone()).unwrap();
        registry
            .record(ManagedDockerOwner::new(ID_A, INSTANCE_A).unwrap())
            .unwrap();
        drop(registry);
        let runner = fake_runner([
            success(inspect_json(ID_A, false, true, INSTANCE_A)),
            success(inspect_json(ID_A, false, true, INSTANCE_A)),
            success(inspect_json(ID_A, false, true, INSTANCE_A)),
            success(Vec::new()),
        ]);
        let mut service =
            ManagedDockerService::with_runner("docker-test", path, runner.clone()).unwrap();

        let removed = service.remove("friendly-name").unwrap();

        assert_eq!(removed.container_id(), ID_A);
        assert!(service.registry().list().is_empty());
        assert_eq!(runner.requests()[3].args, vec!["rm", ID_A]);
    }

    fn inspect_json(id: &str, running: bool, managed: bool, instance: &str) -> String {
        let labels = if managed {
            serde_json::json!({
                super::super::ENABLED_LABEL: "true",
                super::super::MANAGED_LABEL: "true",
                super::super::INSTANCE_LABEL: instance,
            })
        } else {
            serde_json::json!({super::super::ENABLED_LABEL: "true"})
        };
        serde_json::json!([{
            "Id": id,
            "Name": "/agent",
            "Image": IMAGE_ID,
            "Config": {
                "Image": "basic-claude-uv",
                "Labels": labels,
                "User": "1000:1000",
                "WorkingDir": "/workspace"
            },
            "State": {
                "Running": running,
                "Status": if running { "running" } else { "created" },
                "Paused": false,
                "Restarting": false
            }
        }])
        .to_string()
    }

    fn success(stdout: impl Into<Vec<u8>>) -> CommandOutput {
        CommandOutput {
            status: 0,
            stdout: stdout.into(),
            stderr: Vec::new(),
        }
    }

    fn failed(stderr: impl Into<Vec<u8>>) -> CommandOutput {
        CommandOutput {
            status: 1,
            stdout: Vec::new(),
            stderr: stderr.into(),
        }
    }

    fn fake_runner(outputs: impl IntoIterator<Item = CommandOutput>) -> Arc<FakeCommandRunner> {
        Arc::new(FakeCommandRunner {
            requests: Mutex::new(Vec::new()),
            outputs: Mutex::new(outputs.into_iter().collect()),
        })
    }

    fn private_tempdir() -> tempfile::TempDir {
        let directory = tempdir().unwrap();
        set_mode(directory.path(), 0o700);
        directory
    }

    struct FakeCommandRunner {
        requests: Mutex<Vec<CommandRequest>>,
        outputs: Mutex<VecDeque<CommandOutput>>,
    }

    impl FakeCommandRunner {
        fn requests(&self) -> Vec<CommandRequest> {
            self.requests.lock().unwrap().clone()
        }
    }

    impl CommandRunner for FakeCommandRunner {
        fn run(&self, request: &CommandRequest) -> Result<CommandOutput> {
            self.requests.lock().unwrap().push(request.clone());
            self.outputs
                .lock()
                .unwrap()
                .pop_front()
                .ok_or_else(|| anyhow!("unexpected command: {request:?}"))
        }
    }

    #[cfg(unix)]
    fn set_mode(path: &Path, mode: u32) {
        use std::os::unix::fs::PermissionsExt;
        fs::set_permissions(path, fs::Permissions::from_mode(mode)).unwrap();
    }

    #[cfg(not(unix))]
    fn set_mode(_: &Path, _: u32) {}

    #[cfg(unix)]
    fn file_mode(path: &Path) -> u32 {
        use std::os::unix::fs::PermissionsExt;
        fs::metadata(path).unwrap().permissions().mode() & 0o777
    }

    #[cfg(not(unix))]
    fn file_mode(_: &Path) -> u32 {
        0o600
    }
}

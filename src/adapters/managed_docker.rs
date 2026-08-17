//! Ownership-gated Docker operations.
//!
//! Docker is a runtime for Claude and Codex, not a provider. This module keeps
//! Docker authority explicit, resolves mutable names to immutable IDs, and
//! revalidates identity before every exec or lifecycle operation. All commands
//! are argument arrays; provider input is never passed through a shell.

use std::collections::BTreeMap;
use std::fs;
use std::path::{Component, Path, PathBuf};
use std::sync::Arc;
use std::time::Duration;

use anyhow::{bail, Context, Result};
use serde::{Deserialize, Serialize};

use crate::process::{CommandOutput, CommandRequest, CommandRunner, ProcessRunner};

pub const ENABLED_LABEL: &str = "io.open-agent-view.enabled";
pub const MANAGED_LABEL: &str = "io.open-agent-view.managed";
pub const INSTANCE_LABEL: &str = "io.open-agent-view.instance";
pub const PROVIDERS_LABEL: &str = "io.open-agent-view.providers";
pub const VERSION_LABEL: &str = "io.open-agent-view.version";

const AGENT_HOME: &str = "/home/agent";
const WORKSPACE: &str = "/workspace";
const DEFAULT_PIDS_LIMIT: u32 = 512;

/// Authority granted to open-agent-view for an enrolled container.
///
/// The variants are ordered so capability checks fail closed. `Managed` adds
/// container lifecycle authority; it is never granted by an allowlist alone.
#[derive(Clone, Copy, Debug, Eq, Ord, PartialEq, PartialOrd)]
pub enum DockerAuthority {
    Metadata,
    Observe,
    Control,
    Managed,
}

/// Immutable metadata returned by `docker inspect`.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct DockerContainer {
    pub id: String,
    pub name: String,
    pub image: String,
    pub image_id: String,
    pub status: String,
    pub running: bool,
    pub paused: bool,
    pub restarting: bool,
    pub user: String,
    pub working_dir: PathBuf,
    pub labels: BTreeMap<String, String>,
}

impl DockerContainer {
    pub fn available_for_exec(&self) -> bool {
        self.running && !self.paused && !self.restarting
    }

    fn label_is_true(&self, key: &str) -> bool {
        self.labels.get(key).map(String::as_str) == Some("true")
    }
}

/// A container explicitly admitted at a bounded authority level.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct EnrolledDockerContainer {
    container: DockerContainer,
    authority: DockerAuthority,
}

impl EnrolledDockerContainer {
    pub fn container(&self) -> &DockerContainer {
        &self.container
    }

    pub fn authority(&self) -> DockerAuthority {
        self.authority
    }
}

/// External ownership proof for a container created by open-agent-view.
///
/// This record belongs in the application's protected state directory. The
/// full immutable ID and random instance ID must both match container metadata
/// before lifecycle authority is honored.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct ManagedDockerOwner {
    container_id: String,
    instance_id: String,
}

impl ManagedDockerOwner {
    pub fn new(container_id: impl Into<String>, instance_id: impl Into<String>) -> Result<Self> {
        let owner = Self {
            container_id: container_id.into(),
            instance_id: instance_id.into(),
        };
        validate_container_id(&owner.container_id)?;
        validate_instance_id(&owner.instance_id)?;
        Ok(owner)
    }

    pub fn container_id(&self) -> &str {
        &self.container_id
    }

    pub fn instance_id(&self) -> &str {
        &self.instance_id
    }
}

/// An enrolled target that has passed both label and external-owner checks.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ManagedDockerContainer {
    enrolled: EnrolledDockerContainer,
    owner: ManagedDockerOwner,
}

impl ManagedDockerContainer {
    pub fn container(&self) -> &DockerContainer {
        self.enrolled.container()
    }

    pub fn owner(&self) -> &ManagedDockerOwner {
        &self.owner
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum DockerProvider {
    Claude,
    Codex,
}

impl DockerProvider {
    fn executable(self) -> &'static str {
        match self {
            Self::Claude => "claude",
            Self::Codex => "codex",
        }
    }
}

/// A provider command to execute without a shell in an enrolled container.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct DockerProviderCommand {
    provider: DockerProvider,
    args: Vec<String>,
    working_dir: Option<PathBuf>,
    required_authority: DockerAuthority,
    timeout: Duration,
}

impl DockerProviderCommand {
    /// The supported read-only Claude session inventory command.
    pub fn claude_agents(include_completed: bool) -> Self {
        let mut args = vec!["agents".into(), "--json".into()];
        if include_completed {
            args.push("--all".into());
        }
        Self {
            provider: DockerProvider::Claude,
            args,
            working_dir: None,
            required_authority: DockerAuthority::Observe,
            timeout: Duration::from_secs(8),
        }
    }

    pub fn control(provider: DockerProvider, args: Vec<String>) -> Self {
        Self {
            provider,
            args,
            working_dir: None,
            required_authority: DockerAuthority::Control,
            timeout: Duration::from_secs(15),
        }
    }

    pub fn in_working_dir(mut self, path: impl Into<PathBuf>) -> Self {
        self.working_dir = Some(path.into());
        self
    }

    pub fn required_authority(&self) -> DockerAuthority {
        self.required_authority
    }
}

/// Validated inputs for creating a new, stopped managed container.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ManagedDockerCreateSpec {
    name: String,
    instance_id: String,
    image: String,
    workspace: PathBuf,
    state_home: PathBuf,
    uid: u32,
    gid: u32,
    network: String,
    version: String,
    pids_limit: u32,
}

impl ManagedDockerCreateSpec {
    #[allow(clippy::too_many_arguments)]
    pub fn new(
        name: impl Into<String>,
        instance_id: impl Into<String>,
        image: impl Into<String>,
        workspace: impl AsRef<Path>,
        state_home: impl AsRef<Path>,
        uid: u32,
        gid: u32,
        version: impl Into<String>,
    ) -> Result<Self> {
        let name = name.into();
        let instance_id = instance_id.into();
        let image = image.into();
        let version = version.into();
        validate_container_name(&name)?;
        validate_instance_id(&instance_id)?;
        validate_pinned_image(&image)?;
        validate_label_value(&version, "version")?;

        let workspace = canonical_directory(workspace.as_ref(), "workspace")?;
        let state_home = canonical_directory(state_home.as_ref(), "state home")?;
        validate_mount_source(&workspace, "workspace")?;
        validate_mount_source(&state_home, "state home")?;
        if workspace.starts_with(&state_home) || state_home.starts_with(&workspace) {
            bail!("workspace and state home must not contain one another");
        }

        Ok(Self {
            name,
            instance_id,
            image,
            workspace,
            state_home,
            uid,
            gid,
            network: "bridge".into(),
            version,
            pids_limit: DEFAULT_PIDS_LIMIT,
        })
    }

    pub fn with_network(mut self, network: impl Into<String>) -> Result<Self> {
        let network = network.into();
        validate_docker_token(&network, "network")?;
        if network.starts_with("container:") || network == "host" {
            bail!("host and container-shared networking are not allowed for managed agents");
        }
        self.network = network;
        Ok(self)
    }

    pub fn with_pids_limit(mut self, limit: u32) -> Result<Self> {
        if limit == 0 {
            bail!("PID limit must be greater than zero");
        }
        self.pids_limit = limit;
        Ok(self)
    }

    pub fn instance_id(&self) -> &str {
        &self.instance_id
    }

    pub fn create_request(&self, docker_executable: &str) -> CommandRequest {
        let labels = [
            (ENABLED_LABEL, "true"),
            (MANAGED_LABEL, "true"),
            (INSTANCE_LABEL, self.instance_id.as_str()),
            (PROVIDERS_LABEL, "claude,codex"),
            (VERSION_LABEL, self.version.as_str()),
        ];
        let mut args = vec!["create".into(), "--name".into(), self.name.clone()];
        for (key, value) in labels {
            args.extend(["--label".into(), format!("{key}={value}")]);
        }
        args.extend([
            "--init".into(),
            "--cap-drop".into(),
            "ALL".into(),
            "--security-opt".into(),
            "no-new-privileges".into(),
            "--pids-limit".into(),
            self.pids_limit.to_string(),
            "--user".into(),
            format!("{}:{}", self.uid, self.gid),
            "--env".into(),
            format!("HOME={AGENT_HOME}"),
            "--workdir".into(),
            WORKSPACE.into(),
            "--mount".into(),
            bind_mount_argument(&self.workspace, WORKSPACE),
            "--mount".into(),
            bind_mount_argument(&self.state_home, AGENT_HOME),
            "--network".into(),
            self.network.clone(),
            self.image.clone(),
            "sleep".into(),
            "infinity".into(),
        ]);
        let mut request = CommandRequest::new(docker_executable, args);
        request.timeout = Duration::from_secs(30);
        request
    }
}

/// Docker operations backed by an injected command runner.
///
/// Production uses `ProcessRunner`; tests can inspect every planned argv and
/// return fixture outputs without contacting a Docker daemon.
pub struct ManagedDockerRuntime {
    docker_executable: String,
    runner: Arc<dyn CommandRunner>,
}

impl ManagedDockerRuntime {
    pub fn new(docker_executable: impl Into<String>) -> Self {
        Self::with_runner(docker_executable, Arc::new(ProcessRunner))
    }

    pub fn with_runner(
        docker_executable: impl Into<String>,
        runner: Arc<dyn CommandRunner>,
    ) -> Self {
        Self {
            docker_executable: docker_executable.into(),
            runner,
        }
    }

    /// Enroll an exact user-specified name/ID. Explicit enrollment can grant
    /// provider control but never container lifecycle authority.
    pub fn enroll_explicit(
        &self,
        reference: &str,
        authority: DockerAuthority,
    ) -> Result<EnrolledDockerContainer> {
        if authority == DockerAuthority::Managed {
            bail!("explicit enrollment alone cannot grant managed lifecycle authority");
        }
        let container = self.inspect(reference)?;
        Ok(EnrolledDockerContainer {
            container,
            authority,
        })
    }

    /// Discover label-opted-in containers at observe authority. A managed
    /// label does not grant managed authority without an external owner record.
    pub fn discover_enabled(&self) -> Result<Vec<EnrolledDockerContainer>> {
        let mut request = CommandRequest::new(
            self.docker_executable.clone(),
            vec![
                "ps".into(),
                "-a".into(),
                "--no-trunc".into(),
                "--filter".into(),
                format!("label={ENABLED_LABEL}=true"),
                "--format".into(),
                "{{.ID}}".into(),
            ],
        );
        request.timeout = Duration::from_secs(5);
        let output = self.run_success(&request, "docker label discovery")?;
        let ids = output.stdout_text()?;
        let mut enrolled = Vec::new();
        for id in ids.lines().map(str::trim).filter(|line| !line.is_empty()) {
            validate_container_id(id).context("Docker returned a non-immutable container ID")?;
            let container = self.inspect(id)?;
            if !container.label_is_true(ENABLED_LABEL) {
                bail!("container {} no longer has the opt-in label", container.name);
            }
            enrolled.push(EnrolledDockerContainer {
                container,
                authority: DockerAuthority::Observe,
            });
        }
        Ok(enrolled)
    }

    /// Establish managed authority by matching an immutable ID, opt-in labels,
    /// a random instance ID, and a protected external owner record.
    pub fn enroll_managed(
        &self,
        reference: &str,
        owner: ManagedDockerOwner,
    ) -> Result<ManagedDockerContainer> {
        let container = self.inspect(reference)?;
        verify_managed_identity(&container, &owner)?;
        Ok(ManagedDockerContainer {
            enrolled: EnrolledDockerContainer {
                container,
                authority: DockerAuthority::Managed,
            },
            owner,
        })
    }

    /// Create, but do not start, a managed container. The returned owner record
    /// must be persisted before lifecycle operations are exposed.
    pub fn create_managed(
        &self,
        spec: &ManagedDockerCreateSpec,
    ) -> Result<ManagedDockerContainer> {
        let request = spec.create_request(&self.docker_executable);
        let output = self.run_success(&request, "docker create")?;
        let id = output.stdout_text()?.trim();
        validate_container_id(id).context(
            "docker create succeeded but did not return a full immutable container ID",
        )?;
        let owner = ManagedDockerOwner::new(id, spec.instance_id())?;
        let managed = self.enroll_managed(id, owner).with_context(|| {
            format!(
                "created container {id} but could not verify ownership; it was left stopped"
            )
        })?;
        if managed.container().running {
            bail!("newly created managed container unexpectedly reports running");
        }
        Ok(managed)
    }

    /// Execute a Claude/Codex argv after immutable-ID and authority checks.
    pub fn exec_provider(
        &self,
        target: &EnrolledDockerContainer,
        command: &DockerProviderCommand,
    ) -> Result<CommandOutput> {
        if command.required_authority == DockerAuthority::Metadata
            || command.required_authority == DockerAuthority::Managed
        {
            bail!("provider commands must require observe or control authority");
        }
        require_authority(target.authority, command.required_authority)?;
        validate_provider_command(command)?;
        let current = self.reinspect_enrolled(target)?;
        if !current.available_for_exec() {
            bail!(
                "container {} is {} and unavailable for exec",
                current.name,
                current.status
            );
        }

        let mut args = vec!["exec".into()];
        if let Some(path) = &command.working_dir {
            args.extend(["--workdir".into(), path.to_string_lossy().into_owned()]);
        }
        args.push(current.id);
        args.push(command.provider.executable().into());
        args.extend(command.args.clone());
        let mut request = CommandRequest::new(self.docker_executable.clone(), args);
        request.timeout = command.timeout;
        self.run_success(&request, "docker provider exec")
    }

    pub fn start_managed(&self, target: &ManagedDockerContainer) -> Result<()> {
        let current = self.reinspect_managed(target)?;
        if current.running {
            bail!("managed container {} is already running", current.name);
        }
        let request = CommandRequest::new(
            self.docker_executable.clone(),
            vec!["start".into(), current.id],
        );
        self.run_success(&request, "docker start")?;
        Ok(())
    }

    pub fn stop_managed(&self, target: &ManagedDockerContainer) -> Result<()> {
        let current = self.reinspect_managed(target)?;
        if !current.running {
            bail!("managed container {} is not running", current.name);
        }
        let mut request = CommandRequest::new(
            self.docker_executable.clone(),
            vec!["stop".into(), "--time".into(), "10".into(), current.id],
        );
        request.timeout = Duration::from_secs(20);
        self.run_success(&request, "docker stop")?;
        Ok(())
    }

    /// Remove only an already-stopped, verified managed container. No force or
    /// volume-removal flags are used, so persistent state is retained.
    pub fn remove_managed(&self, target: &ManagedDockerContainer) -> Result<()> {
        let current = self.reinspect_managed(target)?;
        if current.running {
            bail!("refusing to remove running managed container {}", current.name);
        }
        let request = CommandRequest::new(
            self.docker_executable.clone(),
            vec!["rm".into(), current.id],
        );
        self.run_success(&request, "docker rm")?;
        Ok(())
    }

    fn inspect(&self, reference: &str) -> Result<DockerContainer> {
        validate_container_reference(reference)?;
        let mut request = CommandRequest::new(
            self.docker_executable.clone(),
            vec![
                "inspect".into(),
                "--type".into(),
                "container".into(),
                reference.into(),
            ],
        );
        request.timeout = Duration::from_secs(5);
        let output = self.run_success(&request, "docker inspect")?;
        parse_inspect(output.stdout_text()?)
    }

    fn reinspect_enrolled(&self, target: &EnrolledDockerContainer) -> Result<DockerContainer> {
        let current = self.inspect(&target.container.id)?;
        if current.id != target.container.id {
            bail!("container identity changed during revalidation");
        }
        Ok(current)
    }

    fn reinspect_managed(&self, target: &ManagedDockerContainer) -> Result<DockerContainer> {
        let current = self.reinspect_enrolled(&target.enrolled)?;
        verify_managed_identity(&current, &target.owner)?;
        Ok(current)
    }

    fn run_success(&self, request: &CommandRequest, operation: &str) -> Result<CommandOutput> {
        let output = self.runner.run(request)?;
        if output.status != 0 {
            bail!(
                "{operation} exited with status {}: {}",
                output.status,
                output.stderr_lossy()
            );
        }
        Ok(output)
    }
}

fn parse_inspect(input: &str) -> Result<DockerContainer> {
    let mut records: Vec<InspectRecord> =
        serde_json::from_str(input).context("invalid docker inspect response")?;
    if records.len() != 1 {
        bail!("docker inspect returned {} records; expected one", records.len());
    }
    let record = records.pop().expect("length checked");
    validate_container_id(&record.id)?;
    if record.name.trim_start_matches('/').is_empty() {
        bail!("docker inspect returned an empty container name");
    }
    Ok(DockerContainer {
        id: record.id,
        name: record.name.trim_start_matches('/').to_owned(),
        image: record.config.image,
        image_id: record.image,
        status: record.state.status,
        running: record.state.running,
        paused: record.state.paused,
        restarting: record.state.restarting,
        user: record.config.user,
        working_dir: PathBuf::from(record.config.working_dir),
        labels: record.config.labels.unwrap_or_default(),
    })
}

fn verify_managed_identity(
    container: &DockerContainer,
    owner: &ManagedDockerOwner,
) -> Result<()> {
    validate_container_id(&owner.container_id).context("invalid external owner record")?;
    validate_instance_id(&owner.instance_id).context("invalid external owner record")?;
    if container.id != owner.container_id {
        bail!("external owner record does not match the immutable container ID");
    }
    if !container.label_is_true(ENABLED_LABEL) || !container.label_is_true(MANAGED_LABEL) {
        bail!("container is not labeled as open-agent-view managed");
    }
    if container.labels.get(INSTANCE_LABEL) != Some(&owner.instance_id) {
        bail!("container instance label does not match the external owner record");
    }
    Ok(())
}

fn require_authority(actual: DockerAuthority, required: DockerAuthority) -> Result<()> {
    if actual < required {
        bail!("Docker target has {actual:?} authority but {required:?} is required");
    }
    Ok(())
}

fn validate_provider_command(command: &DockerProviderCommand) -> Result<()> {
    for argument in &command.args {
        if argument.as_bytes().contains(&0) {
            bail!("provider arguments cannot contain NUL");
        }
    }
    if let Some(path) = &command.working_dir {
        validate_container_path(path, "provider working directory")?;
    }
    Ok(())
}

fn canonical_directory(path: &Path, label: &str) -> Result<PathBuf> {
    let canonical = fs::canonicalize(path)
        .with_context(|| format!("failed to resolve {label} {}", path.display()))?;
    if !canonical.is_dir() {
        bail!("{label} {} is not a directory", canonical.display());
    }
    Ok(canonical)
}

fn validate_mount_source(path: &Path, label: &str) -> Result<()> {
    let text = path
        .to_str()
        .with_context(|| format!("{label} path is not valid UTF-8"))?;
    if text.contains(',') || text.contains('\n') || text.contains('\r') {
        bail!("{label} path contains a character unsupported by Docker --mount");
    }
    Ok(())
}

fn validate_container_path(path: &Path, label: &str) -> Result<()> {
    if !path.is_absolute() || path == Path::new("/") {
        bail!("{label} must be an absolute non-root path");
    }
    if path.components().any(|part| matches!(part, Component::ParentDir)) {
        bail!("{label} cannot contain parent traversal");
    }
    let text = path
        .to_str()
        .with_context(|| format!("{label} is not valid UTF-8"))?;
    if text.as_bytes().contains(&0) {
        bail!("{label} cannot contain NUL");
    }
    Ok(())
}

fn validate_container_id(id: &str) -> Result<()> {
    if id.len() != 64 || !id.bytes().all(|byte| byte.is_ascii_hexdigit()) {
        bail!("container ID must be a full 64-character hexadecimal ID");
    }
    Ok(())
}

fn validate_container_reference(reference: &str) -> Result<()> {
    if reference.len() == 64 && reference.bytes().all(|byte| byte.is_ascii_hexdigit()) {
        return Ok(());
    }
    validate_container_name(reference)
        .context("Docker container reference must be a name or full immutable ID")
}

fn validate_instance_id(id: &str) -> Result<()> {
    let bytes = id.as_bytes();
    if bytes.len() != 36
        || [8, 13, 18, 23].iter().any(|index| bytes[*index] != b'-')
        || bytes.iter().enumerate().any(|(index, byte)| {
            ![8, 13, 18, 23].contains(&index) && !byte.is_ascii_hexdigit()
        })
    {
        bail!("instance ID must be a hyphenated UUID");
    }
    Ok(())
}

fn validate_container_name(name: &str) -> Result<()> {
    if name.is_empty()
        || name.len() > 128
        || !name
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'_' | b'.' | b'-'))
        || !name.as_bytes()[0].is_ascii_alphanumeric()
    {
        bail!("container name contains unsupported characters");
    }
    Ok(())
}

fn validate_pinned_image(image: &str) -> Result<()> {
    let digest = if let Some(value) = image.strip_prefix("sha256:") {
        value
    } else if let Some((_, value)) = image.rsplit_once("@sha256:") {
        value
    } else {
        bail!("managed container image must be pinned by sha256 digest or image ID");
    };
    if digest.len() != 64 || !digest.bytes().all(|byte| byte.is_ascii_hexdigit()) {
        bail!("managed container image has an invalid sha256 digest");
    }
    Ok(())
}

fn validate_label_value(value: &str, label: &str) -> Result<()> {
    if value.is_empty()
        || value.len() > 128
        || value
            .bytes()
            .any(|byte| byte == 0 || byte == b'\n' || byte == b'\r')
    {
        bail!("{label} is not a safe Docker label value");
    }
    Ok(())
}

fn validate_docker_token(value: &str, label: &str) -> Result<()> {
    if value.is_empty()
        || value.len() > 128
        || value
            .bytes()
            .any(|byte| !(byte.is_ascii_alphanumeric() || matches!(byte, b'_' | b'.' | b'-')))
    {
        bail!("{label} contains unsupported characters");
    }
    Ok(())
}

fn bind_mount_argument(source: &Path, destination: &str) -> String {
    format!("type=bind,src={},dst={destination}", source.display())
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "PascalCase")]
struct InspectRecord {
    id: String,
    name: String,
    image: String,
    config: InspectConfig,
    state: InspectState,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "PascalCase")]
struct InspectConfig {
    image: String,
    #[serde(default)]
    labels: Option<BTreeMap<String, String>>,
    #[serde(default)]
    user: String,
    #[serde(default)]
    working_dir: String,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "PascalCase")]
struct InspectState {
    running: bool,
    status: String,
    #[serde(default)]
    paused: bool,
    #[serde(default)]
    restarting: bool,
}

#[cfg(test)]
mod tests {
    use std::collections::VecDeque;
    use std::sync::Mutex;

    use anyhow::anyhow;
    use tempfile::tempdir;

    use super::*;

    const ID_A: &str = "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa";
    const ID_B: &str = "bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb";
    const IMAGE_ID: &str =
        "sha256:cccccccccccccccccccccccccccccccccccccccccccccccccccccccccccccccc";
    const INSTANCE: &str = "01234567-89ab-cdef-0123-456789abcdef";

    #[test]
    fn creation_plan_is_pinned_hardened_and_argument_safe() {
        let directory = tempdir().unwrap();
        let workspace = directory.path().join("workspace with spaces;$(touch nope)");
        let state = directory.path().join("state");
        fs::create_dir_all(&workspace).unwrap();
        fs::create_dir_all(&state).unwrap();
        let spec = ManagedDockerCreateSpec::new(
            "oav-agent-1",
            INSTANCE,
            format!("basic-claude-uv@{IMAGE_ID}"),
            &workspace,
            &state,
            1000,
            1000,
            "0.1.0",
        )
        .unwrap();

        let request = spec.create_request("docker-test");

        assert_eq!(request.program, "docker-test");
        assert_eq!(request.args[0], "create");
        assert!(request.args.windows(2).any(|pair| pair == ["--init", "--cap-drop"]));
        assert!(request.args.windows(2).any(|pair| pair == ["--cap-drop", "ALL"]));
        assert!(request
            .args
            .windows(2)
            .any(|pair| pair == ["--security-opt", "no-new-privileges"]));
        assert!(request
            .args
            .contains(&format!("{INSTANCE_LABEL}={INSTANCE}")));
        assert!(request.args.iter().any(|argument| {
            argument.starts_with("type=bind,src=") && argument.contains("$(touch nope)")
        }));
        assert!(!request.args.iter().any(|argument| argument == "sh" || argument == "-c"));
        assert_eq!(&request.args[request.args.len() - 2..], ["sleep", "infinity"]);
    }

    #[test]
    fn creation_rejects_mutable_image_tags() {
        let directory = tempdir().unwrap();
        let workspace = directory.path().join("workspace");
        let state = directory.path().join("state");
        fs::create_dir_all(&workspace).unwrap();
        fs::create_dir_all(&state).unwrap();

        let error = ManagedDockerCreateSpec::new(
            "oav-agent",
            INSTANCE,
            "basic-claude-uv:latest",
            workspace,
            state,
            1000,
            1000,
            "0.1.0",
        )
        .unwrap_err();

        assert!(error.to_string().contains("pinned"));
    }

    #[test]
    fn explicit_enrollment_resolves_to_full_immutable_id() {
        let runner = fake_runner([success(inspect_json(ID_A, true, false, INSTANCE))]);
        let runtime = ManagedDockerRuntime::with_runner("docker-test", runner.clone());

        let enrolled = runtime
            .enroll_explicit("friendly-name", DockerAuthority::Control)
            .unwrap();

        assert_eq!(enrolled.container().id, ID_A);
        assert_eq!(enrolled.authority(), DockerAuthority::Control);
        assert_eq!(runner.requests()[0].args.last().unwrap(), "friendly-name");
    }

    #[test]
    fn label_discovery_uses_only_the_opt_in_filter_and_observe_authority() {
        let runner = fake_runner([
            success(format!("{ID_A}\n")),
            success(inspect_json(ID_A, true, true, INSTANCE)),
        ]);
        let runtime = ManagedDockerRuntime::with_runner("docker-test", runner.clone());

        let enrolled = runtime.discover_enabled().unwrap();

        assert_eq!(enrolled.len(), 1);
        assert_eq!(enrolled[0].authority(), DockerAuthority::Observe);
        assert_eq!(enrolled[0].container().id, ID_A);
        assert_eq!(
            runner.requests()[0].args,
            vec![
                "ps",
                "-a",
                "--no-trunc",
                "--filter",
                "label=io.open-agent-view.enabled=true",
                "--format",
                "{{.ID}}"
            ]
        );
    }

    #[test]
    fn explicit_enrollment_rejects_option_like_references_before_docker() {
        let runner = fake_runner([]);
        let runtime = ManagedDockerRuntime::with_runner("docker-test", runner.clone());

        let error = runtime
            .enroll_explicit("--format", DockerAuthority::Observe)
            .unwrap_err();

        assert!(error.to_string().contains("container reference"));
        assert!(runner.requests().is_empty());
    }

    #[test]
    fn observe_api_exposes_only_the_supported_claude_inventory() {
        let command = DockerProviderCommand::claude_agents(true)
            .in_working_dir("/workspace/project");

        assert_eq!(command.provider, DockerProvider::Claude);
        assert_eq!(command.args, vec!["agents", "--json", "--all"]);
        assert_eq!(command.required_authority(), DockerAuthority::Observe);
    }

    #[test]
    fn provider_exec_revalidates_and_never_uses_a_shell() {
        let runner = fake_runner([
            success(inspect_json(ID_A, true, false, INSTANCE)),
            success(inspect_json(ID_A, true, false, INSTANCE)),
            success("[]"),
        ]);
        let runtime = ManagedDockerRuntime::with_runner("docker-test", runner.clone());
        let target = runtime
            .enroll_explicit("agent", DockerAuthority::Control)
            .unwrap();
        let command = DockerProviderCommand::control(
            DockerProvider::Claude,
            vec!["--background".into(), "fix it; $(touch /tmp/nope)".into()],
        )
        .in_working_dir("/workspace/project");

        runtime.exec_provider(&target, &command).unwrap();

        let requests = runner.requests();
        assert_eq!(requests.len(), 3);
        assert_eq!(requests[1].args.last().unwrap(), ID_A);
        assert_eq!(
            requests[2].args,
            vec![
                "exec",
                "--workdir",
                "/workspace/project",
                ID_A,
                "claude",
                "--background",
                "fix it; $(touch /tmp/nope)"
            ]
        );
        assert!(!requests[2].args.iter().any(|argument| argument == "sh" || argument == "-c"));
    }

    #[test]
    fn provider_control_is_denied_to_observe_only_target_without_exec() {
        let runner = fake_runner([success(inspect_json(ID_A, true, false, INSTANCE))]);
        let runtime = ManagedDockerRuntime::with_runner("docker-test", runner.clone());
        let target = runtime
            .enroll_explicit("agent", DockerAuthority::Observe)
            .unwrap();

        let error = runtime
            .exec_provider(
                &target,
                &DockerProviderCommand::control(DockerProvider::Codex, vec!["resume".into()]),
            )
            .unwrap_err();

        assert!(error.to_string().contains("Control is required"));
        assert_eq!(runner.requests().len(), 1);
    }

    #[test]
    fn managed_enrollment_requires_labels_and_external_owner() {
        let runner = fake_runner([success(inspect_json(ID_A, false, false, INSTANCE))]);
        let runtime = ManagedDockerRuntime::with_runner("docker-test", runner.clone());
        let owner = ManagedDockerOwner::new(ID_A, INSTANCE).unwrap();

        let error = runtime.enroll_managed(ID_A, owner).unwrap_err();

        assert!(error.to_string().contains("not labeled"));
        assert_eq!(runner.requests().len(), 1);
    }

    #[test]
    fn identity_change_blocks_lifecycle_before_start() {
        let runner = fake_runner([
            success(inspect_json(ID_A, false, true, INSTANCE)),
            success(inspect_json(ID_B, false, true, INSTANCE)),
        ]);
        let runtime = ManagedDockerRuntime::with_runner("docker-test", runner.clone());
        let owner = ManagedDockerOwner::new(ID_A, INSTANCE).unwrap();
        let managed = runtime.enroll_managed(ID_A, owner).unwrap();

        let error = runtime.start_managed(&managed).unwrap_err();

        assert!(error.to_string().contains("identity changed"));
        assert_eq!(runner.requests().len(), 2);
        assert!(!runner.requests().iter().any(|request| request.args[0] == "start"));
    }

    #[test]
    fn verified_managed_start_targets_only_the_full_id() {
        let runner = fake_runner([
            success(inspect_json(ID_A, false, true, INSTANCE)),
            success(inspect_json(ID_A, false, true, INSTANCE)),
            success(ID_A),
        ]);
        let runtime = ManagedDockerRuntime::with_runner("docker-test", runner.clone());
        let owner = ManagedDockerOwner::new(ID_A, INSTANCE).unwrap();
        let managed = runtime.enroll_managed(ID_A, owner).unwrap();

        runtime.start_managed(&managed).unwrap();

        let requests = runner.requests();
        assert_eq!(requests[2].args, vec!["start", ID_A]);
    }

    #[test]
    fn remove_refuses_a_running_managed_container_without_rm() {
        let runner = fake_runner([
            success(inspect_json(ID_A, true, true, INSTANCE)),
            success(inspect_json(ID_A, true, true, INSTANCE)),
        ]);
        let runtime = ManagedDockerRuntime::with_runner("docker-test", runner.clone());
        let owner = ManagedDockerOwner::new(ID_A, INSTANCE).unwrap();
        let managed = runtime.enroll_managed(ID_A, owner).unwrap();

        let error = runtime.remove_managed(&managed).unwrap_err();

        assert!(error.to_string().contains("refusing to remove running"));
        assert!(!runner.requests().iter().any(|request| request.args[0] == "rm"));
    }

    #[test]
    fn create_does_not_implicitly_start_container() {
        let directory = tempdir().unwrap();
        let workspace = directory.path().join("workspace");
        let state = directory.path().join("state");
        fs::create_dir_all(&workspace).unwrap();
        fs::create_dir_all(&state).unwrap();
        let spec = ManagedDockerCreateSpec::new(
            "oav-agent",
            INSTANCE,
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
            success(inspect_json(ID_A, false, true, INSTANCE)),
        ]);
        let runtime = ManagedDockerRuntime::with_runner("docker-test", runner.clone());

        let managed = runtime.create_managed(&spec).unwrap();

        assert_eq!(managed.owner().container_id(), ID_A);
        assert_eq!(runner.requests().len(), 2);
        assert_eq!(runner.requests()[0].args[0], "create");
        assert!(!runner.requests().iter().any(|request| request.args[0] == "start"));
    }

    fn inspect_json(id: &str, running: bool, managed: bool, instance: &str) -> String {
        let labels = if managed {
            serde_json::json!({
                ENABLED_LABEL: "true",
                MANAGED_LABEL: "true",
                INSTANCE_LABEL: instance,
            })
        } else {
            serde_json::json!({ENABLED_LABEL: "true"})
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

    fn fake_runner(
        outputs: impl IntoIterator<Item = CommandOutput>,
    ) -> Arc<FakeCommandRunner> {
        Arc::new(FakeCommandRunner {
            requests: Mutex::new(Vec::new()),
            outputs: Mutex::new(outputs.into_iter().collect()),
        })
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
}

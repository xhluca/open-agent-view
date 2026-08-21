use std::io::{self, IsTerminal, Write};
use std::path::PathBuf;
use std::process::{Command, Stdio};
use std::sync::Arc;
use std::time::{Duration, SystemTime};

use anyhow::{bail, Context, Result};
use clap::{Parser, Subcommand, ValueEnum};

use open_agent_view::adapters::{
    default_managed_docker_registry_path, default_pi_session_dir, generate_managed_instance_id,
    AntigravityController, AntigravityOwnership, AntigravitySource, ClaudeSource, CodexSource,
    CopilotController, CopilotSource, CopilotSupervisor, CursorController, DiscoveryEngine,
    DiscoveryRequest, DockerTarget, FixtureSource, ManagedDockerCreateSpec, ManagedDockerService,
    ManagedDockerStatus, OpenCodeController, OpenCodeSource, PiController, PiSource,
};
#[cfg(target_os = "linux")]
use open_agent_view::adapters::{CursorSource, CursorSupervisor};
use open_agent_view::aliases::{SessionAliasRecord, SessionAliases};
use open_agent_view::control::{ControlHub, ControlHubConfig};
use open_agent_view::doctor::{diagnose, render_text};
use open_agent_view::domain::Provider;
use open_agent_view::hidden::{HiddenSessionRecord, HiddenSessions};
use open_agent_view::maintenance::{
    execute_completed_archive, plan_completed_archive, BulkArchiveReport,
};
#[cfg(target_os = "linux")]
use open_agent_view::opencode_supervisor::OpenCodeSupervisor;
use open_agent_view::pi_supervisor::run_pi_supervisor_daemon;
#[cfg(target_os = "linux")]
use open_agent_view::pi_supervisor::PiSupervisor;
use open_agent_view::terminal::run_dashboard;

#[derive(Clone, Copy, Debug, Eq, PartialEq, ValueEnum)]
enum LaunchProvider {
    Claude,
    Codex,
    Pi,
    #[value(name = "opencode")]
    OpenCode,
    Cursor,
    Copilot,
    Antigravity,
}

#[derive(Clone, Debug, Eq, PartialEq, Subcommand)]
enum Commands {
    /// Download, verify, and install the latest Open Agent View release.
    #[command(alias = "upgrade")]
    Update,
    /// Check provider, Docker, and target availability without changing them.
    Doctor,
    /// Create and control only containers owned by Open Agent View.
    Docker {
        #[command(subcommand)]
        command: DockerCommand,
    },
    /// Preview or perform capability-gated maintenance on provider sessions.
    Sessions {
        #[command(subcommand)]
        command: SessionCommand,
    },
    /// Install one coding-agent harness with its official installer.
    Setup {
        #[arg(value_name = "HARNESS", value_enum)]
        harness: LaunchProvider,
        /// Skip the explicit installer confirmation prompt.
        #[arg(long)]
        yes: bool,
    },
    #[command(name = "__pi-supervisor", hide = true)]
    PiSupervisor {
        #[arg(long)]
        state_dir: PathBuf,
        #[arg(long)]
        socket: PathBuf,
    },
}

#[derive(Clone, Debug, Eq, PartialEq, Subcommand)]
enum SessionCommand {
    /// Archive a bounded batch of exact OAV-owned completed Codex sessions.
    Archive {
        /// Limit candidates to sessions under this directory.
        #[arg(long, value_name = "PATH")]
        cwd: Option<PathBuf>,
        /// Limit candidates to sessions last updated at least this many days ago.
        #[arg(long, value_parser = clap::value_parser!(u64).range(1..=365_000))]
        older_than_days: Option<u64>,
        /// Maximum sessions to archive in this invocation.
        #[arg(long, default_value_t = 100, value_parser = clap::value_parser!(u64).range(1..=1_100))]
        limit: u64,
        /// Perform the planned mutations; omission is a read-only dry run.
        #[arg(long)]
        yes: bool,
    },
    /// Hide one exact normalized session ID locally without changing provider history.
    Hide {
        /// Stable ID from the dashboard Peek view or `open-agent-view --json`.
        #[arg(value_name = "SESSION_ID")]
        session_id: String,
    },
    /// Reveal one locally hidden session ID again.
    Unhide {
        #[arg(value_name = "SESSION_ID")]
        session_id: String,
    },
    /// List session IDs hidden only from Open Agent View.
    Hidden,
    /// Set a private Open Agent View display name without renaming the provider session.
    Rename {
        /// Stable normalized session ID from Peek or JSON output.
        #[arg(value_name = "SESSION_ID")]
        session_id: String,
        /// Local display name. Provider history is not changed.
        #[arg(value_name = "NAME")]
        name: String,
    },
    /// Clear a private display name and follow the latest provider title again.
    ResetName {
        #[arg(value_name = "SESSION_ID")]
        session_id: String,
    },
    /// List private display-name overrides.
    Aliases,
}

#[derive(Clone, Debug, Eq, PartialEq, Subcommand)]
enum DockerCommand {
    /// List containers in the protected managed-container registry.
    List,
    /// Inspect one registered managed container by name or immutable ID.
    Status { container: String },
    /// Create a hardened, stopped container from a digest-pinned image.
    Create {
        #[arg(long)]
        name: String,
        #[arg(long)]
        image: String,
        #[arg(long)]
        workspace: PathBuf,
        #[arg(long = "state-home")]
        state_home: PathBuf,
        #[arg(long, default_value = "bridge")]
        network: String,
        #[arg(long)]
        uid: Option<u32>,
        #[arg(long)]
        gid: Option<u32>,
    },
    /// Start one exact registered managed container.
    Start { container: String },
    /// Stop one exact registered managed container after confirmation.
    Stop {
        container: String,
        #[arg(long)]
        yes: bool,
    },
    /// Remove one stopped managed container without removing its volumes.
    Remove {
        container: String,
        #[arg(long)]
        yes: bool,
    },
}

/// Open terminal dashboard for all your coding agents.
#[derive(Debug, Parser)]
#[command(name = "open-agent-view", version, about, disable_version_flag = true)]
struct Cli {
    /// Print the Open Agent View version.
    #[arg(
        short = 'v',
        short_alias = 'V',
        long,
        action = clap::ArgAction::Version
    )]
    version: Option<bool>,

    #[command(subcommand)]
    command: Option<Commands>,

    /// Print machine-readable JSON instead of the interactive or text view.
    #[arg(long, global = true)]
    json: bool,

    /// Compatibility flag; completed sessions are shown by default.
    #[arg(long)]
    all: bool,

    /// Hide completed sessions at startup (alias: --active-only).
    #[arg(long, visible_alias = "active-only", conflicts_with = "all")]
    hide_completed: bool,

    /// Include foreground interactive sessions as well as background agents.
    #[arg(long)]
    include_interactive: bool,

    /// Include provider sessions that were not created or managed by Open Agent View.
    #[arg(long)]
    include_external: bool,

    /// Maximum persisted-history records read from each provider per refresh.
    #[arg(long, default_value_t = 100, value_parser = clap::value_parser!(u64).range(1..=10_000))]
    history_limit: u64,

    /// Show only sessions started under this working directory.
    #[arg(long, value_name = "PATH")]
    cwd: Option<PathBuf>,

    /// Read a normalized JSON fixture instead of probing installed providers.
    #[arg(long, value_name = "FILE")]
    fixture: Option<PathBuf>,

    /// Disable every host provider while retaining explicit Docker targets.
    #[arg(long)]
    no_host_providers: bool,

    /// Claude executable used for host discovery.
    #[arg(long, default_value = "claude", value_name = "PATH", global = true)]
    claude_bin: String,

    /// Disable Claude discovery on the host.
    #[arg(long)]
    no_host_claude: bool,

    /// Codex executable used for host discovery through App Server.
    #[arg(long, default_value = "codex", value_name = "PATH", global = true)]
    codex_bin: String,

    /// Disable Codex discovery on the host.
    #[arg(long)]
    no_host_codex: bool,

    /// Pi executable used to open persisted host sessions.
    #[arg(long, default_value = "pi", value_name = "PATH", global = true)]
    pi_bin: String,

    /// Override Pi's persisted session directory.
    #[arg(long, value_name = "PATH", global = true)]
    pi_session_dir: Option<PathBuf>,

    /// Disable Pi discovery on the host.
    #[arg(long)]
    no_host_pi: bool,

    /// OpenCode executable used for host discovery and native resume.
    #[arg(long, default_value = "opencode", value_name = "PATH", global = true)]
    opencode_bin: String,

    /// Disable OpenCode discovery on the host.
    #[arg(long)]
    no_host_opencode: bool,

    /// GitHub Copilot CLI executable used for ACP session discovery.
    #[arg(long, default_value = "copilot", value_name = "PATH", global = true)]
    copilot_bin: String,

    /// Disable GitHub Copilot discovery on the host.
    #[arg(long)]
    no_host_copilot: bool,

    /// Cursor agent executable used to open known managed sessions.
    #[arg(
        long,
        default_value = "cursor-agent",
        value_name = "PATH",
        global = true
    )]
    cursor_bin: String,

    /// Disable Cursor session control on the host.
    #[arg(long)]
    no_host_cursor: bool,

    /// Antigravity CLI executable used to open documented recent sessions.
    #[arg(long, default_value = "agy", value_name = "PATH", global = true)]
    antigravity_bin: String,

    /// Disable Antigravity discovery on the host.
    #[arg(long)]
    no_host_antigravity: bool,

    /// Explicitly observe Claude and Codex sessions in this running Docker container.
    #[arg(long = "docker-container", value_name = "NAME_OR_ID", global = true)]
    docker_containers: Vec<String>,

    /// Docker executable used for explicitly enrolled container targets.
    #[arg(long, default_value = "docker", value_name = "PATH", global = true)]
    docker_bin: String,

    /// Override the protected managed-container ownership registry.
    #[arg(long, value_name = "PATH", global = true)]
    managed_docker_registry: Option<PathBuf>,

    /// Initial coding-agent harness used by the new-session composer.
    #[arg(
        long = "harness",
        visible_alias = "launch-provider",
        value_name = "HARNESS",
        value_enum,
        default_value_t = LaunchProvider::Claude
    )]
    launch_provider: LaunchProvider,

    /// Working directory used for newly launched sessions.
    #[arg(long, value_name = "PATH")]
    launch_cwd: Option<PathBuf>,

    /// Provider refresh interval in milliseconds.
    #[arg(long, default_value_t = 15000, value_parser = clap::value_parser!(u64).range(250..))]
    refresh_ms: u64,
}

fn main() -> Result<()> {
    let mut cli = Cli::parse();
    resolve_default_provider_bins(&mut cli);
    if let Some(command) = cli.command.as_ref() {
        match command {
            Commands::Update => run_self_update()?,
            Commands::Doctor => {
                let provider_bins = vec![
                    (Provider::Claude, cli.claude_bin.clone()),
                    (Provider::Codex, cli.codex_bin.clone()),
                    (Provider::Pi, cli.pi_bin.clone()),
                    (Provider::OpenCode, cli.opencode_bin.clone()),
                    (Provider::Cursor, cli.cursor_bin.clone()),
                    (Provider::GitHubCopilot, cli.copilot_bin.clone()),
                    (Provider::Antigravity, cli.antigravity_bin.clone()),
                ];
                let report = diagnose(&provider_bins, &cli.docker_bin, &cli.docker_containers);
                if cli.json {
                    serde_json::to_writer_pretty(io::stdout().lock(), &report)?;
                    println!();
                } else {
                    print!("{}", render_text(&report));
                }
                if report.has_errors() {
                    std::process::exit(1);
                }
            }
            Commands::Docker { command } => run_docker_command(
                command,
                &cli.docker_bin,
                cli.managed_docker_registry.clone(),
                cli.json,
            )?,
            Commands::Sessions { command } => run_session_command(command, &cli)?,
            Commands::Setup { harness, yes } => run_harness_setup(*harness, *yes)?,
            Commands::PiSupervisor { state_dir, socket } => {
                run_pi_supervisor_daemon(state_dir.clone(), socket.clone(), cli.pi_bin.clone())?
            }
        }
        return Ok(());
    }
    let request = discovery_request(&cli);
    // Secure the shared state root before any provider supervisor creates a
    // child directory. With a conventional 0022 umask, create_dir_all on a
    // provider path would otherwise leave the common parent at 0755 and the
    // hidden-session registry would correctly refuse to use it.
    let hidden_sessions = HiddenSessions::load_default()?;
    let session_aliases = SessionAliases::load_default()?;
    let provider_io_enabled = provider_io_enabled(&cli);
    let host_providers_enabled = provider_io_enabled && !cli.no_host_providers;
    let claude_enabled =
        host_providers_enabled && !cli.no_host_claude && executable_available(&cli.claude_bin);
    let codex_enabled =
        host_providers_enabled && !cli.no_host_codex && executable_available(&cli.codex_bin);
    let pi_enabled = host_providers_enabled && !cli.no_host_pi;
    let opencode_enabled =
        host_providers_enabled && !cli.no_host_opencode && executable_available(&cli.opencode_bin);
    let copilot_enabled =
        host_providers_enabled && !cli.no_host_copilot && executable_available(&cli.copilot_bin);
    let cursor_enabled =
        host_providers_enabled && !cli.no_host_cursor && executable_available(&cli.cursor_bin);
    let antigravity_enabled = host_providers_enabled && !cli.no_host_antigravity;
    let antigravity_open_enabled =
        antigravity_enabled && executable_available(&cli.antigravity_bin);
    let launch_cwd = match cli.launch_cwd {
        Some(path) => path,
        None => std::env::current_dir()?,
    };
    let launch_provider = match cli.launch_provider {
        LaunchProvider::Claude => Provider::Claude,
        LaunchProvider::Codex => Provider::Codex,
        LaunchProvider::Pi => Provider::Pi,
        LaunchProvider::OpenCode => Provider::OpenCode,
        LaunchProvider::Cursor => Provider::Cursor,
        LaunchProvider::Copilot => Provider::GitHubCopilot,
        LaunchProvider::Antigravity => Provider::Antigravity,
    };
    let pi_session_dir = if !pi_enabled {
        None
    } else {
        Some(match cli.pi_session_dir.clone() {
            Some(path) => path,
            None => default_pi_session_dir()?,
        })
    };
    let mut control = ControlHub::new(ControlHubConfig {
        claude_enabled,
        codex_enabled,
        claude_bin: cli.claude_bin.clone(),
        codex_bin: cli.codex_bin.clone(),
        docker_bin: cli.docker_bin.clone(),
        launch_provider,
        launch_cwd,
        provider_io_enabled,
    })?;
    #[cfg(target_os = "linux")]
    let pi_supervisor = pi_session_dir
        .as_ref()
        .map(|_| PiSupervisor::host(cli.pi_bin.clone()).map(Arc::new))
        .transpose()?;
    let copilot_supervisor =
        copilot_enabled.then(|| Arc::new(CopilotSupervisor::host(cli.copilot_bin.clone())));
    #[cfg(target_os = "linux")]
    let opencode_supervisor = opencode_enabled
        .then(|| OpenCodeSupervisor::host(cli.opencode_bin.clone()).map(Arc::new))
        .transpose()?;
    #[cfg(target_os = "linux")]
    let cursor_supervisor = cursor_enabled
        .then(|| CursorSupervisor::host(cli.cursor_bin.clone()).map(Arc::new))
        .transpose()?;
    let antigravity_ownership = antigravity_open_enabled
        .then(AntigravityOwnership::load_default)
        .transpose()?;
    if provider_io_enabled {
        if let Some(session_dir) = &pi_session_dir {
            #[cfg(target_os = "linux")]
            let controller = PiController::managed(
                cli.pi_bin.clone(),
                session_dir.clone(),
                pi_supervisor
                    .as_ref()
                    .expect("Pi supervisor exists when the Pi source is enabled")
                    .clone(),
            );
            #[cfg(not(target_os = "linux"))]
            let controller = PiController::host(cli.pi_bin.clone(), session_dir.clone());
            control.register_controller(Arc::new(controller))?;
        }
        if opencode_enabled {
            #[cfg(target_os = "linux")]
            let controller = OpenCodeController::managed(
                cli.opencode_bin.clone(),
                opencode_supervisor
                    .as_ref()
                    .expect("OpenCode supervisor exists when OpenCode is enabled")
                    .clone(),
            );
            #[cfg(not(target_os = "linux"))]
            let controller = OpenCodeController::host(cli.opencode_bin.clone());
            control.register_controller(Arc::new(controller))?;
        }
        if copilot_enabled {
            control.register_controller(Arc::new(CopilotController::managed(
                copilot_supervisor
                    .as_ref()
                    .expect("Copilot supervisor exists when Copilot is enabled")
                    .clone(),
            )))?;
        }
        if cursor_enabled {
            #[cfg(target_os = "linux")]
            let controller = CursorController::managed(
                cursor_supervisor
                    .as_ref()
                    .expect("Cursor supervisor exists when Cursor is enabled")
                    .clone(),
            );
            #[cfg(not(target_os = "linux"))]
            let controller = CursorController::host(cli.cursor_bin.clone());
            control.register_controller(Arc::new(controller))?;
        }
        if antigravity_open_enabled {
            control.register_controller(Arc::new(AntigravityController::managed(
                cli.antigravity_bin.clone(),
                antigravity_ownership
                    .as_ref()
                    .expect("Antigravity ownership exists when its controller is enabled")
                    .clone(),
            )?))?;
        }
    }

    let mut engine = DiscoveryEngine::new();
    if let Some(fixture) = cli.fixture {
        engine.add_source(FixtureSource::new(fixture));
    } else {
        if claude_enabled {
            engine.add_source(ClaudeSource::host(cli.claude_bin));
        }
        if codex_enabled {
            if let Some(supervisor) = control.codex_supervisor() {
                if request.include_external {
                    engine.add_source(CodexSource::managed(supervisor));
                } else {
                    engine.add_source(CodexSource::managed_owned(supervisor));
                }
            }
        }
        if let Some(session_dir) = pi_session_dir {
            #[cfg(target_os = "linux")]
            let source = {
                let supervisor =
                    pi_supervisor.expect("Pi supervisor exists when the Pi source is enabled");
                let discovery_dir = if request.include_external {
                    session_dir
                } else {
                    supervisor.session_dir()
                };
                PiSource::managed(discovery_dir, supervisor)
            };
            #[cfg(not(target_os = "linux"))]
            let source = PiSource::host(session_dir);
            engine.add_source(source);
        }
        if opencode_enabled {
            #[cfg(target_os = "linux")]
            let source = {
                let supervisor = opencode_supervisor
                    .expect("OpenCode supervisor exists when OpenCode is enabled");
                if request.include_external {
                    OpenCodeSource::managed(cli.opencode_bin, supervisor)
                } else {
                    OpenCodeSource::managed_owned(cli.opencode_bin, supervisor)
                }
            };
            #[cfg(not(target_os = "linux"))]
            let source = OpenCodeSource::host(cli.opencode_bin);
            engine.add_source(source);
        }
        if copilot_enabled && request.include_external {
            engine.add_source(CopilotSource::host(cli.copilot_bin));
        }
        #[cfg(target_os = "linux")]
        if let Some(supervisor) = cursor_supervisor {
            engine.add_source(CursorSource::managed(supervisor));
        }
        if antigravity_enabled {
            if let Some(ownership) = antigravity_ownership {
                engine.add_source(AntigravitySource::managed(ownership)?);
            } else if request.include_external {
                engine.add_source(AntigravitySource::default_host()?);
            }
        }
        for container in cli.docker_containers {
            let target = DockerTarget::inspect(&container, &cli.docker_bin)?;
            let display_image = target.display_image();
            engine.add_source(ClaudeSource::docker(
                target.name.clone(),
                target.id.clone(),
                display_image.clone(),
            ));
            engine.add_source(CodexSource::docker(target.name, target.id, display_image));
        }
    }
    if cli.json {
        let mut snapshot = engine.discover(&request);
        control.enrich(&mut snapshot);
        if !request.include_external {
            control.retain_owned(&mut snapshot);
        }
        hidden_sessions.filter_snapshot(&mut snapshot);
        session_aliases.apply_snapshot_with_warning(&mut snapshot);
        serde_json::to_writer_pretty(io::stdout().lock(), &snapshot)?;
        println!();
        return Ok(());
    }

    if !io::stdout().is_terminal() || !io::stdin().is_terminal() {
        bail!("the dashboard requires a TTY; use --json for machine-readable output");
    }

    run_dashboard(
        &engine,
        &request,
        Duration::from_millis(cli.refresh_ms),
        &control,
        hidden_sessions,
        session_aliases,
    )?;

    Ok(())
}

fn provider_io_enabled(cli: &Cli) -> bool {
    cli.fixture.is_none()
}

fn discovery_request(cli: &Cli) -> DiscoveryRequest {
    DiscoveryRequest {
        // `--all` remains accepted for scripts written before completed
        // sessions became the default. `--hide-completed` is the explicit
        // startup opt-out and mirrors `/completed hide` in the dashboard.
        include_completed: cli.all || !cli.hide_completed,
        include_interactive: cli.include_interactive,
        include_external: cli.include_external || cli.fixture.is_some(),
        cwd: cli.cwd.clone(),
        history_limit: cli.history_limit as usize,
        history_oldest_first: false,
    }
}

fn run_session_command(command: &SessionCommand, cli: &Cli) -> Result<()> {
    match command {
        SessionCommand::Archive {
            cwd,
            older_than_days,
            limit,
            yes,
        } => run_completed_archive(cli, cwd.as_deref(), *older_than_days, *limit as usize, *yes),
        SessionCommand::Hide { session_id } => {
            let registry = HiddenSessions::load_default()?;
            let inserted = registry.hide_id(session_id)?;
            if cli.json {
                serde_json::to_writer_pretty(io::stdout().lock(), &registry.list())?;
                println!();
            } else if inserted {
                println!(
                    "Hidden {} from Open Agent View. Provider history was not changed.",
                    sanitize_cli_text(session_id)
                );
            } else {
                println!(
                    "{} is already hidden. Provider history was not changed.",
                    sanitize_cli_text(session_id)
                );
            }
            Ok(())
        }
        SessionCommand::Unhide { session_id } => {
            let registry = HiddenSessions::load_default()?;
            let removed = registry.unhide(session_id)?;
            if cli.json {
                serde_json::to_writer_pretty(io::stdout().lock(), &removed)?;
                println!();
            } else if removed.is_some() {
                println!(
                    "Unhid {}. It will return if provider discovery still reports it.",
                    sanitize_cli_text(session_id)
                );
            } else {
                println!("{} was not hidden.", sanitize_cli_text(session_id));
            }
            Ok(())
        }
        SessionCommand::Hidden => {
            let registry = HiddenSessions::load_default()?;
            print_hidden_sessions(&registry.list(), cli.json)
        }
        SessionCommand::Rename { session_id, name } => {
            let registry = SessionAliases::load_default()?;
            let changed = registry.set_for_id(session_id, name)?;
            if cli.json {
                serde_json::to_writer_pretty(io::stdout().lock(), &registry.list())?;
                println!();
            } else if changed {
                println!(
                    "Named {} locally as {}. The provider title was not changed.",
                    sanitize_cli_text(session_id),
                    sanitize_cli_text(name.trim())
                );
            } else {
                println!(
                    "{} already has local name {}. The provider title was not changed.",
                    sanitize_cli_text(session_id),
                    sanitize_cli_text(name.trim())
                );
            }
            Ok(())
        }
        SessionCommand::ResetName { session_id } => {
            let registry = SessionAliases::load_default()?;
            let removed = registry.clear(session_id)?;
            if cli.json {
                serde_json::to_writer_pretty(io::stdout().lock(), &removed)?;
                println!();
            } else if removed.is_some() {
                println!(
                    "Cleared the local name for {}. The latest provider title will appear on refresh.",
                    sanitize_cli_text(session_id)
                );
            } else {
                println!(
                    "{} did not have a local name.",
                    sanitize_cli_text(session_id)
                );
            }
            Ok(())
        }
        SessionCommand::Aliases => {
            let registry = SessionAliases::load_default()?;
            print_session_aliases(&registry.list(), cli.json)
        }
    }
}

fn print_session_aliases(records: &[SessionAliasRecord], json: bool) -> Result<()> {
    if json {
        serde_json::to_writer_pretty(io::stdout().lock(), records)?;
        println!();
        return Ok(());
    }
    if records.is_empty() {
        println!("No sessions have local Open Agent View names.");
        return Ok(());
    }
    println!(
        "Local session names (provider titles are unchanged): {}",
        records.len()
    );
    for record in records {
        let provider = record
            .provider
            .as_ref()
            .map(Provider::label)
            .unwrap_or("unknown provider");
        println!(
            "  {}  {}  {}",
            sanitize_cli_text(&record.id),
            sanitize_cli_text(provider),
            sanitize_cli_text(&record.alias)
        );
    }
    Ok(())
}

fn print_hidden_sessions(records: &[HiddenSessionRecord], json: bool) -> Result<()> {
    if json {
        serde_json::to_writer_pretty(io::stdout().lock(), records)?;
        println!();
        return Ok(());
    }
    if records.is_empty() {
        println!("No sessions are hidden from Open Agent View.");
        return Ok(());
    }
    println!(
        "Locally hidden sessions (provider history is retained): {}",
        records.len()
    );
    for record in records {
        let provider = record
            .provider
            .as_ref()
            .map(Provider::label)
            .unwrap_or("unknown provider");
        let name = record.name.as_deref().unwrap_or("unknown name");
        println!(
            "  {}  {}  {}",
            sanitize_cli_text(&record.id),
            sanitize_cli_text(provider),
            sanitize_cli_text(name)
        );
    }
    Ok(())
}

fn run_completed_archive(
    cli: &Cli,
    cwd: Option<&std::path::Path>,
    older_than_days: Option<u64>,
    limit: usize,
    yes: bool,
) -> Result<()> {
    if cli.fixture.is_some() {
        bail!("session archiving is disabled while reading a fixture");
    }
    if cli.no_host_providers || cli.no_host_codex {
        bail!("session archiving requires host Codex to be enabled");
    }
    if !executable_available(&cli.codex_bin) {
        bail!("Codex executable is unavailable: {}", cli.codex_bin);
    }

    let scope = cwd
        .map(|path| {
            std::fs::canonicalize(path)
                .with_context(|| format!("failed to resolve archive scope {}", path.display()))
        })
        .transpose()?;
    let updated_before = older_than_days
        .map(|days| {
            let seconds = days
                .checked_mul(86_400)
                .context("older-than duration overflowed")?;
            SystemTime::now()
                .checked_sub(Duration::from_secs(seconds))
                .context("older-than cutoff predates the system clock")
        })
        .transpose()?;

    let control = ControlHub::new(ControlHubConfig {
        claude_enabled: false,
        codex_enabled: true,
        claude_bin: cli.claude_bin.clone(),
        codex_bin: cli.codex_bin.clone(),
        docker_bin: cli.docker_bin.clone(),
        launch_provider: Provider::Codex,
        launch_cwd: std::env::current_dir()?,
        provider_io_enabled: true,
    })?;
    let supervisor = control
        .codex_supervisor()
        .context("Codex supervisor was not initialized")?;
    let mut engine = DiscoveryEngine::new();
    engine.add_source(CodexSource::managed_owned(supervisor));
    let mut snapshot = engine.discover(&DiscoveryRequest {
        include_completed: true,
        include_interactive: true,
        include_external: false,
        cwd: None,
        history_limit: (cli.history_limit as usize).max(limit),
        history_oldest_first: true,
    });
    if !snapshot.warnings.is_empty() {
        bail!(
            "refusing maintenance because Codex discovery was incomplete: {}",
            snapshot.warnings.join("; ")
        );
    }
    control.enrich(&mut snapshot);
    let plan = plan_completed_archive(&snapshot, scope.as_deref(), updated_before, limit);
    let report = if yes {
        execute_completed_archive(&plan, |session| control.archive(session).map(|_| ()))
    } else {
        plan.report().clone()
    };
    print_bulk_archive_report(&report, cli.json)?;
    if !report.failures.is_empty() {
        bail!(
            "{} of {} selected sessions could not be archived",
            report.failures.len(),
            report.selected.len()
        );
    }
    Ok(())
}

fn print_bulk_archive_report(report: &BulkArchiveReport, json: bool) -> Result<()> {
    if json {
        serde_json::to_writer_pretty(io::stdout().lock(), report)?;
        println!();
        return Ok(());
    }

    let mode = if report.dry_run {
        "Dry run"
    } else {
        "Archive run"
    };
    println!(
        "{mode}: {} completed seen; {} matched scope; {} owned and archivable; {} selected.",
        report.completed_seen,
        report.matched_scope,
        report.eligible,
        report.selected.len()
    );
    if report.skipped_without_authority > 0 {
        println!(
            "Skipped {} matched session(s) without exact archive authority.",
            report.skipped_without_authority
        );
    }
    for item in &report.selected {
        println!(
            "  {}  {}  {}",
            sanitize_cli_text(&item.provider_session_id),
            sanitize_cli_text(&item.name),
            sanitize_cli_text(&item.cwd.to_string_lossy())
        );
    }
    if report.dry_run && !report.selected.is_empty() {
        println!("No sessions changed. Re-run the same command with --yes to archive this batch.");
    } else if !report.dry_run {
        println!(
            "Archived {}; {} failed.",
            report.archived.len(),
            report.failures.len()
        );
        for failure in &report.failures {
            println!(
                "  failed {}: {}",
                sanitize_cli_text(&failure.session.provider_session_id),
                sanitize_cli_text(&failure.error)
            );
        }
    }
    Ok(())
}

fn sanitize_cli_text(value: &str) -> String {
    value
        .chars()
        .map(|character| {
            if character.is_control() {
                '\u{fffd}'
            } else {
                character
            }
        })
        .take(240)
        .collect()
}

fn executable_available(program: &str) -> bool {
    executable_file(std::path::Path::new(program))
        || resolve_executable(program).is_some_and(|path| executable_file(&path))
}

enum HarnessInstaller {
    Script { url: &'static str },
    Npm { package: &'static str },
}

fn run_harness_setup(provider: LaunchProvider, confirmed: bool) -> Result<()> {
    let (label, executable, installer) = match provider {
        LaunchProvider::Claude => (
            "Claude Code",
            "claude",
            HarnessInstaller::Script {
                url: "https://claude.ai/install.sh",
            },
        ),
        LaunchProvider::Codex => (
            "Codex CLI",
            "codex",
            HarnessInstaller::Npm {
                package: "@openai/codex",
            },
        ),
        LaunchProvider::Pi => (
            "Pi coding agent",
            "pi",
            HarnessInstaller::Npm {
                package: "@mariozechner/pi-coding-agent",
            },
        ),
        LaunchProvider::OpenCode => (
            "OpenCode",
            "opencode",
            HarnessInstaller::Script {
                url: "https://opencode.ai/install",
            },
        ),
        LaunchProvider::Cursor => (
            "Cursor Agent",
            "cursor-agent",
            HarnessInstaller::Script {
                url: "https://cursor.com/install",
            },
        ),
        LaunchProvider::Copilot => (
            "GitHub Copilot CLI",
            "copilot",
            HarnessInstaller::Script {
                url: "https://gh.io/copilot-install",
            },
        ),
        LaunchProvider::Antigravity => (
            "Antigravity CLI",
            "agy",
            HarnessInstaller::Script {
                url: "https://antigravity.google/cli/install.sh",
            },
        ),
    };
    if executable_available(executable) {
        println!("{label} is already installed. Open the model picker in `open-agent-view`; if authentication is needed, press Enter there.");
        return Ok(());
    }
    let source = match &installer {
        HarnessInstaller::Script { url } => *url,
        HarnessInstaller::Npm { package } => *package,
    };
    if !confirmed {
        if !io::stdin().is_terminal() {
            bail!(
                "setup changes your user installation; rerun with --yes after reviewing {source}"
            );
        }
        print!("Install {label} for the current user from {source}? [y/N] ");
        io::stdout().flush()?;
        let mut answer = String::new();
        io::stdin().read_line(&mut answer)?;
        if !matches!(answer.trim().to_ascii_lowercase().as_str(), "y" | "yes") {
            println!("Installation cancelled.");
            return Ok(());
        }
    }
    println!("Installing {label}…");
    let status = match installer {
        HarnessInstaller::Script { url } => run_official_script_installer(url)?,
        HarnessInstaller::Npm { package } => Command::new("npm")
            .args(["install", "--global", package])
            .stdin(Stdio::inherit())
            .stdout(Stdio::inherit())
            .stderr(Stdio::inherit())
            .status()
            .context(
                "failed to start npm; install Node.js/npm or use the provider's native installer",
            )?,
    };
    if !status.success() {
        bail!("{label} installer exited with status {status}");
    }
    println!("{label} installation completed. Restart `open-agent-view`, choose {label}, then open the model picker; OAV will hand off login if needed.");
    Ok(())
}

fn run_official_script_installer(url: &str) -> Result<std::process::ExitStatus> {
    let directory = std::env::temp_dir().join(format!(
        "open-agent-view-installer-{}-{}",
        std::process::id(),
        SystemTime::now()
            .duration_since(SystemTime::UNIX_EPOCH)
            .unwrap_or_default()
            .as_nanos()
    ));
    std::fs::create_dir(&directory).with_context(|| {
        format!(
            "failed to create installer staging directory {}",
            directory.display()
        )
    })?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(&directory, std::fs::Permissions::from_mode(0o700))?;
    }
    let script = directory.join("install.sh");
    let download = Command::new("curl")
        .args([
            "--fail",
            "--location",
            "--show-error",
            "--progress-bar",
            "--output",
        ])
        .arg(&script)
        .arg(url)
        .stdin(Stdio::null())
        .stdout(Stdio::inherit())
        .stderr(Stdio::inherit())
        .status()
        .context("failed to start curl for the official installer")?;
    if !download.success() {
        let _ = std::fs::remove_file(&script);
        let _ = std::fs::remove_dir(&directory);
        bail!("official installer download failed with status {download}");
    }
    let status = Command::new("bash")
        .arg(&script)
        .stdin(Stdio::inherit())
        .stdout(Stdio::inherit())
        .stderr(Stdio::inherit())
        .status()
        .context("failed to start the downloaded official installer")?;
    let _ = std::fs::remove_file(&script);
    let _ = std::fs::remove_dir(&directory);
    Ok(status)
}

fn run_self_update() -> Result<()> {
    let repository = std::env::var("OAV_REPO").unwrap_or_else(|_| "xhluca/open-agent-view".into());
    if !repository
        .split_once('/')
        .is_some_and(|(owner, name)| !owner.is_empty() && !name.is_empty())
        || repository.chars().any(|character| {
            !(character.is_ascii_alphanumeric() || matches!(character, '/' | '_' | '-' | '.'))
        })
    {
        bail!("OAV_REPO must have the form OWNER/REPO");
    }
    let directory = std::env::temp_dir().join(format!(
        "open-agent-view-update-{}-{}",
        std::process::id(),
        SystemTime::now()
            .duration_since(SystemTime::UNIX_EPOCH)
            .unwrap_or_default()
            .as_nanos()
    ));
    std::fs::create_dir(&directory).with_context(|| {
        format!(
            "failed to create update staging directory {}",
            directory.display()
        )
    })?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(&directory, std::fs::Permissions::from_mode(0o700))?;
    }
    let script = directory.join("install.sh");
    let result = (|| -> Result<()> {
        println!("Checking for the latest Open Agent View release…");
        let mut downloaded = false;
        if executable_available("gh") {
            let output = std::fs::File::create(&script)?;
            let status = Command::new("gh")
                .args([
                    "api",
                    "-H",
                    "Accept: application/vnd.github.raw+json",
                    &format!("repos/{repository}/contents/install.sh"),
                ])
                .stdin(Stdio::null())
                .stdout(Stdio::from(output))
                .stderr(Stdio::null())
                .status()
                .context("failed to start gh while downloading the updater")?;
            downloaded = status.success();
        }
        if !downloaded {
            let status = Command::new("curl")
                .args([
                    "--fail",
                    "--location",
                    "--show-error",
                    "--progress-bar",
                    "--output",
                ])
                .arg(&script)
                .arg(format!(
                    "https://raw.githubusercontent.com/{repository}/main/install.sh"
                ))
                .stdin(Stdio::null())
                .stdout(Stdio::inherit())
                .stderr(Stdio::inherit())
                .status()
                .context("failed to start curl while downloading the updater")?;
            if !status.success() {
                bail!(
                    "could not download the Open Agent View installer; for a private repository, run `gh auth login` and retry"
                );
            }
        }
        if std::fs::metadata(&script)?.len() == 0 {
            bail!("the downloaded Open Agent View installer was empty");
        }
        let install_dir = std::env::var_os("OAV_INSTALL_DIR")
            .map(PathBuf::from)
            .or_else(|| {
                std::env::current_exe()
                    .ok()
                    .and_then(|path| path.parent().map(PathBuf::from))
            })
            .or_else(|| {
                std::env::var_os("HOME")
                    .map(PathBuf::from)
                    .map(|home| home.join(".local/bin"))
            })
            .context("could not determine the current Open Agent View install directory")?;
        let status = Command::new("bash")
            .arg(&script)
            .env("OAV_REPO", &repository)
            .env("OAV_INSTALL_DIR", &install_dir)
            .stdin(Stdio::inherit())
            .stdout(Stdio::inherit())
            .stderr(Stdio::inherit())
            .status()
            .context("failed to start the Open Agent View installer")?;
        if !status.success() {
            bail!("Open Agent View update exited with status {status}");
        }
        println!("Open Agent View update completed.");
        Ok(())
    })();
    let _ = std::fs::remove_file(&script);
    let _ = std::fs::remove_dir(&directory);
    result
}

fn resolve_default_provider_bins(cli: &mut Cli) {
    for executable in [
        &mut cli.claude_bin,
        &mut cli.codex_bin,
        &mut cli.pi_bin,
        &mut cli.opencode_bin,
        &mut cli.copilot_bin,
        &mut cli.cursor_bin,
        &mut cli.antigravity_bin,
    ] {
        if let Some(path) = resolve_executable(executable) {
            *executable = path.to_string_lossy().into_owned();
        }
    }
}

fn resolve_executable(program: &str) -> Option<PathBuf> {
    resolve_executable_from(
        program,
        std::env::var_os("PATH").as_deref(),
        std::env::var_os("HOME").as_deref(),
    )
}

fn resolve_executable_from(
    program: &str,
    search_path: Option<&std::ffi::OsStr>,
    home: Option<&std::ffi::OsStr>,
) -> Option<PathBuf> {
    let candidate = std::path::Path::new(program);
    if candidate.components().count() > 1 {
        return executable_file(candidate).then(|| candidate.to_path_buf());
    }
    if let Some(candidate) = search_path.and_then(|path| {
        std::env::split_paths(path)
            .map(|directory| directory.join(program))
            .find(|candidate| executable_file(candidate))
    }) {
        return Some(candidate);
    }

    let home = PathBuf::from(home?);
    provider_fallback_directories(program)
        .iter()
        .map(|directory| home.join(directory).join(program))
        .find(|candidate| executable_file(candidate))
}

fn provider_fallback_directories(program: &str) -> &'static [&'static str] {
    match program {
        "codex" | "copilot" => &[".local/bin", ".npm-global/bin"],
        "opencode" => &[".local/bin", ".opencode/bin", ".bun/bin"],
        "cursor-agent" => &[".local/bin", ".cursor/bin"],
        "agy" => &[".local/bin", ".antigravity/bin"],
        "claude" | "pi" => &[".local/bin", ".npm-global/bin"],
        _ => &[],
    }
}

fn executable_file(path: &std::path::Path) -> bool {
    let Ok(metadata) = path.metadata() else {
        return false;
    };
    if !metadata.is_file() {
        return false;
    }
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        metadata.permissions().mode() & 0o111 != 0
    }
    #[cfg(not(unix))]
    {
        true
    }
}

fn run_docker_command(
    command: &DockerCommand,
    docker_bin: &str,
    registry_path: Option<PathBuf>,
    json: bool,
) -> Result<()> {
    require_destructive_confirmation(command)?;
    let registry_path = match registry_path {
        Some(path) => path,
        None => default_managed_docker_registry_path()?,
    };
    let mut service = ManagedDockerService::open(docker_bin, registry_path)?;
    match command {
        DockerCommand::List => print_managed_statuses(&service.list(), json)?,
        DockerCommand::Status { container } => {
            let managed = service.enroll(container)?;
            let status = service.status(&managed.container().id)?;
            print_managed_statuses(&[status], json)?;
        }
        DockerCommand::Create {
            name,
            image,
            workspace,
            state_home,
            network,
            uid,
            gid,
        } => {
            let (default_uid, default_gid) = current_user_ids()?;
            let spec = ManagedDockerCreateSpec::new(
                name,
                generate_managed_instance_id()?,
                image,
                workspace,
                state_home,
                uid.unwrap_or(default_uid),
                gid.unwrap_or(default_gid),
                env!("CARGO_PKG_VERSION"),
            )?
            .with_network(network)?;
            let status = service.create(&spec)?;
            print_managed_statuses(&[status], json)?;
            if !json {
                println!(
                    "created stopped; run `open-agent-view docker start {}` when ready",
                    name
                );
            }
        }
        DockerCommand::Start { container } => {
            let status = service.start(container)?;
            print_managed_statuses(&[status], json)?;
        }
        DockerCommand::Stop { container, yes } => {
            debug_assert!(*yes);
            let status = service.stop(container)?;
            print_managed_statuses(&[status], json)?;
        }
        DockerCommand::Remove { container, yes } => {
            debug_assert!(*yes);
            let removed = service.remove(container)?;
            if json {
                serde_json::to_writer_pretty(io::stdout().lock(), &removed)?;
                println!();
            } else {
                println!(
                    "removed managed container {}; persistent workspace/state were retained",
                    removed.container_id()
                );
            }
        }
    }
    Ok(())
}

fn require_destructive_confirmation(command: &DockerCommand) -> Result<()> {
    if matches!(command, DockerCommand::Stop { yes: false, .. }) {
        bail!("refusing to stop without --yes after verifying the exact managed target");
    }
    if matches!(command, DockerCommand::Remove { yes: false, .. }) {
        bail!("refusing to remove without --yes; stop and verify the target first");
    }
    Ok(())
}

fn print_managed_statuses(statuses: &[ManagedDockerStatus], json: bool) -> Result<()> {
    if json {
        serde_json::to_writer_pretty(io::stdout().lock(), statuses)?;
        println!();
        return Ok(());
    }
    if statuses.is_empty() {
        println!("No managed Docker containers are registered.");
        return Ok(());
    }
    for status in statuses {
        println!(
            "{:<12} {:<20} {}  {}",
            format!("{:?}", status.state).to_ascii_lowercase(),
            status.name.as_deref().unwrap_or("—"),
            &status.container_id[..12],
            status.detail.as_deref().unwrap_or("no detail")
        );
    }
    Ok(())
}

fn current_user_ids() -> Result<(u32, u32)> {
    #[cfg(unix)]
    {
        Ok((unsafe { libc::geteuid() }, unsafe { libc::getegid() }))
    }
    #[cfg(not(unix))]
    {
        bail!("managed Docker creation currently requires a Unix host or explicit UID/GID")
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn docker_command(arguments: &[&str]) -> DockerCommand {
        let mut argv = vec!["open-agent-view", "docker"];
        argv.extend_from_slice(arguments);
        let cli = Cli::try_parse_from(argv).unwrap();
        let Some(Commands::Docker { command }) = cli.command else {
            panic!("expected a Docker command");
        };
        command
    }

    fn session_command(arguments: &[&str]) -> SessionCommand {
        let mut argv = vec!["open-agent-view", "sessions"];
        argv.extend_from_slice(arguments);
        let cli = Cli::try_parse_from(argv).unwrap();
        let Some(Commands::Sessions { command }) = cli.command else {
            panic!("expected a sessions command");
        };
        command
    }

    #[test]
    fn parses_digest_pinned_managed_create_command() {
        let cli = Cli::try_parse_from([
            "open-agent-view",
            "docker",
            "create",
            "--name",
            "oav-agent",
            "--image",
            "example@sha256:aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa",
            "--workspace",
            "/work",
            "--state-home",
            "/state",
        ])
        .unwrap();

        assert!(matches!(
            cli.command,
            Some(Commands::Docker {
                command: DockerCommand::Create { .. }
            })
        ));
    }

    #[test]
    fn destructive_managed_commands_do_not_imply_confirmation() {
        let cli =
            Cli::try_parse_from(["open-agent-view", "docker", "remove", "oav-agent"]).unwrap();

        assert!(matches!(
            cli.command,
            Some(Commands::Docker {
                command: DockerCommand::Remove { yes: false, .. }
            })
        ));
    }

    #[test]
    fn bulk_archive_is_dry_run_by_default_and_parses_every_scope() {
        assert_eq!(
            session_command(&[
                "archive",
                "--cwd",
                "/work/project",
                "--older-than-days",
                "30",
                "--limit",
                "250",
            ]),
            SessionCommand::Archive {
                cwd: Some(PathBuf::from("/work/project")),
                older_than_days: Some(30),
                limit: 250,
                yes: false,
            }
        );
        assert_eq!(
            session_command(&["archive", "--yes"]),
            SessionCommand::Archive {
                cwd: None,
                older_than_days: None,
                limit: 100,
                yes: true,
            }
        );
        for arguments in [
            vec!["open-agent-view", "sessions"],
            vec!["open-agent-view", "sessions", "archive", "--limit", "0"],
            vec![
                "open-agent-view",
                "sessions",
                "archive",
                "--older-than-days",
                "0",
            ],
        ] {
            assert!(Cli::try_parse_from(arguments).is_err());
        }
    }

    #[test]
    fn completed_sessions_are_visible_by_default_with_an_explicit_opt_out() {
        for arguments in [vec!["open-agent-view"], vec!["open-agent-view", "--json"]] {
            let cli = Cli::try_parse_from(arguments).unwrap();
            assert!(discovery_request(&cli).include_completed);
        }
        for arguments in [
            vec!["open-agent-view", "--all"],
            vec!["open-agent-view", "--json", "--all"],
        ] {
            let cli = Cli::try_parse_from(arguments).unwrap();
            assert!(discovery_request(&cli).include_completed);
        }
        for arguments in [
            vec!["open-agent-view", "--hide-completed"],
            vec!["open-agent-view", "--json", "--active-only"],
        ] {
            let cli = Cli::try_parse_from(arguments).unwrap();
            assert!(!discovery_request(&cli).include_completed);
        }
        assert!(Cli::try_parse_from(["open-agent-view", "--all", "--hide-completed"]).is_err());
    }

    #[test]
    fn external_provider_history_is_opt_in_and_fixtures_remain_complete() {
        let live = Cli::try_parse_from(["open-agent-view", "--json"]).unwrap();
        assert!(!discovery_request(&live).include_external);

        let external =
            Cli::try_parse_from(["open-agent-view", "--json", "--include-external"]).unwrap();
        assert!(discovery_request(&external).include_external);

        let fixture =
            Cli::try_parse_from(["open-agent-view", "--fixture", "/tmp/snapshot.json"]).unwrap();
        assert!(discovery_request(&fixture).include_external);
    }

    #[test]
    fn cli_text_sanitization_blocks_controls_and_bounds_provider_output() {
        let sanitized = sanitize_cli_text(&format!("name\u{1b}[2J\n{}", "x".repeat(500)));
        assert!(!sanitized.contains('\u{1b}'));
        assert!(!sanitized.contains('\n'));
        assert!(sanitized.contains('\u{fffd}'));
        assert_eq!(sanitized.chars().count(), 240);
    }

    #[test]
    fn parses_every_non_destructive_managed_docker_subcommand_exactly() {
        assert_eq!(docker_command(&["list"]), DockerCommand::List);
        assert_eq!(
            docker_command(&["status", "oav-agent"]),
            DockerCommand::Status {
                container: "oav-agent".into()
            }
        );
        assert_eq!(
            docker_command(&["start", "sha256:exact"]),
            DockerCommand::Start {
                container: "sha256:exact".into()
            }
        );
    }

    #[test]
    fn parses_all_managed_create_fields_without_shell_interpretation() {
        assert_eq!(
            docker_command(&[
                "create",
                "--name",
                "name with spaces",
                "--image",
                "registry/image@sha256:aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa",
                "--workspace",
                "/work space",
                "--state-home",
                "/state space",
                "--network",
                "none",
                "--uid",
                "1234",
                "--gid",
                "5678",
            ]),
            DockerCommand::Create {
                name: "name with spaces".into(),
                image: "registry/image@sha256:aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa".into(),
                workspace: PathBuf::from("/work space"),
                state_home: PathBuf::from("/state space"),
                network: "none".into(),
                uid: Some(1234),
                gid: Some(5678),
            }
        );
    }

    #[test]
    fn stop_and_remove_parse_confirmation_as_false_unless_explicit() {
        assert_eq!(
            docker_command(&["stop", "agent"]),
            DockerCommand::Stop {
                container: "agent".into(),
                yes: false
            }
        );
        assert_eq!(
            docker_command(&["stop", "agent", "--yes"]),
            DockerCommand::Stop {
                container: "agent".into(),
                yes: true
            }
        );
        assert_eq!(
            docker_command(&["remove", "agent"]),
            DockerCommand::Remove {
                container: "agent".into(),
                yes: false
            }
        );
        assert_eq!(
            docker_command(&["remove", "agent", "--yes"]),
            DockerCommand::Remove {
                container: "agent".into(),
                yes: true
            }
        );
    }

    #[test]
    fn confirmation_gate_blocks_only_unconfirmed_destructive_commands() {
        let stop = docker_command(&["stop", "agent"]);
        assert_eq!(
            require_destructive_confirmation(&stop)
                .unwrap_err()
                .to_string(),
            "refusing to stop without --yes after verifying the exact managed target"
        );
        let remove = docker_command(&["remove", "agent"]);
        assert_eq!(
            require_destructive_confirmation(&remove)
                .unwrap_err()
                .to_string(),
            "refusing to remove without --yes; stop and verify the target first"
        );

        for command in [
            docker_command(&["list"]),
            docker_command(&["status", "agent"]),
            docker_command(&["start", "agent"]),
            docker_command(&["stop", "agent", "--yes"]),
            docker_command(&["remove", "agent", "--yes"]),
        ] {
            require_destructive_confirmation(&command).unwrap();
        }
    }

    #[test]
    fn malformed_docker_commands_are_rejected_during_parsing() {
        for arguments in [
            vec!["open-agent-view", "docker"],
            vec!["open-agent-view", "docker", "status"],
            vec!["open-agent-view", "docker", "start"],
            vec!["open-agent-view", "docker", "create", "--name", "agent"],
            vec!["open-agent-view", "docker", "stop", "agent", "--yes=true"],
        ] {
            assert!(Cli::try_parse_from(arguments).is_err());
        }
    }

    #[test]
    fn dashboard_cli_parses_fixture_and_safety_related_options() {
        let cli = Cli::try_parse_from([
            "open-agent-view",
            "--fixture",
            "/tmp/snapshot.json",
            "--no-host-claude",
            "--no-host-codex",
            "--no-host-providers",
            "--no-host-pi",
            "--no-host-opencode",
            "--no-host-copilot",
            "--no-host-cursor",
            "--no-host-antigravity",
            "--all",
            "--include-interactive",
            "--include-external",
            "--history-limit",
            "250",
            "--cwd",
            "/project",
            "--launch-provider",
            "codex",
            "--launch-cwd",
            "/launch",
            "--refresh-ms",
            "250",
            "--docker-container",
            "explicit-container",
        ])
        .unwrap();

        assert_eq!(cli.fixture, Some(PathBuf::from("/tmp/snapshot.json")));
        assert!(cli.no_host_claude);
        assert!(cli.no_host_codex);
        assert!(cli.no_host_providers);
        assert!(cli.no_host_pi);
        assert!(cli.no_host_opencode);
        assert!(cli.no_host_copilot);
        assert!(cli.no_host_cursor);
        assert!(cli.no_host_antigravity);
        assert!(cli.all);
        assert!(cli.include_interactive);
        assert!(cli.include_external);
        assert_eq!(cli.history_limit, 250);
        assert_eq!(cli.cwd, Some(PathBuf::from("/project")));
        assert_eq!(cli.launch_provider, LaunchProvider::Codex);
        assert_eq!(cli.launch_cwd, Some(PathBuf::from("/launch")));
        assert_eq!(cli.refresh_ms, 250);
        assert_eq!(cli.docker_containers, vec!["explicit-container"]);
        assert!(!provider_io_enabled(&cli));
    }

    #[test]
    fn live_discovery_mode_keeps_provider_io_enabled() {
        let cli = Cli::try_parse_from(["open-agent-view", "--json"]).unwrap();
        assert!(provider_io_enabled(&cli));
        assert_eq!(cli.refresh_ms, 15_000);
    }

    #[test]
    fn every_managed_provider_is_available_to_the_composer() {
        for (value, expected) in [
            ("claude", LaunchProvider::Claude),
            ("codex", LaunchProvider::Codex),
            ("pi", LaunchProvider::Pi),
            ("opencode", LaunchProvider::OpenCode),
            ("cursor", LaunchProvider::Cursor),
            ("copilot", LaunchProvider::Copilot),
            ("antigravity", LaunchProvider::Antigravity),
        ] {
            let cli =
                Cli::try_parse_from(["open-agent-view", "--json", "--launch-provider", value])
                    .unwrap();
            assert_eq!(cli.launch_provider, expected);
        }
        let alias =
            Cli::try_parse_from(["open-agent-view", "--json", "--harness", "codex"]).unwrap();
        assert_eq!(alias.launch_provider, LaunchProvider::Codex);
    }

    #[test]
    fn setup_requires_an_exact_supported_harness_and_confirmation_is_explicit() {
        let cli =
            Cli::try_parse_from(["open-agent-view", "setup", "antigravity", "--yes"]).unwrap();
        assert_eq!(
            cli.command,
            Some(Commands::Setup {
                harness: LaunchProvider::Antigravity,
                yes: true,
            })
        );
        assert!(Cli::try_parse_from(["open-agent-view", "setup", "unknown"]).is_err());
    }

    #[test]
    fn refresh_interval_below_the_supported_floor_is_rejected() {
        assert!(Cli::try_parse_from(["open-agent-view", "--refresh-ms", "249"]).is_err());
        assert!(Cli::try_parse_from(["open-agent-view", "--history-limit", "0"]).is_err());
        assert!(Cli::try_parse_from(["open-agent-view", "--history-limit", "10001"]).is_err());
    }

    #[cfg(unix)]
    #[test]
    fn executable_detection_requires_a_real_executable_file() {
        use std::fs;
        use std::os::unix::fs::PermissionsExt;

        let directory = tempfile::tempdir().unwrap();
        let executable = directory.path().join("agent-cli");
        fs::write(&executable, b"#!/bin/sh\n").unwrap();
        fs::set_permissions(&executable, fs::Permissions::from_mode(0o644)).unwrap();
        assert!(!executable_available(executable.to_str().unwrap()));

        fs::set_permissions(&executable, fs::Permissions::from_mode(0o755)).unwrap();
        assert!(executable_available(executable.to_str().unwrap()));
        assert!(!executable_available(
            directory.path().join("missing").to_str().unwrap()
        ));
        assert!(!executable_available(directory.path().to_str().unwrap()));
    }

    #[cfg(unix)]
    #[test]
    fn provider_executables_resolve_path_before_supported_user_install_locations() {
        use std::fs;
        use std::os::unix::fs::PermissionsExt;

        let directory = tempfile::tempdir().unwrap();
        let path_bin = directory.path().join("path-bin");
        let npm_bin = directory.path().join(".npm-global/bin");
        let opencode_bin = directory.path().join(".opencode/bin");
        fs::create_dir_all(&path_bin).unwrap();
        fs::create_dir_all(&npm_bin).unwrap();
        fs::create_dir_all(&opencode_bin).unwrap();

        let path_codex = path_bin.join("codex");
        let fallback_codex = npm_bin.join("codex");
        let fallback_opencode = opencode_bin.join("opencode");
        for executable in [&path_codex, &fallback_codex, &fallback_opencode] {
            fs::write(executable, b"#!/bin/sh\n").unwrap();
            fs::set_permissions(executable, fs::Permissions::from_mode(0o755)).unwrap();
        }

        assert_eq!(
            resolve_executable_from(
                "codex",
                Some(path_bin.as_os_str()),
                Some(directory.path().as_os_str()),
            ),
            Some(path_codex.clone())
        );
        assert_eq!(
            resolve_executable_from("codex", None, Some(directory.path().as_os_str()),),
            Some(fallback_codex)
        );
        assert_eq!(
            resolve_executable_from("opencode", None, Some(directory.path().as_os_str()),),
            Some(fallback_opencode)
        );
        assert_eq!(
            resolve_executable_from("unknown-agent", None, Some(directory.path().as_os_str()),),
            None
        );
        assert_eq!(
            resolve_executable_from(path_codex.to_str().unwrap(), None, None),
            Some(path_codex)
        );
    }
}

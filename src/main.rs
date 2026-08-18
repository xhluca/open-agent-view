use std::io::{self, IsTerminal};
use std::path::PathBuf;
use std::time::Duration;

use anyhow::{bail, Result};
use clap::{Parser, Subcommand, ValueEnum};

use open_agent_view::adapters::{
    default_managed_docker_registry_path, generate_managed_instance_id, ClaudeSource,
    CodexSource, DiscoveryEngine, DiscoveryRequest, DockerTarget, FixtureSource,
    ManagedDockerCreateSpec, ManagedDockerService, ManagedDockerStatus,
};
use open_agent_view::control::ControlHub;
use open_agent_view::domain::Provider;
use open_agent_view::doctor::{diagnose, render_text};
use open_agent_view::terminal::run_dashboard;

#[derive(Clone, Copy, Debug, Eq, PartialEq, ValueEnum)]
enum LaunchProvider {
    Claude,
    Codex,
}

#[derive(Clone, Debug, Eq, PartialEq, Subcommand)]
enum Commands {
    /// Check provider, Docker, and target availability without changing them.
    Doctor,
    /// Create and control only containers owned by Open Agent View.
    Docker {
        #[command(subcommand)]
        command: DockerCommand,
    },
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

/// Open terminal dashboard for Claude and Codex coding agents.
#[derive(Debug, Parser)]
#[command(name = "coding-agents", version, about)]
struct Cli {
    #[command(subcommand)]
    command: Option<Commands>,

    /// Print machine-readable JSON instead of the interactive or text view.
    #[arg(long, global = true)]
    json: bool,

    /// Include completed sessions in JSON output (the TUI includes them by default).
    #[arg(long)]
    all: bool,

    /// Include foreground interactive sessions as well as background agents.
    #[arg(long)]
    include_interactive: bool,

    /// Show only sessions started under this working directory.
    #[arg(long, value_name = "PATH")]
    cwd: Option<PathBuf>,

    /// Read a normalized JSON fixture instead of probing installed providers.
    #[arg(long, value_name = "FILE")]
    fixture: Option<PathBuf>,

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

    /// Explicitly observe Claude and Codex sessions in this running Docker container.
    #[arg(
        long = "docker-container",
        value_name = "NAME_OR_ID",
        global = true
    )]
    docker_containers: Vec<String>,

    /// Docker executable used for explicitly enrolled container targets.
    #[arg(long, default_value = "docker", value_name = "PATH", global = true)]
    docker_bin: String,

    /// Override the protected managed-container ownership registry.
    #[arg(long, value_name = "PATH", global = true)]
    managed_docker_registry: Option<PathBuf>,

    /// Provider used by the new-session composer.
    #[arg(long, value_enum, default_value_t = LaunchProvider::Claude)]
    launch_provider: LaunchProvider,

    /// Working directory used for newly launched sessions.
    #[arg(long, value_name = "PATH")]
    launch_cwd: Option<PathBuf>,

    /// Provider refresh interval in milliseconds.
    #[arg(long, default_value_t = 1500, value_parser = clap::value_parser!(u64).range(250..))]
    refresh_ms: u64,
}

fn main() -> Result<()> {
    let cli = Cli::parse();
    if let Some(command) = cli.command.as_ref() {
        match command {
            Commands::Doctor => {
                let report = diagnose(
                    &cli.claude_bin,
                    &cli.codex_bin,
                    &cli.docker_bin,
                    &cli.docker_containers,
                );
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
        }
        return Ok(());
    }
    let launch_cwd = match cli.launch_cwd {
        Some(path) => path,
        None => std::env::current_dir()?,
    };
    let launch_provider = match cli.launch_provider {
        LaunchProvider::Claude => Provider::Claude,
        LaunchProvider::Codex => Provider::Codex,
    };
    let control = ControlHub::new(
        !cli.no_host_claude,
        !cli.no_host_codex,
        cli.claude_bin.clone(),
        cli.codex_bin.clone(),
        cli.docker_bin.clone(),
        launch_provider,
        launch_cwd,
    )?;

    let mut engine = DiscoveryEngine::new();
    if let Some(fixture) = cli.fixture {
        engine.add_source(FixtureSource::new(fixture));
    } else {
        if !cli.no_host_claude {
            engine.add_source(ClaudeSource::host(cli.claude_bin));
        }
        if !cli.no_host_codex {
            if let Some(supervisor) = control.codex_supervisor() {
                engine.add_source(CodexSource::managed(supervisor));
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
            engine.add_source(CodexSource::docker(
                target.name,
                target.id,
                display_image,
            ));
        }
    }
    let request = DiscoveryRequest {
        include_completed: cli.all || !cli.json,
        include_interactive: cli.include_interactive,
        cwd: cli.cwd,
    };

    if cli.json {
        let mut snapshot = engine.discover(&request);
        control.enrich(&mut snapshot);
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
    )?;

    Ok(())
}

fn run_docker_command(
    command: &DockerCommand,
    docker_bin: &str,
    registry_path: Option<PathBuf>,
    json: bool,
) -> Result<()> {
    if matches!(command, DockerCommand::Stop { yes: false, .. }) {
        bail!("refusing to stop without --yes after verifying the exact managed target");
    }
    if matches!(command, DockerCommand::Remove { yes: false, .. }) {
        bail!("refusing to remove without --yes; stop and verify the target first");
    }
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
                println!("created stopped; run `coding-agents docker start {}` when ready", name);
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

    #[test]
    fn parses_digest_pinned_managed_create_command() {
        let cli = Cli::try_parse_from([
            "coding-agents",
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
        let cli = Cli::try_parse_from(["coding-agents", "docker", "remove", "oav-agent"])
            .unwrap();

        assert!(matches!(
            cli.command,
            Some(Commands::Docker {
                command: DockerCommand::Remove { yes: false, .. }
            })
        ));
    }
}

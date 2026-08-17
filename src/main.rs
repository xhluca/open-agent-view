use std::io::{self, IsTerminal};
use std::path::PathBuf;
use std::time::Duration;

use anyhow::{bail, Result};
use clap::Parser;

use open_agent_view::adapters::{
    ClaudeSource, CodexSource, DiscoveryEngine, DiscoveryRequest, DockerTarget, FixtureSource,
};
use open_agent_view::terminal::run_dashboard;

/// Open terminal dashboard for Claude and Codex coding agents.
#[derive(Debug, Parser)]
#[command(name = "coding-agents", version, about)]
struct Cli {
    /// Print the normalized session snapshot as JSON and exit.
    #[arg(long)]
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
    #[arg(long, default_value = "claude", value_name = "PATH")]
    claude_bin: String,

    /// Disable Claude discovery on the host.
    #[arg(long)]
    no_host_claude: bool,

    /// Codex executable used for host discovery through App Server.
    #[arg(long, default_value = "codex", value_name = "PATH")]
    codex_bin: String,

    /// Disable Codex discovery on the host.
    #[arg(long)]
    no_host_codex: bool,

    /// Explicitly observe Claude and Codex sessions in this running Docker container.
    #[arg(long = "docker-container", value_name = "NAME_OR_ID")]
    docker_containers: Vec<String>,

    /// Docker executable used for explicitly enrolled container targets.
    #[arg(long, default_value = "docker", value_name = "PATH")]
    docker_bin: String,

    /// Provider refresh interval in milliseconds.
    #[arg(long, default_value_t = 1500, value_parser = clap::value_parser!(u64).range(250..))]
    refresh_ms: u64,
}

fn main() -> Result<()> {
    let cli = Cli::parse();

    let mut engine = DiscoveryEngine::new();
    if let Some(fixture) = cli.fixture {
        engine.add_source(FixtureSource::new(fixture));
    } else {
        if !cli.no_host_claude {
            engine.add_source(ClaudeSource::host(cli.claude_bin));
        }
        if !cli.no_host_codex {
            engine.add_source(CodexSource::host(cli.codex_bin));
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
        let snapshot = engine.discover(&request);
        serde_json::to_writer_pretty(io::stdout().lock(), &snapshot)?;
        println!();
        return Ok(());
    }

    if !io::stdout().is_terminal() || !io::stdin().is_terminal() {
        bail!("the dashboard requires a TTY; use --json for machine-readable output");
    }

    run_dashboard(&engine, &request, Duration::from_millis(cli.refresh_ms))?;

    Ok(())
}

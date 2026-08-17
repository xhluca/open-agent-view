use anyhow::Result;
use clap::Parser;

/// Open terminal dashboard for Claude and Codex coding agents.
#[derive(Debug, Parser)]
#[command(name = "coding-agents", version, about)]
struct Cli {
    /// Print the normalized session snapshot as JSON and exit.
    #[arg(long)]
    json: bool,
}

fn main() -> Result<()> {
    let cli = Cli::parse();

    if cli.json {
        println!("{{\"sessions\":[],\"warnings\":[]}}");
    } else {
        println!("open-agent-view is bootstrapping; run with --json for diagnostics");
    }

    Ok(())
}

# open-agent-view

`open-agent-view` is an open terminal dashboard for supervising local and
containerized coding agents. Its installed command is:

```console
coding-agents
```

The project is inspired by the interaction model of `claude agents`, while
using its own implementation, identity, and provider-neutral data model. The
planned adapters cover Claude Code, OpenAI Codex, and Docker runtimes.

> [!IMPORTANT]
> This repository is under active development. The initial milestones focus on
> accurately documenting the reference interface, then building a safe,
> fixture-tested dashboard before enabling process control.

## Development

The minimum supported Rust version is 1.75.

```console
cargo test
cargo run -- --help
cargo install --path .
```

See [the approved product specification](docs/product-spec.md),
[architecture](docs/architecture.md), and [exploration notes](docs/exploration/README.md).

## Safety model

- Existing agent sessions and Docker containers are treated as read-only until
  an explicit action is chosen in the UI.
- Destructive actions require confirmation and identify the exact target.
- Containers must opt in through a configured name, label, or explicit CLI
  selection before they can be controlled.
- Authentication material is never copied into project files or logs.

## Status

Pre-alpha. See [ROADMAP.md](ROADMAP.md).

## Non-affiliation

This is an independent project. It is not affiliated with or endorsed by
Anthropic or OpenAI. Claude is a trademark of Anthropic; OpenAI and Codex are
trademarks of OpenAI.


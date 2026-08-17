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
> This repository is pre-alpha. Read-only host and explicitly enrolled Docker
> discovery is implemented. Lifecycle controls remain disabled unless a future
> owning supervisor can prove authority over the selected session.

## What works

- Host Claude discovery through `claude agents --json`.
- Host Codex discovery through the official App Server JSONL protocol.
- Explicit, observe-only Docker targets with immutable container-ID pinning.
- Status and directory grouping, cyclic navigation, collapsible groups,
  peek/reply composition, details, filtering, help, and confirmation states.
- Deterministic JSON output and Ratatui test-backend coverage.

The dashboard intentionally does not claim control over arbitrary Codex
processes: live Codex ownership is local to the App Server process that started
or resumed the thread. See [the Codex exploration](docs/exploration/codex-integration.md).

## Usage

Launch the dashboard:

```console
cargo run --
```

If only Claude is installed on the host:

```console
cargo run -- --no-host-codex
```

Emit normalized JSON, including completed sessions:

```console
cargo run -- --json --all
```

Observe both providers in an explicitly selected, already-running container:

```console
cargo run -- --docker-container my-agent-container
```

The container remains observe-only. Refresh will never start, restart, or stop
it as a side effect.

### Keyboard map

| Key | Action |
| --- | --- |
| `↑`/`↓`, `j`/`k` | Move cyclically across section headers and sessions |
| `enter` | Collapse/expand a section or open session details |
| `space` | Open/close the reply peek |
| `ctrl+s` | Switch between status and directory grouping |
| `/` | Filter sessions |
| `tab` or printable text | Focus the new-session composer |
| `ctrl+r` | Enter rename composition |
| `ctrl+x` | Arm an exact-target stop/delete confirmation |
| `?` | Show contextual shortcuts |
| `esc` | Close the current mode, clear selection, then quit |

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

Pre-alpha. The read-only dashboard milestone is usable; provider-owned process
control and durable supervision are next. See [ROADMAP.md](ROADMAP.md).

## Non-affiliation

This is an independent project. It is not affiliated with or endorsed by
Anthropic or OpenAI. Claude is a trademark of Anthropic; OpenAI and Codex are
trademarks of OpenAI.

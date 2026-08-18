# open-agent-view

`open-agent-view` is an open terminal dashboard for supervising local and
containerized coding agents. Its installed command is:

```console
coding-agents
```

The project is inspired by the interaction model of `claude agents`, while
using its own implementation, identity, and provider-neutral data model. Its
adapters cover Claude Code, OpenAI Codex, and Docker runtimes.

> [!IMPORTANT]
> This repository is pre-alpha. Existing host sessions and explicitly enrolled
> Docker containers are observe-only. Lifecycle controls are enabled only for
> host Claude and Codex sessions launched and recorded by this installation.

## What works

- Host Claude discovery through `claude agents --json`, background launch, and
  ownership-gated stop.
- Host Codex discovery through the official App Server protocol, using a
  user-private Unix WebSocket for durable managed sessions.
- Owned host Codex transcript inspection, idle reply, exact active-turn steer,
  interrupt, archive, and delete through the durable App Server.
- Explicit, observe-only Docker targets with immutable container-ID pinning.
- Hardened managed-Docker create/start/stop/remove commands backed by a private
  external ownership registry and exact label/ID revalidation.
- Status and directory grouping, cyclic navigation, collapsible groups,
  peek/reply composition, details, filtering, help, and confirmation states.
- Native session opening through `claude attach` and `codex resume`, with safe
  terminal suspension/restoration.
- Claude log reconstruction through a VT100 parser for a useful peek instead of
  leaking raw terminal control sequences.
- Deterministic JSON output and Ratatui test-backend coverage.

The dashboard intentionally does not claim control over arbitrary Claude or Codex
processes: live Codex ownership is local to the App Server process that started
or resumed the thread, and existing Claude sessions were not launched by this
tool. See [the control model](docs/control-model.md) and
[Codex exploration](docs/exploration/codex-integration.md).

## Installation

Tagged releases provide a checksum-verified archive for
`x86_64-unknown-linux-gnu`. A pinned Cargo install is available for other
platforms:

```console
cargo +1.75.0 install \
  --locked \
  --git https://github.com/xhluca/open-agent-view \
  --tag v0.1.0 \
  open-agent-view
```

See [installation and release verification](docs/install.md) for the exact
archive download, SHA-256 verification, user-local install, non-destructive
smoke tests, and maintainer tag procedure. No release is published until a
maintainer pushes a matching `vMAJOR.MINOR.PATCH` tag.

## Usage

Launch the dashboard:

```console
coding-agents
```

If only Claude is installed on the host:

```console
coding-agents --no-host-codex
```

Emit normalized JSON, including completed sessions:

```console
coding-agents --json --all
```

Observe both providers in an explicitly selected, already-running container:

```console
coding-agents --docker-container my-agent-container
```

The container remains observe-only. Refresh will never start, restart, or stop
it as a side effect.

New-session prompts launch host Claude by default. Choose a launch directory or
the managed host Codex provider with:

```console
coding-agents --launch-cwd /path/to/project --launch-provider claude
coding-agents --launch-cwd /path/to/project --launch-provider codex
```

Managed Codex tasks use a private Unix-socket App Server that survives dashboard
restarts. Only exact threads and active turns launched through this supervisor
can be interrupted; pre-existing host threads and Docker threads remain
observe/open-only.

Check the installed providers and any explicitly selected containers without
starting the dashboard:

```console
coding-agents doctor
coding-agents doctor --docker-container my-agent-container
```

Create a stopped managed container only from a digest-pinned image, then start
it explicitly:

```console
coding-agents docker create \
  --name oav-agent \
  --image basic-claude-uv@sha256:FULL_64_HEX_DIGEST \
  --workspace /absolute/project \
  --state-home /absolute/dedicated-agent-home
coding-agents docker start oav-agent
coding-agents docker list
```

The two host directories must already exist. The state home is mounted as the
managed container user's entire home; provision only the credentials/state you
intend that container to use. Existing host Claude/Codex homes are never
mounted automatically. Stop and removal require `--yes`, removal refuses a
running container, and neither workspace nor state data is deleted.

### Keyboard map

| Key | Action |
| --- | --- |
| `↑`/`↓` | Move cyclically across section headers and sessions |
| `enter` | Collapse/expand a section or open session details |
| `space` | Open/close transcript peek; compose when Reply is granted |
| `ctrl+s` | Switch between status and directory grouping |
| `/` | Filter sessions |
| `tab` or printable text | Focus the new-session composer |
| `ctrl+r` | Enter rename composition |
| `ctrl+a` | Confirm archive when the selected provider grants authority |
| `ctrl+x` | Arm an exact-target stop/delete confirmation |
| `?` | Show contextual shortcuts |
| `esc` | Close the current mode, or quit directly from the session list |

## Development

The minimum supported Rust version is 1.75.

```console
cargo test --locked
cargo run -- --help
cargo install --path . --locked
```

See [the approved product specification](docs/product-spec.md),
[architecture](docs/architecture.md), [validation record](docs/testing.md),
[installation guide](docs/install.md), and
[exploration notes](docs/exploration/README.md).

## Safety model

- Existing agent sessions and Docker containers are treated as read-only until
  an explicit action is chosen in the UI.
- Destructive actions require confirmation and identify the exact target.
- Containers must opt in through a configured name, label, or explicit CLI
  selection before they can be controlled.
- Container start/stop/remove additionally require a matching private external
  ownership record; labels alone never grant lifecycle authority.
- Authentication material is never copied into project files or logs.

## Status

Pre-alpha. The dashboard supports ownership-gated host Claude launch/stop,
durable managed host Codex lifecycle controls, and ownership-gated managed
Docker lifecycle commands. See [ROADMAP.md](ROADMAP.md) for the remaining
approval UI and distribution work.

## Non-affiliation

This is an independent project. It is not affiliated with or endorsed by
Anthropic or OpenAI. Claude is a trademark of Anthropic; OpenAI and Codex are
trademarks of OpenAI.

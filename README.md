# Open Agent View

**One control surface for all your local coding agents.**

Open Agent View turns concurrent agent sessions into one live terminal queue.
See what needs input, follow work in progress, review completed tasks, and open
the exact native session without losing its history.

![Open Agent View supervising eight local harness targets](docs/assets/open-agent-view.gif)

The interaction model is inspired by `claude agents`, rebuilt as an independent,
open, provider-neutral project.

## Install

Install the prebuilt binary—no Rust or Cargo required:

```console
curl -fsSL https://open-agent-view.github.io/install.sh | bash
```

Then open any project and run:

```console
open-agent-view
```

The short `opav` command is installed too. This is an early private preview;
the current manually published release supports Linux x86-64 and uses your
existing GitHub CLI authentication to download the private release. See the
[installation guide](docs/install.md) for version pinning, updates, and source
builds.

## Why Open Agent View?

- **One attention queue.** Ready-for-review, needs-input, working, completed,
  and unknown sessions stay visible without terminal hopping.
- **Native when it matters.** Press Enter or Right to foreground the selected
  harness. Return to the queue without killing its work.
- **Fast at scale.** Provider discovery is concurrent; grouping and lookup are
  pre-indexed; large queues render terminal-sized pages instead of tens of
  thousands of rows.
- **Honest controls.** Reply, approve, interrupt, archive, and delete appear
  only when OAV can verify the provider and exact session authority.
- **Managed by default.** OAV shows work it created or explicitly manages.
  Provider-wide history is an explicit `--include-external` opt-in.

## Quick examples

```console
# Focus discovery on this project.
open-agent-view --cwd "$PWD"

# Diagnose installed harnesses and authentication state.
open-agent-view doctor

# Guided install/login for one harness.
open-agent-view setup cursor

# Show bounded external provider history when you need it.
open-agent-view --include-external --history-limit 500

# Upgrade to the latest checksummed release.
opav update
```

Inside the dashboard:

- `↑` / `↓` navigate; `enter` or `→` foregrounds a session; `space` inspects.
- Start typing a task, then use `tab` to choose a harness and `shift+tab` to
  choose an account-advertised model.
- `ctrl+x` stops an active managed session. On the same idle row, the next
  `ctrl+x` deletes it where supported or hides it locally.
- `ctrl+f` filters, `ctrl+r` gives a private OAV display name, and `?` opens the
  complete contextual key map.

The [CLI and keyboard guide](docs/cli.md) documents setup, model selection,
foreground/background gestures, completed visibility, paging, and bulk session
commands.

## Harness support

Support is capability-aware rather than pretending every CLI exposes the same
API.

| Harness | Default discovery | Managed path |
| --- | --- | --- |
| Claude Code | OAV-launched sessions | Native attach and verified stop |
| OpenAI Codex | Durable OAV threads | Reply, requests, interrupt, archive, delete |
| Pi | Native OAV tasks; durable RPC on Linux | Reply, input, stop, delete, resume |
| OpenCode | Durable loopback sessions on Linux | Inspect, reply, interrupt, native resume |
| Cursor | OAV-owned runs on Linux | Models, launch, resume, interrupt |
| GitHub Copilot | OAV-owned native/ACP sessions | Reply, cancel, one-shot approval |
| Antigravity | Exact OAV-launched conversations | Models, launch, resume, stop |
| Terminal | OAV-owned shells | Background, resume, stop, delete |

External history, when requested, remains read-only or native-open unless its
provider offers a separately verified control boundary. Exact versions,
protocol evidence, platform limits, and unsupported actions live in the
[provider notes](docs/exploration/README.md).

## Safety

Visibility never implies authority. OAV does not copy credentials into the
project, does not silently scan containers, and refuses mutations when exact
ownership cannot be revalidated. Demo fixtures fence all provider I/O.

Read the [control and ownership model](docs/control-model.md) for the full
contract.

## Documentation

- [Install, update, and uninstall](docs/install.md)
- [CLI and keyboard reference](docs/cli.md)
- [Troubleshooting and recovery](docs/troubleshooting.md)
- [Architecture](docs/architecture.md)
- [Testing and real-TTY evidence](docs/testing.md)
- [Roadmap](ROADMAP.md)
- [Documentation index](docs/README.md)

Contributions are welcome through [CONTRIBUTING.md](CONTRIBUTING.md). Report
security-sensitive findings through [SECURITY.md](SECURITY.md).

## License

[MIT](LICENSE). Open Agent View is independent and is not affiliated with or
endorsed by Anthropic, OpenAI, GitHub, Cursor, the OpenCode project, the Pi
project, or Google.

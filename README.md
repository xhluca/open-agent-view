# Open Agent View

**One terminal for all your coding agents.**

Open Agent View turns agent sessions into a single live queue: see what needs
input, follow work in progress, review completed tasks when needed, and jump
back into the provider's native interface. Run it as `coding-agents`.

![Open Agent View showing Claude, Codex, Pi, OpenCode, Cursor, GitHub Copilot, and Antigravity sessions](docs/assets/open-agent-view.gif)

The interaction model is inspired by `claude agents`, rebuilt as an independent,
open, provider-neutral project.

> [!NOTE]
> Open Agent View is an early private preview. The manually published v0.1.20
> binary currently covers Linux x86-64; collaborators authenticate with GitHub
> to install it. The source remains portable, but macOS and ARM64 artifacts are
> not claimed until they can be built and tested natively.

> [!IMPORTANT]
> v0.1.20 was packaged, checksummed, smoke-tested, and published manually after
> the repository's hosted build service remained unavailable. The release page
> and installer state the exact Linux x86-64 scope.

## Why Open Agent View?

- **Your managed sessions, one queue.** The default view contains only sessions
  created or explicitly managed by Open Agent View. Provider-wide history is a
  deliberate `--include-external` opt-in, and enabled providers refresh
  concurrently so one slow provider does not hide the others.
- **Responsive large queues.** Completed managed sessions are visible by
  default, but each group renders only a terminal-sized page of at most 25
  sessions behind a selectable Show more row. Each provider reads at most 100
  persisted records per refresh by default;
  `--history-limit` changes that explicit budget. Buffered
  key repeats and typed bursts are coalesced so large histories do not flood
  SSH, tmux, or the terminal renderer.
- **Attention first.** Sessions are grouped by ready for review, needs input,
  working, completed, and unknown state.
- **Native when it matters.** Open a selected session in its own CLI without
  abandoning the dashboard workflow.
- **Safe by default.** Finding a session never grants broad authority to change
  it. Mutating controls require either exact local ownership or an exact
  provider-native active-session revalidation.
- **Host and container aware.** Supervise host agents and explicitly enrolled
  Docker targets without silently scanning or restarting containers.

## Install

Collaborators can install from the private release with their existing GitHub
authentication—no Rust, Cargo, or source checkout:

`gh` is GitHub's official CLI and is required only to authenticate to this
private repository.

```console
gh auth login
gh api -H "Accept: application/vnd.github.raw+json" \
  repos/xhluca/open-agent-view/contents/install.sh | bash
```

After the repository and release are public, installation becomes:

```console
curl -fsSL https://raw.githubusercontent.com/xhluca/open-agent-view/main/install.sh | bash
```

The installer selects a release asset for the current host, verifies its
SHA-256 checksum, and installs `coding-agents` under `~/.local/bin`. The current
manual v0.1.20 release is Linux x86-64 only. See the
[installation guide](docs/install.md) for supported platforms, version pinning,
upgrades, the current pre-release boundary, and contributor source builds.

## Quick start

Open the dashboard from any project:

```console
coding-agents
```

Common user-local installs are found automatically, including Codex under
`~/.npm-global/bin` and OpenCode under `~/.opencode/bin`; use the corresponding
`--*-bin` option only for another custom location.

Useful examples:

```console
# Focus on the current project.
coding-agents --cwd "$PWD"

# Start with only active sessions (completed is visible by default).
coding-agents --hide-completed

# Review bounded provider-wide history only when you explicitly need it.
coding-agents --include-external --history-limit 500

# Include external and interactive history in machine-readable output.
coding-agents --json --include-external --include-interactive

# Add one explicitly selected running container.
coding-agents --docker-container my-agent-container

# Check every configured CLI without starting the dashboard.
coding-agents doctor

# Install a missing harness with its official user-local installer.
# Omit --yes for an interactive confirmation.
coding-agents setup cursor --yes

# Choose the initial task harness (Claude is the default).
coding-agents --harness opencode

# Preview a bounded archive batch; add --yes only after reviewing it.
coding-agents sessions archive --older-than-days 30 --limit 100
coding-agents sessions archive --older-than-days 30 --limit 100 --yes

# Review rows hidden only from Open Agent View.
coding-agents sessions hidden
```

Inside the dashboard, use `↑`/`↓` to move, `enter` or `→` to open, `←` to
background the provider UI and return, `space` to inspect, `ctrl+f` to filter,
and `?` for contextual shortcuts. Start typing to hand off a
new task, then press `tab` for a visible harness picker. Use arrows or `tab` to
preview Claude, Codex, Pi, OpenCode, Cursor, or Copilot; `enter` selects and
`esc` returns without losing the draft. `/harness` opens the same picker,
`/harness NAME` selects directly, and `/model` changes supported models
(`/provider` remains an alias). Antigravity is a launch target when `agy` is
installed. From the task composer, `shift+tab` opens the
searchable model picker without losing the draft; the installed provider's
account-scoped catalog loads without blocking typing or navigation. If the
provider needs authentication, the picker changes Enter (or `l`) into a native
login handoff, then reloads the catalog automatically. `/login` starts the same
setup flow for the selected harness. Every row
spells out its provider name; open Peek to see whether it runs on the host or in
Docker. Groups with more matches than the current page end in a selectable
**Show more** row; filtering searches the complete bounded snapshot, including
rows that have not been revealed. Completed managed sessions are visible by
default; use `/completed hide` for an active-only view and `/completed show` to
restore them. Start with `--hide-completed` (`--active-only`) when desired.
These controls do not opt into unrelated provider history; add
`--include-external` explicitly for that. On an active owned row, `ctrl+x` stops
it immediately. After refresh
shows the same row idle, the next `ctrl+x` deletes it when the provider supports
exact deletion, or removes it reversibly from OAV's view otherwise.

## Provider support

“Supported” means Open Agent View uses a documented provider surface and has an
isolated compatibility test. It does not imply equal lifecycle authority for
every CLI.

| Provider | Sessions shown today | Available actions |
| --- | --- | --- |
| Claude Code | OAV-launched host/background sessions and explicit Docker targets by default; other provider history with `--include-external` | Native login; account-advertised model selection; launch directly into full-screen `attach`, with Left returning to OAV; interrupt an exact OAV-owned host background session only after live provider revalidation |
| OpenAI Codex | Durable OAV-managed host threads and explicit Docker targets | Inspect/open; catalog-backed launch, reply/steer, request handling, interrupt, archive, and delete |
| Pi | Durable OAV-managed RPC sessions and their persisted OAV-owned history on Linux by default; unrelated host JSONL history with `--include-external` | Catalog-backed owned launch, inspect/reply/request handling, Ctrl+X stop then exact delete, and completed-session handoff to native Pi; unrelated history remains inspect/native-resume only |
| OpenCode | Durable OAV-managed authenticated loopback sessions on Linux by default; persisted host history with `--include-external` | External history inspect/native resume; catalog-backed owned launch, discovery, inspect, reply, and interrupt; no inline approval/input yet |
| Cursor | OAV-owned managed runs on Linux; no external/global list because the provider exposes only a TTY picker | Native login; exact account model catalog and selected-model launch; discovery, inspect, native resume/reply after idle, and verified interrupt |
| GitHub Copilot CLI | Process-local OAV-owned ACP sessions by default; persisted ACP sessions with `--include-external` | Native login; account model catalog without creating a session; selected-model ACP launch/reply, inspect, cancel, and exact one-shot allow/reject; persisted rows remain observe/native-resume |
| Antigravity CLI | Exact OAV-launched most-recent workspace conversations by default; other documented last-workspace entries with `--include-external` | First-run login, exact model selection, sandboxed full-screen launch, Left-to-background, and native resume; no inline approval or arbitrary all-history API |

Claude and Codex have managed paths. Linux adds durable Pi and OpenCode plus
OAV-owned Cursor control. Copilot control lasts for the dashboard process's
retained ACP connection. Unrelated provider records remain read-only/native-open.
Antigravity can rediscover only the provider's documented last conversation per
workspace. On non-Linux platforms, Pi and OpenCode keep their
history/native-open paths. See the [provider exploration notes](docs/exploration/README.md)
for tested versions, isolation setup, protocol observations, and boundaries.

## Safety model

Open Agent View separates **visibility** from **authority**. External sessions
are hidden unless explicitly requested and remain observe-only when shown.
Explicitly selected containers are also observe-only unless the installation
can prove it created and still owns the exact target. Credentials are never
copied into the project.

The [control and ownership model](docs/control-model.md) documents the exact
rules. Managed Docker additionally requires an immutable container ID,
digest-pinned image, protected external owner record, and explicit lifecycle
command.

## Documentation

- [Install, upgrade, and uninstall](docs/install.md)
- [CLI and keyboard reference](docs/cli.md)
- [Troubleshooting and recovery](docs/troubleshooting.md)
- [Control and ownership model](docs/control-model.md)
- [Architecture](docs/architecture.md)
- [Testing and real-TTY validation](docs/testing.md)
- [Roadmap and current status](ROADMAP.md)
- [Complete documentation index](docs/README.md)

Contributions are welcome through [CONTRIBUTING.md](CONTRIBUTING.md). Please
report security-sensitive findings through [SECURITY.md](SECURITY.md).

## License and non-affiliation

Open Agent View is available under the [MIT License](LICENSE). It is independent
and is not affiliated with or endorsed by Anthropic, OpenAI, GitHub, Cursor, the
OpenCode project, the Pi project, or Google.

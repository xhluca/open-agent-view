# Open Agent View

**One terminal for all your coding agents.**

Open Agent View turns agent sessions into a single live queue: see what needs
input, follow work in progress, review completed tasks when needed, and jump
back into the provider's native interface. Run it as `coding-agents`.

![Open Agent View showing Claude, Codex, Pi, OpenCode, Cursor, GitHub Copilot, and Antigravity sessions](docs/assets/open-agent-view.gif)

The interaction model is inspired by `claude agents`, rebuilt as an independent,
open, provider-neutral project.

> [!NOTE]
> Open Agent View is an early private preview. Prebuilt releases cover Linux
> and macOS on x86-64 and ARM64; collaborators authenticate with GitHub to
> install them. Provider capabilities and compatibility boundaries may still
> evolve before a public stable release.

> [!IMPORTANT]
> The latest published private release remains v0.1.10. GitHub Actions rejected
> v0.1.12's native packaging jobs at the account billing/spending gate before
> any build ran; the current checkout is the unreleased v0.1.13 candidate with
> the large-history and executable-discovery fixes below. The repository owner
> must resolve that GitHub setting before the installer can deliver a newer
> complete platform matrix; no partial release is presented as complete.

## Why Open Agent View?

- **All sessions, one queue.** Enabled providers refresh concurrently and one
  slow or unavailable provider does not hide results from the others.
- **Responsive large queues.** Each group initially shows a terminal-sized page
  of at most 25 sessions behind a selectable Show more row, and completed
  history is excluded unless `--all` is explicit. When history is requested,
  each provider reads at most 100 persisted records per refresh by default;
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

The installer selects a native Linux or macOS binary, verifies its SHA-256
checksum, and installs `coding-agents` under `~/.local/bin`. See the
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

# Start with completed sessions visible (they are hidden by default).
coding-agents --all

# Load a larger persisted-history window when you actually need it.
coding-agents --all --history-limit 500

# Include persisted interactive history in machine-readable output.
coding-agents --json --all --include-interactive

# Add one explicitly selected running container.
coding-agents --docker-container my-agent-container

# Check every configured CLI without starting the dashboard.
coding-agents doctor

# Choose the initial task harness (Claude is the default).
coding-agents --harness opencode

# Preview a bounded archive batch; add --yes only after reviewing it.
coding-agents sessions archive --older-than-days 30 --limit 100
coding-agents sessions archive --older-than-days 30 --limit 100 --yes

# Review rows hidden only from Open Agent View.
coding-agents sessions hidden
```

Inside the dashboard, use `↑`/`↓` to move, `enter` to open, `space` to inspect,
`ctrl+f` to filter, and `?` for contextual shortcuts. Start typing to hand off a
new task, then press `tab` for a visible harness picker. Use arrows or `tab` to
preview Claude, Codex, Pi, OpenCode, Cursor, or Copilot; `enter` selects and
`esc` returns without losing the draft. `/harness` opens the same picker,
`/harness NAME` selects directly, and `/model` changes supported models
(`/provider` remains an alias). From the task composer, `shift+tab` opens the
searchable model picker without losing the draft; the installed provider's
catalog loads without blocking typing or navigation. Every row
spells out its provider name; open Peek to see whether it runs on the host or in
Docker. Groups with more matches than the current page end in a selectable
**Show more** row; filtering searches the complete bounded snapshot, including
rows that have not been revealed. Completed history is hidden before discovery by default; use
`/completed show` inside the dashboard or start with `--all`, then use
`/completed hide` to return to the active queue. `ctrl+x` stops or deletes only
when exact provider authority exists; otherwise it offers a reversible local
hide instead of changing provider history.

## Provider support

“Supported” means Open Agent View uses a documented provider surface and has an
isolated compatibility test. It does not imply equal lifecycle authority for
every CLI.

| Provider | Sessions shown today | Available actions |
| --- | --- | --- |
| Claude Code | Live host/background sessions and explicit Docker targets | Inspect and native open; launch with a catalog-backed model picker; interrupt an exact host background session only after live provider revalidation |
| OpenAI Codex | Durable OAV-managed host threads and explicit Docker targets | Inspect/open; catalog-backed launch, reply/steer, request handling, interrupt, archive, and delete |
| Pi | Documented host JSONL history plus durable OAV-managed RPC sessions on Linux | Inspect/native resume for history; catalog-backed owned launch, reply/steer, request handling, and interrupt |
| OpenCode | Persisted host history, plus durable OAV-managed authenticated loopback sessions on Linux | External history inspect/native resume; catalog-backed owned launch, discovery, inspect, reply, and interrupt; no inline approval/input yet |
| Cursor | OAV-owned managed runs on Linux; no external/global list because the provider exposes only a TTY picker | Owned launch, discovery, inspect, native resume/reply after idle, and verified interrupt |
| GitHub Copilot CLI | Persisted host sessions from ACP `session/list`, plus process-local OAV-owned ACP sessions | Persisted rows observe/native resume; owned launch/reply, inspect, cancel, and exact one-shot allow/reject |
| Antigravity CLI | The documented most-recent conversation for each host workspace | Native resume; cache entries remain observe-only |

Claude and Codex have managed paths. Linux adds durable Pi and OpenCode plus
OAV-owned Cursor control. Copilot control lasts for the dashboard process's
retained ACP connection. Antigravity and unrelated provider records remain
read-only/native-open. On non-Linux platforms, Pi and OpenCode keep their
history/native-open paths. See the [provider exploration notes](docs/exploration/README.md)
for tested versions, isolation setup, protocol observations, and boundaries.

## Safety model

Open Agent View separates **visibility** from **authority**. Existing sessions
and explicitly selected containers are observe-only unless the installation can
prove it created and still owns the exact target. Destructive actions are
capability-gated and confirmed; credentials are never copied into the project.

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

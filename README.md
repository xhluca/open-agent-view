# Open Agent View

**One terminal for all your coding agents.**

Open Agent View turns agent sessions into a single live queue: see what needs
input, follow work in progress, review completed tasks when needed, and jump
back into the provider's native interface. Run it as `open-agent-view`.

![Open Agent View showing seven coding agents and an OAV-managed terminal](docs/assets/open-agent-view.gif)

The interaction model is inspired by `claude agents`, rebuilt as an independent,
open, provider-neutral project.

> [!NOTE]
> Open Agent View is an early private preview. The manually published v0.1.32
> binary currently covers Linux x86-64; collaborators authenticate with GitHub
> to install it. The source remains portable, but macOS and ARM64 artifacts are
> not claimed until they can be built and tested natively.

> [!IMPORTANT]
> v0.1.32 was packaged, checksummed, smoke-tested, and published manually after
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
SHA-256 checksum, and installs `open-agent-view` under `~/.local/bin`, plus the
short `opav` command. The current
manual v0.1.32 release is Linux x86-64 only. See the
[installation guide](docs/install.md) for supported platforms, version pinning,
upgrades, the current pre-release boundary, and contributor source builds.

## Quick start

Open the dashboard from any project:

```console
open-agent-view
```

Common user-local installs are found automatically, including Codex under
`~/.npm-global/bin` and OpenCode under `~/.opencode/bin`; use the corresponding
`--*-bin` option only for another custom location.

Useful examples:

```console
# Focus on the current project.
open-agent-view --cwd "$PWD"

# Start with only active sessions (completed is visible by default).
open-agent-view --hide-completed

# Review bounded provider-wide history only when you explicitly need it.
open-agent-view --include-external --history-limit 500

# Include external and interactive history in machine-readable output.
open-agent-view --json --include-external --include-interactive

# Add one explicitly selected running container.
open-agent-view --docker-container my-agent-container

# Check every configured CLI without starting the dashboard.
open-agent-view doctor

# Show this installation's version, or replace it with the latest verified release.
opav -v
opav update                 # prints X → Y; `opav upgrade` is an alias

# Open the guided install/login flow for any harness.
open-agent-view setup copilot

# Choose the initial task harness (Claude is the default).
open-agent-view --harness opencode

# Preview a bounded archive batch; add --yes only after reviewing it.
open-agent-view sessions archive --older-than-days 30 --limit 100
open-agent-view sessions archive --older-than-days 30 --limit 100 --yes

# Review rows hidden only from Open Agent View.
open-agent-view sessions hidden

# Give one stable session ID a private dashboard name, then clear it later.
open-agent-view sessions rename 'codex:host:EXACT_ID' 'release captain'
open-agent-view sessions reset-name 'codex:host:EXACT_ID'
```

Inside the dashboard, use `↑`/`↓` to move, `enter` or `→` to open, and `space`
to inspect. Inside a provider, plain arrows edit normally; at a cursor boundary,
repeat Left or Right during the visible countdown to return, or use
`shift+←`/`shift+→` immediately. Use `ctrl+f` to filter and `?` for contextual
shortcuts. Start typing to hand off a
new task, then press `tab` for a visible harness picker. Use arrows or `tab` to
preview Claude, Codex, Pi, OpenCode, Cursor, Copilot, Antigravity, or a plain
Terminal; `enter` selects and
`esc` returns without losing the draft. `/harness` opens the same picker,
`/harness NAME` selects directly, and `/model` changes supported models
(`/provider` remains an alias). Antigravity is a launch target when `agy` is
installed. `/setup NAME` checks installation and opens that harness's own login
in an isolated terminal; press Left/Right twice at a cursor boundary, or
Shift+Left/Right anywhere, to background setup so it can be
resumed from the dashboard without attaching to an unrelated agent. From the
task composer, `shift+tab` opens the
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
`ctrl+r` edits a private OAV display name; it never renames provider history.
If the title is changed inside Claude, Codex, or another native harness, OAV
shows that provider title after refresh unless a local name is set. A local name
wins until it is cleared by submitting an empty rename or with `sessions
reset-name`.

## Provider support

“Supported” means Open Agent View uses a documented provider surface and has an
isolated compatibility test. It does not imply equal lifecycle authority for
every CLI.

| Provider | Sessions shown today | Available actions |
| --- | --- | --- |
| Claude Code | OAV-launched host/background sessions and explicit Docker targets by default; other provider history with `--include-external` | Native login; account-advertised model selection; asynchronous `--background` bootstrap using Claude's returned ID, then full-screen `attach`, with boundary-double-arrow or Shift+Arrow returning to OAV; interrupt an exact OAV-owned host background session only after live provider revalidation |
| OpenAI Codex | Durable OAV-managed host threads and explicit Docker targets | Catalog-backed launch auto-opens the exact native thread; inspect, reply/steer, request handling, interrupt, archive, and delete |
| Pi | Native foreground tasks saved in OAV's managed history plus durable OAV-managed RPC sessions on Linux; unrelated host JSONL history with `--include-external` | Catalog-backed foreground launch, inspect/reply/request handling for RPC-owned work, Ctrl+X stop then exact delete, and native resume; unrelated history remains inspect/native-resume only |
| OpenCode | Durable OAV-managed authenticated loopback sessions on Linux by default; persisted host history with `--include-external` | Catalog-backed owned launch auto-attaches the exact native TUI; external history inspect/native resume; managed inspect, reply, and interrupt; no inline approval/input yet |
| Cursor | OAV-owned managed runs on Linux; no external/global list because the provider exposes only a TTY picker | Native login; exact account model catalog; selected-model launches open Cursor in the foreground; discovery, inspect, native resume/reply after idle, and verified interrupt |
| GitHub Copilot CLI | Native foreground tasks with durable OAV-owned IDs and current persisted message previews; provider-wide ACP history with `--include-external` | Native login and account model catalog; selected-model launches open Copilot's full UI immediately; ACP reply/inspect/cancel and exact one-shot allow/reject remain available for connection-owned rows; native handoff supports ACP builds with or without optional `session/close` |
| Antigravity CLI | Every exact OAV-launched conversation by default; other documented last-workspace entries with `--include-external` | First-run login, private cached account model catalog, sandboxed full-screen launch, immediate live-row correlation, boundary-double-arrow/Shift+Arrow backgrounding, stop, and native resume; a failed native catalog opens recovery rather than accepting an unverified model ID |
| Terminal | Process-local shells created by this dashboard | Full-screen interactive shell, boundary-double-arrow/Shift+Arrow backgrounding, exact resume, Ctrl+X stop, then Ctrl+X delete; provider setup terminals use the same isolated bridge |

Claude and Codex have managed paths. Linux adds durable Pi and OpenCode plus
OAV-owned Cursor control. Copilot control lasts for the dashboard process's
retained ACP connection. Unrelated provider records remain read-only/native-open.
Antigravity's external discovery remains limited to the provider's documented
last conversation per workspace; OAV-owned launches use their exact local
conversation IDs. On non-Linux platforms, Pi and OpenCode keep their
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

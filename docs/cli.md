# CLI and keyboard reference

This reference describes the current checkout. `coding-agents --help` and
`coding-agents <subcommand> --help` remain authoritative for the installed
binary.

## Dashboard and JSON options

```text
coding-agents [OPTIONS]
```

| Option | Meaning |
| --- | --- |
| `--json` | Print a normalized snapshot and do not enter the TUI. |
| `--all` | Include completed sessions in JSON; the TUI already includes them. |
| `--include-interactive` | Include provider sessions reported as foreground/interactive. |
| `--cwd PATH` | Keep sessions whose working directory starts with `PATH`. |
| `--fixture FILE` | Read a normalized snapshot/session array instead of probing providers; all provider operations are fenced. |
| `--no-host-providers` | Disable every host provider while retaining explicit Docker targets. |
| `--claude-bin PATH` | Use a particular Claude executable; default `claude`. |
| `--no-host-claude` | Disable host Claude discovery and control. |
| `--codex-bin PATH` | Use a particular Codex executable; default `codex`. |
| `--no-host-codex` | Disable host Codex discovery and supervision. |
| `--pi-bin PATH` / `--pi-session-dir PATH` | Select Pi and optionally override its documented history store. |
| `--no-host-pi` | Disable host Pi history and managed supervision. |
| `--opencode-bin PATH` / `--no-host-opencode` | Select or disable host OpenCode discovery. |
| `--copilot-bin PATH` / `--no-host-copilot` | Select or disable GitHub Copilot CLI discovery. |
| `--cursor-bin PATH` / `--no-host-cursor` | Select or disable host Cursor support. |
| `--antigravity-bin PATH` / `--no-host-antigravity` | Select or disable host Antigravity discovery. |
| `--docker-container NAME_OR_ID` | Observe both providers in one explicitly selected running container; repeatable. |
| `--docker-bin PATH` | Use a particular Docker executable; default `docker`. |
| `--launch-provider claude\|codex\|pi` | Provider for new-session prompts; default Claude. Pi managed launch currently requires Linux. |
| `--launch-cwd PATH` | Working directory for newly launched host sessions; default current directory. |
| `--refresh-ms N` | Refresh interval, at least 250 ms; default 1500 ms. |

The `--managed-docker-registry PATH` global option applies to the managed
Docker subcommands described below. Provider discovery warnings appear in the
snapshot rather than hiding healthy sessions from another adapter.

Fixture mode is intentionally non-operational even when the JSON advertises
synthetic capabilities: launch, inspect, open, reply, approve/decline,
structured response, interrupt, archive, and delete all refuse before provider
I/O. This makes committed fixtures safe for real-TTY interaction tests.

Useful read-only invocations:

```console
coding-agents --json --all
coding-agents --json --no-host-claude --no-host-codex
coding-agents --json --cwd /absolute/project
coding-agents doctor
coding-agents doctor --json
coding-agents doctor --docker-container exact-name-or-id
```

`doctor` checks executable availability and explicitly named Docker targets. It
does not launch, stop, or modify a provider session or container. A missing
optional host provider is a warning; failure to verify an explicitly requested
container is an error and produces a nonzero exit status.

## Managed Docker lifecycle

Managed Docker is distinct from `--docker-container`. The latter enrolls one
already-running container for observation only. Lifecycle authority exists
only when Open Agent View created the container and its exact immutable ID,
random instance label, and protected external owner record still agree.

Create the mount sources first. Do not make the state home a parent or child of
the workspace:

```console
install -d /absolute/project /absolute/dedicated-agent-home
coding-agents docker create \
  --name oav-agent \
  --image registry.example/agents/runtime@sha256:FULL_64_HEX_DIGEST \
  --workspace /absolute/project \
  --state-home /absolute/dedicated-agent-home \
  --network bridge
```

Creation validates and canonicalizes both directories, requires a digest-pinned
image, creates a stopped container, re-inspects its labels and full ID, and only
then writes the owner record. It does not copy credentials. The workspace is
mounted at `/workspace`; the dedicated state home becomes `/home/agent` and
the container's `HOME`. Both mounts are persistent bind mounts.

The default container identity is the invoking effective UID/GID. Use
`--uid N --gid N` together only when the image and host-directory permissions
require another non-root identity. The default network is Docker's `bridge`.
`--network none` and an existing named Docker network are accepted; host and
`container:...` network sharing are deliberately refused. Creation also uses
an init, drops all capabilities, enables `no-new-privileges`, sets a PID limit,
and runs `sleep infinity`. It does not make the image root filesystem read-only.

Every later command accepts the registered name or immutable ID and revalidates
the immutable identity before acting:

```console
coding-agents docker list
coding-agents docker status oav-agent
coding-agents docker start oav-agent
coding-agents docker stop oav-agent --yes
coding-agents docker status oav-agent --json
coding-agents docker remove oav-agent --yes
```

`start` refuses an already-running container. `stop` refuses a stopped
container and gives Docker ten seconds before its ordinary stop behavior.
`remove` refuses a running container and does not use force or volume-removal
flags. It retains both host directories and forgets the owner record only after
Docker confirms removal. `stop` and `remove` require the literal `--yes`; there
is no interactive CLI prompt.

The default owner registry is:

```text
$XDG_STATE_HOME/open-agent-view/managed-docker/owners.json
```

or, when `XDG_STATE_HOME` is unset:

```text
~/.local/state/open-agent-view/managed-docker/owners.json
```

Use the same `--managed-docker-registry /absolute/path/owners.json` on every
managed-Docker invocation when overriding this location. The registry's parent
must be a real current-user-owned `0700` directory and the existing file must
be a real current-user-owned `0600` regular file. Do not hand-edit it to adopt
an existing container; labels or a record alone are intentionally insufficient.

All Docker lifecycle/status commands support `--json`. JSON status contains
the immutable container ID, random instance ID, optional name/image, normalized
state, and a redacted detail string. It excludes labels, environment values,
and mount details.

## TUI keys and mode behavior

Rows use compact provider/runtime markers to leave more width for the task
summary and right-aligned metadata:

| Marker | Meaning |
| --- | --- |
| `C@H` | Claude on the host |
| `C@D` | Claude in an explicitly enrolled Docker container |
| `X@H` | Codex on the host |
| `X@D` | Codex in an explicitly enrolled Docker container |
| `P@H` | Pi on the host |
| `O@H` | OpenCode on the host |
| `R@H` | Cursor on the host |
| `G@H` | GitHub Copilot CLI on the host |
| `A@H` | Antigravity CLI on the host |
| `?@H` / `?@D` | Unknown/future provider on host / in Docker |

The `X` distinguishes Codex from Claude while keeping every marker three
columns wide. Peek expands the selected row to its full provider name and host
or container runtime label.

| Context | Key | Result |
| --- | --- | --- |
| Session list | `↑` / `↓` | Move cyclically through group headings and rows. |
| Group heading | `enter` | Collapse or expand the group. |
| Session row | `enter` | Suspend the dashboard and open the provider-native interface. |
| Session row | `space` | Inspect transcript/request details when capability is advertised. |
| Inspect peek | type, `enter` | Send an owned provider reply/steer or the current structured answer. |
| Inspect peek | `y` / `n` | Allow once / deny only when the exact capability is advertised. |
| Inspect peek | `enter` with no text | Open the provider-native interface. |
| Session list | `ctrl+s` | Toggle status and working-directory grouping. |
| Session list | `/` | Edit the case-insensitive name/summary/path/provider filter. |
| Session list | `tab` or printable text | Compose a new host task for the configured launch provider/directory. |
| Writable composer | `ctrl+j` | Insert a newline rather than submit. |
| Writable composer | `backspace` | Remove the last character. |
| Session row | `ctrl+r` | Enter rename composition; submission currently reports unsupported. |
| Idle owned Codex row | `ctrl+a`, then `enter` | Confirm archive. |
| Owned row | `ctrl+x`, then `enter` or `ctrl+x` | Confirm exact interrupt when active or delete when completed. |
| Completed group | `ctrl+x`, then `enter` or `ctrl+x` | Delete only when every member grants Delete. |
| Any ordinary view | `?` | Open contextual help; `?`, `enter`, or `esc` closes it. |
| Any overlay/composer | `esc` | Cancel that mode and discard its unsubmitted input. |
| Session list | `esc` | Quit immediately and restore the terminal. |
| Empty session list | `q` | Quit; when a row is selected, printable `q` starts a task like other text. |

Controls are capability-driven. A key listed here can safely do nothing or
show an authority notice for an observe-only, mismatched, expired, or otherwise
unsupported target. Approval `y` is never offered for a file change lacking a
correlated diff, expanded permissions, or unknown request form. See the
[control model](control-model.md) for the exact boundary.

## Runtime state paths

Under `$XDG_STATE_HOME/open-agent-view/`, or `~/.local/state/open-agent-view/`
when `XDG_STATE_HOME` is unset, the current implementation stores:

| Path | Purpose |
| --- | --- |
| `ownership.json` | Exact host Claude session prefixes launched here. |
| `codex-supervisor/` | Detached App Server record, socket, locks, log, and owned Codex thread/turn IDs. |
| `pi/` | Detached Linux RPC supervisor record, socket, locks/logs, and OAV-owned Pi session history. |
| `managed-docker/owners.json` | Exact external proof for managed-container lifecycle. |

These files contain authority metadata and should not be shared between users.
They do not contain collected Codex structured answers. Removal or repair has
safety consequences; follow [troubleshooting and recovery](troubleshooting.md)
instead of deleting state speculatively.

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
| `--all` | Include completed sessions in the dashboard or JSON output. They are excluded by default. |
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
| `--opencode-bin PATH` / `--no-host-opencode` | Select or disable OpenCode history plus durable managed supervision on Linux. |
| `--copilot-bin PATH` / `--no-host-copilot` | Select or disable persisted Copilot discovery and process-local managed ACP control. |
| `--cursor-bin PATH` / `--no-host-cursor` | Select or disable OAV-owned managed Cursor support on Linux. Cursor has no machine-readable global list. |
| `--antigravity-bin PATH` / `--no-host-antigravity` | Select or disable host Antigravity discovery. |
| `--docker-container NAME_OR_ID` | Observe Claude and Codex in one explicitly selected running container; repeatable. |
| `--docker-bin PATH` | Use a particular Docker executable; default `docker`. |
| `--harness` / `--launch-provider claude\|codex\|pi\|opencode\|cursor\|copilot` | Initial harness for new-session prompts; default Claude. Managed Pi, OpenCode, and Cursor launch require Linux; Copilot authority lasts for this dashboard process. |
| `--launch-cwd PATH` | Working directory for newly launched host sessions; default current directory. |
| `--refresh-ms N` | Refresh interval, at least 250 ms; default 15000 ms. Refresh runs off the input thread, and first-launch results appear provider by provider. Use `ctrl+l` for an immediate refresh. |

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
coding-agents --all
coding-agents --json --no-host-claude --no-host-codex
coding-agents --json --cwd /absolute/project
coding-agents doctor
coding-agents doctor --json
coding-agents doctor --docker-container exact-name-or-id
```

`--harness` chooses the initial composer harness; `--launch-provider` remains
an alias. In the new-task composer, `tab` opens a palette containing
only configured launch-capable harnesses. Arrow keys or `tab` preview, `1`–`9`
select directly, `enter` confirms, and `esc` returns without changing the
harness or losing the draft. `/harness NAME` selects explicitly;
`/provider NAME` remains a compatibility alias. Claude and Codex also accept
`/model NAME` or `/model default`; the selected harness/model is always
displayed in the composer border before submission. Pi, OpenCode, Cursor, and
Copilot currently use their provider default model. Copilot retains one process-local ACP control
connection for sessions launched by the current dashboard. A later dashboard
may still list a persisted Copilot session, but that row is
observe/native-open rather than silently inheriting control.

`doctor` checks executable availability and explicitly named Docker targets. It
does not launch, stop, or modify a provider session or container. A missing
optional host provider is a warning; failure to verify an explicitly requested
container is an error and produces a nonzero exit status.

## Completed history and bulk archive

The default dashboard and default JSON snapshot exclude completed sessions.
This filter is applied both by adapters and again at the central discovery
boundary: Claude is queried without `--all`, OpenCode's global
persisted-history query is not started at all, and rows returned in violation
of a provider's active-only/cwd/interactive contract are removed before a
partial snapshot reaches the UI. The header shows `completed hidden` rather
than a misleading zero. Use `coding-agents --all` or `coding-agents --json
--all` only when completed history is needed.

Provider-native bulk archive is currently available for exact OAV-owned,
completed host Codex threads. The first command is always a read-only preview:

```console
coding-agents sessions archive
coding-agents sessions archive --cwd /absolute/project --older-than-days 30 --limit 100
coding-agents --json sessions archive --older-than-days 30
```

The report distinguishes all completed threads seen, those matching the
directory/age scope, those with exact Archive authority, and the bounded batch
selected. It lists skipped matched threads that are visible but unowned. To
apply the reviewed batch, repeat the exact command with `--yes`:

```console
coding-agents sessions archive --cwd /absolute/project --older-than-days 30 --limit 100 --yes
```

The default batch limit is 100 and the maximum is 1,100, matching the guarded
Codex discovery ceiling. Every archive is independently revalidated against
the live owning App Server; one refusal is reported without granting authority
to or silently skipping the remaining selected records. Fixture mode, disabled
host Codex, missing Codex, active threads, external threads, Docker threads,
and providers without a documented archive operation are refused or reported
as ineligible. Open Agent View does not call deletion an archive.

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

Every session row spells out its provider name: Claude, Codex, Pi, OpenCode,
Cursor, GitHub Copilot, Antigravity, or the adapter-provided name for a future
provider. Provider identity takes priority over task summary width on narrow
terminals. Peek expands the selected row with the full host or container runtime
label.

| Context | Key | Result |
| --- | --- | --- |
| Session list | `↑` / `↓` | Move cyclically through group headings and rows. |
| Show more row | `enter` | Reveal the next 25 matching sessions in that group. |
| Group heading | `enter` | Collapse or expand the group. |
| Session row | `enter` | Suspend the dashboard and open the provider-native interface. Claude first explains that `ctrl+z`, not `←`, returns to Open Agent View. |
| Session row | `space` | Inspect transcript/request details when capability is advertised. |
| Inspect peek | type, `enter` | Send an owned provider reply/steer or the current structured answer. |
| Inspect peek | `y` / `n` | Allow once / deny only when the exact capability is advertised. |
| Inspect peek | `enter` with no text | Open the provider-native interface. |
| Session list | `ctrl+s` | Toggle status and working-directory grouping. |
| Session list | `ctrl+f` | Edit the case-insensitive name/summary/path/provider filter. |
| Session list | `ctrl+l` | Request an immediate provider refresh. |
| Session list | `tab`, `/`, or printable text | Compose a new host task. `/` begins a dashboard command rather than a filter. |
| New-task composer | `tab` | Open the visible harness picker. |
| Harness picker | `↑` / `↓`, `←` / `→`, or `tab` / `shift+tab` | Preview configured launch-capable harnesses with wraparound. |
| Harness picker | `enter` or `1`–`9` | Select the highlighted or numbered harness and return to the unchanged draft; changing harness resets the model to its default. |
| Harness picker | `esc` | Return to the unchanged draft without switching harnesses. |
| New-task composer | `/harness` / `/harness NAME` | Open the picker or directly select Claude, Codex, Pi, OpenCode, Cursor, or Copilot when its launch controller is available; `/provider` is an alias. |
| New-task composer | `/model NAME` / `/model default` | Select or reset a Claude/Codex model; unsupported providers refuse locally. |
| New-task composer | `/filter TEXT` / `/help` | Apply a session filter or list dashboard slash commands without contacting a provider. |
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

Paging affects only the interactive list. Counts, filtering, JSON output, and
group-level safety checks always use the complete discovered session set. The
revealed count is remembered across ordinary provider refreshes and reset when
switching views or applying a filter, keeping a newly narrowed queue bounded.

For active host Claude background rows, Interrupt is provider-native rather
than registry-owned: immediately before `claude stop`, Open Agent View reruns
`claude agents --json` and requires the exact full UUID to remain a host
background session in an active state. Interactive, completed, Docker,
missing, or changed rows are refused. Ctrl+X remains a two-step confirmation.

Managed Cursor rows on Linux expose Inspect and either Interrupt while the
verified owned process is active or Reply after it becomes idle. Managed
Cursor native open is likewise refused until the active process exits. Managed
Copilot rows expose Inspect, Reply while idle, Cancel while a prompt is active,
and only the exact `allow_once`/`reject_once` choices offered by a pending ACP
permission request. Persisted Copilot rows from `session/list` do not inherit
those controls.

Managed OpenCode rows on Linux expose Inspect and Reply; while the owned server
reports active work they also expose Interrupt. They refuse native open through
a second server and do not yet expose provider permission or structured-input
requests. External OpenCode history remains inspect/native-open only.

## Runtime state paths

Under `$XDG_STATE_HOME/open-agent-view/`, or `~/.local/state/open-agent-view/`
when `XDG_STATE_HOME` is unset, the current implementation stores:

| Path | Purpose |
| --- | --- |
| `ownership.json` | Exact host Claude session prefixes launched here. |
| `codex-supervisor/` | Detached App Server record, socket, locks, log, and owned Codex thread/turn IDs. |
| `pi/` | Detached Linux RPC supervisor record, socket, locks/logs, and OAV-owned Pi session history. |
| `opencode/` | Private authenticated-loopback server record, lock, log, and exact OAV-owned OpenCode session IDs. |
| `cursor/` | Linux ownership registry, process identities, locks, and bounded logs for OAV-owned Cursor runs. |
| `managed-docker/owners.json` | Exact external proof for managed-container lifecycle. |

These files contain authority metadata and should not be shared between users.
They do not contain collected Codex structured answers. Copilot ACP authority
is held in memory and has no OAV state path. Removal or repair has safety
consequences; follow [troubleshooting and recovery](troubleshooting.md) instead
of deleting state speculatively.

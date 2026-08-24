# CLI and keyboard reference

This reference describes the current checkout. `open-agent-view --help` and
`open-agent-view <subcommand> --help` remain authoritative for the installed
binary. `opav` is the installed shorthand; `coding-agents` remains a guarded
legacy alias. All three execute the same canonical binary and report
`open-agent-view VERSION`.

## Dashboard and JSON options

```text
open-agent-view [OPTIONS]
```

Version and self-update commands:

```console
open-agent-view --version   # -v and -V are accepted
opav update
opav upgrade               # alias of update
```

`update` downloads the repository's current installer (using authenticated
`gh api` first for the private preview, then public `curl` as a fallback) and
runs it for the current install directory. The installer still resolves a
published release asset and verifies its SHA-256 checksum before replacing the
binary. `OAV_REPO` and `OAV_INSTALL_DIR` retain their documented installer
overrides. The final line verifies the binary that was installed and reports
`Updated Open Agent View from X to Y.`; if both versions match, it reports that
Open Agent View is already up to date.

| Option | Meaning |
| --- | --- |
| `--json` | Print a normalized snapshot and do not enter the TUI. |
| `--all` | Compatibility flag that explicitly includes completed sessions; completed is already the default. |
| `--hide-completed` / `--active-only` | Hide completed sessions at startup. `/completed show` restores them without restarting. |
| `--include-interactive` | Include provider sessions reported as foreground/interactive. |
| `--include-external` | Include provider sessions not created or managed by Open Agent View. External history is excluded by default. |
| `--history-limit N` | Read at most `N` persisted-history records per provider per refresh; default 100, range 1–10,000. Live/owned inventories are separate. |
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
| `--harness` / `--launch-provider claude\|codex\|pi\|opencode\|cursor\|copilot\|antigravity\|terminal` | Initial harness for new-session prompts; default Claude. Managed Pi, OpenCode, and Cursor launch require Linux; Copilot authority lasts for this dashboard process; Antigravity uses its native full-screen UI; Terminal opens the user's shell. |
| `--launch-cwd PATH` | Working directory for newly launched host sessions; default current directory. |
| `--refresh-ms N` | Refresh interval, at least 250 ms; default 15000 ms. Refresh runs off the input thread, and first-launch results appear provider by provider. Use `ctrl+l` for an immediate refresh. |

The `--managed-docker-registry PATH` global option applies to the managed
Docker subcommands described below. Provider discovery warnings appear in the
snapshot rather than hiding healthy sessions from another adapter.

Bare provider command defaults are resolved from `PATH` first, then from the
provider's conventional user-local install directories. This includes
`~/.local/bin`, `~/.npm-global/bin` for Codex/Copilot,
`~/.opencode/bin`/`~/.bun/bin` for OpenCode, `~/.cursor/bin`, and
`~/.antigravity/bin`. An explicit path is never replaced by a guessed one.

Fixture mode is intentionally non-operational even when the JSON advertises
synthetic capabilities: launch, inspect, open, reply, approve/decline,
structured response, interrupt, archive, and delete all refuse before provider
I/O. This makes committed fixtures safe for real-TTY interaction tests.

Useful read-only invocations:

```console
open-agent-view --json
open-agent-view --hide-completed
open-agent-view --include-external --history-limit 500
open-agent-view --json --no-host-claude --no-host-codex
open-agent-view --json --cwd /absolute/project
open-agent-view doctor
open-agent-view doctor --json
open-agent-view doctor --docker-container exact-name-or-id
```

Install one missing provider CLI through its official user-local installer:

```console
open-agent-view setup HARNESS
open-agent-view setup HARNESS --yes
```

`HARNESS` accepts the seven coding-agent values plus `terminal` (which is built
in and needs no installation). Without `--yes`, setup requires
an interactive terminal confirmation naming the exact download/package source.
In non-interactive use it refuses before network or installer execution. Shell
installers are staged in a private temporary directory, download with a visible
curl progress bar, and are removed afterward. npm installs retain npm's native
progress. Restart the dashboard after setup so executable discovery runs again.

`--harness` chooses the initial composer harness; `--launch-provider` remains
an alias. In the new-task composer, `tab` opens a palette containing only
configured launch-capable harnesses. Arrow keys or `tab` preview, `1`–`9`
select directly, `enter` confirms, and `esc` returns without changing the
harness or losing the draft. `/harness NAME` selects explicitly;
`/provider NAME` remains a compatibility alias.

For every installed launch-capable provider, `shift+tab` from the new-task composer
opens an asynchronous searchable picker while preserving the current draft;
`/model` opens the same picker as a command. Type to filter, use arrows or `tab`
to move, `page up`/`page down` to move ten choices, `enter` to select, and
`esc` to keep the previous selection and draft. After a successful catalog
load, **Default** is available alongside the exact account models. An
authentication/catalog failure does not offer a blind default launch; it
offers the native sign-in handoff instead. If catalog retrieval itself is
unavailable but the provider supports an exact identifier, type that ID in the
error-state picker and press Enter. Antigravity is deliberately excluded from
this fallback because its CLI validates `--model` against the same unavailable
catalog. Press Enter/`l` for native recovery and Ctrl+R to retry instead.
`/model NAME` accepts an exact custom
identifier without loading the catalog, and `/model default` resets the
selection. `/login` hands the terminal to the selected provider's native
authentication/setup UI. When the catalog reports an authentication failure,
the picker offers `enter`/`l` for that handoff and reloads the same account
catalog automatically. The selected harness/model is always displayed in the composer
border before submission. Catalog sources are provider-native: Claude parses
aliases advertised beside `--model` in `claude --help`; Codex requests all
visible pages of App Server `model/list`; Pi parses `pi --offline
--list-models`; OpenCode parses `opencode models`; Cursor parses `cursor-agent
models`; Copilot queries its headless SDK `models.list` without creating a
session; and Antigravity parses `agy models`. A catalog is informative,
not proof that the current account can successfully invoke every listed model.

A launch-time authentication failure follows the same route: OAV preserves the
task draft and selected model, opens the provider's setup picker, and makes
Enter/`l` run native login. This avoids leaving a Cursor/Copilot error as a
passive footer where Enter would open an unrelated selected row.

The native setup surfaces are `claude auth login`, `codex login`, Pi's
no-session TUI (`/login` inside Pi), `opencode auth login`, `cursor-agent login`,
`copilot login`, and Antigravity's first-run `agy` flow. OAV suspends its
alternate screen before these commands and never reads or copies credentials.
The setup/login UI always gets its own private terminal; Left backgrounds it as
a visible Terminal row and Enter/Right resumes that exact screen. `/setup
HARNESS` uses the same terminal for an installation check, confirmed official
installer, and native login. It never attaches setup to the last agent session.

Select `Terminal` (or `/harness terminal`) to create a plain interactive shell.
The task text becomes the terminal's display name, not a command. Left returns
to OAV, Enter/Right resumes, the first Ctrl+X stops it, and the second Ctrl+X
deletes its completed row. These terminal frontends are process-local and are
stopped when the dashboard itself exits.

Copilot retains one process-local ACP control connection for sessions launched
by the current dashboard. A later dashboard may still list a persisted Copilot
session, but that row is observe/native-open rather than silently inheriting
control.

`doctor` checks executable availability and explicitly named Docker targets. It
does not launch, stop, or modify a provider session or container. A missing
optional host provider is a warning; failure to verify an explicitly requested
container is an error and produces a nonzero exit status.

## Completed history and bulk archive

The default dashboard and JSON snapshot include completed exact OAV-managed
sessions. Ownership and lifecycle visibility remain separate: `/completed hide`
or `--hide-completed` provides an active-only managed view, while `/completed
show` restores completed sessions. These controls do not read unrelated
provider history. Add
`--include-external` when provider-wide history is actually wanted. For
example, `open-agent-view --include-external --history-limit 500` opts into
a bounded completed-history review.

Completed filtering is applied by adapters and again at the central discovery
boundary. Claude is queried without `--all` when completed is hidden, and
OpenCode's global persisted-history query is never started without both
`--include-external` and completed visibility. Rows returned in violation of a
provider's completed/cwd/interactive contract are removed before a partial
snapshot reaches the UI. The header shows `completed hidden` rather than a
misleading zero. `/completed show`, `/completed hide`, and `/completed` update
the running dashboard; `--hide-completed` selects the active-only initial state
and `--all` remains accepted for compatibility. External history is
limited to 100 records per provider by default, with a warning when more exist.
The Show-more row pages only the already discovered window.

### Local hide, provider delete, and provider archive

Ctrl+X follows the selected row's current lifecycle. On an active row with
exact Interrupt authority, the first press stops that exact session. Discovery
refreshes immediately; when the same row becomes idle, the next press deletes
it if the provider grants exact Delete authority. Providers without a safe
delete surface instead remove the idle row locally and reversibly, retaining
provider history. An active row without Interrupt authority still requires an
explicit local-hide confirmation because its live process will continue. The
same rule applies from Peek.

Completed-group deletion remains confirmed. It deletes only when every row
grants Delete; otherwise it offers to hide only the undeletable rows locally.
Bulk stop for an active group remains unavailable.

The local hidden-ID registry can also be managed without opening the TUI:

```console
# Obtain the stable normalized ID from Peek or JSON output. Add
# --include-external when the target is not OAV-managed.
open-agent-view --json --include-external --all

open-agent-view sessions hide 'pi:host:EXACT_ID'
open-agent-view sessions hidden
open-agent-view sessions unhide 'pi:host:EXACT_ID'

# Each maintenance command also supports machine-readable output.
open-agent-view --json sessions hidden
```

`sessions hide` is idempotent and accepts an exact normalized ID even if its
provider row is not currently discoverable. `sessions unhide` only removes the
local suppression; the row returns on the next discovery only if its provider
still reports it. Neither command opens, stops, deletes, archives, or edits a
provider session.

Private display names use a separate registry and never call a provider rename
surface:

```console
open-agent-view sessions rename 'pi:host:EXACT_ID' 'release captain'
open-agent-view sessions aliases
open-agent-view --json sessions aliases
open-agent-view sessions reset-name 'pi:host:EXACT_ID'
```

`rename` and `reset-name` are idempotent. If a native harness renames the same
conversation, the local OAV name continues to win until reset. After reset, the
next refresh displays the provider's latest title. In the TUI, `ctrl+r` edits
the same local name and an empty submission resets it.

Provider-native bulk archive is currently available for exact OAV-owned,
completed host Codex threads. The first command is always a read-only preview:

```console
open-agent-view sessions archive
open-agent-view sessions archive --cwd /absolute/project --older-than-days 30 --limit 100
open-agent-view --json sessions archive --older-than-days 30
```

The report distinguishes all completed threads seen, those matching the
directory/age scope, those with exact Archive authority, and the bounded batch
selected. It lists skipped matched threads that are visible but unowned. To
apply the reviewed batch, repeat the exact command with `--yes`:

```console
open-agent-view sessions archive --cwd /absolute/project --older-than-days 30 --limit 100 --yes
```

The default batch limit is 100 and the maximum is 1,100. Every archive is independently revalidated against
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
open-agent-view docker create \
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
open-agent-view docker list
open-agent-view docker status oav-agent
open-agent-view docker start oav-agent
open-agent-view docker stop oav-agent --yes
open-agent-view docker status oav-agent --json
open-agent-view docker remove oav-agent --yes
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
Cursor, GitHub Copilot, Antigravity, Terminal, or the adapter-provided name for a future
provider. Provider identity takes priority over task summary width on narrow
terminals. Peek expands the selected row with the full host or container runtime
label.

| Context | Key | Result |
| --- | --- | --- |
| Session list | `↑` / `↓` | Move cyclically through group headings and rows. |
| Show more row | `enter` | Reveal the next terminal-sized page (at most 25) in that group. |
| Group heading | `enter` | Collapse or expand the group. |
| Session row | `enter` or `→` | Suspend the dashboard and open the provider's full native interface. The physical screen is cleared before the provider draws. |
| Provider-native interface | `←` | Stop and retain only the provider frontend, return to Open Agent View, and keep the managed backend alive. `enter` or `→` on the same row reattaches and restores its terminal screen. |
| Inline Peek | `←` | Return to the session list without opening the native provider interface. |
| Session row | `space` | Open the inline Peek panel and inspect transcript/request details when capability is advertised. |
| Inspect peek | type, `enter` | Send an owned provider reply/steer or the current structured answer. |
| Inspect peek | `y` / `n` | Allow once / deny only when the exact capability is advertised. |
| Inspect peek | `enter` with no text | Open the provider-native interface when that managed/live boundary allows a second client. |
| Session list | `ctrl+s` | Toggle status and working-directory grouping. |
| Session list | `ctrl+f` | Edit the case-insensitive name/summary/path/provider filter. |
| Session list | `ctrl+l` | Request an immediate provider refresh. |
| Session list | `tab`, `/`, or printable text | Compose a new host task. `/` begins a dashboard command rather than a filter. |
| New-task composer | `tab` | Open the visible harness picker. |
| New-task composer | `shift+tab` | Open the selected harness's model picker without changing the task draft. |
| Harness picker | `↑` / `↓`, `←` / `→`, or `tab` / `shift+tab` | Preview configured launch-capable harnesses with wraparound. |
| Harness picker | `enter` or `1`–`9` | Select the highlighted or numbered harness and return to the unchanged draft; changing harness resets the model to its default. |
| Harness picker | `esc` | Return to the unchanged draft without switching harnesses. |
| New-task composer | `/harness` / `/harness NAME` | Open the picker or directly select Claude, Codex, Pi, OpenCode, Cursor, Copilot, Antigravity, or Terminal when its launch controller is available; `/provider` is an alias. |
| New-task composer | `/model` | Asynchronously load the selected harness's account/catalog model list and open a searchable picker. |
| Model picker | type, `backspace`, `↑` / `↓`, `tab` / `shift+tab`, `page up` / `page down` | Filter and navigate catalog results; provider discovery stays off the input thread. |
| Model picker | `enter` / `esc` | Select the highlighted model, or return with the previous model and draft unchanged. |
| New-task composer | `/model NAME` / `/model default` | Select an exact custom model identifier or reset to the provider default. The provider revalidates it at launch. |
| New-task composer | `/completed [show\|hide]` | Toggle completed discovery, or set it explicitly. `show` refreshes providers; `hide` immediately removes completed rows and keeps later refreshes active-only. |
| New-task composer | `/filter TEXT` / `/help` | Apply a session filter or list dashboard slash commands without contacting a provider. |
| New-task composer | `/setup [HARNESS]` | Open the selected or named harness's isolated install/login terminal. |
| Writable composer | `ctrl+j` | Insert a newline rather than submit. |
| Writable composer | `backspace` | Remove the last character. |
| Writable composer/model filter | `option+backspace` or `ctrl+w` | Remove the previous word. |
| Writable composer/model filter | `cmd+backspace` or `ctrl+u` | Remove to the beginning of the current line. |
| Session row | `ctrl+r` | Open the accented `rename session` composer. The `name ❯` mode label is separate from the editable display name; empty submission clears it and follows the latest provider title again. |
| Idle owned Codex row | `ctrl+a`, then `enter` | Confirm archive. |
| Session row or Peek | `ctrl+x` | Stop an exact active owned session; after refresh reports it idle, press again to delete it or remove it reversibly from OAV's view. Active rows without stop authority require a local-hide confirmation. |
| Completed group | `ctrl+x`, then `enter` or `ctrl+x` | Delete only when every member grants Delete; otherwise offer to hide the undeletable rows locally. |
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
group-level safety checks always use the complete discovered session set. Each
status or directory group initially shows a terminal-sized page of at most 25
session rows, followed by a selectable `Show N more · M hidden` row when more
match. Each Enter reveals at most one more page and moves selection to the first
newly visible row. The
revealed count is remembered across ordinary provider refreshes and reset when
switching views or applying a filter, keeping a newly narrowed queue bounded.

After a successful managed launch, the dashboard refreshes immediately and
uses the exact provider/session hint to select the new row. If the provider
persists its record after the launch response, Open Agent View retries discovery
every 250 ms for up to five seconds. The UI remains interactive during those
retries and reports a manual `ctrl+l` recovery only if the exact row still has
not appeared.

Only host Claude background rows recorded in OAV's ownership registry receive
Interrupt. Immediately before `claude stop`, Open Agent View reruns `claude
agents --json` and requires the exact full UUID to remain a host background
session in an active state. Interactive, completed, Docker, external, missing,
or changed rows are refused. Ctrl+X dispatches the stop directly for the exact
owned row; its next use can remove the row only after refresh reports it idle.

Managed Cursor rows on Linux expose Inspect and either Interrupt while the
verified owned process is active or Reply after it becomes idle. Managed
Cursor native open is likewise refused until the active process exits. Managed
Copilot rows expose Inspect, Reply while idle, Cancel while a prompt is active,
and only the exact `allow_once`/`reject_once` choices offered by a pending ACP
permission request. Persisted Copilot rows from `session/list` do not inherit
those controls.

Managed OpenCode rows on Linux expose Inspect and Reply; while the owned server
reports active work they also expose Interrupt. Native open attaches to the
same exact authenticated loopback server/session rather than starting a second
server. They do not yet expose provider permission or structured-input
requests. External OpenCode history remains inspect/native-open only.

Managed Pi rows expose Ctrl+X stop while their exact RPC process is alive,
including an idle process after a completed turn. Stop closes the selected
supervisor-owned stdin without waiting on a model response. After refresh
observes exit, the same row exposes exact Delete; the next Ctrl+X removes its
validated JSONL file. Enter/Right performs that handoff automatically only for
a completed row, then opens Pi's full native interface. Active work and pending
questions must be stopped explicitly first.

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
| `hidden-sessions.json` | Reversible local suppression records; provider history and live processes are not changed. |
| `managed-docker/owners.json` | Exact external proof for managed-container lifecycle. |

These files contain authority metadata and should not be shared between users.
They do not contain collected Codex structured answers. Copilot ACP authority
is held in memory and has no OAV state path. Removal or repair has safety
consequences; follow [troubleshooting and recovery](troubleshooting.md) instead
of deleting state speculatively.

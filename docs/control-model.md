# Control and ownership model

The dashboard separates **visibility** from **authority**. Its default inventory
contains only sessions Open Agent View created or explicitly manages. The
`--include-external` flag adds provider-wide history for review, but finding an
external session is never permission to interrupt or delete it.

## Local visibility controls are not provider mutations

When an idle selected row does not carry exact Delete capability, `ctrl+x`
hides it only from Open Agent View. An active row without Interrupt authority
requires an explicit hide confirmation because hiding will not stop it. The normalized ID is
stored in the private `hidden-sessions.json` registry and removed from later
dashboard and JSON snapshots. Provider history, conversation state, and live
processes remain unchanged. `open-agent-view sessions hidden` audits that local
list; `sessions unhide SESSION_ID` removes the suppression without recreating
or resuming anything.

This is intentionally distinct from provider-native deletion and archive.
Those actions remain available only where the tables below grant exact
authority and retain their own revalidation. An observe-only row
is never deleted merely because it can be hidden, and a local hidden record is
never presented as an archive.

The same presentation/mutation distinction applies to names. `ctrl+r` writes a
private alias keyed by the normalized session ID in `session-aliases.json`;
`sessions rename SESSION_ID NAME` provides the same local operation from the
CLI. It never calls a provider rename surface. If the native harness changes
its own title, discovery treats that as the canonical provider name. OAV shows
it whenever no local alias exists; while an alias exists, the alias wins.
Submitting an empty `ctrl+r` editor or running `sessions reset-name SESSION_ID`
removes the override and refreshes the latest provider title. `sessions
aliases` audits all overrides.

## Claude and Codex capability matrix

| Operation | Host Claude | Managed host Codex | External host Codex | Explicit Docker target |
| --- | --- | --- | --- | --- |
| Discover | `claude agents --json` | Owning App Server `thread/list` | Same read surface | Provider protocol through exact container ID |
| Inspect | `claude logs`, reconstructed as a terminal screen | `thread/read(includeTurns: true)`, bounded for display | Summary only | Claude logs; Codex summary |
| Open | `claude attach` | `codex --remote … resume` against the owning server | `codex resume` | Interactive `docker exec` to the provider CLI |
| Launch | Claude allocates the ID through `--background`; OAV resolves its exact full UUID, records it, then opens full-screen `claude attach` | `thread/start`, then `turn/start` | Disabled | Disabled for observe-only containers |
| Interrupt | `claude stop`, exact provider-listed active host background sessions only | `turn/interrupt`, owned active turns only | Disabled | Disabled for observe-only containers |
| Inline reply or provider request | Not exposed by the supported non-TTY CLI | Idle `turn/start`; working `turn/steer`; exact one-shot command decisions, safe denials, and non-secret structured input | Native TUI only | Disabled |
| Archive or delete | No supported Claude command | Idle owned threads only | Disabled | Disabled |

Opening a session temporarily suspends the dashboard's alternate screen and
runs the provider's native interactive client behind a private pseudo-terminal.
The screen is cleared first, so a Codex or other provider transcript starts at
the top instead of appending below the previous shell contents. Enter or Right
opens the selected row directly. Plain Left/Right arrows first reach the
provider. If the cursor does not move, OAV displays a 1.6-second return hint;
repeat the same arrow to stop and retain only that frontend, restore the
dashboard, and leave the managed backend alive. Shift+Left/Right performs the
same return immediately. Enter or Right on the same dashboard row resumes the
exact retained frontend and replays its terminal screen. Left also returns from
OAV's inline Peek without starting a provider CLI.

Authentication is a terminal handoff, not an OAV credential store. The model
picker, `/login`, or `/setup` may run a provider's own login/setup UI after suspending the
dashboard. Every handoff has a provider-keyed private PTY; the native return
gesture backgrounds it as a Terminal row and Enter/Right resumes the same
screen. Open Agent View
neither reads the resulting token nor persists
answers; it only retries the provider-native model catalog after the command
returns. Providers revalidate an exact selected model at launch.

Missing-harness setup is a separate explicit mutation. `open-agent-view setup
HARNESS` names the official source and requires a terminal confirmation (or
literal `--yes`). Downloaded shell installers are staged in a private temporary
file rather than streamed directly to a shell. Setup installs a CLI; it grants
no session ownership or control authority.

The Terminal launch target is a convenience controller, not an agent protocol.
Its prompt becomes only a bounded display name and OAV starts the user's shell
without evaluating that name. Its `/shell` picker resolves only installed
allowlisted executable names. Missing allowlisted shells can be selected as an
explicit install action, which runs one detected native package manager as an
argument array after OAV hands over the terminal; the display name and filter
text never enter that command. Its authority is the exact process-local PTY
entry created by this dashboard. Ctrl+X stops that entry; only its completed
record gains Delete. Normal dashboard shutdown stops all retained Terminal and
provider-login frontends, so this is not a durable terminal multiplexer.

## Claude ownership registry

`claude --background` returns an eight-character session ID. OAV deliberately
does not combine `--background` with `--session-id`: current Claude releases
own the background ID and warn that the supplied ID would be ignored. OAV
captures the returned prefix, resolves the exact full UUID from `claude agents
--json --all`, and records that UUID with its provider and runtime in:

```text
$XDG_STATE_HOME/open-agent-view/ownership.json
```

or, when `XDG_STATE_HOME` is unset:

```text
~/.local/state/open-agent-view/ownership.json
```

The file is written atomically with user-only permissions on Unix and records
which sessions Open Agent View launched. Only a matching owned record may
advertise Ctrl+X. Interrupt still does not trust that record or a stale row as
current authority: immediately before stopping, the controller reruns `claude
agents --json` and requires the exact full UUID, host runtime, background kind,
and active state to still match. Interactive, completed, Docker, external,
missing, or changed sessions are refused.

The registry grants provider-session authority only. It never grants authority
to stop or remove a Docker container.

## Codex ownership boundary

Host Codex discovery and launch share one reconnectable App Server listening on
a Unix socket. The server is detached from the dashboard and remains running
after `open-agent-view` exits. A later dashboard connects through
the App Server's WebSocket protocol over its private Unix socket and reloads
the exact thread and active-turn IDs it created. State lives in:

```text
$XDG_STATE_HOME/open-agent-view/codex-supervisor/
```

or `~/.local/state/open-agent-view/codex-supervisor/`. The directory is
current-user-owned and mode `0700`; its lock, log, and JSON record are regular
current-user-owned files with no group/other access. The record contains the
PID of the native process that owns the listening socket (not merely an npm
wrapper), platform process-start token, exact command line, socket path, and
owned thread/turn IDs. Linux uses `/proc`; macOS uses native process metadata,
`KERN_PROCARGS2`, and exact system inspection of the private Unix-socket owner.

Before reconnecting or changing an ownership record, the supervisor verifies
both the persisted start token and exact command line. Normal discovery and
dashboard shutdown never signal a PID loaded from disk. Explicit idle-delete
recovery on Linux opens a pidfd first, revalidates the full identity, and
signals only through that stable kernel handle. macOS refuses that exceptional
restart rather than signal through a reusable numeric PID. A dead or mismatched
record causes a new uniquely named
socket to be created; a verified live process with an unavailable socket is
reported as an error and is not replaced. This deliberately favors avoiding
the wrong process over automatic cleanup.

Only host Codex threads recorded by this supervisor receive Interrupt, and
only while their recorded turn remains active. Pre-existing host threads and
all Docker Codex threads remain observe/open-only. Launch uses
`approvalPolicy: on-request` and the `workspace-write` sandbox; it does not
weaken the user's sandbox to gain automation.

Owned working threads may be steered only with the exact recorded active turn
ID. Idle reply, archive, and delete require the provider to report the thread
as completed and the ownership record to contain no active turn. Normalized
Needs-input state never receives generic Reply authority because it may
represent an approval or structured input request. Instead, the controller
tracks each server request by its opaque string/integer ID plus exact owned
thread and active-turn ID. Transcript rendering keeps only a bounded recent
tail.

The active-turn record is released only by the matching terminal
`turn/completed` event or a resume payload containing that exact terminal turn.
A briefly stale idle `thread/read` after `turn/start` is presented as owned
working state, preserving Ctrl+X until Codex resolves the turn. Deletion first
archives the exact idle thread. If Codex 0.147 wedges without a response or
exact deletion notification, OAV restarts the owner only when every recorded
turn is idle, completes the same ID through an isolated App Server, and restores
all other ownership records. Active work makes this recovery a refusal.
Ordinary discovery holds a shared private recovery lock; the bounded
stop/delete/restart sequence holds it exclusively, so a second dashboard waits
instead of starting or attaching to an intermediate owner.

`open-agent-view sessions archive` uses the same boundary in bulk. It discovers
only ordinary non-archived host Codex threads, enriches them through the owning
supervisor, selects only completed rows carrying the exact Archive capability,
and defaults to a read-only plan. `--cwd`, `--older-than-days`, and the bounded
`--limit` narrow that plan; literal `--yes` authorizes execution. Each selected
thread is revalidated again by `ControlHub` and `CodexSupervisor` immediately
before `thread/archive`. A partial failure is reported per thread and never
causes an unowned or active thread to be adopted.

Codex keeps pending server callbacks inside the owning App Server, not in a
dashboard connection. On reconnect, `thread/resume` first restores the exact
owned active thread and then replays unresolved requests with their original
IDs. `thread/list`, `thread/read`, and initialization alone do not replay them.
Open Agent View therefore never reconstructs a request from rollout history.
It keeps prompts and partially collected answers only in memory, clears a
request on `serverRequest/resolved`, and leaves a sent response in a resolving
state until that notification arrives.

A nonblocking, user-private `controller.lock` lease gives one dashboard process
inline response authority. Other dashboards remain able to observe/open but do
not advertise approval/input capabilities. This matters because the App Server
accepts the first response carrying a pending ID even if it arrives from a
different subscribed connection.

The initial decision set is intentionally narrow:

- a command can be allowed once only when the exact command and directory, or
  an exact network host/protocol request, can be rendered; additional
  permissions disable acceptance;
- command/file requests can be denied with their provider-defined one-shot
  result; permission denial sends an empty turn-scoped grant; MCP elicitation
  sends an explicit decline;
- file acceptance stays native because the request itself omits the diff;
  permission grants and MCP form acceptance stay native because they expand
  authority or require additional schema handling;
- structured questions are accepted only when the complete question set is
  valid and has unique IDs. Answers are held in memory and sent together;
  secret questions and expired auto-resolution requests remain native-only.

No request is answered automatically on receipt, disconnect, or restart.

## Pi ownership boundary

Pi exposes a native TUI plus documented stdio JSONL RPC mode, but no socket for
attaching to an arbitrary running process. Dashboard task submission generates
an exact UUID and starts the native TUI with `--session-id`, the private managed
`--session-dir`, the selected model, and prompt. Scriptable/background control
uses a detached Linux supervisor that retains exact stdin/stdout pipes for its
Pi processes. Dashboard restarts reconnect through a private Unix socket.
Existing JSONL history and unrelated live Pi processes never acquire control
authority.

| Operation | OAV-owned native/RPC Pi | Existing/unrelated Pi history |
| --- | --- | --- |
| Discover | Private JSONL store plus exact RPC live state when applicable | Documented JSONL store |
| Inspect | Bounded persisted transcript, or live `get_messages` for RPC | Bounded persisted transcript |
| Open | Exact background native screen resumes; completed/idle RPC hands off after exact stop | `pi --session ID --session-dir DIR` |
| Launch/reply | Dashboard launches native-first; background API uses RPC `prompt`, with exact model selection on both | Disabled |
| Stop | Ctrl+X terminates the exact retained native PTY or closes the exact RPC stdin | Disabled |
| Confirmation/input | Native UI handles native requests; RPC uses exact pending extension request ID and exact selections | Disabled |
| Delete/archive | Exact managed JSONL deletion only after native/RPC process exit; archive disabled | Disabled |

The supervisor state is under `$XDG_STATE_HOME/open-agent-view/pi/`, or
`~/.local/state/open-agent-view/pi/`. Before any reconnect or control request,
the saved daemon PID must match both its Linux `/proc` start token and exact
command-line bytes. The containing directory is current-user-owned mode `0700`;
records, locks, logs, and sockets have no group/other access. Symlinks and
wrong-owner/permissive authority files are refused. The daemon request socket
is inside that private directory, bounds request/response size, and will not be
started merely by session discovery.

Pi's `--no-approve` means ignore untrusted project-local configuration; it is
not an operating-system sandbox. Built-in tools still run with the managed Pi
process's user permissions. Use a separate OS/container boundary when the task
requires one. Durable Pi control currently requires Linux; macOS keeps
history inspection and native resume only.

A modeled launch additionally requires the daemon to advertise the
`launch_with_model` protocol feature. An older verified daemon is replaced only
after its own session list proves every owned session completed; active work
causes an actionable refusal instead of a shutdown.

The supervisor also advertises exact per-session stop/delete features. Stop
closes only the selected RPC pipe and returns without waiting on a provider
turn, keeping the dashboard responsive. Discovery must then observe the child
exit before Delete is granted. The JSONL header ID and canonical path under the
private managed session root are revalidated immediately before removal.
Persisted files in that managed root remain owned/default-visible even after a
supervisor restart; unrelated Pi history still requires `--include-external`.
When upgrading an older daemon without per-session stop, OAV may shut down that
verified daemon only if no other active Pi session would be affected.

## OpenCode ownership boundary

OpenCode's CLI history and export commands are read-only discovery surfaces.
On Linux, Open Agent View additionally launches one durable `opencode serve`
process bound to an ephemeral `127.0.0.1` port with a random Basic-auth secret.
It stores the server identity and only the canonical session IDs it created in:

```text
$XDG_STATE_HOME/open-agent-view/opencode/server.json
```

or `~/.local/state/open-agent-view/opencode/server.json`. The containing
directory is current-user-owned mode `0700`; the record, lock, and log are
private regular files, with the authority record mode `0600`. A later dashboard
reconnects only after matching the Linux process start token and exact command
line, proving that process owns the recorded loopback listener, and completing
an authenticated health request. A PID, listener, password, or history ID alone
never grants authority.

| Operation | OAV-owned managed OpenCode session | External CLI history |
| --- | --- | --- |
| Discover | Exact owned IDs plus authenticated server status | Read-only global CLI projection |
| Inspect | Bounded server message transcript | Bounded `opencode export` transcript |
| Open | Authenticated native attach to the exact owned server and session | `opencode --session ID` |
| Launch/reply | Create through the owned server and `prompt_async` with an optional exact `providerID`/`modelID`, then dashboard launch auto-attaches | Disabled |
| Interrupt | Authenticated abort only for an owned working session | Disabled |
| Permission/input | Not yet exposed by the dashboard | Disabled |
| Archive/delete | Disabled | Disabled |

The supervisor intentionally does not attach to an arbitrary OpenCode TUI or
unregistered random server. Durable managed control requires Linux; other
platforms retain CLI history inspection and native resume.

## Cursor ownership boundary

Cursor exposes a TTY-only history picker, not a machine-readable global session
list. Open Agent View therefore shows only Cursor sessions it launched itself
on Linux. Foreground launch creates an exact Cursor chat, records it, and
immediately resumes that ID in Cursor's interactive interface. Inline replies
still use detached stream-JSON mode and record the exact process identity plus
bounded output paths under:

```text
$XDG_STATE_HOME/open-agent-view/cursor/
```

or `~/.local/state/open-agent-view/cursor/`. Dashboard restarts rediscover those
records. Before marking a turn active or sending `SIGINT`, the supervisor
matches the saved PID, Linux `/proc` start token, and exact command line. It
never signals a PID merely because it appears in the registry.

| Operation | OAV-owned managed Cursor run | External Cursor session |
| --- | --- | --- |
| Discover | Private registry plus bounded stream-JSON logs | Unavailable; the global picker is TTY-only |
| Inspect | Bounded assistant transcript from the owned log | Disabled |
| Open | Refused while an owned print worker is active; native resume otherwise | Not listed; use Cursor's own TTY picker |
| Launch/reply | Create a chat and open it in the foreground; inline reply only after the prior process/native frontend exits | Disabled |
| Interrupt | `SIGINT` only after exact live-process verification | Disabled |
| Permission/archive/delete | Disabled | Disabled |

Managed Cursor launch and rediscovery currently require Linux. Open Agent View
does not scrape the provider's picker or infer ownership from a chat ID.

## GitHub Copilot ownership boundary

Copilot's official ACP exposes persisted sessions through `session/list`, but
listing a session does not grant control. Those records are observe/native-open
only. OAV persists the exact IDs it creates so their rows survive a restart,
but managed authority still belongs to the exact ACP control connection
retained by the current dashboard process. The registry is visibility and
provenance, not a serialized permission request or live-connection lease.

| Operation | Current connection-owned Copilot session | Persisted ACP list record |
| --- | --- | --- |
| Discover | Private exact-ID registry plus ACP metadata/message replay | Official `session/list` on the discovery connection |
| Inspect | Bounded transcript received on the owning connection; a restarted row retains its latest summary and refreshes from bounded ACP replay | Disabled |
| Open | Idle sessions use advertised `session/close`, or drop the idle owning ACP process when close is absent; native resume then reloads on return | `copilot --resume=ID -C PATH` |
| Launch/reply | Dashboard launch reserves an exact UUID and starts `copilot --session-id ID --interactive PROMPT` in front; connection-owned reply uses `session/prompt` while idle | Disabled |
| Interrupt | ACP cancel for the exact active session prompt | Disabled |
| Permission | Exact offered `allow_once` or `reject_once` option only | Disabled |
| Archive/delete | Disabled | Disabled |

The release/resume/load handoff never overlaps two clients. ACP `session/close`
is optional: when absent, OAV may close its retained ACP process only if every
connection-owned Copilot task is idle, releasing those idle rows too. An active
prompt or permission on any other row refuses the handoff. A backgrounded native
frontend leaves inline authority released until that exact frontend exits and
the session is loaded again. When the dashboard exits, the retained ACP process and its control authority
end. The exact OAV-created row remains visible with its provider timestamp and
latest real message and can be opened natively, but it is not silently adopted
for inline control. Older locally named Copilot rows are migrated for
visibility only. Unknown ACP client requests
are rejected explicitly, and pending permission requests are never answered
automatically.

## Oh My Pi, Grok, Kilo Code, and OpenHands ownership

These native harnesses expose durable session identities and native resume
commands. OAV observes their bounded native inventories but never adopts an
existing record as managed. A foreground launch snapshots the inventory first,
then records ownership only when exactly one new ID appears in the requested
workspace. The private registry stores no credentials or transcript bodies.

| Operation | Exact OAV-created session | External saved session |
| --- | --- | --- |
| Discover | Private owner record plus provider-native inventory | Only with `--include-external` |
| Inspect | Bounded name, latest visible message, workspace, and timestamps | Same read-only projection |
| Open | Exact native resume command | Exact native resume command |
| Launch | Foreground native TUI with the selected exact model and prompt | Disabled |
| Interrupt | Exact retained native frontend in the current dashboard process | Disabled |
| Reply / approval / delete | Native interface only | Disabled |

Oh My Pi JSONL, Grok JSON/update logs, and OpenHands event files are bounded and
read without following symlinks. Kilo Code discovery uses a bounded `SELECT`
through its official JSON `kilo db` interface; it does not read message bodies.
See the [integration contract](exploration/session-migrate-native-integrations.md)
for the exact commands and state roots.

## Managed Docker ownership

`open-agent-view docker create` accepts only a digest-pinned image and creates a
stopped, non-root, `--init`, capability-dropped container with
`no-new-privileges`, a PID limit, explicit bind mounts, and Open Agent View
labels. A cryptographically random instance ID is written both to the label and
to a private external registry. The registry directory/file modes are `0700`
and `0600`, are current-user-owned, and updates are serialized with a file lock
and written atomically.

Every start, stop, and remove resolves the user-facing reference, re-inspects
the full immutable ID, and requires the label instance ID to match the external
record. Stop/remove also require the CLI's explicit `--yes`; remove refuses a
running container, does not pass Docker's volume-removal flag, and forgets the
owner record only after exact removal succeeds. Existing `--docker-container`
targets never enter this authority tier.

The external registry defaults to
`$XDG_STATE_HOME/open-agent-view/managed-docker/owners.json`, or
`~/.local/state/open-agent-view/managed-docker/owners.json` when
`XDG_STATE_HOME` is unset. A CLI override changes this authority namespace and
must be supplied consistently. See the [CLI reference](cli.md) for the complete
lifecycle and [troubleshooting](troubleshooting.md) before repairing refused
state.

## Deliberate limitations

- Inline Claude replies are not implemented by scraping private IPC or editing
  transcript files. Press Enter to attach and reply through Claude itself.
- File-change acceptance, permission grants, MCP form/URL acceptance, secret
  structured input, and unknown request types remain native-only.
- There is not yet a `open-agent-view` status/stop command for the detached Codex
  server. Logs append without rotation. Stale sockets and unverified PIDs are
  intentionally left untouched.
- Durable Codex supervision supports Linux and macOS. Linux verifies `/proc`
  start time, argv, and socket ownership; macOS verifies `proc_pidinfo` start
  time, `KERN_PROCARGS2` argv, and the exact private Unix-socket owner. Explicit
  pidfd-based idle-delete recovery remains Linux-only and safely refuses on
  macOS.
- Durable Pi supervision has the same Linux process-identity requirement;
  unrelated Pi sessions remain inspect/open-only on every platform.
- Managed OpenCode reconnect requires an authenticated loopback server plus
  exact Linux process/listener identity; external history stays inspect/open-only.
  Permission and structured-input requests remain provider-native.
- Managed Cursor sessions require Linux and only OAV-owned runs are listed;
  Cursor's external TTY picker is not scraped.
- Managed Copilot control is process-local to one retained ACP connection;
  persisted `session/list` records remain observe/native-open only.
- Docker containers supplied with `--docker-container` are observe-only;
  managed lifecycle requires creation plus the separate owner record.
- Group deletion is disabled whenever any member lacks Delete authority.
- Prompt and session values are always command arguments, never interpolated
  into a shell string.

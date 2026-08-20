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
processes remain unchanged. `coding-agents sessions hidden` audits that local
list; `sessions unhide SESSION_ID` removes the suppression without recreating
or resuming anything.

This is intentionally distinct from provider-native deletion and archive.
Those actions remain available only where the tables below grant exact
authority and retain their own revalidation. An observe-only row
is never deleted merely because it can be hidden, and a local hidden record is
never presented as an archive.

## Claude and Codex capability matrix

| Operation | Host Claude | Managed host Codex | External host Codex | Explicit Docker target |
| --- | --- | --- | --- | --- |
| Discover | `claude agents --json` | Owning App Server `thread/list` | Same read surface | Provider protocol through exact container ID |
| Inspect | `claude logs`, reconstructed as a terminal screen | `thread/read(includeTurns: true)`, bounded for display | Summary only | Claude logs; Codex summary |
| Open | `claude attach` | `codex --remote … resume` against the owning server | `codex resume` | Interactive `docker exec` to the provider CLI |
| Launch | `claude --background` | `thread/start`, then `turn/start` | Disabled | Disabled for observe-only containers |
| Interrupt | `claude stop`, exact provider-listed active host background sessions only | `turn/interrupt`, owned active turns only | Disabled | Disabled for observe-only containers |
| Inline reply or provider request | Not exposed by the supported non-TTY CLI | Idle `turn/start`; working `turn/steer`; exact one-shot command decisions, safe denials, and non-secret structured input | Native TUI only | Disabled |
| Archive or delete | No supported Claude command | Idle owned threads only | Disabled | Disabled |

Opening a session temporarily suspends the dashboard's alternate screen and
runs the provider's native interactive client behind a private pseudo-terminal.
The screen is cleared first, so a Codex or other provider transcript starts at
the top instead of appending below the previous shell contents. Enter or Right
opens the selected row directly. A plain Left sequence is reserved by OAV: it
stops and retains only that native frontend, restores the dashboard, and leaves
the managed provider backend alive. Enter or Right on the same row resumes the
exact retained frontend and replays its terminal screen. Left also returns from
OAV's inline Peek without starting a provider CLI.

## Claude ownership registry

`claude --background` returns an eight-character session ID. `coding-agents`
records that prefix with its provider and runtime in:

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
after `coding-agents` exits. A later dashboard connects through
the App Server's WebSocket protocol over its private Unix socket and reloads
the exact thread and active-turn IDs it created. State lives in:

```text
$XDG_STATE_HOME/open-agent-view/codex-supervisor/
```

or `~/.local/state/open-agent-view/codex-supervisor/`. The directory is
current-user-owned and mode `0700`; its lock, log, and JSON record are regular
current-user-owned files with no group/other access. The record contains the
PID of the native process that owns the listening socket (not merely an npm
wrapper), Linux `/proc` start token, exact command line, socket path, and owned
thread/turn IDs.

Before reconnecting or changing an ownership record, the supervisor verifies
both the persisted start token and exact command line. Normal discovery and
dashboard shutdown never signal a PID loaded from disk. Explicit idle-delete
recovery opens a pidfd first, revalidates the full identity, and signals only
through that stable kernel handle. A dead or mismatched record causes a new uniquely named
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

`coding-agents sessions archive` uses the same boundary in bulk. It discovers
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

Pi exposes a documented stdio JSONL RPC mode but no socket for attaching to an
arbitrary running process. On Linux, Open Agent View starts one detached
supervisor that retains the exact stdin/stdout pipes for every Pi process it
launches. Dashboard restarts reconnect through a private Unix socket. Existing
JSONL history and unrelated live Pi processes never acquire control authority.

| Operation | OAV-owned live Pi RPC | Existing/unrelated Pi history |
| --- | --- | --- |
| Discover | Supervisor live state plus its private JSONL store | Documented JSONL store |
| Inspect | Bounded `get_messages` transcript | Bounded persisted transcript |
| Open | Completed/idle RPC is stopped by closing its exact owned stdin, then `pi --session ID --session-dir DIR`; active work and pending questions are refused | `pi --session ID --session-dir DIR` |
| Launch/reply | `prompt`; launch may pass an exact catalog/custom `--model`, and active work uses exact `steer` behavior | Disabled |
| Stop | Ctrl+X closes the exact owned RPC stdin without waiting on a model/abort response | Disabled |
| Confirmation/input | Exact pending extension request ID; selections require an exact option | Disabled |
| Delete/archive | Exact managed JSONL deletion only after process exit; archive disabled | Disabled |

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
| Open | Refused while owned by the managed server | `opencode --session ID` |
| Launch/reply | Create through the owned server; `prompt_async` for new work, with an optional exact `providerID`/`modelID` selector | Disabled |
| Interrupt | Authenticated abort only for an owned working session | Disabled |
| Permission/input | Not yet exposed by the dashboard | Disabled |
| Archive/delete | Disabled | Disabled |

The supervisor intentionally does not attach to an arbitrary OpenCode TUI or
unregistered random server. Durable managed control requires Linux; other
platforms retain CLI history inspection and native resume.

## Cursor ownership boundary

Cursor exposes a TTY-only history picker, not a machine-readable global session
list. Open Agent View therefore shows only Cursor sessions it launched itself
on Linux. Launch creates a Cursor chat, runs the turn in detached stream-JSON
mode, and records the exact process identity and bounded output paths under:

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
| Open | Refused while the owned process is active; native resume after idle | Not listed; use Cursor's own TTY picker |
| Launch/reply | Create a chat and run a turn; reply only after the prior process exits | Disabled |
| Interrupt | `SIGINT` only after exact live-process verification | Disabled |
| Permission/archive/delete | Disabled | Disabled |

Managed Cursor launch and rediscovery currently require Linux. Open Agent View
does not scrape the provider's picker or infer ownership from a chat ID.

## GitHub Copilot ownership boundary

Copilot's official ACP exposes persisted sessions through `session/list`, but
listing a session does not grant control. Those records are observe/native-open
only. Managed authority belongs to the exact ACP control connection retained by
the current dashboard process and is not written to an OAV ownership file.

| Operation | Current connection-owned Copilot session | Persisted ACP list record |
| --- | --- | --- |
| Discover | In-memory managed state and ACP events | Official `session/list` on the discovery connection |
| Inspect | Bounded transcript received on the owning connection | Disabled |
| Open | Refused while connection-owned | `copilot --resume=ID -C PATH` |
| Launch/reply | `session/new`, then `session/prompt`; reply only while idle | Disabled |
| Interrupt | ACP cancel for the exact active session prompt | Disabled |
| Permission | Exact offered `allow_once` or `reject_once` option only | Disabled |
| Archive/delete | Disabled | Disabled |

When the dashboard exits, the retained ACP process and its control authority
end. A later `session/list` result remains visible and can be opened natively,
but it is not silently adopted for inline control. Unknown ACP client requests
are rejected explicitly, and pending permission requests are never answered
automatically.

## Managed Docker ownership

`coding-agents docker create` accepts only a digest-pinned image and creates a
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
- There is not yet a `coding-agents` status/stop command for the detached Codex
  server. Logs append without rotation. Stale sockets and unverified PIDs are
  intentionally left untouched.
- Durable Codex supervision currently requires Linux because safe PID reuse
  detection relies on `/proc/<pid>/stat` and `/proc/<pid>/cmdline`.
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

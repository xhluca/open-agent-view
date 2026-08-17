# Control and ownership model

The dashboard separates **visibility** from **authority**. Finding a session is
not permission to interrupt or delete it.

## Current capability matrix

| Operation | Host Claude | Managed host Codex | External host Codex | Explicit Docker target |
| --- | --- | --- | --- | --- |
| Discover | `claude agents --json` | Owning App Server `thread/list` | Same read surface | Provider protocol through exact container ID |
| Inspect | `claude logs`, reconstructed as a terminal screen | Thread summary; full read pending | Thread summary; full read pending | Claude logs; Codex summary |
| Open | `claude attach` | `codex --remote … resume` against the owning server | `codex resume` | Interactive `docker exec` to the provider CLI |
| Launch | `claude --background` | `thread/start`, then `turn/start` | Disabled | Disabled for observe-only containers |
| Interrupt | `claude stop`, owned sessions only | `turn/interrupt`, owned active turns only | Disabled | Disabled for observe-only containers |
| Inline reply or approval | Not exposed by the supported non-TTY CLI | Native TUI only in this milestone | Native TUI only | Disabled |
| Archive or delete | No supported Claude command | Disabled | Disabled | Disabled |

Opening a session temporarily suspends the dashboard's alternate screen and
runs the provider's native interactive client with inherited terminal I/O.
Returning restores raw mode and refreshes the dashboard.

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

The file is written atomically with user-only permissions on Unix. A discovered
full Claude UUID must match the stored prefix, provider, and runtime before the
Interrupt capability is added. Arbitrary pre-existing sessions remain
observe-only even though the underlying Claude installation may be able to
stop them.

The registry grants provider-session authority only. It never grants authority
to stop or remove a Docker container.

## Codex ownership boundary

Host Codex discovery and launch share one reconnectable App Server listening on
a Unix socket. The server is detached from the dashboard and remains running
after `coding-agents` exits. A later dashboard connects through
`codex app-server proxy` and reloads the exact thread and active-turn IDs it
created. State lives in:

```text
$XDG_STATE_HOME/open-agent-view/codex-supervisor/
```

or `~/.local/state/open-agent-view/codex-supervisor/`. The directory is
current-user-owned and mode `0700`; its lock, log, and JSON record are regular
current-user-owned files with no group/other access. The record contains the
server PID, Linux `/proc` start token, exact command line, socket path, and
owned thread/turn IDs.

Before reconnecting or changing an ownership record, the supervisor verifies
both the persisted start token and exact command line. It never signals a PID
loaded from disk. A dead or mismatched record causes a new uniquely named
socket to be created; a verified live process with an unavailable socket is
reported as an error and is not replaced. This deliberately favors avoiding
the wrong process over automatic cleanup.

Only host Codex threads recorded by this supervisor receive Interrupt, and
only while their recorded turn remains active. Pre-existing host threads and
all Docker Codex threads remain observe/open-only. Launch uses
`approvalPolicy: on-request` and the `workspace-write` sandbox; it does not
weaken the user's sandbox to gain automation.

## Deliberate limitations

- Inline Claude replies are not implemented by scraping private IPC or editing
  transcript files. Press Enter to attach and reply through Claude itself.
- Server requests for command approval or user input are not answered in the
  dashboard yet. Open the owned session in the native TUI to respond.
- There is not yet a `coding-agents` status/stop command for the detached Codex
  server. Logs append without rotation. Stale sockets and unverified PIDs are
  intentionally left untouched.
- Durable Codex supervision currently requires Linux because safe PID reuse
  detection relies on `/proc/<pid>/stat` and `/proc/<pid>/cmdline`.
- Docker containers supplied with `--docker-container` are observe-only.
- Group deletion is disabled whenever any member lacks Delete authority.
- Prompt and session values are always command arguments, never interpolated
  into a shell string.

# OpenCode provider exploration

Status: history discovery and read-only transcript inspection are implemented.
On Linux, the CLI wires one shared durable owned-server supervisor into both
discovery and control for launch, reconnect, live discovery, inspection, reply,
and interrupt; ordinary history remains read-only/native-open. Other platforms
keep the history/native-open path.

Explored on 2026-08-18 against:

- `opencode-ai` 1.18.18 (latest npm release at test time)
- official [CLI documentation](https://opencode.ai/docs/cli/)
- official [server and OpenAPI documentation](https://opencode.ai/docs/server/)
- the upstream [OpenCode repository](https://github.com/anomalyco/opencode)

## Product shape

OpenCode's official shell installer is:

```bash
curl -fsSL https://opencode.ai/install | bash
```

Useful probes:

```bash
opencode --version
opencode session list --format json
opencode serve --hostname 127.0.0.1 --port 4096
```

OpenCode has two complementary public interfaces:

1. `opencode session list --format json` and `opencode export <session-id>` expose persisted history.
2. `opencode serve` exposes an OpenAPI 3.1 HTTP server, JSON endpoints, and SSE events for live control.

The TUI itself is a client of an OpenCode server. A normal TUI uses a random local endpoint unless the operator supplies `--hostname` and `--port`, so Open Agent View cannot safely guess how to attach to an arbitrary existing TUI.

## Persistence and read-only discovery

On Linux, the tested isolated store was an SQLite database below `$XDG_DATA_HOME/opencode/`. The first probe used the documented session command:

```bash
opencode session list --format json
```

Real mixed-provider validation then exposed an important 1.18.18 behavior: that command returned only the current workspace's sessions, so a dashboard launched elsewhere silently missed them. The adapter now uses OpenCode's official, read-only `db` command to project the session table globally, including child sessions, and falls back to `session list` for older releases that do not provide `db`. This deliberately couples the global path to the current table columns; a schema change becomes a visible source warning instead of silently reading the SQLite file itself.

OpenCode 1.18.18 prints **zero bytes**, not `[]`, when the store is empty. With a session it returns records with:

```json
{
  "id": "ses_...",
  "title": "OAV isolated fixture",
  "updated": 1787089195916,
  "created": 1787089195916,
  "projectId": "global",
  "directory": "/tmp/project"
}
```

The list command does not include live state. Persisted records are therefore normalized as completed history. Only a connected server can accurately map `busy`, `retry`, or idle state.

Transcript inspection uses the read-only command:

```bash
opencode export <session-id>
```

The adapter formats user/assistant text and caps displayed transcripts at 32 KiB. Discovery alone grants no reply, abort, permission-response, or deletion authority.

## Server lifecycle and control

The local server offers the primitives needed for complete managed control:

- `GET /global/health`: exact server readiness and version;
- `GET/POST /session`: list/create sessions;
- `GET /session/status`: `idle`, `busy`, or `retry` state;
- `GET /session/:id/message`: transcript;
- `POST /session/:id/prompt_async`: start or continue work;
- `POST /session/:id/abort`: interrupt;
- `POST /session/:id/permissions/:permissionID`: answer an exact permission request;
- `DELETE /session/:id`: destructive deletion;
- SSE event streams for state, message, permission, and question changes.

The managed controller launches one loopback-only server and records every
canonical session ID returned by `POST /session`. Reconnection requires all of:

1. the exact Linux `/proc` start token and command-line bytes;
2. proof that the recorded process owns the exact `127.0.0.1` listening socket;
3. a successful authenticated health response using a random 256-bit secret;
4. an exact session ID and absolute working directory in the private ownership
   record.

A loopback listener or PID alone is never authority. Zombie processes are
rejected. Shutdown, used by isolated tests rather than ordinary dashboard exit,
opens a Linux pidfd first, revalidates identity/listener ownership, signals
through `pidfd_send_signal`, and waits for that exact process to exit.

OpenCode supports Basic authentication through `OPENCODE_SERVER_PASSWORD`. The
managed server sets that to an OAV-generated secret and stores it only in a
current-user-owned `0600` record below
`$XDG_STATE_HOME/open-agent-view/opencode/` (or the corresponding
`~/.local/state` path). The state directory is `0700`; symlinks, permissive or
wrong-owner files, malformed IDs/directories, and oversized records are
refused.

OpenCode 1.18.18's actual `serve --help` reports a default port of `0` (random), while the prose documentation still shows `4096`. A managed controller should always pass an explicit selected port and verify `/global/health` instead of relying on either default.

## Capability matrix

| Session origin | Discover | Inspect | Open native UI | Reply | Abort | Permission/input | Delete |
| --- | --- | --- | --- | --- | --- | --- | --- |
| Existing persisted history | yes | yes | possible via `opencode --session <id>` | no | no | no | no |
| Arbitrary unregistered TUI/server | history only | export | possible | no | no | no | no |
| Explicit operator-enrolled server (future) | yes | yes | `opencode attach <url>` | only with explicit authority | exact server | exact request IDs | never by default |
| OAV-owned server/session on Linux | yes | yes | currently refused through a second server | yes | yes | not yet exposed | not exposed |

## Isolated real-CLI and API validation

No real OpenCode store, plugins, or credentials were used. Every command used temporary `XDG_DATA_HOME`, `XDG_CONFIG_HOME`, and `XDG_CACHE_HOME` directories. The server bound only `127.0.0.1`, used `--pure`, and operated in a temporary worktree.

Verified against 1.18.18:

- exact version and help output;
- an empty `session list --format json` result;
- loopback server startup and `/global/health`;
- session creation without invoking a model;
- REST session list and empty status map;
- adding a `noReply` user message without invoking a model;
- message listing and full session export;
- CLI JSON session-list schema after creation;
- global discovery from a different working directory through the read-only DB command;
- parser behavior for blank output, multiple records, cwd filtering, and timestamps;
- host and explicit-Docker command construction;
- read-only transcript formatting;
- coexistence through the provider-neutral discovery engine unit suite;
- combined `coding-agents --json --all` Pi and OpenCode discovery from a third working directory;
- exact native TUI resume by session ID and clean terminal restoration.
- authenticated managed-server startup using a random secret, exact listener
  ownership, `0700`/`0600` state permissions, and canonical owned IDs;
- ten consecutive dashboard-client reconnects to one live fake server;
- owned launch, live/idle state, transcript, active and idle reply, interrupt,
  external-ID refusal, pidfd shutdown, and panic-safe cleanup;
- a credential-empty managed probe against the real 1.18.18 server, including
  creation, async prompt acceptance, listing, inspection, and exact shutdown.

Not claimed:

- a model-backed turn, because the isolation test intentionally supplied no credentials;
- live state from the history CLI;
- control of an arbitrary existing TUI's unregistered random server;
- destructive API behavior.
- permission/question SSE reduction and responses, which remain provider-native;
- treating a `204` from `prompt_async` as proof that a model turn started or
  succeeded. In the credential-empty 1.18.18 probe the request was accepted but
  the asynchronous provider startup could fail before persisting a user
  message, matching the upstream fire-and-forget contract;
- durable managed control on macOS; history discovery/export/native resume are
  retained there until an equally strong process/listener identity primitive is
  available.

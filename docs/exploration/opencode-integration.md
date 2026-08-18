# OpenCode provider exploration

Status: history discovery and read-only transcript inspection implemented; owned-server lifecycle control is not yet integrated.

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

An OAV controller should launch one loopback-only server, persist its exact endpoint plus process-start identity, and record every session ID it creates. Control capabilities must be granted only when both server identity and session ownership are proven. A loopback listener alone is not proof of ownership, and PID reuse must not be trusted.

OpenCode supports Basic authentication through `OPENCODE_SERVER_PASSWORD`. A managed server should use an OAV-generated secret in a user-private state file even though it listens only on `127.0.0.1`.

OpenCode 1.18.18's actual `serve --help` reports a default port of `0` (random), while the prose documentation still shows `4096`. A managed controller should always pass an explicit selected port and verify `/global/health` instead of relying on either default.

## Capability matrix

| Session origin | Discover | Inspect | Open native UI | Reply | Abort | Permission/input | Delete |
| --- | --- | --- | --- | --- | --- | --- | --- |
| Existing persisted history | yes | yes | possible via `opencode --session <id>` | no | no | no | no |
| Arbitrary unregistered TUI/server | history only | export | possible | no | no | no | no |
| Explicit operator-enrolled server (future) | yes | yes | `opencode attach <url>` | only with explicit authority | exact server | exact request IDs | never by default |
| OAV-owned server/session (planned) | yes | yes | attach | yes | yes | yes | owned + confirmation |

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

Not claimed:

- a model-backed turn, because the isolation test intentionally supplied no credentials;
- live state from the history CLI;
- control of an arbitrary existing TUI's unregistered random server;
- destructive API behavior.

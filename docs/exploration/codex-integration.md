# Codex integration exploration

Observed on 2026-08-17. This document distinguishes the current documented
interface from behavior verified against `codex-cli 0.144.4` in
`basic-claude-uv:latest`. App Server is experimental, so every statement about
the installed binary is a compatibility observation rather than a permanent
contract.

## Conclusion

Use **Codex App Server** as the primary Codex adapter. It is the only official
surface that covers all of the dashboard's required operations: session
discovery, transcript reads, new threads, resume, streamed state, approvals,
steering an in-flight turn, interruption, and archive/unarchive. Speak its
JSON-RPC-like protocol directly from Rust over a non-TTY stdio connection.

There is an important ownership boundary: an App Server process can list the
persisted transcript of a thread active in another Codex process, but it cannot
observe that thread's live state or control its turn. A disposable two-server
probe found the externally owned thread as `notLoaded`; `thread/loaded/list`
was empty and `turn/interrupt` returned `thread not found`. Therefore:

- arbitrary pre-existing Codex CLI sessions are discoverable only as persisted,
  read-only history;
- reliable live control requires `open-agent-view` to launch and retain the App
  Server that owns the thread, or to connect to the already-owning server;
- sessions intended to be shared with the ordinary Codex TUI should connect
  that TUI to the same server through `codex --remote`;
- the adapter must never try to "attach" to an externally active thread by
  starting a second server and resuming its rollout. That risks two owners and
  is not a supported control path.

This makes the App Server supervisor, rather than the session JSONL files, the
control plane for Codex sessions created by `coding-agents`.

## Official sources

- [Codex App Server](https://learn.chatgpt.com/docs/app-server.md): protocol,
  lifecycle, methods, event types, approvals, and authentication RPCs.
- [Codex CLI command reference](https://learn.chatgpt.com/docs/developer-commands.md?surface=cli):
  installed commands, transports, remote TUI, and session commands.
- [Authentication](https://learn.chatgpt.com/docs/auth.md): login modes,
  credential storage, headless login, and Docker guidance.
- [Non-interactive mode](https://learn.chatgpt.com/docs/non-interactive-mode.md):
  `codex exec --json`, JSONL events, and exec-session resume.
- [Codex SDK](https://learn.chatgpt.com/docs/codex-sdk.md): higher-level
  TypeScript and Python wrappers and their intended use.
- [Open-source App Server implementation](https://github.com/openai/codex/tree/main/codex-rs/app-server):
  upstream implementation referenced by the official documentation.

The current manual used for this review was refreshed by the bundled OpenAI
documentation helper and reported current at
`/tmp/openai-docs-cache/codex-manual.md`.

## Why the other official surfaces are insufficient

| Surface | Useful for | Missing for this product |
| --- | --- | --- |
| App Server | Rich clients and all required controls | Experimental; ownership is process-local |
| `codex exec --json` | One unattended job and a machine-readable event stream | No arbitrary session listing, interactive approvals, in-flight steer, or cross-process control |
| Interactive `codex`/`resume` | Human terminal use | Owns the screen and is not a stable machine protocol |
| CLI `archive`, `delete`, `unarchive` | Narrow fallback maintenance | No live event stream or rich inspection |
| TypeScript/Python SDK | Starting and continuing programmatic coding threads | Wrong implementation language and less direct access to the dashboard protocol |
| Codex MCP server | Letting another agent call Codex | Not a session dashboard/control API |

`codex exec --json` remains a reasonable launch-only compatibility fallback for
older CLIs without App Server. It must be advertised as reduced capability, not
made to imitate live steering or approval handling.

## App Server protocol

### Transport and handshake

The wire format follows JSON-RPC 2.0 request/response semantics but omits the
`"jsonrpc":"2.0"` member.

- Default `stdio://`: one JSON object per line on stdin/stdout.
- WebSocket: one JSON object per text frame. The current documentation marks
  WebSocket transport experimental and unsupported.
- Unix socket: a WebSocket handshake over the default or an explicit Unix
  socket path.

A client sends exactly one `initialize` request, waits for the matching
response, then sends the `initialized` notification. No other request is valid
before that handshake. Use client metadata such as:

```json
{"method":"initialize","id":1,"params":{"clientInfo":{"name":"open_agent_view","title":"Open Agent View","version":"0.1.10"}}}
```

The 0.144.4 response included `userAgent`, `codexHome`, `platformFamily`, and
`platformOs`. It did not advertise a protocol version or a supported-method
list. Responses can arrive in a different order from requests, so the client
must route them by `id`. Notifications and server-initiated requests can be
interleaved at any time.

Production stdio transport must not allocate a PTY. Parse stdout exclusively as
JSONL and capture bounded stderr separately; Codex diagnostics and ANSI-colored
logs are written to stderr. A PTY also echoes requests and destroys this clean
boundary.

For remote WebSocket use, bind only to loopback unless authentication and TLS
termination are configured. The documented auth modes are capability tokens
and signed bearer tokens. Never put raw tokens in argv; use a token file or the
client's `--remote-auth-token-env` facility. Local stdio is preferable for the
first release.

### Required method set

| Product capability | App Server operation | Notes |
| --- | --- | --- |
| Discover | `thread/list` | Cursor-paginated; pass explicit `sourceKinds` |
| Read | `thread/read` with `includeTurns` | Does not resume, load, or subscribe |
| Start | `thread/start`, then `turn/start` | Starting a thread alone does not call the model |
| Resume | `thread/resume`, then `turn/start` | Resume subscribes this connection to events |
| Reply while idle | `turn/start` | New user turn |
| Steer while working | `turn/steer` | Requires the exact active `expectedTurnId` |
| Interrupt | `turn/interrupt` | Final event has turn status `interrupted` |
| Archive | `thread/archive` | Emits one event per archived thread/descendant |
| Restore | `thread/unarchive` | Separate list uses `archived: true` |
| Delete | `thread/delete` | Permanent; keep behind explicit confirmation |
| Loaded inventory | `thread/loaded/list` | Process-local, not a machine-wide live inventory |
| Auth status | `account/read` | Use `refreshToken: false` for ordinary polling |

For discovery, explicitly request all relevant source kinds. Omitting
`sourceKinds` (or passing `[]`) defaults to the interactive `cli` and `vscode`
sources and silently excludes exec, App Server, and subagent threads. The
0.144.4 schema accepts `cli`, `vscode`, `exec`, `appServer`, `subAgent`,
`subAgentReview`, `subAgentCompact`, `subAgentThreadSpawn`, `subAgentOther`, and
`unknown`.

`thread/list` defaults to non-archived records and returns empty `turns` in its
summaries. Follow a selected row with `thread/read`; do not resume merely to
inspect it. `thread/read` responses are safe for a history/details view and can
include full turns.

The dashboard's default active-only refresh uses the owning server's
`thread/loaded/list`, filters managed hosts to exact supervisor-owned IDs, and
reads only those records. It does not page persisted rollouts merely to prove
that no live OAV-owned session exists. Explicit completed-history discovery
uses `thread/list`, newest first by default, with the shared 100-record budget;
an opaque next cursor produces a usable partial snapshot plus a warning rather
than the former all-or-nothing safety-cap failure.

Current experimental APIs include `thread/turns/list` and `thread/items/list`.
They can page large transcripts without loading a thread, but require
`initialize.params.capabilities.experimentalApi = true`. Keep them out of the
initial compatibility floor; use `thread/read` and add paged history behind a
negotiated feature later.

### Thread and turn lifecycle

The three core objects are a thread (conversation), a turn (one user request
and the resulting work), and an item (message, command, edit, tool call, and so
on).

Start flow:

```text
thread/start -> response + thread/started
turn/start   -> response + thread/status/changed(active) + turn/started
             -> item/started, deltas, item/completed ...
             -> thread/status/changed(idle) + turn/completed
```

The observed 0.144.4 thread status union is:

- `notLoaded`
- `idle`
- `systemError`
- `active`, with `activeFlags` such as `waitingOnApproval` and
  `waitingOnUserInput`

Recommended normalized mapping:

| Codex state | Dashboard state |
| --- | --- |
| `active` with a waiting flag | `needs_input` |
| other `active` | `working` |
| `systemError` | `needs_input` with error diagnostic |
| `idle` after a completed/interrupted/failed turn | `completed` (or `ready_for_review` when the UI has an explicit review signal) |
| `notLoaded` with stored history | derive from the last turn; otherwise `unknown` |

Codex does not provide a direct equivalent of Claude's user-facing “ready for
review” bucket. The application should derive that presentation from the last
completed turn plus local unread/review state, not invent a provider status.

Treat `item/completed` as authoritative for final item state. Useful events
include:

- `turn/started`, `turn/completed`, `turn/diff/updated`, and
  `turn/plan/updated`;
- `item/started`, `item/completed`, `item/agentMessage/delta`, reasoning
  summary deltas, and command output deltas;
- `thread/status/changed`, `thread/archived`, `thread/unarchived`,
  `thread/deleted`, and `thread/closed`;
- `error`, which can be retryable before a final failed turn.

Items include user and agent messages, plans, reasoning, commands, file
changes, MCP and dynamic tool calls, collaboration calls, web search, image
view, review markers, and context compaction. Preserve unknown item and event
types as raw JSON for forward compatibility.

### Input, approvals, and interruption

Use `turn/start` for an idle thread. Use `turn/steer` only while the store has a
known active turn; it appends input to that turn, does not emit another
`turn/started`, and rejects turn-level overrides. The required
`expectedTurnId` prevents steering a stale or newly replaced turn. A 0.144.4
probe confirmed success while active and `no active turn to steer` afterward.

Interactive approvals are server-initiated requests, not notifications. The
client must preserve their request IDs and return a response. Relevant requests
include command approval, file-change approval, permission approval,
`item/tool/requestUserInput`, MCP elicitation, and dynamic tool calls. Pending
requests are reflected in thread status flags and should populate the
dashboard's Needs input section. `serverRequest/resolved` clears UI state.

Do not paper over incomplete approval handling with permissive policy. Expose
only a decision whose exact request context and result payload are understood;
otherwise send the user to the native client. Never silently upgrade to
danger-full-access. Also do not expose `thread/shellCommand` as a generic
adapter operation: the official documentation says it runs outside the thread
sandbox with full access.

Exact-tag schema generation, disposable 0.144.4 probes, and review of the
[outgoing-message implementation](https://github.com/openai/codex/blob/rust-v0.144.4/codex-rs/app-server/src/outgoing_message.rs)
established the following request contract:

- request and notification envelopes omit `jsonrpc`; a server request is
  `{method,id,params}` and its answer is exactly `{id,result}`. IDs may be
  signed integers or strings and must be returned unchanged;
- command decisions include one-shot `accept` and `decline`; persistent
  `acceptForSession` and policy amendments expand authority and require a
  separate UI;
- file-change approval parameters omit the diff, so acceptance requires a
  complete correlated `item/started` file-change item;
- safe permission denial is `{permissions:{},scope:"turn"}` rather than a
  decline enum; MCP elicitation has its own `decline` action payload;
- structured-input answers are a map keyed by the provider's question IDs.
  Secret answers must never be echoed or persisted.

Pending callbacks belong to the App Server, not the client connection. Closing
a connection does not clear them. A newly initialized connection receives no
replay from `thread/list` or `thread/read`; an exact
[`thread/resume`](https://github.com/openai/codex/blob/rust-v0.144.4/codex-rs/app-server/src/request_processors/thread_lifecycle.rs)
on the still-owning process returns the active turn/items and then replays its
pending requests with the original IDs. Any subscribed connection that knows a
pending ID can answer it, and the first response wins. This requires a
single-controller lease in addition to thread/turn ownership checks. Never
auto-answer on receipt, shutdown, or reconnect; wait for
`serverRequest/resolved` after sending a result.

## Authentication and session storage

App Server supports `account/read`, login start/cancel, logout, account update
notifications, and rate-limit reads. Codex can authenticate with ChatGPT,
OpenAI API keys, enterprise access tokens, certain alternate providers, and
experimental externally managed ChatGPT tokens.

The adapter should normally reuse the target environment's Codex credentials
without reading them:

1. spawn App Server in the intended user/container environment;
2. call `account/read` with `refreshToken: false`;
3. show signed-out state or initiate the documented browser/device-code flow
   only on explicit user action;
4. never inspect, copy, log, or serialize credential contents.

The CLI and App Server use `CODEX_HOME` (default `~/.codex`). Depending on
configuration, cached credentials live in `auth.json` or an OS keyring. File
storage contains access tokens and must be treated as a secret. Managed ChatGPT
tokens refresh during use, so any Docker credential volume used by a real
session must be writable. `CODEX_API_KEY` is documented only for a single
`codex exec` invocation and is not an App Server authentication mechanism.

For Docker targets, prefer login/state already provisioned inside an explicitly
enrolled container or a dedicated writable Codex state volume. Do not
automatically mount the host's entire `~/.codex` into a container. Besides
credential exposure, sharing live state across unrelated App Server processes
creates ownership and concurrent-write hazards.

## 0.144.4 runtime probes

All behavioral probes used disposable containers from the already-local image
with `--rm --network none`. No host directory, credential file, Docker socket,
or user session was mounted. No command was executed in the existing long-lived
containers. The probe image resolved to
`sha256:8f170f660813ac358f347fa8a3580139972f3ea7a9fb087834f1da44669d9392`.

Verified behavior:

- `codex --version` reports `codex-cli 0.144.4`.
- `codex app-server` supports stdio, explicit Unix sockets, WebSocket listen,
  WebSocket auth, and exact-version TypeScript/JSON Schema generation.
- Initialization and `account/read` work without credentials;
  `account/read` returned `account: null` and `requiresOpenaiAuth: true`.
- On an empty home, `thread/list` returned an empty cursor-paginated result.
- `thread/start` succeeded offline and created an idle persisted thread.
- `turn/start` returned an in-progress turn and streamed thread, turn, and item
  events before the model connection failed due to the deliberate network
  block.
- `turn/steer` accepted additional input during that active turn.
- `turn/interrupt` returned `{}`, emitted an idle status transition, and ended
  the turn as `interrupted`.
- `thread/read(includeTurns: true)` returned the interrupted turn;
  `thread/list` returned its summary.
- Archive moved it out of the ordinary listing and emitted
  `thread/archived`; `archived: true` found it; unarchive restored it and
  emitted `thread/unarchived`.
- Responses to concurrent read/list requests arrived out of request order.
- A second App Server sharing the same `CODEX_HOME` saw an active first-server
  thread only as `notLoaded`, returned no loaded threads, and rejected an
  interrupt with `thread not found`.

## 0.147.0 authenticated host lifecycle

An opt-in host probe later exercised the installed Codex 0.147.0 with the
user's existing authentication but an isolated temporary OAV supervisor
directory. It created only marker-named disposable threads under `/tmp` and
used no file-changing prompt.

The probe exposed three integration details that synthetic protocol fixtures
could not:

- `thread/read` can briefly report the previous idle state immediately after a
  successful `turn/start`. OAV must retain the exact recorded active turn until
  the matching `turn/completed` notification or terminal resume payload
  arrives; clearing on the idle snapshot makes a later Ctrl+X fail locally.
- The npm entry point remains a Node signal-forwarding parent while its native
  child owns the Unix listener. Durable identity therefore follows the exact
  current-user process holding the listener inode. Stdio/proxy clients run in
  their own process group so dropping the wrapper cannot strand the native
  child with inherited pipes.
- On this version, `thread/delete` for an App-Server-owned idle thread can
  archive/apply without returning its response and can wedge the owning server.
  OAV never treats a timeout alone as success. It accepts only a normal response
  or exact `thread/deleted {threadId}` notification. If every owned turn is
  idle, it may stop the exact listener through a revalidated pidfd, verify or
  complete deletion through a fresh isolated App Server, restart the durable
  owner, and restore every other ownership record. Active owned work forbids
  that restart.

The final lifecycle passed launch, exact assistant transcript, a second turn,
one-time approval when presented, exact-turn interrupt/completion-race
handling, deletion, server recovery, and process cleanup in 37.13 seconds.
Nine exact marker threads produced while developing the provider-version
workaround were subsequently enumerated across ordinary and archived history,
deleted by ID, and independently confirmed absent.

Authenticated model completion was intentionally not tested. The offline test
did verify retryable `error` notifications and cancellation during retry.

### Differences and compatibility hazards

The current documentation is close to the 0.144.4 generated schema, but the
following details matter:

- App Server and its WebSocket transport remain experimental. Even ungated
  methods can evolve.
- Generated schemas are explicitly version-specific. Generate fixtures using
  the binary under test rather than copying examples from current docs.
- The 0.144.4 CLI does not expose the current documented
  `app-server --code-mode-host` flag.
- The `sandbox` shorthand in 0.144.4 `thread/start` accepts CLI spellings such
  as `read-only` and `workspace-write`; `readOnly` was rejected. Structured
  `sandboxPolicy.type`, used on turn requests, uses camelCase values. Keep the
  two wire types distinct.
- The installed binary exposes experimental paged turn/item methods only in
  schemas generated with `--experimental` and after the client opts into
  `experimentalApi`.
- `initialize` has no negotiated protocol-version response. The client must
  inspect `codex --version`, use conservative methods, and degrade on JSON-RPC
  method/field errors.
- `thread/start` in the probe reported a `source` of `vscode` even though the
  custom client name was `open_agent_view_probe`. Do not derive ownership from
  that display field; maintain an application-side ownership registry.
- `thread/resume` without explicit policy overrides returned effective policy
  values different from the earlier start request. Treat the resume response
  as authoritative and send explicit approved policy overrides when required.
- `thread/list` and `thread/read` can report different timestamps while state
  is being indexed. Avoid filesystem-mtime assumptions; use provider values and
  tolerate eventual consistency.
- 0.144.4 contains `app-server daemon` and `proxy` commands, but the image's npm
  installation cannot start the managed daemon. It requires a standalone Codex
  install at `$CODEX_HOME/packages/standalone/current/codex`. Do not depend on
  the managed-daemon command for this image.

## Concrete adapter design

### Runtime topology

Create one supervised App Server endpoint per enrolled execution target:

```text
host target:    codex app-server --listen unix:///private/state/app-server.sock
host framing:   WebSocket handshake and one JSON object per text frame
Docker target:  Docker exec, argv ["codex", "app-server", "--listen", "stdio://"]
```

The Docker Engine exec must remain attached with stdin/stdout/stderr separated,
TTY disabled, a bounded stderr buffer, and no shell interpolation. The Docker
runtime qualifies provider IDs with the immutable full container ID as already
specified in `docker-runtime.md`.

The implemented host supervisor owns the explicit Unix-socket endpoint for all
sessions launched through it. It records the PID, Linux process start token,
exact command-line bytes, socket path, and exact thread/turn IDs. A reconnect
must revalidate that identity and must never signal a PID loaded from disk. Do
not use `app-server daemon` or treat `app-server proxy --sock` as a bridge to
this listener on the current npm-based image: the listener itself expects a
WebSocket handshake.

### Connection and store

1. Run `codex --version`; accept 0.144.4 as the first tested compatibility
   baseline and reject or degrade unknown older versions.
2. Spawn the server without a PTY and complete the initialization handshake.
3. Call `account/read` and surface auth state without touching secrets.
4. Page `thread/list` with explicit source kinds. Store raw provider status and
   the composite runtime/provider/thread ID.
5. Correlate response promises by JSON-RPC `id`; independently feed
   notifications and server requests into an event reducer.
6. `thread/read` on selection. On a dashboard connection, resume only exact
   active threads recorded as owned by this supervisor; this restores the
   subscription and replays unresolved provider requests.
7. After `thread/start` or the ownership-checked `thread/resume`, keep the
   connection subscribed and apply status/item/request events to the normalized
   store.
8. Route idle input to `turn/start`, active input to `turn/steer`, and interrupt
   only with the currently recorded turn ID.

Use one serialized writer and correlate responses by ID. Stdio transports send
newline-delimited JSON; the Unix transport sends one JSON object per WebSocket
text frame. Maintain maps for pending client requests, pending server requests,
loaded threads, active turns, and per-item streaming buffers. On disconnect,
mark the target unavailable without marking its stored sessions deleted.
Reconcile with `thread/list` after reconnect, but do not claim live control
until the owning endpoint is restored.

### Compatibility strategy

- Keep core request/response structs typed, but deserialize envelopes,
  notifications, item unions, enums, and extra fields tolerantly.
- Preserve unknown JSON and show an adapter diagnostic instead of failing the
  whole target.
- Ignore unknown notifications after recording them; never ignore a
  server-initiated request that could block a turn.
- Treat JSON-RPC `-32601` and experimental-capability errors as per-capability
  degradation.
- Check `codex --version` at connection time and include it in diagnostics.
- In CI, run `codex app-server generate-json-schema` for each supported CLI
  fixture and validate representative transcripts against the exact generated
  schema. Do not generate into the user's project at runtime.
- Start with the non-experimental API. Add `experimentalApi` only for a feature
  whose UI, tests, and fallback are complete.
- Never infer provider ownership from thread `source`, rollout path, PID scans,
  or a shared SQLite record. Store ownership when the supervisor starts the
  endpoint/thread.

This design provides full control for sessions launched through
`coding-agents`, honest read-only discovery for other persisted Codex sessions,
and explicit capability degradation instead of fragile transcript scraping.

# Architecture

## Design overview

`coding-agents` is divided into four layers:

```text
CLI/TUI
  |
application state + commands
  |
normalized session model
  |
provider adapters (Claude, Codex, Pi, OpenCode, Cursor, Copilot,
                   Antigravity, Docker, fixtures)
```

The TUI does not interpret provider-specific JSON or invoke CLIs directly.
Adapters normalize provider state and advertise supported capabilities. This
keeps unsupported operations visible and safe instead of guessing commands.

## Normalized model

Every session has:

- a stable provider-qualified identifier;
- provider, runtime, working directory, and optional process/container data;
- one normalized lifecycle state;
- timestamps and a latest useful summary;
- a capability set describing which controls are actually available.

The initial lifecycle states are `ready_for_review`, `needs_input`, `working`,
`completed`, and `unknown`. Raw provider states are retained for diagnostics.

## Adapter contract

Adapters implement discovery first. Control methods are enabled independently:

- `list`
- `inspect`
- `launch`
- `reply`
- `approve` / `decline`
- `respond` (structured input, distinct from conversational reply)
- `resume`
- `interrupt`
- `archive`
- `delete`

An adapter must return an explicit unsupported result when its installed CLI
version cannot perform an action safely.

The control layer separately exposes a normalized `LaunchTarget` inventory.
The task composer renders only those launch-capable targets in its harness
picker. Moving through the picker changes UI selection only; confirmation
switches the harness and resets incompatible model state, while cancellation
preserves both the current harness and draft. `/harness`, `/model`, and the
other dashboard slash commands are reduced locally and never forwarded as a
provider prompt.

## Process model

Read-only discovery runs with strict timeouts. Durable managed providers use a
small local supervisor so work can continue when the TUI closes; providers
whose protocol grants only connection-local authority retain control for the
dashboard process instead. Provider sessions remain the source of truth for
conversation state.

Discovery sources run concurrently. During the first refresh, the dashboard
publishes completed provider results incrementally instead of waiting for the
slowest CLI; the final snapshot is then enriched with ownership-gated control
capabilities. Later refreshes replace the queue atomically and remain entirely
off the input thread. Terminal input is drained in bounded bursts before a
draw. Each group initially contributes at most 25 session rows plus a
selectable Show more control; revealed rows use page-aligned viewports.
Filtering and counts still operate over the full snapshot. Together these
rules bound render work and prevent key-repeat frame backlogs while preserving
exact selection movement.

The default refresh period is 15 seconds because starting several provider
CLIs—especially Claude—can consume substantial CPU and memory even for a small
JSON response. `ctrl+l` requests an immediate refresh. Status grouping is
computed in one snapshot pass, provider labels are deduplicated without sorting
the full history, and ready key/typing bursts produce one final frame.

Completed sessions are excluded from the default discovery request rather than
merely hidden after rendering. This lets providers avoid expensive history
work: Claude does not receive its `--all` flag, and OpenCode does not run its
global persisted-session database query. Because provider versions can violate
those flags, the discovery engine independently enforces completed,
interactive, and cwd filters before every partial snapshot is published.
`--all` opts into both discovery and display. Bulk Codex archive is a separate
bounded maintenance path with a read-only plan, explicit `--yes`, and the same
per-session ownership checks as the TUI.

The implemented Claude path uses Claude's own background service. The
dashboard persists only a provider/runtime/session ownership record and invokes
the supported `attach`, `logs`, and `stop` commands. Managed host Codex uses one
detached App Server listening with WebSocket framing on a private Unix socket.
The dashboard reconnects to that endpoint and grants control only for exact
thread and active-turn IDs stored with its verified Linux process identity.
Server-initiated requests are reduced into a volatile per-connection queue;
they are never reconstructed from transcript files. A separate process-held
controller lease ensures that only one dashboard instance advertises response
authority for a supervisor.

Managed Pi uses a separate detached supervisor because Pi's public RPC
transport is stdio-only. The supervisor retains the child pipes, correlates
JSONL responses, reduces lifecycle/dialog events, and exposes a private Unix
socket to later dashboard clients. Exact Linux process identity protects the
supervisor endpoint; canonical Pi session UUIDs protect every child operation.
The same adapter also reads Pi's documented JSONL store. Those external history
records remain inspect/open-only and are never promoted based on timestamps or
PIDs.

Managed OpenCode on Linux owns one durable authenticated `opencode serve`
process on an ephemeral loopback port. Its private record contains the random
Basic-auth secret, exact Linux process/listener identity, and only the canonical
session IDs created through that server. Later dashboards revalidate all of
those facts before reconnecting. The ordinary CLI history/export path remains
inspect/native-open only and is never promoted from a matching ID.

Managed Cursor on Linux creates chats through the documented CLI and runs each
turn as a detached stream-JSON process. A private registry stores the exact
process identity and bounded log paths, allowing later dashboards to rediscover
only OAV-owned runs. Cursor's external TTY-only picker is not scraped, so there
is no global Cursor list. Reply begins a new process only after the prior turn
is idle; interrupt revalidates the exact Linux process identity first.

GitHub Copilot uses two ACP authority tiers. A discovery connection calls
`session/list`; its persisted results remain observe/native-open only. A
separate retained control connection owns only the sessions it creates during
the current dashboard process, carries their live events, and enables prompt,
inspect, cancel, and exact one-shot permission choices. That managed authority
is process-local and is not reconstructed from persisted session metadata.

Direct Docker Codex discovery uses a bounded stdio App Server and remains
observe/open-only.

Fixture discovery is a test input, not an adapter authority source. When
`--fixture` is present, the control hub fences every provider-I/O method before
dispatch even if the synthetic record advertises actionable capabilities. This
allows real-PTY tests to render and route every UI action without contacting a
provider or Docker.

## Docker boundary

Docker is a runtime wrapper, not a provider. A container can expose Claude,
Codex, or both. Discovery is opt-in through an explicit container name, image,
or project label. The runtime adapter executes provider probes inside that
container and qualifies session identifiers with the container ID.

## Compatibility policy

Protocol parsing is fixture-driven and version-aware. Unknown fields are
ignored, unknown states are preserved, and malformed records do not discard
healthy sessions from other providers.

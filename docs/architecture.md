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
preserves both the current harness and draft. Selectable-model controllers also
expose a read-only catalog method. `shift+tab` from the task composer loads that
catalog on a worker and opens a separately filtered, ten-row-page picker
without changing the draft; `/model` opens the same picker as a command, while
`/model NAME` preserves an exact custom-identifier path. The catalog contract is
implemented by Claude CLI help aliases, Codex App Server `model/list`, Pi's
offline list, OpenCode's configured-model list, Cursor's account catalog,
Copilot's short-lived headless SDK catalog, and Antigravity's model command. A
catalog authentication failure becomes an explicit login action when the
provider has a native setup surface; the same catalog reloads on return.
`/harness`, `/model`, `/login`, `/completed`, and the other dashboard slash
commands are reduced locally and never forwarded as a provider prompt.

Launch presentation is explicit in the same controller contract. Background
controllers run on a worker while the event loop advances a status spinner.
Foreground controllers suspend the dashboard before provider I/O. Claude
creates an exact `--background` UUID, verifies it in `claude agents`, and opens
`attach`; Antigravity starts its sandboxed native UI. Both use the native PTY
bridge so Left retains the frontend and returns to the dashboard.

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
draw. Each group initially contributes a terminal-sized page capped at 25
session rows plus a selectable Show more control; revealed rows use
page-aligned viewports.
Filtering and counts still operate over the full snapshot. Together these
rules bound render work and prevent key-repeat frame backlogs while preserving
exact selection movement.

The application caches normalized ID indices, status counts, provider labels,
and current-view groups when a snapshot/filter/view changes. Navigation and
draw queries reuse those caches instead of rebuilding a 70,000-row grouping on
each arrow event. Local hidden-session filtering uses a hash set and preserves
source order, so a large snapshot is filtered in one pass.

The default refresh period is 15 seconds because starting several provider
CLIs—especially Claude—can consume substantial CPU and memory even for a small
JSON response. `ctrl+l` requests an immediate refresh. Status grouping is
computed in one snapshot pass, provider labels are deduplicated without sorting
the full history, and ready key/typing bursts produce one final frame.

Default discovery is ownership-scoped and includes completed managed work.
Claude rows are filtered against OAV's private launch registry; Codex, Pi,
OpenCode, Cursor, and Copilot contribute only exact supervisor-owned records;
Antigravity contributes only the exact OAV-owned conversation that still
matches its documented last-conversation cache. Explicit Docker targets remain visible because naming them on the
command line is an intentional enrollment action.

`--include-external` adds provider-wide read-only history. Completed sessions
remain a separate visibility control: `/completed show` and `/completed hide`
update the refresh worker without changing the ownership scope;
`--hide-completed` selects an active-only startup and `--all` remains a
compatibility flag. Providers still avoid expensive history work when hidden:
Claude does not receive its `--all` flag, and
OpenCode never runs its global persisted-session database query unless both
external and completed scopes are enabled. Because provider versions can
violate filters, the discovery engine independently enforces completed,
interactive, and cwd rules before every partial snapshot is published.
External history is capped per provider (100 records by default), and a partial
result is returned with a warning instead of discarding the whole provider.
Codex's default active path uses `thread/loaded/list` plus exact reads rather than
scanning rollouts; OpenCode pushes the limit into its read-only SQL query and
streams one JSON-encoded TSV row at a time. Bulk
Codex archive is a separate bounded maintenance path with a read-only plan,
explicit `--yes`, and the same per-session ownership checks as the TUI.

Launch I/O also runs off the terminal-input thread. A successful controller
outcome includes an exact provider session hint when available; the dashboard
refreshes immediately, selects the matching normalized row, and retries delayed
provider persistence at a bounded interval and deadline.

Local hiding is deliberately outside the provider capability model. The
private `hidden-sessions.json` registry stores exact normalized IDs, and every
snapshot is filtered before it reaches the application. Hiding retains
provider history and live processes; unhide removes only the local record.
Provider Interrupt/Delete/Archive remain separately capability-gated operations
with their existing revalidation rules.

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
With `--include-external`, the same adapter also reads Pi's documented JSONL
store. Those external history records remain inspect/open-only and are never
promoted based on timestamps or PIDs.

Managed OpenCode on Linux owns one durable authenticated `opencode serve`
process on an ephemeral loopback port. Its private record contains the random
Basic-auth secret, exact Linux process/listener identity, and only the canonical
session IDs created through that server. Later dashboards revalidate all of
those facts before reconnecting. With `--include-external`, the ordinary CLI
history/export path remains inspect/native-open only and is never promoted from
a matching ID.

Managed Cursor on Linux creates chats through the documented CLI and runs each
turn as a detached stream-JSON process. A private registry stores the exact
process identity and bounded log paths, allowing later dashboards to rediscover
only OAV-owned runs. Cursor's external TTY-only picker is not scraped, so there
is no global Cursor list. Its account model IDs come from `cursor-agent models`,
are revalidated at launch, and are retained for later replies. Reply begins a new process only after the prior turn
is idle; interrupt revalidates the exact Linux process identity first.

GitHub Copilot uses two ACP authority tiers. With `--include-external`, a
discovery connection calls `session/list`; its persisted results remain
observe/native-open only. A
separate retained control connection owns only the sessions it creates during
the current dashboard process, carries their live events, and enables prompt,
inspect, cancel, and exact one-shot permission choices. That managed authority
is process-local and is not reconstructed from persisted session metadata. A
separate short-lived headless SDK connection calls `models.list` and exits
without creating a session; ACP applies the selected model before the first
prompt.

Antigravity exposes no documented all-conversation listing protocol. OAV
records exact conversations it launches and correlates them with the documented
workspace-to-last-conversation cache. Launch uses `--sandbox`, an optional exact
`--model`, and `--prompt-interactive`; it never adds the dangerous
permission-bypass flag. An older conversation can no longer be rediscovered
after the provider replaces that workspace's last-conversation entry.

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

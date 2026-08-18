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
provider adapters (Claude, Codex, Docker, fixtures)
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

## Process model

Read-only discovery runs with strict timeouts. Managed sessions use a small
local supervisor so work can continue when the TUI closes. The supervisor owns
process identifiers and logs; provider sessions remain the source of truth for
conversation state.

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
Direct Docker Codex discovery uses a bounded stdio App Server and remains
observe/open-only.

## Docker boundary

Docker is a runtime wrapper, not a provider. A container can expose Claude,
Codex, or both. Discovery is opt-in through an explicit container name, image,
or project label. The runtime adapter executes provider probes inside that
container and qualifies session identifiers with the container ID.

## Compatibility policy

Protocol parsing is fixture-driven and version-aware. Unknown fields are
ignored, unknown states are preserved, and malformed records do not discard
healthy sessions from other providers.

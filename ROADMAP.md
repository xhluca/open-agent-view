# Roadmap

## Milestone 0 — Interface archaeology

- Record the observed `claude agents` layout, keymap, and state transitions.
- Inventory Claude/Codex session and Docker discovery surfaces.
- Write provider contracts and safety boundaries.

## Milestone 1 — Read-only dashboard

- Normalize fixture, host Claude, host Codex, and Docker-discovered sessions.
- Render grouped status sections with responsive terminal layouts.
- Add navigation, details, help, filtering, and JSON output.
- Cover parsers and layout with deterministic tests.

## Milestone 2 — Managed sessions

- Launch Claude and Codex tasks from the composer.
- Persist managed process metadata across dashboard restarts.
- Resume/reply, interrupt, archive, and delete with explicit confirmations.
- Surface logs and provider errors without leaking credentials.

## Milestone 3 — Distribution

- Install a single `coding-agents` binary with Cargo or a release archive.
- Publish checksums and reproducible smoke-test instructions.
- Validate Linux terminals, SSH, tmux, and narrow-window behavior.


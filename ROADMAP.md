# Roadmap

## Milestone 0 — Interface archaeology ✅

- Record the observed `claude agents` layout, keymap, and state transitions.
- Inventory Claude/Codex session and Docker discovery surfaces.
- Write provider contracts and safety boundaries.

## Milestone 1 — Read-only dashboard (in progress)

- [x] Normalize fixture, host Claude, host Codex, and Docker-discovered sessions.
- [x] Render grouped status sections with responsive terminal layouts.
- [x] Add navigation, details, help, filtering, and JSON output.
- [x] Cover parsers and layout with deterministic tests.
- [ ] Enrich Claude rows with supported latest-message metadata when available.
- [x] Retain one reconnectable host Codex App Server across dashboard restarts.

## Milestone 2 — Managed sessions

- [x] Launch host Claude tasks from the composer.
- [x] Persist Claude ownership metadata across dashboard restarts.
- [x] Open provider-native Claude/Codex sessions from a selected row.
- [x] Stop only exact Claude sessions launched by `coding-agents`.
- [x] Own host Codex launch and exact-turn interrupt through a durable,
  reconnectable App Server supervisor.
- [x] Reply/steer, archive, and delete owned Codex threads with explicit
  capability checks and confirmations.
- [x] Interrupt owned Claude sessions and exact active Codex turns.
- [x] Surface supported Claude logs and provider errors without leaking
  credentials.
- [x] Implement immutable-ID and authority-gated managed-Docker primitives.
- [x] Expose managed-Docker create/start/stop/remove through confirmed CLI
  workflows and persist external ownership records.
- [x] Render exact owned Codex requests, answer one-shot command approvals,
  deny file/permission/MCP requests safely, and collect non-secret structured
  input without persistence.
- [ ] Add diff-correlated file acceptance, explicit permission grants, typed MCP
  form acceptance, and masked secret-input entry.

## Milestone 3 — Distribution

- Install a single `coding-agents` binary with Cargo or a release archive.
- Publish checksums and reproducible smoke-test instructions.
- Validate Linux terminals, SSH, tmux, and narrow-window behavior.

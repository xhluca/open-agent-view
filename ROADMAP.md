# Roadmap

Status at the current private pre-alpha checkout. A checked implementation item
means it is present and covered by the validation layer named in
[docs/testing.md](docs/testing.md); it does not imply a public release or an
authenticated fresh-container end-to-end run.

## Milestone 0 — Interface archaeology ✅

- [x] Record the observed `claude agents` layout, keymap, and state transitions.
- [x] Inventory Claude/Codex session and Docker discovery surfaces.
- [x] Write provider contracts and safety boundaries.

## Milestone 1 — Read-only dashboard (in progress)

- [x] Normalize fixture, host Claude, host Codex, and Docker-discovered sessions.
- [x] Render grouped status sections with responsive terminal layouts.
- [x] Add navigation, details, help, filtering, and JSON output.
- [x] Cover parsers and layout with deterministic tests.
- [ ] Enrich Claude rows with supported latest-message metadata when available.
- [x] Retain one reconnectable host Codex App Server across dashboard restarts.

## Milestone 2 — Managed sessions (in progress)

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
- [ ] Add supported status/stop and log rotation for the detached Codex
  supervisor.
- [ ] Integrate session launch/control into managed containers; current managed
  Docker commands control only the container lifecycle.

## Milestone 3 — Validation and distribution (in progress)

- [x] Build and install a single `coding-agents` binary from an authorized
  checkout with the locked Rust 1.75 dependency set.
- [x] Run locked CI tests/release builds on Rust 1.75.0 and stable.
- [x] Prepare a tag-gated Linux x86_64 archive/checksum workflow and document
  its maintainer procedure.
- [x] Verify both empty TUIs in separate fresh, network-disabled Docker PTYs and
  record the credential limitation.
- [x] Provide exact fresh-container commands, a synthetic all-state fixture,
  exhaustive key-route checklist, troubleshooting, and maintainer/security
  documentation.
- [x] Exercise every current TUI action route through the actual binary in
  isolated real PTYs, including wide, ordinary, narrow, and too-small layouts.
- [x] Recheck the populated Open Agent View and empty Claude reference TUIs in
  separate fresh Docker PTYs at wide, narrow, and tiny sizes.
- [ ] Complete an explicitly authorized authenticated Claude and Codex
  lifecycle in dedicated fresh environments.
- [ ] Validate and record SSH, additional terminal/theme combinations, and any
  supported non-Linux read-only behavior.
- [ ] Push a signed version-matching tag and verify the published checksum and
  archive. No tag or GitHub release exists yet.
- [ ] Decide public-repository/package publication only after the private
  release gates and security review pass.

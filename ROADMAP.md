# Roadmap

Status at the current private-preview checkout. A checked implementation item
means it is present and covered by the validation layer named in
[docs/testing.md](docs/testing.md); it does not imply a public release or an
authenticated fresh-container end-to-end run.

## Milestone 0 — Interface archaeology ✅

- [x] Record the observed `claude agents` layout, keymap, and state transitions.
- [x] Inventory the seven host-provider session surfaces and Docker boundaries.
- [x] Write provider contracts and safety boundaries.

## Milestone 1 — Read-only dashboard (in progress)

- [x] Normalize fixture, seven host providers, and Docker-discovered Claude and
  Codex sessions.
- [x] Render grouped status sections with responsive terminal layouts.
- [x] Add navigation, details, help, filtering, and JSON output.
- [x] Keep completed discovery opt-in at startup and allow a live
  `/completed show|hide` transition without restarting the dashboard.
- [x] Keep ordinary discovery restricted to OAV-created/managed sessions, with
  explicit bounded `--include-external` provider-history review.
- [x] Provide reversible local hide/unhide for observe-only provider history,
  distinct from provider-native archive/delete.
- [x] Cover parsers and layout with deterministic tests.
- [ ] Enrich Claude rows with supported latest-message metadata when available.
- [x] Retain one reconnectable host Codex App Server across dashboard restarts.

## Milestone 2 — Managed sessions (in progress)

- [x] Launch host Claude tasks from the composer.
- [x] Select every configured launch-capable harness from a visible,
  draft-preserving keyboard palette, with explicit model state.
- [x] Load searchable provider-native model catalogs asynchronously for Claude,
  Codex, Pi, and OpenCode through a draft-preserving Shift+Tab route while
  retaining exact custom model identifiers.
- [x] Persist Claude ownership metadata across dashboard restarts.
- [x] Open provider-native sessions from selected rows where the provider
  exposes an exact resume command, using Enter or Right; Left returns from
  inline Peek and Claude documents Ctrl+Z return.
- [x] Stop only exact provider-listed active host Claude background sessions,
  with a fresh full-UUID/state/kind revalidation immediately before mutation.
- [x] Own host Codex launch and exact-turn interrupt through a durable,
  reconnectable App Server supervisor.
- [x] Own Linux Pi launch, reconnect, reply/steer, exact dialog response, and
  interrupt through a durable stdio-RPC supervisor, including exact selected
  models and safe upgrade of idle older daemons; retain history-only support
  elsewhere.
- [x] Own Linux OpenCode launch, reconnect, live discovery, inspection, reply,
  and interrupt through an authenticated loopback server, including documented
  model selectors; keep external history inspect/native-open only.
- [x] Own Linux Cursor launch, rediscovery, bounded inspection, idle reply, and
  exact verified-process interrupt without scraping its external TTY picker.
- [x] Own process-local Copilot ACP launch, prompt/reply, inspection, cancel,
  and exact one-shot permission response; keep persisted list rows
  observe/native-open.
- [x] Reply/steer, archive, and delete owned Codex threads with explicit
  capability checks and lifecycle-state revalidation.
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
- [x] Prepare a tag-gated archive/checksum workflow for Linux x86_64/aarch64
  and macOS x86_64/aarch64, plus a checksum-verifying binary installer, and
  document the maintainer procedure.
- [x] Verify both empty TUIs in separate fresh, network-disabled Docker PTYs and
  record the credential limitation.
- [x] Provide exact fresh-container commands, a synthetic all-state fixture,
  exhaustive key-route checklist, troubleshooting, and maintainer/security
  documentation.
- [x] Exercise every current TUI action route through the actual binary in
  isolated real PTYs, including wide, ordinary, narrow, and too-small layouts.
- [x] Recheck the populated Open Agent View and empty Claude reference TUIs in
  separate fresh Docker PTYs at wide, narrow, and tiny sizes.
- [x] Probe Pi's real RPC and native-TUI contracts in disposable credential-free
  directories, and cover the managed model lifecycle with an isolated fake RPC
  provider.
- [x] Probe OpenCode's history and real credential-empty loopback server in
  disposable directories, and cover managed reconnect/control with an isolated
  authenticated server fixture.
- [ ] Complete an explicitly authorized authenticated Claude and Codex
  lifecycle in dedicated fresh environments.
- [ ] Validate and record SSH, additional terminal/theme combinations, and any
  supported non-Linux read-only behavior.
- [x] Push version-matching private-preview tags and verify the complete native
  archive/checksum set through the published installer workflow (through
  v0.1.10).
- [x] Manually publish the fully gated v0.1.13 Linux x86-64 archive/checksum
  after hosted jobs became unavailable, with unsupported platforms stated
  explicitly rather than filled with untested artifacts.
- [ ] Decide public-repository/package publication only after the private
  release gates and security review pass.

# Changelog

All notable changes will be documented here. Open Agent View remains an early
private preview and may change before a public stable release.

The format follows [Keep a Changelog](https://keepachangelog.com/en/1.1.0/),
and future released versions are intended to follow Semantic Versioning.

## [Unreleased]

### Changed

- Managed Codex completion now follows exact `turn/completed` IDs instead of
  clearing interrupt authority from a briefly stale idle snapshot. Process
  transports own and stop their whole wrapper/native process group, and the
  durable supervisor records the native process that owns its Unix listener.
- Deleting an idle OAV-owned Codex thread archives it first. Codex 0.147 can
  wedge its owning App Server during `thread/delete`; OAV accepts only an exact
  response/`thread/deleted` notification, otherwise restarts only an entirely
  idle exact owner and completes deletion through an isolated App Server while
  preserving all other ownership records.
- Completed history now has a 100-record per-provider refresh budget with an
  explicit `--history-limit` override. Codex returns a useful partial page
  instead of dropping every Codex row after a hard cap, and OpenCode pushes the
  limit into a streaming read-only query instead of materializing roughly
  70,000 rows.
- Default Codex discovery uses the owning App Server's loaded inventory and
  exact thread reads, avoiding persisted-rollout scans on every active refresh.
- Conventional user-local provider installs are resolved after `PATH`, so
  Codex in `~/.npm-global/bin` and OpenCode in `~/.opencode/bin` work without
  manual flags while explicit executable paths remain exact.

### Fixed

- A newly started Codex turn can no longer lose Ctrl+X/reply authority when
  `thread/read` briefly reports the previous idle state.
- Dropping a Codex stdio/proxy client no longer kills only an npm wrapper and
  hangs forever while its native child retains stdout/stderr.
- A resolved npm shim can reconnect to the same verified durable Codex script
  without weakening process-identity checks; a changed symlink target still
  refuses.
- History warnings no longer replace model/harness/composer contextual keys
  while an overlay is active.

## [0.1.12] - 2026-08-20

### Fixed

- Fresh homes now secure the shared Open Agent View state root before any
  provider supervisor creates a child directory. This prevents a conventional
  `0022` umask from leaving the parent at `0755` and blocking startup.
- Real-PTY tests force the release runner's `0022` umask, so the permission
  ordering is covered deterministically on every platform. The failed
  `v0.1.11` tag remains immutable and unpublished; v0.1.12 includes all changes
  documented below.

## [0.1.11] - 2026-08-20

### Added

- `/completed [show|hide]` changes completed-history discovery from inside the
  running dashboard; `--all` remains the initial CLI opt-in.
- `coding-agents sessions hide`, `unhide`, and `hidden` provide reversible
  local suppression for external/observe-only rows without modifying provider
  history or live processes. Ctrl+X offers the same confirmed fallback from a
  session row or Peek when provider stop/delete authority is absent.
- Shift+Tab from the task composer opens a draft-preserving, searchable,
  asynchronous catalog picker for Claude, Codex, Pi, and OpenCode; `/model`
  opens the same picker as a command. Exact custom `/model NAME` and
  provider-default selection remain available.

### Changed

- Pi modeled launch passes the selected identifier to its RPC child; OpenCode
  sends the documented `providerID`/`modelID` object. Claude derives aliases
  from the installed CLI help and Codex pages visible models from the owning
  App Server rather than relying on a hard-coded catalog.
- Successful managed launches trigger immediate discovery and exact new-row
  selection, with bounded retries for providers that persist a record after the
  launch response. Launch and model-list I/O stay off the terminal-input thread.
- Snapshot indices, counts, labels, and groups are cached until the snapshot,
  filter, or view changes, keeping navigation bounded even with 70,000 rows.
- The Show-more page is now sized to the current terminal height and capped at
  25, so the pagination control remains reachable instead of starting below a
  short viewport.

### Fixed

- The bottom composer cursor no longer includes a nonexistent left-border
  column, removing the one-cell offset.
- Pi and OpenCode no longer incorrectly report that their managed launch paths
  lack model selection.
- A verified pre-model Pi daemon is upgraded only when all of its owned work is
  completed; active work now produces an actionable refusal instead of being
  abandoned.

### Security

- Local hiding is stored in a private, atomic, symlink-refusing registry and is
  never presented as provider deletion or archive. Provider mutations retain
  their exact capability checks and confirmations.

## [0.1.10] - 2026-08-20

### Fixed

- The native release matrix now runs the managed-Pi two-harness process probe
  only on Linux, matching Pi's documented durable-supervisor support. Portable
  state, input-routing, and render tests continue to cover the harness picker
  on macOS.
- The failed `v0.1.9` build tag remains immutable and unpublished; no partial
  release assets were presented as installable.

## [0.1.9] - 2026-08-20

### Added

- The new-task composer now has a visible harness picker. `tab` opens every
  configured launch-capable harness; arrows or Tab preview with wraparound,
  `enter` confirms, `1`–`9` selects directly, and `esc` returns without losing
  the draft or changing the current harness.
- `/harness` opens the picker, while `/harness NAME` and the CLI `--harness
  NAME` select explicitly. `/provider` and `--launch-provider` remain
  compatibility aliases.

### Changed

- Composer titles say `harness` explicitly. Switching harnesses resets a
  selected model only after confirmation; previewing or cancelling is
  side-effect free.
- A real-PTY regression exercises palette visibility, arrow/Tab and numeric
  selection, cancellation, confirmation, model reset, draft preservation, and
  clean terminal restoration through the release binary.

## [0.1.8] - 2026-08-19

### Added

- The new-task composer names its selected provider and model. `tab` cycles
  launch-capable providers; `/provider`, `/model`, `/filter`, and `/help` are
  local dashboard commands, while `ctrl+f` exclusively edits the session
  filter and `ctrl+l` requests an immediate refresh.
- Claude attachment now presents the provider-native navigation contract before
  handoff: left arrow stays in Claude's agent view and Ctrl+Z returns to Open
  Agent View without stopping the background session.

### Fixed

- Discovery centrally enforces completed, interactive, and cwd filters even
  when a provider version ignores its own flags, preventing hidden completed
  histories from entering the TUI model.
- Pi native resume passes the exact recursive JSONL file's parent as
  `--session-dir`, fixing `No session found matching ...` for ordinary
  per-workspace stores.
- Stale Antigravity last-conversation cache entries whose workspaces were
  deleted are omitted; a stale injected row is refused before process spawn
  with a precise explanation.
- Ctrl+X is available for active host Claude background sessions and performs a
  fresh full-UUID/runtime/kind/state inventory check immediately before the
  confirmed `claude stop` call.

### Changed

- The automatic refresh default is 15 seconds to reduce repeated provider-CLI
  CPU/memory churn. Status aggregation and provider-label collection no longer
  repeatedly sort or scan full histories on every frame.
- Real-PTY stress coverage now includes slash-command handling and a
  200-character burst with 500 sessions, in addition to bounded 25-row paging
  and 200-arrow coalescing.

### Security

- Claude stopping never trusts a stale dashboard capability or a short-ID
  prefix alone. Interactive, completed, Docker, missing, and changed targets
  are refused after live provider revalidation.

## [0.1.7] - 2026-08-19

### Added

- `coding-agents sessions archive` previews bounded batches of exact OAV-owned
  completed Codex threads, with optional directory, age, and limit scopes.
  Literal `--yes` is required to execute; JSON reporting and per-thread failure
  details are available.

### Changed

- Completed sessions are excluded from both the dashboard and JSON by default.
  `--all` now explicitly opts either interface into completed-history
  discovery, and the TUI says `completed hidden` when they are excluded.
- OpenCode skips its global persisted-history database query entirely unless
  `--all` is present, avoiding the 70,000-row startup path seen in user data.

### Security

- Bulk archive selects only completed rows already carrying Archive authority,
  defaults to a read-only plan, caps each batch, sanitizes text output, and
  revalidates ownership and idle state immediately before every mutation.

## [0.1.6] - 2026-08-19

### Added

- Groups with more than 25 matching sessions now end in a keyboard-selectable
  `Show 25 more · N hidden` row. Enter reveals the next bounded page and moves
  selection to its first session.

### Changed

- TUI paging is presentation-only: counts, filtering, JSON output, and bulk
  safety checks continue to use the complete discovered session set.
- Applying a filter or switching views restores the 25-session bound, while
  ordinary provider refreshes preserve pages the user explicitly revealed.

## [0.1.5] - 2026-08-19

### Fixed

- Buffered key-repeat bursts are now applied before drawing instead of
  repainting hundreds of intermediate frames that can saturate SSH, tmux, and
  tall terminal panes.
- Long lists use page-aligned viewports, so moving within a page repaints only
  the previous and next selected rows rather than shifting every visible row
  on every arrow press.
- Initial discovery publishes each completed provider incrementally, so a slow
  provider with a large history no longer hides sessions already returned by
  faster providers.

### Changed

- The real-PTY suite includes a 500-session stress case that queues 200 arrow
  events, requires the final destination within 750 ms, and caps emitted
  terminal output at 24 KiB.
- A separate real-PTY startup case holds Claude discovery for two seconds and
  requires the fast Antigravity result to render within 750 ms.

## [0.1.4] - 2026-08-18

### Fixed

- Every session row now spells out its provider name—including Claude, Codex,
  Pi, OpenCode, Cursor, GitHub Copilot, and Antigravity—instead of requiring
  users to decode undocumented `C@H`/`X@D`-style markers.
- Provider identity remains visible at narrow terminal widths by yielding task
  name and summary space first; full host/container details remain in Peek.

### Changed

- Real-render and real-PTY coverage now proves that each fixture session and
  its provider name appear on the same terminal row.

## [0.1.3] - 2026-08-18

### Fixed

- Provider discovery and controller enrichment now run on a dedicated refresh
  worker, so slow provider CLIs cannot block drawing, arrow navigation, help,
  filtering, or dashboard exit.
- Arrow-key repeat events are handled as navigation instead of being discarded.
- Unchanged frames no longer repaint on every input-poll tick, and dashboard
  exit cancels in-flight discovery process groups instead of leaving temporary
  provider children behind.

### Changed

- The default provider refresh interval is five seconds instead of 1.5 seconds,
  substantially reducing provider-process churn while retaining a configurable
  `--refresh-ms` override.
- The real-PTY suite now includes a live regression in which Claude discovery
  deliberately stalls for two seconds while exact selected-row arrow repaints
  and exit must each complete within 750 milliseconds; the same test rejects
  idle terminal output when no frame changed.

## [0.1.2] - 2026-08-18

The `v0.1.0` and `v0.1.1` build tags did not publish GitHub releases because
their native gates exposed a macOS portability error and a terminal-test
repaint race, respectively. Neither tag was moved or reused.

### Added

- Provider-neutral Ratatui dashboard with Claude-style state grouping,
  navigation, details/peek, filtering, contextual help, composers,
  confirmations, directory grouping, responsive layouts, and JSON output.
- Host Claude discovery, background launch, native attach, reconstructed log
  inspection, and exact ownership-gated stop.
- Durable host Codex App Server discovery and managed launch, native resume,
  bounded transcript inspection, reply/steer, exact-turn interrupt, archive,
  and delete.
- Reconnectable Codex server-request handling for one-shot command decisions,
  safe denials, and sequential non-secret structured answers, with an exclusive
  controller lease and exact request/thread/turn checks.
- Concurrent host discovery for Pi, OpenCode, Cursor, GitHub Copilot CLI, and
  Antigravity CLI, with provider-specific read-only/native-open boundaries.
- Durable Linux Pi supervision for OAV-owned launch, reconnect, inspect,
  reply/steer, exact dialog response, and interrupt.
- Durable Linux OpenCode supervision through an authenticated loopback server
  for OAV-owned launch, reconnect, discovery, inspection, reply, and interrupt;
  external CLI history remains inspect/native-open only.
- Managed Linux Cursor launch, rediscovery, bounded inspection, idle reply, and
  verified-process interrupt for OAV-owned runs; external/global listing stays
  unavailable because Cursor exposes only a TTY picker.
- Process-local Copilot ACP launch, prompt/reply, bounded inspection, cancel,
  and exact one-shot permission response; persisted `session/list` rows remain
  observe/native-open only.
- Explicit observe-only Docker discovery and a separately protected managed
  container create/start/status/list/stop/remove CLI.
- Private ownership registries, immutable target validation, diagnostics,
  deterministic mocks/fixtures, minimum-Rust CI, a checksum-verifying binary
  installer, and prepared four-target tag-gated release automation.
- Installation, CLI, recovery, security, contribution, exploration, and
  reproducible real-TTY/fresh-container validation documentation.

### Security

- Container lifecycle requires matching immutable ID, random instance label,
  and private external owner record; labels alone do not confer authority.
- Managed Codex control verifies persisted Linux process identity and never
  signals a PID loaded directly from disk.
- Managed Pi and Cursor control verifies private ownership state and exact
  Linux process identity; unrelated provider records never gain authority.
- Managed OpenCode control requires its private `0600` record, authenticated
  health check, and exact Linux process/listener identity before reconnect or
  mutation; external history IDs never confer authority.
- Copilot control is restricted to the retained ACP connection and exact
  pending session/request/option IDs; no persisted list result is adopted.
- Expanded or incompletely rendered provider requests remain native-only; no
  request is answered automatically.
- Dynamic provider/runtime/session/summary/group/warning/notice/confirmation
  text is terminal-control sanitized, display-width bounded, and tested with
  wide graphemes.

### Known limitations

- Version 0.1.2 is a private preview rather than a supported public stable
  release or package-registry distribution.
- No authenticated Claude/Codex lifecycle has been recorded inside the fresh
  credential-free Docker TUI probes.
- Durable Pi, OpenCode, and Cursor managed control require Linux. External
  OpenCode history stays inspect/native-open and has no inline approval/input;
  Cursor has no supported external/global list, and Copilot managed authority
  ends with the dashboard's retained ACP connection.
- File-change acceptance, permission grants, MCP form/URL acceptance, secret
  input, Codex supervisor stop/status/log rotation, and managed-container
  session launch/control remain incomplete.
- SSH and broader terminal/theme/platform validation remain release gates.

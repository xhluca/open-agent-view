# Changelog

All notable changes will be documented here. Open Agent View remains an early
private preview and may change before a public stable release.

The format follows [Keep a Changelog](https://keepachangelog.com/en/1.1.0/),
and future released versions are intended to follow Semantic Versioning.

## [Unreleased]

## [0.1.30] - 2026-08-25

### Fixed

- OAV-owned Claude rows now derive a bounded current description from Claude's
  supported `logs` terminal stream. The normalizer prefers Claude's concise
  recap and falls back to the latest assistant paragraph instead of leaving
  every `status: busy` row blank.
- Claude description probes are bounded, concurrent, and cached between
  refreshes; completed rows reuse their last result instead of spawning work
  indefinitely.

### Tests

- Reproduced the blank row against Claude Code 2.1.241 and the reporting
  account's exact managed `open-agent-view-dev` task. A read-only end-to-end
  JSON probe now returns its current 210-character recap with no warnings.
- Added VT-screen regressions for recap extraction, assistant-message fallback,
  terminal-chrome exclusion, exact owned-session enrichment, and cache reuse.

## [0.1.29] - 2026-08-25

### Changed

- Antigravity launches now always request a distinct `--new-project`, show a
  provisional managed row immediately, and correlate the exact live
  conversation from its bounded local JSONL transcript rather than waiting for
  the workspace cache to update on exit.
- OAV-owned Antigravity rows retain every exact owned conversation and derive
  their current summary and timestamp from the bounded transcript.
- Successful Antigravity account model catalogs are cached privately for 24
  hours, with a bounded seven-day last-known-good fallback and a shorter live
  request timeout.

### Tests

- Added exact live-transcript correlation, provisional-row, private model-cache,
  and foreground PTY regressions.
- Revalidated Antigravity 1.1.20 with the reporting account's existing login in
  isolated OAV state: 14 models, exact selected-model response, immediate
  Working row after Shift+Left, exact Ctrl+X stop, and fresh-process rediscovery.
  No authentication material was read or copied.

## [0.1.28] - 2026-08-25

### Changed

- Every dashboard launch now enters a provider-native foreground interface:
  Claude, Codex, OpenCode, and Cursor safely bootstrap an exact managed ID and
  auto-attach; Pi, Copilot, Antigravity, and Terminal start native-first.
- Pi foreground launch uses a caller-generated UUID in OAV's managed session
  directory, avoiding an unnecessary background RPC handoff.
- Copilot foreground launch uses its documented `--session-id` and
  `--interactive` options, preserving the selected account model without broad
  permission flags.

### Fixed

- Copilot 1.0.80 installations that omit optional ACP `session/close` can now
  open an idle connection-owned row. OAV drops only its retained ACP client,
  refuses while another task on it is active, and never overlaps native and
  ACP ownership.
- Provider-native foreground launch no longer panics when a newly allocated
  container PTY initially reports a `0x0` terminal size; missing dimensions
  use a safe `24x80` fallback until the terminal reports its real size.

### Tests

- Reproduced the reporting account's exact Copilot 1.0.80 capability set
  (`loadSession` and `session/list`, no `session/close`). Added protocol tests
  for that handoff plus real-PTY foreground launch coverage for Copilot and Pi,
  and a fresh read-only Docker regression for an initially unsized PTY.

## [0.1.27] - 2026-08-24

### Added

- Working rows now blink between the live star and a quiet dot on a bounded
  timer without rebuilding session indexes.

### Changed

- Native plain Left/Right remains editor input. When the provider cursor does
  not move, a 1.6-second bottom-line hint lets the same arrow return to OAV;
  Shift+Left and Shift+Right return immediately.

### Tests

- Added process-level PTY coverage for forwarded editing, both timed arrow
  gestures, Shift+Right, exact reattachment, and real-terminal live animation.

## [0.1.26] - 2026-08-24

### Changed

- Plain Left/Right arrows are forwarded to provider-native editors. Shift+Left
  backgrounds the retained frontend and returns to Open Agent View.
- Cursor launches now allocate the exact chat and open its interactive native
  interface in the foreground instead of returning after a print-mode spawn.

### Fixed

- Completed connection-owned Copilot sessions now use the advertised ACP
  close operation before native resume and reload onto the ACP connection after
  the native client exits, instead of refusing every open as concurrent.
- Managed OpenCode and Codex rows derive their summary and recency from current
  assistant messages/provider timestamps rather than keeping the first prompt.

### Tests

- Added exact Shift+Left byte-forwarding, foreground Cursor allocation, Copilot
  close/resume/reload, and real-account Codex/OpenCode freshness regressions.

## [0.1.25] - 2026-08-24

### Fixed

- `opav update` and `opav upgrade` now finish with the verified version
  transition (`Updated Open Agent View from X to Y.`), or explicitly report
  that the installed version is already current.

## [0.1.24] - 2026-08-23

### Added

- Terminal is an eighth launch target for OAV-owned process-local interactive
  shells. Left backgrounds, Enter/Right resumes the exact screen, and two
  Ctrl+X actions stop then delete the row.
- `/setup [HARNESS]` opens the install/login wizard in an isolated setup
  terminal. Backgrounded provider logins appear as Terminal jobs instead of
  attaching to whichever agent frontend was last active.
- Option+Backspace/Ctrl+W delete the previous word and
  Cmd+Backspace/Ctrl+U delete to the current line start in every editable field.

### Fixed

- Antigravity catalog errors are wrapped inside the model picker, retain native
  login/retry actions, and no longer promote search text to an unverified model
  ID. The reporting account reproduced the upstream 1.1.19 catalog timeout and
  empty native `/model` result.
- Managed Pi rows keep newer durable JSONL summaries and modification times
  instead of reverting to a stopped supervisor's first-message preview.
- `/help` opens the full contextual panel, normal local aliases no longer emit
  a clipped warning, and relative ages repaint on every completed refresh.

### Tests

- Added an actual-binary PTY lifecycle for Terminal and expanded isolated
  Cursor/Copilot/Antigravity login recovery, model retry, macOS edit-key,
  eight-provider fixture, and Pi freshness coverage.

## [0.1.23] - 2026-08-21

### Changed

- Rename mode is visually distinct from ordinary task input: its cyan title,
  border, and bold `name ❯` label precede the editable name, while the footer
  explicitly explains how to save, reset, or cancel.

## [0.1.22] - 2026-08-21

### Added

- `open-agent-view -v`/`-V` now match `--version`; `opav update` and its
  `upgrade` alias fetch the repository installer and retain release checksum
  verification.
- Catalog errors can accept an explicitly typed exact model ID, while native
  sign-in remains available through Enter or `l`.

### Fixed

- Claude launch follows the current `--bg` contract: Claude allocates the ID,
  OAV resolves the exact full UUID, records it, refreshes, selects it, and opens
  full-screen attach. It no longer supplies the ignored `--session-id`, leaves
  the user on a stale Pi row, or blocks the input thread during bootstrap.
- Direct Cursor and Copilot authentication failures now preserve the task and
  open an actionable native login modal instead of leaving a passive footer.
- Copilot model discovery retries the same bounded Linux executable-replacement
  race already handled by its ACP transport instead of surfacing `ETXTBSY`.
- Antigravity model-less launches are refused before starting `agy`, preventing
  the observed `neither PlanModel nor RequestedModel specified` termination.

### Tests

- Added exact Claude `backgrounded · ID` parser/argv tests, isolated updater
  and version-alias tests, auth/draft/custom-model state tests, and real-PTY
  launch-to-login coverage for Cursor, Copilot, Claude, and Antigravity.

## [0.1.21] - 2026-08-21

### Added

- `ctrl+r` now stores a private session display name keyed by the stable
  normalized ID. `sessions rename`, `sessions aliases`, and `sessions
  reset-name` expose the same non-provider-mutating layer to scripts. A local
  name wins over native provider title changes until it is explicitly cleared.
- The canonical installed command is now `open-agent-view`, with `opav` as the
  short alias. The installer retains `coding-agents` as a compatibility alias
  and refuses to replace an unrelated existing `opav` command.

### Security

- Session aliases use an atomic 0600 registry under the existing 0700 state
  root, reject control characters, oversized values, symlinks, wrong owners,
  and permissive modes, and never grant provider control authority.

### Tests

- Added unit, CLI, installer, and real-PTY coverage for alias precedence/reset,
  cross-process reload, file modes, canonical/shorthand/legacy commands, and
  unrelated-command collision refusal.

## [0.1.20] - 2026-08-20

### Added

- Cursor, GitHub Copilot, and Antigravity now expose searchable exact
  account-model catalogs. Cursor passes the selected model to managed runs;
  Copilot applies it through ACP before the first prompt; Antigravity passes it
  to a sandboxed native launch.
- Model-catalog authentication failures now offer an in-place native login
  handoff. Enter or `l` suspends OAV, runs the provider login, restores the
  dashboard, and reloads the catalog. `/login` exposes the same setup route for
  Claude, Codex, Pi, OpenCode, Cursor, Copilot, and Antigravity.
- `coding-agents setup HARNESS` installs any of the seven supported provider
  CLIs with its official user-local installer, explicit confirmation, native
  progress, and private staging for downloaded scripts.
- Antigravity is now a launch target. OAV records exact conversations it starts,
  rediscovers the documented last conversation for that workspace, and supports
  first-run login, model selection, full-screen sandboxed launch, Left
  backgrounding, and native resume.

### Fixed

- New Claude tasks now start with an exact background UUID and immediately open
  full-screen `claude attach`; Left returns to the exact new dashboard row. This
  replaces the silent background-only path that could appear frozen and never
  surface a usable native view.
- Background launch workers display an animated progress indicator without
  blocking terminal input. Copilot authentication errors are reduced to a short
  actionable picker message instead of raw protocol JSON.

### Security

- Antigravity launch always includes `--sandbox` and never adds its dangerous
  permission-bypass flag. Its ownership registry is private, atomic, and refuses
  symlinked or group/other-readable authority state.

### Tests

- Added real-PTY signed-out-to-signed-in model flows for Cursor and Copilot,
  exact Cursor model propagation with a deliberately slow launch, foreground
  Claude attach/Left/row selection, and Antigravity login/model/sandbox/Left
  lifecycle. An isolated installer test proves confirmation precedes download,
  progress remains visible, and staged scripts are removed.

## [0.1.19] - 2026-08-20

### Changed

- Completed OAV-managed sessions are visible by default. `--hide-completed`
  (alias `--active-only`) provides an explicit active-only startup, while
  `--all` remains accepted for compatibility.

### Performance

- Default-visible completed queues retain cached grouping/index lookups,
  terminal-sized pages capped at 25 rows, background discovery, bounded
  persisted history, and coalesced key bursts. A real PTY now holds 1,000
  completed rows and verifies startup plus a 208-arrow burst within 750 ms and
  under 24 KiB of terminal output.

## [0.1.18] - 2026-08-20

### Fixed

- Provider-native sessions now run behind a private PTY. Left backgrounds the
  exact frontend and returns to OAV without stopping the managed backend;
  Enter/Right resumes it and restores its retained terminal screen. Fresh
  opens clear the physical screen first, so Codex no longer appends below prior
  shell contents.
- Managed OpenCode native open attaches to the exact authenticated loopback
  server/session. Older records containing bare `opencode` reconnect when the
  configured path resolves to that same verified executable.
- Cursor checks its read-only model catalog before `create-chat`, turning an
  unauthenticated/no-model account into a prompt `cursor-agent login` message
  instead of a 15-second apparent hang.
- Copilot managed-launch authentication errors now give direct `copilot login`
  and `gh` recovery instructions without dumping the ACP response payload or
  adding another credential lookup to each refresh.

### Tests

- Added a nested real-PTY detach/reattach/screen-restore regression, canonical
  OpenCode executable identity coverage, no-create Cursor preflight coverage,
  and fresh network-disabled Rust 1.75/Copilot container reproductions.

## [0.1.17] - 2026-08-20

### Fixed

- Completed Pi native handoff now tolerates an older verified supervisor
  removing its Unix socket just before its process exits, instead of reporting
  a one-time transport error during the safe upgrade.
- Completed-but-attached managed Pi rows now label Ctrl+X as Stop in both the
  footer and help. Delete is advertised only after refresh verifies that the
  owned RPC transport has exited.

## [0.1.16] - 2026-08-20

### Fixed

- Enter/Right can now hand a completed OAV-managed Pi conversation from its
  idle RPC process to Pi's full native interface. Active turns and pending
  questions remain protected from an implicit stop.
- Ctrl+X no longer waits on Pi's turn-abort RPC. The first press closes the
  exact owned RPC transport; after refresh confirms exit, the second press
  deletes only its exact OAV-owned JSONL history. Persisted OAV-managed Pi
  history remains in the default owned view after a supervisor restart.
- A verified older Pi supervisor is migrated safely: per-session stop falls
  back to daemon shutdown only when no other active Pi work would be affected.
  The request returns immediately, while native handoff separately waits for
  the verified transport to exit.

## [0.1.15] - 2026-08-20

### Fixed

- Enter and Right now always suspend the dashboard and open the selected
  provider's full native interface. Managed Pi, Codex, OpenCode, Cursor, and
  Copilot action capabilities no longer divert those keys into the inline
  panel; Space is the sole session-list key for Peek.
- The ignored read-only Pi native-open PTY probe now requests completed history
  explicitly, so it exercises the same completed managed session shown in the
  dashboard instead of waiting behind the default completed-session filter.

## [0.1.14] - 2026-08-20

### Fixed

- Existing Pi supervisors created with a bare `pi` executable now reconnect
  when the configured executable resolves to that same canonical file. A
  genuinely different executable remains a hard refusal, and no live Pi work
  is stopped during the migration.
- Claude background launch no longer kills a slow provider bootstrap after 15
  seconds. OAV preassigns an exact session ID, records it before starting a
  detached provider process, and reaps the launcher only after it exits.
- Exact OAV-owned Codex App Server threads are treated as managed background
  tasks even when Codex labels their source `cli`, so the default foreground
  filter cannot hide a newly launched thread.
- Post-launch refresh temporarily queries completed and interactive results
  until the exact returned provider ID appears, reveals its group/page, and
  selects it. A task that finishes before the first refresh explicitly switches
  the dashboard to completed visibility instead of disappearing.

## [0.1.13] - 2026-08-20

### Added

- `--include-external` explicitly opts the dashboard or JSON output into
  provider sessions that Open Agent View did not create or manage.

### Changed

- Default discovery is ownership-scoped. Claude is filtered through the exact
  launch registry; Codex, Pi, OpenCode, Cursor, and Copilot use their managed
  inventories; Antigravity appears only in external-history mode. `/completed
  show` and `--all` reveal completed owned work without querying unrelated
  provider history.
- Enter and Right open the selected session. Left returns from inline Peek;
  Claude attaches directly and the footer identifies Ctrl+Z as the return key.
- Ctrl+X is lifecycle-based for an exact selected row: an active owned session
  is stopped on the first press, and after refresh reports it idle the next
  press deletes it where provider deletion exists. Safe local hiding and bulk
  deletion remain capability-distinct; idle local removal is immediate and
  reversible, while hiding an active uninterruptible row remains confirmed.

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

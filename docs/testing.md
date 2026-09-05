# Validation record

This file records the checks used for the initial `open-agent-view` prototype.
They are intentionally split between deterministic tests and disposable runtime
probes. Existing user containers and live agent sessions were never used as
test targets for lifecycle operations.

For the reusable Linux, macOS, and Windows acceptance matrix, native-versus-
simulated test boundaries, clean public-install commands, and release artifact
provenance checks, use the
[cross-platform testing guide](cross-platform-testing.md). This file is the
dated evidence log; that guide is the procedure.

The reproducible commands, synthetic populated fixture, exhaustive key route,
visual acceptance criteria, and evidence template live in
[the real-TTY validation guide](tui-validation.md). This file records completed
checks; the guide also contains release gates that are not yet complete.

## v0.1.53 18-harness parity gate (2026-09-03–04)

- The final feature commit `4fffb203ec8685ebe9d500d3bb5f3bcce85447c3`
  passed all ten jobs in [PR CI run 33829276786](https://github.com/xhluca/open-agent-view/actions/runs/33829276786),
  including native Linux x86-64/ARM64, macOS Intel/Apple silicon, Windows
  x64, both Rust versions, quality, native-client corpus, and website gates.
- Published `v0.1.53` from release commit
  `a8614e15f8dd7fd36135ffc68441b006f819ac6e`, after all ten jobs in
  [release CI run 33938216092](https://github.com/xhluca/open-agent-view/actions/runs/33938216092)
  passed. All five archives and adjacent checksums were downloaded from that
  run, checked locally, and uploaded manually. The immutable annotated tag
  peels to that exact SHA; GitHub's published archive digests match locally.
- On the versioned commit, the complete local locked suite and installer
  script passed again. The separately serialized real-TTY run passed all 25
  default scenarios; credential-gated probes remained explicitly ignored.
- Public installer downloads passed in a fresh Debian-based Linux x86-64
  container, native Apple-silicon macOS, and Intel mode under Rosetta. Each
  checked `open-agent-view`, `oav`, legacy `opav`, and empty provider-free JSON
  startup using isolated state. A fresh Ubuntu bootstrap was stopped after
  its package manager stalled before the OAV installer; Ubuntu 22.04 binary
  and installer acceptance is covered by the successful native CI job.
- Linux ARM64, native Intel macOS, and Windows x64 passed native build/test/
  packaging/installer CI. Separate post-publication downloads were not run
  on native ARM Linux or Windows; Rosetta is supplemental to Intel CI.
- Website publication passed audit (zero vulnerabilities), lint, five
  rendered-page tests, 21 browser tests, and export. Pages commit
  `500c17170e49a2e1dd9197e251e6548564a6c6aa` deployed successfully; the public
  page was checked for the 18-harness count and all three added names.

- Added Hermes Agent, MastraCode, and Devin, matching Session Migrate's 18
  coding harnesses; Terminal is counted separately. Product, README, website,
  migration picker, and doctor inventory checks share that count.
- The complete locked suite passed: 410 library tests, 19 CLI tests, all
  default integration suites, and 25 deterministic real-TTY scenarios. The
  opt-in native-corpus test also passed on actual sanitized client databases
  for Hermes 0.20.6, MastraCode 0.37.1, and Devin 3000.6.7.
- Actual Hermes 0.20.6 and MastraCode 0.37.1 TUIs each completed three
  credential-free loopback-model turns through OAV: foreground launch,
  detach/reattach, refreshed dashboard text, OAV restart, exact native resume,
  and continued conversation. The native runs caught and fixed premature
  Hermes input and MastraCode's extra empty startup thread.
- A mixed-provider PTY regression launches all three in one workspace, renames
  them, resumes them, checks their latest previews, stops each exact frontend,
  and hides each row locally while retaining its database. Installer fixtures
  cover missing/existing binaries, consent, failure, and native setup handoff
  for all 18 coding harnesses.
- Fresh Debian containers installed Hermes 0.21.0, MastraCode 0.38.0, and
  Devin 3000.6.14 and reached native setup/login. The Hermes test's login wait
  was terminated after the setup menu appeared, then version/help checks
  passed. No Devin account authentication or model reply is claimed by these
  installer checks. Hermes setup skips the installer's wizard and hands off
  to native setup once from OAV.
- Rust 1.75 all-target Clippy passed in fresh Docker. Website build and all
  five rendered-page tests, lint, and 21 browser tests passed (desktop,
  Mac-laptop-sized Chrome, phone, keyboard, playback, accessibility). Native
  macOS/Windows execution remains the
  required hosted portability gate, not something Linux Docker proves.

Reproduction and limitations:
[shared-SQLite harness guide](exploration/shared-sqlite-harnesses.md).

## Unreleased explicit-YOLO gate (2026-09-03)

- Rust 1.75 Rustfmt and warning-free Clippy passed in the documented read-only
  Docker quality environment.
- The complete locked suite passed: 399 library tests, 19 CLI tests, every
  default integration suite, and 24 deterministic real-TTY scenarios. Four
  credential-gated host probes remained explicitly ignored.
- Exact argv tests cover the verified native mappings for Claude Code, Codex,
  Cursor, Antigravity, Mistral Vibe, Muse Code, Qwen Code, Kimi Code, Oh My Pi,
  Grok, Kilo Code, and OpenHands. Safe launches are checked separately so the
  flags cannot leak into the default path.
- A hub-level regression rejects an unsupported harness before provider
  dispatch. Renderer and parser tests prove that the option is explicit for
  both `open-agent-view` and `oav`, unsupported picker rows are marked, and the
  dangerous mode stays visible in the dashboard and composer.
- A real outer PTY starts OAV with `--yolo`, selects Antigravity and an exact
  model, verifies the native argument and absence of the mutually exclusive
  sandbox flag, sees the warning in the provider screen, backgrounds it, and
  stops the exact retained session from the still-marked dashboard.

## v0.1.50 migration and Codex supervision gate (2026-08-31)

- The complete pull-request matrix passed 18 hosted checks: Rust 1.75 and
  stable, Linux x86-64 and ARM64, macOS Intel and Apple silicon, native Windows
  x64, and the website lint/build/visual suite.
- `cargo test --all-targets` passed 390 library tests and every default
  integration suite. The real-terminal suite passed 23 tests, including the
  complete `Ctrl+M` picker, naming, migration, persistence, and reopen flow.
- A focused regression starts a real mock Codex App Server beneath an
  intentionally overlong state path, verifies the private short socket, and
  reconnects a second supervisor to the same verified process.
- A disposable authenticated Codex-to-Pi run preserved the code word
  `LANTERN`, reopened the migrated conversation in Pi's native TUI, and left
  both source and destination rows visible. The published cast contains no
  Codex socket-timeout warning or provider credential material.

## v0.1.49 shorthand transition gate (2026-08-29)

- The complete locked Rust suite passed with 371 library tests, all integration
  suites, 22 deterministic real-TTY scenarios, and only the explicitly
  credential-gated live-provider probes ignored.
- The Unix installer test packages the current version for every advertised
  Unix target, checksum-verifies it, and proves `open-agent-view`, `oav`, and
  the legacy `opav` compatibility alias report the same version. It also proves
  unrelated commands at both shorthand paths are not replaced and that normal
  installer output does not advertise the legacy spelling.
- The README metadata gate requires the current release badge and `oav` copy,
  and rejects the legacy name from the primary README. The real setup recorder
  likewise refuses any installed version that does not match the package under
  release before it can replace the public demo.

## v0.1.48 native Windows x64 gate (2026-08-28)

- The complete locked Rust suite is compiled and executed on a native
  `windows-latest` runner, not under Wine. Windows-specific coverage includes
  Win32 atomic state-file replacement, `USERPROFILE` operation without
  `HOME`, Windows worktree paths, POSIX Docker container paths from a Windows
  host, operating-system random IDs, and the PowerShell/Command Prompt shell
  catalog.
- The same runner builds `x86_64-pc-windows-msvc`, starts the release binary in
  provider-free JSON mode, packages the `.exe` into the documented ZIP, and
  exercises the PowerShell installer against that local release. The installer
  test verifies both command names, checksum rejection without replacing a
  working installation, and waiting for a running process before an update
  replaces Windows executables.
- A separate disposable Linux Docker environment installs MinGW and the Rust
  Windows GNU standard library, then compiles the library and every test target
  for `x86_64-pc-windows-gnu`. This is a supplementary portability check; the
  native MSVC runner is the release acceptance gate.
- Native Windows intentionally uses foreground provider handoff. Durable
  Unix-socket supervision and PTY background/resume gestures remain available
  through WSL 2 until a ConPTY implementation can preserve the same ownership
  and return guarantees.

## v0.1.47 macOS Codex supervision gate (2026-08-28)

- A native Apple-silicon test starts a disposable Codex App Server on a private
  Unix socket, resolves its exact listener PID, records its native process-start
  token and full argv, completes a real `model/list` RPC, and reconnects a
  second supervisor to that same verified process. The test also checks the
  `0700` state directory and uses an identity-rechecking cleanup guard.
- Rustfmt, warning-free Clippy, and the complete locked suite passed on the real
  Apple-silicon `mbp`. The same complete suite passed for the
  `x86_64-apple-darwin` build under Rosetta, including the macOS Codex test and
  real-PTY interaction coverage.
- The native ARM and Intel release archives were built from the same tree,
  checksum-verified, and installed into separate empty Mac homes with the real
  Bash 3.2 installer. Both `open-agent-view` and `opav` reported 0.1.47. The
  native installed binary also initialized with an explicit Codex executable,
  proving the previous startup-fatal Linux-only identity guard is gone.
- The complete locked Linux suite passed. The packaged Linux x86-64 archive was
  checksum-verified and installed into an empty home in the pinned Debian 12
  image; both command names reported 0.1.47 and an isolated provider-free JSON
  snapshot returned no sessions or warnings.
- Installer tests put a deliberately failing `gh` executable first on `PATH`
  and still complete the public latest-release flow, proving that public
  installation no longer invokes or requires GitHub CLI authentication.

## v0.1.46 cross-platform installer gate (2026-08-28)

- Linux x86-64: Rust 1.75 ran the complete locked test suite and produced the
  release archive with `scripts/package-release.sh`. The verified archive was
  mounted read-only into a fresh
  `debian:12-slim@sha256:abd67ffcfa541b485a3dff59865ab629aa048a6c613e639d36e7456b0b229241`
  container. From an empty home, the real installer checksum-verified the
  archive, installed both commands, reported `open-agent-view 0.1.46` through
  `open-agent-view` and `opav`, and returned empty `sessions` and `warnings`
  through `--json --no-host-providers`.
- macOS Apple silicon: the isolated `mbp` build used Rust 1.75 and the system
  Apple clang toolchain. Rustfmt, warning-free Clippy, the complete macOS test
  suite (including real PTYs), and the release build passed. The real Bash 3.2
  installer test suite then passed without GNU coreutils. From an empty home,
  the packaged `aarch64-apple-darwin` archive checksum-verified, installed both
  commands, reported 0.1.46, and returned an empty JSON snapshot.
- macOS Intel: the same reviewed tree was built for
  `x86_64-apple-darwin`. The exact packaged binary executed through Rosetta,
  and the installer was forced through the Intel `uname` path so archive
  selection, checksum verification, extraction, atomic installation, the
  `opav` symlink, version output, and empty JSON smoke test all exercised the
  Intel artifact rather than the native ARM binary. The native Intel CI runner
  independently builds and tests the same commit.
- `scripts/test-installer.sh` now packages the current crate version for every
  target mapping. It also keeps a dedicated v0.1.45 regression proving legacy
  Linux-only releases still fail clearly on macOS instead of requesting a
  nonexistent or incompatible artifact.

## Automated checks

- `cargo test --locked`: domain normalization, provider parsing, App Server
  JSONL and Unix-WebSocket transport behavior, navigation, responsive
  rendering, process timeouts, ownership persistence, Claude launch parsing,
  VT100 log reconstruction, and doctor output.
- A disposable mock App Server test launches an owned Codex thread, drops the
  first supervisor, reconnects from a second supervisor through the same Unix
  socket, discovers the still-active thread, interrupts its exact turn, and
  rejects an external thread ID. The same test covers bounded transcript read,
  exact active-turn steer, idle reply, archive, and delete. It also asserts
  mode `0700` for the state directory and `0600` for the ownership record. The
  mock PID is re-verified, terminated, and reaped by a panic-safe test guard.
- Codex reducer tests require an exact owned `turn/completed` thread/turn pair
  before releasing active-turn authority, reject external deletion events, and
  prove a stale idle snapshot remains interruptible. Shutdown tests hold a
  stable Linux pidfd, reject a second controller and tampered command line, and
  remove only the exact stopped server record. Process transports run in a new
  process group so npm wrappers and their native App Server children are both
  bounded during teardown.
- A disposable mock Pi RPC executable covers managed launch, live discovery,
  transcript inspection, active steer, confirmation, structured text input,
  nonblocking exact stop, stopped-session delete, completed native handoff,
  exact unowned-ID refusal, modeled launch, and shutdown. A second
  dashboard client reconnects through the same verified daemon while its Pi
  child remains live. Compatibility tests prove an old daemon advertises no
  model feature, refuses replacement while it owns active work, and can be
  safely replaced only after exact owned sessions are completed. Panic-safe
  cleanup stops the exact test daemon; separate tests reject symlinked state
  and permissive/replaced authority records.
- A disposable authenticated OpenCode loopback fixture covers durable managed
  launch, dashboard reconnect, discovery, transcript inspection, active/idle
  reply, interrupt, exact unowned-ID refusal, and panic-safe pidfd shutdown. It
  verifies the private state modes plus exact Linux process and listener
  identity. A separate credential-empty real OpenCode server probe covers
  create, accepted async prompt, list, inspect, and exact shutdown without
  claiming a model-backed turn.
- Disposable provider fixtures cover the additional host adapters without
  touching user credentials or session stores. Cursor tests own a temporary
  executable, workspace, registry, logs, and child process while exercising
  launch/discovery/inspect/interrupt/reply and stale-identity refusal. Copilot
  tests own an ACP subprocess and exercise list pagination plus the retained
  new/prompt/update/permission/completion/reply/cancel lifecycle; a separate
  load fixture proves external list results remain unowned. Copilot restart
  coverage also proves private exact-ID persistence, split user/assistant
  history replay, real latest-message summaries, provider timestamps, and
  visibility when the second process cannot reach ACP. Antigravity tests
  read only a temporary documented cache and build shell-free native commands.
- Mistral Vibe and Qwen Code have public-controller integration tests backed by
  isolated executable app-server/CLI fixtures. They exercise model selection,
  native launch, exact ownership persistence, owned-only discovery, native
  resume, and unowned interrupt refusal through the same public traits used by
  the dashboard. An outer real PTY additionally drives foreground launch,
  native background, exact reattach, verified interrupt, and fresh exact resume
  for both providers. Vibe also covers bounded delayed correlation and
  ambiguity refusal; Qwen covers immediate launch-failure rollback.
  Qwen adapter tests also cover fractional millisecond JSONL timestamps and
  ownership persisted through an independently loaded private-registry handle.
- `tests/managed_cursor_copilot.rs` drives the public `ProviderController`
  surface rather than private supervisor helpers. It covers managed launch,
  enrich, inspect, reply, interrupt, exact one-shot approval, duplicate
  permission cancellation, and controller-side refusal of injected unowned
  records. Copilot runs on Unix CI; the Cursor process lifecycle is Linux-only.
- Protocol tests interleave client responses with string- and numeric-ID server
  requests and assert the exact `{id,result}` response shape. Reconnect tests
  verify that `thread/resume` replays an unresolved approval with the same ID,
  a decision remains resolving until `serverRequest/resolved`, and a second
  dashboard cannot acquire the process-held controller lease.
- Model-catalog tests cover Claude's installed-help alias parser, cursor-paged
  visible Codex `model/list`, Pi's bounded offline table, OpenCode's exact
  `provider/model` output, Cursor's terminal-progress output, Copilot's
  short-lived headless `models.list`, and Antigravity's model table. Managed Pi,
  OpenCode, Cursor, Copilot, and Antigravity assertions prove the selected identifier reaches the provider-native launch
  surface; Shift+Tab preserves the composer draft, catalog workers ignore
  stale-provider results, and picker search stays separate from task input.
- Request-reducer/UI tests reject unowned or wrong-turn requests, incomplete
  command context, duplicate/malformed structured questions, stale deadlines,
  and blind file acceptance. Sequential input tests verify exact option
  normalization and confirm that answers never enter the supervisor record.
- Reference-fidelity tests cover initial row focus, cyclic header/row
  navigation, direct escape-to-quit, printable-to-compose behavior,
  context-sensitive `?`, `ctrl+f` filtering, task slash commands, the visible
  harness palette, searchable/paged model picker, live completed toggle, direct
  idle local removal plus active-hide confirmation, direct Enter/Right open, Left-from-Peek return, Claude
  attach-return guidance, selection reconciliation
  after filtering, Claude
  worktree grouping, aggregate review/working counts, capability-aware help,
  and the narrow footer's retained help affordance. Focused rendering tests
  sanitize terminal-control characters from every provider-derived or dynamic
  surface and use terminal display width for CJK/grapheme-aware row truncation,
  padding, and editable cursor placement. The composer render test asserts the
  cursor immediately follows its typed cells with no phantom left-border
  column. Editable fields also cover Unicode-safe Option+Backspace/Ctrl+W word
  deletion and Cmd+Backspace/Ctrl+U line deletion.
- Safety-focused state tests verify that ready-for-review and needs-input
  sessions are treated as live (and therefore require interrupt authority),
  Ctrl+X emits exact stop for an active owned row and exact delete only after
  the refreshed row is idle, completed sessions use delete language, active groups cannot enter a
  bulk-delete confirmation path, and observe-only rows use reversible local
  hiding without mixing it with provider deletion.
- Private hidden-session registry tests cover idempotent hide/unhide, atomic
  `0600` persistence in a `0700` directory, symlink/wrong-mode/control-character
  refusal, and stable-order filtering of a 70,000-session snapshot. A separate
  70,000-row application regression proves 2,000 navigation/draw queries reuse
  one grouping cache and only a filter change rebuilds it.
- Terminal-loop tests prove model loads and completed-visibility changes return
  nonblocking effects, a deliberately slow managed launch does not block typed
  input, and post-launch selection requires both the exact provider and exact
  provider session ID.
- Session-alias tests prove local precedence over a changed provider title,
  empty/reset behavior, cross-process reload, CLI/JSON round trips, 0600/0700
  persistence, and refusal of control characters, oversized values, symlinks,
  wrong owners, and permissive files. The real PTY renames, filters by the new
  name, resets it, and observes the fixture's canonical provider name again.
- Installer tests execute `open-agent-view` and `opav`, prove an unrelated
  pre-existing `opav` is retained, and retire only OAV's exact obsolete symlink.
- `cargo build --release --locked`: release-mode compilation against the
  committed lock file and Rust 1.75 minimum-version dependency set.
- Release validation locally enforces Rust 1.75 rustfmt, warning-free Clippy,
  all targets, real PTYs, release mode, and the installer. The checked-in
  workflow encodes the wider native-platform contract but was unavailable for
  v0.1.33, whose Linux x86-64 artifact was built and verified manually.
- `scripts/real-tui-tests.sh` runs the serialized real-terminal suite against
  real Unix PTYs with isolated `HOME`/`XDG_STATE_HOME`. At
  120×34, 105×30, and 100×28 they
  exercise populated sections, contextual help, grouping toggle, `ctrl+f`
  filter apply/cancel/clear, slash commands, the harness palette, multiline
  new-task launch/cancellation, peek, rename
  cancellation/submission, native-open suspend/restore, reply, direct
  selected-row interrupt/delete, approval `y`/`n`, bulk delete and archive confirmation, structured
  input, and fixture-fenced refusals. A 90×24 case sends real arrow sequences
  and collapses/expands a group. The 55×18 and 31×7 cases verify the bounded
  narrow layout and explicit too-small fallback. Every case asserts alternate
  screen and cursor restoration from the raw PTY stream.
- Dedicated provider-onboarding PTYs begin with no credentials. Cursor and
  Copilot show an account-catalog authentication error, hand Enter/`l` to a
  native login, reload exact model IDs, preserve the task draft, and select an
  exact model. Cursor then deliberately delays `create-chat`; the dashboard
  animates launch progress and the resulting managed command contains the exact
  model. Separate PTYs prove Claude's provider-allocated background ID is
  resolved to the exact UUID, immediately attaches full-screen, and returns to
  the exact row with the native gesture, while Antigravity performs
  first-run login, exact model selection, `--sandbox` full-screen launch, cache
  ownership, and native return without a dangerous bypass flag.
- Actual-binary PTY tests additionally prove Pi and Copilot native-first launch:
  each receives one exact OAV-generated UUID, the selected model/prompt, opens
  full-screen before returning, and restores the exact new managed row after
  the native background gesture.
- A dedicated real PTY selects the Terminal target, opens its real searchable
  shell picker, confirms the configured shell, launches a private interactive
  shell, backgrounds it with the native gesture, discovers and resumes its exact
  preserved screen, stops it with Ctrl+X, and deletes the completed row with a
  second Ctrl+X. Provider login PTYs use the same isolated registry.
- `tests/setup_installer.rs` covers all fifteen coding-harness installers with an
  isolated `PATH` and fake curl/bash/npm. For every provider it proves non-TTY
  setup refuses before download without `--yes`, confirmed setup uses only the
  exact official URL or package, creates the configured executable, and emits
  the next authentication step. A second Linux case hands every already
  installed provider's exact login arguments to a real PTY.
- `scripts/fresh-provider-setup-tests.sh` is the networked, explicitly invoked
  E2E tier. It starts fifteen independent containers from the pinned
  `node:22-bookworm-slim` digest with empty homes and no mounted credentials or
  workspaces, lets the real OAV binary run each current official installer,
  verifies the installed executable/version, and requires a native PTY login
  handoff. Browser/device authorization is deliberately bounded rather than
  completed. On 2026-08-25 this passed for Claude Code 2.1.245, Codex 0.149.1,
  Pi 0.73.1, OpenCode 1.18.23, Cursor
  2026.08.11-e8db854, Copilot 1.0.80, Antigravity 1.1.20, Mistral Vibe 2.24.3,
  Muse Code 0.2.1 (0.2.1-R1215.1), Qwen Code 0.22.0, and Kimi Code 0.38.0; all
  disposable containers were removed. Vibe's real passive app-server RPC,
  Qwen's real JSONL inventory, Muse's credential-free echo provider, and
  Kimi's unauthenticated provider catalog are also probed in those containers.
  On 2026-08-27 the same empty-home gate passed for Oh My Pi 18.0.8, Grok
  1.0.5 (`5115b46bc9`), Kilo Code 7.5.5, and OpenHands SDK 1.16.1. No account
  state or project workspace was mounted into any of those containers.
- `tests/self_update.rs` runs `--version`, `-v`, and `-V`, then exercises both
  `update` and `upgrade` with isolated fake `curl`/`bash` commands. It verifies
  the exact repository request, install-directory propagation, successful
  handoff, and cleanup of the downloaded installer without network access.
- A dedicated Linux isolated real-PTY case enables fake Claude and managed Pi
  launch controllers and exercises the harness palette through the actual
  binary. It asserts complete choice visibility, arrow/Tab preview, number
  selection, Enter confirmation, Escape cancellation, model reset on a real
  switch, and draft preservation throughout; no provider process receives a
  prompt.
- A second isolated real-PTY case enables only Oh My Pi, Grok, Kilo Code,
  OpenHands, and Terminal. It verifies that all five appear in the real harness
  picker, selects each coding harness with a numbered key, opens its
  asynchronous model picker, checks the exact native model ID, and returns to
  the unchanged task draft after every selection.
- A provider-enabled PTY regression returns two Claude rows, deliberately
  stalls the next provider refresh for two seconds, and verifies that both
  exact selected-row arrow repaints and dashboard exit still finish within 750
  milliseconds. It also asserts that an unchanged frame emits no idle terminal
  output and that cancellation leaves no fake provider child behind. This
  catches accidental provider I/O or unconditional redraws on the input thread.
- The canonical fixture and a real 150×36 PTY render all fifteen coding
  providers plus Terminal together, require every provider-specific row label,
  and retain collision-free normalized IDs.
- A 500-session real-PTY stress case verifies that only the terminal-sized page
  (capped at 25 rows) is initially present, selects the matching Show-more row
  with real arrow keys, reveals the second page, then sends 200 down-arrow
  events in one burst. It requires the
  exact destination to appear within 750 ms while emitting less than 24 KiB,
  guarding bounded rendering, input coalescing, and page-aligned scrolling. It
  also submits `/help` and a 200-character typing burst, requiring each within
  750 ms and bounding terminal output. A
  separate startup case makes Claude discovery sleep for two seconds and
  requires an immediately available Antigravity row within 750 ms, proving
  that the first screen is not gated on the slowest provider.
- An authenticated Antigravity 1.1.20 regression (isolated workspace plus OAV
  state/cache) verified a 14-model live catalog, exact selected-model launch,
  foreground response, Shift+Left return, immediate exact managed row,
  transcript-derived current summary/time, exact Ctrl+X stop, and rediscovery
  from a fresh OAV process. The CLI reused its existing login internally; the
  test did not read or copy authentication material.
- A separate default-mode PTY supplies 1,000 completed fixture records without
  a visibility flag, requires the bounded Completed page within 750 ms, then
  sends 208 arrow events in one burst and requires the exact destination within
  750 ms while emitting less than 24 KiB. It submits `/completed hide`, verifies
  immediate removal, then restores the same bounded page with `/completed
  show`. A second
  real-PTY test points OpenCode at a marker-writing, two-second executable,
  enables `/completed show`, and proves the external-history command is still
  never started without `--include-external`; the adapter test independently
  retains an unused runner response.

## Runtime checks

- Both interactive dashboards were exercised through real allocated TTYs in
  separate fresh `--rm --network none` containers from immutable image ID
  `sha256:8f170f660813ac358f347fa8a3580139972f3ea7a9fb087834f1da44669d9392`:
  `claude agents` 2.1.209 rendered its empty Needs input/Working/Completed
  dashboard, opened and closed shortcut help with `?`, and restored terminal
  modes on Escape; the mounted `open-agent-view` release rendered its empty
  dashboard, opened and closed contextual help, switched to directory view with
  `ctrl+s`, and restored the alternate screen on Escape. The Open Agent View
  probe additionally ran unprivileged with a read-only root, dropped
  capabilities, `no-new-privileges`, a PID limit, and isolated tmpfs home/state.
- The exact fresh-container command shape is documented in
  [the TUI validation guide](tui-validation.md). It pins the immutable image,
  uses a different `--rm` container name for each dashboard, and mounts neither
  host credentials nor a workspace. The committed
  [`populated-sessions.json`](../fixtures/populated-sessions.json) fixture
  supplies nine sessions across every normalized state and actionable
  capability for repeatable populated-screen testing without provider access.
  Fixture mode fences every provider operation,
  including native open and launch, so synthetic capabilities cannot mutate a
  provider.
- The current release binary and nine-session fixture were then rerun in fresh
  isolated containers at 120×34, 55×18, and 31×7. The wide run verified
  populated status rendering, help, directory grouping, explicit provider names,
  and clean exit; 55×18 verified responsive truncation; 31×7 verified the
  explicit minimum-size fallback. Claude 2.1.209 was separately rerun empty at
  the same sizes: wide help/exit, narrow startup/exit, and tiny intro-only
  degradation/exit. The exhaustive action matrix ran against the actual binary
  in host `openpty`; it was not redundantly repeated inside Docker.
- The v0.1.13 candidate repeated that fresh-container comparison after the
  history changes: the populated OAV binary rendered both provider columns,
  help, directory grouping, and clean alternate-screen restoration; Claude
  2.1.209 independently rendered its empty dashboard/help and restored the
  terminal. Both used the same network-disabled, read-only, non-root,
  capability-dropped container profile, and both `--rm` containers were gone
  afterward.
- The v0.1.14 candidate adds a real slow-subprocess regression for detached
  Claude launch, canonical executable-resolution tests for legacy Pi
  supervisors, and an exact read-only probe against the existing managed Codex
  App Server showing that a newly launched thread remains visible without
  `--include-interactive`.
- The v0.1.15 candidate drives Enter on a managed Pi row through the real PTY,
  asserts alternate-screen suspension/restoration instead of Peek, and opens
  the exact completed Pi history on this Linux host in a read-only native-TUI
  probe before returning cleanly to Open Agent View. Space remains independently
  covered as the inline Peek route.
- The v0.1.16 candidate adds a process-level completed-Pi native handoff and a
  stop-to-delete lifecycle. The mock proves stop returns within 500 ms, exit is
  observed before Delete appears, native resume receives the exact ID/session
  directory, deletion revalidates the managed JSONL header/path, and external
  IDs remain refused. A real PTY also bounds completed-Pi Ctrl+X repaint below
  750 ms.
- The v0.1.17 regression case removes the legacy supervisor socket before its
  verified process exits and proves completed-Pi native handoff waits through
  that transition. Render tests also prove an idle-but-attached completed Pi
  row advertises Ctrl+X stop before deletion.
- After the managed-Codex lifecycle fixes, release binary SHA-256
  `efb0a8d8f62f878fc5bb09c6b67da73a871c26f30e7a2dd56d10922a186cbec9`
  was staged mode `0755` and rerun in the same immutable image at a real
  80×24 PTY with the populated fixture. It rendered all state sections and
  explicit Claude/Codex provider columns, opened/closed help, exited through
  Escape, and restored the alternate screen. The eight-target fixture then
  rendered its full provider header and representative rows at 150×36. Claude
  Code 2.1.209 independently rendered its reference empty dashboard at 120×34,
  opened/closed shortcuts, and exited cleanly under the same
  network-disabled/read-only/non-root controls. All named `--rm` containers
  were absent afterward and the exact staging directories were removed.
- Host Claude discovery was compared with `claude agents --json --all`.
- On the reported host environment, Claude 2.1.236 returned completed rows even
  without `--all`; a current-binary JSON probe centrally reduced that result to
  the two active Needs-input rows and removed the stale Antigravity cache row.
  Three warm probes completed in 0.44–0.47 seconds. The Claude CLI process
  itself peaked near 369 MiB, motivating the 15-second default refresh plus
  `ctrl+l` manual refresh.
- The reported mixed store contained more than 1,100 Codex threads and 70,915
  OpenCode history rows. Before the bounded adapters, Codex spent about six
  seconds paging and then discarded all of its rows, while OpenCode admitted
  the full store. With Codex 0.147.0 and OpenCode 1.17.20 resolved from their
  conventional user installs, active-only discovery completed in 1.55 seconds
  without warnings. `--all --history-limit 100` returned 197 total rows (81
  Codex, 100 OpenCode, 10 Claude, and 6 Pi) in 1.48 seconds with explicit
  partial-history warnings. The default 100-record budget repeated in 1.42
  seconds. No task was launched and no history was changed.
- OpenCode 1.17.20 was also probed directly against 501 read-only database
  rows. Its JSON-array output truncated at line 1,851 when stdout was a pipe;
  the bounded `json_object`-per-TSV-row query returned all 501 rows (502 lines
  including the header) and parsed successfully. This is why history discovery
  uses streaming TSV rather than a large JSON array.
- A read-only real-host `openpty` run then used the release binary with the
  reported provider home. It automatically found six launch-capable harnesses,
  moved from Claude to OpenCode with three real Down-arrow events, loaded 467
  OpenCode model choices through Shift+Tab, cancelled without losing the draft,
  enabled bounded completed history, exercised Down/Up and typing, and restored
  the alternate screen on exit. No task was submitted and no provider session
  was modified.
- Cursor `2026.03.20-44cb435`, GitHub Copilot CLI `1.0.80`, and Antigravity
  `1.1.14` were probed with disposable homes/configuration roots and no copied
  credentials. Cursor and Antigravity empty-state interfaces were exercised in
  real PTYs through clean exit and terminal restoration. Copilot's real ACP
  server completed initialize/list without authentication and returned the
  documented authentication error for session creation. No model task was
  dispatched; provider-specific details and exact commands are recorded under
  `docs/exploration/`.
- Pi 0.84.2 was exercised with temporary configuration/session directories,
  offline startup, and no credentials. Strict RPC JSONL, canonical session
  identity, state/name commands, a built-in bash-tool event path that did not
  invoke a model, clean EOF, and exact native TUI resume/terminal restoration
  were verified.
- `tests/real_managed_launch.rs` adds explicit ignored, credentialed host
  lifecycles without reusing the ordinary OAV supervisor state. Pi 0.84.2 used
  a temporary daemon and session directory: it launched an exact-response
  prompt, inspected the assistant transcript, started a second turn,
  interrupted it, stopped the daemon, and proved the exact provider PID exited
  in 3.60 seconds. Codex 0.147.0 used a temporary durable App Server: it
  launched and inspected an exact-response task, exercised a controlled
  `sleep 30` turn and one-time approval when presented, sent exact-turn
  interrupt (or accepted only Codex's precise completion race), archived and
  deleted the exact thread, recovered the idle owner when Codex withheld its
  delete response, restored no test ownership, and removed the exact native
  listener. The final isolated run passed in 37.13 seconds.
- Failed Codex probe iterations were not left as mystery jobs. Nine exact
  `OAV-CODEX-SMOKE` ordinary/archived IDs were enumerated, deleted through
  independent App Servers, and re-listed as absent; the final passing probe
  deleted its own ID. No prompt used a project workspace or modified a file,
  and post-test process scans found no temporary App Server, stdio observer,
  Pi daemon, or Pi RPC child.
- Pi 0.80.6 was additionally resumed from the reported host's real recursive
  per-workspace JSONL directory by exact UUID and exact file-parent
  `--session-dir`; it opened under a PTY and exited with Ctrl+D without the
  former `No session found matching` error. No prompt was sent.
- The official Cursor, Copilot, and Antigravity install/version/help paths were
  then repeated in three new `--rm` containers using pinned Debian/Node image
  digests, tmpfs homes, and no host mounts. Cursor's current installer returned
  `2026.08.11-e8db854`; Copilot's real ACP server again negotiated and listed
  an empty store; Antigravity again returned `1.1.14`. Exact image digests,
  commands, and outputs are in
  `docs/exploration/fresh-container-provider-validation.md`.
- The canonical fifteen-agent-plus-Terminal fixture passed
  `all_supported_providers_coexist_in_one_real_terminal`, including provider
  labels, contextual help, alternate-screen entry, and terminal restoration.
  The same real-PTY test now opens a managed Pi reply composer, a managed
  Cursor interrupt confirmation, and a managed Copilot approval card, then
  proves every action is rejected by the fixture I/O fence.
- The TUI was exercised in a 120-by-30 pseudo-terminal, including navigation,
  grouping, help, and clean shutdown.
- Claude peek was checked against a real host session using read-only logs; the
  VT100 reconstruction surfaced the final assistant screen without escape-code
  leakage.
- Provider-native handoff is covered by a nested real-PTY regression: a plain
  arrow reaches the provider editor, an unchanged cursor produces the timed
  bottom-line hint, repeated Left and Right each background the exact frontend,
  Shift+Right returns immediately, and every reopen restores the retained VT screen.
  Main-account read-only probes repeated the cycle against a managed OpenCode
  session and a managed Codex thread; the OpenCode server and Codex App Server
  remained alive afterward.
- Four opt-in real-host `openpty` regressions now preserve those reproductions:
  nested Pi resume/return, Claude 2.1.243 attach with a real plain-Left hint and
  second-Left return, and a
  mutation-free composer route through Pi/Claude provider cycling,
  draft-preserving Shift+Tab Pi/Claude model selection, the dedicated filter,
  manual refresh, plus the six-harness/OpenCode/bounded-history route described
  above. They are ignored in
  ordinary CI because they require installed provider binaries or private host
  history paths.
- Codex App Server discovery was refreshed repeatedly inside a disposable,
  network-disabled container. Separate protocol probes established that two
  servers sharing `CODEX_HOME` cannot control one another's live turns,
  motivating the shared durable endpoint.
- The release binary was mounted read-only into a disposable container from
  immutable image ID
  `sha256:8f170f660813ac358f347fa8a3580139972f3ea7a9fb087834f1da44669d9392`
  (Codex 0.144.4). It ran as an unprivileged user with no network, credentials,
  or workspace mount; started the durable Unix listener, completed its
  WebSocket handshake, listed an empty store, and exited with no warnings. The
  container and isolated state were then removed.
- Claude and Codex discovery were exercised in disposable,
  network-disabled containers selected explicitly by immutable Docker ID.
  Each probe container was removed afterward.

These fresh-container TTY checks validate empty-state rendering and basic
interaction, not an authenticated provider lifecycle. The containers had no
credentials and no network, so no real Claude or Codex task was dispatched
inside them. Populated session parsing, request replay, lifecycle authority,
approval/input handling, and destructive-action gates are covered by the
deterministic fixtures and disposable mock App Server tests above. A full live
task inside a fresh container remains a separate opt-in credentialed test.

## Claim matrix

| Claim | Evidence | Current status |
| --- | --- | --- |
| Normalization, key routing, responsive layout, capability gates | Locked Rust test suite with Ratatui test backend | Verified |
| Real terminal mode entry/restoration and basic keys | Host PTY plus separate fresh Docker PTYs | Verified |
| Claude/Open Agent View empty-state visual comparison | Fresh Docker PTYs on the same immutable image | Verified manually |
| Every current TUI action route, all normalized states, provider onboarding, and large-queue behavior in a real PTY | Serialized `real_tty` harness using canonical/generated fixtures and fake account-scoped provider CLIs | Verified |
| Default completed-history visibility and bounded bulk archive planning | 1,000-row real PTY, misbehaving-source central-filter tests, real Claude 2.1.236 probe, provider command trap, planner/executor and CLI parser tests | Verified |
| 70,000-row navigation/grouping and local-hide scaling | Cached-group application regression plus one-pass hidden-registry regression | Verified deterministically |
| Searchable async model catalogs, native auth retry, and exact modeled launch | All fifteen harness catalog parsers/transports or explicit exact-ID fallbacks, mock App Server/RPC/HTTP/ACP/headless payload assertions, signed-out real-PTY flows, and isolated setup terminals | Verified deterministically |
| Post-launch refresh/selection without blocking input | Slow-launch worker regression plus exact provider/session hint tests | Verified deterministically |
| Canonical synthetic fixture at wide, narrow, and tiny sizes in fresh Docker PTYs | Reproducible procedure in `tui-validation.md` | Verified manually |
| Windows x64 dashboard, JSON startup, state persistence, packaging, checksum installer, and update-safe executable replacement | Native Windows Server runner plus supplementary Docker cross-compilation of every test target | Verified on Windows |
| Public website real-terminal stories, privacy, responsive layout, player controls, reduced motion, and accessibility | 18 parsed cast/action pairs, 270 extracted audit frames, and desktop/Mac-laptop/phone Playwright and Axe gates | Verified |
| Codex request replay and exact response ownership | Disposable mock App Server | Verified |
| Pi durable RPC launch/reconnect/reply/request/stop/delete/native handoff/model ownership | Disposable mock RPC plus isolated real non-model protocol/TUI/catalog probes | Verified on Linux |
| OpenCode authenticated loopback launch/reconnect/inspect/reply/interrupt/model ownership | Disposable managed-server fixture with exact model payload plus isolated real credential-empty server probe | Verified on Linux |
| Cursor account model/login plus owned launch/log/interrupt/reply authority | Disposable mock CLI, signed-out real PTY, exact Linux PID identity | Verified on Linux |
| Copilot account model/login plus connection-owned prompt/cancel/permission/load authority | Disposable headless/ACP mocks, signed-out real PTY, and real unauthenticated ACP negotiation | Verified |
| Antigravity login/model/sandboxed launch and documented-cache ownership | Disposable cache/command fixtures plus isolated real PTY | Verified |
| Managed-Docker command construction and authority failures | Injected command runner; no Docker daemon | Verified |
| Authenticated managed Codex/Pi host lifecycle in isolated temporary state | Opt-in exact-response launch, inspect, reply, approval race, stop, delete/shutdown and PID cleanup | Verified on the reported host |
| Authenticated Claude/Codex task lifecycle in a fresh container | Dedicated container credentials, network, and disposable tasks required | Not run |
| SSH portability and broad terminal/theme matrix | Environment-specific real-TTY runs required | Not yet claimed |

“Verified manually” records observed behavior but is not a pixel-diff or
automated golden-screen assertion. ANSI pane capture also cannot establish
exact color fidelity; screenshots must be reviewed as external, redacted test
artifacts.

## Documentation checks

Validate the canonical fixture through the same parser used by the application:

```console
jq empty fixtures/populated-sessions.json
target/release/open-agent-view \
  --fixture fixtures/populated-sessions.json \
  --no-host-claude \
  --no-host-codex \
  --json --all |
  jq -e '.sessions | length > 0'
```

The following dependency-free Ruby check resolves every relative Markdown link
from the document that contains it. External URLs are intentionally outside its
scope:

```console
ruby <<'RUBY'
failed = false
Dir.glob('{*.md,docs/**/*.md}').sort.each do |file|
  File.read(file).scan(/\[[^\]]*\]\(([^)]+)\)/).flatten.each do |target|
    target = target.sub(/^</, '').sub(/>$/, '')
    next if target.match?(%r{^(?:https?://|mailto:|#)})
    path = target.split('#', 2).first
    next if path.empty?
    resolved = File.expand_path(path, File.dirname(file))
    unless File.exist?(resolved)
      warn "broken: #{file}: #{target}"
      failed = true
    end
  end
end
exit(failed ? 1 : 0)
RUBY
```

Also run `git diff --check` so whitespace errors do not undermine rendered
examples.

## Safety assertions

- Docker commands use argument arrays and exact inspected container IDs, not a
  shell command assembled from user input.
- An existing session has no stop capability unless its provider/runtime key is
  present in the local ownership registry written at launch time.
- The durable Codex record stores PID start time and exact command-line bytes.
  Linux matches both through `/proc`; macOS matches `proc_pidinfo` start time,
  `KERN_PROCARGS2` argv, and the exact owner of the private Unix socket. Explicit
  Linux idle recovery opens a pidfd and revalidates the complete identity before
  signaling that exact process; macOS safely refuses that exceptional restart.
  Normal dashboard exit never signals the server, and stale sockets are not
  unlinked automatically. Shared/exclusive private recovery locking keeps other
  dashboards from attaching during the bounded replacement window.
- Managed Codex launches use `on-request` approvals and `workspace-write`, with
  no danger-full-access or approval-bypass fallback.
- Managed Pi reconnect requires an exact Linux PID start token and command
  line, with private state/socket/record modes and symlink refusal. Only the
  daemon's in-memory canonical session IDs receive mutations; external JSONL
  history remains inspect/open-only.
- Managed OpenCode reconnect requires the exact Linux process start token and
  command line, ownership of the recorded loopback listener, an authenticated
  health response, and a canonical session ID in the private `0600` record.
  External CLI history never gains server authority.
- Managed-Docker tests use an injected command runner: no test contacts the
  Docker daemon. They cover locked/atomic private ownership persistence,
  random instance IDs, stopped-only creation, exact start/remove argv,
  immutable-ID revalidation, and record removal ordering.
- User-supplied Docker targets remain observe-only. The separately tested
  managed-container API requires immutable identity, matching labels, and an
  external owner record before lifecycle operations.
- Authentication values were not read into the repository, fixtures, or test
  logs.
- Provider/runtime/session/group/summary/warning/notice/confirmation text is
  sanitized before terminal rendering, and dynamic transcript tails are
  bounded; provider text cannot intentionally inject raw terminal controls
  through those surfaces.

## Known unimplemented paths

- Codex file-change acceptance, permission grants, MCP form/URL acceptance,
  secret structured input, supervisor status/stop, and log rotation.
- Managed-container session launch/control remains separate from container
  lifecycle; enter the started container through ordinary Docker tooling or
  observe it with `--docker-container`.
- Claude inline reply and provider-native rename, for which the explored CLI
  exposes no safe background-agent command. OAV-local aliases remain available
  for every normalized session without mutating Claude. Enter opens its native
  attach and the native return gesture backgrounds that frontend; owned Codex threads support inline idle
  reply and active steer.
- Managed OpenCode permission and structured-input requests are not yet exposed
  inline. Durable supervision requires Linux; other platforms retain external
  history inspection and native resume.
- Cursor has no documented global machine-readable inventory. Only sessions
  created through its OAV-owned Linux supervisor are shown and controlled;
  macOS has no managed Cursor discovery/control until an equally race-safe
  process identity primitive is implemented. Cursor's native TTY picker remains
  available outside Open Agent View.
- Copilot provider-wide list results remain observe/native-open in the
  dashboard. OAV-created IDs and bounded latest-message metadata survive a
  restart, while live prompt/permission authority still applies only to the
  retained process-local connection and is never reconstructed from disk.
  Antigravity exposes
  only its documented last-conversation-per-workspace cache; complete history
  and inline controls are not claimed.
- Durable Pi supervision currently requires Linux `/proc` identity. macOS
  retains documented history discovery, inspection, and native resume.

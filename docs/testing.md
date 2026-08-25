# Validation record

This file records the checks used for the initial `open-agent-view` prototype.
They are intentionally split between deterministic tests and disposable runtime
probes. Existing user containers and live agent sessions were never used as
test targets for lifecycle operations.

The reproducible commands, synthetic populated fixture, exhaustive key route,
visual acceptance criteria, and evidence template live in
[the real-TTY validation guide](tui-validation.md). This file records completed
checks; the guide also contains release gates that are not yet complete.

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
  load fixture proves external list results remain unowned. Antigravity tests
  read only a temporary documented cache and build shell-free native commands.
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
- Installer tests execute `open-agent-view`, `opav`, and the legacy
  `coding-agents` alias and prove an unrelated pre-existing `opav` is retained.
- `cargo build --release --locked`: release-mode compilation against the
  committed lock file and Rust 1.75 minimum-version dependency set.
- Release validation locally enforces Rust 1.75 rustfmt, warning-free Clippy,
  all targets, real PTYs, release mode, and the installer. The checked-in
  workflow encodes the wider native-platform contract but was unavailable for
  v0.1.27, whose Linux x86-64 artifact was built and verified manually.
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
- A dedicated real PTY selects the eighth Terminal target, launches a private
  interactive shell, backgrounds it with the native gesture, discovers and resumes its exact
  preserved screen, stops it with Ctrl+X, and deletes the completed row with a
  second Ctrl+X. Provider login PTYs use the same isolated registry.
- `tests/setup_installer.rs` uses an isolated `PATH` and fake curl/bash. It
  proves non-TTY setup refuses before download without `--yes`, confirmed setup
  exposes download/provider progress, uses the exact official URL, executes a
  staged regular file, and removes that file afterward.
- `tests/self_update.rs` runs `--version`, `-v`, and `-V`, then exercises both
  `update` and `upgrade` with isolated fake `gh`/`bash` commands. It verifies
  the exact repository request, install-directory propagation, successful
  handoff, and cleanup of the downloaded installer without network access.
- A dedicated Linux isolated real-PTY case enables fake Claude and managed Pi
  launch controllers and exercises the harness palette through the actual
  binary. It asserts complete choice visibility, arrow/Tab preview, number
  selection, Enter confirmation, Escape cancellation, model reset on a real
  switch, and draft preservation throughout; no provider process receives a
  prompt.
- A provider-enabled PTY regression returns two Claude rows, deliberately
  stalls the next provider refresh for two seconds, and verifies that both
  exact selected-row arrow repaints and dashboard exit still finish within 750
  milliseconds. It also asserts that an unchanged frame emits no idle terminal
  output and that cancellation leaves no fake provider child behind. This
  catches accidental provider I/O or unconditional redraws on the input thread.
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
- The canonical seven-agent-plus-Terminal fixture passed
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
| Searchable async model catalogs, native auth retry, and exact modeled launch | All seven agent catalog parsers/transports, mock App Server/RPC/HTTP/ACP/headless payload assertions, signed-out real-PTY flows, and isolated setup terminals | Verified deterministically |
| Post-launch refresh/selection without blocking input | Slow-launch worker regression plus exact provider/session hint tests | Verified deterministically |
| Canonical synthetic fixture at wide, narrow, and tiny sizes in fresh Docker PTYs | Reproducible procedure in `tui-validation.md` | Verified manually |
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
- The durable Codex record stores PID start time and exact command-line bytes;
  both must match `/proc` before reuse. Explicit idle recovery opens a pidfd and
  revalidates the complete identity before signaling that exact process; normal
  dashboard exit never signals it, and stale sockets are not unlinked
  automatically. Shared/exclusive private recovery locking keeps other
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
- Copilot persisted list results remain observe/native-open in the dashboard.
  The adapter's explicit ACP load contract is tested, but dashboard-managed
  authority currently applies only to sessions launched on its retained
  process-local connection and does not survive a restart. Antigravity exposes
  only its documented last-conversation-per-workspace cache; complete history
  and inline controls are not claimed.
- Durable Pi supervision currently requires Linux `/proc` identity. macOS
  retains documented history discovery, inspection, and native resume.

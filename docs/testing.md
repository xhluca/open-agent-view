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
- A disposable mock Pi RPC executable covers managed launch, live discovery,
  transcript inspection, active steer, confirmation, structured text input,
  interrupt, exact unowned-ID refusal, and shutdown. A second dashboard client
  reconnects through the same verified daemon while its Pi child remains live.
  Panic-safe cleanup stops the exact test daemon; separate tests reject
  symlinked state and permissive/replaced authority records.
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
- Request-reducer/UI tests reject unowned or wrong-turn requests, incomplete
  command context, duplicate/malformed structured questions, stale deadlines,
  and blind file acceptance. Sequential input tests verify exact option
  normalization and confirm that answers never enter the supervisor record.
- Reference-fidelity tests cover initial row focus, cyclic header/row
  navigation, direct escape-to-quit, printable-to-compose behavior,
  context-sensitive `?`, selection reconciliation after filtering, Claude
  worktree grouping, aggregate review/working counts, capability-aware help,
  and the narrow footer's retained help affordance. Focused rendering tests
  sanitize terminal-control characters from every provider-derived or dynamic
  surface and use terminal display width for CJK/grapheme-aware row truncation,
  padding, and editable cursor placement.
- Safety-focused state tests verify that ready-for-review and needs-input
  sessions are treated as live (and therefore require interrupt authority),
  completed sessions use delete language, and active groups cannot enter a
  bulk-delete confirmation path.
- `cargo build --release --locked`: release-mode compilation against the
  committed lock file and Rust 1.75 minimum-version dependency set.
- GitHub Actions enforces Rust 1.75 rustfmt and warning-free Clippy across all
  targets, then repeats tests and release builds on Rust 1.75.0 and stable.
- `scripts/real-tui-tests.sh` runs eleven serialized tests against real Unix PTYs
  with isolated `HOME`/`XDG_STATE_HOME`. At 120×34, 105×30, and 100×28 they
  exercise populated sections, contextual help, grouping toggle, filter
  apply/cancel/clear, multiline new-task launch/cancellation, peek, rename
  cancellation/submission, native-open suspend/restore, reply, interrupt,
  approval `y`/`n`, single/bulk delete and archive confirmation, structured
  input, and fixture-fenced refusals. A 90×24 case sends real arrow sequences
  and collapses/expands a group. The 55×18 and 31×7 cases verify the bounded
  narrow layout and explicit too-small fallback. Every case asserts alternate
  screen and cursor restoration from the raw PTY stream.
- A provider-enabled PTY regression returns two Claude rows, deliberately
  stalls the next provider refresh for two seconds, and verifies that both
  exact selected-row arrow repaints and dashboard exit still finish within 750
  milliseconds. It also asserts that an unchanged frame emits no idle terminal
  output and that cancellation leaves no fake provider child behind. This
  catches accidental provider I/O or unconditional redraws on the input thread.
- A 500-session real-PTY stress case verifies that only 25 rows are initially
  present, selects `Show 25 more · 475 hidden` with real arrow keys, reveals the
  second page, then sends 200 down-arrow events in one burst. It requires the
  exact destination to appear within 750 ms while emitting less than 24 KiB,
  guarding bounded rendering, input coalescing, and page-aligned scrolling. A
  separate startup case makes Claude discovery sleep for two seconds and
  requires an immediately available Antigravity row within 750 ms, proving
  that the first screen is not gated on the slowest provider.
- A separate default-mode PTY supplies 1,000 completed fixture records without
  `--all`, requires a usable `completed hidden` screen within 750 ms, and
  proves that no completed row enters navigation. A second real-PTY test points
  OpenCode at a marker-writing, two-second executable and proves it is never
  started; the adapter test independently retains an unused runner response.

## Runtime checks

- Both interactive dashboards were exercised through real allocated TTYs in
  separate fresh `--rm --network none` containers from immutable image ID
  `sha256:8f170f660813ac358f347fa8a3580139972f3ea7a9fb087834f1da44669d9392`:
  `claude agents` 2.1.209 rendered its empty Needs input/Working/Completed
  dashboard, opened and closed shortcut help with `?`, and restored terminal
  modes on Escape; the mounted `coding-agents` release rendered its empty
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
- Host Claude discovery was compared with `claude agents --json --all`.
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
  were verified. The managed model-prompt lifecycle uses the isolated mock
  above; no authenticated model call is claimed.
- The official Cursor, Copilot, and Antigravity install/version/help paths were
  then repeated in three new `--rm` containers using pinned Debian/Node image
  digests, tmpfs homes, and no host mounts. Cursor's current installer returned
  `2026.08.11-e8db854`; Copilot's real ACP server again negotiated and listed
  an empty store; Antigravity again returned `1.1.14`. Exact image digests,
  commands, and outputs are in
  `docs/exploration/fresh-container-provider-validation.md`.
- The canonical seven-provider fixture passed
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
| Every current TUI action route, all normalized states, and large-queue behavior in a real PTY | Eleven-test `real_tty` harness using canonical and generated fixtures | Verified |
| Default completed-history exclusion and bounded bulk archive planning | 1,000-row real PTY, provider command trap, planner/executor and CLI parser tests | Verified |
| Canonical synthetic fixture at wide, narrow, and tiny sizes in fresh Docker PTYs | Reproducible procedure in `tui-validation.md` | Verified manually |
| Codex request replay and exact response ownership | Disposable mock App Server | Verified |
| Pi durable RPC launch/reconnect/reply/request/interrupt ownership | Disposable mock RPC plus isolated real non-model protocol/TUI probes | Verified on Linux |
| OpenCode authenticated loopback launch/reconnect/inspect/reply/interrupt ownership | Disposable managed-server fixture plus isolated real credential-empty server probe | Verified on Linux |
| Cursor owned launch/log/interrupt/reply authority | Disposable mock CLI with exact Linux PID identity | Verified on Linux |
| Copilot connection-owned prompt/cancel/permission/load authority | Disposable mock ACP plus real unauthenticated ACP negotiation | Verified |
| Antigravity documented-cache discovery and safe native command | Disposable cache/command fixtures plus isolated real PTY | Verified |
| Managed-Docker command construction and authority failures | Injected command runner; no Docker daemon | Verified |
| Authenticated Claude/Codex task lifecycle in a fresh container | Dedicated credentials, network, and disposable tasks required | Not run |
| SSH portability and broad terminal/theme matrix | Environment-specific real-TTY runs required | Not yet claimed |

“Verified manually” records observed behavior but is not a pixel-diff or
automated golden-screen assertion. ANSI pane capture also cannot establish
exact color fidelity; screenshots must be reviewed as external, redacted test
artifacts.

## Documentation checks

Validate the canonical fixture through the same parser used by the application:

```console
jq empty fixtures/populated-sessions.json
target/release/coding-agents \
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
  both must match `/proc` before reuse. Runtime code never signals a persisted
  PID, and stale sockets are not unlinked automatically.
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
- Claude inline reply and rename, for which the explored CLI exposes no safe
  background-agent command. Enter hands the terminal to Claude's native attach
  interface; owned Codex threads support inline idle reply and active steer.
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

# Validation record

This file records the checks used for the initial `open-agent-view` prototype.
They are intentionally split between deterministic tests and disposable runtime
probes. Existing user containers and live agent sessions were never used as
test targets for lifecycle operations.

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
- Reference-fidelity tests cover initial row focus, cyclic header/row
  navigation, direct escape-to-quit, printable-to-compose behavior,
  context-sensitive `?`, selection reconciliation after filtering, Claude
  worktree grouping, aggregate review/working counts, capability-aware help,
  and the narrow footer's retained help affordance.
- Safety-focused state tests verify that ready-for-review and needs-input
  sessions are treated as live (and therefore require interrupt authority),
  completed sessions use delete language, and active groups cannot enter a
  bulk-delete confirmation path.
- `cargo build --release --locked`: release-mode compilation against the
  committed lock file and Rust 1.75 minimum-version dependency set.
- GitHub Actions repeats tests and release builds on Rust 1.75.0 and stable.

## Runtime checks

- Host Claude discovery was compared with `claude agents --json --all`.
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
- Managed-Docker tests use an injected command runner: no test contacts the
  Docker daemon. They cover locked/atomic private ownership persistence,
  random instance IDs, stopped-only creation, exact start/remove argv,
  immutable-ID revalidation, and record removal ordering.
- User-supplied Docker targets remain observe-only. The separately tested
  managed-container API requires immutable identity, matching labels, and an
  external owner record before lifecycle operations.
- Authentication values were not read into the repository, fixtures, or test
  logs.

## Known unimplemented paths

- Codex inline approval/input responses, supervisor status/stop, and log
  rotation.
- Managed-container session launch/control remains separate from container
  lifecycle; enter the started container through ordinary Docker tooling or
  observe it with `--docker-container`.
- Claude inline reply and rename, for which the explored CLI exposes no safe
  background-agent command. Enter hands the terminal to Claude's native attach
  interface; owned Codex threads support inline idle reply and active steer.

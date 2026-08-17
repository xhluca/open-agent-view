# Validation record

This file records the checks used for the initial `open-agent-view` prototype.
They are intentionally split between deterministic tests and disposable runtime
probes. Existing user containers and live agent sessions were never used as
test targets for lifecycle operations.

## Automated checks

- `cargo test --locked`: domain normalization, provider parsing, App Server
  transport behavior, navigation, responsive rendering, process timeouts,
  ownership persistence, Claude launch parsing, VT100 log reconstruction, and
  doctor output.
- A disposable mock App Server test launches an owned Codex thread, drops the
  first supervisor, reconnects from a second supervisor through the same Unix
  socket, discovers the still-active thread, interrupts its exact turn, and
  rejects an external thread ID. It also asserts mode `0700` for the state
  directory and `0600` for the ownership record. The mock PID is re-verified,
  terminated, and reaped by a panic-safe test guard.
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
  network-disabled container. One wrapper and one native server process stayed
  alive during refreshes, and both exited with the dashboard. Separate protocol
  probes established that two servers sharing `CODEX_HOME` cannot control one
  another's live turns, motivating the shared durable endpoint.
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
- User-supplied Docker targets remain observe-only. The separately tested
  managed-container API requires immutable identity, matching labels, and an
  external owner record before lifecycle operations.
- Authentication values were not read into the repository, fixtures, or test
  logs.

## Known unimplemented paths

- Codex steering, inline approval/input responses, archive/delete, supervisor
  status/stop, and log rotation.
- User-facing managed Docker creation and lifecycle commands plus persisted
  external ownership records; the hardened internal API is implemented.
- Provider-native inline reply; Enter hands the terminal to the provider's
  native attach/resume interface instead.

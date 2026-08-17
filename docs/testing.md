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
  alive during refreshes, and both exited with the dashboard.
- Claude and Codex discovery were exercised in disposable,
  network-disabled containers selected explicitly by immutable Docker ID.
  Each probe container was removed afterward.

## Safety assertions

- Docker commands use argument arrays and exact inspected container IDs, not a
  shell command assembled from user input.
- An existing session has no stop capability unless its provider/runtime key is
  present in the local ownership registry written at launch time.
- Docker targets remain observe-only in this milestone.
- Authentication values were not read into the repository, fixtures, or test
  logs.

## Known unimplemented paths

- Durable Codex App Server ownership, launch, steering, and interruption.
- Managed Docker container creation and lifecycle control.
- Provider-native inline reply; Enter hands the terminal to the provider's
  native attach/resume interface instead.

# Changelog

All notable changes will be documented here. Open Agent View remains an early
private preview and may change before a public stable release.

The format follows [Keep a Changelog](https://keepachangelog.com/en/1.1.0/),
and future released versions are intended to follow Semantic Versioning.

## [Unreleased]

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

# Approved product specification

Status: approved on 2026-08-17.

## Product

Build an open reproduction of the workflow exposed by `claude agents`, named
**open-agent-view** and launched from the shell as `open-agent-view`.

The primary experience is a terminal UI that discovers and supervises local
coding-agent sessions on the host or in explicitly enrolled Docker targets. It
offers launch, inspect, reply or steer, resume, interrupt, archive or delete,
filter, and refresh only where the selected provider exposes a verified
capability; no integration is described as having a uniform full lifecycle.

## Priorities

1. Correct and safe lifecycle behavior.
2. A faithful, efficient keyboard interaction model.
3. Provider-neutral architecture and testability.
4. Visual fidelity without copying proprietary branding or code.

## Initial interface scope

- A compact header with runtime context and state counts.
- Sections for ready-for-review, needs-input, working, and completed managed
  sessions by default, with an explicit active-only startup/toggle.
- Rows with status, provider/runtime, name, latest useful summary, age, and
  optional repository metadata.
- Keyboard navigation, contextual help, detail/transcript view, reply flow,
  confirmation dialogs, filters, and a new-session composer with a visible,
  draft-preserving harness picker and draft-preserving searchable provider
  model picker. Authentication errors hand off to the provider's native login
  in an isolated setup terminal and reload the account catalog; credentials
  never pass through OAV.
- A live completed-history toggle, 25-row progressive reveal, and reversible
  local hiding for rows that Open Agent View can observe but does not own.
- Private local session names keyed by stable normalized ID. Provider-native
  titles remain canonical; a local name wins only in OAV until explicitly reset.
- A bounded provider-history window with an explicit CLI override; ordinary
  active-session refreshes must never scan an unbounded persisted store.
- An owned-only default inventory; provider-wide external history requires the
  explicit `--include-external` flag and never inherits mutation authority.
- Graceful behavior in narrow terminals and over SSH/tmux.
- Machine-readable JSON output for scripting and diagnostics.

## Runtime scope

- Host Claude, Codex, Pi, OpenCode, Cursor, GitHub Copilot, and Antigravity via
  their documented CLI/protocol surfaces and explicit ownership boundaries.
- A built-in process-local Terminal target for ordinary interactive shells and
  resumable provider install/login jobs; task text is a name, never evaluated.
- Native foreground launch where the provider exposes a full-screen interface,
  with a boundary double-arrow or Shift+Arrow retaining the frontend and
  restoring the dashboard.
- Confirmed user-local installation of a missing provider harness through its
  official installer, without making Rust or Cargo a user prerequisite.
- Existing Docker containers with explicit discovery policy.
- Optional launch of new, opt-in containers based on a configured image.

## Safety constraints

- Never modify, restart, or stop the existing `webqwen-sbx-*` or `at-codex-*`
  containers during development.
- Active probes use disposable containers based on `basic-claude-uv:latest`.
- Process-control actions target exact session/container identifiers.
- Bypass-permission modes are never enabled by default.
- Secrets and authentication files are referenced in place, never copied into
  the repository or emitted in diagnostics.

## Delivery

- Rust and Ratatui, with Rust 1.75 as the initial MSRV.
- A single binary named `open-agent-view`, installed with `opav` as a shorthand.
- Private development repository at `xhluca/open-agent-view`, suitable for a
  later public release under the MIT license.
- Frequent coherent commits, exploration records, architectural decisions,
  tests, and reproducible installation instructions.

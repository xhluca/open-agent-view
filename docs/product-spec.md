# Approved product specification

Status: approved on 2026-08-17.

## Product

Build an open reproduction of the workflow exposed by `claude agents`, named
**open-agent-view** and launched from the shell as `coding-agents`.

The primary experience is a terminal UI that discovers and supervises Claude
and Codex sessions running either on the host or in Docker. It supports the
full lifecycle: launch, inspect, reply or steer, resume, interrupt, archive or
delete, filter, and refresh.

## Priorities

1. Correct and safe lifecycle behavior.
2. A faithful, efficient keyboard interaction model.
3. Provider-neutral architecture and testability.
4. Visual fidelity without copying proprietary branding or code.

## Initial interface scope

- A compact header with runtime context and state counts.
- Sections for ready-for-review, needs-input, working, and, when `--all` or the
  live `/completed show` command is explicit, completed sessions.
- Rows with status, provider/runtime, name, latest useful summary, age, and
  optional repository metadata.
- Keyboard navigation, contextual help, detail/transcript view, reply flow,
  confirmation dialogs, filters, and a new-session composer with a visible,
  draft-preserving harness picker and draft-preserving searchable provider
  model picker.
- A live completed-history toggle, 25-row progressive reveal, and reversible
  local hiding for rows that Open Agent View can observe but does not own.
- A bounded provider-history window with an explicit CLI override; ordinary
  active-session refreshes must never scan an unbounded persisted store.
- Graceful behavior in narrow terminals and over SSH/tmux.
- Machine-readable JSON output for scripting and diagnostics.

## Runtime scope

- Host Claude via supported CLI surfaces, starting with
  `claude agents --json`.
- Codex via the supported app-server and CLI protocols available in the
  installed version.
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
- A single binary named `coding-agents`.
- Private development repository at `xhluca/open-agent-view`, suitable for a
  later public release under the MIT license.
- Frequent coherent commits, exploration records, architectural decisions,
  tests, and reproducible installation instructions.

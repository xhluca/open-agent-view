# Exploration notebook

These notes capture observable behavior and compatibility constraints before
implementation. Each document separates direct observations from inference so
future CLI changes can be re-tested without treating reverse-engineered details
as stable contracts.

- [`claude-agents.md`](claude-agents.md) — reference TUI behavior and state
  machine.
- [`codex-integration.md`](codex-integration.md) — supported Codex protocol
  options.
- [`docker-runtime.md`](docker-runtime.md) — local image inventory and safe
  runtime boundary.
- [`pi-integration.md`](pi-integration.md) — Pi persistence, RPC, and ownership
  boundaries.
- [`opencode-integration.md`](opencode-integration.md) — OpenCode history and
  managed-server API boundaries.

Return to the [documentation index](../README.md), or use the
[real-TTY validation guide](../tui-validation.md) to repeat the reference and
Open Agent View probes without treating this historical notebook as current
release evidence.

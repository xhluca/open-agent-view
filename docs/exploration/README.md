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
- [`provider-model-selection.md`](provider-model-selection.md) — exact Claude,
  Codex, Pi, and OpenCode catalog/launch surfaces plus picker safety limits.
- [`cursor-integration.md`](cursor-integration.md) — Cursor's TTY-only history
  picker, owned NDJSON runs, and native-resume boundary.
- [`github-copilot-integration.md`](github-copilot-integration.md) — verified
  Copilot ACP discovery and connection-owned control contract.
- [`antigravity-integration.md`](antigravity-integration.md) — Antigravity's
  documented workspace cache and native-only control boundary.
- [`fresh-container-provider-validation.md`](fresh-container-provider-validation.md)
  — no-mount official installer and protocol smoke tests.
- [`main-account-container-regressions.md`](main-account-container-regressions.md)
  — v0.1.17 native-return, executable-identity, launch-latency, and
  authentication reproductions on the reporting account and in isolated
  containers.
- [`auth-model-onboarding.md`](auth-model-onboarding.md) — the seven-agent
  installer/authentication/model-selection contract and isolated foreground
  launch validation.

Return to the [documentation index](../README.md), or use the
[real-TTY validation guide](../tui-validation.md) to repeat the reference and
Open Agent View probes without treating this historical notebook as current
release evidence.

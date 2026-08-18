# Documentation index

Open Agent View is a private pre-alpha. Start with the guide for the job you
are doing; do not infer release readiness from the package version in
`Cargo.toml`.

## Operate the dashboard

- [Install, verify, upgrade, or uninstall](install.md)
- [CLI and keyboard reference](cli.md)
- [Troubleshooting and recovery](troubleshooting.md)
- [Control and ownership boundaries](control-model.md)

## Test a change

- [Validation record and test layers](testing.md)
- [Reproduce real-TTY and fresh-container checks](tui-validation.md)
- [Product specification](product-spec.md)

## Understand or maintain the implementation

- [Architecture](architecture.md)
- [Roadmap and release status](../ROADMAP.md)
- [Release guide](release.md)
- [README design study](readme-inspiration.md)
- [Contribution guide](../CONTRIBUTING.md)
- [Security policy](../SECURITY.md)
- [Changelog](../CHANGELOG.md)

## Exploration record

- [Exploration notebook](exploration/README.md)
- [`claude agents` behavior](exploration/claude-agents.md)
- [Codex App Server integration](exploration/codex-integration.md)
- [Pi persistence and RPC integration](exploration/pi-integration.md)
- [OpenCode history and managed-server integration](exploration/opencode-integration.md)
- [Cursor managed-run boundary](exploration/cursor-integration.md)
- [GitHub Copilot ACP integration](exploration/github-copilot-integration.md)
- [Antigravity workspace-cache integration](exploration/antigravity-integration.md)
- [Docker runtime boundary](exploration/docker-runtime.md)
- [Fresh-container provider validation](exploration/fresh-container-provider-validation.md)

Exploration notes distinguish observations from inferences and retain version
and probe context. Product and operator documentation describe only behavior
implemented and verified in this repository.

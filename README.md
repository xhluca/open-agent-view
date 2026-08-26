<div align="center">

<img src="website/public/favicon.svg" alt="Open Agent View" width="76" height="76">

# Open Agent View

**The open agent view for every local coding harness.**

Monitor parallel sessions, see what needs input, and step back into each
provider's native terminal—all from one queue.

[Website](https://open-agent-view.github.io/) ·
[Install](docs/install.md) ·
[Keyboard guide](docs/cli.md) ·
[Provider notes](docs/exploration/README.md) ·
[Roadmap](ROADMAP.md)

[![Tests](https://img.shields.io/badge/tests-verified-2ea44f.svg)](docs/testing.md)
[![Release](https://img.shields.io/badge/release-v0.1.41-55d3da.svg)](https://github.com/xhluca/open-agent-view/releases/latest)
[![License](https://img.shields.io/badge/license-MIT-55d3da.svg)](LICENSE)

</div>

![A real Open Agent View terminal walkthrough: launch, open a native harness, return, and rename the session](docs/assets/open-agent-view.gif)

> [!NOTE]
> Open Agent View is an early preview. Prebuilt releases currently target
> Linux x86-64 and use your existing GitHub CLI authentication to download the
> private release.

## Why Open Agent View?

Claude Code users already know the value of an agent view: one place to follow
background work and notice when an agent needs help. Other coding harnesses
should not require a separate pile of terminal tabs.

Open Agent View brings Claude Code, Codex, Cursor, GitHub Copilot, OpenCode, Pi,
Antigravity, Mistral Vibe, Muse Code, Qwen Code, Kimi Code, and ordinary terminal
jobs into one open-source dashboard. The conversation still lives in the tool
that created it; selecting a row opens that tool's native interface.

- **Know where to look.** Sessions are grouped as waiting for input, working,
  completed, or unknown, with the harness shown on every row.
- **Return without killing the task.** Open a native session, then move back to
  the dashboard while its work continues.
- **Stay fast as the list grows.** Discovery runs concurrently and the TUI only
  renders the page that fits the terminal.
- **Use controls OAV can prove.** Stop, reply, archive, and delete are offered
  only when the selected provider and session support them safely.

## Quick start

Install the prebuilt binary—no Rust toolchain or Cargo required:

```console
curl -fsSL https://open-agent-view.github.io/install.sh | bash
```

Open a project and launch the dashboard:

```console
cd your-project
open-agent-view
```

The installer also adds the shorter `opav` command. Start typing a task, press
`Tab` to choose a harness, and press `Shift+Tab` to choose one of that account's
available models.

## The everyday workflow

| Do this | In the dashboard |
| --- | --- |
| Move through sessions | `↑` / `↓` |
| Open the selected native session | `Enter` or `→` |
| Return to OAV | `Shift+←`, or `←` twice at an empty prompt |
| Rename a session in OAV | `Ctrl+R` |
| Filter the session list | `Ctrl+F` |
| Stop, then delete or hide a managed session | `Ctrl+X`, then `Ctrl+X` again |
| See the complete contextual key map | `?` |

See the [CLI and keyboard guide](docs/cli.md) for model selection, login/setup,
completed-session visibility, paging, bulk actions, and non-interactive CLI
commands.

## Harnesses

OAV keeps the integration honest: a harness can be visible without necessarily
supporting every mutation another harness exposes.

| Harness | OAV integration |
| --- | --- |
| Claude Code | Managed launch, native attach, verified stop |
| OpenAI Codex | Durable threads, reply, requests, interrupt, archive, delete |
| Cursor | Models, managed launch, native resume, interrupt |
| GitHub Copilot | Managed native/ACP sessions, reply, cancel, approval |
| OpenCode | Durable loopback sessions, inspect, reply, interrupt, resume |
| Pi | Native tasks and durable RPC on Linux, reply, stop, delete, resume |
| Antigravity | Exact managed conversations, models, launch, resume, stop |
| Mistral Vibe | Managed discovery, launch, and native resume |
| Muse Code | Managed discovery, launch, and native resume |
| Qwen Code | Managed discovery, launch, and native resume |
| Kimi Code | Managed discovery, launch, and native resume |
| Terminal | Searchable shell picker, optional native install, background, resume, stop, delete |

Exact CLI versions, model discovery, authentication behavior, platform limits,
and provider-specific caveats live in the [provider notes](docs/exploration/README.md).

## Sessions stay with their tools

OAV reads each installed harness's session state and builds one local index.
Opening a row hands the terminal to that harness; returning brings the shared
queue back. Provider credentials and conversation history are not copied into
the project.

By default, OAV shows sessions it created or explicitly manages. Use
`--include-external` only when you want bounded, provider-wide history. Read the
[control and ownership model](docs/control-model.md) for the complete safety
contract.

## Documentation

- [Install, update, and uninstall](docs/install.md)
- [CLI and keyboard reference](docs/cli.md)
- [Troubleshooting and recovery](docs/troubleshooting.md)
- [Architecture](docs/architecture.md)
- [Testing and real-TTY evidence](docs/testing.md)
- [Demo provenance and reproduction](docs/website.md)
- [Documentation index](docs/README.md)

Contributions are welcome through [CONTRIBUTING.md](CONTRIBUTING.md). Report
security-sensitive findings through [SECURITY.md](SECURITY.md).

## License

[MIT](LICENSE). Open Agent View is independent and is not affiliated with or
endorsed by the providers or CLI projects listed above.

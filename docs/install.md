# Installation

Open Agent View installs one canonical executable named `open-agent-view`. It
also creates `oav` as its short command. The former `opav` spelling remains a
legacy compatibility alias. The normal installation downloads a verified
prebuilt binary: Rust and Cargo are not user prerequisites. An unrelated
existing command at either alias path is never overwritten.

## Supported platforms

The v0.1.49 release covers:

- Linux x86_64 with glibc 2.35 or newer (Debian 12, Ubuntu 22.04+, and similar)
- Linux ARM64 with glibc 2.35 or newer
- macOS Apple silicon
- macOS Intel
- Windows x64

The PowerShell installer selects the `x86_64-pc-windows-msvc` archive,
verifies SHA-256, installs `open-agent-view.exe`, `oav.exe`, and the legacy
`opav.exe` compatibility command without administrator privileges, and adds
the user-local directory to `PATH`.

Apple-silicon installation is exercised natively. The Intel archive is built
for `x86_64-apple-darwin`, exercised through Rosetta, and independently built
and tested on the native Intel macOS CI runner. The installer selects the
archive from `uname` and never reuses a Linux binary on macOS.

The dashboard needs an interactive terminal. `--json` works without a TTY.
All provider CLIs and Docker are optional; install only the providers you
intend to supervise.

## Install a missing coding-agent harness

Open Agent View can stage a provider's official user-local installer, show its
native download/install progress, and run it only after confirmation:

```console
open-agent-view setup claude
open-agent-view setup codex
open-agent-view setup pi
open-agent-view setup opencode
open-agent-view setup cursor
open-agent-view setup copilot
open-agent-view setup antigravity
open-agent-view setup mistral-vibe
open-agent-view setup muse
open-agent-view setup qwen
open-agent-view setup kimi
open-agent-view setup omp
open-agent-view setup grok
open-agent-view setup kilo
open-agent-view setup openhands
```

For a non-interactive script, review the named source first and add `--yes`.
Official shell installers are downloaded to a private temporary file and then
executed; OAV does not pipe a network response directly into a shell. Codex and
Pi use their official npm packages. A failed download/install leaves the
existing OAV binary and provider state alone. Oh My Pi, Grok, and OpenHands use
their official shell installers; Kilo Code uses `@kilocode/cli` from npm.
Restart `open-agent-view` after a
new harness is installed, select it with Tab, and use Shift+Tab for models.
When authentication is required, Enter in the model picker hands the terminal
to the provider's native login and reloads the account catalog afterward. From
the dashboard, `/setup HARNESS` runs the same installation/login sequence in a
private terminal; the native boundary-double-arrow or Shift+Arrow gesture
backgrounds it as a Terminal job and Enter resumes that exact screen. This
prevents setup from inheriting the last opened agent UI.

## One-line installation

Install the latest published release on macOS or Linux:

```console
curl -fsSL \
  https://raw.githubusercontent.com/xhluca/open-agent-view/main/install.sh | bash
```

On Windows, run this in PowerShell:

```powershell
irm https://open-agent-view.github.io/install.ps1 | iex
```

Native Windows opens provider CLIs in the foreground and returns to the
dashboard when their native process exits. Durable Unix-socket supervision and
the Shift+Arrow background gesture remain available through WSL 2; Windows
ConPTY background/resume is not claimed yet. Session discovery, filtering,
renaming, model selection, provider login handoff, foreground launch, JSON
output, and the built-in PowerShell/Command Prompt terminal picker run natively.

The Unix installer writes `~/.local/bin/open-agent-view`, creates an `oav`
symlink, and retains `opav` as a legacy symlink. The Windows installer writes
the equivalent three executable names to
`%LOCALAPPDATA%\Programs\OpenAgentView\bin` and adds that directory to the user
`PATH`. It never requires administrator privileges or edits a PowerShell
profile.

Install a specific version or location with script arguments:

```console
curl -fsSL \
  https://raw.githubusercontent.com/xhluca/open-agent-view/main/install.sh |
  bash -s -- --version MAJOR.MINOR.PATCH --install-dir /absolute/bin
```

The Unix installer requires `curl`, `tar`, `install`, `ln`, `readlink`, and either `sha256sum` or
`shasum`. Run `./install.sh --help` for all arguments and environment variables.
The Windows installer requires Windows PowerShell 5.1 or PowerShell 7 and uses
only built-in archive and checksum commands.

## Verify the installation

Start with checks that do not contact an agent provider:

```console
open-agent-view --version
oav --version
open-agent-view --help
open-agent-view --json --no-host-providers
```

The JSON command should report empty `sessions` and `warnings` arrays. It does
not start the TUI, a provider, Docker, or the durable Codex supervisor.

Then inspect the providers installed on this machine:

```console
open-agent-view doctor
open-agent-view
```

Missing optional providers are warnings. See [troubleshooting](troubleshooting.md)
for provider-specific checks and [TUI validation](tui-validation.md) for a full
interactive test.

## Upgrade

Use the installed shorthand:

```console
oav update
# or: oav upgrade
```

The updater downloads this repository's installer, which resolves the latest
published release, verifies its SHA-256 checksum, and stages the new binary
before replacement. Existing provider sessions and Open Agent View state are
not removed. Its final line reports the verified old-to-new version transition,
or says that the installed version is already current. Re-running the
installation command above is equivalent.

Pin `--version` in automation; the default `latest` channel can change whenever
a new stable release is published.

## Uninstall

Remove the executable installed at `~/.local/bin/open-agent-view`, its `oav`
symlink, and the legacy `opav` compatibility symlink, or their equivalents
under the custom path passed to `--install-dir`.

Uninstalling does not stop or delete provider sessions, containers,
bind-mounted workspaces, state homes, or authority records. See the
[control model](control-model.md) before removing state manually.

## Build from source

Source builds are for contributors and unsupported platforms, not the normal
installation path. They require Rust 1.75 or newer and an authorized checkout:

```console
cargo test --locked
cargo install --path . --locked --root "$HOME/.local"
```

See [`CONTRIBUTING.md`](../CONTRIBUTING.md) for the development workflow and the
[release guide](release.md) for packaging details.

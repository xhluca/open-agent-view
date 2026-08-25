# Installation

Open Agent View installs one canonical executable named `open-agent-view`. It
also creates `opav` as a short symlink. The normal installation downloads a verified prebuilt
binary: Rust and Cargo are not user prerequisites. An unrelated existing
`opav` command is never overwritten.

> [!IMPORTANT]
> The repository and its preview releases are currently private. An authorized
> GitHub account is required to read the installer and download release assets.
> See the [release guide](release.md) for the packaging and verification
> contract.

## Supported platforms

The manually published v0.1.36 release currently covers:

- Linux x86_64 with glibc 2.35 or newer (Debian 12, Ubuntu 22.04+, and similar)

The installer and checked-in native release contract also define these targets,
but v0.1.36 does not claim artifacts for them because they were not built and
tested on native machines:

- Linux ARM64 with glibc 2.35 or newer
- macOS x86_64
- macOS Apple silicon

The installer stops with an explicit explanation when v0.1.36 is requested on
one of those hosts. Use a source build there until a complete native release is
published; do not reuse the Linux binary.

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
```

For a non-interactive script, review the named source first and add `--yes`.
Official shell installers are downloaded to a private temporary file and then
executed; OAV does not pipe a network response directly into a shell. Codex and
Pi use their official npm packages. A failed download/install leaves the
existing OAV binary and provider state alone. Restart `open-agent-view` after a
new harness is installed, select it with Tab, and use Shift+Tab for models.
When authentication is required, Enter in the model picker hands the terminal
to the provider's native login and reloads the account catalog afterward. From
the dashboard, `/setup HARNESS` runs the same installation/login sequence in a
private terminal; the native boundary-double-arrow or Shift+Arrow gesture
backgrounds it as a Terminal job and Enter resumes that exact screen. This
prevents setup from inheriting the last opened agent UI.

## One-line installation

An authorized user of the current private repository can fetch the installer
and release with GitHub CLI authentication:

```console
gh auth login
gh api \
  -H "Accept: application/vnd.github.raw+json" \
  repos/xhluca/open-agent-view/contents/install.sh | bash
```

After the repository and first release become public, the equivalent command is:

```console
curl -fsSL \
  https://raw.githubusercontent.com/xhluca/open-agent-view/main/install.sh | bash
```

Both commands install to `~/.local/bin/open-agent-view`; `opav` invokes that
same file. If the directory is not
already on `PATH`, the installer prints the exact next step. It does not change
shell startup files.

Install a specific version or location with script arguments:

```console
curl -fsSL \
  https://raw.githubusercontent.com/xhluca/open-agent-view/main/install.sh |
  bash -s -- --version MAJOR.MINOR.PATCH --install-dir /absolute/bin
```

The installer requires `curl`, `tar`, `install`, `ln`, `readlink`, and either `sha256sum` or
`shasum`. Run `./install.sh --help` for all arguments and environment variables.

## Verify the installation

Start with checks that do not contact an agent provider:

```console
open-agent-view --version
opav --version
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
opav update
# or: opav upgrade
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

Remove the executable installed at `~/.local/bin/open-agent-view` and its
`opav` symlink, or their equivalents under the custom path
passed to `--install-dir`.

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

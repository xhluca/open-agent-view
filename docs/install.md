# Installation

Open Agent View installs one executable named `coding-agents`. The normal
installation downloads a verified prebuilt binary: Rust and Cargo are not user
prerequisites.

> [!IMPORTANT]
> The repository and its preview releases are currently private. An authorized
> GitHub account is required to read the installer and download release assets.
> See the [release guide](release.md) for the packaging and verification
> contract.

## Supported platforms

The manually published v0.1.13 release currently covers:

- Linux x86_64 with glibc 2.35 or newer (Debian 12, Ubuntu 22.04+, and similar)

The installer and checked-in native release contract also define these targets,
but v0.1.13 does not claim artifacts for them because they were not built and
tested on native machines:

- Linux ARM64 with glibc 2.35 or newer
- macOS x86_64
- macOS Apple silicon

The installer stops with an explicit explanation when v0.1.13 is requested on
one of those hosts. Use a source build there until a complete native release is
published; do not reuse the Linux binary.

The dashboard needs an interactive terminal. `--json` works without a TTY.
All provider CLIs and Docker are optional; install only the providers you
intend to supervise.

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

Both commands install to `~/.local/bin/coding-agents`. If that directory is not
already on `PATH`, the installer prints the exact next step. It does not change
shell startup files.

Install a specific version or location with script arguments:

```console
curl -fsSL \
  https://raw.githubusercontent.com/xhluca/open-agent-view/main/install.sh |
  bash -s -- --version MAJOR.MINOR.PATCH --install-dir /absolute/bin
```

The installer requires `curl`, `tar`, `install`, and either `sha256sum` or
`shasum`. Run `./install.sh --help` for all arguments and environment variables.

## Verify the installation

Start with checks that do not contact an agent provider:

```console
coding-agents --version
coding-agents --help
coding-agents --json --no-host-providers
```

The JSON command should report empty `sessions` and `warnings` arrays. It does
not start the TUI, a provider, Docker, or the durable Codex supervisor.

Then inspect the providers installed on this machine:

```console
coding-agents doctor
coding-agents
```

Missing optional providers are warnings. See [troubleshooting](troubleshooting.md)
for provider-specific checks and [TUI validation](tui-validation.md) for a full
interactive test.

## Upgrade

Run the installer again. It verifies and stages the new binary before replacing
the installed executable. Existing provider sessions and Open Agent View state
are not removed.

Pin `--version` in automation; the default `latest` channel can change whenever
a new stable release is published.

## Uninstall

Remove the executable installed at `~/.local/bin/coding-agents`, or the custom
path passed to `--install-dir`.

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

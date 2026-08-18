# Installation and release verification

`open-agent-view` installs one executable named `coding-agents`.

> [!IMPORTANT]
> No `v0.1.0` tag, GitHub release, public package, or downloadable archive is
> published as of this private pre-alpha. The `0.1.0` Cargo package version is
> development metadata, not evidence of a release. Use the checkout procedure
> below. The archive instructions later in this document become applicable
> only after the matching release actually appears in the repository.

## Prerequisites

- An authorized checkout of the private repository.
- Rust 1.75.0 or newer. CI tests both the minimum version and current stable.
- A terminal for the interactive dashboard; JSON mode works without a TTY.
- Claude Code, Codex, or Docker only for the adapters you intend to use.
- Linux for durable managed host Codex supervision. Other read-only adapters
  may compile elsewhere, but this pre-alpha is not validated as portable.

Claude, Codex, and Docker are optional at startup. Disable an unavailable host
provider with `--no-host-claude` or `--no-host-codex`.

## Install the current private checkout

Clone through the access method authorized for your GitHub account:

```console
git clone git@github.com:xhluca/open-agent-view.git
cd open-agent-view
rustup toolchain install 1.75.0 --profile minimal
cargo +1.75.0 test --locked
cargo +1.75.0 build --release --locked
cargo +1.75.0 install --path . --locked
```

The final command normally installs to the Cargo binary directory
(`$CARGO_HOME/bin`, usually `~/.cargo/bin`). To install under the conventional
user-local prefix instead:

```console
cargo +1.75.0 install --path . --locked --root "$HOME/.local"
```

Ensure the selected `bin` directory is on `PATH`. Record the source revision
when comparing test reports or filing a problem:

```console
git rev-parse HEAD
coding-agents --version
```

`--locked` is required for tests, builds, and installation. It prevents Cargo
from resolving a dependency graph different from the committed `Cargo.lock`.

## Non-destructive smoke tests

Verify the executable and parser without contacting a provider:

```console
coding-agents --version
coding-agents --help
coding-agents --json --no-host-claude --no-host-codex
```

The JSON command should report empty `sessions` and `warnings` arrays. It does
not start the TUI, a provider, Docker, or the durable Codex App Server.

Next inspect locally configured prerequisites:

```console
coding-agents doctor
coding-agents doctor --json
```

`doctor` is read-only. Missing optional host providers are warnings because
single-provider and Docker-only use are valid. An explicitly requested
container that cannot be inspected is an error and makes the command exit
nonzero.

Finally perform a real-TTY empty-state check:

```console
coding-agents --no-host-claude --no-host-codex
```

Press `?`, close help, press `ctrl+s`, and press `esc`. Confirm that the
alternate screen, cursor, and terminal input mode are restored. Follow the
[TUI validation guide](tui-validation.md) for populated fixtures and fresh
Docker isolation. An empty-state smoke test does not validate authenticated
provider lifecycle behavior.

## Upgrade a checkout installation

Review the target commit and changelog, update the checkout without discarding
local work, then repeat the locked test/build/install sequence:

```console
git status --short
git fetch origin
git log --oneline --decorate HEAD..origin/main
git switch main
git pull --ff-only
cargo +1.75.0 test --locked
cargo +1.75.0 install --path . --locked
```

Proceed with the switch/pull only when `git status` is clean and the reviewed
commits are the intended upgrade.

Do not delete `$XDG_STATE_HOME/open-agent-view` (or its default under
`~/.local/state`) while upgrading. Authority records are deliberately retained
across binary replacement. See [troubleshooting](troubleshooting.md) before
changing them.

## Uninstall

For a normal Cargo installation:

```console
cargo uninstall open-agent-view
```

When installed with `--root "$HOME/.local"`, specify the same root:

```console
cargo uninstall open-agent-view --root "$HOME/.local"
```

Uninstalling the executable does not stop or delete provider sessions,
containers, bind-mounted workspaces/state homes, or authority records. This is
intentional; the pre-alpha has no all-state purge operation.

## Future tagged archives (not currently available)

The release workflow is prepared to publish these GNU/Linux x86_64 assets:

```text
open-agent-view-VERSION-x86_64-unknown-linux-gnu.tar.gz
open-agent-view-VERSION-x86_64-unknown-linux-gnu.tar.gz.sha256
```

The binary is built on Ubuntu 22.04, establishing a glibc 2.35 floor. Other
architectures, older GNU/Linux systems, and other operating systems should use
a tested source build until native artifacts exist.

Only use this procedure after confirming the exact `vMAJOR.MINOR.PATCH` release
and both named assets exist in the repository's Releases page. Choose an
explicit version rather than a moving “latest” URL:

```console
OAV_VERSION=MAJOR.MINOR.PATCH
OAV_TARGET=x86_64-unknown-linux-gnu
OAV_ARCHIVE="open-agent-view-${OAV_VERSION}-${OAV_TARGET}.tar.gz"
OAV_RELEASE_URL="https://github.com/xhluca/open-agent-view/releases/download/v${OAV_VERSION}"

curl --fail --location --proto '=https' --tlsv1.2 \
  --remote-name "${OAV_RELEASE_URL}/${OAV_ARCHIVE}"
curl --fail --location --proto '=https' --tlsv1.2 \
  --remote-name "${OAV_RELEASE_URL}/${OAV_ARCHIVE}.sha256"
sha256sum --check "${OAV_ARCHIVE}.sha256"
tar -xzf "${OAV_ARCHIVE}"
install -d "$HOME/.local/bin"
install -m 0755 \
  "open-agent-view-${OAV_VERSION}-${OAV_TARGET}/coding-agents" \
  "$HOME/.local/bin/coding-agents"
```

The checksum command must print the archive name followed by `OK`; stop if it
does not. The archive also contains the exact release `README.md` and `LICENSE`.
The workflow creates sorted members with the tag commit timestamp, numeric
owner/group zero, and normalized file modes so packaging metadata is stable for
the same tagged build.

A tagged source install, once that tag exists, can be pinned as follows:

```console
OAV_TAG=vMAJOR.MINOR.PATCH
cargo +1.75.0 install \
  --locked \
  --git ssh://git@github.com/xhluca/open-agent-view.git \
  --tag "${OAV_TAG}" \
  open-agent-view
```

## Maintainer release procedure

This section describes prepared automation, not a completed release. Before
publishing, finish the release gates in [ROADMAP.md](../ROADMAP.md), update
[CHANGELOG.md](../CHANGELOG.md), and ensure the package version in `Cargo.toml`
matches the intended tag exactly. From the reviewed release commit:

```console
git tag -s vMAJOR.MINOR.PATCH -m "open-agent-view vMAJOR.MINOR.PATCH"
git push origin vMAJOR.MINOR.PATCH
```

Pushing a stable `vMAJOR.MINOR.PATCH` tag triggers
`.github/workflows/release.yml`. The workflow:

1. validates the tag against the Cargo package version;
2. tests and builds the locked source with Rust 1.75.0;
3. creates and verifies the deterministic archive plus SHA-256 file;
4. extracts and smoke-tests the packaged executable;
5. retains both files as workflow artifacts; and
6. creates the GitHub release with generated notes and both original assets.

The workflow refuses prerelease-shaped or version-mismatched tags. It publishes
only the tag GitHub already received (`gh release create --verify-tag`); it
does not create a missing tag or publish from an ordinary branch run. Merging
the workflow alone never creates a release.

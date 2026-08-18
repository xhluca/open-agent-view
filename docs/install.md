# Installation and release verification

`open-agent-view` installs one executable named `coding-agents`. Tagged
releases provide a GNU/Linux x86_64 archive and a separate SHA-256 checksum:

```text
open-agent-view-VERSION-x86_64-unknown-linux-gnu.tar.gz
open-agent-view-VERSION-x86_64-unknown-linux-gnu.tar.gz.sha256
```

The release binary targets `x86_64-unknown-linux-gnu` and is built on Ubuntu
22.04 (glibc 2.35 floor). Other architectures, older GNU/Linux systems, and
operating systems should install from source until native archives are added.
Claude Code, Codex, and Docker are optional at startup; only the adapters for
installed or explicitly enabled runtimes are used.

## Install a tagged Linux archive

Choose an explicit version rather than a moving “latest” URL so the archive
name, source tag, and expected checksum stay reviewable. This example installs
version `0.1.0` for the current user:

```console
VERSION=0.1.0
TARGET=x86_64-unknown-linux-gnu
ARCHIVE="open-agent-view-${VERSION}-${TARGET}.tar.gz"
RELEASE_URL="https://github.com/xhluca/open-agent-view/releases/download/v${VERSION}"

curl --fail --location --proto '=https' --tlsv1.2 \
  --remote-name "${RELEASE_URL}/${ARCHIVE}"
curl --fail --location --proto '=https' --tlsv1.2 \
  --remote-name "${RELEASE_URL}/${ARCHIVE}.sha256"
sha256sum --check "${ARCHIVE}.sha256"

tar -xzf "${ARCHIVE}"
install -d "${HOME}/.local/bin"
install -m 0755 \
  "open-agent-view-${VERSION}-${TARGET}/coding-agents" \
  "${HOME}/.local/bin/coding-agents"
```

The checksum command must print the archive name followed by `OK`. Stop if it
does not. Ensure `${HOME}/.local/bin` is on `PATH`, then run the smoke tests
below.

The archive also contains the exact release `README.md` and `LICENSE`. The
workflow creates it with sorted members, the tag commit timestamp, numeric
owner/group zero, and normalized file modes so packaging metadata is stable
for the same tagged build.

## Install a tagged source build with Cargo

Source installation builds the same locked dependency graph but compiles for
the local machine. Pin both the tag and the documented Rust toolchain:

```console
rustup toolchain install 1.75.0 --profile minimal
cargo +1.75.0 install \
  --locked \
  --git https://github.com/xhluca/open-agent-view \
  --tag v0.1.0 \
  open-agent-view
```

For development from an existing checkout:

```console
cargo test --locked
cargo build --release --locked
cargo install --path . --locked
```

`--locked` is required in all three cases; it prevents Cargo from silently
resolving a dependency graph different from the committed `Cargo.lock`.

## Non-destructive smoke tests

First verify that the installed executable and command parser are healthy:

```console
coding-agents --version
coding-agents --help
```

For release `v0.1.0`, the first command should print:

```text
coding-agents 0.1.0
```

Then exercise normalized startup without probing Claude, Codex, or Docker:

```console
coding-agents --json --no-host-claude --no-host-codex
```

The result should contain empty `sessions` and `warnings` arrays. This command
does not start the TUI or contact a provider.

Finally, inspect the local provider prerequisites without changing sessions:

```console
coding-agents doctor
```

`doctor` is read-only. Missing host providers are warnings because Docker-only
use is valid. An explicitly requested container that cannot be verified is an
error and makes the command exit nonzero.

## Maintainer release procedure

The package version in `Cargo.toml` and the tag must match exactly. After the
commit intended for release passes CI:

```console
git tag -s v0.1.0 -m "open-agent-view v0.1.0"
git push origin v0.1.0
```

Pushing a stable `vMAJOR.MINOR.PATCH` tag triggers
`.github/workflows/release.yml`. The workflow:

1. validates the tag against the Cargo package version;
2. tests and builds the locked source with Rust 1.75.0;
3. creates and verifies the deterministic archive plus SHA-256 file;
4. extracts and smoke-tests the packaged executable;
5. retains both files as workflow artifacts; and
6. creates the GitHub release with generated notes and both original assets.

The workflow refuses prerelease-shaped or version-mismatched tags. It uses the
tag that GitHub already received (`gh release create --verify-tag`); it never
creates a missing tag or publishes from an untagged branch run.

No release is created by merely merging the workflow. Publishing occurs only
after an authorized maintainer pushes a matching tag.

## Uninstall

For an archive installed to the user-local path:

```console
rm "${HOME}/.local/bin/coding-agents"
```

For a Cargo installation:

```console
cargo uninstall open-agent-view
```

Uninstalling the executable does not delete provider sessions. Runtime
ownership/state files are intentionally kept so a future installation cannot
silently adopt unrelated sessions.

# Release guide

Open Agent View is distributed as a prebuilt `coding-agents` executable. Users
should not need Rust or Cargo. This guide is for maintainers preparing the
artifacts consumed by [`install.sh`](../install.sh).

## Current release status

Version 0.1.3 is the current private preview release; version 0.1.2 was the
initial published preview. The unpublished `v0.1.0` and `v0.1.1` build tags
were retained rather than moved after their native release gates exposed,
respectively, a macOS portability error and an incremental terminal-repaint
race in the test harness. The repository is private, so preview installation
requires an authenticated GitHub account until the project is made public. A
version tag alone is not sufficient:
present the one-line installer as usable only after its GitHub release contains
the archive and checksum for every supported target:

```text
open-agent-view-VERSION-x86_64-unknown-linux-gnu.tar.gz
open-agent-view-VERSION-x86_64-unknown-linux-gnu.tar.gz.sha256
open-agent-view-VERSION-aarch64-unknown-linux-gnu.tar.gz
open-agent-view-VERSION-aarch64-unknown-linux-gnu.tar.gz.sha256
open-agent-view-VERSION-x86_64-apple-darwin.tar.gz
open-agent-view-VERSION-x86_64-apple-darwin.tar.gz.sha256
open-agent-view-VERSION-aarch64-apple-darwin.tar.gz
open-agent-view-VERSION-aarch64-apple-darwin.tar.gz.sha256
```

## Automated release contract

Pushing a stable `vMAJOR.MINOR.PATCH` tag triggers
`.github/workflows/release.yml`. The workflow:

1. requires the tag to exactly match the version in `Cargo.toml`;
2. runs the isolated installer tests and the locked Rust test suite;
3. builds Linux x86_64/ARM64 and macOS x86_64/ARM64 on native runners;
4. creates a deterministic archive and SHA-256 checksum;
5. smoke-tests the extracted binary;
6. installs the packaged artifact through the same `install.sh` users run;
7. retains the assets as workflow artifacts; and
8. publishes a GitHub release for the pre-existing tag.

The Ubuntu 22.04 builders establish a glibc 2.35 floor. Windows and older
GNU/Linux systems are not release targets yet. The installer fails clearly on
those systems instead of downloading an incompatible binary.

## Prepare a release

Before creating a tag:

```console
cargo fmt --all -- --check
cargo clippy --all-targets --locked -- -D warnings
cargo test --locked
scripts/test-installer.sh
```

Then:

1. finish the release gates in [`ROADMAP.md`](../ROADMAP.md);
2. update [`CHANGELOG.md`](../CHANGELOG.md);
3. set the intended version in `Cargo.toml` and `Cargo.lock`;
4. review the exact release commit; and
5. obtain maintainer approval to publish.

From the approved commit:

```console
git tag -s vMAJOR.MINOR.PATCH -m "open-agent-view vMAJOR.MINOR.PATCH"
git push origin vMAJOR.MINOR.PATCH
```

The workflow never creates a tag from a branch build. Do not retry a failed
release by moving an existing tag; fix the cause and choose a new version.

## Verify a published release

Confirm the workflow is green and the GitHub release contains the original
archive and checksum. Then exercise both authenticated and public installation
paths as applicable:

```console
OAV_VERSION=MAJOR.MINOR.PATCH ./install.sh
coding-agents --version
coding-agents --json --no-host-claude --no-host-codex
```

For a public release, repeat the command from fresh Linux x86_64, Linux ARM64,
macOS Intel, and macOS Apple silicon environments without repository credentials.
For a private release, repeat it with a least-privilege GitHub account that can
read the repository.

## Distribution security

The installer downloads both release assets, validates that the checksum is a
64-character SHA-256 value, verifies the archive before extraction, stages the
new executable, and atomically replaces `coding-agents` only after verification.
It does not edit shell startup files or fall back to compiling source.

SHA-256 detects corruption and release-asset mismatch, but it does not by
itself prove who built an artifact. Build provenance/attestations and additional
native targets remain follow-up release work.

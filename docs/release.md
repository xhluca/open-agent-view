# Release guide

Open Agent View is distributed as a prebuilt `coding-agents` executable. Users
should not need Rust or Cargo. This guide is for maintainers preparing the
artifacts consumed by [`install.sh`](../install.sh).

## Current release status

Version 0.1.10 is the current published private preview release; v0.1.12 is a
tagged release candidate whose native packaging is waiting on the repository
owner to resolve GitHub Actions billing/spending-limit enforcement. Rerun the
existing v0.1.12 workflow after that account setting is fixed; do not move the
tag or publish a partial target set. Version 0.1.2 was the initial published
preview. The unpublished `v0.1.0`, `v0.1.1`, and `v0.1.9`
build tags were retained rather than moved after their native release gates
exposed, respectively, a macOS portability error, an incremental
terminal-repaint race, and a Linux-only managed-Pi assumption in a macOS PTY
test. The unpublished `v0.1.11` tag is likewise retained after its ARM Linux
runner exposed an insecure shared-state initialization order under a `0022`
umask; v0.1.12 fixes it and tests that umask explicitly. The repository is
private, so preview installation requires an
authenticated GitHub account until the project is made public. A version tag
alone is not sufficient:
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
scripts/real-tui-tests.sh
scripts/test-installer.sh
```

For a release containing the completed/model/lifecycle changes, retain evidence
for these focused gates in addition to the aggregate commands:

- the 70,000-session grouping and local-hide tests complete without rebuilding
  groups during navigation;
- real-PTY exercise covers `/completed show|hide`, draft-preserving Shift+Tab
  model selection, Ctrl+X local-hide wording from list and Peek, exact composer
  cursor placement, and nonblocking post-launch refresh/selection;
- isolated Pi proves selected `--model` propagation and refuses to replace an
  old daemon with active owned work;
- isolated OpenCode proves the exact documented model object is present in the
  asynchronous prompt body;
- Claude and Codex catalog tests consume their provider-native surfaces and
  reject malformed/pagination-overflow results; and
- `coding-agents sessions hide`, `hidden`, and `unhide` are smoke-tested with an
  isolated `HOME`/`XDG_STATE_HOME`, including JSON output and private file modes.

Do not describe authenticated model availability as verified merely because a
credential-free catalog or mock launch passed. Record any real provider model
turn separately with provider version, selected identifier, isolated state,
and cleanup result.

Then:

1. finish the release gates in [`ROADMAP.md`](../ROADMAP.md);
2. update [`CHANGELOG.md`](../CHANGELOG.md);
3. set the intended version in `Cargo.toml` and `Cargo.lock`;
4. review the exact release commit; and
5. obtain maintainer approval to publish.

From the approved commit:

```console
git tag -a vMAJOR.MINOR.PATCH -m "open-agent-view vMAJOR.MINOR.PATCH"
git push origin vMAJOR.MINOR.PATCH
```

The workflow never creates a tag from a branch build. Do not retry a failed
release by moving an existing tag; fix the cause and choose a new version.
Annotated tags are the current repository convention. Moving to signed tags
requires a valid, non-expired maintainer signing key and a documented public-key
verification path; do not claim a signature when those prerequisites are absent.

## Verify a published release

Confirm the workflow is green and the GitHub release contains the original
archive and checksum. Then exercise both authenticated and public installation
paths as applicable:

```console
OAV_VERSION=MAJOR.MINOR.PATCH ./install.sh
coding-agents --version
coding-agents --json --no-host-providers
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

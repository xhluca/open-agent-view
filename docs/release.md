# Release guide

Open Agent View is distributed as a prebuilt `open-agent-view` executable. Users
should not need Rust or Cargo. This guide is for maintainers preparing the
artifacts consumed by [`install.sh`](../install.sh).

## Current release status

Version 0.1.22 is the current published private preview. Hosted jobs were
unavailable, so the maintainer explicitly authorized a manual Linux x86-64
release. Its release page contains only:

```text
open-agent-view-0.1.22-x86_64-unknown-linux-gnu.tar.gz
open-agent-view-0.1.22-x86_64-unknown-linux-gnu.tar.gz.sha256
```

The archive was built, tested, packaged, checksum-verified, installer-tested,
and smoke-tested on the native Linux x86-64 host. No ARM64 or macOS artifact is
claimed for v0.1.22. Version 0.1.2 was the initial published preview. The
unpublished `v0.1.0`, `v0.1.1`, and `v0.1.9`
build tags were retained rather than moved after their native release gates
exposed, respectively, a macOS portability error, an incremental
terminal-repaint race, and a Linux-only managed-Pi assumption in a macOS PTY
test. The unpublished `v0.1.11` tag is likewise retained after its ARM Linux
runner exposed an insecure shared-state initialization order under a `0022`
umask; v0.1.12 fixes it and tests that umask explicitly. The repository is
private, so preview installation requires an
authenticated GitHub account until the project is made public. A version tag
alone is not sufficient:
For future complete native releases, publish the archive and checksum for every
advertised target:

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

## Manual Linux x86-64 release procedure

After the full local gate below, create the same deterministic package shape as
the native workflow:

```console
version="$(cargo metadata --locked --no-deps --format-version 1 | jq -r '.packages[] | select(.name == "open-agent-view") | .version')"
target=x86_64-unknown-linux-gnu
stem="open-agent-view-${version}-${target}"
install -d "dist/${stem}"
install -m 0755 target/release/open-agent-view "dist/${stem}/open-agent-view"
install -m 0644 LICENSE README.md "dist/${stem}/"
source_date_epoch="$(git show -s --format=%ct HEAD)"
tar --sort=name --mtime="@${source_date_epoch}" --owner=0 --group=0 \
  --numeric-owner -C dist -czf "dist/${stem}.tar.gz" "${stem}"
(cd dist && sha256sum "${stem}.tar.gz" >"${stem}.tar.gz.sha256")
```

Extract and smoke-test the archive, test `install.sh` against a temporary local
release root, create and push an annotated version tag, then publish exactly the
verified archive and checksum with `gh release create`. Never
upload an untested cross-compiled artifact merely to fill the matrix.
The archive contains only the canonical `open-agent-view` executable. The
installer creates relative `opav` and legacy `coding-agents` symlinks after
version verification and leaves an unrelated existing alias untouched.

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
- real-PTY exercise covers default-visible completed paging, `/completed show|hide`, draft-preserving Shift+Tab
  model selection, Ctrl+X local-hide wording from list and Peek, exact composer
  cursor placement, and nonblocking post-launch refresh/selection;
- isolated Pi proves selected `--model` propagation and refuses to replace an
  old daemon with active owned work;
- isolated OpenCode proves the exact documented model object is present in the
  asynchronous prompt body;
- Claude and Codex catalog tests consume their provider-native surfaces and
  reject malformed/pagination-overflow results; and
- `open-agent-view sessions hide`, `hidden`, and `unhide` are smoke-tested with an
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

Confirm the GitHub release contains the original verified archive and checksum.
For an automated release also confirm the workflow is green. Then exercise both
authenticated and public installation paths as applicable:

```console
OAV_VERSION=MAJOR.MINOR.PATCH ./install.sh
open-agent-view --version
opav --version
open-agent-view --json --no-host-providers
```

For a public release, repeat the command from fresh Linux x86_64, Linux ARM64,
macOS Intel, and macOS Apple silicon environments without repository credentials.
For a private release, repeat it with a least-privilege GitHub account that can
read the repository.

## Distribution security

The installer downloads both release assets, validates that the checksum is a
64-character SHA-256 value, verifies the archive before extraction, stages the
new executable, and atomically replaces `open-agent-view` only after verification.
It then creates guarded relative shorthand/compatibility symlinks; it never
overwrites a command whose version output does not identify Open Agent View.
It does not edit shell startup files or fall back to compiling source.

SHA-256 detects corruption and release-asset mismatch, but it does not by
itself prove who built an artifact. Build provenance/attestations and additional
native targets remain follow-up release work.

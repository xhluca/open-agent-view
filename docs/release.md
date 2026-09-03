# Release guide

Open Agent View is distributed as a prebuilt `open-agent-view` executable. Users
should not need Rust or Cargo. This guide is for maintainers preparing the
artifacts consumed by [`install.sh`](../install.sh).

## Current release status

Version 0.1.52 is the release described by this guide. Its
[published release](https://github.com/xhluca/open-agent-view/releases/tag/v0.1.52)
contains verified archives and adjacent checksums for:

```text
open-agent-view-0.1.52-x86_64-unknown-linux-gnu.tar.gz
open-agent-view-0.1.52-x86_64-unknown-linux-gnu.tar.gz.sha256
open-agent-view-0.1.52-aarch64-unknown-linux-gnu.tar.gz
open-agent-view-0.1.52-aarch64-unknown-linux-gnu.tar.gz.sha256
open-agent-view-0.1.52-x86_64-apple-darwin.tar.gz
open-agent-view-0.1.52-x86_64-apple-darwin.tar.gz.sha256
open-agent-view-0.1.52-aarch64-apple-darwin.tar.gz
open-agent-view-0.1.52-aarch64-apple-darwin.tar.gz.sha256
open-agent-view-0.1.52-x86_64-pc-windows-msvc.zip
open-agent-view-0.1.52-x86_64-pc-windows-msvc.zip.sha256
```

The archive was built, tested, packaged, checksum-verified, installer-tested,
and smoke-tested both before publication and through the published release.
The adjacent `.sha256` release asset records the verified archive digest.
Apple silicon was exercised on the native `mbp` host. The Intel archive was
executed through Rosetta and the same commit was built and tested by the native
Intel macOS CI runner. Linux ARM64 and Windows x64 were built, tested, packaged,
and installer-tested on native hosted runners. Version
0.1.2 was the initial published release. The
unpublished `v0.1.0`, `v0.1.1`, and `v0.1.9`
build tags were retained rather than moved after their native release gates
exposed, respectively, a macOS portability error, an incremental
terminal-repaint race, and a Linux-only managed-Pi assumption in a macOS PTY
test. The unpublished `v0.1.11` tag is likewise retained after its ARM Linux
runner exposed an insecure shared-state initialization order under a `0022`
umask; v0.1.12 fixes it and tests that umask explicitly. A version tag alone is
not sufficient:
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
open-agent-view-VERSION-x86_64-pc-windows-msvc.zip
open-agent-view-VERSION-x86_64-pc-windows-msvc.zip.sha256
```

## Manual native release procedure

After the full local gate below, create the same deterministic package shape as
the native workflow:

```console
target=x86_64-unknown-linux-gnu
cargo build --release --locked --target "$target"
scripts/package-release.sh "$target"
```

Run the same two commands on a native macOS builder with
`aarch64-apple-darwin`. Add the `x86_64-apple-darwin` Rust target, build it on
Apple silicon, and execute the packaged binary through Rosetta before
publication. `scripts/package-release.sh` uses GNU tar's reproducibility flags
on Linux and disables AppleDouble metadata when packaging with BSD tar.

Extract and smoke-test the archive, test `install.sh` or `install.ps1` against a temporary local
release root, create and push an annotated version tag, then publish exactly the
verified archive and checksum with `gh release create`. Never
upload an untested cross-compiled artifact merely to fill the matrix.
Unix archives contain the canonical `open-agent-view` executable and the
installer creates a relative `oav` shorthand plus the legacy `opav`
compatibility symlink after version verification. Windows archives contain
`open-agent-view.exe`; the PowerShell installer copies the verified executable
to `oav.exe` and the legacy `opav.exe` name because ordinary Windows
installations cannot rely on developer-mode symlinks.

## Publication policy

GitHub Actions runs read-only quality, test, portability, provider-setup, and
website gates, but it does not publish releases or the website. Release
artifacts are built, smoke-tested, checksum-verified, and uploaded manually by
the maintainer from the exact reviewed commit. Pages is exported, tested, and
pushed manually with [`scripts/publish-site.sh`](../scripts/publish-site.sh).

The README deliberately uses repository-owned status badges. Its **Tests**
badge links to the complete evidence record in
[`docs/testing.md`](testing.md), and its static release badge must match the
crate version. The GitHub Actions badge would describe hosted-runner
availability rather than the documented manual release gate when jobs are
rejected before startup. A regression test keeps that endpoint out of the
README and prevents the release badge from drifting behind the package version.

The current manual Linux builder establishes the documented glibc 2.35 floor.
Older GNU/Linux systems and Windows ARM64 are not release targets yet. Each
installer fails clearly instead of downloading an incompatible binary.

## Prepare a release

Before creating a tag, complete the
[cross-platform testing procedure](cross-platform-testing.md) on the exact
reviewed commit. Its native-runner, artifact-provenance, and public-install
checks are release requirements, not interchangeable simulations. The core
local commands are:

```console
cargo fmt --all -- --check
cargo clippy --all-targets --locked -- -D warnings
cargo test --locked
scripts/real-tui-tests.sh
scripts/test-installer.sh
# On a native Windows x64 runner:
.\scripts\test-installer.ps1
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

1. complete the release gates above and record the evidence in
   [`docs/testing.md`](testing.md);
2. update [`CHANGELOG.md`](../CHANGELOG.md);
3. set the intended version in `Cargo.toml` and `Cargo.lock`;
4. review the exact release commit; and
5. obtain maintainer approval to publish.

From the approved commit, create and push the immutable annotated tag, then
publish only the locally verified files:

```console
git tag -a vMAJOR.MINOR.PATCH -m "open-agent-view vMAJOR.MINOR.PATCH"
git push origin vMAJOR.MINOR.PATCH
gh release create vMAJOR.MINOR.PATCH \
  dist/open-agent-view-MAJOR.MINOR.PATCH-x86_64-unknown-linux-gnu.tar.gz \
  dist/open-agent-view-MAJOR.MINOR.PATCH-x86_64-unknown-linux-gnu.tar.gz.sha256 \
  --repo xhluca/open-agent-view \
  --generate-notes \
  --title "Open Agent View vMAJOR.MINOR.PATCH"
```

The installed `gh` version may not support `--verify-tag`. Before creating the
release, verify the annotated tag and its peeled commit explicitly with
`git ls-remote --tags origin refs/tags/vVERSION refs/tags/vVERSION^{}`.

Publication never creates or moves a tag from an unreviewed branch build. Do
not retry a failed release by moving an existing tag; fix the cause and choose
a new version.
Annotated tags are the current repository convention. Moving to signed tags
requires a valid, non-expired maintainer signing key and a documented public-key
verification path; do not claim a signature when those prerequisites are absent.

## Verify a published release

Confirm the GitHub release contains the original verified archive and checksum,
then exercise both authenticated and public installation paths as applicable:

```console
OAV_VERSION=MAJOR.MINOR.PATCH ./install.sh
open-agent-view --version
oav --version
opav --version # legacy compatibility check
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

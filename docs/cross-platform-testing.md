# Cross-platform testing

This is the reproducible acceptance procedure for Linux, macOS, and Windows.
Use it when changing platform code, installers, packaging, terminal behavior, or
release artifacts. The CI definition in [`.github/workflows/ci.yml`](../.github/workflows/ci.yml)
is the executable source of truth; this guide explains what each gate proves and
what it does not prove.

The historical results and exact release evidence live in
[`testing.md`](testing.md). Keep procedure here and dated results there.

## Supported release matrix

| Platform | Release target | Required acceptance environment | Useful supplemental check |
| --- | --- | --- | --- |
| Linux x86-64 | `x86_64-unknown-linux-gnu` | Native Ubuntu x86-64 CI | Fresh Ubuntu container install |
| Linux ARM64 | `aarch64-unknown-linux-gnu` | Native Ubuntu ARM64 CI | ARM64 container on an ARM64 host |
| macOS Intel | `x86_64-apple-darwin` | Native Intel macOS CI | Run the Intel archive under Rosetta |
| macOS Apple silicon | `aarch64-apple-darwin` | Native Apple-silicon macOS CI | Isolated install on a maintained Mac |
| Windows x64 | `x86_64-pc-windows-msvc` | Native `windows-latest` CI in PowerShell | Windows GNU cross-compile |

Do not replace a native gate with a simulation:

- Docker shares its host kernel. It is excellent for a clean Linux filesystem,
  but it does not test macOS, Windows, ConPTY, PowerShell, or native executable
  loading.
- WSL is a supported way to run the Linux release on Windows; it is not a test
  of the native Windows release.
- Rosetta proves the Intel archive can execute on Apple silicon, but native
  Intel CI remains the Intel acceptance gate.
- Cross-compilation and Wine can catch compile or packaging errors, but native
  Windows CI remains the Windows acceptance gate.

If a required native runner is unavailable, record the missing gate and do not
describe that platform as release-verified.

## 1. Test the reviewed source

Start from a clean worktree and record the exact commit:

```console
git status --short
git rev-parse HEAD
cargo fmt --all -- --check
cargo clippy --all-targets --locked -- -D warnings
cargo test --locked
scripts/real-tui-tests.sh
scripts/test-installer.sh
```

The real-TTY suite uses disposable provider fixtures by default. Credentialed
provider probes are opt-in; an ignored credentialed test is not a passing live
provider test. Follow [`tui-validation.md`](tui-validation.md) for the exhaustive
keyboard route, populated-session fixture, and visual acceptance criteria.

For website changes, also run:

```console
cd website
npm ci --no-audit
npm audit --omit=dev --audit-level=high
npm run lint
npm test
npm run test:visual
npm run export
```

### Pinned Linux toolchain in Docker

This is useful when the host does not have the project's minimum Rust toolchain
or its `rustfmt` and Clippy components. It is a Linux check only:

```console
docker run --rm \
  -e CARGO_TARGET_DIR=/tmp/cargo-target \
  -v "$PWD:/workspace:ro" \
  -w /workspace \
  rust:1.75 \
  sh -c 'rustup component add rustfmt clippy &&
    cargo fmt --all -- --check &&
    cargo clippy --all-targets --locked -- -D warnings'
```

The repository is mounted read-only and build output remains inside the
container. Run the full native matrix after this supplemental check.

## 2. Require the native CI matrix

The `ci` workflow must pass on the exact reviewed commit. Its platform jobs
perform these checks:

- native Ubuntu x86-64 and ARM64: locked tests, release build, Unix installer
  tests, packaging, and artifact upload;
- native macOS Intel and Apple silicon: the same Unix gates on each native
  architecture;
- native Windows x64: locked tests, MSVC release build, startup without `HOME`,
  PowerShell installer tests, packaging, and artifact upload;
- Rust 1.75 and stable: locked tests and release builds; and
- website: audit, lint, unit tests, visual browser tests, and static export.

Verify the run rather than relying on a branch badge:

```console
repo=xhluca/open-agent-view
commit="$(git rev-parse HEAD)"
gh run list --repo "$repo" --commit "$commit" --workflow ci.yml
gh run view RUN_ID --repo "$repo"
```

The run's head SHA must equal `$commit`, every required job must be successful,
and the artifacts must come from that run. A later green run on another commit
does not validate the release commit.

## 3. Verify release artifacts

Download the artifacts from the successful run into a new temporary directory:

```console
repo=xhluca/open-agent-view
stage="$(mktemp -d)"
gh run download RUN_ID --repo "$repo" --dir "$stage"
find "$stage" -type f -print | sort
```

There must be one archive and one adjacent `.sha256` file for every target in
the matrix. Verify every checksum before inspecting or publishing an archive:

```console
find "$stage" -name '*.sha256' -print0 |
  while IFS= read -r -d '' checksum; do
    directory="$(dirname "$checksum")"
    filename="$(basename "$checksum")"
    if command -v sha256sum >/dev/null 2>&1; then
      (cd "$directory" && sha256sum -c "$filename")
    else
      (cd "$directory" && shasum -a 256 -c "$filename")
    fi
  done
```

Use `tar -tzf ARCHIVE` for Unix archives and `unzip -Z1 ARCHIVE` for the
Windows ZIP. Each archive must contain one top-level versioned directory and
the canonical executable (`open-agent-view` or `open-agent-view.exe`).

Before publication, enforce the exact-commit invariant:

1. the reviewed source commit is the successful CI run's head SHA;
2. all uploaded archives are the checksum-verified CI artifacts from that run;
3. the annotated version tag peels to that same SHA; and
4. the release is created from that immutable tag.

Never rebuild an artifact after approval and upload it under the old checksum.
Never move a failed or published version tag; fix the issue and use a new
version.

## 4. Test the public installer in clean environments

Pre-publication installer tests use a local release directory and are covered
by `scripts/test-installer.sh` and `scripts/test-installer.ps1`. After
publication, repeat the install through the public GitHub URL so the download,
release naming, checksum asset, extraction, aliases, and startup path are all
exercised together.

Set `VERSION` to the published version without the leading `v`.

### Fresh Linux x86-64 container

```console
VERSION=MAJOR.MINOR.PATCH
docker run --rm -e OAV_TEST_VERSION="$VERSION" ubuntu:22.04 bash -lc '
  set -eu
  apt-get update -qq
  DEBIAN_FRONTEND=noninteractive apt-get install -y -qq ca-certificates curl >/dev/null
  mkdir -p /tmp/oav-bin
  curl -fsSL https://raw.githubusercontent.com/xhluca/open-agent-view/main/install.sh \
    -o /tmp/install-oav.sh
  OAV_VERSION="$OAV_TEST_VERSION" bash /tmp/install-oav.sh --install-dir /tmp/oav-bin
  /tmp/oav-bin/open-agent-view --version
  /tmp/oav-bin/oav --version
  /tmp/oav-bin/open-agent-view --json --no-host-providers
'
```

The version commands must report the requested version. The final command must
return valid JSON with empty `sessions` and `warnings`. This is a clean install
test, not a substitute for the native Linux ARM64 job or for real-TTY tests.

### Native macOS

Run this locally on the Mac architecture being checked, or send the same script
to a controlled Mac over SSH. Do not put a developer-specific host alias in the
project instructions.

```console
VERSION=MAJOR.MINOR.PATCH
test_root="$(mktemp -d)"
trap 'rm -rf -- "$test_root"' EXIT HUP INT TERM
curl -fsSL https://raw.githubusercontent.com/xhluca/open-agent-view/main/install.sh \
  -o "$test_root/install.sh"
OAV_VERSION="$VERSION" bash "$test_root/install.sh" --install-dir "$test_root/bin"
uname -s -m
"$test_root/bin/open-agent-view" --version
"$test_root/bin/oav" --version
"$test_root/bin/open-agent-view" --json --no-host-providers
```

Run on Apple silicon for the ARM archive and rely on the native Intel CI job for
the Intel release gate. An additional Rosetta run of the Intel archive is
useful, but does not replace native Intel CI.

### Native Windows x64 PowerShell

Run in ordinary PowerShell on native x64 Windows:

```powershell
$Version = "MAJOR.MINOR.PATCH"
$Root = Join-Path $env:TEMP ("oav-release-" + [guid]::NewGuid())
$Bin = Join-Path $Root "bin"
try {
    New-Item -ItemType Directory -Path $Root -Force | Out-Null
    $Installer = Join-Path $Root "install.ps1"
    Invoke-WebRequest `
        -UseBasicParsing `
        -Uri "https://raw.githubusercontent.com/xhluca/open-agent-view/main/install.ps1" `
        -OutFile $Installer
    & $Installer -Version $Version -InstallDir $Bin -SkipPathUpdate
    & (Join-Path $Bin "open-agent-view.exe") --version
    & (Join-Path $Bin "oav.exe") --version
    $Snapshot = & (Join-Path $Bin "open-agent-view.exe") --json --no-host-providers |
        ConvertFrom-Json
    if ($Snapshot.sessions.Count -ne 0 -or $Snapshot.warnings.Count -ne 0) {
        throw "unexpected provider-free startup output"
    }
} finally {
    Remove-Item -LiteralPath $Root -Recurse -Force -ErrorAction SilentlyContinue
}
```

This must use the published MSVC ZIP. A passing MinGW cross-compile, Docker
container, WSL run, or Wine run is supplemental and cannot replace it.

## 5. Isolate state and credentials

Cross-platform tests must not mutate a maintainer's real sessions by default.

- Use a new temporary install directory.
- Use temporary `HOME`, `USERPROFILE`, and `XDG_STATE_HOME` values when a test
  reads or writes user state.
- Use `--no-host-providers` for the provider-free startup smoke.
- Use fixture executables and disposable provider data for destructive session
  actions.
- Never copy tokens, browser cookies, keychains, OAuth databases, or session
  secrets into a container or CI artifact.
- Run live authenticated provider tests only when the test explicitly opts in,
  the account and workspace are intended for testing, and cleanup is defined.

Provider installation, login, model discovery, and native-TUI behavior require
their own live test on each claimed platform. A mock verifies OAV's protocol and
error handling; it does not prove that a current provider release still accepts
the same login or model-selection flow.

## 6. Record evidence

For every release, add a dated entry to [`testing.md`](testing.md) with:

- exact Git commit and version;
- successful CI run URL and head SHA;
- native runners and architectures used;
- test counts, ignored opt-in tests, and failures retried;
- archive names and checksum-verification result;
- fresh public-install results for Linux, macOS, and Windows;
- provider CLI versions for any live authenticated probes; and
- explicit gaps, including any platform or credentialed flow not exercised.

That record makes the result auditable. This guide makes the procedure
repeatable.

#!/usr/bin/env bash

set -euo pipefail

repo_dir="$(CDPATH= cd -- "$(dirname -- "$0")/.." && pwd)"
temp_root="$(mktemp -d "${TMPDIR:-/tmp}/open-agent-view-installer-test.XXXXXX")"
trap 'rm -rf -- "$temp_root"' EXIT HUP INT TERM

version="0.1.0"
tag="v${version}"
target="x86_64-unknown-linux-gnu"
stem="open-agent-view-${version}-${target}"
archive="${stem}.tar.gz"
release_dir="${temp_root}/releases/${tag}"

fail() {
  printf 'installer test failed: %s\n' "$*" >&2
  exit 1
}

make_release() {
  local root="$1"
  install -d "${root}/${stem}"
  cat >"${root}/${stem}/coding-agents" <<EOF
#!/usr/bin/env sh
if [ "\${1:-}" = "--version" ]; then
  echo "coding-agents ${version}"
  exit 0
fi
echo fixture-binary
EOF
  chmod 0755 "${root}/${stem}/coding-agents"
  tar -C "$root" -czf "${root}/${archive}" "$stem"
  (
    cd "$root"
    sha256sum "$archive" >"${archive}.sha256"
  )
}

install -d "$release_dir"
make_release "$release_dir"

home="${temp_root}/home"
output="$({
  HOME="$home" \
    PATH="/usr/bin:/bin" \
    OAV_VERSION="$version" \
    OAV_RELEASE_BASE_URL="file://${temp_root}/releases" \
    bash "${repo_dir}/install.sh"
} 2>&1)"
[[ -x "${home}/.local/bin/coding-agents" ]] || fail "default binary was not installed"
[[ "$("${home}/.local/bin/coding-agents" --version)" == "coding-agents ${version}" ]] ||
  fail "default binary reports the wrong version"
[[ "$output" == *"installed coding-agents ${version}"* ]] || fail "success output is missing"
[[ "$output" == *"add ${home}/.local/bin to PATH"* ]] || fail "PATH guidance is missing"

custom_bin="${temp_root}/custom/bin"
PATH="${custom_bin}:/usr/bin:/bin" \
  OAV_RELEASE_BASE_URL="file://${temp_root}/releases" \
  bash "${repo_dir}/install.sh" --version "v${version}" --install-dir "$custom_bin" >/dev/null
[[ -x "${custom_bin}/coding-agents" ]] || fail "custom install directory was ignored"

printf 'old binary\n' >"${custom_bin}/coding-agents"
chmod 0755 "${custom_bin}/coding-agents"
cp "${release_dir}/${archive}.sha256" "${release_dir}/${archive}.sha256.good"
printf '%064d  %s\n' 0 "$archive" >"${release_dir}/${archive}.sha256"
if OAV_VERSION="$version" \
  OAV_INSTALL_DIR="$custom_bin" \
  OAV_RELEASE_BASE_URL="file://${temp_root}/releases" \
  bash "${repo_dir}/install.sh" >"${temp_root}/checksum.out" 2>&1; then
  fail "a bad checksum was accepted"
fi
grep -F "checksum verification failed" "${temp_root}/checksum.out" >/dev/null ||
  fail "checksum failure was not explained"
grep -F "old binary" "${custom_bin}/coding-agents" >/dev/null ||
  fail "failed installation replaced the existing binary"
mv "${release_dir}/${archive}.sha256.good" "${release_dir}/${archive}.sha256"

if _OAV_TEST_UNAME_S=Darwin \
  OAV_VERSION="$version" \
  OAV_RELEASE_BASE_URL="file://${temp_root}/releases" \
  bash "${repo_dir}/install.sh" >"${temp_root}/platform.out" 2>&1; then
  fail "an unsupported platform was accepted"
fi
grep -F "no prebuilt release is available for Darwin" "${temp_root}/platform.out" >/dev/null ||
  fail "unsupported platform failure was not explained"

fake_bin="${temp_root}/fake-bin"
install -d "$fake_bin"
cat >"${fake_bin}/gh" <<'EOF'
#!/usr/bin/env bash
set -euo pipefail
case "${1:-} ${2:-}" in
  "auth status") exit 0 ;;
  "api repos/xhluca/open-agent-view/releases/latest") printf 'v0.1.0\n' ;;
  "release download")
    destination=""
    shift 2
    while (($#)); do
      case "$1" in
        --dir) destination="$2"; shift 2 ;;
        *) shift ;;
      esac
    done
    find "${OAV_TEST_RELEASE_DIR}" -maxdepth 1 -type f -exec cp {} "$destination/" \;
    ;;
  *) printf 'unexpected gh invocation: %s\n' "$*" >&2; exit 1 ;;
esac
EOF
chmod 0755 "${fake_bin}/gh"
private_home="${temp_root}/private-home"
PATH="${fake_bin}:/usr/bin:/bin" \
  HOME="$private_home" \
  OAV_TEST_RELEASE_DIR="$release_dir" \
  bash "${repo_dir}/install.sh" >/dev/null
[[ -x "${private_home}/.local/bin/coding-agents" ]] ||
  fail "authenticated private-release path did not install"

bash "${repo_dir}/install.sh" --help | grep -F "never installs Rust" >/dev/null ||
  fail "installer help does not state the no-Rust behavior"

printf 'installer tests passed\n'

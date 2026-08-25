#!/usr/bin/env bash

set -euo pipefail

repo="${OAV_REPO:-xhluca/open-agent-view}"
requested_version="${OAV_VERSION:-latest}"
install_dir="${OAV_INSTALL_DIR:-${HOME}/.local/bin}"
release_base_url="${OAV_RELEASE_BASE_URL:-}"
github_api_url="${OAV_GITHUB_API_URL:-https://api.github.com}"

say() {
  printf 'open-agent-view: %s\n' "$*"
}

fail() {
  printf 'open-agent-view: error: %s\n' "$*" >&2
  exit 1
}

usage() {
  cat <<'EOF'
Install Open Agent View from a verified GitHub release.

Usage: install.sh [OPTIONS]

Options:
  --version VERSION      Install a specific version (for example, 0.1.12)
  --install-dir DIR      Install directory (default: ~/.local/bin)
  --repo OWNER/REPO      GitHub repository (default: xhluca/open-agent-view)
  -h, --help             Show this help

Environment variables:
  OAV_VERSION            Version to install, or "latest"
  OAV_INSTALL_DIR        Installation directory
  OAV_REPO               GitHub repository
  GH_TOKEN               Token used by gh or curl for a private repository

The installer downloads a prebuilt archive and verifies its SHA-256 checksum.
It installs open-agent-view plus the opav shorthand symlink.
It never installs Rust, invokes Cargo, or edits shell configuration files.
EOF
}

while (($#)); do
  case "$1" in
    --version)
      (($# >= 2)) || fail "--version requires a value"
      requested_version="$2"
      shift 2
      ;;
    --install-dir)
      (($# >= 2)) || fail "--install-dir requires a value"
      install_dir="$2"
      shift 2
      ;;
    --repo)
      (($# >= 2)) || fail "--repo requires a value"
      repo="$2"
      shift 2
      ;;
    -h | --help)
      usage
      exit 0
      ;;
    *)
      fail "unknown option: $1 (try --help)"
      ;;
  esac
done

[[ "$repo" =~ ^[A-Za-z0-9_.-]+/[A-Za-z0-9_.-]+$ ]] ||
  fail "repository must have the form OWNER/REPO"
[[ -n "$install_dir" ]] || fail "installation directory cannot be empty"

for command in curl tar install ln mktemp mv readlink; do
  command -v "$command" >/dev/null 2>&1 || fail "required command not found: $command"
done

os="${_OAV_TEST_UNAME_S:-$(uname -s)}"
architecture="${_OAV_TEST_UNAME_M:-$(uname -m)}"

case "${os}/${architecture}" in
  Linux/x86_64 | Linux/amd64) target="x86_64-unknown-linux-gnu" ;;
  Linux/aarch64 | Linux/arm64) target="aarch64-unknown-linux-gnu" ;;
  Darwin/x86_64 | Darwin/amd64) target="x86_64-apple-darwin" ;;
  Darwin/arm64 | Darwin/aarch64) target="aarch64-apple-darwin" ;;
  *) fail "no prebuilt release is available for ${os}/${architecture}; see docs/install.md for supported platforms" ;;
esac

gh_is_authenticated=false
if [[ -z "$release_base_url" ]] && command -v gh >/dev/null 2>&1; then
  if gh auth status --hostname github.com >/dev/null 2>&1; then
    gh_is_authenticated=true
  fi
fi

curl_args=(--fail --silent --show-error --location --retry 3 --proto '=https,file' --tlsv1.2)
if [[ -n "${GH_TOKEN:-}" ]]; then
  curl_args+=(--header "Authorization: Bearer ${GH_TOKEN}")
  curl_args+=(--header 'X-GitHub-Api-Version: 2022-11-28')
fi

if [[ "$requested_version" == "latest" ]]; then
  if [[ "$gh_is_authenticated" == true ]]; then
    tag="$(gh api "repos/${repo}/releases/latest" --jq .tag_name 2>/dev/null)" ||
      fail "no release found for ${repo}; the first release must be published before binary installation works"
  else
    release_json="$(curl "${curl_args[@]}" "${github_api_url}/repos/${repo}/releases/latest")" ||
      fail "no release found for ${repo}; authenticate with 'gh auth login' if the repository is private"
    tag="$(printf '%s\n' "$release_json" | sed -n 's/.*"tag_name"[[:space:]]*:[[:space:]]*"\([^"]*\)".*/\1/p' | head -n 1)"
    [[ -n "$tag" ]] || fail "the latest release response did not contain a tag"
  fi
else
  tag="v${requested_version#v}"
fi

version="${tag#v}"
[[ "$tag" =~ ^v[0-9]+\.[0-9]+\.[0-9]+$ ]] ||
  fail "release version must have the form MAJOR.MINOR.PATCH (received: ${tag})"

if [[ ( "$version" == "0.1.13" || "$version" == "0.1.14" || "$version" == "0.1.15" || "$version" == "0.1.16" || "$version" == "0.1.17" || "$version" == "0.1.18" || "$version" == "0.1.19" || "$version" == "0.1.20" || "$version" == "0.1.21" || "$version" == "0.1.22" || "$version" == "0.1.23" || "$version" == "0.1.24" || "$version" == "0.1.25" || "$version" == "0.1.26" || "$version" == "0.1.27" || "$version" == "0.1.28" || "$version" == "0.1.29" || "$version" == "0.1.30" || "$version" == "0.1.31" || "$version" == "0.1.32" ) && "$target" != "x86_64-unknown-linux-gnu" ]]; then
  fail "v${version} was manually published only for Linux x86_64; use a source build on ${target} or install a release that provides that target"
fi

stem="open-agent-view-${version}-${target}"
archive="${stem}.tar.gz"
checksum="${archive}.sha256"

temp_dir="$(mktemp -d "${TMPDIR:-/tmp}/open-agent-view-install.XXXXXX")"
staged_binary=""
cleanup() {
  if [[ -n "$staged_binary" && -e "$staged_binary" ]]; then
    rm -f -- "$staged_binary"
  fi
  rm -rf -- "$temp_dir"
}
trap cleanup EXIT HUP INT TERM

say "downloading ${tag} for ${target}"
if [[ -n "$release_base_url" ]]; then
  base="${release_base_url%/}/${tag}"
  curl "${curl_args[@]}" --output "${temp_dir}/${archive}" "${base}/${archive}"
  curl "${curl_args[@]}" --output "${temp_dir}/${checksum}" "${base}/${checksum}"
elif [[ "$gh_is_authenticated" == true ]]; then
  gh release download "$tag" \
    --repo "$repo" \
    --pattern "$archive" \
    --pattern "$checksum" \
    --dir "$temp_dir"
else
  base="https://github.com/${repo}/releases/download/${tag}"
  curl "${curl_args[@]}" --output "${temp_dir}/${archive}" "${base}/${archive}"
  curl "${curl_args[@]}" --output "${temp_dir}/${checksum}" "${base}/${checksum}"
fi

expected_checksum="$(awk 'NR == 1 { print $1 }' "${temp_dir}/${checksum}")"
[[ "$expected_checksum" =~ ^[0-9a-fA-F]{64}$ ]] || fail "release checksum file is malformed"

if command -v sha256sum >/dev/null 2>&1; then
  actual_checksum="$(sha256sum "${temp_dir}/${archive}" | awk '{ print $1 }')"
elif command -v shasum >/dev/null 2>&1; then
  actual_checksum="$(shasum -a 256 "${temp_dir}/${archive}" | awk '{ print $1 }')"
else
  fail "sha256sum or shasum is required to verify the release"
fi

[[ "$actual_checksum" == "$expected_checksum" ]] || fail "release checksum verification failed"

tar -xzf "${temp_dir}/${archive}" -C "$temp_dir"
binary="${temp_dir}/${stem}/open-agent-view"
[[ -f "$binary" && -x "$binary" ]] || fail "release archive does not contain ${stem}/open-agent-view"

install -d "$install_dir"
staged_binary="${install_dir}/.open-agent-view.install.$$"
install -m 0755 "$binary" "$staged_binary"
mv -f -- "$staged_binary" "${install_dir}/open-agent-view"
staged_binary=""

installed_version="$("${install_dir}/open-agent-view" --version 2>/dev/null)" ||
  fail "the installed binary could not be executed"
[[ "$installed_version" == "open-agent-view ${version}" ]] ||
  fail "installed binary reported an unexpected version: ${installed_version}"

install_alias() {
  local alias="$1"
  local destination="${install_dir}/${alias}"
  if [[ -e "$destination" || -L "$destination" ]]; then
    local replace_existing=false
    if [[ -L "$destination" && "$(readlink "$destination")" == "open-agent-view" ]]; then
      replace_existing=true
    fi
    if [[ "$replace_existing" != true ]]; then
      say "left unrelated existing command in place: ${destination}"
      return
    fi
  fi
  staged_binary="${install_dir}/.${alias}.install.$$"
  ln -s "open-agent-view" "$staged_binary"
  mv -f -- "$staged_binary" "$destination"
  staged_binary=""
}

install_alias opav

# Retire only the exact relative compatibility symlink created by older OAV
# installers. Never remove an unrelated file, executable, or differently
# targeted symlink at this path.
obsolete_alias="${install_dir}/coding-agents"
if [[ -L "$obsolete_alias" && "$(readlink "$obsolete_alias")" == "open-agent-view" ]]; then
  rm -f -- "$obsolete_alias"
fi

say "installed open-agent-view ${version} to ${install_dir}/open-agent-view"
say "installed shorthand: opav"
case ":${PATH}:" in
  *":${install_dir}:"*) ;;
  *) say "add ${install_dir} to PATH, then run: open-agent-view (or opav)" ;;
esac

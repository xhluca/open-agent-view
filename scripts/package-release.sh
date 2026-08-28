#!/usr/bin/env bash

set -euo pipefail

repo_dir="$(CDPATH='' cd -- "$(dirname -- "$0")/.." && pwd)"
target="${1:-}"

case "$target" in
  x86_64-unknown-linux-gnu | aarch64-unknown-linux-gnu | x86_64-apple-darwin | aarch64-apple-darwin) ;;
  *)
    printf 'usage: %s TARGET [BINARY]\n' "$0" >&2
    printf 'supported targets: x86_64-unknown-linux-gnu, aarch64-unknown-linux-gnu, x86_64-apple-darwin, aarch64-apple-darwin\n' >&2
    exit 2
    ;;
esac

version="$(sed -n 's/^version = "\([^"]*\)"/\1/p' "${repo_dir}/Cargo.toml" | head -n 1)"
[[ "$version" =~ ^[0-9]+\.[0-9]+\.[0-9]+$ ]] || {
  printf 'could not read a release version from Cargo.toml\n' >&2
  exit 1
}

binary="${2:-${repo_dir}/target/${target}/release/open-agent-view}"
[[ -f "$binary" && -x "$binary" ]] || {
  printf 'release binary is missing or not executable: %s\n' "$binary" >&2
  exit 1
}

reported_version="$("$binary" --version)"
[[ "$reported_version" == "open-agent-view ${version}" ]] || {
  printf 'release binary reports an unexpected version: %s\n' "$reported_version" >&2
  exit 1
}

stem="open-agent-view-${version}-${target}"
archive="${stem}.tar.gz"
dist_dir="${OAV_DIST_DIR:-${repo_dir}/dist}"
[[ -n "$dist_dir" ]] || {
  printf 'release output directory cannot be empty\n' >&2
  exit 1
}
stage_root="$(mktemp -d "${TMPDIR:-/tmp}/open-agent-view-package.XXXXXX")"
trap 'rm -rf -- "$stage_root"' EXIT HUP INT TERM

install -d "${stage_root}/${stem}" "$dist_dir"
install -m 0755 "$binary" "${stage_root}/${stem}/open-agent-view"
install -m 0644 "${repo_dir}/LICENSE" "${repo_dir}/README.md" "${stage_root}/${stem}/"

if tar --version 2>/dev/null | grep -q 'GNU tar'; then
  source_date_epoch="$(git -C "$repo_dir" show -s --format=%ct HEAD)"
  tar --sort=name --mtime="@${source_date_epoch}" --owner=0 --group=0 \
    --numeric-owner -C "$stage_root" -czf "${stage_root}/${archive}" "$stem"
else
  COPYFILE_DISABLE=1 tar -C "$stage_root" -czf "${stage_root}/${archive}" "$stem"
fi

if command -v sha256sum >/dev/null 2>&1; then
  (cd "$stage_root" && sha256sum "$archive" >"${archive}.sha256")
elif command -v shasum >/dev/null 2>&1; then
  (cd "$stage_root" && shasum -a 256 "$archive" >"${archive}.sha256")
else
  printf 'sha256sum or shasum is required to package a release\n' >&2
  exit 1
fi

install -m 0644 "${stage_root}/${archive}" "${dist_dir}/${archive}"
install -m 0644 "${stage_root}/${archive}.sha256" "${dist_dir}/${archive}.sha256"
printf '%s\n%s\n' "${dist_dir}/${archive}" "${dist_dir}/${archive}.sha256"

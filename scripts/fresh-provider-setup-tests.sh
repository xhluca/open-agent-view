#!/usr/bin/env bash

set -euo pipefail

repo_root="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
image="${OAV_SETUP_IMAGE:-node@sha256:83f487e0a63425e5b4d146fb5e5be574bcbe1b7b843d3ebafdd95eaf7767a7e5}"
binary="${repo_root}/target/release/open-agent-view"
guest="${repo_root}/scripts/fresh-provider-setup-case.sh"
if (( $# > 0 )); then
  providers=("$@")
else
  providers=(claude codex pi opencode cursor copilot antigravity mistral-vibe muse qwen kimi)
fi
staging=""

command -v docker >/dev/null 2>&1 || { printf 'docker is required\n' >&2; exit 1; }
cargo build --release --locked
docker image inspect "$image" >/dev/null 2>&1 || docker pull "$image"

cleanup() {
  if [[ -n "$staging" && -d "$staging" ]]; then
    rm -rf -- "$staging"
  fi
  for provider in "${providers[@]}"; do
    docker rm -f "oav-setup-${provider}-$$" >/dev/null 2>&1 || true
  done
}
trap cleanup EXIT HUP INT TERM

staging="$(mktemp -d "${TMPDIR:-/tmp}/oav-provider-setup.XXXXXX")"
install -m 0755 "$binary" "${staging}/open-agent-view"
install -m 0755 "$guest" "${staging}/setup-case.sh"

for provider in "${providers[@]}"; do
  docker run --rm \
    --name "oav-setup-${provider}-$$" \
    --network bridge \
    --security-opt no-new-privileges:true \
    --pids-limit 256 \
    --tmpfs /tmp:rw,exec,nosuid,nodev,size=3g \
    --volume "${staging}/open-agent-view:/usr/local/bin/open-agent-view:ro" \
    --volume "${staging}/setup-case.sh:/usr/local/bin/setup-case.sh:ro" \
    "$image" \
    bash -lc 'apt-get update -qq && apt-get install -y -qq ca-certificates curl git less procps unzip util-linux >/dev/null && exec /usr/local/bin/setup-case.sh "$1"' _ "$provider"
done

for provider in "${providers[@]}"; do
  if docker ps -a --format '{{.Names}}' | grep -Fx "oav-setup-${provider}-$$" >/dev/null; then
    printf 'setup container was not removed: %s\n' "$provider" >&2
    exit 1
  fi
done

printf 'all fresh provider installers and native PTY login handoffs passed\n'

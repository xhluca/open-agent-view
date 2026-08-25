#!/usr/bin/env bash

set -euo pipefail

repo_root="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
image="${OAV_DEMO_IMAGE:-sha256:8f170f660813ac358f347fa8a3580139972f3ea7a9fb087834f1da44669d9392}"
output_dir="${OAV_DEMO_OUTPUT_DIR:-${repo_root}/website/public}"
container="oav-site-demo-$$"
cast="${output_dir}/oav-demo.cast"
gif="${output_dir}/oav-demo.gif"
video="${output_dir}/oav-demo.mp4"
poster="${output_dir}/oav-demo.png"
binary="${repo_root}/target/release/open-agent-view"
fixture="${repo_root}/fixtures/all-providers-sessions.json"
staging=""

for command in docker asciinema agg expect ffmpeg; do
  command -v "$command" >/dev/null 2>&1 || {
    printf 'missing required demo tool: %s\n' "$command" >&2
    exit 1
  }
done

cargo build --release --locked
mkdir -p "$output_dir"
docker image inspect "$image" >/dev/null

cleanup() {
  docker rm -f "$container" >/dev/null 2>&1 || true
  if [[ -n "$staging" && -d "$staging" ]]; then
    rm -rf -- "$staging"
  fi
}
trap cleanup EXIT HUP INT TERM

# The repository is intentionally private by default (umask 077). Stage exact
# copies so the unprivileged container can read only these two demo inputs.
staging="$(mktemp -d "${TMPDIR:-/tmp}/oav-site-demo.XXXXXX")"
install -m 0755 "$binary" "${staging}/open-agent-view"
install -m 0644 "$fixture" "${staging}/sessions.json"
binary="${staging}/open-agent-view"
fixture="${staging}/sessions.json"

asciinema rec \
  --overwrite \
  --quiet \
  --cols 150 \
  --rows 42 \
  --idle-time-limit 2 \
  --command "expect '${repo_root}/scripts/capture-site-demo.exp' '${binary}' '${fixture}' '${image}' '${container}'" \
  "$cast"

agg \
  --theme github-dark \
  --font-size 13 \
  --idle-time-limit 2 \
  --last-frame-duration 2 \
  "$cast" "$gif"

ffmpeg -hide_banner -loglevel error -y \
  -i "$gif" \
  -vf "pad=ceil(iw/2)*2:ceil(ih/2)*2" \
  -movflags +faststart \
  -pix_fmt yuv420p \
  "$video"

ffmpeg -hide_banner -loglevel error -y \
  -sseof -1.5 \
  -i "$video" \
  -frames:v 1 \
  "$poster"

cp "$poster" "${output_dir}/open-agent-view.png"
chmod 0644 "$cast" "$gif" "$video" "$poster" "${output_dir}/open-agent-view.png"

docker ps -a --format '{{.Names}}' | grep -Fx "$container" && {
  printf 'demo container was not removed: %s\n' "$container" >&2
  exit 1
} || true

printf 'captured %s\n' "$video"

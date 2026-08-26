#!/usr/bin/env bash

set -euo pipefail

repo_root="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
public_dir="$repo_root/website/public"
cast="$public_dir/oav-demo.cast"
gif="$public_dir/oav-demo.gif"
video="$public_dir/oav-demo.mp4"
poster="$public_dir/oav-demo.png"

for command in python3 agg ffmpeg; do
  command -v "$command" >/dev/null 2>&1 || {
    printf 'missing required demo tool: %s\n' "$command" >&2
    exit 1
  }
done

for clip in setup claude rename; do
  test -s "$public_dir/demos/$clip.cast" || {
    printf 'missing genuine recording: %s\n' "$public_dir/demos/$clip.cast" >&2
    printf 'capture it with: python3 scripts/capture-real-site-demo.py %s\n' "$clip" >&2
    exit 1
  }
done

python3 "$repo_root/scripts/compose-readme-demo.py"

agg \
  --quiet \
  --no-loop \
  --theme github-dark \
  --font-size 16 \
  --speed 1 \
  --idle-time-limit 3 \
  --last-frame-duration 3 \
  "$cast" "$gif"

ffmpeg -hide_banner -loglevel error -y \
  -i "$gif" \
  -vf "pad=ceil(iw/2)*2:ceil(ih/2)*2" \
  -movflags +faststart \
  -pix_fmt yuv420p \
  "$video"

ffmpeg -hide_banner -loglevel error -y \
  -sseof -0.1 \
  -i "$video" \
  -frames:v 1 \
  "$poster"

cp "$gif" "$repo_root/docs/assets/open-agent-view.gif"
cp "$poster" "$repo_root/docs/assets/open-agent-view.png"
cp "$poster" "$public_dir/open-agent-view.png"
chmod 0644 \
  "$cast" "$gif" "$video" "$poster" \
  "$repo_root/docs/assets/open-agent-view.gif" \
  "$repo_root/docs/assets/open-agent-view.png" \
  "$public_dir/open-agent-view.png"

printf 'composed genuine terminal demo: %s\n' "$video"

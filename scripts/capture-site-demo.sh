#!/usr/bin/env bash

set -euo pipefail

repo_root="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
public_dir="$repo_root/website/public"
cast="$public_dir/oav-demo.cast"
gif="$public_dir/oav-demo.gif"
video="$public_dir/open-agent-view-demo.mp4"
poster="$public_dir/oav-demo.png"

for command in python3 agg ffmpeg ffprobe; do
  command -v "$command" >/dev/null 2>&1 || {
    printf 'missing required demo tool: %s\n' "$command" >&2
    exit 1
  }
done

test -s "$public_dir/demos/overview.cast" || {
  printf 'missing genuine recording: %s\n' "$public_dir/demos/overview.cast" >&2
  printf 'capture it with: python3 scripts/capture-real-site-demo.py overview\n' >&2
  exit 1
}
test -s "$public_dir/demos/overview.actions.json" || {
  printf 'missing genuine action manifest: %s\n' "$public_dir/demos/overview.actions.json" >&2
  exit 1
}

python3 "$repo_root/scripts/compose-readme-demo.py"

agg \
  --quiet \
  --no-loop \
  --theme github-dark \
  --font-size 16 \
  --speed 1 \
  --idle-time-limit 8 \
  --last-frame-duration 3 \
  "$cast" "$public_dir/oav-demo-raw.gif"

ffmpeg -hide_banner -loglevel error -y \
  -i "$public_dir/oav-demo-raw.gif" \
  -vf "fps=30,ass='$public_dir/oav-demo.ass',pad=ceil(iw/2)*2:ceil(ih/2)*2" \
  -movflags +faststart \
  -pix_fmt yuv420p \
  "$video"

video_frame_rate="$(
  ffprobe -v error -select_streams v:0 \
    -show_entries stream=avg_frame_rate -of default=nw=1:nk=1 "$video"
)"
test "$video_frame_rate" = "30/1" || {
  printf 'unexpected demo frame rate: %s (expected 30/1)\n' "$video_frame_rate" >&2
  exit 1
}

ffmpeg -hide_banner -loglevel error -y \
  -i "$video" \
  -vf "fps=15,split[s0][s1];[s0]palettegen=max_colors=128:stats_mode=diff[p];[s1][p]paletteuse=dither=bayer:bayer_scale=3:diff_mode=rectangle" \
  -loop -1 \
  "$gif"

ffmpeg -hide_banner -loglevel error -y \
  -sseof -0.1 \
  -i "$video" \
  -frames:v 1 \
  "$poster"

cp "$gif" "$repo_root/docs/assets/open-agent-view.gif"
cp "$poster" "$repo_root/docs/assets/open-agent-view.png"
cp "$poster" "$public_dir/open-agent-view.png"
rm -f "$public_dir/oav-demo-raw.gif"
chmod 0644 \
  "$cast" "$public_dir/oav-demo.ass" "$gif" "$video" "$poster" \
  "$repo_root/docs/assets/open-agent-view.gif" \
  "$repo_root/docs/assets/open-agent-view.png" \
  "$public_dir/open-agent-view.png"

printf 'composed genuine terminal demo: %s\n' "$video"

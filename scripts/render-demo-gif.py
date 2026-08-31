#!/usr/bin/env python3
"""Render a genuine terminal cast with burned-in action subtitles."""

from __future__ import annotations

import argparse
import json
import shutil
import subprocess
import tempfile
from pathlib import Path


def require_program(name: str) -> str:
    executable = shutil.which(name)
    if executable is None:
        raise RuntimeError(f"required GIF renderer is not installed: {name}")
    return executable


def run(args: list[str]) -> None:
    subprocess.run(args, check=True)


def ass_time(seconds: float) -> str:
    centiseconds = max(0, round(seconds * 100))
    hours, remainder = divmod(centiseconds, 360_000)
    minutes, remainder = divmod(remainder, 6_000)
    whole, fraction = divmod(remainder, 100)
    return f"{hours}:{minutes:02d}:{whole:02d}.{fraction:02d}"


def ass_escape(value: str) -> str:
    return value.replace("\\", r"\\").replace("{", r"\{").replace("}", r"\}")


def write_subtitles(
    path: Path,
    manifest: dict[str, object],
    width: int,
    height: int,
) -> None:
    raw_actions = manifest.get("actions")
    if not isinstance(raw_actions, list) or not raw_actions:
        raise RuntimeError("demo action manifest has no actions")
    duration = float(manifest["duration"])
    events: list[str] = []
    for index, raw_action in enumerate(raw_actions):
        if not isinstance(raw_action, dict):
            raise RuntimeError("demo action manifest contains a non-object action")
        start = float(raw_action["at"])
        next_start = (
            float(raw_actions[index + 1]["at"])
            if index + 1 < len(raw_actions)
            else duration
        )
        end = min(duration, next_start, start + 2.8)
        if end - start < 0.45:
            end = min(duration, start + 0.8)
        events.append(
            "Dialogue: 0,"
            f"{ass_time(start)},{ass_time(end)},Action,,0,0,0,,"
            f"{ass_escape(str(raw_action['action']))}"
        )

    path.write_text(
        f"""[Script Info]
ScriptType: v4.00+
PlayResX: {width}
PlayResY: {height}
WrapStyle: 2

[V4+ Styles]
Format: Name, Fontname, Fontsize, PrimaryColour, SecondaryColour, OutlineColour, BackColour, Bold, Italic, Underline, StrikeOut, ScaleX, ScaleY, Spacing, Angle, BorderStyle, Outline, Shadow, Alignment, MarginL, MarginR, MarginV, Encoding
Style: Action,DejaVu Sans Mono,24,&H00F4F2DD,&H00F4F2DD,&H802E3133,&HC01A1408,-1,0,0,0,100,100,0,0,3,1,0,2,70,70,32,1

[Events]
Format: Layer, Start, End, Style, Name, MarginL, MarginR, MarginV, Effect, Text
"""
        + "\n".join(events)
        + "\n",
        encoding="utf-8",
    )


def media_dimensions(path: Path) -> tuple[int, int]:
    completed = subprocess.run(
        [
            require_program("ffprobe"),
            "-v",
            "error",
            "-select_streams",
            "v:0",
            "-show_entries",
            "stream=width,height",
            "-of",
            "json",
            str(path),
        ],
        check=True,
        capture_output=True,
        text=True,
    )
    stream = json.loads(completed.stdout)["streams"][0]
    return int(stream["width"]), int(stream["height"])


def main() -> int:
    parser = argparse.ArgumentParser()
    parser.add_argument("demo", help="recording name under website/public/demos")
    parser.add_argument("output", type=Path, help="destination GIF")
    parser.add_argument(
        "--mp4",
        type=Path,
        default=None,
        help="optional destination MP4 rendered from the same cast and subtitles",
    )
    args = parser.parse_args()

    repo = Path(__file__).resolve().parent.parent
    demos = repo / "website" / "public" / "demos"
    cast = demos / f"{args.demo}.cast"
    actions = demos / f"{args.demo}.actions.json"
    if not cast.is_file() or not actions.is_file():
        raise RuntimeError(f"capture {args.demo!r} before rendering its GIF")
    manifest = json.loads(actions.read_text(encoding="utf-8"))

    require_program("agg")
    require_program("ffmpeg")
    with tempfile.TemporaryDirectory(prefix="oav-demo-gif.") as temporary:
        temporary_path = Path(temporary)
        raw_gif = temporary_path / "terminal.gif"
        subtitles = temporary_path / "actions.ass"
        run(
            [
                "agg",
                "--quiet",
                "--no-loop",
                "--theme",
                "github-dark",
                "--font-size",
                "18",
                "--speed",
                "1",
                "--idle-time-limit",
                "5",
                "--last-frame-duration",
                "3",
                str(cast),
                str(raw_gif),
            ]
        )
        width, height = media_dimensions(raw_gif)
        write_subtitles(subtitles, manifest, width, height)
        args.output.parent.mkdir(parents=True, exist_ok=True)
        if args.mp4 is not None:
            args.mp4.parent.mkdir(parents=True, exist_ok=True)
            video_filter = (
                f"fps=30,ass='{subtitles.as_posix()}',"
                "pad=ceil(iw/2)*2:ceil(ih/2)*2,format=yuv420p"
            )
            run(
                [
                    "ffmpeg",
                    "-hide_banner",
                    "-loglevel",
                    "error",
                    "-y",
                    "-i",
                    str(raw_gif),
                    "-vf",
                    video_filter,
                    "-an",
                    "-c:v",
                    "libx264",
                    "-preset",
                    "slow",
                    "-crf",
                    "20",
                    "-movflags",
                    "+faststart",
                    str(args.mp4),
                ]
            )
            args.mp4.chmod(0o644)
        filter_graph = (
            f"[0:v]fps=15,ass='{subtitles.as_posix()}',split[s0][s1];"
            "[s0]palettegen=max_colors=128:stats_mode=diff[p];"
            "[s1][p]paletteuse=dither=bayer:bayer_scale=3:diff_mode=rectangle"
        )
        run(
            [
                "ffmpeg",
                "-hide_banner",
                "-loglevel",
                "error",
                "-y",
                "-i",
                str(raw_gif),
                "-filter_complex",
                filter_graph,
                "-loop",
                "-1",
                str(args.output),
            ]
        )
    args.output.chmod(0o644)
    print(args.output)
    if args.mp4 is not None:
        print(args.mp4)
    return 0


if __name__ == "__main__":
    raise SystemExit(main())

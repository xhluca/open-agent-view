#!/usr/bin/env python3
"""Prepare README media from the genuine overview terminal recording.

The terminal bytes remain unchanged. Action cues are emitted separately as
ASS subtitles, so the GIF can explain key presses without inventing TUI rows.
"""

from __future__ import annotations

import json
import shutil
from pathlib import Path


def ass_time(seconds: float) -> str:
    centiseconds = max(0, round(seconds * 100))
    hours, remainder = divmod(centiseconds, 360_000)
    minutes, remainder = divmod(remainder, 6_000)
    whole, fraction = divmod(remainder, 100)
    return f"{hours}:{minutes:02d}:{whole:02d}.{fraction:02d}"


def ass_escape(value: str) -> str:
    return value.replace("\\", r"\\").replace("{", r"\{").replace("}", r"\}")


def main() -> int:
    repo = Path(__file__).resolve().parent.parent
    demos = repo / "website" / "public" / "demos"
    source = demos / "overview.cast"
    manifest_path = demos / "overview.actions.json"
    target = repo / "website" / "public" / "oav-demo.cast"
    subtitles = repo / "website" / "public" / "oav-demo.ass"

    if not source.is_file() or not manifest_path.is_file():
        raise RuntimeError("capture the genuine overview recording before composing media")

    shutil.copyfile(source, target)
    target.chmod(0o644)
    manifest = json.loads(manifest_path.read_text(encoding="utf-8"))
    actions = manifest["actions"]
    events = []
    for index, action in enumerate(actions):
        start = float(action["at"])
        next_start = (
            float(actions[index + 1]["at"])
            if index + 1 < len(actions)
            else float(manifest["duration"])
        )
        end = min(next_start, start + 3.0)
        if end - start < 0.35:
            end = min(float(manifest["duration"]), start + 0.8)
        events.append(
            "Dialogue: 0,"
            f"{ass_time(start)},{ass_time(end)},Action,,0,0,0,,"
            f"{ass_escape(str(action['action']))}"
        )

    subtitles.write_text(
        """[Script Info]
ScriptType: v4.00+
PlayResX: 1291
PlayResY: 784
WrapStyle: 2

[V4+ Styles]
Format: Name, Fontname, Fontsize, PrimaryColour, SecondaryColour, OutlineColour, BackColour, Bold, Italic, Underline, StrikeOut, ScaleX, ScaleY, Spacing, Angle, BorderStyle, Outline, Shadow, Alignment, MarginL, MarginR, MarginV, Encoding
Style: Action,DejaVu Sans Mono,22,&H00F4F2DD,&H00F4F2DD,&H802E3133,&HC01A1408,-1,0,0,0,100,100,0,0,3,1,0,2,70,70,42,1

[Events]
Format: Layer, Start, End, Style, Name, MarginL, MarginR, MarginV, Effect, Text
"""
        + "\n".join(events)
        + "\n",
        encoding="utf-8",
    )
    subtitles.chmod(0o644)
    print(target)
    print(subtitles)
    return 0


if __name__ == "__main__":
    raise SystemExit(main())

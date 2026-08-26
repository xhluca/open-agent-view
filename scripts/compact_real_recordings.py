#!/usr/bin/env python3
"""Shorten waits in genuine Asciinema recordings without inventing output.

Provider CLIs repaint spinners while they wait, so Asciinema's idle-time limit
cannot compress those intervals. This tool applies the same piecewise-linear
time map to every captured terminal event and its action manifest. It changes
only timestamps: the bytes and ordering of the real terminal output remain
untouched.
"""

from __future__ import annotations

import argparse
import bisect
import json
from dataclasses import dataclass
from pathlib import Path
from typing import Any


DEFAULT_MAX_GAP = 3.0
HARNESS_STORIES = (
    "claude",
    "codex",
    "pi",
    "opencode",
    "cursor",
    "copilot",
    "antigravity",
    "terminal",
)


@dataclass(frozen=True)
class CompressionInterval:
    """A genuine cast interval whose elapsed time may be shortened."""

    label: str
    start: float
    end: float
    target_duration: float


def _load_cast(path: Path) -> tuple[dict[str, Any], list[list[Any]]]:
    lines = path.read_text(encoding="utf-8").splitlines()
    if len(lines) < 2:
        raise RuntimeError(f"recording is empty: {path}")
    return json.loads(lines[0]), [json.loads(line) for line in lines[1:] if line]


def retime_recording_intervals(
    cast_path: Path,
    actions_path: Path,
    intervals: list[CompressionInterval],
) -> bool:
    """Shorten selected real-output intervals without changing their bytes.

    The cast and its public action timeline use the same continuous time map,
    so seeking, the progress bar, and the action keycap stay synchronized.
    """

    if not intervals:
        return False
    manifest = json.loads(actions_path.read_text(encoding="utf-8"))
    duration = float(manifest["duration"])
    ordered = sorted(intervals, key=lambda item: item.start)
    previous_end = 0.0
    for interval in ordered:
        source_duration = interval.end - interval.start
        if interval.start < previous_end or interval.end > duration + 0.001:
            raise RuntimeError(f"invalid or overlapping timing interval: {interval}")
        if source_duration <= 0 or not 0 < interval.target_duration <= source_duration:
            raise RuntimeError(f"timing interval does not shorten real time: {interval}")
        previous_end = interval.end

    def remap(value: float) -> float:
        saved = 0.0
        for interval in ordered:
            source_duration = interval.end - interval.start
            removed = source_duration - interval.target_duration
            if value >= interval.end:
                saved += removed
                continue
            if value > interval.start:
                progress = (value - interval.start) / source_duration
                return interval.start - saved + progress * interval.target_duration
            break
        return value - saved

    header, events = _load_cast(cast_path)
    for event in events:
        event[0] = round(remap(float(event[0])), 6)
    for action in manifest.get("actions", []):
        action["at"] = round(remap(float(action["at"])), 3)
    manifest["duration"] = round(remap(duration), 6)
    manifest["timing_adjustments"] = [
        {
            "label": interval.label,
            "source_duration": round(interval.end - interval.start, 3),
            "retimed_duration": round(interval.target_duration, 3),
        }
        for interval in ordered
    ]

    cast_path.write_text(
        "\n".join(json.dumps(item, ensure_ascii=False) for item in (header, *events)) + "\n",
        encoding="utf-8",
    )
    actions_path.write_text(json.dumps(manifest, indent=2) + "\n", encoding="utf-8")
    cast_path.chmod(0o644)
    actions_path.chmod(0o644)
    return True


def compact_recording(
    cast_path: Path,
    actions_path: Path,
    max_gap: float = DEFAULT_MAX_GAP,
) -> bool:
    """Cap gaps between labelled actions; return whether files changed."""

    manifest = json.loads(actions_path.read_text(encoding="utf-8"))
    actions = manifest.get("actions", [])
    duration = float(manifest["duration"])
    original_knots = [0.0, *(float(action["at"]) for action in actions), duration]
    if any(right < left for left, right in zip(original_knots, original_knots[1:])):
        raise RuntimeError(f"action timeline is not ordered: {actions_path}")
    # Action labels are stored to the nearest millisecond while cast events use
    # microseconds. Treat that intentional rounding difference as already
    # compact so repeated capture/composition runs are byte-stable.
    if max(
        (right - left for left, right in zip(original_knots, original_knots[1:])),
        default=0,
    ) <= max_gap + 0.001:
        return False

    compact_knots = [0.0]
    for left, right in zip(original_knots, original_knots[1:]):
        compact_knots.append(compact_knots[-1] + min(right - left, max_gap))

    def remap(value: float) -> float:
        index = min(
            bisect.bisect_right(original_knots, value) - 1,
            len(original_knots) - 2,
        )
        index = max(0, index)
        source_start = original_knots[index]
        source_end = original_knots[index + 1]
        target_start = compact_knots[index]
        target_end = compact_knots[index + 1]
        if source_end == source_start:
            return target_end
        progress = (value - source_start) / (source_end - source_start)
        return target_start + progress * (target_end - target_start)

    header, events = _load_cast(cast_path)
    for event in events:
        event[0] = round(remap(float(event[0])), 6)
    for action in actions:
        action["at"] = round(remap(float(action["at"])), 3)
    manifest["duration"] = round(compact_knots[-1], 6)

    cast_path.write_text(
        "\n".join(json.dumps(item, ensure_ascii=False) for item in (header, *events)) + "\n",
        encoding="utf-8",
    )
    actions_path.write_text(json.dumps(manifest, indent=2) + "\n", encoding="utf-8")
    cast_path.chmod(0o644)
    actions_path.chmod(0o644)
    return True


def main() -> int:
    parser = argparse.ArgumentParser()
    parser.add_argument("stories", nargs="*", metavar="STORY")
    parser.add_argument("--max-gap", type=float, default=DEFAULT_MAX_GAP)
    parser.add_argument(
        "--demo-dir",
        type=Path,
        default=Path(__file__).resolve().parent.parent / "website" / "public" / "demos",
    )
    args = parser.parse_args()
    if args.max_gap <= 0:
        parser.error("--max-gap must be positive")
    unknown = sorted(set(args.stories) - set(HARNESS_STORIES))
    if unknown:
        parser.error(
            f"unknown story: {', '.join(unknown)}; choose from {', '.join(HARNESS_STORIES)}"
        )
    stories = args.stories or HARNESS_STORIES
    for story in stories:
        changed = compact_recording(
            args.demo_dir / f"{story}.cast",
            args.demo_dir / f"{story}.actions.json",
            args.max_gap,
        )
        print(f"{story}: {'compacted' if changed else 'already compact'}")
    return 0


if __name__ == "__main__":
    raise SystemExit(main())

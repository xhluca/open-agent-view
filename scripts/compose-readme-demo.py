#!/usr/bin/env python3
"""Compose the README demo from already-recorded genuine terminal sessions.

The inputs are asciicast v2 timelines captured by capture-real-site-demo.py.
This script never invents terminal rows: it only joins the real setup, Claude,
and rename recordings with a terminal clear between them.
"""

from __future__ import annotations

import json
from pathlib import Path


CLIPS = ("setup", "claude", "rename")
TRANSITION_SECONDS = 0.8


def read_cast(path: Path) -> tuple[dict[str, object], list[list[object]]]:
    lines = path.read_text(encoding="utf-8").splitlines()
    if len(lines) < 2:
        raise RuntimeError(f"real demo cast is empty: {path}")
    return json.loads(lines[0]), [json.loads(line) for line in lines[1:]]


def main() -> int:
    repo = Path(__file__).resolve().parent.parent
    demos = repo / "website" / "public" / "demos"
    target = repo / "website" / "public" / "oav-demo.cast"

    header: dict[str, object] | None = None
    composed: list[list[object]] = []
    offset = 0.0
    for index, name in enumerate(CLIPS):
        current_header, events = read_cast(demos / f"{name}.cast")
        if header is None:
            header = current_header
        elif (
            current_header.get("width"),
            current_header.get("height"),
        ) != (header.get("width"), header.get("height")):
            raise RuntimeError(f"{name}.cast has a different terminal size")

        if index:
            composed.append([round(offset, 6), "o", "\x1b[2J\x1b[H"])
            offset += TRANSITION_SECONDS
        for event in events:
            timestamp, kind, payload = event
            composed.append([round(offset + float(timestamp), 6), kind, payload])
        offset = float(composed[-1][0]) + TRANSITION_SECONDS

    assert header is not None
    header["title"] = "Open Agent View — real terminal walkthrough"
    target.write_text(
        "\n".join(json.dumps(item, ensure_ascii=False) for item in (header, *composed))
        + "\n",
        encoding="utf-8",
    )
    target.chmod(0o644)
    print(target)
    return 0


if __name__ == "__main__":
    raise SystemExit(main())

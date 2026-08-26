from __future__ import annotations

import json
import tempfile
import unittest
from pathlib import Path

try:
    from .compact_real_recordings import CompressionInterval, retime_recording_intervals
except ImportError:  # Direct execution keeps the script directory on sys.path.
    from compact_real_recordings import CompressionInterval, retime_recording_intervals


class RetimeRecordingIntervalsTest(unittest.TestCase):
    def test_retimes_cast_and_actions_without_changing_terminal_bytes(self) -> None:
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            cast = root / "demo.cast"
            actions = root / "demo.actions.json"
            records = [
                {"version": 2, "width": 80, "height": 24},
                [0.0, "o", "before"],
                [1.0, "o", "working-0"],
                [2.0, "o", "working-1"],
                [4.0, "o", "answer-a"],
                [6.0, "r", "answer-b"],
            ]
            cast.write_text(
                "\n".join(json.dumps(record) for record in records) + "\n",
                encoding="utf-8",
            )
            actions.write_text(
                json.dumps(
                    {
                        "duration": 6.0,
                        "actions": [
                            {"at": 1.0, "action": "start", "window": "Codex"},
                            {"at": 4.0, "action": "answer", "window": "Codex"},
                            {"at": 6.0, "action": "done", "window": "Codex"},
                        ],
                    }
                ),
                encoding="utf-8",
            )

            changed = retime_recording_intervals(
                cast,
                actions,
                [CompressionInterval("Working", 1.0, 4.0, 1.0)],
            )

            self.assertTrue(changed)
            cast_records = [json.loads(line) for line in cast.read_text().splitlines()]
            events = cast_records[1:]
            self.assertEqual([event[0] for event in events], [0.0, 1.0, 1.333333, 2.0, 4.0])
            self.assertEqual([event[1:] for event in events], [record[1:] for record in records[1:]])
            self.assertEqual(events[-1][0] - events[-2][0], 2.0)

            manifest = json.loads(actions.read_text())
            self.assertEqual(manifest["duration"], 4.0)
            self.assertEqual([action["at"] for action in manifest["actions"]], [1.0, 2.0, 4.0])
            self.assertEqual(
                manifest["timing_adjustments"],
                [{"label": "Working", "source_duration": 3.0, "retimed_duration": 1.0}],
            )

    def test_rejects_overlapping_intervals(self) -> None:
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            cast = root / "demo.cast"
            actions = root / "demo.actions.json"
            cast.write_text(
                '\n'.join(
                    (
                        '{"version":2,"width":80,"height":24}',
                        '[0,"o","a"]',
                        '[5,"o","b"]',
                    )
                )
                + '\n',
                encoding="utf-8",
            )
            actions.write_text('{"duration":5,"actions":[]}', encoding="utf-8")
            with self.assertRaisesRegex(RuntimeError, "overlapping"):
                retime_recording_intervals(
                    cast,
                    actions,
                    [
                        CompressionInterval("one", 1.0, 3.0, 1.0),
                        CompressionInterval("two", 2.0, 4.0, 1.0),
                    ],
                )


if __name__ == "__main__":
    unittest.main()

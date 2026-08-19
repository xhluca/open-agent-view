# Demo asset provenance

`open-agent-view.gif` and a static populated frame, `open-agent-view.png`, were
captured from the real release-mode `coding-agents` binary on 2026-08-18. The
session data comes from the committed deterministic fixture
[`fixtures/all-providers-sessions.json`](../../fixtures/all-providers-sessions.json),
whose seven providers are also exercised through a real PTY test.

The capture is not a mockup and contains no user credentials or real agent
history. It was recorded at 150 columns by 42 rows with:

```console
cargo build --release --locked
asciinema rec --overwrite --quiet --cols 150 --rows 42 \
  --idle-time-limit 1 \
  -c './target/release/coding-agents \
      --fixture fixtures/all-providers-sessions.json \
      --all \
      --include-interactive --refresh-ms 10000' \
  open-agent-view.cast
agg --theme github-dark --font-size 13 --idle-time-limit 1 \
  --last-frame-duration 2 open-agent-view.cast open-agent-view.gif
```

The recorded interaction opens contextual help, moves selection, and exits.
Fixture mode fences all provider I/O, so replaying the capture cannot mutate a
provider session.

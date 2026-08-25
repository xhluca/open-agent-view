# Demo asset provenance

`open-agent-view.gif` and `open-agent-view.png` were captured from the real
v0.1.32 release-mode binary on 2026-08-25. The session data comes from the
committed deterministic
[`fixtures/all-providers-sessions.json`](../../fixtures/all-providers-sessions.json),
which covers all seven coding agents plus OAV's Terminal target.

The capture is not a mockup and contains no user credentials or real agent
history. [`scripts/capture-site-demo.sh`](../../scripts/capture-site-demo.sh)
records the actual binary through a real PTY in a fresh container pinned by
immutable image ID. That container is network-disabled, unprivileged,
read-only, capability-dropped, PID-limited, and given only a tmpfs home plus the
fixture. Fixture mode fences every provider action.

Reproduce all website media with:

```console
scripts/capture-site-demo.sh
```

The interaction waits for asynchronous discovery, navigates to a Codex request,
opens and closes contextual help, switches between status and directory views,
returns to status view, and exits through OAV's own terminal-restoration path.
The script emits an Asciinema cast, GIF, MP4, poster, and caption track, and verifies that
the disposable container is gone afterward.

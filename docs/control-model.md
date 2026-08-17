# Control and ownership model

The dashboard separates **visibility** from **authority**. Finding a session is
not permission to interrupt or delete it.

## Current capability matrix

| Operation | Host Claude | External Codex | Explicit Docker target |
| --- | --- | --- | --- |
| Discover | `claude agents --json` | App Server `thread/list` | Provider protocol through exact container ID |
| Inspect | `claude logs`, reconstructed as a terminal screen | Thread summary; full read pending | Claude logs; Codex summary |
| Open | `claude attach` | `codex resume` | Interactive `docker exec` to the provider CLI |
| Launch | `claude --background` | Disabled until durable supervisor | Disabled for observe-only containers |
| Stop | `claude stop`, owned sessions only | Disabled | Disabled for observe-only containers |
| Inline reply | Not exposed by the supported non-TTY CLI | Requires owning App Server | Disabled |
| Delete | No supported Claude command | Owning App Server only | Disabled |

Opening a session temporarily suspends the dashboard's alternate screen and
runs the provider's native interactive client with inherited terminal I/O.
Returning restores raw mode and refreshes the dashboard.

## Claude ownership registry

`claude --background` returns an eight-character session ID. `coding-agents`
records that prefix with its provider and runtime in:

```text
$XDG_STATE_HOME/open-agent-view/ownership.json
```

or, when `XDG_STATE_HOME` is unset:

```text
~/.local/state/open-agent-view/ownership.json
```

The file is written atomically with user-only permissions on Unix. A discovered
full Claude UUID must match the stored prefix, provider, and runtime before the
Interrupt capability is added. Arbitrary pre-existing sessions remain
observe-only even though the underlying Claude installation may be able to
stop them.

The registry grants provider-session authority only. It never grants authority
to stop or remove a Docker container.

## Codex ownership boundary

The read-only adapter keeps one App Server process alive per configured target
for the lifetime of the dashboard. That avoids spawning a new server on every
refresh and preserves process-local status.

It still does not claim ownership of pre-existing threads. Codex live control
belongs to the App Server process that started or resumed the thread. Durable
launch, steer, interrupt, approval, archive, and delete will be enabled only
after a reconnectable supervisor owns that App Server endpoint.

## Deliberate limitations

- Inline Claude replies are not implemented by scraping private IPC or editing
  transcript files. Press Enter to attach and reply through Claude itself.
- Codex launch is disabled instead of starting a turn that would be terminated
  when the dashboard exits.
- Docker containers supplied with `--docker-container` are observe-only.
- Group deletion is disabled whenever any member lacks Delete authority.
- Prompt and session values are always command arguments, never interpolated
  into a shell string.


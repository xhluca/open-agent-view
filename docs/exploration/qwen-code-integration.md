# Qwen Code integration

This note records the current official CLI contract used by the Qwen Code host
adapter and the places where OAV deliberately does less.

## Official surfaces checked

The implementation was checked against the official
[`QwenLM/qwen-code`](https://github.com/QwenLM/qwen-code) repository at commit
`a6d30ebc6b856587bf66262938a83549101f3694` and package version `0.22.1` on
2026-08-25. A 2026-08-27 authenticated capture also observed newer Qwen JSONL
using fractional millisecond timestamps; OAV accepts integer, fractional, and
numeric-string milliseconds and floors only the sub-millisecond portion.

- `qwen --session-id UUID --prompt-interactive TEXT` starts an exact native
  interactive session; `--model ID` selects an explicit model.
- `qwen --resume UUID` opens an exact saved session.
- `qwen --yolo` automatically accepts native actions.
- `qwen sessions list --json --limit N` emits bounded JSONL history.
- `qwen sessions ps --json` emits the current live session/PID inventory.
- Authentication is handled by `/auth` in the native Qwen UI. The former
  `qwen auth` command is no longer a supported CLI surface.

The current CLI does not expose one stable account-aware, machine-readable
model catalog. OAV therefore does not invent a list. The picker keeps an exact
typed model-ID path, while the provider's native `/model` UI remains the source
for interactive discovery.

## Implemented boundary

OAV allocates a UUIDv4 and passes it through Qwen's documented `--session-id`
option. It persists that exact ID only after the native process successfully
starts/backgrounds or exits successfully. Spawn and immediate non-zero failures
leave no ownership record. The registry contains only ID, workspace, creation
time, and a bounded display name—never transcript text or credentials.

Default discovery intersects the saved and live JSONL inventories with that
private registry. Discovery re-reads that same owner-only registry before each
snapshot, so separately constructed control/discovery handles cannot lose a
just-persisted ownership record. An active record remains `Managed` when its
UUID is in that registry; labeling it `Interactive` caused the ordinary
external-session filter to remove it as soon as another harness launched.
`--include-external` adds read-only history. A verified live record supplies PID
and working state, but stop authority is granted only for the exact native PTY
retained by the current OAV process. Saved sessions open through
`qwen --resume UUID`; provider-side delete, archive, inline reply, and approval
controls are not claimed.

Normal OAV launches preserve Qwen's approval behavior. The explicit top-level
OAV `--yolo` mode adds Qwen's verified `--yolo` option to a fresh managed
launch and marks both the dashboard and native PTY.

## Tests

Deterministic fake transports cover owned/live merging, external opt-in,
malformed JSONL, fractional timestamps, cross-handle ownership refresh, the
managed classification of active owned UUIDs, symlink refusal, exact typed
model selection, and failed-launch ownership rollback. A public-controller
test drives launch, native background, exact reattach, verified interrupt, and
fresh exact resume inside a real outer PTY. Installer tests verify the exact
official standalone URL,
consent gate, configured binary, and native `/auth` handoff in isolated homes.
The networked tier installs the current official CLI in its own credential-free
disposable Docker container and checks `qwen --version`, `qwen --help`, and the
real auth-free saved/live JSONL inventory commands. The official repository was
at package version 0.22.1 when inspected; the official standalone installer
served Qwen Code 0.22.0 in the 2026-08-25 container run.

No test reads, exports, or mounts a host Qwen account.

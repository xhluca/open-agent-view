# Troubleshooting and recovery

Start with read-only evidence. Open Agent View deliberately refuses recovery
that could adopt or signal the wrong provider process or container.

```console
coding-agents --version
coding-agents doctor
coding-agents --json --all
```

Add the provider's `--no-host-*` flag to isolate one host adapter, and
add an exact `--docker-container NAME_OR_ID` only when you intend to inspect
that running container. Do not paste unreviewed provider transcripts, state
files, environment output, or Docker inspection output into an issue; they can
contain private paths, task text, or authentication metadata.

## The dashboard says a TTY is required

The interactive dashboard requires both stdin and stdout to be terminals. Use
`coding-agents --json` in a pipe, CI job, editor task, or redirected command.
For interactive use, run it directly in a terminal or allocate a TTY with the
relevant SSH/container option.

## One provider is unavailable

`doctor` reports missing host provider executables as warnings because a
single-provider or Docker-only setup is valid. Confirm that the same shell can
run the provider's `--version` command or `docker version` as applicable. If
the executable has a nonstandard location, pass its `--*-bin` option
explicitly.

Use a temporary adapter exclusion to regain the dashboard while investigating:

```console
coding-agents --no-host-codex
coding-agents --no-host-claude
coding-agents --no-host-pi
coding-agents --no-host-opencode
```

An explicitly selected Docker container is stricter: it must exist and already
be running. Open Agent View will not start it during discovery. Check it with
ordinary read-only Docker inspection, or remove the `--docker-container`
option.

## A session is visible but a control is unavailable

Visibility is not authority. Pre-existing Claude sessions, external Codex App
Servers, explicit Docker targets, and sessions whose exact ownership record no
longer matches are observe/open-only. Re-running with elevated privileges does
not legitimately add authority and is not a recovery method.

For a Codex provider request, open peek with `space` and read the exact request.
Only one Open Agent View process holds the controller lease. A second dashboard
can observe the request but will not advertise `y`, `n`, or structured-answer
authority. Close the controlling dashboard and refresh the other if you intend
to transfer control. File changes without a correlated diff, expanded
permission requests, MCP forms/URLs, secret questions, expired questions, and
unknown request shapes intentionally remain native-only; press `enter` from an
empty peek to open the native provider UI.

## Codex supervisor cannot reconnect

The managed host Codex supervisor state is under:

```text
$XDG_STATE_HOME/open-agent-view/codex-supervisor/
```

or `~/.local/state/open-agent-view/codex-supervisor/`. `app-server.log` is the
first diagnostic to inspect locally. The supervisor validates the recorded
Linux PID start token, exact command line, socket location, user ownership, and
file modes before reuse.

- A dead or identity-mismatched record is not used to signal a process. The
  next managed startup uses a new unique socket and leaves stale socket files
  alone.
- A verified live process with an unavailable socket is reported instead of
  being replaced. There is currently no `coding-agents` supervisor stop/status
  command.
- Never kill the numeric PID merely because it appears in `supervisor.json`;
  PIDs can be reused. Do not unlink a socket while a verified server may be
  live.

If normal restart does not recover, close all Open Agent View dashboards,
preserve a private copy of the entire `codex-supervisor` directory for
diagnosis, and report the redacted error plus binary/provider versions. Manual
process cleanup is intentionally outside the supported pre-alpha workflow.

## Pi supervisor cannot reconnect

Managed Linux Pi state is under:

```text
$XDG_STATE_HOME/open-agent-view/pi/
```

or `~/.local/state/open-agent-view/pi/`. Inspect `supervisor.log` for daemon
startup/transport errors and `pi-rpc.log` for provider stderr. These files may
contain private paths or provider messages; redact them before sharing.

The saved daemon PID is not authority by itself. Open Agent View requires the
exact Linux start token, command line, private socket location, owner, and file
modes. A dead daemon is ignored and a later managed launch can create a new
one. A verified live daemon whose socket is unavailable is reported and not
replaced. Never kill the numeric PID or unlink the socket solely from the JSON
record.

Only sessions launched through this supervisor can be controlled. A Pi JSONL
session found in the ordinary history store is intentionally inspect/open-only.
A live managed Pi session also refuses native open to prevent two writers; use
peek/reply controls, interrupt it, or wait for it to finish. On macOS, all Pi
sessions use the history/native-open path because durable supervision currently
depends on Linux `/proc` identity.

## Cursor session is missing or refuses control

Cursor does not expose a machine-readable global session list. On Linux, Open
Agent View lists only runs it launched and recorded under:

```text
$XDG_STATE_HOME/open-agent-view/cursor/
```

or `~/.local/state/open-agent-view/cursor/`. A pre-existing Cursor chat is not
missing discovery data: it is outside the supported dashboard surface. Use
Cursor's native TTY picker for that chat.

An owned active turn offers Interrupt after its PID, start token, and command
line are verified. Reply becomes available only after that process exits. Do
not edit `sessions.json`, signal its recorded numeric PID manually, or delete
the bounded logs to force a state change; preserve the directory and report a
redacted error if exact identity verification refuses control.

## Copilot session is visible but read-only

Persisted Copilot rows come from ACP `session/list` on a discovery connection.
They are observe/native-open only and do not inherit control from a previous
dashboard. Launching with `--launch-provider copilot` creates a session on the
current process's retained ACP control connection; only that connection-owned
row can be inspected, prompted, cancelled, or given an exact one-shot
permission choice.

This authority intentionally ends when the dashboard exits and has no OAV
state file to repair. If a managed row loses its ACP connection, preserve the
provider's persisted session and reopen it natively rather than attempting to
reconstruct authority from its session ID.

## Runtime state permissions are refused

The state root and registry directories must be real, current-user-owned
directories with mode `0700`; authority records and locks must be real,
current-user-owned regular files without group/other access. Symlinks are
refused. First inspect the exact path named in the error:

```console
ls -ld -- /absolute/state/directory
ls -l -- /absolute/state/directory/record-name.json
```

Do not recursively change ownership or permissions on a broad directory. If
the exact state directory is yours and was created for Open Agent View, correct
that exact directory/file or select a new empty state root via
`XDG_STATE_HOME`. Retain the old state until you understand which owned
sessions or containers it identifies.

## Managed Docker reports unavailable or mismatched

Use `coding-agents docker list` and
`coding-agents docker status NAME_OR_ID` first. A missing container, modified
ownership label, changed instance label, registry mismatch, or Docker failure
is surfaced as unavailable/refused. The tool does not adopt the container or
silently rewrite its owner record.

Use the same `--managed-docker-registry` value on every related invocation.
If a new isolated lifecycle is acceptable, point the option at a new path and
create a new container; leave the questionable registry untouched for review.
There is no supported stale-record pruning/adoption command yet.

If creation says Docker created a stopped container but saving the owner record
failed, the error identifies the immutable orphan. It has no Open Agent View
lifecycle authority. Inspect that exact full ID and either retain it for
forensics or remove the verified stopped container with ordinary Docker. Never
copy labels or fabricate an owner entry to adopt it.

Removal retains the host workspace and state-home directories. Conversely,
manually removing those directories while a container exists can break the
agent inside it; Open Agent View does not back them up.

## The terminal looks corrupted after exit

Normal exit, provider-native handoff, and errors are designed to restore raw
mode, cursor visibility, and the alternate screen. If the hosting terminal or
process is killed before cleanup, run:

```console
stty sane
reset
```

Then reproduce with the dimensions, terminal name (`printf '%s\n' "$TERM"`),
SSH/tmux context, exact keys, and relevant versions. The
[real-TTY validation guide](tui-validation.md) explains how to capture a
disposable reproduction without provider credentials.

## Upgrade and uninstall behavior

Install a new checkout over the executable only after its locked tests pass.
Do not discard the state directory during an upgrade: it prevents a future
binary from silently adopting unrelated sessions or containers. Uninstalling
the binary intentionally leaves all provider sessions, host bind mounts, and
Open Agent View authority records in place. There is no supported all-state
purge command in the pre-alpha.

For implementation-level safety rationale, see the
[control and ownership model](control-model.md). For a security-sensitive bug,
follow [SECURITY.md](../SECURITY.md).

# Troubleshooting and recovery

Start with read-only evidence. Open Agent View deliberately refuses recovery
that could adopt or signal the wrong provider process or container.

```console
open-agent-view --version
open-agent-view doctor
open-agent-view --json --all
```

If the installed OAV itself is stale, `opav update` (or `opav upgrade`)
downloads and verifies the latest published release.

Add the provider's `--no-host-*` flag to isolate one host adapter, and
add an exact `--docker-container NAME_OR_ID` only when you intend to inspect
that running container. Do not paste unreviewed provider transcripts, state
files, environment output, or Docker inspection output into an issue; they can
contain private paths, task text, or authentication metadata.

## The dashboard says a TTY is required

The interactive dashboard requires both stdin and stdout to be terminals. Use
`open-agent-view --json` in a pipe, CI job, editor task, or redirected command.
For interactive use, run it directly in a terminal or allocate a TTY with the
relevant SSH/container option.

## One provider is unavailable

`doctor` reports missing host provider executables as warnings because a
single-provider or Docker-only setup is valid. Open Agent View searches `PATH`
and conventional user installs such as `~/.npm-global/bin/codex` and
`~/.opencode/bin/opencode`; confirm that the resolved command's `--version`
works. If the executable has another nonstandard location, pass its `--*-bin`
option explicitly.

To install a missing harness without a source checkout or Cargo, use
`open-agent-view setup HARNESS`. OAV names the official source and asks before it
downloads or installs anything; `--yes` is required in non-interactive use.
Restart the dashboard afterward so Tab's harness picker is rebuilt.

Use a temporary adapter exclusion to regain the dashboard while investigating:

```console
open-agent-view --no-host-codex
open-agent-view --no-host-claude
open-agent-view --no-host-pi
open-agent-view --no-host-opencode
```

An explicitly selected Docker container is stricter: it must exist and already
be running. Open Agent View will not start it during discovery. Check it with
ordinary read-only Docker inspection, or remove the `--docker-container`
option.

## Completed sessions or a large dashboard are sluggish

Plain `open-agent-view` shows completed OAV-managed sessions by default. The
roughly 70,000 rows reported by an earlier build were provider-wide OpenCode
history, not OAV-created work. That global store is no longer queried by
default.

Type `/completed hide` or start with `--hide-completed` (`--active-only`) for an
active-only view; `/completed show` restores completed managed sessions. Add
`--include-external` only when provider-wide history is actually wanted. JSON
uses the same independent ownership and lifecycle controls.

Completed, interactive, and cwd filtering is enforced centrally even when a
provider CLI returns rows that violate its own flags. OpenCode's global history
query runs only with both `--include-external` and completed visibility. When
external history is shown, at most 100 persisted rows per provider are loaded by default. A warning
indicates a partial history window; restart with `--history-limit N` to choose a
larger bounded window. Each group still renders only a terminal-sized page capped at 25 rows
behind a selectable **Show more** row; Enter reveals another page. Filtering
searches the bounded discovered set, including rows not yet revealed. Grouping
is cached until the snapshot, filter, or view actually changes, so arrow
movement and ordinary redraws do not rescan the history window.

The default refresh is 15 seconds because repeatedly starting several provider
CLIs can cause substantial CPU and memory churn. Use `ctrl+l` for an immediate
refresh; use `--refresh-ms` only when you have measured a reason to poll more
often. Provider refreshes, model-catalog loads, and managed launches stay off
the input thread. If typing or arrows remain slow on a release containing these
fixes, capture the provider count, `--all`/`--include-external`/`--include-interactive` flags,
terminal/tmux context, and an isolated real-PTY reproduction.

For exact OAV-owned completed Codex threads, preview bounded provider-native
archiving before changing anything:

```console
open-agent-view sessions archive --older-than-days 30 --limit 100
```

Review every candidate, then repeat with `--yes` if the scope is correct.
External Codex, Claude, Pi, OpenCode, Cursor, Copilot, and Antigravity history
does not gain archive authority merely because it is visible; keep it hidden or
use that provider's documented native maintenance interface.

## I did not create a row and cannot delete it

External provider history is excluded by default. If an unexpected row still
appears without `--include-external`, it must correspond to an OAV ownership
record; report the normalized ID and provider without deleting registry files.
If an idle row was shown with `--include-external`, select it and press `ctrl+x`
to remove it reversibly from OAV's view. For an active row without stop
authority, confirm the **local hide** warning; provider history and the live
process are retained.

For a scriptable equivalent, copy the exact normalized ID from Peek or
`open-agent-view --json --include-external --all`:

```console
open-agent-view sessions hide 'PROVIDER:RUNTIME:EXACT_ID'
open-agent-view sessions hidden
open-agent-view sessions unhide 'PROVIDER:RUNTIME:EXACT_ID'
```

Unhiding does not recreate anything; the row returns only when the provider
still reports it. Use provider-native delete/archive only when you actually
intend to mutate provider history. Do not remove a JSONL file or edit an OAV
ownership registry to make an old row disappear.

## Pi says `No session found matching ...`

Pi's default history store is recursive: a session file can live in a
per-workspace child directory, while `pi --session` searches only the exact
`--session-dir` it receives. Version 0.1.8 resolves the UUID to the JSONL file
and passes that file's parent directory. Upgrade and retry. If it persists,
confirm `open-agent-view --json --include-external --all` reports the exact UUID and that the JSONL
file still exists under the configured `--pi-session-dir`; do not move or edit
provider history as a recovery step.

## Antigravity reports a missing workspace

Antigravity's documented cache can retain the last conversation for a deleted
workspace. Version 0.1.8 suppresses those stale entries during discovery and
refuses a stale injected row with `the cached Antigravity workspace no longer
exists` before spawning `agy`. It does not rewrite or delete Antigravity's
cache. Reopen the conversation from an existing workspace through
Antigravity's native interface if needed.

## Left does not return from a provider-native interface

Upgrade to v0.1.18 or newer. Provider clients now run behind an OAV-owned
pseudo-terminal. Plain Left backgrounds only that frontend and returns to the
dashboard; the managed backend remains alive. Enter or Right on the same row
resumes the exact retained frontend and restores its screen. This applies to
Claude, Codex, Pi, OpenCode, Cursor, Copilot, and Antigravity native opens.

Older builds handed the terminal directly to the provider, so Pi/OpenCode Left
did nothing and Claude consumed it for its own agent view. `ctrl+c` in that
mode could terminate the provider frontend. After upgrading, use Left for the
OAV return path. If the dashboard itself exits, OAV cleans up retained frontend
processes but does not target separately supervised provider backends.

## Slash opens the wrong mode or harness/model selection is unclear

`ctrl+f` is the session filter. `/` starts a local dashboard command: `/help`,
`/harness`, `/harness NAME`, `/model`, `/model NAME`, `/model default`,
`/completed show|hide`, or `/filter TEXT`; `/provider` remains an alias. These
commands are never forwarded as task prompts. The composer border always
displays the chosen harness and model.

Press `tab` while composing to open the complete available harness palette;
`/harness` opens it too. Preview with arrows or `tab`, confirm with `enter` or a
number, and use `esc` to return without switching or losing the draft.
For every configured launch-capable harness, press `shift+tab` from the task composer to
load a searchable model picker without losing the current draft. `/model` with
no argument opens the same picker as a command. Type to filter, navigate with
arrows/Tab/Page Up/Page Down, then Enter; Escape preserves the prior model and
draft. `/model NAME` remains the escape hatch for an exact valid custom
identifier that the catalog does not show.

If the picker reports an authentication error, press Enter or `l`. OAV suspends
the dashboard, runs that provider's native login/setup UI, restores the
dashboard, and reloads the exact account catalog. `/login` starts the same
handoff explicitly. OAV never asks you to paste a token into its UI. Pi's setup
is its no-session TUI; choose `/login` there. Antigravity uses its first-run
browser login.

The same picker now opens automatically when a direct Cursor or Copilot launch
fails authentication. The task draft remains intact; Enter no longer acts on
the dashboard row behind the error. An authenticated Cursor account can still
advertise zero models, and an authenticated `gh` account can still lack
Copilot entitlement—those are provider/account results, not credentials OAV
can manufacture.

For deeper diagnosis, run the provider surface directly in the same shell:
`claude --help`, a healthy managed Codex App Server, `pi --offline
--list-models`, `opencode models`, `cursor-agent models`, `copilot login`, or
`agy models`. Pi/OpenCode entries normally use
`provider/model` form. A listed model can still fail later because the account,
credentials, region, or provider configuration does not authorize it.

If Antigravity previously opened and immediately printed `Agent execution
terminated due to error`, inspect only its redacted provider error. On the
reporting account, Antigravity 1.1.17 logged `neither PlanModel nor
RequestedModel specified`. Current OAV refuses to start Antigravity without an
exact model. If `agy models` times out, type a known exact model ID in OAV's
error-state picker and press Enter, or press `l` for Antigravity's native setup.
OAV does not guess a provider model or bypass its sandbox/permissions.

## A newly launched task does not appear

Managed task submission runs in a worker so the composer, arrows, and Escape
remain responsive. When launch completes, Open Agent View refreshes immediately
and selects the exact new provider/session ID. Providers that persist
asynchronously are retried every 250 ms for up to five seconds. If the footer
eventually asks for a manual refresh, press `ctrl+l`, then compare the relevant
provider in `open-agent-view --json --all`.

The current foreground Claude path runs the documented `--background` launch
on a worker, captures Claude's returned eight-character ID, resolves the exact
full UUID from `claude agents --json --all`, records that exact identity, and
immediately opens `claude attach`. It never supplies `--session-id` alongside
`--background`, because current Claude explicitly ignores that combination.
Left backgrounds only
the retained frontend and returns to the exact new row. Background-provider
launches animate independently of the worker, so arrows, typing, and Escape do
not wait on startup.

Version 0.1.14 also handles two fast-completion edge cases: OAV-owned Codex
threads stay visible even when Codex reports their source as `cli`, and a task
that is already completed at the first refresh automatically reveals completed
sessions and selects the exact returned ID. Claude's bounded bootstrap has a
45-second provider-command allowance plus exact inventory reconciliation; it
runs off the input thread, so a cold start animates without freezing arrows or
typing.

If Codex is missing only inside Open Agent View, confirm `open-agent-view doctor`
and `codex --version` succeed in the same shell. A nonstandard executable can be
selected with `--codex-bin /absolute/path/to/codex`. Do the equivalent with
`--pi-bin` or `--opencode-bin`; do not assume a desktop-launched process has the
same `PATH` as an interactive shell.

## The text cursor is one column to the right

Upgrade to a release containing the composer cursor fix. The bottom composer
has top and bottom borders but no left border; older builds incorrectly added a
phantom border column. The current render path places the cursor immediately
after the prompt prefix and typed display cells, including wide Unicode text.
If it remains shifted, record the terminal name, font, tmux/SSH layer, exact
input, and a screenshot with the cursor visible.

## A session is visible but a control is unavailable

Visibility is not broad authority. External Codex App Servers, explicit Docker
targets, and sessions whose exact ownership record no longer matches remain
observe/open-only. Active host Claude background sessions are the narrow
exception: Ctrl+X is offered only with a fresh exact provider inventory check;
interactive, completed, Docker, missing, or changed Claude rows are refused.
Re-running with elevated privileges does not legitimately add authority and is
not a recovery method.

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
  being replaced. There is currently no `open-agent-view` supervisor stop/status
  command.
- Never kill the numeric PID merely because it appears in `supervisor.json`;
  PIDs can be reused. Do not unlink a socket while a verified server may be
  live.

If normal restart does not recover, close all Open Agent View dashboards,
preserve a private copy of the entire `codex-supervisor` directory for
diagnosis, and report the redacted error plus binary/provider versions. Manual
process cleanup is intentionally outside the supported pre-alpha workflow.

### An owned Codex delete takes longer than expected

Codex 0.147 can withhold `thread/delete` responses for a thread still loaded by
its owning App Server. OAV first archives the exact idle thread. It never calls
a timeout “deleted”: success requires the normal response or the exact
`thread/deleted` notification. If the owner wedges and every OAV-owned turn is
idle, OAV can stop the exact listener through a revalidated pidfd, finish the
same ID through an isolated App Server, restart the durable owner, and restore
the remaining ownership records. A private recovery lock makes other dashboard
connections wait across that replacement. This recovery can take tens of
seconds.

OAV refuses that restart while any owned Codex turn is active. Finish or
interrupt the named work and retry. Do not remove the supervisor record or kill
an App Server by a copied PID; doing so discards the evidence used to keep the
recovery scoped.

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

Only sessions launched through this supervisor can be controlled. Their JSONL
files remain OAV-owned/default-visible after the RPC process or supervisor
exits. A Pi JSONL session found only in the ordinary history store is
intentionally inspect/open-only. Enter on a completed managed Pi session closes
its exact idle RPC transport before native resume. Active work or a pending
question still refuses native open; use Peek or Ctrl+X to stop it first. On macOS, all Pi
sessions use the history/native-open path because durable supervision currently
depends on Linux `/proc` identity.

Ctrl+X never waits for Pi's model-facing abort response. It asks the verified
supervisor to close only the selected RPC stdin and returns immediately. After
refresh observes process exit, the row advertises Delete and a second Ctrl+X
removes only the exact JSONL file under OAV's private managed session root.
The exact header ID and canonical root are checked before removal. An older
supervisor without per-session stop can be shut down only when it owns no other
active Pi work; completed histories remain available afterward.

Supervisors started by version 0.1.10 or earlier do not understand modeled
launches. The current client probes the verified daemon's feature list before a
non-default-model launch. If every owned Pi session is completed, it asks that
exact daemon to shut down, verifies its exit, and starts the upgraded daemon. If
any owned work is active, it refuses the upgrade and names up to three active
sessions; finish or interrupt those exact sessions, then retry. It never kills
an older daemon or abandons live Pi work just to apply a model selection.

Version 0.1.14 accepts the common upgrade case where an older verified record
stores `pi` and the current launcher resolves that same file to an absolute
path such as `~/.local/bin/pi`. Both names must resolve to the same canonical
executable. The client still refuses a different file, and it does not restart
or signal the live daemon merely to migrate the spelling of its path.

## OpenCode supervisor cannot reconnect

Managed Linux OpenCode state is under:

```text
$XDG_STATE_HOME/open-agent-view/opencode/
```

or `~/.local/state/open-agent-view/opencode/`. `server.json` contains the exact
process identity, loopback port, authentication secret, and owned session IDs;
`server.log` can contain private paths or provider messages. Do not share either
without careful redaction.

Reconnection requires the saved PID's Linux start token and command line, exact
ownership of the recorded `127.0.0.1` listener, and an authenticated health
response. Never kill the numeric PID, connect to the recorded port without the
managed client, or hand-edit an external history ID into `server.json`. Preserve
the whole private directory when reporting an identity or listener mismatch.

Only sessions created through that authenticated server receive inspect,
reply, and active-work interrupt controls. Existing CLI history remains
inspect/native-open only. Version 0.1.18 opens a managed row by attaching the
native TUI to that exact authenticated loopback server and session; the secret
is child-local environment, never an argument. Left returns to OAV without
stopping the server, and Enter/Right resumes the retained frontend.

Version 0.1.18 also accepts a live record written as bare `opencode` when the
current configured path (for example `~/.opencode/bin/opencode`) and the
verified server's `/proc/PID/exe` resolve to the same canonical file. A
different executable remains a hard refusal. Inline permission or
structured-input handling is not implemented. On non-Linux platforms OpenCode
stays on the history/native-open path.

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

Before `create-chat`, version 0.1.18 runs Cursor's read-only model catalog with
a four-second bound. An account reporting no models gets an immediate
`cursor-agent login` instruction instead of waiting for the old 15-second
create timeout. A successful catalog is still followed by a bounded
`create-chat`; OAV never adds Cursor's `--force`/`--yolo` flags.

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

If launch reports `Authentication required`, upgrade to v0.1.18 and run
`copilot login`. Copilot also documents `gh auth status` and its own GitHub CLI
credential fallback, but an authenticated GitHub account can still lack
Copilot entitlement or organization policy access. OAV leaves authentication
to Copilot and never reads or persists its token. A failed launch now reports
these actions without dumping the ACP response payload.

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

Use `open-agent-view docker list` and
`open-agent-view docker status NAME_OR_ID` first. A missing container, modified
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

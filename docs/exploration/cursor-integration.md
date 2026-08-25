# Cursor CLI integration exploration

Observed on 2026-08-18 with preinstalled `cursor-agent
2026.03.20-44cb435`; a same-day fresh official install returned
`2026.08.11-e8db854`. The probes used empty temporary homes and XDG
directories; they did not read a real Cursor login, chat, workspace, or
configuration.

## Primary sources

- [Cursor CLI overview](https://docs.cursor.com/en/cli/overview)
- [CLI parameters](https://docs.cursor.com/en/cli/reference/parameters)
- [Structured output](https://docs.cursor.com/en/cli/reference/output-format)
- [CLI permissions](https://docs.cursor.com/cli/reference/permissions)
- [Authentication](https://docs.cursor.com/en/cli/reference/authentication)

Cursor's supported installer is:

```console
curl https://cursor.com/install -fsS | bash
```

The installed executable is `cursor-agent` (`agent` is an advertised alias).

## Disposable observations

The current executable advertises these session operations:

```text
cursor-agent create-chat       create an empty chat and print its ID
cursor-agent ls                open the session/cloud-agent TTY picker
cursor-agent resume            resume the latest chat
cursor-agent --resume ID       resume one exact chat
cursor-agent --continue        continue the previous chat
```

`cursor-agent ls` was run in a real 80x24 PTY with an empty isolated home. It
rendered the full-screen “Sessions and Cloud Agents” picker, changed from
“Loading…” to “No sessions or cloud agents found,” accepted Escape, and
restored the alternate screen and cursor. The same command on non-TTY stdin
failed in Ink's raw-mode check. There is no documented `--json` option for
`ls`; it is not a stable discovery interface.

`cursor-agent status` in the empty home printed “Starting login process” and
“Not logged in,” then remained alive until the bounded probe stopped it. A
dashboard must not use `status` as a frequent health probe because it can enter
an authentication flow.

Help/config probes created only isolated cache files and
`.config/cursor/cli-config.json`. No credentials were supplied or inspected.

`cursor-agent login` is the documented native authentication flow;
`cursor-agent models` returns account-visible model IDs and `--model ID`
selects one. On the reporting account the read-only catalog returned `No models
available for this account`, which is why OAV turns that picker state into an
interactive login action instead of waiting for launch to time out.

## Structured managed runs

New-session launch is intentionally foreground-first: OAV calls documented
`create-chat`, persists that exact ID, then runs interactive `--resume ID`
with the selected model and initial prompt. It does not add `--print`, so the
user sees Cursor immediately. At a cursor boundary, repeating the same plain
arrow during the visible return window retains the frontend; Shift+Left/Right
does so immediately. Plain arrows otherwise remain Cursor editor input.

Print mode supports `text`, `json`, and `stream-json`. The documented JSON and
NDJSON events include a stable `session_id`; stream events include:

- `system/init` with `cwd`, `session_id`, and model;
- user and assistant message events;
- correlated `tool_call` started/completed events;
- a terminal `result` with `is_error`, result text, and `session_id`.

This is suitable for a session that Open Agent View explicitly creates and
whose child process/output it owns. It is not a way to attach to or steer an
already-running Cursor process. `--force`/`--yolo` bypass ordinary approvals
and are deliberately absent from the adapter's command builder.

## Capability boundary

| Operation | Verified surface | Open Agent View policy |
| --- | --- | --- |
| Install/version | Official script; `--version` | Supported by doctor/install docs |
| List every chat | TTY-only `ls` picker | Not claimed |
| Login/models/start | `login`, `models`, `--model`, `create-chat` plus owned print mode | Native login, exact account picker, modeled OAV-managed sessions on Linux |
| Resume/open | `--resume ID`, `--workspace PATH` | Native open, shell-free arguments |
| Read transcript | Owned `stream-json` output | Supported only for OAV-managed turns |
| Reply/steer live work | New `--resume ID --print` turn; no live-steer API | Reply only after the owned turn exits |
| Interrupt live work | Owned child process, not a provider session API | Linux pidfd signal only after exact PID/start/cmdline verification |
| Approve/decline | Native interactive prompt; permission config | Leave to native client |
| Delete/archive | Picker UI only/no machine contract found | Not claimed |

The managed adapter allocates a chat, starts a detached safe print-mode turn,
and stores its exact session ID, workspace, PID identity, and bounded log paths
in a mode-`0600` registry below a mode-`0700` state directory. Discovery reads
only that registry. A session gains inspect/reply/interrupt capabilities only
when the persisted workspace and exact owned process identity still match. The
exact selected model is stored with that record and reused for later replies.
Before signalling, Linux opens a pidfd and revalidates the process; a stale or
tampered identity is refused. It never sends a signal based on PID alone.

The managed process contract is currently Linux-only. On macOS the adapter
refuses before `create-chat`, because the same race-safe process-identity and
signal contract has not been implemented there. Native resume remains
available on macOS. The adapter still does not scrape the global Ink picker or
undocumented storage, so external Cursor sessions do not appear automatically.

## Repeatable checks

```console
cursor-agent --version
cursor-agent --help
cursor-agent create-chat --help
cursor-agent ls --help
cargo test --locked --lib adapters::cursor
cargo test --locked --test real_tty cursor_login_reloads_account_models_and_launches_without_freezing_the_tui -- --exact
```

Use a new disposable home and a real PTY for `ls`; never point a compatibility
probe at a user's logged-in Cursor state. Deterministic lifecycle tests use a
disposable mock executable and private state directory. They exercise
create/discover/inspect/interrupt/reply, registry modes, terminal result
reduction, and a tampered process identity that must never receive a signal.
The clean-container installer/version/help smoke test is recorded in the
[fresh-container provider validation](fresh-container-provider-validation.md).

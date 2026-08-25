# Kimi Code integration exploration

Observed on 2026-08-25 with the current native Kimi Code CLI `0.38.0` and the
official repository at commit `6595955b31a6d03fa5ea702141c7e2c0f00ba050`.
The CLI was installed below a disposable directory and probed without login.
No existing Kimi configuration, OAuth token, API key, or session was read or
copied.

## Primary sources

- [Official Kimi Code repository](https://github.com/MoonshotAI/kimi-code)
- [Kimi command reference](https://github.com/MoonshotAI/kimi-code/blob/main/docs/en/reference/kimi-command.md)
- [Provider and model configuration](https://github.com/MoonshotAI/kimi-code/blob/main/docs/en/configuration/providers.md)
- Official installer: `https://code.kimi.com/kimi-code/install.sh`

The current product is distinct from the older Python `kimi-cli`. OAV targets
the current `kimi` executable and `~/.kimi-code` store only.

Verified command contracts are:

```console
kimi                         # native interactive TUI
kimi --session <ID>          # exact native resume
kimi --model <ALIAS>         # select configured model alias
kimi login                   # RFC 8628 device-code login
kimi provider list --json    # configured providers and model aliases
```

`kimi --prompt` is deliberately not used: current help defines it as a
non-interactive auto-permission run, which would replace the user's native TUI
and approval experience.

## Persistence and exact parsing

The configured home is `${KIMI_CODE_HOME:-$HOME/.kimi-code}`. Current sessions
are indexed by append-only `session_index.jsonl` records:

```json
{"sessionId":"session_<uuid>","sessionDir":"/absolute/path","workDir":"/workspace"}
{"sessionId":"session_<uuid>","deleted":true}
```

The last record for an ID wins and the short deletion record is a tombstone.
OAV mirrors the official parser's resilience to a concurrently truncated line,
but it accepts an active row only when:

- the ID matches current `session_[A-Za-z0-9._-]+` syntax;
- `sessionDir` and `workDir` are absolute;
- canonical `sessionDir` is below `$KIMI_CODE_HOME/sessions`; and
- the directory basename equals the session ID.

This prevents a poisoned index from redirecting OAV to an unrelated file.
Per-session `state.json` supplies `title`, `lastPrompt`, `lastTurnReason`, and
timestamps. Current v2 state writes epoch milliseconds; current-v1/legacy
fixtures use RFC 3339 strings. Both forms are tested. A self-describing
absolute `cwd` or `workDir` in state takes precedence over a stale index
workspace. State reads are bounded to 2 MiB.

## Model, login, and foreground launch

Model discovery runs the documented bounded command
`kimi provider list --json` and returns exact keys from the `models` object.
An unauthenticated fresh install validly returns empty provider/model objects.
OAV then offers native `kimi login`; it never reads or transports the resulting
credential.

For a new task, OAV starts a private native Kimi TUI with optional
`--model <alias>`. The current CLI has no interactive initial-prompt flag, so
OAV waits for the official authenticated welcome text
`Send /help for help information.` before writing `PROMPT` and Enter to the
PTY. Logged-out, provider-setup, and trust dialogs do not contain this marker
and therefore never receive task text.

After native return/background, OAV polls for up to five seconds for exactly
one new same-workspace index row. Zero rows time out without claiming
ownership; multiple rows fail closed as ambiguous. Exact open uses
`kimi --session <ID>`. Interrupt exists only for the exact PTY retained by the
current OAV process. External history is hidden by default and remains
inspect-only when explicitly requested.

Kimi has provider-side deletion records, but OAV does not mutate that store
until a supported public delete/archive contract and ownership semantics are
verified.

## Reproduction

```console
KIMI_INSTALL_DIR="$HOME/.local" \
KIMI_CODE_HOME="$HOME/.kimi-code" \
  bash -c 'curl -fsSL https://code.kimi.com/kimi-code/install.sh | bash'
kimi --version
kimi --help
kimi provider list --json

cargo test --locked adapters::kimi::tests
cargo test --locked adapters::native_owned::tests
cargo test --locked --test muse_kimi_native \
  kimi_controller_gates_task_then_discovers_reattaches_interrupts_and_opens_exactly
```

The Rust tests cover current launch/resume argv, readiness-gated prompt bytes,
exact model parsing, tombstones without path fields, owned-only discovery,
RFC 3339 and millisecond timestamps, state workspace precedence, malformed
append records, poisoned/out-of-root paths, private ownership persistence,
delayed provider-state correlation, and ambiguous-candidate refusal. The real
PTY controller test proves the task is withheld from a login screen, delivered
only after the authenticated marker, then covers Shift+Left background, exact
reattach, verified interrupt, and exact provider resume.

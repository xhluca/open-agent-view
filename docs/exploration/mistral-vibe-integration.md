# Mistral Vibe integration

This note records the contract used by the host Mistral Vibe adapter. It is a
versioned implementation record, not a claim that undocumented provider state
is stable.

## Official surfaces checked

The implementation was checked against the official
[`mistralai/mistral-vibe`](https://github.com/mistralai/mistral-vibe) repository
at commit `a84be0391bf93e93a4025a5e08e8032ecb587123` and its official installer at
`https://mistral.ai/vibe/install.sh` on 2026-08-25.

- `vibe [PROMPT]` opens a new native interactive session.
- `vibe --resume SESSION_ID` opens an exact saved session.
- `vibe --setup` owns authentication and first-run configuration.
- `VIBE_ACTIVE_MODEL=ALIAS` selects one configured model for native launch.
- The installed `vibe-app-server` JSON-RPC program exposes `session/list` and
  `config/read`. The latter returns configured model aliases.

OAV sends `initialize`, `initialized`, then one bounded request to a fresh
app-server process. It does not read Vibe credential files or scrape the TUI.

## Implemented boundary

Discovery normalizes the app server's exact session ID, title/preview, status,
timestamps, working directory, and configured model aliases. Default discovery
shows only IDs in OAV's private ownership registry; `--include-external` adds
read-only provider history.

Launch remains in Vibe's native full-screen TUI. Because Vibe allocates its own
session ID, OAV snapshots `session/list` before launch and polls it for at most
five seconds after native return/backgrounding. It records ownership only when
there is exactly one new same-workspace, launch-time candidate. Zero or several
candidates stay unowned. Missing provider working directories are accepted
only when the exact OAV ownership record supplies the verified launch
directory; unowned records with no directory are omitted. If correlation or
ownership persistence fails while the new native frontend is backgrounded,
OAV stops that exact private PTY instead of leaving an unreachable process.

The provider-native frontend can be backgrounded and resumed in the current
dashboard process. Interrupt is granted only while that exact private PTY is
still held. Provider-side delete, archive, inline reply, and approval controls
are not claimed.

## Tests

Deterministic tests cover JSON-RPC state normalization, model aliases,
owned-only discovery, missing-directory fallback, delayed session visibility,
ambiguous post-launch refusal, and a public-controller launch/background/
reattach/interrupt/exact-resume lifecycle inside a real outer PTY. Installer
tests verify the exact official
URL, consent gate, configured binary, and native `--setup` handoff in isolated
homes. The networked tier installs the current official CLI in its own
credential-free disposable Docker container and checks `vibe --version`,
`vibe --help`, and real passive `session/list` plus `config/read` requests to
the installed `vibe-app-server`. The 2026-08-25 run installed `vibe 2.24.3`.

No test reads, exports, or mounts a host Mistral account.

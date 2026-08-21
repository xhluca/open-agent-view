# Main-account and fresh-container regressions (2026-08-21)

This note records the failures reported against v0.1.17, the read-only evidence
collected from the reporting account, and isolated reproductions. It does not
include tokens, provider transcripts, workspace contents, OpenCode's server
password, or complete process environments.

## Host inventory and non-mutating observations

The installed commands resolved to Claude 2.1.237, Codex 0.147.0, Pi 0.84.2,
OpenCode 1.17.20, Cursor Agent `2026.03.20-44cb435`, and Copilot's native
1.0.80 executable. The following observations were made without submitting a
model prompt:

- A live OAV OpenCode record named the executable `opencode`, while the same
  verified process executable was
  `~/.opencode/bin/opencode`. The record's exact PID/start token/cmdline,
  authenticated health response, loopback listener owner, and session ID all
  still matched. Comparing the two display strings caused the launch refusal.
- The exact managed OpenCode session was attached through the authenticated
  loopback server. Left returned to OAV, Enter restored the retained OpenCode
  screen, and Left returned again. The exact server remained live. The row's
  pre-existing local-hidden state was restored after the probe.
- An exact managed Codex thread was opened read-only. Its update prompt began
  at physical row 1 instead of below prior terminal contents. Left returned to
  OAV, Enter restored that exact screen, and Left returned again. Dashboard
  exit removed only the retained Codex frontend; the App Server was not
  targeted.
- `cursor-agent models` returned `No models available for this account.` in
  about 1.4 seconds. The old path skipped that signal and waited up to 15
  seconds in `create-chat`. The new preflight surfaces `cursor-agent login`
  immediately and does not create a chat.
- Copilot ACP `initialize` succeeded but unauthenticated `session/new` returned
  code `-32000`, `Authentication required`. The installed `gh` account was
  checked only through `gh auth status`; no token value was printed or stored.

The OpenCode and Codex native probes did not send input to the model. Provider
history and managed backend processes were compared before and after; the
selected backends remained live and no test session was added.

## Fresh-container Copilot reproduction

Copilot 1.0.80 was mounted read-only into a fresh
`debian:12-slim@sha256:abd67ffcfa541b485a3dff59865ab629aa048a6c613e639d36e7456b0b229241`
container with an anonymous empty home, no network, a read-only root, no Linux
capabilities, and `no-new-privileges`. ACP `initialize` advertised its login
method; the following `session/new` returned the same authentication error as
the report. The container and its anonymous volume were removed by `--rm`.

The probe deliberately did not mount the host keychain, Copilot config, GitHub
CLI config, or a workspace. It proves the failure classification and response
shape, not entitlement for the reporting account. GitHub's current official
documentation says `COPILOT_GITHUB_TOKEN`, `GH_TOKEN`, and `GITHUB_TOKEN` are
the supported non-interactive credentials and that `copilot login` is the
interactive recovery.

## Network-disabled Rust 1.75 regression container

The source was mounted read-only into
`rust:1.75-slim-bookworm@sha256:70c2a016184099262fd7cee46f3d35fec3568c45c62f87e37f7f665f766b1f74`.
The process ran as the source owner with no network, no capabilities,
`no-new-privileges`, a disposable executable build volume, and a read-only
Cargo dependency cache. The following exact tests passed:

- a bare recorded OpenCode name resolves to the same verified running
  executable, but not to a different executable;
- the nested real-PTY frontend backgrounds on Left, restores the dashboard,
  reattaches without spawning a replacement, replays its screen, and
  backgrounds again; and
- Cursor's no-model result refuses before `create-chat` or ownership-state
  creation.

The disposable build volume was removed after the run. No provider credential,
session directory, OAV state directory, or Docker socket was mounted.

## Cross-provider audit

The executable-spelling failure applies only to durable supervisors that must
reconnect to a process started by an older dashboard:

| Provider | Durable executable identity | Result |
| --- | --- | --- |
| Pi | Recorded/current names resolve through recorded/current PATH and HOME, then canonicalize | Already covered; bare `pi` and its exact user-local path match |
| OpenCode | Verified `/proc/PID/exe` compared with a resolved configured executable | Fixed in v0.1.18 |
| Codex | Canonical command/script identity plus exact process/listener ownership | Already covered |
| Cursor | Each owned turn stores exact PID/start token/cmdline; no persistent server executable migration | Not susceptible to this string comparison |
| Copilot | ACP authority is process-local and is not reconstructed after restart | Not susceptible |
| Claude | Background ownership is provider/session based and revalidated through `claude agents --json` | Not susceptible |

All native provider opens now share the same PTY bridge. Plain Left is reserved
for returning to OAV; other bytes, including Up/Down/Right, are forwarded.
Stopping the retained frontend is distinct from interrupting or deleting the
managed provider session.

## Follow-up launch/onboarding regressions

The reporting host was rechecked with Claude 2.1.238, Pi 0.84.2, Cursor Agent
`2026.03.20-44cb435`, Copilot 1.0.80, Antigravity 1.1.17, and OpenCode 1.17.20:

- Claude's real `--bg` output assigns a short ID itself. Combining it with
  `--session-id` produces the reported warning and can make OAV track a UUID
  Claude ignored. The fixed path captures the returned short ID, resolves the
  exact full inventory UUID, and only then records ownership. One disposable
  background contract probe was stopped using its exact returned ID.
- Cursor still reports no models for this account. Direct submission now opens
  the actionable setup/model modal; Enter or `l` invokes `cursor-agent login`
  rather than acting on the row behind the error.
- Copilot's ACP `session/new` returns `Authentication required`. Direct
  submission now preserves the task and routes into native `copilot login`,
  then reloads the headless account catalog.
- Antigravity's model catalog timed out. The provider log for the reported
  failed task ended with `neither PlanModel nor RequestedModel specified`.
  OAV now refuses any model-less Antigravity launch and lets the user type an
  exact model ID when catalog retrieval is unavailable.

No credential directories were mounted into containers. That separation is
intentional: host probes establish the reporting account's observable result;
fresh empty-home containers establish protocol classification and isolation.
Copying a main-account token/keychain into Docker would broaden exposure and is
not required for these regressions.

## Primary provider references

- [OpenCode CLI: attach, session, export, and authentication](https://dev.opencode.ai/docs/cli/)
- [OpenCode server authentication and loopback defaults](https://dev.opencode.ai/docs/server/)
- [Cursor CLI authentication](https://docs.cursor.com/en/cli/reference/authentication)
- [Cursor CLI parameters and session resume](https://docs.cursor.com/en/cli/reference/parameters)
- [GitHub Copilot CLI authentication](https://docs.github.com/en/copilot/how-tos/copilot-cli/set-up-copilot-cli/authenticate-copilot-cli)
- [GitHub Copilot ACP server](https://docs.github.com/en/copilot/reference/copilot-cli-reference/acp-server)
- [Pi CLI/RPC/session reference](https://github.com/badlogic/pi-mono/blob/main/packages/coding-agent/README.md)

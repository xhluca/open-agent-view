# GitHub Copilot CLI integration exploration

Observed on 2026-08-18 with GitHub Copilot CLI `1.0.80` and ACP protocol
version 1. The npm package and ACP server were installed/run under disposable
prefix, cache, and `COPILOT_HOME` directories without tokens or real sessions.

## Primary sources

- [Installing GitHub Copilot CLI](https://docs.github.com/en/copilot/how-tos/copilot-cli/set-up-copilot-cli/install-copilot-cli)
- [Copilot CLI command reference](https://docs.github.com/en/copilot/reference/copilot-cli-reference/cli-command-reference)
- [Official Copilot ACP server](https://docs.github.com/en/copilot/reference/copilot-cli-reference/acp-server)
- [Copilot CLI configuration directory](https://docs.github.com/en/copilot/reference/copilot-cli-reference/cli-config-dir-reference)
- [ACP session list](https://agentclientprotocol.com/protocol/session-list)
- [ACP session setup](https://agentclientprotocol.com/protocol/session-setup)
- [ACP prompt lifecycle](https://agentclientprotocol.com/protocol/prompt-turn)

The official install choices include a checksum-aware install script,
Homebrew, WinGet, and npm. The isolated probe used the documented npm package:

```console
npm install -g @github/copilot
copilot --version
```

The npm registry reported `1.0.80` as current, and the installed executable
reported `GitHub Copilot CLI 1.0.80`.

## Exact ACP observation

The official `copilot --acp --stdio` mode reserves stdout for newline-delimited
JSON-RPC. Open Agent View starts discovery with auto-update, remote export,
built-in MCP, and custom-instruction loading disabled; session listing should
not execute repository integrations.

An isolated server received:

```json
{"jsonrpc":"2.0","id":1,"method":"initialize","params":{"protocolVersion":1,"clientCapabilities":{}}}
{"jsonrpc":"2.0","id":2,"method":"session/list","params":{}}
```

It returned, with no authentication prompt:

```json
{
  "protocolVersion": 1,
  "agentCapabilities": {
    "loadSession": true,
    "sessionCapabilities": {"close": {}, "list": {}}
  },
  "agentInfo": {"name": "Copilot", "version": "1.0.80"}
}
```

The list result was `{"sessions":[]}`. A subsequent isolated `session/new`
returned JSON-RPC error `Authentication required`, proving that discovery and
task creation have distinct authentication effects.

The server did **not** advertise `sessionCapabilities.delete` or
`sessionCapabilities.resume`. Clients must negotiate capabilities instead of
assuming that every method in a newer ACP schema is implemented. This version
uses `session/load` to restore and replay a persisted conversation.

Model discovery uses a separate official headless SDK process rather than the
ACP session process. OAV frames `connect` and `models.list` with LSP
`Content-Length`, retains only exact bounded `models[].id` values, and then
terminates that owned temporary child. It does not create or load a session to
populate the picker. `copilot login` is the native authentication handoff.

## Session model and storage

`session/list` returns paginated `SessionInfo` values:

- required `sessionId` and absolute `cwd`;
- optional title and RFC 3339 `updatedAt`;
- optional opaque `_meta` and `nextCursor`.

It does not return a live lifecycle state. Listed records remain normalized as
`unknown`; the dashboard must not call a persisted record “working” or
“completed” without connection-owned events.

The official configuration reference locates full event history below
`$COPILOT_HOME/session-state/` (normally `~/.copilot/session-state/`) and the
cross-session index in `session-store.db`. Open Agent View uses ACP rather than
reading or mutating these managed files.

## Connection-owned control

One exact ACP process can support:

- `session/new` and `session/load`;
- `session/prompt` plus streamed `session/update` notifications;
- `session/cancel` for an active prompt;
- `session/request_permission` callbacks;
- advertised `session/close`, which releases an active session but does not
  delete persisted history.

Permission requests carry an opaque string/number request ID and a list of
exact options. A response selects one offered `optionId`, or returns
`cancelled`. On cancellation, all pending requests for that session must first
receive `cancelled`. The adapter preserves IDs, rejects duplicate/unknown
options, and never synthesizes broad permission flags.

Authority is connection-owned: session presence in `session/list` grants no
reply, interrupt, or approval capability. A replacement ACP process must load
the session and establish its own active prompt/request state. Open Agent View
does not reconstruct approvals from disk.

The managed controller retains one ACP process for inline-controlled work.
Dashboard task launch itself is native-first: OAV generates an exact UUID and
runs `copilot --session-id ID -C CWD [--model MODEL] --interactive PROMPT` in
the foreground. Sessions created with `session/new`, or explicitly adopted with
`session/load`, are tracked on that connection with streamed transcript,
active prompt ID, pending permission request, and normalized state. It grants
`approve` only when the current request offers `allow_once`, and `decline` only
when it offers `reject_once`; it never maps an "always" option to a one-shot
dashboard action. Requests for unknown sessions are cancelled.

This retained connection is not a background daemon. If Open Agent View exits,
active prompt and permission authority ends with that process. Persisted
history remains discoverable, but is read-only again until a new controller
explicitly loads the exact session. For an idle current-session open, OAV sends
advertised `session/close` when it exists. The reporting account's installed
1.0.80 build advertised only `session/list` under `sessionCapabilities`; in that
valid optional-capability case OAV closes its retained ACP process only when no
connection-owned task is active. It then starts exact native resume and sends
`session/load` after a normal return. Active prompts or permission requests are
refused. If the native frontend is backgrounded, ACP authority remains released
until that exact frontend exits, so two clients never control the session
concurrently.

## Capability boundary

| Operation | Verified surface | Open Agent View policy |
| --- | --- | --- |
| List all persisted sessions | ACP `session/list` with cursor pagination | Supported |
| Login/list models | `copilot login`; headless SDK `models.list` | Native login and exact account picker, without session creation |
| Open externally | `copilot --resume=ID -C PATH` | Native open |
| Inspect/reply | ACP load/prompt | Only after exact connection ownership |
| Modeled launch | Native `--session-id`, `--model`, and `--interactive`; ACP config remains covered for inline clients | Validate the exact account value, reserve the ID, and enter the full native UI |
| Interrupt | ACP `session/cancel` | Only active prompt on owning connection |
| Approval | ACP request/response | Only exact pending request and offered one-shot option |
| Close | Optional ACP `session/close` | Use when advertised; otherwise close only an idle retained ACP process, never active work |
| Delete | Not advertised by 1.0.80 | Unsupported |
| Live state from list | Not present | Never inferred |

## Deterministic and live checks

```console
cargo test --locked --lib adapters::copilot
cargo test --locked --test real_tty copilot_login_reloads_the_exact_account_model_catalog -- --exact
copilot --version
copilot --help
```

The Rust tests use disposable mock ACP executables to cover initialization,
multi-page listing, unknown-state normalization, exact permission option
validation, cancellation response shape, message limits, and clean child
shutdown. A retained-connection lifecycle fixture covers
new/prompt/update/permission/reject/completion/reply/cancel. A separate fixture
proves that a listed external session remains unowned until an explicit
`session/load` succeeds. The live isolated probe covered the real 1.0.80
initialize/list/new authentication boundary. No credentialed model turn was
dispatched.
The same version and initialize/list exchange were reproduced after installing
the npm package in a fresh Node container with no host mounts; see the
[fresh-container provider validation](fresh-container-provider-validation.md).

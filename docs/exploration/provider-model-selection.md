# Provider model-selection contract

This note records the provider surfaces used by Open Agent View's model picker
and managed-session launch path. It is an implementation record, not a promise
that every provider account can use every model it advertises.

## Picker contract

Pressing Shift+Tab from the new-task composer starts catalog discovery on a
worker while preserving the task draft; submitting `/model` with no argument
opens the same picker as a command. Discovery does not block terminal input,
redraw, or provider-session refresh. The popup keeps its search string separate
from the task input buffer, includes **Default**, filters case-insensitively,
shows ten results per page, and supports arrows, Tab/Shift+Tab, Page Up/Page
Down, Enter, and Escape. A late result for a harness that is no longer selected
is ignored.

The catalog path and explicit selector path are deliberately separate:
`/model NAME` accepts a 1–128-byte identifier without whitespace/control
characters even when the provider's list omitted it. This supports custom
models while keeping the picker bounded. The launch provider performs its own
final validation and authorization.

## Claude Code

Claude Code does not expose a separate machine-readable account model catalog
in the CLI surface used here. Open Agent View runs the configured invocation's
`--help` with a five-second timeout and parses the apostrophe-quoted aliases in
the `--model <model>` description. It refuses an unrecognized help shape rather
than inventing a stale hard-coded list. Full model names remain available
through `/model NAME` and are passed to the existing `claude --background
--model NAME` launch path.

Primary reference:

- [Claude Code CLI `--model`](https://docs.anthropic.com/en/docs/claude-code/cli-usage)

## Codex

Codex catalog discovery uses the exact durable App Server that owns managed
launches. Open Agent View requests `model/list` with `includeHidden: false` and
100 entries per page, follows `nextCursor`, deduplicates identifiers, and caps
the traversal at 200 pages / 20,000 models. This makes the picker reflect that
App Server's current account/configuration rather than a compiled OAV list.
The selected identifier is passed to `thread/start`; omitted selection keeps
Codex's normal default resolution.

Primary references:

- [Codex App Server model catalog](https://github.com/openai/codex/blob/main/codex-rs/docs/codex_mcp_interface.md#models)
- [`model/list` parameter schema](https://github.com/openai/codex/blob/main/codex-rs/app-server-protocol/schema/json/v2/ModelListParams.json)

## Pi

Pi 0.84.2 exposes a non-interactive catalog with:

```console
pi --offline --list-models
```

The output is a whitespace-delimited table whose first two columns are
`provider` and `model`. Open Agent View converts each row to
`provider/model`, deduplicates it, and applies byte/entry limits before showing
it. Pi's documentation describes `--list-models` as the available-model list
and `--model` as accepting a model pattern or ID, including `provider/id`.

A selected model is passed to the OAV-owned RPC child at process creation:

```console
pi --mode rpc --no-approve --session-dir DIR --name NAME \
  --model provider/model
```

This preserves Pi's own model resolution and credential checks. OAV does not
probe a model by making an inference request. The durable daemon advertises a
`launch_with_model` protocol feature, so an older daemon cannot silently ignore
a selected model. OAV replaces the exact verified old daemon only when every
session it owns is completed. If any owned work is active, the launch is refused
with an actionable message; the old daemon and its sessions remain untouched.

Primary references:

- [Pi coding-agent CLI](https://github.com/earendil-works/pi/blob/main/packages/coding-agent/README.md)
- [Pi custom-model behavior](https://github.com/earendil-works/pi/blob/main/packages/coding-agent/docs/models.md)
- [Pi RPC model commands](https://github.com/earendil-works/pi/blob/main/packages/coding-agent/docs/rpc.md)

## OpenCode

OpenCode exposes its configured model identifiers with:

```console
opencode models
```

Each non-empty line must be an exact `provider_id/model_id` identifier. OAV
keeps the first slash as the provider/model boundary, so routed model IDs such
as `openrouter/vendor/model` remain intact.

For an OAV-owned server session, the selected identifier is sent in the
documented asynchronous message body:

```json
{
  "model": {
    "providerID": "anthropic",
    "modelID": "claude-sonnet-4-5"
  },
  "parts": [{"type": "text", "text": "..."}]
}
```

The complete body is validated before OAV calls `POST /session`, preventing an
invalid selector from creating an empty managed session. Provider rejection can
still happen asynchronously after the documented `204 No Content`; that is an
OpenCode server behavior rather than proof that the model is usable.

Primary references:

- [OpenCode model identifiers](https://opencode.ai/docs/models/)
- [OpenCode CLI model listing](https://opencode.ai/docs/cli/)
- [OpenCode server session and message endpoints](https://opencode.ai/docs/server/)

## Cursor

Cursor exposes the authenticated account catalog through `cursor-agent models`.
OAV removes terminal progress rendering, accepts only bounded exact identifier
tokens, and treats `No models available for this account` or an authentication
failure as a native-login opportunity. `cursor-agent login` runs with inherited
terminal I/O; on return the picker reloads automatically.

Before allocating a chat, the managed supervisor lists models again and refuses
an unavailable exact selection. The selected ID is added to the owned
`--resume ID --print --output-format stream-json --model MODEL` turn and stored
with the ownership record for future replies. Cursor's TTY-only global session
picker remains outside this model/catalog support.

Primary references:

- [Cursor CLI authentication](https://docs.cursor.com/en/cli/reference/authentication)
- [Cursor CLI parameters](https://docs.cursor.com/en/cli/reference/parameters)

## GitHub Copilot

Copilot's ACP session process exposes model config options, while its official
headless SDK exposes the account catalog. OAV starts a temporary
`copilot --headless --no-auto-update --stdio`, sends LSP-framed `connect` and
`models.list`, keeps exact bounded `models[].id`, and terminates that child. It
does not call `session/new` merely to fill the picker.

For a launch, the retained ACP connection creates the session, verifies the
selected value against the returned model `configOptions`, sends
`session/set_config_option`, waits for success, and only then sends the first
prompt. `copilot login` is the native login handoff; a successful login drops
only an unused/unowned stale ACP connection.

Primary references:

- [Copilot ACP server](https://docs.github.com/en/copilot/reference/copilot-cli-reference/acp-server)
- [Copilot CLI command reference](https://docs.github.com/en/copilot/reference/copilot-cli-reference/cli-command-reference)

## Antigravity

Antigravity exposes account models through `agy models` and selection through
`--model`. The adapter parses bounded exact IDs and uses first-run `agy` as the
native authentication/setup handoff. An OAV launch is foreground and includes
`--sandbox --model MODEL --prompt-interactive PROMPT`; it never adds
`--dangerously-skip-permissions`.

On the reporting account, Antigravity 1.1.19's own catalog timed out in both a
non-TTY process and a fresh PTY, while its native `/model` reported no models.
Because `agy --model` validates against that same catalog, the generic typed-ID
fallback is unsafe here: OAV keeps the wrapped error visible, Enter/`l` opens
the isolated native setup terminal, and Ctrl+R retries. Search text cannot be
submitted as a model.

After the native UI starts, OAV correlates the exact workspace's documented
last-conversation cache value, records that pair as OAV-owned, and uses it as
the refresh/selection hint. This remains a last-conversation-per-workspace
surface, not a complete model/session management protocol.

Primary reference:

- [Antigravity CLI repository](https://github.com/google-antigravity/antigravity-cli)

## Safety and performance boundaries

- Catalog commands have provider-bounded timeouts (four seconds for Cursor,
  eight seconds for Pi/OpenCode, request-bounded App Server/SDK calls, and 20
  seconds for Antigravity's network-backed catalog).
- stdout is rejected above 4 MiB.
- Parsed catalogs are rejected above 20,000 distinct models.
- Model identifiers are bounded to 128 bytes and cannot contain whitespace or
  control characters.
- Catalog discovery is read-only and never starts a provider supervisor.
- Claude discovery is additionally limited to the small `--help` response;
  Codex pagination is capped at 200 pages with hidden entries excluded.
- Tests use isolated fake executables, fake signed-out accounts, real PTYs, and
  authenticated loopback servers; they do not read or mutate a user's provider
  sessions or credentials.

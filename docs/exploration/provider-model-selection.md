# Provider model-selection contract

This note records the provider surfaces used by Open Agent View's model picker
and managed-session launch path. It is an implementation record, not a promise
that every provider account can use every model it advertises.

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

## Safety and performance boundaries

- Catalog commands have an eight-second timeout.
- stdout is rejected above 4 MiB.
- Parsed catalogs are rejected above 20,000 distinct models.
- Model identifiers are bounded to 128 bytes and cannot contain whitespace or
  control characters.
- Catalog discovery is read-only and never starts a provider supervisor.
- Tests use isolated fake executables and authenticated loopback servers; they
  do not read or mutate a user's provider sessions.

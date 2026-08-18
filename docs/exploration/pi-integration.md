# Pi provider exploration

Status: discovery and read-only transcript inspection implemented; durable managed RPC control is not yet integrated.

Explored on 2026-08-18 against:

- `@earendil-works/pi-coding-agent` 0.84.2 (latest npm release at test time)
- an independently installed Pi 0.80.6 for a compatibility probe
- the current official [Pi documentation](https://pi.dev/docs/latest)
- the official [RPC protocol](https://pi.dev/docs/latest/rpc), [session format](https://pi.dev/docs/latest/session-format), and [security model](https://pi.dev/docs/latest/security)

## Product shape

Pi is installed as the `pi` executable. The current package is `@earendil-works/pi-coding-agent`; older installations may still be under the former `@mariozechner` scope. The official installer is:

```bash
curl -fsSL https://pi.dev/install.sh | sh
```

Useful probes:

```bash
pi --version
pi --help
pi --mode rpc --no-session
```

Pi has three relevant interfaces:

1. Its TUI can open, continue, select, or fork persisted sessions.
2. Sessions are documented append-only JSONL files.
3. `--mode rpc` is a strict LF-delimited JSONL protocol over stdin/stdout.

It does **not** expose a CLI command that lists every session as JSON. The public SDK has `SessionManager.listAll()`, but Open Agent View is a Rust binary and should not need to inject a Node helper simply to read a documented file format.

## Persistence and discovery

The default store is:

```text
~/.pi/agent/sessions/--<working-directory>--/<timestamp>_<uuid>.jsonl
```

Precedence for the store location is:

1. `--session-dir`
2. `PI_CODING_AGENT_SESSION_DIR`
3. `sessionDir` in Pi settings
4. `$PI_CODING_AGENT_DIR/sessions`
5. `~/.pi/agent/sessions`

Each file begins with a `type: "session"` header containing the canonical session UUID and working directory. Later records include messages, model changes, compaction, branches, and `session_info` display-name changes.

The adapter:

- recursively reads only regular `*.jsonl` files;
- never follows symlinks;
- caps discovery at 10,000 files;
- uses the header UUID, not the filename, as the provider identity;
- extracts the latest name and text summary;
- reports a terminal assistant record as `completed`;
- reports an incomplete persisted record as `unknown`;
- never infers that a process is alive from a recently modified file;
- never grants reply, interrupt, or delete authority from file discovery.

This is intentionally conservative. A session file is history, not a control channel.

## RPC lifecycle and control

The supported launch shape is:

```bash
pi --mode rpc --session-dir <owned-directory> --name <name>
```

Commands important to a future durable controller include:

- `get_state`: session ID/file/name, streaming state, queue depth, and model;
- `prompt`: start a turn, steer, or enqueue a follow-up;
- `abort`: abort current agent work;
- `get_messages`, `get_entries`, and `get_last_assistant_text`: inspection;
- `set_session_name`: naming;
- `switch_session`: load another session into the same RPC process;
- `extension_ui_response`: answer a matching extension dialog request.

Events include agent/turn/message/tool lifecycle, queue changes, compaction, retry, and extension UI requests.

The key ownership limitation is transport-related: RPC is stdio-only. An arbitrary Pi TUI or RPC process does not expose an attachable socket. Open Agent View can safely control only a process it launched and whose exact stdin/stdout channel it still owns. Durable control across dashboard restarts therefore needs a small OAV-owned proxy/daemon with a private Unix socket and verified PID identity; simply saving a PID is not sufficient.

Pi's project-trust prompt is not a tool sandbox. In RPC mode, unapproved project-local settings/extensions are ignored, while built-in tools still run with the Pi process's operating-system permissions. Managed launch must preserve Pi's default non-trust behavior and should recommend Docker or another OS boundary for unattended work.

## Capability matrix

| Session origin | Discover | Inspect | Open native UI | Reply/steer | Interrupt | Dialog response | Delete |
| --- | --- | --- | --- | --- | --- | --- | --- |
| Existing JSONL history | yes | yes | possible via `pi --session <id>` | no | no | no | no |
| Existing unrelated live Pi | history only | persisted text | possible | no | no | no | no |
| OAV-owned live RPC (planned) | yes | yes | requires handoff design | yes | yes | yes, exact request ID | owned only |

## Isolated real-CLI validation

No real user session directory or credential file was used. Pi 0.84.2 ran with temporary `PI_CODING_AGENT_DIR` and `--session-dir` paths, `PI_OFFLINE=1`, and all extensions, skills, prompt templates, context files, themes, and tools disabled.

Verified:

- exact version output (`0.84.2`);
- RPC startup and strict JSONL responses;
- `set_session_name` event and correlated response;
- `get_state` shape and canonical UUID;
- `bash` streaming update and final response without calling an LLM;
- clean EOF shutdown;
- parsing current v3 JSONL fixtures;
- nested discovery, cwd filtering, missing stores, invalid/incomplete state handling, and symlink exclusion;
- read-only transcript rendering by exact header UUID;
- coexistence through the provider-neutral discovery engine unit suite;
- exact native TUI resume by UUID with a custom session directory and clean terminal restoration;
- combined `coding-agents --json --all` Pi and OpenCode discovery from a third working directory.

Not claimed:

- a model-backed prompt, because the isolation test intentionally supplied no real credentials;
- reconnection to a pre-existing Pi process, because the provider has no such transport;
- safe control of sessions not launched by Open Agent View.

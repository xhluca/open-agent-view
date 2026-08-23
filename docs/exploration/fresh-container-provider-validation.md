# Fresh-container provider validation

This records the 2026-08-18 black-box smoke tests for Cursor, GitHub Copilot
CLI, and Antigravity CLI. Each command used a new `docker run --rm` container,
a tmpfs home/state root, ordinary outbound network access only for the official
installer, and **no host mounts**. No credential, workspace, Docker socket, or
existing provider store entered a container.

The pinned base images were:

```text
debian@sha256:abd67ffcfa541b485a3dff59865ab629aa048a6c613e639d36e7456b0b229241
node@sha256:d649c27dae7ba0137b3cef5dd75baa422c08dc3d9e3fc0c23dfb172dc3cc6436
```

## Cursor

The Debian container installed `ca-certificates`, `curl`, and `bash`, then ran
the documented installer under an empty tmpfs home:

```console
export HOME=/oavtmp/cursor-home
export XDG_CONFIG_HOME=/oavtmp/cursor-config
export XDG_CACHE_HOME=/oavtmp/cursor-cache
export XDG_STATE_HOME=/oavtmp/cursor-state
curl https://cursor.com/install -fsS | bash
"$HOME/.local/bin/cursor-agent" --version
"$HOME/.local/bin/cursor-agent" create-chat --help
```

It returned `2026.08.11-e8db854` and documented `create-chat` as “Create a new
empty chat and return its ID.” The command did not authenticate or create a
real chat. This verifies installation and the allocator contract; owned
lifecycle behavior is covered by the isolated mock/process tests because a
credentialed model action would not be a safe smoke test.

## GitHub Copilot CLI

The Node container installed the exact documented package into tmpfs:

```console
npm_config_prefix=/oavtmp/npm npm install -g @github/copilot@1.0.80
/oavtmp/npm/bin/copilot --version
```

It returned `GitHub Copilot CLI 1.0.80`. The container then sent protocol-v1
`initialize` and `session/list` requests to the real server with discovery
side effects disabled:

```console
/oavtmp/npm/bin/copilot --acp --stdio \
  --no-auto-update --no-remote --no-remote-export \
  --disable-builtin-mcps --no-custom-instructions
```

The server advertised `loadSession`, `sessionCapabilities.close`, and
`sessionCapabilities.list`, then returned `{"sessions":[]}`. It did not
advertise delete. The tmpfs `COPILOT_HOME` was empty before the probe and no
login was attempted.

## Antigravity CLI

The Debian container downloaded the official installer before running it with
an explicit tmpfs destination:

```console
curl -fsSL https://antigravity.google/cli/install.sh -o /oavtmp/install.sh
bash /oavtmp/install.sh --dir /oavtmp/agy
/oavtmp/agy/agy --version
/oavtmp/agy/agy --help
```

It returned `1.1.14` and documented exact-conversation resume, continue,
print/JSON/stream-JSON, and sandbox mode. Help also exposed the dangerous
permission-bypass flag; Open Agent View does not use it. The smoke test stopped
before authentication and left the empty tmpfs home without a `.gemini`
directory.

## Together in the dashboard

Provider-specific protocol tests prove isolation and authority. Coexistence is
covered separately by the canonical seven-agent-plus-Terminal fixture in one real Unix
PTY:

```console
cargo test --locked --test real_tty \
  all_supported_providers_coexist_in_one_real_terminal
```

That test passed on 2026-08-18. It asserts all provider labels, help rendering,
managed Pi reply, managed Cursor interrupt, managed Copilot approval,
alternate-screen entry, and clean terminal restoration. Every action is
rejected at the fixture I/O fence. The test intentionally uses synthetic
sessions: mixing three real unauthenticated CLIs cannot exercise session rows,
while copying user credentials would defeat the isolation goal.

# Fresh-container provider validation

This records the 2026-08-18 and 2026-08-25 black-box smoke tests for the
supported provider installers. Each command used a new `docker run --rm`
container, a tmpfs home/state root, ordinary outbound network access only for
the official installer, and **no host mounts**. No credential, workspace,
Docker socket, or existing provider store entered a container.

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

## Mistral Vibe

The Debian container used OAV's consent-gated wrapper around Mistral's official
installer:

```console
export HOME=/tmp/oav-home
open-agent-view setup mistral-vibe --yes
vibe --version
vibe --help
```

It returned `vibe 2.24.3`. The test also required the installed
`vibe-app-server`, then sent the real `initialize`/`initialized` handshake and
auth-free `session/list` and `config/read` JSON-RPC requests. Their response
shapes included `items` and `config.models`, respectively—the exact passive
surfaces consumed by OAV. Native `vibe --setup` received a real PTY and was
bounded at the interactive account-setup boundary.

## Qwen Code

The Debian container used OAV's consent-gated wrapper around Qwen's official
standalone installer:

```console
export HOME=/tmp/oav-home
export QWEN_NO_MODIFY_PATH=1
open-agent-view setup qwen --yes
qwen --version
qwen --help
qwen sessions list --json --limit 1
qwen sessions ps --json
```

It returned `0.22.0`. Both bounded session inventory commands succeeded in the
credential-free home; an empty JSONL stream correctly represents no saved or
live sessions. The native Qwen UI received a real PTY for `/auth`, but the test
did not provide or copy an account.

## Muse Code

The Debian container used the official bootstrap with its documented isolated
destination controls:

```console
export HOME=/tmp/oav-home
export MUSE_INSTALL_DIR="$HOME/.local/bin"
export MUSE_NO_MODIFY_PATH=1
curl -fsSL https://dev.meta.ai/install.sh | bash
"$MUSE_INSTALL_DIR/muse" --version
"$MUSE_INSTALL_DIR/muse" --help
"$MUSE_INSTALL_DIR/muse" exec --provider echo 'OAV isolated probe'
```

It returned `Muse Code 0.2.1 (0.2.1-R1215.1)`. The credential-free `echo`
provider completed a real session and repeated the probe text. This covers the
official two-stage bootstrap/launcher installation and an auth-free provider
lifecycle without reading or creating user credentials.

## Kimi Code

The Debian container installed the current native CLI into an isolated prefix
and kept both configuration and sessions in tmpfs:

```console
export HOME=/tmp/oav-home
export KIMI_INSTALL_DIR="$HOME/.local"
export KIMI_NO_MODIFY_PATH=1
export KIMI_CODE_HOME="$HOME/.kimi-code"
curl -fsSL https://code.kimi.com/kimi-code/install.sh | bash
"$KIMI_INSTALL_DIR/bin/kimi" --version
"$KIMI_INSTALL_DIR/bin/kimi" --help
"$KIMI_INSTALL_DIR/bin/kimi" provider list --json
```

It returned `0.38.0`; unauthenticated model/provider discovery returned valid
JSON. The setup test also handed the native login command a real PTY, then
stopped at the bounded browser/device-auth boundary without authenticating.

## Together in the dashboard

Provider-specific protocol tests prove isolation and authority. Coexistence is
covered separately by the canonical eleven-agent-plus-Terminal fixture in one
real Unix PTY:

```console
cargo test --locked --test real_tty \
  all_supported_providers_coexist_in_one_real_terminal
```

The expanded test passed on 2026-08-25. It asserts all provider labels, help
rendering, managed controls, alternate-screen entry, and clean terminal
restoration. Every action is rejected at the fixture I/O fence. The test
intentionally uses synthetic rows: unauthenticated CLIs cannot exercise every
provider's session view, while copying user credentials would defeat the
isolation goal. Muse and Kimi additionally have real public-controller PTY
lifecycle tests covering foreground launch, background, exact reattach,
verified interrupt, and exact fresh resume:

```console
cargo test --locked --test muse_kimi_native
```

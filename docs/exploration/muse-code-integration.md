# Muse Code integration exploration

Observed on 2026-08-25 with Muse Code `0.2.1` (`0.2.1-R1215.1`). All
session-format and lifecycle probes used an empty disposable `HOME`,
`XDG_CONFIG_HOME`, and `XDG_DATA_HOME`. The credential-free `echo` provider
was used; no Meta login was attempted and no existing credential file was
read or copied.

## Primary evidence

- [Meta developer site](https://dev.meta.ai/) (the Muse documentation requires
  a Meta login)
- Official bootstrap: `https://dev.meta.ai/install.sh`
- Official launcher payload: `https://api.meta.ai/muse-launcher.sh`
- Official stable channel:
  `https://api.meta.ai/muse-code/channels/muse-stable`

The bootstrap is not interchangeable with the launcher. The bootstrap honors
`MUSE_INSTALL_DIR`, downloads the launcher with response headers and checksum
verification, installs it atomically as `muse`, and optionally updates `PATH`.
The installed launcher in turn resolves and verifies the platform-specific
Muse release from the stable channel. OAV uses the bootstrap for `/setup`.

The following current command contracts were verified from real help output:

```console
muse [OPTIONS] [PROMPT]
muse resume <session-uuid>
muse login
muse --model <MODEL> [PROMPT]
muse exec --provider echo <PROMPT>
```

OAV never adds `--yolo`, `--disable-approval`, or `--disable-sandbox`.

## Persistence and discovery

Muse has no documented machine-readable session-list command in this build.
Its append-only logs use:

```text
${XDG_DATA_HOME:-$HOME/.local/share}/muse/
  sessions/YYYY/MM/DD/<session-uuid>/session.jsonl
  model-catalog/*.json
```

A real `muse exec --provider echo 'OAV isolated probe'` created a session log
whose metadata exposed the absolute workspace at
`payload.record.workspace_root`, the submitted task at
`payload.event.prompt`, and the final assistant message at
`payload.event.text` on an `assistant_message_committed` event. The model cache
contains `rows[].model_id` values.

OAV reads only a bounded prefix for workspace metadata and a bounded 4 MiB
tail for the latest summary. It skips malformed tail records and does not
follow model-catalog symlinks. A provider log is accepted only after its
canonical path is proven to be the exact
`sessions/.../<recorded-id>/session.jsonl` under the configured Muse data root.

## Authority and native foreground behavior

By default OAV shows only sessions it observed being created through its own
foreground native launch. Its private ownership registry stores only the exact
session ID, workspace, display name, creation time, and provider log path. It
does not store transcript bodies or credentials. Registry files reject
symlinks and are written atomically with user-only permissions.

Launch runs `muse [--model ID] PROMPT` in Muse's native TUI. Returning from the
TUI triggers a bounded five-second poll for exactly one new provider log in the
requested workspace. Zero candidates time out without claiming ownership;
multiple candidates are rejected as ambiguous. Open uses
`muse resume <exact-id>` in the original workspace. Interrupt is available
only while OAV still owns that exact background PTY. Provider history is
read-only and opt-in with `--include-external`.

Muse does not expose a verified delete/archive API in the observed build, so
OAV does not pretend to provide one.

## Reproduction

```console
MUSE_INSTALL_DIR="$HOME/.local/bin" \
MUSE_NO_MODIFY_PATH=1 \
  bash -c 'curl -fsSL https://dev.meta.ai/install.sh | bash'
muse --version
muse --help
muse exec --provider echo 'OAV isolated probe'

cargo test --locked adapters::muse::tests
cargo test --locked adapters::native_owned::tests
cargo test --locked --test muse_kimi_native \
  muse_controller_launch_discover_reattach_interrupt_and_exact_open_use_real_ptys
```

The Rust tests cover shell-free argv construction, exact resume IDs, owned-only
discovery, external-history opt-in, malformed log tails, workspace recovery,
model deduplication/limits, private ownership persistence, symlink rejection,
delayed provider-state correlation, and ambiguous-candidate refusal. The real
PTY controller test covers foreground launch, Shift+Left background, exact
reattach, verified interrupt, and exact provider resume; its failure guard
terminates and reaps every fixture process.

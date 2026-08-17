# Docker runtime exploration

Observed on 2026-08-17. This document records a point-in-time local inventory
and proposes the Docker boundary for `open-agent-view`. Image tags, container
IDs, and CLI versions are observations, not compatibility guarantees.

## Safety and method

The four long-running containers in scope were inspected only through Docker
metadata APIs and `docker diff`. No command was executed in them, and none was
paused, restarted, stopped, updated, or removed. Commands that could initialize
provider state were run only in `--rm --network none` containers created from
the already-local images. Credential values and file contents were not read.

The local Docker client and server are both 26.1.1. The active `default`
context points to `unix:///var/run/docker.sock`; the engine uses overlay2 on
Ubuntu 22.04.3 and did not report the `rootless` security option. Access to this
socket is therefore a highly privileged capability and must not be delegated
to an agent session.

## Image inventory

The current `basic-claude-uv:latest` tag resolves to:

| Property | Observed value |
| --- | --- |
| Image ID | `sha256:8f170f660813ac358f347fa8a3580139972f3ea7a9fb087834f1da44669d9392` |
| Created | 2026-07-14 15:29:50 -04:00 |
| Platform | Linux amd64, Debian 12 |
| Configured user | empty, which means root |
| Home at runtime | `/root` |
| Working directory | `/work` |
| Entrypoint | `docker-entrypoint.sh` |
| Default command | `bash` |
| Claude | `/usr/local/bin/claude`, Claude Code 2.1.209 |
| Codex | `/usr/local/bin/codex`, codex-cli 0.144.4 |
| Git | 2.39.5 |

The two `at-codex-*` containers pin an older, now-untagged image even though
their requested image name remains `basic-claude-uv`:

| Property | Observed value |
| --- | --- |
| Image ID | `sha256:7627b24cfb7886ceec85aeea3a48b93c09460824263e9de0d446a04d382340c3` |
| Created | 2026-07-06 16:14:11 -04:00 |
| Claude | Claude Code 2.1.201 |
| Codex | codex-cli 0.142.5 |

Both binaries are global npm-package symlinks. The entrypoint is the standard
Node image shim: it prepends `node` only when the first argument looks like a
Node script or is not an executable command, then uses `exec`. Neither image
contains `tini`, `dumb-init`, or `gosu`. New managed containers should use
Docker's `--init` rather than copying the legacy process model.

An isolated probe using the host numeric UID/GID, `HOME=/home/agent`, and a
writable tmpfs home successfully ran both `claude --version` and
`codex --version`. `/work` remained root-owned and unwritable. This suggests
the tools themselves can run as a numeric non-root user, but a writable home
and workspace must be mounted explicitly. It does not prove that every tool an
agent may call supports a passwd-less numeric user.

## Existing containers

All four containers were running when inspected:

| Container | Short ID | Pinned image ID | Created | Provider state seen |
| --- | --- | --- | --- | --- |
| `webqwen-sbx-1` | `3cd545c242fd` | `8f170f660813` | 2026-08-05 | Claude |
| `webqwen-sbx-2` | `b6570b9f2509` | `8f170f660813` | 2026-08-05 | Claude settings only |
| `at-codex-1` | `ee2dbdec3c16` | `7627b24cfb78` | 2026-07-13 | Codex |
| `at-codex-2` | `351f4909a406` | `7627b24cfb78` | 2026-07-13 | Claude and Codex |

Their runtime configuration is otherwise identical in the dimensions relevant
to this adapter:

- configured user is empty/root and working directory is `/work`;
- entrypoint is `docker-entrypoint.sh`, command is `sleep infinity`;
- stdin and TTY are disabled on the container's primary process;
- network is the user-defined `at-net` network;
- restart policy is `no`;
- there are no bind mounts, named volumes, ports, labels, or health checks;
- they are not privileged, but do not have a read-only root filesystem,
  dropped capabilities, `no-new-privileges`, resource limits, a PID limit, or
  Docker init enabled.

The container environment adds `CLAUDE_CODE_OAUTH_TOKEN` to the base image's
ordinary Node/UV variables. Only the variable name was observed. Its value was
intentionally never retrieved. An exec inherits the container environment, so
the adapter does not need to inspect, copy, log, or re-inject that value.

The absence of mounts is consequential: worktrees, auth, transcripts, SQLite
databases, and provider settings all live in each container's writable layer.
They are not independently durable or conveniently backed up. Removing one of
these containers would remove its only copy of that state.

### Session and auth locations

`docker diff` establishes only that these paths differ from the image; it does
not establish their private schema or whether every record is valid. No file
contents were inspected.

All four containers have `/root/.claude.json`. Claude state also appears under
`/root/.claude`, including `projects`, `sessions`, `session-env`,
`shell-snapshots`, `settings.json`, backups, and plugin data. The observed
Claude project JSONL counts were one in `webqwen-sbx-1`, zero in
`webqwen-sbx-2`, zero in `at-codex-1`, and three in `at-codex-2`.

Both `at-codex-*` containers have `/root/.codex/auth.json` and
`/root/.codex/config.toml`. Their Codex homes also contain `sessions`,
`shell_snapshots`, logs/cache/plugin data, and several SQLite databases such
as `state_5.sqlite`, `goals_1.sqlite`, `logs_2.sqlite`, and
`memories_1.sqlite`; one had active SQLite WAL/SHM files. Unique session JSONL
paths visible in the layer diff numbered 20 for `at-codex-1` and three for
`at-codex-2`.

The `webqwen-sbx-*` containers had no changed `/work` paths. Each
`at-codex-*` container had more than 70 changed `/work` paths, including an
`AGENTS.md` and an `agent-talk` worktree. This confirms that `/work` is
container-local in the current setup, not a host bind mount.

These paths are discovery hints, not adapter APIs. In particular, the adapter
must not read `auth.json`, `.claude.json`, environment values, or live SQLite
files for ordinary discovery.

## Provider surfaces available inside the image

The following was verified in a disposable, network-disabled container using
the current image:

- `claude agents --json` is non-interactive, exits successfully without auth,
  and returned `[]` for a clean home.
- `claude agents --json --all` includes completed sessions, and `--cwd <path>`
  filters sessions. The help also exposes launch defaults such as `--model`,
  `--effort`, `--permission-mode`, `--agent`, and `--add-dir`.
- `claude --bg` launches a background agent and `claude --resume <id>` resumes
  a conversation, according to the installed help.
- Codex 0.144.4 exposes an app server over stdio, Unix sockets, or WebSocket.
  It also exposes `exec --json`, `exec resume`, interactive `resume`, `fork`,
  `archive`, and `delete`.

Running a provider discovery command may still perform cache cleanup or write
bookkeeping files. It is semantically read-only at the session level, not
guaranteed read-only at the filesystem level. It should therefore run only in
containers the user explicitly allows open-agent-view to probe.

## Recommended runtime model

Docker should wrap a provider adapter; it should not become a third provider.
Claude and Codex retain their own session IDs, schemas, state machines, and
control protocols. Docker supplies the execution location and qualifies the
normalized ID, for example:

```text
docker:<full-container-id>:claude:<provider-session-id>
docker:<full-container-id>:codex:<provider-session-id>
```

Use the immutable full container ID for actions. The human-readable name and
requested image reference are display metadata only. In particular, a
container's `Config.Image` may say `basic-claude-uv` while its immutable image
ID refers to an older, untagged build.

### Eligibility and authority

Image name and container-name prefixes are insufficient authorization. The
four observed containers have no labels, and an unrelated container could
legitimately contain either CLI. Support two explicit enrollment mechanisms:

1. a configuration allowlist containing an exact container name or ID; and
2. an opt-in label, `io.open-agent-view.enabled=true`.

Enrollment has independent authority tiers:

| Tier | Permitted behavior |
| --- | --- |
| `metadata` | inspect Docker metadata; do not exec |
| `observe` | execute provider-native list/inspect probes |
| `control` | launch, reply, resume, interrupt, archive, or delete provider sessions |
| `managed` | start, stop, or remove the container itself |

`managed` must never be inferred from `observe` or `control`. A legacy
allowlist entry should default to `observe`; the existing four containers must
remain non-managed. Container stop/remove controls must be unavailable unless
both `io.open-agent-view.managed=true` and a matching open-agent-view instance
ID are present. Provider-level session deletion remains a separate, confirmed
operation.

Suggested labels for newly created containers are:

```text
io.open-agent-view.enabled=true
io.open-agent-view.managed=true
io.open-agent-view.instance=<random UUID>
io.open-agent-view.providers=claude,codex
io.open-agent-view.version=<creating version>
```

The owner record containing the full container ID, instance UUID, chosen state
directory, workspace mapping, and creation time should also be stored outside
the container. Labels alone are user-editable metadata and are not proof of
ownership.

### Discovery algorithm

1. Ask Docker only for explicitly configured targets and containers with the
   opt-in label. Never scan every container by binary, image, or name prefix.
2. Inspect each candidate and retain its full container ID. Report stopped,
   paused, restarting, and unhealthy targets as unavailable; do not start them
   as a side effect of refresh.
3. Resolve the authority tier from configuration plus ownership metadata.
4. For `observe` or stronger targets, probe only configured providers. A
   one-time `command -v`/version probe may mark a configured provider
   unavailable, but finding a binary must not grant authority.
5. Re-inspect immediately before every control action and require the same full
   ID, instance UUID, running state, and authority. This prevents a removed
   container name from being reused between discovery and action.
6. Isolate failures by target/provider, cap stdout/stderr, enforce timeouts, and
   retain unknown versions and states as diagnostics rather than dropping the
   rest of the snapshot.

The refresh loop should use Docker events as an invalidation hint plus a
low-frequency reconciliation poll. Provider session state still requires
provider-native polling or subscriptions.

### Executing provider commands

Construct Docker Engine exec requests as argument arrays, never shell strings.
Set `WorkingDir` to a validated container path and keep JSON/RPC operations
non-TTY so stdout remains machine-readable. Do not use `bash -lc`, interpolate
prompts into a command, or forward arbitrary host environment variables.

For Claude discovery, the inner argv is conceptually:

```text
claude agents --json --all [--cwd <container-path>]
```

For Codex, start `codex app-server --stdio` as an attached exec and speak the
versioned app-server protocol over its stdin/stdout. A single supervised app
server connection per container is preferable to scraping JSONL or SQLite.
Use provider-native turn/session interruption; killing the Docker client or
container is not a reliable substitute and can strand an exec process.

Docker exec inherits the container's user, working defaults, and configured
environment. Preserve those unless the target configuration explicitly
overrides them. Authentication is referenced in place through that inherited
home/environment; diagnostics expose only whether a supported auth mechanism
appears configured, never its value or file contents.

An exec session is not ownership of the container. Refresh and shutdown of the
TUI must close its attached streams but must not stop the container. If an
app-server exec cannot be cleanly terminated through its protocol, record and
clean up that exact exec/process separately rather than sending a signal to
PID 1.

### Working-directory mapping

Existing targets use container paths. `/work` in the four observed containers
has no host-path mapping, so a host cwd cannot be assumed to refer to it. Store
both sides when a mapping exists:

```text
host_path:      /host/project
container_path: /workspace
```

Reject launch paths outside configured workspace roots and canonicalize the
host source before creating a bind mount. Provider calls always receive the
container path. A session discovered with a cwd outside known mappings remains
visible, but host-side “open directory” actions must be disabled.

## Launch contract for new containers

New containers should be recognizably and recoverably owned by
open-agent-view. Recommended defaults:

- pin the selected image digest rather than relying on a mutable tag;
- generate a collision-resistant name and instance UUID;
- apply all ownership labels above and persist the matching host owner record;
- run with `--init`, `--cap-drop=ALL`, `no-new-privileges`, the default seccomp
  profile, a PID limit, and configurable CPU/memory limits;
- never mount the Docker socket;
- run as the invoking user's numeric UID/GID, with an explicitly writable
  `HOME`, state directory, and workspace;
- bind the chosen workspace at a stable container path such as `/workspace`;
- create a user-owned persistent home under open-agent-view's state directory
  and bind it at `/home/agent` rather than relying on the writable layer;
- use `sleep infinity` only as the supervised container lifetime command and
  keep agent processes as exact, tracked execs;
- select networking explicitly; do not silently join `at-net` or inherit a
  legacy container's network.

The current image has no non-root user and its `/work` is not writable by the
host numeric user. A small derived image with an `agent` account is the robust
long-term option. Until then, a numeric user plus a user-owned bind-mounted
home/workspace works for the CLI binaries but should be presented as an image
compatibility mode.

Authentication needs an explicit policy:

- **isolated (default):** provider auth and sessions live in the managed
  container's persistent home; the user authenticates that environment once;
- **inherit existing container:** exec inherits the already-configured home and
  environment without reading secrets;
- **host-shared (opt-in):** bind a provider state directory in place only after
  warning about concurrent access and CLI-version skew.

Do not copy auth files into project directories. Do not place secret values in
labels, command arguments, logs, owner records, diagnostics, or the long-lived
container configuration. Passing a token at container creation makes it
visible to anyone allowed to inspect the Docker daemon, as the current
containers demonstrate. If a secret must be supplied programmatically, use a
dedicated secret broker or an exec-scoped environment and document Docker's
daemon-level visibility.

Container deletion must be a distinct, strongly confirmed action. Stop/remove
only the exact full ID whose instance UUID matches the host owner record, and
retain its persistent home by default. State deletion should require a second
explicit request and enumerate the exact state path before removal.

## Risks and mitigations

| Risk | Required mitigation |
| --- | --- |
| Docker access is effectively root access | Keep Docker operations in the trusted supervisor; never expose the socket to agents. |
| Name reuse or tag drift targets the wrong object | Pin full container and image IDs; re-inspect before action. |
| Prefix/image autodiscovery hijacks unrelated workloads | Require explicit allowlist or opt-in label and configured providers. |
| Writable-layer-only state disappears with a container | Use a user-owned persistent state bind for newly managed containers; never manage legacy containers implicitly. |
| Credential values leak through inspect/logs | Never read or render values; avoid creation-time secret env vars and command arguments. |
| Concurrent Claude JSONL or Codex SQLite access corrupts state | Prefer supported CLI/app-server APIs; do not parse or copy live private stores. |
| Host and container paths are conflated | Maintain an explicit canonical path mapping and disable unsupported host actions. |
| Cancellation leaves an exec process running | Use provider-native interrupt and track Docker exec IDs/processes exactly. |
| Root-owned workspace files | Run managed sessions as the host UID/GID with user-owned mounts. |
| No init or resource limits in the legacy setup | Apply init, capability, PID, CPU, and memory defaults to newly managed containers. |
| Provider upgrades change JSON/RPC behavior | Capture version with each target, parse defensively, and maintain versioned fixtures. |

## Test and diagnostic contract

Unit tests should use a fake Docker client with fixtures for tagged and
untagged images, stopped/paused targets, missing binaries, name reuse, partial
provider failure, timeouts, and malicious labels/paths. Integration tests use
only disposable containers carrying the opt-in and instance labels. They must
assert that refresh never starts/stops a container, non-managed targets cannot
expose lifecycle controls, secrets are redacted, and cancellation does not
affect PID 1.

A safe diagnostic record may include Docker version, full or deliberately
shortened IDs, names, image ID/reference, state, labels in the
`io.open-agent-view.*` namespace, provider versions, authority tier, and
warnings. It must omit environment values, auth/config contents, transcript
contents unless explicitly requested, and arbitrary labels that may themselves
contain secrets.


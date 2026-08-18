# Antigravity CLI integration exploration

Observed on 2026-08-18 with `agy 1.1.14`. Installation, help, and storage
probes used a disposable home and install directory. They did not use a Google
login, keyring, real conversation, or workspace.

## Primary sources

- [Antigravity CLI repository](https://github.com/google-antigravity/antigravity-cli)
- [Official CLI overview](https://antigravity.google/docs/cli/overview)
- [Managing conversations](https://antigravity.google/docs/cli-conversations)
- [Resume command and documented cache](https://antigravity.google/docs/cli/commands/resume)
- [Headless mode](https://antigravity.google/docs/cli/headless)
- [Permissions](https://antigravity.google/docs/cli/permissions)
- [Background tasks and subagents](https://antigravity.google/docs/cli/subagents)

The official Unix installer is:

```console
curl -fsSL https://antigravity.google/cli/install.sh | bash
```

The downloaded installer was inspected before execution. It selected the
`linux_amd64` manifest, downloaded a release archive, verified its SHA-512,
and installed `agy`. On the observation date the manifest reported version
`1.1.14` and SHA-512:

```text
481f590b102ca6847ef13b865f08d457048a1f3f01851ed2a3818eb09a53264b107ca5e442a8677248d9790fd96eccf4918a2aed82d866b23d294422ba42f67e
```

The installed binary reported `1.1.14`.

The binary was also launched with `--sandbox` in a real PTY under a second
empty home. It rendered the welcome screen, reported that it was not signed
in, offered Google OAuth or Google Cloud project login, accepted the documented
double Ctrl+C exit, and restored cursor/bracketed-paste terminal modes. The
probe stopped before choosing a login method.

## Documented command surface

The current executable exposes:

```text
agy --continue / -c             resume the last workspace conversation
agy --conversation ID           resume an exact conversation
agy --print                     run one headless prompt
agy --output-format json        aggregate headless output
agy --output-format stream-json stream headless events
agy --sandbox                   restrict terminal operations
```

The interactive `/resume` picker can search, resume, rename, and delete
conversations. `/agents` shows live subagents, while `/tasks` shows and can
terminate background terminal tasks. Those are TUI panels, not documented
external management protocols.

`--dangerously-skip-permissions` is an explicit authority bypass. Open Agent
View never adds it. Its new-session command builder selects `--sandbox`.

## The only documented discovery cache

Google documents this file:

```text
~/.gemini/antigravity-cli/cache/last_conversations.json
```

Its schema is a JSON object mapping an absolute workspace path to the most
recent conversation ID for that workspace:

```json
{
  "/work/project-a": "a1b2c3d4-e5f6-7890-abcd-ef1234567890",
  "/work/project-b": "f9e8d7c6-b5a4-3210-fedc-ba9876543210"
}
```

The cache does not contain title, transcript, timestamp, process identity,
approval requests, or lifecycle state. The adapter validates absolute paths
and IDs, returns at most the documented last conversation for each workspace,
and normalizes every entry as `unknown` with no inline capabilities.

Antigravity also uses SQLite conversation databases, but their tables and
protobuf payloads are not a documented compatibility contract. Open Agent
View deliberately does not reverse-engineer them. Consequently it cannot yet
claim complete history discovery.

## Capability boundary

| Operation | Verified surface | Open Agent View policy |
| --- | --- | --- |
| Last session/workspace | Documented JSON cache | Supported, read-only |
| List every conversation | Interactive `/resume` picker only | Not claimed |
| Native resume | `agy --conversation ID` from its workspace | Supported, shell-free open |
| Start | Interactive/headless CLI | Sandboxed command building block; no durable owner yet |
| Read transcript | Undocumented SQLite/protobuf | Not claimed |
| Live state/subagents | In-process `/agents` and `/tasks` panels | Not claimed externally |
| Reply/steer/interrupt | No documented external session protocol | Not claimed |
| Approve/decline | Native TUI permission cards | Leave to native client |
| Rename/delete | Native `/resume` picker | Not claimed externally |

The limitation is intentional: displaying one documented cached session is
useful, but labeling it “all Antigravity sessions” would be incorrect.

## Repeatable checks

```console
agy --version
agy --help
cargo test --locked --lib adapters::antigravity
```

For installer validation, download the manifest and script into a disposable
directory, compare the advertised SHA-512, install with `--dir`, and keep the
probe home separate from the user's keyring and Antigravity configuration.

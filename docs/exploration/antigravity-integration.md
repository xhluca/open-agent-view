# Antigravity CLI integration exploration

Observed initially on 2026-08-18 with `agy 1.1.14`. Installation, help, and storage
probes used a disposable home and install directory. They did not use a Google
login, keyring, real conversation, or workspace.

The managed-launch behavior was revalidated on 2026-08-25 with `agy 1.1.20`
using the reporting account's already authenticated CLI. The provider process
used its existing login normally; the probe did not read, print, copy, or
persist Google cookies, OAuth tokens, keyring entries, or session secrets.

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

`agy models`, `--model ID`, `--new-project`, and `--prompt-interactive PROMPT`
are also exposed.
`--dangerously-skip-permissions` is an explicit authority bypass. Open Agent
View never adds it. Its new-session builder selects `--sandbox`, the exact
account-selected model, and an interactive initial prompt.

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

Antigravity also uses internal conversation state that is not a documented
compatibility contract. Open Agent View does not parse its SQLite/protobuf
stores and consequently cannot claim complete external history discovery.

For an OAV-owned launch only, version 1.1.20 creates a bounded JSONL transcript
at `~/.gemini/antigravity-cli/brain/<conversation-id>/.system_generated/logs/`
as soon as the first prompt is recorded. The documented workspace cache is not
updated until the native UI exits. OAV snapshots the conversation-directory
names before `--new-project`, then accepts only a new regular directory whose
bounded transcript contains the exact launch prompt. This supplies the exact
conversation ID while the native UI is still live. The reader is deliberately
bounded to 8 MiB and 20,000 candidate directories and rejects symlinked or
malformed paths. This is a tested local compatibility shim, not a claim that
Google documents an all-conversation API.

## Capability boundary

| Operation | Verified surface | Open Agent View policy |
| --- | --- | --- |
| Last session/workspace | Documented JSON cache | External last-workspace entries only with `--include-external` |
| OAV-launched session | New local brain directory plus exact launch prompt; provisional OAV record while pending | Every exact OAV-owned launch appears immediately and remains resumable/stoppable |
| List every conversation | Interactive `/resume` picker only | Not claimed |
| Native resume | `agy --conversation ID` from its workspace | Supported, shell-free open |
| Models/login/start | `agy models`, first-run `agy`, sandboxed interactive prompt | Native login, private 24-hour catalog cache with bounded last-known-good fallback, exact model picker, OAV ownership record, full-screen launch and native return gesture |
| Read transcript | Bounded local JSONL for an exact OAV-owned ID | Latest summary/time only; no arbitrary external transcript browsing |
| Live state/subagents | In-process `/agents` and `/tasks` panels | Not claimed externally |
| Reply/steer/interrupt | No documented external session protocol | Native UI handles replies; OAV can stop only its exact retained native frontend |
| Approve/decline | Native TUI permission cards | Leave to native client |
| Rename/delete | Native `/resume` picker | Not claimed externally |

The limitation is intentional: displaying one documented cached session is
useful, but labeling it “all Antigravity sessions” would be incorrect.

OAV persists only `(workspace, conversation ID, local task name)` records it
correlates after its own foreground launch. The registry is atomic and
user-private and rejects symlinks or group/other-readable state. A process-local
provisional row makes an immediately backgrounded launch visible before its
exact ID is learned. This proves OAV launch ownership; it does not add approval,
arbitrary process-control, or delete authority.

## Authenticated 1.1.20 regression

The 2026-08-25 probe used an isolated workspace and isolated OAV state/cache,
while allowing the installed `agy` process to use the reporting account's
existing authentication. Verified results:

- `agy models` returned 14 exact account model IDs; the first live request took
  about 2.2 seconds and the resulting OAV cache was mode `0600`;
- a sandboxed `gemini-3.7-flash-high` foreground launch returned the exact
  marker `OAV_MANAGED_AGY_OK`;
- Shift+Left restored OAV and immediately displayed the new Working row with
  provider `Antigravity`, its exact conversation ID, current answer summary,
  and current transcript timestamp;
- Ctrl+X stopped only that retained native frontend and moved the row to
  Completed;
- a fresh OAV process using the isolated state rediscovered the exact completed
  conversation without `--include-external`.

No account credential or browser secret was extracted for these checks.

## Repeatable checks

```console
agy --version
agy --help
cargo test --locked --lib adapters::antigravity
cargo test --locked --test real_tty antigravity_login_model_selection_and_left_background_are_integrated -- --exact
```

For installer validation, download the manifest and script into a disposable
directory, compare the advertised SHA-512, install with `--dir`, and keep the
probe home separate from the user's keyring and Antigravity configuration.
The official install/version/help path was also repeated in a fresh Debian
container with no host mounts; see the
[fresh-container provider validation](fresh-container-provider-validation.md).
